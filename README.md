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
# Assemble a tree. Picks up ./site/wax-pack.toml if present.
wax-builder build --input ./site --output ./site.wax --sign-key ~/.minisign/pack.key

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

**Manifest content is not validated here.** SPEC §5.6/§6.4 assign the manifest
schema to Track B; `wax-builder` writes the configured rows verbatim and passes
unknown keys through.

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
