# zim2wax

ZIM archive in, `.wax` pack out — DeltOS Track B, component **B1**, v0.

v0 is the lossy **text + image** converter Track B §4 scopes for P1. It reads
a ZIM with the pure-Rust [`zim`](https://crates.io/crates/zim) crate, produces
a `.wax` pack through `wax-builder`'s library API, and writes the Contract §11
build report beside it.

```bash
zim2wax convert --input wikipedia_en_100.zim --output wikipedia_en_100.wax \
                --category reference --min-hw-tier pi_zero_2w

zim2wax probe --input wikipedia_en_100.zim     # read-only: metadata + derivations
```

`--category` and `--min-hw-tier` are **required**: a ZIM carries no analog for
either (Track B §20), and nothing is defaulted. Values are validated against the
Contract's enums before any ZIM work starts.

## What it does

| Step | Rule | Source |
|---|---|---|
| Classify every dirent | article (`text/html`) → root, `<url>.html`; other content → `_assets/<url>`; `M/` → `_meta/<url>`; `X/` (search index) and `W/` (well-known) not emitted | Track B §20 |
| Rewrite in-document references | `href` `src` `poster` `data` `srcset` in HTML, `url()` in CSS — to root-relative canonical paths; anything that doesn't resolve to an emitted entry is left byte-for-byte | Track B §20 |
| Flatten redirects | `redirect_to` = the terminus's canonical path, one hop; cycles and dangling chains dropped with a counted warning | Track B §20, Track A §5.3 |
| Derive the manifest | `name`←Title · `icon`←Illustration (→`_assets/icon.png`, placeholder if absent) · `version`←Date as CalVer · `attribution`←Creator, else Publisher · `entry_point`←main page · `languages`←Language (ISO 639-3→BCP-47, omitted if unmappable) | Track B §20, Contract §11 |
| License | blank → **fail**; allowlisted SPDX id (case-insensitive) → clean; anything else → builds with `license_review_required` in the build report | Contract §11 |
| Build report | `<output>.build-report.json`, always, §11 schema, warnings as one entry per code with a count | Contract §11 |

### Paths, precisely

ZIM ≥ 6.1 puts articles *and* assets in the `C/` namespace, so the namespace
byte alone cannot classify — a `C/` entry is an article iff its mimetype is
`text/html`. The rule is applied uniformly to the legacy scheme too.

**Case is preserved.** `13th_Amendment` and `13th_amendment` are distinct
dirents in real Wikipedia ZIMs; only the prefix strip and the `.html` suffix
from §20's example are applied, not its lowercasing (see the B1 summary flag).

Rewritten references are **root-relative** (`/Photosynthesis.html`,
`/_assets/_res_/style.css`): a pack is served as the root of its own origin
(Contract §8), so that form is correct at any document depth.

### Warnings: the Contract's closed vocabulary

The build report's warning codes are a **closed vocabulary owned by Contract
§11**; the converter emits only these eight. Every dropped source item is
counted under one of them, so `skipped_count` is always their sum (minus the
three that are not drops).

| code | means |
|---|---|
| `unsupported_mimetype` | audio/video (and anything else outside text + image) skipped; Wikipedia's pronunciation audio lands here. References to it are left intact. |
| `redirect_cycle` | a redirect chain closed on itself and was dropped |
| `redirect_dangling` | a redirect's terminus does not exist (or was itself skipped) and was dropped |
| `invalid_path` | a url that is not a valid WAX path (e.g. a Wikipedia redirect titled `Http://…`); also two dirents canonicalizing to the same path — the later one is dropped |
| `reserved_prefix_collision` | an article inside `_assets/` or `_meta/` |
| `license_operator_supplied` | the ZIM stated no license and `--license` supplied one — **always forces `license_review_required`** |
| `attribution_operator_supplied` | the ZIM had neither Creator nor Publisher and `--attribution` supplied the credit line |
| `icon_generated` | no illustration in the ZIM; a placeholder was generated |

Two things the converter does *not* warn about, because nothing usable was
lost: the ZIM's `X/` search indexes (Xapian; not usable by WAX, and the FTS5
rebuild is out of v0 scope) and `W/` well-known entries (the main page reaches
the manifest through `entry_point`). Both are printed as "not copied, by
design" on stdout. An unmappable `Language` is likewise visible only as an
absent `languages` field.

