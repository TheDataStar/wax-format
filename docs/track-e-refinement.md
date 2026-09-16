# DeltOS — Track E

Hardware, OS & Networking — Technical Refinement

*Working draft · September 2026 · fifth of eight per-track refinements, revised after independent technical review (§20), a product-direction amendment on deployment model and hardware philosophy (§21), and a cross-track amendment from Track F's review (§22)*

## 1. Scope

Track E owns the box itself: the OS image, how it updates without bricking itself, the partition layout everything else's persistent state lives on, the OS-side share of the consolidated resource budget (§5.1) — the hardware vocabulary itself now belongs to the cross-track contract §2, Wi-Fi/captive-portal behavior, and the radio/IoT/power hardware that only the Field Ops profile turns on. Three tracks have been citing E6's tier names (pi_zero_2w/pi_4/pi_5/mini_pc) and E10/E11's services without this track ever having published what those actually are — settling that is this document's first job, not an afterthought.

## 2. Base OS Image (E1)

The master plan left this as "decision in P0" against three candidates (balenaOS, Fedora IoT, Yocto). Leaving it open past this document invites Track G to build its own orchestration assumptions against whichever guess it makes.

- **balenaOS is ruled out — rationale corrected after review:** The first draft framed this as a container-compatibility collision with Track G's G1; that overstated it — balenaOS's engine is Docker-compatible, and arbitrary Compose workloads (even K3s) can run on it without difficulty. The real conflict is narrower but still decisive: balenaOS ships its own fleet-update/supervisor model, which would compete with DeltOS's self-hosted ostree update source (§3) for ownership of "how does this box update itself" — two systems with the same job. Fedora IoT/CoreOS-style rpm-ostree hosts are, separately, specifically designed to pair with exactly the kind of container orchestration G1 describes, which is the more direct positive case for that choice, rather than balenaOS's incapability.
- **Recommendation: Fedora IoT (rpm-ostree) as the appliance-image path, with a stated Kiosk-tier risk:** rpm-ostree's image-based, dual-deployment-slot model directly implements E2's A/B requirement natively rather than DeltOS having to build one, has mature aarch64/x86_64 support across every E6 tier, and is maintained upstream rather than requiring DeltOS to own custom board support packages the way a Yocto build would. The open risk: rpm-ostree's baseline footprint is a real concern against Kiosk-tier's tight budget (Pi Zero 2W, 512MB RAM) — a POC validating a minimal rpm-ostree image actually boots and leaves headroom on that specific board is required before this decision is treated as final for that one tier (§17, §5's consolidated budget note). Yocto remains the documented fallback specifically for Kiosk tier if that POC fails — stated honestly, after review, as a real ongoing board-support-package commitment for that one board, not a costless safety net just because it's scoped narrowly.
- **Two supported deployment models, not one — added per product direction (§21):** This document's OS work above is one of two supported ways to get DeltOS onto hardware, not the only one. The appliance-image path (this section) is a purpose-flashed rpm-ostree image for hardware being dedicated to DeltOS outright. Alongside it, an installable-stack path lets someone install DeltOS onto a general-purpose Linux system they already have (repurposed hardware, a spare machine, a VPS) — closer to how Internet-in-a-Box itself is actually obtained. Both paths are DeltOS: same shell, same catalog, same app store, same tool/service set as far as each path's host allows; only how the bits get onto the box differs. See §21 for the full rationale and §3/§8 for how the two paths diverge on updates and provisioning.
- **Installable-stack path: target and packaging:** The installable-stack path targets a common general-purpose Linux distribution the person already has running (Debian/Ubuntu-family, matching the base most repurposed hardware and most low-cost VPS images already run) and lays down DeltOS's containerized service stack (Track G's G1 orchestrator and workloads) on top of it via a single installer — a script or native package, not a from-source build the person has to assemble themselves. It does not attempt to replace or manage the host OS the way the appliance-image path's rpm-ostree does; it assumes the host already exists and is the person's to maintain.

## 3. A/B Atomic Updates (E2)

This is close to free once E1 is rpm-ostree, since ostree's whole model is atomic, versioned filesystem trees with native rollback — the work is wiring DeltOS's own update source and boundary into it correctly, not building A/B from scratch.

