# DeltOS — Track B

Content & Interop — Technical Refinement

*Working draft · September 2026 · second of eight per-track refinements, revised after independent technical review (§15) and three rounds of cross-track amendment (§16-§18)*

## 1. Scope

Track B owns getting real content onto a box: converting the existing ZIM/Kiwix library, crawling original content, curating what ships by default, and the catalog/review pipeline that lets the library grow after launch. Track A's schema is the foundation this sits on — this document surfaces one required amendment to it (§3) and otherwise treats Track A as settled.

## 2. Pack Manifest Schema (B3) — Concrete Fields

NORMATIVE LIST MOVED: the authoritative field list now lives in the Cross-Track Contract §11 and nothing else. This section is retained for its design rationale — why each field exists, what was dropped and why — but where it and the Contract differ on a field's presence, requiredness or value domain, the Contract wins. The move happened because this table was also reproduced in Track A §16, the two copies drifted when guest_accessible was added here and not there, and a builder enforcing against the other copy rejected a valid manifest.

The manifest table (Track A §3) is generic key/value; this is what actually goes in it. Every field below is required unless marked optional. Revised after review — see the notes on id, min_hw_tier, category, and total_size_bytes below. Revised again per product direction — see the notes on runtime_ram_bytes, runtime_storage_bytes, and guest_accessible below.

