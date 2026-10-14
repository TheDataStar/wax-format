//! Directive 04 — measure the four candidate blob-framing codecs.
//!
//! Reads a built pack, pulls every entry back out uncompressed, and re-frames
//! those bytes four ways to answer one question: what does per-entry framing
//! cost in size, and does a shared dictionary buy it back?
//!
//!   1. per-entry          — v0.9 today. One zstd frame per entry.
//!   2. grouped            — ZIM-style. Entries concatenated into blocks,
//!                           one frame per block.
//!   3. per-entry + dict   — one frame per entry, against a dictionary
//!                           trained on the corpus and stored once.
//!   4. grouped + dict     — the absolute-smallest reference.
//!
//! Nothing here touches the format. It measures framing strategies over
//! identical input so the codec decision rests on numbers.
//!
//! usage: codecbench <pack.wax> [--block-bytes N] [--dict-bytes N]
//!                              [--level N] [--sample-count N] [--decode-sample N]

use std::time::Instant;
use wax_core::WaxReader;

const MIB: usize = 1024 * 1024;

fn is_text(mime: Option<&str>) -> bool {
    match mime {
        Some(m) => {
            m.starts_with("text/")
                || m.starts_with("application/json")
                || m.starts_with("application/javascript")
                || m.starts_with("application/xml")
                || m.ends_with("+xml")
        }
        None => false,
    }
}

#[derive(Default, Clone)]
struct Tally {
    total: u64,
    text: u64,
    frames: u64,
}

impl Tally {
    fn add(&mut self, bytes: usize, text: bool) {
        self.total += bytes as u64;
        self.frames += 1;
        if text {
            self.text += bytes as u64;
        }
    }
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pack = match args.first() {
        Some(a) if !a.starts_with("--") => a.clone(),
        _ => return Err("usage: codecbench <pack.wax> [--block-bytes N] [--dict-bytes N] [--level N]".into()),
    };

    let block_bytes = arg(&args, "--block-bytes", MIB);
    let dict_bytes = arg(&args, "--dict-bytes", 110 * 1024);
    let level: i32 = arg(&args, "--level", 3);
    let sample_count = arg(&args, "--sample-count", 4096usize);
    let decode_sample = arg(&args, "--decode-sample", 200usize);

    let r = WaxReader::open(&pack)?;
    let total_entries = r.entry_count()?;
    eprintln!(
        "pack={} entries={} block={}KiB dict={}KiB level={}",
        pack,
        total_entries,
        block_bytes / 1024,
        dict_bytes / 1024,
        level
    );

    // ---- pass 1: dictionary training samples, spread across the corpus ----
    let stride = (total_entries as usize / sample_count.max(1)).max(1);
    let mut samples: Vec<Vec<u8>> = Vec::new();
    let mut sample_bytes = 0usize;
    for (i, e) in r.entries().enumerate() {
        let e = e?;
        if e.is_redirect() || i % stride != 0 || !is_text(e.mime.as_deref()) {
            continue;
        }
        if let Ok(b) = r.read(&e.path) {
            if !b.is_empty() {
                sample_bytes += b.len();
                samples.push(b);
            }
        }
        if samples.len() >= sample_count || sample_bytes > 64 * MIB {
            break;
        }
    }
    eprintln!("dict training: {} samples, {} bytes", samples.len(), sample_bytes);

