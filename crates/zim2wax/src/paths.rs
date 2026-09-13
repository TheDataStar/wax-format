//! Path canonicalization — Track B §20, first [Critical] item.
//!
//! This is the decision that breaks every internal link in a converted
//! Wikipedia if it is wrong, so the rule is stated once, here, and every other
//! module (redirects, hrefs, manifest derivation) calls into it.
//!
//! # The rule
//!
//! A ZIM dirent is `(namespace byte, url)`. Its canonical `entries.path` is:
//!
//! | Source | Canonical | Why |
//! |---|---|---|
//! | article (`text/html`) in `A/` or `C/` | `<url>` + `.html` if it has no `.html`/`.htm` suffix | §20: article namespace flattens to the root, prefix stripped |
//! | any other content in `A/`, `C/`, `I/`, `-/` | `_assets/<url>` | §20: images and media under `_assets/` |
//! | `M/` | `_meta/<url>` | §20: metadata under `_meta/` |
//! | `B/`, `J/`, `U/`, `V/` (legacy metadata-ish) | `_meta/<ns>/<url>` | keeps the four legacy namespaces from colliding |
//! | `W/`, `X/` | *not emitted* | well-known pointers and search indexes; see [`Disposition`] |
//!
//! ZIM ≥ 6.1 puts articles **and** assets in `C/`, so the namespace byte alone
//! cannot classify — a `C/` entry is an article iff its mimetype is
//! `text/html`. Under the legacy scheme `A/` was html-only anyway, so the same
//! mimetype test is applied uniformly.
//!
//! # Case
//!
//! Case is **preserved**. §20's example writes `A/Photosynthesis →
//! photosynthesis.html`; taking that lowercase literally would collide dirents
//! that real ZIMs keep distinct — `13th_Amendment` / `13th_amendment` and
//! `4H_disease` / `4h_disease` are separate entries in `wikipedia_en_100`.
//! Lowercasing is irreversible and the spec's own rule is that comparison is
//! exact and case-sensitive; only the prefix strip and the `.html` suffix from
//! the example are applied. Flagged in the B1 summary.
//!
//! # `_assets/` and `_meta/` are reserved
//!
//! An article whose url already starts with `_assets/` or `_meta/` would land
//! inside a reserved prefix and shadow a real asset. That is a
//! [`Disposition::ReservedPrefixCollision`] and the entry is dropped with a
//! warning rather than emitted.

use zim::Namespace;

/// Reserved prefix for images, media, CSS, JS, fonts — every non-article
/// content entry (§20).
pub const ASSETS_PREFIX: &str = "_assets/";
/// Reserved prefix for ZIM metadata entries (§20).
pub const META_PREFIX: &str = "_meta/";

/// What zim2wax does with a dirent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// Emit as a content entry at this canonical path.
    Emit(String),
    /// `X/` — the embedded Xapian / listing indexes. §4: rebuilt, not copied.
    /// Not emitted (and the FTS5 rebuild is out of scope for v0, Contract §13).
    SearchIndex,
    /// `W/` — libzim's well-known entries (`mainPage`). The main page reaches
    /// the manifest through `entry_point`; nothing else in `W/` is content.
    WellKnown,
    /// An article url that would land inside `_assets/` or `_meta/`.
    ReservedPrefixCollision,
}

