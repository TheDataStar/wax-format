//! What a real edition-to-edition update actually ships.
//!
//! `deltabench` answers a modelling question — what *would* each codec arm cost.
//! This answers the shipped one: given two built packs, for every entry whose
//! content did not change, are the **stored frame bytes** identical?
//!
//! That is the whole carry-forward argument in one measurement. Carry the
//! dictionary forward and an unchanged entry frames to the same bytes, so an
//! update ships nothing for it. Retrain and every frame moves, so the update
//! ships the archive.
//!
//! usage: editiondelta <old.wax> <new.wax>

use std::collections::HashMap;
use wax_core::WaxReader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        return Err("usage: editiondelta <old.wax> <new.wax>".into());
    }
    let old = WaxReader::open(&a[0])?;
    let new = WaxReader::open(&a[1])?;

    let dict_same = match (old.dictionary(), new.dictionary()) {
        (Some(x), Some(y)) => Some(x == y),
        (None, None) => Some(true),
        _ => None,
    };

    // Index the old edition by path -> (content hash, stored span).
    let mut prev: HashMap<String, ([u8; 32], u64, u64)> = HashMap::new();
    for e in old.entries() {
        let e = e?;
        if e.is_redirect() {
            continue;
        }
        if let Some(s) = e.sha256 {
            prev.insert(e.path.clone(), (s, e.offset, e.length));
        }
    }

    let mut unchanged_same_frame = 0u64;
    let mut unchanged_reframed = 0u64;
    let mut reframed_bytes = 0u64;
    let mut genuinely_changed = 0u64;
    let mut changed_bytes = 0u64;
    let mut added = 0u64;
    let mut added_bytes = 0u64;

    for e in new.entries() {
        let e = e?;
        if e.is_redirect() {
            continue;
        }
        let Some(nsha) = e.sha256 else { continue };
        match prev.get(&e.path) {
            None => {
                added += 1;
                added_bytes += e.length;
            }
            Some((osha, _ooff, olen)) if *osha == nsha => {
                // Content identical. Did the stored bytes stay identical too?
                if *olen == e.length {
                    unchanged_same_frame += 1;
                } else {
                    unchanged_reframed += 1;
                    reframed_bytes += e.length;
                }
            }
            Some(_) => {
                genuinely_changed += 1;
                changed_bytes += e.length;
            }
        }
    }

    let total = unchanged_same_frame + unchanged_reframed + genuinely_changed + added;
    let ship = reframed_bytes + changed_bytes + added_bytes;

    println!("## {} -> {}", a[0], a[1]);
    println!(
        "dictionary            : {}",
        match dict_same {
            Some(true) => "identical (carried forward)",
            Some(false) => "DIFFERENT (retrained)",
            None => "present on only one side",
        }
    );
    println!("entries compared      : {total}");
    println!("  content unchanged, same frame : {unchanged_same_frame}");
    println!("  content unchanged, REFRAMED   : {unchanged_reframed}  ({reframed_bytes} bytes)");
    println!("  content changed               : {genuinely_changed}  ({changed_bytes} bytes)");
    println!("  added                         : {added}  ({added_bytes} bytes)");
    println!();
    println!("update ships          : {ship} bytes");
    println!(
        "  of which pure waste : {reframed_bytes} bytes ({:.1}%) -- unchanged content \
         that had to move only because its frame did",
        if ship > 0 { 100.0 * reframed_bytes as f64 / ship as f64 } else { 0.0 }
    );
    Ok(())
}
