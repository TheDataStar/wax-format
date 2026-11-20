//! EPUB → pack.
//!
//! The book's files keep their paths inside the pack, so the book's own
//! links, stylesheets and images work unchanged. WSS adds:
//!
//! * a **contents page** (`_assets/wss/contents.html`), from the book's own
//!   table of contents (EPUB 3 nav, else EPUB 2 NCX, else reading order), as
//!   the pack's start page;
//! * a small **Contents · Previous · Next** bar at the end of each chapter, in
//!   reading order, because a browser has no reading system to move between
//!   chapters;
//! * search over every chapter, cut into passages at its headings.
//!
//! What the book states is used: `dc:title` (name), `dc:creator` or
//! `dc:publisher` (attribution), `dc:rights` (license, reviewed when it is not
//! an SPDX id), `dc:language`, and a PNG cover as the icon.
//!
//! A DRM-protected book is refused: WSS cannot and does not remove DRM. Fonts
//! that a book merely obfuscates (the IDPF and Adobe font schemes) are kept
//! as they are and listed in the report; browsers fall back to another font.

use super::{esc, generated_page, guess_mime, valid_language, Staged};
use crate::recipe::Recipe;
use crate::report::NoteKind;
use crate::stage::{Codec, Search};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

/// The pack's start page for a book.
pub const CONTENTS_PATH: &str = "_assets/wss/contents.html";

/// Font-obfuscation algorithms: not DRM, the content is readable.
const FONT_OBFUSCATION: [&str; 2] = ["http://www.idpf.org/2008/embedding", "http://ns.adobe.com/pdf/enc#RC"];

/// One file the OPF manifest lists.
#[derive(Debug, Clone)]
struct ManifestItem {
    path: String,
    media_type: String,
    properties: String,
}

#[derive(Debug, Default)]
struct Book {
    title: Option<String>,
    creators: Vec<String>,
    publisher: Option<String>,
    languages: Vec<String>,
    rights: Option<String>,
    /// Spine (reading order) as pack paths.
    spine: Vec<String>,
    /// (label, path with optional #fragment) from the book's TOC.
    toc: Vec<(String, String)>,
    cover: Option<String>,
    media: BTreeMap<String, String>,
}

