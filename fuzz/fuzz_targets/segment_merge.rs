#![no_main]
//! A4 fuzz target 3 — segment-chain merge + one-hop redirect resolution.
//! Property: the merge terminates, never panics, and never chases a redirect
//! past one indirection (SPEC §5.3, §5.5, §11.3).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = wax_core::fuzz::check_segment_merge(data);
});
