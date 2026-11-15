//! SPEC v0.12 — deletion, search that follows the chain, the declared title
//! fold, and verifiable append deltas. One test per normative sentence that
//! can be tested.

use std::collections::BTreeMap;
use wax_core::model::{Compression, TOMBSTONE};
use wax_core::reader::signable_digest_of;
use wax_core::writer::{EntryMeta, WaxWriter};
use wax_core::{WaxError, WaxReader};

const UUID: [u8; 16] = [0x12, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 7];

fn manifest() -> BTreeMap<String, String> {
    BTreeMap::from([("name".to_string(), "v0.12".to_string())])
}

fn add(sw: &mut wax_core::StreamingWriter, path: &str, title: &str, body: &str, index: bool) {
    sw.add_entry(
        EntryMeta::new(path).mime("text/html").title(title).compression(Compression::Zstd),
        &mut body.as_bytes(),
    )
    .unwrap();
    if index {
        sw.index_text(path, Some(title), body).unwrap();
    }
}

/// Edition 1: four indexed articles and one alias.
fn edition_one(dir: &std::path::Path) -> std::path::PathBuf {
    let out = dir.join("pack.wax");
    let mut sw = WaxWriter::new(UUID).created_at(1_700_000_000).create(&out, &manifest()).unwrap();
    add(&mut sw, "Hydrogen.html", "Hydrogen", "Hydrogen is the lightest element.", true);
    add(&mut sw, "Helium.html", "Helium", "Helium is the second lightest element.", true);
    add(&mut sw, "Carbon.html", "Carbon", "Carbon forms more compounds than any element.", true);
    add(&mut sw, "Strasse.html", "Straße", "A street, in German.", true);
    sw.add_redirect("C.html", "Carbon.html", None).unwrap();
    sw.finish().unwrap();
    out
}

/// Edition 2, as an append: Helium rewritten, Boron added, Carbon deleted.
fn edition_two(pack: &std::path::Path) {
    let mut sw = WaxWriter::new(UUID).created_at(1_700_100_000).open_append(pack).unwrap();
    add(&mut sw, "Helium.html", "Helium", "Helium fills party balloons.", true);
    add(&mut sw, "Boron.html", "Boron", "Boron is a metalloid.", true);
    sw.add_tombstone("Carbon.html").unwrap();
    sw.finish().unwrap();
}

fn paths(hits: &[wax_core::SearchHit]) -> Vec<&str> {
    hits.iter().map(|h| h.path.as_str()).collect()
}

// --- §5.5.1 tombstones ------------------------------------------------------

#[test]
fn a_tombstone_deletes_the_path_everywhere_a_reader_looks() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path());
    edition_two(&pack);
    let r = WaxReader::open(&pack).unwrap();

    assert!(matches!(r.read("Carbon.html"), Err(WaxError::EntryDeleted(p)) if p == "Carbon.html"));
    assert!(matches!(r.entry("Carbon.html"), Err(WaxError::EntryDeleted(_))));
    assert!(!r.contains("Carbon.html").unwrap());
    let listed: Vec<String> = r.paths().map(|p| p.unwrap()).collect();
    assert!(!listed.iter().any(|p| p == "Carbon.html"), "listing must drop it: {listed:?}");
    assert_eq!(r.entry_count().unwrap(), listed.len() as u64);
    // Deleting a path deletes its aliases: the writer tombstoned C.html too,
    // so it is gone from the listing as well as unreadable.
    assert!(matches!(r.read("C.html"), Err(WaxError::EntryDeleted(p)) if p == "C.html"));
    assert!(!listed.iter().any(|p| p == "C.html"), "the alias goes with it: {listed:?}");
    // Everything else is untouched.
    assert_eq!(r.read("Hydrogen.html").unwrap(), b"Hydrogen is the lightest element.");
}

#[test]
fn a_reader_that_predates_tombstones_refuses_rather_than_serves() {
    // A v0.11 reader sees the row as an unknown codec: a loud refusal, never
    // the deleted bytes and never empty content.
    let e = wax_core::model::Compression::parse("Carbon.html", TOMBSTONE).unwrap_err();
    assert!(matches!(e, WaxError::UnknownCompression { .. }));
}

