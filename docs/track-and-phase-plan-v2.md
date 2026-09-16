# DeltOS Project

Track & Phase Plan — v2

*Working plan · September 2026 · supersedes v1, revised after a holistic cross-document review — see §7*

## 0. What Changed in v2

Two things happened since v1. First, the shell/webOS layer has a real name now: DeltOS, replacing the placeholder “Atrium” used in the Blueprint and Gap Report — the architecture and findings in those two documents are unchanged, just substitute the name. Second, the project scope grew substantially: a full list of services this system should offer (LMS, media serving, office suite, chat, VoIP, email, wikis, publishing, cloud storage, browser IDE, mesh/radio, IoT automation, identity/SSO, container orchestration, and more). That list is fully captured below — nothing was dropped — but it's now organized into two new tracks and a Deployment Profile concept, because not everything belongs on every box.

*Naming note: “DeltOS” comes from δέλτος (deltos), the ancient Greek word for a wax writing tablet — it pairs naturally with the WAX format itself (the wax the tablet holds). Good, coherent naming system; keeping it.*

## 1. The Eight Tracks

| **Track** | **Mission** | **Primary discipline** |
|---|---|---|
| A — Format & Storage Engine | Own the .wax container itself: the binary format, the reader/writer library, and the server that streams it. | Systems / Rust |
| B — Content & Interop | Get real content onto a box: ZIM import, crawling, mapping data, documentation sets, curated bundles. | Content pipeline / data ops |
| C — DeltOS Shell | The kiosk launcher, unified search bar, profiles, and the app experience a student or teacher touches directly. | Frontend / UX |
| D — Search & Intelligence | Full-text and semantic search, plus the tiered offline AI assistant and RAG engine. | ML / search |
| E — Hardware, OS & Networking | The physical/OS layer: base image, updates, Wi-Fi/captive portal, mesh, and the IoT/radio gateway. | Systems / infra |
| F — Admin, Identity & Fleet | The admin console, identity/SSO, password vault, user roles, fleet view, and governance. | Product / security / ops |
| G — Platform Services & Orchestration | The invisible plumbing every app in Track H sits on: self-healing orchestration, reverse proxy, git, registry, local DNS/NTP/package cache, automation engine. | Platform / DevOps |
| H — Applications & Community Services | The actual app catalog riding on Tracks C and G: LMS, media server, office suite, wiki, chat/VoIP, email, cloud storage, browser IDE. | Full-stack / apps |

*G and H are new. Everything the last message asked for lives in one of these eight tracks — see the Component Inventory (§3) for the full mapping.*

## 2. Deployment Profiles

Not every box should run everything — a Pi Zero running a private LTE core isn't a real plan. Profiles say what a given deployment actually turns on; they layer on top of the hardware tiers already defined in E6.

