# DeltOS — Cross-Track Contract

Normative values for every vocabulary shared by more than one track

*v1 · September 2026 · created in response to the Implementability Sweep, §13*

## 1. What This Document Is For

The Implementability Sweep found roughly 140 defects across the nine design documents, and found that most of them trace to one cause: the vocabularies that more than one track consumes — tier names, profile names, role strings, status enums, topic grammars, origin patterns — are defined loosely wherever they were first needed, restated slightly differently in each document that consumes them, and pinned to literal values in none. Fixing those defects one at a time would leave the cause intact.

This document is the remedy. It owns those vocabularies as normative values. Where a track document and this document disagree, this document wins; where a track document restates a value that lives here, that restatement is descriptive and non-binding. A change to any value here is a change in one place, not in the three documents that each held a slightly different copy.

### What this document does NOT own

- **On-disk format values:** SPEC.md owns the .wax container's byte layout, endianness, flag bit positions, hash semantics, compression values and signing construction. Those were settled during the A3/A4 and A2 implementation passes (Track A §15–§17) and are already pinned in one place. This document does not restate them, because a second copy is how the problem started.
- **Where that boundary actually falls:** SPEC.md owns bytes inside the container. This document owns how a shared value is rendered as text anywhere it is written — a build report, a signature's trusted comment, a log line, a config file, a catalog row. The two met during the A2d pass: SPEC.md pinned archive_uuid as bare hex in the minisign trusted comment while this document pinned the canonical text form as hyphenated. That is this document's call, not SPEC.md's, because the value crosses tracks; SPEC.md was corrected, not this one. Where they meet again, the same split applies — and because comparison of that identifier is by parsed value rather than by string, correcting a rendering never invalidates artifacts already signed.
- **Values internal to a single track:** A value only one track reads stays in that track's document. The test for inclusion here is consumption by two or more tracks, not importance.
- **Design decisions still genuinely open:** Where the sweep found something unspecified and no defensible value could be derived from existing material, it is listed in §13 as open rather than given an invented value.

### How to read the tables

Every value in this document is normative and literal. Where a value was collected from an existing document, the source is cited. Where the sweep found no value anywhere and one had to be chosen, it is marked as a new decision — those are the entries most worth a second opinion, and they are listed together again in §12 so they can be reviewed as a set rather than hunted through the document.

## 2. Hardware — Minimum and Preferred Spec, Measured

The single most load-bearing section in the project. Track B's pack requirements, Track C's requirement calculator, Track D's feature gating and Track H's service placement all key off it.

**DeltOS is hardware-agnostic. Capability follows measured resources. No document may gate a feature on a device model.** This replaces the four device-named tiers (`pi_zero_2w`, `pi_4`, `pi_5`, `mini_pc`) that earlier revisions of this section owned. Board names may appear as *examples of a resource class*, never as the gate.

### 2.1 The two reference specs

| | **RAM** | **Storage** | **Arch** | **GPU** | Example machine |
|---|---|---|---|---|---|
| **Minimum spec** | 2 GB | 32 GB | `aarch64` | none assumed | Raspberry Pi 4/5-class ARM64 board |
| **Preferred spec** | 16 GB | 256 GB | `x86_64` | optional, discovered | x86 mini-PC |

The example column is descriptive. A machine qualifies by meeting the resource figures, whatever it is.

- **The Pi Zero 2 W is retired as a target** and appears nowhere as a tier, a floor, or a profile minimum.
- Minimum spec is the floor the whole system must run at. Preferred spec is what the resource-hungry features need. Everything between is decided by measurement, not by naming.

### 2.2 What a box measures, and when

A box measures its own resources **at first boot** and unlocks capability from what it measures. The measured values are published in the box capability record (§9), which is the single place any component asks what this box actually is.

Four dimensions, and only these four, are measured and compared: **RAM**, **storage**, **CPU architecture**, and **GPU presence**.

### 2.3 The resource requirement — what replaces `min_hw_tier`

A pack, feature or app declares the resources it needs. The box runs it when the capability record meets them. This is the replacement vocabulary; it is a **declaration of need**, never a device name.

| **Field** | **Required** | **Meaning** |
|---|---|---|
| `min_ram_bytes` | Required | Steady-state RAM the thing needs to run at all. |
| `min_storage_bytes` | Required | Storage it needs beyond its own archive. |
| `arch` | Required | `aarch64` · `x86_64` · `any`. `any` means architecture-independent content. |
| `gpu` | Optional | Omit when irrelevant. `required` — will not run without one. `preferred` — runs either way, uses one when measured. |

- **The comparison rule:** a box runs a thing when every declared field is met by the capability record. There is no ordering to place a box in and no name to match — a comparison of numbers, and nothing else.
- **Omission is not zero.** An optional field that does not apply is omitted, never written as `0` or `false`. This matches §11's rule for the pack manifest.
- **`gpu: preferred` never gates.** It selects an execution path, which is how Track D's local model degrades from GPU to CPU to absent without a second declaration.

### 2.4 Code currently disagrees — a recorded, deliberate lag

`wax-builder` enforces the retired vocabulary: `MIN_HW_TIERS` is a closed set of the four board names in `crates/wax-builder/src/config.rs`, asserted by fifteen checks across three tests in `crates/wax-builder/tests/manifest_schema.rs`, and present in the example manifest and the `zim2wax` fixtures.

**This document states the target; the code has not moved yet.** Migrating the enum, the validator messages, the tests and the fixtures to §2.3 is **the first implementation directive after this one** and is deliberately out of scope here. Until it lands, a built pack still carries `min_hw_tier` and §11 records both states. This lag is recorded rather than hidden precisely because an undocumented disagreement between a document and its implementation is the failure class this document exists to remove.

## 3. Deployment Profiles

