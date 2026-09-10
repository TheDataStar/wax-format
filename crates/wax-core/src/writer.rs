//! [`WaxWriter`] — assemble a valid archive (SPEC §6, §7).
//!
//! `build` writes a single-segment v0.9 archive. `append` adds a further
//! `(blob region, index segment)` pair following the append-commit protocol
//! (SPEC §7): append blobs + segment, fsync, then overwrite the 128-byte header
//! in place as the final step. `wax-builder append` (A2) drives this path; the
//! A4 conformance suite drives it directly.

use crate::header::WaxHeader;
use crate::model::{Compression, EntryContent, EntryInput};
use crate::segment::SEGMENT_FORMAT_TAG;
use crate::{Result, WaxError, FORMAT_VERSION_MAJOR, FORMAT_VERSION_MINOR, HEADER_LEN};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// A blob to be written plus its `entries` metadata (post-flatten).
struct Prepared {
    path: String,
    title: Option<String>,
    mime: Option<String>,
    redirect_to: Option<String>,
    /// On-disk bytes for the blob (already compressed if applicable).
    blob: Vec<u8>,
    compression: Compression,
    uncompressed_length: u64,
    sha256: Option<[u8; 32]>,
}

pub struct WaxWriter {
    archive_uuid: [u8; 16],
    created_at: u64,
    flags: u16,
}

impl WaxWriter {
    pub fn new(archive_uuid: [u8; 16]) -> Self {
        WaxWriter {
            archive_uuid,
            created_at: now(),
            flags: 0,
        }
    }

    pub fn created_at(mut self, t: u64) -> Self {
        self.created_at = t;
        self
    }

    /// Header `flags` for a fresh [`WaxWriter::build`] (SPEC §2.1). Unknown bits
    /// are cleared on serialization. `append` preserves the existing archive's
    /// flags instead of using this value.
    ///
    /// This matters for signing: `is_signed` lives in the header, and the header
    /// is inside the signed digest (SPEC §8.1), so the flag must be set *before*
    /// the digest is computed.
    pub fn flags(mut self, flags: u16) -> Self {
        self.flags = flags;
        self
    }

