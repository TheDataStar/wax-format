//! Reclaiming space (SPEC v0.13 §7.6): `wax-builder gc`.
//!
//! Appends never rewrite earlier bytes (SPEC §7), so an updated pack keeps
//! every replaced or deleted entry's old bytes, and a crashed append can
//! leave an orphaned tail. Two operations take that space back:
//!
//! * [`reclaim_tail`] — in place, cheap, and signature-preserving: cut
//!   anything past the newest segment, put the header mirror back (§2.4),
//!   and repair slot A from the mirror if it was torn. The §8.1 digest covers
//!   only the header and the segments, so the publisher's signature still
//!   verifies afterwards.
//! * [`compact`] — a new single-segment file holding only what is live:
//!   every live entry's stored bytes copied **verbatim** (same frames, same
//!   dictionary, same hashes — nothing is re-encoded), every live redirect,
//!   the manifest, the dictionary, the passages, and a search index rebuilt
//!   from the exact terms the old one held. The result is a new base
//!   edition: deterministic (a function of the input bytes only, so the
//!   publisher and a box compacting the same edition get the same bytes and
//!   one signature covers both), with the 64-segment budget reset.

use crate::header::{check_of, Check};
use crate::model::Compression;
use crate::reader::{HeaderSource, ReadOptions, WaxReader};
use crate::writer::WaxWriter;
use crate::{Result, WaxError, HEADER_LEN};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// What [`reclaim_tail`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailStats {
    /// Bytes removed from the end of the file (orphaned tail).
    pub reclaimed_bytes: u64,
    /// Slot A was torn and has been restored from the mirror.
    pub repaired_header: bool,
    /// File length afterwards.
    pub file_length: u64,
}

/// Cut an orphaned tail and restore the header mirror, in place (SPEC v0.13
/// §7.6). Changes no byte the §8.1 digest covers, except slot A when it
/// was torn — and then only back to the header the mirror proves.
pub fn reclaim_tail(archive: impl AsRef<Path>) -> Result<TailStats> {
    let archive = archive.as_ref();
    let (raw, logical_end, file_size, source) = {
        let r = WaxReader::open_with(archive, quiet())?;
        (r.header_bytes(), r.logical_end(), r.file_size(), r.header_source())
    };
    let mut f = OpenOptions::new().read(true).write(true).open(archive)?;
    let repaired_header = source == HeaderSource::Mirror;
    if repaired_header {
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&raw)?;
        f.sync_all()?;
    }
    let want_mirror = check_of(&raw) == Check::Valid;
    let target = logical_end + if want_mirror { HEADER_LEN as u64 } else { 0 };
    let mut mirror_ok = false;
    if want_mirror && file_size >= target {
        let mut t = [0u8; HEADER_LEN];
        f.seek(SeekFrom::Start(logical_end))?;
        f.read_exact(&mut t)?;
        mirror_ok = t == raw;
    }
    if !mirror_ok || file_size != target {
        f.set_len(logical_end)?;
        if want_mirror {
            f.seek(SeekFrom::Start(logical_end))?;
            f.write_all(&raw)?;
        }
        f.sync_all()?;
    }
    Ok(TailStats {
        reclaimed_bytes: file_size.saturating_sub(target),
        repaired_header,
        file_length: target,
    })
}

/// Options for [`compact`].
#[derive(Debug, Clone, Copy, Default)]
pub struct CompactOptions {
    /// Leave the search index out instead of refusing when it cannot be
    /// carried over exactly (see [`compact`]).
    pub drop_search: bool,
    /// Skip decoding every entry to check its hash before copying it.
    /// Default (`false`) checks, so compaction never launders a corrupted
    /// entry into a fresh-looking pack.
    pub skip_verify: bool,
}

/// What [`compact`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactStats {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub segments_before: usize,
    pub entries: u64,
    pub redirects: u64,
    pub search_documents: u64,
    pub passages: u64,
    /// The search index was present but left out (`drop_search`).
    pub search_dropped: bool,
}

fn quiet() -> ReadOptions {
    ReadOptions {
        verify_checksums: false,
        lookup_cache_entries: 0,
    }
}

