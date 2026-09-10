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
| [`wax-core`](crates/wax-core) | reader + writer library (components A1) | reader path, writer path, segment-chain merge, one-hop redirects, checksum verification |
| [`wax-builder`](crates/wax-builder) | CLI: directory tree → `.wax` (A2) | minimal `build` / `read` / `ls` / `inspect`; full manifest + signing surface is the next Track A prompt |
| [`fuzz`](fuzz) | `cargo-fuzz` targets against the reader (A4) | 3 targets, build & run clean |

Signing (A7) is **specified** in [SPEC §8](SPEC.md#8-signing-detached-sidecar--normative-for-v09v1x)
(detached minisign sidecar over `SHA-256(header ‖ index-chain)`); the
implementation is a later phase.

## Build & test

Needs Rust (stable) with a C toolchain for the bundled SQLite + zstd.

```bash
cargo build --workspace
cargo test  --workspace      # conformance corpus + round-trip + fuzz smoke
```

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
wax-builder build   --input ./site --output ./site.wax
wax-builder inspect --archive ./site.wax
wax-builder ls      --archive ./site.wax
wax-builder read    --archive ./site.wax --file index.html    # bytes to stdout
```

## Scope

WAX is a **static, read-only** container: HTML/CSS/JS/images/video/SPAs, not
server code. It does not run search — the optional FTS5 `search_index` / the
reserved stand-off search segment is for the host OS to populate and query
(component A9, v2).

## License

MIT.
