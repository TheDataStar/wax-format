//! Writing a backup, and opening one.
//!
//! On the drive a backup is one file, `deltos-backup-<UTC time>-<8 hex>.age`:
//! an [age] file (passphrase recipient, scrypt `2^18`) holding a zstd stream
//! holding a tar stream. The tar's first member is `manifest.json`; every
//! other member is `state/<path under the state tree>`. The manifest lists
//! every member with its size, SHA-256, owner, group and mode, so a restore
//! can refuse anything it does not list.
//!
//! [age]: https://age-encryption.org/v1

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::set::{self, Treatment};
use crate::{hex, now_unix, owners, Error, Layout, Passphrase, Result, FORMAT_VERSION, SCRYPT_LOG_N};

pub const MANIFEST_NAME: &str = "manifest.json";
pub const STATE_PREFIX: &str = "state/";
const ID_PREFIX: &str = "deltos-backup-";
const ID_SUFFIX: &str = ".age";
/// zstd level for the inner stream. The state is small (packs are not in
/// it) and mostly SQLite pages; level 9 is well inside a Pi's means and
/// the size is not what decides anything here.
const ZSTD_LEVEL: i32 = 9;

/// One file or folder in the backup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the state tree, `/`-separated.
    pub path: String,
    /// `dir` or `file`.
    pub kind: String,
    /// Owner and group by name (see [`crate::owners`]).
    pub owner: String,
    pub group: String,
    /// Permission bits (`0o7777` at most).
    pub mode: u32,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub sha256: String,
}

/// An installed pack, listed but not copied (docs/box-backup.md §3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackRecord {
    pub file: String,
    pub archive_uuid: String,
    pub name: String,
    pub version: String,
    pub size: u64,
}

/// Something under the tree that is not in the backup, and why.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Left {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub format: String,
    pub format_version: u32,
    pub created_at: i64,
    /// The profile that started it, or `console` for root at the box.
    pub made_by: String,
    pub entries: Vec<Entry>,
    pub packs: Vec<PackRecord>,
    /// Left out by the backup set's rules.
    pub excluded: Vec<Left>,
    /// Left out because it is not a plain file or folder.
    pub skipped: Vec<Left>,
    /// Latest sign-in activity in the register when the backup was made
    /// (unix seconds), for the review; sessions themselves are not kept.
    pub identity_last_activity: Option<i64>,
}

/// What [`create`] wrote.
#[derive(Debug, Clone, Serialize)]
pub struct Created {
    pub backup_id: String,
    pub bytes: u64,
    pub files: usize,
    pub packs_listed: usize,
    pub excluded: Vec<Left>,
    pub skipped: Vec<Left>,
}

/// One backup found on a drive.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BackupInfo {
    pub backup_id: String,
    pub bytes: u64,
    /// From the file name (`YYYYMMDDTHHMMSSZ`); the file says nothing else
    /// without the passphrase.
    pub made: String,
}

/// A backup id is exactly a name [`create`] makes: no path, no surprises
/// (privilege boundary rule 6).
pub fn check_backup_id(id: &str) -> Result<()> {
    let bad = || Error::Invalid(format!("{id:?} is not a backup id"));
    let mid = id.strip_prefix(ID_PREFIX).and_then(|s| s.strip_suffix(ID_SUFFIX)).ok_or_else(bad)?;
    let (stamp, tag) = mid.split_once('-').ok_or_else(bad)?;
    let stamp_ok = stamp.len() == 16
        && stamp.bytes().enumerate().all(|(i, b)| match i {
            8 => b == b'T',
            15 => b == b'Z',
            _ => b.is_ascii_digit(),
        });
    let tag_ok = tag.len() == 8 && tag.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if stamp_ok && tag_ok {
        Ok(())
    } else {
        Err(bad())
    }
}

