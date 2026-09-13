//! [`WaxWriter`] — assemble a valid archive (SPEC §6, §7).
//!
//! # Streaming (Track A §18)
//!
//! The core of this module is [`StreamingWriter`]: entries are handed over one
//! at a time as a `Read`, their bytes are hashed and compressed in a single
//! pass straight into the archive file, and their index row is inserted into
//! the segment's SQLite database *as they arrive*. Nothing about an archive is
//! held in memory in proportion to its size — not the blobs, and not the index
//! rows. The format already permits this: header, blob body, index footer,
//! with the header written last (SPEC §7).
//!
//! ```text
//! WaxWriter::create(path, manifest)      reserve the 128-byte header
//!   .add_entry(meta, reader)  × n        stream blob → file, row → SQLite
//!   .add_redirect(path, to)   × m        row → SQLite (no blob)
//!   .finish()                            flatten redirects in SQL, VACUUM,
//!                                        copy the index in, commit the header
//! ```
//!
//! [`WaxWriter::build`] and [`WaxWriter::append`] (the `Vec<EntryInput>` API)
//! are thin wrappers over the same path, kept for callers that already hold
//! their entries in memory.
//!
//! Redirect chains are flattened to one hop (SPEC §6.2) at `finish`, inside the
//! index database, with a pointer-jumping `UPDATE` — so that step is bounded
//! by SQLite's page cache too, not by the number of redirects.

use crate::header::WaxHeader;
use crate::model::{Compression, EntryContent, EntryInput};
use crate::segment::SEGMENT_FORMAT_TAG;
use crate::{Result, WaxError, FORMAT_VERSION_MAJOR, FORMAT_VERSION_MINOR, HEADER_LEN};
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use tempfile::NamedTempFile;

/// Read/compress buffer. Bounds the per-entry working memory.
const STREAM_BUF: usize = 64 * 1024;

/// Pointer-jumping iterations before a redirect chain is declared cyclic.
/// Each iteration halves the remaining chain length, so this bounds legitimate
/// depth at 2^64; anything still unresolved is a cycle.
const FLATTEN_ROUNDS: usize = 64;

