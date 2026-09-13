//! The conversion pipeline: ZIM in, `.wax` out (Track B §4, §20).
//!
//! ```text
//! open ZIM ─► plan (classify every dirent) ─► resolve redirects ─► read + rewrite
//!          ─► derive manifest ─► wax_builder::build_from_entries ─► build report
//! ```
//!
//! Every dirent gets exactly one [`Fate`]. Content that is emitted is
//! addressable by its `(namespace, url)` so that in-document references can be
//! rewritten to the canonical path (§20).

use crate::lang::languages_field;
use crate::paths::{bare_mime, canonicalize, is_article_mime, Disposition};
use crate::png::placeholder_icon;
use crate::rewrite::{rewrite_css, rewrite_html};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use wax_builder::config::{ManifestConfig, CATEGORIES, MIN_HW_TIERS};
use wax_builder::{build_from_entries, BuildContext, Warnings, WriteOptions, WriteReport};
use wax_core::{Compression, EntryInput};
use zim::{MimeType, Namespace, Target, Zim};

/// Canonical path of the extracted illustration (§20).
pub const ICON_PATH: &str = "_assets/icon.png";

/// Warning codes this converter emits into the §11 build report.
///
/// Contract §11 pins a **closed** vocabulary; a builder must not ship a code
/// that is not on it. The eight below are the whole list. Conditions this
/// converter detects that have no pinned code are folded into the nearest one
/// ([`warn::INVALID_PATH`] for a canonical-path collision) or kept out of the
/// report and surfaced only in [`ConvertStats`].
pub mod warn {
    /// An entry was skipped because v0 does not carry its media type.
    /// References to it are left intact.
    pub const UNSUPPORTED_MIMETYPE: &str = "unsupported_mimetype";
    /// A redirect chain closed on itself and was dropped.
    pub const REDIRECT_CYCLE: &str = "redirect_cycle";
    /// A redirect's terminus does not exist and was dropped.
    pub const REDIRECT_DANGLING: &str = "redirect_dangling";
    /// A source path could not be canonicalized into a valid pack path and was
    /// dropped. Also covers two dirents canonicalizing to the same path (the
    /// later one is dropped) until the Contract lists a distinct code.
    pub const INVALID_PATH: &str = "invalid_path";
    /// A source path collided with a reserved prefix (`_assets/`, `_meta/`).
    pub const RESERVED_PREFIX_COLLISION: &str = "reserved_prefix_collision";
    /// The source stated no license and the operator supplied one. Always
    /// accompanies `license_review_required`.
    pub const LICENSE_OPERATOR_SUPPLIED: &str = "license_operator_supplied";
    /// The source carried neither Creator nor Publisher and the operator
    /// supplied the credit line.
    pub const ATTRIBUTION_OPERATOR_SUPPLIED: &str = "attribution_operator_supplied";
    /// The source carried no illustration and a placeholder was generated.
    pub const ICON_GENERATED: &str = "icon_generated";
}

/// A canonical path is usable only if it is already in SPEC §6.1's stored
/// form: `normalize_path` must accept it *and* leave it unchanged, because the
/// href resolver and the manifest look entries up by this exact string.
fn is_valid_wax_path(p: &str) -> bool {
    wax_core::writer::normalize_path(p).map(|n| n == p).unwrap_or(false)
}

/// Mimetypes v0 converts. Everything else is [`warn::UNSUPPORTED_MIMETYPE`].
/// "Text + image" plus what a page needs to render: stylesheets, scripts,
/// fonts, JSON/XML data, subtitles (text). Audio and video are out (§4 scope).
pub fn is_supported_mime(bare: &str) -> bool {
    bare.starts_with("text/")
        || bare.starts_with("image/")
        || bare.starts_with("font/")
        || matches!(
            bare,
            "application/javascript"
                | "application/x-javascript"
                | "application/ecmascript"
                | "application/json"
                | "application/ld+json"
                | "application/xml"
                | "application/xhtml+xml"
                | "application/rss+xml"
                | "application/atom+xml"
                | "application/font-woff"
                | "application/font-woff2"
                | "application/x-font-woff"
                | "application/x-font-ttf"
                | "application/x-font-opentype"
                | "application/vnd.ms-fontobject"
                | "application/manifest+json"
                | "image/svg+xml"
        )
}

