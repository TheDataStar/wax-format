//! A read-only SQLite VFS that exposes a byte range of the archive as a
//! database file (Track A §18, A1b).
//!
//! Index segments are SQLite databases embedded at an offset inside the
//! `.wax` file (SPEC §4). Reading one used to mean copying its bytes into a
//! temp file so SQLite could open it — O(index bytes) of I/O and scratch per
//! open. This VFS instead wraps the platform default VFS and shifts every read
//! by the segment's offset, so SQLite pages the segment straight out of the
//! archive on demand and nothing is copied.
//!
//! Usage is through a URI: `file:<archive>?vfs=wax&off=<o>&len=<l>&immutable=1`.
//! `off`/`len` are only honoured for the main database file; journals and
//! temp files (SQLite never opens either for a read-only immutable database,
//! but the fallback is still correct) pass straight through to the default VFS.
//!
//! Everything here is `unsafe extern "C"` glue and must never unwind: no
//! `unwrap`, no allocation that can fail unchecked, every failure is an SQLite
//! result code.

use rusqlite::ffi;
use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::ffi::{c_char, c_int, c_void, CStr};
use std::mem::{size_of, MaybeUninit};
use std::path::Path;
use std::ptr::{self, null_mut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;

/// The registered VFS name (`?vfs=wax`).
pub const VFS_NAME: &CStr = c"wax";

/// A file opened through the wax VFS: the default VFS's own file object plus
/// the window we expose of it. `len < 0` means "not windowed" (pass-through).
#[repr(C)]
struct WaxFile {
    base: ffi::sqlite3_file,
    inner: *mut ffi::sqlite3_file,
    inner_layout: Option<Layout>,
    off: i64,
    len: i64,
}

static REGISTER: Once = Once::new();
static READ_CALLS: AtomicU64 = AtomicU64::new(0);
static READ_BYTES: AtomicU64 = AtomicU64::new(0);

/// `(calls, bytes)` of windowed reads SQLite has issued through this VFS
/// since process start. A diagnostic for "how many index page reads did that
/// cost" — the number that predicts latency on SD-card-class storage, where
/// each call is a random read.
pub fn read_stats() -> (u64, u64) {
    (READ_CALLS.load(Ordering::Relaxed), READ_BYTES.load(Ordering::Relaxed))
}

static mut VFS: MaybeUninit<ffi::sqlite3_vfs> = MaybeUninit::uninit();

/// Register the VFS with SQLite (once per process). Returns `false` only if
/// SQLite has no default VFS, which cannot happen on a supported platform.
pub fn ensure_registered() -> bool {
    REGISTER.call_once(|| unsafe {
        // sqlite3_vfs_find initializes the library itself; being explicit
        // costs nothing.
        ffi::sqlite3_initialize();
        let dflt = ffi::sqlite3_vfs_find(ptr::null());
        if dflt.is_null() {
            return;
        }
        let vfs = ffi::sqlite3_vfs {
            iVersion: 1,
            szOsFile: size_of::<WaxFile>() as c_int,
            mxPathname: (*dflt).mxPathname,
            pNext: null_mut(),
            zName: VFS_NAME.as_ptr(),
            pAppData: dflt as *mut c_void,
            xOpen: Some(x_open),
            xDelete: Some(x_delete),
            xAccess: Some(x_access),
            xFullPathname: Some(x_full_pathname),
            xDlOpen: None,
            xDlError: None,
            xDlSym: None,
            xDlClose: None,
            xRandomness: Some(x_randomness),
            xSleep: Some(x_sleep),
            xCurrentTime: Some(x_current_time),
            xGetLastError: Some(x_get_last_error),
            xCurrentTimeInt64: None,
            xSetSystemCall: None,
            xGetSystemCall: None,
            xNextSystemCall: None,
        };
        let slot = ptr::addr_of_mut!(VFS) as *mut ffi::sqlite3_vfs;
        ptr::write(slot, vfs);
        ffi::sqlite3_vfs_register(slot, 0);
    });
    unsafe { !ffi::sqlite3_vfs_find(VFS_NAME.as_ptr()).is_null() }
}

/// The `file:` URI that opens bytes `[off, off+len)` of `archive` as a
/// read-only, immutable SQLite database through this VFS. `None` if the path
/// is not valid UTF-8 (SQLite URIs are text).
pub fn segment_uri(archive: &Path, off: u64, len: u64) -> Option<String> {
    let text = archive.to_str()?;
    let mut uri = String::with_capacity(text.len() + 64);
    uri.push_str("file:");
    for b in text.bytes() {
        match b {
            b'\\' => uri.push('/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(b as char)
            }
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri.push_str(&format!("?vfs=wax&off={off}&len={len}&immutable=1&nolock=1&mode=ro"));
    Some(uri)
}

// --- helpers ------------------------------------------------------------------

unsafe fn default_vfs(vfs: *mut ffi::sqlite3_vfs) -> *mut ffi::sqlite3_vfs {
    (*vfs).pAppData as *mut ffi::sqlite3_vfs
}

/// Call a method on the wrapped file, or return `SQLITE_IOERR` if the default
/// VFS did not provide it.
macro_rules! inner {
    ($f:expr, $method:ident $(, $arg:expr)*) => {{
        let inner = (*$f).inner;
        if inner.is_null() || (*inner).pMethods.is_null() {
            ffi::SQLITE_IOERR
        } else {
            match (*(*inner).pMethods).$method {
                Some(m) => m(inner $(, $arg)*),
                None => ffi::SQLITE_IOERR,
            }
        }
    }};
}

// --- VFS methods --------------------------------------------------------------

unsafe extern "C" fn x_open(
    vfs: *mut ffi::sqlite3_vfs,
    name: ffi::sqlite3_filename,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out_flags: *mut c_int,
) -> c_int {
    let f = file as *mut WaxFile;
    (*f).base.pMethods = ptr::null();
    (*f).inner = null_mut();
    (*f).inner_layout = None;
    (*f).off = 0;
    (*f).len = -1;

    let dflt = default_vfs(vfs);
    let Some(open) = (*dflt).xOpen else {
        return ffi::SQLITE_CANTOPEN;
    };
    let size = ((*dflt).szOsFile.max(1)) as usize;
    let Ok(layout) = Layout::from_size_align(size, 16) else {
        return ffi::SQLITE_NOMEM;
    };
    let inner = alloc_zeroed(layout) as *mut ffi::sqlite3_file;
    if inner.is_null() {
        return ffi::SQLITE_NOMEM;
    }

    let rc = open(dflt, name, inner, flags, out_flags);
    if rc != ffi::SQLITE_OK {
        if !(*inner).pMethods.is_null() {
            if let Some(close) = (*(*inner).pMethods).xClose {
                close(inner);
            }
        }
        dealloc(inner as *mut u8, layout);
        return rc;
    }

    if flags & ffi::SQLITE_OPEN_MAIN_DB != 0 && !name.is_null() {
        (*f).off = ffi::sqlite3_uri_int64(name, c"off".as_ptr(), 0).max(0);
        (*f).len = ffi::sqlite3_uri_int64(name, c"len".as_ptr(), -1);
    }
    (*f).inner = inner;
    (*f).inner_layout = Some(layout);
    (*f).base.pMethods = &IO_METHODS;
    ffi::SQLITE_OK
}

unsafe extern "C" fn x_delete(vfs: *mut ffi::sqlite3_vfs, name: *const c_char, sync_dir: c_int) -> c_int {
    let d = default_vfs(vfs);
    match (*d).xDelete {
        Some(m) => m(d, name, sync_dir),
        None => ffi::SQLITE_IOERR_DELETE,
    }
}

unsafe extern "C" fn x_access(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    flags: c_int,
    out: *mut c_int,
) -> c_int {
    let d = default_vfs(vfs);
    match (*d).xAccess {
        Some(m) => m(d, name, flags, out),
        None => ffi::SQLITE_IOERR_ACCESS,
    }
}

unsafe extern "C" fn x_full_pathname(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    n_out: c_int,
    out: *mut c_char,
) -> c_int {
    let d = default_vfs(vfs);
    match (*d).xFullPathname {
        Some(m) => m(d, name, n_out, out),
        None => ffi::SQLITE_CANTOPEN,
    }
}

unsafe extern "C" fn x_randomness(vfs: *mut ffi::sqlite3_vfs, n: c_int, out: *mut c_char) -> c_int {
    let d = default_vfs(vfs);
    match (*d).xRandomness {
        Some(m) => m(d, n, out),
        None => 0,
    }
}

unsafe extern "C" fn x_sleep(vfs: *mut ffi::sqlite3_vfs, micros: c_int) -> c_int {
    let d = default_vfs(vfs);
    match (*d).xSleep {
        Some(m) => m(d, micros),
        None => 0,
    }
}

unsafe extern "C" fn x_current_time(vfs: *mut ffi::sqlite3_vfs, out: *mut f64) -> c_int {
    let d = default_vfs(vfs);
    match (*d).xCurrentTime {
        Some(m) => m(d, out),
        None => ffi::SQLITE_ERROR,
    }
}

unsafe extern "C" fn x_get_last_error(vfs: *mut ffi::sqlite3_vfs, n: c_int, out: *mut c_char) -> c_int {
    let d = default_vfs(vfs);
    match (*d).xGetLastError {
        Some(m) => m(d, n, out),
        None => 0,
    }
}

// --- file methods -------------------------------------------------------------

static IO_METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(f_close),
    xRead: Some(f_read),
    xWrite: Some(f_write),
    xTruncate: Some(f_truncate),
    xSync: Some(f_sync),
    xFileSize: Some(f_file_size),
    xLock: Some(f_lock),
    xUnlock: Some(f_unlock),
    xCheckReservedLock: Some(f_check_reserved_lock),
    xFileControl: Some(f_file_control),
    xSectorSize: Some(f_sector_size),
    xDeviceCharacteristics: Some(f_device_characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};

unsafe fn windowed(f: *mut WaxFile) -> bool {
    (*f).len >= 0
}

unsafe extern "C" fn f_close(file: *mut ffi::sqlite3_file) -> c_int {
    let f = file as *mut WaxFile;
    let mut rc = ffi::SQLITE_OK;
    let inner = (*f).inner;
    if !inner.is_null() {
        if !(*inner).pMethods.is_null() {
            if let Some(close) = (*(*inner).pMethods).xClose {
                rc = close(inner);
            }
        }
        if let Some(layout) = (*f).inner_layout.take() {
            dealloc(inner as *mut u8, layout);
        }
        (*f).inner = null_mut();
    }
    (*f).base.pMethods = ptr::null();
    rc
}

unsafe extern "C" fn f_read(file: *mut ffi::sqlite3_file, buf: *mut c_void, amt: c_int, ofst: i64) -> c_int {
    let f = file as *mut WaxFile;
    if !windowed(f) {
        return inner!(f, xRead, buf, amt, ofst);
    }
    if amt < 0 || ofst < 0 {
        return ffi::SQLITE_IOERR_READ;
    }
    let len = (*f).len;
    // SQLite requires the unread tail of a short read to be zero-filled.
    let avail = (len - ofst.min(len)).min(amt as i64) as c_int;
    if avail < amt {
        ptr::write_bytes((buf as *mut u8).add(avail as usize), 0, (amt - avail) as usize);
    }
    if avail == 0 {
        return ffi::SQLITE_IOERR_SHORT_READ;
    }
    let Some(abs) = (*f).off.checked_add(ofst) else {
        return ffi::SQLITE_IOERR_READ;
    };
    READ_CALLS.fetch_add(1, Ordering::Relaxed);
    READ_BYTES.fetch_add(avail as u64, Ordering::Relaxed);
    let rc = inner!(f, xRead, buf, avail, abs);
    if rc == ffi::SQLITE_OK && avail < amt {
        ffi::SQLITE_IOERR_SHORT_READ
    } else {
        rc
    }
}

unsafe extern "C" fn f_write(file: *mut ffi::sqlite3_file, buf: *const c_void, amt: c_int, ofst: i64) -> c_int {
    let f = file as *mut WaxFile;
    if windowed(f) {
        return ffi::SQLITE_READONLY;
    }
    inner!(f, xWrite, buf, amt, ofst)
}

unsafe extern "C" fn f_truncate(file: *mut ffi::sqlite3_file, size: i64) -> c_int {
    let f = file as *mut WaxFile;
    if windowed(f) {
        return ffi::SQLITE_READONLY;
    }
    inner!(f, xTruncate, size)
}

unsafe extern "C" fn f_sync(file: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    let f = file as *mut WaxFile;
    if windowed(f) {
        return ffi::SQLITE_OK;
    }
    inner!(f, xSync, flags)
}

unsafe extern "C" fn f_file_size(file: *mut ffi::sqlite3_file, out: *mut i64) -> c_int {
    let f = file as *mut WaxFile;
    if windowed(f) {
        *out = (*f).len;
        return ffi::SQLITE_OK;
    }
    inner!(f, xFileSize, out)
}

unsafe extern "C" fn f_lock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    let f = file as *mut WaxFile;
    if windowed(f) {
        return ffi::SQLITE_OK;
    }
    inner!(f, xLock, level)
}

unsafe extern "C" fn f_unlock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    let f = file as *mut WaxFile;
    if windowed(f) {
        return ffi::SQLITE_OK;
    }
    inner!(f, xUnlock, level)
}

unsafe extern "C" fn f_check_reserved_lock(file: *mut ffi::sqlite3_file, out: *mut c_int) -> c_int {
    let f = file as *mut WaxFile;
    if windowed(f) {
        *out = 0;
        return ffi::SQLITE_OK;
    }
    inner!(f, xCheckReservedLock, out)
}

unsafe extern "C" fn f_file_control(file: *mut ffi::sqlite3_file, op: c_int, arg: *mut c_void) -> c_int {
    let f = file as *mut WaxFile;
    if windowed(f) {
        return ffi::SQLITE_NOTFOUND;
    }
    inner!(f, xFileControl, op, arg)
}

unsafe extern "C" fn f_sector_size(file: *mut ffi::sqlite3_file) -> c_int {
    let f = file as *mut WaxFile;
    let inner = (*f).inner;
    if inner.is_null() || (*inner).pMethods.is_null() {
        return 4096;
    }
    match (*(*inner).pMethods).xSectorSize {
        Some(m) => m(inner),
        None => 4096,
    }
}

unsafe extern "C" fn f_device_characteristics(file: *mut ffi::sqlite3_file) -> c_int {
    let f = file as *mut WaxFile;
    let inner = (*f).inner;
    let base = if inner.is_null() || (*inner).pMethods.is_null() {
        0
    } else {
        match (*(*inner).pMethods).xDeviceCharacteristics {
            Some(m) => m(inner),
            None => 0,
        }
    };
    if windowed(f) {
        base | ffi::SQLITE_IOCAP_IMMUTABLE
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, OpenFlags};
    use std::io::Write;

    fn open(uri: &str) -> rusqlite::Result<Connection> {
        assert!(ensure_registered());
        Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
    }

    /// A database embedded after a prefix and before a suffix is readable
    /// through the window, writes are refused, and the archive is untouched.
    #[test]
    fn reads_a_database_embedded_at_an_offset() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("plain.db");
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(
                "PRAGMA page_size=4096; PRAGMA journal_mode=OFF;
                 CREATE TABLE t(k TEXT PRIMARY KEY, v INTEGER);
                 INSERT INTO t VALUES('a',1),('b',2),('c',3); VACUUM;",
            )
            .unwrap();
        }
        let bytes = std::fs::read(&db).unwrap();
        let prefix = vec![0xEEu8; 1234];
        let archive = dir.path().join("embedded with space & odd%.wax");
        let mut f = std::fs::File::create(&archive).unwrap();
        f.write_all(&prefix).unwrap();
        f.write_all(&bytes).unwrap();
        f.write_all(&[0x11u8; 999]).unwrap();
        drop(f);

        let uri = segment_uri(&archive, prefix.len() as u64, bytes.len() as u64).unwrap();
        let c = open(&uri).unwrap();
        let sum: i64 = c.query_row("SELECT SUM(v) FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(sum, 6);
        let v: i64 = c.query_row("SELECT v FROM t WHERE k='b'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 2);
        assert!(c.execute("INSERT INTO t VALUES('z',9)", []).is_err());
        drop(c);
        let after = std::fs::read(&archive).unwrap();
        assert_eq!(after[..prefix.len()], prefix[..]);
        assert_eq!(&after[prefix.len()..prefix.len() + bytes.len()], &bytes[..]);
    }

    /// Garbage in the window is an error, not a panic.
    #[test]
    fn garbage_window_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("junk.wax");
        std::fs::write(&archive, vec![0xFFu8; 4096]).unwrap();
        let uri = segment_uri(&archive, 0, 4096).unwrap();
        let c = open(&uri).unwrap();
        assert!(c.query_row("SELECT 1 FROM sqlite_master", [], |_| Ok(())).is_err());
    }

    /// A window that claims more bytes than the file has produces zero-filled
    /// short reads: an error or an empty result, never a panic.
    #[test]
    fn window_past_eof_is_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("plain.db");
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(1);")
            .unwrap();
        let len = std::fs::metadata(&db).unwrap().len();
        let uri = segment_uri(&db, 0, len + 100_000).unwrap();
        let c = open(&uri).unwrap();
        let _ = c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get::<_, i64>(0));
    }
}
