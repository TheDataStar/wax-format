//! Directive 04 — measure what each codec arm costs on an *update*.
//!
//! WAX's headline feature is that a new edition ships only the entries that
//! changed. That property is a function of framing: with one frame per entry
//! only changed entries move, but with grouped blocks a single changed article
//! re-frames its whole block and everything sharing it.
//!
//! Takes two consecutive editions of the same archive and reports, per arm,
//! how many bytes an update would carry.
//!
//!   1. per-entry        — changed entries only.
//!   3. per-entry + dict — changed entries only, dictionary-compressed. The
//!                         dictionary is trained on the OLD edition and reused,
//!                         which is the realistic case: a dictionary that is
//!                         retrained every edition would invalidate every frame.
//!   2. grouped          — every block containing at least one changed entry.
//!   4. grouped + dict   — same, dictionary-compressed.
//!
//! usage: deltabench <old.wax> <new.wax> [--block-bytes N] [--dict-bytes N] [--level N]
//!        deltabench <pack.wax> <pack.wax> --churn-pct P
//!
//! `--churn-pct` replaces the content comparison with a synthetic edit set:
//! every Nth entry is treated as changed. Real consecutive editions answer
//! "what does a real update cost"; a churn sweep answers the different and
//! sharper question of *where grouping starts to lose*, by holding the content
//! fixed and varying only how much of it moved.