/// Compression per entry: already-entropy-coded formats verbatim, text zstd.
/// Same policy as wax-builder's extension list, keyed on mimetype instead.
fn compression_for(bare: &str) -> Compression {
    if bare == "image/svg+xml" {
        return Compression::Zstd;
    }
    if bare.starts_with("image/")
        || bare.starts_with("font/")
        || bare.starts_with("application/font")
        || bare.starts_with("application/x-font")
        || bare == "application/vnd.ms-fontobject"
    {
        Compression::None
    } else {
        Compression::Zstd
    }
}

/// What the operator supplies. `category` and `min_hw_tier` have no source in
/// a ZIM (§20) and are required; nothing is defaulted.
#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub category: String,
    pub min_hw_tier: String,
    /// Used **only** when the ZIM carries no `License` metadata. Real Wikipedia
    /// ZIMs (mwoffliner 1.17) omit it, and Contract §11 makes a blank license
    /// a hard failure — so without this the flagship content cannot convert.
    /// Never overrides a license the ZIM does state. Flagged in the B1 summary.
    pub license_if_absent: Option<String>,
    /// Used **only** when the ZIM carries neither `Creator` nor `Publisher`
    /// (Track B §20; Contract §11). Never overrides a credit the ZIM states.
    pub attribution_if_absent: Option<String>,
    pub created_at: Option<u64>,
    pub archive_uuid: Option<[u8; 16]>,
    pub sign_key: Option<PathBuf>,
}

impl ConvertOptions {
    /// Fail early on the two operator-supplied enums, with the Contract's
    /// domains in the message, before any ZIM work is done.
    pub fn validate(&self) -> Result<()> {
        if !CATEGORIES.contains(&self.category.as_str()) {
            bail!(
                "--category {:?} is not permitted; must be one of: {} (Contract §11)",
                self.category,
                CATEGORIES.join(", ")
            );
        }
        if !MIN_HW_TIERS.contains(&self.min_hw_tier.as_str()) {
            bail!(
                "--min-hw-tier {:?} is not permitted; must be one of: {} (Contract §2). \
                 `generic` is box-reported only and never declarable by a pack.",
                self.min_hw_tier,
                MIN_HW_TIERS.join(", ")
            );
        }
        Ok(())
    }
}

/// Where a redirect chain ends: the terminus dirent and its canonical path,
/// or the warning code for why it was dropped.
type ChainOutcome = std::result::Result<(u32, String), &'static str>;

/// Per-dirent outcome of planning.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fate {
    /// Content to emit at this canonical path.
    Content(String),
    /// A redirect dirent; resolved in a second step.
    Redirect { target_idx: u32 },
    /// Not emitted. `Some(code)` raises that build-report warning; `None` is a
    /// by-design non-emission (`X/`, `W/`) that is counted in stats only.
    Skipped(Option<&'static str>),
}

/// Everything derived from the ZIM's `M/` metadata that the manifest needs.
#[derive(Debug, Clone, Default)]
pub struct Derived {
    pub name: String,
    pub version: String,
    pub attribution: String,
    pub license: String,
    pub languages: Option<String>,
    pub entry_point: String,
    /// `Some(key)` when an `Illustration_*` metadata entry supplied the icon.
    pub icon_source: Option<String>,
}

/// Conversion counts beyond what the build report carries.
#[derive(Debug, Clone, Default)]
pub struct ConvertStats {
    pub dirents: usize,
    pub content_emitted: usize,
    pub redirects_emitted: usize,
    pub hrefs_rewritten: usize,
    pub bytes_in: u64,
    /// `X/` search-index dirents, not copied by design (Track B §4). Not a
    /// warning: nothing was lost that WAX could have used.
    pub search_index_entries: usize,
    /// `W/` well-known dirents, not copied by design (the main page reaches
    /// the manifest through `entry_point`).
    pub wellknown_entries: usize,
    /// Two dirents canonicalized to the same path; reported as `invalid_path`.
    pub path_collisions: usize,
    /// `Language` had no BCP-47 mapping; `languages` omitted. Observable in
    /// the manifest rather than the report.
    pub language_unmapped: bool,
}

