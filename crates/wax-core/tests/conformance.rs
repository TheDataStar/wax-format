//! A4 conformance corpus — SPEC §11.1 (positive) and §11.2 (negative).
//!
//! Each test names the SPEC row it encodes. Negative tests assert the *error
//! group* (variant), never the `Display` string.

mod common;

use common::*;
use wax_core::{Compression, EntryInput, WaxError, WaxReader};

// ===========================================================================
// §11.1  positive corpus — must open and behave
// ===========================================================================

#[test]
fn pos_minimum_size_archive() {
    let f = minimal();
    let r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.paths().map(|p| p.unwrap()).collect::<Vec<_>>(), vec!["a"]);
    assert_eq!(r.read("a").unwrap(), Vec::<u8>::new());
    assert_eq!(r.segment_count(), 1);
}

#[test]
fn pos_reserved_field_all_ones_still_opens() {
    let base = minimal().bytes();
    // set the 56 reserved bytes (72..128) to 0xFF
    let mutated = patch(base.clone(), hoff::RESERVED, &[0xFF; 56]);
    let f = write_wax(&mutated);
    let r = WaxReader::open(&f.path).unwrap();
    // identical behaviour to the zero-reserved twin
    assert_eq!(r.read("a").unwrap(), Vec::<u8>::new());
}

#[test]
fn pos_unicode_paths_roundtrip_and_sort() {
    let entries = vec![
        EntryInput::data("科学/index.html", "内容".as_bytes().to_vec(), Compression::Zstd),
        EntryInput::data("emoji/🚀.txt", "rocket".as_bytes().to_vec(), Compression::None),
        EntryInput::data("café/ résumé.txt", b"x".to_vec(), Compression::None),
    ];
    let f = valid(entries);
    let r = WaxReader::open(&f.path).unwrap();
    let paths: Vec<String> = r.paths().map(|p| p.unwrap()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "list() is code-point sorted");
    assert_eq!(r.read("科学/index.html").unwrap(), "内容".as_bytes());
    assert_eq!(r.read("emoji/🚀.txt").unwrap(), b"rocket");
}

#[test]
fn pos_redirect_depth_one_resolves() {
    // author declares alias -> target; builder flattens (already depth 1 here)
    let f = valid(vec![
        EntryInput::data("real.html", b"<h1>hi</h1>".to_vec(), Compression::None),
        EntryInput::redirect("alias.html", "real.html"),
    ]);
    let r = WaxReader::open(&f.path).unwrap();
    // both paths visible
    assert!(r.paths().any(|p| p.unwrap() == "alias.html"));
    assert!(r.paths().any(|p| p.unwrap() == "real.html"));
    // resolve + read follow the hop
    let resolved = r.resolve("alias.html").unwrap();
    assert_eq!(resolved.entry.path, "real.html");
    assert_eq!(r.read("alias.html").unwrap(), b"<h1>hi</h1>");
}

#[test]
fn pos_builder_flattens_declared_redirect_chain() {
    // A -> B -> C declared; on disk must be A -> C and B -> C (SPEC §6.2)
    let f = valid(vec![
        EntryInput::data("c.html", b"C".to_vec(), Compression::None),
        EntryInput::redirect("b.html", "c.html"),
        EntryInput::redirect("a.html", "b.html"),
    ]);
    let r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.entry("a.html").unwrap().redirect_to.as_deref(), Some("c.html"));
    assert_eq!(r.entry("b.html").unwrap().redirect_to.as_deref(), Some("c.html"));
}

#[test]
fn pos_unknown_minor_version_and_extra_column() {
    // craft a valid single-segment archive, then bump minor to 0xFF and add a
    // junk column to `entries`.
    let blob = b"hello".to_vec();
    let rows = [row_data("p.txt", 128, &blob, "none")];
    let seg = craft_segment(&rows, blob.len() as u64);

    // add junk column by rebuilding the segment db through sqlite
    let seg = add_junk_column(&seg);

    let bytes = assemble_single(&blob, &seg, |h| {
        h.version_minor = 0xFF;
    });
    let f = write_wax(&bytes);
    let r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.header().version_minor, 0xFF);
    assert_eq!(r.read("p.txt").unwrap(), b"hello");
}

