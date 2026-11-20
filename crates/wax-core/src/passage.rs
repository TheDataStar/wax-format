//! Passages: build-time chunks of an entry, cut along its headings (SPEC
//! v0.13 §5.10, contract §13 decision 5).
//!
//! A passage is identified by **the entry's path plus its position** (the
//! ordinal, from 0). The cut points are computed once, when the pack is
//! built, by the rule a segment declares in `segment_meta.passage_rule`, and
//! stored in the segment's `passages` table, so a citation made today names
//! the same span of text on every box and after every rebuild of the same
//! source.
//!
//! The table stores only where each passage starts, as small numbers, one
//! row per entry. A passage's heading and link anchor are read from the
//! entry's own bytes when asked for, so they cost no space.

use crate::{Result, WaxError};

/// The one rule this version defines: cut before every HTML heading
/// element, `<h1>` to `<h6>` (SPEC v0.13 §5.10).
pub const RULE_HTML_HEADINGS: &str = "html-headings-1";

/// Rules this build can read.
pub const SUPPORTED_RULES: &[&str] = &[RULE_HTML_HEADINGS];

/// ASCII case-insensitive `starts_with` on bytes.
fn starts_ci(b: &[u8], at: usize, pat: &[u8]) -> bool {
    b.len() >= at + pat.len() && b[at..at + pat.len()].eq_ignore_ascii_case(pat)
}

fn is_tag_end(c: u8) -> bool {
    matches!(c, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'/' | b'>')
}

/// Find `pat` (ASCII case-insensitive) at or after `from`.
fn find_ci(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    if pat.is_empty() || b.len() < pat.len() {
        return None;
    }
    (from..=b.len() - pat.len()).find(|&i| b[i..i + pat.len()].eq_ignore_ascii_case(pat))
}

/// The `html-headings-1` cut points of `html`: the byte offset of the `<` of
/// every heading start tag, ascending, always beginning with `0` (the part
/// before the first heading, or the first heading itself when the entry
/// starts with one).
///
/// Normative (SPEC v0.13 §5.10). A heading start tag is `<`, then `h` or `H`,
/// then one digit `1`–`6`, then a tab, line feed, form feed, carriage
/// return, space, `/` or `>`. Text inside a comment (`<!--` to the next
/// `-->`) and inside a `script` or `style` element (from the start tag to
/// the next `</script` or `</style`, ASCII case-insensitive) is never
/// searched. Nothing else about HTML is interpreted, so any two writers cut
/// the same bytes the same way.
pub fn html_heading_starts(html: &[u8]) -> Vec<u64> {
    let mut starts = vec![0u64];
    let b = html;
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if starts_ci(b, i, b"<!--") {
            i = find_ci(b, i + 4, b"-->").map(|e| e + 3).unwrap_or(b.len());
            continue;
        }
        let raw = [&b"script"[..], &b"style"[..]].into_iter().find(|name| {
            starts_ci(b, i + 1, name) && b.get(i + 1 + name.len()).is_some_and(|&c| is_tag_end(c))
        });
        if let Some(name) = raw {
            let mut close = b"</".to_vec();
            close.extend_from_slice(name);
            i = find_ci(b, i + 1 + name.len(), &close).unwrap_or(b.len());
            continue;
        }
        if b.len() > i + 3
            && (b[i + 1] == b'h' || b[i + 1] == b'H')
            && (b'1'..=b'6').contains(&b[i + 2])
            && is_tag_end(b[i + 3])
        {
            if i > 0 {
                starts.push(i as u64);
            }
            i += 3;
            continue;
        }
        i += 1;
    }
    starts
}

/// Encode cut points for the `passages.starts` column: each start after the
/// first (which is always 0) as the unsigned LEB128 varint of its distance
/// from the previous one.
pub fn encode_starts(starts: &[u64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(starts.len() * 2);
    for w in starts.windows(2) {
        let mut d = w[1] - w[0];
        loop {
            let byte = (d & 0x7f) as u8;
            d >>= 7;
            if d == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }
    out
}

/// Decode `passages.starts` for an entry of `len` uncompressed bytes,
/// checking it: every distance at least 1, every start inside the entry.
pub fn decode_starts(path: &str, blob: &[u8], len: u64) -> Result<Vec<u64>> {
    let bad = |detail: &str| WaxError::BadPassageTable {
        path: path.to_string(),
        detail: detail.to_string(),
    };
    let mut starts = vec![0u64];
    let mut cur = 0u64;
    let mut i = 0usize;
    while i < blob.len() {
        let mut d: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = *blob.get(i).ok_or_else(|| bad("truncated varint"))?;
            i += 1;
            if shift >= 63 && byte > 1 {
                return Err(bad("varint overflows 64 bits"));
            }
            d |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        if d == 0 {
            return Err(bad("two passages start at the same byte"));
        }
        cur = cur.checked_add(d).ok_or_else(|| bad("start overflows"))?;
        if cur >= len {
            return Err(bad("a passage starts past the end of the entry"));
        }
        starts.push(cur);
    }
    Ok(starts)
}

/// One passage of an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passage {
    /// Position within the entry, from 0. With the path, the passage's id.
    pub ordinal: u32,
    /// Byte span within the entry's uncompressed content.
    pub start: u64,
    pub end: u64,
    /// The heading's visible text, when the passage starts with one.
    pub heading: Option<String>,
    /// The heading's `id` attribute, for linking as `#anchor`.
    pub anchor: Option<String>,
}

/// Cut `content` at `starts` and read each passage's heading and anchor.
pub fn describe(content: &[u8], starts: &[u64]) -> Vec<Passage> {
    let len = content.len() as u64;
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let end = starts.get(i + 1).copied().unwrap_or(len).min(len);
            let s = s.min(end);
            let (heading, anchor) = heading_at(&content[s as usize..end as usize]);
            Passage {
                ordinal: i as u32,
                start: s,
                end,
                heading,
                anchor,
            }
        })
        .collect()
}