#[derive(Debug)]
pub struct ConvertReport {
    pub write: WriteReport,
    pub derived: Derived,
    pub stats: ConvertStats,
    pub warnings: Warnings,
}

fn mime_str(m: &MimeType) -> Option<&str> {
    match m {
        MimeType::Type(s) => Some(s.as_str()),
        _ => None,
    }
}

fn metadata_string(z: &Zim, key: &str) -> Result<Option<String>> {
    match z.metadata(key)? {
        Some(c) => {
            let v = c.to_vec()?;
            Ok(Some(String::from_utf8_lossy(&v).trim().to_string()).filter(|s| !s.is_empty()))
        }
        None => Ok(None),
    }
}

/// ZIM `Date` (`YYYY-MM-DD`) → CalVer `YYYY.MM.D` (Contract §11: a reformat,
/// not a computation — the zero-padded month carries straight across; the day
/// becomes the release counter, unpadded, starting at 1).
pub fn calver_from_zim_date(date: &str) -> Result<String> {
    let date = date.trim();
    let parts: Vec<&str> = date.split('-').collect();
    let [y, m, d] = parts.as_slice() else {
        bail!("ZIM Date {date:?} is not YYYY-MM-DD");
    };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 || !parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())) {
        bail!("ZIM Date {date:?} is not YYYY-MM-DD");
    }
    let day: u32 = d.parse().unwrap_or(0);
    if day == 0 {
        bail!("ZIM Date {date:?} has day 00; CalVer's release counter starts at 1");
    }
    let v = format!("{y}.{m}.{day}");
    wax_builder::config::check_calver(&v).map_err(|e| anyhow!("derived version {v:?} is not CalVer: {e}"))?;
    Ok(v)
}

