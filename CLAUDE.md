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
| `wax-builder` | `build` / `append` / `inspect` / `verify` / `ls` / `read`; manifest, aliases, compression policy, UUIDv4 identity, minisign hook (A7). | 93 tests |
| `zim2wax` | Track B v0, text + image. §20 canonical paths, href rewriting, redirect flattening, manifest derivation, §11 licensing + build report. | 61 tests |
| `fuzz/` | 3 `cargo-fuzz` targets: `header-parse`, `index-loader`, `segment-merge`. | seed corpora committed |
| WAX format | **v0.9 frozen.** `SPEC.md` is normative and byte-exact. | — |

**Test suite: 214 tests.** Platform results and all measured figures live in
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

### Direction — settled and now written into the docs

Directive 02 wrote all of this into the design set. These are no longer "supersedes until someone rewrites it" notes — **the documents say it**, and the document named is the owner.

- **DeltOS is hardware-agnostic.** Device-name tiers are **retired entirely**. Capability follows measured **RAM, storage, CPU architecture and GPU presence**. Minimum spec 2 GB / 32 GB / `aarch64`; preferred 16 GB / 256 GB / `x86_64`. Board names are examples, never gates. The Pi Zero 2 W is not a target anywhere. → **contract §2**
- **`min_hw_tier` is replaced** by `min_ram_bytes` / `min_storage_bytes` / `arch` / optional `gpu`. **Shipped** — `wax-builder` and `zim2wax` validate and emit them, and a pack built before the migration still opens via a read-time mapping. → **contract §2.3, §11**
- **One WebOS, one light visual language**, admin through learner. No dark theme, no separate admin look. Layered by default, flat fallback **decided by the rendering client**, honouring its reduced-transparency and reduced-motion settings. → **design-language §2, §4, §5, §22**
- **The app contract** — one manifest declaring resources, address and roles, sign-in, health check, backup set and participation, read by the launcher, proxy, backup, health monitoring and store. Adding an app changes no core code. → **contract §15**
- **No telemetry, ever**, with F14 aggregate-local as the sole exception; the shell itself is translatable; backups cover hosted sites, security-lab work and code-studio projects; one licence check for everything DeltOS distributes; **plain HTTP for unmanaged visitors with the secure-context cost recorded**. → **contract §16**
- **Self-healing specified in full** — five properties, including that a *frozen* service is healed and that the minimum profile heals through the OS supervisor. → **track-g §14.3** and **track-e §23.5**
- **Authorization is a permission model, not a role enum.** A **permission** is the unit of "may do this"; a **role is a named bundle of permissions, stored as data**, not a code literal. The four names ship as **built-in default bundles** (`admin` holds everything); an admin composes more — librarian, moderator, lab operator — with no code change. **Default-deny against effective permissions.** **Authentication is a separate axis**: sign-in shape never determines what an identity may do, so a passwordless profile can hold any bundle. → **contract §4**, implemented by **track-f §24**
- **`deltos-identityd` owns the model and runs at the minimum spec; F12 enforces and does not define.** A full open-source IdP is **optional, named, gated on measured resources, never on the floor** — the built-in model is complete without it. No package pinned. → **track-f §24.2**
- **The catalogue is enumerated, never totalled.** No fixed component count survives. **H13 is Community broadcast** — a local radio and podcast station for Community Hub and Field Ops. → **plan §8.1**, **track-h §19.10**

### Blocking decisions — answered

**Enumerated in `docs/cross-track-contract.md` §13, with answers.** Do not re-report them as missing; an earlier version of this file wrongly said no document listed them.

Seven are settled: search-index ownership (Track A owns the bytes, additive minor version, tokenizer declared and refused if unknown), per-service floors (each app declares its own), profile-switch sessions (storage partition per `profile_slot`), the passage unit (deterministic build-time chunks along headings), cross-source ranking (Reciprocal Rank Fusion), the rollback trigger (shape settled; **the window and failure count are measured on real hardware, not guessed**), and the design-system accessibility values (**closed** — the locked tokens).

**One is direction only, not a finished design:** the shell privilege boundary — loopback WebSocket, per-boot token, strict origin check, closed operation list. **It still needs its own design session before C1 is built.** Reading it as final is the mistake §13 exists to prevent.

**Answered since** (§13.2): `community_hub` and `field_ops` do **not** require `x86_64` — the floor is the resources, so a 16 GB ARM64 box qualifies for both. x86 stays the preferred spec, never a gate.

**Still genuinely open** (§13.2): the exact icon set, and the measured per-service floors for the ten existing Track H services.

### Design tokens are locked and verified

`design-language.md` §4 and §5 carry the locked palette, type scale, spacing, radius, focus indicator and touch-target minimum. **I verified every stated contrast ratio before transcribing it** — they are correct, measured against `surface` `#F2EEE6`.

Worth knowing, and recorded in §4: measured against the darker `bg` `#E9E6DF` every ratio is lower — `ink-2` 4.63, `accent-ink` 4.87, `success` 4.65, `accent` 3.34 — all still passing, but with less headroom than the quoted figures suggest. **Validate a new token against `bg`, not `surface`.** `ink-3` measures 2.82 on `bg`, which is why it is decorative-only.

The previous palette's blanket AA claim was false: six pairings failed 4.5:1, worst at 2.99. The accent split into `accent` (fills, 3.6:1, UI) and `accent-ink` (text and focus ring, 5.3:1) is the fix — one value could not serve both roles at AA.

### What comes next

**Still blocking, unchanged:** the Directive 01 rig work. Linux x86 and ARM64 runs, the real-content measurements, the WAX-vs-ZIM output sizes and two of three determinism legs all wait on the two machines being reachable. **The read-only SQLite VFS behind every pack open has still only ever run on Windows.** Windows determinism anchor: `bdc3bc4df07447306dfc8b02d04b51c3d5b0927004115ab7a5c3157ae6983a11`.

**Migration 1 of 2 is done.** `min_hw_tier` → measured resources shipped in Directive 03: `MIN_HW_TIERS` is gone, `zim2wax` emits the resource fields, and every enum assertion was migrated rather than deleted. Legacy packs open via a read-time mapping, proven against real pre-migration artifacts committed at `crates/wax-builder/tests/fixtures/legacy-tier-*.wax`.

**Migration 2 of 2 is not started: the role enum → data-driven permissions** (contract §4). It rides with the Track F identity build rather than standing alone, since the identity service is largely unbuilt.

**Directive 04 — measurement.** The three compression options — per-article, grouped, and per-article with a shared dictionary — measured on the real archives for size, read speed **on SD and on SSD**, and one-month delta size. Needs the rigs and the §5.7 output-size baseline that does not exist yet.

**Then the P1 build line:** the embedded search index (contract §13 decision 1, specified in track-a §20.1) and **A5 `wax-serve`** with the Kiosk hostname-routing model (track-a §20.3).

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
