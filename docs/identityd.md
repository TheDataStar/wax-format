# The box's sign-in service (`deltos-identityd`)

*Written 2026-11-21 with the build in `crates/deltos-identity` (the service), `crates/deltlib` (`deltd`'s side: the sign-in endpoints, the write checks and the HTTPS listener) and `system/` (its user, directories and boot unit). It builds step 2 of `docs/privilege-boundary.md` §13 (sessions, the anti-forgery token, browser ids, pack partitions and re-confirmation tickets) and the session group of `docs/deltd-api-v1.md` §6.1. Tested in code and in the emulated ARM64 machine (§8); **not yet on a real Pi or real phones** (Commander's hold, 2026-11-20).*

**What this document owns:** how `deltos-identityd` stores people and sign-ins, the local socket `deltd` talks to it on, and the values it runs on. **What it does not own:** the rules it enforces, which are `docs/privilege-boundary.md` §4, §6 and §10 and contract §4 and §5.1, cited here and not restated as binding; and the role and permission vocabulary (contract §4).

Plain words used below: a **session** is the box's memory that "this browser is signed in as this person", kept by the browser as a **cookie**. A **hash** is a one-way fingerprint of a secret: the box can check a secret against it but cannot get the secret back from it. **Argon2id** is a deliberately slow hash made for passwords. A **socket** here is a file-like connection point that only programs on the box can open.

---

## 1. In short

- **One small program holds the register of people** (who exists, how they sign in, which roles they hold) and every sign-in. It has no network at all and no special powers. Only `deltd`, the box's web server, can talk to it, through one local socket, and it checks which program is calling on every connection.
- **Four ways to sign in**, matching the decisions already made:

  | Who | How | Gets |
  |---|---|---|
  | Teacher, admin (anyone with a password) | name and password, **over HTTPS only** for anyone holding a permission that changes the box or shows a class's progress (contract §5.1) | a **secure** session over HTTPS |
  | Student or Guest, on their own phone | the **sign-in card** a teacher issues (QR or `XXXXX-XXXXX` code), once | a **plain** session, and the phone is **remembered** |
  | The same student, later | nothing: the remembered phone starts a session by itself | a plain session |
  | Student or Guest, on the box's own screen | tap their name (kiosk only) | a plain session that ends after 10 idle minutes |

- **Nothing secret is kept in the clear.** Passwords are kept as Argon2id hashes; session ids, browser ids, remembered-device credentials and re-confirmation tickets are 256-bit random values kept only as hashes; card codes are kept as a keyed hash whose key is in a separate file. A copy of the database hands over no live session and no password.
- **`deltd` now waits for it at boot,** and both wait for the box's first admin (§6). There is no anonymous mode and no test table on a real box.

## 2. The parts

| Part | Runs as | Holds | Notes |
|---|---|---|---|
| `deltos-identityd` | its own user `deltos-identity`, **no capabilities, no network** (only loopback inside its sandbox) | `/var/lib/deltos/identity/` (`0700`): `identity.sqlite` and `card-key`, both `0600` | Answers only on its socket. Sweeps expired sessions, devices, cards and tickets every 10 minutes. |
| its socket | — | `/run/deltos/identity/identityd.sock`, `0660`, group `deltos-identc` | The directory is setgid `deltos-identc`, so the socket is that group's; `deltd` is the group's only member. A caller is also checked by user id from the kernel (`SO_PEERCRED`). |
| `deltd` | `deltos-deltd` | nothing about people | Adds only what it alone knows (HTTPS or not, the box's own screen or not, the device's address, the cookies) and forwards (deltd API §6.1: "deltd adds nothing"). Cannot see the register directory. |
| `deltos-identity-provision` | run once as `deltos-identity` | — | Makes the first admin (§6). |

