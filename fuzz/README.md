# wax-core fuzz targets (component A4)

`cargo-fuzz` / libFuzzer targets for the WAX **reader** — the trust boundary
where untrusted archive bytes enter `wax-core`. Property under test for every
target: **malformed input must fail cleanly — a `WaxError`, never a panic, an
out-of-bounds read, an unbounded allocation, or a hang** (SPEC §9, §11.3).

Each target is a one-line shim over a function in
[`wax_core::fuzz`](../crates/wax-core/src/fuzz.rs). Those same functions run in
`cargo test` via [`tests/fuzz_smoke.rs`](../crates/wax-core/tests/fuzz_smoke.rs),
so the property is checked on every CI run even where libFuzzer is unavailable.

| Target (`cargo fuzz run …`) | Function | Exercises |
|---|---|---|
| `header-parse`   | `check_header_parse`  | `WaxHeader::parse` + `validate` against adversarial file-size hypotheses (SPEC §2) |
| `index-loader`   | `check_index_loader`  | opening arbitrary bytes *in place* as a SQLite index segment through the windowing VFS (the reader's own path), reading `segment_meta`, paging `entries`, a point lookup, `manifest` (SPEC §4, §12.23) |
| `segment-merge`  | `check_segment_merge` | segment-chain merge (last-segment-wins) + one-hop redirect resolution over a compact synthetic model (SPEC §5.3, §5.5) |

## Running

Requires a nightly toolchain and `cargo-fuzz`:

```bash
rustup toolchain install nightly
cargo install cargo-fuzz

cargo +nightly fuzz run header-parse
cargo +nightly fuzz run index-loader   -- -max_total_time=300
cargo +nightly fuzz run segment-merge  -- -max_total_time=300
```

Seed corpora live in `corpus/<target>/` and are checked in
(regenerate with `cargo run -p wax-core --example gen_fixtures`). Crash
reproducers, if any, land in `artifacts/` (git-ignored).

## Status / known gaps (SPEC §12.15–16)

* Verified to **build and run on `x86_64-pc-windows-msvc` with nightly**; also
  the usual Linux/macOS path. Short local runs (10⁴–10⁷ execs/target) are
  clean.
* `index-loader` is slow (~500 exec/s) because each case materialises a temp
  SQLite file — that is the real reader path, kept deliberately.
* No large-scale / long-duration campaign yet, and no real-world corpus:
  `zim2wax` does not exist, so every seed is hand-built and small. Broad
  fuzzing against realistic multi-GB archives is a later activity once real
  content exists.
