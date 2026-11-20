# ADR-0003 — Compression level, dictionary size, other codecs and media

**Status:** Accepted (Commander delegated the choice, 2026-11-20)
**Date measured:** 2026-11-20 · **Owner:** Track A (the `.wax` container format)
**Builds on:** [ADR-0001](ADR-0001-text-compression-codec.md), which settled the
*framing* (one zstd frame per entry, against a shared dictionary) at level 3 only.
**Raw results:** [`docs/bench/compressbench-climate-change-2026-11-20.md`](../bench/compressbench-climate-change-2026-11-20.md)
**Harness:** `crates/wax-bench/src/compressbench.rs`

---

## In plain words

A pack stores each article squeezed on its own, using a shared "cheat sheet"
(the dictionary) of phrases that recur across articles. ADR-0001 chose that.
This run answers the knobs it left open:

- **How hard to squeeze (the level).** Squeezing harder at build time makes text
  **16% smaller** and costs nothing when the box reads it. It only makes the
  *build* slower, and builds happen on a desktop or server, not the box.
  → **Build at level 19.**
- **How big a cheat sheet.** A bigger one looks better on the articles it was
  made from, but a pack keeps the same cheat sheet for every future edition, and
  on *new* text a bigger one does worse. → **Keep 110 KiB.**
- **Other squeezers (brotli, xz).** Two to three times bigger output and 10–35×
  slower to unpack. → **No.**
