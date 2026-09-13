//! ZIM `Language` (ISO 639-3, possibly comma-separated) → BCP-47 (Contract §11).
//!
//! BCP-47 uses the ISO 639-1 two-letter code where one exists and the 639-3
//! three-letter code otherwise, so the mapping is: if the source is a
//! three-letter code with a two-letter equivalent, use the two-letter form;
//! otherwise pass the code through. Every result is then validated against the
//! IANA registry via `language-tags` — a code with no valid mapping is
//! **omitted** rather than guessed (§11).

/// ISO 639-3 (= 639-2/T) three-letter → ISO 639-1 two-letter, for every
/// language that has both. Source: ISO 639-2 code list, terminology column.
const ISO_639_3_TO_1: &[(&str, &str)] = &[
    ("aar", "aa"), ("abk", "ab"), ("afr", "af"), ("aka", "ak"), ("amh", "am"), ("ara", "ar"),
    ("arg", "an"), ("asm", "as"), ("ava", "av"), ("ave", "ae"), ("aym", "ay"), ("aze", "az"),
    ("bak", "ba"), ("bam", "bm"), ("bel", "be"), ("ben", "bn"), ("bis", "bi"), ("bod", "bo"),
    ("bos", "bs"), ("bre", "br"), ("bul", "bg"), ("cat", "ca"), ("ces", "cs"), ("cha", "ch"),
    ("che", "ce"), ("chu", "cu"), ("chv", "cv"), ("cor", "kw"), ("cos", "co"), ("cre", "cr"),
    ("cym", "cy"), ("dan", "da"), ("deu", "de"), ("div", "dv"), ("dzo", "dz"), ("ell", "el"),
    ("eng", "en"), ("epo", "eo"), ("est", "et"), ("eus", "eu"), ("ewe", "ee"), ("fao", "fo"),
    ("fas", "fa"), ("fij", "fj"), ("fin", "fi"), ("fra", "fr"), ("fry", "fy"), ("ful", "ff"),
    ("gla", "gd"), ("gle", "ga"), ("glg", "gl"), ("glv", "gv"), ("grn", "gn"), ("guj", "gu"),
    ("hat", "ht"), ("hau", "ha"), ("heb", "he"), ("her", "hz"), ("hin", "hi"), ("hmo", "ho"),
    ("hrv", "hr"), ("hun", "hu"), ("hye", "hy"), ("ibo", "ig"), ("ido", "io"), ("iii", "ii"),
    ("iku", "iu"), ("ile", "ie"), ("ina", "ia"), ("ind", "id"), ("ipk", "ik"), ("isl", "is"),
    ("ita", "it"), ("jav", "jv"), ("jpn", "ja"), ("kal", "kl"), ("kan", "kn"), ("kas", "ks"),
    ("kat", "ka"), ("kau", "kr"), ("kaz", "kk"), ("khm", "km"), ("kik", "ki"), ("kin", "rw"),
    ("kir", "ky"), ("kom", "kv"), ("kon", "kg"), ("kor", "ko"), ("kua", "kj"), ("kur", "ku"),
    ("lao", "lo"), ("lat", "la"), ("lav", "lv"), ("lim", "li"), ("lin", "ln"), ("lit", "lt"),
    ("ltz", "lb"), ("lub", "lu"), ("lug", "lg"), ("mah", "mh"), ("mal", "ml"), ("mar", "mr"),
    ("mkd", "mk"), ("mlg", "mg"), ("mlt", "mt"), ("mon", "mn"), ("mri", "mi"), ("msa", "ms"),
    ("mya", "my"), ("nau", "na"), ("nav", "nv"), ("nbl", "nr"), ("nde", "nd"), ("ndo", "ng"),
    ("nep", "ne"), ("nld", "nl"), ("nno", "nn"), ("nob", "nb"), ("nor", "no"), ("nya", "ny"),
    ("oci", "oc"), ("oji", "oj"), ("ori", "or"), ("orm", "om"), ("oss", "os"), ("pan", "pa"),
    ("pli", "pi"), ("pol", "pl"), ("por", "pt"), ("pus", "ps"), ("que", "qu"), ("roh", "rm"),
    ("ron", "ro"), ("run", "rn"), ("rus", "ru"), ("sag", "sg"), ("san", "sa"), ("sin", "si"),
    ("slk", "sk"), ("slv", "sl"), ("sme", "se"), ("smo", "sm"), ("sna", "sn"), ("snd", "sd"),
    ("som", "so"), ("sot", "st"), ("spa", "es"), ("sqi", "sq"), ("srd", "sc"), ("srp", "sr"),
    ("ssw", "ss"), ("sun", "su"), ("swa", "sw"), ("swe", "sv"), ("tah", "ty"), ("tam", "ta"),
    ("tat", "tt"), ("tel", "te"), ("tgk", "tg"), ("tgl", "tl"), ("tha", "th"), ("tir", "ti"),
    ("ton", "to"), ("tsn", "tn"), ("tso", "ts"), ("tuk", "tk"), ("tur", "tr"), ("twi", "tw"),
    ("uig", "ug"), ("ukr", "uk"), ("urd", "ur"), ("uzb", "uz"), ("ven", "ve"), ("vie", "vi"),
    ("vol", "vo"), ("wln", "wa"), ("wol", "wo"), ("xho", "xh"), ("yid", "yi"), ("yor", "yo"),
    ("zha", "za"), ("zho", "zh"), ("zul", "zu"),
    // ISO 639-2/B bibliographic codes that some scrapers still write:
    ("alb", "sq"), ("arm", "hy"), ("baq", "eu"), ("bur", "my"), ("chi", "zh"), ("cze", "cs"),
    ("dut", "nl"), ("fre", "fr"), ("geo", "ka"), ("ger", "de"), ("gre", "el"), ("ice", "is"),
    ("mac", "mk"), ("mao", "mi"), ("may", "ms"), ("per", "fa"), ("rum", "ro"), ("slo", "sk"),
    ("tib", "bo"), ("wel", "cy"),
];

