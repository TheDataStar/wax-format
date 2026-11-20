//! SPEC v0.13 — crash-safe header (check + mirror), CJK-capable search
//! (`wax-bigram-16`), passages, and reclaiming space (`gc`). One test per
//! normative sentence that can be tested.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wax_core::compact::{compact, reclaim_tail, CompactOptions};
use wax_core::header::{check_of, Check};
use wax_core::model::Compression;
use wax_core::reader::signable_digest_of;
use wax_core::tokenize::{match_expr, WAX_BIGRAM};
use wax_core::writer::{rollback_last_update, EntryMeta, WaxWriter};
use wax_core::{HeaderSource, WaxError, WaxReader};

const UUID: [u8; 16] = [0x13, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 9];

fn manifest() -> BTreeMap<String, String> {
    BTreeMap::from([("name".to_string(), "v0.13".to_string())])
}

fn html(title: &str, sections: &[(&str, &str)]) -> String {
    let mut s = format!("<html><head><title>{title}</title></head><body><p>Lead of {title}.</p>");
    for (h, body) in sections {
        let id = h.replace(' ', "_");
        s.push_str(&format!("<h2 id=\"{id}\">{h}</h2><p>{body}</p>"));
    }
    s.push_str("</body></html>");
    s
}

fn add(sw: &mut wax_core::StreamingWriter, path: &str, title: &str, page: &str) {
    sw.add_entry(
        EntryMeta::new(path).mime("text/html").title(title).compression(Compression::Zstd),
        &mut page.as_bytes(),
    )
    .unwrap();
    sw.index_text(path, Some(title), &wax_core::text::html_to_text(page)).unwrap();
    sw.index_passages(path, page.as_bytes()).unwrap();
}

/// Edition 1: three articles (one Japanese, one Chinese), an alias, an image.
fn edition_one(dir: &Path, tokenizer: &str) -> PathBuf {
    let out = dir.join("pack.wax");
    let mut sw = WaxWriter::new(UUID).created_at(1_700_000_000).create(&out, &manifest()).unwrap();
    sw.set_search_tokenizer(tokenizer).unwrap();
    add(
        &mut sw,
        "Kyoto.html",
        "Kyoto",
        &html("Kyoto", &[("History", "Kyoto was the capital of Japan."), ("Climate", "Kyoto Protocol was signed here in 1997.")]),
    );
    add(
        &mut sw,
        "Tokyo.html",
        "東京",
        &html("東京", &[("歴史", "東京都庁は新宿にある。"), ("交通", "東京駅は丸の内にある。")]),
    );
    add(
        &mut sw,
        "Beijing.html",
        "北京",
        &html("北京", &[("历史", "北京大学是中国的大学。"), ("地理", "北京有很多山和水。")]),
    );
    add(&mut sw, "Strasse.html", "Straße", &html("Straße", &[("Name", "Die Straße ist lang.")]));
    sw.add_redirect("Kyoto_City.html", "Kyoto.html", None).unwrap();
    sw.add_entry(EntryMeta::new("img/a.png").mime("image/png").compression(Compression::None), &mut &[7u8; 300][..])
        .unwrap();
    sw.finish().unwrap();
    out
}

/// Edition 2: Kyoto rewritten, Osaka added, Beijing deleted.
fn edition_two(pack: &Path) {
    let mut sw = WaxWriter::new(UUID).created_at(1_700_100_000).open_append(pack).unwrap();
    add(&mut sw, "Kyoto.html", "Kyoto", &html("Kyoto", &[("Temples", "Kyoto has many temples.")]));
    add(&mut sw, "Osaka.html", "大阪", &html("大阪", &[("食べ物", "大阪はたこ焼きで有名。")]));
    sw.add_tombstone("Beijing.html").unwrap();
    sw.finish().unwrap();
}

fn search(r: &WaxReader, q: &str) -> Vec<String> {
    let e = match_expr(q, false).unwrap();
    r.search(&e, 20).unwrap().into_iter().map(|h| h.path).collect()
}

// --- §2.3 / §2.4 the header check and the mirror -----------------------------