Profiles and hardware are deliberately separate axes: a profile says **what a box runs**, the capability record (§9) says **what the hardware is**. The sweep found the two conflated in four documents; the mapping is published here so they stop drifting apart.

Each profile states the **minimum resources** it needs — never a device, and no longer a tier name.

| **Profile** | **Wire value** | **Min RAM** | **Min storage** | **What it adds** |
|---|---|---|---|---|
| Kiosk | `kiosk` | 2 GB | 32 GB | Tracks A–D core plus Track F's baseline daemons. No Track G, no Track H. |
| Classroom | `classroom` | 2 GB | 32 GB | Full Track C; G1/G2; Standard Track H; captive portal. |
| Community Hub | `community_hub` | 16 GB | 256 GB | Full Track G; most of Track H. |
| Field Ops | `field_ops` | 16 GB | 256 GB | Community Hub plus the opt-in IoT/SDR/RF and smart-grid modules. |

- **Total ordering:** `kiosk` < `classroom` < `community_hub` < `field_ops`. "Classroom and above" and similar phrasings resolve against this ordering and no other.
- **Profile is chosen; resources are measured.** A profile is selected at provisioning or pre-seeded onto an image; resources are measured at first boot. A profile may not be selected on a box below its stated floor.
- **Kiosk now floors at the minimum spec.** Its previous floor was the retired `pi_zero_2w` at 512 MB. Kiosk and Classroom therefore share the §2.1 minimum spec; they differ in what they run, not in what they demand. Kiosk remains the profile with no Track G and no Track H, which is what makes it the cheap one — not a smaller board.
- **Community Hub and Field Ops** carry forward the RAM and storage floors their previous `mini_pc` mapping guaranteed. Only the device name was dropped; the numbers are unchanged.
- **[Answered] `community_hub` and `field_ops` do *not* require `x86_64`.** The floor is the resources above and nothing else: **a 16 GB ARM64 box meeting the RAM and storage numbers qualifies for both profiles.** Their previous `mini_pc` mapping bundled 16 GB with an x86 architecture in a single label, so the repo never recorded which was actually required — only the numbers ever were. **x86 remains the preferred spec (§2.1), never a gate.** Requiring it would reintroduce precisely the device-shaped gate §2 exists to remove.
- **[New decision]** "Advanced" is not a profile. It appears in Track E §9 and §13 as though it were one, in a document whose own enumeration lists only four. Field Ops already carries the opt-in modules the phrase was reaching for; Track E's two uses are reworded to `field_ops`.
## 4. Authorization — Permissions, and Roles as Data

**This section owns the authorization model. No other document holds a role list.**

**Authentication and authorization are two axes and this document keeps them apart.** §5.1 owns *who you are and how you sign in* — the identity archetypes, the account derivation, which archetypes carry a password. **This section owns what you may do.** Conflating the two is what produced the closed four-role literal that this section previously held: a sign-in shape was being used as a permission set, so every new authority needed either a new sign-in shape or a carve-out.

### 4.1 A permission is the unit of "may do this"

A **permission** is a named capability. Representative examples, drawn from the catalogue the current components imply:

`content.install` · `pack.publish` · `site.host` · `security_lab.use` · `community.moderate` · `service.restart` · `fleet.manage` · `profile.manage` · `benchmark.view`

- **These are illustrative, not the catalogue.** The catalogue is **assembled at runtime from app manifests** (§15) — each app contributes the permissions it defines. Freezing a closed list here would recreate, one layer down, exactly the rigidity this section removes.
- The permission catalogue is **owned by the identity service**, which is the component that can see every installed app's declarations at once.

### 4.2 A role is a named bundle of permissions, stored as data

- **A role is data, never a code literal.** It is a named set of permissions, stored and editable, not an enum member a compiler knows about.
- **The four existing roles ship as built-in default bundles** — `admin`, `teacher`, `student`, `guest`. They are not deleted and not special-cased; they are the bundles DeltOS ships so a box is usable the moment it boots. `admin` holds every permission; the others hold sensible defaults.
- **An admin composes additional roles from the catalogue, with no code change.** Librarian, moderator, author, IT operator, parent, lab operator — each is a bundle an administrator assembles, and none requires a release.

#### The shipped default bundles

What the four built-ins hold on a fresh box. **These are seed values, not a schema** — an admin may edit any of them, and the catalogue they draw from grows as apps are installed (§4.1).

| **Capability** | `admin` | `teacher` | `student` | `guest` |
|---|---|---|---|---|
| `content.install` (install/remove packs, mount removable media) | ✓ | | | |
| `network.configure` (join network, configure AP) | ✓ | | | |
| `profile.manage` — own class or group | ✓ | ✓ | | |
| `progress.view` — across own class or group | ✓ | ✓ | | |
| `progress.view.self` | ✓ | ✓ | ✓ | |
| `admin.console` (F1) | ✓ | | | |
| `fleet.manage` (F6 pairing) · `vault.access` (F13) | ✓ | | | |
| Content browsing | ✓ | ✓ | ✓ | ✓ (manifest-permitted packs only) |

- **`admin` holds every permission in the catalogue**, including permissions contributed by apps installed later. That is a property of the bundle, not a list to maintain.
- **This table replaces the fixed operations matrix Track F previously held.** The operations are unchanged; what changed is that they are now permissions in a catalogue rather than columns against a closed enum, so adding a ninth capability no longer requires a new row in a central table that two other tracks gate on.
- **The teacher surface stays in-shell.** `profile.manage` and `progress.view` are class-scoped and reached through the shell, never through F1's console — which is why `admin.console` is a separate permission rather than something implied by holding the others.
- **`none` is not a role.** It remains the sentinel meaning no profile matched. It is never stored and never appears among a profile's roles.

### 4.3 Default-deny, now general

**An identity holds only the permissions its roles grant. Anything ungranted is denied.**

