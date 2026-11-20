//! The `wax-bigram-16` search tokenizer (SPEC v0.13 §5.7.2).
//!
//! SQLite's built-in tokenizers cannot search Chinese, Japanese or Korean
//! under SPEC §5.7's `detail=none` index: `unicode61` keeps a whole run of
//! CJK characters as one token, so a word inside a sentence never matches,
//! and `trigram` refuses any query term longer than three characters and can
//! never match a two-character word, which is most Chinese words.
//!
//! `wax-bigram-16` fixes that without a custom SQLite tokenizer, so every
//! box can reproduce it with what is already built in. The text is rewritten
//! *before* it reaches SQLite, by the same function on the build side and on
//! the query side, and the FTS5 table itself stays plain `unicode61`:
//!
//! 1. fold the whole text with `nfkc-cf-16` (SPEC §5.7.1), which also fixes
//!    `ß`/`ss`, ligatures and full-width forms for every script;
//! 2. replace every run of CJK characters (the code points [`is_cjk`] names)
//!    with its overlapping two-character pieces, plus the run's last
//!    character on its own, each separated by spaces.
//!
//! Everything else is left for `unicode61` to split as it always has, so a
//! pack in any other language indexes exactly as it would under
//! `unicode61`, plus the fold.
//!
//! A query word of two or more CJK characters is asked as all of its pieces;
//! a single character is asked as a prefix, which finds it both inside a run
//! (as the first half of a piece) and at a run's end (the lone character).

use crate::fold::{fold_title, TITLE_FOLD_NFKC_CF};

/// The declared name (SPEC v0.13 §5.7.2). The `16` pins the fold's Unicode
/// version, the same way `nfkc-cf-16` does.
pub const WAX_BIGRAM: &str = "wax-bigram-16";

/// The FTS5 table under `wax-bigram-16`: `unicode61` with its default
/// diacritic removal, and with combining marks (`M*`) counted as part of a
/// word. SQLite's default counts only letters, digits and private-use
/// characters (`L* N* Co`), which cuts every Indic word at its vowel signs
/// and viramas (`हिन्दी` becomes `ह`, `न`, `द`) and vocalised Arabic at its
/// harakat; measured, not assumed (SPEC v0.13 §5.7.2).
pub const WAX_BIGRAM_SQLITE: &str = "unicode61 categories 'L* N* Co M*'";

/// The SQLite tokenizer the FTS5 table is actually created with for a pack
/// declaring `tokenizer`: [`WAX_BIGRAM_SQLITE`] for `wax-bigram-16`,
/// otherwise the declared name itself.
pub fn sqlite_tokenizer(tokenizer: &str) -> &str {
    if tokenizer == WAX_BIGRAM {
        WAX_BIGRAM_SQLITE
    } else {
        tokenizer
    }
}

/// `tokenize=` argument for a `CREATE VIRTUAL TABLE … USING fts5(…)`, as an
/// SQL string literal. Only ever called with a known tokenizer.
pub fn sqlite_tokenize_arg(tokenizer: &str) -> String {
    format!("'{}'", sqlite_tokenizer(tokenizer).replace('\'', "''"))
}

/// CJK code points for `wax-bigram-16`, as inclusive ranges, ascending.
///
/// Han (CJK Unified Ideographs and every extension through planes 2–3,
/// compatibility ideographs), Hiragana, Katakana, Bopomofo, Hangul (jamo and
/// syllables), and the ideographic iteration and number marks. Left out on
/// purpose: the characters inside these blocks that `unicode61` treats as
/// separators (U+3099–U+309C combining and spacing sound marks, U+30A0,
/// U+30FB), because a piece must always be exactly one `unicode61` token;
/// and the Kangxi and supplementary radicals, which NFKC maps to ordinary
/// ideographs or which are symbols. Pinned here, not taken from a Unicode
/// script table, so every build agrees on the set.
pub const CJK_RANGES: &[(u32, u32)] = &[
    (0x1100, 0x11FF),   // Hangul Jamo
    (0x3005, 0x3007),   // 々 〆 〇
    (0x3021, 0x3029),   // Hangzhou numerals
    (0x3031, 0x3035),   // kana repeat marks
    (0x303B, 0x303C),   // 〻 〼
    (0x3041, 0x3098),   // Hiragana letters
    (0x309D, 0x309F),   // ゝ ゞ ゟ
    (0x30A1, 0x30FA),   // Katakana letters
    (0x30FC, 0x30FF),   // ー ヽ ヾ ヿ
    (0x3105, 0x312F),   // Bopomofo
    (0x3131, 0x318E),   // Hangul Compatibility Jamo
    (0x31A0, 0x31BF),   // Bopomofo Extended
    (0x31F0, 0x31FF),   // Katakana Phonetic Extensions
    (0x3400, 0x4DBF),   // CJK Extension A
    (0x4E00, 0x9FFF),   // CJK Unified Ideographs
    (0xA960, 0xA97F),   // Hangul Jamo Extended-A
    (0xAC00, 0xD7AF),   // Hangul Syllables
    (0xD7B0, 0xD7FF),   // Hangul Jamo Extended-B
    (0xF900, 0xFAFF),   // CJK Compatibility Ideographs
    (0x20000, 0x3FFFD), // planes 2 and 3: Extensions B onward
];

