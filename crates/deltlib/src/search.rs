//! Search across every pack a person may see (deltd API §6.3).
//!
//! Each pack answers from its own embedded index (SPEC §5.7), which
//! `wax-core` already merges across that pack's segments by position. Packs
//! are then fused the same way: hit *n* of every pack sits at position *n*,
//! ties broken by `archive_uuid`. The packs' lists are disjoint (a hit names
//! its pack), so this is exactly Reciprocal Rank Fusion (contract §13
//! decision 6) for any constant, with no constant to choose.
//!
//! **Queries never reach FTS5 as typed.** The text is split into words the
//! same way the declared tokenizer splits them and each word is quoted, so
//! no FTS5 syntax (`OR`, `NEAR`, `*`, `:`, quotes) can be injected, and so a
//! word never becomes a phrase: SPEC §5.7's `detail=none` refuses phrase
//! queries outright.
//!
//! **CJK and `ß`** *(SPEC v0.13)*: packs declaring `wax-bigram-16` get their
//! query from `wax_core::tokenize`, the same code that rewrote their text.
//!
//! **Accent-insensitive.** `unicode61` removes diacritics on both sides, so
//! `cafe` finds `café`. Title completion follows each pack's declared fold
//! (SPEC §5.7.1), which keeps accents; [`suggest_pack`] adds the
//! accent-insensitive matches after the exact ones.

use std::collections::HashSet;

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;
use wax_core::{WaxError, WaxReader};

/// Why one pack gave no results for a query. Listed in the response, never
/// silently dropped (deltd API §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// The pack carries no search index.
    NoIndex,
    /// The pack's tokenizer is one this box cannot reproduce.
    UnknownTokenizer,
    /// The pack's segments declare different tokenizers.
    TokenizerMismatch,
    /// Every word is shorter than the tokenizer can match (`trigram`: 3).
    QueryTooShort,
    /// The pack's index refused the query.
    QueryNotSupported,
}

impl Skip {
    pub fn code(self) -> &'static str {
        match self {
            Skip::NoIndex => "no_index",
            Skip::UnknownTokenizer => "unknown_tokenizer",
            Skip::TokenizerMismatch => "tokenizer_mismatch",
            Skip::QueryTooShort => "query_too_short",
            Skip::QueryNotSupported => "query_not_supported",
        }
    }
}

