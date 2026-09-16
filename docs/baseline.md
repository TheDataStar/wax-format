# Measurement baseline

Figures every later session compares against. Recorded here so no session ever
depends on a document outside this repository.

The numbers in §1 come from the Directive 01 addendum, which restated the
project handoff's §10. **They were measured on Windows, release build, heap
sampled at short intervals.** Where this file and the repo's own tables
(`crates/zim2wax/README.md`, `docs/track-a-refinement.md` §18, the reader
figures near the end of `SPEC.md`) disagree, **the repo is the source** — this
file is a transcription of an external document and the repo is not.

## 1. Handoff §10 — the Windows baseline

### The three archives

Kiwix English Wikipedia ZIMs. The dated edition used for the baseline was never
recorded; `crates/zim2wax/README.md` shows `wikipedia_en_100_2026-08` only as an
example path. Later runs use the most recent available edition of each and
record exact filenames and sizes. **A size difference against the table below is
expected drift, not a regression.**

| Archive | Source size | Entries | Convert time | Convert heap | Pack open cost |
|---|---:|---:|---:|---:|---:|
| `wikipedia_en_chemistry_mini` | 24 MB | 58,257 | 4.6 s | 23.5 MB | 1.9 MB |
| `wikipedia_en_100` | 318 MB | 9,272 | 0.9 s | 20.5 MB | 1.9 MB |
| `wikipedia_en_history_maxi` | 2,267 MB | 382,608 | 63 s | 55.9 MB | 1.9 MB |

**Two entry counts are both correct and both get reported.** The column above
counts resulting WAX entries. The repo tables count ZIM dirents: 58,260 /
9,337 / 382,612 respectively.

### Other baseline claims

| Claim | Value |
|---|---|
| Memory cap | The 2,267 MB conversion completed under a hard **128 MB** commit limit, peak **57.1 MB**. On Linux, reproduce with a kernel-enforced limit. |
| Pack open, before the streaming reader | 382,608-entry pack cost **332 MB**, ~**870 bytes/entry**. |
| Pack open, with the streaming reader | **1.9 MB**; the whole pack verifies under a **64 MB** cap. |
| Index reads | ~**1.3** index page reads per lookup, ~**1.6** per full read. |
| `WITHOUT ROWID` effect | A path-ordered walk dropped from **222,617 random** reads to **11,563 sequential**; index shrank ~**35%**. |
| Cache | A bounded lookup cache plus SQLite's page cache brings a hot set to ~**0.25** page reads per operation. |
| Scratch space | A full-Wikipedia conversion needs ~**1.7 GB** scratch. **Extrapolated, not measured** — do not cite as measured. |

Reproduce all of the above with the `readbench` example in `wax-core`, which
reports index page-read counts.

## 2. Output size — not yet measured

Directive 01 §5.7 records WAX output size against source ZIM size for all three
archives, split text/media where practical. **Nobody has measured this.** The
Directive 03 compression decision (per-article vs grouped vs per-article with a
shared dictionary) depends on it. Record only; do not act on it.

## 3. Determinism

Two builds are byte-identical only when the archive identifier and **both**
timestamps are pinned. The pack creation time and each segment's write time are
separate clocks — `SPEC.md` §12.20 records that `created_at` is stored twice,
in header bytes 24..32 *and* in each segment's `segment_meta.created_at`.

Pin with `--archive-uuid` and `--created-at` (or `$SOURCE_DATE_EPOCH`).

## 4. Prerequisites (handoff §9.1, and the repo README)

* Rust stable **plus a C toolchain** — the bundled SQLite and zstd need it.
* Nightly Rust plus `cargo-fuzz`, for the three fuzz targets.
* The **`minisign`** binary with a **password-less** key; unattended builds
  cannot answer a passphrase prompt. The test suite mints its own via
  `minisign -G -W -f`.
* **CI rule:** set `WAX_REQUIRE_MINISIGN=1`, which turns a missing `minisign`
  into a hard failure. The signing tests once reported green while silently
  skipping.