/// Write a compacted copy of `input` to `output` (SPEC v0.13 §7.6).
///
/// The search index cannot be re-derived from the pack (it is contentless:
/// the text that was indexed is not stored), so it is carried over from the
/// terms the old index holds: every live document is re-added with exactly
/// its old terms, and the result is then read back and checked term for
/// term. That works for every tokenizer whose output is its own fixed point
/// (`unicode61`, `wax-bigram-16`). For one that is not (`trigram`, `porter
/// unicode61`) the check fails and compaction is refused — or, with
/// `drop_search`, the pack is written without an index. Scores change, as
/// they must: bm25 weighs each term by how many live documents hold it.
pub fn compact(input: impl AsRef<Path>, output: impl AsRef<Path>, opts: CompactOptions) -> Result<CompactStats> {
    let (input, output) = (input.as_ref(), output.as_ref());
    if output.exists() && std::fs::canonicalize(output).ok() == std::fs::canonicalize(input).ok() {
        return Err(WaxError::Compaction {
            detail: "output is the input; write to a new file and rename it over".into(),
        });
    }
    let r = WaxReader::open_with(input, quiet())?;
    let h = *r.header();

    // Deterministic: identity, time and flags come from the input header.
    let mut w = WaxWriter::new(h.archive_uuid)
        .created_at(h.created_at)
        .flags(h.flags)
        .create(output, r.manifest())?;
    if let Some(d) = r.dictionary() {
        w.carry_dictionary(d)?;
    }

    // --- entries, in path order, bytes verbatim ---
    let mut stats = CompactStats {
        input_bytes: r.file_size(),
        output_bytes: 0,
        segments_before: r.segment_count(),
        entries: 0,
        redirects: 0,
        search_documents: 0,
        passages: 0,
        search_dropped: false,
    };
    for e in r.entries() {
        let e = e?;
        if let Some(to) = &e.redirect_to {
            // A live redirect must still land on live content.
            r.resolve(&e.path)?;
            w.add_redirect(e.path.clone(), to.clone(), e.title.clone())?;
            stats.redirects += 1;
            continue;
        }
        if !opts.skip_verify {
            verify_span(&r, &e)?;
        }
        w.add_raw_entry(&e, &mut r.span_reader(e.offset, e.length))?;
        stats.entries += 1;
        if let Some(blob) = r.passage_blob(&e.path)? {
            w.add_passage_starts(&e.path, &blob)?;
            stats.passages += 1;
        }
    }

    // --- search: carry the live documents' exact terms ---
    let carry = r.has_search_index() && !opts.drop_search;
    let expected = if carry {
        let tok = r.search_tokenizer().unwrap_or_default().to_string();
        w.set_search_tokenizer(&tok)?;
        let terms = live_terms(&r)?;
        {
            let mut docs = terms.prepare("SELECT path, title FROM docs ORDER BY path")?;
            let mut tq = terms.prepare("SELECT term FROM terms WHERE path = ?1 ORDER BY term")?;
            let mut rows = docs.query([])?;
            while let Some(row) = rows.next()? {
                let path: String = row.get(0)?;
                let title: Option<String> = row.get(1)?;
                let ts = tq
                    .query_map([&path], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                w.index_terms(&path, title.as_deref(), &ts.join(" "))?;
                stats.search_documents += 1;
            }
        }
        Some(terms)
    } else {
        stats.search_dropped = r.has_search_index();
        None
    };
    drop(r);
    w.finish()?;

    if let Some(expected) = expected {
        if let Err(e) = check_terms(output, &expected) {
            let _ = std::fs::remove_file(output);
            return Err(e);
        }
    }
    stats.output_bytes = std::fs::metadata(output)?.len();
    Ok(stats)
}

/// Decode one stored entry as a stream and check its hash.
fn verify_span(r: &WaxReader, e: &crate::model::Entry) -> Result<()> {
    let mismatch = || WaxError::ChecksumMismatch { path: e.path.clone() };
    let codec = Compression::parse(&e.path, &e.compression)?;
    let span = r.span_reader(e.offset, e.length);
    let mut hasher = Sha256::new();
    let mut sink = HashSink(&mut hasher, 0);
    let decode_err = |err: io::Error| WaxError::Decompress {
        path: e.path.clone(),
        detail: err.to_string(),
    };
    match codec {
        Compression::None => {
            io::copy(&mut { span }, &mut sink)?;
        }
        Compression::Zstd => {
            let mut d = zstd::stream::read::Decoder::new(span).map_err(decode_err)?;
            io::copy(&mut d, &mut sink).map_err(decode_err)?;
        }
        Compression::ZstdDict => {
            let dict = r
                .dictionary()
                .ok_or_else(|| WaxError::DictionaryMissing { path: e.path.clone() })?;
            let mut d = zstd::stream::read::Decoder::with_dictionary(io::BufReader::new(span), dict)
                .map_err(decode_err)?;
            io::copy(&mut d, &mut sink).map_err(decode_err)?;
        }
    }
    let n = sink.1;
    if n != e.uncompressed_length {
        return Err(mismatch());
    }
    let got: [u8; 32] = hasher.finalize().into();
    if e.sha256.is_some_and(|h| h != got) {
        return Err(mismatch());
    }
    Ok(())
}

struct HashSink<'a>(&'a mut Sha256, u64);

