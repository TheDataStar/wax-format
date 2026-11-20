//! [`WaxReader`] — open an archive and serve entries (SPEC §5).
//!
//! Open path: parse header → validate → walk the segment chain backward to the
//! base (each segment opened in place through the [`crate::vfs`] window) →
//! check blob-section integrity. Nothing proportional to the entry count is
//! held or read: opening costs O(segments). (A non-zero `volume_id` is
//! rejected when that entry is looked up, SPEC v0.12 §5.2, not by a scan at
//! open.)
//!
//! Lookup path: a point query per segment, newest first — the first hit is the
//! last-segment-wins answer (SPEC §5.5). Read path: resolve at most one
//! redirect hop, pull the blob span, decompress, verify `sha256`. Iteration
//! (`entries`, `paths`) is a streaming k-way merge over the segments' path
//! order, one page of rows per segment in memory.

use crate::header::WaxHeader;
use crate::model::{Compression, Entry, Resolved, SearchHit, Suggestion};
use crate::segment::Segment;
use crate::writer::MANIFEST_UPDATE_REPLACE;
use crate::{Result, WaxError, HEADER_LEN, MAX_SEGMENTS, MIN_SQLITE_LEN};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Rows fetched per segment per page while iterating. Bounds iteration memory
/// at `PAGE_ROWS × segments` entries.
pub const PAGE_ROWS: usize = 256;

/// Default size of the lookup cache (entries). See [`ReadOptions`].
pub const DEFAULT_LOOKUP_CACHE: usize = 1024;

/// Options controlling reader strictness and memory.
#[derive(Debug, Clone, Copy)]
pub struct ReadOptions {
    /// Verify each entry's `sha256` after decompression (SPEC §3.1). Default on.
    pub verify_checksums: bool,
    /// Bound on the path → entry lookup cache in front of the index queries.
    /// Each cached entry is one `Entry` (path, title, mime, and fixed fields;
    /// a few hundred bytes for typical paths). `0` disables it. Default
    /// [`DEFAULT_LOOKUP_CACHE`].
    pub lookup_cache_entries: usize,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            verify_checksums: true,
            lookup_cache_entries: DEFAULT_LOOKUP_CACHE,
        }
    }
}

/// Bounded path → entry cache. When full it is cleared rather than evicted
/// piecemeal: the bound is what matters, and the working set of a pack is
/// re-warmed in a handful of lookups.
struct LookupCache {
    /// path → (index into `segments` of the segment that answered, its row).
    map: HashMap<String, (usize, Entry)>,
    cap: usize,
    hits: u64,
    misses: u64,
}

impl LookupCache {
    fn get(&mut self, path: &str) -> Option<(usize, Entry)> {
        let hit = self.map.get(path).cloned();
        if hit.is_some() {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        hit
    }

    fn put(&mut self, seg: usize, entry: &Entry) {
        if self.cap == 0 {
            return;
        }
        if self.map.len() >= self.cap {
            self.map.clear();
        }
        self.map.insert(entry.path.clone(), (seg, entry.clone()));
    }
}

impl std::fmt::Debug for WaxReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaxReader")
            .field("path", &self.path)
            .field("file_size", &self.file_size)
            .field("segments", &self.segments.len())
            .field("version", &(self.header.version_major, self.header.version_minor))
            .finish()
    }
}

/// Which copy of the header a reader opened the archive with (SPEC v0.13
/// §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderSource {
    /// Slot A, at offset 0: every normal open.
    SlotA,
    /// The mirror at the end of the file, because slot A was torn by a crash
    /// during the header write. The archive is in the state that write was
    /// committing; `wax-builder gc --tail-only` (or the next append) copies
    /// the mirror back into slot A.
    Mirror,
}

pub struct WaxReader {
    path: PathBuf,
    file: File,
    file_size: u64,
    header: WaxHeader,
    /// The 128 bytes `header` was parsed from (slot A, or the mirror).
    header_raw: [u8; HEADER_LEN],
    /// Where the header came from (SPEC v0.13 §2.4).
    header_source: HeaderSource,
    /// Segments in ascending `segment_index` order (base first).
    segments: Vec<Segment>,
    /// The effective manifest (SPEC §5.6, v0.13 §5.6.1); small, held.
    manifest: BTreeMap<String, String>,
    /// The segment that supplied `manifest`: 0, or the newest segment that
    /// declares a manifest update.
    manifest_segment: usize,
    /// The tokenizer the earliest indexed segment declares (SPEC §5.7), or
    /// None when no segment carries a search index.
    search_tokenizer: Option<String>,
    /// Indices into `segments` of every segment carrying a search index,
    /// ascending (SPEC v0.12 §5.7: any segment may carry one).
    search_segments: Vec<usize>,
    /// Set when two indexed segments declare different tokenizers; search is
    /// then refused rather than merged (first, other).
    tokenizer_conflict: Option<(String, String)>,
    /// The dictionary in zstd prepared form, digested once at open. Preparing
    /// it per read costs more than the codec saves -- measured 12.7us -> 16.3us
    /// on a real pack.
    dict_prepared: Option<zstd::dict::DecoderDictionary<'static>>,
    /// Base-segment shared dictionary (SPEC v0.10 §3.1). Held because every
    /// `zstd-dict` entry needs it, and it is ~110 KiB against a pack-open cost
    /// already measured under 1.2 MB. `None` for every v0.9 archive.
    dictionary: Option<Vec<u8>>,
    opts: ReadOptions,
    cache: RefCell<LookupCache>,
}

