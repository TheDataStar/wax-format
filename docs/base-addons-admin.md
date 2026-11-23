# Base DeltOS, Add-ons and the Admin Dashboard

**Status:** design, first stage built (2026-11-22). The manifest format, the
fit check and the capacity calculator are in `crates/deltos-apps`; base
manifests in `system/apps/`; draft add-on manifests in
`crates/deltos-apps/addons/`. Contract changes this needs are marked
**[Proposed 2026-11-22]** where they are made (§9 below lists them).

Asked for by Commander (2026-11-22): treat the DeltOS screen as the **fed**
version that the box serves to every device at its own address, keep the
capacity sums in view, split a **base** DeltOS from **add-ons** downloaded
later (each with its own requirements), and build the **Admin Dashboard**
the owner uses to set up, curate and manage the box.

Precedence as always: `SPEC.md` > `cross-track-contract.md` > this document.
Nothing here invents a value the contract owns; numbers nobody has measured
are written as **open**.

## 1. In plain words

- **One box, three ways in.** Visitors' phones, tablets and laptops open
  `home.deltos.lan` and get the DeltOS screen (the "fed" version). The box's
  owner opens the **Admin Dashboard**, an app inside the same DeltOS screen,
  over HTTPS. The box's own screen, if it has one, is just another browser.
- **The device draws, the box answers.** Every window, menu and animation is
  drawn by the visitor's device. The box only answers requests: pages,
  searches, sign-in. That is why one small box can serve a room.
- **What runs out first is the Wi-Fi, not the box's processor.** Measured on
  one core of our development machine, the box server answers about 10,000
  pages a second, which is tens of thousands of reading students. A Pi 4's
  built-in Wi-Fi is reported to let only **8 devices join** with its stock
  firmware. That is the real limit and must be measured on hardware (§4).
- **Base DeltOS** is what every box has and can always run on the minimum
  spec (2 GB, 32 GB, ARM64): the home screen and library, sign-in, the box's
  Wi-Fi, names and certificates, backup, and the Admin Dashboard.
- **An add-on** is an optional app (for example email or a media hub) that an
  admin installs later from the DeltOS add-on store or a USB drive. Each one
  comes with a single file, its **manifest**, that says what it needs (RAM,
  storage, processor type, graphics), which ports it uses, who may open it,
  how to check it is healthy and what to back up.
- **The box checks before it installs.** It adds up what everything already
  installed needs, adds the new add-on, and compares that with what it
  measured about itself. If any figure has never been measured, it says
  "cannot tell" rather than guessing.

## 2. Three ways into one box

```
 visitors' devices ──plain HTTP──►  home.deltos.lan   ──►  the fed DeltOS screen
 (phones, tablets,                  (deltd: screen,         (Library, apps, search)
  laptops)                           box API, packs)
                                         │
 the owner's device ──HTTPS─────►  home.deltos.lan   ──►  the same screen, plus the
 (certificate installed once)            │                 Admin Dashboard app
                                         │
                                         ├── box API ──► deltos-shelld (closed list of actions)
                                         ├── identityd (people, sessions)
                                         └── add-ons at <name>.deltos.lan (through the proxy)
```

| | The fed screen | The Admin Dashboard |
|---|---|---|
| Who | Everyone who joins the box: students, teachers, guests | The box's owner and anyone given `admin.console` |
| Address | `http://home.deltos.lan` (and HTTPS where the certificate is installed) | `https://home.deltos.lan`, the "Box settings" app in the Manage group |
| Connection | Plain HTTP is fine (contract §16) | HTTPS only, with a session made over HTTPS (contract §4.2, §16) |
| What it can change | The person's own things (notes, settings) | The box: content, add-ons, people, network, backups, updates |