use std::collections::HashMap;
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

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if positional.len() < 2 {
        return Err("usage: deltabench <old.wax> <new.wax> [--block-bytes N] [--dict-bytes N]".into());
    }
    let (old_path, new_path) = (positional[0].clone(), positional[1].clone());

    let block_bytes = arg(&args, "--block-bytes", MIB);
    let dict_bytes = arg(&args, "--dict-bytes", 110 * 1024);
    let level: i32 = arg(&args, "--level", 3);
    let sample_count = arg(&args, "--sample-count", 4096usize);
    let churn_pct: f64 = arg(&args, "--churn-pct", -1.0f64);

    let old = WaxReader::open(&old_path)?;
    let new = WaxReader::open(&new_path)?;
    eprintln!(
        "old={} ({} entries)  new={} ({} entries)",
        old_path,
        old.entry_count()?,
        new_path,
        new.entry_count()?
    );

    // Content identity comes from the stored sha256, so "changed" means the
    // bytes changed -- not merely that the entry was rewritten.
    let mut old_sha: HashMap<String, [u8; 32]> = HashMap::new();
    for e in old.entries() {
        let e = e?;
        if let Some(s) = e.sha256 {
            old_sha.insert(e.path.clone(), s);
        }
    }
    eprintln!("indexed {} old entries by content hash", old_sha.len());

    // Train the dictionary on the OLD edition and reuse it for the new one.
    // Retraining per edition would change every frame and destroy the delta.
    let old_total = old.entry_count()? as usize;
    let stride = (old_total / sample_count.max(1)).max(1);
    let mut samples: Vec<Vec<u8>> = Vec::new();
    let mut sbytes = 0usize;
    for (i, e) in old.entries().enumerate() {
        let e = e?;
        if e.is_redirect() || i % stride != 0 || !is_text(e.mime.as_deref()) {
            continue;
        }
        if let Ok(b) = old.read(&e.path) {
            if !b.is_empty() {
                sbytes += b.len();
                samples.push(b);
            }
        }
        if samples.len() >= sample_count || sbytes > 64 * MIB {
            break;
        }
    }
    let dict = if samples.len() >= 8 {
        zstd::dict::from_samples(&samples, dict_bytes).unwrap_or_default()
    } else {
        Vec::new()
    };
    eprintln!("dict trained on OLD edition: {} bytes from {} samples", dict.len(), samples.len());
    drop(samples);

    let mut c_plain = zstd::bulk::Compressor::new(level)?;
    let mut c_dict = if dict.is_empty() {
        None
    } else {
        Some(zstd::bulk::Compressor::with_dictionary(level, &dict)?)
    };

    // Walk the new edition in path order, assigning entries to blocks exactly
    // as a grouped writer would, and track which blocks a changed entry dirties.
    let mut d1 = 0u64; // per-entry delta
    let mut d3 = 0u64; // per-entry + dict delta
    let mut d2 = 0u64; // grouped delta
    let mut d4 = 0u64; // grouped + dict delta

    let mut full1 = 0u64; // full new edition, per-entry, for context
    let mut full3 = 0u64;

    let mut changed = 0u64;
    let mut unchanged = 0u64;
    let mut added = 0u64;

    let mut blk: Vec<u8> = Vec::with_capacity(block_bytes + 64 * 1024);
    let mut blk_dirty = false;
    let mut blocks_total = 0u64;
    let mut blocks_dirty = 0u64;

    let mut seen = 0u64;
    for e in new.entries() {
        let e = e?;
        if e.is_redirect() {
            continue;
        }
        let body = match new.read(&e.path) {
            Ok(b) if !b.is_empty() => b,
            _ => continue,
        };
        seen += 1;

        if churn_pct >= 0.0 {
            // Synthetic edit set: content held fixed, only the changed-set varies.
            let every = if churn_pct <= 0.0 { u64::MAX } else { (100.0 / churn_pct).round() as u64 };
            let is_changed = every != u64::MAX && seen % every.max(1) == 0;
            if is_changed {
                changed += 1;
            } else {
                unchanged += 1;
            }
            let c1 = c_plain.compress(&body)?.len() as u64;
            full1 += c1;
            if is_changed {
                d1 += c1;
            }
            if let Some(c) = c_dict.as_mut() {
                let c3 = c.compress(&body)?.len() as u64;
                full3 += c3;
                if is_changed {
                    d3 += c3;
                }
            }
            blk.extend_from_slice(&body);
            blk_dirty |= is_changed;
            if blk.len() >= block_bytes {
                blocks_total += 1;
                let b2 = c_plain.compress(&blk)?.len() as u64;
                let b4 = match c_dict.as_mut() {
                    Some(c) => c.compress(&blk)?.len() as u64,
                    None => 0,
                };
                if blk_dirty {
                    blocks_dirty += 1;
                    d2 += b2;
                    d4 += b4;
                }
                blk.clear();
                blk_dirty = false;
            }
            continue;
        }

        let is_changed = match (old_sha.get(&e.path), e.sha256) {
            (Some(o), Some(n)) => {
                if *o == n {
                    unchanged += 1;
                    false
                } else {
                    changed += 1;
                    true
                }
            }
            (None, _) => {
                added += 1;
                true
            }
            (Some(_), None) => {
                changed += 1;
                true
            }
        };

        let c1 = c_plain.compress(&body)?.len() as u64;
        full1 += c1;
        if is_changed {
            d1 += c1;
        }
        if let Some(c) = c_dict.as_mut() {
            let c3 = c.compress(&body)?.len() as u64;
            full3 += c3;
            if is_changed {
                d3 += c3;
            }
        }

        blk.extend_from_slice(&body);
        blk_dirty |= is_changed;
        if blk.len() >= block_bytes {
            blocks_total += 1;
            let b2 = c_plain.compress(&blk)?.len() as u64;
            let b4 = match c_dict.as_mut() {
                Some(c) => c.compress(&blk)?.len() as u64,
                None => 0,
            };
            if blk_dirty {
                blocks_dirty += 1;
                d2 += b2;
                d4 += b4;
            }
            blk.clear();
            blk_dirty = false;
        }
    }
    if !blk.is_empty() {
        blocks_total += 1;
        let b2 = c_plain.compress(&blk)?.len() as u64;
        let b4 = match c_dict.as_mut() {
            Some(c) => c.compress(&blk)?.len() as u64,
            None => 0,
        };
        if blk_dirty {
            blocks_dirty += 1;
            d2 += b2;
            d4 += b4;
        }
    }

    let touched = changed + added;
    let total = touched + unchanged;
    println!("## delta {} -> {}", old_path, new_path);
    println!("entries_unchanged = {}", unchanged);
    println!("entries_changed   = {}", changed);
    println!("entries_added     = {}", added);
    println!(
        "churn             = {:.1}% of {} entries",
        100.0 * touched as f64 / total.max(1) as f64,
        total
    );
    println!("blocks_total      = {}", blocks_total);
    println!(
        "blocks_dirty      = {} ({:.1}%)",
        blocks_dirty,
        100.0 * blocks_dirty as f64 / blocks_total.max(1) as f64
    );
    println!();
    println!("{:<22} {:>14} {:>14} {:>12}", "arm", "delta_bytes", "full_bytes", "delta_share");
    for (name, d, f) in [
        ("1 per-entry", d1, full1),
        ("2 grouped", d2, 0u64),
        ("3 per-entry+dict", d3, full3),
        ("4 grouped+dict", d4, 0u64),
    ] {
        if d == 0 && f == 0 {
            println!("{:<22} {:>14}", name, "(skipped)");
            continue;
        }
        let share = if f > 0 {
            format!("{:.1}%", 100.0 * d as f64 / f as f64)
        } else {
            "-".into()
        };
        println!("{:<22} {:>14} {:>14} {:>12}", name, d, f, share);
    }
    println!();
    if d1 > 0 {
        println!("grouped delta vs per-entry delta      : {:.2}x", d2 as f64 / d1 as f64);
    }
    if d3 > 0 {
        println!("grouped+dict delta vs per-entry+dict  : {:.2}x", d4 as f64 / d3 as f64);
    }
    Ok(())
}