#[test]
fn every_v0_13_write_ends_with_a_mirror_of_a_checked_header() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    for edition in 1..=2 {
        let b = std::fs::read(&pack).unwrap();
        let mut a = [0u8; 128];
        a.copy_from_slice(&b[..128]);
        assert_eq!(check_of(&a), Check::Valid);
        assert_eq!(&b[b.len() - 128..], &b[..128], "mirror == slot A");
        let r = WaxReader::open(&pack).unwrap();
        assert_eq!(r.header_source(), HeaderSource::SlotA);
        assert!(r.has_mirror().unwrap());
        assert_eq!(r.logical_end() + 128, r.file_size());
        drop(r);
        if edition == 1 {
            edition_two(&pack);
        }
    }
}

#[test]
fn a_torn_slot_a_write_recovers_the_new_state_from_the_mirror() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let old = std::fs::read(&pack).unwrap();
    edition_two(&pack);
    let new = std::fs::read(&pack).unwrap();
    let new_digest = signable_digest_of(&pack).unwrap().0;
    let old_digest = {
        let f = dir.path().join("old.wax");
        let mut b = old.clone();
        b.truncate(old.len() - 128);
        std::fs::write(&f, &b).unwrap();
        signable_digest_of(&f).unwrap().0
    };
    // Every split of the 128-byte write into a new prefix and an old
    // suffix, and the reverse: never a mixed state, always old or new.
    let mut mixed = 0;
    for cut in 1..128usize {
        for new_first in [true, false] {
            let mut torn = new.clone();
            if new_first {
                torn[cut..128].copy_from_slice(&old[cut..128]);
            } else {
                torn[..cut].copy_from_slice(&old[..cut]);
            }
            let f = dir.path().join("torn.wax");
            std::fs::write(&f, &torn).unwrap();
            let r = WaxReader::open(&f).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
            let d = r.signable_digest().unwrap();
            if torn[..128] == old[..128] {
                assert_eq!(d, old_digest, "cut {cut}: untouched old header");
            } else {
                if torn[..128] != new[..128] {
                    mixed += 1;
                    assert_eq!(r.header_source(), HeaderSource::Mirror, "cut {cut}");
                }
                assert_eq!(d, new_digest, "cut {cut}");
                assert!(!r.read("Osaka.html").unwrap().is_empty());
            }
        }
    }
    assert!(mixed > 100, "the cases exercised real tears ({mixed})");
}

#[test]
fn a_crash_before_the_slot_a_write_leaves_the_old_state() {
    // Mirror written and synced, slot A still the old header: SPEC §7.2's
    // "the archive is exactly its pre-append state" must still hold.
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let old = std::fs::read(&pack).unwrap();
    let old_digest = signable_digest_of(&pack).unwrap().0;
    edition_two(&pack);
    let mut crashed = std::fs::read(&pack).unwrap();
    crashed[..128].copy_from_slice(&old[..128]);
    std::fs::write(&pack, &crashed).unwrap();
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.header_source(), HeaderSource::SlotA);
    assert_eq!(r.signable_digest().unwrap(), old_digest);
    assert!(matches!(r.read("Osaka.html"), Err(WaxError::EntryNotFound(_))));
}

#[test]
fn a_pre_v0_13_base_is_protected_too() {
    // The old header has no check, so the reader tells "never overwritten"
    // from "torn" by the header the mirror's segment says it replaced.
    let dir = tempfile::tempdir().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../wax-builder/tests/fixtures/legacy-tier-pi_5.wax");
    let pack = dir.path().join("legacy.wax");
    std::fs::copy(&src, &pack).unwrap();
    let old = std::fs::read(&pack).unwrap();
    let mut a = [0u8; 128];
    a.copy_from_slice(&old[..128]);
    assert_eq!(check_of(&a), Check::Absent);

    let mut sw = WaxWriter::new(UUID).open_append(&pack).unwrap();
    sw.add_entry(EntryMeta::new("new.html"), &mut &b"fresh"[..]).unwrap();
    sw.finish().unwrap();
    let new = std::fs::read(&pack).unwrap();

    let mut crashed = new.clone();
    crashed[..128].copy_from_slice(&old[..128]);
    std::fs::write(&pack, &crashed).unwrap();
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.header_source(), HeaderSource::SlotA, "untouched old header wins");
    assert!(!r.contains("new.html").unwrap());
    drop(r);

    let mut torn = new.clone();
    torn[48..128].copy_from_slice(&old[48..128]);
    std::fs::write(&pack, &torn).unwrap();
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.header_source(), HeaderSource::Mirror, "a mix of old and new is torn");
    assert_eq!(r.read("new.html").unwrap(), b"fresh");
}

