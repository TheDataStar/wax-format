//! SPEC §5.7 — the embedded search segment, defined by its tests.
//!
//! Written before the implementation (Directive 07 §4). The properties that
//! matter most here are the two failure modes, not the happy path:
//!
//!   * a pack with no index must open and report search *absent*, never error —
//!     that is what keeps the segment additive, and
//!   * a pack indexed with a tokenizer the reader does not implement must be
//!     refused outright, because the alternative is returning confidently wrong
//!     results for text that is present.

use std::collections::BTreeMap;
use wax_core::model::Compression;
use wax_core::writer::{EntryMeta, WaxWriter};
use wax_core::WaxReader;

const UUID: [u8; 16] = [
    0x5e, 0xa5, 0x00, 0x00, 0x40, 0x00, 0x80, 0x00, 0, 0, 0, 0, 0, 0, 0, 2,
];

fn manifest() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("name".into(), "Search".into());
    m
}

struct Doc(&'static str, &'static str, &'static str); // path, title, body

const DOCS: &[Doc] = &[
    Doc("a/hydrogen.html", "Hydrogen", "Hydrogen is the lightest element and the most abundant chemical substance in the universe."),
    Doc("a/helium.html", "Helium", "Helium is a colourless, odourless, inert noble gas and the second lightest element."),
    Doc("a/lithium.html", "Lithium", "Lithium is a soft silvery alkali metal, the lightest metal and lightest solid element."),
    Doc("a/carbon.html", "Carbon", "Carbon is a nonmetal that forms more compounds than any other element."),
    Doc("a/helium_three.html", "Helium-3", "Helium-3 is a light, stable isotope of helium with two protons and one neutron."),
];

fn build_indexed(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let out = dir.join(name);
    let w = WaxWriter::new(UUID).created_at(1_700_000_000);
    let mut sw = w.create(&out, &manifest()).unwrap();
    for Doc(p, t, b) in DOCS {
        let html = format!("<html><head><title>{t}</title></head><body><p>{b}</p></body></html>");
        sw.add_entry(
            EntryMeta::new(*p).mime("text/html").title(*t).compression(Compression::Zstd),
            &mut html.as_bytes(),
        )
        .unwrap();
        sw.index_text(p, Some(t), b).unwrap();
    }
    sw.finish().unwrap();
    out
}

fn build_plain(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let out = dir.join(name);
    let w = WaxWriter::new(UUID).created_at(1_700_000_000);
    let mut sw = w.create(&out, &manifest()).unwrap();
    for Doc(p, t, b) in DOCS {
        sw.add_entry(
            EntryMeta::new(*p).mime("text/html").title(*t).compression(Compression::Zstd),
            &mut b.as_bytes(),
        )
        .unwrap();
    }
    sw.finish().unwrap();
    out
}

#[test]
fn an_indexed_pack_answers_a_full_text_query() {
    let dir = tempfile::tempdir().unwrap();
    let pack = build_indexed(dir.path(), "s.wax");
    let r = WaxReader::open(&pack).unwrap();

    let hits = r.search("lightest", 10).unwrap();
    let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
    assert!(paths.contains(&"a/hydrogen.html"), "got {paths:?}");
    assert!(paths.contains(&"a/lithium.html"), "got {paths:?}");
    assert!(!paths.contains(&"a/carbon.html"), "carbon does not say lightest: {paths:?}");

    // A term in no document returns nothing rather than erroring.
    assert!(r.search("tungsten", 10).unwrap().is_empty());
}

#[test]
fn search_respects_its_limit_and_returns_openable_paths() {
    let dir = tempfile::tempdir().unwrap();
    let pack = build_indexed(dir.path(), "s.wax");
    let r = WaxReader::open(&pack).unwrap();

    let hits = r.search("element", 2).unwrap();
    assert!(hits.len() <= 2, "limit must be honoured, got {}", hits.len());
    for h in &hits {
        // A hit is only useful if the pack can actually serve it.
        r.read(&h.path).unwrap_or_else(|e| panic!("hit {} must be readable: {e}", h.path));
    }
}

#[test]
fn title_suggest_does_prefix_completion() {
    let dir = tempfile::tempdir().unwrap();
    let pack = build_indexed(dir.path(), "s.wax");
    let r = WaxReader::open(&pack).unwrap();

    let s = r.suggest("Hel", 10).unwrap();
    let titles: Vec<&str> = s.iter().map(|x| x.title.as_str()).collect();
    assert!(titles.contains(&"Helium"), "got {titles:?}");
    assert!(titles.contains(&"Helium-3"), "got {titles:?}");
    assert!(!titles.contains(&"Carbon"), "got {titles:?}");

    // What a search bar actually does: case-insensitive as you type.
    let lower = r.suggest("hel", 10).unwrap();
    assert_eq!(lower.len(), s.len(), "suggestion must not depend on case");

    assert!(r.suggest("Zz", 10).unwrap().is_empty());
}

#[test]
fn a_pack_without_an_index_reports_search_absent_not_an_error() {
    // The whole reason the segment is additive. A reader must serve entries
    // from a pack that has no index and say search is unavailable.
    let dir = tempfile::tempdir().unwrap();
    let pack = build_plain(dir.path(), "p.wax");
    let r = WaxReader::open(&pack).unwrap();

    assert!(!r.has_search_index(), "this pack has no index");
    assert!(r.search_tokenizer().is_none());
    assert!(r.search("lightest", 10).is_err(), "querying an absent index is an error…");
    // …but the pack itself is entirely usable.
    assert_eq!(r.read("a/carbon.html").unwrap(), DOCS[3].2.as_bytes());
}

#[test]
fn the_real_legacy_v0_9_packs_still_open_and_report_search_absent() {
    for name in ["legacy-tier-pi_zero_2w.wax", "legacy-tier-pi_5.wax"] {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../wax-builder/tests/fixtures")
            .join(name);
        let r = WaxReader::open(&p).unwrap_or_else(|e| panic!("{name} must open: {e}"));
        assert!(!r.has_search_index(), "{name} predates search entirely");
        let ep = r.manifest().get("entry_point").cloned().unwrap();
        assert!(!r.read(&ep).unwrap().is_empty(), "{name} still serves content");
    }
}

#[test]
fn an_unrecognised_tokenizer_is_refused_rather_than_answered_wrongly() {
    // Querying an index built with a tokenizer this reader cannot reproduce
    // would return confidently wrong results for text that IS present. A
    // refusal is the only safe answer.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("weird.wax");
    let w = WaxWriter::new(UUID).created_at(1_700_000_000);
    let mut sw = w.create(&out, &manifest()).unwrap();
    sw.set_search_tokenizer("martian-v3").unwrap();
    for Doc(p, t, b) in DOCS {
        sw.add_entry(
            EntryMeta::new(*p).mime("text/html").title(*t).compression(Compression::Zstd),
            &mut b.as_bytes(),
        )
        .unwrap();
        sw.index_text(p, Some(t), b).unwrap();
    }
    sw.finish().unwrap();

    let r = WaxReader::open(&out).unwrap();
    assert!(r.has_search_index());
    assert_eq!(r.search_tokenizer(), Some("martian-v3"));
    let err = r.search("lightest", 10).unwrap_err().to_string();
    assert!(err.contains("martian-v3"), "the error must name the tokenizer: {err}");
    // The pack is still perfectly readable; only search is refused.
    assert_eq!(r.read("a/carbon.html").unwrap(), DOCS[3].2.as_bytes());
}

#[test]
fn the_index_builds_byte_identically() {
    // Determinism covers the index too, or the anchor stops meaning anything.
    let dir = tempfile::tempdir().unwrap();
    let a = build_indexed(dir.path(), "a.wax");
    let b = build_indexed(dir.path(), "b.wax");
    assert_eq!(
        std::fs::read(&a).unwrap(),
        std::fs::read(&b).unwrap(),
        "two builds of one indexed corpus must be byte-identical"
    );
}

#[test]
fn querying_does_not_load_the_index_into_memory() {
    // The read-path discipline that keeps opens at ~1 MB has to survive search:
    // a query is a point lookup against the segment in place, not a load.
    let dir = tempfile::tempdir().unwrap();
    let pack = build_indexed(dir.path(), "s.wax");
    let r = WaxReader::open(&pack).unwrap();
    let before = r.cache_stats();
    let hits = r.search("element", 10).unwrap();
    assert!(!hits.is_empty());
    // Nothing about searching should disturb the entry lookup cache.
    assert_eq!(r.cache_stats(), before, "search must not churn the entry cache");
}
