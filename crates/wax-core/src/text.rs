//! HTML → plain text for the embedded search index (SPEC §5.7) and for passage
//! scoring (SPEC v0.13 §5.10).
//!
//! The index is contentless (`content=''`), so what goes in is only ever
//! tokenized, never shown: this does not need to render a page, only to hand
//! the tokenizer the words a reader would see. It therefore:
//!
//! * drops markup, comments, and the contents of `<script>`, `<style>`,
//!   `<noscript>`, `<template>` and `<head>` (none of which a reader sees);
//! * decodes character references — numeric, and the named ones that occur
//!   in real pack content — so `caf&eacute;` indexes as `café`;
//! * turns every tag boundary into a space, so `a<br>b` is two words —
//!   **except** the boundaries of inline phrasing elements (`<span>`, `<a>`,
//!   `<em>`, `<ruby>` …), which a reader does not see as a gap. Japanese and
//!   Chinese put no spaces between words, so a space at `<ruby>角</ruby>が`
//!   would cut the run `角が立つ` in two and `wax-bigram-16` (SPEC §5.7.2)
//!   could never find the phrase;
//! * drops ruby annotations (`<rt>`, `<rp>`): the reading printed above `智`
//!   is not part of the sentence, and keeping it puts `ち` between `智` and
//!   `に`, which breaks every phrase across it. The base text stays.
//!
//! It is a pure function of its input, which SPEC §5.7's determinism rule
//! needs: the same page always yields the same text, so the same index.

/// Elements whose contents are not text a reader sees, or (ruby annotations)
/// not part of the running text.
const SKIP: &[&str] = &["script", "style", "noscript", "template", "head", "rt", "rp"];

/// Inline phrasing elements: their tag boundaries are not word breaks.
const INLINE: &[&str] = &[
    "a", "abbr", "b", "bdi", "bdo", "cite", "code", "data", "del", "dfn", "em", "font", "i", "ins", "kbd", "mark",
    "q", "rb", "rp", "rt", "rtc", "ruby", "s", "samp", "small", "span", "strong", "sub", "sup", "time", "tt", "u",
    "var", "wbr",
];

/// `<rt>` and `<rp>` may be left unclosed in HTML; the annotation then ends
/// at the next ruby part or at `</ruby>`, never later.
fn ends_ruby_note(skipping: &str, name: &str, closing: bool) -> bool {
    matches!(skipping, "rt" | "rp")
        && (matches!(name, "rb" | "rt" | "rp" | "rtc") && !closing || name == "ruby" && closing)
}

/// Visible text of an HTML document, whitespace-collapsed.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 3);
    let b = html.as_bytes();
    let mut i = 0;
    let mut skip_until: Option<&'static str> = None;

    while i < b.len() {
        if b[i] == b'<' {
            if html[i..].starts_with("<!--") {
                i = html[i..].find("-->").map(|e| i + e + 3).unwrap_or(b.len());
                continue;
            }
            let end = tag_end(b, i);
            let tag = &html[i + 1..end.saturating_sub(1).max(i + 1)];
            let closing = tag.starts_with('/');
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect();
            if let Some(s) = skip_until {
                if (closing && name == s) || ends_ruby_note(s, &name, closing) {
                    skip_until = None;
                }
            }
            match skip_until {
                Some(_) => {}
                None => {
                    if !closing && !tag.ends_with('/') {
                        if let Some(s) = SKIP.iter().find(|s| **s == name) {
                            skip_until = Some(s);
                        }
                    }
                }
            }
            if !INLINE.contains(&name.as_str()) {
                push_space(&mut out);
            }
            i = end;
            continue;
        }
        let start = i;
        while i < b.len() && b[i] != b'<' {
            i += 1;
        }
        if skip_until.is_none() {
            push_text(&mut out, &html[start..i]);
        }
    }
    let t = out.trim_end().len();
    out.truncate(t);
    out
}

/// Index just past the `>` closing the tag that starts at `i`, honouring
/// quoted attribute values.
pub(crate) fn tag_end(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    let mut quote: Option<u8> = None;
    while j < b.len() {
        let c = b[j];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if c == b'>' => return j + 1,
            None => {}
        }
        j += 1;
    }
    b.len()
}

fn push_space(out: &mut String) {
    if !out.is_empty() && !out.ends_with(' ') {
        out.push(' ');
    }
}

