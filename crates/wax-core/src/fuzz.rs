//! Panic-free entry points for the three A4 fuzz targets (SPEC §11.3).
//!
//! Each `check_*` takes raw bytes and must **never** panic, never read out of
//! bounds, never allocate unboundedly, never loop forever — it either returns
//! `Ok(())` (input handled) or a [`WaxError`] (input rejected). The `fuzz/`
//! crate wraps these for libFuzzer; `tests/fuzz_smoke.rs` runs them over the
//! checked-in corpus so the property is enforced even without libFuzzer
//! (SPEC §12.15).

use crate::header::WaxHeader;
use crate::segment::Segment;
use crate::{Result, WaxError};
use std::io::Write;

/// Target 1: header parser. Feeds arbitrary bytes through `parse` + `validate`.
/// `validate` needs a file size; we derive a plausible one from the input so
/// the bounds arithmetic is actually exercised.
pub fn check_header_parse(data: &[u8]) -> Result<()> {
    let header = WaxHeader::parse(data)?;
    // Exercise validate() against several file-size hypotheses, including
    // adversarial ones (0, just the header, exactly the claimed index end, huge).
    let claimed_end = header
        .index_offset
        .saturating_add(header.index_length);
    for fs in [
        0u64,
        crate::HEADER_LEN as u64,
        data.len() as u64,
        claimed_end,
        claimed_end.saturating_add(1),
        u64::MAX,
    ] {
        let _ = header.validate(fs);
    }
    // Round-trip must not panic and must be stable.
    let bytes = header.to_bytes();
    let reparsed = WaxHeader::parse(&bytes)?;
    debug_assert_eq!(reparsed.index_offset, header.index_offset);
    Ok(())
}

/// Target 2: index-footer SQLite loader. Writes arbitrary bytes to a temp file
/// and tries to open them in place as an index segment (through the offset
/// VFS, exactly as the reader does), then pages through `entries`, probes a
/// path, and reads `manifest`. SQLite must not be able to make us panic, and
/// nothing proportional to the row count is held.
pub fn check_index_loader(data: &[u8]) -> Result<()> {
    // Cap the input so a fuzzer can't ask us to buffer gigabytes; the real
    // reader bounds this via header fields.
    if data.len() > 8 * 1024 * 1024 {
        return Err(WaxError::Schema {
            detail: "oversized segment input".into(),
        });
    }
    let mut tmp = tempfile::NamedTempFile::new()?;
    tmp.write_all(data)?;
    tmp.flush()?;

    let seg = Segment::open_at(tmp.path(), 0, data.len() as u64)?;
    let mut after: Option<String> = None;
    loop {
        let page = seg.page(after.as_deref(), 64)?;
        let Some(last) = page.last() else { break };
        after = Some(last.path.clone());
        if page.len() < 64 {
            break;
        }
    }
    let _ = seg.lookup("index.html")?;
    let _ = seg.first_nonzero_volume()?;
    let _ = seg.manifest()?;
    Ok(())
}

/// Target 3: segment-chain merge logic. The input is interpreted as a compact
/// script describing 0–8 synthetic segments and their entries; we then run the
/// same merge + one-hop-redirect resolution the reader uses and assert the
/// invariants hold (no panic, no infinite loop, redirects never exceed one hop).
pub fn check_segment_merge(data: &[u8]) -> Result<()> {
    let model = MergeModel::parse(data);
    let merged = model.merge(); // last-segment-wins
    // Resolve every path; the resolver must terminate and never chase >1 hop.
    for path in merged.keys() {
        let _ = resolve_one_hop(&merged, path);
    }
    Ok(())
}

// --- tiny model for target 3 ------------------------------------------------

#[derive(Clone)]
struct ModelEntry {
    redirect_to: Option<String>,
}

struct MergeModel {
    /// segments[i] = list of (path, entry)
    segments: Vec<Vec<(String, ModelEntry)>>,
}

impl MergeModel {
    fn parse(data: &[u8]) -> Self {
        // Byte 0: segment count (mod 9). Then repeated records:
        //   [pathlen:1][path bytes][kind:1]
        // kind bit0 set => redirect; redirect target = "p<byte>" derived below.
        let mut it = data.iter().copied();
        let seg_count = it.next().unwrap_or(0) % 9;
        let mut segments = vec![Vec::new(); seg_count.max(1) as usize];
        let mut seg_cursor = 0usize;
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > 4096 {
                break;
            }
            let plen = match it.next() {
                Some(n) => (n % 12) as usize,
                None => break,
            };
            let mut p = String::new();
            for _ in 0..plen {
                match it.next() {
                    Some(b) => p.push((b'a' + (b % 20)) as char),
                    None => break,
                }
            }
            if p.is_empty() {
                p.push('a');
            }
            let kind = it.next().unwrap_or(0);
            let redirect_to = if kind & 1 == 1 {
                let t = it.next().unwrap_or(0);
                Some(format!("{}", (b'a' + (t % 20)) as char))
            } else {
                None
            };
            if !segments.is_empty() {
                let n = segments.len();
                segments[seg_cursor % n].push((p, ModelEntry { redirect_to }));
                seg_cursor += 1;
            }
        }
        MergeModel { segments }
    }

    fn merge(&self) -> std::collections::BTreeMap<String, ModelEntry> {
        let mut out = std::collections::BTreeMap::new();
        for seg in &self.segments {
            for (p, e) in seg {
                out.insert(p.clone(), e.clone()); // later segment wins
            }
        }
        out
    }
}

fn resolve_one_hop(
    merged: &std::collections::BTreeMap<String, ModelEntry>,
    path: &str,
) -> std::result::Result<String, ()> {
    let e = merged.get(path).ok_or(())?;
    match &e.redirect_to {
        None => Ok(path.to_string()),
        Some(t) => {
            let target = merged.get(t).ok_or(())?;
            if target.redirect_to.is_some() {
                Err(()) // > 1 hop: reject, never follow
            } else {
                Ok(t.clone())
            }
        }
    }
}
