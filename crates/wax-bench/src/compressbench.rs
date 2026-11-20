//! compressbench — the compression measurement behind Directive 04 (second run).
//!
//! ADR-0001 settled the *framing*: one zstd frame per entry, against a shared
//! dictionary (`zstd-dict`). It measured at level 3 only and left four things
//! open that this harness measures on identical input:
//!
//!   1. the zstd level (SPEC fixes none; `wax-core` hard-codes 3),
//!   2. the dictionary size (`wax-core` hard-codes 110 KiB),
//!   3. whether another codec (brotli, xz) would beat zstd per entry,
//!   4. grouping only the *small* entries (ADR-0001 rejected grouping all),
//!   5. whether media, stored verbatim today, would gain from any codec.
//!
//! Everything is read back out of a built pack, decoded to plain bytes, and
//! re-encoded here. Nothing touches the format. Timings are single-threaded,
//! best of `--reps` passes, on whatever machine runs this — they are relative,
//! not a statement about the Pi.
//!
//! usage: compressbench <pack.wax> [--reps N] [--quick]

use std::io::Read;
use std::time::{Duration, Instant};
use wax_core::WaxReader;

const KIB: usize = 1024;

// The dictionary training rule zim2wax uses (convert.rs): entry-path order,
// at most 2048 samples, each truncated to 64 KiB, at most 16 MiB in total.
const DICT_SAMPLE_MAX: usize = 2048;
const DICT_SAMPLE_BYTES: usize = 16 * 1024 * KIB;
const DICT_SAMPLE_EACH: usize = 64 * KIB;

struct Item {
    mime: String,
    body: Vec<u8>,
}

