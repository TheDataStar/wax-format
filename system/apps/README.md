# Base DeltOS app manifests

One `deltos-app.toml`-format file per **base** service: the part of DeltOS
every box has, installed with the system and never removed by the add-on
store. Format: `docs/base-addons-admin.md` §5 (contract §15). Checked by
`deltos-apps check system/apps/*.toml` and by the `deltos-apps` tests.

Installed to `/usr/lib/deltos/apps/` by the system image; the fit check reads
them together with installed add-ons to make the RAM budget (contract §6:
the sum of declared floors).

| File | Service | Floor evidence |
|---|---|---|
| `home.toml` | `deltd`: the WebOS address, box API and pack server | provisional (x86_64 dev container, 2026-11-22) |
| `identity.toml` | `deltos-identityd`: people, roles, sessions | provisional (x86_64 dev container, 2026-11-22) |
| `dns-dhcp.toml` | `dnsmasq`: the box's names and addresses | **open** |
| `hotspot.toml` | `hostapd`: the box's own Wi-Fi | **open** |

`deltos-tls-provision` (certificates) and `deltos-backup` are tools that run
and exit, not services, so they have no manifest: nothing of theirs stays in
memory.

An **open** floor is not a zero: until it is measured, every fit check on
the box answers "cannot tell" for RAM. Measuring these two on the minimum
spec (aarch64, 2 GB) is the next step (`docs/base-addons-admin.md` §11).
