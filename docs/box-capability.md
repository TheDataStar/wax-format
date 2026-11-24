# The box measures itself: the capability record

*Written 2026-11-24 with the build in `crates/deltos-apps` (`src/measure.rs`, the `deltos-capability` program) and `system/systemd/deltos-capability.service`. Builds contract §2.2 and §9's "every box publishes one capability record, written at first boot from its own measurements". Tested in the emulated ARM64 box (§6); **not yet on a real Pi** (Commander's hold, 2026-11-20).*

**What this document owns:** how each field of `/var/lib/deltos/capability.json` is measured, when, and by what. **What it does not own:** the record's fields and path (contract §9, §10), the profile floors (contract §3), and the fit checks that read it (`docs/base-addons-admin.md` §6, `docs/shelld.md` §7.2).

Plain words: the **capability record** is one small file that says what this box's hardware is (memory, drive, processor type, graphics, Wi-Fi, clock) and what the person setting it up chose it to run (the **profile**). Every install asks it "can this box run that?".

---

## 1. In short

- **One program, two uses.** `deltos-capability provision --profile P` once, when the box is set up; `deltos-capability measure` at every boot, from `deltos-capability.service`, before any DeltOS service starts.
- **The profile is chosen, never measured or defaulted** (contract §3). `provision` refuses a profile the box is below, and refuses to run a second time: after setup, changing the profile is an admin action (not built yet).
- **The hardware is measured at every boot**, not only the first. A box's drive can move to other hardware; a record that described the old board would let the fit check install what this one cannot run. If nothing changed, the file is not rewritten.
- **Nothing is guessed.** A reading that cannot be taken fails the command and names the reading. A box with a radio and no `iw` to ask it fails rather than claiming it can or cannot be a hotspot.
- **Before this, nothing wrote the record**, so on a box set up as documented every pack and add-on install was refused "the box has not measured itself yet" (found 2026-11-24, `docs/shelld.md` §6).

## 2. The fields, and how each is measured

| Field | Measured as | Why this way |
|---|---|---|
| `schema` | `1` | Contract §10: state carries a version its owner refuses when newer. A record without one (written by hand or by the old VM stand-in) is read as schema 1. |
| `profile` | The `--profile` given to `provision`; kept by `measure` | Contract §3: chosen, not measured. |
| `ram_bytes` | The kernel's `MemTotal`, **rounded up to a whole GiB** | Firmware, graphics and the kernel always keep a slice before Linux counts: a "2 GB" Pi 4 shows about 1.8 GiB, the 2 GB VM 1,955 MiB. Without rounding no 2 GB board could ever meet contract §2.1's 2 GB minimum spec, and every legacy `pi_4` pack (2 GiB, contract §11) would be refused on the very hardware it was built for. The rounded figure is the size the board is sold as. |
| `os_ram_bytes` | `ram_bytes` − `MemAvailable`, **only while no DeltOS service is running** | Contract §9: "the RAM in use … with no DeltOS service running". Taken from the rounded `ram_bytes` so the slice firmware keeps is counted here: the add-on fit check adds this to the sum of declared floors and compares with `ram_bytes`, and the sum must not count memory Linux never had. If a DeltOS process is running (`deltd`, `deltos-identityd`, `deltos-shelld`, `hostapd`, `dnsmasq`), the previous value is kept and the program says so. |
| `storage_bytes` | The size of the **whole drive** the state tree is on (`/sys/dev/block/M:m`, its parent for a partition, 512-byte sectors). Where the state is on no block device (a container), the size of its filesystem | The profile floors are drive sizes; the free space installs need is measured live by each install, never from this field. |
| `arch` | The kernel's architecture (`/proc/sys/kernel/arch`, else `uname`): `aarch64` or `x86_64` | The kernel's, not this program's build. Any other refuses: contract §2.1 names only these two. |
| `gpu` | `none`, or the kernel driver of each **render node** (`/sys/class/drm/renderD*`), sorted and joined with `,`: `v3d` on a Pi-class board, `i915` or `amdgpu` on a mini PC | A render node is a graphics device that can be asked to compute, as opposed to one that only drives a screen. The driver is what was measured; which drivers are good enough for a feature declaring `gpu: required` is open (§5). |
| `has_wifi` | Any network interface with a `phy80211` link | |
| `can_ap` | `iw phy` lists `AP` among a radio's supported interface modes | The kernel's own list; `false` without a radio, a refusal with a radio and no `iw`. |
| `has_rtc` | Any `/sys/class/rtc/rtc*` | A clock device exists. A board whose clock has no battery still reports one; the certificates' clock floor (`docs/box-certificates.md`) does not depend on this field. |

