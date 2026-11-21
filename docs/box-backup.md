# Box Backup and Restore (F10)

*Design and build record, 2026-11-21. Implements Track F §9 (F10) on `main` after merge `b426c4e`. Built in `crates/deltos-backup`. Values more than one track consumes are proposed in the contract in the same pull request (§9 here), not restated as binding.*

---

## 1. In plain words

A box keeps everything it knows in one folder, `/var/lib/deltos/`: the people and their passwords, the box's own certificate (its "stamp", which teachers' laptops trust), the Wi-Fi settings, the catalog of packs, and whatever people create. A **backup** is a copy of that folder in **one locked file** on a USB drive. The lock is a **passphrase** the admin types when making the backup. Without the passphrase the file is noise: it does not even say how many people the box has.

A **restore** puts that folder back, usually onto a new box after the old one broke. It happens in two steps. First the box **opens and checks** the backup and shows the admin what is in it: how many people and which admins, the certificate stamp, the Wi-Fi name, which packs to reinstall. Nothing changes yet. Then, after the admin types their own password again, the box **swaps it in**. Because the stamp comes back too, every teacher's device that trusted the old box trusts the new one with nothing to set up.

Pack files are **not** in the backup. They are large, they can be fetched again, and a reinstalled pack is checked again anyway. The backup lists them, so the admin knows what to reinstall.

## 2. Where it sits

| Part | Does |
|---|---|
| `deltos_backup::archive` | Makes a backup (`create`), lists backups on a drive (`list`). |
| `deltos_backup::restore` | `review` (open, check, stage, report) and `apply` (swap in, or put everything back). |
| `deltos_backup::actions` | The three actions behind the privilege boundary, the same shape as `deltos_net::actions`: nothing changes the box except `perform`, which takes only an `Authorized`, which only `authorize` makes. |
| `deltos-backup` (command) | The same code for **root at the box's own console**: `create`, `list`, `review`, `restore`. Root already owns the box (privilege boundary rule 2), so this adds no power; it is how a box is backed up until `deltos-shelld` and the admin screens exist. |

`deltos-shelld` is not built yet. When it is, it calls `actions::authorize` then `actions::perform`, resolving drive ids to mount points through the `Drives` trait and stopping and starting services through the `Services` trait.

## 3. What a backup holds (the backup set)

**The rule: everything under `/var/lib/deltos/`, except a short named list.** Track F §9 makes the whole tree the one backup boundary, and contract §16 says backups cover everything people create. Including by default means a new kind of state is backed up the day it appears. Excluding by default would lose it silently, which contract §16 calls worse than no backup.

