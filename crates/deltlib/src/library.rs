//! The pack library: every installed `.wax` under the packs directory,
//! open and ready to serve.
//!
//! * **One `deltd` serves every pack** (deltd API §1). Packs are found by
//!   scanning the packs directory (contract §10: `/var/lib/deltos/packs/`).
//! * **New editions are picked up without a rebuild or restart.** A pack's
//!   state is its 128-byte header, which the append-commit protocol rewrites
//!   last (SPEC §7.1), so a changed header means a committed update and the
//!   pack is reopened; an update mid-write still reads as the old edition.
//!   Each segment's own search index (SPEC v0.12 §5.7) comes with it.
//! * **A pack keeps its hostname across updates**, and a removed pack's
//!   index is retired (see [`crate::catalog`]).
//!
//! `WaxReader` is not `Sync` (its lookup cache is a `RefCell`), so each pack
//! holds a small pool of readers, each behind a mutex.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use sha2::{Digest, Sha256};
use wax_core::{ReadOptions, WaxReader, HEADER_LEN};

use crate::catalog::{Catalog, CatalogError, Slot};

/// Lowercase hyphenated UUID text (contract §11's canonical form).
pub fn uuid_text(b: &[u8; 16]) -> String {
    let h = hex(b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

pub fn hex(b: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(D[(x >> 4) as usize] as char);
        s.push(D[(x & 15) as usize] as char);
    }
    s
}

/// A pool of readers for one pack.
pub struct ReaderPool {
    readers: Vec<Mutex<WaxReader>>,
    next: AtomicUsize,
}

impl ReaderPool {
    /// Run `f` with any free reader, waiting for one if all are busy.
    pub fn with<R>(&self, f: impl FnOnce(&WaxReader) -> R) -> R {
        let n = self.readers.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed) % n;
        for k in 0..n {
            if let Ok(r) = self.readers[(start + k) % n].try_lock() {
                return f(&r);
            }
        }
        let r = self.readers[start].lock().unwrap_or_else(|p| p.into_inner());
        f(&r)
    }
}

/// One installed pack.
pub struct Pack {
    pub archive_uuid: String,
    pub slot: Slot,
    pub path: PathBuf,
    pub file_size: u64,
    /// SHA-256 of the header: changes with every committed update. Feeds the
    /// ETags of everything the API derives from this pack (deltd API §3 rule 10).
    pub state_token: String,
    pub manifest: BTreeMap<String, String>,
    pub search_tokenizer: Option<String>,
    pub segment_count: usize,
    pub live_entry_count: Option<u64>,
    pub readers: ReaderPool,
    /// A separate handle for raw blob reads (media ranges, zstd passthrough),
    /// so long transfers never hold a reader.
    raw: File,
    header: [u8; HEADER_LEN],
}

impl Pack {
    fn open(path: &Path, slot_for: &mut dyn FnMut(&str) -> Result<Slot, CatalogError>, pool: usize) -> Result<Pack, String> {
        let header = read_header(path).map_err(|e| e.to_string())?;
        let opts = ReadOptions::default();
        let mut readers = Vec::with_capacity(pool.max(1));
        for _ in 0..pool.max(1) {
            readers.push(Mutex::new(WaxReader::open_with(path, opts).map_err(|e| e.to_string())?));
        }
        let r0 = readers[0].lock().unwrap();
        let archive_uuid = uuid_text(&r0.header().archive_uuid);
        let manifest = r0.manifest().clone();
        let search_tokenizer = r0.search_tokenizer().map(str::to_string);
        let segment_count = r0.segment_count();
        let live_entry_count = r0.stored_entry_count();
        let file_size = r0.file_size();
        drop(r0);
        let slot = slot_for(&archive_uuid).map_err(|e| e.to_string())?;
        Ok(Pack {
            state_token: hex(&Sha256::digest(header)),
            archive_uuid,
            slot,
            path: path.to_path_buf(),
            file_size,
            manifest,
            search_tokenizer,
            segment_count,
            live_entry_count,
            readers: ReaderPool { readers, next: AtomicUsize::new(0) },
            raw: File::open(path).map_err(|e| e.to_string())?,
            header,
        })
    }

    pub fn name(&self) -> &str {
        self.manifest.get("name").map(String::as_str).unwrap_or("")
    }

    /// The manifest's `guest_accessible` (contract §11; default false). The
    /// admin override belongs to the catalog and is not built yet.
    pub fn guest_accessible(&self) -> bool {
        self.manifest.get("guest_accessible").map(String::as_str) == Some("true")
    }

    pub fn languages(&self) -> Vec<String> {
        self.manifest
            .get("languages")
            .map(|l| l.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    }

    /// Read `len` stored bytes at `offset` within the blob region, exactly as
    /// stored (no decoding, no checksum). Bounds-checked against the file.
    pub fn read_stored(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        offset.checked_add(len).filter(|e| *e <= self.file_size).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "blob span outside the file")
        })?;
        let mut buf = vec![0u8; len as usize];
        read_exact_at(&self.raw, &mut buf, offset)?;
        Ok(buf)
    }
}

