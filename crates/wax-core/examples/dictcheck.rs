//! Pre-flight for Directive 05: is zstd dictionary training deterministic?
//!
//! The whole v0.10 design rests on this. If training the same corpus twice
//! yields two different dictionaries, every entry reframes, the determinism
//! anchor breaks, and the delta advantage that made arm 3 win evaporates.
//! A non-deterministic dictionary is a defect to find now, not to ship.
//!
//! Checks three things:
//!   1. same samples, same order, same process  -> identical dictionary?
//!   2. same samples, repeated many times       -> still identical?
//!   3. does sample ORDER change the result?    -> tells us whether the
//!      builder must pin an order (it should anyway, but we need to know)
//!
//! usage: dictcheck <pack.wax> [--dict-bytes N] [--sample-count N] [--rounds N]

use wax_core::WaxReader;

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b);
    h.finalize().iter().map(|x| format!("{x:02x}")).collect()
}

fn is_text(mime: Option<&str>) -> bool {
    match mime {
        Some(m) => m.starts_with("text/") || m.ends_with("+xml"),
        None => false,
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
    let pack = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .ok_or("usage: dictcheck <pack.wax>")?
        .clone();
    let dict_bytes = arg(&args, "--dict-bytes", 110 * 1024);
    let sample_count = arg(&args, "--sample-count", 2048usize);
    let rounds = arg(&args, "--rounds", 5usize);

    let r = WaxReader::open(&pack)?;
    let total = r.entry_count()? as usize;
    let stride = (total / sample_count.max(1)).max(1);

    // Collect in the reader's own order. entries() walks the path primary key,
    // so this order is a property of the pack, not of this run.
    let mut samples: Vec<Vec<u8>> = Vec::new();
    for (i, e) in r.entries().enumerate() {
        let e = e?;
        if e.is_redirect() || i % stride != 0 || !is_text(e.mime.as_deref()) {
            continue;
        }
        if let Ok(b) = r.read(&e.path) {
            if !b.is_empty() {
                samples.push(b);
            }
        }
        if samples.len() >= sample_count {
            break;
        }
    }
    println!("samples: {} ({} bytes)", samples.len(), samples.iter().map(|s| s.len()).sum::<usize>());

    // --- 1 & 2: repeat training, identical input ---
    let mut hashes = Vec::new();
    for n in 0..rounds {
        let d = zstd::dict::from_samples(&samples, dict_bytes)?;
        let h = sha256_hex(&d);
        println!("  round {n}: {} bytes  sha256={}", d.len(), &h[..16]);
        hashes.push(h);
    }
    let stable = hashes.windows(2).all(|w| w[0] == w[1]);
    println!(
        "REPEATABLE (same input, same order): {}",
        if stable { "YES" } else { "NO -- DEFECT" }
    );

    // --- 3: does sample order matter? ---
    let mut reordered = samples.clone();
    reordered.reverse();
    let d_fwd = zstd::dict::from_samples(&samples, dict_bytes)?;
    let d_rev = zstd::dict::from_samples(&reordered, dict_bytes)?;
    let same = sha256_hex(&d_fwd) == sha256_hex(&d_rev);
    println!(
        "ORDER-INDEPENDENT: {} (forward vs reversed sample order)",
        if same { "YES" } else { "NO -- the builder must pin sample order" }
    );

    // --- what a reader actually needs: does the dict round-trip content? ---
    let mut c = zstd::bulk::Compressor::with_dictionary(3, &d_fwd)?;
    let dd = zstd::dict::DecoderDictionary::copy(&d_fwd);
    let mut ok = 0usize;
    let mut checked = 0usize;
    for s in samples.iter().take(500) {
        let framed = c.compress(s)?;
        let mut d = zstd::bulk::Decompressor::with_prepared_dictionary(&dd)?;
        let back = d.decompress(&framed, s.len() * 4 + 1024)?;
        checked += 1;
        if back == *s {
            ok += 1;
        }
    }
    println!("ROUND-TRIP: {ok}/{checked} entries recovered byte-exact");
    Ok(())
}
