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
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use tempfile::NamedTempFile;

/// Read/compress buffer. Bounds the per-entry working memory.
const STREAM_BUF: usize = 64 * 1024;

/// The tokenizer a pack is indexed with unless it declares another
/// (SPEC §5.7). `unicode61` is built into every SQLite with FTS5, so the query
/// side can reproduce it on every tier -- which is the actual constraint. It
/// does not segment CJK; a CJK pack declares `trigram`, also built in, which is
/// exactly why the tokenizer is per-pack rather than fixed by the format.
pub const DEFAULT_TOKENIZER: &str = "unicode61";

/// `segment_meta.manifest_update` value: this segment's `manifest` table
/// replaces the pack's manifest whole (SPEC v0.13 §5.6.1).
pub const MANIFEST_UPDATE_REPLACE: &str = "replace";

/// Tokenizers this build can reproduce on the query side. A pack declaring
/// anything else is refused at query time rather than answered wrongly.
pub const SUPPORTED_TOKENIZERS: [&str; 4] = [
    "unicode61",
    "trigram",
    "porter unicode61",
    crate::tokenize::WAX_BIGRAM,
];

/// Shared-dictionary size, SPEC v0.10. 110 KiB is the size Directive 04
/// measured the codec decision against; it is a knob, not a format value.
const DICT_BYTES: usize = 110 * 1024;

/// Below this, zstd cannot train anything useful and silently returns a poor
/// dictionary, so refuse rather than ship one that helps nothing.
const MIN_DICT_SAMPLES: usize = 8;

/// zstd level a writer uses unless told otherwise. Kept at 3 so that existing
/// builds stay byte-identical; ADR-0003 measured level 19 as 16% smaller text
/// with no decode cost and recommends it for off-box builds
/// ([`WaxWriter::zstd_level`]).
pub const DEFAULT_ZSTD_LEVEL: i32 = 3;

/// Highest level a writer accepts. zstd's levels 20–22 ("ultra") raise the
/// window past 8 MiB, which SPEC §3's decoder memory bound forbids; levels up
/// to 19 comply (ADR-0003, `tests/window_bound.rs`).
pub const MAX_ZSTD_LEVEL: i32 = 19;

/// Pointer-jumping iterations before a redirect chain is declared cyclic.
/// Each iteration halves the remaining chain length, so this bounds legitimate
/// depth at 2^64; anything still unresolved is a cycle.
const FLATTEN_ROUNDS: usize = 64;

/// Archive-level configuration: identity, timestamps, header flags.
pub struct WaxWriter {
    archive_uuid: [u8; 16],
    created_at: u64,
    flags: u16,
    zstd_level: i32,
}

impl WaxWriter {
    pub fn new(archive_uuid: [u8; 16]) -> Self {
        WaxWriter {
            archive_uuid,
            created_at: now(),
            flags: 0,
            zstd_level: DEFAULT_ZSTD_LEVEL,
        }
    }

    /// zstd level for `zstd` and `zstd-dict` entries (SPEC §3). The level is
    /// not recorded in the archive and readers need nothing new; it only
    /// changes how hard the writer works. Must be 1..=[`MAX_ZSTD_LEVEL`],
    /// checked when the writer is created or opened.
    pub fn zstd_level(mut self, level: i32) -> Self {
        self.zstd_level = level;
        self
    }