fn read_header(path: &Path) -> std::io::Result<[u8; HEADER_LEN]> {
    let f = File::open(path)?;
    let mut h = [0u8; HEADER_LEN];
    read_exact_at(&f, &mut h, 0)?;
    Ok(h)
}

#[cfg(unix)]
fn read_exact_at(f: &File, buf: &mut [u8], off: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.read_exact_at(buf, off)
}

#[cfg(windows)]
fn read_exact_at(f: &File, mut buf: &mut [u8], mut off: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_read(buf, off)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut buf[n..];
        off += n as u64;
    }
    Ok(())
}

/// An immutable view of the installed packs. Requests take one snapshot and
/// use it throughout, so a refresh never changes a pack under a request.
#[derive(Default)]
pub struct Snapshot {
    pub by_uuid: BTreeMap<String, Arc<Pack>>,
    by_index: HashMap<u16, Arc<Pack>>,
    /// Files that failed to open, with the reason.
    pub damaged: Vec<(PathBuf, String)>,
}

impl Snapshot {
    pub fn by_index(&self, pack_index: u16) -> Option<&Arc<Pack>> {
        self.by_index.get(&pack_index)
    }
}

/// What a refresh changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RefreshReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub damaged: Vec<PathBuf>,
}

/// Box-side problems a health monitor should hear about (Track F healthd is
/// not built; these are counted and logged).
#[derive(Default)]
pub struct Health {
    events: AtomicU64,
}

impl Health {
    pub fn record(&self, what: &str) {
        self.events.fetch_add(1, Ordering::Relaxed);
        eprintln!("deltd health: {what}");
    }
    pub fn count(&self) -> u64 {
        self.events.load(Ordering::Relaxed)
    }
}

pub struct Library {
    dir: PathBuf,
    readers_per_pack: usize,
    catalog: Mutex<Catalog>,
    current: RwLock<Arc<Snapshot>>,
    pub health: Health,
}

impl Library {
    pub fn new(dir: impl Into<PathBuf>, catalog: Catalog, readers_per_pack: usize) -> Library {
        Library {
            dir: dir.into(),
            readers_per_pack: readers_per_pack.max(1),
            catalog: Mutex::new(catalog),
            current: RwLock::new(Arc::new(Snapshot::default())),
            health: Health::default(),
        }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.current.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Rescan the packs directory. Unchanged packs are kept as they are;
    /// changed ones are reopened; vanished ones are retired.
    pub fn refresh(&self) -> std::io::Result<RefreshReport> {
        let old = self.snapshot();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "wax") && p.is_file())
            .collect();
        files.sort();

        let old_by_path: HashMap<&Path, &Arc<Pack>> =
            old.by_uuid.values().map(|p| (p.path.as_path(), p)).collect();
        let mut next = Snapshot::default();
        let mut report = RefreshReport::default();
        let mut catalog = self.catalog.lock().unwrap_or_else(|p| p.into_inner());

        for f in files {
            let header = match read_header(&f) {
                Ok(h) => h,
                Err(e) => {
                    next.damaged.push((f.clone(), e.to_string()));
                    continue;
                }
            };
            if let Some(p) = old_by_path.get(f.as_path()) {
                if p.header == header {
                    if !next.by_uuid.contains_key(&p.archive_uuid) {
                        next.by_uuid.insert(p.archive_uuid.clone(), Arc::clone(p));
                    }
                    continue;
                }
            }
            match Pack::open(&f, &mut |u| catalog.assign(u), self.readers_per_pack) {
                Ok(p) => {
                    if next.by_uuid.contains_key(&p.archive_uuid) {
                        let why = format!("a second file carries archive_uuid {}", p.archive_uuid);
                        self.health.record(&format!("{}: {why}; not served", f.display()));
                        next.damaged.push((f.clone(), why));
                        continue;
                    }
                    if old.by_uuid.contains_key(&p.archive_uuid) {
                        report.updated.push(p.archive_uuid.clone());
                    } else {
                        report.added.push(p.archive_uuid.clone());
                    }
                    next.by_uuid.insert(p.archive_uuid.clone(), Arc::new(p));
                }
                Err(e) => {
                    self.health.record(&format!("{}: cannot open: {e}", f.display()));
                    next.damaged.push((f.clone(), e));
                }
            }
        }
        for uuid in old.by_uuid.keys() {
            if !next.by_uuid.contains_key(uuid) {
                if let Err(e) = catalog.retire(uuid) {
                    self.health.record(&format!("retiring {uuid}: {e}"));
                }
                report.removed.push(uuid.clone());
            }
        }
        // the catalog may remember packs from before a restart that are gone now
        if let Ok(installed) = catalog.installed() {
            for uuid in installed {
                if !next.by_uuid.contains_key(&uuid) && !report.removed.contains(&uuid) {
                    let _ = catalog.retire(&uuid);
                    report.removed.push(uuid);
                }
            }
        }
        drop(catalog);
        report.damaged = next.damaged.iter().map(|(p, _)| p.clone()).collect();
        for p in next.by_uuid.values() {
            next.by_index.insert(p.slot.pack_index, Arc::clone(p));
        }
        *self.current.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(next);
        Ok(report)
    }
}