This is the same posture the previous permission table implied, made explicit and made extensible. Previously the rule read "any privileged operation not listed in Track F's table is admin-only until that table adds a row" — which was default-deny with a central table as the only way to grant anything. The rule is now general: the check is against the identity's **effective permissions** — the union of its roles' bundles — and a new authority arrives by an app declaring it, not by an edit to a central table.

### 4.4 Sign-in shape and permission set are independent

**A passwordless identity can hold any role bundle.** A passwordless student profile simply carries the `student` bundle; nothing about being passwordless constrains what a bundle may contain. §5.1's rules about which archetypes carry a password are unchanged and are about authentication only.

This independence is what makes the model work at the floor. The minimum spec runs passwordless profiles for good reasons — shared devices, young learners, no keyboard — and under the old literal that shape also fixed what those profiles could do.

### 4.5 What this replaces, and why

The previous model was a **closed set of four lowercase literals** stored in `profiles.role`, with a fixed eight-row operation table.

It broke on the catalogue. Hosting owners, community moderators, security-lab operators, content authors, IT staff and cross-org fleet managers are **more distinct authorities than four names can carry**, and the first attempt to add one — the security lab — had to be written as "a permission, not a fifth role." That carve-out was the tell: the model was already being worked around the first time it was extended. Hardcoding each new authority is precisely the rigidity §15's app contract removed everywhere else in the system.

## 5. Identity and Progress

### 5.1 Identity: how a person signs in, and how their account is derived

**This subsection owns authentication only. What an identity may *do* is §4's, and the two are deliberately separate.** An identity archetype describes a sign-in experience; it does not describe a permission set. A passwordless profile can hold any role bundle §4 defines.

#### Identity archetypes

These are unchanged, and they are about sign-in shape alone:

| **Archetype** | **Sign-in** | **Notes** |
|---|---|---|
| Passworded | `admin`, `teacher` | `password_hash` is **never** null, enforced as a table-level CHECK. |
| Passwordless | `student`, `guest` | `password_hash` is nullable. Chosen for shared devices, young learners and hardware with no comfortable keyboard. |

- **Guest emits nothing.** `identityd` rejects any `progress_events` insert whose profile resolves to guest, so "nothing about a Guest session is remembered" holds regardless of which component emits. This is a property of the guest identity, not of a permission.
- **These archetype names coincide with the four built-in role bundles (§4.2), and that coincidence is not the model.** A box that composes a `librarian` role assigns it to an identity whose sign-in shape is still one of the two above. Adding a role never adds an archetype.

#### Derived account names


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
- **Guest emits nothing** — owned by §5.1 and not restated here. It is a property of the guest identity, which is why it lives with the archetypes rather than with this schema.

## 6. Feature Resource Floors

The sweep found the hardware budget perfectly circular: Tracks D, G and H each defer to Track E's consolidated budget, and Track E waits for numbers from exactly those tracks. Someone has to state numbers first. These are the floors a feature needs, expressed the only way §2 permits — **as measured resources**, never as a device or a tier name.

| **Feature** | **`min_ram_bytes`** | **`min_storage_bytes`** | **`gpu`** | **Gate** |
|---|---|---|---|---|
| Shell + baseline daemons | 384 MB | 4 GB | — | Every profile. Leaves headroom for one pack within the §2.1 minimum spec. |
| G1 + G2 (orchestrator, proxy) | 768 MB | 8 GB | — | `classroom` and above. Previously uncosted at the cheapest profile that runs it. |
| D5 small model (1–3B, Q4) | 3 GB | 6 GB | `preferred` | Measured RAM ≥ 4 GB. |
| D5 larger model (7–8B, Q4) | 8 GB | 12 GB | `preferred` | Measured RAM ≥ 16 GB. |

- **The D5 rows are why the tier names had to go.** Both were previously written as "`pi_5` floor, not `pi_4`" and "`mini_pc`" — but a tier is a floor, so a `pi_5` box may carry 4 GB or 16 GB, and the name answered the wrong question. The gate is the measured number and always was.
- **`gpu: preferred` on both D5 rows** is what lets Track D's local model use a measured GPU where one exists and degrade to CPU where it doesn't, without a second declaration. It never gates.
- **Per-service floors are no longer this table's problem.** §15's app contract requires every app and service to declare its own measured floor in its manifest. The consolidated budget is therefore **the sum of declared floors**, computed from the installed set, rather than one number blocked on every track reporting at once. This closes the circularity at its cause.

*The four rows above remain provisional until each owning track measures its own component on real hardware, per the measure-don't-estimate rule. A provisional figure is marked as such and is a number to argue with, not evidence.*
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

- **Variable ranges:** `pack_index` is an integer 0–511, allocated by the on-device catalog at install time. `profile_slot` is an integer 0–31, assigned at profile creation and stable for that profile's life. The resulting port range is 41000–57383, and no other DeltOS service may bind inside it.
- **[Amended decision] A `pack_index` may be reused, but only after its stored browser data is cleared.** This document previously said an index was *never* reused. That was a security-motivated rule with an unacceptable consequence: 512 installs over a box's whole life exhausted the space permanently, and a long-lived classroom box would eventually refuse to install anything. The rule is therefore bounded rather than absolute — an address returns to the pool only once everything the previous occupant stored against that origin (cookies, `localStorage`, IndexedDB, cache storage, service-worker registrations) has been cleared. Until the clear completes the address stays allocated. Reuse without the clear would hand a new pack the previous pack's origin-scoped state, which is the outcome the original rule existed to prevent; clearing first preserves the guarantee without the exhaustion. Owned here; Track C's catalog performs the clear.
- **deltos.lan is normative:** Track E gives the DNS suffix as "e.g. deltos.lan" and Track E §17 then treats it as settled. It is settled: deltos.lan and deltos-packs.lan, both served by the local resolver, both covered by the internal CA.
- **[New decision]** Two wildcard certificates, not one. *.deltos.lan and *.deltos-packs.lan are issued separately, so the certificate fronting admin surfaces is not also the certificate fronting untrusted pack content. Track C cites a single shared wildcard certificate that Track G never actually defines, and Track G's only stated issuance model — per-hostname on demand — contradicts it. Track G owns closing that gap; this document fixes the shape it must close to.
- **Profile-slot exhaustion:** A device supports 32 concurrent profiles. Creation beyond that fails with a stated error rather than sharing or recycling a live profile's slot; slots are never shared between live profiles.