**Decided here (delegated): the Admin Dashboard is an app inside the WebOS
at `home.deltos.lan`, not a separate site at `admin.deltos.lan`.** Reasons:
the privilege boundary already puts every box-changing action behind the box
API at `home.deltos.lan` (`privilege-boundary.md` §2), so the dashboard is a
screen over those actions, not a second server; a separate origin would need
the still-open sign-in hand-off for service surfaces (contract §13.2); and the
UI review already placed Box settings in the launcher's Manage group, so it
looks and behaves like every other app (Jakob's law). Track F §5's "F1 is a
separate server-rendered local web app" is superseded by this; its hardening
(TLS, secure cookie, rate-limited sign-in, change the generated password at
first sign-in) is kept and is already built in `identityd` and `deltd`.
`admin.deltos.lan` stays reserved in contract §8 and unused.

## 3. Base DeltOS

Base DeltOS is the set of services every box runs, at every profile,
including Kiosk. It must fit the minimum spec with room for packs and
people. The store can never remove a base service.

| Part | Service | State | Manifest |
|---|---|---|---|
| Home screen, box API, library, search | `deltd` | Built (PR #4, #11) | `system/apps/home.toml` |
| Sign-in, people, roles, sessions | `deltos-identityd` | Built (PR #11) | `system/apps/identity.toml` |
| Box names and addresses | `dnsmasq` | Built (PR #5) | `system/apps/dns-dhcp.toml` |
| Box Wi-Fi | `hostapd` | Built (PR #10) | `system/apps/hotspot.toml` |
| Certificates | `deltos-tls-provision` | Built (PR #12); runs and exits | none: nothing stays running |
| Backup and restore | `deltos-backup` | Built (PR #13); runs and exits | none: nothing stays running |
| The closed list of box actions | `deltos-shelld` | **Not built** | added when built |
| Install pipeline (packs and add-ons) | `deltos-installd` (F4) | **Not built** | added when built |
| Health monitor | `deltos-healthd` (F5) | **Not built** | added when built |
| The WebOS screen itself (static files) | served by `deltd` | **Not built** (the shell mock is the draft) | part of `home` |
| Admin Dashboard | WebOS app | **Not built** (§8 is its design) | part of `home` |

Contract §6 holds a provisional 384 MB / 4 GB for "shell + baseline daemons".
The two base services measured so far use about 19 MiB idle together on x86_64
(§4.3); two are still open, and the operating system's own share has never
been measured (§6). So the 384 MB stays the number to argue with until the
base is measured on the minimum spec.

## 4. The fed screen and its capacity

### 4.1 Who does the work

| Work | Done by |
|---|---|
| Drawing the desktop, windows, menus, animation, themes | **The visitor's device** (costs the box nothing) |
| Sending the WebOS screen's files the first time | The box, once per device; ordinary HTTP caching after that (no service workers on plain HTTP, contract §16) |
| Pages inside packs, their pictures and styles | The box (`deltd` reads the pack and, for most pages, decompresses it) |
| Search and suggestions | The box (`deltd`, across every pack) |
| Signing in with a password | The box (`identityd`, argon2: 19 MiB and about 18 ms per check) |
| Signing in with a sign-in card or a remembered device | The box (cheap: a keyed hash, no argon2) |
| Add-ons (video, email, ...) | The box, inside each add-on; each declares its own cost (§5) |

### 4.2 The four limits

`deltos-apps capacity` computes how many people can be **active at once**
from measured figures and a usage scenario, and reports which limit is hit
first. Any figure not measured is reported as missing; the tool never fills
one in.

| Limit | People at once | Inputs |
|---|---|---|
| Radio stations | the number of devices the radio lets join | `radio_max_stations` |
| Radio data | usable radio bits/s × ceiling ÷ (8 × (page bytes × pages/min + first-visit bytes × first-visit share) ÷ 60) | `radio_bits_per_second`, `page_bytes`, `first_visit_bytes` |
| CPU | cores × ceiling ÷ ((page CPU s × pages/min + search CPU s × searches/min) ÷ 60) | `cpu_cores`, `page_cpu_seconds`, `search_cpu_seconds` |
| RAM | RAM left after every floor ÷ RAM per active person | `ram_free_bytes`, `ram_bytes_per_active_user` |

It also reports how many **password sign-ins at once** fit in the free RAM
(19 MiB each). Card and remembered-device sign-ins are not limited this way.

A **scenario** (pages a minute, searches a minute, share of first visits, and
the ceiling, the share of CPU and radio we allow ourselves before calling the
box full) is a what-if, not a measurement. Two are shipped in
`crates/deltos-apps/capacity/`; a box replaces them with its own aggregate
counts from F14 usage insights (never per learner, never leaving the box).

### 4.3 Measured so far (x86_64 development container, not the target)

Method: `deltd` release build pinned to **one core** (`taskset -c 0`), the
load generator (`oha`) on the other three, the real
`wikipedia_en_climate_change_mini_2024-06` ZIM (openzim test suite)
converted with `wss` at zstd 19 (8,925,435 bytes, 3,821 articles). Every
article URL in turn; CPU from `/proc/<pid>/stat`, memory from
`/proc/<pid>/status`. 2026-11-22.

| Clients at once | Pages/s | CPU per page | p50 | p95 | p99 | `deltd` RAM, high-water (MiB) |
|---|---:|---:|---:|---:|---:|---:|
| 1 | 8,129 | 0.095 ms | 0.11 ms | 0.16 ms | 0.22 ms | 13.4 |
| 10 | 10,722 | 0.092 ms | 0.93 ms | 1.49 ms | 1.87 ms | 29.9 |
| 40 | 10,093 | 0.098 ms | 3.93 ms | 5.59 ms | 6.88 ms | 88.6 |
| 100 | 8,429 | 0.118 ms | 11.99 ms | 17.36 ms | 22.81 ms | 85.8 |
| 250 | 9,975 | 0.100 ms | 24.39 ms | 42.85 ms | 58.61 ms | 102.7 |

| Other request | Rate (10 clients, 1 core) | CPU each | p50 | p95 |
|---|---:|---:|---:|---:|
| Search, one or two words, one pack | 6,569/s | 0.152 ms | 1.47 ms | 2.68 ms |
| Title suggestions ("clim") | 3,808/s | 0.261 ms | 2.70 ms | 4.08 ms |
| Password check (argon2id, 19 MiB, t=2) | about 55/s | 18.3 ms | | |

| Bytes (300 random articles) | Sent today | If compressed with gzip |
|---|---:|---:|
| A page's own HTML and files (mean) | 9,123 | 3,021 |
| The pack's shared scripts and styles (26 files, fetched once per device) | 829,968 | 228,682 |

Idle memory: `deltd` 13.4 MiB with one pack, `identityd` 5.3 MiB with one admin.

### 4.4 What the numbers already say

1. **The processor is not the limit for reading.** One x86_64 core answers
   about 10,000 pages a second. In the "research rush" what-if (a page every
   5 s and a search every 15 s each) that is about 16,500 people per core at
   half load. A Pi 4 core is slower by an amount we have **not measured**, but
   it would have to be thousands of times slower to matter. Search across
   many packs costs more per query and grows with the number of packs (11
   packs measured 1.5 ms p50 in the end-to-end check); it is the CPU figure to
   watch.
2. **The radio is the limit, twice over.**
   - *Devices that can join.* Public reports say the Pi 4's built-in Wi-Fi
     in access-point mode stops at **8 devices**: the Broadcom chip's own
     memory holds that many clients, and firmware security fixes have made
     it tighter, not looser. A balena maintainer's answer for a Pi 4 is "use
     a dongle" (one USB adapter then took 18 clients)
     ([balena forum, 2021](https://forums.balena.io/t/balena-hotspot-8-maximum-connections-rpi4/267477));
     the same 8 is reported for the Pi 3B+
     ([OpenWrt forum, 2022](https://forum.openwrt.org/t/remove-limit-of-8-devices-on-wifi-network-on-raspberry-pi/128734)),
     and offline-library users hit it too
     ([World Possible / RACHEL, 2021](https://community.worldpossible.org/t/raspberry-pi-max-wifi-clients-bug/2050)).
     **Reported, not measured here.** If it holds, a class of 25 phones cannot
     all join the board's own radio, and a classroom box needs a USB Wi-Fi
     adapter or a separate access point (and some USB chips have similar
     limits, so the adapter must be measured too). Mini PCs vary by card
     (some Intel cards cannot act as an access point on 5 GHz at all). This
     is the first thing to measure when hardware testing starts.
   - *Data it can carry.* In the "reading class" what-if everyone arrives in
     the same minute, so the shared files dominate: about 114 kbit/s per
     person, so **each 10 Mbit/s of real Wi-Fi throughput serves about 43
     people** at half load. Real throughput on the board's 2.4 GHz radio is
     not measured.
3. **Compressing for browsers would cut the radio load to about a third.**
   Browsers over plain HTTP do not accept zstd, so today `deltd` sends pages
   **uncompressed** (no `Content-Encoding`). Plain gzip would shrink a
   device's first visit from 830 kB to 229 kB and each page from 9.1 kB to
   3.0 kB. Its CPU cost on the box is not measured. Next step for `deltd`
   (§11, stage 2): offer gzip, measure the cost, and prefer storing
   pre-compressed shared files.
4. **`deltd`'s memory levels off at about 100 MiB** under heavy load (40 to 250
   clients at once), from 13 MiB idle. It does not grow per person beyond that,
   so RAM is not the reading limit on a 2 GB box once the base fits.
5. **Password sign-in bursts need room.** Each password check holds 19 MiB
   for about 18 ms (x86_64). Thirty teachers pressing Sign in in the same
   instant would want about 570 MiB at once; in practice they queue. Students
   use sign-in cards, which cost almost nothing. `identityd` should cap
   password checks running at once by the free RAM (stage 2); the cap comes
   from this sum, not a guess.
6. **The WebOS screen's own first-visit size is not known yet**, because the
   screen is not built. It is a figure the shell build must report, and the
   capacity sum includes it the moment it exists. A pack's shared files are
   already 230 kB gzipped, so the screen should aim well under that.

### 4.5 Still to measure on the minimum spec

With `deltos-apps capacity --measured crates/deltos-apps/capacity/measured-min-spec.toml`
everything below shows as missing today. On the first real-hardware day
(held by Commander, 2026-11-20), in this order:

| Figure | How |
|---|---|
| `radio_max_stations` | join phones (or `wpa_supplicant` clients on a second machine) until one is refused |
| `radio_bits_per_second` | `iperf3` from several joined devices at once, through the box's radio |
| `page_cpu_seconds`, `search_cpu_seconds`, `cpu_cores` | the same `oha` run as §4.3 on the board, packs on SD card and on SSD |
| `ram_free_bytes`, `os_ram_bytes` | `MemAvailable` after first boot with base DeltOS running and idle (also the base manifests' open floors) |

The emulated ARM64 VM (`system/tests/vm/`) can measure the memory figures
early, since memory use does not depend on emulation speed; its CPU and radio
figures mean nothing.

**First VM memory run, 2026-11-22** (PR #18, `docs/box-radio-and-boot.md`
§6.1; 2 GB, 4 cores, Ubuntu 24.04 cloud image):

- The operating system alone used 232 MiB. About 140 MiB of that is
  cloud-image services a box image would drop, so this is not yet a box's
  `os_ram_bytes`.
- The whole box used 239 MiB idle and 252 MiB after the full test.
- Each service's memory:

  | Service | Memory |
  |---|---|
  | hostapd | 7.3 MiB |
  | dnsmasq | 4.3 MiB |
  | identityd | 4.7 MiB idle, 81 MiB with 40 people signing in at once |
  | deltd | 8.5 MiB |

The identity floor now uses the ARM64 burst figure. `dns-dhcp` and `hotspot`
stay `open`: their memory is measured, but their storage is not.

**Memory is not what limits a 2 GB box.** About 1.7 GB is still free with
everything running.

## 5. Add-ons and their manifest

### 5.1 What an add-on is

An add-on is an app from Track G or Track H (or a later one) that a box can
run if it fits. Each comes as a **bundle**: one signed file holding its
manifest (`deltos-app.toml`), its Docker Compose file (Track G §2: one
authoring format, run by G1), its icon and its own licence text. Every add-on
except `platform` stands on the **platform** add-on (G1 orchestrator + G2
reverse proxy, contract §6: 768 MB / 8 GB provisional), so the first add-on
a box installs brings the platform with it, and Kiosk boxes never run any
(contract §3).

Add-ons the catalogue already names (Track H, Track G; each still needs its
floors measured, contract §13.2):

| Add-on | From | Lowest profile |
|---|---|---|
| Courses (Kolibri) | H1 | classroom |
| Video and music (media hub, Jellyfin) | H2 | classroom |
| Documents (office suite) | H3 | community_hub |
| Code studio | H4, H12 | community_hub |
| Cloud files | H5 | community_hub |
| Wiki | H6 | classroom |
| Website builder | H7 | community_hub |
| Chat, Calls | H8, H10 | community_hub |
| Email | H9 | community_hub |
| Books and audio, Community radio, Game room, Nearby share, Commons, Classroom tools, Hosting, Security lab | H11, H13 to H19 | per their Track H sections |
| Git, flows, metrics, print, software depot | G3, G6, G7, G8, G9 | community_hub |

The "lowest profile" column restates Track H §13's scoping; the contract §3
profile floor still applies on top (`community_hub` = 16 GB / 256 GB).

### 5.2 The manifest, schema 1

TOML, one file, strict: an unknown key is an error. Contract §15 says the
wire format is the implementing track's to settle; this is it. The resource
fields are contract §2.3's, unchanged. Checked by `deltos-apps check`.

| Group | Field | Required | Meaning |
|---|---|---|---|
| | `schema` | yes | `1` |
| | `id` | yes | `[a-z][a-z0-9-]*`, 2 to 32 characters |
| | `name` | yes | What it does, in plain words ("Video and music", not "Jellyfin") |
| | `version` | yes | `MAJOR.MINOR.PATCH` of this packaging |
| | `kind` | yes | `base` or `addon` |
| | `summary` | yes | One sentence for the store and the dashboard |
| | `licence` | yes | Checked by the one licence check (contract §11, §16); off the allowlist means human review, as for packs |
| | `upstream` | no | The software behind it, shown to admins |
| | `min_profile` | yes | Contract §3; add-ons start at `classroom` |
| | `depends` | no | Other app ids, e.g. `["platform"]` |
| `[resources]` | `evidence` | yes | `measured`, `provisional` or `open` (§5.3) |
| | `min_ram_bytes`, `min_storage_bytes` | unless open | Contract §2.3 idle floor. **Absent when open, never 0** |
| | `arch` | yes | `aarch64`, `x86_64` or `any` |
| | `gpu` | no | `required` or `preferred` (preferred never gates) |
| | `install_bytes` | no | The install itself (images, program files) |
| | `basis` | unless open | Where and how it was measured |
| `[load]` | `evidence`, `basis` | if present | As above |
| | `ram_bytes_per_active_user`, `peak_ram_bytes`, `max_active_users` | at least one | How cost grows with use, for §4 |
| `[run]` | `form` | yes | `systemd` (base, and the platform) or `compose` (every other add-on) |
| | `entry` | yes | The `.service` unit, or the compose file in the bundle |
| | `listens` | no | `{ port, proto = tcp/udp, bind = loopback/lan }`; add-ons are **loopback only**, reached through the proxy |
| | `capabilities` | no | Linux `CAP_*`; **add-ons get none** (the platform excepted) |
| | `devices` | no | `gpu`, `wifi_radio` (base only), `removable_storage`, `audio`, `serial` |
| `[launcher]` | `group`, `icon` | no | One of the six launcher groups (`learn`, `create`, `talk`, `watch_and_listen`, `share`, `manage`) and an icon in the bundle |
| `[reach]` | `host` | no | `<host>.deltos.lan` (contract §8); `home` and `www` are reserved |
| | `requires` | yes | Permissions needed to open it; never empty, never a role |
| | `secure_only` | no | HTTPS with a secure session only |
| `[[defines]]` | `name`, `summary`, `secure_only`, `seed_bundles` | per entry | Permissions it adds to the catalogue (contract §4.1), and which shipped bundles get each one on install (`teacher`, `student`, `guest`; `admin` has everything already; a secure-only permission is never seeded to `guest`) |
| `[sign_in]` | `method` | yes | `webos`, `adapter_api`, `adapter_files`, `forward_auth`, `none` (Track H §2) |
| `[health]` | `probe`, `target` | yes | `systemd`, `http`, `tcp` or `compose`; nothing runs unwatched |
| `[backup]` | `paths`, `listed`, `exclude` | yes | What F10 copies, lists by name only, or skips; all under `/var/lib/deltos/` |
| `[participates]` | `search`, `notifications`, `progress` | no | Each opt-in, default off (contract §15) |

A worked example is `crates/deltos-apps/addons/media-hub.toml`.

### 5.3 Three kinds of evidence

The rule "never invent a value" lives in the data:

| Evidence | Means | What the store does |
|---|---|---|
| `measured` | Measured on the class of machine the add-on declares (its `arch`; the minimum spec for `any`) | Installs when it fits |
| `provisional` | A real measurement on another machine, or a contract §6 provisional figure | Installs when it fits, labelled "estimated" |
| `open` | Nobody has measured it; no number is written | **Cannot tell** whether it fits (§6) |

**Decided here (delegated): an add-on whose floor is open is not offered for
one-tap install.** The store shows it as "Not measured yet" and an admin can
still install it from the dashboard's add-on page with a re-confirmation
("This add-on has not been measured. The box may run short of memory."). Why:
refusing outright would make every Track H add-on uninstallable until each is
measured, while silent installs are exactly the invented-number failure the
rule exists to stop. This is reversible and can become a hard refusal later.

## 6. The fit check

`deltos-apps fit` (and the dashboard, through the same library) answers
"may this box install these add-ons?":

1. **Profile, architecture, GPU.** The box's profile must be at least each
   add-on's `min_profile`; `arch` must match or be `any`; `gpu = "required"`
   needs a measured GPU. Contract §2.3: a comparison of numbers and names
   from the capability record, nothing else.
2. **Dependencies.** Each `depends` entry must be installed or in the same
   set. The store installs the platform and the add-on together.
3. **Collisions.** No two apps on the box may listen on the same port, use the
   same `<host>.deltos.lan`, or define the same permission.
4. **RAM.** The operating system's own use, plus every installed app's
   `min_ram_bytes`, plus the set's, must fit in the box's measured RAM
   (contract §6: the budget is the sum of declared floors).
5. **Storage.** The set's `min_storage_bytes` plus `install_bytes` must fit in
   the free space measured now on the state filesystem.

Verdicts: **Fits**; **Fits, estimated** (some floor provisional); **Cannot
tell** (nothing blocks, but an open floor or the unmeasured operating system
makes a sum unknown; the answer names them); **Does not fit** (each reason
listed). A sum is "does not fit" even with open floors when the known part is
already over.

**[Proposed 2026-11-22] The capability record gains `os_ram_bytes`**
(contract §9): the RAM in use after first boot with no DeltOS service
running, measured by the provisioning step alongside the other four. Without
it no RAM sum can be complete, because the operating system is the biggest
single user of a 2 GB box and nothing declares it.

Today, on the minimum spec record, installing the media hub reports **Cannot
tell**: the operating system, `dns-dhcp`, `hotspot` and the media hub itself
have never been measured. That is the honest state, and it is why §4.5 is the
next step.

There is no headroom margin in the check yet (a share of RAM kept spare). Any
value would be a guess until floors are measured; it is open (§10).

## 7. Getting add-ons onto a box

**The bundle.** `<id>-<version>.deltos-addon`: a tar file holding
`deltos-app.toml`, the compose file, the icon, `LICENCE`, and the container
images as OCI archives (so a box with no internet can install from a USB
drive), signed with minisign by the project key the same way packs are (F7,
Track A A7). A bundle that fails its signature check is `failed_signature`
(contract §7.2) and never retried.

**Where they come from.**

| Source | How |
|---|---|
| The DeltOS add-on store (the future web app) | When the box has an uplink and an admin presses "Check for add-ons", the box downloads a **signed catalogue**: the manifests and bundle hashes, nothing about the box sent out (contract §16, no telemetry). The box runs the fit check on every entry locally and shows the result. Downloads resume (HTTP ranges), one at a time (Track F §4). |
| A USB drive | The same bundles, listed when an admin mounts a drive (`mount_removable`). |
| Another paired box | Later, with F6 pairing. |

The web app is the same catalogue rendered for people choosing hardware:
each add-on's page shows its manifest's requirements in plain words ("needs
16 GB of memory, an x86 or ARM processor, a graphics chip helps") and the
evidence behind each number.

**The pipeline.** `deltos-installd` (F4) installs add-ons through the same
four stages as packs: fetch, verify signature and version, place
(`/var/lib/deltos/apps/<id>/`, renamed into place only when complete),
register. Then G1 starts it, G2 gives it its address, `identityd` adds its
permissions to the catalogue and seeds the bundles, and the health monitor
starts probing. Removal is the reverse; the admin chooses whether its data is
kept (default) or deleted (re-confirmed).

**[Proposed 2026-11-22] New actions on the privilege boundary's closed list**
(`privilege-boundary.md` §8), all secure only:

| Action | What it does | Arguments | Permission | Re-confirm |
|---|---|---|---|---|
| `install_addon` | Install a signed add-on bundle (and the dependencies the fit check named) | source id from the store, drive or upload listing; expected id and version | `apps.manage` | Only for an add-on whose floor is open (§5.3) |
| `update_addon` / `roll_back_addon` | Move to a newer version, or back to the retained previous one (G1 keeps the last two) | add-on id; version from the listing | `apps.manage` | No |
| `remove_addon` | Stop and remove an add-on | add-on id; keep or delete its data | `apps.manage` | **Yes** |
| `set_addon_running` | Stop or start an installed add-on without removing it | add-on id; on or off | `service.restart` | No |

**[Proposed 2026-11-22] New permission `apps.manage`** (contract §4.2),
admin only by default, secure only. It is kept apart from `content.install`
because installing software that runs on the box is a bigger trust step than
installing a reading pack, and an admin may want a librarian who can do the
second and not the first.

**[Proposed 2026-11-22] Paths** (contract §10): `/var/lib/deltos/apps/<id>/`
for each add-on's own data (its volumes, its `backup.paths`), and
`/usr/lib/deltos/apps/` for the installed manifests (base ones from the
system image, add-on ones copied from their bundles).

## 8. The Admin Dashboard

### 8.1 Shape

An app called **Box settings** in the launcher's Manage group, visible only
to people holding `admin.console` (shown, not blocked: others never see it).
Laptop size opens it as a window: a 248 px sidebar with settings search at
the top (Ctrl K from anywhere), grouped sections, installed add-ons' pages,
and "About this box and help" pinned at the bottom. The window's title bar
carries the secure-connection lock, a **jobs tray** (long work with progress,
time left and cancel) and an **alerts bell** with a count. Phone size opens
one section at a time with a "Sections" back button, and tables turn into
stacked rows. Dark by default, like the rest of DeltOS. Opening it over plain
HTTP shows one screen: "Box settings needs a secure connection" with the
steps to install the box's certificate (`box-certificates.md`), instead of a
sign-in that would be refused.

Following Track F §5: **actions that change something are short wizards or
sheets** with the consequential choice confirmed (the admin's password again
where §8 of the privilege boundary says re-confirm); **views are plain
dashboards**.

Clickable mock (example data, not real): v2, reviewed 2026-11-22 against
modern admin dashboards and the Laws of UX, in the "Admin Dashboard review"
doc. Neither lives in the repo.

### 8.2 Sections

Sixteen sections in five groups, plus one page per installed add-on.

| Group | Section | Shows | Does | Built on |
|---|---|---|---|---|
| — | **Overview** (home) | "Finish setting up, n of 6 done" until done; four plain questions (design language §14): Is everything working? How full is storage? Who is on now? Is anything waiting? "Needs you" alerts with the next step; 24-hour lines for processor, temperature and memory; recent activity | Links into each section | F5 health, §4 estimate, F14 counts |
| — | **Set up this box** (first run) | A wizard: welcome, the admin's own password (replaces the generated one), box name, Wi-Fi name, password and country, profile (only those the box's measured resources allow), install the certificate on this device, starter content, a second admin, a first backup | `configure_ap`, profile choice, `identityd` password change | E3 (`deltos-net`), contract §3, `box-certificates.md` |
| Box | **Health and services** | Every service with its contract §7.1 state as a dot and a word, memory use against its declared floor, recent log lines; processor load, temperature and throttling | Restart a service, save its log to a drive | F5, `restart_service` |
| Box | **Storage** | What uses the space (packs, add-on data, backups kept, system), each drive, free space | Clean up staged and superseded files, eject a drive | `mount_removable` / `eject_removable`, `wax-builder gc` |
| Box | **Network and devices** | The box's Wi-Fi, devices joined now (against the measured radio limit), the uplink if any, names on `deltos.lan` | Change the hotspot, join a network for updates, name or reserve a device | `configure_ap`, `join_network`, E10 registry |
| Box | **Power, time and language** | Clock and whether it is trusted (most boxes have no clock battery), time zone, the shell's language | Restart or shut down safely; set the clock from this device; choose time zone and language | proposed actions, §9 |
| Content | **Library** | Every pack: name, size, languages, who can see it (guest on or off), last used (F14, aggregate), updates waiting; search, filter, sort, select many | Install from a drive, the store or an upload; update; roll back; remove; convert a ZIM from a drive in the background; choose which packs guests see; arrange collections | `install_pack`, `remove_pack`, `roll_back_pack`, `convert_zim_from_drive`, `stage_upload` |
| Content | **Add-ons** | Installed add-ons with health and memory use; the store by category, with each add-on's fit verdict in plain words ("Fits", "Fits, estimated", "Not measured yet", "Needs 16 GB, this box has 4 GB") and an "Only what fits" switch | Install (with its platform), update, roll back, stop/start, remove (keep or delete data) | §6 fit check, §7 actions |
| Content | **Home screen and notices** | What visitors see first: featured packs and apps, a notice to everyone, guest access | Edit and preview | proposed, §9 |
| People | **People and classes** | People, classes, roles (as permission bundles, including permissions add-ons define), sign-in cards, remembered devices | Add people (one or a whole class from a list), print sign-in cards, reset a password, remove a device, make a new role from permissions | `identityd` (`profile.manage`, role editing under `admin.console`) |
| People | **Sign-in and security** | The box certificate and its fingerprint, admins (a warning while there is only one), secure sessions open now, session lengths | Sign a session out, add an admin, re-offer the certificate | `identityd`, `deltos-tls` |
| Care | **Backups** | Last backup and its age, drives present | Make a backup (wizard: drive, passphrase twice), review and restore (staged review, then re-confirm) | `start_backup`, `review_backup`, `restore_backup` |
| Care | **Updates** | System version, an update waiting with its notes, the previous version kept | Apply, roll back (re-confirmed) | `apply_update`, `roll_back_update` |
| Care | **Activity** | The action log: who changed what, when, from which device; filter by person and time | Save to a drive | privilege boundary action log |
| Fleet | **Other boxes** ("DeltOS instances") | Paired boxes and their health, storage, version and last seen (F6); the box's own screens (kiosks) and which profile each shows | Pair a box (re-confirmed), set a kiosk's default Simple mode or class | `pair_box`, F6 |
| — | **About this box and help** | Name, version, profile, address; the capability record (what the box measured at first start); help stored on the box; licences | Save a support file to a drive (health, versions, logs; no names or personal data; nothing is ever sent) | capability record, contract §16 |
| Apps | **One page per installed add-on** | Overview, Settings, Logs, Versions tabs | The add-on's own settings, stop/start, update, roll back, remove | §8.5 |

"DeltOS instances" is read as **other boxes in the fleet and the box's own
screens**, since one box runs one DeltOS; if Commander meant something else,
this is the one row to change.

### 8.3 Laws of UX, applied

- **Jakob's law:** a settings window like macOS System Settings and ChromeOS
  Settings, with the same chrome, colours and type as every other DeltOS app.
- **Hick's law and Miller's law:** sixteen sections in five named groups, no
  group longer than four, and search for everything else.
- **Fitts's law:** every control at least 44 × 44 px, table rows 48 px, the
  page's main action top-right in the same place on every page.
- **Tesler's law:** the fit check carries the complexity; the admin sees one
  verdict in words, with the numbers one tap away.
- **Zeigarnik effect and goal-gradient:** "Finish setting up, 4 of 6 done"
  stays on Overview with a bar until done.
- **Peak-end rule:** the setup wizard ends on what the box can now do and the
  address to give people; every risky action ends in a message saying what
  happened and how to undo it.
- **Doherty threshold:** every view answers under 400 ms; long work (installs,
  backups, conversions) becomes a job in the jobs tray, not a frozen screen.
- **Von Restorff effect:** one filled orange button per page; alerts carry a
  severity stripe.
- **Postel's law:** search accepts everyday words ("wifi", "hotspot", "time");
  a class list can be pasted from a spreadsheet in any common shape, and the
  dashboard shows what it understood before adding anyone.
- **Serial position:** Overview first, help last and pinned.

### 8.4 Visual rules (design language §4, §5)

Measured on the mock by a script that loads every page in a browser, dark
and light (2026-11-22). The dashboard is built to pass the same check.

| Part | Rule |
|---|---|
| Type | Inter for the interface (14 px labels and controls, 16 px card headings, 24 px page titles); Atkinson Hyperlegible 16/24 px for explanations; JetBrains Mono for sizes, versions, addresses and times; 12/16 px small print never carries an action. No other sizes. |
| Spacing | The 4 px scale only: 8 inside controls, 16 inside cards, 24 between sections, 32 page edge. |
| Corners | The shell's radius tokens: 8 controls, 10 cards, 14 panels, 20 window, round pills. |
| Lines | 1 px dividers; a 2 px focus ring in `accent-ink`. |
| Width | Content up to 1,040 px; paragraphs up to 72 characters. |
| Targets | 44 × 44 px minimum; checkboxes and switches get a 44 px touch area. |
| Colour | The DeltOS palettes, dark default; text at 4.5 : 1 or more in both themes. Filled buttons use `accent` in dark and `accent-ink` in light (light `accent` with white text measures 4.16 : 1, a fail). |
| State | Always a dot and a word, never colour alone; colour-blind mode swaps success to blue. |
| Phone | Below 720 px: the sidebar becomes a list, pages open full screen, tables become stacked rows; no sideways scrolling at 390 px. |

### 8.5 How add-ons extend the dashboard

Adding a service changes no dashboard code. From an add-on's manifest
(§5.2) the dashboard already knows its name, icon, health check, logs,
versions, backup set, address and the permissions it defines. Installed, an
add-on appears under "Apps on this box" with a page whose tabs are the same
for every add-on: **Overview** (state, memory against its floor, address),
**Settings**, **Logs**, **Versions**. Its permissions appear in the roles
editor.

Its own settings need one more manifest section, **[Proposed 2026-11-22]**,
optional, schema 1 (contract §15):

```toml
[[admin.settings]]
key = "max_upload_mb"           # stored for the app under /var/lib/deltos/apps/<id>/
label = "Largest upload"        # shown, translatable like the shell
kind = "number"                 # one of: switch, number, text, choice
unit = "MB"
min = 1
max = 4096
default = 200
permission = "media.manage"     # needed to change it; defaults to apps.manage
```

The dashboard draws every setting with its own standard controls, so every
add-on looks and behaves alike, and it applies a change through the add-on
(restart only if the add-on says it must). Settings carry no secrets: a
password or key an add-on needs stays in its own secure setup. The parser
(`crates/deltos-apps`) does not accept `[admin]` yet; it is built with the
dashboard in stage 4, once the contract change is adopted.

### 8.6 Records: adding, viewing, changing, removing

Every record an admin manages is described once, as data: its fields (with
kind, whether needed, length, pattern, uniqueness, help line), the columns
its list shows, and for each operation its label, the box call behind it,
whether it needs the admin's password again, and what removing it does in
plain words. The list, record, form and remove screens are drawn from that
description, so a new record type, including an add-on's own records, gets
the same four screens with no new screen code. The mock implements exactly
this (example data); stage 4 builds it for real.

| Record | Lives in | Add | Change | Remove | Password again | Backed by |
|---|---|---|---|---|---|---|
| Person | People and classes | Add person, or a pasted class list | Name, roles, class, sign-in | Remove (sessions end; notes and progress deleted; log kept). The only admin cannot be removed | Remove | `create_profile`; proposed `update_profile`, `remove_profile`; `issue_card`, `set_password` |
| Class | People and classes | Add class | Name, teacher, packs shown first, Simple mode | Delete (students stay, without a class) | No | proposed `create_class`, `update_class`, `remove_class` |
| Role | People and classes | New role, or Duplicate | Name, purpose, permissions (grouped; add-ons add theirs) | Delete. Built-in roles cannot be changed or deleted | Remove | proposed `create_role`, `update_role`, `remove_role` (contract §4: roles are data) |
| Remembered device | People and classes | By signing in | No | Forget | No | `list_devices`, `remove_device` |
| Pack | Library | Add packs (drive, store, upload) | Who sees it, collection; update; go back | Remove | Remove | `install_pack`, `roll_back_pack`, `remove_pack`; `set_home_screen` |
| Collection | Library | New collection | Name, description, packs | Delete, with Undo (packs stay) | No | `set_home_screen` |
| Notice | Home screen and notices | New notice | Text (200 characters), audience, from, until, pinned | Delete, with Undo | No | `set_home_screen` |
| Name on `deltos.lan` | Network and devices | Name a device | Name, device, fixed address | Remove name, with Undo | No | `approve_name`, `set_address_reservation` (box-network-plan §7) |
| Box screen (kiosk) | Other boxes | Add a screen | Name, opens to, class, Simple mode | Remove screen | No | proposed `add_kiosk`, `update_kiosk`, `remove_kiosk` |
| Paired box | Other boxes | Pair a box (6-digit code) | Rename | Unpair (nothing deleted on either box) | Add, remove | `pair_box`; proposed `rename_box`, `unpair_box` |
| Add-on | Add-ons | Install | Its own settings (§8.5); update; go back; stop and start | Remove, keeping or deleting its data | Remove; install when not measured | §7 actions |
| Backup | Backups | Back up now | No | On the drive | Restore | `start_backup`, `review_backup`, `restore_backup` |
| Secure session | Sign-in and security | By signing in | No | Sign out | No | `sign_out` |

**How sure before removing.** Three levels, chosen per record:

1. *Easy to put back* (a notice, a collection, a device name): removed at
   once, with Undo in the message for 8 seconds.
2. *Cannot be put back* (a class, a remembered device, a screen): a sheet says
   exactly what happens, then asks.
3. *Changes the box or someone's data* (a person, a pack, a role, an add-on,
   pairing): as 2, plus the admin's password again (privilege boundary §6).

Something that must stay (the only admin, a built-in role) keeps its button
visible but off, with the reason beside it.

**Page patterns** (the mock's "Patterns guide" page shows each, live):

- *List:* search first, at most two filters, the main action top right, rows
  48 px, the whole row opens the record, "Showing x of y", a bar for actions
  on several selected rows.
- *Record:* the path back at the top, Edit top right, then details, actions,
  history (from the action log), and removal last in its own box.
- *Form:* the same form adds and changes; one column, at most 480 px (640 for
  long text); labels above, "(optional)" on optional fields, a help line
  below, a character count where there is a limit. Checked on Save: a summary
  at the top links to each field, and each field says what to do. Save says
  what it does ("Add person", "Save changes"); leaving with changes asks.
- *Empty:* an icon, one sentence on why it is empty, and the first step.
- *Loading:* grey rows shaped like the real ones, shown only after 0.4 s.
- *Failed:* what happened, what it means, what to do, and Try again;
  "Nothing was changed" when true.
- *Settings rows:* a switch acts at once with no Save; a change that
  disconnects people asks first.

## 9. Contract changes this needs

All marked **[Proposed 2026-11-22]**, to be adopted with this pull request:

1. **§9:** the capability record gains `os_ram_bytes` (§6).
2. **§4.2:** a new permission `apps.manage`, admin only, secure only (§7).
3. **§10:** paths `/var/lib/deltos/apps/<id>/` and `/usr/lib/deltos/apps/` (§7).
4. **§8:** `admin.deltos.lan` stays reserved; the Admin Dashboard is an app at
   `home.deltos.lan` (§2).
5. **§15:** the wire format is this document's §5.2 (schema 1), plus the
   optional `[admin]` settings section of §8.5.

And `privilege-boundary.md` §8 gains the four add-on actions (§7).

The dashboard's other new sections need these, also **[Proposed
2026-11-22]**, all secure only, under `admin.console` unless named:

| Action | What it does | Re-confirm |
|---|---|---|
| `power_box` | Restart or shut down the box safely (jobs finish or are stopped first) | **Yes** |
| `set_clock` | Set the time from the admin's device and the time zone; a box with no clock battery marks the time trusted, so `deltos-tls` can reissue provisional certificates | No |
| `set_language` | The shell's default language | No |
| `set_home_screen` | Featured packs and apps, the notice to everyone, guest access (`content.install`) | No |
| `save_support_file` / `export_activity` | Write the support file or the action log to a mounted drive | No |

The records of §8.6 also need, **[Proposed 2026-11-22]**: in `identityd`,
`update_profile`, `remove_profile`, `create_class`, `update_class`,
`remove_class`, `create_role`, `update_role`, `remove_role` (role editing
was already planned with F1; `admin.console` for roles, `profile.manage`
scoped to the class for the rest); in `deltos-shelld`, `add_kiosk`,
`update_kiosk`, `remove_kiosk` (`admin.console`) and `rename_box`,
`unpair_box` (`fleet.manage`, unpair re-confirmed).

Where the home screen settings are stored (a `deltd` record or a file under
`/var/lib/deltos/`) is decided when stage 4 builds them.

## 10. Open, not filled in here

- **Every measured figure in §4.5**, the operating system's RAM, and the open
  base floors (`dns-dhcp`, `hotspot`). Held for hardware testing; the VM can
  take the memory ones earlier.
- **Every Track H add-on's floor** (contract §13.2). Each add-on gets measured
  when it is packaged, on the minimum spec for `any`.
- **A headroom margin** in the fit check (RAM kept spare): a number only after
  floors are measured.
- **The WebOS screen's first-visit size budget:** set from the first shell
  build's measurement (§4.4 item 6).
- **Docker's engine runs as root.** The platform add-on brings a root daemon
  onto the box. Track G chose Compose (Track G §2); whether a rootless engine
  or a different runtime is required on boxes that hold children's data is a
  security question for Track G, not settled here.
- **How a box trusts an add-on not signed by the project key** (a school's
  own app): the same question as box-converted packs (architecture Q2,
  contract §13.2). Until decided, only project-signed bundles install.
- **The future web app's hosting** (architecture open Q3, who hosts the WSS
  SaaS) also decides where the add-on catalogue is served from.
- **The icon set** (contract §13.2), which the launcher entries need.

## 11. Build plan, in stages

| Stage | What | State |
|---|---|---|
| 1 | This design; the manifest format, fit check and capacity calculator (`crates/deltos-apps`, 29 tests); base manifests; three draft add-on manifests; the capacity measurements on x86_64 | **This pull request** |
| 2 | `deltd`: gzip for browsers (§4.4 item 3) with its CPU cost measured; `identityd`: cap password checks at once by free RAM; measure the base floors and `os_ram_bytes` in the ARM64 VM | Next |
| 3 | `deltos-shelld` with the closed list, and `deltos-installd` for packs and add-ons (F4), with the add-on actions and `apps.manage` | **Add-ons and shelld built 2026-11-23** (`docs/shelld.md`); packs next |
| 4 | The WebOS shell build from the shell mock, served by `deltd`, with its first-visit size measured; the Admin Dashboard app on it: Overview, Set up, Library, People first, then Add-ons, Network, Backups, Updates, Other boxes, Activity | After 3 |
| 5 | The platform add-on (G1 + G2), then the first two add-ons (media hub, email), each with its floor measured | After 4 |
| 6 | The add-on store web app and its signed catalogue | After 5 |
| — | Every figure marked open, on the minimum spec | When hardware testing resumes |