/// The heading text and `id` of a passage that begins with a heading tag.
fn heading_at(span: &[u8]) -> (Option<String>, Option<String>) {
    if span.len() < 4 || span[0] != b'<' || !(span[1] == b'h' || span[1] == b'H') {
        return (None, None);
    }
    let level = span[2];
    if !(b'1'..=b'6').contains(&level) || !is_tag_end(span[3]) {
        return (None, None);
    }
    let tag_end = crate::text::tag_end(span, 0);
    let tag = String::from_utf8_lossy(&span[..tag_end]);
    let anchor = attr(&tag, "id");
    let close = [b'<', b'/', span[1], level];
    let body_end = find_ci(span, tag_end, &close).unwrap_or(span.len());
    let text = crate::text::html_to_text(&String::from_utf8_lossy(&span[tag_end..body_end]));
    let heading = Some(text.trim().to_string()).filter(|t| !t.is_empty());
    (heading, anchor)
}

/// The value of attribute `name` in one start tag's text.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut from = 0;
    while let Some(p) = lower[from..].find(name).map(|p| p + from) {
        from = p + name.len();
        let before_ok = p > 0 && b[p - 1].is_ascii_whitespace();
        let mut j = p + name.len();
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if !before_ok || b.get(j) != Some(&b'=') {
            continue;
        }
        j += 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let raw = &tag[j..];
        let v = match raw.as_bytes().first() {
            Some(&q) if q == b'"' || q == b'\'' => raw[1..].split(q as char).next().unwrap_or(""),
            _ => raw
                .split(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                .next()
                .unwrap_or(""),
        };
        let v = crate::text::html_to_text(v);
        return Some(v).filter(|v| !v.is_empty());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_before_every_heading_and_nowhere_else() {
        let html = b"<html><head><style>h2{}</style><script>x='<h2>'</script></head>\
            <body><p>lead</p><!-- <h2>no</h2> --><h2 id=\"History\">History</h2><p>a</p>\
            <H3 id=early>Early <i>days</i></H3><p>b</p><header>not a heading</header>\
            <h7>no</h7><h2>Later</h2>";
        let s = html_heading_starts(html);
        let at = |pat: &str| {
            html.windows(pat.len()).position(|w| w == pat.as_bytes()).unwrap() as u64
        };
        assert_eq!(s, vec![0, at("<h2 id"), at("<H3"), at("<h2>Later")]);
        let p = describe(html, &s);
        assert_eq!(p[1].heading.as_deref(), Some("History"));
        assert_eq!(p[1].anchor.as_deref(), Some("History"));
        assert_eq!(p[2].heading.as_deref(), Some("Early days"));
        assert_eq!(p[2].anchor.as_deref(), Some("early"));
        assert_eq!(p[3].anchor, None);
        assert_eq!(p[0].heading, None);
        assert_eq!(p.last().unwrap().end, html.len() as u64);
    }

    #[test]
    fn an_entry_starting_with_a_heading_has_no_empty_lead() {
        assert_eq!(html_heading_starts(b"<h1>T</h1>x<h2>y</h2>"), vec![0, 11]);
        assert_eq!(html_heading_starts(b"no headings"), vec![0]);
    }

    #[test]
    fn starts_round_trip_and_bad_tables_are_refused() {
        let s = vec![0, 1, 200, 70_000, 9_000_000];
        let blob = encode_starts(&s);
        assert_eq!(decode_starts("p", &blob, 9_000_001).unwrap(), s);
        assert_eq!(decode_starts("p", &[], 10).unwrap(), vec![0]);
        assert!(decode_starts("p", &blob, 9_000_000).is_err(), "start past end");
        assert!(decode_starts("p", &[0], 10).is_err(), "zero distance");
        assert!(decode_starts("p", &[0x80], 10).is_err(), "truncated");
        assert!(decode_starts("p", &[0xff; 11], u64::MAX).is_err(), "overflow");
    }
}
