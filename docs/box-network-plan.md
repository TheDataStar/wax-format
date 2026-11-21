# The box's own network: names, addresses and DHCP

*Written 2026-11-20 alongside the first `deltd` build (`crates/deltlib`). **Built the same day in `crates/deltos-net`** (build order steps 1 and 2, §9): the registry, the generated `dnsmasq` and `nftables` settings, the checked actions, and `deltd`'s answers to phones' internet checks, tested against a real `dnsmasq` on a virtual network. Answers the question "must the box host its own DNS and DHCP, and does it need an IPAM tool?" against contract §8 and §16, Track E §6 and §14, and `docs/privilege-boundary.md`.*

**What this document owns:** how a phone on the box's network finds the box's names, gets an address, and why; and what the box tracks about names and addresses. **What it does not own:** the hostname scheme (contract §8), the Wi-Fi radio and captive-portal behaviour (Track E §6, E3), and who may change any of it (`docs/privilege-boundary.md`). Contract values proposed here are marked **proposed** and are not binding until contract §8 or §13.2 says so.

Words used below, in plain terms: **DNS** turns a name like `library.deltos.lan` into the box's address. **DHCP** hands a phone an address when it joins a Wi-Fi network, and tells it which DNS server to ask. **IPAM** ("IP address management") is keeping a list of which address and name belongs to what.

---

## 1. In short

- **Yes, the box must answer DNS itself, on every network it serves.** Every DeltOS surface is a *name*, and most of those names are created on the fly: each person gets their own pack hostnames on each browser (contract §8). Nothing but the box can answer for names that did not exist a minute ago.
- **Yes, the box must run DHCP on its own Wi-Fi** (the hotspot), which is the normal way DeltOS is used. Without it a phone joining the hotspot has no address and does not know to ask the box for names.
- **No, it must never run DHCP on someone else's network** (a school's LAN). Two DHCP servers on one network break it for everyone.
- **No separate IPAM product.** What needs tracking is small: the box's own addresses, the hotspot's address pool and its leases, a few fixed reservations, and the names an admin approves (Track E §23.3). One small table owned by E10 covers it. NetBox or phpIPAM would need a database server and more memory than the whole box budget allows at the 2 GB floor.
- **Recommended software: `dnsmasq` for DNS and DHCP, Avahi for `.local` discovery.** This replaces Track E §14's CoreDNS recommendation, which predates the DHCP need (§5). Flagged as a Track E amendment, not applied here.

## 2. Why the box's names need the box's DNS

Contract §8 puts every surface on its own hostname:

| Surface | Name | Changes |
|---|---|---|
| WebOS and API | `<webos-host>.deltos.lan` (name still open, contract §13.2) | Fixed |
| Service surfaces | `<service>.deltos.lan` | When an app is installed, or a name is approved (Track E §23.3) |
| Pack content | `pack-<index>-g<generation>-<partition>.deltos-packs.lan` | **A new partition for every person on every browser** (contract §8) |

So:

1. **Only a wildcard answer works.** The box answers *every* name under `deltos.lan` and `deltos-packs.lan` with its own address. `deltd` then refuses any name it did not issue (deltd API §4.4, privilege boundary rule 16), so the wildcard grants nothing by itself.
2. **mDNS cannot do this.** `.local` names are a different zone (Track E §14), and mDNS has no wildcard. Avahi stays, for what it is for: boxes finding each other.
3. **Serving by IP address would break pack isolation.** Every pack would share one origin, so one pack could read another's storage. The names are the security boundary, and the names need DNS.
4. **Everything else, when there is no internet uplink, is answered "no such name" at once.** A phone then fails fast instead of waiting on a lookup that will never return. With an uplink, other names are forwarded upstream as normal.

## 3. The two ways a box meets phones

### 3.1 The box's own Wi-Fi (the default)

The box runs the hotspot (Track E §6), DHCP and DNS. Each phone that joins gets an address from the box's pool, with the box as its DNS server.

- **Hotspot subnet: proposed `10.42.0.0/24`, box at `10.42.0.1`.** This is the address NetworkManager's own hotspot mode uses, so it is what a stock Linux image already does. A `10.` range is also less likely to clash with a home or school uplink, which is usually `192.168.x.x`. **Proposed for contract §8;** no code depends on it.
- **The box has an internet uplink as well** (Track E §6: concurrent AP and client mode). Phones on the hotspot still use the box's DNS; the box forwards non-box names upstream. Whether phones also get internet *through* the box is a separate, admin-set choice and off by default (contract §16: nothing leaves the box unless an admin set it up).

### 3.2 The box joined to an existing network (school Wi-Fi or wired LAN)

Here the school's router hands out addresses and answers DNS, and it knows nothing about `deltos.lan`. The box must not start a second DHCP server. Three ways to make the names work, in order of preference:

1. **Keep the box's own hotspot running beside the school network** (recommended; needs a radio that supports both at once, which E7 checks at flash time, Track E §6). Learners join the box's Wi-Fi; the school network is the uplink.
2. **The school's IT adds a forwarding rule:** "send `deltos.lan` and `deltos-packs.lan` to the box." The box shows the admin the exact two rules and the address to use. The box also needs a fixed address there: a reservation on the school's router, which the admin asks their IT for.
3. **Neither is possible:** the box says so plainly in the admin console, rather than half working.

## 4. What the box tracks (the "IPAM" part)

One table, owned by E10, under `/var/lib/deltos/` (path **proposed** for contract §10: `/var/lib/deltos/network/`), carrying a schema version like all state there (contract §10):

| Kept | Why |
|---|---|
| The box's own addresses on each network | The wildcard answers point at them |
| The hotspot's pool and current leases (device address, the device's own name, when it expires) | So two devices never get the same address, and an admin can see what is connected |
| Fixed reservations | A second box in a fleet, a printer, a projector |
| Approved names (Track E §23.3) | A person requests a name, an admin approves it, it resolves at once without restarting anything |

