//! A2e — the streaming writer (Track A §18).
//!
//! The Vec-based API is a wrapper over [`StreamingWriter`], so the whole
//! conformance suite already exercises the streaming core; these tests pin
//! the properties that are specific to streaming: readers instead of buffers,
//! bounded working memory, equivalence with the Vec path, SQL-side redirect
//! flattening, and the append protocol on the streaming path.

mod common;

use common::*;
use std::collections::BTreeMap;
use std::io::{self, Read};
use wax_core::writer::EntryMeta;
use wax_core::{Compression, EntryInput, WaxError, WaxReader, WaxWriter};

fn writer() -> WaxWriter {
    WaxWriter::new(TEST_UUID).created_at(1_700_000_000)
}

/// A `Read` that yields `len` deterministic bytes without ever holding them.
struct Synthetic {
    remaining: u64,
    seed: u64,
}

impl Synthetic {
    fn new(len: u64, seed: u64) -> Self {
        Synthetic {
            remaining: len,
            seed,
        }
    }
}

impl Read for Synthetic {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = (self.remaining.min(buf.len() as u64)) as usize;
        for b in &mut buf[..n] {
            // xorshift; text-ish bytes so zstd has something to do
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            *b = b'a' + (self.seed % 26) as u8;
        }
        self.remaining -= n as u64;
        Ok(n)
    }
}

#[test]
fn entries_stream_from_a_reader_and_read_back_exactly() {
    let f = new_fixture("s.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    let a = w
        .add_entry(
            EntryMeta::new("a.txt").mime("text/plain").compression(Compression::Zstd),
            &mut io::Cursor::new(b"hello streaming".to_vec()),
        )
        .unwrap();
    let b = w
        .add_entry(
            EntryMeta::new("b.bin").compression(Compression::None),
            &mut io::Cursor::new(vec![7u8; 1000]),
        )
        .unwrap();
    let st = w.finish().unwrap();
    assert_eq!(st.entries, 2);
    assert_eq!(a.uncompressed_length, 15);
    assert_eq!(b.length, 1000, "stored verbatim");
    assert_eq!(b.uncompressed_length, 1000);
    assert!(b.offset > a.offset);

    let mut r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.read("a.txt").unwrap(), b"hello streaming");
    assert_eq!(r.read("b.bin").unwrap(), vec![7u8; 1000]);
    assert_eq!(r.entry("a.txt").unwrap().mime.as_deref(), Some("text/plain"));
    // sha256 recorded over the uncompressed bytes and verified on read (SPEC §3.1)
    assert_eq!(r.entry("a.txt").unwrap().sha256, Some(a.sha256));
}