#[test]
fn a_fresh_build_that_crashed_before_its_header_is_recovered() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let mut b = std::fs::read(&pack).unwrap();
    b[..128].fill(0); // the placeholder a fresh build writes first
    std::fs::write(&pack, &b).unwrap();
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.header_source(), HeaderSource::Mirror);
    assert!(r.contains("Kyoto.html").unwrap());
}

#[test]
fn appending_after_a_torn_header_repairs_slot_a_first_so_rollback_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let old = std::fs::read(&pack).unwrap();
    edition_two(&pack);
    let ed2 = std::fs::read(&pack).unwrap();
    let mut torn = ed2.clone();
    torn[64..128].copy_from_slice(&old[64..128]);
    std::fs::write(&pack, &torn).unwrap();

    let mut sw = WaxWriter::new(UUID).created_at(1_700_200_000).open_append(&pack).unwrap();
    sw.add_entry(EntryMeta::new("third.html"), &mut &b"3"[..]).unwrap();
    sw.finish().unwrap();
    rollback_last_update(&pack).unwrap();
    assert_eq!(std::fs::read(&pack).unwrap(), ed2, "back to edition 2, byte for byte");
}

#[test]
fn rollback_restores_the_mirror_and_a_crash_mid_rollback_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let ed1 = std::fs::read(&pack).unwrap();
    edition_two(&pack);
    let ed2 = std::fs::read(&pack).unwrap();
    // crash after rollback's header write, before the truncation
    let mut mid = ed2.clone();
    mid[..128].copy_from_slice(&ed1[..128]);
    std::fs::write(&pack, &mid).unwrap();
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.header_source(), HeaderSource::SlotA);
    assert!(!r.contains("Osaka.html").unwrap());
    drop(r);
    // finishing it with gc --tail-only gives edition 1 exactly
    reclaim_tail(&pack).unwrap();
    assert_eq!(std::fs::read(&pack).unwrap(), ed1);
    // and the normal rollback is byte-identical, mirror included
    std::fs::write(&pack, &ed2).unwrap();
    rollback_last_update(&pack).unwrap();
    assert_eq!(std::fs::read(&pack).unwrap(), ed1);
}

// --- §5.7.2 wax-bigram-16 --------------------------------------------------------

#[test]
fn cjk_words_of_one_two_and_more_characters_are_found() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.search_tokenizer(), Some(WAX_BIGRAM));
    assert_eq!(search(&r, "北京大学"), ["Beijing.html"], "four characters");
    assert_eq!(search(&r, "大学"), ["Beijing.html"], "two characters, mid-sentence");
    assert_eq!(search(&r, "水"), ["Beijing.html"], "one character, at a run's end");
    assert_eq!(search(&r, "山"), ["Beijing.html"], "one character, inside a run");
    assert_eq!(search(&r, "新宿"), ["Tokyo.html"], "kanji word inside Japanese");
    assert_eq!(search(&r, "東京駅"), ["Tokyo.html"]);
    assert_eq!(search(&r, "東京"), ["Tokyo.html"], "title");
    // a bigram index can match a pair that spans two words: 東京都庁 holds 京都
    assert_eq!(search(&r, "京都"), ["Tokyo.html"]);
    // Latin text still searches as unicode61 did, plus the fold
    assert_eq!(search(&r, "kyoto protocol"), ["Kyoto.html"]);
    assert_eq!(search(&r, "STRASSE"), ["Strasse.html"], "ß folds to ss");
    assert_eq!(search(&r, "strasse"), ["Strasse.html"]);
}