/// Run the conversion.
pub fn convert(zim_path: &Path, output: &Path, opts: &ConvertOptions) -> Result<ConvertReport> {
    opts.validate()?;
    let z = Zim::new(zim_path).with_context(|| format!("opening ZIM {}", zim_path.display()))?;
    let mut warnings = Warnings::new();
    let mut stats = ConvertStats::default();

    // ------------------------------------------------------------------
    // 1. Plan: classify every dirent.
    // ------------------------------------------------------------------
    let mut fates: Vec<Fate> = Vec::new();
    let mut keys: Vec<(u8, String)> = Vec::new(); // idx -> (ns, url)
    let mut mimes: Vec<Option<String>> = Vec::new(); // idx -> bare mime
    let mut titles: Vec<Option<String>> = Vec::new();
    let mut by_key: HashMap<(u8, String), u32> = HashMap::new();
    let mut canon_owner: HashMap<String, u32> = HashMap::new();

    for (idx, e) in z.iterate_by_urls().enumerate() {
        let e = e.with_context(|| format!("reading dirent #{idx}"))?;
        let idx = idx as u32;
        stats.dirents += 1;
        let key = (e.namespace.as_byte(), e.url.clone());
        by_key.insert(key.clone(), idx);
        keys.push(key);
        // Track B §20: for a non-text/html entry, a title of exactly "null" is
        // mwoffliner's placeholder and is treated as absent. Deliberately not
        // applied to articles, so an article titled "Null" survives.
        let is_article = mime_str(&e.mime_type).map(bare_mime).as_deref().is_some_and(is_article_mime);
        let title = Some(e.title.trim().to_string())
            .filter(|t| !t.is_empty())
            .filter(|t| is_article || t != "null");
        titles.push(title);

        let fate = match e.target {
            Some(Target::Redirect(t)) => {
                mimes.push(None);
                Fate::Redirect { target_idx: t }
            }
            _ => {
                let mime = mime_str(&e.mime_type).map(bare_mime);
                mimes.push(mime.clone());
                match canonicalize(e.namespace, &e.url, mime.as_deref().unwrap_or("")) {
                    Disposition::SearchIndex => {
                        stats.search_index_entries += 1;
                        Fate::Skipped(None)
                    }
                    Disposition::WellKnown => {
                        stats.wellknown_entries += 1;
                        Fate::Skipped(None)
                    }
                    Disposition::ReservedPrefixCollision => {
                        Fate::Skipped(Some(warn::RESERVED_PREFIX_COLLISION))
                    }
                    Disposition::Emit(_) if !mime.as_deref().is_some_and(is_supported_mime) => {
                        Fate::Skipped(Some(warn::UNSUPPORTED_MIMETYPE))
                    }
                    Disposition::Emit(path) => {
                        if !is_valid_wax_path(&path) {
                            Fate::Skipped(Some(warn::INVALID_PATH))
                        } else if canon_owner.contains_key(&path) {
                            stats.path_collisions += 1;
                            Fate::Skipped(Some(warn::INVALID_PATH))
                        } else {
                            canon_owner.insert(path.clone(), idx);
                            Fate::Content(path)
                        }
                    }
                }
            }
        };
        if let Fate::Skipped(Some(code)) = &fate {
            warnings.bump(code);
        }
        fates.push(fate);
    }

    // ------------------------------------------------------------------
    // 2. Redirects: flatten to the terminus; drop cycles and dangling (§20).
    //
    // Two phases on purpose: chains are walked against the *original*
    // classification, so a redirect dropped in this step never turns a later
    // chain through it from "cycle" into "dangling". Every alias points at the
    // terminus content, never at an intermediate redirect.
    // ------------------------------------------------------------------
    let mut redirect_targets: Vec<Option<(String, String)>> = vec![None; fates.len()]; // idx -> (alias canonical, target canonical)
    let mut resolved: Vec<(usize, ChainOutcome)> = Vec::new();
    for idx in 0..fates.len() {
        let Fate::Redirect { target_idx } = fates[idx] else { continue };
        let mut seen: HashSet<u32> = HashSet::new();
        seen.insert(idx as u32);
        let mut cur = target_idx;
        let terminus = loop {
            if !seen.insert(cur) {
                break Err(warn::REDIRECT_CYCLE);
            }
            match fates.get(cur as usize) {
                Some(Fate::Redirect { target_idx }) => cur = *target_idx,
                Some(Fate::Content(path)) => break Ok((cur, path.clone())),
                Some(Fate::Skipped(_)) | None => break Err(warn::REDIRECT_DANGLING),
            }
        };
        resolved.push((idx, terminus));
    }
    for (idx, terminus) in resolved {
        match terminus {
            Ok((tidx, target_path)) => {
                // the alias lands where its target's mime would put it
                let (ns, url) = &keys[idx];
                let target_mime = mimes[tidx as usize].clone().unwrap_or_default();
                match canonicalize(Namespace::from(*ns), url, &target_mime) {
                    Disposition::Emit(alias_path) => {
                        if !is_valid_wax_path(&alias_path) {
                            warnings.bump(warn::INVALID_PATH);
                            fates[idx] = Fate::Skipped(Some(warn::INVALID_PATH));
                        } else if canon_owner.contains_key(&alias_path) {
                            stats.path_collisions += 1;
                            warnings.bump(warn::INVALID_PATH);
                            fates[idx] = Fate::Skipped(Some(warn::INVALID_PATH));
                        } else {
                            canon_owner.insert(alias_path.clone(), idx as u32);
                            redirect_targets[idx] = Some((alias_path, target_path));
                        }
                    }
                    Disposition::ReservedPrefixCollision => {
                        warnings.bump(warn::RESERVED_PREFIX_COLLISION);
                        fates[idx] = Fate::Skipped(Some(warn::RESERVED_PREFIX_COLLISION));
                    }
                    Disposition::SearchIndex | Disposition::WellKnown => {
                        // a redirect living in X/ or W/ (e.g. W/mainPage): not content
                        stats.wellknown_entries += 1;
                        fates[idx] = Fate::Skipped(None);
                    }
                }
            }
            Err(code) => {
                warnings.bump(code);
                fates[idx] = Fate::Skipped(Some(code));
            }
        }
    }

    // Resolver for href rewriting: (ns, url) -> emitted canonical path.
    // Redirect aliases resolve to the alias path (wax-core follows the one hop).
    let canonical_of = |idx: u32| -> Option<String> {
        match &fates[idx as usize] {
            Fate::Content(p) => Some(p.clone()),
            Fate::Redirect { .. } => redirect_targets[idx as usize].as_ref().map(|(a, _)| a.clone()),
            Fate::Skipped(_) => None,
        }
    };
    let resolver = |ns: Namespace, url: &str| -> Option<String> {
        by_key.get(&(ns.as_byte(), url.to_string())).and_then(|i| canonical_of(*i))
    };

    // ------------------------------------------------------------------
    // 3. Read content, rewrite references, build entries.
    // ------------------------------------------------------------------
    let mut entries: Vec<EntryInput> = Vec::with_capacity(fates.len());
    for (idx, fate) in fates.iter().enumerate() {
        match fate {
            Fate::Content(path) => {
                let e = z.get_by_url_index(idx as u32)?;
                let bare = mimes[idx].clone().unwrap_or_default();
                let raw = z
                    .entry_content(&e)?
                    .ok_or_else(|| anyhow!("dirent #{idx} {:?} has no content", e.url))?
                    .to_vec()?;
                stats.bytes_in += raw.len() as u64;
                let bytes = if is_article_mime(&bare) || bare == "text/css" {
                    let text = String::from_utf8_lossy(&raw);
                    let (out, rs) = if bare == "text/css" {
                        rewrite_css(&text, e.namespace, &e.url, &resolver)
                    } else {
                        rewrite_html(&text, e.namespace, &e.url, &resolver)
                    };
                    stats.hrefs_rewritten += rs.rewritten;
                    out.into_bytes()
                } else {
                    raw
                };
                let mut ei = EntryInput::data(path.clone(), bytes, compression_for(&bare));
                if let Some(m) = mime_str(&e.mime_type) {
                    ei = ei.with_mime(m);
                }
                if let Some(t) = &titles[idx] {
                    ei = ei.with_title(t.clone());
                }
                entries.push(ei);
                stats.content_emitted += 1;
            }
            Fate::Redirect { .. } => {
                if let Some((alias, target)) = &redirect_targets[idx] {
                    let mut ei = EntryInput::redirect(alias.clone(), target.clone());
                    if let Some(t) = &titles[idx] {
                        ei = ei.with_title(t.clone());
                    }
                    entries.push(ei);
                    stats.redirects_emitted += 1;
                }
            }
            Fate::Skipped(_) => {}
        }
    }

    // ------------------------------------------------------------------
    // 4. Manifest derivations (§20).
    // ------------------------------------------------------------------
    let name = metadata_string(&z, "Title")?
        .ok_or_else(|| anyhow!("ZIM has no Title metadata; manifest.name is required (Contract §11)"))?;

    let version = calver_from_zim_date(
        &metadata_string(&z, "Date")?
            .ok_or_else(|| anyhow!("ZIM has no Date metadata; manifest.version is required (Contract §11)"))?,
    )?;

    let attribution = match metadata_string(&z, "Creator")?.or(metadata_string(&z, "Publisher")?) {
        Some(a) => a,
        None => match &opts.attribution_if_absent {
            Some(a) if !a.trim().is_empty() => {
                warnings.bump(warn::ATTRIBUTION_OPERATOR_SUPPLIED);
                a.trim().to_string()
            }
            _ => bail!(
                "ZIM carries neither Creator nor Publisher metadata, and attribution is \
                 required (Contract §11 — CC-BY compliance depends on the credit line). \
                 Pass --attribution <credit line> to state it (used only because the ZIM \
                 has none)."
            ),
        },
    };

    let mut license_operator_supplied = false;
    let license = match metadata_string(&z, "License")? {
        Some(l) => l,
        None => match &opts.license_if_absent {
            Some(l) if !l.trim().is_empty() => {
                license_operator_supplied = true;
                warnings.bump(warn::LICENSE_OPERATOR_SUPPLIED);
                l.trim().to_string()
            }
            _ => String::new(),
        },
    };
    if license.trim().is_empty() {
        bail!(
            "ZIM has no License metadata and a blank license is a hard build failure \
             (Contract §11). Current Wikipedia ZIMs omit it: pass --license <SPDX id or \
             text> to state the license the ZIM left out (it is used only because the \
             ZIM has none, and the pack is always routed to license review)."
        );
    }

    let mut languages = None;
    if let Some(l) = metadata_string(&z, "Language")? {
        languages = languages_field(&l);
        if languages.is_none() {
            stats.language_unmapped = true;
        }
    }

    // entry_point ← main page, through redirects, must be emitted content.
    let main = z
        .main_page()?
        .ok_or_else(|| anyhow!("ZIM has no main page; manifest.entry_point is required (Contract §11)"))?;
    let main = z.resolve(main)?;
    let entry_point = resolver(main.namespace, &main.url).ok_or_else(|| {
        anyhow!(
            "ZIM main page {}/{} was not emitted (skipped or unsupported); entry_point \
             cannot be derived",
            main.namespace.as_byte() as char,
            main.url
        )
    })?;

    // icon ← Illustration_*, else a legacy favicon, else a placeholder (§20).
    let (icon_bytes, icon_source) = find_illustration(&z)?;
    if icon_source.is_none() {
        warnings.bump(warn::ICON_GENERATED);
    }
    let derived = Derived {
        name,
        version,
        attribution,
        license,
        languages,
        entry_point,
        icon_source,
    };
    if canon_owner.contains_key(ICON_PATH) {
        // a source entry already lives at _assets/icon.png; the extracted
        // illustration takes precedence and the source entry is dropped
        stats.path_collisions += 1;
        warnings.bump(warn::INVALID_PATH);
        entries.retain(|e| e.path != ICON_PATH);
    }
    entries.push(EntryInput::data(ICON_PATH, icon_bytes, Compression::None).with_mime("image/png"));

    let manifest = ManifestConfig {
        name: Some(derived.name.clone()),
        icon: Some(ICON_PATH.to_string()),
        category: Some(opts.category.clone()),
        license: Some(derived.license.clone()),
        attribution: Some(derived.attribution.clone()),
        version: Some(derived.version.clone()),
        min_hw_tier: Some(opts.min_hw_tier.clone()),
        entry_point: Some(derived.entry_point.clone()),
        languages: derived.languages.clone(),
        ..ManifestConfig::default()
    };

    // ------------------------------------------------------------------
    // 5. Build.
    // ------------------------------------------------------------------
    // Deterministic order: wax-core lays blobs down in the order given.
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    // skipped_count = source items dropped for a warned reason (§11's example:
    // 1204 skipped ↔ unsupported_mimetype 1204). By-design non-emission of X/
    // and W/ is not a drop and is not counted.
    let skipped = warnings.count(warn::UNSUPPORTED_MIMETYPE)
        + warnings.count(warn::REDIRECT_CYCLE)
        + warnings.count(warn::REDIRECT_DANGLING)
        + warnings.count(warn::RESERVED_PREFIX_COLLISION)
        + warnings.count(warn::INVALID_PATH);

    let write = build_from_entries(
        output,
        entries,
        &manifest,
        &WriteOptions {
            created_at: opts.created_at,
            sign_key: opts.sign_key.clone(),
            archive_uuid: opts.archive_uuid,
        },
        BuildContext {
            builder_version: Some(format!("zim2wax {}", env!("CARGO_PKG_VERSION"))),
            warnings: warnings.clone(),
            skipped_count: skipped,
            // Contract §11: an operator's license claim is reviewed, not trusted
            force_license_review: license_operator_supplied,
        },
    )
    .with_context(|| format!("building {}", output.display()))?;

    Ok(ConvertReport {
        write,
        derived,
        stats,
        warnings,
    })
}

