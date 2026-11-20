//! PDF → pack.
//!
//! The original PDF is kept, byte for byte, so nothing about it is lost. Next
//! to it WSS generates a **text page**: the PDF's text, page by page, as plain
//! HTML with a heading per page. That page is what makes a PDF useful on a
//! box: search finds it (SPEC §5.7), a search hit lands on the right page
//! (each page heading is a passage, SPEC v0.13 §5.10), and phones whose
//! browsers have no PDF viewer can still read it.
//!
//! A PDF with no text layer (a scan) is carried, and the report says search
//! cannot find it; WSS does not run OCR. Text extraction runs on
//! `pdf-extract`, which can panic on malformed files, so it is isolated and a
//! failure becomes a note, never a failed build.

use super::{esc, generated_page, Staged, GENERATED_DIR};
use crate::recipe::Recipe;
use crate::report::NoteKind;
use crate::stage::{Codec, Search};
use anyhow::{bail, Context, Result};
use std::path::Path;

/// What a PDF states and contains.
#[derive(Debug, Clone, Default)]
pub struct PdfText {
    pub title: Option<String>,
    pub author: Option<String>,
    /// Text per page; empty strings for pages without text.
    pub pages: Vec<String>,
}

impl PdfText {
    pub fn has_text(&self) -> bool {
        self.pages.iter().any(|p| !p.trim().is_empty())
    }
}

/// Read a PDF's metadata and text. `Err` carries a plain reason.
pub fn read(bytes: &[u8]) -> std::result::Result<PdfText, String> {
    let doc = lopdf::Document::load_mem(bytes).map_err(|e| format!("not a readable PDF: {e}"))?;
    let (title, author) = info_strings(&doc);
    let owned = bytes.to_vec();
    let pages = std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem_by_pages(&owned))
        .map_err(|_| "the text extractor failed on this file".to_string())?
        .map_err(|e| format!("text could not be extracted: {e}"))?;
    Ok(PdfText { title, author, pages })
}

fn info_strings(doc: &lopdf::Document) -> (Option<String>, Option<String>) {
    let Ok(info) = doc.trailer.get_deref(b"Info", doc).and_then(|o| o.as_dict()) else {
        return (None, None);
    };
    let get = |k: &[u8]| {
        info.get_deref(k, doc).ok().and_then(|o| match o {
            lopdf::Object::String(b, _) => Some(decode_pdf_string(b)),
            _ => None,
        })
    };
    let clean = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    (clean(get(b"Title")), clean(get(b"Author")))
}

/// PDF text strings: UTF-16BE with a BOM, else PDFDocEncoding, which agrees
/// with Latin-1 for every printable character that matters here.
fn decode_pdf_string(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let units: Vec<u16> = b[2..].chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
        String::from_utf16_lossy(&units)
    } else if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&b[3..]).into_owned()
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

/// The generated text page for one PDF. `original` is the root-relative href
/// of the kept PDF.
pub fn text_page(title: &str, original: &str, text: &PdfText, lang: Option<&str>) -> String {
    let mut body = format!(
        "<p><a href=\"{}\">Open the original PDF</a> · {} pages</p>\n",
        esc(original),
        text.pages.len()
    );
    for (i, page) in text.pages.iter().enumerate() {
        let n = i + 1;
        body.push_str(&format!("<h2 id=\"page-{n}\">Page {n}</h2>\n"));
        let paras: Vec<&str> = page.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).collect();
        if paras.is_empty() {
            body.push_str("<p><em>No text on this page.</em></p>\n");
        }
        for p in paras {
            body.push_str(&format!("<p>{}</p>\n", esc(&p.split_whitespace().collect::<Vec<_>>().join(" "))));
        }
    }
    generated_page(title, lang, &body)
}

/// Pack path of the text page for the PDF at `pdf_path`.
pub fn text_page_path(pdf_path: &str) -> String {
    format!("{GENERATED_DIR}pdf/{pdf_path}.html")
}

/// Stage one PDF and its text page. Returns what the PDF states and the text
/// page's path, or `None` when the PDF itself was not staged.
pub fn stage_pdf(
    staged: &mut Staged,
    item: &str,
    path: &str,
    bytes: &[u8],
    lang: Option<&str>,
) -> Result<Option<(PdfText, String)>> {
    let text = match read(bytes) {
        Ok(t) => t,
        Err(why) => {
            // Still carried as a download; it just cannot be searched.
            if staged.stage_file(item, path, "application/pdf", None, bytes)? {
                staged.notes.add(NoteKind::NoSearchableText, item, why);
            }
            return Ok(None);
        }
    };
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let title = text.title.clone().unwrap_or(stem);
    if !staged.stage_file(item, path, "application/pdf", Some(&title), bytes)? {
        return Ok(None);
    }
    if !text.has_text() {
        staged.notes.add(
            NoteKind::NoSearchableText,
            item,
            "no text layer (a scanned PDF?); WSS does not run OCR, so search will not find it",
        );
    }
    let page_path = text_page_path(path);
    let html = text_page(&title, &zim2wax::paths::href_for(path), &text, lang);
    staged
        .staging
        .add(&page_path, Some("text/html"), Some(&title), html.as_bytes(), Codec::Text, Search::Html)?;
    Ok(Some((text, page_path)))
}

/// A single PDF file → pack. The text page is the entry point.
pub fn import(file: &Path, recipe: &Recipe) -> Result<Staged> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    if !bytes.starts_with(b"%PDF-") {
        bail!("{} is not a PDF (no %PDF- signature)", file.display());
    }
    let mut staged = Staged::new("pdf")?;
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document.pdf".into());
    let path = super::html::sanitize_component(&name);
    let lang = recipe.pack.languages.as_deref();
    match stage_pdf(&mut staged, &name, &path, &bytes, lang)? {
        Some((text, page)) => {
            staged.facts.name = text.title;
            staged.facts.attribution = text.author;
            staged.facts.entry_point = Some(page);
        }
        None => {
            if !staged.staging.contains(&path)? {
                bail!("{} could not be carried; see the notes", file.display());
            }
            // Unreadable but carried: open the file itself.
            staged.facts.entry_point = Some(path.clone());
        }
    }
    if staged.facts.name.is_none() {
        staged.facts.name = Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned());
    }
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_info_strings() {
        assert_eq!(decode_pdf_string(&[0xFE, 0xFF, 0, b'H', 0, b'i']), "Hi");
        assert_eq!(decode_pdf_string(b"Plain"), "Plain");
    }

    #[test]
    fn text_page_has_a_heading_per_page() {
        let t = PdfText { title: None, author: None, pages: vec!["Alpha beta\n\ngamma".into(), "".into()] };
        let h = text_page("Doc", "/doc.pdf", &t, Some("en"));
        assert!(h.contains("<h2 id=\"page-1\">Page 1</h2>"));
        assert!(h.contains("<p>gamma</p>"));
        assert!(h.contains("No text on this page"));
        // 0 (the page head), then the <h1> and one <h2> per page.
        assert_eq!(wax_core::passage::html_heading_starts(h.as_bytes()).len(), 4);
    }

    #[test]
    fn garbage_is_a_reason_not_a_panic() {
        assert!(read(b"%PDF-1.4 nonsense").is_err());
    }
}