impl Write for HashSink<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        self.1 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The live documents of every indexed segment and their terms, in a
/// scratch database on disk, so memory stays bounded however large the
/// index: `docs(path, title)` and `terms(path, term)`.
struct Scratch {
    conn: Connection,
    _file: tempfile::NamedTempFile,
}

impl std::ops::Deref for Scratch {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

fn live_terms(r: &WaxReader) -> Result<Scratch> {
    let file = tempfile::NamedTempFile::new()?;
    let c = Connection::open(file.path())?;
    c.execute_batch(
        "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
         CREATE TABLE raw_docs (seg INTEGER, id INTEGER, path TEXT, title TEXT, PRIMARY KEY (seg, id));
         CREATE TABLE raw_terms (seg INTEGER, id INTEGER, term TEXT);
         BEGIN;",
    )?;
    for &si in r.search_segment_indices() {
        let seg = r.segment(si);
        {
            let mut ins = c.prepare("INSERT INTO raw_docs VALUES (?1, ?2, ?3, ?4)")?;
            seg.for_each_search_doc(|id, path, title| {
                if r.search_row_owned(&path, si)? {
                    ins.execute(rusqlite::params![si as i64, id, path, title])?;
                }
                Ok(())
            })?;
        }
        let mut ins = c.prepare("INSERT INTO raw_terms VALUES (?1, ?2, ?3)")?;
        seg.for_each_search_term(|id, term| {
            ins.execute(rusqlite::params![si as i64, id, term])?;
            Ok(())
        })?;
    }
    // A path is owned by one segment, so its documents all come from there;
    // a segment that indexed the same path twice gets the union.
    c.execute_batch(
        "CREATE TABLE docs AS
             SELECT path, min(title) AS title FROM raw_docs GROUP BY path;
         CREATE UNIQUE INDEX docs_path ON docs(path);
         CREATE TABLE terms AS
             SELECT DISTINCT d.path AS path, t.term AS term
               FROM raw_terms t JOIN raw_docs d ON d.seg = t.seg AND d.id = t.id;
         CREATE INDEX terms_path ON terms(path, term);
         DROP TABLE raw_terms; DROP TABLE raw_docs;
         COMMIT;",
    )?;
    Ok(Scratch { conn: c, _file: file })
}

/// Read the new index back and require exactly the expected terms per
/// document. Compared inside the scratch database, so memory stays bounded.
fn check_terms(output: &Path, expected: &Scratch) -> Result<()> {
    let r = WaxReader::open_with(output, quiet())?;
    expected.execute_batch(
        "CREATE TABLE got_docs (id INTEGER PRIMARY KEY, path TEXT);
         CREATE TABLE got_terms (id INTEGER, term TEXT);
         BEGIN;",
    )?;
    for &si in r.search_segment_indices() {
        let seg = r.segment(si);
        let mut d = expected.prepare("INSERT INTO got_docs VALUES (?1, ?2)")?;
        seg.for_each_search_doc(|id, path, _| {
            d.execute(rusqlite::params![id, path])?;
            Ok(())
        })?;
        let mut t = expected.prepare("INSERT INTO got_terms VALUES (?1, ?2)")?;
        seg.for_each_search_term(|id, term| {
            t.execute(rusqlite::params![id, term])?;
            Ok(())
        })?;
    }
    expected.execute_batch("COMMIT;")?;
    let differ: i64 = expected.query_row(
        "SELECT count(*) FROM (
             SELECT * FROM (SELECT d.path, t.term FROM got_terms t JOIN got_docs d ON d.id = t.id
                            EXCEPT SELECT path, term FROM terms)
             UNION ALL
             SELECT * FROM (SELECT path, term FROM terms
                            EXCEPT SELECT d.path, t.term FROM got_terms t JOIN got_docs d ON d.id = t.id))",
        [],
        |row| row.get(0),
    )?;
    if differ != 0 {
        let tok = r.search_tokenizer().unwrap_or_default().to_string();
        return Err(WaxError::Compaction {
            detail: format!(
                "the search index cannot be carried over exactly under tokenizer {tok:?} \
                 (its tokens are not their own tokens); rebuild the pack from its source, \
                 or compact without search"
            ),
        });
    }
    Ok(())
}