pub fn import(file: &Path, recipe: &Recipe) -> Result<Staged> {
    let f = std::fs::File::open(file).with_context(|| format!("opening {}", file.display()))?;
    let mut zip = zip::ZipArchive::new(f).with_context(|| format!("{} is not an EPUB (not a zip)", file.display()))?;
    let mut staged = Staged::new("epub")?;

    let container = read_text(&mut zip, "META-INF/container.xml")
        .map_err(|_| anyhow!("{} is not an EPUB: META-INF/container.xml is missing", file.display()))?;
    let opf_path = rootfile(&container)?;
    let obfuscated = check_encryption(&mut zip)?;
    let opf = read_text(&mut zip, &opf_path).with_context(|| format!("reading the package document {opf_path}"))?;
    let mut book = parse_opf(&opf, &opf_path, &mut zip)?;
    if book.toc.is_empty() {
        book.toc = book.spine.iter().map(|p| (String::new(), p.clone())).collect();
    }

    // Chapter titles from the TOC, by path (first label wins).
    let mut chapter_titles: BTreeMap<String, String> = BTreeMap::new();
    for (label, target) in &book.toc {
        let path = target.split('#').next().unwrap_or("").to_string();
        if !label.is_empty() {
            chapter_titles.entry(path).or_insert_with(|| label.clone());
        }
    }
    let spine_pos: BTreeMap<&str, usize> = book.spine.iter().enumerate().map(|(i, p)| (p.as_str(), i)).collect();

    let names: Vec<String> = (0..zip.len()).filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string())).collect();
    for name in names {
        if name.ends_with('/') || name == "mimetype" || name.starts_with("META-INF/") {
            continue; // container plumbing, not content
        }
        let mut bytes = Vec::new();
        zip.by_name(&name)?.read_to_end(&mut bytes)?;
        let mime = book.media.get(&name).cloned().unwrap_or_else(|| guess_mime(&name));
        if obfuscated.contains(&name) {
            staged.note(
                NoteKind::ObfuscatedFont,
                &name,
                "this font is obfuscated by the book; it is kept as it is and browsers will use another font",
            );
        }
        match spine_pos.get(name.as_str()) {
            Some(&i) => {
                let html = String::from_utf8_lossy(&bytes).into_owned();
                let title = chapter_titles.get(&name).cloned().or_else(|| super::html_title(&html));
                let with_nav = add_nav(&html, book.spine.get(i.wrapping_sub(1)), book.spine.get(i + 1));
                staged.stage_file(&name, &name, &mime, title.as_deref(), with_nav.as_bytes())?;
            }
            None => {
                // Only chapters in the reading order are searched; the nav
                // document and other markup outside the spine are navigation.
                staged.stage_file_opts(&name, &name, &mime, None, &bytes, false)?;
            }
        }
    }

    let title = book.title.clone().unwrap_or_else(|| {
        file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Book".into())
    });
    let langs: Vec<String> = book.languages.iter().filter(|l| valid_language(l)).cloned().collect();
    let languages = (!langs.is_empty()).then(|| langs.join(","));
    let contents = contents_page(&title, &book, &staged, languages.as_deref().or(recipe.pack.languages.as_deref()))?;
    staged
        .staging
        .add(CONTENTS_PATH, Some("text/html"), Some(&title), contents.as_bytes(), Codec::Text, Search::No)?;

    staged.facts.name = book.title.clone();
    staged.facts.attribution = if book.creators.is_empty() { book.publisher.clone() } else { Some(book.creators.join(", ")) };
    staged.facts.license = book.rights.clone();
    staged.facts.languages = languages;
    staged.facts.entry_point = Some(CONTENTS_PATH.to_string());
    if let Some(c) = &book.cover {
        if staged.staging.get(c)?.is_some_and(|i| i.mime.as_deref() == Some("image/png")) {
            staged.facts.icon = Some(c.clone());
        }
    }
    Ok(staged)
}

fn read_text<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<String> {
    let mut s = String::new();
    zip.by_name(name)?.read_to_string(&mut s)?;
    Ok(s)
}

fn xml(text: &str) -> Result<roxmltree::Document<'_>> {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    roxmltree::Document::parse_with_options(text, opts).map_err(|e| anyhow!("malformed XML: {e}"))
}

fn rootfile(container: &str) -> Result<String> {
    let doc = xml(container)?;
    doc.descendants()
        .find(|n| n.has_tag_name("rootfile") && n.attribute("media-type").is_none_or(|m| m == "application/oebps-package+xml"))
        .and_then(|n| n.attribute("full-path"))
        .map(str::to_string)
        .ok_or_else(|| anyhow!("META-INF/container.xml names no package document"))
}

/// Refuse DRM; return the paths of merely obfuscated fonts.
fn check_encryption<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> Result<BTreeSet<String>> {
    let Ok(text) = read_text(zip, "META-INF/encryption.xml") else {
        return Ok(BTreeSet::new());
    };
    let doc = xml(&text)?;
    let mut fonts = BTreeSet::new();
    for data in doc.descendants().filter(|n| n.has_tag_name("EncryptedData")) {
        let alg = data
            .descendants()
            .find(|n| n.has_tag_name("EncryptionMethod"))
            .and_then(|n| n.attribute("Algorithm"))
            .unwrap_or("");
        let uri = data
            .descendants()
            .find(|n| n.has_tag_name("CipherReference"))
            .and_then(|n| n.attribute("URI"))
            .unwrap_or("");
        if !FONT_OBFUSCATION.contains(&alg) {
            bail!(
                "this EPUB is DRM-protected ({uri} is encrypted with {alg}). WSS cannot convert \
                 DRM-protected books; use a DRM-free copy."
            );
        }
        fonts.insert(resolve("", uri));
    }
    Ok(fonts)
}