## 9. The Box Capability Record

Four tracks need to ask what a box actually is, and no document defined where that answer lives. **§2's measured-resources rule depends entirely on this record existing** — with the device-name tiers retired, this is now the *only* place a component can learn what hardware it is running on.

- **[New decision]** Every box publishes one capability record, written at first boot from its own measurements and readable by any local component.

```json
{ "profile": "classroom",
  "ram_bytes": 8589934592, "storage_bytes": 128849018880,
  "arch": "aarch64", "gpu": "none",
  "has_wifi": true, "can_ap": true, "has_rtc": false }
```

- **`tier` is gone.** Earlier revisions carried a `"tier": "pi_5"` field alongside the measurements. It is removed, not deprecated: a name that summarised the numbers beside it could only ever disagree with them, and gating on it is what produced the sweep's "D5 on a 2 GB Pi 4" finding. Components compare the measured fields (§2.3).
- **`gpu`** is `none` or the class of accelerator measured. A feature declaring `gpu: preferred` reads this to choose an execution path; a feature declaring `gpu: required` will not start against `none`.
- **Path:** `/var/lib/deltos/capability.json` — see §10.
- **Who writes it:** Track E's provisioning tool for the appliance path; the installer for the installable-stack path. The installable-stack case is why `has_wifi` and `can_ap` are present: the sweep found a single onboarding wizard performing Wi-Fi setup on a path that explicitly targets a VPS, which has no radio. Onboarding reads this record and skips the radio-dependent steps rather than failing.
- **The installable-stack path needs no special case now.** It previously required a `generic` tier so that a non-board host had a name to report. With names gone, such a host simply publishes its measurements like any other box, and the `generic` tier is retired along with the other four.

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

## 11. The Pack Manifest (B3) and Its Field Formats

This table was previously written out in full in two documents — Track B §2, which designed it, and Track A §16, which reproduced it "so this document is self-sufficient." Two copies is precisely the duplication this contract exists to remove, and it failed exactly as predicted: Track B gained a guest_accessible field per product direction, Track A's copy never did, and wax-builder — built against Track A's copy — now rejects a valid manifest as carrying an unknown key. This is the one normative copy. Both track documents now cite it instead of restating it.

| **Key** | **Req?** | **Value domain** |
|---|---|---|
| name | Required | Display name for the launcher. |
| icon | Required | Path within the archive to an icon entry; must resolve to a real entry at build time. A root-level path such as icon.svg is valid. Not a data:/http:/file: URI. |
| category | Required | One of: reference · education · media · tools · civic · health. |
| license | Required | SPDX identifier or free text — see the licensing rules below. |
| attribution | Required | Human-readable credit line. |
| version | Required | CalVer YYYY.MM.N — see below. |
| min_ram_bytes | Required | Steady-state RAM the pack needs to run at all, in bytes. Positive; a floor of zero gates nothing. |
| min_storage_bytes | Required | Storage needed beyond the archive itself, in bytes. Positive. |
| arch | Required | One of: `aarch64` · `x86_64` · `any`. Most web packs are architecture-independent and declare `any`. |
| entry_point | Required | Path within the archive to the launch target; must resolve to a real entry at build time. |
| guest_accessible | Optional | Boolean, default false. Whether the pack appears in a Guest profile's launcher grid. The author's default only — an admin's per-pack override lives in the on-device catalog, since the manifest is inside the signed archive and an admin cannot alter it. |
| gpu | Optional | One of: `required` · `preferred`. Omitted entirely when an accelerator is irrelevant — never `none` or `false`. `preferred` selects an execution path and never gates. |
| runtime_ram_bytes | Optional | Steady-state memory this pack's own services need once running. Omit when negligible rather than writing 0. |
| runtime_storage_bytes | Optional | Writable storage needed beyond the archive itself. Omit when none. |
| languages | Optional | Comma-separated BCP-47 codes — see below. |
| depends_on | Optional | Comma-separated archive_uuid values — see below. |

- **Ten required, six optional, nothing else:** Any key outside this table is a build error. There is no id field — archive_uuid is the only identity a pack carries. There is no total_size_bytes — archive size lives in the catalog (Track A §17, Track B §19). Adding a key here is a change to this document, and to this document only.

- **[New decision] The hardware requirement is four measured fields.** `min_hw_tier` is replaced by `min_ram_bytes`, `min_storage_bytes` and `arch` as required fields, plus optional `gpu` — the §2.3 vocabulary, identical to what a feature or an app declares, so one comparison rule serves packs, features and apps alike. **This has shipped:** the manifest is ten required, six optional.
- **The code has migrated.** `wax-builder` and `zim2wax` now validate and emit the four measured fields; the closed board-tier set is gone from shipped code. `min_hw_tier` in a `wax-pack.toml` is a build error carrying the replacement fields.
- **A pack built before the migration still opens**, and this is tested against real pre-migration artifacts rather than fixtures written for the test. On read, a retired `min_hw_tier` is **mapped** to its resource floor — never rewritten into the pack.

#### Legacy tier floors — read-time compatibility, not buildable values

**[New decision]** Each retired tier maps to **its own historical resource floor**, not to the current minimum:

