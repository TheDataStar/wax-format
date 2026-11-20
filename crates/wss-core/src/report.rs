//! The import report: everything an importer could not carry, and why, in
//! plain words (WSS spec: "nothing is dropped silently").
//!
//! This is **not** the Contract §11 build report. That report's warning codes
//! are a closed list the catalog's intake routes on, and WSS only ever writes
//! codes that are on it. The notes here are WSS's own, finer-grained account
//! for the person running the build; they are written beside the pack as
//! `<pack>.wss-report.json` and never read by a box. Where a note has a §11
//! equivalent, the importer raises that code too. Where it has none
//! (a crawl page that failed, a PDF without a text layer), the build report
//! stays silent until the Contract adopts a code (WSS spec, open question 3:
//! `uncaptured_page`, `external_request`, `transcode_failed`,
//! `unsupported_source_item`).

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Notes kept in full; past this only the per-kind counts grow, so a crawl of
/// a million failing URLs cannot exhaust memory.
pub const MAX_KEPT_NOTES: usize = 10_000;

/// What happened to one source item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// Not carried: its media type is not one a pack carries yet (§11
    /// `unsupported_mimetype`).
    UnsupportedType,
    /// Not carried: its path cannot be a pack path (§11 `invalid_path`).
    InvalidPath,
    /// Not carried: its path starts with `_assets/` or `_meta/`, which the
    /// pack reserves (§11 `reserved_prefix_collision`).
    ReservedPrefix,
    /// Carried, but its text could not be extracted, so search will not find
    /// it (a scanned PDF, a damaged file).
    NoSearchableText,
    /// Not carried: a page the crawl could not fetch, was told not to fetch
    /// by robots.txt, or did not reach before a limit.
    UncapturedPage,
    /// A reference to a page or file outside the pack was left as it was;
    /// it will not open offline.
    ExternalReference,
    /// A file inside the source that is itself a source (a .zim or .epub in a
    /// folder) was carried as a download, not opened up.
    NotExpanded,
    /// Carried as it is, but the book scrambles this font (EPUB font
    /// obfuscation), so browsers show another font.
    ObfuscatedFont,
}

impl NoteKind {
    pub fn plain(self) -> &'static str {
        match self {
            NoteKind::UnsupportedType => "not carried: packs do not carry this kind of file yet",
            NoteKind::InvalidPath => "not carried: its name cannot be used as a path inside a pack",
            NoteKind::ReservedPrefix => "not carried: its folder name is reserved inside packs",
            NoteKind::NoSearchableText => "carried, but search cannot find it: no text could be read from it",
            NoteKind::UncapturedPage => "not carried: the page could not be captured",
            NoteKind::ExternalReference => "left pointing outside the pack: it will not open offline",
            NoteKind::NotExpanded => "carried as a download, not opened up as a pack of its own",
            NoteKind::ObfuscatedFont => "carried, but the book scrambles this font, so browsers use another one",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub kind: NoteKind,
    /// The source item: a file path relative to the source, or a URL.
    pub item: String,
    pub detail: String,
}

/// Notes plus exact counts per kind.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Notes {
    pub counts: BTreeMap<NoteKind, u64>,
    pub notes: Vec<Note>,
}

impl Notes {
    pub fn add(&mut self, kind: NoteKind, item: impl Into<String>, detail: impl Into<String>) {
        *self.counts.entry(kind).or_default() += 1;
        if self.notes.len() < MAX_KEPT_NOTES {
            self.notes.push(Note { kind, item: item.into(), detail: detail.into() });
        }
    }
    pub fn count(&self, kind: NoteKind) -> u64 {
        self.counts.get(&kind).copied().unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }
    pub fn total(&self) -> u64 {
        self.counts.values().sum()
    }
}

/// `<pack>.wss-report.json`, beside the pack.
pub fn path_for(archive: &Path) -> PathBuf {
    let name = archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    archive.with_file_name(format!("{name}.wss-report.json"))
}

/// The file written beside the pack.
#[derive(Debug, Clone, Serialize)]
pub struct ImportReport<'a> {
    pub wss_report_version: u32,
    pub importer: &'a str,
    pub source: String,
    pub archive_uuid: String,
    pub zstd_level: i32,
    pub notes: &'a Notes,
    /// Original URL → pack path, for captured websites. Empty otherwise.
    pub url_map: &'a BTreeMap<String, String>,
}

impl ImportReport<'_> {
    pub fn write(&self, archive: &Path) -> anyhow::Result<PathBuf> {
        let p = path_for(archive);
        std::fs::write(&p, serde_json::to_string_pretty(self)? + "\n")?;
        Ok(p)
    }
}
