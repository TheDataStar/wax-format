# Privilege Boundary — Who May Change the Box

*Design, no code. Written against SPEC v0.12 and `docs/cross-track-contract.md` on `main` (merge `8ada2fe`). Drafted 2026-11-20 in the Delt design thread; reviewed and revised 2026-11-20 before landing here. The review record is §15.*

**What this document owns:** how a request to change the box travels from a browser to the part of the box that holds the keys, and every check on the way. It is the design that contract §13 decision 2 asked for before C1 is built.

**What it does not own:** values more than one track consumes. Those live in the contract and are cited here, never restated as binding. Where this document needed a new contract value, the change is made in the contract in the same pull request (§12), and this document cites it.

**Companion:** `docs/deltd-api-v1.md`, the API every browser request in this document arrives through.

---

## 1. In plain words

A box does two very different kinds of thing. Most of the time it **shows** things: articles, videos, lessons. Sometimes it **changes itself**: it installs or removes a pack, reads a USB drive, joins a Wi-Fi network, restarts a service, restores a backup. The first kind is harmless if a stranger does it. The second kind is not.

The **privilege boundary** is the line between those two kinds of thing. "Privilege" just means the power to change the box. This document is the design for the gate at that line. It answers one question: when somebody taps **Install** in the WebOS, how does the box make sure the request really came from that person, that the person is allowed to do it, and that nothing else (a booby-trapped pack, a student on a shared screen, someone listening on the Wi-Fi, another website) can push the same button?

The answer is **five checks in a row**. A request must pass all five:

1. **Who are you?** The person is signed in, and the browser holds that sign-in.
2. **Is the line safe?** Changing the box needs a secure (HTTPS) connection **and** a sign-in that was itself made over HTTPS. A sign-in made over plain HTTP can never change the box, even if it is later replayed over HTTPS.
3. **Did you really mean it?** The request came from the WebOS itself, not from a pack page or another website pretending to be it.
4. **Are you allowed?** The person holds the permission this action needs (for example `content.install`), checked twice, by two different programs.
5. **Is this on the list?** The program that actually holds the keys, `deltos-shelld`, only knows a short, fixed list of actions with fixed arguments. Anything not on the list does not exist, whoever asks.

Every action that passes is written to a log on the box that an admin can read.

## 2. Why the earlier direction changes

Contract §13 decision 2 recorded a direction: a **loopback WebSocket**, a **per-boot token**, a **strict origin check** and a **closed list** of operations. That was written when the main screen was a kiosk browser on the box talking to `deltos-shelld` at `127.0.0.1`.

DeltOS is now used **mainly through other devices' browsers**: phones, laptops and tablets on the box's Wi-Fi. A loopback socket is unreachable from those devices by design, so the earlier direction only ever worked for the box's own screen. Keeping it would mean two ways in, one for the kiosk and one for everyone else, and two ways in is how one of them ends up less guarded.

| Earlier direction | This design | Why |
|---|---|---|
| Loopback WebSocket from the shell page to `deltos-shelld` | **No browser ever talks to `deltos-shelld`.** It listens only on a local Unix socket (a file-like connection point that only programs on the box can open) restricted to two named programs. | Remote browsers cannot reach loopback anyway, and a socket no web page can reach cannot be attacked from a web page. |
| Per-boot token handed to the shell page | **The person's own sign-in session**, checked on every request. | A boot-wide token proves "this is the shell page", not "this is Ms Okafor". Contract §4's permission model needs the person. |
| Strict origin check | **Kept**, on the box API, plus a per-session anti-forgery token. | Stops other sites and pack pages from sending requests with a person's cookie. |
| Closed list of operations | **Kept**, written out in §8 with the permission and arguments each needs. | The list is the last line of defence; it holds even if everything above it fails. |

The kiosk screen becomes just another browser using the same WebOS address and the same checks (contract §8). **There is one way in.**

## 3. The parts involved

