# DeltOS — Track A

Format & Storage Engine — Technical Refinement (v2, reviewed)

*Working draft · September 2026 · revised after independent technical review (§12) and two rounds of cross-track amendment (§13-§14)*

## 1. Scope

Track A owns the .wax container itself — everything from the byte-exact header layout through the delta/patch engine and signing scheme. This is the track every other track builds on, so its open questions get resolved here first, before B–H can assume answers that don't exist yet.

## 2. WAX v0.9 Header — Proposed Byte Layout

128 bytes (two 64-byte cache lines) — revised from the 64 bytes in the current wax-core prototype. The extra 64 bytes exist specifically so the v2 search-index pointer has a named home instead of being squeezed into 8 leftover bytes (see §12, finding 3).

| **Offset** | **Field** | **Size** | **Notes** |
|---|---|---|---|
| 0–3 | magic | 4B | ASCII "WAX1" — readers reject anything else. |
| 4 | version_major | 1B | Breaking changes bump this. A reader must refuse an unknown major version rather than guess. |
| 5 | version_minor | 1B | Additive-only changes. A reader ignores unknown minor-version index columns instead of failing. |
| 6–7 | flags | 2B | Bit flags: has_search_index · has_delta_base · is_multi_volume · is_signed. |
| 8–23 | archive_uuid | 16B | Stable identity for this pack across versions — what a delta update matches against. |
| 24–31 | created_at | 8B | Unix epoch seconds. |
| 32–39 | index_offset | 8B | Byte offset to the index footer (base segment's manifest — see §4). |
| 40–47 | index_length | 8B | Length of the index footer in bytes. |
| 48–55 | blob_section_length | 8B | Total size of the blob body, for a fast integrity pre-check. |
| 56–63 | search_index_offset | 8B | Offset to the stand-off v2 search segment (§7). 0 = absent, e.g. all of v0.9. |
| 64–71 | search_index_length | 8B | Length of that segment. Named explicitly rather than left in generic reserved space. |
| 72–127 | reserved | 56B | Zero-filled, generous headroom for v1.x/v2 fields not yet identified. |

## 3. Index Schema (SQLite Footer)

Keep SQLite as the documented, first-class index format — not an internal detail abstracted away later. Proposed tables:

- **entries:** path TEXT PRIMARY KEY, title TEXT, offset INTEGER, length INTEGER, uncompressed_length INTEGER, mime TEXT, compression TEXT, sha256 BLOB, volume_id INTEGER DEFAULT 0, redirect_to TEXT DEFAULT NULL. Each entry's bytes are stored as one contiguous span — see §5 for why this stays simple rather than moving to chunked storage. title (amendment — added during the Track B review, before implementation) is the separate human-readable label an entry needs alongside its path/URL — ZIM keeps Title and URL distinct, and the original schema had no field to carry it, silently conflating the two. Nullable, since not every entry (e.g. an image) has a meaningful title. redirect_to (amendment — added during Track B design, before implementation, once zim2wax needed a way to represent ZIM's redirect entries) lets a path alias to another canonical path, with offset/length ignored when set; wax-builder flattens redirect chains to a single hop at build time so no reader ever follows more than one indirection.
- **manifest:** key TEXT PRIMARY KEY, value TEXT — B3's pack manifest fields (name, icon, category, license, version, min_hw_tier). Lives only in the base segment (segment 0) and is immutable across appends — a manifest change is a new pack version, not an append. This removes the otherwise-undefined question of how two segments' manifest tables would merge.
- **search_index:** optional SQLite FTS5 virtual table for v0.9 (see §7) — the section A9 refers to in its v0.9 form.
- **signatures:** signer TEXT, algo TEXT, signature BLOB, signed_at INTEGER. Defined now but inactive through v0.9/v1.x, which sign via an external sidecar file (§6) — this table only starts being written once v2's TUF-based multi-key signing (F7) lands. Calling this out explicitly avoids the two mechanisms being confused.

Segment-spanning note: the entries PRIMARY KEY is enforced per segment (each segment is its own SQLite file), not globally — a reader merges the segment chain with last-segment-wins semantics per path. A second-language implementation (A10) needs this merge behavior specified, since the schema alone doesn't express it.

## 4. The Decision That Gates Everything Else: Segmented vs. Single-Footer Index

