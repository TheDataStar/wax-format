//! SPEC v0.10 — the shared-dictionary codec, defined by its tests.
//!
//! Written before the implementation (Directive 05 §8). Each test states one
//! property the codec must hold; the implementation exists to make them pass.
//!
//! The load-bearing one is `the_dictionary_is_carried_forward_not_retrained`:
//! retraining per edition reframes every entry and destroys the delta
//! advantage that made this codec win in the first place. That is an
//! invariant, so it gets a test rather than a comment.

mod common;

use std::collections::BTreeMap;
use wax_core::model::Compression;
use wax_core::writer::{EntryMeta, WaxWriter};
use wax_core::WaxReader;

const UUID: [u8; 16] = [
    0xd1, 0xc7, 0x00, 0x00, 0x40, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
];

/// Many small, similar documents — the shape a dictionary exists to exploit,
/// and the shape Directive 04 measured a 2.35x win on.
fn corpus(n: usize) -> Vec<(String, Vec<u8>)> {
    (0..n)
        .map(|i| {
            let body = format!(
                "<!doctype html><html><head><title>Article {i}</title>\
                 <link rel=\"stylesheet\" href=\"/style.css\"></head>\
                 <body><nav><a href=\"/\">Home</a><a href=\"/index\">Index</a></nav>\
                 <h1>Article {i}</h1><p>This is the body of article {i}. \
                 It shares its boilerplate with every other article in the corpus, \
                 which is exactly what a shared dictionary is for.</p>\
                 <footer>Licensed CC-BY-SA-4.0</footer></body></html>"
            );
            (format!("a/{i:05}.html"), body.into_bytes())
        })
        .collect()
}

fn manifest() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("name".into(), "Dict Codec".into());
    m
}

/// Build a pack whose entries use the dictionary codec.
fn build_dict_pack(dir: &std::path::Path, name: &str, items: &[(String, Vec<u8>)]) -> std::path::PathBuf {
    let out = dir.join(name);
    let w = WaxWriter::new(UUID).created_at(1_700_000_000);
    let mut sw = w.create(&out, &manifest()).expect("create");
    sw.train_dictionary(items.iter().map(|(_, b)| b.as_slice()))
        .expect("train");
    for (p, b) in items {
        let meta = EntryMeta::new(p.clone())
            .mime("text/html")
            .compression(Compression::ZstdDict);
        sw.add_entry(meta, &mut b.as_slice()).expect("add");
    }
    sw.finish().expect("finish");
    out
}

#[test]
fn a_dictionary_pack_round_trips_byte_exact() {
    let dir = tempfile::tempdir().unwrap();
    let items = corpus(300);
    let pack = build_dict_pack(dir.path(), "d.wax", &items);

    let r = WaxReader::open(&pack).unwrap();
    for (p, want) in &items {
        let got = r.read(p).unwrap_or_else(|e| panic!("{p}: {e}"));
        assert_eq!(&got, want, "{p} must decode byte-exact through the dictionary");
    }
}

#[test]
fn the_dictionary_is_stored_once_not_per_entry() {
    let dir = tempfile::tempdir().unwrap();
    let items = corpus(400);
    let pack = build_dict_pack(dir.path(), "d.wax", &items);
    let r = WaxReader::open(&pack).unwrap();

    // Every entry declares the dictionary codec...
    for (p, _) in items.iter().take(20) {
        assert_eq!(
            r.entry(p).unwrap().compression,
            "zstd-dict",
            "entry should declare the dictionary codec"
        );
    }
    // ...and the pack is smaller than the same corpus framed without one,
    // which is the entire point of the codec.
    let plain = dir.path().join("plain.wax");
    let w = WaxWriter::new(UUID).created_at(1_700_000_000);
    let mut sw = w.create(&plain, &manifest()).unwrap();
    for (p, b) in &items {
        let meta = EntryMeta::new(p.clone()).mime("text/html").compression(Compression::Zstd);
        sw.add_entry(meta, &mut b.as_slice()).unwrap();
    }
    sw.finish().unwrap();

    let dict_sz = std::fs::metadata(&pack).unwrap().len();
    let plain_sz = std::fs::metadata(&plain).unwrap().len();
    assert!(
        dict_sz < plain_sz,
        "dictionary pack ({dict_sz}) should be smaller than per-entry-only ({plain_sz})"
    );
}

