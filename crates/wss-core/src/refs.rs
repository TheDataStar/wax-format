//! Find and rewrite the URL references in HTML and CSS.
//!
//! The same conservative scanner as `zim2wax::rewrite` (not a parser: it
//! walks tags with a small state machine that understands quoted attribute
//! values, comments and `<script>`/`<style>` bodies, and passes anything it
//! does not understand through unchanged), but generic: the caller decides
//! what each reference becomes. The website importer uses it twice, once to
//! collect links while crawling and once to point captured links at pack
//! paths.

/// Where a reference was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// `<a href>` / `<area href>`: a page to visit.
    Link,
    /// Anything a page loads to render: `src`, `<link href>`, `srcset`,
    /// `poster`, CSS `url()`.
    Resource,
}

const URL_ATTRS: [&str; 4] = ["href", "src", "poster", "data"];

/// Rewrite every URL-bearing attribute; `f` returns the replacement or
/// `None` to leave the value byte for byte.
pub fn rewrite_html(html: &str, f: &mut dyn FnMut(RefKind, &str) -> Option<String>) -> String {
    let mut out = String::with_capacity(html.len() + html.len() / 16);
    let b = html.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            let start = i;
            while i < b.len() && b[i] != b'<' {
                i += 1;
            }
            out.push_str(&html[start..i]);
            continue;
        }
        if html[i..].starts_with("<!--") {
            let end = html[i..].find("-->").map(|e| i + e + 3).unwrap_or(b.len());
            out.push_str(&html[i..end]);
            i = end;
            continue;
        }
        if html[i..].starts_with("<!") || html[i..].starts_with("<?") || html[i..].starts_with("</") {
            let end = html[i..].find('>').map(|e| i + e + 1).unwrap_or(b.len());
            out.push_str(&html[i..end]);
            i = end;
            continue;
        }
        let tag_start = i;
        i += 1;
        let name_start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' {
            i += 1;
        }
        let tag = html[name_start..i].to_ascii_lowercase();
        out.push_str(&html[tag_start..i]);
        loop {
            let ws = i;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            out.push_str(&html[ws..i]);
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
            let an = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'>' && b[i] != b'/' {
                i += 1;
            }
            let attr = html[an..i].to_ascii_lowercase();
            out.push_str(&html[an..i]);
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= b.len() || b[j] != b'=' {
                continue;
            }
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
            let kind = if attr == "href" && (tag == "a" || tag == "area") { RefKind::Link } else { RefKind::Resource };
            let replaced = if URL_ATTRS.contains(&attr.as_str()) {
                // `<link rel=canonical|alternate>` and `<base>` are not loaded.
                if tag == "base" {
                    None
                } else {
                    f(kind, value)
                }
            } else if attr == "srcset" {
                let r = rewrite_srcset(value, f);
                (r != value).then_some(r)
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
                    if val_end <= b.len() && val_end > 0 && b.get(val_end - 1) == Some(&(q as u8)) {
                        out.push(q);
                    }
                }
                (None, Some(new)) => out.push_str(&new),
                (None, None) => out.push_str(value),
            }
            i = val_end;
        }
        if tag == "script" || tag == "style" {
            let close = format!("</{tag}");
            let lower = html[i..].to_ascii_lowercase();
            let end = lower.find(&close).map(|e| i + e).unwrap_or(b.len());
            if tag == "style" {
                out.push_str(&rewrite_css(&html[i..end], f));
            } else {
                out.push_str(&html[i..end]);
            }
            i = end;
        }
    }
    out
}

fn rewrite_srcset(value: &str, f: &mut dyn FnMut(RefKind, &str) -> Option<String>) -> String {
    let mut out = Vec::new();
    for cand in value.split(',') {
        let trimmed = cand.trim_start();
        let lead = &cand[..cand.len() - trimmed.len()];
        let (url, rest) = match trimmed.find(char::is_whitespace) {
            Some(i) => (&trimmed[..i], &trimmed[i..]),
            None => (trimmed, ""),
        };
        match f(RefKind::Resource, url) {
            Some(new) => out.push(format!("{lead}{new}{rest}")),
            None => out.push(cand.to_string()),
        }
    }
    out.join(",")
}

/// Rewrite `url(…)` references in a stylesheet.
pub fn rewrite_css(css: &str, f: &mut dyn FnMut(RefKind, &str) -> Option<String>) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(pos) = rest.find("url(") {
        out.push_str(&rest[..pos + 4]);
        rest = &rest[pos + 4..];
        let Some(close) = rest.find(')') else { break };
        let inner = &rest[..close];
        let trimmed = inner.trim();
        let (quote, raw) = match trimmed.chars().next() {
            Some(q @ ('"' | '\'')) if trimmed.ends_with(q) && trimmed.len() >= 2 => (Some(q), &trimmed[1..trimmed.len() - 1]),
            _ => (None, trimmed),
        };
        match f(RefKind::Resource, raw) {
            Some(new) => match quote {
                Some(q) => {
                    out.push(q);
                    out.push_str(&new);
                    out.push(q);
                }
                None => out.push_str(&new),
            },
            None => out.push_str(inner),
        }
        rest = &rest[close..];
    }
    out.push_str(rest);
    out
}

/// Collect every reference without changing anything.
pub fn collect_html(html: &str) -> Vec<(RefKind, String)> {
    let mut v = Vec::new();
    rewrite_html(html, &mut |k, u| {
        v.push((k, u.to_string()));
        None
    });
    v
}

pub fn collect_css(css: &str) -> Vec<String> {
    let mut v = Vec::new();
    rewrite_css(css, &mut |_, u| {
        v.push(u.to_string());
        None
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_links_and_resources() {
        let h = r#"<a href="/a">x</a><img src=b.png srcset="c.png 1x, d.png 2x"><link rel=stylesheet href='s.css'>
<!-- <a href="no"> --><script>var u = "<a href='nope'>";</script><style>p{background:url("e.png")}</style>"#;
        let got = collect_html(h);
        assert_eq!(
            got,
            vec![
                (RefKind::Link, "/a".to_string()),
                (RefKind::Resource, "b.png".into()),
                (RefKind::Resource, "c.png".into()),
                (RefKind::Resource, "d.png".into()),
                (RefKind::Resource, "s.css".into()),
                (RefKind::Resource, "e.png".into()),
            ]
        );
    }

    #[test]
    fn rewrites_only_what_the_callback_returns() {
        let h = r#"<a href="/a">x</a> <a href="https://elsewhere/">y</a>"#;
        let out = rewrite_html(h, &mut |_, u| (u == "/a").then(|| "/a.html".to_string()));
        assert_eq!(out, r#"<a href="/a.html">x</a> <a href="https://elsewhere/">y</a>"#);
        assert_eq!(rewrite_html("<p>unchanged & <b>bold</b></p>", &mut |_, _| None), "<p>unchanged & <b>bold</b></p>");
    }
}