/// The backups in a folder, newest first.
pub fn list(dir: &Path) -> Result<Vec<BackupInfo>> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if check_backup_id(&name).is_err() {
            continue;
        }
        let md = e.metadata()?;
        if !md.is_file() {
            continue;
        }
        let made = name[ID_PREFIX.len()..ID_PREFIX.len() + 16].to_string();
        out.push(BackupInfo { backup_id: name, bytes: md.len(), made });
    }
    out.sort_by(|a, b| b.backup_id.cmp(&a.backup_id));
    Ok(out)
}

/// UTC `YYYYMMDDTHHMMSSZ` from unix seconds (civil-from-days, H. Hinnant).
pub(crate) fn utc_stamp(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

fn rel_str(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

fn is_sqlite(path: &Path) -> io::Result<bool> {
    let mut f = File::open(path)?;
    let mut magic = [0u8; 16];
    match f.read_exact(&mut magic) {
        Ok(()) => Ok(&magic == b"SQLite format 3\0"),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

fn hash_file(path: &Path) -> io::Result<(u64, String)> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((n, hex(&h.finalize())))
}

/// A consistent copy of a live SQLite database, through SQLite itself.
fn copy_sqlite(src: &Path, dst: &Path, rel: &str) -> Result<Option<i64>> {
    let c = rusqlite::Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    c.execute("VACUUM INTO ?1", [dst.to_string_lossy()])?;
    drop(c);
    if rel != set::IDENTITY_DB {
        return Ok(None);
    }
    // the register: note the latest activity, then drop live sessions and
    // tickets from the copy (set::IDENTITY_DROPPED_TABLES) and rewrite it
    // so their pages are gone too
    let c = rusqlite::Connection::open(dst)?;
    let has = |t: &str| -> rusqlite::Result<bool> {
        c.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1", [t], |r| r.get::<_, i64>(0)).map(|n| n > 0)
    };
    let mut last: Option<i64> = None;
    for (t, col) in [("sessions", "last_active"), ("devices", "last_used")] {
        if has(t)? {
            let v: Option<i64> = c.query_row(&format!("SELECT max({col}) FROM {t}"), [], |r| r.get(0))?;
            last = last.max(v);
        }
    }
    for t in set::IDENTITY_DROPPED_TABLES {
        if has(t)? {
            c.execute(&format!("DELETE FROM {t}"), [])?;
        }
    }
    c.execute_batch("VACUUM")?;
    Ok(last)
}

struct Walk {
    entries: Vec<Entry>,
    skipped: Vec<Left>,
    last_activity: Option<i64>,
}

fn owner_of(md: &fs::Metadata) -> (String, String) {
    (
        owners::user_name(md.uid()).unwrap_or_else(|| md.uid().to_string()),
        owners::group_name(md.gid()).unwrap_or_else(|| md.gid().to_string()),
    )
}

/// Copy one included subtree into the snapshot folder, recording each entry.
fn walk(root: &Path, rel: &Path, snap: &Path, w: &mut Walk) -> Result<()> {
    let src = root.join(rel);
    let md = fs::symlink_metadata(&src)?;
    let rs = rel_str(rel);
    let ft = md.file_type();
    let (owner, group) = owner_of(&md);
    let mode = md.permissions().mode() & 0o7777;
    if ft.is_dir() {
        fs::create_dir_all(snap.join(rel))?;
        w.entries.push(Entry { path: rs, kind: "dir".into(), owner, group, mode, size: 0, sha256: String::new() });
        let mut kids: Vec<_> = fs::read_dir(&src)?.collect::<io::Result<Vec<_>>>()?;
        kids.sort_by_key(|e| e.file_name());
        for k in kids {
            let name = k.file_name();
            let name_s = name.to_string_lossy();
            if set::is_sqlite_side_file(&name_s) {
                continue;
            }
            walk(root, &rel.join(&name), snap, w)?;
        }
    } else if ft.is_file() {
        let dst = snap.join(rel);
        if is_sqlite(&src)? {
            let la = copy_sqlite(&src, &dst, &rs)?;
            w.last_activity = w.last_activity.max(la);
        } else {
            fs::copy(&src, &dst)?;
        }
        let (size, sha256) = hash_file(&dst)?;
        w.entries.push(Entry { path: rs, kind: "file".into(), owner, group, mode, size, sha256 });
    } else {
        let what = if ft.is_symlink() { "a symbolic link" } else { "not a plain file or folder" };
        w.skipped.push(Left { path: rs, reason: format!("{what}; a backup holds only plain files and folders") });
    }
    Ok(())
}

fn pack_record(path: &Path) -> PackRecord {
    let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    match wax_core::WaxReader::open(path) {
        Ok(r) => {
            let m = r.manifest();
            let get = |k: &str| m.get(k).cloned().unwrap_or_default();
            PackRecord {
                file,
                archive_uuid: deltlib::library::uuid_text(&r.header().archive_uuid),
                name: get("name"),
                version: get("version"),
                size,
            }
        }
        // listed anyway, so the review shows the admin it was there
        Err(_) => PackRecord { file, archive_uuid: String::new(), name: String::new(), version: String::new(), size },
    }
}

fn list_packs(dir: &Path) -> Result<Vec<PackRecord>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut names: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "wax").unwrap_or(false) && p.is_file())
        .collect();
    names.sort();
    for p in names {
        out.push(pack_record(&p));
    }
    Ok(out)
}