#[test]
fn cjk_search_follows_updates() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    edition_two(&pack);
    let r = WaxReader::open(&pack).unwrap();
    assert!(search(&r, "北京").is_empty(), "deleted");
    assert_eq!(search(&r, "たこ焼き"), ["Osaka.html"], "added, kana");
    assert!(search(&r, "protocol").is_empty(), "replaced text is gone");
    assert_eq!(search(&r, "temples"), ["Kyoto.html"]);
}

#[test]
fn an_append_keeps_the_pack_tokenizer() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let mut sw = WaxWriter::new(UUID).open_append(&pack).unwrap();
    assert!(matches!(sw.set_search_tokenizer("unicode61"), Err(WaxError::TokenizerMismatch { .. })));
    sw.set_search_tokenizer(WAX_BIGRAM).unwrap();
}

// --- §5.10 passages --------------------------------------------------------

#[test]
fn passages_are_cut_at_headings_and_named_by_path_and_position() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let r = WaxReader::open(&pack).unwrap();
    let p = r.passages("Kyoto.html").unwrap();
    assert_eq!(p.len(), 3);
    assert_eq!(p[0].ordinal, 0);
    assert_eq!(p[0].heading, None, "the lead");
    assert_eq!(p[1].heading.as_deref(), Some("History"));
    assert_eq!(p[2].anchor.as_deref(), Some("Climate"));
    let page = r.read("Kyoto.html").unwrap();
    assert!(std::str::from_utf8(&page[p[2].start as usize..p[2].end as usize]).unwrap().contains("1997"));
    // through an alias, and absent for content without a row
    assert_eq!(r.passages("Kyoto_City.html").unwrap(), p);
    assert!(matches!(r.passages("img/a.png"), Err(WaxError::PassagesUnavailable { .. })));
}

#[test]
fn passage_search_points_at_the_matching_section() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let r = WaxReader::open(&pack).unwrap();
    let hits = r.search_passages(&match_expr("protocol", false).unwrap(), 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "Kyoto.html");
    assert_eq!(hits[0].passage.as_ref().unwrap().heading.as_deref(), Some("Climate"));
    let hits = r.search_passages(&match_expr("新宿", false).unwrap(), 5).unwrap();
    assert_eq!(hits[0].passage.as_ref().unwrap().heading.as_deref(), Some("歴史"));
    let best = r.best_passages("Tokyo.html", &match_expr("丸の内", false).unwrap(), 3).unwrap();
    assert_eq!(best.iter().map(|p| p.ordinal).collect::<Vec<_>>(), [2]);
}

#[test]
fn passages_follow_updates_and_pre_v0_13_packs_have_none() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    edition_two(&pack);
    let r = WaxReader::open(&pack).unwrap();
    let p = r.passages("Kyoto.html").unwrap();
    assert_eq!(p.iter().filter_map(|p| p.heading.clone()).collect::<Vec<_>>(), ["Temples"]);
    assert!(matches!(r.passages("Beijing.html"), Err(WaxError::EntryDeleted(_))));

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../wax-builder/tests/fixtures/legacy-tier-pi_5.wax");
    let legacy = WaxReader::open(src).unwrap();
    let ep = legacy.manifest().get("entry_point").cloned().unwrap();
    assert!(matches!(legacy.passages(&ep), Err(WaxError::PassagesUnavailable { .. })));
}

#[test]
fn the_writer_refuses_passages_for_entries_it_does_not_carry() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let mut sw = WaxWriter::new(UUID).open_append(&pack).unwrap();
    sw.index_passages("Tokyo.html", b"<h2>x</h2>").unwrap(); // not re-added here
    assert!(matches!(sw.finish(), Err(WaxError::Schema { .. })));
}

// --- §7.6 gc -----------------------------------------------------------------

fn snapshot(r: &WaxReader) -> Vec<(String, Vec<u8>)> {
    r.paths()
        .map(|p| {
            let p = p.unwrap();
            let b = r.read(&p).unwrap();
            (p, b)
        })
        .collect()
}

