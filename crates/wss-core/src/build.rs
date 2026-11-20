//! Staged source → `.wax` pack, the same way for every importer.
//!
//! ```text
//! resolve manifest (source facts + recipe, refuse what neither states)
//!   ─► PackStream::create at the recipe's zstd level (19 by default, ADR-0003)
//!   ─► dictionary: carried from the previous edition, or trained from a
//!      path-ordered sample (SPEC §5.9)
//!   ─► every staged item in path order: entry or redirect, search text
//!      (`wax-bigram-16`, SPEC v0.13 §5.7.2) and passages (§5.10)
//!   ─► finish: §11 checks, sign, build report
//! ```

use crate::importers::Staged;
use crate::recipe::Recipe;
use crate::stage::{Body, Codec, Search};
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use wax_builder::config::{classify_license, ManifestConfig};
use wax_builder::{BuildContext, EntryMeta, PackStream, WriteOptions, WriteReport};
use wax_core::Compression;

/// zim2wax's training bounds (SPEC §5.9): ≤2048 samples, each ≤64 KiB,
/// ≤16 MiB total, and no dictionary below 32 samples. ADR-0003 measured the
/// codec decision against exactly this rule, so WSS uses the same one.
const DICT_SAMPLE_MAX: usize = 2048;
const DICT_SAMPLE_BYTES: usize = 16 * 1024 * 1024;
const DICT_SAMPLE_EACH: usize = 64 * 1024;
const DICT_MIN_SAMPLES: usize = 32;

/// Where a generated placeholder icon goes (zim2wax's path, Track B §20).
pub const ICON_PATH: &str = zim2wax::ICON_PATH;