| **Profile** | **Hardware** | **Tracks / tiers included** | **Example use case** |
|---|---|---|---|
| Kiosk | Pi Zero 2W (E6's pi_zero_2w tier) | A–D core, plus Track F's baseline daemons (F2 first-boot security, F3/F11 identity, F4 install pipeline) — a single-reader box still needs a first-boot admin account even if never touched again (Track F §14). No G or H. | A single reference-library reader. |
| Classroom | Pi 4/5 | Full C; G1/G2 (baseline orchestration and ingress — Track G §9 states these apply from Classroom profile upward, since Track H's services can't run without them); Standard-tier H (LMS, media, wiki, cloud storage); E3 captive portal. | A one-room school or after-school program. |
| Community Hub | Mini-PC / NUC | Full G (orchestration, git, registry, dashboards); most of Track H (office suite, chat, email, VoIP). | A library or community center running many services. |
| Field Ops / Advanced | Community Hub + specialized radio/sensor hardware | Adds E12–E15 (IoT/SDR/RF, smart-grid automation, the last driven by G6's flow engine) — opt-in only. | Disaster response, off-grid research, rural clinics. |

A foundational amendment made after the eight tracks were drafted (Track E §21): DeltOS now supports two co-equal deployment paths onto hardware, not a single OS choice. The appliance-image path (rpm-ostree-based, Fedora IoT or similar) is recommended for hardware dedicated to DeltOS outright and gives the strongest update/rollback guarantee; an installable-stack path lets DeltOS's containerized stack lay on top of hardware or systems someone already has and is running a general-purpose Debian/Ubuntu-family host, closer to how Internet-in-a-Box is actually deployed today. Both are available on every profile above; a profile says what runs, not which path put it there. Alongside this, the earlier idea of a single fixed "optimum hardware spec" was replaced with a dynamic, checkbox-style hardware requirement calculator (Track B §17, Track C §7/§19) — an admin selects which services and packs they intend to run, and the tool computes the RAM/storage a box actually needs for that selection, rather than DeltOS publishing one number that's wrong for most deployments.

## 3. Component Inventory by Track

Full list, grouped by track. Items carried over from v1 are unchanged; new items from this round are tagged with a target phase and, where it matters, a tier.

### Track A — Format & Storage Engine  (unchanged from v1)

- **A1 — wax-core:** Rust reader/writer library for the .wax container itself.
- **A2 — wax-builder:** CLI that assembles a directory tree into a signed, compressed .wax archive.
- **A3 — SPEC.md:** The formal, versioned byte-layout specification.
- **A4 — Conformance suite & fuzzers:** Reference test corpus plus cargo-fuzz targets for the reader.
- **A5 — wax-serve:** HTTP server exposing byte-range/streaming reads.
- **A6 — wax-delta:** Append/patch engine for in-place updates (WAX v2).
- **A7 — Pack signing:** minisign now, TUF later.
- **A8 — Multi-volume archives:** Splitting a library across volumes.
- **A9 — Embedded search-index section:** Format-level section Track D writes into and reads from.
- **A10 — Reference reader (2nd language):** Proves the spec is portable, not just the code.

### Track B — Content & Interop

- **B1 — zim2wax:** Converts existing ZIM archives into WAX packs.
- **B2 — Crawler pipeline:** Site-to-pack tooling for original content.
- **B3 — Pack manifest schema:** Name, icon, category, license, version, minimum hardware tier.
- **B4 — On-device catalog:** Local pack/metadata index on each box, synced from B13's catalog-index. Renamed from "wax-hub" during Track B design once that name was split into B4 (this, on-device) and B13 (the publishing service) — see B13.
- **B5 — Starter packs:** Curated grade/subject-tagged bundles for first run.
- **B6 — Licensing/attribution validator:** Checks manifest licensing fields at build time.
- **B7 — WAX Studio:** Curation/authoring tool, separate from the runtime (later phase).
- **B8 — Incremental re-crawl tooling:** Updates a pack from its source without a full re-crawl.
- **B9 — Community submission & review pipeline:** Intake and moderation for contributed packs.
- **B10 — Offline mapping pipeline:** OSM extracts + OSRM routing graphs + offline tiles, packaged as a standard WAX pack — this is where the requested mapping capability lives. Phase corrected to P3 during Track B design/review — it depends on Track A's A8 multi-volume support, which itself lands P3.  *[P3]*
- **B11 — Offline geocoding:** Forward geocoding (address text to coordinate) via Track D's full-text search stack over OSM place names, with SQLite R-Tree used only for reverse geocoding/disambiguation — not self-hosted Nominatim, dropped during Track B design to avoid a PostgreSQL+PostGIS dependency. Design finalized during Track B's independent review, after an earlier revision incorrectly proposed R-Tree alone for the forward-geocoding case.  *[P3]*
- **B12 — DevDocs offline pack:** Mirrors devdocs.io documentation sets as a standard WAX content pack rather than a separate running service.  *[P2]*
- **B13 — Catalog publishing service:** The central infrastructure B9's pipeline feeds and where the trusted project-key signature is applied — distinct from B4, which is the on-device catalog format/sync client that mirrors from this service via a catalog-index WAX pack (Track B §5). Added during Track B design to stop "wax-hub" naming both a format and a service.  *[New · Track B]*

### Track C — DeltOS Shell

- **C1 — DeltOS Shell:** The full-screen kiosk launcher, split into two processes rather than one privileged PWA (settled during Track C design as the one decision that has to be made correctly in P0): deltos-shell, an unprivileged web app over Chromium/WebKit, and deltos-shelld, a small native helper holding the actual OS-level privileges behind a narrow IPC surface (Track C §2).
- **C2 — Pack/app registry:** Reads exclusively from B4's on-device catalog (not directly from manifests) to populate the launcher grid; multi-sourced beyond WAX packs alone, since C11, Track G, and Track H services also register entries here (Track C §3).
- **C3 — Unified search bar:** Queries every installed pack via Track D's API.
- **C4 — Per-pack sandbox:** iframe+CSP now, WASM (Wasmtime) isolation later.
- **C5 — Multi-profile / classroom mode:** Separate students/classes on one shared device.
- **C6 — Offline app store UI:** Browsing B4's on-device catalog fully offline.
- **C7 — USB / peer install flow:** Installing from a USB drive or peer box like installing from the internet.
- **C8 — Coach/progress dashboard:** Per-learner progress, live in P3.
- **C9 — Accessibility layer:** TTS, translation, high-contrast/large-text.
- **C10 — First-boot onboarding wizard:** Guided setup, no raw config files exposed.
- **C11 — Ask DeltOS:** The ChatGPT-style conversational interface, built into the shell as a first-class app rather than a bolt-on, calling Track D's D5/D6 RAG engine underneath.  *[P4 · corrected from P5 to match Track D's own settled timing for D6's full RAG experience (Track C §14/§17, Track D §7/§14)]*

### Track D — Search & Intelligence  (unchanged from v1)

- **D1 — Full-text search service:** An embedded SQLite FTS5 table per pack at v0.9–v1.x (matching Track A's own sequencing for A9), moving to Tantivy at v2, replacing Xapian; a tier-adaptive aggregation service, deltos-searchd, gives C3 the single query endpoint it needs across however many packs and index generations are actually installed (Track D §2).
- **D2 — Vector/semantic index:** sqlite-vec or LanceDB alongside the text index.
- **D3 — Multilingual tokenization:** CJK, Indic, and Swahili-family coverage at launch.
- **D4 — Build-time embedding pipeline:** Content embedded once at pack-build time.
- **D5 — Local LLM runtime:** llama.cpp/GGUF, tiered by hardware.
- **D6 — RAG orchestration:** Retrieves passages, answers with citations — surfaced to users via C11.
- **D7 — Hardware-tier feature gating:** One codebase, graceful degradation.
- **D8 — Federated search:** Optional cross-box query fan-out (later phase).

### Track E — Hardware, OS & Networking

- **E1 — Base OS image:** Minimal, read-only-root (balenaOS / Fedora IoT / Yocto — decision in P0).
- **E2 — A/B atomic updates:** Automatic rollback to last-known-good.
- **E3 — Wi-Fi AP & captive portal:** Rebuilt to fix documented Android/macOS/AP+STA friction.
- **E4 — Network diagnostics:** Plain-language connectivity troubleshooting.
- **E5 — Storage/partition layout:** Read-only OS partition + writable data partition.
- **E6 — Hardware-tier profiles:** Pi Zero 2W / Pi 4 / Pi 5 / mini-PC capability tiers.
- **E7 — Provisioning/flashing tool:** Guided imaging tool for admins building new boxes.
- **E8 — Mesh networking module:** Yggdrasil or Reticulum for multi-box, off-Wi-Fi topologies (later phase).
- **E9 — Power/solar dashboard:** Charge state and panel-positioning guidance for the solar hardware SKU.
- **E10 — Local DNS & service discovery:** dnsmasq/CoreDNS plus mDNS/Avahi so services resolve as friendly LAN hostnames.  *[P1]*
- **E11 — Local NTP:** chrony, optionally GPS-disciplined for sites with no internet access ever.  *[P1]*
- **E12 — IoT gateway bridge:** Bluetooth/NFC/Z-Wave device bridging (e.g. Zwave JS UI) onto a shared MQTT bus (Mosquitto).  *[P4 · Community Hub / Field Ops]*
- **E13 — Community sensing (SDR):** Receive-only AIS/ADS-B/weather-satellite decoding via RTL-SDR (dump1090, AIS-catcher). Passive reception is legal without a license almost everywhere; this is the only RF item safe to treat as a normal opt-in module.  *[P5 · Field Ops, Advanced]*
- **E14 — Advanced RF: private cellular core:** Open5GS/srsRAN for a private LTE/5G testbed. Flagged as regulatory-sensitive: transmitting requires licensed or unlicensed-band spectrum (e.g. CBRS) in most countries — this is not a default-on feature. Note: “6G” has no deployable standard yet, so scope this to 4G/5G only.  *[P5+ · Field Ops, Advanced, opt-in]*
- **E15 — Smart-grid/power automation hooks:** Modbus/MQTT integration for solar, battery, and grid telemetry, feeding E9's dashboard and driven by G6's flow engine.  *[P4 · Community Hub / Field Ops]*

### Track F — Admin, Identity & Fleet

- **F1 — Admin console:** One unified, wizard-driven web UI.
- **F2 — First-boot setup:** Forces a unique generated admin password.
- **F3 — User/role management:** Admin, teacher, student roles.
- **F4 — Content install/update pipeline:** Resumable, queued, atomic.
- **F5 — Device health monitoring:** Storage, uptime, temperature, connectivity.
- **F6 — Fleet dashboard:** Multi-box view for organizations.
- **F7 — Signing & key management:** Operationalizes A7's keys, then TUF.
- **F8 — Governance:** SPEC.md hosting, CONTRIBUTING docs, public roadmap.
- **F9 — Documentation site:** Admin and deployment guides.
- **F10 — Backup/restore tooling:** Content and per-learner progress data.
- **F11 — Identity & progress data model:** The shared record of who a learner is and what they've done — needed before C8 can be built.
- **F12 — SSO / identity provider:** Authelia in front of G2's reverse proxy — one login across every self-hosted service in Track H.  *[P4]*
- **F13 — Password vault:** Vaultwarden (Bitwarden-compatible, Rust) for admins managing shared credentials — narrowed to admin-only during Track F design/review; extending access to teachers is an open decision, not settled scope, and would need to wait for F12/SSO (Track F §11).  *[P4]*

### Track G — Platform Services & Orchestration  (new)

The plumbing every app in Track H depends on. This is what makes “forty separate services” feel like one coherent box instead of forty admin panels — and what makes a crashed service repair itself instead of paging a human.

- **G1 — Service orchestrator:** Docker Compose (with healthchecks) as the single authoring format across every tier — a single-node K3s deployment, named as an option in earlier drafts, was deliberately dropped in favor of one format for the common case (Track G §2); clustered K3s remains reserved for genuine multi-box clustering only, scaffolded from the same Compose files via Kompose rather than fully hand-translated, and without real high availability for stateful services. A crashed service is detected and restarted automatically — no manual intervention.  *[P2 · corrected from P3 per Track G's own review — H2 (media server) is tagged P2 and has nothing to run under otherwise]*
- **G2 — Reverse proxy / ingress:** Caddy or Traefik fronting every service under one hostname with automatic internal TLS — foundational; most of Track H sits behind this.  *[P2]*
- **G3 — Local git service:** Gitea (or its Forgejo fork), SQLite-backed, fed via the browser.  *[P4]*
- **G4 — Local container registry:** Lightweight OCI registry for fleet-wide image distribution with no internet.  *[P4]*
- **G5 — Apt-cacher:** apt-cacher-ng caching package proxy — a fleet of boxes stops re-downloading the same OS packages.  *[P4]*
- **G6 — Visual flow-automation engine:** Node-RED (or equivalent) for wiring hardware, APIs, and services together, including physical alerts — the same engine driving E15's smart-grid automation.  *[P4]*
- **G7 — Metrics & dashboard stack:** Prometheus/VictoriaMetrics + Grafana, graphing community and device metrics (power consumption, sensor data) over time.  *[P4]*

### Track H — Applications & Community Services  (new)

The app catalog itself — almost everything from the expanded feature list lands here, installed and launched the same way as any WAX content pack through the DeltOS Shell (Track C).

- **H1 — Learning Management System:** Kolibri as the primary LMS — reuse, don't rebuild; reaches C8's coach dashboard through a dedicated sync bridge (deltos-kolibri-sync) rather than a direct tie, since C8 otherwise reads progress exclusively through WAX content packs' own postMessage bridge, a mechanism Kolibri as an external service doesn't use (Track H §3, Track F §20).  *[P3]*
- **H2 — Media server:** A Jellyfin-based library server for attached-storage video/music, browsed through DeltOS with a Netflix/Apple-Music-style UI.  *[P2]*
- **H3 — Browser office suite:** Collabora Online or OnlyOffice Docs, integrated with H5.  *[P4]*
- **H4 — Browser IDE:** code-server (VS Code in the browser).  *[P4]*
- **H5 — Cloud storage & content sharing:** Nextcloud.  *[P3]*
- **H6 — Wiki:** Wiki.js or DokuWiki, sized to hardware tier.  *[P3]*
- **H7 — Publishing / CMS:** A lightweight flat-file CMS (e.g. Grav) on constrained hardware; WordPress available on Community Hub tier.  *[P4]*
- **H8 — Secure chat & collaboration:** A Matrix homeserver — Conduit (lightweight, Rust) preferred over Synapse for this hardware range.  *[P3]*
- **H9 — Email server & webmail:** A lightweight mail stack (e.g. Maddy) plus Roundcube webmail, scoped to the local network.  *[P4]*
- **H10 — VoIP / voice & video:** Built on H8's Matrix stack (Element Call + coturn) rather than standing up a separate Asterisk deployment.  *[P4]*

### Flagged During Track Refinement, Not Yet Owned by Any Track

- **— — Download-time website configurator:** Referenced as an existing, expected feature by Track E §17, Track B §17, and Track C §7/§19 (a configurator a downloader interacts with before or during getting DeltOS, presumably feeding the hardware-requirement calculator's selections into the actual download/build), but it doesn't fit cleanly inside any of the eight tracks as scoped so far and no track has claimed it. Listed here so it isn't lost between tracks; needs an owning track decided before implementation prompts are drawn up for whichever track ends up building it.

## 4. A Service-Selection Filter

With this many services, consistency matters more than any single choice. The filter used above and going forward: prefer single-binary, Go- or Rust-native, SQLite-or-flat-file-backed tools over heavier Python/Node/JVM stacks, wherever a credible option exists. It's why Conduit beat Synapse, Vaultwarden beat a Java Bitwarden server, and Gitea beat GitLab — each is a fraction of the memory footprint, cross-compiles to ARM cleanly, and fits the same operational model as WAX itself. Not every service on the list has a lightweight option (Nextcloud and Kolibri are both real, heavier stacks) — those get admitted deliberately, not by default.

## 5. Cross-Track Dependencies & Open Questions  (updated)

- G2 (reverse proxy) is now the most load-bearing single component in the plan — nearly every Track H service and F12 (SSO) sits behind it. It should be running reliably before more than one or two Track H apps are added.
- G1 (orchestrator) should land before Track H proliferates — adding a fifth self-hosted service with no self-healing supervisor is how a box becomes unmanageable in the field.
- F12 (SSO) ideally exists before Track H's service count grows past a handful — retrofitting single sign-on across ten already-deployed apps is much more painful than designing it in from the third or fourth.
- E14 (private cellular / advanced RF) is regulatory-sensitive and hardware-specific — keep it opt-in and never bundle it into the default build, unlike everything else in Track E.
- D's ranked/semantic search depends on A shipping the embedded search-index section (A9) — specifically its P2 SQLite FTS5 form, not the v2 Tantivy stand-off form, which A9 doesn't ship until P5 (corrected after Track A's review pass; see the Track A Refinement doc, §10/§12).
- Pack-signature verification and rollback-mitigation enforcement (A7) now start at P2, not P4 — an earlier draft tagged them a P4 "hardening" item, but Track F's review caught that B13, C6/C7, and F4 all assume this protection is live from P2, since it's exactly when unsigned or stale packs first become installable (Track A §14, Track F §16).
- Resolved, no longer open: F11 (identity & progress data model) — settled in Track F's own initial draft (deltos-identityd, the profiles/progress_events schema, Track F §3) ahead of, not merely on, its P2 target; Track C confirms C8's P3 build against it is now unconditional rather than provisional (Track C §20).
- B1 (zim2wax) remains the critical-path unlock for every other track's testing.

## 6. Immediate Next Steps  (updated)

Write SPEC.md (A3) and stand up the conformance/fuzz suite (A4).

Build zim2wax v0 (B1) — even lossy — to unlock real content for every other track's testing.

Prototype wax-serve (A5) against one real video file to prove byte-range streaming under load.

Publish governance, license posture, and the public roadmap (F8).

Stand up one physical Pi 4/5 + one x86 test rig running today's prototype end to end.

New: settle the orchestrator (G1) and reverse-proxy (G2) choice early — nearly the entire Community Hub profile is built on top of these two decisions.

Once these are done, individual components are ready to be turned into scoped prompts for Claude Code. (This line previously named the set as "A1–H10"; the inventory is enumerated in the track documents and is not totalled here — see §8.1.)

## 7. Holistic Cross-Document Review — Findings and Resolutions

With all eight tracks now individually drafted and independently reviewed, this pass looked at all nine documents (this plan plus the eight track refinements) together for the first time — specifically for drift between this plan's original component inventory/narrative and what each track actually settled on, and for scope named in a track but never reflected back up here. The eight tracks' own cross-amendment discipline held up well at that layer — no unresolved contradiction was found between two track documents that neither side's own review had already caught. Every finding below is this plan lagging a track, or a genuinely unowned item never surfacing here.

- **[Critical]** C1 was described as a single "privileged PWA," the exact framing Track C's own design rejects — the shell is split into an unprivileged deltos-shell and a separate privileged deltos-shelld precisely so the browser-hosted app is never the thing holding OS-level privileges. Resolved: C1's description corrected to name the two-process split (§3).
- **[Critical]** The Classroom profile row listed no Track G, despite Track G stating G1/G2 apply from Classroom profile upward and Track H stating every one of its services is Track G-orchestrated — Classroom's own listed H-services (LMS, media, wiki, cloud storage) cannot run without them. Resolved: G1/G2 added to the Classroom row (§2).
- **[Critical]** The Kiosk profile row read "A–D, core only. No G or H... no admin needed," despite Track F stating its first-boot/identity/install daemons apply down to Kiosk profile, and Track C role-gating its own baseline shell operations against exactly those daemons even at Kiosk. Resolved: Kiosk's row corrected to include Track F's baseline daemons; "no admin needed" removed (§2).
- **[Critical]** This plan's Deployment Profiles section still described a single hardware-tier model with no fixed "optimum spec" concept ever revisited, while Track E's post-review product-direction amendment (§21) settled two co-equal deployment paths (appliance-image and installable-stack) and replaced any fixed optimum-spec idea with a dynamic hardware-requirement calculator — both foundational, cross-cutting changes with no trace anywhere in this plan. Resolved: both added as a new paragraph following §2's table.
- **[Security]** F13 was described as serving "admins and teachers," directly contradicting Track F's own settled admin-only scope (narrowed during Track F's design after catching this exact contradiction on its own side, but never patched at the source). Resolved: F13's description corrected to admin-only, with teacher access noted as a genuinely open decision rather than settled scope (§3).
- **[Moderate]** C11 (Ask DeltOS) was tagged P5 here, but Track C's own settled table — confirmed independently by Track D — corrects this to P4, matching when D6's full RAG experience actually lands. Resolved: C11's tag corrected to P4 (§3).
- **[Moderate]** D1 described only the v2 end state ("Tantivy-based, replacing Xapian"), omitting the v0.9–v1.x embedded-SQLite-FTS5 stage Track A's own sequencing requires, and the new deltos-searchd aggregation service Track D introduced to give C3 one query endpoint across index generations. Resolved: D1's description updated to name both (§3).
- **[Moderate]** H1 claimed Kolibri "ties directly into C8's coach dashboard," but C8 reads progress exclusively through WAX packs' own postMessage bridge — a mechanism an external service like Kolibri doesn't use, which is exactly why Track H had to design a dedicated sync bridge and a new progress_events column. Resolved: H1's description corrected to name the sync-bridge mechanism (§3).
- **[Moderate]** C2 described reading manifests (B3) directly to populate the launcher grid, but Track C settled on C2 reading exclusively from B4's on-device catalog cache, and being multi-sourced beyond WAX packs once C11/Track G/Track H services register too. Resolved: C2's description updated to match (§3).
- **[Moderate]** The download-time website configurator is referenced as an existing, expected feature by three different track documents (Track E §17, Track B §17, Track C §7/§19) but was never named in this plan's inventory and has no owning track. Resolved: added as a flagged, explicitly unowned item so it isn't lost between tracks (§3).
- **[Moderate]** G1's description still named single-node K3s as an available option on mini-PC/NUC hardware, though Track G's own review concluded this option was deliberately foreclosed, not merely clarified, in favor of Docker Compose as the one authoring format everywhere, with K3s reserved for genuine multi-box clustering. Resolved: G1's description corrected to state the foreclosure explicitly (§3).
- **[Moderate]** §5 carried the A9 FTS5-vs-Tantivy phase correction from Track A's review but not the structurally identical A7 correction from the same document — pack-signing and rollback enforcement moving from a P4 "hardening" tag to P2, once Track F's review showed B13/C6/C7/F4 all need it live from P2. Resolved: a matching §5 bullet added.
- **[Moderate]** §5 and §6 both still listed F11 (identity & progress data model) as needing an owner decision, though Track F settled its full design in its own initial draft, ahead of the P2 target, and Track C independently confirmed C8's P3 build against it is unconditional. Resolved: §5's bullet reworded to state the resolution and its citation; §6's now-redundant next step removed.
- **[Minor]** The Field Ops/Advanced profile row cited "E12–E14," excluding E15 (smart-grid automation) despite describing E15's own mechanism ("G6-driven smart-grid automation") in the same cell. Resolved: range corrected to E12–E15 (§2).
- **[Minor]** The Kiosk profile row named "old netbook" as example hardware, but Track E's authoritative E6 tier table defines no tier covering a resource-constrained x86 netbook — only pi_zero_2w, pi_4, pi_5, and mini_pc. Resolved: the Kiosk hardware example narrowed to Pi Zero 2W, naming the actual E6 tier it maps to (§2).

## 8. Amendment (per product direction — the catalogue, and how it is counted)

### 8.1 The inventory is enumerated, never totalled

This plan's inventory (§3) previously implied a fixed set, and `INDEX.md` described it as "A1–H10". **Neither is maintained as a count any more.** The catalogue is stated by enumeration in the track documents, each component owned by exactly one of them, and a fixed total in a second place is a number that goes stale the first time the catalogue grows — which it now has.

**Where the additions live.** Each is specified — goal, property, why — in its owning track document, not here:

| Component | Owner |
|---|---|
| B14 Migration; video packs; community archive | `track-b-refinement.md` §21 |
| C7 widened to a USB shelf | `track-c-refinement.md` §24 |
| AI acceleration (`gpu: preferred`) | `track-d-refinement.md` §17 |
| E7 configurator and install parity; E10 name registry; E8 radio Q&A; the OS-supervisor half of self-healing | `track-e-refinement.md` §23 |
| F14 usage insights; F15 network defense; F1 visible benchmark; the remote bridge; F10's widened backup set | `track-f-refinement.md` §23 |
| G8 print; G9 software depot; self-healing in full | `track-g-refinement.md` §14 |
| H11–H19 (including **H13 Community broadcast**, §19.10); H2 plug-in media | `track-h-refinement.md` §19 |
| Search-index ownership; the passage unit; A5 hostname routing | `track-a-refinement.md` §20 |
| The role and permission model | `cross-track-contract.md` §4, implemented by `track-f-refinement.md` §24 |

**H13 is assigned: Community broadcast** — a local radio and podcast station for Community Hub and Field Ops (`track-h-refinement.md` §19.10). It was left visibly vacant in the first pass rather than filled speculatively or closed by renumbering; leaving the gap visible is what made it easy to fill correctly rather than forgotten.

### 8.2 The download-time configurator now has an owner

§7 of this plan recorded the configurator as "referenced by three track documents and owned by none" — added as a flagged, explicitly unowned item so it would not be lost between tracks.

**It is owned. E7 has it** (`track-e-refinement.md` §23.1), alongside install parity. The flagged-unowned entry is resolved, and this is the precedent working as intended: an item with no home was made visible rather than quietly assumed, and it stayed visible until someone gave it one.

### 8.3 What the additions rest on

Every component above is reachable only because of one prior decision, recorded in cross-track contract §15: **the app contract.** Each app or service declares its resources, address and roles, sign-in, health check, backup set and participation once, in one manifest, and the launcher, reverse proxy, backup, health monitoring and store all read it.

Without it, each addition in §8.1 would mean edits in five platform components, and the cost of the catalogue would grow with its size. That is the difference between a catalogue this plan can sequence and a feature list it cannot.