    /// Build a fresh single-segment archive at `output`.
    pub fn build(
        &self,
        output: impl AsRef<Path>,
        entries: Vec<EntryInput>,
        manifest: &BTreeMap<String, String>,
    ) -> Result<()> {
        let prepared = prepare(entries, &BTreeSet::new())?;

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .read(true)
            .open(output.as_ref())?;

        // 1. header placeholder
        file.write_all(&[0u8; HEADER_LEN])?;

        // 2. blob region 0
        let blob_region_offset = HEADER_LEN as u64;
        let mut cursor = blob_region_offset;
        let mut rows: Vec<SegRow> = Vec::with_capacity(prepared.len());
        for p in &prepared {
            if p.redirect_to.is_some() {
                rows.push(SegRow::from_redirect(p));
                continue;
            }
            let offset = cursor;
            file.write_all(&p.blob)?;
            cursor += p.blob.len() as u64;
            rows.push(SegRow::from_blob(p, offset));
        }
        let blob_region_length = cursor - blob_region_offset;

        // 3. index segment 0
        let index_offset = cursor;
        let seg_bytes = build_segment_db(SegmentSpec {
            segment_index: 0,
            blob_region_offset,
            blob_region_length,
            created_at: self.created_at as i64,
            prev_segment: None,
            rows: &rows,
            manifest: Some(manifest),
        })?;
        file.write_all(&seg_bytes)?;
        let index_length = seg_bytes.len() as u64;

        // 4. fsync, then overwrite header (SPEC §7.1)
        file.sync_all()?;
        let header = WaxHeader {
            version_major: FORMAT_VERSION_MAJOR,
            version_minor: FORMAT_VERSION_MINOR,
            flags: self.flags,
            archive_uuid: self.archive_uuid,
            created_at: self.created_at,
            index_offset,
            index_length,
            blob_section_length: blob_region_length,
            search_index_offset: 0,
            search_index_length: 0,
        };
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header.to_bytes())?;
        file.sync_all()?;
        Ok(())
    }

    /// Append a `(blob region, index segment)` pair to an existing archive,
    /// following SPEC §7.1. Drives `wax-builder append` (A2).
    pub fn append(&self, archive: impl AsRef<Path>, entries: Vec<EntryInput>) -> Result<()> {
        // Redirect targets in an append may point at entries carried by earlier
        // segments, so those paths count as valid targets during flattening.
        let existing: BTreeSet<String> = {
            let reader = crate::reader::WaxReader::open_with(
                archive.as_ref(),
                crate::reader::ReadOptions { verify_checksums: false },
            )?;
            reader.paths().map(|s| s.to_string()).collect()
        };
        let prepared = prepare(entries, &existing)?;

        let mut file = OpenOptions::new().read(true).write(true).open(archive.as_ref())?;
        let file_size = file.seek(SeekFrom::End(0))?;

        let mut hbuf = [0u8; HEADER_LEN];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut hbuf)?;
        let old = WaxHeader::parse(&hbuf)?;
        old.validate(file_size)?;

        let prev_segment = Some((old.index_offset, old.index_length));
        let next_index = read_segment_index(&mut file, old.index_offset, old.index_length)? + 1;

        // 1. append blob region at EOF
        file.seek(SeekFrom::Start(file_size))?;
        let blob_region_offset = file_size;
        let mut cursor = blob_region_offset;
        let mut rows: Vec<SegRow> = Vec::with_capacity(prepared.len());
        for p in &prepared {
            if p.redirect_to.is_some() {
                rows.push(SegRow::from_redirect(p));
                continue;
            }
            let offset = cursor;
            file.write_all(&p.blob)?;
            cursor += p.blob.len() as u64;
            rows.push(SegRow::from_blob(p, offset));
        }
        let blob_region_length = cursor - blob_region_offset;

        // 2. append index segment
        let index_offset = cursor;
        let seg_bytes = build_segment_db(SegmentSpec {
            segment_index: next_index,
            blob_region_offset,
            blob_region_length,
            created_at: self.created_at as i64,
            prev_segment,
            rows: &rows,
            manifest: None,
        })?;
        file.write_all(&seg_bytes)?;
        let index_length = seg_bytes.len() as u64;

        // 3. fsync
        file.sync_all()?;

        // 4. overwrite header in place (single 128-byte write)
        let header = WaxHeader {
            version_major: old.version_major,
            version_minor: old.version_minor,
            flags: old.flags,
            archive_uuid: old.archive_uuid,
            created_at: self.created_at,
            index_offset,
            index_length,
            blob_section_length: old.blob_section_length + blob_region_length,
            search_index_offset: old.search_index_offset,
            search_index_length: old.search_index_length,
        };
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header.to_bytes())?;
        file.sync_all()?;
        Ok(())
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Path normalization + redirect flattening (SPEC §6.1, §6.2)
// ---------------------------------------------------------------------------

/// Normalize an authored path to the stored form, or reject it.
pub fn normalize_path(input: &str) -> Result<String> {
    let replaced = input.replace('\\', "/");
    let trimmed = replaced.trim_start_matches('/');
    if trimmed.is_empty() {
        return Err(WaxError::Schema {
            detail: format!("empty path from {input:?}"),
        });
    }
    if trimmed.ends_with('/') {
        return Err(WaxError::Schema {
            detail: format!("path {input:?} ends with '/'"),
        });
    }
    for comp in trimmed.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." {
            return Err(WaxError::Schema {
                detail: format!("path {input:?} has an empty or traversal component"),
            });
        }
    }
    Ok(trimmed.to_string())
}

