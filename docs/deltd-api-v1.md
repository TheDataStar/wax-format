# deltd API v1

*Design, no code. Written against SPEC v0.12 and `docs/cross-track-contract.md` on `main` (merge `8ada2fe`). Drafted 2026-11-16 in the Delt design thread; reviewed and revised 2026-11-20 before landing here. The review record is §12.*

**What this document owns:** the public, versioned API of `deltd`, the one server on the box that serves every pack and answers every Library request from the WebOS, the Classroom app and the Wax Service Shell (WSS); and how `deltd` serves pack content.

**What it does not own:** contract values (cited, never restated as binding), the `.wax` bytes (SPEC.md), and the checks for changing the box, which are `docs/privilege-boundary.md`'s. Every admin endpoint here only checks and hands off to `deltos-shelld` as that document says.

---

## 1. In short

- **Two surfaces.** The API sees who is signed in; pack content never does. That split keeps an untrusted pack away from anyone's session.
- **Pack pages are served exactly as built,** with exact caching, byte ranges for video, and `410 Gone` for pages an update removed. They are served only to people who signed in, on hostnames issued to them.
- **Every endpoint names its permission,** and anything unnamed is denied. Endpoints that change the box, or show a class's progress, also need HTTPS and a sign-in made over HTTPS.
- **Notes stay private.** Teachers see who opened an item they assigned, never what anyone wrote and never anything else anyone read.
- **The two contract problems the first draft found are fixed in the contract** in the same pull request: people are no longer capped at 32 per box, and progress is credited through the signed-in session (contract §8, §5.2).

## 2. Where things live

`deltd` answers on two kinds of hostname, and the split is the main security property. It does its own hostname routing, so this holds at the Kiosk profile too, where there is no reverse proxy (Track A §20.3).

| Surface | Hostname (contract §8) | Sees the sign-in? | Serves |
|---|---|---|---|
| **WebOS and API** | the WebOS hostname on `deltos.lan` — **one origin for both**; the name itself is still open (§10) | Yes: `identityd`'s session cookie, set on this hostname only | The WebOS's own files, and `/api/v1/...` JSON |
| **Pack content** | `pack-<pack_index>-g<pack_generation>-<pack_partition>.deltos-packs.lan` | Never. No cookie is set or read on this zone. | The pack's pages and media, raw, inside the WebOS's sandboxed frame |

- **One origin for the WebOS and the API** means no cross-origin requests between them, so no CORS (the browser rules that let one site call another) is ever turned on. `deltd` answers no CORS request on any surface.
- **The kiosk uses the same hostname,** over HTTPS (privilege boundary §10). There is no separate loopback address for the box's own screen any more (contract §8).
- **Who the caller is.** The API learns the person from `identityd`'s session, never from a field in the request. The pack zone learns nothing about who is reading beyond the pack partition in the hostname, which keeps one person's in-pack storage apart from another's and is never treated as proof of identity on its own. Anything tied to a person (a note, a progress event) is written through the API.
- **Plain HTTP.** Visitors' phones reach both surfaces over plain HTTP; HTTPS where the box's certificate is installed (contract §16). No endpoint a visitor uses may need a browser feature that requires a secure connection.

## 3. Rules every endpoint follows