/// Map one ZIM language code to a valid BCP-47 tag, or `None` if it cannot be.
pub fn to_bcp47(code: &str) -> Option<String> {
    let code = code.trim();
    if code.is_empty() {
        return None;
    }
    let lower = code.to_ascii_lowercase();
    let candidate = if lower.len() == 3 && lower.bytes().all(|b| b.is_ascii_alphabetic()) {
        ISO_639_3_TO_1
            .iter()
            .find(|(three, _)| *three == lower)
            .map(|(_, two)| two.to_string())
            .unwrap_or(lower)
    } else {
        // already a 2-letter code or a longer tag (e.g. "pt-BR"): pass through
        code.to_string()
    };
    let tag = language_tags::LanguageTag::parse(&candidate).ok()?;
    tag.validate().ok()?;
    Some(tag.to_string())
}

/// Map a ZIM `Language` value (`eng`, `eng,fra`, `en`) to the manifest's
/// comma-separated BCP-47 list. Unmappable elements are dropped; returns
/// `None` when nothing survives (the field is then omitted, §11).
pub fn languages_field(zim_language: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for part in zim_language.split(',') {
        if let Some(tag) = to_bcp47(part) {
            if !out.contains(&tag) {
                out.push(tag);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_letter_codes_map_to_two_letter() {
        assert_eq!(to_bcp47("eng").as_deref(), Some("en"));
        assert_eq!(to_bcp47("fra").as_deref(), Some("fr"));
        assert_eq!(to_bcp47("swa").as_deref(), Some("sw"));
        assert_eq!(to_bcp47("spa").as_deref(), Some("es"));
        assert_eq!(to_bcp47("ENG").as_deref(), Some("en"));
        // bibliographic variant
        assert_eq!(to_bcp47("fre").as_deref(), Some("fr"));
    }

    #[test]
    fn three_letter_codes_without_two_letter_form_pass_through() {
        // Hawaiian: no 639-1 code; "haw" is itself the BCP-47 primary subtag
        assert_eq!(to_bcp47("haw").as_deref(), Some("haw"));
    }

    #[test]
    fn two_letter_and_full_tags_pass_through() {
        assert_eq!(to_bcp47("en").as_deref(), Some("en"));
        assert_eq!(to_bcp47("pt-BR").as_deref(), Some("pt-BR"));
        assert_eq!(to_bcp47("zh-Hans").as_deref(), Some("zh-Hans"));
    }

    #[test]
    fn unmappable_is_omitted() {
        assert_eq!(to_bcp47("zzz"), None);
        assert_eq!(to_bcp47("english"), None);
        assert_eq!(to_bcp47(""), None);
        assert_eq!(to_bcp47("N/A"), None);
    }

    #[test]
    fn multi_language_zims() {
        assert_eq!(languages_field("eng,fra").as_deref(), Some("en,fr"));
        assert_eq!(languages_field("eng, zzz ,fra").as_deref(), Some("en,fr"));
        assert_eq!(languages_field("eng,en").as_deref(), Some("en"), "deduplicated");
        assert_eq!(languages_field("zzz"), None);
    }
}