| Retired tier | Maps to | Why that point |
|---|---:|---|
| `pi_zero_2w` | 2 GB / 32 GB | It sat *below* today's minimum. Mapping **up** is safe — nothing below the minimum spec is supported at all, so there is no lower floor to resolve to. |
| `pi_4` | 2 GB / 32 GB | Already the minimum spec (§2.1). Unchanged. |
| `pi_5` | **4 GB / 64 GB** | Its real historical point, *above* the minimum. |

`arch` maps to `any` for all three: the old names bundled an architecture with their resource point, and only the numbers were ever the requirement.

- **The rule these three values encode: a legacy pack must never resolve to a floor *lower* than the one it was built against.** Doing so would let a box install content it cannot run — a silent failure that surfaces only when a learner opens the pack. Mapping level or upward is always safe; mapping downward never is. Only `pi_5` ever sat above the minimum, so only `pi_5` needs a point of its own.
- **These are read-time compatibility values for packs already in the wild. They are not buildable tiers.** Declaring `min_hw_tier` in a `wax-pack.toml` remains a hard error, and the four names stay retired for authoring — a new pack states explicit resource fields. This table exists so that old packs keep working, not so that the tier axis can come back.
- **A `min_hw_tier` value outside these three cannot be mapped** and is reported as unresolvable rather than guessed at. That is the one case where a legacy pack's requirement is genuinely unknown, and inventing a floor for it would be the exact failure the rule above prevents.

*This closes the open item recorded when the migration landed, which noted that `pi_5` had no stated resource point and that mapping it to the minimum lost a guarantee. The guarantee is no longer lost: `pi_5` resolves to the floor it was built against.*
- **[New decision]** guest_accessible is added to the normative list. It existed in Track B §2 and in no other document, which made every conforming builder reject it. Its admin-override half is explicitly relocated to the catalog, because an admin cannot modify a field sealed inside a signed archive — the same trap that removed total_size_bytes.

### Field formats

- **[New decision]** archive_uuid's text form is canonical lowercase hyphenated (8-4-4-4-12, as in 4c0cfba1-3e77-4b1e-9a02-1f9b3c5d7e01). Tools accept the bare 32-hex form on input and normalize on write; nothing ever emits the bare form. Four tracks serialize this value and the repo already carries two spellings — wax-builder's inspect prints bare hex while the Contract's own examples were hyphenated — so a catalog comparing strings rather than parsed UUIDs would silently fail to match a pack against itself.
- **[New decision]** Language codes are validated against the IANA subtag registry, not merely checked for well-formedness. "english" and "zz" are both syntactically valid BCP-47 and both wrong, and a mistyped language silently degrades search rather than failing visibly, so the stricter check is worth its cost. Validators embed a registry snapshot, which means a subtag registered after that snapshot is rejected until the dependency is bumped — a routine update, noted here so it is a known property rather than a surprise.
- **Language codes:** BCP-47 (en, pt-BR, zh-Hans). Track B's manifest carries them and Track D's tokenizer selection reads them; neither names a standard, and ZIM's own metadata uses ISO 639-3, so the converter must map rather than pass through. Where a source code has no BCP-47 mapping, the field is omitted rather than guessed.
- **[New decision]** N is any integer from 1 upward that increases within a given month — a release counter for a hand-built pack, and the source date's day-of-month for a converted one. Both are valid and both order correctly, which is what N exists for; B1 was right that the day is the only reading under which deriving a version from a source date stays a reformat rather than a computation. What N must never be is a value that can decrease within a month.
- **[New decision]** CalVer's digits are pinned: YYYY is four digits, MM is a zero-padded two-digit month 01–12, and N has no leading zero. So 2026.09.1 and 2026.08.20 are valid; 2026.9.1, 2026.13.1 and 2026.09.01 are not. Zero-padding the month keeps lexical and numeric ordering identical, and matches the YYYY-MM-DD shape a ZIM's own Date metadata carries, so zim2wax's derivation is a reformat rather than a computation.
- **Pack version strings:** CalVer YYYY.MM.N, ordered by the three numeric components. Track B calls the field "human-facing semver" and then gives 2026.09.1 as the example — which is not valid semver, since semver forbids the leading zero. It is CalVer; the description was wrong, not the example.
- **MQTT topics:** deltos/v1/<device_class>/<device_id>/<metric>, with device_class closed to battery · solar · grid · env · contact · aircraft · vessel, device_id matching [a-z0-9-]+ and stable per physical device from pairing, and metric carrying fixed SI units per name. Payload is {"value": …, "unit": "…", "ts": <epoch ms>}, retained, QoS 1. Commands use the reserved prefix deltos/v1/actions/ and are the only topics any component may publish to without owning the device.
- **License allowlist:** Literal SPDX identifiers, not families: CC0-1.0 · CC-BY-4.0 · CC-BY-SA-3.0 · CC-BY-SA-4.0 · GFDL-1.3-or-later · MIT · Apache-2.0 · GPL-2.0-only · GPL-3.0-only · GPL-3.0-or-later. Track B's list names families ("CC-BY", "GPL-family") and "public domain", none of which are valid SPDX ids, in a check that ships at P1.
- **[New decision]** Licensing has three outcomes, not two. A blank license is a hard build failure. A license on the allowlist builds clean. A license that is valid free text or a recognized-but-not-allowlisted identifier builds with a license_review_required outcome that routes the pack to human review. Track B currently permits free text in the schema, has the builder refuse anything off the allowlist, and gives a human reviewer an "unrecognized license" state — three rules that cannot all hold, and that fail on real Wikipedia archives, which carry free-text or absent license metadata.
- **[New decision]** An absent license is not a blank license, and the two must not share an outcome. This document said "blank is a hard build failure" and predicted sources carrying "free-text or absent" metadata — then resolved only the free-text half. B1 found the consequence: no ZIM obtainable anywhere, including the 2026 English Wikipedia, carries License metadata at all, so the rule as written forbade converting the flagship P1 content. Absence means unknown, not unlicensed — Wikipedia is CC-BY-SA whether or not its metadata says so. Resolved as a fourth outcome: where a source states no license, the operator must supply one with a --license flag, and the resulting pack always carries license_review_required so a human checks the claim. A build with neither a source license nor an operator-supplied one still hard-fails, which keeps the property the original rule existed for — nothing ships with its licensing unexamined.
- **[New decision]** The same rule covers attribution. §11 requires it; Track B §20 derived it as Creator, else Publisher, else the empty string — which produces a value §11 rejects. attribution stays required, because CC-BY compliance depends on it and a missing credit line is a licensing problem rather than a cosmetic one. Where a source carries neither field, an --attribution flag supplies it, the same shape as --license. An operator statement is the sanctioned path for a required field the source lacks; inventing one silently is not.
- **[New decision]** Allowlist matching is case-insensitive, normalized to SPDX's canonical casing on write. SPDX's own specification treats identifiers as case-insensitive, and exact matching would route cc-by-sa-4.0 to human review while CC-BY-SA-4.0 builds clean — which on real Wikipedia archives means sending the flagship P1 content to a moderator over letter case. "Literal" in the list above constrains which identifiers are allowed, not how they are capitalized.
- **[New decision]** license_review_required is not a manifest key. It is a build-report outcome — wax-builder emits it alongside the archive, the catalog's intake reads it to route the pack to review, and the catalog carries the resolved review status from then on. Putting it in the manifest would repeat the total_size_bytes mistake exactly: a value that changes after the build, sealed inside an immutable, signature-covered table, with no permitted path to correct it once a reviewer approves the pack. The rule generalizes — the manifest holds what the author knows at build time and nothing whose value can change afterwards.