    fn checked_level(&self) -> Result<i32> {
        check_zstd_level(self.zstd_level)
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
        let zstd_level = self.checked_level()?;
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
            dictionary: None,
            search_tokenizer: None,
            pack_tokenizer: None,
            search_rows: 0,
            passage_rows: 0,
            prev_state_digest: None,
            prev_header: None,
            tombstones: 0,
            manifest_update: None,
            zstd_level,
        })
    }

    /// Start an append segment on an existing archive (SPEC §7.1). Blob bytes
    /// and the new index segment go at the end of the file; the header is
    /// rewritten in place as the final step of `finish`.
    ///
    /// Redirects added to this segment may target entries carried by earlier
    /// segments; those are checked by point lookup through a reader held open
    /// on the archive until the new segment is flattened, so append costs
    /// nothing proportional to the entries already there.
    pub fn open_append(&self, archive: impl AsRef<Path>) -> Result<StreamingWriter> {
        let zstd_level = self.checked_level()?;
        let existing = crate::reader::WaxReader::open_with(
            archive.as_ref(),
            crate::reader::ReadOptions {
                verify_checksums: false,
                lookup_cache_entries: 0,
            },
        )?;

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(archive.as_ref())?;
        let file_size = file.seek(SeekFrom::End(0))?;
        // The header the reader chose (SPEC v0.13 §2.4). If slot A was torn
        // and the mirror stands in, put the mirror back in slot A first, so
        // `prev_header` below records a real header and rollback works.
        let hbuf = existing.header_bytes();
        let old = *existing.header();
        if existing.header_source() == crate::reader::HeaderSource::Mirror {
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&hbuf)?;
            file.sync_all()?;
        }
        let next_index = read_segment_index(&mut file, old.index_offset, old.index_length)? + 1;
        // A reader refuses a chain longer than MAX_SEGMENTS (SPEC §5.1), so a
        // writer that appended past it would turn a readable pack into one
        // nobody can open. Refuse before touching the file.
        if next_index >= crate::MAX_SEGMENTS as u64 {
            return Err(WaxError::TooManySegments);
        }
        // The archive's logical end is the end of its newest index segment
        // (SPEC §7.1). Anything past it is the orphaned tail of an append that
        // crashed before its header write (§7.2). It is reclaimed here rather
        // than appended after, so the new segment's `blob_region_offset` is
        // the same on every copy of this edition, with or without that
        // garbage — which is what lets the update travel as a byte tail
        // (§7.4).
        let logical_end = old.index_offset + old.index_length;
        if file_size > logical_end {
            file.set_len(logical_end)?;
            file.sync_all()?;
        }
        let file_size = logical_end;

        // Carry the pack's dictionary forward (SPEC §5.9). This happens here,
        // not at the call site, on purpose: carry-forward is the invariant the
        // whole codec decision rests on, so it is the structural default of
        // appending rather than something every caller must remember. A caller
        // that genuinely wants a different dictionary has to say so explicitly
        // by calling `train_dictionary` after opening.
        //
        // Note this only makes the dictionary available for *encoding* new
        // entries. The table itself stays in segment 0 and is not rewritten
        // into the appended segment — see `finish`.
        let carried = existing.dictionary().map(|d| d.to_vec());

        // The state this segment extends (SPEC v0.12 §7.4): the §8.1 digest
        // of the archive exactly as it stands, recorded in the new segment so
        // a box receiving this update as a byte tail can check, before it
        // appends anything, that the tail was built against its own copy.
        let prev_state_digest = existing.signable_digest()?;
        // Search rows appended here must be built with the pack's tokenizer
        // (SPEC v0.12 §5.7): results from two tokenizers cannot be merged.
        let pack_tokenizer = existing.search_tokenizer().map(str::to_string);

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
            dictionary: carried,
            search_tokenizer: pack_tokenizer.clone(),
            pack_tokenizer,
            search_rows: 0,
            passage_rows: 0,
            prev_state_digest: Some(prev_state_digest),
            prev_header: Some(hbuf),
            tombstones: 0,
            manifest_update: None,
            zstd_level,
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
    /// The archive as it stands (append only): earlier segments' paths are
    /// valid redirect targets, resolved by point lookup.
    external_paths: Option<crate::reader::WaxReader>,
    entries: u64,
    redirects: u64,
    /// The pack's shared dictionary (SPEC v0.10 §3.1), stored once in
    /// segment 0. `None` until trained or carried forward.
    dictionary: Option<Vec<u8>>,
    /// Declared search tokenizer (SPEC §5.7). `None` until the first indexed
    /// entry, at which point the default is recorded.
    search_tokenizer: Option<String>,
    /// The tokenizer an earlier segment of this pack already declares (append
    /// only). An appended index must use the same one (SPEC v0.12 §5.7).
    pack_tokenizer: Option<String>,
    /// Documents indexed so far; doubles as the next FTS rowid.
    search_rows: u64,
    /// Entries given a `passages` row in this segment (SPEC v0.13 §5.10).
    passage_rows: u64,
    /// The §8.1 digest of the archive this segment is appended to (SPEC
    /// v0.12 §7.4). `None` for a fresh build.
    prev_state_digest: Option<[u8; 32]>,
    /// The raw header this append will overwrite (SPEC v0.12 §7.5).
    prev_header: Option<[u8; HEADER_LEN]>,
    /// Tombstones added to this segment (SPEC v0.12 §5.5.1).
    tombstones: u64,
    /// A replacement manifest (SPEC v0.13 §5.6.1), written at `finish`.
    manifest_update: Option<BTreeMap<String, String>>,
    /// zstd level for compressed entries (SPEC §3; ADR-0003).
    zstd_level: i32,
}