/// The words of `q` as `unicode61` separates them: runs of letters and
/// digits (Unicode categories L and N, and the marks that attach to them).
pub fn words(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in q.chars() {
        if ch.is_alphanumeric() || (is_combining_mark(ch) && !cur.is_empty()) {
            cur.push(ch);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The FTS5 expression for `q` under `tokenizer`, or why there is none.
/// `prefix_last` makes the last word a prefix (`"caf"*`), for completion.
pub fn match_expr(q: &str, tokenizer: &str, prefix_last: bool) -> Result<String, Skip> {
    // SPEC v0.13 §5.7.2: the query goes through the same rewriting as the
    // pack's text, which wax-core owns, so box and builder cannot drift.
    if tokenizer == wax_core::tokenize::WAX_BIGRAM {
        return wax_core::tokenize::match_expr(q, prefix_last).ok_or(Skip::QueryTooShort);
    }
    let ws = words(q);
    if ws.is_empty() {
        return Err(Skip::QueryTooShort);
    }
    let mut terms: Vec<String> = Vec::new();
    match tokenizer {
        "unicode61" => {
            let n = ws.len();
            for (i, w) in ws.into_iter().enumerate() {
                let star = if prefix_last && i + 1 == n { "*" } else { "" };
                terms.push(format!("\"{w}\"{star}"));
            }
        }
        "trigram" => {
            // Under detail=none a term of more than three characters is a
            // phrase and is refused, so a longer word is asked as every one
            // of its three-character windows, all required. That matches
            // every page containing the word, and may also match a page that
            // has the windows apart. A word under three characters cannot be
            // matched at all (a recorded format gap).
            for w in ws {
                let cs: Vec<char> = w.chars().collect();
                if cs.len() < 3 {
                    continue;
                }
                for win in cs.windows(3) {
                    let t: String = win.iter().collect();
                    terms.push(format!("\"{}\"", t.replace('"', "\"\"")));
                }
            }
            if terms.is_empty() {
                return Err(Skip::QueryTooShort);
            }
        }
        _ => return Err(Skip::UnknownTokenizer),
    }
    Ok(terms.join(" "))
}

/// One hit, at its position in its own pack's ranked list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub pack: String,
    pub position: usize,
    pub path: String,
    pub title: Option<String>,
}

/// How many title matches go ahead of the full-text hits in one pack.
pub const TITLE_BOOST: usize = 5;

/// Up to `limit` ranked hits from one pack.
///
/// Pages whose title *is* the query (then: begins with it, at a word
/// boundary) come first, up to [`TITLE_BOOST`], then the full-text ranking.
/// The boost exists because SPEC §5.7's `detail=none` index keeps neither
/// columns nor term counts, so its bm25 cannot tell a page *about* the query
/// from a short page that merely mentions it. The list is a pure function of
/// the query and the pack, so cursor paging stays stable.
pub fn search_pack(r: &WaxReader, pack: &str, q: &str, limit: usize) -> Result<Vec<Hit>, Skip> {
    let tok = r.search_tokenizer().ok_or(Skip::NoIndex)?.to_string();
    let expr = match_expr(q, &tok, false)?;
    let mut ranked: Vec<(String, Option<String>)> = Vec::new();
    let want = base_fold(q.trim());
    if let Ok(cands) = r.suggest(q.trim(), TITLE_BOOST * 4) {
        let mut exact = Vec::new();
        let mut starts = Vec::new();
        for c in cands {
            let f = base_fold(&c.title);
            if f == want {
                exact.push((c.path, Some(c.title)));
            } else if f.starts_with(&format!("{want} ")) {
                starts.push((c.path, Some(c.title)));
            }
        }
        ranked.extend(exact.into_iter().chain(starts).take(TITLE_BOOST));
    }
    let hits = r.search(&expr, limit + ranked.len()).map_err(|e| skip_for(&e))?;
    let boosted: HashSet<String> = ranked.iter().map(|(p, _)| p.clone()).collect();
    ranked.extend(hits.into_iter().filter(|h| !boosted.contains(&h.path)).map(|h| (h.path, h.title)));
    Ok(ranked
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(i, (path, title))| Hit { pack: pack.to_string(), position: i, path, title })
        .collect())
}

fn skip_for(e: &WaxError) -> Skip {
    match e {
        WaxError::SearchUnavailable => Skip::NoIndex,
        WaxError::UnknownTokenizer { .. } => Skip::UnknownTokenizer,
        WaxError::TokenizerMismatch { .. } => Skip::TokenizerMismatch,
        _ => Skip::QueryNotSupported,
    }
}

/// Fuse per-pack ranked lists by position, ties by pack id (RRF for
/// disjoint lists).
pub fn fuse(mut lists: Vec<Vec<Hit>>) -> Vec<Hit> {
    let mut all: Vec<Hit> = lists.drain(..).flatten().collect();
    all.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.pack.cmp(&b.pack)));
    all
}

/// A title, folded for accent-insensitive comparison: NFKC case fold, then
/// every combining mark removed. Used only for matching on the box; it is
/// not a fold any pack declares.
pub fn base_fold(s: &str) -> String {
    let folded = caseless::default_case_fold_str(&s.nfkc().collect::<String>());
    folded.nfd().filter(|c| !is_combining_mark(*c)).nfc().collect()
}

/// How many full-text hits the accent-insensitive completion pass inspects
/// per pack. A bound on work per keystroke, not a result count.
pub const SUGGEST_SCAN: usize = 200;

/// One completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub pack: String,
    pub path: String,
    pub title: String,
    /// Matched exactly under the pack's declared fold, rather than only once
    /// accents are ignored.
    pub exact: bool,
}

