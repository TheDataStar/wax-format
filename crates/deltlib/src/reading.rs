//! Reading a page without its layout (deltd API §6.4): its sections, for
//! the contents list and for anchoring highlights (`outline`), and its
//! readable text and images, for reader mode and large text (`reader`).
//!
//! Sections are the page's passages (SPEC v0.13 §5.10): cut at build time
//! along its headings, so a section's id, the page path plus its ordinal,
//! is the same id a highlight anchors to and stays valid across rebuilds. A
//! page from a pack built before v0.13 has no passages; it is then one
//! section, the whole page.
//!
//! Nothing here interprets HTML beyond what the passage rule and
//! `wax_core::text` already do: block boundaries become paragraph breaks,
//! `<img>` becomes an image reference, everything else is visible text.

use serde::Serialize;
use wax_core::passage::Passage;

/// Largest page reader mode or a highlight check decodes. Not a contract
/// value: it keeps one request's memory bounded on a 2 GB box, and a page
/// this size is no longer a page anyone reads in reader mode.
pub const MAX_READABLE_BYTES: u64 = 8 * 1024 * 1024;

/// Whether an entry is a page (and not media, a style sheet or a script).
pub fn is_page(mime: Option<&str>) -> bool {
    let m = mime.unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    m == "text/html" || m == "application/xhtml+xml"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Section {
    /// The passage ordinal (with the page path, its id), or `None` for a page
    /// with no passages.
    pub ordinal: Option<u32>,
    pub heading: Option<String>,
    /// 1–6 when the section starts with a heading.
    pub level: Option<u8>,
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Image {
    /// The image's path inside the pack, resolved against the page's own
    /// path, for the WebOS to load from the person's pack hostname.
    pub path: String,
    pub alt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadableSection {
    #[serde(flatten)]
    pub section: Section,
    /// Paragraphs of visible text, in order. The heading is not repeated.
    pub paragraphs: Vec<String>,
    pub images: Vec<Image>,
}

fn level_at(content: &[u8], p: &Passage) -> Option<u8> {
    p.heading.as_ref()?;
    let d = *content.get(p.start as usize + 2)?;
    (b'1'..=b'6').contains(&d).then(|| d - b'0')
}

/// The page's sections.
pub fn outline(content: &[u8], passages: Option<&[Passage]>) -> Vec<Section> {
    match passages {
        None => vec![Section { ordinal: None, heading: None, level: None, anchor: None }],
        Some(ps) => ps
            .iter()
            .map(|p| Section { ordinal: Some(p.ordinal), heading: p.heading.clone(), level: level_at(content, p), anchor: p.anchor.clone() })
            .collect(),
    }
}

/// The page's readable text and images, section by section. `page_path` is
/// the page's own path in the pack, for resolving image references.
pub fn readable(content: &[u8], passages: Option<&[Passage]>, page_path: &str) -> Vec<ReadableSection> {
    let sections = outline(content, passages);
    let spans: Vec<(usize, usize)> = match passages {
        None => vec![(0, content.len())],
        Some(ps) => ps.iter().map(|p| (p.start as usize, (p.end as usize).min(content.len()))).collect(),
    };
    sections
        .into_iter()
        .zip(spans)
        .map(|(section, (s, e))| {
            let span = &content[s.min(e)..e];
            let body = skip_heading(span, section.heading.is_some());
            ReadableSection { paragraphs: paragraphs(body), images: images(body, page_path), section }
        })
        .collect()
}

/// The span after its opening heading element, when it starts with one.
fn skip_heading(span: &[u8], has_heading: bool) -> &[u8] {
    if !has_heading || span.len() < 4 {
        return span;
    }
    let close = [b'<', b'/', span[1].to_ascii_lowercase(), span[2]];
    let lower: Vec<u8> = span.iter().map(u8::to_ascii_lowercase).collect();
    match lower.windows(4).position(|w| w == close) {
        Some(at) => {
            let end = lower[at..].iter().position(|&c| c == b'>').map(|k| at + k + 1).unwrap_or(span.len());
            &span[end..]
        }
        None => span,
    }
}

/// Elements whose boundaries break a paragraph.
const BLOCKS: &[&str] = &[
    "address", "article", "aside", "blockquote", "br", "caption", "dd", "div", "dl", "dt", "figcaption", "figure",
    "footer", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "li", "main", "nav", "ol", "p", "pre", "section",
    "table", "td", "th", "tr", "ul",
];

/// A character no page text holds (private use), marking a block boundary.
const BREAK: char = '\u{E000}';

fn paragraphs(html: &[u8]) -> Vec<String> {
    let src = String::from_utf8_lossy(html).replace(BREAK, " ");
    let b = src.as_bytes();
    let mut marked = String::with_capacity(src.len() + 64);
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            let j = if b.get(i + 1) == Some(&b'/') { i + 2 } else { i + 1 };
            let name: String = src[j..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
            if BLOCKS.contains(&name.as_str()) {
                marked.push_str(&src[last..i]);
                marked.push(BREAK);
                last = i;
            }
        }
        i += 1;
    }
    marked.push_str(&src[last..]);
    // html_to_text collapses whitespace but keeps BREAK, which is not
    // whitespace; script, style and comments vanish with whatever they hold.
    wax_core::text::html_to_text(&marked)
        .split(BREAK)
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect()
}

fn images(html: &[u8], page_path: &str) -> Vec<Image> {
    let src = String::from_utf8_lossy(html);
    let lower = src.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<img").map(|k| k + from) {
        from = at + 4;
        if !lower.as_bytes().get(at + 4).is_some_and(|c| c.is_ascii_whitespace() || *c == b'/' || *c == b'>') {
            continue;
        }
        let end = lower[at..].find('>').map(|k| at + k).unwrap_or(lower.len());
        let tag = &src[at..end];
        let Some(s) = attr(tag, "src") else { continue };
        if let Some(path) = resolve(page_path, &s) {
            out.push(Image { path, alt: attr(tag, "alt").unwrap_or_default() });
        }
    }
    out
}

/// One attribute's value from one start tag.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut from = 0;
    while let Some(p) = lower[from..].find(name).map(|k| k + from) {
        from = p + name.len();
        if p == 0 || !b[p - 1].is_ascii_whitespace() {
            continue;
        }
        let mut j = p + name.len();
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if b.get(j) != Some(&b'=') {
            continue;
        }
        j += 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let (start, stop) = match b.get(j) {
            Some(&q @ (b'"' | b'\'')) => (j + 1, lower[j + 1..].find(q as char).map(|k| j + 1 + k).unwrap_or(b.len())),
            Some(_) => (j, lower[j..].find(|c: char| c.is_ascii_whitespace() || c == '>').map(|k| j + k).unwrap_or(b.len())),
            None => return None,
        };
        return Some(decode_entities(&tag[start..stop]));
    }
    None
}