impl WaxReader {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_with(path, ReadOptions::default())
    }

    pub fn open_with<P: AsRef<Path>>(path: P, opts: ReadOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let file_size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        // --- header ---
        let mut hbuf = [0u8; HEADER_LEN];
        match file.read_exact(&mut hbuf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(WaxError::TruncatedHeader { found: file_size })
            }
            Err(e) => return Err(e.into()),
        }
        // --- which header: slot A, or its mirror (SPEC v0.13 §2.4) ---
        let (header, header_raw, header_source) = Self::choose_header(&path, &file, hbuf, file_size)?;

        // --- segment chain (SPEC §5.1) ---
        let segments = Self::walk_chain(&path, &header, file_size)?;

        // --- blob-section integrity (SPEC §4.3) ---
        Self::check_blob_section(&header, &segments)?;

        // --- volume_id (SPEC v0.12 §5.2): checked per entry at lookup. The
        // open-time scan this replaced read every `entries` page of every
        // segment — the one O(entries) cost left at open, seconds per pack
        // on SD-card storage at Wikipedia scale. ---

        // --- search segments: any segment may carry one (SPEC v0.12 §5.7) ---
        let mut search_segments = Vec::new();
        let mut search_tokenizer: Option<String> = None;
        let mut tokenizer_conflict = None;
        for (i, seg) in segments.iter().enumerate() {
            if !seg.has_search_index()? {
                continue;
            }
            // An index built before the tokenizer was recorded is read as
            // the default, which is what it would have used.
            let declared = seg
                .search_tokenizer()?
                .unwrap_or_else(|| crate::writer::DEFAULT_TOKENIZER.to_string());
            match &search_tokenizer {
                None => search_tokenizer = Some(declared),
                Some(first) if *first != declared && tokenizer_conflict.is_none() => {
                    tokenizer_conflict = Some((first.clone(), declared));
                }
                Some(_) => {}
            }
            search_segments.push(i);
        }

        // --- manifest (SPEC §5.6, v0.13 §5.6.1) ---
        // The base segment's, unless a later segment declares a manifest
        // update: then the newest such segment's table replaces it whole. A
        // `manifest` table in a segment that declares nothing is ignored, as
        // every earlier version required.
        let mut manifest = BTreeMap::new();
        let mut dictionary = None;
        let mut manifest_segment = 0usize;
        for (i, seg) in segments.iter().enumerate().skip(1) {
            match seg.manifest_update()?.as_deref() {
                None => {}
                Some(MANIFEST_UPDATE_REPLACE) => {
                    if !seg.has_manifest_table()? {
                        return Err(WaxError::BadManifestUpdate {
                            segment: i,
                            detail: "declares a replacement but has no manifest table".into(),
                        });
                    }
                    manifest_segment = i;
                }
                Some(other) => {
                    return Err(WaxError::BadManifestUpdate {
                        segment: i,
                        detail: format!("unknown manifest_update {other:?}"),
                    });
                }
            }
        }
        if let Some(seg) = segments.get(manifest_segment) {
            for (k, v) in seg.manifest()? {
                manifest.insert(k, v);
            }
        }
        if let Some(base) = segments.first() {
            // --- shared dictionary: base segment only (SPEC v0.10 §3.1) ---
            dictionary = base.dictionary()?;
        }

        Ok(WaxReader {
            path,
            file,
            file_size,
            header,
            header_raw,
            header_source,
            segments,
            manifest,
            manifest_segment,
            search_tokenizer,
            search_segments,
            tokenizer_conflict,
            dict_prepared: dictionary
                .as_deref()
                .map(zstd::dict::DecoderDictionary::copy),
            dictionary,
            opts,
            cache: RefCell::new(LookupCache {
                map: HashMap::new(),
                cap: opts.lookup_cache_entries,
                hits: 0,
                misses: 0,
            }),
        })
    }

    /// SPEC v0.13 §2.4: pick slot A or the mirror at the end of the file.
    ///
    /// The mirror is the last 128 bytes when they are a header that claims
    /// and passes its check (§2.3) and whose newest segment ends exactly
    /// where the mirror begins. With no such mirror, slot A is the header,
    /// as before v0.13. With one that differs from slot A, slot A still wins
    /// when it is intact: it passes its own check, or (written before
    /// v0.13, so without one) it is byte-for-byte the header the mirror's
    /// newest segment records as the one it replaced (`prev_header`) —
    /// which is exactly the state after a crash before the slot-A write, or
    /// during a rollback. Anything else means slot A was torn, and the
    /// mirror, written and synced before slot A was touched, is the header.
    fn choose_header(
        path: &Path,
        file: &File,
        a_raw: [u8; HEADER_LEN],
        file_size: u64,
    ) -> Result<(WaxHeader, [u8; HEADER_LEN], HeaderSource)> {
        use crate::header::{check_of, Check};
        let slot_a = || -> Result<(WaxHeader, [u8; HEADER_LEN], HeaderSource)> {
            let h = WaxHeader::parse(&a_raw)?;
            h.validate(file_size)?;
            if check_of(&a_raw) == Check::Invalid {
                return Err(WaxError::HeaderCheckMismatch);
            }
            Ok((h, a_raw, HeaderSource::SlotA))
        };

        let Some(mirror_at) = file_size.checked_sub(HEADER_LEN as u64).filter(|&m| m >= HEADER_LEN as u64) else {
            return slot_a();
        };
        let mut t_raw = [0u8; HEADER_LEN];
        if Self::read_exact_at(file, mirror_at, &mut t_raw).is_err() {
            return slot_a();
        }
        if t_raw == a_raw {
            return slot_a();
        }
        let mirror = (|| -> Option<WaxHeader> {
            if check_of(&t_raw) != Check::Valid {
                return None;
            }
            let t = WaxHeader::parse(&t_raw).ok()?;
            if t.index_offset.checked_add(t.index_length)? != mirror_at {
                return None;
            }
            t.validate(file_size).ok()?;
            Some(t)
        })();
        let Some(t) = mirror else {
            return slot_a();
        };

        // Slot A intact by its own check?
        if WaxHeader::parse(&a_raw).is_ok_and(|h| h.validate(file_size).is_ok())
            && check_of(&a_raw) == Check::Valid
        {
            return slot_a();
        }
        // Slot A intact as the header the mirror's state replaced?
        let replaced = Segment::open_at(path, t.index_offset, t.index_length)
            .ok()
            .and_then(|seg| seg.meta.prev_header);
        if replaced == Some(a_raw)
            && WaxHeader::parse(&a_raw).is_ok_and(|h| h.validate(file_size).is_ok())
        {
            return slot_a();
        }
        Ok((t, t_raw, HeaderSource::Mirror))
    }

    /// Positional read of exactly `buf.len()` bytes at `offset`; leaves no
    /// cursor state behind, so reads take `&self`.
    fn read_exact_at(file: &File, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
        while !buf.is_empty() {
            #[cfg(windows)]
            let n = {
                use std::os::windows::fs::FileExt;
                file.seek_read(buf, offset)?
            };
            #[cfg(unix)]
            let n = {
                use std::os::unix::fs::FileExt;
                file.read_at(buf, offset)?
            };
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "blob span ends past end of file",
                ));
            }
            buf = &mut buf[n..];
            offset += n as u64;
        }
        Ok(())
    }

    fn walk_chain(path: &Path, header: &WaxHeader, file_size: u64) -> Result<Vec<Segment>> {
        let mut chain: Vec<Segment> = Vec::new();
        let mut seen: Vec<u64> = Vec::new();
        let mut cur = (header.index_offset, header.index_length);

        loop {
            if chain.len() >= MAX_SEGMENTS {
                return Err(WaxError::TooManySegments);
            }
            if seen.contains(&cur.0) {
                return Err(WaxError::SegmentChainCycle { offset: cur.0 });
            }
            seen.push(cur.0);

            let seg = Segment::open_at(path, cur.0, cur.1)?;

            let prev = seg.meta.prev_segment;
            chain.push(seg);

            match prev {
                None => break,
                Some((po, pl)) => {
                    // bounds: previous segment must sit strictly before this
                    // segment and be a plausible SQLite db (SPEC §5.1).
                    let end = po.checked_add(pl);
                    let ok = po >= HEADER_LEN as u64
                        && pl >= MIN_SQLITE_LEN
                        && end.is_some_and(|e| e <= cur.0 && e <= file_size);
                    if !ok {
                        return Err(WaxError::PrevSegmentOutOfBounds {
                            offset: po,
                            length: pl,
                        });
                    }
                    cur = (po, pl);
                }
            }
        }

        chain.reverse(); // now base-first
        Self::check_chain_order(&chain)?;
        Ok(chain)
    }

    fn check_chain_order(chain: &[Segment]) -> Result<()> {
        for (i, seg) in chain.iter().enumerate() {
            if seg.meta.segment_index != i as u64 {
                return Err(WaxError::BrokenSegmentChain {
                    detail: format!(
                        "segment at position {i} declares segment_index {}",
                        seg.meta.segment_index
                    ),
                });
            }
        }
        if chain.is_empty() {
            return Err(WaxError::BrokenSegmentChain {
                detail: "empty chain".into(),
            });
        }
        Ok(())
    }

    fn check_blob_section(header: &WaxHeader, segments: &[Segment]) -> Result<()> {
        // Sum of blob regions must equal the header field (SPEC §4.3).
        let mut total: u64 = 0;
        for seg in segments {
            let m = &seg.meta;
            let end = m.blob_region_offset.checked_add(m.blob_region_length);
            let region_ok = m.blob_region_offset >= HEADER_LEN as u64
                && end.is_some_and(|e| e <= seg.disk_offset);
            if !region_ok {
                return Err(WaxError::BlobSectionLengthMismatch {
                    detail: format!(
                        "segment {} blob region [{}, +{}) does not fit before its index at {}",
                        m.segment_index, m.blob_region_offset, m.blob_region_length, seg.disk_offset
                    ),
                });
            }
            total = total
                .checked_add(m.blob_region_length)
                .ok_or_else(|| WaxError::BlobSectionLengthMismatch {
                    detail: "blob region lengths overflow u64".into(),
                })?;
        }
        if total != header.blob_section_length {
            return Err(WaxError::BlobSectionLengthMismatch {
                detail: format!(
                    "sum of blob regions = {total}, header blob_section_length = {}",
                    header.blob_section_length
                ),
            });
        }
        // Single-segment fast-path equality (SPEC §4.3).
        if segments.len() == 1
            && header.index_offset != HEADER_LEN as u64 + header.blob_section_length
        {
            return Err(WaxError::BlobSectionLengthMismatch {
                detail: format!(
                    "single-segment archive: index_offset {} != 128 + blob_section_length {}",
                    header.index_offset, header.blob_section_length
                ),
            });
        }
        Ok(())
    }

    // --- public surface -------------------------------------------------

    /// Which header copy this open used (SPEC v0.13 §2.4).
    pub fn header_source(&self) -> HeaderSource {
        self.header_source
    }

    /// The exact 128 header bytes this open used.
    pub fn header_bytes(&self) -> [u8; HEADER_LEN] {
        self.header_raw
    }

    /// End of the newest index segment: everything after it is either the
    /// header mirror (§2.4) or the orphaned tail of a crashed append (§7.2).
    pub fn logical_end(&self) -> u64 {
        self.header.index_offset + self.header.index_length
    }

    /// Whether the file ends with a valid mirror of the header it was
    /// opened with, and nothing else past its logical end (SPEC v0.13 §2.4).
    pub fn has_mirror(&self) -> Result<bool> {
        if self.file_size != self.logical_end() + HEADER_LEN as u64 {
            return Ok(false);
        }
        let mut t = [0u8; HEADER_LEN];
        Self::read_exact_at(&self.file, self.logical_end(), &mut t)?;
        Ok(t == self.header_raw)
    }

    pub fn header(&self) -> &WaxHeader {
        &self.header
    }

    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn manifest(&self) -> &BTreeMap<String, String> {
        &self.manifest
    }

    /// The segment the manifest was read from (SPEC v0.13 §5.6.1): `0` for the
    /// base, or the newest appended segment that replaced it.
    pub fn manifest_segment(&self) -> usize {
        self.manifest_segment
    }

    /// True when this pack carries an embedded search index (SPEC §5.7).
    ///
    /// `false` is not an error — it is the normal state of every pack built
    /// before search existed, and of any pack published without one.
    pub fn has_search_index(&self) -> bool {
        self.search_tokenizer.is_some()
    }

    /// The tokenizer this pack was indexed with, or `None` when it carries no
    /// index. A reader that cannot reproduce it refuses to query (§5.7).
    pub fn search_tokenizer(&self) -> Option<&str> {
        self.search_tokenizer.as_deref()
    }

    /// Full-text search over the embedded index, best match first.
    ///
    /// Queried in place through the same windowing VFS entry lookups use, so
    /// this costs page reads rather than loading the index.
    ///
    /// Every indexed segment is queried (SPEC v0.12 §5.7). A hit from segment
    /// *s* counts only while its path still resolves, through the
    /// last-segment-wins merge, to *s* itself: a newer segment that replaced
    /// the entry carries its own row for the new text, and a tombstone
    /// removes it. The surviving lists are therefore disjoint, and they are
    /// merged by per-segment position (raw bm25 scores from two segments are
    /// not comparable), newer segment first on a tie — the order Reciprocal
    /// Rank Fusion gives for disjoint lists whatever its constant.
    ///
    /// Errors when the pack has no index, or when it declares a tokenizer this
    /// build cannot reproduce — answering anyway would mean returning
    /// confidently wrong results for text that is present.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.check_searchable()?;
        let mut ranked: Vec<(usize, usize, SearchHit)> = Vec::new(); // (rank, seg, hit)
        for &si in self.search_segments.iter().rev() {
            let seg = &self.segments[si];
            let mut rank = 0usize;
            let mut offset = 0usize;
            let page = limit.max(16);
            'seg: loop {
                let rows = seg.search(query, page, offset)?;
                let n = rows.len();
                offset += n;
                for (path, title) in rows {
                    if self.owned_by(&path, si)? {
                        ranked.push((rank, si, SearchHit { path, title }));
                        rank += 1;
                        if rank >= limit {
                            break 'seg;
                        }
                    }
                }
                if n < page {
                    break;
                }
            }
        }
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        Ok(ranked.into_iter().take(limit).map(|(_, _, h)| h).collect())
    }

    /// Title prefix completion, for a search bar, ordered by folded title.
    ///
    /// The prefix is folded under the fold the index declares (SPEC v0.12
    /// §5.7.1); a fold this build cannot reproduce is refused, never
    /// approximated. Hits are validated against the merge exactly as for
    /// [`search`](Self::search).
    pub fn suggest(&self, prefix: &str, limit: usize) -> Result<Vec<Suggestion>> {
        self.check_searchable()?;
        let mut all: Vec<(String, String, String)> = Vec::new(); // (fold, path, title)
        let mut fold_name: Option<String> = None;
        for &si in self.search_segments.iter().rev() {
            let seg = &self.segments[si];
            let fold = seg.title_fold()?;
            match &fold_name {
                None => fold_name = Some(fold.clone()),
                Some(f) if *f != fold => {
                    return Err(WaxError::UnknownTitleFold {
                        fold: format!("{f} and {fold} mixed across segments"),
                    })
                }
                Some(_) => {}
            }
            let lo = crate::fold::fold_title(&fold, prefix)
                .ok_or_else(|| WaxError::UnknownTitleFold { fold: fold.clone() })?;
            let mut hi = lo.clone();
            hi.push('\u{10FFFF}');
            let mut kept = 0usize;
            let mut offset = 0usize;
            let page = limit.max(16);
            'seg: loop {
                let rows = seg.suggest(&lo, &hi, page, offset)?;
                let n = rows.len();
                offset += n;
                for (path, title, f) in rows {
                    if self.owned_by(&path, si)? {
                        all.push((f, path, title));
                        kept += 1;
                        if kept >= limit {
                            break 'seg;
                        }
                    }
                }
                if n < page {
                    break;
                }
            }
        }
        all.sort();
        all.dedup_by(|a, b| a.1 == b.1);
        Ok(all
            .into_iter()
            .take(limit)
            .map(|(_, path, title)| Suggestion { path, title })
            .collect())
    }

    /// The passages of `path` (SPEC v0.13 §5.10), after following a
    /// redirect. Their ids are `(path, ordinal)`; headings and anchors are
    /// read from the entry's own bytes.
    ///
    /// `PassagesUnavailable` when the segment that owns the entry carries no
    /// passage row for it (every pack built before v0.13, and any entry its
    /// builder did not cut); `UnknownPassageRule` when it declares a rule
    /// this build does not know.
    pub fn passages(&self, path: &str) -> Result<Vec<crate::passage::Passage>> {
        let (entry, starts) = self.passage_starts(path)?;
        let content = self.read(&entry.path)?;
        Ok(crate::passage::describe(&content, &starts))
    }

    /// The raw `passages.starts` value the owning segment stores for
    /// `path`, if any (compaction carries it over byte for byte).
    pub(crate) fn passage_blob(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let Some((si, _)) = self.lookup_seg(path)? else {
            return Ok(None);
        };
        let seg = &self.segments[si];
        match seg.passage_rule()? {
            Some(rule) if crate::passage::SUPPORTED_RULES.contains(&rule.as_str()) => seg.passage_starts(path),
            Some(rule) => Err(WaxError::UnknownPassageRule { rule }),
            None => Ok(None),
        }
    }

    /// Indices of the segments that carry a search index, ascending.
    pub(crate) fn search_segment_indices(&self) -> &[usize] {
        &self.search_segments
    }

    pub(crate) fn segment(&self, i: usize) -> &Segment {
        &self.segments[i]
    }

    /// Whether search row `path` of segment `si` is still live (SPEC v0.12
    /// §5.7 ownership).
    pub(crate) fn search_row_owned(&self, path: &str, si: usize) -> Result<bool> {
        self.owned_by(path, si)
    }

    /// The raw stored bytes `[offset, offset + length)` as a stream
    /// (compaction copies spans without holding them).
    pub(crate) fn span_reader(&self, offset: u64, length: u64) -> SpanReader<'_> {
        SpanReader {
            file: &self.file,
            offset,
            remaining: length,
        }
    }

    /// The owning entry of `path` and its decoded passage starts.
    fn passage_starts(&self, path: &str) -> Result<(Entry, Vec<u64>)> {
        let target = self.resolve(path)?.entry.path;
        let (si, entry) = self
            .lookup_seg(&target)?
            .ok_or_else(|| WaxError::EntryNotFound(target.clone()))?;
        let seg = &self.segments[si];
        let unavailable = || WaxError::PassagesUnavailable { path: target.clone() };
        let rule = seg.passage_rule()?.ok_or_else(unavailable)?;
        if !crate::passage::SUPPORTED_RULES.contains(&rule.as_str()) {
            return Err(WaxError::UnknownPassageRule { rule });
        }
        let blob = seg.passage_starts(&target)?.ok_or_else(unavailable)?;
        let starts = crate::passage::decode_starts(&target, &blob, entry.uncompressed_length)?;
        Ok((entry, starts))
    }

    /// The passages of `path` that match `query` (an FTS5 expression, built
    /// the same way as for [`search`](Self::search)), best first, at most
    /// `limit`.
    ///
    /// Scored when asked, not stored: the entry is read (it is usually the
    /// page about to be shown anyway), each passage's visible text is put
    /// through the pack's own tokenizer in a throwaway in-memory index, and
    /// that index answers the query. So the pack pays only for the cut
    /// points, never for a second, passage-sized index.
    pub fn best_passages(
        &self,
        path: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::passage::Passage>> {
        self.check_searchable()?;
        let tok = self.search_tokenizer.clone().unwrap_or_default();
        let passages = self.passages(path)?;
        let content = self.read(&self.resolve(path)?.entry.path)?;
        let mem = rusqlite::Connection::open_in_memory()?;
        mem.execute_batch(&format!(
            "CREATE VIRTUAL TABLE p USING fts5(body, detail=none, tokenize={});",
            crate::tokenize::sqlite_tokenize_arg(&tok)
        ))?;
        {
            let mut ins = mem.prepare("INSERT INTO p(rowid, body) VALUES (?1, ?2)")?;
            for p in &passages {
                let span = &content[p.start as usize..p.end as usize];
                let text = crate::text::html_to_text(&String::from_utf8_lossy(span));
                let text = if tok == crate::tokenize::WAX_BIGRAM {
                    crate::tokenize::prepare_document(&text)
                } else {
                    text
                };
                ins.execute(rusqlite::params![p.ordinal as i64, text])?;
            }
        }
        let mut st = mem.prepare("SELECT rowid FROM p WHERE p MATCH ?1 ORDER BY rank, rowid LIMIT ?2")?;
        let ids = st
            .query_map(rusqlite::params![query, limit as i64], |r| r.get::<_, i64>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ids
            .into_iter()
            .filter_map(|i| passages.get(i as usize).cloned())
            .collect())
    }

    /// Passage-level search (SPEC v0.13 §5.10): the entry hits of
    /// [`search`](Self::search), in the same order, each with the passage
    /// inside it that best matches. An entry without passages is returned
    /// whole (`passage: None`); an entry whose passages none match on their
    /// own (the words are spread across several) gets its first passage.
    pub fn search_passages(&self, query: &str, limit: usize) -> Result<Vec<crate::model::PassageHit>> {
        let hits = self.search(query, limit)?;
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            let passage = match self.best_passages(&h.path, query, 1) {
                Ok(mut v) if !v.is_empty() => Some(v.remove(0)),
                Ok(_) => self.passages(&h.path)?.into_iter().next(),
                Err(WaxError::PassagesUnavailable { .. }) => None,
                Err(e) => return Err(e),
            };
            out.push(crate::model::PassageHit {
                path: h.path,
                title: h.title,
                passage,
            });
        }
        Ok(out)
    }

    /// Whether this pack has an index this build can query.
    fn check_searchable(&self) -> Result<()> {
        let tok = self
            .search_tokenizer
            .as_deref()
            .ok_or(WaxError::SearchUnavailable)?;
        if let Some((first, other)) = &self.tokenizer_conflict {
            return Err(WaxError::TokenizerMismatch {
                first: first.clone(),
                other: other.clone(),
            });
        }
        if !crate::writer::SUPPORTED_TOKENIZERS.contains(&tok) {
            return Err(WaxError::UnknownTokenizer {
                tokenizer: tok.to_string(),
            });
        }
        Ok(())
    }

    /// True when `path`, through the merge, still lives in segment `si` and is
    /// not deleted — the validity test for a search row from `si`.
    ///
    /// Probes the segments directly rather than through the lookup cache, so
    /// searching never churns the cache that serves page reads: any newer
    /// segment carrying the path supersedes the row, and `si`'s own row must
    /// exist and not be a tombstone.
    fn owned_by(&self, path: &str, si: usize) -> Result<bool> {
        for seg in &self.segments[si + 1..] {
            if seg.lookup(path)?.is_some() {
                return Ok(false);
            }
        }
        Ok(match self.segments[si].lookup(path)? {
            Some(e) => !e.is_tombstone() && e.volume_id == 0,
            None => false,
        })
    }

    /// The SPEC v0.12 §7.4 `prev_state_digest` of segment `segment_index`:
    /// the §8.1 digest of the archive state that segment was appended to.
    /// `None` for the base and for segments written before v0.12.
    ///
    /// A box applying an update compares this against
    /// [`signable_digest`](Self::signable_digest) of its own copy *before*
    /// appending the received tail; a mismatch means the update was built
    /// against a different edition and must not be applied.
    pub fn prev_state_digest(&self, segment_index: usize) -> Option<[u8; 32]> {
        self.segments
            .get(segment_index)
            .and_then(|s| s.meta.prev_state_digest)
    }

    /// The pack's shared dictionary (SPEC v0.10 §3.1), or `None` for a v0.9
    /// archive and for any pack whose entries are all plain-framed.
    ///
    /// A builder producing the next edition reads this and carries it forward
    /// rather than retraining — see [`StreamingWriter::carry_dictionary`].
    ///
    /// [`StreamingWriter::carry_dictionary`]: crate::writer::StreamingWriter::carry_dictionary
    pub fn dictionary(&self) -> Option<&[u8]> {
        self.dictionary.as_deref()
    }

    /// `(hits, misses)` of the lookup cache since open. Diagnostic.
    pub fn cache_stats(&self) -> (u64, u64) {
        let c = self.cache.borrow();
        (c.hits, c.misses)
    }

    /// The merged (pre-redirect-resolution) entry for `path`, or `None`.
    /// Newest segment first; the first segment that has the path wins
    /// (SPEC §5.5).
    ///
    /// The row may be a tombstone; callers decide what that means.
    fn lookup(&self, path: &str) -> Result<Option<Entry>> {
        Ok(self.lookup_seg(path)?.map(|(_, e)| e))
    }

    /// As [`lookup`](Self::lookup), also naming the segment (index into
    /// `segments`) that answered. Rejects a non-zero `volume_id` here, per
    /// entry, rather than by a scan at open (SPEC v0.12 §5.2).
    fn lookup_seg(&self, path: &str) -> Result<Option<(usize, Entry)>> {
        if let Some(hit) = self.cache.borrow_mut().get(path) {
            return Ok(Some(hit));
        }
        for (i, seg) in self.segments.iter().enumerate().rev() {
            if let Some(e) = seg.lookup(path)? {
                if e.volume_id != 0 {
                    return Err(WaxError::UnexpectedVolumeId {
                        path: e.path,
                        found: e.volume_id,
                    });
                }
                self.cache.borrow_mut().put(i, &e);
                return Ok(Some((i, e)));
            }
        }
        Ok(None)
    }

    /// The merged (pre-redirect-resolution) entry for `path`.
    /// [`WaxError::EntryNotFound`] if no segment carries it,
    /// [`WaxError::EntryDeleted`] if the newest segment that does deletes it.
    pub fn entry(&self, path: &str) -> Result<Entry> {
        match self.lookup(path)? {
            None => Err(WaxError::EntryNotFound(path.to_string())),
            Some(e) if e.is_tombstone() => Err(WaxError::EntryDeleted(path.to_string())),
            Some(e) => Ok(e),
        }
    }

    /// Whether `path` exists in the merged view (redirect aliases included,
    /// deleted paths not).
    pub fn contains(&self, path: &str) -> Result<bool> {
        Ok(self.lookup(path)?.is_some_and(|e| !e.is_tombstone()))
    }

    /// Number of distinct paths across the segment chain. One `COUNT(*)` for
    /// a single-segment archive; a streaming merge otherwise.
    pub fn entry_count(&self) -> Result<u64> {
        if let Some(n) = self.stored_entry_count() {
            return Ok(n);
        }
        if self.segments.len() == 1 {
            return self.segments[0].count();
        }
        let mut n = 0u64;
        for e in self.entries() {
            e?;
            n += 1;
        }
        Ok(n)
    }

    /// All merged entries, ascending by path, streamed (SPEC §5.5). Holds at
    /// most [`PAGE_ROWS`] rows per segment. An error ends the iteration.
    /// Deleted paths (SPEC v0.12 §5.5.1) are left out.
    pub fn entries(&self) -> Entries<'_> {
        self.entries_after(None)
    }

    /// Like [`entries`](Self::entries), but starting strictly after `after`
    /// in path byte order. Each segment's cursor seeks with one index range
    /// scan, so a server can page through a large pack (keyset pagination)
    /// without walking it from the start.
    pub fn entries_after(&self, after: Option<&str>) -> Entries<'_> {
        Entries {
            cursors: self
                .segments
                .iter()
                .map(|seg| SegCursor {
                    seg,
                    buf: VecDeque::new(),
                    last: after.map(str::to_string),
                    exhausted: false,
                })
                .collect(),
            done: false,
        }
    }

    /// All entry paths, ascending, streamed (SPEC §5.5). Includes redirect
    /// aliases.
    pub fn paths(&self) -> impl Iterator<Item = Result<String>> + '_ {
        self.entries().map(|e| e.map(|e| e.path))
    }

    /// Resolve `path`, following at most one redirect hop (SPEC §5.3).
    pub fn resolve(&self, path: &str) -> Result<Resolved> {
        let e = self.entry(path)?;
        match &e.redirect_to {
            None => Ok(Resolved {
                requested: path.to_string(),
                entry: e,
            }),
            Some(target) => {
                let t = self
                    .lookup(target)?
                    .ok_or_else(|| WaxError::DanglingRedirect {
                        from: path.to_string(),
                        to: target.clone(),
                    })?;
                // An alias whose target a later segment deleted is gone too
                // (SPEC v0.12 §5.5.1), not malformed.
                if t.is_tombstone() {
                    return Err(WaxError::EntryDeleted(path.to_string()));
                }
                if t.redirect_to.is_some() {
                    return Err(WaxError::RedirectChainTooDeep {
                        from: path.to_string(),
                        via: target.clone(),
                    });
                }
                Ok(Resolved {
                    requested: path.to_string(),
                    entry: t,
                })
            }
        }
    }

    /// Read and decompress the content for `path` (following one redirect hop).
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let resolved = self.resolve(path)?;
        let e = resolved.entry;

        let codec = Compression::parse(&e.path, &e.compression)?;

        // bounds-check the blob span against the file (SPEC §3).
        e.offset
            .checked_add(e.length)
            .filter(|end| *end <= self.file_size)
            .ok_or_else(|| WaxError::Schema {
                detail: format!(
                    "entry {:?} blob span [{}, +{}) is outside the file",
                    e.path, e.offset, e.length
                ),
            })?;

        let mut raw = vec![0u8; e.length as usize];
        Self::read_exact_at(&self.file, e.offset, &mut raw)?;
        let content = match codec {
            Compression::None => raw,
            Compression::Zstd => {
                zstd::stream::decode_all(&raw[..]).map_err(|err| WaxError::Decompress {
                    path: e.path.clone(),
                    detail: err.to_string(),
                })?
            }
            Compression::ZstdDict => {
                // One frame, exactly as for Zstd — the only difference is the
                // dictionary. Random access is unchanged, which is the property
                // this codec exists to keep.
                //
                // The dictionary is *prepared once at open*, not handed to the
                // decoder raw on every read. Measured: re-digesting ~110 KB per
                // read cost 12.7µs -> 16.3µs on a real pack, which would have
                // made the codec slower to read than the one it replaced. The
                // prepared form is the whole difference.
                let prepared =
                    self.dict_prepared
                        .as_ref()
                        .ok_or_else(|| WaxError::DictionaryMissing {
                            path: e.path.clone(),
                        })?;
                let mut dec = zstd::bulk::Decompressor::with_prepared_dictionary(prepared)
                    .map_err(|err| WaxError::Decompress {
                        path: e.path.clone(),
                        detail: err.to_string(),
                    })?;
                dec.decompress(&raw[..], e.uncompressed_length as usize)
                    .map_err(|err| WaxError::Decompress {
                        path: e.path.clone(),
                        detail: err.to_string(),
                    })?
            }
        };

        if content.len() as u64 != e.uncompressed_length {
            return Err(WaxError::Decompress {
                path: e.path.clone(),
                detail: format!(
                    "decoded {} bytes, entry says uncompressed_length = {}",
                    content.len(),
                    e.uncompressed_length
                ),
            });
        }

        if self.opts.verify_checksums {
            if let Some(expected) = e.sha256 {
                let got: [u8; 32] = Sha256::digest(&content).into();
                if got != expected {
                    return Err(WaxError::ChecksumMismatch {
                        path: e.path.clone(),
                    });
                }
            }
        }

        Ok(content)
    }

    /// Recompute the signable digest (SPEC §8.1): SHA-256 over the header bytes
    /// followed by every segment database, base-first, streamed through a
    /// 1 MiB buffer.
    /// The live entry count the newest segment records (SPEC v0.12 §4.2
    /// `live_entry_count`), when it records one. [`entry_count`](Self::entry_count)
    /// uses it; `wax-builder verify` checks it against a full walk.
    pub fn stored_entry_count(&self) -> Option<u64> {
        self.segments.last().and_then(|s| s.meta.live_entry_count)
    }

    /// The 128 header bytes the newest segment's append overwrote (SPEC
    /// v0.12 §7.5), when it recorded them.
    pub fn prev_header(&self, segment_index: usize) -> Option<[u8; HEADER_LEN]> {
        self.segments.get(segment_index).and_then(|s| s.meta.prev_header)
    }

    pub(crate) fn segment_meta(&self, i: usize) -> &crate::segment::SegmentMeta {
        &self.segments[i].meta
    }

    pub fn signable_digest(&self) -> Result<[u8; 32]> {
        self.digest_of_state(&self.header, self.segments.len())
    }

    /// The §8.1 digest of the state made of `header` and the first `upto`
    /// segments: what the archive's digest was before later appends, given
    /// the header it had then (SPEC v0.12 §7.5 rollback check).
    pub(crate) fn digest_of_state(&self, header: &WaxHeader, upto: usize) -> Result<[u8; 32]> {
        let mut hasher = Sha256::new();
        hasher.update(header.to_bytes());
        let mut buf = vec![0u8; 1 << 20];
        for seg in &self.segments[..upto] {
            let mut off = seg.disk_offset;
            let mut remaining = seg.db_len;
            while remaining > 0 {
                let want = remaining.min(buf.len() as u64) as usize;
                Self::read_exact_at(&self.file, off, &mut buf[..want])?;
                hasher.update(&buf[..want]);
                off += want as u64;
                remaining -= want as u64;
            }
        }
        Ok(hasher.finalize().into())
    }

    /// Test hook: the query plans behind [`WaxReader::entry`] and
    /// [`WaxReader::entries`] for the base segment.
    #[doc(hidden)]
    pub fn query_plans(&self) -> Result<(String, String)> {
        let s = &self.segments[0];
        Ok((s.lookup_plan()?, s.page_plan()?))
    }
}

