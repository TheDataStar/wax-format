//! A4 fuzz smoke tests — run the three panic-free entry points
//! ([`wax_core::fuzz`]) over adversarial inputs and the checked-in seed corpus,
//! so the "never panic on malformed input" property is enforced on platforms
//! where libFuzzer is unavailable (SPEC §11.3, §12.15).
//!
//! `cargo fuzz` targets in `fuzz/` call the exact same functions.

use std::path::Path;
use wax_core::fuzz::{check_header_parse, check_index_loader, check_segment_merge};

/// Every byte string here must be handled without a panic. The return value is
/// irrelevant — `Ok` (handled) and `Err` (rejected) are both fine.
fn adversarial_inputs() -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = vec![
        vec![],
        vec![0],
        vec![0xFF; 1],
        b"WAX1".to_vec(),
        b"WAX1\x00\x09".to_vec(),
        vec![0x00; 128],
        vec![0xFF; 128],
        vec![0xFF; 4096],
        {
            // valid magic, garbage everywhere else
            let mut b = vec![0u8; 200];
            b[..4].copy_from_slice(b"WAX1");
            b
        },
        {
            // magic + huge index_offset/length
            let mut b = vec![0u8; 128];
            b[..4].copy_from_slice(b"WAX1");
            b[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
            b[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
            b
        },
        b"SQLite format 3\x00".to_vec(),
        {
            let mut b = b"SQLite format 3\x00".to_vec();
            b.extend(std::iter::repeat_n(0xAB, 4096));
            b
        },
    ];
    // A real segment db is a good adversarial seed for the loader/merge too.
    if let Ok(seg) = wax_core::writer::build_segment_db_with_meta(
        &[],
        &[
            ("format".to_string(), "wax-index-segment".to_string()),
            ("segment_index".to_string(), "0".to_string()),
            ("blob_region_offset".to_string(), "128".to_string()),
            ("blob_region_length".to_string(), "0".to_string()),
            ("created_at".to_string(), "0".to_string()),
        ],
        None,
    ) {
        v.push(seg.clone());
        // and a truncated copy
        v.push(seg[..seg.len() / 2].to_vec());
    }
    v
}

fn corpus_dir(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus")
        .join(name)
}

fn corpus_inputs(name: &str) -> Vec<Vec<u8>> {
    let dir = corpus_dir(name);
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path().is_file() {
                if let Ok(b) = std::fs::read(e.path()) {
                    out.push(b);
                }
            }
        }
    }
    out
}

#[test]
fn header_parse_never_panics() {
    for input in adversarial_inputs().iter().chain(corpus_inputs("header-parse").iter()) {
        let _ = check_header_parse(input);
    }
}

#[test]
fn index_loader_never_panics() {
    for input in adversarial_inputs().iter().chain(corpus_inputs("index-loader").iter()) {
        let _ = check_index_loader(input);
    }
}

#[test]
fn segment_merge_never_panics() {
    for input in adversarial_inputs().iter().chain(corpus_inputs("segment-merge").iter()) {
        let _ = check_segment_merge(input);
    }
    // plus a brute sweep of short byte strings
    for a in 0u16..=255 {
        for b in [0u8, 1, 2, 9, 255] {
            let _ = check_segment_merge(&[a as u8, b, a as u8, b, b, a as u8]);
        }
    }
}

#[test]
fn header_parse_roundtrips_on_all_valid_headers() {
    // parse(to_bytes(h)) == h-ish for a spread of field values
    for &io in &[128u64, 512, 1 << 20] {
        for &il in &[512u64, 4096, 1 << 16] {
            let h = wax_core::header::WaxHeader {
                index_offset: io,
                index_length: il,
                blob_section_length: io - 128,
                ..wax_core::header::WaxHeader::default()
            };
            let bytes = h.to_bytes();
            let parsed = wax_core::header::WaxHeader::parse(&bytes).unwrap();
            assert_eq!(parsed, h);
        }
    }
}