#[test]
fn a_blob_larger_than_the_stream_buffer_is_never_materialized() {
    // 8 MiB through a 64 KiB buffer; the source itself never allocates it.
    let f = new_fixture("big.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    let len = 8 * 1024 * 1024;
    let st = w
        .add_entry(
            EntryMeta::new("big").compression(Compression::Zstd),
            &mut Synthetic::new(len, 0x9E37_79B9),
        )
        .unwrap();
    assert_eq!(st.uncompressed_length, len);
    assert!(st.length < len, "zstd should compress the synthetic text");
    w.finish().unwrap();
    // reads back with the checksum verified
    let mut r = WaxReader::open(&f.path).unwrap();
    let mut expect = Vec::new();
    Synthetic::new(len, 0x9E37_79B9).read_to_end(&mut expect).unwrap();
    assert_eq!(r.read("big").unwrap(), expect);
}

#[test]
fn streaming_and_vec_builds_are_byte_identical() {
    let entries = vec![
        EntryInput::data("index.html", b"<h1>x</h1>".to_vec(), Compression::Zstd).with_title("Home"),
        EntryInput::data("img.png", vec![1, 2, 3], Compression::None).with_mime("image/png"),
        EntryInput::redirect("alias.html", "index.html"),
        EntryInput::redirect("alias2.html", "alias.html"),
    ];
    let mut manifest = BTreeMap::new();
    manifest.insert("name".to_string(), "P".to_string());

    let a = new_fixture("vec.wax");
    writer().build(&a.path, entries.clone(), &manifest).unwrap();

    let b = new_fixture("stream.wax");
    let mut w = writer().create(&b.path, &manifest).unwrap();
    for e in entries {
        match e.content {
            wax_core::EntryContent::Data { bytes, compression } => {
                let mut m = EntryMeta::new(e.path).compression(compression);
                if let Some(x) = e.mime {
                    m = m.mime(x);
                }
                if let Some(t) = e.title {
                    m = m.title(t);
                }
                w.add_entry(m, &mut io::Cursor::new(bytes)).unwrap();
            }
            wax_core::EntryContent::Redirect { to } => w.add_redirect(e.path, to, e.title).unwrap(),
        }
    }
    w.finish().unwrap();

    assert_eq!(a.bytes(), b.bytes(), "same entries, same order, same pins → same bytes");
}

#[test]
fn redirect_chains_are_flattened_in_sql_at_finish() {
    let f = new_fixture("chain.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    w.add_entry(EntryMeta::new("real"), &mut io::Cursor::new(b"R".to_vec())).unwrap();
    // declared in the worst order for a naive resolver: deepest first
    w.add_redirect("d", "c", None).unwrap();
    w.add_redirect("c", "b", None).unwrap();
    w.add_redirect("b", "a", None).unwrap();
    w.add_redirect("a", "real", None).unwrap();
    let st = w.finish().unwrap();
    assert_eq!(st.redirects, 4);
    let mut r = WaxReader::open(&f.path).unwrap();
    for alias in ["a", "b", "c", "d"] {
        assert_eq!(r.entry(alias).unwrap().redirect_to.as_deref(), Some("real"), "{alias}");
        assert_eq!(r.read(alias).unwrap(), b"R");
    }
}

#[test]
fn a_long_redirect_chain_flattens_in_logarithmic_rounds() {
    // 2000 hops: pointer jumping needs ~11 rounds; a one-hop-per-round scheme
    // would need 2000 and the 64-round cap would misreport it as a cycle.
    let f = new_fixture("long.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    w.add_entry(EntryMeta::new("t"), &mut io::Cursor::new(b"T".to_vec())).unwrap();
    for i in 0..2000 {
        let to = if i == 0 { "t".to_string() } else { format!("r{}", i - 1) };
        w.add_redirect(format!("r{i}"), to, None).unwrap();
    }
    w.finish().unwrap();
    let r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.entry("r1999").unwrap().redirect_to.as_deref(), Some("t"));
}

#[test]
fn cycles_and_dangling_are_the_same_errors_as_before() {
    let f = new_fixture("cyc.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    w.add_entry(EntryMeta::new("real"), &mut io::Cursor::new(b"R".to_vec())).unwrap();
    w.add_redirect("a", "b", None).unwrap();
    w.add_redirect("b", "a", None).unwrap();
    let e = w.finish().unwrap_err();
    assert!(matches!(e, WaxError::RedirectChainTooDeep { .. }), "{e:?}");

    let f = new_fixture("dang.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    w.add_entry(EntryMeta::new("real"), &mut io::Cursor::new(b"R".to_vec())).unwrap();
    w.add_redirect("a", "nope", None).unwrap();
    let e = w.finish().unwrap_err();
    assert!(matches!(e, WaxError::DanglingRedirect { ref from, ref to } if from == "a" && to == "nope"), "{e:?}");

    let f = new_fixture("self.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    w.add_entry(EntryMeta::new("real"), &mut io::Cursor::new(b"R".to_vec())).unwrap();
    w.add_redirect("me", "me", None).unwrap();
    assert!(matches!(w.finish().unwrap_err(), WaxError::RedirectChainTooDeep { .. }));
}

#[test]
fn duplicate_paths_are_rejected_at_insert() {
    let f = new_fixture("dup.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    w.add_entry(EntryMeta::new("x"), &mut io::Cursor::new(b"1".to_vec())).unwrap();
    let e = w.add_entry(EntryMeta::new("/x"), &mut io::Cursor::new(b"2".to_vec())).unwrap_err();
    assert!(matches!(e, WaxError::Schema { ref detail } if detail.contains("duplicate path")), "{e:?}");
}

#[test]
fn has_path_is_a_point_lookup_in_the_index() {
    let f = new_fixture("hp.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    assert!(!w.has_path("a").unwrap());
    w.add_entry(EntryMeta::new("a"), &mut io::Cursor::new(b"A".to_vec())).unwrap();
    assert!(w.has_path("a").unwrap());
    assert!(w.has_path("/a").unwrap(), "normalized before lookup (leading slash stripped)");
    assert!(!w.has_path("b").unwrap());
    w.add_redirect("b", "a", None).unwrap();
    assert!(w.has_path("b").unwrap());
    w.finish().unwrap();
}

#[test]
fn streaming_append_follows_the_commit_protocol() {
    let f = new_fixture("app.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    w.add_entry(EntryMeta::new("base"), &mut io::Cursor::new(b"B".to_vec())).unwrap();
    w.finish().unwrap();
    let before = f.bytes();

    let mut a = writer().created_at(1_700_000_100).open_append(&f.path).unwrap();
    a.add_entry(EntryMeta::new("added"), &mut io::Cursor::new(b"A".to_vec())).unwrap();
    // a redirect in the append may target the base segment
    a.add_redirect("alias", "base", None).unwrap();
    a.finish().unwrap();
    let after = f.bytes();

    // §7: nothing already written changes except the 128-byte header
    assert_eq!(&before[128..], &after[128..before.len()], "prior bytes untouched");
    assert_ne!(&before[..128], &after[..128], "header rewritten");

    let mut r = WaxReader::open(&f.path).unwrap();
    assert_eq!(r.segment_count(), 2);
    assert_eq!(r.read("base").unwrap(), b"B");
    assert_eq!(r.read("added").unwrap(), b"A");
    assert_eq!(r.read("alias").unwrap(), b"B");
}

#[test]
fn signable_digest_of_matches_the_reader_without_loading_entries() {
    let f = new_fixture("dig.wax");
    let mut w = writer().create(&f.path, &BTreeMap::new()).unwrap();
    for i in 0..50 {
        w.add_entry(EntryMeta::new(format!("e{i}")), &mut io::Cursor::new(vec![i as u8; 10])).unwrap();
    }
    w.finish().unwrap();
    let (lazy, uuid, created) = wax_core::reader::signable_digest_of(&f.path).unwrap();
    let mut r = WaxReader::open(&f.path).unwrap();
    assert_eq!(lazy, r.signable_digest().unwrap());
    assert_eq!(uuid, TEST_UUID);
    assert_eq!(created, 1_700_000_000);
}
