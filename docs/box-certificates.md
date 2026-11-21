# The box's own certificates

_Built 2026-11-21 (`crates/deltos-tls`, `system/systemd/deltos-tls.*`). Decisions taken under Commander's delegation of routine decisions; each is recorded with its reason below._

HTTPS is what lets a teacher or admin change the box (contract §4.2, §16): their passwords are typed only over HTTPS, and box-changing actions need a session made over HTTPS (`docs/identityd.md`). HTTPS needs certificates, and a box with no internet connection cannot get them from a public authority. So **every box makes its own small certificate authority (CA)** the first time it starts, and from it the two server certificates contract §8 asks for. A device trusts the box once, by installing the CA's public certificate, and from then on trusts every certificate that box makes.

In plain words: the CA is the box's own stamp. The server certificates are what `deltd` shows a browser, stamped with it. A device that has been told "trust this stamp" accepts them without a warning.

## 1. What is made

| Thing | Names it covers | Lives | Kept at |
|---|---|---|---|
| The box CA | may vouch **only** for names under `deltos.lan` and `deltos-packs.lan` (name constraint, §2) | 10 years | `/var/lib/deltos/ca/ca.key` (0600 root), `ca.crt` (0644) |
| WebOS and services certificate | `*.deltos.lan`, `home.deltos.lan` | 397 days | `/var/lib/deltos/tls/deltos.lan.key` (0640 root:deltos-deltd), `.crt` |
| Pack-zone certificate | `*.deltos-packs.lan` | 397 days | `/var/lib/deltos/tls/deltos-packs.lan.key`, `.crt` |
| The CA's public certificate, for devices | — | — | `/var/lib/deltos/tls/ca.crt` (0644) |
| What was issued and when it ends | — | — | `/var/lib/deltos/tls/issued.json` |

Two certificates, never one, so the certificate fronting the WebOS never fronts untrusted pack content (contract §8). `deltd` already picks between them by name (`deltlib::tls`).

**The CA key never sits where `deltd` can read it.** `/var/lib/deltos/ca/` is root's alone and is made inaccessible to `deltd`'s sandbox; only the server keys are in `deltd`'s group.

## 2. Decisions

| Decision | Chosen | Why |
|---|---|---|
| Who issues the box's certificates at the Kiosk and Classroom profiles | **The box's own CA, made by `deltos-tls-provision`**, not Track G's proxy | Track G's proxy (G2) is dormant at the Kiosk profile (track-g §9) and lands at P2, but HTTPS is needed from the first admin onward at every profile. |
| What happens when Track G's proxy (G2) arrives | **G2 reuses this root**: Caddy's internal CA is configured with an intermediate signed by the box CA (Caddy `pki` app, `intermediate` cert/key), never a new root | So a device enrols once for the box's life, not again when G2 lands. The box CA's path length is 1 to allow exactly that one intermediate; the root key stays root-only. |
| Name constraint | **Permitted: `deltos.lan`, `deltos-packs.lan`; nothing else** | A device that trusts the box CA cannot be fooled by it about any other website, even if the CA key were stolen. Modern browsers and operating systems enforce it; tested with the same verifier browsers' rules come from (rustls/webpki) and with OpenSSL. |
| Key type | **ECDSA P-256** | Accepted by every current browser and OS, fast to make on a 2 GB ARM box, small handshakes. |
| CA lifetime | **10 years** | A teacher enrols a device once for the box's working life. |
| Server certificate lifetime | **397 days** | Just under the 398-day ceiling Apple and browsers apply even to private CAs, so no device ever refuses one for being too long. |
| Renewal | **When fewer than 30 days remain**, checked at every boot and daily | A box can be off for weeks; a month's margin covers that. Renewal makes new server keys under the same CA, so enrolled devices notice nothing. |
| Start of validity | **One day before now** | A phone whose clock is a little behind the box's does not see a certificate that is "not yet valid". |
| A box with no working clock | **Never date anything before 2026-11-01 (the clock floor).** Below it, certificates are dated from the floor, marked provisional, and reissued from the real time once the clock is right | A Pi without a battery-backed clock can boot believing it is 1970; certificates made then look expired to every phone. |
| Half a CA (key without certificate, or the reverse) | **Refused; nothing is replaced** | Silently starting a new CA would make every enrolled device distrust the box. The admin must restore it, or remove both on purpose. |
| Renewal and `deltd` | `deltd` reads its certificates at start; the provisioner queues `systemctl try-restart deltos-deltd.service` only when something was reissued | Sessions live in `identityd`, so a restart signs nobody out (VM-tested). |

## 3. When it runs