### The build report

Introducing license_review_required as "a build-report outcome" created a cross-track interface — wax-builder writes it, the catalog's intake reads it — and then left it unowned, which is the defect this document exists to prevent and which it committed itself. Pinned here.

- **[New decision]** Every successful build writes <archive-filename>.build-report.json beside the archive, by default rather than on request. The catalog's intake cannot depend on a report a builder may or may not have been asked to emit, and an absent report is a failed intake rather than an assumed-clean pack.

{ "report_version": 1,
  "archive_uuid": "4c0cfba1-3e77-4b1e-9a02-1f9b3c5d7e01",
  "archive_filename": "wikipedia-en.wax",
  "built_at": 1789198261422,
  "builder_version": "wax-builder 0.3.1",
  "entry_count": 6512044, "redirect_count": 2210338, "skipped_count": 1204,
  "license": "CC-BY-SA-4.0",
  "license_review_required": false,
  "signed": true,
  "warnings": [ { "code": "unsupported_mimetype", "count": 1204 } ] }

- **Field rules:** built_at is Unix epoch milliseconds UTC, matching §5.2's timestamp rule. archive_uuid is canonical hyphenated. warnings carries one entry per distinct code with a count, not one entry per occurrence — a Wikipedia conversion skipping a million entries must not produce a million-line report. Readers ignore unknown keys; adding a key keeps report_version, changing or removing one bumps it.
- **[New decision]** Warning codes are a closed vocabulary owned here, not free text. The schema above showed one code by example, which left a builder free to invent its own set — and the catalog's intake branches on these, so an invented code is an intake that silently fails to route. The codes below are those the documents already require; a builder emitting any code not on this list reports it for inclusion here rather than shipping it.

| **Code** | **Means** |
|---|---|
| unsupported_mimetype | An entry was skipped because v0 does not carry its media type. References to it are left intact. |
| redirect_cycle | A redirect chain closed on itself and was dropped. |
| redirect_dangling | A redirect's terminus does not exist and was dropped. |
| invalid_path | A source path could not be canonicalized into a valid pack path and was dropped. |
| reserved_prefix_collision | A source path collided with a reserved prefix (_assets/, _meta/) and was dropped. |
| license_operator_supplied | The source stated no license and the operator supplied one. Always accompanies license_review_required. |
| attribution_operator_supplied | The source carried neither Creator nor Publisher and the operator supplied the credit line. |
| icon_generated | The source carried no illustration and a placeholder was generated. |

- **What it is not:** Not a trust artifact. It sits outside the signature, so intake treats it as the builder's own claim about its run — useful for routing and diagnostics, never as evidence about a pack's contents. Anything intake must trust is verified against the signed archive itself.

## 12. Every New Decision in One Place

Every value in this document that had no defensible source anywhere — chosen rather than collected — is listed here. These are the entries most worth disagreeing with, so they sit together rather than scattered. **The list is stated by enumeration, not by count:** an earlier revision opened with a fixed total that the table had already outgrown.