/// A private working folder inside the state tree's staging area.
pub(crate) fn private_dir(parent: &Path, prefix: &str) -> Result<PathBuf> {
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    let mut tag = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut tag);
    let d = parent.join(format!("{prefix}{}", hex(&tag)));
    fs::create_dir(&d)?;
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700))?;
    Ok(d)
}

/// Write a backup of the state tree into `target` (a mounted drive's
/// folder). `made_by` is the profile id for the log, or `console`.
pub fn create(layout: &Layout, target: &Path, passphrase: &Passphrase, made_by: &str) -> Result<Created> {
    if !target.is_dir() {
        return Err(Error::NotFound(format!("{} is not a folder", target.display())));
    }
    let snap = private_dir(&layout.staging(), "snapshot-")?;
    let out = (|| {
        let mut w = Walk { entries: Vec::new(), skipped: Vec::new(), last_activity: None };
        let mut excluded = Vec::new();
        let mut packs = Vec::new();
        let mut tops: Vec<_> = fs::read_dir(&layout.root)?.collect::<io::Result<Vec<_>>>()?;
        tops.sort_by_key(|e| e.file_name());
        for t in tops {
            let name = t.file_name().to_string_lossy().into_owned();
            if name == set::STAGING_DIR || name == set::PREVIOUS_DIR {
                continue; // a restore's own folders; never listed
            }
            match set::treatment(&name) {
                Treatment::Include => walk(&layout.root, Path::new(&name), &snap, &mut w)?,
                Treatment::PackList => {
                    packs = list_packs(&layout.root.join(&name))?;
                    excluded.push(Left {
                        path: name,
                        reason: "pack files are listed, not copied: they are large, can be fetched again, and are verified again when reinstalled".into(),
                    });
                }
                Treatment::Exclude(why) => excluded.push(Left { path: name, reason: why.into() }),
            }
        }
        let created_at = now_unix();
        let manifest = Manifest {
            format: "deltos-backup".into(),
            format_version: FORMAT_VERSION,
            created_at,
            made_by: made_by.into(),
            entries: w.entries,
            packs,
            excluded,
            skipped: w.skipped,
            identity_last_activity: w.last_activity,
        };
        let mut tag = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut tag);
        let id = format!("{ID_PREFIX}{}-{}{ID_SUFFIX}", utc_stamp(created_at), hex(&tag));
        let part = target.join(format!(".{id}.part"));
        let bytes = write_archive(&part, &manifest, &snap, passphrase).inspect_err(|_| {
            let _ = fs::remove_file(&part);
        })?;
        fs::rename(&part, target.join(&id))?;
        File::open(target)?.sync_all()?;
        Ok(Created {
            backup_id: id,
            bytes,
            files: manifest.entries.iter().filter(|e| e.kind == "file").count(),
            packs_listed: manifest.packs.len(),
            excluded: manifest.excluded,
            skipped: manifest.skipped,
        })
    })();
    let _ = fs::remove_dir_all(&snap);
    out
}