| Under `/var/lib/deltos/` | In the backup | Why |
|---|---|---|
| `identity/` (the register: people, password hashes, roles, remembered devices, sign-in cards, pack partitions, the card key) | **Yes**, except live sessions and unspent re-confirmation tickets | Sessions and tickets belong to browsers and minutes, not people; bringing them back would revive sign-ins nobody made on the new box. Everyone signs in again. Remembered devices, cards and partitions are kept, so phones, cards and in-pack storage keep working. |
| `ca/` (the CA key and certificate) | **Yes** | The stamp. Losing it means every device enrols again (`box-certificates.md` §7). The key is a secret, which is why the file is encrypted. |
| `tls/` (server certificates and keys) | **Yes** | Kept with their CA so the restored box serves the same certificates; `deltos-tls` renews them from the restored CA when due. |
| `network/` (hotspot name, password, country, channel, names, reservations, uplink sharing) | **Yes** | The box's settings. The Wi-Fi password is a secret, protected the same way. |
| `b4/` (deltd's pack-hostname catalog) | **Yes** | Keeps each pack's hostname, so devices' in-pack storage carries over when the same pack is back. If the pack file is missing when `deltd` starts, `deltd` retires its slot as it always does. |
| `packs-data/`, notes, reading progress, a Track H service's data, any folder not named here | **Yes** | Whatever people create. Notes and reading progress have no path yet (contract §13.2); wherever they land under this tree, they are in the backup without a change here. |
| `packs/` | **Listed, not copied**: file, `archive_uuid`, name, version, size | Large; can be fetched again; and Track F §9 requires a restored pack to go through install verification anyway, so a copied pack would be checked exactly like a fetched one. A box-converted or teacher-uploaded pack cannot be fetched again; until their trust is decided (contract §13.2) they cannot be installed at all, so there is nothing yet to lose. Revisit when that is decided. |
| `capability.json` | No | The measured RAM, storage, architecture and GPU of **this** hardware (contract §9). A restore onto other hardware must measure its own. |
| `health/` | No | Health snapshots of the old box. |
| `.restore-staging/`, `.restore-previous/` | No | The restore's own folders (§5). |
| Anything that is not a plain file or folder (a symbolic link, a socket) | No, and named in the backup's list as skipped | A restore never creates links; a link could point anywhere. |

Not under the tree, so never in a backup: hotspot leases (`/run`, personal data, `box-network-plan.md` §4), the generated settings in `/run/deltos/network/` (rebuilt from the registry at every boot), and the operating system (Track F §9: out of scope).

**Consistent copies.** Each SQLite database is copied through SQLite itself (`VACUUM INTO`), which takes a consistent copy while its program keeps running. Its `-wal`, `-shm` and `-journal` side files are never copied on their own. Other files are copied as they are; the services that write them write them whole.

## 4. The lock: encryption and the key

**Format: [age](https://age-encryption.org/v1)** (version 1), passphrase recipient, through the `age` Rust crate. Inside: a zstd stream (level 9) of a tar stream, whose first member is `manifest.json` and whose other members are `state/<path>`.

Why age: Track F §9 asked for encryption at rest with a passphrase and left the format open. age is a small, published, reviewed format with independent implementations, and it authenticates every 64 KiB chunk, so a changed or cut-off file is refused rather than half-restored. It also means a backup can be opened **off the box** with the standard `age` tool (`age -d backup.age | zstd -d | tar x`) if every box is lost, which matters more for a disaster than any DeltOS-specific format would.

**The key is the passphrase, and only the passphrase.**

- The admin types it at backup time. It is separate from every sign-in password (Track F §9). The box never stores it, never logs it (privilege boundary rule 12: `[redacted]`), and never puts it in a command line (the command reads it from the terminal without echo, or from standard input).
- **Rules: at least 10 characters, at most 256 bytes**, the box's own password rule (`deltos_identity::policy`), because a person types it the same way, and a copy of the file can be attacked offline for as long as anyone likes.
- **Work factor: scrypt `N = 2^18`** (256 MiB of memory), fixed. This is age's own documented default, "roughly 1 second on a modern machine". It is fixed rather than measured on the box that writes the backup, so a backup made on a fast mini PC still opens on a 2 GB Pi (contract §2.1's minimum spec). A restore accepts **no higher** factor, so a hostile file cannot make the box spend more memory than that. Measured under emulation in §8.
- **No recovery.** A forgotten passphrase means the backup is lost; the command says so when it writes one. There is deliberately no second key on the box: a key the box could use to open its own backups would sit on the box beside the data, and a stolen box would hand over every backup made from it.
- **Integrity for the register, beyond the confirmation step.** Track F §9 left open a stronger check than a human looking at the summary (§15 there). The lock gives one: only someone who knows the passphrase can make a file that opens with it, and every chunk is authenticated, so a substituted or edited backup on a bare USB drive does not open. The staged review stays as well.

## 5. Restore, step by step

1. **Review** (`review_backup`, or `deltos-backup review`). The backup is decrypted into `/var/lib/deltos/.restore-staging/review-<id>/` (root only, `0700`), on the same filesystem as the live state so the swap is a rename, never a copy. Every member must be in the backup's own list, with the listed size and SHA-256, at a plain relative path; nothing outside `state/`, nothing twice, nothing missing. Then:
   - state **newer than this box's programs** is refused (contract §10): the register's `schema_version`, the network registry's and the catalog's `user_version`;
   - **half a CA** (key without certificate, or the reverse) is refused, as `deltos-tls` refuses it (`box-certificates.md` §2);
   - every file's **owner and group must exist** on this box (by name; see below);
   - the report: people by kind, roles and how many hold each, the **box-wide admins' sign-in names** (who can sign in afterwards), remembered devices, sign-in cards, last activity; the backup's CA fingerprint beside this box's; the network mode, interface, Wi-Fi name and country (never the password); every listed pack, and whether a pack with that `archive_uuid` is on this box; what is replaced and what is kept; warnings in plain words.
   One review is staged at a time; a new review replaces the last.
2. **Restore** (`restore_backup`, re-confirmed, or `deltos-backup restore` with "restore" typed). It names the review and its **review digest** (SHA-256 over the backup id and the exact manifest bytes), so the confirmation is bound to exactly the backup the admin saw. Then owners and modes are set on the staged copy, the services are stopped (`deltd`, `identityd`), and each top-level part is swapped: the live one moved into `/var/lib/deltos/.restore-previous/`, the staged one moved into place. **If any swap fails, every swap already made is put back** and nothing has changed. Then `deltos-tls` runs (checks the restored CA, renews if due), the network settings are regenerated (the path watcher restarts the radio and DNS/DHCP if they changed), and `identityd` and `deltd` start.
3. **Afterwards.** The admin signs in with an admin account **from the backup**; the fresh box's throwaway admin is gone, as Track F §9 intends (a fresh box always runs its own first boot first, and its admin then restores over it, having seen the warning). Packs listed as missing are reinstalled through the normal install checks. What was replaced stays in `.restore-previous/` until the next restore.

**Owners by name.** A fresh box makes its system users with `systemd-sysusers`, which chooses the numbers itself, so `deltos-identity` may be a different number on the new box. The backup records each file's owner and group by name and mode, and the restore looks the names up on the box it runs on.

**Before first boot: not possible.** Restore is an admin action, and an admin exists only after first boot (Track F §9). At the console, root can restore at any time, which is no more than root could do anyway.

## 6. The actions and the command

| Action | Arguments | Permission | Secure only | Re-confirm |
|---|---|---|---|---|
| `start_backup` | drive id; passphrase | `backup.manage` | yes | no |
| `review_backup` | drive id, backup id; passphrase | `backup.manage` | yes | no |
| `restore_backup` | review id, review digest | `backup.manage` | yes | **yes**, ticket bound to these exact arguments |

A backup id is exactly a name the box makes: `deltos-backup-<UTC YYYYMMDDTHHMMSSZ>-<8 hex>.age`; anything else is refused (rule 6). Passphrases enter the ticket's argument digest only as their SHA-256, and the action summary for the log shows `[redacted]`. A backup file is written `0600` under a hidden `.part` name, flushed, then renamed, so a pulled drive never holds half a backup under a real name.

```
deltos-backup create  --to /media/usb            # asks for the passphrase twice
deltos-backup list    /media/usb
deltos-backup review  --from /media/usb deltos-backup-20261121T090000Z-1a2b3c4d.age
deltos-backup restore <review id>                # type "restore" to go ahead
```

## 7. Decided here, under Commander's delegation

| Decision | Value | Why |
|---|---|---|
| Backup set | whole tree minus four named exclusions | §3 |
| Pack files | listed, not copied | §3 |
| Sessions and tickets | dropped from the backup | §3 |
| Format | age v1 passphrase, zstd 9, tar, manifest first | §4 |
| Passphrase rule | 10 characters to 256 bytes, the box's password rule | §4 |
| scrypt work factor | `2^18` fixed, the most a restore accepts | §4 |
| Key recovery | none | §4 |
| Restore flow | `review_backup` then `restore_backup` bound to the review digest | §5, privilege boundary §8 amended |
| Staging and previous folders | `/var/lib/deltos/.restore-staging/`, `/var/lib/deltos/.restore-previous/`, root `0700` | proposed for contract §10 |

## 8. Tests

- `crates/deltos-backup/tests/backup.rs` (13): a full round trip on a real state tree made by the register, `deltos-tls`, the network registry and the catalog crates, onto a "fresh box" with its own new CA and admin, bringing back the same CA key and fingerprint, the old admin's password, the Wi-Fi password and an unnamed folder's note, with `deltos-tls` then issuing nothing; sessions dropped; nothing readable in the file without the passphrase; a wrong passphrase, a flipped byte, a cut-off file, a newer schema and half a CA refused, with nothing left staged; a confirmation for another review refused; a failed stop and a swap failing half way leave the box as it was; missing packs named for reinstall; the three actions refused over plain HTTP, on a plain session, without `backup.manage`, and (restore) without a ticket for these exact arguments, which works once.
- `system/tests/vm/` (emulated ARM64, 2 GB): on the running box, root makes a backup at the console; it holds nothing readable; a wrong passphrase is refused. The box is then wiped and set up again from nothing with a different Wi-Fi name, a new first admin and a new CA, so a device that trusted the old box no longer trusts it. The review shows the old stamp, the admin, the Wi-Fi name and the pack; the restore brings the stamp back, the services up with no failed unit, owners and modes right; the virtual phone that trusted the box before rejoins the restored hotspot on its own, trusts HTTPS again, the old admin signs in with their old password, the throwaway admin is gone, and the student's remembered phone signs in without the card. Run of 2026-11-21: **RESULT PASS, 119 of 119 checks**; under TCG emulation (2 GB) the backup took 14 s and the restore 30 s; the backup file was 6,681 bytes beside a 63,045-byte pack.

## 9. Contract and document changes in this pull request

- **Contract §10**, proposed: `/var/lib/deltos/.restore-staging/` and `/var/lib/deltos/.restore-previous/`.
- **Privilege boundary §8**: `review_backup` added; `restore_backup` takes the review id and digest (the passphrase was already given at review).
- **deltd API v1 §5** table: `POST /backups/{id}/review`; `POST /backups/{id}/restore` takes the review id and digest.
- **`box-certificates.md` §7**: the CA key in the backup set, done.
- **Track F §15**: the encryption format and the register's integrity check, answered here.

## 10. Not decided here

| Open | Owner |
|---|---|
| Backups on a schedule, and to a network location (Track F §9 names both) | F1 admin console, with shelld's drive handling |
| The drive listing and mounting for writing (rule 19) that hands out drive ids | `deltos-shelld` |
| The action log's path (privilege boundary §14) | privilege boundary |
| A restore onto hardware whose Wi-Fi interface has another name: the review shows the interface; changing it is `configure_ap`/`join_network`'s | Track E |
| Undoing a restore from `.restore-previous/` as an action | F1, if wanted |
| Including box-converted packs once their trust is decided | contract §13.2 |
| Per-app backup sets from manifests (contract §15.1 durability), when apps exist; the whole-tree rule already covers their data folders | Track G / H |
