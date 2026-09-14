//! [`WaxReader`] — open an archive and serve entries (SPEC §5).
//!
//! Open path: parse header → validate → walk the segment chain backward to the
//! base (each segment opened in place through the [`crate::vfs`] window) →
//! check blob-section integrity → reject any non-zero `volume_id`. Nothing
//! proportional to the entry count is held: opening costs O(segments).
//!
//! Lookup path: a point query per segment, newest first — the first hit is the
//! last-segment-wins answer (SPEC §5.5). Read path: resolve at most one
//! redirect hop, pull the blob span, decompress, verify `sha256`. Iteration
//! (`entries`, `paths`) is a streaming k-way merge over the segments' path
//! order, one page of rows per segment in memory.

use crate::header::WaxHeader;
use crate::model::{Compression, Entry, Resolved};
use crate::segment::Segment;
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
    map: HashMap<String, Entry>,
    cap: usize,
    hits: u64,
    misses: u64,
}

impl LookupCache {
    fn get(&mut self, path: &str) -> Option<Entry> {
        let hit = self.map.get(path).cloned();
        if hit.is_some() {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        hit
    }

    fn put(&mut self, entry: &Entry) {
        if self.cap == 0 {
            return;
        }
        if self.map.len() >= self.cap {
            self.map.clear();
        }
        self.map.insert(entry.path.clone(), entry.clone());
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

pub struct WaxReader {
    path: PathBuf,
    file: File,
    file_size: u64,
    header: WaxHeader,
    /// Segments in ascending `segment_index` order (base first).
    segments: Vec<Segment>,
    /// Base-segment manifest (SPEC §5.6); small, held.
    manifest: BTreeMap<String, String>,
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
        let header = WaxHeader::parse(&hbuf)?;
        header.validate(file_size)?;

        // --- segment chain (SPEC §5.1) ---
        let segments = Self::walk_chain(&path, &header, file_size)?;

        // --- blob-section integrity (SPEC §4.3) ---
        Self::check_blob_section(&header, &segments)?;

        // --- volume_id guard (SPEC §5.2): a scan per segment, not a load ---
        for seg in &segments {
            if let Some((path, found)) = seg.first_nonzero_volume()? {
                return Err(WaxError::UnexpectedVolumeId { path, found });
            }
        }

        // --- manifest: base segment only (SPEC §5.6) ---
        let mut manifest = BTreeMap::new();
        if let Some(base) = segments.first() {
            for (k, v) in base.manifest()? {
                manifest.insert(k, v);
            }
        }

        Ok(WaxReader {
            path,
            file,
            file_size,
            header,
            segments,
            manifest,
            opts,
            cache: RefCell::new(LookupCache {
                map: HashMap::new(),
                cap: opts.lookup_cache_entries,
                hits: 0,
                misses: 0,
            }),
        })
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

    /// `(hits, misses)` of the lookup cache since open. Diagnostic.
    pub fn cache_stats(&self) -> (u64, u64) {
        let c = self.cache.borrow();
        (c.hits, c.misses)
    }

    /// The merged (pre-redirect-resolution) entry for `path`, or `None`.
    /// Newest segment first; the first segment that has the path wins
    /// (SPEC §5.5).
    fn lookup(&self, path: &str) -> Result<Option<Entry>> {
        if let Some(hit) = self.cache.borrow_mut().get(path) {
            return Ok(Some(hit));
        }
        for seg in self.segments.iter().rev() {
            if let Some(e) = seg.lookup(path)? {
                self.cache.borrow_mut().put(&e);
                return Ok(Some(e));
            }
        }
        Ok(None)
    }

    /// The merged (pre-redirect-resolution) entry for `path`.
    /// [`WaxError::EntryNotFound`] if no segment carries it.
    pub fn entry(&self, path: &str) -> Result<Entry> {
        self.lookup(path)?
            .ok_or_else(|| WaxError::EntryNotFound(path.to_string()))
    }

    /// Whether any segment carries `path` (redirect aliases included).
    pub fn contains(&self, path: &str) -> Result<bool> {
        Ok(self.lookup(path)?.is_some())
    }

    /// Number of distinct paths across the segment chain. One `COUNT(*)` for
    /// a single-segment archive; a streaming merge otherwise.
    pub fn entry_count(&self) -> Result<u64> {
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
    pub fn entries(&self) -> Entries<'_> {
        Entries {
            cursors: self
                .segments
                .iter()
                .map(|seg| SegCursor {
                    seg,
                    buf: VecDeque::new(),
                    last: None,
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
    pub fn signable_digest(&self) -> Result<[u8; 32]> {
        let mut hasher = Sha256::new();
        hasher.update(self.header.to_bytes());
        let mut buf = vec![0u8; 1 << 20];
        for seg in &self.segments {
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