/// The mimetype with any parameters stripped and lower-cased:
/// `text/html; charset=utf-8` → `text/html`.
pub fn bare_mime(mime: &str) -> String {
    mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// True for the mimetype of an article — the thing that flattens to the root.
pub fn is_article_mime(bare_mime: &str) -> bool {
    bare_mime == "text/html" || bare_mime == "application/xhtml+xml"
}

/// Canonical path for a content dirent. `mime` is the dirent's mimetype string
/// (parameters allowed). Redirect dirents have no mimetype of their own; pass
/// the *target's* mime so the alias lands where the target does.
pub fn canonicalize(namespace: Namespace, url: &str, mime: &str) -> Disposition {
    use Namespace::*;
    let url = url.trim_start_matches('/');
    match namespace {
        FulltextIndex => Disposition::SearchIndex,
        CategoriesArticle => Disposition::WellKnown,
        Metadata => Disposition::Emit(format!("{META_PREFIX}{url}")),
        ArticleMetaData | ImagesText | CategoriesText | CategoriesArticleList => {
            Disposition::Emit(format!("{META_PREFIX}{}/{url}", namespace.as_byte() as char))
        }
        Articles | UserContent | ImagesFile | Layout | Other(_) => {
            if is_article_mime(&bare_mime(mime)) {
                if url.starts_with(ASSETS_PREFIX) || url.starts_with(META_PREFIX) {
                    return Disposition::ReservedPrefixCollision;
                }
                Disposition::Emit(with_html_suffix(url))
            } else {
                Disposition::Emit(format!("{ASSETS_PREFIX}{url}"))
            }
        }
    }
}

/// Append `.html` unless the url already ends in `.html` / `.htm`
/// (case-insensitive check on the suffix only; the path itself keeps its case).
pub fn with_html_suffix(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.ends_with(".html") || lower.ends_with(".htm") {
        url.to_string()
    } else {
        format!("{url}.html")
    }
}

// ---------------------------------------------------------------------------
// Href resolution
// ---------------------------------------------------------------------------

/// A parsed in-document reference, split so the parts a rewrite must preserve
/// (query, fragment) survive untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Href<'a> {
    /// Everything before `?`/`#`, percent-decoded.
    pub path: String,
    /// `?…` including the `?`, verbatim, if present.
    pub query: &'a str,
    /// `#…` including the `#`, verbatim, if present.
    pub fragment: &'a str,
}

/// Split an attribute value into path / query / fragment and percent-decode
/// the path. Returns `None` for references that can never point into the ZIM:
/// absolute URLs with a scheme, protocol-relative `//…`, `mailto:`, `data:`,
/// `javascript:`, and empty or fragment-only values.
pub fn parse_href(raw: &str) -> Option<Href<'_>> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('#') || raw.starts_with("//") {
        return None;
    }
    // A real URL scheme, as opposed to a Wikipedia title with a colon
    // ("File:Foo.jpg", "Category:Birds"). Anything followed by "//" is a scheme;
    // otherwise only the handful of schemes that appear in hrefs count.
    if let Some(colon) = raw.find(':') {
        let scheme = raw[..colon].to_ascii_lowercase();
        let syntactically_scheme = !scheme.is_empty()
            && scheme.as_bytes()[0].is_ascii_alphabetic()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.');
        let has_authority = raw[colon + 1..].starts_with("//");
        let known_opaque = matches!(
            scheme.as_str(),
            "mailto" | "data" | "javascript" | "tel" | "sms" | "blob" | "about" | "urn"
                | "magnet" | "geo" | "ipfs" | "ipns" | "news" | "xmpp"
        );
        if syntactically_scheme && (has_authority || known_opaque) {
            return None;
        }
    }
    let (before_frag, fragment) = match raw.find('#') {
        Some(i) => (&raw[..i], &raw[i..]),
        None => (raw, ""),
    };
    let (path_enc, query) = match before_frag.find('?') {
        Some(i) => (&before_frag[..i], &before_frag[i..]),
        None => (before_frag, ""),
    };
    Some(Href {
        path: percent_decode(path_enc),
        query,
        fragment,
    })
}

