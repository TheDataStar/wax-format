//! The entry points a front end calls: recognise a source, convert it, and
//! publish a refreshed source as an update to an existing pack.
//!
//! ```text
//! convert(source, out, recipe)   any source → a first edition (or a new base)
//! refresh(pack, source, recipe)  same source, newer content → appended update
//! ```

use crate::build::{build_staged, DictionaryOutcome};
use crate::importers;
use crate::manage::{self, UpdateReport};
use crate::recipe::Recipe;
use crate::report::{ImportReport, Notes};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// What kind of source a path or URL is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Zim,
    /// A folder: an HTML site, documents, or a built client-side web app.
    Folder,
    Epub,
    Pdf,
    /// An http(s) URL: website capture.
    Website,
}

impl SourceKind {
    pub fn importer(self) -> &'static str {
        match self {
            SourceKind::Zim => "zim",
            SourceKind::Folder => "html",
            SourceKind::Epub => "epub",
            SourceKind::Pdf => "pdf",
            SourceKind::Website => "web",
        }
    }
}

/// ZIM magic number, 72173914 little-endian (openZIM file format).
const ZIM_MAGIC: [u8; 4] = [0x5A, 0x49, 0x4D, 0x04];

/// Recognise a source by what it is, not by its name: URL scheme, folder,
/// or the file's first bytes.
pub fn detect(source: &str) -> Result<SourceKind> {
    let lower = source.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Ok(SourceKind::Website);
    }
    let p = Path::new(source);
    if p.is_dir() {
        return Ok(SourceKind::Folder);
    }
    let mut head = [0u8; 64];
    let n = std::fs::File::open(p)
        .with_context(|| format!("{source} is not a URL, a folder or a readable file"))?
        .read(&mut head)?;
    let head = &head[..n];
    if head.starts_with(&ZIM_MAGIC) {
        return Ok(SourceKind::Zim);
    }
    if head.starts_with(b"%PDF-") {
        return Ok(SourceKind::Pdf);
    }
    // An EPUB's first zip member is the uncompressed `mimetype` file.
    if head.starts_with(b"PK\x03\x04") && head.len() >= 58 && &head[30..38] == b"mimetype" && &head[38..58] == b"application/epub+zip" {
        return Ok(SourceKind::Epub);
    }
    if head.starts_with(b"PK\x03\x04") && lower.ends_with(".epub") {
        return Ok(SourceKind::Epub);
    }
    bail!(
        "{source} is not a source WSS can convert yet: give a .zim, .epub or .pdf file, a folder, \
         or an http(s) URL"
    )
}

/// Everything a front end shows after a conversion.
#[derive(Debug, Clone, Serialize)]
pub struct ConvertOutcome {
    pub kind: SourceKind,
    pub archive: PathBuf,
    pub archive_uuid: String,
    pub entries: u64,
    pub redirects: u64,
    pub indexed: u64,
    pub passages: u64,
    pub dictionary: String,
    pub zstd_level: i32,
    pub license: Option<String>,
    pub license_review_required: bool,
    pub signature_sidecar: Option<PathBuf>,
    /// Contract §11 codes and counts, as in the build report.
    pub warnings: BTreeMap<String, u64>,
    pub build_report: PathBuf,
    pub import_report: PathBuf,
    pub notes: Notes,
}

