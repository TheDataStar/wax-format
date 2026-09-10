#![no_main]
//! A4 fuzz target 1 — the fixed 128-byte header parser + bounds validation.
//! Property: never panics, never reads out of bounds (SPEC §11.3).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = wax_core::fuzz::check_header_parse(data);
});
