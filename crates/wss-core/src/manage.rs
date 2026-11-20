//! Managing packs that already exist: inspect, compare editions, publish an
//! update as an appended segment, compact (gc), sign and verify.
//!
//! Every function returns a plain, serialisable value, so the CLI prints it
//! and a GUI or the SaaS shows it without parsing text.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wax_core::header::flag;
use wax_core::{Compression, Entry, WaxReader};

// ---------------------------------------------------------------------------
// inspect
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PackInfo {
    pub path: PathBuf,
    pub file_bytes: u64,
    pub format_version: String,
    pub archive_uuid: String,
    pub created_at: u64,
    pub segments: usize,
    pub signed_flag: bool,
    pub signature_sidecar: Option<PathBuf>,
    pub manifest: BTreeMap<String, String>,
    pub entries: u64,
    pub redirects: u64,
    /// Compressed bytes per codec (`none`, `zstd`, `zstd-dict`).
    pub stored_bytes_by_codec: BTreeMap<String, u64>,
    pub uncompressed_bytes: u64,
    pub dictionary_bytes: Option<usize>,
    pub search_tokenizer: Option<String>,
    /// The header was recovered from its mirror (SPEC v0.13 §2.4).
    pub header_from_mirror: bool,
}

pub fn inspect(pack: &Path) -> Result<PackInfo> {
    let r = WaxReader::open(pack).with_context(|| format!("opening {}", pack.display()))?;
    let h = *r.header();
    let mut info = PackInfo {
        path: pack.to_path_buf(),
        file_bytes: r.file_size(),
        format_version: format!("{}.{}", h.version_major, h.version_minor),
        archive_uuid: wax_builder::uuid_text(&h.archive_uuid),
        created_at: h.created_at,
        segments: r.segment_count(),
        signed_flag: h.has_flag(flag::IS_SIGNED),
        signature_sidecar: Some(wax_builder::sign::sidecar_path(pack)).filter(|p| p.is_file()),
        manifest: r.manifest().clone(),
        entries: 0,
        redirects: 0,
        stored_bytes_by_codec: BTreeMap::new(),
        uncompressed_bytes: 0,
        dictionary_bytes: r.dictionary().map(<[u8]>::len),
        search_tokenizer: r.search_tokenizer().map(str::to_string),
        header_from_mirror: r.header_source() == wax_core::HeaderSource::Mirror,
    };
    for e in r.entries() {
        let e = e?;
        if e.is_redirect() {
            info.redirects += 1;
            continue;
        }
        info.entries += 1;
        *info.stored_bytes_by_codec.entry(e.compression.clone()).or_default() += e.length;
        info.uncompressed_bytes += e.uncompressed_length;
    }
    Ok(info)
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

/// How two editions differ, entry by entry. Content is compared by its
/// SHA-256 (SPEC §3.1), never by its compressed bytes, so a different
/// compression level or dictionary is not a change.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EditionDiff {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: u64,
    /// Manifest keys whose value differs (`version` included).
    pub manifest_changes: BTreeMap<String, (Option<String>, Option<String>)>,
    /// Uncompressed bytes of the added and changed entries.
    pub changed_bytes: u64,
}

impl EditionDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

fn same(a: &Entry, b: &Entry) -> bool {
    a.redirect_to == b.redirect_to && a.sha256 == b.sha256 && a.mime == b.mime && a.title == b.title
}

