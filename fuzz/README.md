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
```

**On Windows, one more prerequisite.** The targets are built with
AddressSanitizer, and an ASan-instrumented binary needs the MSVC sanitizer
runtime `clang_rt.asan_dynamic-x86_64.dll` on `PATH` to start at all. Without
it every target **builds cleanly and then dies at launch** with exit code
`0xc0000135` / `STATUS_DLL_NOT_FOUND`. That is a missing DLL, **not** a broken
build and not a defect in the target. The DLL ships with the VC build tools:

```bash
# add to PATH (adjust the MSVC version directory to match your install)
"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\<ver>\bin\Hostx64\x64"
```

`smoke.sh` locates and prepends it for you via `vswhere`.

### The short pass

```bash
fuzz/smoke.sh          # all three targets, 45s each, tree left clean
fuzz/smoke.sh 300      # longer
```

### Running a target directly

```bash
cargo +nightly fuzz run header-parse   <scratch-corpus> corpus/header-parse
cargo +nightly fuzz run index-loader   <scratch-corpus> corpus/index-loader  -- -max_total_time=300
cargo +nightly fuzz run segment-merge  <scratch-corpus> corpus/segment-merge -- -max_total_time=300
```

**Pass a scratch corpus directory first.** libFuzzer writes newly discovered
inputs into the *first* corpus directory it is given and reads the rest
read-only. Plain `cargo +nightly fuzz run <target>` defaults that first
directory to the committed `corpus/<target>/`, so a routine run silently grows
the checked-in seeds — a 45-second pass over the three targets added **221**
files. Growing the seed corpus should be deliberate, so give libFuzzer
somewhere else to write. `smoke.sh` uses `fuzz/smoke-corpus/` (git-ignored).

Seed corpora live in `corpus/<target>/` and are checked in
(regenerate with `cargo run -p wax-core --example gen_fixtures`). Crash
reproducers, if any, land in `artifacts/` (git-ignored).

## Status / known gaps (SPEC §12.15–16)

* Verified to **build and run on `x86_64-pc-windows-msvc` with nightly**; also
  the usual Linux/macOS path. Short local runs (10⁴–10⁷ execs/target) are
  clean.
* `index-loader` is slow (~500 exec/s) because each case materialises a temp
  SQLite file — that is the real reader path, kept deliberately.
* No large-scale / long-duration campaign yet, and no real-world corpus: every
  seed is hand-built and small. `zim2wax` now exists, so real archives are
  available as a corpus source — not yet used here. Broad fuzzing against
  realistic multi-GB archives is a later activity.
