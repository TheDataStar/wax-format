# The box's own Wi-Fi, and how the box starts itself

*Written 2026-11-21 with the build in `crates/deltos-net` (the radio) and `system/` (the boot units, users, directories and installer). Builds Track E's E3 hotspot (Track E §6) and the boot half of `docs/box-network-plan.md` §9 step 2, under `docs/privilege-boundary.md`. Tested in an emulated ARM64 machine with virtual Wi-Fi radios (§6); **not yet on a real Pi or real phones** (Commander's hold, 2026-11-20).*

**What this document owns:** how the box's hotspot is set up, and which programs start at boot, in what order, as which user, holding which powers. **What it does not own:** names and addresses (`docs/box-network-plan.md`), who may change any of it (`docs/privilege-boundary.md`), and the OS image itself (Track E §2, E1).

Plain words used below: **hostapd** is the program that makes a Wi-Fi card act as an access point. **systemd** is the part of Linux that starts programs at boot and restarts them when they crash; a **unit** is one of its small settings files. A **capability** is one narrow power (for example "may use port 80") that a program can hold without being the all-powerful root user.

---

## 1. In short

- **The box runs its own Wi-Fi with hostapd**, WPA2 with AES only, on 2.4 GHz. Phones cannot see each other on it, only the box.
- **Four values, none defaulted:** the network name, its password, the country, and the channel. A default name or password would be the same on every box; the country decides which channels are legal. Whoever sets up the box gives all four (§4), and an admin changes them with the `configure_ap` action, which asks for their password again (privilege boundary §8).
- **The password is root's alone.** It is stored in the network registry, in a directory only root can read, and the generated `hostapd.conf` is created owner-only before a byte is written. It never appears in a log or a debug print.
- **Everything starts on its own at boot, in a fixed order** (§2): settings, address, firewall, radio, DNS and DHCP, the sign-in service (`deltos-identityd`, `docs/identityd.md`), then `deltd`. A box that has not been set up yet starts nothing and reports nothing as failed.
- **Each program holds only what it needs** (§3): `deltd` runs as its own user with one power (port 80) and cannot see the network settings; `dnsmasq` runs as its own user with three; hostapd keeps root but only the two radio powers.
- **A crashed service comes back by itself** within seconds, with a growing delay and a cap so a broken one is not restarted forever. A change made through a checked action takes effect without a reboot, restarting only what it touched.

## 2. Boot order

All of it hangs off one target, `deltos-box.target`, enabled for normal boot.

| Step | Unit | Does | Runs only when |
|---|---|---|---|
| 1 | `deltos-net-render` | Writes the settings files (`dnsmasq.conf`, names, reservations, `uplink.nft`, `hostapd.conf`, `hotspot.env`, `deltd.env`) from the registry into `/run/deltos/network/` | The registry exists |
| 2 | `deltos-hotspot-address` | Gives the hotspot interface the box's address; keeps NetworkManager off it; unblocks Wi-Fi where the image blocks it | A hotspot is set |
| 3 | `deltos-uplink-firewall` | Loads the uplink firewall **before the radio exists**, so a hotspot device is never forwarded anywhere while it is unset | Step 1 wrote it |
| 4 | `deltos-hostapd` | The radio | The box runs a hotspot and its radio is set |
| 5 | `deltos-dnsmasq` | DNS for the box's names; DHCP on the hotspot only | Step 1 wrote it |
| 6a | `deltos-identityd` | The register of people and sign-ins, on a local socket only `deltd` can open (`docs/identityd.md`) | The box has its first admin (§4) |
| 6c | `deltos-tls` | Makes the box's own CA the first time, and renews the two server certificates when fewer than 30 days remain; otherwise changes nothing. Runs before `deltd`, and daily from `deltos-tls.timer` (`docs/box-certificates.md`) | Always |
| 6b | `deltos-deltd` | The box API and pack server, on `home.deltos.lan`, over HTTP and HTTPS with step 6c's certificates | Step 6a's register exists |
| — | `deltos-net-changed.path` | Watches the settings files; when a checked action rewrites one, restarts only the service it belongs to | Always |

- **The generated files live in `/run`, which is empty at every boot.** They are rebuilt from the registry each time, so a crash, an interrupted change or an OS rollback can never leave stale ones. The registry is the only copy that is kept.
- **A box without settings starts nothing.** Each step's condition is the file the step before it writes, so nothing starts on a missing file, nothing loops on a restart, and nothing shows as failed. If the registry exists but is damaged, step 1 fails and the steps that need it do not run: no DHCP is better than wrong DHCP on someone's network.
- **`deltd` starts whether or not there is a hotspot** (a box joined to a school network still serves its library), but only after step 1 when step 1 runs, so its answers to phones' internet checks match what the DNS says (`deltd.env`, box-network-plan §6).
- **Restarts:** every long-running service restarts on failure after 2 seconds, growing to at most 60 seconds over five steps, and stops trying after 10 failures in 10 minutes, ending `failed`. These are starting values, not measured ones. The growing delay needs systemd 254 or newer and is ignored, with a warning, on older systems.