| Part | In plain words | Runs as | May do |
|---|---|---|---|
| **The WebOS** | The web page people use, in any browser, the kiosk included. | — (it is a page) | Show things, and send requests to the box API with the person's sign-in. Holds no powers of its own. |
| **The box API**, served by `deltd` | The front desk. Every request from a browser arrives here. | its own unprivileged system user, `deltos-deltd` | Check the session, the connection and the origin; ask `identityd` about permission; pass approved actions on. It cannot change the box by itself. |
| **`deltos-identityd`** | The register of people, roles, sessions and sign-ins (contract §4, §5.1). | its own system user | Answer "is this session live, how was it made, and does its person hold permission X in scope Y?" Issue and verify re-confirmation tickets (§6). |
| **`deltos-shelld`** | The key holder. The only program with the power to install, mount, change networks and so on. | privileged | Run actions from its fixed list (§8) and its fixed internal list (§9), and nothing else. Re-checks permission itself before each one. |
| **Pack pages** | Third-party content, on the separate `deltos-packs.lan` zone (contract §8). | — | Display themselves. They never see a sign-in and cannot reach the box API. |
| **Helpers** (converter, mount helper) | Short-lived programs `deltos-shelld` starts for risky work on untrusted input. | a separate unprivileged user per helper, no network | Do one job on one input, write only into a staging folder. |

**Where TLS ends.** "TLS" is the encryption behind HTTPS. At the Kiosk profile there is no reverse proxy, so `deltd` serves HTTPS itself. Above Kiosk, Track G's proxy may sit in front. Either way, the box API must know for certain whether *this* request arrived over HTTPS. A proxy tells `deltd` over a local socket that only the proxy can open; a transport claim in an ordinary HTTP header from the network (such as `X-Forwarded-Proto`) is never believed.

## 4. Sessions

A **session** is the box's memory that "this browser is signed in as this person". The browser holds it as a cookie, a small value it sends back with every request to the WebOS address. Everything in this document hangs on sessions being right, so they are specified here in full. `identityd` owns sessions; `deltd` only presents them.

### 4.1 Two kinds of session, never mixed

| | **Plain session** | **Secure session** |
|---|---|---|
| Made by | signing in over plain HTTP | signing in over HTTPS |
| Cookie | `delt_session` — `HttpOnly`, `SameSite=Strict`, `Path=/`, **no `Domain` attribute** | `__Host-delt_session` — the same, plus `Secure` |
| Can do | read, learn, personal notes, everything not marked secure only | everything the person's permissions allow |
| Can change the box or view a class's progress | **Never** | Yes |

- **A plain session never becomes a secure one.** Moving to HTTPS means signing in again over HTTPS. This is the rule that makes the HTTPS decision real: someone who copies a plain-HTTP cookie off the Wi-Fi and replays it over HTTPS gets a request whose session `identityd` recorded as plain, and it is refused.
- **`identityd` records the transport at sign-in** and returns it with every session check. The box API compares it with the transport of the current request; both must be HTTPS for anything marked secure only (§8).
- **No `Domain` attribute** means the cookie belongs to the WebOS hostname alone, not to every `*.deltos.lan` service. A third-party service on the service zone (a wiki, a git server) never receives the WebOS session. The `__Host-` prefix on HTTPS additionally stops any sibling host from overwriting the secure cookie.
- `HttpOnly` means page scripts cannot read the cookie. `SameSite=Strict` means the browser does not send it on requests started by any other site. `deltos.lan` and `deltos-packs.lan` are different sites (`.lan` is not a public suffix, so each is its own registrable domain), so a pack page cannot cause the cookie to be sent.

### 4.2 Session values

- A session id is at least 128 bits from the operating system's secure random source. `identityd` stores only a hash of it, so a copy of its database does not hand over live sessions.
- **A new id at every sign-in**, never reusing one the browser already presented. This stops a "session fixation" attack, where someone plants a session id in a victim's browser and waits for them to sign in on it.
- **Ended on:** sign-out (which keeps a remembered device, §10, unless the person chooses "forget this device"); switching person on a shared screen (§7 rule 11); a password change or reset (every session of that person); removal of the profile; and an admin's "sign out everywhere".
- **Permission changes take effect on the next request.** Permissions are looked up live on each check, not copied into the session, so taking `content.install` away from someone works immediately.
- **How long a session lasts** before it ends itself, idle and absolute, on a shared screen and on a personal device, is **open** (§14). It is a number to choose with teachers, not to guess here. Whatever the numbers, a secure session's limits are never longer than a plain session's.

### 4.3 The anti-forgery token

Alongside the cookie, `identityd` makes a second random value per session, the **anti-forgery token** (often called a CSRF token: "cross-site request forgery" is another site making your browser send a request you did not mean). The WebOS reads it from the API's session endpoint (`GET /api/v1/session`, deltd API §6.1), keeps it in memory, and sends it in the `X-Delt-CSRF` header on every state-changing request. Another site cannot read it, so it cannot forge the header.

### 4.4 Browser ids and pack partitions