**Profile floors** (contract §3): `kiosk` and `classroom` need 2 GiB of memory and a 32 GB drive; `community_hub` and `field_ops` 16 GiB and 256 GB. A drive is compared by the size it is sold as: the next power of two in GB at or above its bytes. A "32 GB" card holds 31.1 to 32.0 billion bytes and never more, so its bytes alone would put every such card under a 32 GB floor (the same for a 250 GB drive against 256 GB). The record keeps the true bytes; only the floor comparison uses the size class.

A box re-measured below its profile's floor (its drive moved to a smaller board) is recorded as it is, with a warning in the boot log. The record says what the box is; the fit checks then refuse what it cannot run.

## 3. How it runs

- **At setup**, step 0 of `system/install.sh`'s list, before the network settings: `/usr/lib/deltos/bin/deltos-capability provision --profile classroom` (for example). On an appliance image the profile may be pre-seeded by the flashing tool (E7), which runs the same command; where the choice comes from is E7's.
- **At every boot**, `deltos-capability.service` (oneshot, part of `deltos-box.target`): after the local filesystems, before `hostapd`, `dnsmasq`, `identityd`, `shelld` and `deltd`, so `os_ram_bytes` is read with none of them running. Skipped, not failed, on a box nobody has set up (no record yet).
- **Root, with three powers only:** seeing every process (to know none of ours runs), reading what root reads, and asking the radio what it supports (netlink). No network, the system read-only except `/var/lib/deltos`, and the network registry, the people register and the certificate folders hidden from it.
- **Written whole or not at all:** a new file beside the old, flushed, renamed over it. `0644 root`: every local component may read what the box is (contract §9).
- **Backups do not carry it** (`docs/box-backup.md` §3), and a restore leaves it in place: it describes this hardware, not anything a person made. Checked in the VM run.

## 4. Decisions made here

Made under Commander's delegation (2026-11-20); recorded as proposals in contract §9.

| Decision | Chosen | Why |
|---|---|---|
| When to measure | Every boot, not only the first | Contract §2.2 says first boot; measuring again changes nothing on unchanged hardware and stops a moved drive carrying the old board's numbers. |
| RAM | `MemTotal` rounded up to a whole GiB | §2 above: without it no 2 GB board meets the 2 GB minimum spec. |
| `os_ram_bytes` | Installed RAM less `MemAvailable`, before any DeltOS service | Counts what firmware keeps, so the add-on RAM sum is honest. |
| Storage | The whole drive; floor by size class | §2 above: no "32 GB" card holds 32 billion bytes. |
| GPU | Render-node drivers, or `none` | A measurement, not a class list nobody has defined (§5). |
| Schema number | `schema: 1` in the record | Contract §10's rule; readers that do not know it ignore it. |
| Program home | `deltos-apps`, beside the fit check that reads the record | The record's type already lives there (`fit::Capability`). |

## 5. Open, and found

- **What counts as a GPU for `gpu: required`** is undefined. Contract §9 says `gpu` is "`none` or the class of accelerator measured" and names no classes. Today any render node satisfies `required` in both fit checks, so a Pi's `v3d` would admit a feature that really needs a large accelerator. Nothing declares `gpu: required` yet (the media hub declares `preferred`, which never gates). Raised in contract §13.2.
- **Found, pre-existing: legacy packs need 32 GiB free beyond the archive.** Contract §11's read-time mapping gives a legacy `pi_4` pack `min_storage_bytes` = 32 GiB, the minimum spec's whole drive, and the pack fit check compares that with free space. A 32 GB card can never have 32 GiB free, so no legacy `pi_4` pack can install on the minimum spec. The mapping is contract §11's (Directive 03a), so it is reported, not changed here. Raised in contract §13.2.
- **The profile cannot be changed after setup.** That is an admin action through shelld (with a re-check against the new floor), not built.

## 6. How it was tested

`crates/deltos-apps/tests/measure.rs` (5 tests) and 3 unit tests build a made-up `/proc` and `/sys` for a Pi-class board (2 GB, a 31.9-billion-byte card, a radio, a `v3d` render node) and a mini PC (16 GB, no render node): every field; the floor refusing Community Hub on 2 GB and writing nothing; the second `provision` refused; `measure` keeping the profile, not rewriting an unchanged record, keeping `os_ram_bytes` while a service runs, and recording a smaller board with a warning; a radio without `iw`, an unknown architecture, a missing `/proc/meminfo` and a newer schema all refused; the record read back as shelld reads it.

The emulated ARM64 box (`system/tests/vm/`) now provisions the record instead of writing a stand-in: Community Hub refused on 2 GB, Classroom accepted, a second provision refused; at the second boot the unit ran before every DeltOS service, every field matches the machine, `os_ram_bytes` was read with nothing running and kept when measured again with everything running, the record survived the backup restore, and the pack installs passed on the real record. Results: `docs/shelld.md` §8 and the PR.
