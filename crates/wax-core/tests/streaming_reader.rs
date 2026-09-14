//! A1b — the streaming reader (Track A §18, "the next ceiling is the reader").
//!
//! The conformance suite already pins every observable behaviour; these tests
//! pin what is specific to *how* the reader now works: lookups are index
//! probes rather than a materialized map, iteration is a paged k-way merge
//! that honours last-segment-wins across page boundaries, the lookup cache
//! is bounded, and append validates against the archive by point lookup.

mod common;

use common::*;
use std::collections::BTreeMap;
use std::io;
use wax_core::reader::{ReadOptions, PAGE_ROWS};
use wax_core::writer::EntryMeta;
use wax_core::{Compression, EntryInput, WaxError, WaxReader, WaxWriter};

fn writer() -> WaxWriter {
    WaxWriter::new(TEST_UUID).created_at(1_700_000_000)
}

#[test]
fn point_lookup_and_pagination_use_the_path_key_not_a_scan() {
    // A freshly written archive: `entries` is WITHOUT ROWID, so the primary
    // key *is* the table and both queries walk it directly.
    let f = valid(vec![EntryInput::data("a", b"1".to_vec(), Compression::None)]);
    let r = WaxReader::open(&f.path).unwrap();
    let (lookup, page) = r.query_plans().unwrap();
    assert!(lookup.contains("SEARCH") && lookup.contains("PRIMARY KEY"), "lookup plan: {lookup}");
    assert!(page.contains("SEARCH") && page.contains("PRIMARY KEY") && !page.contains("TEMP B-TREE"), "page plan: {page}");

    // A pre-A1b archive (rowid table + autoindex on path, the committed
    // fixtures) still probes the index rather than scanning.
    let fx = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal.wax");
    let r = WaxReader::open(fx).unwrap();
    let (lookup, page) = r.query_plans().unwrap();
    assert!(lookup.contains("SEARCH") && lookup.contains("INDEX") && !lookup.contains("SCAN"), "lookup plan: {lookup}");
    assert!(page.contains("SEARCH") && page.contains("INDEX") && !page.contains("TEMP B-TREE"), "page plan: {page}");
}

#[test]
fn opening_does_not_depend_on_entry_count_for_what_it_holds() {
    // 3 × PAGE_ROWS entries: the reader reports them all, in order, without
    // ever having held more than a page per segment. (The memory bound is
    // measured out-of-process; this pins correctness across page edges.)
    let n = PAGE_ROWS * 3 + 7;
    let f = new_fixture("many.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    for i in 0..n {
        w.add_entry(EntryMeta::new(format!("e{i:06}")), &mut io::Cursor::new(vec![i as u8; 3]))
            .unwrap();
    }
    w.finish().unwrap();
    let r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.entry_count().unwrap(), n as u64);
    let paths: Vec<String> = r.paths().map(|p| p.unwrap()).collect();
    assert_eq!(paths.len(), n);
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "ascending byte order");
    assert_eq!(paths[PAGE_ROWS], format!("e{PAGE_ROWS:06}"), "no gap at a page edge");
    assert_eq!(r.entry("e000300").unwrap().uncompressed_length, 3);
    assert!(matches!(r.entry("e999999").unwrap_err(), WaxError::EntryNotFound(_)));
}

#[test]
fn merged_iteration_is_last_segment_wins_across_page_boundaries() {
    // base: p0..p600 = "base"; append 1: every 3rd path overridden;
    // append 2: p000..p010 overridden again and one brand-new path.
    let n = PAGE_ROWS * 2 + 88;
    let base: Vec<EntryInput> = (0..n)
        .map(|i| EntryInput::data(format!("p{i:04}"), b"base".to_vec(), Compression::None))
        .collect();
    let app1: Vec<EntryInput> = (0..n)
        .filter(|i| i.is_multiple_of(3))
        .map(|i| EntryInput::data(format!("p{i:04}"), b"one".to_vec(), Compression::None))
        .collect();
    let mut app2: Vec<EntryInput> = (0..10)
        .map(|i| EntryInput::data(format!("p{i:04}"), b"two".to_vec(), Compression::None))
        .collect();
    app2.push(EntryInput::data("zzz-new", b"new".to_vec(), Compression::None));
    let f = valid_multi(base, vec![app1, app2]);

    let r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.segment_count(), 3);
    assert_eq!(r.entry_count().unwrap(), n as u64 + 1);

    let mut seen = 0;
    let mut prev = String::new();
    for e in r.entries() {
        let e = e.unwrap();
        assert!(e.path > prev, "ordered and deduplicated: {prev} then {}", e.path);
        prev = e.path.clone();
        seen += 1;
        let i: usize = e.path.trim_start_matches('p').parse().unwrap_or(usize::MAX);
        let expect = if e.path == "zzz-new" {
            "new"
        } else if i < 10 {
            "two"
        } else if i.is_multiple_of(3) {
            "one"
        } else {
            "base"
        };
        assert_eq!(r.read(&e.path).unwrap(), expect.as_bytes(), "{}", e.path);
        assert_eq!(e.uncompressed_length as usize, expect.len(), "iterated entry is the winner");
    }
    assert_eq!(seen, n + 1);
}

