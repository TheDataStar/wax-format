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

**They are enumerated, in `docs/cross-track-contract.md` §13 "Still Open — Not
Invented Here".** An earlier version of this file claimed no document listed
them; that was wrong — it came from grepping for the phrase "blocking decision"
rather than reading §13. Corrected 2026-09-15. Do not re-report this as a gap.

The seven, in the contract's own words:

1. Per-service RAM and disk floors for all ten Track H services — the numbers
   Track E's consolidated budget waits on. Only Track H can supply them.
2. **The shell's IPC surface** — wire format, argument types, response and error
   shapes for every privileged operation. The contract calls this "a design
   session, not a value to pin, and the largest single gap in the set."
3. The passage/chunk unit for embedding, retrieval and citation. Track D's
   citations cannot locate anything inside a large article until it exists.
4. The cross-source search ranking rule — scores from separately-built indexes
   are not comparable. RRF is the obvious candidate but it is Track D's call.
5. What binds a Track H service session to the active profile, and what a
   profile switch does to an open one. Today a switch leaves the previous
   profile's service session authenticated.
6. The update-failure detection rule — what "known-good" means and what triggers
   rollback. Track E specifies the rollback boundary but never its trigger.
7. Design Language: focus-indicator token, spacing scale, touch-target minimums,
   interaction-state palette, reconciled status-icon vocabulary — **plus the
   colour pairings that fail the document's own WCAG AA claim**, the
   primary-action accent among them. See the measured ratios below.

The sweep's five Critical cross-cutting findings (`implementability-sweep.md`
§4) are the *defect register*; the contract is where several were already
**fixed**. Before citing one as open, check the contract: §2 restates the tier
table as guaranteed floors, §2/§3 publish both total orderings and the
profile→tier mapping, and §2 adds the `generic` tier for the installable-stack
path. The sweep is history, not a to-do list.

### Measured: the design language's AA claim is false

`design-language.md` §4 asserts "Every color pairing above meets or exceeds
WCAG AA contrast (4.5:1 for body text)". Computed from its own hex values
(W3C relative-luminance formula), **six text pairings fail 4.5:1**:

| Theme | Pairing | Ratio |
|---|---|---:|
| Light | Accent `C1622A` on Background `F2EEE6` | **3.60** |
| Light | Accent `C1622A` on Surface `FAF8F4` | **3.92** |
| Light | Success `4B7B4E` on Background `F2EEE6` | **4.28** |
| Dark | Success `5C9A60` on Surface `2A2723` | **4.42** |
| Dark | Error `C24545` on Background `1E1C19` | **3.43** |
| Dark | Error `C24545` on Surface `2A2723` | **2.99** — fails even 3:1 |

Border/divider sits at 1.24–1.42 against both surfaces in both themes; whether
that matters depends on whether hairlines count as UI components under 1.4.11.

The contract says *five*. This measurement says six, and the discrepancy is
unresolved — the handoff PDF that is the cited source is still not in `docs/`.
Under the no-dark-theme direction the three dark rows disappear with the theme,
leaving three light-theme failures to fix.

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