Each browser gets a random **browser id** (128 bits, in a host-only `HttpOnly` cookie on the WebOS address, holding nothing about the person). For each person signing in on that browser, `identityd` issues a **pack partition**: a random value that becomes part of every pack hostname that person gets on that browser (contract §8). Because each person on each browser has their own pack hostnames, their in-pack storage is kept apart, and the box-wide limit on people is gone. This is how the approved fix "the 32-slot limit counts browsers, not people" is delivered; §12 Change 1 explains why it ended up stronger than a numbered slot.

## 5. The path of one request

A teacher installs a pack from their laptop, which has the box's certificate installed.

1. **Sign-in.** The teacher opens the WebOS over HTTPS and signs in with their password. `identityd` checks it, makes a secure session, and the browser stores `__Host-delt_session`.
2. **The request.** The teacher taps Install. The WebOS sends `POST /api/v1/installs` with the cookie, the `X-Delt-CSRF` header and a JSON body naming the catalog item.
3. **Front-desk checks.** The box API checks, in order, and refuses at the first failure: the `Host` is the WebOS's own name; the `Origin` header is exactly the WebOS's own HTTPS origin; the anti-forgery token matches the session; the request arrived over HTTPS; `identityd` says the session is live **and secure**.
4. **Permission check.** The box API asks `identityd`: does this session's person hold `content.install`? A no is refused with a plain message.
5. **Hand-over.** The box API passes `install_pack` to `deltos-shelld` over its Unix socket, with the session reference and the arguments.
6. **Second check.** `deltos-shelld` confirms from the operating system that the caller is the `deltos-deltd` user, then asks `identityd` the same questions itself: live, secure, permission held. It never takes the box API's word for who the person is.
7. **Action and log.** `deltos-shelld` checks the arguments against the action's fixed shape, starts the job, and logs who, what, when and the result.

Step 6 looks redundant and is deliberate. The box API faces the network and is the part most likely to have a bug. If a bug lets a request skip the API's checks, the key holder still asks the register directly. §11 states exactly what this does and does not protect against.

## 6. Re-confirmation, done so it cannot be replayed

Irreversible actions (§8, "Re-confirm") ask the person to type their password again. Done naively, the box API would check the password and tell `deltos-shelld` "confirmed", which a buggy API could say about anything. Instead:

1. The WebOS sends the password with the request.
2. The box API passes it to `identityd`, which checks it and returns a **confirmation ticket**: single-use, valid for one minute at most, and bound to this session, this action and a digest of these exact arguments. `identityd` rate-limits these attempts like sign-ins.
3. `deltos-shelld` redeems the ticket with `identityd` before acting. A ticket for `remove_pack` of one pack cannot be used to remove another, or twice.

**Only an identity with a password can re-confirm.** Contract §4.4 lets an admin give any permission to any profile, including a passwordless one. That stays true for granting, but a passwordless identity can never pass re-confirmation, so it can never run a re-confirm action. The admin console says so when such a permission is assigned to a passwordless profile (contract §4.4 note).

## 7. The rules

Each rule is written so a test can check it (§13).