/// Convert `source` into a pack at `output`.
pub fn convert(source: &str, output: &Path, recipe: &Recipe) -> Result<ConvertOutcome> {
    let kind = detect(source)?;
    recipe.validate()?;
    if kind == SourceKind::Zim {
        let r = importers::zim::convert(Path::new(source), output, recipe)?;
        let notes = Notes::default();
        let empty = BTreeMap::new();
        let import_report = ImportReport {
            wss_report_version: 1,
            importer: "zim",
            source: source.to_string(),
            archive_uuid: r.write.archive_uuid_text(),
            zstd_level: recipe.build.zstd_level,
            notes: &notes,
            url_map: &empty,
        }
        .write(output)?;
        let indexed = match &r.search {
            zim2wax::SearchIndexOutcome::Built { documents, .. } => *documents,
            _ => 0,
        };
        let dictionary = dictionary_summary(output)?;
        return Ok(ConvertOutcome {
            kind,
            archive: r.write.archive.clone(),
            archive_uuid: r.write.archive_uuid_text(),
            entries: r.stats.content_emitted as u64,
            redirects: r.stats.redirects_emitted as u64,
            indexed,
            passages: r.stats.passage_entries as u64,
            dictionary,
            zstd_level: recipe.build.zstd_level,
            license: r.write.license.as_ref().map(|l| l.license().to_string()),
            license_review_required: r.write.license_review_required(),
            signature_sidecar: r.write.sidecar.clone(),
            warnings: r.write.warnings.iter().map(|(k, v)| (k.to_string(), v)).collect(),
            build_report: r.write.report_path.clone(),
            import_report,
            notes,
        });
    }

    let staged = match kind {
        SourceKind::Folder => importers::html::import(Path::new(source), recipe)?,
        SourceKind::Epub => importers::epub::import(Path::new(source), recipe)?,
        SourceKind::Pdf => importers::pdf::import(Path::new(source), recipe)?,
        SourceKind::Website => importers::web::import(source, recipe)?,
        SourceKind::Zim => unreachable!(),
    };
    // Build through `<output>.partial` and rename only on success (as
    // zim2wax does): a failed or killed build never leaves something at the
    // output path that looks like a pack.
    let mut partial = output.as_os_str().to_os_string();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let partial_report = wax_builder::BuildReport::path_for(&partial);
    let _ = std::fs::remove_file(&partial);
    let _ = std::fs::remove_file(&partial_report);
    let (mut w, stats, staged) = match build_staged(staged, &partial, recipe) {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            let _ = std::fs::remove_file(&partial_report);
            return Err(e);
        }
    };
    std::fs::rename(&partial, output).with_context(|| format!("publishing {}", output.display()))?;
    w.archive = output.to_path_buf();
    w.report_path = publish_build_report(&partial_report, output)?;
    if let Some(sig) = w.sidecar.take() {
        let dest = wax_builder::sign::sidecar_path(output);
        std::fs::rename(&sig, &dest).with_context(|| format!("publishing {}", dest.display()))?;
        w.sidecar = Some(dest);
    }
    let import_report = ImportReport {
        wss_report_version: 1,
        importer: staged.importer,
        source: source.to_string(),
        archive_uuid: w.archive_uuid_text(),
        zstd_level: stats.zstd_level,
        notes: &staged.notes,
        url_map: &staged.url_map,
    }
    .write(output)?;
    Ok(ConvertOutcome {
        kind,
        archive: w.archive.clone(),
        archive_uuid: w.archive_uuid_text(),
        entries: stats.entries,
        redirects: stats.redirects,
        indexed: stats.indexed,
        passages: stats.passages,
        dictionary: match stats.dictionary {
            DictionaryOutcome::Carried => "carried from the previous edition".into(),
            DictionaryOutcome::Trained { samples } => format!("trained on {samples} pages"),
            DictionaryOutcome::None => "none (too little text to pay for one)".into(),
        },
        zstd_level: stats.zstd_level,
        license: w.license.as_ref().map(|l| l.license().to_string()),
        license_review_required: w.license_review_required(),
        signature_sidecar: w.sidecar.clone(),
        warnings: w.warnings.iter().map(|(k, v)| (k.to_string(), v)).collect(),
        build_report: w.report_path.clone(),
        import_report,
        notes: staged.notes,
    })
}

/// Move the build report written beside `<output>.partial` to beside
/// `output`, with its `archive_filename` naming the published file.
fn publish_build_report(partial_report: &Path, output: &Path) -> Result<PathBuf> {
    let final_report = wax_builder::BuildReport::path_for(output);
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(partial_report)?)?;
    if let Some(name) = output.file_name() {
        v["archive_filename"] = serde_json::Value::String(name.to_string_lossy().into_owned());
    }
    std::fs::write(&final_report, serde_json::to_string_pretty(&v)? + "\n")?;
    std::fs::remove_file(partial_report)?;
    Ok(final_report)
}

fn dictionary_summary(p: &Path) -> Result<String> {
    Ok(match wax_core::WaxReader::open(p)?.dictionary() {
        Some(d) => format!("{} KiB", d.len() / 1024),
        None => "none".into(),
    })
}

/// What a refresh did: the candidate's conversion plus the update.
#[derive(Debug, Clone, Serialize)]
pub struct RefreshOutcome {
    pub candidate: ConvertOutcome,
    pub update: UpdateReport,
}

/// Re-convert `source` as the next edition of `pack` and append only what
/// changed (WSS spec, "How an edition is made"). The candidate is built with
/// the pack's identity and its dictionary carried forward (SPEC §5.9), in a
/// temporary file beside the pack, which is removed afterwards.
pub fn refresh(pack: &Path, source: &str, recipe: &Recipe) -> Result<RefreshOutcome> {
    let r = wax_core::WaxReader::open(pack).with_context(|| format!("opening {}", pack.display()))?;
    let uuid = wax_builder::uuid_text(&r.header().archive_uuid);
    drop(r);
    if let Some(u) = recipe.build.archive_uuid.as_deref() {
        if wax_builder::parse_uuid(u)? != wax_builder::parse_uuid(&uuid)? {
            bail!("the recipe pins archive_uuid {u}, but the pack is {uuid}");
        }
    }
    let mut cand_recipe = recipe.clone();
    cand_recipe.build.archive_uuid = Some(uuid);
    cand_recipe.build.carry_dictionary_from = Some(pack.to_path_buf());
    cand_recipe.build.sign_key = None; // the candidate is never published
    let dir = tempfile::Builder::new()
        .prefix(".wss-candidate")
        .tempdir_in(pack.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")))?;
    let cand = dir.path().join("candidate.wax");
    let candidate = convert(source, &cand, &cand_recipe)?;
    let update = manage::update(pack, &cand, recipe.build.zstd_level, recipe.created_at(), recipe.sign_key().as_deref())?;
    Ok(RefreshOutcome { candidate, update })
}