#[test]
fn pos_multi_segment_last_wins() {
    // base: a=1, b=1 ; append: a=2, c=2   => a resolves to "2", b to "1", c to "2"
    let f = valid_multi(
        vec![
            EntryInput::data("a", b"one".to_vec(), Compression::None),
            EntryInput::data("b", b"one".to_vec(), Compression::None),
        ],
        vec![vec![
            EntryInput::data("a", b"two".to_vec(), Compression::None),
            EntryInput::data("c", b"two".to_vec(), Compression::None),
        ]],
    );
    let r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.segment_count(), 2);
    assert_eq!(r.read("a").unwrap(), b"two", "later segment wins");
    assert_eq!(r.read("b").unwrap(), b"one", "untouched entry from base");
    assert_eq!(r.read("c").unwrap(), b"two", "new entry from append");
}

#[test]
fn pos_multi_segment_append_turns_entry_into_redirect() {
    let f = valid_multi(
        vec![
            EntryInput::data("page", b"old".to_vec(), Compression::None),
            EntryInput::data("canonical", b"canon".to_vec(), Compression::None),
        ],
        vec![vec![EntryInput::redirect("page", "canonical")]],
    );
    let r = WaxReader::open(&f.path).unwrap();
    let res = r.resolve("page").unwrap();
    assert_eq!(res.entry.path, "canonical");
    assert_eq!(r.read("page").unwrap(), b"canon");
}

// ===========================================================================
// §11.2  negative corpus — must reject cleanly (Err, never panic)
// ===========================================================================

fn expect_open_err(bytes: &[u8]) -> WaxError {
    let f = write_wax(bytes);
    WaxReader::open(&f.path).expect_err("archive should have been rejected")
}

#[test]
fn neg_bad_magic() {
    let bytes = patch(minimal().bytes(), hoff::MAGIC, b"WAX2");
    assert!(matches!(expect_open_err(&bytes), WaxError::BadMagic { .. }));
}

#[test]
fn neg_truncated_header() {
    let f = write_wax(&[0u8; 100]);
    let e = WaxReader::open(&f.path).unwrap_err();
    assert!(matches!(e, WaxError::TruncatedHeader { .. }));
}

#[test]
fn neg_unsupported_major_version() {
    let bytes = patch(minimal().bytes(), hoff::VERSION_MAJOR, &[1]);
    assert!(matches!(
        expect_open_err(&bytes),
        WaxError::UnsupportedMajorVersion { found: 1 }
    ));
}

#[test]
fn neg_index_offset_in_header() {
    let bytes = patch(minimal().bytes(), hoff::INDEX_OFFSET, &le64(0));
    assert!(matches!(
        expect_open_err(&bytes),
        WaxError::IndexOffsetInHeader { .. }
    ));
}

#[test]
fn neg_index_out_of_bounds() {
    let bytes = patch(minimal().bytes(), hoff::INDEX_LENGTH, &le64(1 << 40));
    assert!(matches!(
        expect_open_err(&bytes),
        WaxError::IndexOutOfBounds { .. }
    ));
}

#[test]
fn neg_index_too_small() {
    let bytes = patch(minimal().bytes(), hoff::INDEX_LENGTH, &le64(100));
    assert!(matches!(
        expect_open_err(&bytes),
        WaxError::IndexTooSmall { found: 100 }
    ));
}

#[test]
fn neg_blob_section_length_mismatch_single_segment() {
    let bytes = patch(minimal().bytes(), hoff::BLOB_SECTION_LENGTH, &le64(99999));
    assert!(matches!(
        expect_open_err(&bytes),
        WaxError::BlobSectionLengthMismatch { .. }
    ));
}

#[test]
fn neg_blob_section_length_mismatch_multi_segment() {
    let f = valid_multi(
        vec![EntryInput::data("a", b"one".to_vec(), Compression::None)],
        vec![vec![EntryInput::data("b", b"two".to_vec(), Compression::None)]],
    );
    let bytes = patch(f.bytes(), hoff::BLOB_SECTION_LENGTH, &le64(123));
    assert!(matches!(
        expect_open_err(&bytes),
        WaxError::BlobSectionLengthMismatch { .. }
    ));
}

#[test]
fn neg_index_segment_not_sqlite() {
    let base = minimal().bytes();
    // overwrite the index region with 0xFF, same length
    let r = WaxReader::open(&write_wax(&base).path).unwrap();
    let off = r.header().index_offset as usize;
    let len = r.header().index_length as usize;
    drop(r);
    let mut bytes = base.clone();
    for b in &mut bytes[off..off + len] {
        *b = 0xFF;
    }
    let e = expect_open_err(&bytes);
    assert!(
        matches!(e, WaxError::NotAnIndexSegment { .. } | WaxError::Schema { .. }),
        "got {e:?}"
    );
}

