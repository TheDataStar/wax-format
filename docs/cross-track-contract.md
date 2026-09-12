# DeltOS — Cross-Track Contract

Normative values for every vocabulary shared by more than one track

*v1 · September 2026 · created in response to the Implementability Sweep, §13*

## 1. What This Document Is For

The Implementability Sweep found roughly 140 defects across the nine design documents, and found that most of them trace to one cause: the vocabularies that more than one track consumes — tier names, profile names, role strings, status enums, topic grammars, origin patterns — are defined loosely wherever they were first needed, restated slightly differently in each document that consumes them, and pinned to literal values in none. Fixing those defects one at a time would leave the cause intact.

This document is the remedy. It owns those vocabularies as normative values. Where a track document and this document disagree, this document wins; where a track document restates a value that lives here, that restatement is descriptive and non-binding. A change to any value here is a change in one place, not in the three documents that each held a slightly different copy.

### What this document does NOT own

- **On-disk format values:** SPEC.md owns the .wax container's byte layout, endianness, flag bit positions, hash semantics, compression values and signing construction. Those were settled during the A3/A4 and A2 implementation passes (Track A §15–§17) and are already pinned in one place. This document does not restate them, because a second copy is how the problem started.
- **Values internal to a single track:** A value only one track reads stays in that track's document. The test for inclusion here is consumption by two or more tracks, not importance.
- **Design decisions still genuinely open:** Where the sweep found something unspecified and no defensible value could be derived from existing material, it is listed in §13 as open rather than given an invented value.

### How to read the tables

Every value in this document is normative and literal. Where a value was collected from an existing document, the source is cited. Where the sweep found no value anywhere and one had to be chosen, it is marked as a new decision — those are the entries most worth a second opinion, and they are listed together again in §12 so they can be reviewed as a set rather than hunted through the document.

## 2. Hardware Tiers

The single most load-bearing table in the project. Track B's min_hw_tier, Track C's requirement calculator, Track D's feature gating and Track H's service placement all key off it.

Track E's original table stated RAM and storage as ranges (pi_4 as "2–8GB"), which made the tier name useless as a gate — a pack declaring min_hw_tier: pi_4 could land on a 2GB or an 8GB board. Restated here as guaranteed floors. The floor is what any other track may assume; headroom above it is discovered at runtime and never inferred from the tier name.

| **Tier** | **Min RAM** | **Min storage** | **Arch** | **Notes** |
|---|---|---|---|---|
| pi_zero_2w | 512 MB | 16 GB | aarch64 | Kiosk-profile floor. No Track G or Track H services. |
| pi_4 | 2 GB | 32 GB | aarch64 | A 1 GB Pi 4 SKU exists and is below this floor — it is not this tier. |
| pi_5 | 4 GB | 64 GB | aarch64 | First tier meeting Track D's local-LLM floor (§6 below). |
| mini_pc | 16 GB | 256 GB | x86_64 | Community Hub and Field Ops floor. |
| generic | declared | declared | either | Installable-stack path only — see below. |

- **Total ordering:** pi_zero_2w < pi_4 < pi_5 < mini_pc. A pack or feature declaring a min_hw_tier runs on that tier and every tier above it. The word "tier" always refers to this axis and never to a Deployment Profile.
- **[New decision]** The installable-stack path gets a generic tier rather than being forced into a board name. A host that is not one of the four boards reports tier generic plus its measured RAM, storage and architecture, and is treated as equivalent to the highest preset tier whose floors it meets. This closes the sweep's finding that every tier-gated decision in three tracks was undecidable on a deployment path the project has already committed to as co-equal.
- **[New decision]** Tier name is a coarse floor for catalog filtering only. Runtime feature gating uses measured resources, not the tier name — because a tier is a floor, a box at pi_5 may have 4 GB or 16 GB, and a feature needing 8 GB must ask what the box actually has. Every box publishes both its tier and its measured RAM/storage/arch through the capability record in §9. Gating on the tier name alone is what produced the sweep's D5-on-a-2GB-Pi-4 finding.

## 3. Deployment Profiles

Profiles and tiers are deliberately separate axes: a profile says what a box runs, a tier says what the hardware is. The sweep found the two conflated in four documents despite an earlier review pass having corrected several instances — they kept recurring because no mapping between the axes was ever published. It is published here.