`deltos-tls.service` (oneshot, root) runs at every boot **before `deltd`**, and `deltos-tls.timer` runs it daily (with up to an hour's random delay; `Persistent`, so a box that was off catches up). Each run:

1. makes the CA if there is none (and refuses if there is half of one);
2. reissues a server certificate if it is missing, has fewer than 30 days left, was made under an untrusted clock that is now trusted, or was issued by a different CA;
3. writes the public `ca.crt` beside the server certificates;
4. prints the CA's SHA-256 fingerprint (`journalctl -u deltos-tls`);
5. if anything was reissued, queues a restart of `deltd`; otherwise changes nothing.

The service is root because it writes the CA key and hands server keys to `deltd`'s group (`fchown`, so the only power it keeps is `CAP_CHOWN`). It has no network and can write only `/var/lib/deltos/ca/` and `/var/lib/deltos/tls/`. Every file is written whole (temporary file, then rename), with its mode set exactly whatever the unit's umask, so `deltd` never reads half a certificate.

## 4. Where the fingerprint comes from

The fingerprint is how a person knows the certificate on their device is really this box's and not one handed to them by someone else on the Wi-Fi. It must be read from **the box itself**, never only from the network:

- **Now:** the box's console. `deltos-tls-provision` prints it at every run (`journalctl -u deltos-tls`), and `install.sh` says where to look.
- **Later (not built):** the kiosk's WebOS shows it, and F2's first-boot screen shows it with the QR code (track-g §3, track-f §2).

## 5. Trusting the box on a device

`deltd` offers the CA on the WebOS host, **over plain HTTP and without a session**, because a device that does not trust the box yet cannot use HTTPS with it:

- `GET http://home.deltos.lan/api/v1/certificate` → `{"fingerprint_sha256": "AB:CD:…", "download": "/api/v1/certificate/ca.crt", "https": true}`
- `GET http://home.deltos.lan/api/v1/certificate/ca.crt` → the CA certificate, DER, `application/x-x509-ca-cert`

Neither is secret. Over plain HTTP both could be swapped by an attacker on the Wi-Fi, which is exactly why the person compares the fingerprint with the one the box shows itself (§4) before trusting it. The answer over HTTP proves nothing by itself.

How a person installs it, for the WebOS walkthrough to say in its own words later (privilege boundary §10):

| Device | Steps |
|---|---|
| iPhone, iPad | Open the download link in Safari, allow the profile, then Settings → General → VPN & Device Management → install it; then Settings → General → About → Certificate Trust Settings → turn on full trust for "DeltOS box …". |
| Android | Settings → Security (or Security & privacy) → Encryption & credentials → Install a certificate → CA certificate, and pick the downloaded file. Chrome trusts it; Firefox needs "Use third-party CA certificates" turned on. |
| Windows | Open the file → Install Certificate → Local Machine or Current User → "Trusted Root Certification Authorities". |
| macOS | Open the file (it goes into Keychain Access) → find "DeltOS box …" → Trust → "Always Trust". |
| ChromeOS | Settings → Privacy and security → Security → Manage certificates → Authorities → Import, and tick "Trust this certificate for identifying websites". |
| Linux | Firefox: Settings → Privacy & Security → Certificates → View Certificates → Authorities → Import. Chrome and the system: the distribution's CA store. |

The kiosk browser ships with the box's CA already trusted (privilege boundary §10); the image build copies `/var/lib/deltos/ca/ca.crt` into the kiosk browser's store when the kiosk is built (Track E, not built).

## 6. Tests

- `crates/deltos-tls/tests/ca.rs` (7 tests): the CA and two certificates, each verified for its own zone and refused for the other; **a certificate for `bank.example.com` signed with the stolen CA key is refused** (the name constraint); nothing changes until renewal is due, then only the server certificates renew, under the same CA, still trusted by a device that trusted it once; a box booting at 1970 still makes certificates a phone accepts, and reissues them once its clock is right; key modes, including under a strict umask; half a CA refused.
- `crates/deltlib/tests/signin.rs`: a device downloads the CA over plain HTTP with no session, its fingerprint is the one the box reported, and a client trusting only that file reaches both zones over HTTPS.
- `system/tests/vm/` (emulated ARM64, 2 GB): the certificates are made at boot before `deltd` by the real unit; the timer is scheduled; the CA key is root's alone and invisible to `deltd`; the virtual phone downloads the CA over HTTP, checks its fingerprint against the one the box printed, and then signs the admin in over HTTPS with it; OpenSSL refuses a `bank.example.com` certificate made with the CA key; a daily check with nothing due changes nothing; a reissued certificate restarts `deltd` and the device still trusts it.

## 7. Not decided here

| Open | Owner |
|---|---|
| The CA key in the box's backup set (losing it means every device enrols again; it is a secret, so the backup must protect it) | the backup work (contract §16) |
| Showing the fingerprint on the kiosk and first-boot screens, and the QR code | WebOS / F2 |
| Revoking a certificate (no CRL or OCSP is published: the zone is private and server certificates renew yearly) | Track G, if ever needed |
| Moving the box to a new CA on purpose (a stolen box, say) | F1 admin console |