fn parse_opf<R: Read + std::io::Seek>(opf: &str, opf_path: &str, zip: &mut zip::ZipArchive<R>) -> Result<Book> {
    let base = opf_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let doc = xml(opf).context("the package document")?;
    let mut book = Book::default();
    let text = |n: roxmltree::Node| n.text().map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty());

    let mut cover_id = None;
    if let Some(meta) = doc.descendants().find(|n| n.has_tag_name("metadata")) {
        for n in meta.children().filter(|n| n.is_element()) {
            match n.tag_name().name() {
                "title" if book.title.is_none() => book.title = text(n),
                "creator" => book.creators.extend(text(n)),
                "publisher" if book.publisher.is_none() => book.publisher = text(n),
                "language" => book.languages.extend(text(n)),
                "rights" if book.rights.is_none() => book.rights = text(n),
                "meta" if n.attribute("name") == Some("cover") => cover_id = n.attribute("content").map(str::to_string),
                _ => {}
            }
        }
    }

    let mut items: BTreeMap<String, ManifestItem> = BTreeMap::new();
    for n in doc.descendants().filter(|n| n.has_tag_name("item")) {
        if let (Some(id), Some(href)) = (n.attribute("id"), n.attribute("href")) {
            let item = ManifestItem {
                path: resolve(base, href),
                media_type: n.attribute("media-type").unwrap_or("").to_string(),
                properties: n.attribute("properties").unwrap_or("").to_string(),
            };
            book.media.insert(item.path.clone(), item.media_type.clone());
            items.insert(id.to_string(), item);
        }
    }
    book.cover = items
        .values()
        .find(|i| i.properties.split_whitespace().any(|p| p == "cover-image"))
        .or_else(|| cover_id.as_deref().and_then(|id| items.get(id)))
        .map(|i| i.path.clone());

    let spine = doc.descendants().find(|n| n.has_tag_name("spine"));
    if let Some(spine) = spine {
        for r in spine.children().filter(|n| n.has_tag_name("itemref")) {
            if let Some(item) = r.attribute("idref").and_then(|id| items.get(id)) {
                book.spine.push(item.path.clone());
            }
        }
    }

    // EPUB 3 nav document, else the EPUB 2 NCX.
    if let Some(nav) = items.values().find(|i| i.properties.split_whitespace().any(|p| p == "nav")) {
        if let Ok(t) = read_text(zip, &nav.path) {
            book.toc = parse_nav(&t, &nav.path);
        }
    }
    if book.toc.is_empty() {
        let ncx = spine
            .and_then(|s| s.attribute("toc"))
            .and_then(|id| items.get(id))
            .or_else(|| items.values().find(|i| i.media_type == "application/x-dtbncx+xml"));
        if let Some(ncx) = ncx {
            if let Ok(t) = read_text(zip, &ncx.path) {
                book.toc = parse_ncx(&t, &ncx.path);
            }
        }
    }
    Ok(book)
}

fn parse_nav(text: &str, nav_path: &str) -> Vec<(String, String)> {
    let base = nav_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let Ok(doc) = xml(text) else { return vec![] };
    let toc = doc
        .descendants()
        .find(|n| n.has_tag_name("nav") && n.attributes().any(|a| a.name() == "type" && a.value().split_whitespace().any(|v| v == "toc")))
        .or_else(|| doc.descendants().find(|n| n.has_tag_name("nav")));
    let Some(toc) = toc else { return vec![] };
    toc.descendants()
        .filter(|n| n.has_tag_name("a"))
        .filter_map(|a| {
            let href = a.attribute("href")?;
            let label: String = a.descendants().filter(|n| n.is_text()).filter_map(|n| n.text()).collect::<String>();
            Some((label.split_whitespace().collect::<Vec<_>>().join(" "), resolve_keep_fragment(base, href)))
        })
        .collect()
}

fn parse_ncx(text: &str, ncx_path: &str) -> Vec<(String, String)> {
    let base = ncx_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let Ok(doc) = xml(text) else { return vec![] };
    doc.descendants()
        .filter(|n| n.has_tag_name("navPoint"))
        .filter_map(|p| {
            let label = p
                .children()
                .find(|n| n.has_tag_name("navLabel"))?
                .descendants()
                .find(|n| n.has_tag_name("text"))?
                .text()?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let src = p.children().find(|n| n.has_tag_name("content"))?.attribute("src")?;
            Some((label, resolve_keep_fragment(base, src)))
        })
        .collect()
}