/// Whether `c` belongs to a CJK run under `wax-bigram-16`.
pub fn is_cjk(c: char) -> bool {
    let c = c as u32;
    // Binary search over the sorted, disjoint ranges.
    CJK_RANGES
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// The pieces of one CJK run, document side: every overlapping pair, then
/// the last character alone. A one-character run is just that character.
fn run_pieces(run: &[char], out: &mut String) {
    for w in run.windows(2) {
        out.push(' ');
        out.push(w[0]);
        out.push(w[1]);
    }
    out.push(' ');
    out.push(*run.last().expect("non-empty run"));
    out.push(' ');
}

/// Rewrite `text` for indexing under `wax-bigram-16` (SPEC v0.13 §5.7.2).
/// The result goes into the `unicode61` FTS5 table as is.
pub fn prepare_document(text: &str) -> String {
    let folded = fold_title(TITLE_FOLD_NFKC_CF, text).expect("nfkc-cf-16 is built in");
    let mut out = String::with_capacity(folded.len() + folded.len() / 2);
    let mut run: Vec<char> = Vec::new();
    for c in folded.chars() {
        if is_cjk(c) {
            run.push(c);
            continue;
        }
        if !run.is_empty() {
            run_pieces(&run, &mut out);
            run.clear();
        }
        out.push(c);
    }
    if !run.is_empty() {
        run_pieces(&run, &mut out);
    }
    out
}

/// One query term: the token text, and whether it is asked as a prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTerm {
    pub text: String,
    pub prefix: bool,
}

/// The terms of a query typed by a person, under `wax-bigram-16`.
///
/// Words are the runs `unicode61` would keep (letters, digits, and marks
/// attached to them), after the fold. A CJK run of two or more characters
/// becomes its overlapping pairs, all required; a single CJK character
/// becomes a prefix term, which matches both the pairs it starts and the
/// lone character a run ends with. `prefix_last` makes the last non-CJK
/// word a prefix too (title-style completion).
pub fn query_terms(q: &str, prefix_last: bool) -> Vec<QueryTerm> {
    use unicode_normalization::char::is_combining_mark;
    let folded = fold_title(TITLE_FOLD_NFKC_CF, q).expect("nfkc-cf-16 is built in");
    let mut words: Vec<(String, bool)> = Vec::new(); // (word, is_cjk)
    let mut cur = String::new();
    let mut cur_cjk = false;
    let flush = |cur: &mut String, cjk: bool, words: &mut Vec<(String, bool)>| {
        if !cur.is_empty() {
            words.push((std::mem::take(cur), cjk));
        }
    };
    for c in folded.chars() {
        let cjk = is_cjk(c);
        let word_char = cjk || c.is_alphanumeric() || (is_combining_mark(c) && !cur.is_empty());
        if !word_char {
            flush(&mut cur, cur_cjk, &mut words);
            continue;
        }
        if !cur.is_empty() && cjk != cur_cjk && !(is_combining_mark(c) && !cjk) {
            flush(&mut cur, cur_cjk, &mut words);
        }
        if cur.is_empty() {
            cur_cjk = cjk;
        }
        cur.push(c);
    }
    flush(&mut cur, cur_cjk, &mut words);

    let n = words.len();
    let mut out = Vec::new();
    for (i, (w, cjk)) in words.into_iter().enumerate() {
        if cjk {
            let cs: Vec<char> = w.chars().collect();
            if cs.len() == 1 {
                out.push(QueryTerm { text: w, prefix: true });
            } else {
                for p in cs.windows(2) {
                    out.push(QueryTerm {
                        text: p.iter().collect(),
                        prefix: false,
                    });
                }
            }
        } else {
            out.push(QueryTerm {
                text: w,
                prefix: prefix_last && i + 1 == n,
            });
        }
    }
    out
}