#[test]
fn neg_segment_missing_entries_table() {
    // segment with only segment_meta, no `entries`
    let seg = craft_segment_meta(
        &[],
        &[
            ("format", "wax-index-segment"),
            ("segment_index", "0"),
            ("blob_region_offset", "128"),
            ("blob_region_length", "0"),
            ("created_at", "1700000000"),
        ],
    );
    // craft_segment_meta still creates the `entries` table (build_segment_db_with_meta
    // always does). To truly drop it we drop the table via sqlite:
    let seg = drop_entries_table(&seg);
    let bytes = assemble_single(&[], &seg, |_| {});
    assert!(matches!(expect_open_err(&bytes), WaxError::Schema { .. }));
}

#[test]
fn neg_prev_segment_out_of_bounds() {
    let blob = b"z".to_vec();
    let rows = [row_data("p", 128, &blob, "none")];
    let seg = craft_segment_meta(
        &rows,
        &[
            ("format", "wax-index-segment"),
            ("segment_index", "1"),
            ("blob_region_offset", "128"),
            ("blob_region_length", "1"),
            ("created_at", "1700000000"),
            ("prev_segment_offset", "999999999"),
            ("prev_segment_length", "4096"),
        ],
    );
    let bytes = assemble_single(&blob, &seg, |h| {
        // header points at this (segment 1); blob_section_length must still match
        h.blob_section_length = 1;
    });
    assert!(matches!(
        expect_open_err(&bytes),
        WaxError::PrevSegmentOutOfBounds { .. }
    ));
}

#[test]
fn neg_segment_chain_cycle() {
    // Two segments whose prev pointers reference each other.
    // Layout: [header][blobA][segA][blobB][segB], header -> segB, segB.prev -> segA,
    // segA.prev -> segB  (the cycle).
    let blob_a = b"a".to_vec();
    let blob_b = b"b".to_vec();

    // We need offsets before we can write the meta, so lay out sizes first with
    // placeholder segments, then rebuild once offsets are known.
    let seg_a_tmp = craft_segment_meta(&[row_data("a", 0, &blob_a, "none")], &[("format", "wax-index-segment")]);
    let off_seg_a = HEADER_LEN + blob_a.len();
    let off_blob_b = off_seg_a + seg_a_tmp.len();
    let off_seg_b = off_blob_b + blob_b.len();

    let seg_a = craft_segment_meta(
        &[row_data("a", (HEADER_LEN) as u64, &blob_a, "none")],
        &[
            ("format", "wax-index-segment"),
            ("segment_index", "0"),
            ("blob_region_offset", &HEADER_LEN.to_string()),
            ("blob_region_length", &blob_a.len().to_string()),
            ("created_at", "1700000000"),
            ("prev_segment_offset", &off_seg_b.to_string()),
            ("prev_segment_length", "4096"),
        ],
    );
    // recompute offsets with the real seg_a length
    let off_blob_b = off_seg_a + seg_a.len();
    let off_seg_b = off_blob_b + blob_b.len();
    let seg_b = craft_segment_meta(
        &[row_data("b", off_blob_b as u64, &blob_b, "none")],
        &[
            ("format", "wax-index-segment"),
            ("segment_index", "1"),
            ("blob_region_offset", &off_blob_b.to_string()),
            ("blob_region_length", &blob_b.len().to_string()),
            ("created_at", "1700000000"),
            ("prev_segment_offset", &off_seg_a.to_string()),
            ("prev_segment_length", &seg_a.len().to_string()),
        ],
    );

    let mut bytes = Vec::new();
    let mut header = wax_core::header::WaxHeader {
        archive_uuid: TEST_UUID,
        created_at: 1_700_000_000,
        index_offset: off_seg_b as u64,
        index_length: seg_b.len() as u64,
        blob_section_length: (blob_a.len() + blob_b.len()) as u64,
        ..wax_core::header::WaxHeader::default()
    };
    header.version_minor = 9;
    bytes.extend_from_slice(&header.to_bytes());
    bytes.extend_from_slice(&blob_a);
    bytes.extend_from_slice(&seg_a);
    bytes.extend_from_slice(&blob_b);
    bytes.extend_from_slice(&seg_b);

    let e = expect_open_err(&bytes);
    assert!(
        matches!(e, WaxError::SegmentChainCycle { .. } | WaxError::PrevSegmentOutOfBounds { .. }),
        "got {e:?}"
    );
}