| **Profile** | **Wire value** | **Min tier** | **What it adds** |
|---|---|---|---|
| Kiosk | kiosk | pi_zero_2w | Tracks A–D core plus Track F's baseline daemons. No Track G, no Track H. |
| Classroom | classroom | pi_4 | Full Track C; G1/G2; Standard-tier Track H; captive portal. |
| Community Hub | community_hub | mini_pc | Full Track G; most of Track H. |
| Field Ops | field_ops | mini_pc | Community Hub plus the opt-in IoT/SDR/RF and smart-grid modules. |

- **Total ordering:** kiosk < classroom < community_hub < field_ops. "Classroom and above" and similar phrasings resolve against this ordering and no other.
- **Profile is chosen; tier is detected:** A profile is selected at provisioning or pre-seeded onto an image; a tier is measured at first boot. A profile may not be selected on hardware below its minimum tier.
- **[New decision]** "Advanced" is not a profile. It appears in Track E §9 and §13 as though it were one, in a document whose own enumeration lists only four. Field Ops already carries the opt-in modules the phrase was reaching for. Track E's two uses should be reworded to field_ops.

## 4. Roles and Permission Defaults

Track F defines these properly and both consuming tracks can build against it. Collected here because Track C and Track E each gate privileged operations on a vocabulary neither document could see, which is exactly the shape this contract exists to fix.

- **Closed set:** admin, teacher, student, guest — lowercase ASCII literals, stored in profiles.role.
- **none is not a role:** It is the sentinel query_role returns when no profile matches. It is never stored, and never appears in profiles.role's permitted values.
- **[New decision]** Default deny. Any privileged operation not listed in Track F §22's permission table is admin-only until that table adds a row. Track F states the table but never its default, leaving every operation outside the nine listed rows undecided for the two tracks that gate on it.
- **Passwordless roles:** password_hash is nullable for student and guest, never for admin or teacher, enforced as a table-level CHECK. Track F §21 states this for student only and §22 later makes guest passwordless without revisiting it.

## 5. Identity and Progress

### 5.1 Profile identity and derived account names

Track H provisions an account in up to eight third-party services per profile, and the sweep found a derivation rule specified for exactly one of them — so deprovisioning and role changes could not locate the account in the others.

- **[New decision]** One derivation, used by every adapter: localpart = "p" + the first 12 hex characters of profile_id, lowercase. This satisfies the Matrix localpart grammar (the strictest of the target services), is stable across renames, and never encodes a display name. Each adapter additionally persists its own profile_id to backend-account-id mapping under §10's adapter path, because a derivation alone cannot survive a backend that assigns its own ids.

profile_id 4c0cfba1-3e77-… →  localpart p4c0cfba13e77  →  mail p4c0cfba13e77@deltos.lan

- **Display names are never identifiers:** A profile rename changes display name only, never the localpart. Adapters must handle a profile_renamed event by updating display name alone.

### 5.2 Progress event schema

Three components share this: Track C's shell emits, Track H's Kolibri bridge writes, Track C's C8 dashboard reads. The sweep found four of its five elements undefined.

| **Field** | **Type** | **Normative value** |
|---|---|---|
| event_id | TEXT | Source-supplied, opaque. UNIQUE(event_source, event_id); writes are insert-or-ignore, so a repeated sync is idempotent. |
| profile_id | TEXT | Derived by the receiver from the sending origin — never trusted from a caller-supplied field. |
| event_source | TEXT | Closed: wax_pack · kolibri. Extending it is a change to this document. |
| pack_id | TEXT | wax_pack: the archive_uuid. kolibri: kolibri:<channel_id>:<content_node_id>, both lowercase hex. |
| event_type | TEXT | Closed: pack_opened · pack_closed · entry_viewed · unit_completed · quiz_scored. Unknown values are stored verbatim and ignored by rollups, never rejected. |
| timestamp | INTEGER | Unix epoch milliseconds, UTC. |
| seq | INTEGER | Monotonic per (profile_id, event_source), assigned by the writer. Orders events correctly when the clock is unset or steps — a real case on hardware with no RTC. |
| payload | TEXT | UTF-8 JSON object, 4 KiB serialized maximum. Oversized rows are rejected at write. |

