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

### What is skipped, and how you find out

Every skipped source item is a warning in the build report:

| code | meaning |
|---|---|
| `unsupported_mimetype` | audio/video (and anything else outside text + image); Wikipedia's pronunciation audio lands here. The entry is skipped, its references are left intact. |
| `redirect_cycle` / `redirect_dangling` | dropped redirects |
| `invalid_path` | a url that is not a valid WAX path (e.g. a Wikipedia redirect titled `Http://…`) |
| `canonical_path_collision` / `reserved_prefix_collision` | two dirents mapping to one path; an article inside `_assets/` or `_meta/` |
| `search_index_not_copied` / `wellknown_not_copied` | `X/` and `W/` entries, by design |
| `icon_placeholder` / `language_unmapped` | derivation fallbacks that were taken |

### `--license`

Current Wikipedia ZIMs (mwoffliner 1.17) carry **no** `License` metadata, and
the Contract makes a blank license a hard failure — so without an operator
statement the flagship content cannot convert. `--license <SPDX id or text>`
supplies the license **only when the ZIM has none**; it never overrides a
license the ZIM states. This is flagged against the Contract in the B1 summary.

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

56 tests: unit tests for canonicalization, href/CSS rewriting, ISO 639-3
mapping, CalVer derivation and the placeholder PNG; an integration suite over
synthetic ZIMs (a test-only ZIM writer in `tests/common/`) covering redirect
flattening, cycles, dangling, each manifest derivation and fallback, the three
licensing outcomes, unsupported-mimetype skipping, required-flag rejection at
both the library and CLI level; and the two committed openzim test-suite
archives (`tests/fixtures/`, one per namespace scheme).

A real Wikipedia ZIM (333 MB, not committed) is exercised when
`ZIM2WAX_REAL_ZIM` points at one:

```bash
ZIM2WAX_REAL_ZIM=/path/to/wikipedia_en_100_2026-08.zim cargo test --release -p zim2wax real_wikipedia_zim
```