This is the one call in Track A that has to be made in P0, not discovered later — it changes the read path, and changing it after B/C/D are built against it is expensive.

- **v1 today (single footer):** One SQLite index, written once at the end of the file. Simple, fast to read, but write-once — exactly ZIM's limitation, inherited rather than fixed.
- **v2 proposal (segmented index):** The index becomes a chain of segments: a base segment plus zero or more append segments, each a small SQLite database covering only what changed. A reader unions them at open time. An append writes new blob bytes and a new segment to the end of the file first.
- **Append commit protocol (added after review):** The blob section and every previously-written index segment are never rewritten — but the header's pointer fields (index_offset, index_length, blob_section_length) necessarily are, since they live in the base file. The correct sequence is write-then-commit-pointer: append the new blob bytes and new segment, fsync, then overwrite the fixed 128-byte header in place as the last, atomic step. Only that final small, fixed-size write touches existing bytes — everything else is pure append. This replaces the earlier, imprecise claim that "the base file's bytes are never rewritten."
- **Recommendation:** Adopt the segmented index for v2, but ship v0.9/v1.x with a single segment (segment count = 1) — the on-disk shape is future-proof from day one even though early versions never exercise the multi-segment path.

## 5. Delta / Patch Engine (A6)

Revised after review: chunking is a technique wax-delta uses to compute a patch, not a change to how blobs are stored on disk. §3's entries table is unchanged — each version of a file is still one contiguous span, which keeps A1 (the reader) and A5 (byte-range serving) simple.

- **Computing a delta:** wax-delta reads the old and new versions of a pack directly (each via its own entries' offset/length), splits each changed blob into content-defined chunks with FastCDC purely in memory, and diffs the two chunk sets to produce a patch: a small set of "copy these bytes from the old file" and "here are new literal bytes" instructions, plus a new index segment describing the result.
- **Where this runs (added after review):** Delta computation is CPU/memory-heavier than delta application. It's scoped to run on the publishing/catalog service (B13, per Track B's B4/B13 split — B4 is strictly the on-device client) when a pack update is published, or on Community Hub-tier hardware for box-to-box peer sync — never required on Kiosk-tier (Pi Zero 2 W) boxes. Those boxes only ever apply a received patch: append new bytes, append a new segment, commit the header per §4 — cheap regardless of hardware tier.
- **Append vs. patch:** "Append" (pure addition) is the trivial case: a new segment with only new entries. "Patch" (an existing entry changes) is the general case using the diff above. One mechanism covers both.

## 6. Signing (A7)

- **What gets signed (revised after review):** A digest over the header bytes together with the current index (segment-chain state) — not the header alone or the index alone. Excluding the header left flags, archive_uuid, and the pointer fields themselves unauthenticated, meaning an attacker could tamper with any of them without invalidating the signature.
- **v0.9/v1.x mechanism:** minisign over that combined digest, shipped as an external sidecar .minisig file next to the .wax. Re-signed on every append, since the header changes on every append (§4).
- **Known residual risk: rollback (added after review):** Signing the header closes tampering, but not rollback — an attacker who retains an old, validly-signed (header, index, .minisig) triple could reintroduce it wholesale, silently reverting a pack to older content without breaking the signature. Full protection (monotonic version enforcement) is deferred to v2's TUF integration (F7). As a v0.9/v1.x mitigation, a reader should track the highest archive_uuid + version it has seen and refuse to open an older, otherwise-validly-signed version for the same UUID — timing corrected after Track F's review (§14): a P2 requirement, not a P4 one, since Track F's F4 install pipeline and Track C's C7 both apply this check from the point content first becomes installable, not as later hardening.
- **v2+ mechanism:** Move the signature into the signatures table (§3) once TUF-based multi-key management (F7) exists, so a pack can carry multiple signers/roles.

## 7. Embedded Search-Index Section (A9)

Sequenced to match Track D's own tiering:

- **v0.9:** A SQLite FTS5 virtual table inside the index footer — cheap, works on a Pi Zero, and is what Track D's D1 tier-0 search reads directly. No new section format needed.
- **v2:** A stand-off Tantivy segment referenced by the header's search_index_offset/search_index_length fields (§2 — now named explicitly rather than borrowed from generic reserved space), built once Track D's Tantivy integration has stabilized enough to define what that segment needs to hold.