- **[New decision]** The event_type enum, the millisecond-UTC timestamp, the monotonic seq companion, the 4 KiB payload cap, the kolibri pack_id encoding, and the event_id uniqueness constraint are all new. None existed anywhere. The uniqueness constraint is the consequential one: without it, the Kolibri sync is a poll loop with no idempotency key and duplicates every row it has already written on any re-run.
- **Guest emits nothing:** identityd rejects any progress_events insert whose profile_id resolves to role guest, so the Design Language's "nothing about a Guest session is remembered" holds regardless of which component emits.

## 6. Feature Resource Floors

The sweep found the hardware budget perfectly circular: Tracks D, G and H each defer to Track E's consolidated budget, and Track E is waiting for numbers from exactly those tracks. Someone has to state numbers first. These are the floors a feature needs, independent of tier, so a box gates on measured resources per §2.

| **Feature** | **RAM floor** | **Disk floor** | **Gate** |
|---|---|---|---|
| Shell + baseline daemons | 384 MB | 4 GB | Every profile. Fits pi_zero_2w's 512 MB floor with headroom for one pack. |
| G1 + G2 (orchestrator, proxy) | 768 MB | 8 GB | classroom and above. Previously uncosted at the lowest tier that runs it. |
| D5 small model (1–3B, Q4) | 3 GB | 6 GB | Measured RAM ≥ 4 GB — i.e. pi_5 floor, not pi_4. |
| D5 larger model (7–8B, Q4) | 8 GB | 12 GB | Measured RAM ≥ 16 GB — mini_pc. |

*These are stated as provisional floors so that Tracks D, G, H and E have a concrete number to argue with rather than a blank. Each owning track should replace its own row with a measured figure once its component runs; the per-service Track H floors are still missing entirely and are listed as open in §13.*

## 7. Service and Install State Vocabularies

### 7.1 Service health

Track G exposes this; Track F's health daemon polls it. Neither document defines the value set.

healthy · starting · unhealthy · restarting · failed · stopped

- **failed is terminal:** It means automatic restart has been abandoned, not that a restart is in progress. Track G must state a consecutive-failure ceiling after which a service enters failed rather than restarting forever; the ceiling itself is Track G's to choose.

### 7.2 Install and queue state

Track C's store UI and Track F's install pipeline read and write the same table, so they must agree on a vocabulary neither document supplies.

queued · fetching · verifying · installing · installed · update_available
failed_signature · failed_rollback · failed_transport · cancelled · removing

- **Verification failures are terminal:** failed_signature and failed_rollback require admin action and are never retried automatically. failed_transport retries with backoff.

## 8. Origins, Hostnames and the Pack/Admin Separation

The sweep's most consequential security finding sits here, and it is a cross-track one: Track C places per-pack origins on the same domain that Track G puts admin surfaces on, and Track G's forward-auth session cookie must be scoped to that shared parent to work at all. Because .lan is not on the Public Suffix List, a cookie scoped to .deltos.lan is readable and settable by every pack origin — so untrusted pack content could read or fixate an admin session. Neither document could see the collision; each half is reasonable alone.

- **[New decision]** Pack origins move to a separate zone. Admin and service surfaces stay under deltos.lan; pack content is served under deltos-packs.lan, a distinct zone sharing no cookie scope with it. No session cookie is ever scoped to deltos-packs.lan.

| **Surface** | **Origin** |
|---|---|
| Shell UI | http://127.0.0.1:40999 — reserved, outside the pack port range |
| Pack content, pre-proxy | http://127.0.0.1:PORT, PORT = 41000 + (pack_index × 32) + profile_slot |
| Pack content, post-proxy | https://pack-<pack_index>-slot-<profile_slot>.deltos-packs.lan |
| Admin, service surfaces | https://<service>.deltos.lan (admin, git, flows, cloud, wiki, mail, code, blog) |