fn push_text(out: &mut String, raw: &str) {
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        push_words(out, &rest[..amp]);
        let after = &rest[amp + 1..];
        match decode_ref(after) {
            Some((ch, used)) => {
                if ch.is_whitespace() {
                    push_space(out);
                } else {
                    out.push(ch);
                }
                rest = &after[used..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    push_words(out, rest);
}

fn push_words(out: &mut String, s: &str) {
    for ch in s.chars() {
        if ch.is_whitespace() {
            push_space(out);
        } else {
            out.push(ch);
        }
    }
}

/// Decode one character reference (the text after `&`). Returns the char and
/// how many bytes of `s` it used, including the `;`.
fn decode_ref(s: &str) -> Option<(char, usize)> {
    let semi = s.find(';').filter(|&n| n <= 32)?;
    let body = &s[..semi];
    let ch = if let Some(num) = body.strip_prefix('#') {
        let v = if let Some(h) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
            u32::from_str_radix(h, 16).ok()?
        } else {
            num.parse::<u32>().ok()?
        };
        char::from_u32(v).filter(|c| *c != '\0')?
    } else {
        named(body)?
    };
    Some((ch, semi + 1))
}

/// Named references. Not the whole HTML table: the ones that occur in
/// converted wiki and book content. An unknown name is left as literal text,
/// which costs one unmatched token, never a wrong one.
fn named(n: &str) -> Option<char> {
    Some(match n {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" | "ensp" | "emsp" | "thinsp" => ' ',
        "shy" => return None,
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "middot" => '·',
        "bull" => '•',
        "deg" => '°',
        "times" => '×',
        "divide" => '÷',
        "minus" => '−',
        "plusmn" => '±',
        "micro" => 'µ',
        "copy" => '©',
        "reg" => '®',
        "sect" => '§',
        "para" => '¶',
        "eacute" => 'é',
        "egrave" => 'è',
        "ecirc" => 'ê',
        "euml" => 'ë',
        "aacute" => 'á',
        "agrave" => 'à',
        "acirc" => 'â',
        "auml" => 'ä',
        "atilde" => 'ã',
        "aring" => 'å',
        "aelig" => 'æ',
        "ccedil" => 'ç',
        "iacute" => 'í',
        "igrave" => 'ì',
        "icirc" => 'î',
        "iuml" => 'ï',
        "ntilde" => 'ñ',
        "oacute" => 'ó',
        "ograve" => 'ò',
        "ocirc" => 'ô',
        "ouml" => 'ö',
        "otilde" => 'õ',
        "oslash" => 'ø',
        "uacute" => 'ú',
        "ugrave" => 'ù',
        "ucirc" => 'û',
        "uuml" => 'ü',
        "yacute" => 'ý',
        "szlig" => 'ß',
        "Eacute" => 'É',
        "Agrave" => 'À',
        "Ccedil" => 'Ç',
        "Ouml" => 'Ö',
        "Uuml" => 'Ü',
        "Auml" => 'Ä',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::html_to_text;

    #[test]
    fn strips_markup_and_hidden_elements() {
        let html = "<html><head><title>T</title><style>p{color:red}</style></head>\
                    <body><p>Hydrogen <b>is</b> light.</p><script>var x='<p>no</p>';</script>\
                    <!-- hidden --><p>Second&nbsp;line</p></body></html>";
        assert_eq!(html_to_text(html), "Hydrogen is light. Second line");
    }

    #[test]
    fn decodes_references() {
        assert_eq!(html_to_text("caf&eacute; &amp; cr&#232;me &#x41;"), "café & crème A");
        assert_eq!(html_to_text("AT&T &unknown; x"), "AT&T &unknown; x");
    }

    #[test]
    fn tag_boundaries_split_words() {
        assert_eq!(html_to_text("a<br>b<td>c</td>"), "a b c");
        assert_eq!(html_to_text("<a title=\"x > y\">link</a>"), "link");
    }

    #[test]
    fn inline_tags_do_not_split_words() {
        // Japanese has no spaces: an inline tag must not cut the run.
        assert_eq!(html_to_text("<p><span>角</span>が<a href=x>立つ</a>。</p>"), "角が立つ。");
        assert_eq!(html_to_text("<p>un<em>believ</em>able</p><p>next</p>"), "unbelievable next");
    }

    #[test]
    fn ruby_annotations_are_dropped() {
        // 草枕, first line, as the IDPF sample writes it.
        let html = "<p><ruby>智<rt>ち</rt></ruby>に働けば<ruby>角<rt>かど</rt></ruby>が立つ。</p>";
        assert_eq!(html_to_text(html), "智に働けば角が立つ。");
        assert_eq!(html_to_text("<ruby>漢<rp>(</rp><rt>かん</rt><rp>)</rp>字</ruby>"), "漢字");
        // Unclosed <rt> (valid HTML) ends at </ruby>, not at the end of the page.
        assert_eq!(html_to_text("<ruby>漢<rt>かん</ruby>字<p>kept</p>"), "漢字 kept");
        assert_eq!(html_to_text("<ruby>漢<rt>かん<rb>字<rt>じ</ruby>"), "漢字");
    }

    #[test]
    fn self_closing_skip_tags_do_not_swallow_the_page() {
        assert_eq!(html_to_text("<script src=x.js/><p>kept</p>"), "kept");
    }
}