/// Resolve an EPUB-relative href (percent-encoded) to a zip/pack path,
/// dropping any fragment or query.
pub fn resolve(base: &str, href: &str) -> String {
    let href = href.split(['#', '?']).next().unwrap_or("");
    let decoded = zim2wax::paths::percent_decode(href);
    let mut parts: Vec<&str> = if decoded.starts_with('/') { vec![] } else { base.split('/').filter(|s| !s.is_empty()).collect() };
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn resolve_keep_fragment(base: &str, href: &str) -> String {
    let path = resolve(base, href);
    match href.split_once('#') {
        Some((_, frag)) if !frag.is_empty() => format!("{path}#{frag}"),
        _ => path,
    }
}

fn link(path: &str) -> String {
    match path.split_once('#') {
        Some((p, f)) => format!("{}#{}", zim2wax::paths::href_for(p), f),
        None => zim2wax::paths::href_for(path),
    }
}

/// Insert the chapter bar before `</body>`; a chapter without one is left
/// alone.
fn add_nav(html: &str, prev: Option<&String>, next: Option<&String>) -> String {
    let Some(at) = html.to_ascii_lowercase().rfind("</body") else {
        return html.to_string();
    };
    let mut links = vec![format!("<a href=\"{}\">Contents</a>", esc(&link(CONTENTS_PATH)))];
    if let Some(p) = prev {
        links.push(format!("<a href=\"{}\" rel=\"prev\">Previous</a>", esc(&link(p))));
    }
    if let Some(n) = next {
        links.push(format!("<a href=\"{}\" rel=\"next\">Next</a>", esc(&link(n))));
    }
    let bar = format!(
        "<nav class=\"wss-chapter-nav\" aria-label=\"Chapters\" style=\"margin:2em 0;padding-top:1em;border-top:1px solid #DDD6C8;display:flex;gap:1.5em\">{}</nav>\n",
        links.join(" ")
    );
    format!("{}{}{}", &html[..at], bar, &html[at..])
}

fn contents_page(title: &str, book: &Book, staged: &Staged, lang: Option<&str>) -> Result<String> {
    let mut body = String::new();
    let by = if book.creators.is_empty() { book.publisher.clone() } else { Some(book.creators.join(", ")) };
    if let Some(by) = by {
        body.push_str(&format!("<p>{}</p>\n", esc(&by)));
    }
    body.push_str("<nav aria-label=\"Contents\"><ol>\n");
    for (label, target) in &book.toc {
        let path = target.split('#').next().unwrap_or("");
        if !staged.staging.contains(path)? {
            continue;
        }
        let label = if label.is_empty() {
            staged.staging.get(path)?.and_then(|i| i.title).unwrap_or_else(|| path.to_string())
        } else {
            label.clone()
        };
        body.push_str(&format!("<li><a href=\"{}\">{}</a></li>\n", esc(&link(target)), esc(&label)));
    }
    body.push_str("</ol></nav>\n");
    Ok(generated_page(title, lang, &body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_and_encoded_hrefs() {
        assert_eq!(resolve("OEBPS", "text/ch%201.xhtml#x"), "OEBPS/text/ch 1.xhtml");
        assert_eq!(resolve("OEBPS/text", "../img/a.png"), "OEBPS/img/a.png");
        assert_eq!(resolve("", "ch1.xhtml"), "ch1.xhtml");
        assert_eq!(resolve_keep_fragment("OEBPS", "ch1.xhtml#s2"), "OEBPS/ch1.xhtml#s2");
    }

    #[test]
    fn nav_bar_goes_before_body_end() {
        let out = add_nav("<html><body><p>x</p></BODY></html>", None, Some(&"OEBPS/b.xhtml".to_string()));
        assert!(out.contains("<p>x</p><nav"));
        assert!(out.contains("href=\"/OEBPS/b.xhtml\" rel=\"next\""));
        assert!(!out.contains("rel=\"prev\""));
        assert_eq!(add_nav("<p>no body</p>", None, None), "<p>no body</p>");
    }
}