#[test]
fn compaction_keeps_exactly_what_is_live_and_reclaims_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    edition_two(&pack);
    let before = WaxReader::open(&pack).unwrap();
    let out = dir.path().join("compact.wax");
    let st = compact(&pack, &out, CompactOptions::default()).unwrap();
    let after = WaxReader::open(&out).unwrap();

    assert_eq!(after.segment_count(), 1);
    assert!(st.output_bytes < st.input_bytes, "{st:?}");
    assert_eq!(snapshot(&after), snapshot(&before), "every live path, same bytes");
    assert_eq!(after.manifest(), before.manifest());
    assert_eq!(after.header().archive_uuid, UUID);
    assert_eq!(after.header().created_at, before.header().created_at);
    assert_eq!(after.entry_count().unwrap(), before.entry_count().unwrap());
    assert!(after.has_mirror().unwrap());
    // the same entries' stored frames, verbatim
    for p in ["Osaka.html", "Tokyo.html", "img/a.png"] {
        let (a, b) = (before.entry(p).unwrap(), after.entry(p).unwrap());
        assert_eq!((a.length, a.sha256, a.compression.clone()), (b.length, b.sha256, b.compression.clone()));
    }
    // search answers the same set; passages carried
    for q in ["北京", "たこ焼き", "kyoto", "temples", "東京駅", "水", "strasse", "protocol"] {
        let mut x = search(&before, q);
        let mut y = search(&after, q);
        x.sort();
        y.sort();
        assert_eq!(x, y, "{q}");
    }
    assert_eq!(after.passages("Tokyo.html").unwrap(), before.passages("Tokyo.html").unwrap());
    assert_eq!(after.suggest("osa", 5).unwrap().len(), 0);
    assert_eq!(after.suggest("大", 5).unwrap()[0].path, "Osaka.html");
}

#[test]
fn compaction_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    edition_two(&pack);
    let (a, b) = (dir.path().join("a.wax"), dir.path().join("b.wax"));
    compact(&pack, &a, CompactOptions::default()).unwrap();
    compact(&pack, &b, CompactOptions::default()).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    // compacting a compacted pack changes nothing
    let c = dir.path().join("c.wax");
    compact(&a, &c, CompactOptions::default()).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&c).unwrap());
}

#[test]
fn a_compacted_pack_takes_updates_again() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    edition_two(&pack);
    let out = dir.path().join("compact.wax");
    compact(&pack, &out, CompactOptions::default()).unwrap();
    let mut sw = WaxWriter::new(UUID).open_append(&out).unwrap();
    add(&mut sw, "Nara.html", "奈良", &html("奈良", &[("鹿", "奈良公園の鹿。")]));
    sw.finish().unwrap();
    let r = WaxReader::open(&out).unwrap();
    assert_eq!(search(&r, "公園"), ["Nara.html"]);
    assert_eq!(search(&r, "たこ焼き"), ["Osaka.html"]);
}

#[test]
fn compaction_carries_unicode61_and_refuses_trigram_unless_told_to_drop_search() {
    let dir = tempfile::tempdir().unwrap();
    for (tok, carried) in [("unicode61", true), ("trigram", false)] {
        let sub = dir.path().join(tok);
        std::fs::create_dir(&sub).unwrap();
        let pack = edition_one(&sub, tok);
        edition_two(&pack);
        let out = sub.join("out.wax");
        let res = compact(&pack, &out, CompactOptions::default());
        if carried {
            res.unwrap();
            let r = WaxReader::open(&out).unwrap();
            assert_eq!(r.search("\"kyoto\"", 5).unwrap().len(), 1);
        } else {
            assert!(matches!(res, Err(WaxError::Compaction { .. })), "{tok}: {res:?}");
            assert!(!out.exists(), "a refused compaction leaves no output");
            let st = compact(&pack, &out, CompactOptions { drop_search: true, ..Default::default() }).unwrap();
            assert!(st.search_dropped);
            assert!(!WaxReader::open(&out).unwrap().has_search_index());
        }
    }
}

