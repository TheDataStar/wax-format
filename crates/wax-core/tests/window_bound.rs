//! SPEC §3, "Decoder memory bound": every `zstd` / `zstd-dict` frame declares a
//! window of at most 8 MiB (or is a single-segment frame of at most 8 MiB).
//! Checked on the bytes the writer actually puts on disk, for an entry large
//! enough that an unbounded window would show.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use wax_core::model::Compression;
use wax_core::writer::{EntryMeta, WaxWriter};
use wax_core::WaxReader;

const UUID: [u8; 16] = [0x12, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 9];
const BOUND: u64 = 8 * 1024 * 1024;

/// Window size a frame header declares (RFC 8878 §3.1.1.1). Returns the
/// content size for a single-segment frame, where no window descriptor exists.
fn declared_window(frame: &[u8]) -> u64 {
    assert_eq!(&frame[..4], &[0x28, 0xB5, 0x2F, 0xFD], "zstd magic");
    let fhd = frame[4];
    let single_segment = fhd & 0x20 != 0;
    if !single_segment {
        let wd = frame[5];
        let exponent = (wd >> 3) as u32;
        let mantissa = (wd & 7) as u64;
        let base = 1u64 << (10 + exponent);
        return base + (base / 8) * mantissa;
    }
    // Single segment: Window_Size = Frame_Content_Size.
    let fcs_flag = fhd >> 6;
    let did_flag = fhd & 3;
    let did_len = [0usize, 1, 2, 4][did_flag as usize];
    let at = 5 + did_len;
    match fcs_flag {
        0 => frame[at] as u64,
        1 => u16::from_le_bytes([frame[at], frame[at + 1]]) as u64 + 256,
        2 => u32::from_le_bytes(frame[at..at + 4].try_into().unwrap()) as u64,
        _ => u64::from_le_bytes(frame[at..at + 8].try_into().unwrap()),
    }
}

#[test]
fn writer_frames_respect_the_8_mib_window_bound() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("big.wax");
    // 12 MiB of varied text: more than the bound, so a writer with an
    // unbounded window would declare one larger than 8 MiB.
    let mut big = Vec::with_capacity(12 << 20);
    let mut x: u32 = 1;
    while big.len() < 12 << 20 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        big.extend_from_slice(format!("<p>row {} value {}</p>\n", x >> 8, x % 977).as_bytes());
    }
    let mut sw = WaxWriter::new(UUID)
        .created_at(1_700_000_000)
        .create(&out, &BTreeMap::from([("name".to_string(), "w".to_string())]))
        .unwrap();
    sw.add_entry(EntryMeta::new("big.html").mime("text/html").compression(Compression::Zstd), &mut &big[..])
        .unwrap();
    sw.add_entry(EntryMeta::new("small.html").mime("text/html").compression(Compression::Zstd), &mut &b"<p>hi</p>"[..])
        .unwrap();
    sw.finish().unwrap();

    let r = WaxReader::open(&out).unwrap();
    let mut f = std::fs::File::open(&out).unwrap();
    for e in r.entries() {
        let e = e.unwrap();
        let mut head = vec![0u8; 18.min(e.length as usize)];
        f.seek(SeekFrom::Start(e.offset)).unwrap();
        f.read_exact(&mut head).unwrap();
        let w = declared_window(&head);
        assert!(w <= BOUND, "{}: declared window {} > 8 MiB", e.path, w);
    }
    assert_eq!(r.read("big.html").unwrap(), big);

    // Negative control: the parser does see an over-bound window. A window
    // log of 25 (what "ultra" level 20 selects) declares 32 MiB.
    let mut enc = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
    enc.window_log(25).unwrap();
    std::io::Write::write_all(&mut enc, &big).unwrap();
    let ultra = enc.finish().unwrap();
    assert!(declared_window(&ultra) > BOUND, "negative control: window log 25 must exceed the bound");
}

/// ADR-0003: off-box builds use level 19, the highest level that stays inside
/// the bound. Checked on disk, for both codecs, and the writer refuses the
/// "ultra" levels that would break it.
#[test]
fn level_19_frames_respect_the_bound_and_ultra_levels_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("l19.wax");
    let mut big = Vec::with_capacity(12 << 20);
    let mut x: u32 = 7;
    while big.len() < 12 << 20 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        big.extend_from_slice(format!("<p>row {} value {}</p>\n", x >> 8, x % 977).as_bytes());
    }
    let mut sw = WaxWriter::new(UUID)
        .created_at(1_700_000_000)
        .zstd_level(19)
        .create(&out, &BTreeMap::from([("name".to_string(), "w".to_string())]))
        .unwrap();
    assert_eq!(sw.zstd_level(), 19);
    let samples: Vec<Vec<u8>> = (0..64).map(|i| format!("<p>sample {i} of the text</p>").repeat(20).into_bytes()).collect();
    sw.train_dictionary(samples.iter().map(|s| s.as_slice())).unwrap();
    sw.add_entry(EntryMeta::new("big.html").mime("text/html").compression(Compression::Zstd), &mut &big[..])
        .unwrap();
    sw.add_entry(EntryMeta::new("dict.html").mime("text/html").compression(Compression::ZstdDict), &mut &big[..4 << 20])
        .unwrap();
    sw.finish().unwrap();

    let r = WaxReader::open(&out).unwrap();
    let mut f = std::fs::File::open(&out).unwrap();
    for e in r.entries() {
        let e = e.unwrap();
        let mut head = vec![0u8; 18.min(e.length as usize)];
        f.seek(SeekFrom::Start(e.offset)).unwrap();
        f.read_exact(&mut head).unwrap();
        assert!(declared_window(&head) <= BOUND, "{}: level 19 window over 8 MiB", e.path);
    }
    assert_eq!(r.read("big.html").unwrap(), big);
    assert_eq!(r.read("dict.html").unwrap(), &big[..4 << 20]);

    for bad in [0, 20, 22, -1] {
        let res = WaxWriter::new(UUID).zstd_level(bad).create(dir.path().join(format!("bad{bad}.wax")), &BTreeMap::new());
        assert!(res.is_err(), "level {bad} must be refused");
    }
}