/// Compare `old` with `new`. One merge pass over both indexes in path order
/// (`entries()` is sorted, SPEC §5.5), so memory does not grow with the pack
/// beyond the three path lists returned.
pub fn diff(old: &Path, new: &Path) -> Result<EditionDiff> {
    let a = WaxReader::open(old).with_context(|| format!("opening {}", old.display()))?;
    let b = WaxReader::open(new).with_context(|| format!("opening {}", new.display()))?;
    let mut d = EditionDiff::default();
    let keys: std::collections::BTreeSet<&String> = a.manifest().keys().chain(b.manifest().keys()).collect();
    for k in keys {
        let (x, y) = (a.manifest().get(k), b.manifest().get(k));
        if x != y {
            d.manifest_changes.insert(k.clone(), (x.cloned(), y.cloned()));
        }
    }
    let mut ia = a.entries().peekable();
    let mut ib = b.entries().peekable();
    loop {
        match (ia.peek(), ib.peek()) {
            (None, None) => break,
            (Some(Err(_)), _) => return Err(ia.next().unwrap().unwrap_err().into()),
            (_, Some(Err(_))) => return Err(ib.next().unwrap().unwrap_err().into()),
            (Some(Ok(x)), None) => {
                d.removed.push(x.path.clone());
                ia.next();
            }
            (None, Some(Ok(y))) => {
                d.changed_bytes += y.uncompressed_length;
                d.added.push(y.path.clone());
                ib.next();
            }
            (Some(Ok(x)), Some(Ok(y))) => match x.path.as_bytes().cmp(y.path.as_bytes()) {
                std::cmp::Ordering::Less => {
                    d.removed.push(x.path.clone());
                    ia.next();
                }
                std::cmp::Ordering::Greater => {
                    d.changed_bytes += y.uncompressed_length;
                    d.added.push(y.path.clone());
                    ib.next();
                }
                std::cmp::Ordering::Equal => {
                    if same(x, y) {
                        d.unchanged += 1;
                    } else {
                        d.changed_bytes += y.uncompressed_length;
                        d.changed.push(y.path.clone());
                    }
                    ia.next();
                    ib.next();
                }
            },
        }
    }
    Ok(d)
}

// ---------------------------------------------------------------------------
// update: publish a candidate edition as an appended segment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct UpdateReport {
    pub diff: EditionDiff,
    /// False when the candidate matched the pack and nothing was written.
    pub appended: bool,
    pub segments: usize,
    /// Appended updates still possible before a new base edition is needed
    /// (a pack holds at most 64 segments, SPEC §5.1, §7.4).
    pub segment_budget_left: usize,
    /// Bytes a box downloads for this update: everything past the old
    /// logical end, plus the new header (SPEC §7.4).
    pub update_bytes: u64,
    pub file_bytes: u64,
    pub signature_sidecar: Option<PathBuf>,
    /// Plain-language facts the person should know before publishing.
    pub cautions: Vec<String>,
}