/// The icon bytes and where they came from. Order: `Illustration_48x48@1`,
/// any other `Illustration_*` metadata entry, a legacy `-/favicon`, and
/// finally the generated placeholder (§20).
fn find_illustration(z: &Zim) -> Result<(Vec<u8>, Option<String>)> {
    let mut keys = z.metadata_keys()?;
    keys.sort();
    let preferred = "Illustration_48x48@1";
    let candidates: Vec<String> = std::iter::once(preferred.to_string())
        .chain(keys.into_iter().filter(|k| k.starts_with("Illustration_") && k != preferred))
        .collect();
    for k in candidates {
        if let Some(c) = z.metadata(&k)? {
            let bytes = c.to_vec()?;
            if !bytes.is_empty() {
                return Ok((bytes, Some(format!("M/{k}"))));
            }
        }
    }
    // legacy scheme: "-/favicon" (often a redirect to I/favicon.png)
    if let Some(fav) = z.get_by_path(Namespace::Layout, "favicon")? {
        let fav = z.resolve(fav)?;
        if let Some(c) = z.entry_content(&fav)? {
            let bytes = c.to_vec()?;
            if !bytes.is_empty() {
                return Ok((bytes, Some(format!("{}/{}", fav.namespace.as_byte() as char, fav.url))));
            }
        }
    }
    Ok((placeholder_icon(), None))
}