impl StreamingWriter {
    /// Train this pack's shared dictionary from a sample of its own content.
    ///
    /// **Sample order is part of the result.** zstd's trainer is deterministic
    /// for a given input *sequence* but not order-independent, so two builds
    /// agree only if they present the same samples in the same order. Callers
    /// must therefore iterate something stable — entry path order, not
    /// filesystem or insertion order. That is the whole basis of the v0.10
    /// determinism guarantee, and it is why this takes an ordered iterator
    /// rather than a set.
    ///
    /// Prefer [`carry_dictionary`](Self::carry_dictionary) for any edition
    /// after the first: retraining reframes every entry (SPEC §3.1).
    pub fn train_dictionary<'a, I>(&mut self, samples: I) -> Result<()>
    where
        I: IntoIterator<Item = &'a [u8]>,
    {
        let owned: Vec<Vec<u8>> = samples
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_vec())
            .collect();
        if owned.len() < MIN_DICT_SAMPLES {
            return Err(WaxError::Dictionary {
                detail: format!(
                    "training needs at least {MIN_DICT_SAMPLES} non-empty samples, got {}",
                    owned.len()
                ),
            });
        }
        let dict = zstd::dict::from_samples(&owned, DICT_BYTES)
            .map_err(|e| WaxError::Dictionary { detail: format!("training failed: {e}") })?;
        self.dictionary = Some(dict);
        Ok(())
    }

    /// Carry a dictionary forward from a previous edition, verbatim.
    ///
    /// This is the default for every edition after the first. An entry whose
    /// bytes did not change then frames to the identical stored bytes, so a
    /// delta ships only what actually changed — the property Directive 04
    /// measured at 16–59x better than grouped framing at realistic churn.
    /// Retraining instead would move every frame.
    pub fn carry_dictionary(&mut self, dict: &[u8]) -> Result<()> {
        if dict.is_empty() {
            return Err(WaxError::Dictionary {
                detail: "refusing to carry an empty dictionary forward".into(),
            });
        }
        self.dictionary = Some(dict.to_vec());
        Ok(())
    }

    /// The dictionary this pack will store, if any.
    pub fn dictionary(&self) -> Option<&[u8]> {
        self.dictionary.as_deref()
    }

    /// The zstd level compressed entries are written at.
    pub fn zstd_level(&self) -> i32 {
        self.zstd_level
    }

    /// Change the zstd level for entries added from now on (1..=19). Entries
    /// already written keep theirs; a level is never recorded, so mixing is
    /// valid, but a reproducible build sets it once, before the first entry.
    pub fn set_zstd_level(&mut self, level: i32) -> Result<()> {
        self.zstd_level = check_zstd_level(level)?;
        Ok(())
    }

    /// Declare the tokenizer this pack's search index is built with
    /// (SPEC §5.7). Defaults to [`DEFAULT_TOKENIZER`].
    ///
    /// Recorded so a reader can **refuse** an index it cannot reproduce rather
    /// than answering a query with confidently wrong results. Must be set
    /// before the first [`index_text`](Self::index_text).
    pub fn set_search_tokenizer(&mut self, name: &str) -> Result<()> {
        if self.search_rows > 0 {
            return Err(WaxError::Schema {
                detail: "the search tokenizer must be declared before the first indexed entry"
                    .into(),
            });
        }
        if let Some(pack) = &self.pack_tokenizer {
            if pack != name {
                return Err(WaxError::TokenizerMismatch {
                    first: pack.clone(),
                    other: name.to_string(),
                });
            }
        }
        self.search_tokenizer = Some(name.to_string());
        Ok(())
    }

    /// Add one document to the pack's embedded search index (SPEC §5.7).
    ///
    /// `body` is the *rendered text* to index, not markup — the caller decides
    /// what searchable means, exactly as it decides what to store. The index is
    /// contentless: it holds no copy of the text, only the terms, so indexing
    /// does not double the pack.
    ///
    /// Any segment may carry an index (SPEC v0.12 §5.7), but only for the
    /// entries **that segment itself** adds or replaces: `path` must be added
    /// to this writer as content or a redirect, which `finish` checks. On an
    /// append, index every new or changed document again; a reader drops
    /// older segments' rows for paths a newer segment owns.
    pub fn index_text(&mut self, path: &str, title: Option<&str>, body: &str) -> Result<()> {
        let path = &normalize_path(path)?;
        let tok = self
            .search_tokenizer
            .clone()
            .unwrap_or_else(|| DEFAULT_TOKENIZER.to_string());
        self.index.ensure_search(&tok)?;
        self.search_rows += 1;
        // `wax-bigram-16` rewrites the text before SQLite sees it (SPEC
        // v0.13 §5.7.2); `search_docs.title` keeps the title as written.
        let (fts_title, fts_body) = if tok == crate::tokenize::WAX_BIGRAM {
            (
                crate::tokenize::prepare_document(title.unwrap_or("")),
                crate::tokenize::prepare_document(body),
            )
        } else {
            (title.unwrap_or("").to_string(), body.to_string())
        };
        self.index
            .index_text(self.search_rows as i64, path, title, &fts_title, &fts_body)?;
        Ok(())
    }

    /// Re-add one document from the exact terms an earlier index holds for
    /// it, bypassing any rewriting (compaction, SPEC v0.13 §7.6). `terms`
    /// are already tokens of the pack's tokenizer.
    pub(crate) fn index_terms(&mut self, path: &str, title: Option<&str>, terms: &str) -> Result<()> {
        let path = &normalize_path(path)?;
        let tok = self
            .search_tokenizer
            .clone()
            .unwrap_or_else(|| DEFAULT_TOKENIZER.to_string());
        self.index.ensure_search(&tok)?;
        self.search_rows += 1;
        self.index
            .index_text(self.search_rows as i64, path, title, "", terms)?;
        Ok(())
    }

    /// How many documents have been indexed.
    pub fn indexed_count(&self) -> u64 {
        self.search_rows
    }

    /// Record the passages of `path` (SPEC v0.13 §5.10): `content` is the
    /// entry's uncompressed bytes, exactly as stored, and is cut with
    /// `html-headings-1`. `path` must be content this segment adds, which
    /// `finish` checks. Returns the number of passages.
    pub fn index_passages(&mut self, path: &str, content: &[u8]) -> Result<usize> {
        let starts = crate::passage::html_heading_starts(content);
        self.add_passage_starts(path, &crate::passage::encode_starts(&starts))?;
        Ok(starts.len())
    }

    /// Store an already-encoded `passages.starts` value for `path`
    /// (compaction carries them over unchanged).
    pub(crate) fn add_passage_starts(&mut self, path: &str, starts: &[u8]) -> Result<()> {
        let path = normalize_path(path)?;
        self.index.insert_passages(&path, starts)?;
        self.passage_rows += 1;
        Ok(())
    }

    /// Entries given passages in this segment so far.
    pub fn passage_count(&self) -> u64 {
        self.passage_rows
    }

    /// Replace the pack's manifest (SPEC v0.13 §5.6.1). On an append the new
    /// segment carries the whole new manifest and declares
    /// `manifest_update = "replace"`, so a reader shows it instead of the
    /// base segment's; every older segment, the dictionary and the identity
    /// are untouched, and rolling the update back (§7.5) restores the old
    /// manifest with it. On a fresh build it simply sets the manifest.
    ///
    /// The format treats the manifest as opaque: which keys may change, and
    /// how a `version` must move, is the builder's check, not this one.
    pub fn set_manifest(&mut self, manifest: &BTreeMap<String, String>) {
        self.manifest_update = Some(manifest.clone());
    }

    /// Delete `path` from the pack (SPEC v0.12 §5.5.1): a zero-length row
    /// whose `compression` is `tombstone`. Last-segment-wins does the rest —
    /// a reader treats the path as gone, drops it from listings and from
    /// search, and refuses reads of it with `EntryDeleted`. A reader that
    /// predates v0.12 refuses the read with `UnknownCompression`, so no
    /// reader ever serves the deleted content.
    ///
    /// Append only: a fresh build has nothing to delete. The path must exist
    /// in the archive being appended to, and must not also be added to this
    /// segment.
    pub fn add_tombstone(&mut self, path: &str) -> Result<()> {
        let path = normalize_path(path)?;
        let Some(existing) = &self.external_paths else {
            return Err(WaxError::Schema {
                detail: format!(
                    "cannot delete {path:?}: tombstones only exist in appended segments \
                     (SPEC v0.12 §5.5.1); a fresh build simply omits the path"
                ),
            });
        };
        if !existing.contains(&path)? {
            return Err(WaxError::EntryNotFound(path));
        }
        self.index.insert(&SegRow {
            path,
            title: None,
            offset: 0,
            length: 0,
            uncompressed_length: 0,
            mime: None,
            compression: crate::model::TOMBSTONE.to_string(),
            sha256: None,
            volume_id: 0,
            redirect_to: None,
        })?;
        self.tombstones += 1;
        Ok(())
    }

    /// Tombstones added to this segment so far.
    pub fn tombstones_added(&self) -> u64 {
        self.tombstones
    }
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
                let mut enc = zstd::stream::write::Encoder::new(&mut sink, self.zstd_level)?;
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
            Compression::ZstdDict => {
                let dict = self
                    .dictionary
                    .as_deref()
                    .ok_or_else(|| WaxError::DictionaryMissing { path: path.clone() })?;
                let mut enc = zstd::stream::write::Encoder::with_dictionary(&mut sink, self.zstd_level, dict)?;
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

    /// Copy an entry's stored bytes as they are, already encoded with
    /// `entry.compression` (compaction, SPEC v0.13 §7.6). The caller vouches
    /// for `sha256` and `uncompressed_length`; nothing is re-encoded, which
    /// is what keeps a compacted pack's frames identical to the original's.
    pub(crate) fn add_raw_entry(&mut self, entry: &crate::model::Entry, src: &mut dyn Read) -> Result<()> {
        let path = normalize_path(&entry.path)?;
        if entry.compression == Compression::ZstdDict.as_str() && self.dictionary.is_none() {
            return Err(WaxError::DictionaryMissing { path });
        }
        let offset = self.cursor;
        let written = io::copy(&mut src.take(entry.length), &mut self.file)?;
        if written != entry.length {
            return Err(WaxError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("entry {path:?} ends past the end of the archive"),
            )));
        }
        self.cursor += written;
        self.index.insert(&SegRow {
            path,
            title: entry.title.clone(),
            offset: offset as i64,
            length: entry.length as i64,
            uncompressed_length: entry.uncompressed_length as i64,
            mime: entry.mime.clone(),
            compression: entry.compression.clone(),
            sha256: entry.sha256.map(|h| h.to_vec()),
            volume_id: 0,
            redirect_to: None,
        })?;
        self.entries += 1;
        Ok(())
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

    /// True if `path` (normalized) has been added to this segment as content
    /// or a redirect. A point lookup in the index database; no path set is
    /// held in memory. A tombstone added here does not count.
    pub fn has_path(&self, path: &str) -> Result<bool> {
        let p = normalize_path(path)?;
        self.index.has_live_path(&p)
    }

    /// True if `path` names a content entry, a redirect resolving to one, or
    /// (on append) a path in an earlier segment.
    pub fn resolves(&self, path: &str) -> Result<bool> {
        if self.has_path(path)? {
            return Ok(true);
        }
        if self.index.is_tombstoned(&normalize_path(path)?)? {
            return Ok(false);
        }
        match &self.external_paths {
            Some(r) => r.contains(&normalize_path(path).unwrap_or_default()),
            None => Ok(false),
        }
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

        // Deleting a path deletes its aliases (SPEC v0.12 §5.5.1): every
        // earlier redirect onto a path this segment tombstones gets its own
        // tombstone, unless this segment re-adds that alias. One streaming
        // pass over the existing merged index, and only when something was
        // deleted — a publisher-side cost, never a reader's.
        if self.tombstones > 0 {
            if let Some(existing) = &self.external_paths {
                let mut orphaned = Vec::new();
                for e in existing.entries() {
                    let e = e?;
                    if let Some(to) = &e.redirect_to {
                        if self.index.is_tombstoned(to)? && !self.index.has_row(&e.path)? {
                            orphaned.push(e.path);
                        }
                    }
                }
                for alias in orphaned {
                    self.index.insert(&SegRow {
                        path: alias,
                        title: None,
                        offset: 0,
                        length: 0,
                        uncompressed_length: 0,
                        mime: None,
                        compression: crate::model::TOMBSTONE.to_string(),
                        sha256: None,
                        volume_id: 0,
                        redirect_to: None,
                    })?;
                    self.tombstones += 1;
                }
            }
        }

        // Redirect flattening + validation (SPEC §6.2), entirely in SQL.
        self.index
            .flatten_redirects(self.external_paths.as_ref())?;

        // The merged live count after this segment (SPEC v0.12 §4.2), so no
        // reader ever walks the chain to count. One point lookup per row this
        // segment writes, against the prior state; the prior count comes from
        // the prior segment's own key when it has one.
        let live_entry_count = match &self.external_paths {
            None => self.index.row_count()?,
            Some(existing) => {
                let mut n = existing.entry_count()? as i64;
                self.index.for_each_row(|path, is_tombstone| {
                    let was = existing.contains(path)?;
                    n += (!is_tombstone) as i64 - was as i64;
                    Ok(())
                })?;
                n as u64
            }
        };
        // The reader on the prior segments is no longer needed; drop it
        // before the file is extended and the header rewritten.
        self.external_paths = None;

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
        if let Some(d) = self.prev_state_digest {
            meta.push(("prev_state_digest".into(), hex(&d)));
        }
        if let Some(h) = self.prev_header {
            meta.push(("prev_header".into(), hex(&h)));
        }
        meta.push(("live_entry_count".into(), live_entry_count.to_string()));
        if let Some(m) = self.manifest_update.take() {
            self.index.write_manifest(&m)?;
            if self.segment_index > 0 {
                meta.push(("manifest_update".into(), MANIFEST_UPDATE_REPLACE.into()));
            }
        }
        self.index.write_meta(&meta)?;

        // The shared dictionary lives in the index segment, not the blob region,
        // and that is deliberate: the signing digest covers header + index-chain
        // (SPEC section 8), so storing it here puts it inside the signature.
        // Every entry decoding depends on it, so an unsigned dictionary would let
        // a swap corrupt all content while the signature still verified.
        //
        // Segment 0 only, the same placement rule as `manifest` (SPEC §5.9).
        // An append carries the dictionary forward for *encoding* its new
        // entries, but must not rewrite the table into its own segment —
        // a reader would ignore it there anyway, and duplicating it would
        // grow every append by ~110 KB for nothing.
        // Search segment (SPEC v0.12 §5.7), in whichever segment indexed
        // anything. The declared tokenizer and title fold are recorded even
        // when they are the defaults, so a reader never has to assume.
        if self.search_rows > 0 {
            self.index.check_search_paths()?;
            let tok = self
                .search_tokenizer
                .clone()
                .unwrap_or_else(|| DEFAULT_TOKENIZER.to_string());
            self.index.write_search_meta(&tok)?;
            self.index.optimize_search()?;
        }

        // Passage table (SPEC v0.13 §5.10): rows only for this segment's own
        // content entries, starts inside each entry, rule declared.
        if self.passage_rows > 0 {
            self.index.check_passage_rows()?;
            self.index.write_meta(&[(
                "passage_rule".to_string(),
                crate::passage::RULE_HTML_HEADINGS.to_string(),
            )])?;
        }

        if let (Some(d), 0) = (self.dictionary.as_deref(), self.segment_index) {
            self.index.write_dictionary(d)?;
        }

        // Seal and stream the index database into the archive.
        let index_offset = self.cursor;
        let mut db = self.index.seal()?;
        let index_length = io::copy(&mut db, &mut self.file)?;
        self.cursor += index_length;

        // fsync the tail (SPEC §7.1 step 3).
        self.file.sync_all()?;
        let header = match self.header {
            HeaderPlan::Fresh {
                archive_uuid,
                created_at,
                flags,
            } => WaxHeader {
                version_major: FORMAT_VERSION_MAJOR,
                version_minor: FORMAT_VERSION_MINOR,
                flags: flags | crate::header::flag::HAS_HEADER_CHECK,
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
                // The header names the newest minor any segment may use
                // (SPEC v0.12 §10): an append by this writer can add
                // tombstones or a search segment, so it never lowers it.
                version_minor: old.version_minor.max(FORMAT_VERSION_MINOR),
                flags: old.flags | crate::header::flag::HAS_HEADER_CHECK,
                archive_uuid: old.archive_uuid,
                created_at,
                index_offset,
                index_length,
                blob_section_length: old.blob_section_length + blob_region_length,
                search_index_offset: old.search_index_offset,
                search_index_length: old.search_index_length,
            },
        };
        // The mirror first (SPEC v0.13 §2.4): a copy of the new header
        // after the newest segment, synced before slot A is touched, so a
        // header write torn by power loss can be recovered from it.
        let raw = header.to_bytes();
        self.file.write_all(&raw)?;
        self.file.sync_all()?;
        // Then slot A, in place (SPEC §7.1 steps 4–5).
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&raw)?;
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