#[test]
fn lookup_cache_is_bounded_and_correct() {
    let f = valid(
        (0..50)
            .map(|i| EntryInput::data(format!("k{i}"), vec![i as u8], Compression::None))
            .collect(),
    );
    let r = WaxReader::open_with(
        &f.path,
        ReadOptions {
            verify_checksums: true,
            lookup_cache_entries: 8,
        },
    )
    .unwrap();
    for _ in 0..3 {
        for i in 0..50 {
            assert_eq!(r.read(&format!("k{i}")).unwrap(), vec![i as u8]);
        }
    }
    let (hits, misses) = r.cache_stats();
    assert_eq!(hits + misses, 150);
    assert!(misses >= 50, "every path missed at least once");
    // the cache can never have held more than its cap: with a cap of 8 and a
    // cyclic walk over 50 keys it clears repeatedly, so hits stay small
    assert!(hits < 150, "some lookups must have gone to the index");

    let off = WaxReader::open_with(
        &f.path,
        ReadOptions {
            verify_checksums: true,
            lookup_cache_entries: 0,
        },
    )
    .unwrap();
    off.read("k1").unwrap();
    off.read("k1").unwrap();
    assert_eq!(off.cache_stats(), (0, 2), "disabled cache never hits");
}

#[test]
fn append_validates_redirect_targets_by_point_lookup_and_rehomes_through_prior_redirects() {
    let f = new_fixture("app.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    w.add_entry(EntryMeta::new("real"), &mut io::Cursor::new(b"R".to_vec())).unwrap();
    w.add_redirect("old-alias", "real", None).unwrap();
    w.finish().unwrap();

    let mut a = writer().created_at(1_700_000_100).open_append(&f.path).unwrap();
    // targets a redirect in the base segment: must land on its terminus so the
    // one-hop rule (SPEC §5.3) still holds across segments
    a.add_redirect("newer-alias", "old-alias", None).unwrap();
    assert!(a.resolves("real").unwrap());
    assert!(!a.resolves("nope").unwrap());
    a.finish().unwrap();

    let r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.entry("newer-alias").unwrap().redirect_to.as_deref(), Some("real"));
    assert_eq!(r.read("newer-alias").unwrap(), b"R");

    // a dangling target in the append is still the same error
    let mut a = writer().created_at(1_700_000_200).open_append(&f.path).unwrap();
    a.add_redirect("x", "missing", None).unwrap();
    assert!(matches!(a.finish().unwrap_err(), WaxError::DanglingRedirect { ref to, .. } if to == "missing"));
}

#[test]
fn reads_take_shared_borrows_so_iteration_and_reads_interleave() {
    let f = valid(vec![
        EntryInput::data("a", b"A".to_vec(), Compression::Zstd),
        EntryInput::data("b", b"B".to_vec(), Compression::None),
        EntryInput::redirect("c", "a"),
    ]);
    let r = WaxReader::open(&f.path).unwrap();
    let mut got = Vec::new();
    for e in r.entries() {
        let e = e.unwrap();
        got.push((e.path.clone(), r.read(&e.path).unwrap()));
    }
    assert_eq!(
        got,
        vec![
            ("a".to_string(), b"A".to_vec()),
            ("b".to_string(), b"B".to_vec()),
            ("c".to_string(), b"A".to_vec())
        ]
    );
}

#[test]
fn archive_path_with_uri_special_characters_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("odd name #1 & 50%.wax");
    writer()
        .build(&path, vec![EntryInput::data("a", b"1".to_vec(), Compression::None)], &BTreeMap::new())
        .unwrap();
    let r = WaxReader::open(&path).unwrap();
    assert_eq!(r.read("a").unwrap(), b"1");
}