fn decode_entities(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// An image reference resolved to a path inside the pack, or `None` for one
/// that points outside it (another site, a `data:` URL): the box never
/// fetches anything from outside (contract §16), and reader mode does not
/// carry inline data.
pub fn resolve(page_path: &str, reference: &str) -> Option<String> {
    let r = reference.trim();
    let r = r.split(['#', '?']).next().unwrap_or("");
    if r.is_empty() || r.starts_with("//") {
        return None;
    }
    if let Some(colon) = r.find(':') {
        if !r[..colon].contains('/') {
            return None; // a scheme
        }
    }
    let mut parts: Vec<&str> = if r.starts_with('/') {
        vec![]
    } else {
        let mut p: Vec<&str> = page_path.split('/').collect();
        p.pop();
        p
    };
    for seg in r.trim_start_matches('/').split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    let path = parts.join("/");
    let decoded = percent_encoding::percent_decode_str(&path).decode_utf8().ok()?.into_owned();
    (!decoded.is_empty()).then_some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &[u8] = b"<p>Lead <b>para</b>.</p><p>Second.<br>Third</p><h2 id=\"h\">History</h2>\
<p>Long ago.</p><img src=\"../media/a%20b.png\" alt=\"A &amp; B\"><script>var x = 1;</script>\
<h3>Sub</h3><ul><li>One</li><li>Two</li></ul><img src=\"https://example.com/x.png\">";

    fn ps() -> Vec<Passage> {
        wax_core::passage::describe(PAGE, &wax_core::passage::html_heading_starts(PAGE))
    }

    #[test]
    fn outline_lists_sections_with_levels() {
        let o = outline(PAGE, Some(&ps()));
        assert_eq!(o.len(), 3);
        assert_eq!((o[0].ordinal, o[0].heading.as_deref(), o[0].level), (Some(0), None, None));
        assert_eq!((o[1].heading.as_deref(), o[1].level, o[1].anchor.as_deref()), (Some("History"), Some(2), Some("h")));
        assert_eq!((o[2].heading.as_deref(), o[2].level), (Some("Sub"), Some(3)));
        assert_eq!(outline(PAGE, None).len(), 1, "no passages: the whole page is one section");
    }

    #[test]
    fn reader_keeps_paragraphs_and_in_pack_images() {
        let r = readable(PAGE, Some(&ps()), "wiki/A/Page.html");
        assert_eq!(r[0].paragraphs, ["Lead para.", "Second.", "Third"]);
        assert_eq!(r[1].paragraphs, ["Long ago."], "the heading is not repeated; scripts are not text");
        assert_eq!(r[1].images, vec![Image { path: "wiki/media/a b.png".into(), alt: "A & B".into() }]);
        assert_eq!(r[2].paragraphs, ["One", "Two"]);
        assert!(r[2].images.is_empty(), "images from other sites are left out");
    }

    #[test]
    fn references_resolve_inside_the_pack_only() {
        assert_eq!(resolve("a/b.html", "c.png").as_deref(), Some("a/c.png"));
        assert_eq!(resolve("a/b.html", "/c.png").as_deref(), Some("c.png"));
        assert_eq!(resolve("a/b.html", "./x/../c.png?v=1#f").as_deref(), Some("a/c.png"));
        assert_eq!(resolve("b.html", "../c.png"), None, "cannot climb out of the pack");
        assert_eq!(resolve("b.html", "data:image/png;base64,AAAA"), None);
        assert_eq!(resolve("b.html", "//cdn.example/x.png"), None);
        assert!(is_page(Some("text/html; charset=utf-8")) && !is_page(Some("image/png")) && !is_page(None));
    }
}