### `--license` and `--attribution`

Current Wikipedia ZIMs (mwoffliner 1.17) carry **no** `License` metadata, and a
blank license is a hard failure. `--license <SPDX id or text>` supplies one
**only when the ZIM has none** — it never overrides a stated license — and
the resulting pack is **always routed to review** (§11: an operator's claim is
reviewed rather than trusted, even when it names an allowlisted id).

`--attribution <credit line>` is the same shape for a ZIM with neither
`Creator` nor `Publisher`. It raises its own warning but does not by itself
force review.

## Memory: bounded, not proportional (Track A §18)

The converter streams. Blobs go from the ZIM's memory-mapped cluster through
SHA-256 + zstd straight into the pack one at a time; index rows are inserted
into the segment's SQLite database as they arrive; the `(namespace, url)`
lookup that href rewriting needs lives in a temp SQLite file, not a map; and
content is emitted in cluster order so exactly one decompressed cluster is
resident. The only per-dirent heap is an 8-byte classification.

Measured on Windows, release build, sampled at 20 ms:

| ZIM | size | dirents | hrefs rewritten | wall | **private bytes (heap), peak** | working set, peak |
|---|---:|---:|---:|---:|---:|---:|
| `wikipedia_en_chemistry_mini` | 24 MB | 58,260 | 233,416 | 4.6 s | **23.5 MB** | 36 MB |
| `wikipedia_en_100` | 318 MB | 9,337 | 11,751 | 0.9 s | **20.5 MB** | 67 MB |
| `wikipedia_en_history_maxi` | 2,267 MB | 382,612 | 5,177,603 | 63 s | **55.9 MB** | 2,150 MB |

Private bytes — the memory a process actually owns and the number that can
exhaust a 512 MB box — moves from 20 MB to 56 MB across a 100× spread in input
size, and what movement there is tracks dirent count (~8 B each plus two
bounded caches), not bytes. The working-set column is the `zim` crate's mmap
of the input: every blob read touches a mapped page, and the OS charges touched
file-backed pages to the working set. Those pages are clean page cache,
reclaimed under pressure, never a failure mode. Proof: the 2,267 MB conversion
**completes inside a Job Object with a hard 128 MB commit limit** (peak commit
57.1 MB by the kernel's accounting), and the result verifies entry-for-entry.

Extrapolating to a full English Wikipedia (~6.5 M dirents): ~50 MB of
classification plus ~50 MB of emission ordering plus the constant caches —
comfortably inside `pi_zero_2w`'s 512 MB floor.

## Not in v0

* The `search_index` FTS5 table (Track B §4 says zim2wax rebuilds it; schema and
  tokenizer ownership are unresolved between Tracks A and D, Contract §13). The
  ZIM's Xapian indexes are not copied either — they are not usable by WAX.
* Audio and video (P2, once byte-range serving is proven — Track B §4).
* Multi-volume output (A8).

## Tests

```bash
cargo test -p zim2wax
```

61 tests: unit tests for canonicalization, href/CSS rewriting, ISO 639-3
mapping, CalVer derivation and the placeholder PNG; an integration suite over
synthetic ZIMs (a test-only ZIM writer in `tests/common/`) covering redirect
flattening, cycles, dangling, each manifest derivation and fallback, the three
licensing outcomes plus the operator-supplied fourth, unsupported-mimetype
skipping, the `"null"`-title rule, a sweep asserting every code the converter
can raise is on the Contract's list, required-flag rejection at both the
library and CLI level; and the two committed openzim test-suite
archives (`tests/fixtures/`, one per namespace scheme).

A real Wikipedia ZIM (333 MB, not committed) is exercised when
`ZIM2WAX_REAL_ZIM` points at one:

```bash
ZIM2WAX_REAL_ZIM=/path/to/wikipedia_en_100_2026-08.zim cargo test --release -p zim2wax real_wikipedia_zim
```