## 8. Multi-Volume Archives (A8)

Volume 0 carries the header and the full index; the entries table's volume_id column says which physical file (name.wax.0, name.wax.1, …) actually holds a given blob. The index is never split across volumes — only blob data is.

## 9. Open Decisions Needed Before P1

- Confirm the §4 append-commit protocol (write-then-commit-pointer) matches what wax-core's writer can actually guarantee atomically on the target filesystems (ext4, F2FS on SD cards) — a torn header write during power loss is the one failure mode this protocol has to survive.
- Confirm FastCDC (or name an alternative chunker) for §5 — affects wax-builder's dependency list now, not later.
- Decide the v0.9/v1.x rollback mitigation from §6 (device-side "highest version seen" tracking) — sign off on it as a tracked P2 item (corrected after Track F's review, §14) rather than leaving it implicit.
- Decide whether A10 (second-language reference reader) happens in P0 or P1 — recommend P1, once the header/index shape from this document is frozen, so it validates the real spec rather than a moving target. It must also implement the §3 segment-merge (last-segment-wins) behavior, not just parse a single segment.

## 10. Refined Phase Sequencing for Track A

| **Phase** | **Track A deliverable** |
|---|---|
| P0 | SPEC.md (A3) frozen, including the §2 128-byte header layout and the §4 segmented-index + commit-protocol decisions. Conformance/fuzz harness (A4) stood up against v0.9's single-segment shape. |
| P1 | Streaming writer (A2); minimal wax-serve byte-range reads (A5); second-language reference reader (A10) built against the now-frozen spec, including segment-merge behavior. |
| P2 | SQLite FTS5 section (A9, v0.9 form) standardized in coordination with Track D's D1 — this is the form that discharges the master plan's P2 cross-track dependency, not the v2 Tantivy form. Pack signing (A7) enforced by default, including the §6 rollback mitigation — moved forward from P4 after Track F's review (§14), to match when Track B's B13 and Track F's F4 actually need it live. |
| P3 | Multi-volume archives (A8). |
| P4 | wax-delta (A6, hub/Community-Hub-side computation per §5) built on the segmented-index design. |
| P5 | Tantivy stand-off segment (A9, v2 form) once Track D's D1/D2 stabilize; format frozen as v2. |

## 11. What This Unblocks

- Track D can build D1 against a concrete v0.9 shape (§7's FTS5 table) immediately, with the master plan's terminology now corrected to match (§12).
- Track C's wax-serve dependency (C1/C2 opening a real pack) is unblocked as soon as P1's A5 lands.
- Track G's reverse proxy (G2) and orchestrator (G1) don't depend on Track A and can proceed in parallel.

## 12. Independent Review — Findings and Resolutions

This document was reviewed by a separate pass before being treated as settled, specifically to avoid the spec grading its own homework. Six of seven findings were valid and are folded into the sections above; the seventh is noted as scoped rather than eliminated.

- **[Critical]** §3/§5 originally described incompatible storage models — a contiguous per-entry blob layout alongside a permanent cross-file chunk store the schema had no room for. Resolved: chunking is now explicitly a transient technique internal to wax-delta, not an on-disk format change (§5).
- **[Critical]** §4 claimed the base file's bytes are "never rewritten" by an append, while the header holding index_offset is itself part of that file. Resolved: added the explicit write-then-commit-pointer protocol and corrected the claim's scope (§4).
- **[Critical]** 8 reserved header bytes can't hold a 16-byte offset+length pointer, the format's own convention for that kind of reference. Resolved: header extended to 128 bytes with search_index_offset/search_index_length named explicitly (§2).
- **[Security]** The signature covered only the index, leaving flags, archive_uuid, and the pointer fields themselves unauthenticated, plus an unaddressed rollback path. Resolved: signature now covers header+index together; rollback risk is explicitly documented with a proposed v0.9 mitigation and full fix deferred to v2/TUF, rather than left unaddressed (§6).
- **[Moderate]** The master Track & Phase Plan's cross-track dependency line didn't distinguish A9's FTS5 form (P2) from its Tantivy form (P5), risking Track D planning against the wrong one. Resolved here (§10) and patched in the master document.
- **[Moderate]** The entries PRIMARY KEY and manifest-table behavior across a segment chain weren't specified. Resolved: reader-side last-segment-wins for entries, manifest confined to the base segment only (§3).
- **[Scoped, not eliminated]** Delta computation could be too heavy for a Pi Zero 2 W. Resolved as a byproduct of the chunking fix: computation is scoped to hub/Community-Hub tier; Kiosk-tier boxes only ever apply a received patch (§5).

## 13. Post-Review Amendments (from Track B design and review)

Two further schema changes, made while designing and then reviewing Track B, before any implementation exists to be broken by them:

- **redirect_to (during Track B design):** entries gained a redirect_to column (§3). zim2wax needs a way to represent ZIM's redirect entries — Wikipedia-scale ZIMs contain huge numbers of them — and the original schema had no way to alias one path to another without duplicating content. See the Track B Refinement, §3, for the full rationale.
- **title (from Track B's independent review):** entries gained a title column (§3). ZIM keeps Title and URL as distinct concepts — a page's display title and its storage path are not the same string — and the original schema conflated them by only carrying path. zim2wax needs a destination for the ZIM Title field; see the Track B Refinement, §4, for how the mapping table uses it.

Also corrected: §5's "where this runs" note referenced "wax-hub, B4" for delta publishing — stale terminology from before Track B split that single component into B4 (on-device catalog client) and B13 (catalog publishing service). It now correctly names B13, since delta computation is a publishing-side operation. Flagging both here so Track A's spec history stays in one place rather than scattered across documents.

## 14. Post-Review Amendment (from Track F's review)

Track F's independent review caught a genuine three-way scheduling conflict this document had introduced: §6's rollback mitigation and A7's signing enforcement were tagged here as P4 hardening items, "not a P0 blocker" — but Track B's B13 (catalog publishing) and Track C's C6/C7 (install flows) were already scheduled to depend on signed, verified packs from P2, and Track F's own F4 install pipeline was built assuming the same check is mandatory from wherever F4 first runs. Nothing had reconciled Track A's own P4 tag against what three other tracks already assumed.

- **Resolved by moving the tag, not the mechanism:** Both the rollback mitigation (§6) and A7's signing enforcement now land at P2, matching the phase where Track B's B13 first has anything to sign and Track C's C6/C7 first need the check live (§9, §10). The mechanism itself is unchanged — this is a scheduling correction, not a design change — and it removes what would otherwise have been a real gap: content installable from P2 with no rollback protection active until P4, on the exact threat model (a stale, still-validly-signed pack) that protection exists for. See the Track F Refinement, §4 and §16, for how this lands on that document's own phase table.

## 15. Implementation Resolution — A3/A4 Build (SPEC.md + wax-core)

The first implementation pass (SPEC.md, wax-core reader/writer, conformance suite, fuzz harness — branch track-a/spec-and-conformance) surfaced one genuine self-contradiction in this document and a set of byte-exact details this document specifies as a schema/behavior but never pins to a concrete on-the-wire value. Each is resolved below and is now settled design, not an implementation-time guess — SPEC.md should be treated as carrying these, superseding the affected prose in §2–§8 wherever it's silent or in conflict.

- **[Critical]** §2's header-field table describes index_offset as pointing to "the index footer (base segment's manifest)" — i.e. always the base segment — while §4's append-commit protocol requires index_offset to be rewritten on every append. Taken together these contradict each other once a second segment exists: a reader given only the header has no way to enumerate segments 0…N-1 from a single "base segment" pointer that never moves, yet the pointer is specified to move. Resolved: index_offset/index_length always point to the newest (most recently written) segment, not the base — §2's "base segment" wording was accurate only for v0.9's single-segment case and is corrected. Each non-base segment carries its own prev_segment_offset/prev_segment_length back-link (new segment_meta key/value table, alongside entries/manifest/search_index/signatures in §3) so a reader walks backward from the header's pointer to the base, then merges forward, last-segment-wins. This is the concrete form of the "reader merges the segment chain at open time" behavior §3 and §4 already required but never mechanized.
- **blob_section_length under multiple segments:** §2 defines this field only as "total size of the blob body" against v0.9's single contiguous span; once appends interleave [blob][index][blob][index]… segments, no single contiguous span exists. Resolved: the field is the sum of every segment's blob_region_length (tracked per-segment in segment_meta), and v0.9's single-segment case keeps the cheap header-only invariant index_offset == 128 + blob_section_length as a fast path.
- **Header endianness — unspecified in §2:** Resolved: little-endian, fixed for all versions. Matches every current and planned target architecture (x86_64, ARM64); a reader for a hypothetical big-endian host must byte-swap on load.
- **flags bit positions — unspecified in §2:** §2 lists has_search_index · has_delta_base · is_multi_volume · is_signed with no bit numbers. Resolved: bits 0–3 respectively, in that listed order. is_signed is informational only — reader verify behavior is driven by the presence of a .minisig sidecar (§6), not this bit.
- **sha256 column input — unspecified in §3:** entries.sha256 is defined but §3 never says whether it hashes compressed or uncompressed bytes. Resolved: SHA-256 of the entry's uncompressed content — what a reader returns and what wax-serve (A5) would stream — so the hash is stable across a future re-compression of the same content. Verified by default on read; opt-out flag provided for the byte-range-serving path where re-reading the full entry to verify defeats the point.
- **compression value set — unspecified in §3 for v0.9:** Resolved: {"none", "zstd"} for v0.9/v1.x. Any other value is a hard per-entry read error (UnknownCompression), never a silent skip — matches this document's general "refuse rather than guess" posture (§2's magic/version handling, §12's original findings).
- **Minisign sidecar specifics — under-specified in §6:** §6 fixes the mechanism (minisign, external .minisig, re-signed every append, digest over header+index together) but not the exact digest construction or file convention. Resolved: digest = SHA-256(header bytes ‖ all segment bytes, in chain order); prehashed signing mode (-H); sidecar named <archive>.minisig; the trusted comment binds archive_uuid so a swapped-in signature from a different pack's chain fails verification even if the raw signature bytes were otherwise valid.
- **volume_id enforcement ahead of A8:** §8 (multi-volume) is v2/P3 scope; this document never states what a v0.9 reader should do if it sees a non-zero volume_id anyway. Resolved: hard error (UnexpectedVolumeId) rather than silent ignore — a v0.9 file has no business claiming a second volume, and silently accepting one would mask a real corruption or a builder bug.
- **Smaller implementation details, resolved conservatively:** Index segments are stored uncompressed (raw SQLite pages) — no compression layer over the footer itself. created_at is an unsigned 64-bit second count. A reader does exact-match path lookups only; normalizing a lookup path (case, unicode form, trailing slash) is a builder-time obligation, not a reader behavior. Reserved header bytes are fully ignored on read, confirmed by a conformance case that opens a file with reserved set to all-0xFF. An empty manifest table (no rows) is valid and opaque to wax-core — Track B's B3 manifest-field population is out of this layer's scope.
- **Observed consequence, not a spec change — SQLite footer floor size:** With the writer's default SQLite page_size of 4096, even a one-entry archive's index footer is roughly 20–33 KiB, dominating a tiny archive's total size. Real packs (thousands of entries) amortize this away; it's noted here as a real cost of the "index is documented SQLite, not a custom format" decision in §3, not a defect — a smaller page_size is available later if minimum-pack-size ever becomes a real constraint (e.g. very small Track H component packs).

Left deliberately open, per this document's own §9 — not resolved by the implementation pass and not silently assumed: whether the §4 write-then-commit-pointer header overwrite is atomic under power loss on ext4 / F2FS-on-SD (flagged as an assumption pending real-hardware testing), and the §5 chunker choice for wax-delta (A6, out of scope for this A3/A4 pass).

## 16. Implementation Resolution — A2 Build (wax-builder)

The second implementation pass (wax-builder: assembly, manifest, signing hook, CLI — branch track-a/wax-builder) ran without access to the Track B Refinement doc a third time (see §17) and surfaced one real gap this document can close directly, since Track B's B3 schema already exists and settles it.

- **[Moderate]** wax-builder currently writes every [manifest] key from its config file straight through as TEXT with no validation — category and min_hw_tier accept any string, total_size_bytes is treated as a passthrough value rather than computed, and the field set itself was treated as illustrative rather than exhaustive. Resolved: Track B §2's B3 schema (reproduced here so this document is self-sufficient on the point) is the exhaustive field list, not a sample. wax-builder must validate against it, not merely pass it through.

The field table that stood here has been removed rather than updated. It was a second copy of Track B §2's schema, reproduced so this document would be self-sufficient, and the two copies drifted exactly as duplicated vocabularies do: Track B gained a guest_accessible field per product direction, this copy never did, and wax-builder — enforcing against this copy — rejected a valid manifest as carrying an unknown key. The normative field list now lives in one place, the Cross-Track Contract §11, and both this document and Track B §2 cite it. The rule below still holds and is restated there.

- **No id field:** Deliberately absent — an earlier draft's id duplicated archive_uuid without adding meaning and was dropped during Track B's own review. wax-builder must not invent one; archive_uuid (Track A §2, header field) is the only identity a pack carries.
- **archive_uuid vs. determinism (A2 brief items 3 and 6):** Confirmed correct: a fresh UUIDv4 by default, an explicit override flag for pinning it, and note that a byte-identical rebuild test necessarily pins archive_uuid and every timestamp (both header.created_at and each segment's segment_meta.created_at, per §15's already-resolved dual-timestamp point) — those are two different, legitimately distinct clocks (pack creation vs. this segment's write time), not one value duplicated by mistake, so both need pinning independently rather than one implying the other.
- **SPEC §8.2 minisign flag correction:** Confirmed: -H is a verify-side flag in minisign 0.12 (plain -S to sign, -V -H to verify), not a sign-side flag as originally written. The design intent (prehashed signing) is unchanged; only the CLI invocation in SPEC.md's §8.2 was wrong and is corrected.
- **Judgment calls, endorsed as-is:** Manifest change on append is a hard error (matches this document's own §5.6 "a manifest change is a new pack version"). Compression policy is extension-based (verbatim for already-entropy-coded formats, zstd otherwise) rather than compress-and-pick-smaller — deterministic and cheap beats marginally smaller. No minisign password env var — unattended builds need a password-less key; documented, not worked around.

## 17. Implementation Resolution — A2b Build (manifest enforcement)

Enforcing §16's schema in wax-builder surfaced two structural problems with a single field and one genuine internal contradiction. All three are resolved below; the first is a change to Track B's B3 schema, recorded in that document's own §19 as well.

- **[Critical]** total_size_bytes cannot work as a manifest field. Two independent failures, both structural rather than implementation defects: (a) it is self-referential — the value records the archive's own size but lives inside that archive, so a longer decimal grows the index, which grows the file, which changes the value; the implementation needed a write-measure-rewrite fixed-point loop, at the cost of compressing every pack twice. (b) It goes permanently stale on append — the manifest is segment-0-only and immutable across appends (§5.6), while an append by definition grows the file, so after any append the recorded value understates reality and, by design, nothing can ever correct it. Resolved: the field is removed from B3 entirely. Track B §5's on-device catalog already carries packs.size, populated by B13 — which is the single point where a pack is signed and published (Track B §5, §9) and therefore always holds the actual file, can measure it exactly, can re-measure after an append, and can sum across A8 volumes. The stated purpose of the field — letting the catalog and Track C's install UI show a real download size without opening the archive — is the catalog's case precisely, and the catalog already served it. The manifest copy was redundant with packs.size and was the copy carrying both defects.
- **The principle behind that removal, stated so it generalizes:** A manifest carries what only the pack's author knows; a catalog carries what any holder of the file can measure. Archive size is measurable by anyone holding the archive, so it belongs to the catalog. runtime_ram_bytes and runtime_storage_bytes stay in the manifest under exactly the same rule — a pack's steady-state memory footprint and its writable-storage needs are authored properties that no amount of inspecting the file will reveal, and neither is self-referential.
- **[Moderate]** depends_on was defined as "comma-separated pack ids" in the same table whose next note removes id and states that archive_uuid is the only identity a pack carries — the two cannot both hold, leaving a consumer resolving dependencies with no defined referent. Resolved: depends_on carries comma-separated archive_uuid values. This is consistent with Track B §5, where B4's packs table is keyed on archive_uuid and the catalog groups a pack's version history by it directly.
- **Scoped, not eliminated — depends_on has no version constraint:** Because archive_uuid is deliberately stable across a pack's versions (§2), depends_on can express "needs pack X" but not "needs pack X at version ≥ N". That is adequate for the field's current scope (an optional hint, e.g. an office-suite pack naming a cloud-storage pack) and is not worth inventing a constraint syntax for before a real dependency exists to constrain. Flagged so it is a known limit rather than a later surprise.
- **icon and entry_point — validation is required, and §16's earlier wording was wrong:** Both must be verified at build time to resolve to real entries in the archive; a manifest naming a nonexistent launch target is a broken pack that would otherwise fail only once it reached a device. Correcting this document's own earlier gloss: §16 previously said icon must not be "a filename alone," which overreached — Track B §2 says only "path within the archive," and a root-level icon.svg is both a bare filename and a perfectly valid path. Rejecting it would have broken valid packs. Data/http/file URIs remain invalid, and tolerating a leading / or ./ in config while writing the configured spelling through verbatim is correct.
- **Unknown manifest keys are rejected outright:** Confirmed as the right reading of "exhaustive field list, not a sample" — id keeps its own dedicated error message as the most likely instance, but any off-table key is a build error. Adding a field to B3 is a coordinated schema change across Track A, Track B and this document, never a silent extension by a config file.
- **Deferred, with a named trigger — the double-compression cost disappears with the field:** The fixed-point loop existed only to settle total_size_bytes; removing the field removes the loop and the second compression pass with it. The separate prepare-once/write-many refactor in wax-core (staging compressed blobs so any multi-pass write reuses them) is therefore no longer urgent for A2b, but remains worth doing before B1 builds Wikipedia-scale packs, since A5's byte-range serving and zim2wax's own pipeline both benefit from the same staged-blob API.

## 18. Scale Finding from B1 — the Deferred Refactor's Trigger Has Fired

B1 converted a real 333 MB English Wikipedia ZIM end to end: 9,337 dirents into 5,542 content entries and 3,729 redirects in 8.8 seconds, with every one of the resulting 9,272 entries re-read and verified. That is the first time this format has met production-scale content, and it produced one hard result — the deferral recorded in §17 is now due.

- **The reason for the refactor has changed, and the old reason is gone:** §17 deferred a "prepare-once/write-many" path so that a multi-pass write could reuse already-compressed blobs. That rationale existed only to soften the cost of the total_size_bytes fixed-point loop, and A2c removed the field and the loop with it — there is no second pass left to optimize. What remains is a different and larger problem: wax-core's writer takes a Vec of entries, so a conversion materializes every blob in memory at once. 333 MB fits comfortably; a full Wikipedia at roughly 100 GB does not, and neither does anything approaching it on a box whose lowest tier has 512 MB of RAM in total. The refactor needed now is streaming, not staging.
- **The format already permits it:** A .wax file is a header, then a blob body, then an index footer, and §4's append-commit protocol already writes the header last. So a writer can stream: reserve the header, write blob bytes as they arrive while recording each entry's offset and length, build the index as it goes, then write the footer and commit the header. Nothing in the byte layout has to change — this is an API and a memory-discipline change inside wax-core, not a format revision.
- **The index must stream too:** Accumulating index rows in memory instead of blobs only moves the ceiling. At Wikipedia's roughly six million entries, the rows alone would run to the low gigabytes. The index is a SQLite database and SQLite writes incrementally, so rows are inserted into the index file as entries arrive rather than collected and written at the end.
- **Peak memory must not scale with archive size:** That is the acceptance criterion, and it is measurable: converting a larger archive should not raise peak resident memory meaningfully above converting a smaller one. A fixed budget that holds at 333 MB but not at 100 GB has not solved the problem, it has moved it.

## 19. Process Note — Repeated Document-Access Gap (closed)

Three consecutive implementation passes (A3/A4, A2, and the start of A2b) ran without the Track & Phase Plan v2 or any Track Refinement document actually reachable from the wax-format repo — the first time because only inline-pasted text was available, the second and third despite the gap being flagged after each. That was a recurring process failure, not a one-off: every prompt assuming docs/ existed produced exactly the category of gap catalogued in §15, §16 and §17, because the implementer was reconstructing settled design from fragments instead of reading it.

Closed during A2b: this document now lives in the repo at docs/track-a-refinement.docx, alongside a verified plain-text extraction (docs/track-a-refinement.md) and the dependency-free script that regenerates it. Worth noting what the fix immediately bought — the two findings above (total_size_bytes and depends_on) are both cases where the implementer could finally see two parts of the schema at once and notice they contradicted each other. Neither was findable from the field list alone, which is all the earlier passes ever received.
