//! A4 round-trip correctness — write an archive, read it back, verify every
//! entry's bytes, redirect resolution, and the signable digest (SPEC §11).

mod common;

use common::*;
use std::collections::BTreeMap;
use wax_core::{Compression, EntryInput, WaxReader};

#[test]
fn roundtrip_mixed_content_exact_bytes() {
    let cases: Vec<(&str, Vec<u8>, Compression)> = vec![
        ("index.html", b"<!doctype html><h1>Home</h1>".to_vec(), Compression::Zstd),
        ("img/logo.bin", (0u8..=255).cycle().take(4096).collect(), Compression::Zstd),
        ("data/empty", Vec::new(), Compression::None),
        ("data/one-byte", vec![0x42], Compression::None),
        ("notes.txt", "unicode: café — 日本語 — 🚀".as_bytes().to_vec(), Compression::Zstd),
    ];

    let entries: Vec<EntryInput> = cases
        .iter()
        .map(|(p, b, c)| EntryInput::data(*p, b.clone(), *c).with_mime("application/octet-stream"))
        .collect();

    let f = valid(entries);
    let r = WaxReader::open(&f.path).unwrap();

    for (path, expected, _) in &cases {
        let got = r.read(path).unwrap();
        assert_eq!(&got, expected, "content mismatch for {path}");
    }

    let listed: Vec<String> = r.paths().map(|p| p.unwrap()).collect();
    assert_eq!(listed.len(), cases.len());
}

#[test]
fn roundtrip_redirect_chains_resolve() {
    let f = valid(vec![
        EntryInput::data("target.html", b"TARGET".to_vec(), Compression::None),
        EntryInput::redirect("hop1", "target.html"),
        EntryInput::redirect("hop2", "hop1"), // flattened to -> target.html
        EntryInput::redirect("hop3", "hop2"), // flattened to -> target.html
    ]);
    let r = WaxReader::open(&f.path).unwrap();
    for alias in ["hop1", "hop2", "hop3"] {
        assert_eq!(r.resolve(alias).unwrap().entry.path, "target.html", "{alias}");
        assert_eq!(r.read(alias).unwrap(), b"TARGET", "{alias}");
    }
}

#[test]
fn roundtrip_manifest_base_segment_only() {
    let mut manifest = BTreeMap::new();
    manifest.insert("title".to_string(), "Test Pack".to_string());
    manifest.insert("lang".to_string(), "en".to_string());

    let f = valid_with_manifest(
        vec![EntryInput::data("a", b"x".to_vec(), Compression::None)],
        manifest.clone(),
    );
    let r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.manifest(), &manifest);
}

#[test]
fn roundtrip_append_preserves_earlier_bytes() {
    // The append-commit protocol must not disturb segment 0's blobs.
    let f = valid_multi(
        vec![
            EntryInput::data("keep.txt", b"KEEP ME EXACTLY".to_vec(), Compression::None),
            EntryInput::data("also.txt", b"also kept".to_vec(), Compression::Zstd),
        ],
        vec![
            vec![EntryInput::data("added-1.txt", b"first append".to_vec(), Compression::None)],
            vec![EntryInput::data("added-2.txt", b"second append".to_vec(), Compression::Zstd)],
        ],
    );
    let r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.segment_count(), 3);
    assert_eq!(r.read("keep.txt").unwrap(), b"KEEP ME EXACTLY");
    assert_eq!(r.read("also.txt").unwrap(), b"also kept");
    assert_eq!(r.read("added-1.txt").unwrap(), b"first append");
    assert_eq!(r.read("added-2.txt").unwrap(), b"second append");
}

#[test]
fn signable_digest_is_stable_and_changes_on_append() {
    let f = valid_multi(
        vec![EntryInput::data("a", b"a".to_vec(), Compression::None)],
        vec![],
    );
    let r1 = WaxReader::open(&f.path).unwrap();
    let d1 = r1.signable_digest().unwrap();
    let r2 = WaxReader::open(&f.path).unwrap();
    let d2 = r2.signable_digest().unwrap();
    assert_eq!(d1, d2, "digest is deterministic");

    // append and re-open: digest must move (SPEC §8.1)
    wax_core::WaxWriter::new(TEST_UUID)
        .created_at(1_700_000_100)
        .append(&f.path, vec![EntryInput::data("b", b"b".to_vec(), Compression::None)])
        .unwrap();
    let r3 = WaxReader::open(&f.path).unwrap();
    let d3 = r3.signable_digest().unwrap();
    assert_ne!(d1, d3, "digest changes when a segment is appended");
}

#[test]
fn checksum_verification_can_be_disabled() {
    let f = valid(vec![EntryInput::data("p", b"data".to_vec(), Compression::None)]);
    let mut bytes = f.bytes();
    bytes[wax_core::HEADER_LEN + 1] ^= 0x01; // corrupt blob
    let f2 = write_wax(&bytes);

    // default: rejected
    let strict = WaxReader::open(&f2.path).unwrap();
    assert!(strict.read("p").is_err());

    // opt-out: bytes come back (caller took responsibility)
    let loose =
        WaxReader::open_with(&f2.path, wax_core::reader::ReadOptions { verify_checksums: false, ..Default::default() })
            .unwrap();
    assert_eq!(loose.read("p").unwrap().len(), 4);
}