1. **Public and versioned.** Everything is under `/api/v1`. Within v1, changes are additive only: new endpoints, new optional fields, new error codes. Removing or changing anything is v2, served alongside v1 until callers move.
2. **Described by a machine-readable document.** `GET /api/v1/openapi.json` is the contract for callers; CI checks the server against it.
3. **JSON in and out,** UTF-8, field names in `snake_case`. Pack identity is always the canonical hyphenated `archive_uuid` (contract §11). Requests with a body must send `Content-Type: application/json`.
4. **Permission per endpoint.** Every endpoint names the permission it needs (contract §4), or says it needs none. Default is deny. The check runs against the signed-in person's **effective permissions, and scope** where the permission is scoped (for example `progress.view` for one's own class), looked up live from `identityd` on every request.
5. **Secure only where it changes the box.** Endpoints marked **Secure** below need an HTTPS request **and** a session made over HTTPS (privilege boundary §4.1). Otherwise they answer `secure_connection_required`.
6. **Writes are protected against forgery.** Every `POST`, `PUT`, `PATCH` and `DELETE` needs the session, an `Origin` header exactly equal to the WebOS's own origin, and the session's anti-forgery token in `X-Delt-CSRF` (privilege boundary §4.3, §7 rule 7). `GET` and `HEAD` never change anything.
7. **Lists page with a cursor**, never a page number, so results stay stable while packs update. Each list returns `items` and `next`.
8. **Long jobs return a job.** Installs, updates, conversions, backups and exports answer `202` with a job id; `GET /api/v1/jobs/{id}` reports state in contract §7.2's words where they apply.
9. **Language follows the person.** Error titles and labels come in the language the person chose, falling back to `Accept-Language` (contract §16, translatable interface).
10. **Caching is exact, and private data is never cached.** Anything derived from pack content carries an ETag built from the content hashes it came from. Everything under `/me`, `/session` and `/classes` is `Cache-Control: no-store`, so a shared browser never keeps someone's notes or a class's progress.
11. **Nothing leaves the box.** The API makes no outbound calls except the catalog sync and peer transfers an admin set up (contract §16).
12. **Safe page headers.** Every WebOS and API response carries `Content-Security-Policy` with `frame-ancestors 'none'` (and `frame-src` limited to the pack zone on WebOS pages), `X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer`; over HTTPS, also `Strict-Transport-Security` for the WebOS hostname only (privilege boundary §7 rules 14, 17).

## 4. Pack content

On the pack zone `deltd` is a plain, fast file server: `GET` and `HEAD` on the entry's path, nothing else. It never rewrites a page and never injects a toolbar. The WebOS draws the reading tools around the pack's frame, which is how the pack stays exactly as its author built it.

### 4.1 What a request finds

| The request finds | Response |
|---|---|
| A normal entry | `200` with the entry's `mime` type, or `206` for a byte range |
| An alias (redirect) | `301` to the target's path; one hop at most (SPEC §5.3) |
| A deleted page (tombstone, SPEC §5.5.1) | `410 Gone`, so a bookmark to a removed page says it was removed rather than "not found" |
| Nothing | `404` with a small page in the box's default language (the pack zone does not know the person) |
| `/` | `302` to the manifest's `entry_point` |
| A corrupt entry (hash mismatch) | `500` and a health event; never the bad bytes |
| A compression method it does not know | `500`; refuse rather than guess (SPEC §3) |
| A hostname it did not issue, or no longer serves (§4.4) | `404`, the same page as "nothing", before any lookup |
| Any method other than `GET` or `HEAD` | `405` |

### 4.2 Headers on every pack response

- **`ETag`** is the entry's `sha256` for the plain representation, and the same value with a `-zst` suffix for the zstd-encoded one, with **`Vary: Accept-Encoding`**. Two encodings of the same page are two different byte strings, so they need two tags; the hash part stays the same across editions when the page did not change. `If-None-Match` answers `304`.
- **`Accept-Ranges: bytes`** on plain (not encoded) responses, so video seeks. Media is stored uncompressed, so a range is a direct read. A range request for a text entry is answered from its decompressed bytes.
- **`Content-Security-Policy`** keeps a pack inside its own origin: every fetch directive limited to `'self'` (plus `data:` and `blob:`), `form-action 'self'`, `base-uri 'self'`, and `frame-ancestors` limited to the WebOS origin. A pack cannot phone home, reach the API, post a form to the API, or be framed by anything but the WebOS.
- **`Cross-Origin-Resource-Policy: same-origin`**, `X-Content-Type-Options: nosniff` and `Referrer-Policy: same-origin`.
- **No `Set-Cookie`, ever,** and any incoming `Cookie` header is dropped before lookup.
- **`Cache-Control`:** HTML entries are `no-cache` (the browser keeps them but asks every time, and a `304` is cheap), so a hostname that stops being served (§4.4) stops working at once. Other entries may be kept until their ETag changes.

Search, contents lists and everything else about a pack are on the API, not the pack zone.

### 4.3 Compression and plain HTTP

