//! `deltos-pack-check`: the helper that reads a pack for `deltos-shelld`
//! (privilege boundary §3 "Helpers", rule 18; `docs/shelld.md` §7.1).
//!
//! A pack's index segments are SQLite databases from outside the box.
//! Opening them is the risky part of an install, so the key holder does not
//! do it: it starts this program as its own unprivileged user, with no
//! network, handing it the staged pack as its standard input and nothing
//! else. It writes nothing; it prints one JSON answer and exits.
//!
//!   deltos-pack-check check /dev/stdin
//!     {"segments": [[offset, length], ...], "manifest": {...}}
//!   deltos-pack-check rollback-plan /dev/stdin
//!     {"prev_header": "<256 hex>", "prev_end": N, "prev_ranges": [[o, l], ...]}
//!
//! Nothing it says is taken on trust where it matters: shelld hashes the
//! header and the ranges itself and checks the publisher's signature over
//! that digest, so ranges that are not the signed ones are refused.

use std::process::ExitCode;

use wax_core::{ReadOptions, WaxReader};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(mode), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: deltos-pack-check check|rollback-plan PACK");
        return ExitCode::from(2);
    };
    let r = match mode.as_str() {
        "check" => check(path),
        "rollback-plan" => plan(path),
        m => Err(format!("no mode {m:?}")),
    };
    match r {
        Ok(v) => {
            println!("{v}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn check(path: &str) -> Result<serde_json::Value, String> {
    let r = WaxReader::open_with(path, ReadOptions { verify_checksums: false, lookup_cache_entries: 0 }).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "segments": r.segment_ranges(), "manifest": r.manifest() }))
}

fn plan(path: &str) -> Result<serde_json::Value, String> {
    let p = wax_core::rollback_plan(path).map_err(|e| e.to_string())?;
    let hex: String = p.prev_header.iter().map(|b| format!("{b:02x}")).collect();
    Ok(serde_json::json!({ "prev_header": hex, "prev_end": p.prev_end, "prev_ranges": p.prev_ranges }))
}