| **§** | **Decision** | **What it resolves** |
|---|---|---|
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
| 11 | The B3 field table moves here, guest_accessible included | Two copies drifted; a valid manifest was rejected as unknown-key. |
| 11 | archive_uuid is canonical lowercase hyphenated | Four tracks serialize it; the repo already held two spellings. |
| 11 | license_review_required is a build report, not a manifest key | In the manifest it would repeat the total_size_bytes trap exactly. |
| 1 | SPEC.md owns container bytes; this document owns text renderings | The two collided over archive_uuid in a signature's trusted comment. |
| 11 | CalVer digits pinned (zero-padded month, unpadded counter) | YYYY.MM.N admitted four readings; ZIM's own Date reformats cleanly into one. |
| 11 | SPDX matching is case-insensitive | Exact matching sent every lowercase-licensed Wikipedia pack to a moderator. |
| 11 | Build report: default-emitted, named, schema pinned | It is a cross-track interface this document created and left unowned. |
| 11 | BCP-47 validated against the IANA registry, not just well-formedness | "english" is well-formed and wrong; a typo should fail at build, not at search. |
| 11 | Absent license is a fourth outcome, not the blank one | No real ZIM states a license; the rule as written forbade converting Wikipedia. |
| 11 | attribution stays required; --attribution supplies it | §20 derived an empty string, which §11 rejects. CC-BY needs the credit line. |
| 11 | N may be a counter or a source date's day | Day-of-month is the only reading under which a version derivation stays a reformat. |
| 11 | Warning codes are a closed, listed vocabulary | Intake branches on them; an invented code is a silently misrouted pack. |
| 2 | **Device-name tiers retired entirely; capability follows measured resources** | A tier name summarised numbers sitting beside it and could only disagree with them. Supersedes the three `generic`/tier-gating decisions this register previously carried. |
| 2.3 | **`min_ram_bytes` / `min_storage_bytes` / `arch` / optional `gpu` replace `min_hw_tier`** | A pack, feature and app now declare need in one vocabulary, compared by numbers with no ordering to place a box in. |
| 3 | **Profiles floor on measured resources, not a tier** | Kiosk's floor was the retired Pi Zero 2 W; the mapping had to be restated without it. |
| 8 | **`pack_index` may be reused once its stored browser data is cleared** | "Never reused" exhausted 512 addresses over a box's life and eventually refused all installs. |
| 9 | **`tier` removed from the capability record** | A name beside the measurements could only ever contradict them. |
| 13 | **The blocking decisions are enumerated here, with answers** | They were referred to as a known set that no document held. |
| 15 | **The app contract — one manifest five platform components read** | Adding an app was five edits in five components, so catalogue cost grew with catalogue size. |
| 15 | **Each app declares its own measured resource floor** | The consolidated hardware budget was circular across four tracks. |
| 16 | **No telemetry, with F14 aggregate-local as the sole exception** | Stated in no document, and an unstated privacy property erodes one track at a time. |
| 16 | **Plain HTTP for unmanaged visitors; the secure-context cost is recorded** | Features needing a secure context are unavailable to visitors' phones, which is a design constraint, not a deployment detail. |
| 4 | **Roles become data bundles over a permission catalogue; the closed four-role literal is retired** | Four names could not carry the authorities the catalogue needs, and the first extension attempt had to be written as a carve-out. |
| 4 | **Default-deny generalised to effective permissions** | The previous rule made a central table the only way to grant anything. |
| 5.1 | **Authentication and authorization separated explicitly** | A sign-in shape was doing duty as a permission set, which is what produced the closed literal. |
| 15 | **Apps declare permissions required and defined, never roles** | A new app introducing an authority had no way to do so without a central edit. |
| 3, 13.2 | **`community_hub` / `field_ops` floor is resources, not architecture** | The `mini_pc` label bundled 16 GB with x86; only the numbers were ever the requirement. |
| 11 | **Legacy tier floors: `pi_zero_2w`/`pi_4` → 2 GB/32 GB, `pi_5` → 4 GB/64 GB** | A legacy pack must never resolve below the floor it was built against; only `pi_5` ever sat above the minimum. Read-time only — no tier becomes buildable again. |

## 13. The Blocking Decisions — Enumerated, With Their Answers

**This is the list.** The project referred to "the blocking decisions" as a known set for some time without any document holding it; this section is that list, and it is the only one. A decision that blocks more than one track belongs here, answered or explicitly open.

Each entry carries the settled direction. **Where an entry still needs its own design session, it says so** — a recorded direction is not a finished design, and reading one as final is the mistake this section exists to prevent.

| # | Decision | Answer | State |
|---|---|---|---|
| 1 | **Search-index ownership** | **Track A owns the index bytes**, as an *additive minor version* of the format. A pack declares the tokenizer it was indexed with; a reader **refuses a tokenizer it does not recognise** rather than guessing. | Settled |
| 2 | **Shell privilege boundary** | A **loopback WebSocket**, a **per-boot token**, a **strict origin check**, and a **closed list** of allowed privileged operations. | **Direction only — needs its own design session before C1 is built.** |
| 3 | **Per-service resource floors** | Settled by the app contract (§15): **each app declares its own measured floor**, so the consolidated budget is the sum of declared floors rather than a number waiting on every track at once. | Settled |
| 4 | **Service sessions vs profile switching** | The kiosk browser keeps a **separate storage partition per `profile_slot`**, so switching profile switches cookies. Phones are single-user and unaffected. | Settled |
| 5 | **Passage unit for search & citation** | Chunks are cut **deterministically at build time along heading structure**. A chunk id is **the pack path plus the chunk's position**. Chunks live **in the pack**, so the format owns them. | Settled |
| 6 | **Cross-source ranking** | **Reciprocal Rank Fusion.** | Settled |
| 7 | **Rollback trigger** | A boot counts as healthy when **the required services report healthy within a set window**; repeated failures roll back. | Settled in shape. **The window and the failure count are numbers the implementer measures on real hardware** — they are not guessed here. |
| 8 | **Design-system accessibility values** | **Closed.** The locked, AA-verified tokens in `design-language.md` §4 — palette, type scale, spacing, radius, focus indicator and touch-target minimum. | Settled |

### 13.1 The constraint decision 1 carries

The query side must run **the same tokenizer** the pack was indexed with, on every box, at the minimum spec. That bounds the tokenizer choices — CJK segmentation in particular, where the usable approaches differ sharply in dictionary size and memory cost. **Track A owns choosing within that bound**; this document only records that the bound exists, because a tokenizer chosen for build-side quality alone can be one the minimum spec cannot run.

Decision 1 unblocks every pack already built, all of which are browse-only today.