/// Read-only look at what a conversion *would* derive, for `zim2wax probe`.
pub struct Probe {
    pub version: (u16, u16),
    pub dirents: u32,
    pub metadata: BTreeMap<String, String>,
    pub main_page: Option<String>,
    pub derived_version: Result<String>,
    pub languages: Option<String>,
    pub illustration: Option<String>,
}

pub fn probe(zim_path: &Path) -> Result<Probe> {
    let z = Zim::new(zim_path).with_context(|| format!("opening ZIM {}", zim_path.display()))?;
    let mut metadata = BTreeMap::new();
    for k in z.metadata_keys()? {
        if k.starts_with("Illustration_") {
            continue;
        }
        if let Some(v) = metadata_string(&z, &k)? {
            metadata.insert(k, v);
        }
    }
    let main_page = match z.main_page()? {
        Some(m) => {
            let m = z.resolve(m)?;
            Some(format!("{}/{}", m.namespace.as_byte() as char, m.url))
        }
        None => None,
    };
    let derived_version = match metadata.get("Date") {
        Some(d) => calver_from_zim_date(d),
        None => Err(anyhow!("no Date metadata")),
    };
    let languages = metadata.get("Language").and_then(|l| languages_field(l));
    let illustration = find_illustration(&z)?.1;
    Ok(Probe {
        version: (z.header.version_major, z.header.version_minor),
        dirents: z.header.article_count,
        metadata,
        main_page,
        derived_version,
        languages,
        illustration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calver_from_zim_date_reformats() {
        assert_eq!(calver_from_zim_date("2026-08-20").unwrap(), "2026.08.20");
        assert_eq!(calver_from_zim_date("2020-11-05").unwrap(), "2020.11.5", "day unpadded");
        assert_eq!(calver_from_zim_date("2021-06-02").unwrap(), "2021.06.2");
        assert!(calver_from_zim_date("2026-8-20").is_err());
        assert!(calver_from_zim_date("2026-13-01").is_err());
        assert!(calver_from_zim_date("2026-08-00").is_err());
        assert!(calver_from_zim_date("2026-08").is_err());
        assert!(calver_from_zim_date("20260820").is_err());
    }

    #[test]
    fn supported_mime_scope() {
        for m in ["text/html", "text/css", "text/vtt", "image/webp", "image/svg+xml", "application/javascript", "font/woff2"] {
            assert!(is_supported_mime(m), "{m}");
        }
        for m in ["video/webm", "application/ogg", "audio/mpeg", "application/pdf", "application/octet-stream+xapian"] {
            assert!(!is_supported_mime(m), "{m}");
        }
    }

    #[test]
    fn option_validation_uses_contract_enums() {
        let mut o = ConvertOptions {
            category: "reference".into(),
            min_hw_tier: "pi_4".into(),
            license_if_absent: None,
            attribution_if_absent: None,
            created_at: None,
            archive_uuid: None,
            sign_key: None,
        };
        assert!(o.validate().is_ok());
        o.category = "science".into();
        assert!(o.validate().unwrap_err().to_string().contains("--category"));
        o.category = "reference".into();
        o.min_hw_tier = "generic".into();
        let e = o.validate().unwrap_err().to_string();
        assert!(e.contains("--min-hw-tier") && e.contains("generic"), "{e}");
    }
}