- **Variable ranges:** pack_index is an integer 0–511, allocated by the on-device catalog at install time and never reused. profile_slot is an integer 0–31, assigned at profile creation and stable for that profile's life. The resulting port range is 41000–57383, and no other DeltOS service may bind inside it.
- **deltos.lan is normative:** Track E gives the DNS suffix as "e.g. deltos.lan" and Track E §17 then treats it as settled. It is settled: deltos.lan and deltos-packs.lan, both served by the local resolver, both covered by the internal CA.
- **[New decision]** Two wildcard certificates, not one. *.deltos.lan and *.deltos-packs.lan are issued separately, so the certificate fronting admin surfaces is not also the certificate fronting untrusted pack content. Track C cites a single shared wildcard certificate that Track G never actually defines, and Track G's only stated issuance model — per-hostname on demand — contradicts it. Track G owns closing that gap; this document fixes the shape it must close to.
- **Profile-slot exhaustion:** A device supports 32 concurrent profiles. Creation beyond that fails with a stated error rather than sharing or recycling a live profile's slot; slots are never shared between live profiles.

## 9. The Box Capability Record

Four tracks need to ask what a box actually is, and no document defines where that answer lives. §2's measured-resources rule depends on it existing.

- **[New decision]** Every box publishes one capability record, written at first boot and readable by any local component.

{ "profile": "classroom", "tier": "pi_5", "ram_bytes": 8589934592,
  "storage_bytes": 128849018880, "arch": "aarch64",
  "has_wifi": true, "can_ap": true, "has_rtc": false }

- **Path:** /var/lib/deltos/capability.json — see §10.
- **Who writes it:** Track E's provisioning tool for the appliance path; the installer for the installable-stack path. The installable-stack case is why has_wifi and can_ap are present: the sweep found a single onboarding wizard performing Wi-Fi setup on a path that explicitly targets a VPS, which has no radio. Onboarding reads this record and skips the radio-dependent steps rather than failing.

## 10. Filesystem Paths

Track E confines DeltOS state to one tree so that OS rollback has a defined boundary. The sweep found two things living under it with no assigned path.

| **Path** | **Holds** |
|---|---|
| /var/lib/deltos/capability.json | §9's capability record. |
| /var/lib/deltos/b4/ | On-device catalog database. |
| /var/lib/deltos/packs/ | Installed .wax archives and their signature sidecars. |
| /var/lib/deltos/packs-data/<archive_uuid>/ | Per-pack writable storage — the runtime_storage_bytes a manifest declares. Created at install, removed at uninstall. |
| /var/lib/deltos/identity/ | Profiles and progress events. |
| /var/lib/deltos/health/ | Health snapshots. |
| /var/lib/deltos/<service>-adapter/ | Each Track H adapter's profile-to-account mapping (§5.1). |

- **[New decision]** State written under this tree carries a schema version that its owner refuses to open if newer than it understands. This is the missing half of Track E's rollback design: /var is shared and never rolled back, so a rollback hands old code data that new code already migrated. Track E presents this as the guarantee without noticing it is also the hazard.

## 11. Smaller Shared Vocabularies

- **Language codes:** BCP-47 (en, pt-BR, zh-Hans). Track B's manifest carries them and Track D's tokenizer selection reads them; neither names a standard, and ZIM's own metadata uses ISO 639-3, so the converter must map rather than pass through. Where a source code has no BCP-47 mapping, the field is omitted rather than guessed.
- **Pack version strings:** CalVer YYYY.MM.N, ordered by the three numeric components. Track B calls the field "human-facing semver" and then gives 2026.09.1 as the example — which is not valid semver, since semver forbids the leading zero. It is CalVer; the description was wrong, not the example.
- **MQTT topics:** deltos/v1/<device_class>/<device_id>/<metric>, with device_class closed to battery · solar · grid · env · contact · aircraft · vessel, device_id matching [a-z0-9-]+ and stable per physical device from pairing, and metric carrying fixed SI units per name. Payload is {"value": …, "unit": "…", "ts": <epoch ms>}, retained, QoS 1. Commands use the reserved prefix deltos/v1/actions/ and are the only topics any component may publish to without owning the device.
- **License allowlist:** Literal SPDX identifiers, not families: CC0-1.0 · CC-BY-4.0 · CC-BY-SA-3.0 · CC-BY-SA-4.0 · GFDL-1.3-or-later · MIT · Apache-2.0 · GPL-2.0-only · GPL-3.0-only · GPL-3.0-or-later. Track B's list names families ("CC-BY", "GPL-family") and "public domain", none of which are valid SPDX ids, in a check that ships at P1.
- **[New decision]** Licensing has three outcomes, not two. A blank license is a hard build failure. A license on the allowlist builds clean. A license that is valid free text or a recognized-but-not-allowlisted identifier builds with a license_review_required flag that routes the pack to human review. Track B currently permits free text in the schema, has the builder refuse anything off the allowlist, and gives a human reviewer an "unrecognized license" state — three rules that cannot all hold, and that fail on real Wikipedia archives, which carry free-text or absent license metadata.