/// Title completions from one pack: exact matches under the declared fold
/// first (SPEC §5.7.1), then titles that match once accents are ignored.
pub fn suggest_pack(r: &WaxReader, pack: &str, q: &str, limit: usize) -> Result<Vec<Completion>, Skip> {
    let tok = r.search_tokenizer().ok_or(Skip::NoIndex)?.to_string();
    let mut out: Vec<Completion> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    match r.suggest(q, limit) {
        Ok(s) => {
            for c in s {
                seen.insert(c.path.clone());
                out.push(Completion { pack: pack.into(), path: c.path, title: c.title, exact: true });
            }
        }
        Err(WaxError::UnknownTitleFold { .. }) => {}
        Err(e) => return Err(skip_for(&e)),
    }
    if out.len() >= limit || !(tok == "unicode61" || tok == wax_core::tokenize::WAX_BIGRAM) {
        return Ok(out);
    }
    let want = base_fold(q.trim_start());
    if want.is_empty() {
        return Ok(out);
    }
    let Ok(expr) = match_expr(q, &tok, true) else { return Ok(out) };
    if let Ok(hits) = r.search(&expr, SUGGEST_SCAN) {
        let mut extra: Vec<Completion> = hits
            .into_iter()
            .filter_map(|h| {
                let title = h.title?;
                (!seen.contains(&h.path) && base_fold(&title).starts_with(&want)).then(|| Completion {
                    pack: pack.into(),
                    path: h.path,
                    title,
                    exact: false,
                })
            })
            .collect();
        extra.sort_by(|a, b| base_fold(&a.title).cmp(&base_fold(&b.title)).then(a.path.cmp(&b.path)));
        out.extend(extra.into_iter().take(limit - out.len()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_like_unicode61() {
        assert_eq!(words("e-mail OR \"x\" NEAR(a b) c*"), ["e", "mail", "OR", "x", "NEAR", "a", "b", "c"]);
        assert_eq!(words("  "), Vec::<String>::new());
        assert_eq!(words("Montréal café"), ["Montréal", "café"]);
    }

    #[test]
    fn expressions_are_quoted_words() {
        assert_eq!(match_expr("hydrogen OR oxygen", "unicode61", false).unwrap(), "\"hydrogen\" \"OR\" \"oxygen\"");
        assert_eq!(match_expr("caf", "unicode61", true).unwrap(), "\"caf\"*");
        assert_eq!(match_expr("北京大学", "trigram", false).unwrap(), "\"北京大\" \"京大学\"");
        assert_eq!(match_expr("北京", "trigram", false), Err(Skip::QueryTooShort));
        // v0.13: wax-bigram-16 answers what trigram could not
        assert_eq!(match_expr("北京", "wax-bigram-16", false).unwrap(), "\"北京\"");
        assert_eq!(match_expr("北京大学", "wax-bigram-16", false).unwrap(), "\"北京\" \"京大\" \"大学\"");
        assert_eq!(match_expr("Straße", "wax-bigram-16", true).unwrap(), "\"strasse\"*");
        assert_eq!(match_expr("x", "martian", false), Err(Skip::UnknownTokenizer));
        assert_eq!(match_expr("!!", "unicode61", false), Err(Skip::QueryTooShort));
    }

    #[test]
    fn fusion_is_by_position_then_pack() {
        let h = |p: &str, i: usize| Hit { pack: p.into(), position: i, path: format!("{p}{i}"), title: None };
        let fused = fuse(vec![vec![h("b", 0), h("b", 1)], vec![h("a", 0), h("a", 1), h("a", 2)]]);
        let order: Vec<String> = fused.iter().map(|x| x.path.clone()).collect();
        assert_eq!(order, ["a0", "b0", "a1", "b1", "a2"]);
    }

    #[test]
    fn base_fold_ignores_accents_and_case() {
        assert_eq!(base_fold("Café"), base_fold("cafe"));
        assert_eq!(base_fold("ÉCOLE"), "ecole");
        assert_eq!(base_fold("Straße"), "strasse");
        assert_eq!(base_fold("ﬁne"), "fine");
    }
}