/// Bring `pack` up to `candidate` by appending one segment that holds only
/// what changed: new and changed entries, a tombstone per removed entry, and
/// search rows and passages for the changed pages (SPEC v0.12 §5.5.1, §5.7,
/// §7.4). The pack's dictionary is carried forward (SPEC §5.9), so entries are
/// framed exactly as a fresh build with that dictionary would frame them.
///
/// When the candidate's manifest differs (a new `version`, most often), the
/// segment carries the candidate's whole manifest (SPEC v0.13 §5.6.1), whose
/// `version` must be newer. Refused when the update cannot be an append: a
/// different pack identity or a different search tokenizer. Those need a new
/// base edition, which boxes download in full.
pub fn update(pack: &Path, candidate: &Path, zstd_level: i32, created_at: Option<u64>, sign_key: Option<&Path>) -> Result<UpdateReport> {
    let cur = WaxReader::open(pack).with_context(|| format!("opening {}", pack.display()))?;
    let cand = WaxReader::open(candidate).with_context(|| format!("opening {}", candidate.display()))?;
    if cur.header().archive_uuid != cand.header().archive_uuid {
        bail!(
            "the candidate is a different pack (archive_uuid {} vs {}). Build it with this pack's \
             identity (`wss update` does this for you), or publish it as a new pack.",
            wax_builder::uuid_text(&cand.header().archive_uuid),
            wax_builder::uuid_text(&cur.header().archive_uuid)
        );
    }
    let d = diff(pack, candidate)?;
    let mut cautions = Vec::new();
    // An appended edition may carry a whole new manifest (SPEC v0.13
    // §5.6.1). Like `wax-builder append`, a changed manifest must move its
    // CalVer `version` forward: an edition that changes what the pack says
    // about itself is a newer edition.
    let new_manifest = if d.manifest_changes.is_empty() {
        None
    } else {
        check_version_advances(cur.manifest().get("version"), cand.manifest().get("version"), &d)?;
        Some(cand.manifest().clone())
    };
    if cur.search_tokenizer().is_some() && cand.search_tokenizer().is_some() && cur.search_tokenizer() != cand.search_tokenizer() {
        bail!(
            "the candidate's search index uses {:?} but the pack's uses {:?}; an appended index must use \
             the pack's tokenizer (SPEC v0.12 §5.7). Rebuild as a new base edition.",
            cand.search_tokenizer(),
            cur.search_tokenizer()
        );
    }
    if cur.dictionary().is_some() && cand.dictionary().is_some() && cur.dictionary() != cand.dictionary() {
        cautions.push(
            "the candidate was built with a different dictionary than the pack. Its entries are \
             re-framed with the pack's own dictionary, so they decode correctly, but build later \
             candidates with the pack's dictionary carried forward (`wss update` does this)."
                .into(),
        );
    }
    if d.is_empty() && new_manifest.is_none() {
        return Ok(UpdateReport {
            diff: d,
            appended: false,
            segments: cur.segment_count(),
            segment_budget_left: wax_core::MAX_SEGMENTS.saturating_sub(cur.segment_count()),
            update_bytes: 0,
            file_bytes: cur.file_size(),
            signature_sidecar: None,
            cautions,
        });
    }
    let old_logical_end = cur.logical_end();
    let has_search = cur.has_search_index() || cand.has_search_index();
    let signed = cur.header().has_flag(flag::IS_SIGNED);
    drop(cur);

    let mut w = wax_core::WaxWriter::new([0; 16]).zstd_level(zstd_level);
    if let Some(t) = created_at {
        w = w.created_at(t);
    }
    let mut sw = w.open_append(pack).with_context(|| format!("opening {} for an update", pack.display()))?;
    let use_dict = sw.dictionary().is_some();
    let html_text = |bytes: &[u8]| wax_core::text::html_to_text(&String::from_utf8_lossy(bytes));
    for path in d.added.iter().chain(d.changed.iter()) {
        let e = cand.entry(path)?;
        if let Some(to) = &e.redirect_to {
            sw.add_redirect(path.clone(), to.clone(), e.title.clone())?;
            continue;
        }
        let bytes = cand.read(path)?;
        let mut codec = Compression::parse(path, &e.compression)?;
        if codec == Compression::ZstdDict && !use_dict {
            codec = Compression::Zstd;
        }
        let mut meta = wax_core::EntryMeta::new(path.clone()).compression(codec);
        if let Some(m) = &e.mime {
            meta = meta.mime(m.clone());
        }
        if let Some(t) = &e.title {
            meta = meta.title(t.clone());
        }
        sw.add_entry(meta, &mut &bytes[..])?;
        // Re-index exactly what the candidate indexed: a page with passages
        // was indexed by the importer, and the passage rule needs HTML.
        if has_search && cand.passages(path).is_ok() {
            sw.index_text(path, e.title.as_deref(), &html_text(&bytes))?;
            sw.index_passages(path, &bytes)?;
        }
    }
    for path in &d.removed {
        sw.add_tombstone(path)?;
    }
    if let Some(m) = &new_manifest {
        // Contract §11: icon and entry_point must name live entries of the
        // updated pack.
        for key in ["icon", "entry_point"] {
            let value = m.get(key).map(String::as_str).unwrap_or_default();
            if !sw.resolves(value)? {
                bail!("the new manifest's {key} {value:?} does not name an entry in the updated pack (contract §11)");
            }
        }
        sw.set_manifest(m);
    }
    sw.finish().with_context(|| format!("appending the update to {}", pack.display()))?;

    let sidecar = match sign_key {
        Some(k) => Some(wax_builder::sign::sign(pack, k)?),
        None => {
            if signed {
                cautions.push(
                    "the pack is marked signed but no key was given, so its old signature no longer \
                     matches. Sign it (`wss sign`) before publishing."
                        .into(),
                );
            }
            None
        }
    };
    let after = WaxReader::open(pack)?;
    Ok(UpdateReport {
        diff: d,
        appended: true,
        segments: after.segment_count(),
        segment_budget_left: wax_core::MAX_SEGMENTS.saturating_sub(after.segment_count()),
        update_bytes: after.file_size() - old_logical_end + wax_core::HEADER_LEN as u64,
        file_bytes: after.file_size(),
        signature_sidecar: sidecar,
        cautions,
    })
}

/// CalVer `YYYY.MM.N` as a comparable triple.
fn calver(v: &str) -> Option<(u32, u32, u32)> {
    wax_builder::config::check_calver(v).ok()?;
    let mut p = v.split('.').map(|x| x.parse::<u32>().ok());
    Some((p.next()??, p.next()??, p.next()??))
}

