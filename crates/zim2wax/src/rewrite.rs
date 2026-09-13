//! In-document reference rewriting — Track B §20: "zim2wax rewrites
//! in-document hrefs to match at conversion time rather than relying on the
//! serving layer to resolve them."
//!
//! Two rewriters, both deliberately conservative:
//!
//! * [`rewrite_html`] scans `href`, `src`, `poster`, `data` and `srcset`
//!   attributes on any tag.
//! * [`rewrite_css`] scans `url(…)` tokens.
//!
//! A reference is rewritten **only** when it resolves to a dirent that is
//! being emitted; everything else — external URLs, fragments, references to
//! skipped media, references that resolve to nothing — is left byte-for-byte.
//! That is what "leave references intact" for skipped mimetypes means in
//! practice: an `<audio src>` still points where it pointed, it just has
//! nothing behind it.
//!
//! The HTML scanner is not a parser. It walks tags with a small state machine
//! that understands quoted attribute values, comments, and `<script>`/`<style>`
//! raw-text bodies, which is what mwoffliner output needs. It never restructures
//! the document, so anything it does not understand passes through unchanged.

use crate::paths::{href_for, parse_href, resolve_in_zim};
use zim::Namespace;

/// Resolves a `(namespace, url)` inside the ZIM to its emitted canonical path,
/// or `None` if that dirent is not part of the pack.
pub trait Resolver {
    fn canonical_for(&self, ns: Namespace, url: &str) -> Option<String>;
}

impl<F> Resolver for F
where
    F: Fn(Namespace, &str) -> Option<String>,
{
    fn canonical_for(&self, ns: Namespace, url: &str) -> Option<String> {
        self(ns, url)
    }
}

/// Attributes whose value is a single URL.
const URL_ATTRS: [&str; 4] = ["href", "src", "poster", "data"];
/// Attributes whose value is a comma-separated candidate list (`url [descriptor]`).
const SRCSET_ATTRS: [&str; 1] = ["srcset"];

/// Statistics from one rewrite pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RewriteStats {
    /// References examined.
    pub seen: usize,
    /// References rewritten to a canonical path.
    pub rewritten: usize,
}

/// Rewrite one reference value. Returns the replacement if the reference
/// resolves to an emitted dirent.
fn rewrite_one(
    value: &str,
    base_ns: Namespace,
    base_url: &str,
    resolver: &dyn Resolver,
) -> Option<String> {
    let href = parse_href(value)?;
    let (ns, url) = resolve_in_zim(base_ns, base_url, &href.path)?;
    let canonical = resolver.canonical_for(ns, &url)?;
    Some(format!("{}{}{}", href_for(&canonical), href.query, href.fragment))
}

fn rewrite_srcset(
    value: &str,
    base_ns: Namespace,
    base_url: &str,
    resolver: &dyn Resolver,
    stats: &mut RewriteStats,
) -> String {
    // candidates are comma-separated; each is "url" or "url descriptor".
    // URLs never contain unencoded commas in mwoffliner output, but a comma
    // inside a url would be followed by a non-space, whereas a candidate
    // separator is followed by whitespace — good enough, and a wrong split
    // only means that candidate is left untouched.
    let mut out = Vec::new();
    for cand in value.split(',') {
        let trimmed = cand.trim_start();
        let lead = &cand[..cand.len() - trimmed.len()];
        let (url, rest) = match trimmed.find(char::is_whitespace) {
            Some(i) => (&trimmed[..i], &trimmed[i..]),
            None => (trimmed, ""),
        };
        stats.seen += 1;
        match rewrite_one(url, base_ns, base_url, resolver) {
            Some(new) => {
                stats.rewritten += 1;
                out.push(format!("{lead}{new}{rest}"));
            }
            None => out.push(cand.to_string()),
        }
    }
    out.join(",")
}

