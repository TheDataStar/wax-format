//! [`WaxReader`] — open an archive and serve entries (SPEC §5).
//!
//! Open path: parse header → validate → walk the segment chain backward to the
//! base → check blob-section integrity → build the merged path index
//! (last-segment-wins). Read path: resolve at most one redirect hop, pull the
//! blob span, decompress, verify `sha256`.

use crate::header::WaxHeader;
use crate::model::{Compression, Entry, Resolved};
use crate::segment::Segment;
use crate::{Result, WaxError, HEADER_LEN, MAX_SEGMENTS, MIN_SQLITE_LEN};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Options controlling reader strictness.
#[derive(Debug, Clone, Copy)]
pub struct ReadOptions {
    /// Verify each entry's `sha256` after decompression (SPEC §3.1). Default on.
    pub verify_checksums: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            verify_checksums: true,
        }
    }
}

impl std::fmt::Debug for WaxReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaxReader")
            .field("file_size", &self.file_size)
            .field("segments", &self.segments.len())
            .field("entries", &self.merged.len())
            .field("version", &(self.header.version_major, self.header.version_minor))
            .finish()
    }
}

pub struct WaxReader {
    file: File,
    file_size: u64,
    header: WaxHeader,
    /// Segments in ascending `segment_index` order (base first).
    segments: Vec<Segment>,
    /// path → (segment index in `segments`, entry). Last-segment-wins already
    /// applied (SPEC §5.5).
    merged: BTreeMap<String, Entry>,
    manifest: BTreeMap<String, String>,
    opts: ReadOptions,
}

impl WaxReader {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_with(path, ReadOptions::default())
    }

    pub fn open_with<P: AsRef<Path>>(path: P, opts: ReadOptions) -> Result<Self> {
        let mut file = File::open(path)?;
        let file_size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        // --- header ---
        let mut hbuf = [0u8; HEADER_LEN];
        match file.read_exact(&mut hbuf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(WaxError::TruncatedHeader { found: file_size })
            }
            Err(e) => return Err(e.into()),
        }
        let header = WaxHeader::parse(&hbuf)?;
        header.validate(file_size)?;

        // --- segment chain (SPEC §5.1) ---
        let segments = Self::walk_chain(&mut file, &header, file_size)?;

        // --- blob-section integrity (SPEC §4.3) ---
        Self::check_blob_section(&header, &segments)?;

        // --- merge (SPEC §5.5) + volume_id guard (SPEC §5.2) ---
        let mut merged: BTreeMap<String, Entry> = BTreeMap::new();
        for seg in &segments {
            for entry in seg.entries()? {
                if entry.volume_id != 0 {
                    return Err(WaxError::UnexpectedVolumeId {
                        path: entry.path,
                        found: entry.volume_id,
                    });
                }
                merged.insert(entry.path.clone(), entry);
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
            file,
            file_size,
            header,
            segments,
            merged,
            manifest,
            opts,
        })
    }

    fn read_range(file: &mut File, offset: u64, length: u64) -> Result<Vec<u8>> {
        // length is already bounds-checked by the caller.
        let mut buf = vec![0u8; length as usize];
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn walk_chain(file: &mut File, header: &WaxHeader, file_size: u64) -> Result<Vec<Segment>> {
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

            let bytes = Self::read_range(file, cur.0, cur.1)?;
            let seg = Segment::open(cur.0, &bytes)?;

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

    /// All entry paths, ascending (SPEC §5.5). Includes redirect aliases.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.merged.keys().map(|s| s.as_str())
    }

    /// The merged (pre-redirect-resolution) entry for `path`.
    pub fn entry(&self, path: &str) -> Option<&Entry> {
        self.merged.get(path)
    }

    /// All merged entries, ascending by path.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.merged.values()
    }

    /// Resolve `path`, following at most one redirect hop (SPEC §5.3).
    pub fn resolve(&self, path: &str) -> Result<Resolved> {
        let e = self
            .merged
            .get(path)
            .ok_or_else(|| WaxError::EntryNotFound(path.to_string()))?;
        match &e.redirect_to {
            None => Ok(Resolved {
                requested: path.to_string(),
                entry: e.clone(),
            }),
            Some(target) => {
                let t = self.merged.get(target).ok_or_else(|| WaxError::DanglingRedirect {
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
                    entry: t.clone(),
                })
            }
        }
    }

    /// Read and decompress the content for `path` (following one redirect hop).
    pub fn read(&mut self, path: &str) -> Result<Vec<u8>> {
        let resolved = self.resolve(path)?;
        let e = resolved.entry;

        let codec = Compression::parse(&e.path, &e.compression)?;

        // bounds-check the blob span against the file (SPEC §3).
        let end = e
            .offset
            .checked_add(e.length)
            .filter(|end| *end <= self.file_size)
            .ok_or_else(|| WaxError::Schema {
                detail: format!(
                    "entry {:?} blob span [{}, +{}) is outside the file",
                    e.path, e.offset, e.length
                ),
            })?;
        let _ = end;

        let raw = Self::read_range(&mut self.file, e.offset, e.length)?;
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
    /// followed by every segment database, base-first. A7 (signing) builds on
    /// this; exposed now so the conformance suite can pin it.
    pub fn signable_digest(&mut self) -> Result<[u8; 32]> {
        let ranges: Vec<(u64, u64)> = self
            .segments
            .iter()
            .map(|s| (s.disk_offset, s.db_len))
            .collect();
        let mut hasher = Sha256::new();
        hasher.update(self.header.to_bytes());
        for (off, len) in ranges {
            let bytes = Self::read_range(&mut self.file, off, len)?;
            hasher.update(&bytes);
        }
        Ok(hasher.finalize().into())
    }
}