fn check_version_advances(old: Option<&String>, new: Option<&String>, d: &EditionDiff) -> Result<()> {
    let changed: Vec<&str> = d.manifest_changes.keys().map(String::as_str).collect();
    match (old.and_then(|v| calver(v)), new.and_then(|v| calver(v))) {
        (Some(o), Some(n)) if n > o => Ok(()),
        (Some(_), Some(_)) => bail!(
            "the manifest changed ({}) but its version {} is not newer than the pack's {}. An update \
             that changes the manifest is a newer edition, so raise the version (CalVer YYYY.MM.N, \
             contract §11).",
            changed.join(", "),
            new.map(String::as_str).unwrap_or("none"),
            old.map(String::as_str).unwrap_or("none")
        ),
        // The pack's own version predates the CalVer rule; the candidate's
        // was validated when it was built.
        (None, Some(_)) => Ok(()),
        (_, None) => bail!("the candidate's manifest has no valid CalVer version (contract §11)"),
    }
}

// ---------------------------------------------------------------------------
// gc, sign, verify
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct GcReport {
    pub tail_only: bool,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub segments_before: usize,
    /// True when the result is a new base edition that must be re-signed.
    pub new_base_edition: bool,
    pub search_dropped: bool,
}

/// Reclaim space (SPEC v0.13 §7.6). `tail_only` truncates an orphaned tail in
/// place and keeps the signature valid; otherwise the pack is compacted into
/// one segment, a new base edition, written beside it and renamed over it.
pub fn gc(pack: &Path, output: Option<&Path>, tail_only: bool, drop_search: bool) -> Result<GcReport> {
    let before = std::fs::metadata(pack)?.len();
    if tail_only {
        if output.is_some() {
            bail!("tail-only gc works in place; it takes no output");
        }
        let segments_before = WaxReader::open(pack)?.segment_count();
        let s = wax_core::compact::reclaim_tail(pack)?;
        return Ok(GcReport {
            tail_only: true,
            bytes_before: before,
            bytes_after: s.file_length,
            segments_before,
            new_base_edition: false,
            search_dropped: false,
        });
    }
    let opts = wax_core::compact::CompactOptions { drop_search, ..Default::default() };
    let (target, tmp) = match output {
        Some(o) => (o.to_path_buf(), None),
        None => {
            let mut t = pack.as_os_str().to_owned();
            t.push(".gc-tmp");
            (pack.to_path_buf(), Some(PathBuf::from(t)))
        }
    };
    let s = wax_core::compact::compact(pack, tmp.as_deref().unwrap_or(&target), opts)?;
    if let Some(t) = &tmp {
        std::fs::File::open(t)?.sync_all()?;
        std::fs::rename(t, &target)?;
    }
    Ok(GcReport {
        tail_only: false,
        bytes_before: s.input_bytes,
        bytes_after: s.output_bytes,
        segments_before: s.segments_before,
        new_base_edition: true,
        search_dropped: s.search_dropped,
    })
}

/// Undo the newest appended update, byte for byte (SPEC v0.12 §7.5). The
/// previous edition's signature sidecar, if kept, verifies again.
pub fn rollback(pack: &Path) -> Result<wax_core::RollbackStats> {
    Ok(wax_core::rollback_last_update(pack)?)
}

/// Sign `pack` with a minisign secret key (SPEC §8), writing `<pack>.minisig`.
pub fn sign(pack: &Path, seckey: &Path) -> Result<PathBuf> {
    wax_builder::sign::sign(pack, seckey)
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyOutcome {
    pub ok: bool,
    pub entries_checked: usize,
    pub bad_entries: Vec<(String, String)>,
    pub signature: Option<String>,
    pub signature_error: Option<String>,
    pub header_note: Option<String>,
}

/// Read every entry back (hash check, SPEC §3.1), check passages and the
/// stored entry count, and the signature when a public key is given.
pub fn verify(pack: &Path, pubkey: Option<&wax_builder::sign::PubKey>, require_signature: bool) -> Result<VerifyOutcome> {
    let r = wax_builder::verify_pack(pack, pubkey, require_signature)?;
    Ok(VerifyOutcome {
        ok: r.ok(),
        entries_checked: r.entries_checked,
        bad_entries: r.bad_entries.clone(),
        signature: r.signature.as_ref().map(|s| s.trusted_comment.clone()),
        signature_error: r.signature_error.clone(),
        header_note: r.header_note.clone(),
    })
}