- **Squeezing small articles together in batches.** Buys almost nothing beyond
  what the cheat sheet already does, and makes reads and updates expensive.
  → **No** (ADR-0001's rejection stands).
- **Pictures.** WebP, PNG, JPEG are already squeezed; every method saved under
  0.1%. → **Keep storing them as-is.** Plain TrueType/OpenType fonts are the
  exception: they shrink by more than half. → **Compress TTF/OTF fonts.**
- **One new safety rule.** Each squeezed article must be unpackable with at
  most 8 MiB of working memory, so a 2 GB box can never be asked for more.

## Context

ADR-0001 measured four framings at zstd level 3 and adopted per-entry +
dictionary (`zstd-dict`, SPEC v0.10). Left open, and listed in SPEC §12 note 25
after the 2026-11-15 WAX-vs-ZIM review: the zstd level (SPEC fixes none,
`wax-core` hard-codes 3), the dictionary size (`wax-core` hard-codes 110 KiB),
grouped compression for small entries, and "media stored uncompressed". The
earlier review found level 19 alone was one of two changes that took WAX from
6% larger than ZIM to 7% smaller.

## Method

- **Corpus:** `wikipedia_en_climate_change_mini_2024-06.zim` (openzim
  zim-testing-suite, 8,978,471 bytes), converted with `zim2wax` at `main`
  `8ada2fe`. 3,992 content entries: 3,889 text (30.3 MB raw; median article
  6.5 KB, p99 25 KB) and 103 images (100 WebP, 3 PNG; 2.3 MB). The kiwix
  download mirror is blocked from this environment, so ADR-0001's three larger
  archives could not be re-run.
- **Machine:** cloud x86_64 (Intel Xeon @ 2.10 GHz), single thread, best of 3
  passes per frame. Timings are relative; nothing here is a Pi number.
- Every entry is read back out of the pack, decoded, and re-encoded per arm.
  Dictionaries are trained by the zim2wax rule (path order, ≤2048 samples, each
  ≤64 KiB, ≤16 MiB total). Sizes include the dictionary.

## Evidence

### Level (text, with the 110 KiB dictionary)

| level | bytes + dict | vs level 3 | encode MB/s | decode µs/article (mean / p99) |
|---:|---:|---:|---:|---:|
| 1 | 4,366,660 | +18.7% | 492 | 5.0 / 17.8 |
| **3** (today) | 3,679,896 | 0 | 410 | 4.7 / 17.7 |
| 9 | 3,264,579 | −11.3% | 71 | 4.5 / 18.5 |
| 15 | 3,087,461 | −16.1% | 12 | 4.4 / 18.9 |
| **19** | 3,076,143 | **−16.4%** | 6.1 | 4.5 / 19.2 |
| 22 | 3,069,208 | −16.6% | 1.7 | 4.6 / 19.6 |

Decode time is flat across levels (zstd decodes any level with the same
algorithm; a smaller frame is if anything slightly faster). The dictionary
matters far more than the level: without it, level 19 is 2.6× larger than
level 3 *with* it. Level 22 buys 0.2% for a further 3.6× build time.

**Whole-pack estimate:** the built pack is 8,214,299 bytes (0.915× the ZIM).
Level 19 removes ~0.6 MB of text frames, so the same pack would be ~7.6 MB,
**~0.85× the ZIM** — an estimate from the frame totals, not a rebuilt pack,
because `wax-core` cannot yet be told a level (follow-up below).

### Dictionary size — and why in-sample numbers mislead

Trained on the whole pack and measured on the same pack (section 2), 256 KiB
beats 110 KiB at level 19 (−19.2% vs −16.4%). But SPEC §5.9 carries one
dictionary forward to every later edition, so it will mostly meet text it was
*not* trained on. Held out — train on even entries, measure on odd (2b):

| dict | level 3 | level 19 |
|---:|---:|---:|
| 64 KiB | +1.2% | −13.0% |
| **110 KiB** | **0** | **−16.1%** |
| 256 KiB | +3.4% | −14.0% |
| 512 KiB | +14.3% | −4.5% |

110 KiB is the best held-out size at both levels on this corpus. A larger
archive amortises a larger dictionary better, so this may move on a full
Wikipedia; it is a value to re-measure on a large archive, not to guess. (A
1 MiB request could not be trained from this sample budget: zstd returned a
132 KiB dictionary that was 56% worse, so it is not reported as a size.)

### Other codecs, per entry

| codec | vs zstd-3 + dict | decode µs (mean) |
|---|---:|---:|
| brotli q11 (built-in web dictionary) | +101.7% | 43.1 |
| brotli q5 | +134.4% | 42.9 |
| xz -6 / -9e | +169.0% | 147–167 |

Neither has a trained per-pack dictionary, and on 6.5 KB documents that is the
whole game. Both also decode slower, roughly 10× (brotli) and 35× (xz), which the box pays on every read.

### Grouping small entries (re-examining ADR-0001 for the small-entry case)

| level 19, dict | bytes + dict | vs per-entry level 19 | read µs, small article |
|---|---:|---:|---:|
| per-entry (recommended) | 3,076,143 | 0 | 4.3 |
| group entries < 4 KiB, 64 KiB blocks | 3,062,007 | −0.5% | 22.9 |
| group entries < 16 KiB, 64 KiB blocks | 2,896,350 | −5.8% | 31.5 |
| group all, 64 KiB blocks | 2,871,740 | −6.6% | 34.3 |
| group all, 1 MiB blocks | 2,806,104 | −8.8% | 491.7 |

Grouping only the genuinely small entries buys 0.5%: the dictionary already
captures the redundancy between small documents. Grouping everything buys
6.6–8.8% at 8–115× the single-article read cost, plus ADR-0001's 16–59× larger
deltas at realistic churn. The trade is the same one ADR-0001 refused.

### Media

| type | entries | raw | best codec saving |
|---|---:|---:|---:|
| image/webp | 100 | 2,284,008 | 0.03% |
| image/png | 3 | 20,637 | 0.00% |
| TTF/OTF fonts (108 system fonts, not in this pack) | 108 | 114,515,836 | **54% (zstd-3) / 60% (zstd-19)** |

Entropy-coded media is correctly stored as `"none"`. "Media stored
uncompressed" is therefore **not a format gap**: any further media saving
comes from *re-encoding* images at conversion time (e.g. resizing, or lossless
PNG→WebP), which is a converter (Wax Service Shell) feature, not a container
one. Fonts are the exception and were a real defect: `zim2wax` stored all
`font/*` verbatim, including uncompressed TrueType/OpenType. The font figure
was measured with Python `zstandard` over `/usr/share/fonts` on this machine.
BMP, TIFF, EOT and ICO are plausibly compressible too, but unmeasured, so they
stay verbatim.

### Plain-HTTP serving (what the box does per visitor page)

Browsers advertise `br` and `zstd` in `Accept-Encoding` **only over HTTPS**
(Chromium: "like Brotli, Zstd is only available in secure contexts i.e. over
https", blink-dev Intent to Ship, Feb 2024; Brotli maintainers: "intentionally
advertised only over HTTPS", 2016). Visitors use plain HTTP (contract §16),
so stored frames can never be passed through: the box decodes every page, and
`zstd-dict` frames could not be passed through even over HTTPS, since browsers
do not have the pack's dictionary.

| per text page | x86 µs (mean) | wire bytes (30.3 MB corpus) |
|---|---:|---:|
| decode stored frame | 4.3–4.6 | 30.3 MB as identity |
| + gzip level 1 | +62 | 11.2 MB (37%) |
| + gzip level 6 | +122 | 9.9 MB (33%) |

Decoding is negligible; re-encoding to gzip dominates and still costs well
under a millisecond per page here. That is input for A5 `wax-serve`, not a
format decision.

## Decision

1. **SPEC §3: writers SHOULD build at level 19 off-box; MAY use lower levels
   on the box.** The level is not recorded and readers need nothing new.
2. **SPEC §3: new normative bound — every zstd frame's window ≤ 8 MiB.** This is
   RFC 8878's recommended decoder support and the browser `Content-Encoding`
   limit; levels ≤ 19 already comply and every existing `wax-core` archive
   does (level 3, window log ≤ 21). A test (`crates/wax-core/tests/window_bound.rs`)
   checks the writer's on-disk frames, with a negative control.
3. **Dictionary stays 110 KiB.** Re-measure on a large archive before changing.
4. **No brotli, no xz, no grouping.** No new `compression` value.
5. **Media stays `"none"`; TTF/OTF fonts become plain `"zstd"`** in `zim2wax`
   (no dictionary; it is trained on text). `wax-builder` already compressed
   `.ttf`/`.otf`, since its verbatim list names only WOFF/WOFF2.

No version change: nothing about the bytes a reader must understand changes.

## Still to confirm on ARM (not run, by Commander's instruction)

- **Decode speed on a Cortex-A72 class box** (Pi 4 floor). Expected several
  times slower than this x86 in absolute terms; the *ratios* above (level makes
  no decode difference, brotli/xz are 10–35× slower) are properties of the
  algorithms and should hold, but are unmeasured on ARM.
- **gzip-1 cost per page on ARM**, which decides whether `wax-serve` compresses
  for visitors on a loaded box.
- **Read from SD card vs SSD** (ADR-0001's open item): unchanged by this run.
- **Larger archives** (chemistry_mini, history_maxi, a full Wikipedia) for the
  dictionary-size question.

## Follow-ups (not in this change)

- `wax-core`: let a writer choose the level (today a literal `3` in
  `writer.rs`), and have `zim2wax`/`wax-builder` default to 19. Builds get
  ~65× slower per thread (≈6 MB/s of text); per-entry framing parallelises
  across cores. Determinism anchors that hash built packs will change.
- `wax-serve` (A5): gzip-1 for text to plain-HTTP clients that accept it;
  identity for media.