- **Update source:** An ostree repo, self-hosted rather than pointed at any third-party infrastructure — served the same way Track B's B13 serves the catalog-index (Track B §5), keeping "how does a box learn about and fetch an update" to one pattern across OS images and content packs, not two.
- **The rollback boundary, made explicit — precision added after review:** ostree's split is /usr as the read-only, versioned image, with /etc and /var both writable but not preserved identically: /var is directly shared across deployments, while /etc undergoes a three-way merge on every deployment switch that can silently drop a local edit if the upstream default also changed. DeltOS sidesteps /etc's merge semantics entirely by keeping all of its own state under /var only (§4) — this is this document's own E5 requirement, not a separate mechanism DeltOS has to build.
- **This guarantee has a precondition Track B and Track C need to confirm, not just this document (added after review):** "Installed packs, B4's catalog, and F11's progress never get rolled back" only holds if those actually resolve under /var/lib/deltos (§4) in Track B's and Track C's own implementations. Track B's B4 schema and Track C's C4/C5 storage design were both drafted and reviewed before this path convention existed, so this isn't yet a confirmed fact — it's a requirement flowing backward onto two already-settled documents, the same way Track B's schema needs flowed backward onto Track A. See the companion amendments to the Track B and Track C documents, and §4's browser-profile note below.
- **This section describes the appliance-image path only — added per product direction (§21):** ostree's atomic A/B rollback is native to the appliance-image path (§2) and this section's guarantees apply there in full. The installable-stack path has no ostree underneath it and cannot inherit this mechanism: its update story is instead container-image versioning at the Track G/G1 orchestration layer (pull a new tagged service image, keep the previous tag available to redeploy on failure) plus ordinary host-package updates the person's own system already handles outside DeltOS's control. This is a real, stated gap relative to the appliance path's guarantee, not an equivalent mechanism under a different name — flagged as an open decision (§17) for how much of it DeltOS should own versus leaving to the host.

## 4. Storage & Partition Layout (E5)

"Read-only OS partition + writable data partition" needs an actual mount-point convention, or every other track's persistent state (B4's catalog, installed packs, F11's progress data, Track G's service volumes) ends up scattered without a common backup boundary for F10.

- **One root for everything persistent:** /var/lib/deltos/ is the single directory every DeltOS-specific writable state lives under — B4's catalog database, installed .wax packs, F11's progress store, Track G's per-service data volumes all get their own subdirectory under this root, never a path outside it. Track F's F10 (backup/restore) then has exactly one tree to snapshot, not a hand-maintained list of paths that drifts as new components are added.
- **Wear-leveling note:** On microSD-based Kiosk/Classroom-tier hardware, frequent writes (B4 catalog updates, F11 progress events) concentrated in one place is a real wear concern over a multi-year deployment. Flagged as an open decision (§17) — whether this warrants a separate partition/volume from general /var, or just a wear-aware filesystem choice (e.g. F2FS) for /var/lib/deltos specifically — rather than assumed away.
- **The browser's own storage needs pinning here too (added after review):** Track C's C4/C5 isolation scheme depends on Chromium's per-origin localStorage/IndexedDB, which lives inside Chromium's own profile directory — a path Chromium otherwise defaults on its own, not necessarily under /var. deltos-shell's kiosk instance must be launched with its profile directory explicitly set under /var/lib/deltos/browser-profile/, or Track C's entire per-(pack, profile-slot) storage design (Track C §5) sits somewhere ostree's rollback guarantee (§3) doesn't actually cover.

## 5. Hardware Scoping (E6) — Superseded; the Contract Owns It

**This section no longer defines hardware. `docs/cross-track-contract.md` §2 does, and nothing here restates it.**

This document previously held a four-row tier table — `pi_zero_2w` / `pi_4` / `pi_5` / `mini_pc`, with RAM as ranges and Cortex CPU classes — described as "the authoritative source for what each tier's hardware actually is". Two things ended it:

1. **The ranges made the names useless as a gate.** `pi_4` at "2–8GB" meant a pack declaring that tier could land on either, so the name guaranteed nothing. The contract restated the figures as guaranteed floors, and this table was never updated to match — leaving two tables and one of them wrong, which is the exact failure the contract exists to remove.
2. **Device-name tiers are retired outright.** Per settled product direction, DeltOS is hardware-agnostic: capability follows **measured resources** — RAM, storage, CPU architecture and GPU presence — and **no document may gate a feature on a device model.** The Pi Zero 2 W is no longer a target at all.

What replaces it, all owned by the contract:

- **Minimum spec** (2 GB / 32 GB / `aarch64`, a Pi 4/5-class board as the example) and **preferred spec** (16 GB / 256 GB / `x86_64`, an x86 mini-PC as the example) — contract §2.1. Board names are examples of a resource class, never gates.
- **What a box measures at first boot**, and the capability record that publishes it — contract §2.2 and §9.
- **The `min_ram_bytes` / `min_storage_bytes` / `arch` / `gpu` declaration** a pack, feature or app makes — contract §2.3.
- **Profile floors**, now stated as resources — contract §3.

### 5.1 What Track E still owns here

- **The consolidated budget.** The concern this section raised — that the RAM figures were hardware specs, never summed against real concurrent load — was correct and is not resolved by retiring the table. It is now answerable rather than circular: contract §15's app contract makes **every app and service declare its own measured floor**, so the budget is the sum of declared floors over the installed set, computed on the box. Track E owns summing the **OS-side** contributors that no app manifest covers: the update stack's own footprint, the kiosk browser, `deltos-shelld`, and the Track F baseline daemons.
- **The requirement calculator's data source.** The calculator described below still works exactly as framed — it sums per-item declared resource costs and displays a live minimum for whatever the person selected. It now sums §2.3 declarations instead of reading tier floors out of a table here, which is strictly better: the figures come from the things themselves.