/// Rewrite the URL-bearing attributes of an HTML document. `base_ns`/`base_url`
/// identify the dirent the document came from (relative references resolve
/// against it).
pub fn rewrite_html(
    html: &str,
    base_ns: Namespace,
    base_url: &str,
    resolver: &dyn Resolver,
) -> (String, RewriteStats) {
    let mut stats = RewriteStats::default();
    let mut out = String::with_capacity(html.len() + html.len() / 16);
    let b = html.as_bytes();
    let mut i = 0;

    while i < b.len() {
        if b[i] != b'<' {
            // copy a run of text
            let start = i;
            while i < b.len() && b[i] != b'<' {
                i += 1;
            }
            out.push_str(&html[start..i]);
            continue;
        }
        // comment
        if html[i..].starts_with("<!--") {
            let end = html[i..].find("-->").map(|e| i + e + 3).unwrap_or(b.len());
            out.push_str(&html[i..end]);
            i = end;
            continue;
        }
        // doctype / processing instruction / closing tag: copy to '>'
        if html[i..].starts_with("<!") || html[i..].starts_with("<?") || html[i..].starts_with("</") {
            let end = html[i..].find('>').map(|e| i + e + 1).unwrap_or(b.len());
            out.push_str(&html[i..end]);
            i = end;
            continue;
        }
        // an opening tag: scan attributes until the closing '>' (honouring quotes)
        let tag_start = i;
        i += 1;
        let name_start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' {
            i += 1;
        }
        let tag_name = html[name_start..i].to_ascii_lowercase();
        out.push_str(&html[tag_start..i]);

        // attributes
        loop {
            // whitespace
            let ws_start = i;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            out.push_str(&html[ws_start..i]);
            if i >= b.len() {
                break;
            }
            if b[i] == b'>' {
                out.push('>');
                i += 1;
                break;
            }
            if b[i] == b'/' {
                out.push('/');
                i += 1;
                continue;
            }
            // attribute name
            let an_start = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'>' && b[i] != b'/' {
                i += 1;
            }
            let attr_name = html[an_start..i].to_ascii_lowercase();
            out.push_str(&html[an_start..i]);
            // optional "= value"
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'=' {
                out.push_str(&html[i..=j]);
                i = j + 1;
                while i < b.len() && b[i].is_ascii_whitespace() {
                    out.push(b[i] as char);
                    i += 1;
                }
                if i >= b.len() {
                    break;
                }
                let (value, val_end, quote) = if b[i] == b'"' || b[i] == b'\'' {
                    let q = b[i];
                    let vs = i + 1;
                    let ve = html[vs..].find(q as char).map(|e| vs + e).unwrap_or(b.len());
                    (&html[vs..ve], (ve + 1).min(b.len()), Some(q as char))
                } else {
                    let vs = i;
                    let mut ve = i;
                    while ve < b.len() && !b[ve].is_ascii_whitespace() && b[ve] != b'>' {
                        ve += 1;
                    }
                    (&html[vs..ve], ve, None)
                };

                let replaced: Option<String> = if URL_ATTRS.contains(&attr_name.as_str()) {
                    stats.seen += 1;
                    let r = rewrite_one(value, base_ns, base_url, resolver);
                    if r.is_some() {
                        stats.rewritten += 1;
                    }
                    r
                } else if SRCSET_ATTRS.contains(&attr_name.as_str()) {
                    let r = rewrite_srcset(value, base_ns, base_url, resolver, &mut stats);
                    if r != value {
                        Some(r)
                    } else {
                        None
                    }
                } else {
                    None
                };

                match (quote, replaced) {
                    (Some(q), Some(new)) => {
                        out.push(q);
                        out.push_str(&new);
                        out.push(q);
                    }
                    (Some(q), None) => {
                        out.push(q);
                        out.push_str(value);
                        // the closing quote may be missing at EOF
                        if val_end <= b.len() && val_end > 0 && b.get(val_end - 1) == Some(&(q as u8)) {
                            out.push(q);
                        }
                    }
                    (None, Some(new)) => out.push_str(&new),
                    (None, None) => out.push_str(value),
                }
                i = val_end;
            }
        }

        // raw-text elements: copy their body verbatim up to the closing tag
        if tag_name == "script" || tag_name == "style" {
            let close = format!("</{tag_name}");
            let lower_rest = html[i..].to_ascii_lowercase();
            let end = lower_rest.find(&close).map(|e| i + e).unwrap_or(b.len());
            out.push_str(&html[i..end]);
            i = end;
        }
    }
    (out, stats)
}