#[test]
fn compaction_refuses_a_corrupted_entry() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let off = WaxReader::open(&pack).unwrap().entry("img/a.png").unwrap().offset as usize;
    let mut b = std::fs::read(&pack).unwrap();
    b[off] ^= 0xff;
    std::fs::write(&pack, &b).unwrap();
    let res = compact(&pack, dir.path().join("o.wax"), CompactOptions::default());
    assert!(matches!(res, Err(WaxError::ChecksumMismatch { .. })), "{res:?}");
}

#[test]
fn reclaiming_the_tail_keeps_the_signature_digest() {
    let dir = tempfile::tempdir().unwrap();
    let pack = edition_one(dir.path(), WAX_BIGRAM);
    let clean = std::fs::read(&pack).unwrap();
    let digest = signable_digest_of(&pack).unwrap().0;
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&pack).unwrap();
        f.write_all(&[0xAB; 5000]).unwrap();
    }
    let st = reclaim_tail(&pack).unwrap();
    assert_eq!(st.reclaimed_bytes, 5000);
    assert_eq!(std::fs::read(&pack).unwrap(), clean);
    assert_eq!(signable_digest_of(&pack).unwrap().0, digest);
}

// ---------------------------------------------------------------------------
// Manifest updates (SPEC v0.13 §5.6.1)
// ---------------------------------------------------------------------------

fn manifest_v(version: &str) -> BTreeMap<String, String> {
    let mut m = manifest();
    m.insert("version".into(), version.into());
    m
}

/// Edition 1 with a versioned manifest, then an append that adds a page and
/// replaces the manifest.
fn versioned_pack(dir: &Path) -> (PathBuf, Vec<u8>) {
    let out = dir.join("versioned.wax");
    let mut sw = WaxWriter::new(UUID).created_at(1_700_000_000).create(&out, &manifest_v("2026.09.1")).unwrap();
    add(&mut sw, "A.html", "A", &html("A", &[("One", "first")]));
    sw.finish().unwrap();
    let before = std::fs::read(&out).unwrap();
    let mut sw = WaxWriter::new(UUID).created_at(1_700_100_000).open_append(&out).unwrap();
    add(&mut sw, "B.html", "B", &html("B", &[("Two", "second")]));
    sw.set_manifest(&manifest_v("2026.10.1"));
    sw.finish().unwrap();
    (out, before)
}

#[test]
fn an_append_can_replace_the_manifest_and_rollback_undoes_it() {
    let dir = tempfile::tempdir().unwrap();
    let (pack, before) = versioned_pack(dir.path());
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.manifest(), &manifest_v("2026.10.1"));
    assert_eq!(r.manifest_segment(), 1);
    assert!(!r.read("A.html").unwrap().is_empty(), "older entries untouched");
    drop(r);

    // A later append that sets nothing keeps the replacement.
    let mut sw = WaxWriter::new(UUID).created_at(1_700_200_000).open_append(&pack).unwrap();
    add(&mut sw, "C.html", "C", &html("C", &[("Three", "third")]));
    sw.finish().unwrap();
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.manifest(), &manifest_v("2026.10.1"));
    assert_eq!(r.manifest_segment(), 1);
    drop(r);

    rollback_last_update(&pack).unwrap();
    rollback_last_update(&pack).unwrap();
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.manifest(), &manifest_v("2026.09.1"));
    assert_eq!(r.manifest_segment(), 0);
    drop(r);
    assert_eq!(std::fs::read(&pack).unwrap(), before, "byte-identical after undo");
}

#[test]
fn a_manifest_update_travels_in_the_update_tail() {
    // SPEC §7.4: an update is the old pack's bytes up to its logical end, then
    // the new segment. Replacing the manifest must not touch earlier bytes.
    let dir = tempfile::tempdir().unwrap();
    let (pack, before) = versioned_pack(dir.path());
    let after = std::fs::read(&pack).unwrap();
    let old_end = before.len() - 128; // minus the mirror
    assert_eq!(&after[128..old_end], &before[128..old_end]);
    // Applying exactly that tail to the old copy gives the new edition.
    let mut applied = after[..128].to_vec();
    applied.extend_from_slice(&before[128..old_end]);
    applied.extend_from_slice(&after[old_end..]);
    assert_eq!(applied, after);
    let f = dir.path().join("before.wax");
    std::fs::write(&f, &before).unwrap();
    assert_ne!(signable_digest_of(&f).unwrap(), signable_digest_of(&pack).unwrap());
}