- **DeltOS still does not prescribe an optimum configuration.** Per product direction, it exposes a checkbox-style selector of tools and features — at download time (§8, E7) and later in Track C's C6 app store — that computes and displays a live minimum and preferred requirement for whatever has actually been selected, and leaves sourcing the hardware to the person. "Optimum hardware we can source and list" may still exist as informational site copy, but it is downstream of the calculator's output, never a substitute for it.
- **Division of labour with Track D's D7 is unchanged in shape:** the contract is now the authoritative source for what hardware *is*; Track D's D7 (Track D §8) remains the authoritative source for which features that hardware is judged capable of running.

## 6. Wi-Fi AP & Captive Portal (E3)

The master plan cites "documented Android/macOS/AP+STA friction" without saying what actually breaks or how this rebuild fixes it — worth being concrete, since these are well-known, specific failure modes, not vague flakiness.

- **Concurrent AP+STA requirement:** A box that's both an access point (serving the classroom) and, optionally, a Wi-Fi client (reaching an uplink for updates) needs a chipset/driver combination that genuinely supports concurrent AP+STA mode — not all do, and a board selected without checking this constraint silently loses one mode or the other. E7's provisioning tool (§8) should validate this against the detected hardware at flash time, not leave it to be discovered in the field.
- **Captive portal detection, per OS:** Each OS probes a different well-known URL to decide whether a network has a working captive portal (Android's /generate_204, Apple's /hotspot-detect.html, Microsoft's /connecttest.txt); a captive portal implementation that only answers one of these correctly is exactly what produces the "connected but shows no internet" complaints the master plan is reacting to. E3 needs to answer all three correctly, not just the one whichever developer tested against.
- **Interface to Track C's C10:** E3 exposes its setup operations (scan networks, join network, configure AP SSID/password) through the same category of narrow, versioned local IPC call Track C's C1 already established for deltos-shelld (Track C §2) — C10's onboarding wizard calls through that, never shelling out to raw wpa_supplicant/hostapd config files directly.
- **These operations need role-gating too — a gap review caught:** Riding on C1's IPC category isn't the same as riding on C1's role-check coverage: Track C's post-review role-gating fix explicitly lists install_pack, remove_pack, and mount_removable as checked against F3's role model (Track C §2), and never mentions Wi-Fi operations. Reconfiguring the classroom AP's SSID/password or joining an arbitrary uplink network is at least as sensitive as those three. E3's join_network and configure_ap operations need to be added to that same role-checked set, sourced from F3 the same way — not assumed to be covered for free by sharing a transport category.

## 7. Network Diagnostics (E4)

"Plain-language connectivity troubleshooting" is useful to two different consumers — Track F's F1 admin console and Track C's C10 onboarding wizard — and shouldn't be built twice.

- **One diagnostic service, two callers:** E4 exposes a small local API (link status, DHCP/IP state, DNS resolution check, AP+STA mode state) that both F1 and C10 call and render in their own UI idiom — a teacher's onboarding screen and an admin's troubleshooting panel show the same underlying facts differently, rather than each reimplementing ping/traceroute logic independently.

## 8. Provisioning & Flashing Tool (E7)

A guided imaging tool for admins building new boxes — concretely, this needs to pick the right image for the hardware in hand and reduce what C10's onboarding wizard has to ask interactively.

- **Tier-aware imaging:** E7 either detects the target board or asks the admin directly, then writes the correct E6-tier image (§5) — including, per §2, the Yocto fallback image specifically if Kiosk-tier's rpm-ostree POC doesn't pan out. It also validates the concurrent AP+STA requirement (§6) against known-good chipset/driver combinations before writing, catching a bad hardware choice at flash time instead of in the field.
- **First-boot pre-seeding:** Wi-Fi credentials and Deployment Profile selection can optionally be pre-seeded onto the image at flash time (for a fleet being provisioned in bulk by one admin) — C10's wizard (Track C §10) then skips straight past whatever was pre-seeded rather than asking again, without E7 and C10 needing two different config formats to agree on.
- **A second flow for the installable-stack path — added per product direction (§21):** E7 covers imaging dedicated hardware; the installable-stack path (§2) needs an equivalent guided experience that isn't flashing anything. That flow is the installer itself: it checks the host meets the tool/feature selection's computed minimum (§5's calculator output), lays down Track G's service stack, and then hands off into the same C10 onboarding wizard used by the appliance-image path for Wi-Fi/profile setup — one onboarding experience regardless of which path got DeltOS onto the box, only the delivery mechanism ahead of it differs.

## 9. Mesh Networking (E8)

Yggdrasil and Reticulum solve different problems; picking one exclusively would be forcing a single tool onto two use cases the master plan's own feature list actually distinguishes.

- **Yggdrasil — box-to-box over real links:** For a multi-building deployment (a school campus, several huts) where boxes reach each other over ordinary Wi-Fi or Ethernet but without a shared router — Yggdrasil's self-routing encrypted overlay gives each box a stable address without manual routing config. This is Track D's D8 federated search's realistic transport once boxes aren't on one LAN.
- **Reticulum — genuinely bandwidth-starved links:** Reserved for actual radio links (LoRa, packet radio) pairing with Field Ops-profile hardware — a fundamentally different bandwidth regime than Yggdrasil targets. Not a competing choice; a different layer for a different physical link, scoped to Field Ops/Advanced only.
- **Answering Track D's open question on peer discovery (added after review):** Track D's D8 flagged discovering peers as an item needing Track E's confirmation (Track D §9, §13). The answer has two parts, not one mechanism: on a single LAN segment (boxes in the same building), E10's Avahi/mDNS (§14) already provides automatic discovery — a box simply announces itself and others see it, no Yggdrasil-specific work needed. Across sites (different buildings, different networks), there is no automatic discovery — Yggdrasil doesn't provide a directory service, so an admin explicitly pairs boxes by exchanging Yggdrasil addresses through Track F's forthcoming F6 fleet console. Federated search (Track D §9) should assume the same-LAN case is automatic and the cross-site case is admin-configured, not automatic either way.
- **Mixed topologies are out of scope for v1, stated plainly (added after review):** A real Field Ops deployment can plausibly mix both — some boxes on Wi-Fi/Ethernet, others reachable only over LoRa. Bridging a Yggdrasil segment to a Reticulum segment automatically is not designed here; a deployment needing both would require a manually configured gateway node running both stacks, not a transparent bridge. A moderate-bandwidth, genuinely IP-capable radio link (point-to-point microwave, satellite backhaul) is simplest classified as a Yggdrasil link, not a Reticulum one — Reticulum is reserved specifically for links too constrained to run ordinary IP at all.

## 10. Power / Solar Dashboard (E9)

Building a second, separate dashboard here would duplicate Track G's G7 (metrics & dashboard stack) — E9's actual job is narrower than "dashboard."

- **Telemetry only, display deferred to G7:** E9 defines and exposes the charge-controller telemetry (battery %, charge current, panel voltage — via whatever protocol the reference solar SKU's controller speaks, e.g. Victron VE.Direct or a generic Modbus MPPT controller) as a small set of published values. Displaying that over time, alongside every other metric the box tracks, is G7's job — flagged forward for Track G's own refinement rather than built twice.
- **Panel-positioning guidance — restored after review, not dropped:** The master plan's own E9 line item names this explicitly, and the first draft of this section silently narrowed E9's scope to raw telemetry without saying so. It belongs with E9, not G7: a simple computed value (comparing charge-current trends across the day against an expected solar-angle curve to suggest "tilt further south/east," say) is a derived reading, the same category as the raw telemetry values above, not a dashboard — E9 computes and publishes it, G7 still just displays it.

## 11. IoT Gateway Bridge & Smart-Grid Hooks (E12, E15)

Both components are protocol bridges into MQTT — the automation logic that reacts to what's on that bus belongs to Track G's G6 (visual flow-automation engine), not to Track E.

- **Division of labor:** E12 (Bluetooth/NFC/Z-Wave via Zwave JS UI) and E15 (Modbus for solar/battery/grid telemetry) both publish onto a shared Mosquitto MQTT bus under a stated topic-naming convention — stable enough for G6's flows and G7's dashboards to subscribe against without needing to know which physical protocol produced a given reading. Track E's job stops at publishing correctly-named topics; reacting to them (an automation rule, a physical alert) is entirely G6's design to make.
- **Topic convention, corrected after review:** deltos/v1/<device-class>/<device-id>/<metric> — the first draft's version embedded the publishing box's hardware tier in the path, which is a property of the box, not the sensor, and would break every subscription on a hardware swap. Dropped, and a schema-version segment added instead, matching the versioning discipline used everywhere else in the project (WAX's version_major/minor, B3's manifest version) so a future change to a metric's shape has a way to signal itself on the bus.
- **Minimum broker security posture — a gap review caught:** Nothing in the first draft said anything about who can publish or subscribe. Community Hub and Field Ops deployments explicitly run on shared or semi-public networks (libraries, community centers, disaster response) — a default/anonymous Mosquitto instance lets any device on that network spoof telemetry or, once G6 exists, potentially trigger a physical alert G6 is described as capable of. Minimum posture, owned here since Track E owns the bus: bind Mosquitto to a localhost/VPN-only interface where possible, and require username-plus-per-topic-prefix ACLs otherwise — full policy design can wait for G-track integration, but an unauthenticated default cannot ship.

## 12. Community Sensing / SDR (E13)

Receive-only AIS/ADS-B/weather-satellite decoding via RTL-SDR — legally the simplest RF item in the plan, and worth keeping that simple by scoping Track E's job narrowly.

- **Hardware:** A standard RTL2832U-based USB dongle — cheap, widely available, receive-only by design, which is what keeps this legally uncomplicated almost everywhere (Track E14 below is a categorically different, transmit-capable hardware class; the two should never be confused as "the same radio hardware, different software").
- **Decode and publish, display is someone else's job:** E13's scope stops at running dump1090/AIS-catcher and publishing decoded tracks (aircraft, vessels, weather data) — likely onto the same MQTT bus §11 establishes, for consistency. Which track actually renders a map of this data (a Track H dashboard, or Track G's G7) is an open item flagged for whichever track claims it (§17), not decided here.

## 13. Advanced RF: Private Cellular Core (E14)

Already flagged as regulatory-sensitive and opt-in-only in the master plan; this document adds the one distinction worth making explicit given §12's hardware.

- **Different hardware class, not a software toggle on E13's dongle:** A private LTE/5G core (Open5GS/srsRAN) needs transmit-capable SDR hardware (a BladeRF- or USRP-class unit) — categorically more expensive and specialized than E13's receive-only RTL-SDR dongle. Nothing about this component should be read as "the same $30 dongle, different software"; it's a distinct, deliberately opt-in hardware purchase for Field Ops/Advanced deployments specifically pursuing this capability.

## 14. Local DNS & Service Discovery (E10)

The master plan leaves dnsmasq vs. CoreDNS open; this needs a decision since Track G's forthcoming G2 (reverse proxy) will need friendly hostnames to front.

- **Recommendation: CoreDNS for DNS, Avahi for mDNS:** CoreDNS is Go-native, matching the project's own service-selection filter (master plan §4) more directly than dnsmasq (C). mDNS/service discovery, though, is better served by Avahi specifically — it's the standard, already-present mDNS daemon on virtually every Linux distribution, and replacing it with a CoreDNS plugin would be swapping a mature, purpose-built tool for a secondary feature of a general-purpose one. Two small tools, each used for what it's actually best at, rather than one tool stretched to cover both.
- **Naming convention — a protocol conflict caught, not just deferred:** The first draft's example (wiki.deltos.local) collides with the split it was illustrating: RFC 6762 reserves .local exclusively for mDNS, and most client resolvers (systemd-resolved, macOS, Android) route any .local query to multicast DNS only, never to a configured unicast server — meaning CoreDNS's zone would simply never be queried for names under .local, undermining the reason CoreDNS was chosen over Avahi in the first place. Fixed at the protocol level, not left for Track G to discover: CoreDNS serves a distinct suffix (e.g. deltos.lan), and .local names resolve via Avahi/mDNS only. The specific hostnames under deltos.lan are still Track G's G2 to settle jointly (§17) — only the suffix split needed fixing here.

## 15. Local NTP (E11)

chrony, optionally GPS-disciplined — the master plan's own framing ("for sites with no internet access ever") surfaces a hardware gap worth stating plainly rather than assuming away.

- **The RTC gap on boards without a battery-backed clock:** Many ARM64 single-board machines, the Pi 4 and 5 included, ship with no battery-backed real-time clock, meaning the system clock resets to a fixed epoch on every power loss unless something disciplines it. For a genuinely offline Field Ops deployment (no internet, no GPS fix indoors), that's a real problem: a cheap I2C RTC module (e.g. a DS3231) is a stated hardware recommendation for any Field Ops-profile box relying on E11 without a reliable external time source — not a nice-to-have.
- **Why correct time matters beyond E11 itself:** TLS certificate validity (Track G's forthcoming G2) and session/token expiry (Track F's forthcoming F12) both depend on a sane wall clock, more strictly than most of what's in this document. Flagged forward as a dependency those tracks' own refinements need to account for, not assumed to be someone else's problem.

## 16. Hardware & Profile Scoping

- E1, E2, E5, E6, E10, and E11 apply to every deployment down to the Kiosk profile — this is the baseline platform, not an add-on.
- E3, E4, and E7 matter most at the Classroom profile and above, where an admin or teacher (rather than a single fixed reader deployment) is actually configuring the box.
- E8, E9, E12, E13, E14, and E15 are Community Hub/Field Ops-only by design — nothing in this set should ever be a default-on feature on Kiosk- or Classroom-profile hardware.

## 17. Open Decisions Needed Before Implementation

- Validate a minimal rpm-ostree image's real footprint on Pi Zero 2W hardware (§2) before the Kiosk profile's OS choice is treated as final — the Yocto fallback is documented but not the preferred outcome.
- Decide whether /var/lib/deltos (§4) needs its own partition/volume and a wear-aware filesystem (F2FS) for microSD-based tiers, rather than sharing general /var's wear characteristics.
- Settle the LAN service-naming convention under deltos.lan (§14) jointly with Track G's G2 once that track's refinement exists — E10 shouldn't unilaterally pick specific hostnames Track G then has to adapt to.
- Decide which track (G or H) owns displaying E13's decoded sensing data (§12) — Track E's scope stops at publishing it.
- Confirm with Track G and Track F that E11's clock-dependency note (§15) is accounted for in G2's TLS design and F12's token-expiry design once those tracks are drafted.
- Produce a real consolidated memory budget for Kiosk tier (rpm-ostree + Chromium + wax-serve instances + Piper TTS + deltos-shelld + Track F's identityd/installd/healthd, summed against 512MB) and for Community Hub's 16GB floor (K3s + Track G services + Track H apps + D5's larger model + D1/D2's indexes, summed concurrently) — §5 states the concern but the arithmetic itself still needs doing against real measured footprints, not estimated ones.
- Confirm with Track B and Track C that B4's catalog file, installed pack storage, and Track C's Chromium profile directory all resolve under /var/lib/deltos in their actual implementations (§3, §4) — see the companion amendments to those two documents.
- Decide how much of an update/rollback story DeltOS should build for the installable-stack path (§3) versus leaving host-level updates entirely to the person's own system — container-image-tag rollback at the Track G orchestration layer is the stated minimum, not a full equivalent to ostree's guarantee.
- Decide which track owns the download-time website configurator (the pre-download version of §5's checkbox calculator, run before any DeltOS component exists on the visitor's machine) — it doesn't fit cleanly inside any of the eight tracks as scoped so far and is likely either a Track H item or a standalone site component outside the eight-track structure (§21).

## 18. Refined Phase Sequencing for Track E

| **Phase** | **Track E deliverable** |
|---|---|
| P0 | E1's OS decision proven on at least one board per tier family (a Pi and an x86 box, per the master plan's own next-steps item); E6's tier table (§5) published, since B3/D7/C already reference it. |
| P1 | E2 (A/B updates via ostree) and E5 (/var/lib/deltos convention) — the persistence guarantees every other track's writable state depends on; E10 and E11, matching the master plan's existing tags. |
| P2 | E3 (Wi-Fi AP/captive portal) and E4 (network diagnostics), unblocking Track C's C10 onboarding wizard; E7 (provisioning tool). |
| P4 | E12 and E15 (IoT/smart-grid MQTT bridges), matching the master plan's existing tags; E8 (mesh) and E9 (power telemetry) — grouped here with the rest of the Community Hub/Field Ops hardware set rather than left unstated. |
| P5 | E13 (community sensing/SDR), matching the master plan's existing tag. |
| P5+ | E14 (private cellular core), opt-in only — matching the master plan's existing tag. |

## 19. What This Unblocks

- Every other track gets an actual definition for the E6 tier names it's been citing (§5) instead of an assumed vocabulary nobody had published yet.
- Track C's C10 onboarding wizard gets concrete APIs to wrap for both network setup (§6) and diagnostics (§7), and Track F's F1 admin console gets the same diagnostics API rather than a second implementation.
- Track G's forthcoming G1 (orchestrator) and G2 (reverse proxy) both get a settled OS/update foundation (§2, §3) and a stated naming/telemetry contract (§10, §11, §14) to build against instead of open questions.
- Track D's D8 gets its outstanding peer-discovery question answered (§9) rather than left open one track further down the dependency chain.

## 20. Independent Review — Findings and Resolutions

- **[Critical]** The claim that ostree rollback protects installed packs, B4's catalog, and F11's progress rested entirely on those actually resolving under /var/lib/deltos — a path this document invents, which Track B and Track C were drafted and reviewed without ever agreeing to. Resolved: reframed as a requirement flowing backward onto those two settled documents rather than an already-discharged fact, with companion amendments added to both, plus an explicit requirement that Chromium's own profile directory (where Track C's per-pack storage physically lives) is pinned under the same root (§3, §4).
- **[Critical]** E6's RAM figures were never summed against the actual concurrent software load at either end of the range — Kiosk tier's 512MB against rpm-ostree plus Chromium plus per-pack wax-serve plus Piper TTS, and Community Hub's 16GB floor against the full Track G/H stack plus Track D's larger model. Each concern lived in a different track's open-decision list with nothing reconciling them. Resolved: consolidated into one flagged budget item owned by the table that states the numbers (§5, §17), rather than left scattered.
- **[Security]** E3's Wi-Fi configuration operations (join network, configure AP) were assumed covered by Track C's role-gating fix just because they'd share its IPC category — but Track C's actual checked-operation list names only install/remove/mount, never Wi-Fi. Resolved: explicitly added to the role-gated set, sourced from F3 the same way (§6), with a companion note added to the Track C document.
- **[Security]** E12/E15's shared MQTT bus had no stated authentication or ACL model, despite Community Hub/Field Ops deployments explicitly running on shared or semi-public networks — an unauthenticated broker would let any device on the network spoof telemetry or trigger a physical alert once G6 exists. Resolved: minimum posture stated (localhost/VPN-only binding, or username-plus-per-topic ACLs) as Track E's own responsibility for the bus it owns (§11).
- **[Moderate]** The recommended CoreDNS+Avahi split used a .local example hostname, which RFC 6762 reserves exclusively for mDNS — most client resolvers would never query CoreDNS for it, undermining the reason CoreDNS was chosen at all. Resolved: CoreDNS scoped to a distinct deltos.lan suffix, with .local left to Avahi/mDNS exclusively (§14).
- **[Moderate]** balenaOS's exclusion was framed as a container-compatibility collision with Track G's G1, which overstates the actual conflict (balenaOS can run Compose/K3s workloads fine); and the Yocto fallback for Kiosk tier was framed as a costless safety net despite reintroducing the exact board-support-package burden cited against Yocto generally. Resolved: reframed around the real conflict (competing update/supervisor ownership) and the fallback's real cost stated honestly (§2).
- **[Moderate]** The MQTT topic convention embedded the publishing box's hardware tier in the path (breaking every subscription on a hardware swap) and carried no schema-version segment, unlike the versioning discipline used everywhere else in the project. Resolved: tier dropped from the topic, a version segment added (§11).
- **[Moderate]** Track D's D8 had explicitly flagged peer discovery as an open question for Track E to confirm; this document, now drafted, never addressed it. Resolved: same-LAN discovery answered via E10's Avahi/mDNS, cross-site discovery answered as admin-configured via Track F's forthcoming F6, rather than left dangling a second time (§9).
- **[Moderate]** The Yggdrasil/Reticulum split didn't address a mixed-topology deployment (some boxes on Wi-Fi, others only on radio) or classify a moderate-bandwidth, IP-capable radio link between the two. Resolved: mixed topologies stated as out of scope for v1 (a manual gateway node, not automatic bridging), with IP-capable links classified as Yggdrasil regardless of being carried over radio (§9).
- **[Moderate]** E9's scope was narrowed to raw telemetry only, silently dropping "panel-positioning guidance" despite it being named explicitly in the master plan's own E9 line item. Resolved: restored as a simple computed value E9 derives from telemetry trends, still just published data for G7 to display, not a scope expansion into dashboard-building (§10).
- **[Minor]** Track E repeatedly wrote "Kiosk-tier hardware," "Field Ops-tier hardware," etc., conflating Deployment Profile names with the hardware vocabulary — the same error Track B's review had already caught elsewhere. Now doubly stale: the tier names are retired entirely (contract §2) and the remaining "Field Ops tier" in the E6 table at all. Resolved: swept for consistent Profile/tier phrasing throughout (§2, §5, §6, §9, §15, §16, §17).
- **[Minor]** §3 described /etc and /var as preserved identically across an ostree update; in fact only /var is directly shared, while /etc undergoes a three-way merge that can drop local edits. Resolved: description corrected, noting DeltOS's own state avoids /etc entirely so the distinction doesn't affect its guarantees (§3).

## 21. Architecture Amendment (per product direction on deployment model and hardware philosophy)

After this document's independent review, the product owner raised a foundational check: DeltOS should follow Internet-in-a-Box's own model of hardware-agnostic software downloaded onto commodity hardware meeting a minimum spec, with the underlying OS kept lightweight/agnostic and DeltOS itself doing the heavy lifting — while still functioning as one unified system, with room for optional hardware (HATs, external storage) to extend capability. Three concrete decisions came out of that check-in and are recorded here, since they touch this document's core recommendations directly.

- **Decision 1 — both deployment models, not a choice between them:** This document's appliance-image work (rpm-ostree, §2-§3) is not being replaced or reconsidered — it stays as the recommended path for hardware being dedicated to DeltOS outright, and it is what gives DeltOS its strongest update/rollback guarantee. Alongside it, DeltOS now also supports an installable-stack path (§2) for hardware or systems someone already has, closer to how IIAB is actually obtained in practice. Neither path is the OS choice being second-guessed here — this is an addition, not a reversal, of §2's recommendation.
- **Decision 2 — a dynamic calculator replaces a fixed "optimum" spec:** Rather than DeltOS publishing one prescribed minimum spec and one prescribed optimum spec, the person selects the tools/features they want (a checkbox-style menu, available at download time and later inside DeltOS's own app store) and sees a live, recalculating minimum-and-optimum requirement readout for exactly that selection. DeltOS's job shifts from prescribing hardware to maximizing how many tools/features actually work across the hardware range described by E6 (§5) — sourcing suitable hardware is explicitly left to the person. This does not remove E6's tier table or Track D's D7 gating table; both remain the underlying data the calculator sums against (§5).
- **Decision 3 — hardware add-ons stay a general principle for now:** HATs and external/expansion storage for increasing a given tool's capacity (e.g. a larger local search index or a bigger media library) are confirmed as something DeltOS's architecture should not foreclose, but no specific accessory catalog is being built yet — this stays a general extensibility principle (additional storage mounts under /var/lib/deltos §4; additional radio/sensor hardware following E8/E12/E13's existing patterns) rather than a bill of named SKUs, consistent with how lightly Track E already scoped E13/E14's optional hardware.
- **What this does not change:** Track E's board-support and OS recommendations (§2), the E6 tier table's numbers (§5), and the phase sequencing already agreed (§18) all stand. The amendment is additive: a second deployment path, a different mechanism for communicating hardware requirements, and an explicit non-decision on accessory cataloging — not a re-opening of anything this document had already settled before its independent review.

## 22. Post-Review Amendment (from Track F's review)

Track F's independent review noted that §5's still-open Kiosk-tier budget concern — rpm-ostree, Chromium kiosk mode, deltos-shelld, per-pack wax-serve instances, and Piper TTS, all competing for 512MB — never accounted for Track F's own daemons, even though Track F's §14 states plainly that F2, F3/F11, and F4 all apply down to Kiosk profile.

- **Three more names added to an already-open item, not a newly-closed one:** deltos-identityd (Track F's F3/F11 service), deltos-installd (F4), and deltos-healthd (F5) are added to §5's enumeration and to §17's open-decision bullet — all three are stated in Track F's own design as lightweight, SQLite-backed, and (for installd and healthd) idle or low-frequency-polling processes rather than continuously active ones, but that's a design intention, not a measured footprint. This remains exactly the kind of consolidated, real-measurement budget §5 and §17 already called for before this amendment — the open decision is unchanged in kind, only more complete in what it needs to sum.

## 23. Amendment (per product direction — catalogue additions and self-healing)

Each item below states its **goal**, the **property that must hold**, and **why**. All are subject to the app contract (cross-track contract §15), the single licence check (§16) where they carry third-party content, and measured-resource gating (§2).

### 23.1 E7 gains the download-time configurator

**Goal.** The checkbox-style selector that computes a live minimum and preferred requirement for the tools a person actually selected becomes part of E7 (provisioning and installer), rather than remaining unowned.

**Property.** The configurator's readout is the **sum of `min_ram_bytes` / `min_storage_bytes` declarations** from the selected items (contract §2.3), never a figure maintained by hand in a document. Adding a tool to the catalogue changes the readout with no edit here.

**Why.** The master plan flagged this as referenced by three track documents (Track E §17, Track B §17, Track C §7/§19) and owned by none — precisely the "no home, so it gets a TBD" pattern this pass exists to end. E7 already owns the download and flashing path the configurator sits in front of.

### 23.2 E7 gains install parity

**Goal.** A **one-command install on a Debian/Ubuntu host**, plus a supported **WSL2 path**, alongside the appliance image.

**Property.** The installable-stack path is co-equal with the image: a box installed this way publishes the same capability record (contract §9) and is gated identically. Nothing may assume the appliance path.

**Why.** Both competitors offer a one-command install, and it is how most people will first try DeltOS. WSL2 in particular makes the whole system evaluable on a Windows machine with no hardware at all. The `generic` tier that previously existed to give such a host a name is retired — with measured resources there is nothing left to special-case.

### 23.3 E10 gains a self-service name registry

**Goal.** A person requests a name; an admin approves it; it resolves on the box's network at once.

**Property.** Names resolve under the zones the contract already owns (§8) — service names under `deltos.lan`, hosted sites under the hosting zone — and **an approved name resolves without a service restart**. Approval is an admin action; request is not.

**Why.** H18 hosting and the Commons make name allocation a routine act by non-administrators. Without a registry, every name is a manual DNS edit, which does not scale past a handful and makes hosting effectively admin-only.

### 23.4 E8 gains radio Q&A

**Goal.** On Field Ops, *Ask DeltOS* answers over the low-bandwidth radio mesh.

**Property.** **Text only, with pack citations.** The transport budget is the mesh's, not the model's: an answer that does not fit the link is truncated with its citations intact, never silently dropped. Field Ops only.

**Why.** The mesh already reaches places nothing else does; a question-and-answer service is the highest-value thing that fits in its bandwidth. Citations matter more here than anywhere else, because a recipient on a radio link cannot easily go and check.

### 23.5 Self-healing — the OS supervisor's half

The full model is cross-track; G1 owns orchestration-level healing and this document owns the OS-level supervisor. **Point 2 of the model is Track E's:**

**Goal.** No box, at any profile, runs a service with nothing watching it.

**Property.** **Where G1 orchestration is not present, the OS service supervisor owns restart.** At the minimum profile there is no orchestrator, so the supervisor is the whole of the healing model there — and it must act on an **unhealthy health-check result**, not only on process exit, since a frozen-but-running service exits nothing.

**Why.** Healing was specified as an orchestration property, and orchestration starts at `classroom`. That left the cheapest profile — the one most likely to be unattended in a place with no administrator — as the only one where a crashed service stayed crashed. Exactly backwards.

- **Crash-loop handling is shared with G1:** bounded backoff and an admin alert **before** a service reaches the terminal `failed` state (contract §7.1), so a box never sits silently restarting a broken service forever.
- **Corrupted data is not a restart case.** The supervisor hands it to F10 backup/restore rather than looping. A restart cannot repair bad bytes, and looping on them turns a recoverable fault into an outage.