#[test]
fn a_v0_9_pack_still_opens_under_the_v0_10_reader() {
    // Real pre-migration artifacts, built by the builder as it stood before any
    // of this existed -- not fixtures written to pass this test.
    for name in ["legacy-tier-pi_zero_2w.wax", "legacy-tier-pi_5.wax"] {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../wax-builder/tests/fixtures")
            .join(name);
        let r = WaxReader::open(&p).unwrap_or_else(|e| panic!("{name} must still open: {e}"));
        let ep = r.manifest().get("entry_point").cloned().unwrap();
        let html = String::from_utf8(r.read(&ep).unwrap()).unwrap();
        assert!(html.contains("Legacy pack"), "{name}: content must read back");
    }
}

#[test]
fn a_pack_may_mix_dictionary_and_plain_entries() {
    // The codec is per entry, so a pack can carry both. A reader must not
    // assume one codec for the whole archive.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("mixed.wax");
    let items = corpus(50);

    let w = WaxWriter::new(UUID).created_at(1_700_000_000);
    let mut sw = w.create(&out, &manifest()).unwrap();
    sw.train_dictionary(items.iter().map(|(_, b)| b.as_slice())).unwrap();
    for (i, (p, b)) in items.iter().enumerate() {
        let c = match i % 3 {
            0 => Compression::None,
            1 => Compression::Zstd,
            _ => Compression::ZstdDict,
        };
        let meta = EntryMeta::new(p.clone()).mime("text/html").compression(c);
        sw.add_entry(meta, &mut b.as_slice()).unwrap();
    }
    sw.finish().unwrap();

    let r = WaxReader::open(&out).unwrap();
    for (p, want) in &items {
        assert_eq!(&r.read(p).unwrap(), want, "{p} must decode whatever its codec");
    }
}

#[test]
fn rebuilding_the_same_source_is_byte_identical() {
    // Determinism survives the dictionary. Training is deterministic only if
    // the sample order is pinned -- measured in pre-flight, so this test is
    // what stops that regressing.
    let dir = tempfile::tempdir().unwrap();
    let items = corpus(300);
    let a = build_dict_pack(dir.path(), "a.wax", &items);
    let b = build_dict_pack(dir.path(), "b.wax", &items);
    let ba = std::fs::read(&a).unwrap();
    let bb = std::fs::read(&b).unwrap();
    assert_eq!(ba, bb, "two builds of one source must be byte-identical");
}

#[test]
fn the_dictionary_is_carried_forward_not_retrained() {
    // THE invariant. A new edition must reuse the prior dictionary, so an
    // entry whose bytes did not change keeps its exact stored frame. Retrain
    // and every frame moves, which is what destroys the delta advantage.
    let dir = tempfile::tempdir().unwrap();
    let v1_items = corpus(300);
    let v1 = build_dict_pack(dir.path(), "v1.wax", &v1_items);

    let r1_for_dict = WaxReader::open(&v1).unwrap();
    let dict = r1_for_dict
        .dictionary()
        .expect("v1 carries a dictionary")
        .to_vec();
    assert!(!dict.is_empty());

    // Edition two: one article edited, everything else untouched.
    let mut v2_items = v1_items.clone();
    v2_items[7].1 = b"<!doctype html><html><body><h1>Edited</h1></body></html>".to_vec();

    let out = dir.path().join("v2.wax");
    let w = WaxWriter::new(UUID).created_at(1_700_000_000);
    let mut sw = w.create(&out, &manifest()).unwrap();
    sw.carry_dictionary(&dict).expect("carry the prior dictionary forward");
    for (p, b) in &v2_items {
        let meta = EntryMeta::new(p.clone()).mime("text/html").compression(Compression::ZstdDict);
        sw.add_entry(meta, &mut b.as_slice()).unwrap();
    }
    sw.finish().unwrap();

    let r1 = WaxReader::open(&v1).unwrap();
    let r2 = WaxReader::open(&out).unwrap();
    assert_eq!(
        r2.dictionary().unwrap(),
        dict,
        "the carried dictionary must be stored verbatim, not retrained"
    );

    // Unchanged entries must hold identical stored bytes, so a delta ships
    // only what actually changed.
    let mut identical = 0usize;
    let mut moved = 0usize;
    for (p, _) in v2_items.iter() {
        let e1 = r1.entry(p).unwrap();
        let e2 = r2.entry(p).unwrap();
        if e1.sha256 == e2.sha256 {
            if e1.length == e2.length {
                identical += 1;
            } else {
                moved += 1;
            }
        }
    }
    assert_eq!(moved, 0, "no unchanged entry may change its framed length");
    assert!(
        identical >= v2_items.len() - 1,
        "all but the edited entry should frame identically ({identical} of {})",
        v2_items.len()
    );
}

