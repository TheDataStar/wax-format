# WAX Archive Format — Specification

**Format version: 0.9** (`version_major = 0`, `version_minor = 9`)
**Status: FROZEN for v0.9.** Byte layout and index schema below are settled
decisions from the DeltOS *Track A Refinement* document (v2, post-review).
This file is the authoritative, byte-exact contract. Code implements this
document; this document is not reverse-engineered from code.

Document component: **A3**. Related components: A1 (`wax-core` reader/writer),
A2 (`wax-builder` CLI), A4 (conformance suite & fuzzers), A7 (pack signing).

---

## 0. Terminology & conventions

| Term | Meaning |
|------|---------|
| **Archive** / **pack** | A single `.wax` file. |
| **Entry** | One stored resource, addressed by its `path`. |
| **Blob** | The on-disk byte span holding one entry's (optionally compressed) content. |
| **Blob region** | A contiguous run of blob bytes written by one build or append step. |
| **Index segment** | One embedded SQLite database describing the entries of exactly one blob region. |
| **Segment chain** | The ordered list of index segments `segment[0] … segment[N]`, base first. |
| **Base segment** | `segment[0]`, written by the initial build. Holds the immutable `manifest`. |
| **Append** | Adding a new blob region + new index segment without rewriting existing bytes (except the fixed header). |
| **Reader** | Any consumer that opens an archive for reading (`wax-core` reader path). |
| **Builder** / **writer** | Any producer of archives (`wax-core` writer path, `wax-builder`). |

### 0.1 Numeric encoding

* All multi-byte integer fields in the **fixed header** are **little-endian**,
  unsigned, unless stated otherwise.
  *(Rationale: all current and planned target platforms — ARมv7/ARM64/x86-64 —
  are little-endian (targets: ARMv7, ARM64, x86-64); this lets the reader map the
  header with zero byte-swapping. See §12 "Implementation notes / assumptions" —
  this choice is not stated verbatim in the Refinement doc and is flagged there.)*