fn prepare(entries: Vec<EntryInput>, known_external: &BTreeSet<String>) -> Result<Vec<Prepared>> {
    // normalize + dedup-check
    let mut norm: Vec<(String, EntryInput)> = Vec::with_capacity(entries.len());
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for e in entries {
        let path = normalize_path(&e.path)?;
        if seen.insert(path.clone(), ()).is_some() {
            return Err(WaxError::Schema {
                detail: format!("duplicate path after normalization: {path:?}"),
            });
        }
        norm.push((path, e));
    }

    // Build a view of which normalized paths are redirects → their target.
    let mut redirect_target: BTreeMap<String, String> = BTreeMap::new();
    for (path, e) in &norm {
        if let EntryContent::Redirect { to } = &e.content {
            redirect_target.insert(path.clone(), normalize_path(to)?);
        }
    }
    let all_paths: BTreeMap<String, ()> = norm.iter().map(|(p, _)| (p.clone(), ())).collect();

    // Flatten: for each redirect, follow until a non-redirect target.
    let flatten = |start: &str| -> Result<String> {
        let mut cur = start.to_string();
        for _ in 0..(redirect_target.len() + 1) {
            match redirect_target.get(&cur) {
                None => {
                    if !all_paths.contains_key(&cur) && !known_external.contains(&cur) {
                        return Err(WaxError::DanglingRedirect {
                            from: start.to_string(),
                            to: cur,
                        });
                    }
                    return Ok(cur);
                }
                Some(next) => {
                    if next == start {
                        return Err(WaxError::RedirectChainTooDeep {
                            from: start.to_string(),
                            via: cur,
                        });
                    }
                    cur = next.clone();
                }
            }
        }
        Err(WaxError::RedirectChainTooDeep {
            from: start.to_string(),
            via: cur,
        })
    };

    let mut out = Vec::with_capacity(norm.len());
    for (path, e) in norm {
        match e.content {
            EntryContent::Redirect { .. } => {
                let target = flatten(&path)?;
                out.push(Prepared {
                    path,
                    title: e.title,
                    mime: e.mime,
                    redirect_to: Some(target),
                    blob: Vec::new(),
                    compression: Compression::None,
                    uncompressed_length: 0,
                    sha256: None,
                });
            }
            EntryContent::Data { bytes, compression } => {
                let sha: [u8; 32] = Sha256::digest(&bytes).into();
                let uncompressed_length = bytes.len() as u64;
                let blob = match compression {
                    Compression::None => bytes,
                    Compression::Zstd => zstd::stream::encode_all(&bytes[..], 3)
                        .map_err(|err| WaxError::Decompress {
                            path: path.clone(),
                            detail: format!("compress: {err}"),
                        })?,
                };
                out.push(Prepared {
                    path,
                    title: e.title,
                    mime: e.mime,
                    redirect_to: None,
                    blob,
                    compression,
                    uncompressed_length,
                    sha256: Some(sha),
                });
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// SQLite segment construction
// ---------------------------------------------------------------------------

/// A single row destined for `entries`. `pub` so fixtures/tests can craft
/// arbitrary (including deliberately malformed) segments via [`build_segment_db`].
pub struct SegRow {
    pub path: String,
    pub title: Option<String>,
    pub offset: i64,
    pub length: i64,
    pub uncompressed_length: i64,
    pub mime: Option<String>,
    pub compression: String,
    pub sha256: Option<Vec<u8>>,
    pub volume_id: i64,
    pub redirect_to: Option<String>,
}

impl SegRow {
    fn from_blob(p: &Prepared, offset: u64) -> Self {
        SegRow {
            path: p.path.clone(),
            title: p.title.clone(),
            offset: offset as i64,
            length: p.blob.len() as i64,
            uncompressed_length: p.uncompressed_length as i64,
            mime: p.mime.clone(),
            compression: p.compression.as_str().to_string(),
            sha256: p.sha256.map(|h| h.to_vec()),
            volume_id: 0,
            redirect_to: None,
        }
    }
    fn from_redirect(p: &Prepared) -> Self {
        SegRow {
            path: p.path.clone(),
            title: p.title.clone(),
            offset: 0,
            length: 0,
            uncompressed_length: 0,
            mime: p.mime.clone(),
            compression: "none".to_string(),
            sha256: None,
            volume_id: 0,
            redirect_to: p.redirect_to.clone(),
        }
    }
}

/// Everything needed to serialize one index segment.
pub struct SegmentSpec<'a> {
    pub segment_index: u64,
    pub blob_region_offset: u64,
    pub blob_region_length: u64,
    pub created_at: i64,
    pub prev_segment: Option<(u64, u64)>,
    pub rows: &'a [SegRow],
    pub manifest: Option<&'a BTreeMap<String, String>>,
}

/// Serialize an index segment to its SQLite bytes (SPEC §4, §5) from a
/// [`SegmentSpec`] — the normal, well-formed path.
///
/// Public so the conformance suite can build valid multi-segment layouts
/// without going through [`WaxWriter`]. For deliberately malformed segments,
/// use [`build_segment_db_with_meta`].
pub fn build_segment_db(spec: SegmentSpec) -> Result<Vec<u8>> {
    let mut meta: Vec<(String, String)> = vec![
        ("format".into(), SEGMENT_FORMAT_TAG.into()),
        ("segment_index".into(), spec.segment_index.to_string()),
        ("blob_region_offset".into(), spec.blob_region_offset.to_string()),
        ("blob_region_length".into(), spec.blob_region_length.to_string()),
        ("created_at".into(), spec.created_at.to_string()),
    ];
    if let Some((po, pl)) = spec.prev_segment {
        meta.push(("prev_segment_offset".into(), po.to_string()));
        meta.push(("prev_segment_length".into(), pl.to_string()));
    }
    build_segment_db_with_meta(spec.rows, &meta, spec.manifest)
}

/// Low-level segment serializer: writes exactly the `entries` rows and
/// `segment_meta` pairs given, with no validation or auto-populated keys.
///
/// This is the escape hatch the A4 conformance suite uses to craft adversarial
/// segments (missing keys, out-of-bounds `prev_segment_*`, on-disk redirect
/// chains, `volume_id != 0`, unknown `compression`, …). Well-formed callers
/// should use [`build_segment_db`] / [`WaxWriter`].
pub fn build_segment_db_with_meta(
    rows: &[SegRow],
    meta: &[(String, String)],
    manifest: Option<&BTreeMap<String, String>>,
) -> Result<Vec<u8>> {
    let tmp = tempfile::Builder::new().suffix(".db").tempfile()?;
    let path = tmp.path().to_path_buf();
    {
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "PRAGMA page_size=4096;
             PRAGMA journal_mode=OFF;
             CREATE TABLE entries (
                path TEXT PRIMARY KEY,
                title TEXT,
                offset INTEGER,
                length INTEGER,
                uncompressed_length INTEGER,
                mime TEXT,
                compression TEXT,
                sha256 BLOB,
                volume_id INTEGER DEFAULT 0,
                redirect_to TEXT DEFAULT NULL
             );
             CREATE TABLE segment_meta (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE signatures (
                signer TEXT, algo TEXT, signature BLOB, signed_at INTEGER
             );",
        )?;

        {
            let mut stmt = conn.prepare(
                "INSERT INTO entries
                 (path, title, offset, length, uncompressed_length, mime, compression, sha256, volume_id, redirect_to)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )?;
            for r in rows {
                stmt.execute(rusqlite::params![
                    r.path,
                    r.title,
                    r.offset,
                    r.length,
                    r.uncompressed_length,
                    r.mime,
                    r.compression,
                    r.sha256,
                    r.volume_id,
                    r.redirect_to,
                ])?;
            }
        }
        {
            let mut m = conn.prepare("INSERT INTO segment_meta (key, value) VALUES (?1, ?2)")?;
            for (k, v) in meta {
                m.execute(rusqlite::params![k, v])?;
            }
        }
        if let Some(manifest) = manifest {
            conn.execute("CREATE TABLE manifest (key TEXT PRIMARY KEY, value TEXT)", [])?;
            let mut mm = conn.prepare("INSERT INTO manifest (key, value) VALUES (?1, ?2)")?;
            for (k, v) in manifest {
                mm.execute(rusqlite::params![k, v])?;
            }
        }

        conn.execute("VACUUM", [])?;
        conn.close().map_err(|(_, e)| e)?;
    }

    let mut bytes = Vec::new();
    File::open(&path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_segment_index(file: &mut File, offset: u64, length: u64) -> Result<u64> {
    let mut buf = vec![0u8; length as usize];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut buf)?;
    let tmp = tempfile::NamedTempFile::new()?;
    std::fs::write(tmp.path(), &buf)?;
    let conn = Connection::open(tmp.path())?;
    let v: String = conn.query_row(
        "SELECT value FROM segment_meta WHERE key='segment_index'",
        [],
        |r| r.get(0),
    )?;
    v.trim().parse::<u64>().map_err(|_| WaxError::BrokenSegmentChain {
        detail: format!("prev segment has non-numeric segment_index {v:?}"),
    })
}