/// Archive-level configuration: identity, timestamps, header flags.
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

    /// Pin the header's `created_at` (and each segment's
    /// `segment_meta.created_at`). Required for byte-identical rebuilds.
    pub fn created_at(mut self, t: u64) -> Self {
        self.created_at = t;
        self
    }

    /// Header `flags` (SPEC §2.1). Unknown bits are cleared on write.
    pub fn flags(mut self, flags: u16) -> Self {
        self.flags = flags;
        self
    }

    // ------------------------------------------------------------------
    // Streaming API
    // ------------------------------------------------------------------

    /// Start a fresh single-segment archive at `output`. Entries are then
    /// streamed in with [`StreamingWriter::add_entry`] /
    /// [`StreamingWriter::add_redirect`] and the archive is committed by
    /// [`StreamingWriter::finish`].
    pub fn create(
        &self,
        output: impl AsRef<Path>,
        manifest: &BTreeMap<String, String>,
    ) -> Result<StreamingWriter> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .read(true)
            .open(output.as_ref())?;
        // 1. header placeholder — committed last (SPEC §7.1)
        file.write_all(&[0u8; HEADER_LEN])?;

        let index = IndexBuilder::open(Some(manifest))?;
        Ok(StreamingWriter {
            file,
            cursor: HEADER_LEN as u64,
            blob_region_offset: HEADER_LEN as u64,
            index,
            header: HeaderPlan::Fresh {
                archive_uuid: self.archive_uuid,
                created_at: self.created_at,
                flags: self.flags,
            },
            segment_index: 0,
            prev_segment: None,
            created_at: self.created_at,
            external_paths: None,
            entries: 0,
            redirects: 0,
        })
    }

    /// Start an append segment on an existing archive (SPEC §7.1). Blob bytes
    /// and the new index segment go at the end of the file; the header is
    /// rewritten in place as the final step of `finish`.
    ///
    /// Redirects added to this segment may target entries carried by earlier
    /// segments; those paths are read from the archive up front.
    pub fn open_append(&self, archive: impl AsRef<Path>) -> Result<StreamingWriter> {
        // Paths already in the archive count as valid redirect targets. This
        // reads the merged index through the reader, which is O(existing
        // entries) in memory — acceptable for append today (no converter uses
        // it); a streaming variant would ATTACH the prior segments instead.
        let existing: BTreeSet<String> = {
            let reader = crate::reader::WaxReader::open_with(
                archive.as_ref(),
                crate::reader::ReadOptions {
                    verify_checksums: false,
                },
            )?;
            reader.paths().map(|s| s.to_string()).collect()
        };

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(archive.as_ref())?;
        let file_size = file.seek(SeekFrom::End(0))?;
        let mut hbuf = [0u8; HEADER_LEN];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut hbuf)?;
        let old = WaxHeader::parse(&hbuf)?;
        old.validate(file_size)?;
        let next_index = read_segment_index(&mut file, old.index_offset, old.index_length)? + 1;

        file.seek(SeekFrom::Start(file_size))?;
        let index = IndexBuilder::open(None)?;
        Ok(StreamingWriter {
            file,
            cursor: file_size,
            blob_region_offset: file_size,
            index,
            header: HeaderPlan::Append {
                old,
                created_at: self.created_at,
            },
            segment_index: next_index,
            prev_segment: Some((old.index_offset, old.index_length)),
            created_at: self.created_at,
            external_paths: Some(existing),
            entries: 0,
            redirects: 0,
        })
    }

    // ------------------------------------------------------------------
    // Vec-based API — thin wrappers over the streaming path
    // ------------------------------------------------------------------

    /// Build a fresh single-segment archive from in-memory entries.
    pub fn build(
        &self,
        output: impl AsRef<Path>,
        entries: Vec<EntryInput>,
        manifest: &BTreeMap<String, String>,
    ) -> Result<()> {
        let mut w = self.create(output, manifest)?;
        w.add_all(entries)?;
        w.finish()?;
        Ok(())
    }

    /// Append a `(blob region, index segment)` pair from in-memory entries.
    pub fn append(&self, archive: impl AsRef<Path>, entries: Vec<EntryInput>) -> Result<()> {
        let mut w = self.open_append(archive)?;
        w.add_all(entries)?;
        w.finish()?;
        Ok(())
    }
}

/// Everything about one content entry except its bytes.
#[derive(Debug, Clone, Default)]
pub struct EntryMeta {
    pub path: String,
    pub mime: Option<String>,
    pub title: Option<String>,
    pub compression: Option<Compression>,
}

impl EntryMeta {
    pub fn new(path: impl Into<String>) -> Self {
        EntryMeta {
            path: path.into(),
            ..Default::default()
        }
    }
    pub fn mime(mut self, m: impl Into<String>) -> Self {
        self.mime = Some(m.into());
        self
    }
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }
    pub fn compression(mut self, c: Compression) -> Self {
        self.compression = Some(c);
        self
    }
}

/// What [`StreamingWriter::add_entry`] recorded for one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryStats {
    pub offset: u64,
    /// On-disk (compressed) length.
    pub length: u64,
    pub uncompressed_length: u64,
    pub sha256: [u8; 32],
}

/// What [`StreamingWriter::finish`] committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishStats {
    pub entries: u64,
    pub redirects: u64,
    pub blob_region_length: u64,
    pub index_length: u64,
}

enum HeaderPlan {
    Fresh {
        archive_uuid: [u8; 16],
        created_at: u64,
        flags: u16,
    },
    Append {
        old: WaxHeader,
        created_at: u64,
    },
}

/// An archive being written. See the module docs.
pub struct StreamingWriter {
    file: File,
    cursor: u64,
    blob_region_offset: u64,
    index: IndexBuilder,
    header: HeaderPlan,
    segment_index: u64,
    prev_segment: Option<(u64, u64)>,
    created_at: u64,
    /// Paths in earlier segments (append only): valid redirect targets.
    external_paths: Option<BTreeSet<String>>,
    entries: u64,
    redirects: u64,
}