* Integer columns inside index segments use SQLite's native storage
  (`INTEGER`, up to 8 bytes, two's-complement). Values defined in this spec
  are non-negative and fit in 63 bits.
* `sha256` columns are stored as a `BLOB` of exactly 32 bytes.
* Text is UTF-8 with no BOM. Paths additionally follow §6.1.

### 0.2 Requirement keywords

"MUST", "MUST NOT", "SHOULD", "MAY" follow RFC 2119. A reader that violates a
MUST is non-conforming; the conformance suite (A4) encodes each testable MUST.

---

## 1. File layout (overview)

```
offset 0                                                                 EOF
+----------------+----------------------------------------------------------+
| Header         | Body                                                     |
| 128 bytes      | one or more (blob region, index segment) pairs           |
+----------------+----------------------------------------------------------+

Body, general form (after N appends → N+1 segments):

  128                      : blob region 0        (segment 0's entries' blobs)
  128 + |br0|              : index segment 0      (SQLite; holds `manifest`)
  ...                      : blob region 1
  ...                      : index segment 1
  ...                                              ...
  ...                      : blob region N
  index_offset             : index segment N      (SQLite)   <- header points here
  index_offset+index_length: EOF
```

* **v0.9 emits exactly one segment** (`segment count = 1`, no `is_multi_volume`,
  no appends performed by any v0.9 writer). For a v0.9 single-segment archive
  the layout collapses to:

  ```
  0                     : header (128 bytes)
  128                   : blob region 0        (blob_section_length bytes)
  128 + blob_section_length : index segment 0  (index_length bytes)  == index_offset
  EOF                   : index_offset + index_length
  ```

  and the invariant `index_offset == 128 + blob_section_length` MUST hold
  (§4.3, checked by the reader).

* The **general segment-chain shape MUST still be implemented by every reader**
  (merge logic, §5) and exercised by the conformance suite, even though no v0.9
  writer produces more than one segment.

* Index segment bytes are a **raw, uncompressed SQLite database file** (SQLite's
  own on-disk format, page size chosen by the writer). The reader copies or maps
  those exact bytes and opens them with SQLite. There is no WAX-level wrapper,
  framing, or compression around a segment.

---

## 2. Header — 128 bytes, fixed layout

All offsets are from the start of the file. The header occupies bytes `[0, 128)`.

| Offset | Field                  | Size | Type    | Notes |
|-------:|------------------------|-----:|---------|-------|
| 0      | `magic`                | 4    | bytes   | ASCII `"WAX1"` = `57 41 58 31`. Reader MUST reject any other value. |
| 4      | `version_major`        | 1    | u8      | `0` for this spec. Reader MUST refuse to open an archive whose `version_major` it does not implement. |
| 5      | `version_minor`        | 1    | u8      | `9` for this spec. Reader MUST NOT fail on an unknown (higher) minor; it ignores index columns it does not know (§5.4). |
| 6      | `flags`                | 2    | u16 LE  | Bit flags, §2.1. Reader MUST ignore bits it does not know. |
| 8      | `archive_uuid`         | 16   | bytes   | Stable archive identity, preserved across every version/append of the same logical pack. A delta update (A6, later) matches against this. Format is an RFC 4122 UUID's 16 raw bytes; WAX does not require any particular version. All-zero is discouraged but not rejected. |
| 24     | `created_at`           | 8    | u64 LE  | Unix epoch **seconds** at which this archive state was written. On append it is updated to the append time. |
| 32     | `index_offset`         | 8    | u64 LE  | Byte offset of the **newest** index segment (`segment[N]`). Rewritten on every append. |
| 40     | `index_length`         | 8    | u64 LE  | Length in bytes of the newest index segment. Rewritten on every append. |
| 48     | `blob_section_length`  | 8    | u64 LE  | Sum of the lengths of **all** blob regions in the file (§4.3). Rewritten on every append. Enables a fast integrity pre-check before opening SQLite. |
| 56     | `search_index_offset`  | 8    | u64 LE  | Offset of the v2 stand-off search segment. **`0` = absent, which is the case for all of v0.9.** |
| 64     | `search_index_length`  | 8    | u64 LE  | Length of that segment. `0` when `search_index_offset` is `0`. |
| 72     | `reserved`             | 56   | bytes   | MUST be written as zero by v0.9 builders. Reader MUST ignore its contents entirely (any value, including all-`0xFF`, is still a valid header). Reserved for future fixed-header fields. |

Total: `72 + 56 = 128` bytes.

### 2.1 `flags` bits

`flags` is a little-endian `u16`. Bit 0 is the least-significant bit.

| Bit | Name              | Set when… |
|----:|-------------------|-----------|
| 0   | `has_search_index`| `search_index_offset != 0`. (Always 0 in v0.9.) |
| 1   | `has_delta_base`  | This archive is a delta/patch pack built against a base pack (A6, later). Always 0 in v0.9. |
| 2   | `is_multi_volume` | This archive is one volume of a multi-volume set (A8, later). Always 0 in v0.9. |
| 3   | `is_signed`       | A detached signature sidecar (§8) is expected to exist alongside the file. Informational only; the reader's signature policy does not depend on this bit. |
| 4–15| —                 | Reserved, MUST be written 0, reader MUST ignore. |

> **Interpretation flag (§12):** the Refinement doc lists the four flag names
> in order but does not assign explicit bit positions. Bit positions 0–3 in
> listed order are assigned here.

### 2.2 Header validation (reader, on open)

In order:

1. `magic == "WAX1"` — else `BadMagic`.
2. The file is at least 128 bytes — else `TruncatedHeader`.
3. `version_major == 0` — else `UnsupportedMajorVersion { found }`.
   (`version_minor` is **not** checked; any value is accepted.)
4. `index_length >= 512` (minimum SQLite database size is one 512-byte page) —
   else `IndexTooSmall`.
5. `index_offset >= 128` — else `IndexOffsetInHeader`.
6. `index_offset + index_length` does not overflow `u64` and is
   `<= file_size` — else `IndexOutOfBounds`.
7. `blob_section_length` consistency (§4.3).

Only after all of these does the reader open the SQLite segment.

---

## 3. Body — blob regions

* A blob region is a concatenation of entry blobs with no padding or alignment
  between them. Entry order within a region is writer's choice.
* **Each entry's bytes are one contiguous span** `[offset, offset + length)`,
  where `offset` and `length` come from that entry's `entries` row (§5.2).
  `offset` is absolute (from start of file).
* An entry blob holds the entry content encoded per its `compression` column:
  * `compression = "none"` → the span is the content verbatim;
    `length == uncompressed_length`.
  * `compression = "zstd"` → the span is a single complete Zstandard frame
    (RFC 8878) whose decoded output is the content; `uncompressed_length` is
    the decoded size and `length` is the frame size on disk.
* v0.9 defines exactly these two `compression` values. A reader that encounters
  any other value for an entry it is asked to read MUST return
  `UnknownCompression { value }` rather than guessing.
* Redirect entries (`redirect_to` non-NULL) own no blob bytes; their `offset`
  and `length` MUST be `0` and are ignored (§5.3).
* Blob spans of non-redirect entries within a segment MUST lie inside that
  segment's blob region (§4.2). They MAY be referenced by more than one path
  only via `redirect_to`; the format does not otherwise deduplicate.

### 3.1 `sha256`

`entries.sha256` is the SHA-256 (32 raw bytes) of the entry's **uncompressed
content** (the bytes a reader returns to its caller), not of the on-disk
compressed span.

* Redirect entries: `sha256` MAY be NULL or the 32-byte hash of the target's
  content; readers MUST NOT rely on it for redirects.
* A reader MAY verify `sha256` after decompression. `wax-core`'s reader verifies
  by default and returns `ChecksumMismatch { path }` on failure; verification
  can be disabled by the caller for throughput.

---

## 4. Body — index segments

Each `(blob region, index segment)` pair is written together. Once written, a
segment's bytes and its blob region's bytes are **immutable** — no writer ever
rewrites them (§7).

### 4.1 Segment = SQLite database

An index segment is a standalone SQLite database. It MUST contain the tables
`entries`, `segment_meta`, and `signatures`; `segment[0]` additionally contains
`manifest`; any segment MAY contain `search_index` (§5.6). Extra tables and
extra columns MUST be ignored by the reader.

The reader opens each segment **read-only** and MUST tolerate SQLite errors
(malformed database, missing table, wrong column type) by returning a WAX error,
never by panicking (A4 fuzz target 2).

### 4.2 `segment_meta` — chain linkage

```sql
CREATE TABLE segment_meta (
    key   TEXT PRIMARY KEY,
    value TEXT
);
```

Required keys in **every** segment:

| key                    | value |
|------------------------|-------|
| `format`               | `"wax-index-segment"` (constant; reader rejects a segment without it → `NotAnIndexSegment`). |
| `segment_index`        | Decimal string, `"0"` for the base, incrementing by 1 per append. |
| `blob_region_offset`   | Decimal string; absolute file offset where this segment's blob region begins. |
| `blob_region_length`   | Decimal string; length in bytes of this segment's blob region (`0` is legal — a segment MAY add only redirects). |
| `created_at`           | Decimal string; Unix epoch seconds for this segment's write. |

Required in every segment **except** `segment[0]`:

| key                     | value |
|-------------------------|-------|
| `prev_segment_offset`   | Decimal string; absolute offset of the previous segment's index database. |
| `prev_segment_length`   | Decimal string; length of the previous segment's index database. |

`segment[0]` MUST NOT contain `prev_segment_*` (their absence is how the reader
detects the base).

> **Design-gap flag (§12):** the Refinement doc states the on-disk shape must
> already be "a chain of segments (base + append), reader-merged with
> last-segment-wins per path" but does **not** specify how a reader locates
> segments `0 … N-1` given only the header's single pointer to segment `N`.
> `segment_meta` with a backward `prev_segment_*` link is introduced here to
> make the stated requirement implementable. This is the single largest piece
> of the spec that is derived rather than transcribed.

### 4.3 Blob-section integrity pre-check (reader)

After header validation (§2.2) and after walking the segment chain (§5.1), the
reader MUST verify:

* For each segment, `blob_region_offset >= 128` and
  `blob_region_offset + blob_region_length <= (offset of segment[0]'s index for
  the base; otherwise the offset of that segment's own index database)`.
  Equivalently: every blob region lies fully before its own index segment and
  after the header.
* `Σ blob_region_length` over the whole chain `== header.blob_section_length`
  — else `BlobSectionLengthMismatch`.
* **Single-segment fast path (v0.9):** when the chain has length 1,
  `index_offset == 128 + blob_section_length` MUST hold — else
  `BlobSectionLengthMismatch`. (This is the cheap check the field exists for;
  it needs only the header.)

---

## 5. The index schema

### 5.1 Segment-chain walk

```
seg   := open_sqlite(bytes[index_offset .. index_offset+index_length])
chain := [seg]
guard := 64                      # hard cap on chain length; else TooManySegments
while seg.segment_meta has prev_segment_offset:
    (po, pl) := (prev_segment_offset, prev_segment_length)
    reject if po < 128 or po+pl > index_offset or pl < 512      # PrevSegmentOutOfBounds
    reject if po already seen                                   # SegmentChainCycle
    seg := open_sqlite(bytes[po .. po+pl])
    prepend seg to chain
    guard -= 1; reject if guard == 0                            # TooManySegments
assert chain[0].segment_meta.segment_index == "0"               # else BrokenSegmentChain
assert chain is contiguous 0,1,2,… by segment_index             # else BrokenSegmentChain
```

The reader now holds `chain[0 … N]` in ascending `segment_index` order.

### 5.2 `entries`

```sql
CREATE TABLE entries (
    path                TEXT PRIMARY KEY,
    title               TEXT,
    offset              INTEGER,
    length              INTEGER,
    uncompressed_length INTEGER,
    mime                TEXT,
    compression         TEXT,
    sha256              BLOB,
    volume_id           INTEGER DEFAULT 0,
    redirect_to         TEXT DEFAULT NULL
);
```

| Column | Meaning / constraints |
|--------|-----------------------|
| `path` | Canonical entry key. §6.1. PRIMARY KEY **within its own segment** — uniqueness is not enforced across segments (that is what the merge resolves). |
| `title` | Human-readable title; nullable. Not interpreted by the format. |
| `offset` | Absolute file offset of the blob span. `0` for redirects. |
| `length` | On-disk (compressed) length of the blob span. `0` for redirects. |
| `uncompressed_length` | Decoded content length. `0` for redirects. For `compression="none"`, equals `length`. |
| `mime` | MIME type string, e.g. `text/html`; nullable. Readers that need a value where it is NULL SHOULD substitute `application/octet-stream`. |
| `compression` | `"none"` or `"zstd"` in v0.9 (§3). For redirects, ignored; SHOULD be `"none"`. |
| `sha256` | 32-byte BLOB, hash of uncompressed content (§3.1); nullable for redirects. |
| `volume_id` | Multi-volume member that holds this entry's blob (A8, later). **Always `0` in v0.9**; a non-zero value in a v0.9 archive is malformed → `UnexpectedVolumeId`. |
| `redirect_to` | If non-NULL, this `path` is an alias — §5.3. |

The reader MUST query `entries` with an **explicit column list**, never
`SELECT *`, so that unknown columns added by a future minor version are ignored
(§5.4). If `redirect_to` or `title` is absent from `entries` (older/oddly-built
segment), the reader treats it as all-NULL rather than failing
(`PRAGMA table_info(entries)` gate).

### 5.3 Redirects — single-hop rule

* A **redirect entry** has `redirect_to` set to some other path `T`.
* At **build time**, redirect chains are flattened: if the author declares
  `A → B` and `B → C`, the builder MUST emit `A → C` and `B → C` (every
  redirect points straight at a non-redirect entry). See §6.2.
* At **read time**, resolution is at most one indirection:

  ```
  resolve(p):
      e := merged_entry(p)                     # §5.5; None -> EntryNotFound
      if e.redirect_to is NULL: return e
      t := merged_entry(e.redirect_to)         # None -> DanglingRedirect { from: p, to }
      if t.redirect_to is not NULL:            # chain not flattened
          return Err(RedirectChainTooDeep { from: p, via: e.redirect_to })
      return t
  ```

  A reader MUST NOT perform a second lookup after the first indirection. An
  archive that would require it is malformed, and the reader reports it rather
  than following it. (A4 corpus: "redirect_to chains — must reject >1 hop".)
* A redirect whose target is itself (`redirect_to == path`) is malformed →
  `RedirectChainTooDeep` (the one-hop target still has `redirect_to` set).

### 5.4 Version tolerance

* `version_major` mismatch → refuse at header stage (§2.2).
* `version_minor` higher than known: the reader proceeds. Because it selects
  explicit columns, any extra `entries`/`segment_meta` columns are invisible to
  it. Extra tables are ignored.
* The conformance suite includes a file with `version_minor = 0xFF` and an added
  junk column on `entries`; the reader MUST open it and read entries normally.

### 5.5 Merge — last-segment-wins

The reader presents a single logical entry set built from the chain:

```
merged_entry(path):
    for seg in chain reversed (segment_index N, N-1, …, 0):
        if seg.entries has row for path: return that row
    return None
```

* **Last (highest `segment_index`) wins** per path. A later segment can
  override an entry (new content, new redirect) or introduce a new one.
* v0.9 has no tombstone / deletion marker. (A later minor MAY add one; readers
  ignoring unknown columns stay compatible.)
* `list()` returns the union of paths, each resolved through `merged_entry`
  (and, for callers that want it, through `resolve` for redirect flattening).
* Iteration order of `list()` is `path` ascending (`ORDER BY path`), stable
  across segments.

### 5.6 `manifest`

```sql
CREATE TABLE manifest (
    key   TEXT PRIMARY KEY,
    value TEXT
);
```

* Present **only in `segment[0]`**. A reader MUST ignore any `manifest` table
  found in `segment[1…N]`.
* **Immutable across appends.** Changing manifest content is by definition a new
  pack version (new build, possibly new `archive_uuid` policy per Track B), not
  an append.
* The **set of keys and their semantics is Track B's manifest-schema work** and
  is deliberately not specified in A3. A3 fixes only: table name, location,
  immutability, and `(key TEXT PRIMARY KEY, value TEXT)` shape. `wax-core` in
  this phase exposes `manifest()` as an opaque `Map<String,String>` and does not
  validate keys.

### 5.7 `search_index`

```sql
-- optional, v0.9 form of component A9
CREATE VIRTUAL TABLE search_index USING fts5(path UNINDEXED, title, body);
```

* **Optional.** Absent in every archive a v0.9 builder produces.
* When present it is an FTS5 virtual table; a reader without FTS5 compiled in
  MUST still open the archive and serve entries, simply treating search as
  unavailable.
* The stand-off binary search segment pointed to by `search_index_offset`
  (header §2) is the **v2** form and is entirely unused in v0.9
  (`search_index_offset == 0`). Space is reserved in the header only.
* A3/A4 scope: reserve the header fields and the table name. No search
  implementation, no ranking, no query surface.

### 5.8 `signatures`

```sql
CREATE TABLE signatures (
    signer     TEXT,
    algo       TEXT,
    signature  BLOB,
    signed_at  INTEGER
);
```

* The table exists in the schema from v0.9 so that the on-disk shape is stable,
  **but no v0.9 or v1.x writer writes rows to it.** Signing through v1.x is done
  with an external detached sidecar (§8). A reader MUST NOT require rows here and
  SHOULD ignore any it finds during v0.9/v1.x.
* In-archive signature rows are reserved for the v2/TUF trust model.

---

## 6. Builder obligations

These constrain any conforming writer (`wax-core` writer, `wax-builder`). They
are not reader-checked except where a resulting archive would be malformed.

### 6.1 Path normalization

A stored `path`:

* is UTF-8, uses `/` as the only separator;
* has no leading `/`, no `.` or `..` component, no empty component, no trailing
  `/`, no backslash;
* SHOULD be Unicode NFC.

The builder normalizes OS paths to this form (`\` → `/`, strip the input root,
reject traversal). Two input files normalizing to the same `path` is a build
error. The reader does **not** re-normalize lookups in v0.9: a caller asking for
`"a/b.html"` must pass exactly the stored form. (Case sensitivity is therefore
the archive's, i.e. exact-match.)

### 6.2 Redirect flattening (build time)

Given author-declared redirects, the builder computes, for each redirect source,
the ultimate non-redirect target by following the declared graph:

* A cycle in declared redirects is a build error (`RedirectCycle`).
* A redirect whose ultimate target has no entry is a build error
  (`DanglingRedirect`) — v0.9 does not emit redirects to nonexistent paths.
* The emitted `entries.redirect_to` for every redirect row is that ultimate
  target, so the on-disk graph has depth exactly 1.

### 6.3 Writing order & the append-commit protocol

See §7. A fresh build is the degenerate case: write header placeholder → write
blob region 0 → write index segment 0 → `fsync` → overwrite header.

### 6.4 What `wax-builder` owns vs. Track B

* **`wax-builder` (A2) owns:** directory walk, path normalization, MIME
  detection, per-entry compression choice, redirect declaration input, blob +
  segment layout, header finalization, invoking the signer (A7).
* **`manifest` *content*** (which keys, validation, provenance metadata) is
  **Track B**. In this phase `wax-builder` writes only whatever minimal
  `manifest` rows Track B has frozen; if none are frozen yet it writes an empty
  `manifest` table. This boundary is called out in the Refinement doc and is not
  resolved here.

---

## 7. Append-commit protocol (normative)

WAX archives support in-place append without rewriting existing content. The
protocol below is what makes an interrupted append safe to detect.

**Invariant:** the blob regions and index segments already in the file are never
modified. The **only** bytes an append overwrites are the fixed 128-byte header.

### 7.1 Sequence (writer)

Starting from a valid archive with `N+1` segments (`segment[0…N]`), header `H`:

1. **Append blob region `N+1`** at end of file (current EOF).
2. **Append index segment `N+1`** immediately after it. This SQLite database
   contains: the new/overriding `entries` rows, a full `segment_meta`
   (including `prev_segment_offset/length` pointing at `segment[N]`'s database,
   `segment_index = N+1`, its own `blob_region_offset/length`), an empty
   `signatures` table, and **no `manifest`**.
3. **`fsync`** the archive file (and the containing directory on POSIX) so that
   all appended bytes are durable.
4. **Overwrite the header in place** at offset 0 with `H'`:
   `index_offset` / `index_length` → segment `N+1`;
   `blob_section_length` → old value + `|blob region N+1|`;
   `created_at` → now; `flags` unchanged unless a flag now applies.
   This is a single `pwrite` of exactly 128 bytes at offset 0.
5. **`fsync`** again.

### 7.2 Crash semantics

* Crash before step 4 completes: the header still points at `segment[N]`. The
  appended bytes past `index_offset + index_length` are ignored by the reader
  (it never reads past the newest segment it knows about). The archive is
  exactly its pre-append state. A subsequent append or a `wax-builder gc` can
  reclaim / overwrite the orphaned tail.
* Crash during step 4 (torn 128-byte write): see §7.3 — this is the residual
  risk.
* Because step 4 changes `blob_section_length` and `index_offset` together and
  they are validated against each other and the file size (§4.3), a header that
  is internally inconsistent (e.g. new `index_offset` but old
  `blob_section_length`) is detected as `BlobSectionLengthMismatch` rather than
  silently accepted.

### 7.3 Atomicity assumption (flagged, §12)

The protocol assumes the final 128-byte header write is **atomic with respect to
power loss** on the target filesystems — a reader after a crash sees either the
entire old header or the entire new header, never a mix.

* A 128-byte write starting at offset 0 is within a single filesystem block on
  ext4 and F2FS (block size ≥ 4 KiB), and both use journaling / atomic metadata
  that in practice make a single-sector-aligned overwrite atomic. This is
  **widely relied upon but not something this document can prove for SD-card
  F2FS under sudden power loss**, where the FTL may not honor sector atomicity.
* **This is an open item** (Refinement doc "Explicitly open"). Until confirmed,
  builders SHOULD treat append as best-effort and readers MUST validate the
  header (§2.2, §4.3) rather than trusting it. A future revision MAY add a header
  checksum + an A/B header slot to remove the assumption; the 56-byte
  `reserved` area leaves room for a header CRC.

---

## 8. Signing (detached sidecar) — normative for v0.9/v1.x

Full A7 *implementation* is a later phase; the mechanism it must implement is
fixed here.

### 8.1 Signed material

The signature covers the **header bytes and the current index (segment-chain
state) together** — never either alone:

```
signed_message := SHA-256(
      header_bytes[0..128]
   || segment[0]_database_bytes
   || segment[1]_database_bytes
   || …
   || segment[N]_database_bytes
)                                      # 32-byte digest
```

* Segments are concatenated in ascending `segment_index` order.
* Blob regions are **not** directly in the digest; each entry's `sha256` in the
  (signed) index binds the blob content transitively.
* The digest changes whenever the header changes (every append) or any segment
  is added — so the sidecar MUST be regenerated on every append.

### 8.2 Sidecar file

* Algorithm: **minisign** (Ed25519), signing `signed_message` in **prehashed
  mode** (minisign `-H`; the 32-byte SHA-256 above is the message minisign
  itself signs, and minisign additionally prehashes with BLAKE2b per its format).
* File name: `<archive-filename>.minisig`, in the same directory.
* The minisign *trusted comment* SHOULD carry `archive_uuid` (hex) and the
  header's `created_at`, so a verifier can bind the sidecar to a specific
  archive state.
* `flags.is_signed` SHOULD be set when a sidecar is expected.

### 8.3 Verification (reader)

`wax-core`'s reader, given a public key:

1. Recompute `signed_message` from the opened archive.
2. Load `<archive>.minisig`; verify the minisign signature over `signed_message`
   with the trusted public key.
3. Check the trusted-comment `archive_uuid` matches the header — else
   `SignatureArchiveMismatch`.
4. On any failure → `SignatureInvalid`; the reader surfaces it and the caller
   decides. `wax-core` refuses content reads from a signature-required archive
   whose signature does not verify.

With no public key configured, the reader skips signature verification (opens
unsigned) unless the caller sets a "require signature" policy.

### 8.4 Residual rollback risk (flagged, §12)

The sidecar scheme signs *a* valid `(header, segment-chain)` state but does not
prove it is the *latest* such state. An attacker who has ever observed a
previous validly signed `(archive, .minisig)` pair can serve that older pair;
both verify. Mitigations **not** in v0.9:

* No monotonic counter / timestamp is enforced across versions (the signed
  `created_at` is advisory — an attacker replays the whole old triple, matching
  `created_at` included).
* Full anti-rollback (signed version metadata, revocation, key rotation) is
  deferred to the **v2 / TUF-based** trust model.

Deployments that need rollback protection in the interim must pin the expected
`created_at` / content digest out of band. SPEC.md states this limitation
rather than implying the sidecar is sufficient.

---

## 9. Reader error taxonomy (informative)

The conformance suite (A4) asserts the *class* of failure, not the exact string.
`wax-core` groups them as:

| Group | Variants (v0.9) | Must be… |
|-------|-----------------|----------|
| `Header` | `BadMagic`, `TruncatedHeader`, `UnsupportedMajorVersion`, `IndexTooSmall`, `IndexOffsetInHeader`, `IndexOutOfBounds` | rejected at open |
| `BlobSection` | `BlobSectionLengthMismatch` | rejected at open |
| `Segment` | `NotAnIndexSegment`, `PrevSegmentOutOfBounds`, `SegmentChainCycle`, `TooManySegments`, `BrokenSegmentChain`, malformed-SQLite | rejected at open |
| `Schema` | missing `entries`, `entries` wrong shape, `UnexpectedVolumeId` | rejected at open |
| `Lookup` | `EntryNotFound`, `DanglingRedirect`, `RedirectChainTooDeep` | returned at `get`/`resolve` |
| `Content` | `UnknownCompression`, decompression failure, `ChecksumMismatch` | returned at `read` |
| `Signature` | `SignatureInvalid`, `SignatureArchiveMismatch`, `SignatureMissing` | returned by verify path |

**No malformed input may cause a panic, an out-of-bounds read, an unbounded
allocation, or a non-terminating loop.** This is the core A4 fuzz property.

---

## 10. Versioning of this document

* This is **WAX format 0.9**. The pair `(version_major, version_minor)` in the
  header is `(0, 9)`.
* Backward-incompatible change → bump `version_major`, and readers of the old
  major refuse the new archives (§2.2).
* Backward-compatible additive change (new optional table, new nullable
  `entries` column, new `flags` bit, new `segment_meta` key) → bump
  `version_minor`. Old readers keep working via §5.4.
* The crate/release version of `wax-core` / `wax-builder` (currently `1.x`) is
  **independent** of the format version. "v0.9/v1.x" in this document means
  "format 0.9, as shipped by tool releases in the 1.x line."
* Changes to §2 (header) or §5.2 (`entries`) require a version bump and a
  conformance-corpus update in the same change.

---

## 11. Conformance (A4) — required coverage

The suite in `crates/wax-core/tests/` and the fuzz crate `fuzz/` MUST cover:

### 11.1 Positive corpus (must open & behave)

| Case | Assertion |
|------|-----------|
| Minimum-size archive (one 0-byte entry, `compression="none"`) | opens; `list()` = that path; `read()` = `[]` |
| Reserved field all-`0xFF` | opens; behaves identically to zero-reserved twin |
| Unicode paths (NFC, multi-script, emoji) | round-trip exact bytes; `list()` sorted by code point |
| `redirect_to` present, depth 1 | `resolve(alias)` returns target content; `list()` shows both paths |
| Unknown higher `version_minor` + extra junk column on `entries` | opens; entries read normally; junk column invisible |
| Multi-segment (2–3 segments) with overlapping paths | last-segment-wins verified per path; non-overlapping paths from all segments visible |
| Multi-segment where a later segment turns a path into a redirect | `resolve` follows the new redirect, one hop |

### 11.2 Negative corpus (must reject cleanly — error, no panic)

| Case | Expected group |
|------|----------------|
| Bad magic (`WAX2`, random) | `Header/BadMagic` |
| File < 128 bytes | `Header/TruncatedHeader` |
| `version_major = 1` | `Header/UnsupportedMajorVersion` |
| `index_offset = 0` / `< 128` | `Header/IndexOffsetInHeader` |
| `index_offset + index_length > file_size` | `Header/IndexOutOfBounds` |
| `index_length < 512` | `Header/IndexTooSmall` |
| `blob_section_length` ≠ `index_offset − 128` (single segment) | `BlobSection/BlobSectionLengthMismatch` |
| Σ `blob_region_length` ≠ `blob_section_length` (multi-segment) | `BlobSection/BlobSectionLengthMismatch` |
| Index segment bytes not a SQLite db | `Segment` (malformed SQLite) |
| Segment missing `entries` | `Schema` |
| `segment_meta.prev_segment_offset` out of bounds | `Segment/PrevSegmentOutOfBounds` |
| Segment chain with a cycle | `Segment/SegmentChainCycle` |
| `redirect_to` chain of depth 2 on disk | `Lookup/RedirectChainTooDeep` at `resolve` |
| `redirect_to` to a nonexistent path | `Lookup/DanglingRedirect` at `resolve` |
| `volume_id = 1` in a v0.9 archive | `Schema/UnexpectedVolumeId` |
| Entry `compression = "brotli"` | `Content/UnknownCompression` at `read` |
| Entry blob truncated / `sha256` wrong | `Content/ChecksumMismatch` or decompression error at `read` |

### 11.3 Fuzz targets (`cargo fuzz`)

| Target | Input | Property |
|--------|-------|----------|
| `header_parse` | arbitrary ≤ 4 KiB | `Header::parse` + `validate(file_size)` never panics; returns `Ok`/`Err` |
| `index_loader` | arbitrary ≤ 256 KiB, written to a temp file and opened as a segment | segment open + `entries`/`segment_meta` read never panics; bounded memory |
| `segment_merge` | `arbitrary`-derived model of 0–8 synthetic segments with overlapping paths/redirects | chain walk + merge + `resolve` never panics, never infinite-loops, one-hop redirect rule holds |

Each fuzz target's body is also callable as a plain function
(`wax_core::fuzz::check_*`) and is exercised by the normal `cargo test` run with
the checked-in corpus, so the properties are enforced on platforms where
libFuzzer is unavailable (see §12).

---

## 12. Implementation notes / assumptions (flagged deviations & open items)

Points where this document goes beyond, interprets, or cannot verify the
Refinement doc. These are surfaced deliberately for the design-doc feedback loop.

1. **Header endianness = little-endian.** Not stated in the Refinement doc.
   Chosen for zero-copy header mapping on LE targets. A BE target would need a
   swap; no such target is planned.
2. **`flags` bit positions.** Doc lists four flag names in order; bits 0–3 are
   assigned in that order here. `is_signed` (bit 3) is informational; the
   reader's verify policy does not branch on it.
3. **Segment-chain linkage mechanism (`segment_meta` + `prev_segment_*`).**
   The doc requires the chain shape and last-segment-wins merge but does not say
   how a reader discovers earlier segments from the header's single pointer.
   The backward-link table introduced in §4.2 is the derived mechanism. **This
   is the largest derived element and should be reviewed against the doc's
   intent.**
4. **`blob_section_length` semantics under multi-segment.** Defined here as the
   sum over all blob regions, with a single-segment fast-path equality
   (`index_offset == 128 + blob_section_length`). The doc calls it "total blob
   body size, for fast integrity pre-check"; the multi-segment generalization
   is derived.
5. **`sha256` is over uncompressed content.** The doc names the column but not
   its input domain. Uncompressed content chosen because that is what a reader
   can verify post-decompression and what a server ultimately emits.
6. **`compression` value set = {`none`, `zstd`}.** The doc says entries carry a
   `compression` column but does not enumerate values for v0.9. Any other value
   is a read-time `UnknownCompression` error (no guessing).
7. **Index segments are stored uncompressed.** The doc does not say. SQLite
   databases are stored raw so the reader can open them without a decompress
   pass; this matches the existing implementation.
8. **`created_at` is `u64` seconds (unsigned).** The doc says "unix epoch
   seconds"; unsigned chosen (no pre-1970 archives) to simplify bounds.
9. **Minisign prehashed mode + `<archive>.minisig` name + trusted-comment
   binding.** The doc fixes "minisign over that combined digest, external
   `.minisig` sidecar"; prehashed mode, the exact filename, and the
   `archive_uuid` trusted-comment binding are specified here to make
   verification unambiguous.
10. **`reserved` bytes: reader ignores entirely.** Confirmed by the corpus case
    "max reserved-field values (must open)". Writer still MUST zero them.
11. **`manifest` content is out of scope (Track B).** Per the Refinement doc's
    A2/Track-B boundary. `wax-core` treats it as an opaque string map in this
    phase; `wax-builder` writes an empty `manifest` table if Track B has frozen
    no keys.
12. **Open item — append atomicity on ext4 / F2FS-on-SD under power loss**
    (§7.3). Not verifiable in this environment. Recorded as an assumption; a
    header CRC + A/B header slot in `reserved` is the likely future fix.
13. **Open item — residual signature rollback risk** (§8.4). Documented, not
    fixed in v0.9; deferred to v2/TUF.
14. **Open item — wax-delta chunker (FastCDC assumed).** Out of scope for
    A3/A4; not blocked on here. Noted for completeness.
15. **libFuzzer availability.** `cargo fuzz` needs a nightly toolchain. It was
    verified to **build and run on `x86_64-pc-windows-msvc` (nightly)** as well
    as the usual Linux/macOS path; short local runs are clean. The fuzz
    *properties* are additionally enforced without libFuzzer via the
    `wax_core::fuzz::check_*` functions under `cargo test`. Large-scale /
    long-duration corpus fuzzing is still a CI activity and a known gap until
    real content (`zim2wax` output) exists.
16. **No real-world corpus yet.** `zim2wax` does not exist; all fixtures are
    hand-built and small. Broad fuzzing against realistic archives is deferred.
17. **SQLite index has a per-segment size floor.** With the writer's default
    `PRAGMA page_size = 4096`, an index segment is ≥ ~20–32 KiB even when it
    describes a single entry, so the "minimum-size archive" fixture is ~33 KiB,
    almost all index overhead. This is acceptable for real packs (thousands to
    millions of entries) but makes tiny archives disproportionately large. If a
    use case needs small archives, `page_size = 512` drops the floor ~8×. Not a
    spec conflict — flagged because it is a real consequence of the
    "SQLite footer" decision that the Refinement doc may want to record.

---

## 13. Change log

| Format version | Date | Change |
|----------------|------|--------|
| 0.9 | 2026-09-09 | Initial frozen specification (A3). Header 128 B; SQLite segment chain; `entries` / `manifest` / `segment_meta` / `signatures` / optional `search_index`; append-commit protocol; detached minisign sidecar signing model. |