/// Resolve `href_path` (already decoded, no query/fragment) against the dirent
/// that contains it, returning the `(namespace, url)` it names inside the ZIM.
///
/// The base is the virtual absolute path `/<ns>/<url>`; RFC 3986 dot-segment
/// removal is applied; the first segment of the result is the namespace. A
/// reference that climbs above the root, or lands on a bare namespace with no
/// url, resolves to `None`.
pub fn resolve_in_zim(base_ns: Namespace, base_url: &str, href_path: &str) -> Option<(Namespace, String)> {
    let base_ns_char = base_ns.as_byte() as char;
    // Absolute-from-root references ("/C/Foo" or "/Foo") — the first form names
    // a namespace explicitly; the second is treated as within the base namespace.
    let joined: String = if let Some(abs) = href_path.strip_prefix('/') {
        let mut parts = abs.splitn(2, '/');
        let first = parts.next().unwrap_or("");
        if first.len() == 1 && parts.clone().next().is_some() {
            abs.to_string()
        } else {
            format!("{base_ns_char}/{abs}")
        }
    } else {
        // relative: replace the last segment of the base url
        let base_dir = match base_url.rfind('/') {
            Some(i) => &base_url[..i],
            None => "",
        };
        if base_dir.is_empty() {
            format!("{base_ns_char}/{href_path}")
        } else {
            format!("{base_ns_char}/{base_dir}/{href_path}")
        }
    };

    // dot-segment removal over "<ns>/<segments…>"
    let mut out: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "." | "" if !out.is_empty() => {
                // "" from a trailing slash or "//": keep a trailing "" so that
                // "dir/" resolves to "dir/" (which will not match a dirent).
                if seg.is_empty() {
                    out.push("");
                }
            }
            "." => {}
            ".." => {
                // The virtual root sits above the namespaces, so "../I/x" from
                // "/A/Foo" legitimately climbs to "/" and into "I/". Only
                // climbing above the root is an error.
                if out.is_empty() {
                    return None;
                }
                out.pop();
            }
            s => out.push(s),
        }
    }
    if out.len() < 2 {
        return None;
    }
    let ns_str = out[0];
    if ns_str.len() != 1 {
        return None;
    }
    let ns = Namespace::from(ns_str.as_bytes()[0]);
    let url = out[1..].join("/");
    if url.is_empty() {
        return None;
    }
    Some((ns, url))
}

/// Percent-decode; invalid sequences are left as-is.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = &s[i + 1..i + 3];
            if let Ok(v) = u8::from_str_radix(h, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encode a canonical path for use inside an href. Only the characters
/// that would break an attribute value or be misread as a delimiter are
/// encoded; UTF-8 is left raw (every browser accepts it and it keeps Wikipedia
/// titles readable in view-source).
pub fn percent_encode_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for c in p.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '"' => out.push_str("%22"),
            '#' => out.push_str("%23"),
            '%' => out.push_str("%25"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '?' => out.push_str("%3F"),
            c => out.push(c),
        }
    }
    out
}

