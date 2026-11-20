//! Importers: one per kind of source (WSS spec, "Importers").
//!
//! Every importer except ZIM stages its source ([`crate::stage`]) and reports
//! what the source itself states ([`SourceFacts`]); [`crate::build`] then
//! turns that into a pack the same way for all of them. ZIM is delegated
//! whole to `zim2wax`, so a ZIM converted by WSS and one converted on a box
//! are the same code (WSS spec: "one importer, three places").

pub mod epub;
pub mod html;
pub mod pdf;
pub mod web;
pub mod zim;

use crate::report::{NoteKind, Notes};
use crate::stage::{Codec, Search, Staging};
use std::collections::BTreeMap;
use wax_builder::Warnings;

/// What a source states about itself. `None` means the source is silent and
/// the recipe must supply the value (or the build stops).
#[derive(Debug, Clone, Default)]
pub struct SourceFacts {
    pub name: Option<String>,
    pub license: Option<String>,
    pub attribution: Option<String>,
    pub languages: Option<String>,
    /// The importer's start page, used when the recipe names none.
    pub entry_point: Option<String>,
    /// A staged image to use as the pack icon.
    pub icon: Option<String>,
}

/// An importer's output: the staging area plus everything the build and the
/// reports need.
pub struct Staged {
    pub importer: &'static str,
    pub staging: Staging,
    pub facts: SourceFacts,
    pub notes: Notes,
    /// Contract §11 warning codes only (closed list).
    pub warnings: Warnings,
    /// Source items not carried, for the build report's `skipped_count`.
    pub skipped: u64,
    /// Original URL → pack path (website capture only).
    pub url_map: BTreeMap<String, String>,
}

impl Staged {
    pub fn new(importer: &'static str) -> anyhow::Result<Self> {
        Ok(Staged {
            importer,
            staging: Staging::create()?,
            facts: SourceFacts::default(),
            notes: Notes::default(),
            warnings: Warnings::new(),
            skipped: 0,
            url_map: BTreeMap::new(),
        })
    }

    /// Record a skipped item: the plain note, the §11 code, and the skipped
    /// count.
    pub fn skip(&mut self, kind: NoteKind, item: impl Into<String>, detail: impl Into<String>) {
        self.skip_n(kind, item, detail, 1);
    }

    /// [`skip`](Self::skip) for `n` items summarised in one note (a crawl
    /// that stopped at its page limit with `n` URLs still queued).
    pub fn skip_n(&mut self, kind: NoteKind, item: impl Into<String>, detail: impl Into<String>, n: u64) {
        self.skipped += n;
        self.note_n(kind, item, detail, n);
    }

    /// Record something about an item that *was* carried (no text for
    /// search, kept unopened, references left pointing elsewhere): the plain
    /// note and its §11 code.
    pub fn note(&mut self, kind: NoteKind, item: impl Into<String>, detail: impl Into<String>) {
        self.note_n(kind, item, detail, 1);
    }

    /// [`note`](Self::note) standing for `n` occurrences in the build report.
    pub fn note_n(&mut self, kind: NoteKind, item: impl Into<String>, detail: impl Into<String>, n: u64) {
        self.warnings.add(contract_code(kind), n);
        self.notes.add(kind, item, detail);
    }

    /// Stage one source file at `path`, applying the shared policy: path must
    /// be a valid pack path outside the reserved prefixes, and the type must
    /// be one packs carry. Returns whether it was staged.
    pub fn stage_file(
        &mut self,
        item: &str,
        path: &str,
        mime: &str,
        title: Option<&str>,
        bytes: &[u8],
    ) -> anyhow::Result<bool> {
        self.stage_file_opts(item, path, mime, title, bytes, true)
    }

    /// [`stage_file`](Self::stage_file), with `searchable = false` keeping an
    /// HTML page out of the search index (an EPUB's navigation document).
    pub fn stage_file_opts(
        &mut self,
        item: &str,
        path: &str,
        mime: &str,
        title: Option<&str>,
        bytes: &[u8],
        searchable: bool,
    ) -> anyhow::Result<bool> {
        let Some(path) = self.check_path(item, path) else {
            return Ok(false);
        };
        let bare = bare_mime(mime);
        if !is_carried_mime(&bare) {
            self.skip(NoteKind::UnsupportedType, item, format!("type {bare}"));
            return Ok(false);
        }
        let search = if searchable && is_html_mime(&bare) { Search::Html } else { Search::No };
        if !self.staging.add(&path, Some(&bare), title, bytes, codec_for(&bare), search)? {
            self.skip(NoteKind::InvalidPath, item, format!("{path} is already in the pack"));
            return Ok(false);
        }
        Ok(true)
    }