/// The SPEC §8.1 signable digest of an archive: `(digest, archive_uuid,
/// created_at)`. Kept as a free function for callers that only sign or
/// verify; opening is O(segments) so it is simply open + digest.
pub fn signable_digest_of<P: AsRef<Path>>(path: P) -> Result<([u8; 32], [u8; 16], u64)> {
    let r = WaxReader::open_with(
        path,
        ReadOptions {
            verify_checksums: false,
            lookup_cache_entries: 0,
        },
    )?;
    Ok((r.signable_digest()?, r.header.archive_uuid, r.header.created_at))
}

// ---------------------------------------------------------------------------
// Streaming merged iteration
// ---------------------------------------------------------------------------

struct SegCursor<'r> {
    seg: &'r Segment,
    buf: VecDeque<Entry>,
    /// Last path handed out of this segment; the next page starts after it.
    last: Option<String>,
    exhausted: bool,
}

impl SegCursor<'_> {
    /// Make sure the head of `buf` is the next row, if there is one.
    fn fill(&mut self) -> Result<()> {
        if !self.buf.is_empty() || self.exhausted {
            return Ok(());
        }
        let page = self.seg.page(self.last.as_deref(), PAGE_ROWS)?;
        if page.len() < PAGE_ROWS {
            self.exhausted = true;
        }
        if let Some(l) = page.last() {
            self.last = Some(l.path.clone());
        }
        self.buf.extend(page);
        Ok(())
    }
}