**The register's tables** (schema version 1; a newer one is refused, contract §10): `profiles` (with contract §5.1's table-level CHECK that a passworded row always has a password hash, and that only passworded rows have a sign-in name), `roles`, `role_permissions`, `profile_roles` (a role at a scope: box-wide, or one class), `browsers`, `partitions` (one random pack partition per browser and person, contract §8), `sessions`, `devices`, `cards`, `tickets`.

**Roles are data** (contract §4.2). The four built-in bundles are seeded on a new register with contract §4.2's defaults: `admin` holds every permission, including ones apps add later (a flag on the bundle, not a list); `teacher` holds `profile.manage`, `progress.view` and `progress.view.self`, granted at a class's scope; `student` holds `progress.view.self`; `guest` holds nothing beyond reading. `lists.assign` is not seeded: whether `teacher` gets it is still open (contract §13.2).

## 3. The rules it enforces, and where each comes from

| Rule | From | Where in the code |
|---|---|---|
| A session made over plain HTTP is plain forever; secure-only work needs an HTTPS request **and** a secure session | privilege boundary §4.1 | `Register::secure_session` |
| Teacher and admin passwords are refused over plain HTTP, before the password is even checked | contract §5.1, privilege boundary §10 | `Register::sign_in_password`, `password_allowed` |
| A new id at every sign-in; any session the browser presented ends first (no session fixation; switching person never overlaps) | privilege boundary §4.2, rule 11 | `Register::start_session` |
| Permissions are looked up live on every check, with scope | privilege boundary §4.2, rule 4 | `effective`, `SessionView::holds` |
| One anti-forgery token per session, compared on every write | privilege boundary §4.3, rule 7 | `deltd`: `signin::write_allowed` |
| A pack partition per (browser, person); its hostnames are served only while that person has a live session on that browser | contract §8, deltd API §4.4 | `Register::session_for_partition` |
| A sign-in card signs in only a passwordless person, only to a plain session; a new card replaces the old at once; issuing and removing are secure only and scoped to the teacher's class | privilege boundary §10 | `issue_card`, `sign_in_card`, `remove_device` |
| Tap-your-name and its list answer the box's own screen only | privilege boundary §10, deltd API §6.1 | `tap_list`, `sign_in_tap` |
| Re-confirmation tickets: single use, one minute, bound to session, action and an argument digest; only an identity with a password can get one; only `deltos-shelld` can redeem one | privilege boundary §6, contract §4.4 | `issue_ticket`, `redeem_ticket`, `protocol::SHELLD_CALLS` |
| Sign-out, sign-out everywhere, a password change or reset, and removing a profile end sessions | privilege boundary §4.2 | `sign_out`, `sign_out_everywhere`, `set_password_hash`, foreign keys |
| Guest stores nothing and cannot "sign out everywhere" | contract §5.1, deltd API §6.1 | `sign_out_everywhere` |

## 4. The socket

One JSON object per line each way. A request is `{"call": "<name>", ...}`; the answer is `{"ok": ...}` or `{"error": {"code": "...", "detail": ...}}`, the code always one of deltd API §7's. A connection carries any number of requests in turn; a line over 16 KiB is refused.

| Caller | Calls |
|---|---|
| `deltd` (user `deltos-deltd`) | `session`, `session_for_partition`, `tap_list`, `password_allowed`, `sign_in_password`, `sign_in_card`, `sign_in_device`, `sign_in_tap`, `sign_out`, `sign_out_everywhere`, `issue_card`, `list_devices`, `remove_device`, `create_profile`, `list_members`, `change_password`, `set_password`, `issue_ticket` |
| `deltos-shelld` (user `deltos-shelld`, once it exists) | `session`, `has_permission`, `redeem_ticket` only: its second check (privilege boundary §5 step 6) and ticket redemption. It can write nothing. |
| anyone else | disconnected before a byte is read |

Calls that act on a browser carry a `ctx`: `https`, `kiosk`, `peer` (for rate limits only, never stored), and the `browser` and `session` cookies the browser sent.