#[test]
fn compaction_keeps_the_replaced_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let (pack, _) = versioned_pack(dir.path());
    let out = dir.path().join("compact.wax");
    compact(&pack, &out, CompactOptions::default()).unwrap();
    let r = WaxReader::open(&out).unwrap();
    assert_eq!(r.segment_count(), 1);
    assert_eq!(r.manifest(), &manifest_v("2026.10.1"));
    assert_eq!(r.manifest_segment(), 0);
}

#[test]
fn setting_the_manifest_on_a_fresh_build_just_sets_it() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("fresh.wax");
    let mut sw = WaxWriter::new(UUID).created_at(1_700_000_000).create(&out, &manifest_v("2026.09.1")).unwrap();
    add(&mut sw, "A.html", "A", &html("A", &[("One", "first")]));
    sw.set_manifest(&manifest_v("2026.09.2"));
    sw.finish().unwrap();
    let r = WaxReader::open(&out).unwrap();
    assert_eq!(r.manifest(), &manifest_v("2026.09.2"));
    assert_eq!(r.manifest_segment(), 0, "only an appended segment declares an update");
}

/// Rewrite the newest segment of `pack` with `f`, fixing the header's
/// `index_length` and check; the mirror is dropped.
fn rewrite_newest_segment(pack: &Path, f: impl FnOnce(&rusqlite::Connection)) {
    let bytes = std::fs::read(pack).unwrap();
    let h = wax_core::header::WaxHeader::parse(&bytes[..128]).unwrap();
    let (o, l) = (h.index_offset as usize, h.index_length as usize);
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("seg.db");
    std::fs::write(&p, &bytes[o..o + l]).unwrap();
    {
        let conn = rusqlite::Connection::open(&p).unwrap();
        f(&conn);
        conn.execute("VACUUM", []).unwrap();
        conn.close().unwrap();
    }
    let seg = std::fs::read(&p).unwrap();
    let mut h2 = h;
    h2.index_length = seg.len() as u64;
    let mut out = h2.to_bytes().to_vec();
    out.extend_from_slice(&bytes[128..o]);
    out.extend_from_slice(&seg);
    std::fs::write(pack, out).unwrap();
}

#[test]
fn a_manifest_table_in_a_segment_that_declares_nothing_is_ignored() {
    // Every version before v0.13 told readers to ignore a manifest outside
    // segment 0; that stays true unless the segment declares the update.
    let dir = tempfile::tempdir().unwrap();
    let (pack, _) = versioned_pack(dir.path());
    rewrite_newest_segment(&pack, |c| {
        c.execute("DELETE FROM segment_meta WHERE key = 'manifest_update'", []).unwrap();
    });
    let r = WaxReader::open(&pack).unwrap();
    assert_eq!(r.manifest(), &manifest_v("2026.09.1"));
    assert_eq!(r.manifest_segment(), 0);
}

#[test]
fn a_malformed_manifest_update_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (pack, _) = versioned_pack(dir.path());
    let copy = dir.path().join("copy.wax");
    std::fs::copy(&pack, &copy).unwrap();

    rewrite_newest_segment(&pack, |c| {
        c.execute("UPDATE segment_meta SET value = 'merge' WHERE key = 'manifest_update'", []).unwrap();
    });
    let err = WaxReader::open(&pack).unwrap_err();
    assert!(matches!(err, WaxError::BadManifestUpdate { segment: 1, .. }), "{err}");

    rewrite_newest_segment(&copy, |c| {
        c.execute("DROP TABLE manifest", []).unwrap();
    });
    let err = WaxReader::open(&copy).unwrap_err();
    assert!(matches!(err, WaxError::BadManifestUpdate { segment: 1, .. }), "{err}");
}