/// Same split as zim2wax `compression_for`: text and SVG are compressed,
/// images and fonts are stored verbatim.
fn is_text(mime: &str) -> bool {
    if mime == "image/svg+xml" {
        return true;
    }
    !(mime.starts_with("image/")
        || mime.starts_with("font/")
        || mime.starts_with("application/font")
        || mime.starts_with("application/x-font")
        || mime == "application/vnd.ms-fontobject"
        || mime.starts_with("audio/")
        || mime.starts_with("video/"))
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn train(texts: &[&Item], dict_bytes: usize) -> Vec<u8> {
    let mut samples: Vec<&[u8]> = Vec::new();
    let mut total = 0usize;
    for t in texts {
        if samples.len() >= DICT_SAMPLE_MAX || total >= DICT_SAMPLE_BYTES {
            break;
        }
        let s = &t.body[..t.body.len().min(DICT_SAMPLE_EACH)];
        total += s.len();
        samples.push(s);
    }
    zstd::dict::from_samples(&samples, dict_bytes).expect("dictionary training")
}

/// A codec under test: encode one buffer, decode one buffer.
#[derive(Clone)]
enum Codec {
    Zstd { level: i32, dict: Option<std::sync::Arc<Vec<u8>>> },
    Brotli { quality: i32, lgwin: i32 },
    Xz { preset: u32 },
}

impl Codec {
    fn name(&self) -> String {
        match self {
            Codec::Zstd { level, dict: None } => format!("zstd-{level}"),
            Codec::Zstd { level, dict: Some(d) } => format!("zstd-{level} + {} KiB dict", d.len() / KIB),
            Codec::Brotli { quality, lgwin } => format!("brotli q{quality} w{lgwin}"),
            Codec::Xz { preset } => {
                if preset & 0x8000_0000 != 0 {
                    format!("xz -{}e", preset & 0xff)
                } else {
                    format!("xz -{preset}")
                }
            }
        }
    }

    fn dict_len(&self) -> usize {
        match self {
            Codec::Zstd { dict: Some(d), .. } => d.len(),
            _ => 0,
        }
    }

    fn encode_all(&self, bufs: &[&[u8]]) -> (Vec<Vec<u8>>, Duration) {
        let t0 = Instant::now();
        let out = match self {
            Codec::Zstd { level, dict } => {
                let mut c = match dict {
                    Some(d) => zstd::bulk::Compressor::with_dictionary(*level, d).unwrap(),
                    None => zstd::bulk::Compressor::new(*level).unwrap(),
                };
                // Content size in the frame header, as wax-core's streaming
                // encoder does not write it; the difference is a few bytes.
                bufs.iter().map(|b| c.compress(b).unwrap()).collect()
            }
            Codec::Brotli { quality, lgwin } => bufs
                .iter()
                .map(|b| {
                    let p = brotli::enc::BrotliEncoderParams {
                        quality: *quality,
                        lgwin: *lgwin,
                        ..Default::default()
                    };
                    let mut out = Vec::new();
                    brotli::BrotliCompress(&mut &b[..], &mut out, &p).unwrap();
                    out
                })
                .collect(),
            Codec::Xz { preset } => bufs
                .iter()
                .map(|b| {
                    let mut out = Vec::new();
                    xz2::read::XzEncoder::new(&b[..], *preset).read_to_end(&mut out).unwrap();
                    out
                })
                .collect(),
        };
        (out, t0.elapsed())
    }

    /// Decode every frame once; return per-frame durations.
    fn decode_all(&self, frames: &[Vec<u8>], raw_lens: &[usize]) -> Vec<Duration> {
        let max = raw_lens.iter().copied().max().unwrap_or(0);
        let mut buf: Vec<u8> = Vec::with_capacity(max + 64);
        let mut times = Vec::with_capacity(frames.len());
        match self {
            Codec::Zstd { dict, .. } => {
                // A prepared dictionary, created once per open pack — what the
                // wax-core reader does (DecoderDictionary at open).
                let prepared = dict.as_ref().map(|d| zstd::dict::DecoderDictionary::copy(d));
                let mut d = match &prepared {
                    Some(p) => zstd::bulk::Decompressor::with_prepared_dictionary(p).unwrap(),
                    None => zstd::bulk::Decompressor::new().unwrap(),
                };
                for (f, &n) in frames.iter().zip(raw_lens) {
                    buf.clear();
                    let t = Instant::now();
                    let got = d.decompress_to_buffer(&f[..], &mut buf).unwrap();
                    times.push(t.elapsed());
                    assert_eq!(got, n);
                }
            }
            Codec::Brotli { .. } => {
                for (f, &n) in frames.iter().zip(raw_lens) {
                    buf.clear();
                    let t = Instant::now();
                    brotli::BrotliDecompress(&mut &f[..], &mut buf).unwrap();
                    times.push(t.elapsed());
                    assert_eq!(buf.len(), n);
                }
            }
            Codec::Xz { .. } => {
                for (f, &n) in frames.iter().zip(raw_lens) {
                    buf.clear();
                    let t = Instant::now();
                    xz2::read::XzDecoder::new(&f[..]).read_to_end(&mut buf).unwrap();
                    times.push(t.elapsed());
                    assert_eq!(buf.len(), n);
                }
            }
        }
        times
    }
}

struct Row {
    name: String,
    bytes: u64,
    dict: u64,
    raw: u64,
    enc_mbs: f64,
    dec_mbs: f64,
    mean_us: f64,
    p50_us: f64,
    p99_us: f64,
}

fn measure(codec: &Codec, bufs: &[&[u8]], reps: usize) -> Row {
    let raw_lens: Vec<usize> = bufs.iter().map(|b| b.len()).collect();
    let raw: u64 = raw_lens.iter().map(|&n| n as u64).sum();
    let (frames, enc) = codec.encode_all(bufs);
    let bytes: u64 = frames.iter().map(|f| f.len() as u64).sum();
    // Best of `reps` for each frame: removes scheduler noise, keeps the shape.
    let mut best: Vec<Duration> = vec![Duration::MAX; frames.len()];
    for _ in 0..reps.max(1) {
        for (b, t) in best.iter_mut().zip(codec.decode_all(&frames, &raw_lens)) {
            *b = (*b).min(t);
        }
    }
    let total: Duration = best.iter().sum();
    let mut us: Vec<f64> = best.iter().map(|d| d.as_secs_f64() * 1e6).collect();
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| us[((us.len() as f64 - 1.0) * p).round() as usize];
    Row {
        name: codec.name(),
        bytes,
        dict: codec.dict_len() as u64,
        raw,
        enc_mbs: raw as f64 / 1e6 / enc.as_secs_f64(),
        dec_mbs: raw as f64 / 1e6 / total.as_secs_f64(),
        mean_us: total.as_secs_f64() * 1e6 / frames.len() as f64,
        p50_us: pct(0.5),
        p99_us: pct(0.99),
    }
}

fn header() {
    println!("| codec | bytes | + dict | ratio (incl. dict) | vs zstd-3+dict | encode MB/s | decode MB/s | mean µs | p50 µs | p99 µs |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
}

fn print_row(r: &Row, base: u64) {
    let tot = r.bytes + r.dict;
    println!(
        "| {} | {} | {} | {:.4} | {:+.1}% | {:.1} | {:.0} | {:.1} | {:.1} | {:.1} |",
        r.name,
        r.bytes,
        r.dict,
        tot as f64 / r.raw as f64,
        (tot as f64 / base as f64 - 1.0) * 100.0,
        r.enc_mbs,
        r.dec_mbs,
        r.mean_us,
        r.p50_us,
        r.p99_us
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pack = match args.first() {
        Some(a) if !a.starts_with("--") => a.clone(),
        _ => return Err("usage: compressbench <pack.wax> [--reps N] [--quick]".into()),
    };
    let reps: usize = arg(&args, "--reps", 3);
    let quick = args.iter().any(|a| a == "--quick");

    // ---- load: every non-redirect entry, decoded, in path order ----
    let r = WaxReader::open(&pack)?;
    let mut items: Vec<Item> = Vec::new();
    for e in r.entries() {
        let e = e?;
        if e.is_redirect() || e.compression == "tombstone" {
            continue;
        }
        let body = r.read(&e.path)?;
        if body.is_empty() {
            continue;
        }
        let mime = e.mime.clone().unwrap_or_default();
        let mime = mime.split(';').next().unwrap_or("").trim().to_string();
        items.push(Item { mime, body });
    }
    let texts: Vec<&Item> = items.iter().filter(|i| is_text(&i.mime)).collect();
    let media: Vec<&Item> = items.iter().filter(|i| !is_text(&i.mime)).collect();
    let text_bufs: Vec<&[u8]> = texts.iter().map(|i| &i.body[..]).collect();
    let text_raw: u64 = text_bufs.iter().map(|b| b.len() as u64).sum();
    let mut sizes: Vec<usize> = text_bufs.iter().map(|b| b.len()).collect();
    sizes.sort();
    let q = |p: f64| sizes[((sizes.len() as f64 - 1.0) * p).round() as usize];

    println!("# compressbench — {}", pack);
    println!();
    println!(
        "entries with content: {} (text {}, media {}); text raw {} bytes; media raw {} bytes",
        items.len(),
        texts.len(),
        media.len(),
        text_raw,
        media.iter().map(|i| i.body.len() as u64).sum::<u64>()
    );
    println!(
        "text entry size: p10 {} · p50 {} · p90 {} · p99 {} · max {} bytes; decode timings best of {} passes, single thread",
        q(0.1),
        q(0.5),
        q(0.9),
        q(0.99),
        sizes[sizes.len() - 1],
        reps
    );
    println!();

    let d110 = std::sync::Arc::new(train(&texts, 110 * KIB));
    let base = {
        let r = measure(&Codec::Zstd { level: 3, dict: Some(d110.clone()) }, &text_bufs, reps);
        r.bytes + r.dict
    };

    // ---- 1. zstd level sweep, with and without the dictionary ----
    println!("## 1. zstd level, text entries (one frame per entry)");
    println!();
    header();
    let levels: &[i32] = if quick { &[1, 3, 19] } else { &[1, 3, 6, 9, 12, 15, 17, 19, 22] };
    for &dict in &[false, true] {
        for &level in levels {
            let c = Codec::Zstd { level, dict: dict.then(|| d110.clone()) };
            print_row(&measure(&c, &text_bufs, reps), base);
        }
    }
    println!();

    // ---- 2. dictionary size ----
    println!("## 2. dictionary size (trained by the zim2wax rule), text entries");
    println!();
    header();
    let dsizes: &[usize] = if quick { &[64, 110, 256] } else { &[16, 32, 64, 110, 256, 512] };
    for &level in &[3, 19] {
        for &ds in dsizes {
            let d = std::sync::Arc::new(train(&texts, ds * KIB));
            let c = Codec::Zstd { level, dict: Some(d) };
            print_row(&measure(&c, &text_bufs, reps), base);
        }
    }
    println!();

    // ---- 2b. dictionary size, held out ----
    // A dictionary is carried forward unchanged to every later edition
    // (SPEC 5.9), so it will meet text it was not trained on. Train on the
    // even-indexed entries, measure on the odd ones: an over-fitted big
    // dictionary shows up here as a smaller gain than in section 2.
    println!("## 2b. dictionary size, held out (train on even entries, measure odd), text entries");
    println!();
    header();
    let even: Vec<&Item> = texts.iter().step_by(2).copied().collect();
    let odd: Vec<&[u8]> = texts.iter().skip(1).step_by(2).map(|i| &i.body[..]).collect();
    let odd_base = {
        let d = std::sync::Arc::new(train(&even, 110 * KIB));
        let r = measure(&Codec::Zstd { level: 3, dict: Some(d) }, &odd, 1);
        r.bytes + r.dict
    };
    for &level in &[3, 19] {
        for &ds in dsizes {
            let d = std::sync::Arc::new(train(&even, ds * KIB));
            print_row(&measure(&Codec::Zstd { level, dict: Some(d) }, &odd, reps), odd_base);
        }
    }
    println!();
    println!("(in 2b, the vs column is against zstd-3 + 110 KiB trained on the even half)");
    println!();

    // ---- 3. other codecs, per entry ----
    println!("## 3. other codecs, per entry, text entries (no trained dictionary)");
    println!();
    println!("brotli carries its own built-in web-text dictionary; xz has none.");
    println!();
    header();
    let others: Vec<Codec> = if quick {
        vec![Codec::Brotli { quality: 11, lgwin: 22 }, Codec::Xz { preset: 6 }]
    } else {
        vec![
            Codec::Brotli { quality: 5, lgwin: 22 },
            Codec::Brotli { quality: 9, lgwin: 22 },
            Codec::Brotli { quality: 11, lgwin: 22 },
            Codec::Xz { preset: 6 },
            Codec::Xz { preset: 9 | 0x8000_0000 },
        ]
    };
    for c in &others {
        print_row(&measure(c, &text_bufs, reps), base);
    }
    println!();

    // ---- 4. grouping only small entries ----
    println!("## 4. grouping small text entries into blocks (large entries stay one frame each)");
    println!();
    println!("`read µs` is the cost to reach one small article: its whole block must be decoded.");
    println!("Delta cost is the ADR-0001 concern: one changed article re-ships its whole block.");
    println!();
    println!("| level | small = under | block | frames | bytes + dict | vs zstd-3+dict | read µs (small article, mean) | read µs (per-entry, same level) |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    for &level in &[3, 19] {
        let pe = measure(&Codec::Zstd { level, dict: Some(d110.clone()) }, &text_bufs, reps);
        for &thresh in &[4 * KIB, 16 * KIB, usize::MAX] {
            for &block in &[64 * KIB, 256 * KIB, 1024 * KIB] {
                let mut bufs: Vec<Vec<u8>> = Vec::new();
                let mut is_block: Vec<bool> = Vec::new();
                let mut cur: Vec<u8> = Vec::new();
                for b in &text_bufs {
                    if b.len() >= thresh {
                        bufs.push(b.to_vec());
                        is_block.push(false);
                        continue;
                    }
                    cur.extend_from_slice(b);
                    if cur.len() >= block {
                        bufs.push(std::mem::take(&mut cur));
                        is_block.push(true);
                    }
                }
                if !cur.is_empty() {
                    bufs.push(cur);
                    is_block.push(true);
                }
                let refs: Vec<&[u8]> = bufs.iter().map(|b| &b[..]).collect();
                let c = Codec::Zstd { level, dict: Some(d110.clone()) };
                let row = measure(&c, &refs, reps);
                // Block-decode time averaged over the blocks only: the cost
                // every small-article read now pays (best of `reps`).
                let (frames, _) = c.encode_all(&refs);
                let lens: Vec<usize> = refs.iter().map(|b| b.len()).collect();
                let mut best = vec![Duration::MAX; frames.len()];
                for _ in 0..reps.max(1) {
                    for (b, t) in best.iter_mut().zip(c.decode_all(&frames, &lens)) {
                        *b = (*b).min(t);
                    }
                }
                let (sm, n) = best
                    .iter()
                    .zip(&is_block)
                    .filter(|(_, &blk)| blk)
                    .fold((Duration::ZERO, 0u32), |(s, n), (t, _)| (s + *t, n + 1));
                let read_us = if n > 0 { sm.as_secs_f64() * 1e6 / n as f64 } else { f64::NAN };
                let tot = row.bytes + row.dict;
                println!(
                    "| {} | {} | {} KiB | {} | {} | {:+.1}% | {:.1} | {:.1} |",
                    level,
                    if thresh == usize::MAX { "all".to_string() } else { format!("{} KiB", thresh / KIB) },
                    block / KIB,
                    bufs.len(),
                    tot,
                    (tot as f64 / base as f64 - 1.0) * 100.0,
                    read_us,
                    pe.mean_us
                );
            }
        }
    }
    println!();

    // ---- 5. media ----
    println!("## 5. media (stored verbatim today), by mimetype");
    println!();
    let mut mimes: Vec<String> = media.iter().map(|i| i.mime.clone()).collect();
    mimes.sort();
    mimes.dedup();
    let mcodecs = [
        Codec::Zstd { level: 3, dict: None },
        Codec::Zstd { level: 19, dict: None },
        Codec::Brotli { quality: 11, lgwin: 22 },
        Codec::Xz { preset: 9 | 0x8000_0000 },
    ];
    print!("| mimetype | entries | raw bytes |");
    for c in &mcodecs {
        print!(" {} |", c.name());
    }
    println!();
    print!("|---|---:|---:|");
    for _ in &mcodecs {
        print!("---:|");
    }
    println!();
    for m in &mimes {
        let bufs: Vec<&[u8]> = media.iter().filter(|i| &i.mime == m).map(|i| &i.body[..]).collect();
        let raw: u64 = bufs.iter().map(|b| b.len() as u64).sum();
        print!("| {} | {} | {} |", m, bufs.len(), raw);
        for c in &mcodecs {
            let (f, _) = c.encode_all(&bufs);
            // A writer would keep the smaller of compressed and verbatim per entry.
            let best: u64 = f.iter().zip(&bufs).map(|(f, b)| f.len().min(b.len()) as u64).sum();
            print!(" {:+.2}% |", (best as f64 / raw as f64 - 1.0) * 100.0);
        }
        println!();
    }
    println!();
    println!("(media cells: size change if each entry keeps the smaller of codec output and verbatim)");
    println!();

    // ---- 6. plain-HTTP serving: what the box does per page request ----
    // Browsers advertise zstd/br only over HTTPS, so a plain-HTTP visitor gets
    // either identity bytes or gzip. Stored frames can never pass through: the
    // box decodes, then optionally re-encodes as gzip for the wire.
    println!("## 6. plain-HTTP serve path, per text entry (decode stored frame, then optional gzip for the wire)");
    println!();
    println!("| stored as | decode µs (mean) | + gzip-1 µs | + gzip-6 µs | wire bytes identity | wire bytes gzip-1 | wire bytes gzip-6 |");
    println!("|---|---:|---:|---:|---:|---:|---:|");
    for &level in &[3, 19] {
        let c = Codec::Zstd { level, dict: Some(d110.clone()) };
        let dec = measure(&c, &text_bufs, reps);
        let gz = |lvl: u32| {
            use std::io::Write;
            let mut best = Duration::MAX;
            let mut bytes = 0u64;
            for _ in 0..reps.max(1) {
                bytes = 0;
                let t = Instant::now();
                for b in &text_bufs {
                    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(lvl));
                    e.write_all(b).unwrap();
                    bytes += e.finish().unwrap().len() as u64;
                }
                best = best.min(t.elapsed());
            }
            (best.as_secs_f64() * 1e6 / text_bufs.len() as f64, bytes)
        };
        let (g1, b1) = gz(1);
        let (g6, b6) = gz(6);
        println!(
            "| zstd-{} + 110 KiB dict | {:.1} | {:.1} | {:.1} | {} | {} | {} |",
            level, dec.mean_us, g1, g6, text_raw, b1, b6
        );
    }
    Ok(())
}
