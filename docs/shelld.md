# deltos-shelld — the Key Holder, and Add-on Installs

_First build 2026-11-23 (stage 3 of `docs/base-addons-admin.md` §11); pack
installs added 2026-11-23 (§7)._
Implements `docs/privilege-boundary.md` §3, §5 steps 6–7, §6 step 3, rules
1–6, 10, 12 and 18, the add-on half of `deltos-installd` (F4,
`docs/base-addons-admin.md` §7) and its pack half (Track F §4, §7 here).
Code: `crates/deltos-shelld`.

## 1. In plain words

`deltos-shelld` is the only program on the box allowed to change it. The
box API (`deltd`) faces the network and holds no powers; when a person asks
to change something, `deltd` passes the request to shelld over a local
socket. Shelld does not take `deltd`'s word for anything. It asks the
sign-in service (`identityd`) itself whether the session is live, was made
over HTTPS, and holds the permission, and for anything that cannot be undone
it redeems the one-use password ticket the person earned by typing their
password again. Only then does it act, and every request, allowed or refused,
goes into a log the box keeps.

It knows a fixed list of actions and nothing else. There is no "run this
command".

## 2. How it runs

| | |
|---|---|
| User | **root** (privilege boundary §3: "privileged"). It writes the network registry (root's alone), the add-on folders and, on a restore, the whole state tree. |
| Door | One Unix socket, `/run/deltos/shelld/shelld.sock`, mode `0660`, in a folder `2750 root:deltos-shellc`. `deltos-deltd` is the group's only member (rule 1). `PrivateNetwork=yes`: no TCP port, no UDP port, only loopback. |
| Callers | Every connection's user id is read from the kernel (`SO_PEERCRED`). Only `deltos-deltd` is served; anyone else is disconnected before a byte is read (rule 2). |
| Its own questions | To `identityd`, which recognises shelld by its user id. **Decided 2026-11-23:** since shelld runs as root, `identityd` runs with `--shelld-user root`. Root may then make only shelld's three calls (`session`, `has_permission`, `redeem_ticket`); a root process could read the register's files directly anyway (rule 2: root already owns the box), so nothing is given away. |
| Unit | `system/systemd/deltos-shelld.service`: after `identityd`, before `deltd`, started by `deltos-box.target`, waits for the first admin like `identityd`. |

**One worker.** Every action that passed its checks runs on one thread, one
at a time, in the order checked. Two actions never change the same register
at once, and installs run one at a time (Track F §4). Checks run on each
connection's own thread, so a long install never delays a refusal.

## 3. The socket's calls

One JSON object per line each way, the same wire format as `identityd`
(`{"ok": ...}` or `{"error": {"code", "detail"}}`, codes from deltd API §7).

| Call | Arguments | Answers |
|---|---|---|
| `describe` | `action`, `args` | The permission, whether it asks again, the argument digest a ticket is bound to, and whether it is long. Changes nothing, needs no session. |
| `act` | `action`, `args`, `session`, `over_https`, `ticket` (if it asks again) | A short action: its result. A long one: `{"job": id}` at once. |
| `job` | `job`, `session`, `over_https` | The job's state. Only the person who started it can read it; to anyone else it does not exist. |
| `list` | `listing`, `session`, `over_https` | `addons`, `addon_sources` (`apps.manage`), `packs`, `pack_sources` (`content.install`), `drives` (`backup.manage`), `leases` (`network.configure`), `action_log` (`admin.console`, the newest 200 entries), `actions` (the list itself, no session). |

**How `deltd` will use a re-confirmed action** (privilege boundary §6): it
calls `describe` with the person's exact arguments, has `identityd` issue a
ticket for that action and digest after the person types their password
(`issue_ticket`), then sends `act` with the ticket. Shelld computes the
digest again itself and `identityd` checks the two match, once, within the
minute. The digest is computed in one place (each action's own crate), so
the two sides can never disagree on the bytes.

`over_https` is `deltd`'s statement of how the request reached the box.
Shelld cannot see the network, so it cannot check this itself; it checks
that the **session** was made over HTTPS, which `identityd` records and no
network request can change. A buggy `deltd` that reports plain HTTP as HTTPS
can still only act for a session that was made over HTTPS by someone who
holds the permission (privilege boundary §11).

**Jobs** (contract §7.2): `queued`, then the action's own states (`fetching`,
`verifying`, `installing`, `installed` for add-ons; `running` for backups),
ending in `done`, `failed_signature` or `failed_rollback` (both terminal,
never retried: contract §7.2) or `failed` with the error. Kept in memory, the newest 64 finished ones; a restart of
shelld forgets them (the log keeps the outcome).

### 3.1 The list as built

| Built | Not built yet (refused by name: "on the list but not built yet") |
|---|---|
| `approve_name`, `remove_name`, `set_address_reservation`, `remove_address_reservation`, `set_uplink_sharing`, `configure_ap` (network, `deltos_net::actions`) | |
| `start_backup`, `review_backup`, `restore_backup` (`deltos_backup::actions`) | `mount_removable`, `eject_removable`, `convert_zim_from_drive`, `stage_upload`, `join_network`, `restart_service`, `pair_box`, `apply_update`, `roll_back_update` |
| `install_addon`, `update_addon`, `roll_back_addon`, `remove_addon`, `set_addon_running` (§4) | `power_box`, `set_clock`, `set_language`, `set_home_screen`, `save_support_file`, `export_activity` (stage 4, with the dashboard) |
| `install_pack`, `remove_pack`, `roll_back_pack` (§7) | |

Each family keeps its own type, checks and `Authorized` in the crate that
owns what it changes: nothing in those crates changes the box except through
a function that takes an `Authorized`, which only that crate's `authorize`
makes. Shelld reads the request into the family's fixed shape
(`deny_unknown_fields`: an extra argument is an error, never ignored) and
hands it over.

## 4. Add-ons

### 4.1 The pipeline

`install_addon` and `update_addon` take a **source id** from the
`addon_sources` listing (`<origin>/<file>.deltos-addon`; a path is refused
before anything is read, rule 6), and the id and version the admin chose.

1. **Fetching.** The bundle and its `.minisig` beside it are **copied** into
   root's staging folder (`/usr/lib/deltos/apps/.staging/<random>/`) before
   anything reads them, so a file swapped on the drive after the check cannot
   be the one installed. At least twice the bundle's size must be free.
2. **Verifying.** The copy's minisign signature is checked against the
   add-on key (§4.3), in shelld, with a small verify-only library (no program
   is started on untrusted input). A failure ends the job `failed_signature`.
3. **Installing.** The bundle (a tar file) is unpacked with **plain files and
   folders only**: a path outside the bundle, a link, a device file or a
   name given twice is refused, every file is written `0644` and every
   folder `0755` whatever the archive says (so nothing set-user-id), at most
   10,000 entries. The manifest must be an add-on, must name the id and
   version the admin chose, and the licence, the compose file and the icon
   it names must be there.
4. **Fit check** (`docs/base-addons-admin.md` §6) against the base manifests
   and every installed add-on (the one being updated left out). **Does not
   fit** ends the job `does_not_fit` with each reason. **Cannot tell** ends it
   `confirmation_required` unless the request said `accept_unmeasured`, which
   makes the action ask for the password again (§4.4).
5. **Placed** by renaming into `/usr/lib/deltos/apps/<id>/<version>/`, its
   data folder `/var/lib/deltos/apps/<id>/` made (`0750`) if new, then
   **registered** in `/var/lib/deltos/apps/installed.json`, written whole,
   synced and renamed into place. A crash before the register is written
   leaves files no register names, cleaned up by the next install.

`update_addon` needs a newer version; the previous one is kept for
`roll_back_addon` and the one before that removed (G1 keeps two).
`remove_addon` always asks again, refuses while another installed add-on
depends on it, takes it out of the register first and then removes its
files, and its data only if the admin chose to delete it.
`set_addon_running` (`service.restart`) records whether the admin wants it
running.

**Starting and stopping is not built.** That is G1, the platform add-on
(stage 5). Until then `installed.json` records `wanted_running`, and G1 will
act on it. The add-on's permissions joining `identityd`'s catalogue, its
address from G2 and its health checks wait for the same pieces.

**Dependencies one at a time.** The fit check refuses a compose add-on
until `platform` is installed, so the dashboard installs the platform
first, then the add-on, as two actions. The design's "with the dependencies
the fit check named" in one action can come with the store (stage 6); it
needs a multi-bundle source the store does not have yet.

### 4.2 Where things go

| Path | Holds |
|---|---|
| `/usr/lib/deltos/apps/<id>.toml` | Base manifests, from the system image (unchanged). |
| `/usr/lib/deltos/apps/<id>/<version>/` | **[Decided 2026-11-23]** An add-on's unpacked bundle: its manifest, compose file, icon, licence and image archives. Outside the state tree, so backups never carry image archives (they are reinstalled from their bundles). |
| `/usr/lib/deltos/apps/.staging/` | Installs in progress, on the same filesystem, so placing is a rename. |
| `/var/lib/deltos/apps/<id>/` | The add-on's own data (contract §10, proposed 2026-11-22). |
| `/var/lib/deltos/apps/installed.json` | **[Decided 2026-11-23]** The register of installed add-ons. Inside the state tree, so a backup carries it and a restore knows which add-ons to put back. (App ids cannot hold a dot, so it never collides with an add-on's folder.) |

### 4.3 The add-on key

Bundles must be signed with the project's add-on key (`--addon-key`, a
minisign public key file). **There is no project add-on key yet**: nobody
has made one, and its custody is the project's to decide with the store
(stage 6). Until the system image ships one in `/usr/lib/deltos/shelld.env`,
**every add-on install is refused** with `failed_signature`, never let
through unsigned. How a box trusts a bundle not signed by the project (a
school's own app) is open (`docs/base-addons-admin.md` §10).

### 4.4 An add-on nobody has measured

`docs/base-addons-admin.md` §5.3 lets an admin install an add-on whose floor
is open, after a password re-confirmation. A ticket must be bound to the
request before shelld has read the bundle, so the request carries
`accept_unmeasured`: false, and a "cannot tell" install fails
`confirmation_required` with the reason; true, and the action asks for the
password before it starts. **Stricter than §5.3, decided 2026-11-23:** it
asks whenever the answer is "cannot tell", which includes a box whose own
operating system has never been measured, not only an add-on with an open
floor. Either way the box may run short of memory, and the admin should know.

## 5. The action log

`/var/lib/deltos/shelld/actions.log` (**decided 2026-11-23**, contract §10),
one JSON line per request: `at` (Unix seconds), `by` (profile id, or
`unknown` when no live session was given), `action`, `args` (a summary with
every secret as `[redacted]`: Wi-Fi passwords, backup passphrases; ticket
passwords never reach shelld at all), `result` (`ok`, `started` for a job,
or the refusal or failure code), `detail` and `job`. Refusals are logged as
well as successes. Each line is appended and synced before the answer goes
back; if the log cannot be written, the action reports `internal`.

The file is opened for each entry, not held, because a restore replaces the
state tree and the next entry must go to the file now in place. It is
carried by a backup like the rest of the state tree and never otherwise
leaves the box (contract §16); `export_activity` (stage 4) will write it to
a drive on an admin's request.

## 6. What waits, and why

- **Packs from the catalog, a peer or an upload.** `install_pack` is built
  for packs on a drive (§7); the catalog's download folder (B4/B13), peers
  (D8) and uploads (`stage_upload`, whose trust is open) add origins to the
  same listing when they exist.
- **`mount_removable`**: rule 19 allows only "the filesystem types Track E
  names", and Track E names none. Until it does, shelld lists any mount point
  under `/run/deltos/drives/` as a drive (root at the console can mount one
  there), and backups and add-on listings work through that.
- **`convert_zim_from_drive`, `stage_upload`**: how a box-converted or
  uploaded pack is trusted is open (privilege boundary §14).
- **`join_network`**: needs the Wi-Fi scan listing (E3 client mode).
- **`restart_service`**: needs G1 for add-ons; base services restart on
  failure on their own.
- **The internal list** (§9): its callers (`identityd` on kiosk sign-out,
  `deltd`'s catalogue, the health daemon) do not call shelld yet.
- **`deltd`'s side**: the endpoints that pass requests to shelld (deltd API
  §6, privilege boundary §5 step 5) come with the dashboard (stage 4);
  `deltos_shelld::client::Client` is what `deltd` will hold.

## 7. Packs

### 7.1 In plain words

A **pack** is one `.wax` file (SPEC §1): a library of pages. Installing one
means checking that the project signed it, that it is not an older copy of a
pack the box already had, and that the box can run it, and then putting it
where `deltd` serves packs from. `deltd` notices the new file within a few
seconds (it looks every 5 seconds), gives it its own web address and starts
serving it; nothing restarts.

The risky part is **reading** the pack. Its index is made of SQLite
databases written by somebody else, and a database crafted to break SQLite
could take over whatever program opens it. So the key holder never opens
one. It starts a small helper, `deltos-pack-check`, as its own user
(`deltos-packcheck`) with no network and no powers, and the helper says
where the databases lie in the file and what the manifest says. Shelld then
checks what it was told: it reads the 128-byte header itself, hashes the
header and those byte ranges itself, and checks the publisher's signature
over that hash itself. Ranges that are not the ones the publisher signed
give a hash the signature does not cover, and the pack is refused.

### 7.2 The pipeline

`install_pack` takes a **source id** from the `pack_sources` listing
(`<origin>/<file>.wax`; a path is refused, rule 6) and the `archive_uuid`
the admin chose, in its canonical form only (contract §11). It is a job:

1. **Fetching.** At least the pack's size must be free. The pack and its
   `.minisig` are **copied** into the install's own folder under
   `/var/lib/deltos/packs/.staging/<random>/` before anything reads them.
   `.staging` is `0711` (the helper can pass through, not list); the
   install's folder is `0750 root:deltos-packcheck`, so the helper can read
   that one pack and nothing else of root's.
2. **Verifying.** Shelld reads the header and refuses a file that is not
   the pack chosen. The helper (§7.1) answers with the segment ranges and
   the manifest. Shelld checks the ranges are in order, apart, inside the
   file, after the header, at most 64 (SPEC §5.1), and that the last is
   where the header points; computes the SPEC §8.1 digest; and checks the
   minisign signature over it with the pack key (§7.3), prehashed only
   (SPEC §8.2's `-H`), and that its trusted comment names this archive
   (SPEC §8.3). Any failure ends the job `failed_signature`. Then the
   **older-edition check** (§7.4): an older edition ends it `failed_rollback`.
3. **Installing.** The **fit check** is contract §2.3's comparison rule
   against the capability record (§9): `min_ram_bytes` against `ram_bytes`,
   `arch` (`any` or the box's own), `gpu: required` against a box with none,
   and storage. A retired `min_hw_tier` maps to its own floor (contract §11)
   and one that cannot be mapped is refused. **Storage, decided
   2026-11-23:** what is free with the staged copy already counted must
   cover the larger of `min_storage_bytes` and `runtime_storage_bytes`, not
   their sum: both are defined as "beyond the archive itself", so adding
   them would count the same need twice; the larger is the one that must
   fit. Every pack in `depends_on` must be installed. A box already holding
   512 packs refuses a new one (contract §8: `pack_index` 0–511). Each
   reason is named; the code is `not_enough_resources`, or
   `not_enough_storage` when storage is all that is short.
4. **Placed.** The new signature is renamed beside the pack's place first,
   then the pack: `/var/lib/deltos/packs/<archive_uuid>.wax` (`0644`, root),
   or, for an update, the file name the installed edition already has, so
   `deltd` sees an updated pack and keeps its hostname and its readers'
   in-pack storage (contract §8). For an update the old edition's signature
   is kept first as `<file>.minisig.previous` (§7.5). Its writable storage
   `/var/lib/deltos/packs-data/<archive_uuid>/` (contract §10) is made
   `0750`, owned by `deltd`'s user, if new.
5. **Registered** in `/var/lib/deltos/shelld/packs.json` (§7.4).

An install of the edition already installed is refused; an update must be
newer than the installed edition, by version or, at the same version, by
the header's `created_at`.

`remove_pack` always asks for the password again (privilege boundary §8),
refuses while another pack shelld installed lists it in `depends_on`,
removes the file, both signatures and the pack's storage, and keeps the
newest edition seen. `deltd`'s next scan stops serving it and frees its
`pack_index` for a later install at a higher generation: that is
`retire_pack_hostnames` (privilege boundary §9), which `deltd`'s catalog
already performs on every scan, so nothing calls it from shelld.

### 7.3 The pack key

Packs must be signed with the project's pack key (`--pack-key`, a minisign
public key file; Track F §8, "the project key"). **No such key exists yet**,
for the same reason as the add-on key (§4.3): its custody is the project's
to decide. Until the system image ships one in `/usr/lib/deltos/shelld.env`,
**every pack install is refused** with `failed_signature`, never let through
unsigned. Track F §8 describes one project keypair; the box takes the two
keys separately so either custody can be chosen, and the image may point
both flags at the same file. Packs baked into the image (Track F §4,
"starter packs") are in place before the first boot and need no install.

### 7.4 An older edition is refused (Track A §6)

A validly signed older edition of a pack still verifies (SPEC §8.4), so a
box that accepted any signed pack could be walked back to old content.
Track A §6 asks each box to remember the highest edition it has seen and
refuse anything older for the same `archive_uuid`. **Decided 2026-11-23:**
shelld remembers both the manifest's `version` (CalVer, contract §11) and
the header's `created_at` (raised by every append, SPEC §2), and refuses an
edition older in either. The `created_at` half is read by shelld itself, so
a helper that misreports the version still cannot slip an old edition in.
The record is `newest_seen` in **`/var/lib/deltos/shelld/packs.json`
(decided 2026-11-23**, beside the action log, contract §10), kept after a
pack is removed, carried by a backup like the rest of the state tree, and
never lowered by `roll_back_pack`: the admin chose that, and the newer
edition may come back.

### 7.5 Rolling back an update

`roll_back_pack` undoes a pack's last update in place (SPEC §7.5): the
result is byte for byte the earlier edition, so that edition's own kept
signature verifies again. Shelld does not open the pack for this either.
The helper works out the plan (`wax_core::rollback_plan`: the earlier
header, where the file ends, the earlier segments' ranges, checked against
the newest segment's `prev_state_digest`); shelld hashes the earlier header
and ranges itself and checks them against the **kept** signature
`<file>.minisig.previous` before writing a byte, then applies the plan
(`wax_core::apply_rollback`, which touches only bytes: the header, then the
cut, then the v0.13 mirror) and puts that signature back in place. One step
is kept: a second rollback has no earlier signature to check and is refused
`edition_mismatch`, as is a pack whose last update was not an appended
edition. `deltd` sees the header change on its next scan; a page request in
flight during those few seconds can fail once, as during any update.

### 7.6 What the helper can and cannot do

The helper reads one file and prints one answer of at most 1 MiB. It runs
as `deltos-packcheck`, its uid and group are checked after the switch and
before it starts (any other group left and it does not run), it inherits
shelld's lack of network, and it can read only its install's folder. What
a helper taken over by a crafted pack could still do, recorded rather than
hidden:

- **Misreport the manifest** (name, version, floors, `depends_on`). The
  version half of §7.4 is backed by `created_at`, which shelld reads
  itself; the fit check could be fooled into installing a pack the box
  cannot run, which harms only that pack's readers.
- **Misreport where earlier segments lie** in a multi-segment pack whose
  signed bytes it rearranged. Shelld checks every range against the file
  and the newest one against the header, and the signature covers what is
  hashed, but where the newest segment says its predecessor is cannot be
  checked without opening it. The result would be a pack whose earlier
  segment `deltd` reads is not the signed one. `deltd` runs unprivileged
  and serves pack content as untrusted anyway (contract §8), so nothing
  gains a power; closing it fully needs a pack format whose segment
  pointers live outside SQLite, a SPEC question.

### 7.7 Decided under delegation, 2026-11-23

- the register path `/var/lib/deltos/shelld/packs.json` (§7.4);
- the helper's user `deltos-packcheck`, and its binary
  `/usr/lib/deltos/bin/deltos-pack-check` (privilege boundary §3 names a
  user per helper and no names);
- an older edition is judged by version **and** `created_at` (§7.4);
- storage is the larger of the two "beyond the archive" floors (§7.2);
- `roll_back_pack` is short, not a job: it writes a header and cuts the
  file, whatever the pack's size;
- new API code `edition_older` (409, deltd API §7): an older edition than
  the box has had; the job ends `failed_rollback`.

## 8. Tests

`crates/deltos-shelld/tests/packs.rs`, six tests, against the same real
`identityd` and the real `deltos-pack-check`:

- a pack installed from a source listing, byte for byte, with its storage
  made and staging left empty; the same edition refused; an appended newer
  edition installed as an update, rolled back byte for byte with its own
  signature back, a second rollback refused, the newer edition installed
  again; removal refused without the password, then the pack and its
  storage removed; the older edition then refused `failed_rollback`;
- signed by another key, a box with no key, a byte changed after signing,
  no signature, another pack's genuine signature: each `failed_signature`,
  nothing placed;
- a lying helper (no ranges, not where the header points, the header
  counted twice, past the end, overlapping): each refused, nothing placed;
- a pack built for another architecture, one needing more memory than the
  box has, a file that is not the pack chosen, a path as a source, a bare
  uuid, a pack whose `depends_on` is not installed: each refused with its
  reason; a pack with no kept earlier edition cannot be rolled back;
- plain HTTP, and a teacher (no `content.install`), refused for actions and
  listings; an extra argument refused;
- as root: the helper run as another user cannot read another install's
  folder and can read its own, and the pack's storage belongs to `deltd`'s
  user.

`crates/deltos-shelld/tests/shelld.rs`, ten tests, each against a real
`identityd` register on its own socket and shelld on another, called the way
`deltd` will call it:

- a connection from anyone but deltd, disconnected;
- each check refusing on its own: no session, a session said to be plain
  HTTP (a box API that forgot to check), a teacher without the permission, an
  action not on the list, one on the list but not built, an argument outside
  the shape; then the admin allowed, and all eight in the log with who asked;
- a ticket refused for other arguments, accepted once, refused when reused;
- no Wi-Fi password, backup passphrase or admin password in the log;
- an add-on installed (after the real platform manifest), refused a second
  install, updated, rolled back, stopped, refused removal without the
  password, kept from removing the platform it stands on, removed keeping
  its data, put back, removed with its data;
- a bundle signed by another key, a box with no key, a bundle changed after
  signing, and one with no signature: each `failed_signature`;
- a signed bundle holding `../escape`, a symlink and a device file: each
  refused, nothing written outside;
- an unmeasured add-on refused, then refused again without the password,
  then installed with it (verdict `cannot_tell`);
- an add-on too big for the box refused with its reasons; a bundle that is
  not the one chosen refused; a path as a source refused;
- another admin cannot read someone's job.

The emulated minimum-spec box (`system/tests/vm`) also checks shelld as
installed: root, no network, the socket's mode and group, its start order,
deltd's user served, another user and a group member that is not deltd both
refused, and then with the admin's real HTTPS session: a made-up session
and a plain-HTTP claim refused, a name approved and resolved by the phone
with no restart by hand, a backup to a drive as a job, a bundle on the drive
listed and refused for want of a key, and the log as rule 12 says.

**Result, 2026-11-23:** 136 of 136 checks pass on the emulated 2 GB ARM64 box
(results in the project files, `vm-results-2026-11-23/`). The backup through
shelld took 25 s under emulation. Shelld itself used **3.3 MiB** of memory
(resident) idle and **3.3 MiB** after every check, peak 3.3 MiB.
A base manifest for shelld (`system/apps/`) waits for its storage figure,
since the action log's growth has not been measured.
