//! Validate the committed hand-built fixtures in `tests/fixtures/`
//! (regenerate with `cargo run -p wax-core --example gen_fixtures`).
//!
//! These give other tracks (and `zim2wax`, once it exists) a stable set of
//! reference archives, and pin their behaviour here.

use std::path::PathBuf;
use wax_core::{WaxError, WaxReader};

fn fx(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

#[test]
fn fixture_minimal_opens_and_reads() {
    let r = WaxReader::open(fx("minimal.wax")).unwrap();
    assert_eq!(r.segment_count(), 1);
    assert_eq!(r.read("index.html").unwrap(), b"<h1>ok</h1>");
}

#[test]
fn fixture_redirect_chain_is_flattened_on_disk() {
    let r = WaxReader::open(fx("redirect-chain.wax")).unwrap();
    // every alias points straight at the canonical entry (depth 1)
    for alias in ["articles/old-name.html", "articles/older-name.html", "index.html"] {
        assert_eq!(
            r.entry(alias).unwrap().redirect_to.as_deref(),
            Some("articles/canonical.html"),
            "{alias} not flattened"
        );
        assert_eq!(r.read(alias).unwrap(), b"CANON");
    }
    assert_eq!(r.manifest().get("title").map(String::as_str), Some("Redirect Fixture"));
}

#[test]
fn fixture_multi_segment_merges_last_wins() {
    let r = WaxReader::open(fx("multi-segment.wax")).unwrap();
    assert_eq!(r.segment_count(), 3);
    assert_eq!(r.read("a.txt").unwrap(), b"a-base");
    assert_eq!(r.read("shared.txt").unwrap(), b"v2", "append overrides base");
    assert_eq!(r.read("b.txt").unwrap(), b"b-append");
}

#[test]
fn fixture_corrupt_signature_archive_opens_but_digest_wont_match_sidecar() {
    // The archive itself is valid...
    let r = WaxReader::open(fx("corrupt-signature.wax")).unwrap();
    let digest = r.signable_digest().unwrap();
    assert_eq!(digest.len(), 32);

    // ...and the sidecar is present but bogus. A7's verify path will reject it;
    // for now assert the sidecar is not a parseable minisign signature over the
    // real digest (it is literally the string "invalid").
    let sig = std::fs::read_to_string(fx("corrupt-signature.wax.minisig")).unwrap();
    assert!(sig.contains("DELIBERATELY INVALID"));
    assert!(!sig.contains(&hex(&digest)), "sidecar must not carry the real digest");
}

#[test]
fn fixture_tampering_changes_the_signable_digest() {
    // Core of the signing model (SPEC §8.1): any change to header or index
    // moves the digest, so a signature over the old state fails.
    let bytes = std::fs::read(fx("minimal.wax")).unwrap();
    let dir = tempfile::tempdir().unwrap();

    let clean = dir.path().join("clean.wax");
    std::fs::write(&clean, &bytes).unwrap();
    let d_clean = WaxReader::open(&clean).unwrap().signable_digest().unwrap();

    // flip a byte in the index segment
    let mut tampered_bytes = bytes.clone();
    let off = WaxReader::open(&clean).unwrap().header().index_offset as usize;
    tampered_bytes[off + 100] ^= 0x20;
    let tampered = dir.path().join("tampered.wax");
    std::fs::write(&tampered, &tampered_bytes).unwrap();

    match WaxReader::open(&tampered) {
        Ok(r) => {
            let d = r.signable_digest().unwrap();
            assert_ne!(d_clean, d, "digest must change when the index is tampered");
        }
        // A byte flip may also make SQLite refuse the db outright — also fine.
        Err(WaxError::NotAnIndexSegment { .. })
        | Err(WaxError::Schema { .. })
        | Err(WaxError::Sqlite(_)) => {}
        Err(e) => panic!("unexpected error: {e:?}"),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
