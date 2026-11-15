//! Title folding for prefix completion (SPEC v0.12 §5.7.1).
//!
//! `search_docs.title_fold` is compared against a fold of what the person
//! typed, so the index side and the query side MUST fold identically: a
//! mismatch is a silent miss on a title that is present, the same failure the
//! declared tokenizer exists to prevent. The fold is therefore *declared* in
//! `segment_meta.title_fold`, and a reader refuses completion under a fold it
//! cannot reproduce.

use caseless::default_case_fold_str;
use unicode_normalization::UnicodeNormalization;

/// The fold every v0.12 writer declares: `NFKC(full_case_fold(NFKC(s)))`,
/// case folding per Unicode 16.0 `CaseFolding.txt` (status C + F, no T), no
/// diacritic stripping, no locale tailoring.
pub const TITLE_FOLD_NFKC_CF: &str = "nfkc-cf-16";

/// What a v0.11 segment used before the fold was declared: Unicode default
/// lowercasing (`str::to_lowercase`). A segment with a search index and no
/// `title_fold` key is read as this.
pub const TITLE_FOLD_LEGACY_LOWER: &str = "lower";

/// Folds this build can reproduce on the query side.
pub const SUPPORTED_TITLE_FOLDS: &[&str] = &[TITLE_FOLD_NFKC_CF, TITLE_FOLD_LEGACY_LOWER];

/// Fold `s` under the named fold, or `None` if this build does not know it.
pub fn fold_title(fold: &str, s: &str) -> Option<String> {
    match fold {
        TITLE_FOLD_NFKC_CF => {
            let nfkc: String = s.nfkc().collect();
            Some(default_case_fold_str(&nfkc).nfkc().collect())
        }
        TITLE_FOLD_LEGACY_LOWER => Some(s.to_lowercase()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfkc_cf_pins_the_cases_simple_lowercase_gets_wrong() {
        let f = |s| fold_title(TITLE_FOLD_NFKC_CF, s).unwrap();
        assert_eq!(f("Straße"), "strasse"); // full folding, not lowercase
        assert_eq!(f("ﬁlm"), "film"); // compatibility ligature
        assert_eq!(f("\u{2126}"), "\u{3c9}"); // OHM SIGN -> omega
        assert_eq!(f("Café"), f("Cafe\u{301}")); // composed vs decomposed
        assert_eq!(f("Ångström"), "ångström"); // diacritics are kept
        assert_eq!(f("İstanbul"), "i\u{307}stanbul"); // no Turkish tailoring
        assert_eq!(f("ΣΊΣΥΦΟΣ"), f("σίσυφος"));
    }

    #[test]
    fn unknown_fold_is_refused() {
        assert!(fold_title("lower-tr", "x").is_none());
    }
}