fn tar_header(size: u64, mode: u32, dir: bool) -> tar::Header {
    let mut h = tar::Header::new_ustar();
    h.set_size(size);
    h.set_mode(mode);
    h.set_uid(0);
    h.set_gid(0);
    h.set_mtime(0);
    h.set_entry_type(if dir { tar::EntryType::Directory } else { tar::EntryType::Regular });
    h
}

fn write_archive(part: &Path, m: &Manifest, snap: &Path, passphrase: &Passphrase) -> Result<u64> {
    let file = fs::OpenOptions::new().write(true).create_new(true).mode_0600().open(part)?;
    let mut recipient = age::scrypt::Recipient::new(passphrase.secret());
    recipient.set_work_factor(SCRYPT_LOG_N);
    let enc = age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
        .map_err(|e| Error::Io(io::Error::other(e.to_string())))?;
    let aw = enc.wrap_output(BufWriter::new(file))?;
    let zw = zstd::Encoder::new(aw, ZSTD_LEVEL)?;
    let mut tb = tar::Builder::new(zw);
    let mj = serde_json::to_vec_pretty(m).map_err(|e| Error::Io(io::Error::other(e)))?;
    let mut h = tar_header(mj.len() as u64, 0o600, false);
    tb.append_data(&mut h, MANIFEST_NAME, &mj[..])?;
    for e in &m.entries {
        let name = format!("{STATE_PREFIX}{}", e.path);
        if e.kind == "dir" {
            let mut h = tar_header(0, 0o700, true);
            tb.append_data(&mut h, &name, io::empty())?;
        } else {
            let mut h = tar_header(e.size, 0o600, false);
            tb.append_data(&mut h, &name, File::open(snap.join(&e.path))?)?;
        }
    }
    let zw = tb.into_inner()?;
    let aw = zw.finish()?;
    let bw = aw.finish()?;
    let file = bw.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    Ok(file.metadata()?.len())
}

trait Mode0600 {
    fn mode_0600(&mut self) -> &mut Self;
}
impl Mode0600 for fs::OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }
}

/// Open a backup with its passphrase: the tar stream inside.
pub(crate) fn open(path: &Path, passphrase: &Passphrase) -> Result<tar::Archive<Box<dyn Read>>> {
    let f = File::open(path)?;
    let dec = age::Decryptor::new(BufReader::new(f)).map_err(|_| Error::Unreadable)?;
    if !dec.is_scrypt() {
        return Err(Error::Unreadable);
    }
    let mut id = age::scrypt::Identity::new(passphrase.secret());
    id.set_max_work_factor(SCRYPT_LOG_N);
    let r = dec.decrypt(std::iter::once(&id as &dyn age::Identity)).map_err(|_| Error::Unreadable)?;
    let z = zstd::Decoder::new(r)?;
    Ok(tar::Archive::new(Box::new(z) as Box<dyn Read>))
}

/// Index of a manifest's entries by path.
pub(crate) fn by_path(m: &Manifest) -> BTreeMap<&str, &Entry> {
    m.entries.iter().map(|e| (e.path.as_str(), e)).collect()
}

impl Passphrase {
    /// For tests and the console's "type it again" check.
    pub fn same_as(&self, other: &Passphrase) -> bool {
        self.expose() == other.expose()
    }
}

/// UTC `YYYYMMDDTHHMMSSZ` (for the command's messages).
pub fn utc_stamp_pub(t: i64) -> String {
    utc_stamp(t)
}