    /// The pack path for `path`, or a skip note. Valid means SPEC §6.1's
    /// stored form, unchanged by normalisation, and not under `_assets/` or
    /// `_meta/` (which the pack reserves for what the builder generates).
    pub fn check_path(&mut self, item: &str, path: &str) -> Option<String> {
        match wax_core::writer::normalize_path(path) {
            Ok(n) if n == path => {}
            Ok(n) => {
                self.skip(NoteKind::InvalidPath, item, format!("{path:?} is not in stored form (would be {n:?})"));
                return None;
            }
            Err(e) => {
                self.skip(NoteKind::InvalidPath, item, e.to_string());
                return None;
            }
        }
        if is_reserved(path) {
            self.skip(NoteKind::ReservedPrefix, item, format!("{path} starts with a reserved folder"));
            return None;
        }
        Some(path.to_string())
    }
}

/// The Contract §11 warning code for each note kind. Every kind has one:
/// the first three were already on the closed list; the other four were
/// added to it with this importer set (contract §11, 2026-11-20).
pub fn contract_code(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::UnsupportedType => zim2wax::convert::warn::UNSUPPORTED_MIMETYPE,
        NoteKind::InvalidPath => zim2wax::convert::warn::INVALID_PATH,
        NoteKind::ReservedPrefix => zim2wax::convert::warn::RESERVED_PREFIX_COLLISION,
        NoteKind::UncapturedPage => code::UNCAPTURED_PAGE,
        NoteKind::ExternalReference => code::EXTERNAL_REFERENCE,
        NoteKind::NoSearchableText => code::NO_SEARCHABLE_TEXT,
        NoteKind::NotExpanded | NoteKind::ObfuscatedFont => code::KEPT_UNOPENED,
    }
}

/// The codes this importer set added to the Contract §11 list.
pub mod code {
    /// A page or file a crawl found but did not capture: the fetch failed,
    /// robots.txt asked not to, a limit was reached, or it redirected off
    /// the site. Counted per URL; each is also a skipped item.
    pub const UNCAPTURED_PAGE: &str = "uncaptured_page";
    /// References to another site were left as they are and will not open
    /// offline. Counted per reference.
    pub const EXTERNAL_REFERENCE: &str = "external_reference";
    /// An item was carried, but no text could be read from it for search (a
    /// scanned PDF).
    pub const NO_SEARCHABLE_TEXT: &str = "no_searchable_text";
    /// An item was carried as a file but could not be opened up: an EPUB
    /// inside a folder, a font a book obfuscates.
    pub const KEPT_UNOPENED: &str = "kept_unopened";
}

/// Paths the pack reserves for what builders generate (Contract §11
/// `reserved_prefix_collision`).
pub fn is_reserved(path: &str) -> bool {
    path.starts_with("_assets/") || path.starts_with("_meta/")
}

/// Where generated pages go. Under `_assets/`, so they can never collide with
/// a source file.
pub const GENERATED_DIR: &str = "_assets/wss/";