### What a checked change does

`deltos_net::apply` (behind shelld's `authorize`) rewrites only the files whose content changed. The watcher compares each file with the copy last applied and acts on the difference:

| File changed | Then |
|---|---|
| `hostapd.conf` | hostapd restarts (stops, if the hotspot was removed). Phones rejoin on their own. |
| `dnsmasq.conf` | dnsmasq restarts. Approved names and reservations need no restart at all (box-network-plan §4). |
| `uplink.nft`, `hotspot.env` | The firewall reloads; an address change also redoes step 2 and restarts the radio and DNS. |
| `deltd.env` | `deltd` restarts. |

So shelld (not built yet) needs no power to restart anything: writing the registry through `apply` is enough.

## 3. Who runs what

| Program | User | Powers held | Can write | Cannot see |
|---|---|---|---|---|
| `deltd` | `deltos-deltd` | `CAP_NET_BIND_SERVICE` (ports 80 and 443) only | `/var/lib/deltos/b4/` | `/var/lib/deltos/network/`, `/run/deltos/network/`, `/var/lib/deltos/identity/`, `/var/lib/deltos/ca/` |
| `deltos-identityd` | `deltos-identity` | none, and no network (loopback only) | `/var/lib/deltos/identity/`, its socket | `/var/lib/deltos/network/`, `/run/deltos/network/`, `/var/lib/deltos/tls/`, `/var/lib/deltos/ca/` |
| `deltos-tls-provision` | root | `CAP_CHOWN` only (to hand server keys to `deltd`'s group), and no network | `/var/lib/deltos/ca/`, `/var/lib/deltos/tls/` | `/var/lib/deltos/network/`, `/var/lib/deltos/identity/` |
| `dnsmasq` | `deltos-dns` | bind ports 53 and 67, raw sockets, network admin (for DHCP) | its lease file only | `/var/lib/deltos/` |
| `hostapd` | root | `CAP_NET_ADMIN`, `CAP_NET_RAW` only | nothing | — |
| `deltos-net-render` | root | none, and no network at all | the registry's directory and the settings directory | — |
| the short boot steps | root | as a boot script needs | — | — |

- **Users and groups** are made by `system/sysusers.d/deltos.conf`: `deltos-deltd`, `deltos-dns`, and the group `deltos-shellc` with `deltos-deltd` as its only member, so privilege boundary rule 1 holds from the first box even though `deltos-shelld` is not built.
- **Every service is sandboxed** the same way: the system is read-only to it, home folders hidden, its own `/tmp`, no new privileges, no writable-and-executable memory, a narrow list of system calls and address families. `systemd-analyze verify` passes on every unit.
- **Directories** come from `system/tmpfiles.d/deltos.conf`. `/var/lib/deltos/network/` is `0700 root`. `/run/deltos/network/` is `2750 root:deltos-dns`, so every generated file is readable by dnsmasq and by nothing else unprivileged, except `hostapd.conf`, which is `0600 root`.
- **Leases are kept only until shutdown** (they sit in `/run`) and are never backed up (box-network-plan §4: they are personal data).
- **hostapd has no control socket.** Nothing on the box drives it at run time, so there is nothing to attack; a change rewrites its file and restarts it.

## 4. Setting up a new box

Three things, once per box, all of them choices for whoever sets it up:

1. **Network settings,** with `deltos-net-provision <registry> <settings-file>`. It works **only on a box with no settings yet** and refuses ever after, so it is never a way around the checked actions. The file states every value; there are no defaults (format in `crates/deltos-net/src/bin/provision.rs`). Internet sharing always starts off; only an admin turns it on (contract §16). Where the file comes from on an appliance image (for example written by E7's flashing tool onto the boot partition) is E7's to decide; it holds the Wi-Fi password, so E7 should delete it once read.
2. **The first admin,** with `deltos-identity-provision` (`docs/identityd.md` §6), which works only once. `deltd`'s identity source is built in: `deltos-identityd`'s socket. Until there is an admin, neither starts, and neither shows as failed. **The box's certificates need no step:** `deltos-tls` makes them at the first boot (`docs/box-certificates.md`) and prints the fingerprint teachers compare before trusting the box.
3. **Install** with `system/install.sh --bin-dir <built binaries>` on an installable-stack machine (Debian or Ubuntu), or `--destdir <image tree>` when building an appliance image. On a live machine it refuses, and says why, if the distribution's own `dnsmasq` or `hostapd` service is running there: two DNS or DHCP servers on one machine fight, and the person's services are theirs to stop, not ours.

## 5. Decisions made here

Made under Commander's delegation (2026-11-20); each is local to the box and binds no other track.

| Decision | Chosen | Why |
|---|---|---|
| Security | WPA2-Personal, AES (CCMP) only | Every phone in use supports it; TKIP is broken. WPA3 waits for real-handset testing, since its mixed mode trips some older phones. |
| Band | 2.4 GHz, 802.11n | The Pi's built-in radio and every phone support it; it reaches furthest across a room. 5 GHz needs per-country channel rules (DFS) best tested on hardware. |
| Phones see each other | No (`ap_isolate`) | Learners' phones need the box, not one another. |
| Value limits | Name 1–32 bytes, password 8–63 printable ASCII, country two capital letters, channel 1–13 | The 802.11 standard's own limits; channel legality per country is left to the kernel's regulatory database, which refuses an illegal one. |
| Boot files location | `/run/deltos/network/` | Rebuilt every boot from the registry; no new persistent path. |
| Registry location | `/var/lib/deltos/network/registry.sqlite` | box-network-plan §8 item 2's proposal, made concrete here; still a proposal for contract §10. |
| New system user | `deltos-dns` for dnsmasq | Its own user, as the privilege boundary does for `deltd`. |
| Program location | `/usr/lib/deltos/bin/` and `/usr/lib/deltos/libexec/` | Inside the read-only image on the appliance path (Track E §3), off everyone's `PATH`. |
| Registry schema | Version 2 adds the radio; version 1 is upgraded in place | Contract §10's versioning rule. |
| **The WebOS hostname** (was open in contract §13.2) | **`home.deltos.lan`** | Short enough to type on a phone and print on a sign-in card; says what it is, the box's home screen rather than one app (`library` would undersell the WebOS, which also launches services); clashes with none of contract §8's service names. Recorded in contract §8. E10 refuses to approve it for any other device, since it is the one origin that carries people's sessions. |

## 6. How it was tested

`system/tests/vm/run.sh` boots an emulated **ARM64 machine with 2 GB of RAM** (the minimum spec) running Ubuntu 24.04, with two virtual Wi-Fi radios (`mac80211_hwsim`): one is the box's, the other is moved into its own network namespace and acts as a phone with its own Wi-Fi client, DHCP client and browser (`curl`). The first boot installs the box as a person would, checks an unprovisioned box starts nothing and fails nothing, provisions it, and reboots. The second boot is a real boot: the checks run against what came up on its own. They cover every claim in §1 to §3: start order and no failures; users, powers and what each sandbox can see; a wrong password refused; the phone joining, getting an address and the box as DNS; box names resolving and outside names answered "no such name" at once; the box API refusing a phone with no session and answering a signed-in one; search across packs; the phones' internet checks; nothing forwarded while sharing is off; no DNS query logged; dnsmasq, hostapd and `deltd` each killed and coming back; and a real `configure_ap` through `authorize`/`apply` moving the radio to another channel, the phone following, and DNS untouched. Results and measured timings are in the PR.

The unit tests (`crates/deltos-net/tests/radio.rs`) cover the values, the password never printing, the action's checks, the file permissions, the files following the registry, and the schema upgrade.

**Not tested, and why:** real Pi radios (the Broadcom chip's quirks, and running the hotspot beside a Wi-Fi uplink), real Android and iPhone handsets, and 5 GHz. All wait on Commander's hardware hold.

## 7. Not built yet, and who it belongs to

| Item | Owner |
|---|---|
| `join_network`: the box joining another Wi-Fi network as a client, and the scan listing | Track E (E3) |
| The hotspot beside a Wi-Fi uplink on one radio (a second virtual interface), checked at flash time | Track E (E3, E7) |
| Learning upstream DNS servers when the uplink changes, so `upstream_dns` follows it | Track E (E10) |
| A health check that catches a *frozen* `deltd` (systemd's watchdog needs `deltd` to report in) | `deltd`, Track E §23.5 |
| An admin alert before a service reaches `failed` | Track F (F5) |
| The uplink firewall's forward chain drops **all** forwarding on the box, which will also block containers when Track G's orchestration arrives; it must then allow container traffic explicitly | Track G with E10 |
| Where E7's settings file comes from on an appliance image, and deleting it after use | Track E (E7) |
| The appliance image build (E1) using `install.sh --destdir` | Track E (E1) |