/// Iterator behind [`WaxReader::entries`]: a k-way merge of the segments'
/// path-ordered rows, ties resolved to the newest segment (SPEC §5.5).
pub struct Entries<'r> {
    cursors: Vec<SegCursor<'r>>,
    done: bool,
}

impl Iterator for Entries<'_> {
    type Item = Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.next_merged() {
                Some(Ok(e)) if e.is_tombstone() => continue,
                Some(Ok(e)) if e.volume_id != 0 => {
                    self.done = true;
                    return Some(Err(WaxError::UnexpectedVolumeId {
                        path: e.path,
                        found: e.volume_id,
                    }));
                }
                other => return other,
            }
        }
    }
}

impl Entries<'_> {
    /// The next row of the k-way merge, tombstones included.
    fn next_merged(&mut self) -> Option<Result<Entry>> {
        if self.done {
            return None;
        }
        for c in &mut self.cursors {
            if let Err(e) = c.fill() {
                self.done = true;
                return Some(Err(e));
            }
        }
        // smallest head path; on ties the highest segment index wins
        let mut best: Option<(usize, &str)> = None;
        for (i, c) in self.cursors.iter().enumerate() {
            if let Some(head) = c.buf.front() {
                let better = match best {
                    None => true,
                    Some((_, p)) => head.path.as_str() <= p, // `<=` keeps the newer segment on ties
                };
                if better {
                    best = Some((i, head.path.as_str()));
                }
            }
        }
        let Some((win, path)) = best else {
            self.done = true;
            return None;
        };
        let path = path.to_string();
        let mut out = None;
        for (i, c) in self.cursors.iter_mut().enumerate() {
            if c.buf.front().is_some_and(|h| h.path == path) {
                let e = c.buf.pop_front();
                if i == win {
                    out = e;
                }
            }
        }
        out.map(Ok)
    }
}

/// A bounded positional reader over one span of the archive file.
pub(crate) struct SpanReader<'a> {
    file: &'a File,
    offset: u64,
    remaining: u64,
}

impl Read for SpanReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.remaining).min(1 << 16) as usize;
        WaxReader::read_exact_at(self.file, self.offset, &mut buf[..want])?;
        self.offset += want as u64;
        self.remaining -= want as u64;
        Ok(want)
    }
}
