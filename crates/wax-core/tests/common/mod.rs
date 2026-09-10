//! Shared fixture helpers for the A4 conformance suite (SPEC §11).
//!
//! Two families of builders:
//!   * `valid_*` — go through the real [`WaxWriter`], produce spec-correct files.
//!   * `craft_*` / `assemble_*` — bypass the writer to place arbitrary (often
//!     deliberately malformed) bytes on disk.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;
use wax_core::header::WaxHeader;
use wax_core::writer::{build_segment_db_with_meta, SegRow};
use wax_core::{Compression, EntryInput, WaxWriter};
pub use wax_core::HEADER_LEN;

pub const TEST_UUID: [u8; 16] = *b"WAXCONFORMANCE\0\0";

/// A scratch dir + a path inside it. Keep the returned value alive for the
/// duration of the test.
pub struct Fixture {
    pub dir: TempDir,
    pub path: PathBuf,
}

impl Fixture {
    pub fn bytes(&self) -> Vec<u8> {
        std::fs::read(&self.path).unwrap()
    }
}

pub fn new_fixture(name: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    Fixture { dir, path }
}

/// Write raw bytes to a fresh `.wax` file.
pub fn write_wax(bytes: &[u8]) -> Fixture {
    let f = new_fixture("crafted.wax");
    std::fs::write(&f.path, bytes).unwrap();
    f
}

// ---------------------------------------------------------------------------
// valid archives (real WaxWriter)
// ---------------------------------------------------------------------------

pub fn valid(entries: Vec<EntryInput>) -> Fixture {
    valid_with_manifest(entries, BTreeMap::new())
}

pub fn valid_with_manifest(entries: Vec<EntryInput>, manifest: BTreeMap<String, String>) -> Fixture {
    let f = new_fixture("valid.wax");
    WaxWriter::new(TEST_UUID)
        .created_at(1_700_000_000)
        .build(&f.path, entries, &manifest)
        .unwrap();
    f
}

/// A valid multi-segment archive: `build` then one `append` per extra vec.
pub fn valid_multi(base: Vec<EntryInput>, appends: Vec<Vec<EntryInput>>) -> Fixture {
    let f = new_fixture("multi.wax");
    let w = WaxWriter::new(TEST_UUID).created_at(1_700_000_000);
    w.build(&f.path, base, &BTreeMap::new()).unwrap();
    for extra in appends {
        w.append(&f.path, extra).unwrap();
    }
    f
}

/// The minimum-size positive fixture: one zero-byte entry, stored uncompressed.
pub fn minimal() -> Fixture {
    valid(vec![EntryInput::data("a", Vec::new(), Compression::None)])
}

// ---------------------------------------------------------------------------
// byte surgery
// ---------------------------------------------------------------------------

pub fn patch(mut bytes: Vec<u8>, offset: usize, new: &[u8]) -> Vec<u8> {
    bytes[offset..offset + new.len()].copy_from_slice(new);
    bytes
}

/// Header field offsets (SPEC §2).
pub mod hoff {
    pub const MAGIC: usize = 0;
    pub const VERSION_MAJOR: usize = 4;
    pub const VERSION_MINOR: usize = 5;
    pub const FLAGS: usize = 6;
    pub const CREATED_AT: usize = 24;
    pub const INDEX_OFFSET: usize = 32;
    pub const INDEX_LENGTH: usize = 40;
    pub const BLOB_SECTION_LENGTH: usize = 48;
    pub const RESERVED: usize = 72;
}

// ---------------------------------------------------------------------------
// crafting arbitrary single-segment archives
// ---------------------------------------------------------------------------

/// Assemble `[header][blob][segment]` with the header's pointer fields computed
/// to match (`blob_section_length == index_offset - 128`). `mutate` gets the
/// last say over the header before it is serialized.
pub fn assemble_single(
    blob: &[u8],
    segment_db: &[u8],
    mutate: impl FnOnce(&mut WaxHeader),
) -> Vec<u8> {
    let index_offset = HEADER_LEN as u64 + blob.len() as u64;
    let mut header = WaxHeader {
        archive_uuid: TEST_UUID,
        created_at: 1_700_000_000,
        index_offset,
        index_length: segment_db.len() as u64,
        blob_section_length: blob.len() as u64,
        ..WaxHeader::default()
    };
    mutate(&mut header);
    let mut out = Vec::new();
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(blob);
    out.extend_from_slice(segment_db);
    out
}

/// A well-formed single segment holding exactly `rows`, blob region
/// `[128, 128+blob_len)`.
pub fn craft_segment(rows: &[SegRow], blob_len: u64) -> Vec<u8> {
    let meta = vec![
        ("format".to_string(), "wax-index-segment".to_string()),
        ("segment_index".to_string(), "0".to_string()),
        ("blob_region_offset".to_string(), (HEADER_LEN as u64).to_string()),
        ("blob_region_length".to_string(), blob_len.to_string()),
        ("created_at".to_string(), "1700000000".to_string()),
    ];
    build_segment_db_with_meta(rows, &meta, Some(&BTreeMap::new())).unwrap()
}

/// A segment with fully custom `segment_meta` pairs (for chain-linkage fixtures).
pub fn craft_segment_meta(rows: &[SegRow], meta: &[(&str, &str)]) -> Vec<u8> {
    let owned: Vec<(String, String)> = meta
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    build_segment_db_with_meta(rows, &owned, None).unwrap()
}

// row builders -------------------------------------------------------------

pub fn row_data(path: &str, offset: u64, blob: &[u8], compression: &str) -> SegRow {
    use sha2::{Digest, Sha256};
    // sha256 over *uncompressed* content; for "none" that's the blob itself.
    let sha = if compression == "none" {
        Some(Sha256::digest(blob).to_vec())
    } else {
        None
    };
    SegRow {
        path: path.to_string(),
        title: None,
        offset: offset as i64,
        length: blob.len() as i64,
        uncompressed_length: blob.len() as i64,
        mime: Some("application/octet-stream".to_string()),
        compression: compression.to_string(),
        sha256: sha,
        volume_id: 0,
        redirect_to: None,
    }
}

pub fn row_redirect(path: &str, to: &str) -> SegRow {
    SegRow {
        path: path.to_string(),
        title: None,
        offset: 0,
        length: 0,
        uncompressed_length: 0,
        mime: None,
        compression: "none".to_string(),
        sha256: None,
        volume_id: 0,
        redirect_to: Some(to.to_string()),
    }
}

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

pub fn le64(v: u64) -> [u8; 8] {
    v.to_le_bytes()
}

/// Write bytes into a fixture directory next to the archive (e.g. a `.minisig`).
pub fn sidecar(f: &Fixture, ext: &str, bytes: &[u8]) -> PathBuf {
    let p = f.path.with_extension(ext);
    let mut file = std::fs::File::create(&p).unwrap();
    file.write_all(bytes).unwrap();
    p
}
