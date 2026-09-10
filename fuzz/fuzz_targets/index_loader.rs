#![no_main]
//! A4 fuzz target 2 — the index-footer SQLite loader.
//! Arbitrary bytes are opened as an index segment and its tables are read.
//! Property: SQLite / schema errors surface as `WaxError`, never a panic
//! (SPEC §11.3).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = wax_core::fuzz::check_index_loader(data);
});