**`deltd`'s side** (`deltlib::IdentitydLink`): one kept connection, reopened when `identityd` restarts. Every pack-zone request asks whether its partition is live, so answers are kept for 5 seconds and **all of them are dropped on any call that can change a session**; every sign-in and sign-out passes through `deltd`, so those take effect at once, and a session that simply runs out is still served for at most 5 seconds. If `identityd` cannot be reached, nobody is signed in: the API answers `not_signed_in`, sign-in answers `sign_in_unavailable` (503), and no pack hostname is served.

## 5. Decisions made here

Made 2026-11-21 under Commander's delegation of routine decisions. The first block answers contract §13.2's open item "how long a session lasts … with how long a remembered device lasts and the sign-in card's code length and validity", now recorded there. The rate limits and the password-hash cost are **starting values to be measured on the minimum spec**, not measured ones (deltd API §10 item 9 stays open for the measurement). Every number is in `crates/deltos-identity/src/policy.rs`.

| Decision | Chosen | Why |
|---|---|---|
| Session on the box's own screen (kiosk), plain or secure | ends after **10 idle minutes**, and after **8 hours** regardless | A shared screen left signed in is the classic way the next pupil reads someone's notes; 10 minutes outlasts reading a page, and 8 hours is a school day. |
| Plain session on a person's own device | **1 day idle, 7 days** absolute | The remembered device starts a new one without the card, so short limits cost a learner nothing and bound a copied cookie's life. |
| Secure session on a person's own device | **30 minutes idle, 12 hours** absolute | A teacher's laptop left open in a classroom; never longer than a plain session (privilege boundary §4.2). |
| Remembered device (`delt_device`) | ends after **90 days unused**, and after **1 year** regardless | Covers a holiday break; a phone that stopped coming back drops out on its own, and the teacher can remove any one at once. |
| Sign-in card code | **10 characters** from Crockford's base-32 alphabet (no I, L, O, U; typed letters O, I, L are read as 0, 1, 1), shown `XXXXX-XXXXX`: 50 bits | Short enough to type on a phone from paper; at 10 tries per 10 minutes per device it cannot be guessed. Kept as a keyed hash (HMAC-SHA-256), because 50 bits in a plain hash could be brute-forced from a stolen database. |
| Sign-in card validity | **14 days** from issue, usable on **any number** of that person's devices until it expires or is replaced | A teacher prints cards for a class once and pupils sign in over the following days, a phone and a home tablet each; every use shows up as a device the teacher can see and remove. |
| What the QR code holds | `http://home.deltos.lan/#card=<code>` | The code sits in the part of the address after `#`, which browsers never send to a server and never write into its logs; the WebOS reads it and signs in. |
| Rate limits (starting values) | **10 failed attempts per 10 minutes**, per device address and per person (per name for passwords, per device for cards); then refused until the window passes | Blocks repeated guessing (F15) without locking out a class of 40 phones; failures only, so a class signing in together is never slowed. |
| Password hash | **Argon2id, 19 MiB, 2 passes, 1 lane** (OWASP's minimum) | Fits many sign-ins at once on a 2 GB box. Starting value; measure the sign-in time on the minimum spec. |
| Password length | at least **10 characters**, at most 256 bytes | Teachers' and admins' passwords guard the box. |
| A passworded person signs in with | a **sign-in name**: 1–32 of `a-z 0-9 . _ -`, unique (contract §5.1 now records it) | Off the kiosk there is no list of names to pick from; display names are not unique and are never identifiers (contract §5.1). |
| Asking before the password box | `GET /api/v1/sign-in/check?sign_in_name=` says whether a password may be typed over this connection | So the WebOS can tell a teacher on plain HTTP to use their certified device **before** they type the password, which then never crosses the Wi-Fi. It tells anyone on the box whether a name belongs to a teacher or admin; on a school box those names are not secret. |
| "The box's own screen" | a request whose connection comes from **loopback** (the kiosk browser runs on the box and resolves `home.deltos.lan` to `127.0.0.1`) | `deltd` alone sees the connection; no header from the network is believed (privilege boundary §3). Above the Kiosk profile, a proxy in front must say so over its local socket, as for HTTPS. |
| Cookie names | `delt_session` / `__Host-delt_session` (already decided), plus `delt_browser` / `__Host-delt_browser` (the browser id, 400 days) and `delt_device` / `__Host-delt_device` (the remembered device), all host-only, `HttpOnly`, `SameSite=Strict`, `Path=/` | One naming rule for all three: the `__Host-` name and `Secure` over HTTPS, the plain name over HTTP, as privilege boundary §4.1 does for sessions. A kiosk session cookie ends with the browser. |
| Who may create a profile | an admin (`admin.console`) creates anyone; a teacher (`profile.manage` for a class) may add a **passwordless student to that class** and nothing else | Teachers enrol their own pupils without the admin; a teacher can never make another teacher or an admin. |
| Profile and card changes | all **secure only** | They hand out or take back a way into someone's account (privilege boundary §10 already says so for cards). |
| Removing a remembered device | also ends that person's **plain** sessions at once | A lost phone must stop working now, not when its session runs out; the person signs in again on their other devices with no card needed. |
| Forgetting this device (`DELETE /me/device`) | also signs this browser out | The session was started by that device. |
| A card for Guest | allowed, by an admin (`profile.manage` box-wide; Guest has no class) | Lets a library print one Guest card for walk-in visitors' own phones; Guest still sees only guest-accessible packs and stores nothing. |
| Two new error codes (deltd API §7) | `sign_in_failed` (401: wrong name, password or code, all said the same way) and `sign_in_unavailable` (503: `identityd` not reachable) | Adding codes is allowed within v1; neither existing code meant either. |
| Register files | `/var/lib/deltos/identity/identity.sqlite` and `card-key` | Inside contract §10's `/var/lib/deltos/identity/`. |
| Socket and group | `/run/deltos/identity/identityd.sock`, group `deltos-identc` | As `deltos-shellc` does for shelld (privilege boundary rule 1). |
| The box's certificates | `/var/lib/deltos/tls/`: `deltos.lan.crt`/`.key` and `deltos-packs.lan.crt`/`.key`, `0640 root:deltos-deltd` | Contract §8's two wildcards. **Proposed for contract §10** with the other new paths. Made and renewed by the box's own CA (`docs/box-certificates.md`). |

## 6. Setting up a box

Once per box, after the network settings (`docs/box-radio-and-boot.md` §4):

1. **The first admin:** `runuser -u deltos-identity -- /usr/lib/deltos/bin/deltos-identity-provision --display-name 'Name' --sign-in-name admin`. It works **only while the register has no admin** and refuses ever after. The password is generated (20 characters, 100 bits, `XXXXX-XXXXX-XXXXX-XXXXX`), printed **once**, and only its hash is kept. This is Track F §2's first-boot step in command-line form; when `deltos-shelld` and the kiosk screen exist, F2's full-screen one-time display wraps the same call.
2. **The certificates** in `/var/lib/deltos/tls/`, made by `deltos-tls.service` at boot before `deltd` (`docs/box-certificates.md`). Until they are there, `deltd` serves plain HTTP only and says that teachers and admins cannot sign in.

**At boot** the order is: settings → address → firewall → radio → DNS/DHCP → **`deltos-identityd`** → `deltd` (`docs/box-radio-and-boot.md` §2). `identityd` tells systemd when its socket is ready, so `deltd` never starts ahead of it. A box with no admin yet starts neither and shows neither as failed. `identityd` restarts on a crash like every other service, and sessions survive it (they are in the register).

## 7. The HTTPS listener

`deltd` now serves HTTPS itself on port 443 beside plain HTTP on port 80, at every profile (Track A §20.3). The listener tags every request with how it arrived, which is how sign-in knows for certain whether a request came over HTTPS. It picks the certificate by name: a `*.deltos-packs.lan` name gets the pack-zone certificate, anything else the `*.deltos.lan` one, so the certificate fronting the WebOS never fronts untrusted pack content (contract §8). Over HTTPS, WebOS answers carry `Strict-Transport-Security` for `home.deltos.lan` only (privilege boundary rule 17).

## 8. How it was tested

- `crates/deltos-identity/tests/register.rs` (18 tests): the privilege boundary §13 sign-in tests against the register itself, with a fake clock: secure and plain sessions; teacher and admin passwords refused over HTTP; rate limits and recovery; cards (issue secure only, plain session even over HTTPS, a replaced card and an expired card refused, never for a passworded person); devices (remembered, removed and refused at once); teachers limited to their own class; tap-your-name kiosk only; switching person on one browser; partitions shut at sign-out; idle and absolute limits; tickets reused, late, for another action, for other arguments or for another session, each refused; a passwordless person never re-confirms; a password change ending every session; the table-level CHECK; a newer schema refused; files `0600` and no secret in the clear on disk.
- `crates/deltos-identity/tests/socket.rs` (4 tests): a caller that is not `deltd` or `shelld` is disconnected; `deltd` cannot redeem tickets; `shelld` can do nothing but its three calls.
- `crates/deltlib/tests/signin.rs` (9 tests, the ninth with the box's own CA, `docs/box-certificates.md` §6): the real register on a real socket behind the real router: a student's whole day on a phone (card, cookies and their attributes, reading, the pack hostname, sign-out shutting it, the remembered phone, the teacher removing it); admin passwords refused over HTTP, and the check before typing; `__Host-` cookies and HSTS over HTTPS; **a plain session replayed over HTTPS refused**, and a secure session over plain HTTP refused; writes with a missing, foreign, wrong-scheme or pack `Origin`, or a missing or wrong anti-forgery token, each refused; tap-your-name kiosk only, and switching person ending the first; `identityd` down; and a real TLS handshake picking the certificate by zone.
- `system/tests/vm/run.sh`: the emulated ARM64 machine (2 GB) now installs `identityd`, provisions the first admin, and on the real second boot checks the start order, `identityd`'s user, powers, network and file permissions, the socket refusing another user, and then a virtual phone: admin refused over HTTP, admin signs in over HTTPS, adds a student and issues a card; the phone signs in with the card over plain HTTP, searches, reads its own pack hostname, signs out (hostname shut), and comes back as a remembered device; the box's own screen gets the tap-your-name list and the phone does not; `deltd` and `identityd` each killed and coming back with the phone still signed in. Results and timings are in the pull request.

## 9. Not built yet, and who it belongs to

| Item | Owner |
|---|---|
| Installing the box CA on the kiosk image, and the WebOS walkthrough for teachers' devices (issuing is built: `docs/box-certificates.md`) | Track E, WebOS |
| The WebOS sign-in screens, the card printing page and the teacher's device list | Track C (WebOS), later |
| `deltos-shelld`, which redeems tickets and makes the second check | Track C (C1) |
| Re-confirm on the admin endpoints (`confirm_password`), using `issue_ticket` | `deltd`, with each admin endpoint |
| F2's full-screen first-boot password display on the kiosk | Track F (F2) with C10 |
| The "Allow Guest access" setting that creates and removes Guest (F1) | Track F (F1) |
| Editing roles and bundles, removing profiles, moving a pupil between classes | Track F (F1, F3) |
| `entry_viewed` and progress events through the session (contract §5.2) | `deltd` (deltd API step 3) |
| Clearing a person's pack storage on the kiosk at sign-out (`clear_kiosk_partition`) | `deltos-shelld` (privilege boundary §9) |
| Admin alerts on repeated failed sign-ins (F15) | Track F (F15) |
| Sign-in for the other `*.deltos.lan` services (forward-auth hand-off from `identityd`) | Track G (G2) |
| Measuring the rate limits and the password-hash time on the minimum spec | Track F with Track D |