#[test]
fn neg_on_disk_redirect_chain_depth_two() {
    // entries a->b, b->c, c real  (builder would have flattened; we bypass it)
    let blob = b"C".to_vec();
    let rows = [
        row_redirect("a", "b"),
        row_redirect("b", "c"),
        row_data("c", 128, &blob, "none"),
    ];
    let seg = craft_segment(&rows, blob.len() as u64);
    let bytes = assemble_single(&blob, &seg, |_| {});
    let f = write_wax(&bytes);
    let r = WaxReader::open(&f.path).unwrap(); // opens fine
    let e = r.resolve("a").unwrap_err();
    assert!(matches!(e, WaxError::RedirectChainTooDeep { .. }), "got {e:?}");
    // reading also errors, never follows
    assert!(matches!(
        r.read("a").unwrap_err(),
        WaxError::RedirectChainTooDeep { .. }
    ));
}

#[test]
fn neg_dangling_redirect() {
    let seg = craft_segment(&[row_redirect("a", "nope")], 0);
    let bytes = assemble_single(&[], &seg, |_| {});
    let f = write_wax(&bytes);
    let r = WaxReader::open(&f.path).unwrap();
    assert!(matches!(
        r.resolve("a").unwrap_err(),
        WaxError::DanglingRedirect { .. }
    ));
}

#[test]
fn neg_unexpected_volume_id() {
    let blob = b"x".to_vec();
    let mut row = row_data("p", 128, &blob, "none");
    row.volume_id = 1;
    let seg = craft_segment(&[row], blob.len() as u64);
    let bytes = assemble_single(&blob, &seg, |_| {});
    assert!(matches!(
        expect_open_err(&bytes),
        WaxError::UnexpectedVolumeId { found: 1, .. }
    ));
}

#[test]
fn neg_unknown_compression() {
    let blob = b"raw-bytes".to_vec();
    let mut row = row_data("p", 128, &blob, "brotli");
    row.sha256 = None;
    let seg = craft_segment(&[row], blob.len() as u64);
    let bytes = assemble_single(&blob, &seg, |_| {});
    let f = write_wax(&bytes);
    let r = WaxReader::open(&f.path).unwrap(); // opens fine
    assert!(matches!(
        r.read("p").unwrap_err(),
        WaxError::UnknownCompression { .. }
    ));
}

#[test]
fn neg_checksum_mismatch_on_corrupted_blob() {
    let f = valid(vec![EntryInput::data(
        "p.txt",
        b"the original bytes".to_vec(),
        Compression::None,
    )]);
    let mut bytes = f.bytes();
    // flip one byte inside the blob region (just after the header)
    bytes[HEADER_LEN + 2] ^= 0xFF;
    let f2 = write_wax(&bytes);
    let r = WaxReader::open(&f2.path).unwrap();
    let e = r.read("p.txt").unwrap_err();
    assert!(
        matches!(e, WaxError::ChecksumMismatch { .. } | WaxError::Decompress { .. }),
        "got {e:?}"
    );
}

// ===========================================================================
// helpers that need raw sqlite surgery
// ===========================================================================

fn add_junk_column(seg_bytes: &[u8]) -> Vec<u8> {
    with_sqlite(seg_bytes, |conn| {
        conn.execute("ALTER TABLE entries ADD COLUMN junk_v2 TEXT DEFAULT 'ignored'", [])
            .unwrap();
        conn.execute("UPDATE entries SET junk_v2 = 'x'", []).unwrap();
    })
}

fn drop_entries_table(seg_bytes: &[u8]) -> Vec<u8> {
    with_sqlite(seg_bytes, |conn| {
        conn.execute("DROP TABLE entries", []).unwrap();
    })
}

fn with_sqlite(seg_bytes: &[u8], f: impl FnOnce(&rusqlite::Connection)) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("seg.db");
    std::fs::write(&p, seg_bytes).unwrap();
    {
        let conn = rusqlite::Connection::open(&p).unwrap();
        f(&conn);
        conn.execute("VACUUM", []).unwrap();
        conn.close().unwrap();
    }
    std::fs::read(&p).unwrap()
}