| **Key** | **Type** | **Notes** |
|---|---|---|
| name | string | Display name shown in the DeltOS Shell launcher (Track C, C2). |
| icon | string | Path within the archive to an icon image entry. |
| category | enum | One of a fixed taxonomy: reference · education · media · tools · civic · health. Drives launcher grouping (C2). Revised after review: this no longer also claims to drive which Deployment Profile a pack is offered under — that's a catalog-side (B13) curation decision the catalog-index (§5) carries, not something a fixed enum on the pack itself should be overloaded to express. |
| license | string | SPDX identifier where one applies (CC-BY-SA-3.0, CC0-1.0, MIT, …) or a short free-text description. Required, checked by B6. |
| attribution | string | Human-readable credit line, carried through from source (e.g. a ZIM's Creator/Publisher metadata). |
| version | string | Human-facing semver, e.g. 2026.09.1 — separate from Track A's internal segment/version mechanics. |
| min_hw_tier | enum | Revised after review: pi_zero_2w · pi_4 · pi_5 · mini_pc — Track E's actual E6 hardware-tier vocabulary. The original draft used the Deployment Profile names (Kiosk/Classroom/Community Hub/Field Ops), but Profiles and hardware tiers are a deliberately separate axis (master plan §2) — a pack manifest should declare the hardware it needs, not a deployment context it can't know about. |
| total_size_bytes | — removed | REMOVED from the manifest during A2b implementation — see §19. §5's catalog packs.size column carries archive size instead, populated by B13, which always holds the actual file. A config supplying this key is a build error. |
| runtime_ram_bytes | integer (optional) | New field, added per product direction. This pack's own steady-state memory footprint once running (its wax-serve instance, plus any companion service it needs, e.g. a model runtime) — not the archive's on-disk size, which §5's catalog packs.size covers. Feeds Track C's C6 dynamic requirement calculator (§5's amendment); omitted (treated as negligible) for a pack that is pure static content with no companion service. Stays in the manifest under §19's rule: an authored property no inspection of the file would reveal. |
| runtime_storage_bytes | integer (optional) | New field, added per product direction. Additional writable storage this pack needs beyond its own archive once installed (a search index, a cache, user-generated data) — distinct from the archive download itself, which §5's catalog packs.size carries. Also feeds the calculator; omitted where a pack needs no writable storage beyond what unpacking already accounts for. Also authored, not measurable — see §19. |
| guest_accessible | boolean (optional) | New field, added per product direction (Track F §22). Defaults false. Set true by a pack's author or the admin to make it visible in a Guest profile's launcher grid (Track C §3) — a pack with no opinion set is invisible to Guest by default, not visible until someone opts it out. Irrelevant to every other role, which already sees the full catalog per §2's category/min_hw_tier filtering. |
| entry_point | string | Path within the archive to the pack's launch target (an index.html, typically). |
| languages | string (optional) | Comma-separated language codes covered — read by Track D's D3 tokenization to pick the right index config at build time. |
| depends_on | string (optional) | Comma-separated archive_uuid values this pack expects to be installed, e.g. an office-suite pack depending on a cloud-storage pack. Corrected in §19 — this row previously said "pack ids," stale wording for the id field this same section removed. |

Note on identity (removed after review): the original draft also included an id field described as a "stable logical identifier, distinct from archive_uuid." It was dropped — it duplicated Track A's archive_uuid without adding a distinct meaning, and having two identity fields on the same record invited them to drift out of sync. Track A's archive_uuid is the one identity a pack carries; the catalog (§5) groups a pack's version history by it directly.

## 3. Amendment to Track A: Redirect Support

Discovered while designing zim2wax, not invented speculatively: ZIM's dirent model includes redirect entries, and Wikipedia-scale ZIMs contain enormous numbers of them (a large fraction of all titles, in practice). Track A's entries table had no way to represent "this path is really that path" without duplicating content.

- **Fix applied directly to Track A:** entries gains redirect_to TEXT DEFAULT NULL. When set, offset/length are ignored and a reader follows redirect_to instead. wax-builder flattens redirect chains to a single hop at build time, so no reader ever follows more than one indirection — this keeps A1 (the reader) simple and avoids a loop-detection requirement at read time.

This has already been written into the Track A document (§3, §13) rather than left as a note here — Track A's spec history should live in one place.

## 4. Content Pipeline

### B1 — zim2wax

| **ZIM concept** | **WAX equivalent** | **Notes** |
|---|---|---|
| Dirent (content entry) | entries row | Path, mimetype, and blob copied across directly; the dirent's Title becomes entries.title (second Track A amendment, added after review — see below). |
| Dirent (redirect entry) | entries row with redirect_to set | See §3. |
| Title/URL pointer lists | entries.path (URL) + entries.title (display title) | Revised after review: the original draft mapped both of ZIM's Title and URL pointer lists onto entries.path alone, which conflates a page's storage path with its human-readable title. WAX now keeps them as separate columns on the same row — no separate pointer list needed either way. |
| Embedded Xapian index | search_index (FTS5) table | Rebuilt from article text at conversion time — not copied binary, since the index formats aren't compatible. |
| ZIM metadata (Description, Language, Creator, Publisher, Date, License) | manifest fields (§2) | Direct field-to-field mapping; license flows straight into B6's check. Title is per-article, not archive-level, so it's excluded here — see entries.title above, not a manifest field. |
| Main page pointer | manifest.entry_point | — |

- **Second Track A amendment (from this review):** entries also gained a title TEXT column (nullable — not every entry, e.g. an image, has a meaningful title). The original schema had no field for it, silently conflating Title and URL/path the way the table above used to. Written into Track A §3/§13 directly, the same treatment as redirect_to in §3 above.
- **Scope:** v0 (lossy) targets text + image ZIMs only, matching the master plan's P1 milestone. Video/audio support is a P2 addition once byte-range serving (Track A, A5) is proven against real media files.

### B2 — Crawler Pipeline

- **Recommendation:** Don't build a web crawler from scratch. Reuse Browsertrix Crawler (Webrecorder's headless-Chromium, WARC-producing crawler — the same engine Kiwix's own zimit is built on) and write warc2wax, a converter analogous to zim2wax, rather than reinventing JS rendering, rate limiting, and dedup logic that already exists and is maintained. This directly follows the "interop before ambition" principle from the Blueprint (§03 there) — just applied to tooling, not only content.
- **What this actually improves on zimit:** Not the crawler itself (same engine) but the output: WARC → WAX conversion can target Track A's byte-range-friendly contiguous storage and A9's search index directly, rather than being constrained by ZIM's dirent model — closing the Gap Report's finding that zimit's JS-heavy/paywalled-site struggles are a crawler-engine limitation Track B inherits regardless, but the output-side limitations are Track B's to fix.

### B8 — Incremental Re-Crawl

Simpler than the master plan's inventory description implies, once Track A's delta engine (A6) is accounted for: B8 doesn't need its own change-detection logic. A fresh full re-crawl produces a new full WAX pack; A6 computes the byte-level delta between old and new versions for efficient distribution. This avoids building two separate incremental mechanisms (one in the crawler, one in the format) that would need to agree with each other.

- **archive_uuid preservation (added after review):** For that to work, the re-crawl's output has to reuse the same archive_uuid as the pack it's replacing — A6 matches old/new versions by archive_uuid (Track A §2), so a re-crawl that mints a fresh UUID (which a naive rebuild would, by default) would look like an unrelated new pack rather than an update, silently defeating delta distribution. This is wax-builder's responsibility, not B8's: when B2's orchestration re-crawls into an existing pack's lineage, it passes the prior pack's archive_uuid through to wax-builder's rebuild path rather than letting the build mint a new one.

## 5. Catalog & Distribution

The master plan's B4 ("wax-hub") was doing double duty as both a data format and a service name. Splitting it, added as B13 in the master inventory:

- **B4 — on-device catalog:** A local SQLite database on each box: packs(archive_uuid, name, version, sha256, size, manifest_json, source_url, signature), install_state(pack_id, status, installed_at). Track C's C6 (offline app store UI) and Track F's F4 (install pipeline) both read/write this same database — one format, not two. (id renamed to archive_uuid after review, to match §2's dropped manifest id field — this is Track A's real identity, not a second invented one.)
- **B13 — catalog publishing service (new):** The central infrastructure that B9's review pipeline feeds. It is also the only place the trusted project-key signature (Track A, A7) is ever applied — see the corrected sequencing under B9 below.
- **B4↔B13 sync mechanism (made concrete after review):** The first draft left open how a box's B4 actually learns what's new. Resolved by not inventing a second protocol: B13 maintains the catalog itself — the list of published packs, their manifests, current versions — as a special, frequently-updated WAX pack of its own (a "catalog-index" pack). It's distributed to every B4 instance through the exact same mechanism as any other pack: Track A's A6 delta engine for the update itself, an optional fleet-internal mirroring mode for the fetch (corrected in §18 below — not Track G's G3/G4 as originally named here). A box's B4 syncs by fetching the latest catalog-index delta, applying it, and reading the result — no bespoke catalog-sync protocol to build, test, or keep compatible with A6 as it evolves independently.

### B9 — Community Submission & Review, Concrete Flow

| **State** | **What happens** |
|---|---|
| submitted | A .wax pack (or a source to crawl/convert) is submitted to B13. |
| automated-checks | B6's licensing check, Track A's A4-style structural validation, and a content-safety/malware scan run automatically. Failure returns to the submitter with a specific reason — never a silent rejection. |
| in-review | A human moderator reviews anything automated checks couldn't resolve (an unrecognized license, borderline category). |
| approved → signed → published | B13 signs with the project key (Track A, A7) — the one and only point in the whole pipeline where that trusted signature is applied — and adds the pack to the catalog-index (above), from which B4 instances sync it. |
| rejected | Returned with a reason; resubmission is a new pass through the same pipeline, not a special case. |

- **Signing authority, clarified after review:** wax-builder (A2) never holds or applies the project's trusted signing key, at any point in this flow — a submission it produces is, at most, an unsigned or self-signed candidate for B13 to evaluate, not something a device would ever trust on its own. Only B13 applies the trusted project-key signature, and only at the approved → signed → published step above, after both automated checks and human review pass. The earlier draft used "signed" without saying which key or which party held it; this replaces that ambiguity.

## 6. Licensing Policy (B6)

A default allowlist, checked at wax-builder build time (Track A, A2) — not left to reviewer judgment on every submission:

- Auto-approved: CC0, CC-BY, CC-BY-SA, public domain, GFDL (the Wikipedia-family license), MIT, Apache-2.0, GPL-family — anything with a recognized SPDX identifier in this set.
- Flagged for manual review: anything outside that list, or a manifest.license field left blank — wax-builder refuses to produce a submission-ready pack in this case (it won't emit a candidate .wax at all), rather than silently letting an unlicensed submission reach the B9 review queue. Terminology fix from review: the earlier "unsigned-for-distribution pack" phrasing implied wax-builder was withholding a signature — it never holds the trusted project-key signature to begin with (§5), so what it's actually withholding here is the candidate pack itself.

## 7. Starter Packs (B5) — Concrete Proposal

Endless Key's curated-bundle model, given actual content:

- **Foundations:** A vital-articles subset of Wikipedia + Wiktionary + a core math/science reference set. The default first-run install on every profile.
- **K–6 Classroom:** Age-appropriate Kolibri channels (Track H, H1) plus a matching reference subset.
- **Community Health:** Medical-reference wikis (e.g. the WikEM/Medical Wikipedia family) plus relevant first-aid material — targeted at the Community Hub and Field Ops profiles.
- **Field Reference:** Offline maps (B10) plus first-aid and weather reference material — targeted specifically at the Field Ops profile.

## 8. Mapping & Geocoding (B10, B11)

- **B10 — offline mapping pipeline:** OSM regional extract (.osm.pbf) → vector tiles via Tilemaker or Planetiler (MBTiles output) → an OSRM routing graph from the same extract → packaged with MapLibre GL JS (FOSS, no server dependency) as the viewer, entry_point pointing at it. Storage-heavy — a country-sized extract with tiles can run tens of GB, so this is scoped to Classroom tier and above (Track A's A8 multi-volume applies here first among all content types).
- **B11 — offline geocoding, revised again after review:** Full Nominatim needs PostgreSQL+PostGIS — a real exception to the project's SQLite-first filter (Gap Report §5) that's worth avoiding rather than accepting, so moving off it stays the right call. But the first revision (SQLite R-Tree over an indexed table of place names) solved the wrong half of the problem: R-Tree indexes spatial extent — coordinate-to-nearest-point — which is reverse geocoding, not the forward geocoding (address text → coordinate) the master plan's own stated use case ("local address to location lookup") actually needs, and text search isn't what R-Tree is for. Corrected design: forward geocoding reuses Track D's own full-text search stack (D1's FTS5 at v0.9, the Tantivy tier at v2) — index OSM place and street names from the same B10 extract as a fuzzy/tokenized search corpus, so a typo'd or partial address still resolves rather than requiring an exact string match. R-Tree stays, but scoped down to reverse geocoding and to disambiguating among multiple text-search matches (e.g. nearest to a last-known position). Exact house-number interpolation (resolving a specific street number to a point between two mapped addresses) is explicitly out of scope for v1 — a stated limitation, not a silent gap.

## 9. DevDocs Offline Pack (B12)

DevDocs already produces offline docsets designed to run without a server. B12 is a thin adapter: take DevDocs' existing offline output, repackage it into a WAX pack with its own viewer as entry_point. Mostly a repackaging script, not new engineering — consistent with treating this as content (Track B) rather than a running service (which is why it isn't in Track H).

## 10. WAX Studio (B7)

A separate curation frontend — thin UI over the same wax-builder (A2) and B1/B2 pipelines a command-line curator would use, so it has no logic of its own to drift out of sync with the CLI tools. On publish, it writes to B13 through the same submission flow as any other contributor. Deliberately a later-phase item (P5 per the master plan) — building it before B1/B2 are proven would mean designing a UI around pipelines that might still change shape.

## 11. Hardware & Location Scoping

Stated explicitly so this doesn't surface as a review finding later:

- Crawling (B2), catalog review/scanning (B9), and map-tile generation (B10) are hub/build-side operations only — never expected to run on a Kiosk- or Classroom-tier box. Only the resulting packs and the lightweight sync/install path touch constrained hardware.
- zim2wax (B1) itself, by contrast, is lightweight enough to run on-device if needed (it's a format transcode, not a crawl), though the primary path is still to run it centrally and distribute the resulting packs.

## 12. Open Decisions Needed Before Implementation

- Confirm the Track A redirect_to and title amendments (§3, §4) against Track A's actual reader implementation once A1 exists — this document assumes them, but Track A's own review cadence should re-confirm once code exists to check.
- Confirm Tilemaker vs. Planetiler for B10's tile generation — a build-tooling choice, not a format choice, so lower stakes than most Track A decisions but still worth settling before B10 work starts.
- Confirm Track D's fuzzy/tokenized text search gives acceptable forward-geocoding result quality (§8, revised design) before committing away from a dedicated geocoding library — worth a small proof-of-concept against one region's OSM extract rather than assuming it's sufficient.
- Work out B4's bootstrap path (raised by the catalog-index design, §5): a box's very first sync has no catalog-index pack to diff against, so the appliance image needs to ship with an initial catalog-index and starter packs (§7) pre-installed rather than requiring a first-boot fetch from nothing.
- Decide B13's own hosting/ownership — it's central infrastructure, not something that ships on an appliance image, so it has different operational requirements (uptime, moderation staffing) than everything else in this plan.

## 13. Refined Phase Sequencing for Track B

Revised after review: three phase tags were out of step with either the master plan's own tags or with what these components actually depend on. Fixes below; the corresponding master-plan tag for B10 is patched to match this table, which is authoritative for Track B timing.

| **Phase** | **Track B deliverable** |
|---|---|
| P0 | zim2wax proof-of-concept (B1, lossy OK) — unblocks every other track's testing. |
| P1 | zim2wax v1 for text+image ZIMs (§4); manifest schema (§2) frozen; B6 licensing allowlist implemented in wax-builder. |
| P2 | Video/audio support in zim2wax; B13 stood up with B9's review pipeline; first 20 packs published including the Foundations starter pack (§7); B12 DevDocs pack (moved up after review — it's a repackaging script with no real dependency beyond B13 existing, so there was no reason it was waiting for P5). |
| P3 | B10 mapping pipeline; B11 geocoding (moved up after review, §8 — the redesigned v1 needs only B10's own OSM extract and Track D's v0.9 FTS5 tier, both already available by this phase, not the P5-only Tantivy tier the original design assumed); remaining starter packs (§7); manifest/licensing standard finalized across all published packs. |
| P4 | B9 community submission pipeline fully operational. |
| P5 | WAX Studio (B7); B8 incremental re-crawl (pushed back from P4 after review — it leans entirely on A6, §4, which itself lands earlier in Track A's own sequencing but leaves too little slack for B8's re-crawl validation work to also land by P4); public catalog (B13) opened to outside contributors. |

## 14. What This Unblocks

- Every other track gets real content to test against as soon as P0's zim2wax proof-of-concept exists — this was already the master plan's single most-cited critical-path item, and nothing here changes that.
- Track C's launcher (C2) and Track F's install pipeline (F4) can both be built against §5's concrete B4 schema instead of an abstract "catalog" concept.
- Track D's D3 (multilingual tokenization) gets a real signal to build against — §2's languages manifest field — instead of having to detect language from raw content.
- Track D's D1 (v0.9 FTS5) now has a second concrete consumer beyond in-pack search — B11's forward geocoding (§8) — reinforcing that tier's priority rather than adding a competing requirement.

## 15. Independent Review — Findings and Resolutions

- **[Critical]** B11's design (SQLite R-Tree over indexed place names) solved reverse geocoding, but the master plan's actual stated use case — address text to coordinate — is forward geocoding, which R-Tree doesn't address. Resolved: forward geocoding now reuses Track D's FTS5/Tantivy full-text search stack; R-Tree is rescoped to reverse geocoding and match disambiguation only, with house-number interpolation explicitly out of scope for v1 (§8).
- **[Critical]** The B4↔B13 sync mechanism was named but never specified — B4 instances had no defined way to learn what B13 had published. Resolved: B13's catalog is itself distributed as a special "catalog-index" WAX pack, synced through the same A6 delta path as any other pack, rather than inventing a bespoke protocol (§5).
- **[Critical]** B6/B9's signing language was ambiguous about which party holds the trusted project key and at which pipeline step it's applied, and §6's "unsigned-for-distribution pack" phrasing implied wax-builder does some signing when it never holds that key at all. Resolved: wax-builder never applies the trusted signature; only B13 does, only at approved → signed → published (§5, §6, §9).
- **[Moderate]** ZIM's Title and URL are distinct concepts, but the schema only carried path, and §4's B1 mapping table folded Title into manifest metadata (archive-level) rather than per-article data. Resolved: Track A's entries table gains a title column (a second, review-driven amendment alongside redirect_to); §4's mapping table corrected to reflect it.
- **[Moderate]** B8's reliance on A6 for incremental re-crawl silently assumed the re-crawled pack would carry the same archive_uuid as the pack it replaces, without that being anyone's stated responsibility. Resolved: made explicit as a wax-builder rebuild-path requirement, driven by B2's orchestration (§4).
- **[Moderate]** §2's manifest schema had three smaller issues: an id field that duplicated archive_uuid without a distinct purpose, a min_hw_tier using Deployment Profile names instead of Track E's actual hardware-tier vocabulary, and category overreaching into Profile-gating it can't cleanly express. Resolved: id dropped, min_hw_tier corrected to E6's tier names, category's scope narrowed to launcher grouping; a new total_size_bytes field was also added, computed multi-volume-aware by wax-builder (§2).
- **[Moderate]** Three of Track B's own phase tags (B10, B11, B12) didn't match either the master plan's tags or what these components actually depend on — most visibly, B12 (DevDocs) had no real dependency justifying a P5 placement. Resolved: B12 moved to P2, B11 moved to P3 alongside B10 once its geocoding redesign showed it only needs the v0.9 FTS5 tier, and the master plan's own B10 tag is patched to match this table (§13).
- **[Moderate]** B8 (incremental re-crawl) was scheduled for P4 in the same phase A6 itself is still maturing in Track A's own sequencing, leaving no real slack to validate B8 against a genuine re-crawled pack. Resolved: B8 pushed to P5 (§13).
- **[Minor]** A stale reference in Track A (§5) named "wax-hub, B4" as where delta computation runs — leftover from before B4/B13 were split. Resolved directly in Track A, now naming B13 (Track A §5, §13).

## 16. Post-Review Amendment (from Track E's review)

Track E's independent review flagged that this document's B4 design (§5) never stated a concrete filesystem path for the on-device catalog database or installed pack files, and that Track E's own rollback-safety guarantee (Track E §3) only holds if those resolve under Track E's /var/lib/deltos convention (Track E §4). Confirmed here rather than left as an assumption: B4's catalog database and every installed .wax pack live under /var/lib/deltos/ (specifically /var/lib/deltos/b4/ and /var/lib/deltos/packs/, or equivalent subdirectories) — never anywhere else on the box. This costs nothing to commit to now, before implementation exists to put it somewhere else by default, and it's what makes Track E's ostree rollback guarantee actually true for B4's state rather than merely assumed.

## 17. Post-Review Amendment (per product direction on hardware philosophy)

A product-direction check-in after Track E's review settled that DeltOS replaces the idea of a single published "optimum" hardware spec with a dynamic, checkbox-style calculator — the person selects tools/features (at download time and, later, inside DeltOS's own app store, Track C §6) and sees a live minimum/optimum requirement readout for that selection, rather than DeltOS prescribing one fixed configuration (full rationale in Track E's new §21).

- **What this requires of B3's manifest:** The calculator needs a resource cost per pack to sum, not just a hardware-tier floor. §2's manifest schema gains two new optional fields for this: runtime_ram_bytes and runtime_storage_bytes, alongside min_hw_tier (hardware floor) and the pack's download size. All four numbers are distinct and all four matter to the calculator: min_hw_tier gates whether a pack can run on a tier at all, download size is what gets fetched, and runtime_ram_bytes/runtime_storage_bytes are what the pack costs once running — the numbers Track C's C6 calculator actually sums against Track E's E6 tier floors. Amended in §19: download size is read from §5's catalog packs.size rather than a manifest field, which costs C6 nothing — C6 is an offline browser over B4's on-device catalog (master plan, C6) and so is already reading that table.
- **Who computes these values:** Unlike total_size_bytes, these aren't mechanically derivable from the archive by wax-builder — a pack's real memory/storage footprint depends on how its companion service behaves under load, not just what's in the archive. They are pack-author-supplied estimates, refined over time from real measurements (the same open-ended validation problem already flagged for Track E's own hardware-tier budget, Track E §17) — stated as optional and best-effort in §2's table for exactly this reason, rather than implying a precision the pipeline can't yet guarantee.

## 18. Amendment (from Track G's design)

§5's B4↔B13 sync bullet named "Track G's G3/G4 for fleet-internal mirroring" of catalog-index and pack downloads, written before Track G's own refinement existed to check that against. Track G's initial draft shows this was a mismatch: G3 is source-code hosting and G4 is a container-image registry — neither is built to cache an arbitrary HTTP pack download.

- **Corrected reference:** §5 now names the actual mechanism instead: Track G's G2 (reverse proxy) supports an optional fleet-cache mode, where one admin-designated box transparently caches B13-sourced fetches for other same-LAN fleet members proxying through it — a reverse-proxy capability, not a git or container-registry one. See the Track G Refinement, §3, for the full design and its own open trust-boundary question (Track G §10).

## 19. Amendment (from Track A's A2b implementation)

Enforcing §2's schema in wax-builder for the first time — rather than passing manifest keys through unvalidated — surfaced two problems in this section that no design-layer review had caught, because both only become visible when something actually tries to write the field. Supersedes the total_size_bytes and depends_on portions of §15's review findings and §17's calculator amendment; the rest of both stands.

- **[Critical]** total_size_bytes cannot work as a manifest field, for two independent structural reasons. It is self-referential: the value records the archive's own size while living inside that archive, so a longer decimal grows the index segment, which grows the file, which changes the value — the implementation needed a write-measure-rewrite fixed-point loop, compressing every pack twice to converge. And it goes permanently stale on append: this section's own manifest-immutability rule (Track A §5.6) confines the manifest to segment 0, while an append by definition grows the file, so after any append the recorded size understates reality and nothing is permitted to correct it. Resolved: the field is removed from §2 outright. §5's catalog already carries packs.size, populated by B13 — the single point where a pack is signed and published (§5, §9), always in possession of the actual file, able to measure it exactly, re-measure after an append, and sum across Track A's A8 volumes. The field's stated purpose (a real download size without opening the archive) is precisely the catalog's case, and the catalog was already serving it.
- **The rule that follows from it:** A manifest carries what only the pack's author knows; a catalog carries what any holder of the file can measure. Archive size is measurable by anyone holding the archive and belongs to the catalog. runtime_ram_bytes and runtime_storage_bytes stay in the manifest under the same rule — authored estimates of runtime behavior that no inspection of the archive would reveal, as §17 already noted when distinguishing who computes them. Worth applying to any future field proposed for §2.
- **[Moderate]** depends_on was specified as "comma-separated pack ids" in the same table whose identity note removes id and states that Track A's archive_uuid is the only identity a pack carries — leaving any consumer resolving a dependency with no defined referent. Resolved: depends_on carries comma-separated archive_uuid values, consistent with §5, where B4's packs table is keyed on archive_uuid and the catalog groups a pack's version history by it directly.
- **Known limit, deliberately not solved yet:** Because archive_uuid is stable across a pack's versions (Track A §2), depends_on expresses "needs pack X" but not "needs pack X at version ≥ N". Adequate for the field's present scope — an optional hint, and no shipped pack yet has a real dependency — and not worth inventing a constraint syntax for ahead of a case that needs one. Recorded so it is a known limit rather than a later surprise, most likely to matter first for Track H's service packs (H3 depending on H5, per the master plan's own example).

## 20. Amendment (from the Implementability Sweep) — B1 Blockers

An implementability sweep of all nine documents, run before writing B1's implementation prompt rather than after, found five defects in this document that would have stopped zim2wax within the first hour and two smaller ones worth fixing in the same pass. All seven are resolved below. Cross-track vocabularies referenced here (language codes, version format, the SPDX allowlist) now live in the Cross-Track Contract and are cited rather than restated.

- **[Critical]** §4's ZIM mapping never states the canonical form of entries.path. ZIM paths are namespace-qualified (A/, I/, C/ depending on ZIM version) while intra-article links are relative and namespace-free, and the same unstated convention governs icon, entry_point and redirect_to, which all resolve against entries.path. A wrong choice breaks every internal link in a converted Wikipedia. Resolved: the article namespace flattens to the root with its prefix stripped (A/Photosynthesis → Photosynthesis.html); non-article namespaces map to reserved path prefixes (_assets/ for images and media, _meta/ for metadata entries); zim2wax rewrites in-document hrefs to match at conversion time rather than relying on the serving layer to resolve them. Path comparison is exact and case-sensitive; normalization is a build-time obligation, never a reader behavior (Track A §17).
- **[Critical]** This section's own example previously read A/Photosynthesis → photosynthesis.html, lowercasing the path — which contradicts the case-sensitivity rule in the same sentence and would have destroyed data. A real Wikipedia ZIM carries 13th_Amendment, 13th_amendment and 13th_ammendment as three distinct dirents, and 4H_disease alongside 4h_disease; folding case collides them irreversibly and silently. Corrected above: the prefix strip and the .html suffix are the rule, the case fold never was. Found by B1 against real data, not by any review of this text.
- **[Critical]** Namespace alone cannot classify an entry on any current ZIM. Under the ≥6.1 layout, articles, images, CSS and video all share the C/ namespace — a real Wikipedia ZIM holds C/Michael_Jackson, C/_assets_/… and C/_mw_/… side by side — so the namespace-to-prefix mapping above is necessary but not sufficient. Resolved: a C/ entry is an article if and only if its mimetype is text/html; everything else routes to _assets/. The same mimetype-first reading is applied to the legacy A/I/- scheme so one rule covers both.
- **Reserved prefixes can collide with real source paths:** A source entry whose own url begins _assets/ or _meta/ would shadow a converted asset. Such entries are dropped with a reserved_prefix_collision warning rather than silently overwriting. Not seen in practice — mwoffliner uses _assets_ with a trailing underscore — but the rule is stated rather than left to chance.
- **Icon fallback order:** Illustration_48x48@1, then any other Illustration_* entry, then the legacy -/favicon entry, then a generated placeholder. The legacy-favicon step matters because in a 5.0-era ZIM the favicon is the illustration; without it, older archives would all get placeholders despite carrying a real icon. A generated placeholder raises icon_generated in the build report.
- **The literal string "null" is not a title:** mwoffliner writes title="null" on every non-article dirent, which would otherwise put the string null into several thousand entries.title rows and surface it in the shell. Resolved: for a non-text/html entry, a title of exactly null (lowercase) is treated as absent. The check is deliberately narrow — it is not applied to articles, so a genuine article titled "Null" survives.
- **[Critical]** Five required manifest fields — name, icon, category, version, min_hw_tier — have no source in §4's mapping table, and ZIM carries no analog for category or min_hw_tier at all. Since Track A's A2b pass made every required field a hard build failure, zim2wax cannot emit a pack at all until each has a defined origin. Resolved: name derives from ZIM Title; icon from the ZIM Illustration entry, extracted to _assets/icon.png (falling back to a generated placeholder, never omitted); version derives from ZIM Date as CalVer per the Contract; category and min_hw_tier have no derivable source and are supplied by required zim2wax flags (--category, --min-hw-tier), with conversion failing if absent rather than defaulting. Two further operator flags join them, for required fields a source may carry nothing for at all: --license, mandatory where the ZIM states no license — which is every ZIM examined, Wikipedia included — and --attribution, mandatory where the ZIM carries neither Creator nor Publisher. Both raise their own build-report warning, and --license additionally forces license_review_required, so an operator's claim is reviewed rather than trusted (Cross-Track Contract §11). attribution otherwise composes as Creator, falling back to Publisher — the earlier "then to the empty string" is struck, since an empty string is a value §11 rejects and the flag is the sanctioned path instead; entry_point derives from the ZIM main-page pointer.
- **[Critical]** redirect_to's content was never defined, while ZIM's own redirect dirents store a target dirent index rather than a path — so zim2wax had to translate into an undefined form. Resolved: redirect_to holds the target's entries.path in the canonical form above, and must resolve to a non-redirect row. Cycles and chains whose terminus is missing are dropped at build time with a counted warning, never emitted — matching Track A's requirement that a reader never follows more than one hop.
- **[Critical]** The licensing rules could not all hold: §2 permits license to be free text, §6 has wax-builder refuse to emit any pack outside a fixed allowlist, and §9 gives a moderator an "unrecognized license" state that is therefore unreachable. Real Wikipedia ZIMs carry free-text or absent license metadata, so this failed on the flagship P1 content. Resolved per the Cross-Track Contract §11: blank is a hard build failure; an allowlisted SPDX identifier builds clean; valid free text or a recognized-but-not-allowlisted identifier builds with a license_review_required flag that routes the pack to B9's in-review state. §2's row is amended to say free text is permitted and always routes to manual review.
- **[Critical]** §6's auto-approve list names license families rather than SPDX identifiers — "CC-BY", "GPL-family" and "GFDL" are not valid ids, and "public domain" has none — leaving the matching rule (exact, prefix, family expansion, -or-later, compound expressions) to be invented in a check shipping at P1. Resolved: the allowlist is the literal identifier set in the Cross-Track Contract §11, matched exactly, with compound SPDX expressions routed to review rather than evaluated.
- **install_state's key, corrected:** §5's install_state(pack_id, status, installed_at) still used the identity field this document's own review deleted, exactly as the sibling packs table did before it was corrected. It is install_state(archive_uuid, status, installed_at), a foreign key onto packs.archive_uuid; the rename applies to every table in §5, not only to packs. status draws on the Cross-Track Contract §7.2 vocabulary, which Track C's C6 and Track F's F4 both read and write.
- **The catalog's sha256, defined:** §5's packs.sha256 never said what it digests, which is undefined for a multi-volume pack and is the value F4 verifies a download against. It is the digest of each volume file exactly as published, stored as an ordered comma-separated list for a multi-volume pack, computed by B13 at the signing step over the bytes it will serve.

Two further sweep findings against this document are real but not B1 blockers and are deferred rather than resolved here: the catalog-index sync mechanism (§5) is asserted without stating how a box discovers a new catalog version, what the index pack contains, or how removals are represented; and the automated content-safety scan named in §9's pipeline has no component id, no owning track and no phase. Both need answering before B13 is built, neither before zim2wax.