    let dict: Vec<u8> = if samples.len() >= 8 {
        match zstd::dict::from_samples(&samples, dict_bytes) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("  dictionary training failed: {e}; dict arms skipped");
                Vec::new()
            }
        }
    } else {
        eprintln!("  too few text samples to train a dictionary");
        Vec::new()
    };
    eprintln!("dict bytes: {}", dict.len());
    drop(samples);

    // ---- pass 2: stream every entry through all four arms ----
    let mut a1 = Tally::default();
    let mut a2 = Tally::default();
    let mut a3 = Tally::default();
    let mut a4 = Tally::default();

    let mut blk: Vec<u8> = Vec::with_capacity(block_bytes + 64 * 1024);
    let mut blk_text = true;
    let mut raw_total = 0u64;
    let mut raw_text = 0u64;
    let mut n_entries = 0u64;

    let mut c_plain = zstd::bulk::Compressor::new(level)?;
    let mut c_dict = if dict.is_empty() {
        None
    } else {
        Some(zstd::bulk::Compressor::with_dictionary(level, &dict)?)
    };

    let t0 = Instant::now();
    for e in r.entries() {
        let e = e?;
        if e.is_redirect() {
            continue;
        }
        let body = match r.read(&e.path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if body.is_empty() {
            continue;
        }
        let text = is_text(e.mime.as_deref());
        raw_total += body.len() as u64;
        n_entries += 1;
        if text {
            raw_text += body.len() as u64;
        }

        a1.add(c_plain.compress(&body)?.len(), text);
        if let Some(c) = c_dict.as_mut() {
            a3.add(c.compress(&body)?.len(), text);
        }

        blk.extend_from_slice(&body);
        blk_text &= text;
        if blk.len() >= block_bytes {
            a2.add(c_plain.compress(&blk)?.len(), blk_text);
            if let Some(c) = c_dict.as_mut() {
                a4.add(c.compress(&blk)?.len(), blk_text);
            }
            blk.clear();
            blk_text = true;
        }
    }
    if !blk.is_empty() {
        a2.add(c_plain.compress(&blk)?.len(), blk_text);
        if let Some(c) = c_dict.as_mut() {
            a4.add(c.compress(&blk)?.len(), blk_text);
        }
    }
    let elapsed = t0.elapsed();

    // ---- single-article decode cost ----
    // Per-entry arms inflate one frame. Grouped arms must inflate a whole
    // block to reach one article; that difference is the point of measuring.
    let mut paths: Vec<String> = Vec::new();
    let dstride = (total_entries as usize / decode_sample.max(1)).max(1);
    for (i, e) in r.entries().enumerate() {
        let e = e?;
        if !e.is_redirect() && is_text(e.mime.as_deref()) && i % dstride == 0 {
            paths.push(e.path.clone());
        }
        if paths.len() >= decode_sample {
            break;
        }
    }

    let mut one_entry_ns = 0u128;
    let mut one_block_ns = 0u128;
    let mut one_entry_dict_ns = 0u128;
    let mut n_dec = 0u32;
    let ddict = zstd::dict::DecoderDictionary::copy(&dict);
    let mut blockbuf: Vec<u8> = Vec::with_capacity(block_bytes + 64 * 1024);
    for p in &paths {
        let body = match r.read(p) {
            Ok(b) if !b.is_empty() => b,
            _ => continue,
        };
        let framed = c_plain.compress(&body)?;
        let t = Instant::now();
        let _ = zstd::bulk::decompress(&framed, body.len() * 4 + 1024)?;
        one_entry_ns += t.elapsed().as_nanos();

        // Arm 3 read path: same one-frame decode, but against the dictionary.
        // This is the constraint check -- a dictionary must not cost read speed.
        if let Some(c) = c_dict.as_mut() {
            let framed_d = c.compress(&body)?;
            let mut dd = zstd::bulk::Decompressor::with_prepared_dictionary(&ddict)?;
            let t = Instant::now();
            let _ = dd.decompress(&framed_d, body.len() * 4 + 1024)?;
            one_entry_dict_ns += t.elapsed().as_nanos();
        }

        blockbuf.clear();
        while blockbuf.len() < block_bytes {
            blockbuf.extend_from_slice(&body);
        }
        let fblock = c_plain.compress(&blockbuf)?;
        let t = Instant::now();
        let _ = zstd::bulk::decompress(&fblock, blockbuf.len() + 1024)?;
        one_block_ns += t.elapsed().as_nanos();
        n_dec += 1;
    }

    // ---- report ----
    let zim = std::env::var("CODECBENCH_ZIM_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    println!("## {}", pack);
    println!("entries_compressed = {}", n_entries);
    println!("raw_total_bytes    = {}", raw_total);
    println!("raw_text_bytes     = {}", raw_text);
    println!("dict_bytes         = {}", dict.len());
    println!("bench_seconds      = {:.1}", elapsed.as_secs_f64());
    if let Some(z) = zim {
        println!("zim_bytes          = {}", z);
    }
    println!();
    println!(
        "{:<22} {:>14} {:>14} {:>8} {:>8} {:>8}",
        "arm", "total_bytes", "text_bytes", "frames", "vs_raw", "vs_zim"
    );
    for (name, t) in [
        ("1 per-entry", &a1),
        ("2 grouped", &a2),
        ("3 per-entry+dict", &a3),
        ("4 grouped+dict", &a4),
    ] {
        if t.total == 0 {
            println!("{:<22} {:>14}", name, "(skipped)");
            continue;
        }
        // The dictionary ships inside the pack once, so dict arms carry it.
        let extra = if name.contains("dict") { dict.len() as u64 } else { 0 };
        let tot = t.total + extra;
        println!(
            "{:<22} {:>14} {:>14} {:>8} {:>8.3} {:>8}",
            name,
            tot,
            t.text,
            t.frames,
            tot as f64 / raw_total as f64,
            zim.map(|z| format!("{:.3}", tot as f64 / z as f64))
                .unwrap_or_else(|| "-".into())
        );
    }
    println!();
    if n_dec > 0 {
        println!(
            "single-article decode (n={}): per-entry={:.1}us  per-entry+dict={:.1}us  whole-block={:.1}us  block/entry={:.1}x",
            n_dec,
            one_entry_ns as f64 / n_dec as f64 / 1000.0,
            one_entry_dict_ns as f64 / n_dec as f64 / 1000.0,
            one_block_ns as f64 / n_dec as f64 / 1000.0,
            one_block_ns as f64 / one_entry_ns.max(1) as f64
        );
    }
    Ok(())
}