/// SPEC §5.2. `entries` is `WITHOUT ROWID` so the `path` primary key is the
/// table itself: one B-tree descent per lookup, sequential pages per ordered
/// walk (SPEC §12.23 has the numbers). Readers accept the older rowid layout.
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
        redirect_to TEXT DEFAULT NULL,
        passages BLOB DEFAULT NULL
     ) WITHOUT ROWID;
     CREATE TABLE segment_meta (key TEXT PRIMARY KEY, value TEXT);
     CREATE TABLE signatures (
        signer TEXT, algo TEXT, signature BLOB, signed_at INTEGER
     );";

impl IndexBuilder {
    /// Create the search tables once, on first use (SPEC §5.7).
    ///
    /// The FTS table is **contentless** (`content=''`) and `detail=none`.
    /// Both are size decisions, measured rather than assumed: over real pack
    /// content a content-storing index cost 0.659x the indexed text and a
    /// contentless one with full detail 0.430x, against 0.241x here. A
    /// content-storing index would also put a second copy of every article in
    /// the pack, which is the one thing a format that just won on size cannot
    /// afford. The cost is that phrase queries and snippets need the v2
    /// stand-off segment; term and boolean queries work.
    fn ensure_search(&self, tokenizer: &str) -> Result<()> {
        let conn = self.conn.as_ref().expect("connection open");
        let exists = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='search_index'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            return Ok(());
        }
        // The tokenizer name reaches SQL, so admit only the forms this build
        // can reproduce on the query side; anything else is stored as a
        // declaration and the index is built with the default.
        let tok = if SUPPORTED_TOKENIZERS.contains(&tokenizer) {
            crate::tokenize::sqlite_tokenize_arg(tokenizer)
        } else {
            crate::tokenize::sqlite_tokenize_arg(DEFAULT_TOKENIZER)
        };
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE search_index USING fts5(
                 title, body, content='', detail=none, tokenize={tok}
             );
             CREATE TABLE search_docs (
                 id         INTEGER PRIMARY KEY,
                 path       TEXT NOT NULL,
                 title      TEXT,
                 title_fold TEXT
             );
             CREATE INDEX search_docs_fold ON search_docs(title_fold);"
        ))?;
        Ok(())
    }

    /// Index one document. `id` is the FTS rowid and the `search_docs` key.
    fn index_text(
        &self,
        id: i64,
        path: &str,
        title: Option<&str>,
        fts_title: &str,
        fts_body: &str,
    ) -> Result<()> {
        let conn = self.conn.as_ref().expect("connection open");
        conn.execute(
            "INSERT INTO search_index(rowid, title, body) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, fts_title, fts_body],
        )?;
        conn.execute(
            "INSERT INTO search_docs(id, path, title, title_fold) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                id,
                path,
                title,
                title.and_then(|t| crate::fold::fold_title(crate::fold::TITLE_FOLD_NFKC_CF, t))
            ],
        )?;
        Ok(())
    }

    /// Merge the FTS b-tree into one segment. Smaller on disk, and -- the
    /// reason it is not optional here -- deterministic: an unoptimised index
    /// carries whatever segment structure the insert order happened to produce.
    fn optimize_search(&self) -> Result<()> {
        let conn = self.conn.as_ref().expect("connection open");
        conn.execute_batch("INSERT INTO search_index(search_index) VALUES ('optimize')")?;
        Ok(())
    }

    /// Record the declared tokenizer so a reader can refuse what it cannot
    /// reproduce (SPEC §5.7).
    fn write_search_meta(&self, tokenizer: &str) -> Result<()> {
        let conn = self.conn.as_ref().expect("connection open");
        conn.execute(
            "INSERT OR REPLACE INTO segment_meta(key, value) VALUES ('search_tokenizer', ?1)",
            rusqlite::params![tokenizer],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO segment_meta(key, value) VALUES ('title_fold', ?1)",
            rusqlite::params![crate::fold::TITLE_FOLD_NFKC_CF],
        )?;
        Ok(())
    }

    /// Store the pack shared dictionary (SPEC v0.10 section 3.1). One row, one
    /// blob, segment 0 only -- the same placement rule as `manifest`.
    fn write_dictionary(&self, bytes: &[u8]) -> Result<()> {
        let conn = self.conn.as_ref().expect("connection open");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS dictionary (id INTEGER PRIMARY KEY, bytes BLOB NOT NULL)",
            [],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO dictionary (id, bytes) VALUES (0, ?1)",
            rusqlite::params![bytes],
        )?;
        Ok(())
    }

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

    fn row_count(&self) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM entries WHERE compression IS NOT 'tombstone' OR redirect_to IS NOT NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    /// Stream every row of this segment as (path, is_tombstone).
    fn for_each_row(&self, mut f: impl FnMut(&str, bool) -> Result<()>) -> Result<()> {
        let mut stmt = self.conn().prepare(
            "SELECT path, (compression = 'tombstone' AND redirect_to IS NULL) FROM entries",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let path: String = r.get(0)?;
            let tomb: bool = r.get(1)?;
            f(&path, tomb)?;
        }
        Ok(())
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

    fn has_live_path(&self, path: &str) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM entries WHERE path = ?1
                   AND (redirect_to IS NOT NULL OR compression IS NOT 'tombstone')",
                [path],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    fn has_row(&self, path: &str) -> Result<bool> {
        Ok(self
            .conn()
            .query_row("SELECT 1 FROM entries WHERE path = ?1", [path], |_| Ok(()))
            .optional()?
            .is_some())
    }

    fn is_tombstoned(&self, path: &str) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM entries WHERE path = ?1
                   AND redirect_to IS NULL AND compression = 'tombstone'",
                [path],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// SPEC v0.12 §5.7: a segment's search rows describe only entries that
    /// segment itself carries. A row for any other path would be dropped by
    /// every reader (it fails the ownership test), so writing one is a bug in
    /// the caller, reported here rather than shipped.
    /// Stage passage starts for `path`. They may arrive before the entry
    /// itself, so they wait in a scratch table that `finish` moves into
    /// `entries.passages` and drops (SPEC v0.13 §5.10).
    fn insert_passages(&self, path: &str, starts: &[u8]) -> Result<()> {
        let conn = self.conn();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS passages (
                 path   TEXT PRIMARY KEY,
                 starts BLOB NOT NULL
             ) WITHOUT ROWID;",
        )?;
        let n = conn.execute(
            "INSERT OR IGNORE INTO passages (path, starts) VALUES (?1, ?2)",
            rusqlite::params![path, starts],
        )?;
        if n == 0 {
            return Err(WaxError::Schema {
                detail: format!("passages for {path:?} recorded twice in one segment"),
            });
        }
        Ok(())
    }

    /// Every staged passage row must name a content entry of this segment,
    /// and every start must lie inside it (SPEC v0.13 §5.10); then the rows
    /// move into `entries.passages`.
    fn check_passage_rows(&self) -> Result<()> {
        let mut st = self.conn().prepare(
            "SELECT p.path, p.starts, e.uncompressed_length, e.compression, e.redirect_to
               FROM passages p LEFT JOIN entries e ON e.path = p.path
              ORDER BY p.path",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let path: String = r.get(0)?;
            let blob: Vec<u8> = r.get(1)?;
            let len: Option<i64> = r.get(2)?;
            let comp: Option<String> = r.get(3)?;
            let redirect: Option<String> = r.get(4)?;
            let ok = len.is_some() && redirect.is_none() && comp.as_deref() != Some(crate::model::TOMBSTONE);
            if !ok {
                return Err(WaxError::Schema {
                    detail: format!(
                        "passages for {path:?}, which is not a content entry of this segment \
                         (SPEC v0.13 §5.10)"
                    ),
                });
            }
            crate::passage::decode_starts(&path, &blob, len.unwrap_or(0).max(0) as u64)?;
        }
        drop(rows);
        drop(st);
        self.conn().execute_batch(
            "UPDATE entries SET passages = (SELECT starts FROM passages p WHERE p.path = entries.path)
              WHERE path IN (SELECT path FROM passages);
             DROP TABLE passages;",
        )?;
        Ok(())
    }

    fn check_search_paths(&self) -> Result<()> {
        let stray: Option<String> = self
            .conn()
            .query_row(
                "SELECT d.path FROM search_docs d
                  WHERE NOT EXISTS (SELECT 1 FROM entries e WHERE e.path = d.path
                       AND (e.redirect_to IS NOT NULL OR e.compression IS NOT 'tombstone'))
                  ORDER BY d.id LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match stray {
            None => Ok(()),
            Some(path) => Err(WaxError::Schema {
                detail: format!(
                    "search row for {path:?}, which this segment does not carry; a segment \
                     indexes only its own entries (SPEC v0.12 §5.7)"
                ),
            }),
        }
    }

    /// SPEC §6.2 at finish: every redirect ends up pointing at a non-redirect.
    ///
    /// Pointer jumping: each round replaces `r.redirect_to` with its target's
    /// `redirect_to` wherever the target is itself a redirect, so chain length
    /// halves per round. Anything still unresolved after [`FLATTEN_ROUNDS`]
    /// is a cycle. Then a terminus that is neither in this segment nor in
    /// `external` (the archive being appended to) is dangling; an external
    /// terminus that is itself a redirect is replaced by its own target, so
    /// the one-hop rule (SPEC §5.3) holds across segments too.
    fn flatten_redirects(&self, external: Option<&crate::reader::WaxReader>) -> Result<()> {
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

        // A redirect in this segment onto a path this segment deletes is
        // dangling by construction (SPEC v0.12 §5.5.1).
        if let Some((from, to)) = conn
            .query_row(
                "SELECT r.path, r.redirect_to FROM entries r
                   JOIN entries t ON t.path = r.redirect_to
                  WHERE t.compression = 'tombstone' AND t.redirect_to IS NULL
                  ORDER BY r.path LIMIT 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            return Err(WaxError::DanglingRedirect { from, to });
        }

        // Dangling: terminus not in this segment. Check the archive being
        // appended to for the survivors.
        let mut stmt = conn.prepare(
            "SELECT r.path, r.redirect_to FROM entries r
             WHERE r.redirect_to IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM entries t WHERE t.path = r.redirect_to)
             ORDER BY r.path",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        let mut rehome: Vec<(String, String)> = Vec::new();
        for row in rows {
            let (from, to) = row?;
            let Some(ext) = external else {
                return Err(WaxError::DanglingRedirect { from, to });
            };
            match ext.entry(&to) {
                Ok(e) => {
                    if let Some(t) = e.redirect_to {
                        rehome.push((from, t));
                    }
                }
                Err(WaxError::EntryNotFound(_)) | Err(WaxError::EntryDeleted(_)) => {
                    return Err(WaxError::DanglingRedirect { from, to });
                }
                Err(e) => return Err(e),
            }
        }
        drop(stmt);
        for (from, to) in rehome {
            conn.execute("UPDATE entries SET redirect_to = ?2 WHERE path = ?1", [from, to])?;
        }
        conn.execute_batch("DROP INDEX tmp_redirect_idx")?;
        Ok(())
    }

    /// Write `m` as this segment's whole `manifest` table.
    fn write_manifest(&self, m: &BTreeMap<String, String>) -> Result<()> {
        let c = self.conn();
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS manifest (key TEXT PRIMARY KEY, value TEXT); \
             DELETE FROM manifest;",
        )?;
        let mut stmt = c.prepare("INSERT INTO manifest (key, value) VALUES (?1, ?2)")?;
        for (k, v) in m {
            stmt.execute(rusqlite::params![k, v])?;
        }
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
/// Accept a zstd level only within 1..=[`MAX_ZSTD_LEVEL`] (SPEC §3 window bound).
pub fn check_zstd_level(level: i32) -> Result<i32> {
    if (1..=MAX_ZSTD_LEVEL).contains(&level) {
        Ok(level)
    } else {
        Err(WaxError::Schema {
            detail: format!(
                "zstd level {level} is outside 1..={MAX_ZSTD_LEVEL}; levels above {MAX_ZSTD_LEVEL} \
                 exceed SPEC §3's 8 MiB decoder window bound"
            ),
        })
    }
}

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

/// Lowercase hex of a digest, as stored in `segment_meta` (SPEC v0.12 §7.4).
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What [`rollback_last_update`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackStats {
    /// Segments in the chain after the rollback.
    pub segments: usize,
    /// File length after the rollback.
    pub file_length: u64,
}

/// Undo the newest append (SPEC v0.12 §7.5): restore the header it
/// overwrote and cut the file back to the end of the previous segment.
///
/// Checked before anything is written: the stored header must point at the
/// previous segment, and the digest of (stored header ‖ earlier segments)
/// must equal the newest segment's `prev_state_digest`. The result is then
/// byte-identical to the edition before the update, so that edition's
/// signature sidecar verifies again. Crash-safe in the same way as an
/// append: the header is written first, and until the truncation lands the
/// removed segment is just an orphaned tail (§7.2).
pub fn rollback_last_update(archive: impl AsRef<Path>) -> Result<RollbackStats> {
    let archive = archive.as_ref();
    let (prev_raw, prev_end, upto) = {
        let r = crate::reader::WaxReader::open_with(
            archive,
            crate::reader::ReadOptions {
                verify_checksums: false,
                lookup_cache_entries: 0,
            },
        )?;
        let n = r.segment_count();
        if n < 2 {
            return Err(WaxError::Schema {
                detail: "nothing to roll back: the archive has a single segment".into(),
            });
        }
        let newest = r.segment_meta(n - 1);
        let (Some(raw), Some(digest), Some((po, pl))) =
            (newest.prev_header, newest.prev_state_digest, newest.prev_segment)
        else {
            return Err(WaxError::Schema {
                detail: format!(
                    "segment {} records no prev_header/prev_state_digest; it predates \
                     verifiable rollback (SPEC v0.12 §7.5)",
                    n - 1
                ),
            });
        };
        let old = WaxHeader::parse(&raw)?;
        if (old.index_offset, old.index_length) != (po, pl) {
            return Err(WaxError::BadPrevStateDigest { segment: (n - 1) as u64 });
        }
        if r.digest_of_state(&old, n - 1)? != digest {
            return Err(WaxError::BadPrevStateDigest { segment: (n - 1) as u64 });
        }
        old.validate(po + pl)?;
        (raw, po + pl, n - 1)
    };
    let mut file = OpenOptions::new().read(true).write(true).open(archive)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&prev_raw)?;
    file.sync_all()?;
    file.set_len(prev_end)?;
    file.sync_all()?;
    // A state written by a v0.13 writer ended with its header mirror (SPEC
    // v0.13 §2.4); put it back so the result is byte-identical.
    let mut file_length = prev_end;
    if crate::header::check_of(&prev_raw) == crate::header::Check::Valid {
        file.seek(SeekFrom::Start(prev_end))?;
        file.write_all(&prev_raw)?;
        file.sync_all()?;
        file_length += HEADER_LEN as u64;
    }
    Ok(RollbackStats {
        segments: upto,
        file_length,
    })
}