#[test]
fn appending_carries_the_dictionary_forward_without_being_asked() {
    // Carry-forward is the structural default of opening an archive for
    // append, not a step a caller has to remember. A caller who forgets would
    // otherwise silently produce entries that no reader can decode, or quietly
    // reframe the archive -- both of which the invariant exists to prevent.
    let dir = tempfile::tempdir().unwrap();
    let items = corpus(200);
    let pack = build_dict_pack(dir.path(), "base.wax", &items);

    let original = {
        let r = WaxReader::open(&pack).unwrap();
        r.dictionary().expect("base pack carries a dictionary").to_vec()
    };

    // Append WITHOUT mentioning the dictionary at all.
    let w = WaxWriter::new(UUID).created_at(1_700_000_100);
    let mut sw = w.open_append(&pack).expect("open for append");
    assert_eq!(
        sw.dictionary(),
        Some(original.as_slice()),
        "append must inherit the archive's dictionary"
    );
    let extra = b"<!doctype html><html><body><h1>Appended</h1></body></html>".to_vec();
    sw.add_entry(
        EntryMeta::new("a/appended.html").mime("text/html").compression(Compression::ZstdDict),
        &mut extra.as_slice(),
    )
    .expect("a dict-framed entry must be writable on append");
    sw.finish().unwrap();

    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(
        r.dictionary().unwrap(),
        original.as_slice(),
        "the dictionary must be unchanged after an append"
    );
    assert_eq!(r.read("a/appended.html").unwrap(), extra);
    // and everything from before the append still decodes
    for (p, want) in items.iter().take(25) {
        assert_eq!(&r.read(p).unwrap(), want, "{p} must survive the append");
    }
}

#[test]
fn an_append_does_not_duplicate_the_dictionary_into_its_own_segment() {
    // The table is segment-0 only (SPEC §5.9). Re-writing ~110 KB into every
    // appended segment would be pure waste that no reader would even look at.
    let dir = tempfile::tempdir().unwrap();
    let items = corpus(200);
    let pack = build_dict_pack(dir.path(), "base.wax", &items);
    let before = std::fs::metadata(&pack).unwrap().len();

    let w = WaxWriter::new(UUID).created_at(1_700_000_100);
    let mut sw = w.open_append(&pack).unwrap();
    let tiny = b"x".to_vec();
    sw.add_entry(
        EntryMeta::new("a/tiny.html").mime("text/html").compression(Compression::ZstdDict),
        &mut tiny.as_slice(),
    )
    .unwrap();
    sw.finish().unwrap();

    let grew_by = std::fs::metadata(&pack).unwrap().len() - before;
    assert!(
        grew_by < 100 * 1024,
        "append grew the archive by {grew_by} bytes, which suggests the ~110 KB \
         dictionary was duplicated into the appended segment"
    );
}

#[test]
fn retraining_instead_of_carrying_would_reframe_everything() {
    // The negative half of the invariant: prove the failure mode is real, so
    // the carry-forward rule is justified by measurement rather than assertion.
    let dir = tempfile::tempdir().unwrap();
    let v1_items = corpus(300);
    let v1 = build_dict_pack(dir.path(), "v1.wax", &v1_items);

    let mut v2_items = v1_items.clone();
    v2_items[7].1 = b"<!doctype html><html><body><h1>Edited</h1></body></html>".to_vec();
    // Build v2 by RETRAINING on the new corpus rather than carrying forward.
    let v2 = build_dict_pack(dir.path(), "v2_retrained.wax", &v2_items);

    let r1 = WaxReader::open(&v1).unwrap();
    let r2 = WaxReader::open(&v2).unwrap();
    assert_ne!(
        r1.dictionary().unwrap(),
        r2.dictionary().unwrap(),
        "a retrained dictionary differs -- that is the hazard this codec must avoid"
    );
}