`dnsmasq` reads approved names from a directory it watches, so an approval takes effect without a restart, which is exactly Track E §23.3's requirement.

**Leases are personal data.** A lease ties a device to an address, and the pack hostnames a device asks for name a person's partition. So: lease lists are shown only to an admin over HTTPS (privilege boundary §4.1), **DNS queries are never logged** (contract §16, no telemetry), and leases are left out of backups.

## 5. Software

| Need | Recommended | Why |
|---|---|---|
| DNS (box names, wildcard, forwarding) | `dnsmasq` | One small process for DNS and DHCP. It is what most Linux hotspots already run. It supports wildcard answers for a whole zone, watches a directory for new names (no restart), and hands out the DNS server and the captive-portal address (RFC 8910) in DHCP. |
| DHCP on the hotspot | `dnsmasq` (same process) | CoreDNS, which Track E §14 recommended for DNS, does no DHCP at all, so it would need a second server (such as Kea) beside it. One small process beats two larger ones at the 2 GB floor. |
| `.local` discovery between boxes | Avahi | Unchanged from Track E §14. |

**Not measured yet:** the memory of `dnsmasq` against CoreDNS plus Kea on the minimum spec. The recommendation rests on dnsmasq doing both jobs in one process; the figures go in `docs/baseline.md` when measured, and they are the evidence the Track E amendment needs.

## 6. Phones and the "no internet" problem

When a phone joins Wi-Fi it checks whether the internet works by fetching a known address (Android: `connectivitycheck.gstatic.com/generate_204`; Apple: `captive.apple.com/hotspot-detect.html`; Windows: `www.msftconnecttest.com/connecttest.txt`). On an offline box these checks fail, and phones react in ways that matter here:

- **Android may send traffic over mobile data instead of the Wi-Fi.** Its name lookups then go to the phone carrier, which knows nothing about `deltos.lan`, and the box "disappears".
- **iPhones open a small sign-in window** if the check gets an unexpected answer. That window is a separate browser with its own cookies, so a sign-in made there is not the phone's real browser (privilege boundary §4.4 gives each browser its own partition).
- **Phones that set their own DNS** (Android "Private DNS" set to a named provider, some VPNs) never ask the box at all.

**Recommendation for E3:** the box's DNS answers those check addresses with the box's own address, and the box answers each check as "internet works" (a `204` for Android, `Success` for Apple, the expected text for Windows). Then phones keep using the Wi-Fi and the box's DNS, and no stray sign-in window opens. People reach the library by its address or by scanning the QR code on the box's screen or on their sign-in card. **This must be tested on real Android and iPhone handsets before it is relied on** (that testing is deferred, per Commander, 2026-11-20).

**Where this lands in the server:** at the minimum profile there is no reverse proxy, so `deltd` owns port 80 and must answer those check addresses itself. Privilege boundary rule 16 says `deltd` answers only names it serves, so the check names and their exact paths become an explicit, closed list in `deltd`: those paths only, `GET` only, no cookies, no content. **Follow-up for `deltd`;** not in this first build.

## 7. Who may change the network

Every change below is a `deltos-shelld` action behind `network.configure`, **Secure** (HTTPS and a session made over HTTPS), checked twice (privilege boundary §5). `join_network` and `configure_ap` already exist (deltd API §6.8). This plan needs three more, **proposed** for the privilege boundary's closed action list:

| Proposed action | Does | Re-confirm |
|---|---|---|
| `approve_name` | Adds a requested name to the registry (Track E §23.3) | No |
| `set_address_reservation` | Adds or removes a fixed address on the hotspot | No |
| `set_uplink_sharing` | Turns internet sharing for hotspot devices on or off (§3.1) | Yes: it changes what leaves the box |

And one listing: `GET /network/leases`, `network.configure`, Secure.

## 8. Open, and who decides

| # | Question | Recommendation | Owner |
|---|---|---|---|
| 1 | Hotspot subnet | `10.42.0.0/24`, box `.1` (§3.1) | Contract §8 |
| 2 | Network state path | `/var/lib/deltos/network/` (§4) | Contract §10 |
| 3 | DNS and DHCP software | `dnsmasq` + Avahi, replacing CoreDNS (§5), once the memory figures are measured | Track E §14 |
| 4 | How phones' internet checks are answered | "Internet works", tested on real handsets first (§6) | Track E §6 (E3) |
| 5 | The three new actions (§7) | Add to the closed list | `docs/privilege-boundary.md` |
| 6 | The WebOS hostname | Still open; both DNS and `deltd` need it | Contract §13.2 |

## 9. Build order

1. ~~**E10 network registry and `dnsmasq` configuration generator**~~ **Built** (`crates/deltos-net`): the registry, the wildcard records, the hotspot DHCP settings, the watched names and reservations directories, the uplink firewall file, and the §7 actions behind shelld's checks. Tested without a radio: a real `dnsmasq` on a virtual ethernet pair, with a DHCP client in its own network namespace standing in for a phone.
2. ~~**`deltd`'s connectivity-check answers**~~ **Built** (`deltd --answer-connectivity-checks`), answering only the listed paths, `GET`/`HEAD` only, no cookie.
   **Built since (2026-11-21, `docs/box-radio-and-boot.md`):** the hotspot radio (`configure_ap`) and the boot units that run `deltos-net-render`, the firewall, `hostapd`, `dnsmasq` and `deltd`. Not built yet: `join_network` and the `deltos-shelld` socket that calls the actions.
3. **Admin endpoints** for §7, built with the rest of deltd API step 4.
4. **Real handset testing** of §3 and §6, when Commander starts hardware testing.