/// What a build did, beyond the §11 build report.
#[derive(Debug, Clone)]
pub struct BuildStats {
    pub entries: u64,
    pub redirects: u64,
    pub indexed: u64,
    pub passages: u64,
    pub dictionary: DictionaryOutcome,
    pub zstd_level: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictionaryOutcome {
    Carried,
    Trained { samples: usize },
    /// Too little text for a dictionary to pay for itself.
    None,
}

/// Turn source facts and the recipe into the §11 manifest. Source-stated
/// values win; the recipe fills only what the source leaves blank; anything
/// neither states stops the build (the project's "refuse rather than guess").
///
/// Returns the manifest and whether the license was operator-supplied (which
/// forces license review, Contract §11).
pub fn resolve_manifest(staged: &mut Staged, recipe: &Recipe) -> Result<(ManifestConfig, bool)> {
    use zim2wax::convert::warn;
    let p = &recipe.pack;
    let f = staged.facts.clone();

    // The recipe's name and languages win: a person naming their pack, or
    // correcting a page's <title> or lang attribute, means it. (A ZIM never
    // reaches here; zim2wax derives its own.) License and attribution are the
    // other way round: what the source states wins, and an operator-supplied
    // value is reviewed (Contract §11).
    let name = p
        .name
        .clone()
        .or(f.name)
        .ok_or_else(|| anyhow!("name is required: the source states no title, so set one in the recipe (--name)"))?;
    let version = p.version.clone().ok_or_else(|| {
        anyhow!("version is required: CalVer YYYY.MM.N (Contract §11). This source states no edition, so set --version")
    })?;

    let (license, license_operator_supplied) = match (f.license, &p.license) {
        (Some(l), _) => (l, false),
        (None, Some(l)) => {
            staged.warnings.add(warn::LICENSE_OPERATOR_SUPPLIED, 1);
            (l.clone(), true)
        }
        (None, None) => bail!(
            "license is required: the source states none, and a blank license is a hard failure \
             (Contract §11). Set --license with an SPDX id or the licence text; it will be marked \
             for review."
        ),
    };
    let attribution = match (f.attribution, &p.attribution) {
        (Some(a), _) => a,
        (None, Some(a)) => {
            staged.warnings.add(warn::ATTRIBUTION_OPERATOR_SUPPLIED, 1);
            a.clone()
        }
        (None, None) => bail!(
            "attribution is required: the source names no author or publisher (Contract §11). Set --attribution."
        ),
    };

    let entry_point = match (&p.entry_point, f.entry_point) {
        (Some(e), _) => e.clone(),
        (None, Some(e)) => e,
        (None, None) => bail!("entry_point is required: this source has no obvious start page, so set --entry-point"),
    };

    let icon = match f.icon {
        Some(i) => i,
        None => {
            staged
                .staging
                .add(ICON_PATH, Some("image/png"), None, &zim2wax::png::placeholder_icon(), Codec::None, Search::No)?;
            staged.warnings.add(warn::ICON_GENERATED, 1);
            ICON_PATH.to_string()
        }
    };

    let m = ManifestConfig {
        name: Some(name),
        icon: Some(icon),
        category: p.category.clone(),
        license: Some(license),
        attribution: Some(attribution),
        version: Some(version),
        min_ram_bytes: p.min_ram_bytes,
        min_storage_bytes: p.min_storage_bytes,
        arch: p.arch.clone(),
        entry_point: Some(entry_point),
        guest_accessible: p.guest_accessible,
        gpu: p.gpu.clone(),
        languages: p.languages.clone().or(f.languages),
        ..Default::default()
    };
    m.validate(None).context("invalid manifest")?;
    Ok((m, license_operator_supplied))
}

/// Build the pack at `output` from `staged`.
pub fn build_staged(mut staged: Staged, output: &Path, recipe: &Recipe) -> Result<(WriteReport, BuildStats, Staged)> {
    recipe.validate()?;
    if staged.staging.is_empty()? {
        bail!("nothing to build: the source yielded no item a pack can carry (see the notes above)");
    }
    let (manifest, operator_license) = resolve_manifest(&mut staged, recipe)?;
    let level = recipe.build.zstd_level;

    let warnings = std::mem::take(&mut staged.warnings);
    let mut pack = PackStream::create(
        output,
        &manifest,
        &WriteOptions {
            created_at: recipe.created_at(),
            sign_key: recipe.sign_key(),
            archive_uuid: recipe.archive_uuid()?,
        },
        BuildContext {
            builder_version: Some(format!("wss {} ({})", env!("CARGO_PKG_VERSION"), staged.importer)),
            warnings,
            skipped_count: staged.skipped,
            force_license_review: operator_license
                || classify_license(manifest.license.as_deref().unwrap_or_default()).review_required(),
        },
    )
    .with_context(|| format!("creating {}", output.display()))?;
    pack.set_zstd_level(level)?;

    let dictionary = match recipe.build.carry_dictionary_from.as_deref() {
        Some(prev) => {
            let r = wax_core::WaxReader::open(prev)
                .with_context(|| format!("opening the previous edition {}", prev.display()))?;
            match r.dictionary() {
                Some(d) => {
                    pack.carry_dictionary(d)?;
                    DictionaryOutcome::Carried
                }
                None => DictionaryOutcome::None,
            }
        }
        None => {
            let samples = staged.staging.dict_samples(DICT_SAMPLE_MAX, DICT_SAMPLE_BYTES, DICT_SAMPLE_EACH)?;
            if samples.len() >= DICT_MIN_SAMPLES {
                pack.train_dictionary(samples.iter().map(Vec::as_slice))?;
                DictionaryOutcome::Trained { samples: samples.len() }
            } else {
                DictionaryOutcome::None
            }
        }
    };
    let use_dict = pack.dictionary().is_some();
    let search = recipe.build.search_index;
    if search {
        pack.set_search_tokenizer(wax_core::tokenize::WAX_BIGRAM)?;
    }

    let mut stats = BuildStats {
        entries: 0,
        redirects: 0,
        indexed: 0,
        passages: 0,
        dictionary,
        zstd_level: level,
    };
    staged.staging.for_each(|item| {
        match item.body {
            Body::Redirect { to } => {
                pack.add_redirect(item.path, to, item.title)?;
                stats.redirects += 1;
            }
            Body::Data { bytes, codec } => {
                let compression = match codec {
                    Codec::Text if use_dict => Compression::ZstdDict,
                    Codec::Text | Codec::Zstd => Compression::Zstd,
                    Codec::None => Compression::None,
                };
                let html = (search && item.search == Search::Html && !item.path.starts_with(CONTENTS_PREFIX))
                    .then(|| String::from_utf8_lossy(&bytes).into_owned());
                let title = item.title.clone().or_else(|| html.as_deref().and_then(crate::importers::html_title));
                let mut meta = EntryMeta::new(item.path.clone()).compression(compression);
                if let Some(m) = &item.mime {
                    meta = meta.mime(m.clone());
                }
                if let Some(t) = &title {
                    meta = meta.title(t.clone());
                }
                pack.add_entry(meta, &mut &bytes[..])?;
                stats.entries += 1;
                if let Some(h) = html {
                    pack.index_text(&item.path, title.as_deref(), &wax_core::text::html_to_text(&h))?;
                    stats.indexed += 1;
                    pack.index_passages(&item.path, &bytes)?;
                    stats.passages += 1;
                }
            }
        }
        Ok(())
    })?;

    let report = pack.finish()?;
    Ok((report, stats, staged))
}

/// Generated contents pages are navigation, not content: they are not put in
/// the search index (a query would otherwise match every chapter title twice).
pub const CONTENTS_PREFIX: &str = "_assets/wss/contents";