1. **`deltos-shelld` has no network door.** It listens only on a Unix socket under `/run/deltos/`, owned by its own user, with group `deltos-shellc` and mode `0660`. The `deltos-deltd` user is the only member of that group. It opens no TCP port and no WebSocket.
2. **Only named callers are served.** Beyond the file permissions, `deltos-shelld` reads the caller's user id from the operating system for every connection (`SO_PEERCRED`, already used by Track C §2 and Track F §3) and serves only `deltos-deltd` for the action list (§8) and the internal callers named in §9 for internal calls. Root is out of scope: a process running as root already owns the box.
3. **Every listed action names a person.** No action in §8 runs without a session `identityd` confirms is live. There is no "system" or "anonymous" caller for them. Box housekeeping uses the separate internal list (§9), which no session can trigger.
4. **Permission is checked twice,** by the box API and by `deltos-shelld`, both against `identityd`, both including scope (for example "own class") where the permission is scoped (contract §4.2).
5. **The list is closed.** `deltos-shelld` knows the actions in §8 and §9 and nothing else. Adding one is a code change and a change to this document. There is no "run this command" action, ever.
6. **Arguments have fixed shapes.** Each action accepts only its stated arguments, each with a type and limits. Packs, drives, files on drives, networks and backups are named by ids `deltos-shelld` itself handed out in an earlier listing, never by a path or command the caller writes.
7. **Requests must come from the WebOS.** The box API refuses any state-changing request (`POST`, `PUT`, `PATCH`, `DELETE`) whose `Origin` is missing or is not exactly the WebOS's own origin, or whose `X-Delt-CSRF` header does not match the session. Sign-in itself is checked the same way. `GET` requests never change anything.
8. **Packs can never ask.** No cookie is set or read on `deltos-packs.lan` (contract §8). The box API refuses any request whose `Origin` is a pack address. A pack's only channel to the WebOS is `postMessage` (a browser feature that lets a framed page send a message to the page around it). The WebOS accepts a message only when it comes from the frame it opened (`event.source`) and from that frame's own pack origin (`event.origin`), and only for the kinds on Track C §4's fixed list. It acts on a person's behalf only for harmless kinds (progress, navigation), and it fills in the pack and the person itself; it never takes them from the message.
9. **Changing the box needs a secure connection and a secure session.** Everything marked secure only in §8, plus `progress.view` across a class and `admin.console`, is refused over plain HTTP, and refused over HTTPS for a plain session (§4.1). Decided by Commander, 2026-11-20 (§10).
10. **Irreversible actions ask again,** through the ticket in §6.
11. **Switching person ends the session.** On a shared screen, choosing another profile signs the previous person out first. Person B is never using person A's session, not even for a moment.
12. **Every action is logged on the box.** Who, what, when, the arguments in summary, and the result, in an append-only log only `deltos-shelld` writes, readable with `admin.console`. **Secrets are never logged:** Wi-Fi passwords, backup passphrases and confirmation passwords appear as `[redacted]`. The log never leaves the box (contract §16). Its path is open (§14).
13. **Ask DeltOS stays answer-only.** The assistant (C11) has no route to any action on the list, whatever a model reply says (Track C §11, unchanged).
14. **The WebOS cannot be framed.** Every WebOS response forbids being shown inside another page (`Content-Security-Policy: frame-ancestors 'none'`), so no page, pack or otherwise, can lay an invisible Install button under someone's tap ("clickjacking").
15. **The WebOS only frames packs safely.** The pack frame never carries the `allow-top-navigation`, `allow-top-navigation-by-user-activation` or `allow-popups-to-escape-sandbox` sandbox tokens, so a pack cannot replace the WebOS page with a fake one. Track C owns the rest of the frame's settings.
16. **Unknown hostnames are refused.** `deltd` answers only the hostnames it serves (the WebOS name, issued pack hostnames, and the box's own service names). A request naming any other `Host` is refused before routing. This stops "DNS rebinding", where an outside website tricks a phone into treating the box as that website.
17. **Managed devices stay on HTTPS.** Over HTTPS the WebOS sends `Strict-Transport-Security` for its own hostname only (not its subdomains), so a teacher's device that has the certificate never quietly falls back to plain HTTP. Visitors without the certificate never receive it.
18. **Untrusted input is handled by helpers, not the key holder.** Converting a ZIM, reading a USB drive's file listing and checking an uploaded pack run in the helpers of §3, as unprivileged users with no network, writing only to a staging folder. `deltos-shelld` then installs the result through the normal checks.
19. **Removable drives are mounted harmlessly:** read-only unless the action writes a backup, never allowing programs to run from the drive or special device files on it, and only for the filesystem types Track E names.

## 8. The fixed list of actions

The first version of the closed list. Permission names are contract §4 names; `backup.manage` and `system.update` are added to the contract in this pull request (§12 Change 5). **Every row is secure only** (Commander's decision, §10).

| Action | What it does | Arguments (fixed shape) | Permission | Re-confirm |
|---|---|---|---|---|
| `install_pack` | Install or update a signed pack | source id from a catalog, drive, peer or upload listing; expected `archive_uuid` | `content.install` | No |
| `remove_pack` | Remove an installed pack | `archive_uuid` | `content.install` | **Yes** |
| `roll_back_pack` | Undo a pack's last update (SPEC §7.5) | `archive_uuid` | `content.install` | No |
| `mount_removable` / `eject_removable` | Read a USB drive, or safely eject it | drive id from the drive listing | `content.install` | No |
| `convert_zim_from_drive` | Convert a ZIM on a mounted drive into a pack, in the background, when measured storage allows (decided 2026-11-15) | drive id, file id from that drive's listing | `content.install` | No |
| `stage_upload` | Accept an uploaded pack file into staging for `install_pack` | upload id; declared size | `content.install` | No |
| `join_network` / `configure_ap` | Join a Wi-Fi network, or change the box's own hotspot | network id from the scan listing, or hotspot name and passphrase within Track E's limits | `network.configure` | **Yes** |
| `restart_service` | Restart one named DeltOS service | service name from the installed app manifests (contract §15) | `service.restart` | No |
| `pair_box` | Pair with another box (F6) | pairing code | `fleet.manage` | **Yes** |
| `start_backup` | Write an encrypted backup to a mounted drive (F10) | drive id; passphrase | `backup.manage` | No |
| `restore_backup` | Restore from a backup, with Track F §9's staged review | drive id, backup id; passphrase | `backup.manage` | **Yes** |
| `apply_update` / `roll_back_update` | Apply a system update, or go back to the previous one | update id from the update listing | `system.update` | **Yes** |

- **Listings are reads, and come first.** Drive contents, Wi-Fi scans, uploads and available updates are listed by read calls that hand out the ids the actions above take (rule 6). A listing shows each person only what they could act on or see.
- **Long actions return a job.** `install_pack`, `convert_zim_from_drive`, backups and updates report progress through the job states of contract §7.2. The permission is checked when the job starts; a job already running is not stopped if the person signs out.
- **A box-converted pack has no publisher signature.** `install_pack` requires a signed pack. How a pack converted on the box, or uploaded by a teacher, is trusted is the same question as the architecture doc's open Q2 (teacher-signed packs), and is open (§14). Until it is decided, a conversion finishes as a staged pack that `install_pack` refuses, rather than being installed unsigned.

What is deliberately **not** on this list:

- **Creating, editing and deleting profiles and roles.** Those change records in `identityd`, not the box itself, so `identityd` performs them under `profile.manage`, scoped to the person's own class or group, behind the same front-desk checks. Role editing (making new bundles) is `admin.console` and secure only.
- **Notes and highlights.** Private to the person (decided 2026-11-15), stored per person, and never readable through any action here, including by admins. A backup carries them as part of the identity data it protects; it does not show them.
- **Anything Delt does to show content.** Reading, search and export run in `deltd` with no special powers.

## 9. The internal list

Box housekeeping that no person triggers directly. Closed in the same way as §8, but callable only by the named internal caller and never with a session.

| Internal call | Caller | What it does |
|---|---|---|
| `clear_kiosk_partition` | `identityd`, when a person signs out on the kiosk | Clears that person's pack storage in the kiosk browser (Track C §5) |
| `retire_pack_hostnames` | `deltd`'s catalog, after `remove_pack` | Stops serving the removed pack's hostnames and frees its `pack_index` for a later install at a higher generation (contract §8) |
| `report_health` | Track F's health daemon | Read-only state for §7.1 health |

## 10. Decided: HTTPS for changing the box

Contract §16 says visitors' own phones reach the box over **plain HTTP**, and only managed devices (where the box's certificate is installed) use HTTPS. Over plain HTTP, anyone on the same Wi-Fi with the right tools can read what is sent, including a typed password and a session cookie.

**Decided by Commander, 2026-11-20:** changing the box, and seeing a class's progress, need HTTPS. Everything else, sign-in and learning included, works over plain HTTP. A teacher or admin installs the box's certificate on their own device once, during setup, and the WebOS walks them through it. A person's own progress (`progress.view.self`) stays available over plain HTTP. (Rejected: everything over plain HTTP with a warning.)

**The kiosk uses HTTPS.** The kiosk browser ships with the box's certificate already trusted and opens the WebOS over HTTPS, so the box's own screen is never stuck on plain HTTP (contract §8).

**Decided by Commander, 2026-11-20: teacher and admin passwords are only ever typed over HTTPS.** A password typed over plain HTTP can be read by anyone on the Wi-Fi, who can then sign in over HTTPS from their own device as that teacher; HTTPS protects the person whose device trusts the certificate, not against a stranger who chooses to connect. So a password belonging to anyone who holds a secure-only permission is accepted only over HTTPS. Over plain HTTP the WebOS does not show them a password box; it tells them to use their certified device. Students and guests are unaffected. (Rejected: plain-HTTP sign-in with reduced powers, which still exposes the password.)

**Decided 2026-11-20 (Commander delegated it to the recommendation): students sign in on their own phones with a teacher-issued sign-in card.** Tap-your-name sign-in (Track F §21) was designed for a shared screen a teacher can see. On personal phones anyone could tap a classmate's name and read their private notes. So:

- **Tap-your-name stays on the shared kiosk only.** The list of names is never sent to any other device (deltd API §6.1).
- **Off the kiosk, a passwordless person signs in with a sign-in card:** a QR code with the same secret as a short typed code, which a teacher (with `profile.manage` for that class) or an admin prints or shows. The student scans or types it once on a phone. Still no password to remember.
- **The phone then remembers the person.** Using a card makes a **remembered device**: a long-lived, host-only, `HttpOnly` cookie (`delt_device`) holding a random device credential (at least 128 bits, stored hashed by `identityd`). It is not a session. It lets that browser start a new plain session for that person without the card again.
- **The teacher stays in control.** The teacher can see each student's remembered devices and remove any one. Issuing a new card replaces the old one, and the old card stops working at once. A lost phone is fixed by removing that device.
- **Issuing a card, and removing a remembered device, are secure only.** They hand out or take back a way into someone's account. They need no new permission, because teachers already hold `profile.manage` for their class, and their sessions are secure anyway (their passwords are HTTPS only).
- **What a card cannot do:** a card signs in only a passwordless identity, and only to a plain session. It never unlocks a secure-only permission (contract §4.2), so it cannot change the box.
- **What it does not stop:** a card used over plain HTTP can be read off the Wi-Fi once, as any plain-HTTP traffic can. A student who shows their card to a classmate has shared their account. Both risks are why the teacher can see and remove devices. The code length and how long an unused card stays valid are set together with the session lengths (§14), and card use is rate-limited like sign-in (deltd API §9).

## 11. What each check stops, and what it does not

| Threat, in plain words | What stops it |
|---|---|
| A booby-trapped pack tries to install something or change Wi-Fi | Packs live on another site and never hold a sign-in (rule 8); their origin is refused (rules 7, 8); their own security policy forbids requests to other origins (deltd API §4). |
| Another website the person visits tries to send a request to the box | `SameSite=Strict` cookie, exact origin check and anti-forgery token (rule 7). |
| A page shows the WebOS invisibly and tricks a tap on Install | The WebOS cannot be framed (rule 14). |
| A pack tries to replace the WebOS page with a fake sign-in page | The pack frame cannot navigate the top page (rule 15). |
| An outside website tries to reach the box through the visitor's phone (DNS rebinding) | Unknown hostnames are refused (rule 16), and the origin check fails anyway. |
| A student on a shared classroom screen tries the teacher's Install button | Their session does not hold `content.install` (rule 4); switching person signs the teacher out (rule 11). |
| Someone on the Wi-Fi copies a session cookie sent over plain HTTP | A plain session can never change the box or see class progress, even when replayed over HTTPS (§4.1). |
| A student taps a classmate's name on their own phone to read their notes | The name list is kiosk-only; off the kiosk a passwordless person signs in with a teacher-issued card (§10). |
| Someone on the Wi-Fi reads a teacher's password typed over plain HTTP | Those passwords are never accepted over plain HTTP (§10, decided 2026-11-20). |
| A third-party service on `deltos.lan` (a wiki, a blog) has a bug | The WebOS cookie has no `Domain` and is never sent to it; the secure cookie's `__Host-` prefix stops it being overwritten (§4.1). |
| A bug in the box API skips its own permission check | `deltos-shelld` asks `identityd` itself (rule 4) and only knows the fixed list (rule 5). |
| A crafted argument tries to reach files outside the pack store | Fixed argument shapes; everything named by an id `deltos-shelld` issued (rule 6). |
| A crafted ZIM, pack or USB drive tries to exploit the converter or the mount | Untrusted input is handled by unprivileged helpers with no network (rule 18); drives are mounted harmlessly (rule 19). |
| A pack's text tells Ask DeltOS to install something | Ask DeltOS cannot reach the list at all (rule 13). |
| Someone grabs an unattended signed-in laptop and removes packs | Re-confirmation with the password for irreversible actions (§6), and the log shows who and when (rule 12). |

**What this does not protect against, stated so nobody assumes it does:**

- **A fully taken-over `deltd` while an admin is signed in.** `deltd` sees every session cookie it is sent. If an attacker controls it completely, they can present a live admin session to `deltos-shelld` and run non-re-confirm actions as that admin. The double check stops `deltd` *bugs* that skip a check; it cannot stop `deltd` lying with a real session. What still holds: only listed actions with fixed arguments, a log entry naming the admin for every one, and no irreversible action without a ticket `identityd` issues against the real password. This is why `deltd` runs unprivileged and is the first thing fuzzed (deltd API §11).
- **A shared browser outside the kiosk.** On a family phone or a library laptop, the browser's own history is visible to the next person who uses it. Signing out shuts that person's pack hostnames on that browser, so their in-pack data can no longer be opened there (deltd API §4.4), but the box cannot erase what the browser keeps for itself.
- **Plain-HTTP reading is readable.** What a visitor reads over plain HTTP can be seen by others on the Wi-Fi, including the pack hostnames they were issued. That is the accepted cost of contract §16; it exposes reading, never the power to change the box.

## 12. Contract changes, as made in this pull request

The contract owns these values (contract §14), so each change is made there, in this pull request, and nowhere else. Commander approved Changes 1 and 2 on 2026-11-20; Changes 3 to 5 were proposed with the draft; Changes 6 and 7 are review fixes, marked as new decisions in contract §12 for review in the pull request.

- **Change 1 — pack hostnames are per person per browser, not per profile (contract §8).** Approved goal: the 32-slot limit counts browsers in use, not people. The draft did this by renaming `profile_slot` to a `browser_slot` numbered 0–31 per browser. The review found two holes in that: a browser whose WebOS cookie is cleared but whose pack storage is not would hand the next person the previous person's slot and their stored pack data; and the "clear before reuse" step cannot reach a visitor's phone that never comes back. The landed version replaces the slot number with a random **pack partition** per (browser, person), so a new person, or a reset browser, always gets fresh pack origins and nothing needs clearing for safety. It also removes the 32-per-browser limit entirely, and, because `deltd` only serves partitions it issued, it means a device that never signed in cannot read packs at all and `guest_accessible` can actually be enforced.
- **Change 2 — progress comes from the sign-in, not the pack address (contract §5.2).** `profile_id` is taken from a live signed-in session, never from a field in the event and never from the pack origin alone. See deltd API §8.2 for the three delivery paths.
- **Change 3 — decision 2 becomes "Designed" (contract §13),** pointing at this document.
- **Change 4 — one WebOS address, used by the kiosk too (contract §8).** The `http://127.0.0.1:40999` shell row and the loopback pack-port rows are retired, since `deltd` routes by hostname at every profile (Track A §20.3) and the kiosk uses the WebOS address. The WebOS and the box API share one origin. **The hostname itself is still unnamed** (§14).
- **Change 5 — two new permissions (contract §4.2):** `backup.manage` and `system.update`, admin only by default. Without names, under default-deny (contract §4.3) nobody but `admin` could ever be given these, and an admin could not delegate backups to an IT helper without handing over the whole box.
- **Change 6 (review) — pack hostnames carry a generation (contract §8).** A `pack_index` reused after a removal gets a higher generation number in its hostname, so a new pack never inherits the browser storage a removed pack left on a visitor's phone. The previous rule ("free the index only after its browser data is cleared") could not be met for phones.
- **Change 7 (review) — re-confirm needs a password (contract §4.4 note),** as in §6.

## 13. Build and test plan

Smallest useful piece first:

1. `deltos-shelld` with its socket, the group and peer checks, the `identityd` re-check, the log, and **one** action (`mount_removable`, with its helper). This is Track C's P0 proof, updated to this design.
2. Sessions in `identityd` (§4): both kinds, the anti-forgery token, browser ids, partitions, and tickets (§6).
3. The box API's front-desk checks (§5 step 3).
4. The rest of the action list, one at a time, each with its argument shape and listing.
5. Pack partitions in pack hostnames, and progress through the session (Changes 1 and 2), with the browser tests below.

**Tests that must exist before the boundary is called done:**

- A pack page that tries every way to reach the box API (fetch, form post, image tag, top-level navigation, `postMessage` with a forged message and with a forged origin) and is refused each time.
- A box API build with its permission check removed, which still cannot get an action past `deltos-shelld`.
- **A plain-HTTP session cookie replayed over HTTPS against a secure-only action, refused.**
- A secure-only action over plain HTTP, refused; the same with a secure session, allowed.
- Requests with a missing `Origin`, a wrong `Origin`, a pack `Origin`, a missing anti-forgery token and a wrong one: each refused.
- A request with an unknown `Host`, refused.
- The WebOS loaded inside a frame from another origin: the browser refuses to show it.
- A confirmation ticket reused, used after a minute, used for a different action or different arguments: each refused.
- A teacher of class A asking for class B's progress, refused (scope).
- A student session on the kiosk that cannot run any listed action.
- A passwordless profile given `content.install` by an admin, which still cannot `remove_pack`.
- The tap-your-name list requested from any device but the kiosk, refused. A sign-in card used for a passworded person, or to get a secure session, refused. A replaced card, and a removed remembered device, each refused at once.
- Two people signing in one after the other on the same phone browser: the second never sees the first's pack data; then the same after clearing only the WebOS site's cookies. Run on the current Android and iOS default browsers.
- A port scan of the box and of loopback that finds nothing from `deltos-shelld`; a connection to its socket from any other user, refused.
- Every listed action appears in the log with the right person, and no secret appears in the log.

## 14. Still open

Not answered here, and not to be filled in by whoever implements first. Each is also in contract §13.2.

- **The WebOS hostname** (Change 4). Needs a name in contract §8 before C1 is built.
- **How long a session lasts,** idle and absolute, shared screen and personal device (§4.2), together with how long a remembered device lasts and the sign-in card's code length and validity (§10).
- **Where the action log lives** under `/var/lib/deltos/` (rule 12). Needs a path in contract §10.
- **How box-converted and teacher-uploaded packs are trusted** (§8), the same question as the architecture doc's Q2.
- **How other services on `deltos.lan` sign people in** now that the WebOS cookie is not shared with them (§4.1). Track G's forward-auth (G2) needs its own per-service sign-in that hands over from `identityd`, never a parent-domain cookie. Track G owns the design.

## 15. Review record (2026-11-20)

What the review changed from the draft, and why.

| # | Finding | Severity | Change |
|---|---|---|---|
| 1 | A plain-HTTP session cookie, copied off the Wi-Fi, could be replayed over HTTPS: the draft checked only the request's connection, not how the session was made. This defeated the HTTPS decision. | Critical | Two kinds of session, never mixed (§4.1); the check compares both. |
| 2 | A teacher password typed over plain HTTP gives an eavesdropper full control, whatever the HTTPS rule says. | Critical | Put to Commander; decided 2026-11-20: HTTPS only (§10). |
| 3 | Passwordless sign-in on personal phones lets classmates read each other's private notes. | High | Teacher-issued sign-in cards and remembered devices (§10), decided 2026-11-20. |
| 4 | `browser_slot` could leak one person's pack data to the next after a partial browser reset, and pack-index reuse required clearing phones the box cannot reach. | High | Random pack partitions and hostname generations (Changes 1 and 6). |
| 5 | The cookie scope was "the `deltos.lan` zone", which would send the WebOS session to every third-party service there. | High | Host-only cookie, `__Host-` on HTTPS (§4.1); Track G's sign-in listed as open. |
| 6 | The socket was "readable and writable only by its own user", which would also lock out the box API it must serve. | Medium | Group `deltos-shellc`, mode `0660`, plus the peer check (rules 1, 2). |
| 7 | Re-confirmation was a password check the box API reported on, so a buggy API could claim it. | Medium | Single-use tickets bound to action and arguments (§6). |
| 8 | Re-confirm assumed passwordless people never hold those permissions, which contract §4.4 does not guarantee. | Medium | Re-confirm needs a password (§6, Change 7). |
| 9 | No session rules: id strength, rotation at sign-in, revocation, live permission lookup. | Medium | §4.2. |
| 10 | The two docs disagreed on forgery protection (a fixed header against a per-session value). | Medium | One mechanism, the per-session token (§4.3), in both docs. |
| 11 | No protection against clickjacking, top navigation by packs, DNS rebinding, or SSL-stripping on managed devices. | Medium | Rules 14 to 17. |
| 12 | Untrusted ZIMs, uploads and drives were handled by the key holder itself. | Medium | Unprivileged helpers, harmless mounts (rules 18, 19). |
| 13 | The kiosk would land on plain HTTP and be unable to change the box. | Medium | The kiosk uses HTTPS with the certificate preinstalled (§10). |
| 14 | Box-converted packs are unsigned but `install_pack` requires a signature. | Medium | Staged, not installed, until Q2 is decided (§8). |
| 15 | Scoped permissions ("own class") were checked as plain yes/no. | Medium | Scope is part of both checks (rule 4). |
| 16 | Secrets could end up in the action log. | Low | Redacted (rule 12). |
| 17 | The residual risk of a fully compromised `deltd` was not stated. | Low | §11. |
| 18 | Rollback of a pack update and upload staging had no action. | Low | `roll_back_pack`, `stage_upload` (§8). |