/// The href a rewritten document uses to reach `canonical`: root-relative.
///
/// A pack is served as the root of its own origin (Contract §8: one
/// `pack-<n>-slot-<m>.deltos-packs.lan` origin per pack, and `127.0.0.1:PORT`
/// pre-proxy), so `/`-rooted references are correct at any document depth and
/// need no `../` arithmetic.
pub fn href_for(canonical: &str) -> String {
    format!("/{}", percent_encode_path(canonical))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_flattens_to_root_with_html_suffix() {
        assert_eq!(
            canonicalize(Namespace::Articles, "Photosynthesis", "text/html"),
            Disposition::Emit("Photosynthesis.html".into())
        );
        assert_eq!(
            canonicalize(Namespace::UserContent, "Michael_Jackson", "text/html; charset=utf-8"),
            Disposition::Emit("Michael_Jackson.html".into())
        );
        // already suffixed: unchanged
        assert_eq!(
            canonicalize(Namespace::Articles, "main.html", "text/html"),
            Disposition::Emit("main.html".into())
        );
        assert_eq!(
            canonicalize(Namespace::UserContent, "index.HTM", "text/html"),
            Disposition::Emit("index.HTM".into())
        );
    }

    #[test]
    fn case_is_preserved_not_folded() {
        let a = canonicalize(Namespace::UserContent, "13th_Amendment", "text/html");
        let b = canonicalize(Namespace::UserContent, "13th_amendment", "text/html");
        assert_ne!(a, b);
        assert_eq!(a, Disposition::Emit("13th_Amendment.html".into()));
    }

    #[test]
    fn non_article_content_goes_under_assets() {
        assert_eq!(
            canonicalize(Namespace::ImagesFile, "favicon.png", "image/png"),
            Disposition::Emit("_assets/favicon.png".into())
        );
        assert_eq!(
            canonicalize(Namespace::UserContent, "_assets_/ab.jpg", "image/webp"),
            Disposition::Emit("_assets/_assets_/ab.jpg".into())
        );
        assert_eq!(
            canonicalize(Namespace::UserContent, "_res_/style.css", "text/css"),
            Disposition::Emit("_assets/_res_/style.css".into())
        );
        assert_eq!(
            canonicalize(Namespace::Layout, "style.css", "text/css"),
            Disposition::Emit("_assets/style.css".into())
        );
    }

    #[test]
    fn metadata_and_legacy_namespaces_go_under_meta() {
        assert_eq!(
            canonicalize(Namespace::Metadata, "Title", "text/plain"),
            Disposition::Emit("_meta/Title".into())
        );
        assert_eq!(
            canonicalize(Namespace::ArticleMetaData, "x", "text/plain"),
            Disposition::Emit("_meta/B/x".into())
        );
    }

    #[test]
    fn indexes_and_wellknown_are_not_emitted() {
        assert_eq!(
            canonicalize(Namespace::FulltextIndex, "fulltext/xapian", "application/octet-stream+xapian"),
            Disposition::SearchIndex
        );
        assert_eq!(
            canonicalize(Namespace::CategoriesArticle, "mainPage", "text/html"),
            Disposition::WellKnown
        );
    }

    #[test]
    fn article_inside_reserved_prefix_is_a_collision() {
        assert_eq!(
            canonicalize(Namespace::UserContent, "_assets/evil", "text/html"),
            Disposition::ReservedPrefixCollision
        );
        assert_eq!(
            canonicalize(Namespace::Articles, "_meta/x", "text/html"),
            Disposition::ReservedPrefixCollision
        );
    }

    #[test]
    fn parse_href_splits_and_decodes() {
        let h = parse_href("Michael_Jackson%27s_Thriller?x=1#Section").unwrap();
        assert_eq!(h.path, "Michael_Jackson's_Thriller");
        assert_eq!(h.query, "?x=1");
        assert_eq!(h.fragment, "#Section");
        assert!(parse_href("https://en.wikipedia.org/wiki/X").is_none());
        assert!(parse_href("mailto:a@b").is_none());
        assert!(parse_href("javascript:void(0)").is_none());
        assert!(parse_href("data:image/png;base64,AAAA").is_none());
        assert!(parse_href("#top").is_none());
        assert!(parse_href("").is_none());
        assert!(parse_href("//cdn.example/x.js").is_none());
        // a Wikipedia "File:"/"Category:" title is not a scheme
        assert_eq!(parse_href("File:Foo.jpg").unwrap().path, "File:Foo.jpg");
        assert_eq!(parse_href("Category:Birds").unwrap().path, "Category:Birds");
        assert!(parse_href("ftp://x/y").is_none());
        assert!(parse_href("tel:+123").is_none());
        assert_eq!(parse_href("./_res_/style.css").unwrap().path, "./_res_/style.css");
    }

    #[test]
    fn resolve_relative_references_inside_the_zim() {
        let c = Namespace::UserContent;
        assert_eq!(resolve_in_zim(c, "African_Americans", "Michael_Jackson"), Some((c, "Michael_Jackson".into())));
        assert_eq!(resolve_in_zim(c, "African_Americans", "./_assets_/x.png"), Some((c, "_assets_/x.png".into())));
        assert_eq!(resolve_in_zim(c, "dir/page", "../other"), Some((c, "other".into())));
        assert_eq!(resolve_in_zim(c, "dir/page", "sib"), Some((c, "dir/sib".into())));
        // legacy cross-namespace: ../I/foo.png from A/Foo
        assert_eq!(
            resolve_in_zim(Namespace::Articles, "Foo", "../I/foo.png"),
            Some((Namespace::ImagesFile, "foo.png".into()))
        );
        // absolute with explicit namespace
        assert_eq!(resolve_in_zim(c, "x", "/I/bar.png"), Some((Namespace::ImagesFile, "bar.png".into())));
        // climbing out of the archive
        assert_eq!(resolve_in_zim(c, "x", "../../etc"), None);
        // a directory reference never names a dirent
        assert_eq!(resolve_in_zim(c, "x", "dir/"), Some((c, "dir/".into())));
    }

    #[test]
    fn href_for_is_root_relative_and_encoded() {
        assert_eq!(href_for("Michael_Jackson.html"), "/Michael_Jackson.html");
        assert_eq!(href_for("_assets/a b#c.png"), "/_assets/a%20b%23c.png");
        assert_eq!(href_for("Cafe\u{301}.html"), "/Cafe\u{301}.html");
    }

    #[test]
    fn percent_decode_leaves_invalid_sequences() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
        assert_eq!(percent_decode("%C3%A9"), "é");
    }
}