impl StreamingWriter {
    /// Stream one content entry. Bytes are read from `src` in
    /// [`STREAM_BUF`]-sized chunks, hashed (SHA-256 over the uncompressed
    /// content, SPEC §3.1) and compressed in the same pass, and written
    /// directly to the archive. The index row is inserted immediately.
    pub fn add_entry(&mut self, meta: EntryMeta, src: &mut dyn Read) -> Result<EntryStats> {
        let path = normalize_path(&meta.path)?;
        let codec = meta.compression.unwrap_or(Compression::Zstd);
        let offset = self.cursor;

        let mut hasher = Sha256::new();
        let mut uncompressed: u64 = 0;
        let mut buf = vec![0u8; STREAM_BUF];
        let mut sink = CountingWriter {
            inner: &mut self.file,
            written: 0,
        };

        match codec {
            Compression::None => loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                sink.write_all(&buf[..n])?;
                uncompressed += n as u64;
            },
            Compression::Zstd => {
                let mut enc = zstd::stream::write::Encoder::new(&mut sink, 3)?;
                loop {
                    let n = src.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                    enc.write_all(&buf[..n])?;
                    uncompressed += n as u64;
                }
                enc.finish()?;
            }
        }
        let length = sink.written;
        self.cursor += length;
        let sha256: [u8; 32] = hasher.finalize().into();

        self.index.insert(&SegRow {
            path,
            title: meta.title,
            offset: offset as i64,
            length: length as i64,
            uncompressed_length: uncompressed as i64,
            mime: meta.mime,
            compression: codec.as_str().to_string(),
            sha256: Some(sha256.to_vec()),
            volume_id: 0,
            redirect_to: None,
        })?;
        self.entries += 1;
        Ok(EntryStats {
            offset,
            length,
            uncompressed_length: uncompressed,
            sha256,
        })
    }

    /// Add a redirect entry. Carries no blob. The declared target may itself be
    /// a redirect; chains are flattened to one hop at [`finish`](Self::finish).
    pub fn add_redirect(
        &mut self,
        path: impl Into<String>,
        to: impl Into<String>,
        title: Option<String>,
    ) -> Result<()> {
        let path = normalize_path(&path.into())?;
        let to = normalize_path(&to.into())?;
        self.index.insert(&SegRow {
            path,
            title,
            offset: 0,
            length: 0,
            uncompressed_length: 0,
            mime: None,
            compression: "none".to_string(),
            sha256: None,
            volume_id: 0,
            redirect_to: Some(to),
        })?;
        self.redirects += 1;
        Ok(())
    }

    /// Add already-materialized entries (the `Vec` API's bridge).
    pub fn add_all(&mut self, entries: Vec<EntryInput>) -> Result<()> {
        for e in entries {
            match e.content {
                EntryContent::Data { bytes, compression } => {
                    let meta = EntryMeta {
                        path: e.path,
                        mime: e.mime,
                        title: e.title,
                        compression: Some(compression),
                    };
                    self.add_entry(meta, &mut io::Cursor::new(bytes))?;
                }
                EntryContent::Redirect { to } => {
                    self.add_redirect(e.path, to, e.title)?;
                }
            }
        }
        Ok(())
    }

    /// True if `path` (normalized) has been added to this segment. A point
    /// lookup in the index database; no path set is held in memory.
    pub fn has_path(&self, path: &str) -> Result<bool> {
        let p = normalize_path(path)?;
        self.index.has_path(&p)
    }

    /// True if `path` names a content entry, a redirect resolving to one, or
    /// (on append) a path in an earlier segment.
    pub fn resolves(&self, path: &str) -> Result<bool> {
        if self.has_path(path)? {
            return Ok(true);
        }
        Ok(self
            .external_paths
            .as_ref()
            .is_some_and(|s| s.contains(normalize_path(path).unwrap_or_default().as_str())))
    }

    pub fn entries_added(&self) -> u64 {
        self.entries
    }
    pub fn redirects_added(&self) -> u64 {
        self.redirects
    }

    /// Flatten redirects, seal the index, append it, and commit the header —
    /// the SPEC §7.1 sequence.
    pub fn finish(mut self) -> Result<FinishStats> {
        let blob_region_length = self.cursor - self.blob_region_offset;

        // Redirect flattening + validation (SPEC §6.2), entirely in SQL.
        self.index
            .flatten_redirects(self.external_paths.as_ref())?;

        // segment_meta (SPEC §4.2)
        let mut meta: Vec<(String, String)> = vec![
            ("format".into(), SEGMENT_FORMAT_TAG.into()),
            ("segment_index".into(), self.segment_index.to_string()),
            ("blob_region_offset".into(), self.blob_region_offset.to_string()),
            ("blob_region_length".into(), blob_region_length.to_string()),
            ("created_at".into(), (self.created_at as i64).to_string()),
        ];
        if let Some((po, pl)) = self.prev_segment {
            meta.push(("prev_segment_offset".into(), po.to_string()));
            meta.push(("prev_segment_length".into(), pl.to_string()));
        }
        self.index.write_meta(&meta)?;

        // Seal and stream the index database into the archive.
        let index_offset = self.cursor;
        let mut db = self.index.seal()?;
        let index_length = io::copy(&mut db, &mut self.file)?;
        self.cursor += index_length;

        // fsync, then overwrite the header in place (SPEC §7.1 steps 3–5).
        self.file.sync_all()?;
        let header = match self.header {
            HeaderPlan::Fresh {
                archive_uuid,
                created_at,
                flags,
            } => WaxHeader {
                version_major: FORMAT_VERSION_MAJOR,
                version_minor: FORMAT_VERSION_MINOR,
                flags,
                archive_uuid,
                created_at,
                index_offset,
                index_length,
                blob_section_length: blob_region_length,
                search_index_offset: 0,
                search_index_length: 0,
            },
            HeaderPlan::Append { old, created_at } => WaxHeader {
                version_major: old.version_major,
                version_minor: old.version_minor,
                flags: old.flags,
                archive_uuid: old.archive_uuid,
                created_at,
                index_offset,
                index_length,
                blob_section_length: old.blob_section_length + blob_region_length,
                search_index_offset: old.search_index_offset,
                search_index_length: old.search_index_length,
            },
        };
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header.to_bytes())?;
        self.file.sync_all()?;

        Ok(FinishStats {
            entries: self.entries,
            redirects: self.redirects,
            blob_region_length,
            index_length,
        })
    }
}