## 12. Every New Decision in One Place

Fourteen values in this document had no defensible source anywhere and were chosen rather than collected. They are the entries most worth disagreeing with, so they are listed together rather than left scattered.

| **§** | **Decision** | **What it resolves** |
|---|---|---|
| 2 | generic tier for installable-stack hosts | Tier gating was undecidable on a committed deployment path. |
| 2 | Tier gates catalog filtering; measured resources gate runtime | A tier is a floor, so the name alone cannot answer whether a feature fits. |
| 3 | "Advanced" is not a profile | Used as one in a document listing only four. |
| 4 | Default deny for unlisted operations | Two tracks gate on a table with no stated default. |
| 5.1 | localpart = p + 12 hex of profile_id | Deprovisioning could not locate accounts in 5 of 6 services. |
| 5.2 | event_type enum, ms-UTC timestamp, seq, 4 KiB cap, kolibri pack_id | Four of five schema elements undefined at a committed interface. |
| 5.2 | UNIQUE(event_source, event_id), insert-or-ignore | The Kolibri sync duplicated every row on any re-run. |
| 6 | Provisional RAM/disk floors for four features | The budget was circular; someone had to state numbers. |
| 8 | Pack origins move to deltos-packs.lan | Admin session cookie was readable by untrusted pack content. |
| 8 | Port formula, pack_index 0–511, profile_slot 0–31 | The derivation was named with no formula and no ranges. |
| 8 | Two wildcard certificates, not one | Track C cited a certificate Track G never defined. |
| 9 | The capability record and its path | Nothing defined where a box's own description lives. |
| 10 | Schema version on all state under /var/lib/deltos | Rollback hands old code newly-migrated data. |
| 11 | Three licensing outcomes | Three stated rules could not all hold, and failed on Wikipedia. |

## 13. Still Open — Not Invented Here

The sweep found these unspecified and no value could be derived from existing material. They are listed so they are tracked rather than silently filled in by whoever implements first.

- Per-service RAM and disk floors for all ten Track H services — the numbers Track E's consolidated budget is waiting for, and which only Track H can supply.
- The shell's IPC surface: wire format, argument types, response and error shapes for every privileged operation. This is a design session, not a value to pin, and it is the largest single gap in the set.
- The passage/chunk unit for embedding, retrieval and citation — size, overlap, boundary rule and chunk identifier. Track D's citations cannot locate anything inside a large article until this exists.
- The cross-source search ranking rule. Scores from separately-built indexes are not comparable, so a normalization must be chosen; Reciprocal Rank Fusion is the obvious candidate but it is Track D's call.
- What binds a Track H service session to the active profile, and what a profile switch does to an open one. Currently a profile switch leaves the previous profile's service session authenticated.
- The update-failure detection rule: what "known-good" means and what triggers rollback. Track E specifies the rollback boundary but never its trigger.
- Design Language: the focus-indicator token, the spacing scale, touch-target minimums, the interaction-state palette, and a reconciled status-icon vocabulary. Plus five color pairings that fail the document's own WCAG AA claim and need new values, the primary-action accent among them.

## 14. Amendment Rule

A value in this document changes here, in this document, and nowhere else. A track document that needs a different value raises it against this one rather than restating it locally — a local restatement is how three documents came to hold three slightly different copies of the same vocabulary, which is the cause this document exists to remove.

Adding a value is the same operation as changing one: if a second track begins consuming something a track document currently owns privately, it moves here at that moment, not later.

*A document like this goes stale the moment implementation outruns it. The practical guard is the same one the sweep used: any prompt written against a track document should cite this document alongside it, and anything an implementer has to invent that belongs in one of these tables is a defect report against this document, not a decision for the implementer to make quietly.*