### 13.2 Still genuinely open

Not answered, and not to be filled in by whoever implements first:

- ~~Whether `community_hub` and `field_ops` require `x86_64`~~ — **answered: no.** The floor is the resources, not the architecture. A 16 GB ARM64 box meeting the RAM and storage numbers **qualifies for both profiles**. The old `mini_pc` label bundled 16 GB with x86 in one name, and only the numbers were ever the requirement. **x86 remains the preferred spec (§2.1), never a gate** — treating it as one would reintroduce exactly the device-shaped gate §2 removes.
- **The exact icon set** implementing the design language's icon rule. `design-language.md` specifies the visual rule, not the asset source, and nothing in the locked token set depends on the answer. **Owned by `design-language.md`.**
- **Per-service measured floors for the ten existing Track H services.** The *mechanism* is settled (decision 3) — each service declares its own. The *numbers* still have to be measured, service by service, by Track H.

## 14. Amendment Rule

A value in this document changes here, in this document, and nowhere else. A track document that needs a different value raises it against this one rather than restating it locally — a local restatement is how three documents came to hold three slightly different copies of the same vocabulary, which is the cause this document exists to remove.

Adding a value is the same operation as changing one: if a second track begins consuming something a track document currently owns privately, it moves here at that moment, not later.

*A document like this goes stale the moment implementation outruns it. The practical guard is the same one the sweep used: any prompt written against a track document should cite this document alongside it, and anything an implementer has to invent that belongs in one of these tables is a defect report against this document, not a decision for the implementer to make quietly.*

## 15. The App Contract — One Manifest Every Platform Service Reads

**This is the most important addition in this document.** Every app and service — the ten already in Track H, and every addition the catalogue gains — is wired into the platform through one declared contract, not by hand.

An **app manifest** is distinct from the **pack manifest** of §11: a pack is content, an app is a running service. They share §2.3's resource vocabulary and nothing else.

### 15.1 What an app manifest declares

| **Group** | **Declares** |
|---|---|
| Resources | `min_ram_bytes`, `min_storage_bytes`, `arch`, optional `gpu` — the §2.3 fields, unchanged. This is the app's **own measured floor**. |
| Reachability | Its address under the service zone (§8), and **the permissions required to reach it** — never a role list. |
| Authority | **The permissions it defines.** An app that introduces a new authority — a moderator capability, a publish capability — contributes it to the catalogue (§4.1) here, and an admin can then put it in any role bundle. |
| Identity | Its sign-in method, so the identity bridge provisions it rather than each service inventing a login. |
| Liveness | Its health check — the probe whose result drives §7.1's health vocabulary and the self-healing model. |
| Durability | **What of its data gets backed up**, so F10 does not need per-service knowledge. |
| Participation | Whether it contributes to **unified search**, **notifications** and **progress tracking** — each independently, each opt-in. |

### 15.2 Why it is worth this much

Five platform components read this one manifest and nothing else: **the launcher**, **the reverse proxy**, **backup/restore**, **health monitoring**, and **the app store**.

- **[New decision] Adding an app or a tool is writing one manifest and changing no core code.** That property is what makes a large catalogue affordable. Without it, every addition is five edits in five components, and the catalogue's cost grows with its size — which is precisely how a feature list becomes unbuildable.
- **It closes the resource-budget circularity at its cause** (§6): the consolidated budget is the sum of declared floors over the installed set, computed on the box, not a figure blocked on every track reporting at once.
- **Participation is opt-in per capability.** An app that contributes to search but not to progress tracking says exactly that. A service that appears in no launcher still declares its health check, so nothing runs unwatched.
- **[New decision] An app declares permissions, never roles.** It states the permissions it *requires* to be reached and the permissions it *defines*. The reverse proxy and forward-auth enforce against the identity's **effective permissions** (§4.3). This is what lets a new app introduce its own authority without an edit to any central role list — the same property, applied to authorization, that §15.2 gives the rest of the platform.

*The manifest's wire format, field names beyond the §2.3 group, and its schema version are the implementing track's to settle. This document owns what must be declared and who reads it; it does not invent the serialization.*

## 16. Cross-Cutting Properties — Stated Once, Here

Each of these holds across every track. They are stated here so no track document restates them differently.

- **No telemetry. Ever.** DeltOS reports nothing outward — no usage, no diagnostics, no crash reports, no phone-home of any kind. **The sole exception is F14 usage insights**, which is local to the box and aggregate-only: which packs get used, never which learner used them. Nothing leaves the box. A track needing an exception raises it here rather than adding one locally.
- **The interface itself is translatable, not only the content.** The WebOS shell — admin console included — is localizable to the same standard as the content packs. A string baked into a component is a defect against this property.
- **Backups cover everything people create.** F10 backup/restore covers hosted sites, security-lab work and code-studio projects alongside the existing content and progress data. Any new surface where a person creates something durable declares it under §15's durability group, which is how F10 learns about it without per-service knowledge.
- **One licence check, for everything DeltOS distributes.** Games, depot software and security tools pass through the **same build-time licence validator** that packs already use (§11's licensing rules). There is not a second validator with second rules.
- **LAN transport: plain HTTP for visitors, HTTPS only where the CA can be installed.** Visitors' own phones reach the box over plain HTTP, with every pack and every hosted site isolated **by hostname** (§8). HTTPS applies only to managed devices, where the box's certificate authority can be installed.
  - **[New decision] The cost is recorded, not glossed.** Browser features that require a **secure context** are unavailable to plain-HTTP visitors. That excludes service workers and therefore offline caching of the shell, the Web Crypto API's `subtle` interface, geolocation, camera and microphone capture, and the clipboard API, among others. Any feature specified against an unmanaged visitor's phone must work without them — a track discovering it needs one has found a design problem, not a deployment problem, and raises it here.
