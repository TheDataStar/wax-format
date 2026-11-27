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

_Last updated: 2026-11-27 (personal data and reading; dark default corrected). Earlier: 2026-09-15, Directive 01._

### What is built and measured

| Component | State | Evidence |
|---|---|---|
| `wax-core` | Streaming reader (A1b) + streaming writer (A2e), both memory-bounded. Segment-chain merge, one-hop redirects, checksum verification. | 60 tests |
| `wax-builder` | `build` / `append` / `inspect` / `verify` / `ls` / `read`; manifest, aliases, compression policy, UUIDv4 identity, minisign hook (A7). | 95 tests |
| `zim2wax` | Track B v0, text + image; **builds the embedded search index by default** (titles + visible text, `wax-bigram-16` for every language including CJK, SPEC v0.13 §5.7.2) and **cuts every article into passages at its headings** (§5.10). §20 canonical paths, href rewriting, redirect flattening, manifest derivation, §11 licensing + build report. | 61 tests |
| `deltlib` / `deltd` | **The pack server (A5) and cross-pack search, first build (2026-11-20).** One `deltd` serves every pack by hostname (contract §8): issued, live pack hostnames only; `410` for removed pages; ETag per encoding; ranges; zstd passthrough; dictionary entries decoded on the box. API v1 Library and Search groups: one query across all packs, fused by position (RRF), accent-insensitive, new editions searchable on refresh. People and sign-in come from `deltos-identityd` (below); the fixed table is dev-only. **Personal data and reading (2026-11-27, API step 3):** each person's bookmarks, highlights with notes (re-found after an update, flagged and never moved if not), reading lists, history (recorded as pages are served) and export of it all, private to the person, in `/var/lib/deltos/deltd/personal.sqlite`; page outlines and reader mode from the page's passages. Not yet: read-aloud, progress events. Notes and findings: `crates/deltlib/NOTES.md`. | 28 + 7 + 10 unit tests |
| `deltos-net` | **E10, the box's own network (2026-11-20).** Name and address registry; generated `dnsmasq` (wildcard `deltos.lan`/`deltos-packs.lan`, hotspot DHCP, names and reservations live without restart, no query log) and `nftables` (uplink sharing); the network actions behind shelld's checks (secure session, `network.configure`, re-confirm ticket). `docs/box-network-plan.md`. | 11 tests, 1 against real dnsmasq (root; `DELTOS_REQUIRE_NET_E2E=1`) |
| `system/` + `deltos-net` radio | **E3, the box's own Wi-Fi, and boot startup (2026-11-21).** The hotspot (hostapd, WPA2-AES, 2.4 GHz, name/password/country/channel never defaulted; password root-only, never printed) behind a checked `configure_ap`; first-boot-only `deltos-net-provision`; systemd units that start settings → address → firewall → radio → DNS/DHCP → `deltd` in order, each as its own user with only the powers it needs, restart on crash, and apply checked changes without a reboot. `system/install.sh`. `docs/box-radio-and-boot.md`. | 8 unit tests; `system/tests/vm/run.sh` (emulated ARM64, 2 GB, virtual radios and phone) |
| `deltos-identity` / `deltos-identityd` | **The box's sign-in service (2026-11-21).** The register of people, roles (as data, contract §4), sessions, sign-in cards, remembered devices, pack partitions and re-confirmation tickets, in SQLite under `/var/lib/deltos/identity/`; nothing secret stored in the clear. A local socket only `deltd` (and later `deltos-shelld`, three read calls) may open, checked by peer user id. Plain and secure sessions never mixed; teacher and admin passwords HTTPS only; cards and devices; tap-your-name kiosk only. `deltd` forwards deltd API §6.1 to it, checks `Origin` and the anti-forgery token on every write, and now serves **HTTPS** itself (two certificates by zone). First admin via `deltos-identity-provision` (once). Session lengths and other values chosen: `docs/identityd.md` §5. | 22 tests + 8 in `deltlib/tests/signin.rs`; VM recipe |
| `deltos-tls` | **The box's own certificates (2026-11-21).** Every box makes its own CA the first time it starts (ECDSA P-256, 10 years, **name-constrained to `deltos.lan` and `deltos-packs.lan`**, so it cannot vouch for any other site) and the two server certificates from it (397 days, renewed with 30 days left; a 2026-11-01 clock floor for boxes without a real-time clock). `deltos-tls.service` at boot before `deltd`, `deltos-tls.timer` daily. `deltd` offers the CA at `http://home.deltos.lan/api/v1/certificate/ca.crt`, checked by its fingerprint. Track G's proxy reuses this root later. `docs/box-certificates.md`. | 7 tests + 1 in `deltlib/tests/signin.rs`; VM recipe |
| `deltos-backup` | **The box's backup and restore, F10 (2026-11-21).** One file per backup on a drive: the whole state tree `/var/lib/deltos/` except pack files (listed, not copied), the hardware's own measurements and health snapshots; live sessions dropped. **Encrypted with a passphrase the admin types** (age v1, scrypt `2^18`, 10 characters minimum), never stored or logged, no recovery. Restore is a **staged review** (people, admins, the certificate stamp, Wi-Fi name, packs to reinstall; newer schemas and half a CA refused) then a re-confirmed swap that puts everything back if any step fails; the same CA comes back, so enrolled devices keep trusting the box. `start_backup` / `review_backup` / `restore_backup` behind the privilege-boundary checks (`backup.manage`, secure, re-confirm), and `deltos-backup` for root at the console. `docs/box-backup.md`. | 13 tests; VM recipe (back up, wipe to a fresh box with a new CA, restore): 119/119 |
| `delt-desktop` (+ `delt-desktop-app`) | **The Delt desktop reader** (Kiwix-Desktop equivalent, 2026-11-20): deltlib run in-process for one person — its API answers the window, its pack zone serves pages on a private `deltpack:` scheme, no network port. Adds library management, bookmarks, private notes, history, light/dark/colour-blind themes, and edition updates applied in place (SPEC §7.4) and undone (§7.5). The Tauri window is `crates/delt-desktop-app`, **outside the workspace** (needs the platform web view). Notes: `crates/delt-desktop/NOTES.md`. | 24 tests |
| `wss-core` / `wss` | **The Wax Service Shell engine and CLI, first build (2026-11-20).** Converts ZIM (delegated to zim2wax, byte-identical), HTML/document folders, EPUB, PDF and websites into packs at zstd level 19 (ADR-0003), searchable with passages; `refresh` publishes a changed source as an appended update (diff by hash, tombstones, carried dictionary, new manifest and version per SPEC v0.13 §5.6.1), plus `inspect`, `diff`, `update`, `rollback`, `gc`, `sign`, `verify`. Everything not carried is listed in `<pack>.wss-report.json`; the §11 build report carries closed codes only (four added to contract §11 for these importers). Decisions and gaps: `crates/wss-core/NOTES.md`. | 33 tests |
| `deltos-apps` | **Base DeltOS vs add-ons, and capacity (2026-11-22).** The app manifest's wire format (`deltos-app.toml`, schema 1, contract §15) with `evidence` = measured / provisional / open so no floor is invented; the fit check (profile, arch, GPU, dependencies, port/host/permission collisions, RAM as the sum of declared floors plus the box's own `os_ram_bytes`, storage) answering fits / fits, estimated / cannot tell / does not fit; the people-at-once estimate (radio stations, radio data, CPU, RAM) from measured figures only. Base manifests in `system/apps/`, draft add-ons in `crates/deltos-apps/addons/`. Design, capacity measurements and the Admin Dashboard: `docs/base-addons-admin.md`. | 29 tests |
| `deltos-shelld` | **The key holder, and add-on installs (2026-11-23).** The only program that changes the box: root, one socket in `/run/deltos/shelld/` only `deltd`'s user may open (peer user id checked), no network. Re-checks every request with `identityd` itself (live, secure, permission) and redeems the re-confirm ticket; a closed list (network, backup and the five add-on actions built; the rest refused by name as not built); long actions are jobs; every request logged to `/var/lib/deltos/shelld/actions.log`, secrets redacted. Add-on half of `deltos-installd`: copy, minisign check against the add-on key (none exists yet, so every install is refused), safe unpack, fit check, rename into `/usr/lib/deltos/apps/<id>/<version>/`, register. Starting add-ons waits for G1. **Pack installs (2026-11-23):** `install_pack` / `roll_back_pack` / `remove_pack` from a drive; shelld never opens a pack's SQLite: `deltos-pack-check` runs as `deltos-packcheck` (no network) and reports segment ranges and manifest, shelld hashes the header and ranges itself and checks the pack key's signature (none exists yet, so every install is refused); older editions refused by version and `created_at` (Track A §6, `/var/lib/deltos/shelld/packs.json`); fit check (contract §2.3); rollback in place checked against the kept earlier signature; `deltd` picks changes up on its 5 s scan. `docs/shelld.md`. | 10 + 6 tests (+4 unit); VM recipe |
| `deltos-capability` (in `deltos-apps`) | **The box measures itself (2026-11-24).** Writes contract §9's capability record at setup with the chosen profile (refused below its floor, and only once) and again at every boot before any DeltOS service: RAM as sold (`MemTotal` rounded up to a GiB), `os_ram_bytes` with nothing of ours running, the whole drive (floor by size class), arch, render-node GPU or `none`, radio and hotspot ability (`iw`), clock. Before this nothing wrote it and every install was refused. `docs/box-capability.md`. | 5 tests + 3 unit; VM recipe 158/158 |
| `fuzz/` | 3 `cargo-fuzz` targets: `header-parse`, `index-loader`, `segment-merge`. | seed corpora committed |
| WAX format | **v0.13** (2026-11-20, proposed in its own PR, stacked on PR #4): crash-safe header (check + mirror at end of file), `wax-bigram-16` CJK-capable search, passages, `wax-builder gc`, and appended editions that replace the manifest (so `version` can change on an update). v0.12 (merged 2026-11-15, PR #1) added tombstones, search on appended segments, declared title fold, verifiable append deltas. `SPEC.md` is normative and byte-exact; §13 is the version history. | `crates/wax-core/tests/v0_12.rs`, `v0_13.rs` |

**Test suite: 407 tests** (2026-11-21, Linux x86_64, minisign 0.12 installed so the signing tests ran; 460 pass without minisign after the box certificates, 2026-11-21; 527 without minisign after pack installs, 2026-11-23; 535 after the capability record, 2026-11-24; 553 after personal data and reading, 2026-11-27; end-to-end check with real inputs in `docs/e2e-check-2026-11-21.md`; `real_wikipedia_zim` fails on the climate-change *mini* ZIM, which has no audio/video — pre-existing, not a regression). Platform results and all measured figures live in
`docs/baseline.md` and the Directive 01 report. Read `docs/baseline.md` before
quoting any performance number.

**WAX vs ZIM size, first measurement (2026-11-21):** the climate-change *mini* ZIM (8,978,471 bytes) becomes an 8,925,435-byte pack through `wss` (zstd 19, search index included): **0.994×**. One small English ZIM only; larger archives and other languages still to measure (Directive 01 §5.7).

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
- **One WebOS, one visual language**, admin through learner, no separate admin look. **Dark is the default theme, with a light theme and a colour-blind setting** (Commander, 2026-11-15 and 2026-11-22). This overrides `design-language.md` §2, which still says "one light visual language, no dark theme": that document is installed from upstream and needs the correction there; until then the newer decision wins (`docs/base-addons-admin.md` §8, `crates/delt-desktop/src/theme.rs`). Layered by default, flat fallback **decided by the rendering client**, honouring its reduced-transparency and reduced-motion settings. → **design-language §4, §5, §22**
- **The app contract** — one manifest declaring resources, address and roles, sign-in, health check, backup set and participation, read by the launcher, proxy, backup, health monitoring and store. Adding an app changes no core code. → **contract §15**
- **No telemetry, ever**, with F14 aggregate-local as the sole exception; the shell itself is translatable; backups cover hosted sites, security-lab work and code-studio projects; one licence check for everything DeltOS distributes; **plain HTTP for unmanaged visitors with the secure-context cost recorded**. → **contract §16**
- **Self-healing specified in full** — five properties, including that a *frozen* service is healed and that the minimum profile heals through the OS supervisor. → **track-g §14.3** and **track-e §23.5**
- **Authorization is a permission model, not a role enum.** A **permission** is the unit of "may do this"; a **role is a named bundle of permissions, stored as data**, not a code literal. The four names ship as **built-in default bundles** (`admin` holds everything); an admin composes more — librarian, moderator, lab operator — with no code change. **Default-deny against effective permissions.** **Authentication is a separate axis**: sign-in shape never determines what an identity may do, so a passwordless profile can hold any bundle. → **contract §4**, implemented by **track-f §24**
- **`deltos-identityd` owns the model and runs at the minimum spec; F12 enforces and does not define.** A full open-source IdP is **optional, named, gated on measured resources, never on the floor** — the built-in model is complete without it. No package pinned. → **track-f §24.2**
- **The catalogue is enumerated, never totalled.** No fixed component count survives. **H13 is Community broadcast** — a local radio and podcast station for Community Hub and Field Ops. → **plan §8.1**, **track-h §19.10**

### Blocking decisions — answered

**Enumerated in `docs/cross-track-contract.md` §13, with answers.** Do not re-report them as missing; an earlier version of this file wrongly said no document listed them.

Seven are settled: search-index ownership (Track A owns the bytes, additive minor version, tokenizer declared and refused if unknown), per-service floors (each app declares its own), profile-switch sessions (a separate kiosk storage partition per person; `profile_slot` is retired, contract §8), the passage unit (deterministic build-time chunks along headings), cross-source ranking (Reciprocal Rank Fusion), the rollback trigger (shape settled; **the window and failure count are measured on real hardware, not guessed**), and the design-system accessibility values (**closed** — the locked tokens).

**The eighth, the shell privilege boundary, is now designed** (2026-11-20): `docs/privilege-boundary.md`, with its companion `docs/deltd-api-v1.md`. No browser reaches `deltos-shelld`; every action carries the person's session, checked by the box API and again by `deltos-shelld`; a closed action list; changing the box needs HTTPS and a session made over HTTPS. Its remaining open values (session and remembered-device lengths, two paths) are in contract §13.2; the WebOS hostname was answered 2026-11-21: `home.deltos.lan` — do not fill them in while building.

**Answered since** (§13.2): `community_hub` and `field_ops` do **not** require `x86_64` — the floor is the resources, so a 16 GB ARM64 box qualifies for both. x86 stays the preferred spec, never a gate.

**Still genuinely open** (§13.2): the exact icon set, and the measured per-service floors for the ten existing Track H services.

### Crate names versus the Rust stack decision

Commander's stack decision (2026-11-15) names **`waxlib`** for the format and **`deltlib`** for the library, search and serving. In this repository the format library is **`wax-core`** (reading and writing `.wax`), with **`wax-builder`** (the build tool) on top. `wax-core` *is* `waxlib`: the name predates the decision, and renaming it would touch every crate for no change in what it does (audit of 2026-11-27). `deltlib` is named as decided.

### Design tokens are locked and verified

`design-language.md` §4 and §5 carry the locked palette, type scale, spacing, radius, focus indicator and touch-target minimum. **I verified every stated contrast ratio before transcribing it** — they are correct, measured against `surface` `#F2EEE6`.

Worth knowing, and recorded in §4: measured against the darker `bg` `#E9E6DF` every ratio is lower — `ink-2` 4.63, `accent-ink` 4.87, `success` 4.65, `accent` 3.34 — all still passing, but with less headroom than the quoted figures suggest. **Validate a new token against `bg`, not `surface`.** `ink-3` measures 2.82 on `bg`, which is why it is decorative-only.

The previous palette's blanket AA claim was false: six pairings failed 4.5:1, worst at 2.99. The accent split into `accent` (fills, 3.6:1, UI) and `accent-ink` (text and focus ring, 5.3:1) is the fix — one value could not serve both roles at AA.

### What comes next

**Still blocking, unchanged:** the Directive 01 rig work. Linux x86 and ARM64 runs, the real-content measurements, the WAX-vs-ZIM output sizes and two of three determinism legs all wait on the two machines being reachable. **The read-only SQLite VFS behind every pack open has still only ever run on Windows.** Windows determinism anchor: `bdc3bc4df07447306dfc8b02d04b51c3d5b0927004115ab7a5c3157ae6983a11`.

**Migration 1 of 2 is done.** `min_hw_tier` → measured resources shipped in Directive 03: `MIN_HW_TIERS` is gone, `zim2wax` emits the resource fields, and every enum assertion was migrated rather than deleted. Legacy packs open via a read-time mapping, proven against real pre-migration artifacts committed at `crates/wax-builder/tests/fixtures/legacy-tier-*.wax`.

**Legacy floors are settled (Directive 03a).** Each retired tier maps to **its own historical floor**, not to the current minimum: `pi_zero_2w` and `pi_4` to 2 GB/32 GB, **`pi_5` to 4 GB/64 GB**. The rule: *a legacy pack must never resolve below the floor it was built against*, because that would let a box install content it cannot run. Read-time compatibility only — no tier is buildable, and a `min_hw_tier` outside the four is reported unresolvable rather than guessed. → **contract §11**

**Migration 2 of 2 is built with the identity service (2026-11-21): roles are data.** `deltos-identityd` stores roles as named permission bundles in its register, seeds the four built-ins from contract §4.2, grants them at a scope (box-wide or one class), and answers checks from effective permissions; no code holds a role enum. Still to come with F1: editing bundles and composing new roles, and the permission catalogue assembled from app manifests (contract §15).

**Directive 04 — measurement: done.** Framing settled by **ADR-0001** (per-entry + shared dictionary, SPEC v0.10). The remaining knobs were measured 2026-11-20 in **ADR-0003**: build at zstd level 19 (16% smaller text, no decode cost), keep the 110 KiB dictionary, no brotli/xz/grouping, entropy-coded media stays verbatim, TTF/OTF fonts now compressed, and a new SPEC §3 rule bounds every frame's window to 8 MiB. Still open: the same numbers on ARM, SD vs SSD reads, and larger archives; and letting `wax-core` take a level (it hard-codes 3).

**P1 build line, started 2026-11-20:** the embedded search index is built by `zim2wax`, and `deltd` (A5, which replaces per-pack `wax-serve`) serves packs and search with hostname routing at every profile (track-a §20.3). The real `identityd` link and HTTPS are built (`docs/identityd.md`, 2026-11-21). Next for `deltd`: `entry_viewed`, personal data, then admin through `deltos-shelld`; the box makes its own certificates (`docs/box-certificates.md`). The box's DNS/DHCP is `docs/box-network-plan.md`, built in `crates/deltos-net`. The hotspot radio and boot units are `docs/box-radio-and-boot.md`, built in `crates/deltos-net` and `system/`.

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