#[test]
fn tombstones_are_refused_where_they_cannot_mean_anything() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("fresh.wax");
    let mut sw = WaxWriter::new(UUID).create(&fresh, &manifest()).unwrap();
    assert!(sw.add_tombstone("x.html").is_err(), "a fresh build has nothing to delete");

    let pack = edition_one(dir.path());
    let mut sw = WaxWriter::new(UUID).open_append(&pack).unwrap();
    assert!(matches!(sw.add_tombstone("Nope.html"), Err(WaxError::EntryNotFound(_))));

    // A redirect added in the same segment onto a path it deletes dangles.
    let mut sw = WaxWriter::new(UUID).open_append(&pack).unwrap();
    sw.add_tombstone("Carbon.html").unwrap();
    sw.add_redirect("Cee.html", "Carbon.html", None).unwrap();
    assert!(matches!(sw.finish(), Err(WaxError::DanglingRedirect { .. })));
}

// --- §5.7 search follows the chain ------------------------------------------

#[test]
fn search_sees_new_text_forgets_old_text_and_drops_deleted_articles() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path());
    assert!(paths(&WaxReader::open(&pack).unwrap().search("lightest", 10).unwrap())
        .contains(&"Helium.html"));
    edition_two(&pack);
    let r = WaxReader::open(&pack).unwrap();

    let lightest = r.search("lightest", 10).unwrap();
    assert_eq!(paths(&lightest), ["Hydrogen.html"], "Helium no longer says lightest");
    assert_eq!(paths(&r.search("balloons", 10).unwrap()), ["Helium.html"]);
    assert_eq!(paths(&r.search("metalloid", 10).unwrap()), ["Boron.html"]);
    assert!(r.search("compounds", 10).unwrap().is_empty(), "Carbon was deleted");
    // Every hit is servable.
    for h in r.search("element OR balloons OR metalloid", 10).unwrap() {
        r.read(&h.path).unwrap();
    }
}

#[test]
fn suggestions_follow_the_chain_too() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path());
    edition_two(&pack);
    let r = WaxReader::open(&pack).unwrap();
    let titles = |p: &str| -> Vec<String> {
        r.suggest(p, 10).unwrap().into_iter().map(|s| s.title).collect()
    };
    assert_eq!(titles("Bo"), ["Boron"]);
    assert_eq!(titles("He"), ["Helium"], "one Helium, from the newest segment");
    assert!(titles("Car").is_empty(), "Carbon was deleted");
}

#[test]
fn an_append_cannot_index_with_a_different_tokenizer() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path());
    let mut sw = WaxWriter::new(UUID).open_append(&pack).unwrap();
    assert!(matches!(
        sw.set_search_tokenizer("trigram"),
        Err(WaxError::TokenizerMismatch { .. })
    ));
}

#[test]
fn a_segment_indexes_only_its_own_entries() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path());
    let mut sw = WaxWriter::new(UUID).open_append(&pack).unwrap();
    // Hydrogen lives in segment 0; a row for it here would never be served.
    sw.index_text("Hydrogen.html", Some("Hydrogen"), "stale copy").unwrap();
    assert!(matches!(sw.finish(), Err(WaxError::Schema { .. })));
}

// --- §5.7.1 the declared title fold -----------------------------------------

#[test]
fn the_title_fold_is_full_case_folding_and_declared() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path());
    let r = WaxReader::open(&pack).unwrap();
    // Plain lowercasing would never match "strasse" to "Straße".
    let s: Vec<String> = r.suggest("STRASSE", 10).unwrap().into_iter().map(|s| s.title).collect();
    assert_eq!(s, ["Straße"]);
    let s: Vec<String> = r.suggest("straß", 10).unwrap().into_iter().map(|s| s.title).collect();
    assert_eq!(s, ["Straße"]);
}

// --- §7.4 verifiable append deltas ------------------------------------------

#[test]
fn each_append_records_the_state_it_extends() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path());
    let (before, _, _) = signable_digest_of(&pack).unwrap();
    edition_two(&pack);
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.prev_state_digest(0), None, "the base extends nothing");
    assert_eq!(r.prev_state_digest(1), Some(before));
}