struct CountingWriter<W: Write> {
    inner: W,
    written: u64,
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

// ---------------------------------------------------------------------------
// The index segment, built incrementally on disk
// ---------------------------------------------------------------------------

/// A segment's SQLite database under construction. Rows go in as they arrive
/// inside one transaction; SQLite spills dirty pages to the temp file, so the
/// working set is its page cache, not the row count.
struct IndexBuilder {
    tmp: NamedTempFile,
    conn: Option<Connection>,
}

const SCHEMA: &str = "PRAGMA page_size=4096;
     PRAGMA journal_mode=OFF;
     PRAGMA synchronous=OFF;
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
     );";

impl IndexBuilder {
    fn open(manifest: Option<&BTreeMap<String, String>>) -> Result<Self> {
        let tmp = tempfile::Builder::new().suffix(".db").tempfile()?;
        let conn = Connection::open(tmp.path())?;
        conn.execute_batch(SCHEMA)?;
        if let Some(m) = manifest {
            conn.execute("CREATE TABLE manifest (key TEXT PRIMARY KEY, value TEXT)", [])?;
            let mut stmt = conn.prepare("INSERT INTO manifest (key, value) VALUES (?1, ?2)")?;
            for (k, v) in m {
                stmt.execute(rusqlite::params![k, v])?;
            }
        }
        conn.execute_batch("BEGIN")?;
        Ok(IndexBuilder {
            tmp,
            conn: Some(conn),
        })
    }

    fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("index not yet sealed")
    }

    fn insert(&self, r: &SegRow) -> Result<()> {
        let res = self.conn().execute(
            "INSERT INTO entries
             (path, title, offset, length, uncompressed_length, mime, compression, sha256, volume_id, redirect_to)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            rusqlite::params![
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
            ],
        );
        match res {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(WaxError::Schema {
                    detail: format!("duplicate path after normalization: {:?}", r.path),
                })
            }
            Err(e) => Err(e.into()),
        }
    }

    fn has_path(&self, path: &str) -> Result<bool> {
        Ok(self
            .conn()
            .query_row("SELECT 1 FROM entries WHERE path = ?1", [path], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// SPEC §6.2 at finish: every redirect ends up pointing at a non-redirect.
    ///
    /// Pointer jumping: each round replaces `r.redirect_to` with its target's
    /// `redirect_to` wherever the target is itself a redirect, so chain length
    /// halves per round. Anything still unresolved after [`FLATTEN_ROUNDS`]
    /// is a cycle. Then a terminus that is neither in this segment nor in
    /// `external` is dangling. Both are the same errors the in-memory path
    /// produced, so callers see no difference.
    fn flatten_redirects(&self, external: Option<&BTreeSet<String>>) -> Result<()> {
        let conn = self.conn();
        conn.execute_batch("COMMIT; BEGIN;")?;
        // A temporary index on redirect_to keeps each round to an index probe
        // per redirect. Dropped before VACUUM so the on-disk bytes are
        // identical to a build that never needed it.
        conn.execute_batch("CREATE INDEX tmp_redirect_idx ON entries(redirect_to)")?;

        let mut resolved = false;
        for _ in 0..FLATTEN_ROUNDS {
            let changed = conn.execute(
                "UPDATE entries SET redirect_to = (
                     SELECT t.redirect_to FROM entries t WHERE t.path = entries.redirect_to
                 )
                 WHERE redirect_to IS NOT NULL
                   AND EXISTS (SELECT 1 FROM entries t
                               WHERE t.path = entries.redirect_to AND t.redirect_to IS NOT NULL)",
                [],
            )?;
            if changed == 0 {
                resolved = true;
                break;
            }
        }
        if !resolved {
            // still pointing at a redirect after the cap: a cycle
            let (from, via): (String, String) = conn.query_row(
                "SELECT r.path, r.redirect_to FROM entries r
                 JOIN entries t ON t.path = r.redirect_to
                 WHERE r.redirect_to IS NOT NULL AND t.redirect_to IS NOT NULL
                 ORDER BY r.path LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            return Err(WaxError::RedirectChainTooDeep { from, via });
        }

        // Dangling: terminus not in this segment. Check external paths for the
        // survivors (append case).
        let mut stmt = conn.prepare(
            "SELECT r.path, r.redirect_to FROM entries r
             WHERE r.redirect_to IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM entries t WHERE t.path = r.redirect_to)
             ORDER BY r.path",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        for row in rows {
            let (from, to) = row?;
            if !external.is_some_and(|s| s.contains(&to)) {
                return Err(WaxError::DanglingRedirect { from, to });
            }
        }
        drop(stmt);
        conn.execute_batch("DROP INDEX tmp_redirect_idx")?;
        Ok(())
    }

    fn write_meta(&self, meta: &[(String, String)]) -> Result<()> {
        let mut stmt = self
            .conn()
            .prepare("INSERT INTO segment_meta (key, value) VALUES (?1, ?2)")?;
        for (k, v) in meta {
            stmt.execute(rusqlite::params![k, v])?;
        }
        Ok(())
    }

    /// Commit, VACUUM (canonical page layout → deterministic bytes), close,
    /// and hand back the database file for streaming into the archive.
    fn seal(&mut self) -> Result<File> {
        let conn = self.conn.take().expect("sealed twice");
        conn.execute_batch("COMMIT")?;
        conn.execute("VACUUM", [])?;
        conn.close().map_err(|(_, e)| e)?;
        Ok(File::open(self.tmp.path())?)
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Path normalization (SPEC §6.1)
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

// ---------------------------------------------------------------------------
// Segment construction for fixtures and tests
// ---------------------------------------------------------------------------

/// A single row destined for `entries`. `pub` so fixtures/tests can craft
/// arbitrary (including deliberately malformed) segments via
/// [`build_segment_db_with_meta`].
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
    let mut ib = IndexBuilder::open(manifest)?;
    for r in rows {
        ib.insert(r)?;
    }
    ib.write_meta(meta)?;
    let mut db = ib.seal()?;
    let mut bytes = Vec::new();
    db.read_to_end(&mut bytes)?;
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