/// Rewrite `url(...)` references in a stylesheet.
pub fn rewrite_css(
    css: &str,
    base_ns: Namespace,
    base_url: &str,
    resolver: &dyn Resolver,
) -> (String, RewriteStats) {
    let mut stats = RewriteStats::default();
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(pos) = rest.find("url(") {
        out.push_str(&rest[..pos + 4]);
        rest = &rest[pos + 4..];
        let close = match rest.find(')') {
            Some(c) => c,
            None => break,
        };
        let inner = &rest[..close];
        let trimmed = inner.trim();
        let (quote, raw) = match trimmed.chars().next() {
            Some(q @ ('"' | '\'')) if trimmed.ends_with(q) && trimmed.len() >= 2 => {
                (Some(q), &trimmed[1..trimmed.len() - 1])
            }
            _ => (None, trimmed),
        };
        stats.seen += 1;
        match rewrite_one(raw, base_ns, base_url, resolver) {
            Some(new) => {
                stats.rewritten += 1;
                match quote {
                    Some(q) => {
                        out.push(q);
                        out.push_str(&new);
                        out.push(q);
                    }
                    None => out.push_str(&new),
                }
            }
            None => out.push_str(inner),
        }
        rest = &rest[close..];
    }
    out.push_str(rest);
    (out, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resolver(map: &[(Namespace, &str, &str)]) -> HashMap<(u8, String), String> {
        map.iter()
            .map(|(ns, url, canon)| ((ns.as_byte(), url.to_string()), canon.to_string()))
            .collect()
    }

    fn r(map: &HashMap<(u8, String), String>) -> impl Fn(Namespace, &str) -> Option<String> + '_ {
        move |ns, url| map.get(&(ns.as_byte(), url.to_string())).cloned()
    }

    #[test]
    fn rewrites_resolvable_hrefs_and_leaves_the_rest() {
        let c = Namespace::UserContent;
        let map = resolver(&[
            (c, "Michael_Jackson", "Michael_Jackson.html"),
            (c, "_assets_/x.png", "_assets/_assets_/x.png"),
            (c, "_res_/style.css", "_assets/_res_/style.css"),
        ]);
        let html = r##"<!doctype html><html><head><link rel="stylesheet" href="./_res_/style.css"></head>
<body><a href="Michael_Jackson#Early_life">MJ</a> <a href='Missing_Page'>x</a>
<a href="https://en.wikipedia.org/wiki/X">ext</a> <img src="./_assets_/x.png" alt="a>b">
<img srcset="./_assets_/x.png 1x, ./_assets_/missing.png 2x"> <a href="#top">top</a>
<script>var s = "<a href='Michael_Jackson'>";</script></body></html>"##;
        let (out, stats) = rewrite_html(html, c, "African_Americans", &r(&map));
        assert!(out.contains(r#"href="/_assets/_res_/style.css""#), "{out}");
        assert!(out.contains(r#"href="/Michael_Jackson.html#Early_life""#), "{out}");
        assert!(out.contains(r#"href='Missing_Page'"#), "unresolvable left alone: {out}");
        assert!(out.contains(r#"href="https://en.wikipedia.org/wiki/X""#), "{out}");
        assert!(out.contains(r#"src="/_assets/_assets_/x.png" alt="a>b""#), "{out}");
        assert!(out.contains(r#"srcset="/_assets/_assets_/x.png 1x, ./_assets_/missing.png 2x""#), "{out}");
        assert!(out.contains(r##"href="#top""##), "{out}");
        assert!(out.contains(r#"<script>var s = "<a href='Michael_Jackson'>";</script>"#), "script body untouched: {out}");
        assert_eq!(stats.rewritten, 4, "{stats:?}");
    }

    #[test]
    fn percent_encoded_hrefs_match_raw_dirent_urls() {
        let c = Namespace::UserContent;
        let map = resolver(&[(c, "Michael_Jackson's_Thriller", "Michael_Jackson's_Thriller.html")]);
        let html = r#"<a href="Michael_Jackson%27s_Thriller">t</a>"#;
        let (out, _) = rewrite_html(html, c, "x", &r(&map));
        assert!(out.contains(r#"href="/Michael_Jackson's_Thriller.html""#), "{out}");
    }

    #[test]
    fn legacy_cross_namespace_references() {
        let a = Namespace::Articles;
        let map = resolver(&[
            (Namespace::ImagesFile, "logo.png", "_assets/logo.png"),
            (Namespace::Layout, "s.css", "_assets/s.css"),
            (a, "Other", "Other.html"),
        ]);
        let html = r#"<img src="../I/logo.png"><link href="../-/s.css"><a href="Other">o</a>"#;
        let (out, stats) = rewrite_html(html, a, "Foo", &r(&map));
        assert_eq!(out, r#"<img src="/_assets/logo.png"><link href="/_assets/s.css"><a href="/Other.html">o</a>"#);
        assert_eq!(stats.rewritten, 3);
    }

    #[test]
    fn unquoted_attributes_and_self_closing_tags() {
        let c = Namespace::UserContent;
        let map = resolver(&[(c, "p.png", "_assets/p.png")]);
        // per HTML5 an unquoted value runs to whitespace or '>', so "p.png/" in
        // "<img src=p.png/>" is the value (with the slash) and does not resolve;
        // the quoted and space-terminated forms do.
        let (out, _) = rewrite_html("<img src=p.png alt=x><br/><img src=\"p.png\"/>", c, "x", &r(&map));
        assert_eq!(out, "<img src=/_assets/p.png alt=x><br/><img src=\"/_assets/p.png\"/>");
    }

    #[test]
    fn bytes_are_identical_when_nothing_resolves() {
        let c = Namespace::UserContent;
        let map = resolver(&[]);
        let html = "<html><!-- c --><body><a href=\"x\">x</a><img src='y.png' data-x=1></body></html>";
        let (out, stats) = rewrite_html(html, c, "z", &r(&map));
        assert_eq!(out, html);
        assert_eq!(stats.rewritten, 0);
        assert_eq!(stats.seen, 2);
    }

    #[test]
    fn css_url_references() {
        let c = Namespace::UserContent;
        let map = resolver(&[
            (c, "_res_/font.woff2", "_assets/_res_/font.woff2"),
            (c, "_res_/bg.png", "_assets/_res_/bg.png"),
        ]);
        let css = r#"@font-face{src:url("font.woff2") format("woff2")} .a{background:url(bg.png)} .b{background:url('missing.png')} .c{background:url(data:image/png;base64,AAAA)}"#;
        let (out, stats) = rewrite_css(css, c, "_res_/style.css", &r(&map));
        assert!(out.contains(r#"url("/_assets/_res_/font.woff2")"#), "{out}");
        assert!(out.contains("url(/_assets/_res_/bg.png)"), "{out}");
        assert!(out.contains("url('missing.png')"), "{out}");
        assert!(out.contains("url(data:image/png;base64,AAAA)"), "{out}");
        assert_eq!(stats.rewritten, 2);
    }
}