The draft promised "compressed bytes passed straight to the browser". That holds less often than it sounds, and the difference matters for speed on a 2 GB box:

- Browsers advertise `zstd` in `Accept-Encoding` **only over HTTPS**. Visitors use plain HTTP (contract §16), so for them `deltd` always decompresses on the box. **To be confirmed on the target phone browsers during testing.**
- Entries stored with the shared dictionary (ADR-0001, SPEC §3) are always decompressed on the box, since a browser cannot use the pack's dictionary.
- So passthrough applies only to plain-zstd entries sent to HTTPS clients. Read-speed measurements for `deltd` must include box-side decompression per request on the minimum spec, since that is the common visitor path.

### 4.4 Which pack hostnames are served

A pack hostname carries three parts (contract §8): the pack's `pack_index`, its `pack_generation`, and a `pack_partition`, a random value `identityd` issues for one person on one browser (privilege boundary §4.4). `deltd` serves a pack hostname only when all of these hold:

1. the `pack_index` holds an installed pack and the generation is that index's current one;
2. the partition was issued and belongs to a person with a **live session on the browser it was issued for**;
3. that person may see this pack (§5).

What this gives, in plain words:

- **A device that never signed in cannot read packs.** There is no anonymous way in (Track F §22).
- **`guest_accessible` is enforced,** not just a launcher filter: a Guest's partition is refused on a pack Guest may not see.
- **Signing out shuts that person's pack hostnames on that browser.** Their in-pack storage stays in the browser, but no page of theirs will load again for the next person using it, because HTML is revalidated on every load (§4.2). On the kiosk, `identityd` also has the box clear it (privilege boundary §9).
- **A new pack at a reused index never inherits a removed pack's storage,** because its generation is higher.

`deltd` keeps the live set in memory, updated by `identityd` at every sign-in and sign-out, so the check costs no extra round trip per request.

## 5. Who may see which packs

