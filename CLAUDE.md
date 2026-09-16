# CLAUDE.md — read this first

Ground truth for the WAX / DeltOS repository. **Every session starts here.**
Where this file disagrees with any handoff document, briefing or chat summary,
**this file wins** — those live outside the repo and go stale.

This file does **not** override the two documents that own their own domains:

| Owns | Document | Beats |
|---|---|---|
| The bytes inside the container | `SPEC.md` | everything, on format |
| Every value more than one track consumes | `docs/cross-track-contract.md` | every track document |
| A single track's design | `docs/*-refinement.md` | nothing above it |
| Historical context only | a handoff PDF filed under `docs/` | nothing — **reference, not normative** |

Code implements the spec. If they disagree, the spec wins.

---

## CURRENT STATE

_Last updated: 2026-09-15, Directive 01._

### What is built and measured

| Component | State | Evidence |
|---|---|---|
| `wax-core` | Streaming reader (A1b) + streaming writer (A2e), both memory-bounded. Segment-chain merge, one-hop redirects, checksum verification. | 60 tests |
| `wax-builder` | `build` / `append` / `inspect` / `verify` / `ls` / `read`; manifest, aliases, compression policy, UUIDv4 identity, minisign hook (A7). | 84 tests |
| `zim2wax` | Track B v0, text + image. §20 canonical paths, href rewriting, redirect flattening, manifest derivation, §11 licensing + build report. | 61 tests |
| `fuzz/` | 3 `cargo-fuzz` targets: `header-parse`, `index-loader`, `segment-merge`. | seed corpora committed |
| WAX format | **v0.9 frozen.** `SPEC.md` is normative and byte-exact. | — |

**Test suite: 205 tests.** Platform results and all measured figures live in
`docs/baseline.md` and the Directive 01 report. Read `docs/baseline.md` before
quoting any performance number.

**Not yet measured:** WAX output size vs source ZIM size (Directive 01 §5.7).
The Directive 03 compression decision depends on it.

### Two tests need an external input — both are hardened

Each would otherwise return early and report `ok`, which is indistinguishable
from a real pass in the summary. Both now use **one mechanism**: unset, they
skip with a loud `SKIPPED (` line; with the require flag set, the missing input
is a hard failure. **CI sets both.**

| Needs | Provide with | Require flag |
|---|---|---|
| the `minisign` binary (8 signing tests) | on `PATH`, or `$WAX_MINISIGN` | `WAX_REQUIRE_MINISIGN=1` |
| a real Wikipedia ZIM (`real_wikipedia_zim`) | `$ZIM2WAX_REAL_ZIM` | `ZIM2WAX_REQUIRE_REAL_ZIM=1` |

`$ZIM2WAX_REAL_ZIM` pointing at a path that is not a readable file is **always**
a hard failure, flag or not — that is a misconfiguration, never a valid skip.

One grep audits the whole suite — but **`--nocapture` is required.** Cargo
swallows a *passing* test's stderr, so the "loud" skip line is invisible under a
plain `cargo test`, for the signing tests just as much as the real-content one:

```bash
cargo test --workspace -- --nocapture 2>&1 | grep 'SKIPPED ('
```

Nothing printed means every test ran its subject. This is exactly why the
require flags matter more than the message: **in CI set the flags** and do not
rely on anyone reading the skip line.

### Direction — settled, supersedes any earlier document

* **DeltOS is hardware-agnostic.** Capability is decided by **measured
  resources, never by device name.** Minimum spec is a Raspberry Pi 4/5-class
  ARM64 machine; preferred spec is an x86 mini-PC. **The Pi Zero 2 W is no
  longer a target.** This supersedes the tier tables in `docs/track-e-refinement.md`
  §5 until Directive 02 rewrites them.
* **DeltOS is one WebOS** with a **single light visual language** from admin
  through to learner. This supersedes the flat-only rules in
  `docs/design-language.md` until Directive 02 rewrites them.
* **Visitors' own phones reach the box over plain HTTP**, with packs isolated
  **by hostname**. HTTPS is used only where the box's certificate authority can
  be installed.
* **Approved feature additions** from the alignment brief are pending the docs
  revision. **That list is still growing — it is referenced, never counted.**
  Do not treat any snapshot of it as complete.

