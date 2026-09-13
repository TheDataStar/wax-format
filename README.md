# WAX — Web Archive eXtended (`.wax`)

> A random-access container for the offline web — entire sites (an encyclopedia,
> a documentation set, a library) in one signed, individually-compressed file
> that a server can stream pages and media straight out of.

Part of **DeltOS Track A**. The on-disk format is defined, byte for byte, in
**[`SPEC.md`](SPEC.md)** — WAX format **v0.9**. Code implements the spec; if they
disagree, the spec wins.

## Layout in one paragraph

`[ 128-byte header ][ blob region ][ SQLite index segment ]` — repeated
`(blob region, index segment)` for each append; v0.9 writers emit exactly one
pair. The header carries the magic, version, `archive_uuid`, and pointers to the
newest index segment. Each entry's bytes are one contiguous span, stored raw or
as a single zstd frame. The index is an embedded SQLite database: an `entries`
table (`path`, `title`, `offset`, `length`, `sha256`, `redirect_to`, …), a
segment-0-only `manifest`, `segment_meta` chain linkage, and a `signatures`
table reserved for the v2 trust model. Appends never rewrite existing bytes —
only the fixed header is overwritten, as the final atomic step
([SPEC §7](SPEC.md#7-append-commit-protocol-normative)).

## Crates

| Crate | Role | Status |
|-------|------|--------|
| [`wax-core`](crates/wax-core) | reader + writer library (A1) | reader path, writer path, segment-chain merge, one-hop redirects, checksum verification |
| [`wax-builder`](crates/wax-builder) | CLI: directory tree → signed `.wax` (A2) | `build` / `append` / `inspect` / `verify` (+ `ls`, `read`); manifest, aliases, compression policy, UUIDv4 identity, minisign hook |
| [`zim2wax`](crates/zim2wax) | ZIM — `.wax` converter (Track B, B1) | v0 text + image: §20 canonical paths, href rewriting, redirect flattening, manifest derivation, §11 licensing + build report; verified against a real Wikipedia ZIM |
| [`fuzz`](fuzz) | `cargo-fuzz` targets against the reader (A4) | 3 targets, build & run clean |

Signing (A7) is specified in [SPEC §8](SPEC.md#8-signing-detached-sidecar--normative-for-v09v1x)
— a detached minisign sidecar over `SHA-256(header ‖ index-chain)` — and is
wired into `wax-builder` by shelling out to the `minisign` binary.

## Build & test

Needs Rust (stable) with a C toolchain for the bundled SQLite + zstd.

```bash
cargo build --workspace
cargo test  --workspace      # conformance corpus + round-trip + fuzz smoke
```

The `wax-builder` signing tests need the `minisign` binary. They skip (loudly)
when it is missing; set `WAX_REQUIRE_MINISIGN=1` — as CI should — to turn a
missing binary into a failure instead of a silent skip.

Fuzzing (nightly + `cargo install cargo-fuzz`):

```bash
cargo +nightly fuzz run header-parse
cargo +nightly fuzz run index-loader
cargo +nightly fuzz run segment-merge
```

Regenerate the checked-in fixtures / fuzz seeds:

```bash
cargo run -p wax-core --example gen_fixtures
```

## CLI

```bash
# Assemble a tree. Picks up ./site/wax-pack.toml if present. Always writes
# ./site.wax.build-report.json beside the archive (Contract §11).
wax-builder build --input ./site --output ./site.wax --sign-key ~/.minisign/pack.key

# Convert a ZIM (see crates/zim2wax/README.md).
zim2wax convert --input wiki.zim --output wiki.wax --category reference --min-hw-tier pi_zero_2w

# Add a new segment to an existing pack and re-sign it.
wax-builder append --archive ./site.wax --input ./update --sign-key ~/.minisign/pack.key

# Header, manifest, segment chain (--entries adds the entry table).
wax-builder inspect --archive ./site.wax --entries

# Re-read every entry (sha256 check) and verify the sidecar.
wax-builder verify --archive ./site.wax --pubkey ./pack.pub --require-signature

wax-builder read --archive ./site.wax --file index.html   # bytes to stdout
```

Build configuration lives in `wax-pack.toml` — manifest rows, redirect aliases,
per-entry titles, and compression policy. See
[`wax-pack.example.toml`](crates/wax-builder/wax-pack.example.toml).

### Pack manifest (Contract §11)

The `[manifest]` block is **validated at build time** against the one normative
field list, [`docs/cross-track-contract.md`](docs/cross-track-contract.md) §11
— eight required, five optional, nothing else. An unknown key is a build
error, not a pass-through.

| Field | Required | Domain |
|-------|----------|--------|
| `name` | yes | string |
| `icon` | yes | path to an entry **inside** the archive (root-level is fine; URIs are not) |
| `category` | yes | `reference` · `education` · `media` · `tools` · `civic` · `health` |
| `license` | yes | SPDX id or free text — see *Licensing* below |
| `attribution` | yes | string |
| `version` | yes | CalVer `YYYY.MM.N`, e.g. `2026.09.1` — not semver |
| `min_hw_tier` | yes | `pi_zero_2w` · `pi_4` · `pi_5` · `mini_pc` |
| `entry_point` | yes | path to the launch target inside the archive |
| `guest_accessible` | no | boolean; the author's default for Guest visibility (admin override is catalog-side) |
| `runtime_ram_bytes` | no | integer; omitted when unset, never written as `0` |
| `runtime_storage_bytes` | no | integer; omitted when unset |
| `languages` | no | comma-separated BCP-47 tags, each validated against the IANA registry |
| `depends_on` | no | comma-separated `archive_uuid` values; each must parse as a UUID |

Two keys are **removed from the schema** and rejected with a dedicated message
rather than silently dropped: `id` (`archive_uuid` is the only identity a pack
carries) and `total_size_bytes` (archive size lives in the catalog's
`packs.size`; see [`docs/track-a-refinement.md`](docs/track-a-refinement.md) §17).

`min_hw_tier` is a board tier. `generic` is rejected — it is a value a box
reports about itself, never one a pack declares (Contract §2). Deployment
Profile names (Kiosk / Classroom / Community Hub / Field Ops) are a different
axis and are rejected with a message saying so.

`depends_on` is validated for shape only; whether the referenced pack exists is
the catalog's question at install time.

An omitted `[manifest]` block is still legal — `wax-core` treats an empty
manifest as valid and opaque. Enforcement applies to a pack that declares one.

#### Licensing has three outcomes

| `license` value | Result |
|---|---|
| blank / missing | **build fails** |
| on the Contract §11 allowlist (literal match: `CC0-1.0`, `CC-BY-4.0`, `CC-BY-SA-3.0`, `CC-BY-SA-4.0`, `GFDL-1.3-or-later`, `MIT`, `Apache-2.0`, `GPL-2.0-only`, `GPL-3.0-only`, `GPL-3.0-or-later`) | builds clean |
| anything else — free text, or an SPDX id not on the allowlist | builds, with **`license_review_required`** in the build report |

`license_review_required` is a **build-report outcome, not a manifest key** —
it must be able to change once a reviewer approves the pack, and nothing sealed
inside the signed archive can. Every successful build writes
`<archive-filename>.build-report.json` beside the archive with the Contract §11
schema (`report_version`, `archive_uuid`, counts, `license`,
`license_review_required`, `signed`, `warnings` as one entry per code with a
count). The catalog's intake reads it; it is not a trust artifact.

#### `archive_uuid` text form

Canonical is lowercase hyphenated (`4c0cfba1-3e77-4b1e-9a02-1f9b3c5d7e01`).
Every input that takes one — `--archive-uuid`, `depends_on` elements — also
accepts the bare 32-hex form; every output (`inspect`, the build report, the
sidecar's trusted comment, `depends_on` as written) emits only the canonical form.

### Signing

`wax-builder` shells out to [`minisign`](https://jedisct1.github.io/minisign/);
key generation is out of scope. Point at a keypair with `--sign-key` /
`--pubkey`, or `$WAX_MINISIGN_KEY` / `$WAX_MINISIGN_PUBKEY`; `$WAX_MINISIGN`
overrides the binary path. Use a password-less key
(`minisign -G -W`) for unattended builds — an encrypted key makes minisign
prompt on the terminal.

An append changes the header and the segment chain, so it invalidates the
existing sidecar (SPEC §8.1). Pass `--sign-key` to `append` to re-sign;
without it the CLI warns that the sidecar is now stale.

### Reproducible builds

A fresh pack gets a random UUIDv4 `archive_uuid`, so two builds differ by
design. Pin both the identity and the timestamp to get byte-identical output:

```bash
wax-builder build --input ./site --output ./site.wax   --archive-uuid 4a1b2c3d-4e5f-4607-8a99-aabbccddeeff   --created-at 1700000000
```

`$SOURCE_DATE_EPOCH` is honoured when `--created-at` is not given.

## Scope

WAX is a **static, read-only** container: HTML/CSS/JS/images/video/SPAs, not
server code. It does not run search — the optional FTS5 `search_index` / the
reserved stand-off search segment is for the host OS to populate and query
(component A9, v2).

## License

MIT.