- **Signed in** means any live session, Guest included. A Guest sees only packs marked `guest_accessible` (contract §11), or that an admin has opened to Guest in the catalog, and can store nothing.
- v1 has **no per-pack restriction beyond Guest.** Every other signed-in person may see every installed pack. Restricting a pack to some people (a teacher's answer key, say) is an open product question (§10).

## 6. Endpoints

Eight groups. "Signed in" means any live session (§5). "Not Guest" excludes the Guest profile. **Secure** marks the endpoints of §3 rule 5. Permission names are contract §4's; `lists.assign` is contributed by the Classroom app's manifest (contract §15). All paths are under `/api/v1`.

### 6.1 Session

These are `identityd`'s: `deltd` forwards them to `identityd` over its local socket and adds nothing.

| Endpoint | Does | Needs |
|---|---|---|
| `GET /session` | Who is signed in: display name, effective permissions, whether the session is plain or secure, when it ends, and the anti-forgery token | None (answers `not_signed_in`) |
| `POST /session` | Sign in. Makes a new session id every time; sets `delt_session`, or `__Host-delt_session` over HTTPS | None; rate-limited per device |
| `DELETE /session` | Sign out of this browser; shuts this person's pack hostnames on it (§4.4) | Signed in |
| `DELETE /me/sessions` | Sign out everywhere | Signed in, not Guest |
| `GET /sign-in/profiles` | The tap-your-name list for passwordless sign-in | None, **kiosk only** until the student sign-in decision is made (§10) |

**How a passwordless person signs in on their own phone** is pending Commander's decision (privilege boundary §10). Until then, `GET /sign-in/profiles` is answered only on the kiosk, because listing every student's name to any device on the Wi-Fi is itself a privacy leak. **Teacher and admin passwords are HTTPS only** (decided 2026-11-20): `POST /session` with a password for a person who holds any secure-only permission is refused over plain HTTP with `secure_connection_required`.

### 6.2 Library

| Endpoint | Does | Needs |
|---|---|---|
| `GET /packs` | Installed packs the person may see, filterable by language, category, outdated | Signed in |
| `GET /packs/{uuid}` | Manifest, edition, size, live page count, search status and tokenizer, and this person's **pack hostname on this browser** | Signed in |
| `GET /packs/{uuid}/pages?prefix=` | Page list in path order, for browsing and A to Z indexes | Signed in |
| `GET /packs/{uuid}/random` | A random article (uses the article list once the format adds it, ask F6) | Signed in |
| `GET /catalog` | Packs available from the catalog, with the box's verdict: fits, needs more storage, needs more RAM | Signed in |
| `GET /updates` | Every outdated pack with the exact download size and whether it applies to this edition (SPEC §7.4) | `content.install` |

### 6.3 Search

| Endpoint | Does | Needs |
|---|---|---|
| `GET /search?q=&packs=&lang=&category=&cursor=` | One query across every pack the person may see, fused by rank (Reciprocal Rank Fusion, contract §13 decision 6), grouped by pack, with snippets. Packs whose tokenizer the box cannot run are listed as skipped, never silently left out. | Signed in |
| `GET /suggest?q=` | Title completion across packs, using each pack's declared title fold | Signed in |
| `GET /search/packs/{uuid}?q=` | The same, inside one pack | Signed in |

### 6.4 Reading

| Endpoint | Does | Needs |
|---|---|---|
| `GET /packs/{uuid}/outline?path=` | The page's sections, for the contents list and for anchoring notes (needs the chunks table, ask F3) | Signed in |
| `GET /packs/{uuid}/reader?path=` | The page's readable text and images without layout, for reader mode and large text | Signed in |
| `GET /packs/{uuid}/speech?path=` | Sentences with audio from the box's text-to-speech, so the WebOS can highlight each sentence as it plays | Signed in, when the box's measured resources allow it |
| `POST /progress-events` | The WebOS reports `pack_opened` and `pack_closed`, and relays `unit_completed` and `quiz_scored` from a pack frame (§8.2) | Signed in, not Guest |

### 6.5 Personal (private to the person, never shown to anyone else)

| Endpoint | Does | Needs |
|---|---|---|
| `GET/POST/PATCH/DELETE /me/bookmarks` | Saved pages | Signed in, not Guest |
| `GET/POST/PATCH/DELETE /me/highlights` | Highlighted passages, each with an optional note | Signed in, not Guest |
| `GET/POST/PATCH/DELETE /me/lists` | The person's own reading lists | Signed in, not Guest |
| `GET /me/history` and `DELETE /me/history` | Pages read, newest first; clearable | Signed in, not Guest |
| `GET /me/needs-attention` | Highlights whose passage changed in an update and need re-anchoring | Signed in, not Guest |
| `GET /me/export` | Everything above as one file, so a learner can take their notes with them | Signed in, not Guest |

A highlight's anchor stores the pack, the page path, the section id and the quoted text. After an update the box re-finds the quote; if it cannot, the highlight is flagged, never moved silently. No permission, `admin` included, reads another person's `/me` data; the endpoints take no person argument at all.

### 6.6 Classroom (for the Classroom app)

Classes are `identityd`'s `class_or_group` (Track F §3).

| Endpoint | Does | Needs |
|---|---|---|
| `POST /classes/{id}/lists` | Assign a reading list to a class | `lists.assign`, for that class |
| `GET /classes/{id}/lists` | Assigned lists, for teacher and students | Member of the class, or its teacher |
| `GET /classes/{id}/lists/{list}/progress` | For each item on that list, who has opened it and who has completed it | `progress.view`, for that class · **Secure** |

The progress view reports **only the items on the assigned list.** The events behind it record more (contract §5.2), but no endpoint shows a teacher anything else a student read, and never their notes.

### 6.7 Export

| Endpoint | Does | Needs |
|---|---|---|
| `POST /exports` | One page, a section, or a reading list as HTML, EPUB or PDF. Returns a job. Licence and attribution are written into every file. | Signed in, not Guest |
| `GET /exports/{id}/file` | Download the finished file | The person who asked |

PDF is built on the box when its measured resources allow; otherwise the job returns a print-ready page and says so (architecture doc, Q5). How long a finished file is kept is open (§10).

### 6.8 Admin

These endpoints never act themselves. They check, then hand the job to `deltos-shelld` (privilege boundary §5). **All of them are Secure.** Endpoints marked "re-confirm" need the person's password in the request's `confirm_password` field (privilege boundary §6).

| Endpoint | shelld action | Needs |
|---|---|---|
| `POST /installs` | `install_pack`, from a catalog, drive, peer or staged-upload id | `content.install` |
| `POST /uploads` | `stage_upload` | `content.install` |
| `DELETE /packs/{uuid}` | `remove_pack` | `content.install` · re-confirm |
| `POST /packs/{uuid}/rollback` | `roll_back_pack` (SPEC §7.5) | `content.install` |
| `GET /drives` and `GET /drives/{id}/files` | listing; hands out the ids actions take | `content.install` |
| `POST /drives/{id}/mount` and `POST /drives/{id}/eject` | `mount_removable` / `eject_removable` | `content.install` |
| `POST /conversions` | `convert_zim_from_drive` (decided 2026-11-15) | `content.install` |
| `GET /network/scan` | listing | `network.configure` |
| `POST /network/join` and `PUT /network/hotspot` | `join_network` / `configure_ap` | `network.configure` · re-confirm |
| `POST /services/{name}/restart` | `restart_service` | `service.restart` |
| `POST /fleet/pairings` | `pair_box` | `fleet.manage` · re-confirm |
| `GET /backups?drive=` | listing | `backup.manage` |
| `POST /backups` | `start_backup` | `backup.manage` |
| `POST /backups/{id}/restore` | `restore_backup`, with the staged review | `backup.manage` · re-confirm |
| `GET /system-updates` | listing | `system.update` |
| `POST /system-updates/{id}/apply` and `POST /system-updates/rollback` | `apply_update` / `roll_back_update` | `system.update` · re-confirm |
| `GET /audit` | Read the action log | `admin.console` |

Two more endpoints serve everyone:

| Endpoint | Does | Needs |
|---|---|---|
| `GET /jobs/{id}` | State of a job | The person who started it; anyone else needs `admin.console` and Secure |
| `GET /status` | Version, measured capability, which features are on and why | Signed in; `benchmark.view` for the full detail |

## 7. Errors

Every error has the same JSON shape and a code from one closed list, so the WebOS can react to the code and show the message. It follows the standard problem format (RFC 9457), served as `application/problem+json`: `status`, `code`, `title` in the person's language, `detail`, and `job` or `pack` where relevant. Pack-zone errors are small HTML pages instead (§4.1).

| Code | HTTP | Means |
|---|---|---|
| `invalid_request` | 400 | A field is missing or malformed; `detail` names it |
| `not_signed_in` | 401 | No live session, or it has ended |
| `permission_denied` | 403 | The person lacks the permission the endpoint names, or holds it for a different scope |
| `guest_not_allowed` | 403 | Guest tried to store something, or asked about a pack Guest may not see |
| `request_rejected` | 403 | Wrong or missing `Origin`, or wrong or missing anti-forgery token. Deliberately says no more |
| `secure_connection_required` | 403 | The endpoint, or this sign-in, needs HTTPS and a session made over HTTPS |
| `confirmation_required` | 403 | The action needs the password again, or the one given was wrong |
| `pack_not_found` | 404 | No installed pack with that id |
| `page_not_found` | 404 | No page at that path |
| `page_removed` | 410 | The page was deleted in an update |
| `edition_mismatch` | 409 | An update does not build on the edition installed (SPEC §7.4) |
| `search_unavailable` | 409 | The pack has no search index, or its tokenizer cannot run here |
| `signature_invalid` | 422 | Signature check failed; the job ends in `failed_signature` and waits for an admin |
| `not_enough_resources` | 503 | The feature's declared floor is above what this box measured; `detail` gives both numbers |
| `not_enough_storage` | 507 | An install, upload or conversion would exceed measured free storage |
| `too_many_requests` | 429 | Too many attempts or requests from one device or person (F15, §9) |
| `pack_damaged` | 500 | A hash check failed; a health event is raised |
| `internal` | 500 | Anything else; logged on the box |

Adding a code is allowed within v1, since callers must treat an unknown code as `internal`. Changing what an existing code means is not.

## 8. Progress events

### 8.1 What is recorded

Progress events use contract §5.2's schema unchanged except for where `profile_id` comes from: **always from a live signed-in session, never from a field in the event and never from a pack address alone** (contract §5.2, approved 2026-11-20). `pack_id` is likewise filled in by `deltd` from the pack hostname, never taken from the message. Guest emits nothing (contract §5.1).

### 8.2 Three delivery paths

Most WAX packs are static pages that send nothing, and a framed pack on another origin is invisible to the WebOS around it. So each event type has one path:

| Event | Delivered by | `profile_id` comes from |
|---|---|---|
| `pack_opened`, `pack_closed` | The WebOS, `POST /progress-events` | The request's session |
| `unit_completed`, `quiz_scored` | The pack, by `postMessage` to the WebOS, which checks it (privilege boundary §7 rule 8) and relays it with `POST /progress-events`, naming the frame's pack hostname | The request's session; `deltd` also checks that the named hostname's partition belongs to that same person |
| `entry_viewed` | `deltd` itself, when it serves an HTML entry on a pack hostname | The live session the hostname's partition was issued under (§4.4); nothing is recorded if there is none |
| (Kolibri events) | Track H's bridge, unchanged | Contract §5.1's adapter mapping |

The `entry_viewed` path does use the pack hostname, but only as a pointer to a live session: the partition is random, issued for one person on one browser, and counts only while that person is signed in there. Someone who copies a partition off the Wi-Fi can add false "viewed" rows for that person, and nothing more; it is recorded here so nobody treats `entry_viewed` as proof.

## 9. Load and limits

The box's floor is 2 GB of RAM (contract §2.1), and one class can mean 40 phones at once.

- **Sign-in, re-confirmation and the sign-in card** are rate-limited per device and per person (F15).
- **Search, speech and export** are limited per person so one device cannot starve the rest.
- The numbers for both are **measured on the minimum spec, not chosen here** (§10).

## 10. Open questions

Not answered here, and not to be filled in by whoever implements first. Items that are contract values are also in contract §13.2.

| # | Question | Why it matters | Recommendation | Owner |
|---|---|---|---|---|
| 1 | **The WebOS hostname** | Needs a fixed name every client can reach; this API shares it | Name it in contract §8 and the local name service (E10) before C1 is built | Contract §8 |
| 2 | **Where personal data lives** | Notes and lists need a path, a schema version (contract §10) and a place in backups | A `deltd` directory under `/var/lib/deltos/`, added to contract §10 and declared in `deltd`'s backup set (§15) | Contract §10 |
| 3 | **Who gets `lists.assign` by default** | Teachers need it on a fresh box | The shipped `teacher` bundle | Contract §4 |
| 4 | ~~Teacher and admin passwords over plain HTTP~~ | **Decided 2026-11-20: HTTPS only** (§6.1) | — | Closed |
| 5 | **How students sign in on their own phones** | Decides what replaces tap-your-name off the kiosk (§6.1) | A teacher-issued sign-in card | Commander, pending |
| 6 | **How long a session lasts** | Shared screens and personal devices need different limits | Choose with teachers | Contract, Track F |
| 7 | **Restricting a pack to some people** | v1 lets every signed-in person see every pack (§5) | Decide whether v1 needs it at all | Commander |
| 8 | **How long an export file is kept** | Storage on a 32 GB box | Measure, then choose | Track C |
| 9 | **Rate-limit numbers** (§9) | Fairness at 40 phones on 2 GB | Measure on the minimum spec | Track D, Track F |
| 10 | **Snippet speed on an SD card** | Snippets are cut at query time | Measure before release; ship titles-only results if too slow | Track D |
| 11 | **Which PDF engine runs on the box** | Size and memory at 2 GB | Measure candidates against the floor | Track C |
| 12 | **Section ids for highlights** | `outline` and anchors need the chunks table | Waits on format ask F3 | Track A |

The first draft's questions 1 and 2 (the 32-profile limit and progress tied to a pack address) are closed: both are fixed in the contract (§12).

## 11. Build and test plan

`deltd` is built in four steps. Each ends with the server measured on a 2 GB ARM64 box serving phones over plain HTTP, because that is where it has to work.

1. **Pack serving.** Hostname routing and the §4.4 checks, every row of §4.1, ETags per encoding, ranges, and the zstd and dictionary paths. Tested against the conformance corpus, including tombstones and rolled-back editions.
2. **Sessions, library and search.** §6.1 through §6.3 and `/status`, with snippets behind a switch until their speed is measured. Tested with packs in several languages and tokenizers, and with a pack whose tokenizer the box cannot run.
3. **Personal data and reading.** Bookmarks, highlights, lists, history, export of one's own data, reader mode, read-aloud, and the three progress paths. Highlights are tested across a real edition update, including the flagged case.
4. **Admin, Classroom and export.** Every §6.8 endpoint through `deltos-shelld`, built to `docs/privilege-boundary.md`. Classroom lists, then HTML, EPUB and PDF export.

**In CI on every change:**

- the server is checked against `openapi.json`;
- every endpoint is called without the permission it names, and with it for the wrong scope, and must answer `permission_denied`;
- every Secure endpoint is called over plain HTTP, and over HTTPS with a plain session, and must answer `secure_connection_required`;
- every write is sent with no `Origin`, a wrong one, a pack one, and with no or a wrong `X-Delt-CSRF`, and must answer `request_rejected`;
- every pack response is checked for no `Set-Cookie`, the §4.2 headers, and a distinct ETag per encoding;
- a pack hostname with an unknown partition, a signed-out partition, a stale generation, and a Guest partition on a non-Guest pack must each answer `404`;
- every `/me`, `/session` and `/classes` response is checked for `Cache-Control: no-store`;
- a fuzzer sends malformed hostnames, paths, headers and JSON bodies. `deltd` is the network-facing part (privilege boundary §11), so it is fuzzed first and hardest.

## 12. Review record (2026-11-20)

What the review changed from the 2026-11-16 draft, and why. Findings that belong to the privilege boundary are recorded there (its §15).

| # | Finding | Severity | Change |
|---|---|---|---|
| 1 | No endpoint was marked as needing HTTPS, although Commander decided changing the box and class progress need it. | Critical | §3 rule 5; Secure on every admin endpoint and on class progress. |
| 2 | The pack zone served any device on the Wi-Fi, so there was in fact an anonymous way in, and `guest_accessible` could not be enforced. | High | Pack hostnames carry an issued partition, served only while its person is signed in (§4.4, §5). |
| 3 | There were no sign-in or sign-out endpoints at all. | High | §6.1. |
| 4 | The API and the WebOS were on different proposed hostnames, which forces cross-origin requests. | High | One origin for both (§2). |
| 5 | Forgery protection was a fixed header, while the privilege boundary used a per-session token. | Medium | One mechanism (§3 rule 6). |
| 6 | The same ETag was used for the plain and zstd-encoded bytes, which breaks HTTP caching. | Medium | One ETag per encoding, with `Vary` (§4.2). |
| 7 | "Compressed bytes passed straight through" does not happen for plain-HTTP visitors, whose browsers do not accept zstd. | Medium | §4.3; flagged to the compression measurement. |
| 8 | Teachers could see a student's whole reading history through progress events. | Medium | Class progress shows only assigned items (§6.6). |
| 9 | Private notes could stay in a shared browser's cache. | Medium | `no-store` on private responses (§3 rule 10). |
| 10 | Pack pages could be framed by any site, and post forms to the API. | Medium | `frame-ancestors` and `form-action` in the pack policy (§4.2). |
| 11 | Static packs send no progress, so `entry_viewed` had no real source. | Medium | Three delivery paths (§8.2). |
| 12 | Scoped permissions ("own class") were checked as plain yes/no. | Medium | Scope checked (§3 rule 4; §6.6). |
| 13 | Several admin actions in the privilege boundary had no endpoint, and listings had none. | Low | §6.8 completed. |
| 14 | No error for refused origins, insecure connections or a missing re-confirmation. | Low | Three codes added (§7). |
| 15 | Export was open to Guest, who can store nothing. | Low | Not Guest (§6.7). |
| 16 | No stated load limits for 40 phones at the 2 GB floor. | Low | §9, numbers to be measured. |