/// The FTS5 expression for a person's query under `wax-bigram-16`: every
/// term quoted (so no FTS5 syntax can be injected) and all required.
/// `None` when the query has no words.
pub fn match_expr(q: &str, prefix_last: bool) -> Option<String> {
    let terms = query_terms(q, prefix_last);
    if terms.is_empty() {
        return None;
    }
    Some(
        terms
            .iter()
            .map(|t| {
                let star = if t.prefix { "*" } else { "" };
                format!("\"{}\"{star}", t.text.replace('"', "\"\""))
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_sorted_and_disjoint() {
        for w in CJK_RANGES.windows(2) {
            assert!(w[0].0 <= w[0].1 && w[0].1 < w[1].0, "{w:x?}");
        }
    }

    #[test]
    fn cjk_runs_become_pairs_and_a_last_character() {
        assert_eq!(prepare_document("東京都庁"), " 東京 京都 都庁 庁 ");
        assert_eq!(prepare_document("水"), " 水 ");
        assert_eq!(prepare_document("iPhone手机2024年"), "iphone 手机 机 2024 年 ");
        assert_eq!(prepare_document("Straße ﬁlm"), "strasse film");
        // full-width Latin and half-width katakana fold before splitting
        assert_eq!(prepare_document("ＡＢＣ ｶﾀｶﾅ"), "abc  カタ タカ カナ ナ ");
    }

    #[test]
    fn queries() {
        assert_eq!(match_expr("東京都", false).unwrap(), "\"東京\" \"京都\"");
        assert_eq!(match_expr("水", false).unwrap(), "\"水\"*");
        assert_eq!(match_expr("Straße 東京", true).unwrap(), "\"strasse\" \"東京\"");
        assert_eq!(match_expr("kyoto prot", true).unwrap(), "\"kyoto\" \"prot\"*");
        assert_eq!(match_expr("\"OR\" NEAR(", false).unwrap(), "\"or\" \"near\"");
        assert!(match_expr(" ,. ", false).is_none());
    }
}

#[cfg(test)]
mod sqlite_agreement {
    use super::*;

    /// Every CJK code point must be exactly one token of the table, or a piece
    /// could split inside SQLite and the query side would disagree with the
    /// index.
    #[test]
    fn every_cjk_code_point_is_one_token() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(
            &format!(
                "CREATE VIRTUAL TABLE s USING fts5(b, content='', detail=none, tokenize={});
                 CREATE VIRTUAL TABLE temp.v USING fts5vocab(main, s, instance);",
                sqlite_tokenize_arg(WAX_BIGRAM)
            ),
        )
        .unwrap();
        let mut id = 0i64;
        let mut ins = c.prepare("INSERT INTO s(rowid, b) VALUES (?1, ?2)").unwrap();
        for &(lo, hi) in CJK_RANGES {
            // the planes-2/3 range is sampled; its unassigned code points are
            // token characters to unicode61, like the assigned ones
            let step = if hi - lo > 0x10000 { 97 } else { 1 };
            let mut cp = lo;
            while cp <= hi {
                if let Some(ch) = char::from_u32(cp) {
                    id += 1;
                    ins.execute(rusqlite::params![id, format!("x{ch}y")]).unwrap();
                }
                cp += step;
            }
        }
        drop(ins);
        let split: i64 = c
            .query_row(
                "SELECT count(*) FROM (SELECT doc FROM temp.v GROUP BY doc HAVING count(*) != 1)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(split, 0, "some CJK code point is a unicode61 separator");
    }
}

#[cfg(test)]
mod scripts {
    use super::*;

    /// Words in scripts with combining marks stay whole (SPEC v0.13 §5.7.2).
    #[test]
    fn indic_and_vocalised_arabic_words_stay_whole() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(&format!(
            "CREATE VIRTUAL TABLE s USING fts5(b, content='', detail=none, tokenize={});",
            sqlite_tokenize_arg(WAX_BIGRAM)
        ))
        .unwrap();
        let docs = ["हिन्दी भाषा है", "தமிழ் மொழி", "اَلْعَرَبِيَّةُ لُغَةٌ", "Ein café in der Straße", "東京都庁"];
        for (i, d) in docs.iter().enumerate() {
            c.execute("INSERT INTO s(rowid, b) VALUES (?1, ?2)", rusqlite::params![i as i64, prepare_document(d)])
                .unwrap();
        }
        let hit = |q: &str| -> Vec<i64> {
            let e = match_expr(q, false).unwrap();
            let mut st = c.prepare("SELECT rowid FROM s WHERE s MATCH ?1 ORDER BY rowid").unwrap();
            st.query_map([e], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(hit("हिन्दी"), [0]);
        assert_eq!(hit("मोटर"), Vec::<i64>::new());
        assert_eq!(hit("தமிழ்"), [1]);
        assert_eq!(hit("اَلْعَرَبِيَّةُ"), [2]);
        assert_eq!(hit("cafe strasse"), [3]);
        assert_eq!(hit("都庁"), [4]);
    }
}
