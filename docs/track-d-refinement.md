# DeltOS — Track D

Search & Intelligence — Technical Refinement

*Working draft · September 2026 · fourth of eight per-track refinements, revised after independent technical review — see §16*

## 1. Scope

Track D owns everything that turns installed content into an answer: full-text search across every pack, an optional semantic layer on top of it, the tokenization that makes non-English content searchable at all, and the tiered local LLM/RAG stack surfaced through Track C's C11. Three tracks already made commitments this document has to honor exactly: Track A reserved header fields for a v2 search segment (A9, §7), Track B's B11 geocoding assumed a specific FTS5/Tantivy tiering (§8), and Track C's C3/C11 assumed a single aggregated search endpoint and a specific security contract for RAG-retrieved content (§4, §11). This document settles those open items rather than leaving them for whoever implements first.

## 2. Full-Text Search Service (D1)

The master plan says D1 is "Tantivy-based, replacing Xapian" — true at v2, but Track A's own sequencing (A9, §7) makes v0.9/v1.x an embedded SQLite FTS5 table per pack, and Track C's C3 needs one aggregated endpoint, not per-pack fan-out. Something has to own turning N per-pack indexes into one answer.

- **deltos-searchd:** A persistent local service — the concrete answer to what Track C's C3 calls "a single local endpoint that already does cross-pack aggregation and ranking" (Track C §4). It is the only thing C3 talks to; C3 never opens a pack's A9 section directly.
- **The "omnibus table" idea didn't survive review — corrected below:** The first draft proposed one merged FTS5 table across every installed pack, framed as a cheap incremental union. Review caught two real problems: SQLite FTS5's tokenizer is fixed per virtual table at creation and applies to every row in it, but D3 (§4) already has different packs using different tokenizers (ICU for CJK/Indic, default unicode61 otherwise) — one shared table can't correctly host both. And there's no cheap ATTACH-based union that avoids the cost anyway: merging necessarily means copying each pack's indexed text into deltos-searchd's own database and re-indexing it, not a free reference. The corrected design below is tier-adaptive rather than pretending the cost away.
- **Kiosk tier: direct per-pack query, no merged catalog:** Track C's own hardware scoping (Track C §12) treats Kiosk as a single-reader deployment with a small number of installed packs — small enough that querying each pack's own search_index directly (via A1's reader, opened once per query) is acceptable, and the copy/re-index cost of a merged catalog isn't worth paying on the tier that can least afford it. deltos-searchd still presents one aggregated endpoint to C3 at this tier; it just implements it as live per-pack fan-out over a small N rather than a maintained copy.
- **Classroom tier and above: a merged catalog, partitioned by tokenizer:** Where install counts are large enough that per-pack fan-out stops being cheap, deltos-searchd maintains one merged catalog — but as a small set of omnibus FTS5 tables, one per distinct tokenizer configuration in actual use (e.g. default, ICU-CJK, ICU-Indic), not one table forced to hold every language. A query is dispatched to every tokenizer-partition table in use (bounded by the number of languages actually installed, typically small) and the results are merged at the query layer — keeping the "don't fan out over every installed pack" benefit without the tokenizer conflict.
- **Segment-chain merging isn't D1's problem to solve twice:** A single pack can itself accumulate multiple append segments over its life (Track A §4), each with its own search_index table. deltos-searchd never merges a pack's own segment chain itself — it asks Track A's A1 reader library for that pack's current, already-merged search_index content (A1 already implements the last-segment-wins merge Track A §3 specifies) and only re-indexes that result into the catalog. One merge mechanism, owned by the layer that already has to implement it for every other reason.
- **Sync trigger:** deltos-searchd subscribes to the same B4 change-notification event Track C's C2 already subscribes to (Track C §3) — a pack install, update, or removal triggers a re-index of that pack's contribution to the merged catalog, not a full rebuild. Flagged as an open confirmation, not an already-settled fact: Track C's design of that event bus was scoped around distinguishing deltos-shell from a pack origin, and never explicitly named a second native-process subscriber (§13).
- **v2 form:** Once the Tantivy stand-off segment (Track A §7 v2) lands, deltos-searchd's merged catalog becomes a merged Tantivy index built the same way — Tantivy's own segment/merge model is a natural fit for exactly this kind of incremental aggregation.

## 3. Vector / Semantic Index (D2)

The master plan leaves sqlite-vec vs. LanceDB open. LanceDB is a real, capable engine, but it's a columnar store outside the project's own service-selection filter (master plan §4 — prefer SQLite-or-flat-file-backed tools wherever a credible option exists), and running two different database engines for text vs. vector search on the same box (alongside SQLite everywhere else) adds an operational surface this appliance category shouldn't carry without a strong reason.

- **Recommendation:** sqlite-vec by default, everywhere the vector index is offered at all. It's an SQLite extension, not a second engine — consistent with Track A's index footer, B4's catalog, and every other SQLite-backed component in the plan. Its scaling behavior against a Wikipedia-sized corpus on constrained hardware is a real open question, flagged below rather than assumed away.
- **Where the vectors actually live — the v2 search bundle, with an open question review surfaced:** Rather than asking Track A for a second header pointer alongside search_index_offset/length (A9, §7), the vector index shares the same stand-off region: at v2, that offset/length pair points to one self-contained bundle holding both the Tantivy text segment and a sqlite-vec file side by side. That much holds. What the first draft got ahead of itself on: Track A's own index is explicitly a segment chain so appends never require rewriting existing data (Track A §4) — but a single monolithic bundle behind one non-chained offset/length pair would need a full rewrite on every append, reintroducing the exact write-once cost Track A's segmented design exists to eliminate. This needs the same treatment as Track A's main index, not a simpler one: the v2 search region likely needs to be its own small segment chain (a natural fit for Tantivy's own segment/merge model) rather than one blob. Left as an explicit open decision (§13) for Track A's next revision cycle, not asserted as settled — §15's original framing overstated how finished this is.
- **Tier gating:** Vector search is a Community Hub-tier-and-above feature (§8's consolidated table) — sqlite-vec's memory footprint for a meaningfully sized corpus doesn't fit Kiosk or Classroom-tier budgets. Below that tier, search is FTS5/Tantivy text-only, which C3's degraded-mode design (Track C §4) already accounts for.

## 4. Multilingual Tokenization (D3)

The master plan states "CJK, Indic, and Swahili-family coverage at launch" without saying where the tokenizer decision is made or enforced — and it can't be made twice, once at build time and once at query time, without the two disagreeing.

- **Build-time ownership:** The tokenizer used to populate a pack's search_index table (Track A §3) is chosen when wax-builder builds that pack (B1/B2, Track B) — SQLite's default unicode61 tokenizer doesn't segment CJK or many Indic scripts correctly, so those packs need the ICU tokenizer extension compiled into wax-builder's FTS5 build step. This is a build-time dependency Track B's B1/B2 pipeline needs to carry, not something D1 can fix after the fact at query time.
- **Query-time consistency, without a new schema field:** D1 needs to query each pack with a compatible tokenizer, but that doesn't require amending Track A's schema a third time: B3's manifest already carries a languages field (Track B §2), cached in B4 and already read by C2 (Track C §3). deltos-searchd derives the correct query-time tokenizer/language config per pack from that same field it can already read out of B4 — one existing source of truth, not a new one.
- **Coverage gap, stated plainly:** "At launch" for Swahili-family and most Indic scripts realistically means "ICU's segmentation is adequate," not "tuned." Genuinely good ranking for those languages (stemming, stopword lists) is a later-phase quality pass, not a v0.9 claim — noted here so the master plan's "at launch" doesn't overstate what P1 actually delivers.

## 5. Build-Time Embedding Pipeline (D4)

"Content embedded once at pack-build time" is the right call for hardware reasons — computing embeddings for a Wikipedia-scale corpus on a Pi is a non-starter — and it fits directly into the pipeline B1/B2 already run.

- **Where it runs:** Inside the same B1 (zim2wax) / B2 (crawler pipeline) build step that populates search_index (Track A §3), using whatever embedding model D4 settles on (open decision below) to produce the sqlite-vec data described in §3. This is build-side work, same hardware scoping Track B already established for delta computation and crawling (Track B §11) — never expected to run on Kiosk- or Classroom-tier hardware.
- **Model choice, left open:** Which embedding model, its dimensionality, and its licensing are not decided here — they affect the bundle's on-disk size (multiplied across every pack that ships with vector search enabled) and need a POC against real content before committing. Flagged as an open decision (§13) rather than guessed at.

## 6. Local LLM Runtime (D5)

"llama.cpp/GGUF, tiered by hardware" needs an actual tier table — Track C's C11 already depends on knowing, concretely, which hardware runs a model at all (Track C §11, "hardware-tier absence, not failure").

| **Deployment Profile (E6 hardware)** | **D5 local LLM** | **Notes** |
|---|---|---|
| Kiosk (pi_zero_2w) | None | No spare RAM/CPU for any GGUF model at usable quality or latency. C11 (Ask DeltOS) is absent from the launcher on this tier (Track C §11), not degraded. |
| Classroom (pi_4 / pi_5) | A small, quantized ~1–3B-class model | Usable for short, citation-grounded answers against retrieved passages (D6) — not general-purpose chat quality. Latency, not just footprint, needs a POC against real Pi 4 hardware before this tier is committed to. |
| Community Hub (mini_pc) | A larger ~7–8B-class quantized model | Meaningfully better answer quality; still local, still offline. This is the tier the master plan's "ChatGPT-style interface" ambition is realistically aimed at. |

- **Consequence for D2/D6:** Since vector search (D2) is gated to Community Hub-tier-and-above (§3) and D5's smallest usable tier is Classroom, RAG (D6) on Classroom-tier hardware runs against FTS5/Tantivy-retrieved passages only, without the semantic layer — a graceful, stated degradation rather than an unstated quality cliff.

## 7. RAG Orchestration (D6)

D6 retrieves passages via D1 (and D2 where available) and answers with citations through C11 — Track C's review already fixed the security contract this component has to implement, not just describe.

- **Untrusted context — a partial mitigation, stated honestly (revised after review):** Per Track C §11's post-review fix, every passage D6 retrieves is a pack's content, and not every pack has necessarily passed B9's review (a USB/peer-installed pack, Track C §7). D6's prompt template keeps retrieved passages in a clearly delimited context block the system prompt tells the model to treat as reference material, never as instructions. This is worth having, but it is not a complete defense — delimiting untrusted text is a known-bypassable mitigation against instruction-following models, and it does not stop a hostile passage from steering the model's displayed output (a false claim, a misleading "citation") even though it can't invoke IPC actions. The real backstop is §11's answer-only restriction below; delimiting is a second, weaker layer on top of it, not a substitute for it.
- **Answer-only in v1:** D6 exposes no function-calling or action-taking surface to C11 at all (Track C §11) — it returns text and citations, full stop. There is nothing in D6's v1 API a model response could invoke against deltos-shelld's IPC (Track C §2) even if a retrieved passage tried to induce it. C11 reaches D5/D6 over deltos-shelld's own privileged IPC, per Track C's settled design (Track C §11) — not over deltos-searchd's WebSocket (§10), which is a separate, lower-trust transport reserved for C3's plain search traffic.
- **Source flagging, added after review:** Beyond the prompt-level delimiting above, an answer's citations (below) should visibly distinguish a passage from a pack that passed B9's full review from one that didn't (a C7 USB/peer install) — giving a user a signal beyond "the model said so" when judging how much to trust a given answer.
- **Citations:** Every answer carries a reference back to the source pack and entry path (Track A's entries.path, and entries.title where set — Track A §3/§13) so a user can verify a claim against the original content, not just trust the model's synthesis.

## 8. Hardware-Tier Feature Gating (D7) — the Consolidated Table

D7 is listed as its own component, but it isn't a separate mechanism — it's the single source of truth for every tier decision made in §2–§7 and in Track C's own hardware-gated features. Stated once here rather than scattered:

| **Feature** | **Kiosk** | **Classroom** | **Community Hub** | **Field Ops** |
|---|---|---|---|---|
| D1 text search (FTS5/Tantivy) | Yes | Yes | Yes | Yes |
| D2 vector/semantic search | No | No | Yes | Yes |
| D5 local LLM | No | Small model | Larger model | Larger model |
| D6 RAG / C11 Ask DeltOS | Absent | Text-only retrieval | Full (text + semantic) | Full (text + semantic) |
| D8 federated search | No | No | Optional | Optional |

- **Why this belongs in Track D, not Track C:** C2's launcher rendering (Track C §3) and C11's presence check (Track C §11) both consume this table as a signal, but the tier assignments themselves are Track D's technical judgment about what each engine actually needs — keeping the authoritative table here means Track C never has to guess at a threshold Track D might later change.

## 9. Federated Search (D8)

Explicitly a later-phase, optional feature — scoped lightly here on purpose, since building it out now would be designing ahead of the tracks it depends on.

- **Peer discovery:** Reuses whatever multi-box discovery Track E already provides — mDNS/Avahi via E10 on a shared LAN, or Yggdrasil/Reticulum via E8 for off-Wi-Fi mesh topologies — rather than D8 inventing its own discovery protocol.
- **Fan-out shape:** A box's deltos-searchd (§2) forwards a query to discovered peers' deltos-searchd instances and merges ranked results — the same aggregation role §2 already plays locally, extended across boxes. Left at this level of detail deliberately; a full design is premature before D1's local form is proven.
- **Peer results are not local results — a trust gap review caught:** A peer box's content never passed this box's own installation, B9 review, or Track A signature verification — it's a strictly less-trusted input than anything C7 already treats carefully for local USB/peer installs. Two firm rules, not left implicit: federated results are excluded from D6's RAG context by default (never blended into an answer's evidence) unless an admin explicitly opts a fleet into cross-box trust (a Track F F6/F12 fleet-configuration decision, not a Track D default); and any federated result surfaced directly in C3's search results (not through RAG) is visibly labeled with its originating peer box, never presented as if it were local content.

## 10. Confirming Track C's Open Decisions

- **C3's transport (resolved) — scope corrected after review:** WebSocket, not REST, but scoped to C3/D1/D2 only. The first draft also folded C11's RAG traffic onto this same WebSocket to deltos-searchd; review caught that this overrides, rather than confirms, Track C's already-settled C11 design (Track C §11), which calls D5/D6 over deltos-shelld's privileged IPC precisely because that channel carries the Origin-allowlist and per-session capability token Track C's own review added. Corrected: deltos-searchd's WebSocket serves C3's plain search traffic only; C11 stays on deltos-shelld's IPC exactly as Track C specifies, streaming its token-by-token answers over that channel rather than a second transport. Two transports, not one — but each matched to the trust level of what it carries.
- **deltos-searchd's WebSocket needs its own authentication (added after review):** Track C's C1 review required Origin-header allowlisting and a per-session capability token on deltos-shelld's bridge specifically because same-origin policy doesn't stop a page from opening a cross-origin WebSocket connection — only the server checking Origin does. deltos-searchd's WebSocket is exactly that kind of endpoint and needs the identical protection: an unreviewed pack (C4's isolation notwithstanding — isolation stops storage/cookie leakage, not an outbound WebSocket connection attempt) could otherwise query deltos-searchd directly. deltos-searchd allowlists exactly deltos-shell's origin and requires the same class of session token deltos-shelld already uses.
- **C11's security contract (confirmed, not just acknowledged):** §7's untrusted-context and answer-only design is what actually discharges Track C §11's post-review fix — this document is where that commitment is implemented, not merely referenced.
- **B4 event-bus subscription — flagged for Track C's confirmation, not assumed (added after review):** §2's sync trigger assumes deltos-searchd can subscribe to deltos-shelld's B4 change-notification bus as a second native-process listener alongside deltos-shell itself. Track C's own design of that bus was scoped around distinguishing deltos-shell's origin from a pack's (Track C §2) and never named a second subscriber. Likely fine mechanically (peer-credential checking works the same way for any native process), but this document treats it as an assumption needing Track C's confirmation, not an already-agreed fact (§13).

## 11. Confirming Track B's B11 Dependency

Track B's B11 (Track B §8, revised) assumed forward geocoding could reuse "Track D's own full-text search stack." True, but it needs one clarification this document supplies: geocoding's OSM place-name index is its own dedicated FTS5/Tantivy table, built and queried the same way as pack content but never merged into deltos-searchd's cross-pack catalog (§2) — a search for "Springfield" in the address bar and a search for "Springfield" in C3's content search bar are two different indexes using the same engine, not one shared result set. Worth stating explicitly so a future implementer doesn't try to unify them.

## 12. Hardware & Profile Scoping

- D1's FTS5 tier and D7's gating table (§8) apply on every hardware tier down to Kiosk — text search is baseline, not an add-on.
- D2, D5, and D6's full form are Community Hub-tier-and-above features; Classroom tier gets a reduced D5/D6 experience per §6/§8, not their absence.
- D4's embedding computation and D3's build-time tokenizer selection are build-side (wax-builder) operations, never expected to run on-device — consistent with how Track B already scoped B1/B2/B8's heavier work.

## 13. Open Decisions Needed Before Implementation

- Validate sqlite-vec's query performance against a realistically large corpus (a full-language Wikipedia subset) on Community Hub-tier hardware before committing to it over a heavier alternative (§3) — the service-selection-filter argument for it is about consistency, not yet proven performance.
- Choose D4's embedding model, dimensionality, and license, and measure the resulting per-pack size overhead against a real content pack (§5).
- Validate D5's Classroom-tier (Pi 4/5) model choice against real latency, not just memory footprint, before treating that row of §6's table as final.
- Settle whether the v2 search region needs its own segment chain rather than one monolithic bundle (§3, raised by review) — this is a real open question about Track A's format, not a byte-level detail, and needs Track A's next revision cycle to resolve before v2 implementation starts.
- Confirm with Track C that deltos-shelld's B4 change-notification bus is intended to support a second native-process subscriber beyond deltos-shell itself (§2, §10) — likely fine, but not yet confirmed by the track that owns that bus.
- Confirm with Track F what fleet-trust configuration (F6/F12) governs opting a fleet into cross-box federated results feeding D6's RAG context (§9) — D8 defaults to excluding them, but the opt-in mechanism itself belongs to Track F.
- Confirm with Track E whether E10 (local DNS/mDNS) or E8 (mesh) is the more realistic default peer-discovery path for D8 (§9) once those components are further along.

## 14. Refined Phase Sequencing for Track D

| **Phase** | **Track D deliverable** |
|---|---|
| P0 | deltos-searchd proof-of-concept (D1): query a single pack's FTS5 table through the aggregator, proving the service exists before proving it merges anything. Uses an ad hoc test fixture pack, independent of Track A's own P2 commitment for A9's standardized v0.9 form (Track A §10) — this POC doesn't wait on that. |
| P1 | D3's build-time tokenizer selection wired into B1/B2 (Track B); D1's merged-catalog sync against B4's change-notification for more than one installed pack. |
| P2 | D1's FTS5 tier complete and serving Track C's C3 in production, matching Track A's A9 P2 commitment; D7's gating table (§8) implemented and consumed by C2/C11. |
| P3 | D5's Classroom-tier small model and D6's text-only RAG (no D2) — the reduced experience §6/§8 describes; B11's geocoding index (Track B §13) live against this tier's FTS5. |
| P4 | D2 (sqlite-vec) and D5's Community Hub-tier larger model together, since D6's full experience needs both. |
| P5 | D1's Tantivy/v2 form and the v2 search bundle (§3), matching Track A's A9 P5 commitment; D8 federated search. |

## 15. What This Unblocks

- Track C's C3 and C11 both get the concrete contract they were designed against an assumption of (§10) — the single aggregated endpoint, its transport, and the security rules governing what C11 can and can't do with retrieved content.
- Track B's B11 gets its geocoding index relationship to Track D clarified (§11) rather than left as a plausible-sounding but unconfirmed reuse.
- Track A's header gets a concrete direction for where v2's vector data lives (§3) — one bundle behind the pointer pair A9 already reserved — though whether that bundle needs its own segment chain is now an explicit open question for Track A's next revision cycle, not a closed one.

## 16. Independent Review — Findings and Resolutions

- **[Critical]** The original "one omnibus FTS5 table" merge design was technically unworkable: FTS5's tokenizer is fixed per table, but different packs use different tokenizers (D3); and no cheap ATTACH-based union exists, so "merging" always means copying and re-indexing text. Resolved: the design is now tier-adaptive — Kiosk tier queries a small number of packs directly via A1's reader, Classroom-tier-and-above maintains a merged catalog partitioned into one omnibus table per tokenizer in use, and per-pack segment-chain merging is delegated to A1's reader rather than reimplemented (§2).
- **[Critical]** The v2 "search bundle" behind one non-chained header pointer would need a full rewrite on every append, reintroducing exactly the write-once cost Track A's segmented index design exists to eliminate. Resolved: reframed as an open question for Track A's next revision cycle — the v2 search region likely needs its own segment chain, mirroring Track A §4's pattern, rather than one monolithic blob — instead of asserting the bundle shape as settled (§3, §13, §15).
- **[Critical]** §10 had C11's RAG traffic sharing deltos-searchd's WebSocket with C3's search traffic, which silently overrode Track C's already-settled C11 design (calling D5/D6 over deltos-shelld's privileged IPC specifically for its Origin-allowlist and capability-token protections). Resolved: two transports, not one — deltos-searchd's WebSocket carries C3's search traffic only; C11 stays on deltos-shelld's IPC exactly as Track C specifies (§7, §10).
- **[Security]** deltos-searchd's WebSocket had no stated authentication, despite being exactly the kind of localhost endpoint Track C's own review flagged as reachable from an unreviewed pack's origin. Resolved: deltos-searchd now allowlists deltos-shell's Origin and requires the same class of session token Track C's deltos-shelld bridge uses (§10).
- **[Security]** D8's federated search fan-out treated peer-box results as equivalent to local ones, with no distinction for RAG context or citations — a peer box's content never passed this box's own B9 review or signature verification. Resolved: federated results are excluded from D6's RAG context by default (opt-in only via Track F fleet configuration) and, when shown directly in search results, are visibly labeled with their originating peer box (§9).
- **[Security]** Framing the delimited-context-block prompt design as fully "implementing" Track C's security fix overstated what one prompt-template convention can guarantee — delimiting untrusted text is a known-bypassable mitigation against a model's displayed output, even though it can't invoke IPC actions. Resolved: reframed as a partial, secondary layer on top of the real backstop (answer-only, no function-calling), with source-flagging added as a further mitigation (§7).
- **[Moderate]** Track C's C11 phase tag (P5) was justified as "matching Track D's own D5/D6 timing," but Track D's own phase table lands D6's full experience at P4 — a mismatch between two documents both treating the number as settled. Resolved: flagged explicitly for Track C's phase table to reconsider (§7, cross-referenced); see the companion amendment to the Track C document.
- **[Moderate]** D1's P0 milestone didn't say whether its test pack could exist before Track A's own P2 commitment for A9's standardized v0.9 form. Resolved: stated explicitly as an ad hoc fixture, independent of that commitment (§14).
- **[Moderate]** deltos-searchd's subscription to deltos-shelld's B4 change-notification bus (§2) assumed a second native-process subscriber was already supported, when Track C's design of that bus was scoped only around distinguishing deltos-shell from a pack origin. Resolved: flagged as needing Track C's confirmation rather than treated as agreed (§10, §13).
- **[Minor]** The D5 hardware-tier table's column header read "Hardware tier (E6)" but listed Deployment Profile names, not E6's actual tier vocabulary — the same conflation Track B's review caught and fixed for the manifest schema. Resolved: retitled to separate the Profile name from its E6 hardware mapping (§6).

## 17. Amendment (per product direction — AI acceleration)

**Goal.** The local model uses GPU acceleration where the box measures a usable GPU.

**Property.** The model declares **`gpu: preferred`** (cross-track contract §2.3), which means it **runs either way and never gates on the accelerator**: GPU where one is measured, CPU where none is, and absent where the measured RAM does not meet the model's floor. Three outcomes from one declaration, decided against the capability record (§9), never against a device name.

**Why.** Accelerators are exactly the kind of hardware variation the retired tier names could not express — two boxes with identical RAM and storage can differ entirely in whether the model is usable. `preferred` rather than `required` is the whole point: a box without a GPU loses speed, not the feature.

- **D5's resource floors are unchanged** and remain in contract §6, now carrying `gpu: preferred` on both rows. The floors are RAM-based and measured; the accelerator selects an execution path within them.