#[test]
fn the_update_is_the_byte_tail_plus_a_header() {
    // What makes a box-side update cheap and checkable: the new edition is
    // the old file's bytes, a new header, and an appended tail.
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path());
    let old = std::fs::read(&pack).unwrap();
    edition_two(&pack);
    let new = std::fs::read(&pack).unwrap();
    assert_eq!(&new[128..old.len()], &old[128..], "nothing before the old EOF moved");

    // Apply it the way a box would: check the tail extends *this* copy, then
    // write the new header and append the tail.
    let copy = dir.path().join("box-copy.wax");
    std::fs::write(&copy, &old).unwrap();
    let (mine, _, _) = signable_digest_of(&copy).unwrap();
    assert_eq!(WaxReader::open(&pack).unwrap().prev_state_digest(1), Some(mine));
    let mut applied = new[..128].to_vec();
    applied.extend_from_slice(&old[128..]);
    applied.extend_from_slice(&new[old.len()..]);
    std::fs::write(&copy, &applied).unwrap();
    assert_eq!(std::fs::read(&copy).unwrap(), new);
    WaxReader::open(&copy).unwrap();
}

#[test]
fn an_orphaned_tail_is_reclaimed_so_the_update_still_travels_as_a_byte_tail() {
    // Review finding (2026-11-15): after a crashed append (SPEC §7.2) the
    // writer used to append *after* the orphaned bytes. The §8.1 digest does
    // not cover them, so a box holding a clean copy accepted the update and
    // placed every new blob at the wrong offset. The writer now reclaims the
    // orphan first, so both copies produce the same tail.
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path());
    let clean = std::fs::read(&pack).unwrap();
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&pack).unwrap();
        f.write_all(&[0xAB; 4096]).unwrap(); // the crashed append's leftovers
    }
    edition_two(&pack);
    let new = std::fs::read(&pack).unwrap();
    assert_eq!(&new[128..clean.len()], &clean[128..], "nothing before the logical end moved");
    assert_ne!(
        &new[clean.len()..clean.len() + 4096],
        &[0xAB; 4096][..],
        "the orphan must be reclaimed, not shipped inside the update"
    );

    let copy = dir.path().join("box-copy.wax");
    let mut applied = new[..128].to_vec();
    applied.extend_from_slice(&clean[128..]);
    applied.extend_from_slice(&new[clean.len()..]);
    std::fs::write(&copy, &applied).unwrap();
    assert_eq!(std::fs::read(&copy).unwrap(), new);
    let r = WaxReader::open(&copy).unwrap();
    assert_eq!(r.read("Boron.html").unwrap(), b"Boron is a metalloid.");
}

#[test]
fn an_append_past_the_segment_limit_is_refused_and_the_pack_stays_readable() {
    // Review finding (2026-11-15): the writer used to write segment 64, which
    // every reader then refuses to open (SPEC §5.1), turning a working pack
    // into an unreadable one.
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path());
    for i in 1..wax_core::MAX_SEGMENTS as u64 {
        let mut sw = WaxWriter::new(UUID).created_at(1_700_000_000 + i).open_append(&pack).unwrap();
        add(&mut sw, &format!("p{i}.html"), "P", "x", false);
        sw.finish().unwrap();
    }
    let before = std::fs::read(&pack).unwrap();
    let err = WaxWriter::new(UUID).created_at(1).open_append(&pack).err().expect("must refuse");
    assert!(matches!(err, WaxError::TooManySegments), "got {err}");
    assert_eq!(std::fs::read(&pack).unwrap(), before, "a refused append changes nothing");
    WaxReader::open(&pack).unwrap();
}

// --- §10 header minor on append ---------------------------------------------

#[test]
fn appending_to_an_older_pack_raises_the_header_minor_and_keeps_it_readable() {
    let dir = tempfile::tempdir().unwrap();
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../wax-builder/tests/fixtures/legacy-tier-pi_5.wax");
    let pack = dir.path().join("legacy.wax");
    std::fs::copy(&src, &pack).unwrap();
    let r = WaxReader::open(&pack).unwrap();
    let old_minor = r.header().version_minor;
    let ep = r.manifest().get("entry_point").cloned().unwrap();
    let ep_bytes = r.read(&ep).unwrap();
    drop(r);
    assert!(old_minor < 12);

    let mut sw = WaxWriter::new(UUID).open_append(&pack).unwrap();
    add(&mut sw, "new.html", "New", "fresh", false);
    sw.finish().unwrap();

    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.header().version_minor, 12);
    assert_eq!(r.read(&ep).unwrap(), ep_bytes);
    assert_eq!(r.read("new.html").unwrap(), b"fresh");
}