### Blocking decisions

Directive 01 §3 and Directive 02 refer to "the blocking decisions" as a known,
named set. **No document in this repo enumerates that set.** Recorded here as a
defect against the owning document; Directive 02 is to record the set and its
proposed answers.

What *is* grounded in the repo — the five Critical cross-cutting findings in
`docs/implementability-sweep.md` §4, each invalidating logic in three or more
documents:

1. **Hardware tier table states ranges, not floors** (Track E §5). `pi_4` is
   "2–8GB", so `min_hw_tier` gates nothing. Superseded in direction above;
   Directive 02 makes it minimum/preferred with measured floors.
2. **No total ordering between tiers and deployment profiles**, and no mapping
   between the two axes, though four documents compare them.
3. **The installable-stack path matches no tier name**, making tier-gated
   decisions undecidable on a path already committed to.
4. **The progress-event schema has four of five elements undefined** — and
   `progress_events` has no primary key, so the Kolibri poll loop duplicates
   every row on re-run.
5. **Nine cross-document amendments are recorded only in the document that
   decided them** — unverified at the owning document.

Plus two decisions Directive 01 explicitly defers:

* **The LAN HTTP decision** — Directive 02 records it.
* **Compression: per-article vs grouped vs per-article with a shared
  dictionary** — Directive 03 measures it against §5.7's baseline.

### Next directive

**Directive 02 — the docs revision.** Hardware tiers become minimum/preferred
specs from measured resources; the single light design language replaces the
flat-only rules; every approved feature addition becomes an owned component;
an extensibility contract lets new apps and services be added without core
changes; self-healing is fully specified; the Kiosk-profile gaps are fixed
(serving phones, restarting crashed services, reusing pack numbers); the
download-time configurator moves to E7; the LAN HTTP decision and the proposed
answers to the blocking decisions are recorded.

Then **Directive 03** (compression, needs §5.7), then the embedded search index
and **A5 `wax-serve`**.

---

## Session-start ritual

Run this before doing anything else. It finds the newest commit carrying a
recorded note, then shows you every commit since then that lacks one — those
are the changes nobody has written up yet.

```bash
git fetch origin --tags
LAST=$(git log --format='%H' | while read -r c; do \
         git notes show "$c" >/dev/null 2>&1 && { echo "$c"; break; }; done)
if [ -n "$LAST" ]; then
  echo "newest noted commit:"; git log --oneline -1 "$LAST"
  echo "--- commits since, WITHOUT notes (review these) ---"
  git log --oneline "$LAST..HEAD"
else
  echo "no commit carries a note yet - review the whole history"
fi
```

Read notes with `git log --notes`, add one with `git notes add -m '...' <sha>`.
Notes live in `refs/notes/commits` and are **not** pushed by default:

```bash
git push origin refs/notes/commits      # after adding
git fetch origin refs/notes/commits:refs/notes/commits
```

---

## Build & test

Needs Rust stable **plus a C toolchain** (bundled SQLite + zstd), the
`minisign` binary, and for fuzzing nightly + `cargo-fuzz`.

```bash
# CI: both require-flags set, so no test can pass by not running
WAX_REQUIRE_MINISIGN=1 \
ZIM2WAX_REQUIRE_REAL_ZIM=1 ZIM2WAX_REAL_ZIM=/archives/wikipedia_en_100.zim \
  cargo test --workspace

# locally, with no ZIM to hand — the real-content test then skips, loudly
WAX_REQUIRE_MINISIGN=1 cargo test --workspace
```

On Windows, `cargo` locates MSVC Build Tools on its own; `with-msvc.bat` is
only needed when it cannot.

```bash
fuzz/smoke.sh            # all three targets, 45s each, leaves the tree clean
```

**Build output is never tracked.** `.gitignore` covers `/target`,
`/crates/*/target`, `/fuzz/target`. `main` tracked 1,592 build-output files
until Directive 01; do not let them back.

## Trunk

`main` is the trunk and carries all code, `SPEC.md`, `docs/` and `fuzz/`.
The state of `main` before Directive 01's merge is recoverable at the tag
**`main-pre-track-a-merge`**. `track-a/streaming-reader` is retained, not
deleted, and now points at the same commit as `main`.