pub fn bare_mime(m: &str) -> String {
    m.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

pub fn is_html_mime(bare: &str) -> bool {
    bare == "text/html" || bare == "application/xhtml+xml"
}

/// What WSS carries: zim2wax's v0 scope (text, images, fonts, stylesheets,
/// scripts, data), plus PDF and EPUB files kept as originals. Audio and
/// video wait for the transcoding importer (WSS release 2).
pub fn is_carried_mime(bare: &str) -> bool {
    zim2wax::convert::is_supported_mime(bare) || bare == "application/pdf" || bare == "application/epub+zip"
}

/// Compression per entry, the zim2wax policy keyed on mimetype (ADR-0003):
/// text against the dictionary, TTF/OTF as plain zstd, entropy-coded media
/// verbatim. PDF and EPUB are stored verbatim: both are mostly compressed
/// already and the gain is unmeasured, so they stay `none` until measured.
pub fn codec_for(bare: &str) -> Codec {
    if bare == "image/svg+xml" {
        return Codec::Text;
    }
    if matches!(
        bare,
        "font/ttf"
            | "font/otf"
            | "font/sfnt"
            | "font/collection"
            | "application/x-font-ttf"
            | "application/x-font-opentype"
            | "application/x-font-truetype"
    ) {
        return Codec::Zstd;
    }
    if bare.starts_with("image/")
        || bare.starts_with("font/")
        || bare.starts_with("application/font")
        || bare.starts_with("application/x-font")
        || bare == "application/vnd.ms-fontobject"
        || bare == "application/pdf"
        || bare == "application/epub+zip"
    {
        Codec::None
    } else {
        Codec::Text
    }
}

/// Mime for a file name, by extension.
pub fn guess_mime(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    // mime_guess maps .xhtml correctly but gives nothing useful for a few
    // web files packs commonly carry.
    if lower.ends_with(".mjs") {
        return "application/javascript".into();
    }
    if lower.ends_with(".webmanifest") {
        return "application/manifest+json".into();
    }
    mime_guess::from_path(name).first_or_octet_stream().essence_str().to_string()
}

/// The `<title>` of an HTML document, whitespace-collapsed, entities decoded.
pub fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let close = lower[open_end..].find("</title")? + open_end;
    let t = wax_core::text::html_to_text(&format!("<p>{}</p>", &html[open_end..close]));
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty()).then_some(t)
}

/// The `lang` attribute of the root `<html>` element.
pub fn html_lang(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<html")?;
    let end = lower[start..].find('>')? + start;
    let tag = &html[start..end];
    let ltag = &lower[start..end];
    let at = ltag.find(" lang=")? + 6;
    let rest = &tag[at..];
    let v = match rest.chars().next()? {
        q @ ('"' | '\'') => rest[1..].split(q).next()?,
        _ => rest.split(|c: char| c.is_whitespace()).next()?,
    };
    let v = v.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// A well-formed BCP-47 tag (what the manifest's `languages` takes). A
/// source's malformed `lang` is dropped rather than failing the build.
pub fn valid_language(tag: &str) -> bool {
    language_tags::LanguageTag::parse(tag).is_ok()
}

/// Minimal HTML escaping for generated pages.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A generated page (a contents page, a PDF's text page) in the DeltOS
/// light tokens (design-language §4: `surface`, `ink`, `accent-ink`). No dark
/// tokens are defined yet, so dark mode uses the browser's own system colours
/// rather than invented values; the focus ring follows §5.
pub fn generated_page(title: &str, lang: Option<&str>, body: &str) -> String {
    let lang_attr = lang
        .and_then(|l| l.split(',').next())
        .map(|l| format!(" lang=\"{}\"", esc(l.trim())))
        .unwrap_or_default();
    format!(
        "<!doctype html>\n<html{lang_attr}>\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{t}</title>\n<style>\n\
         :root{{color-scheme:light dark}}\n\
         body{{background:#F2EEE6;color:#2B2926;font:1rem/1.6 system-ui,sans-serif;max-width:46rem;margin:0 auto;padding:1rem}}\n\
         a{{color:#A4460C}} a:focus-visible{{outline:2px solid #A4460C;outline-offset:2px}}\n\
         @media (prefers-color-scheme:dark){{body{{background:Canvas;color:CanvasText}}a{{color:LinkText}}a:focus-visible{{outline-color:LinkText}}}}\n\
         pre{{white-space:pre-wrap;font:inherit}}\n\
         </style>\n</head>\n<body>\n<h1>{t}</h1>\n{body}\n</body>\n</html>\n",
        t = esc(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_and_lang() {
        let h = "<!doctype html><HTML class=x lang=\"fr-CA\"><head><title>\n Caf&eacute;  au lait </title>";
        assert_eq!(html_title(h).as_deref(), Some("Café au lait"));
        assert_eq!(html_lang(h).as_deref(), Some("fr-CA"));
        assert_eq!(html_title("<p>none</p>"), None);
    }

    #[test]
    fn policy() {
        assert_eq!(codec_for("text/html"), Codec::Text);
        assert_eq!(codec_for("image/webp"), Codec::None);
        assert_eq!(codec_for("font/ttf"), Codec::Zstd);
        assert_eq!(codec_for("application/pdf"), Codec::None);
        assert!(is_carried_mime("application/pdf"));
        assert!(!is_carried_mime("video/mp4"));
        assert!(is_reserved("_assets/x") && !is_reserved("assets/x"));
    }
}
