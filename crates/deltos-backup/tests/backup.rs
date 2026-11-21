//! Backup and restore against a real state tree: the register, the CA, the
//! network registry and the pack catalog made by their own crates.

use std::fs;
use std::path::{Path, PathBuf};

use deltos_backup::actions::{self, Action, Caller, IdentityCheck, Outcome, Refusal};
use deltos_backup::restore::{self, NoServices, Services};
use deltos_backup::{archive, Error, Layout, Passphrase};
use deltos_identity::{Ctx, Register};

const PASS: &str = "correct horse battery";
const HOST: &str = "home.deltos.lan";
const SSID: &str = "Backup Test Wi-Fi";
const WIFI_PASS: &str = "radio secret 12345";

fn fixture_pack() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../wax-builder/tests/fixtures/legacy-tier-pi_5.wax")
}

fn https() -> Ctx {
    Ctx { https: true, kiosk: false, peer: "10.42.0.9".into(), browser: None, session: None }
}

struct Box_ {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    drive: PathBuf,
    admin_password: String,
}

fn ca_paths(root: &Path) -> deltos_tls::Paths {
    deltos_tls::Paths { ca_dir: root.join("ca"), tls_dir: root.join("tls"), key_group: None }
}

/// A provisioned box: one admin (signed in), a remembered device, the CA,
/// the hotspot, one pack and its catalog slot, plus things never backed up.
fn make_box() -> Box_ {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("state");
    let drive = tmp.path().join("drive");
    fs::create_dir_all(root.join("identity")).unwrap();
    fs::create_dir_all(root.join("network")).unwrap();
    fs::create_dir_all(root.join("b4")).unwrap();
    fs::create_dir_all(root.join("packs")).unwrap();
    fs::create_dir_all(root.join("health")).unwrap();
    fs::create_dir_all(&drive).unwrap();
    let reg = Register::open(&root.join("identity"), HOST).unwrap();
    let (_, admin_password) = reg.provision_first_admin("Ada Admin", "ada").unwrap();
    reg.sign_in_password(&https(), "ada", &admin_password).unwrap();
    drop(reg);
    deltos_tls::ensure(&ca_paths(&root), 1_800_000_000).unwrap();
    let mut net = deltos_net::Registry::open(&root.join("network/registry.sqlite")).unwrap();
    net.set_settings(&deltos_net::Settings {
        mode: deltos_net::Mode::Hotspot { interface: "wlan0".into(), box_address: "10.42.0.1".parse().unwrap(), prefix_len: 24 },
        uplink_interface: None,
        upstream_dns: vec![],
        uplink_sharing: false,
    })
    .unwrap();
    net.set_radio(&deltos_net::Radio { ssid: SSID.into(), passphrase: deltos_net::Secret::new(WIFI_PASS), country: "GB".into(), channel: 6 }).unwrap();
    drop(net);
    fs::copy(fixture_pack(), root.join("packs/science.wax")).unwrap();
    let uuid = deltlib::library::uuid_text(&wax_core::WaxReader::open(root.join("packs/science.wax")).unwrap().header().archive_uuid);
    let mut cat = deltlib::Catalog::open(&root.join("b4/deltd-pack-hostnames.sqlite")).unwrap();
    cat.assign(&uuid).unwrap();
    drop(cat);
    fs::write(root.join("capability.json"), br#"{"ram_bytes":2147483648}"#).unwrap();
    fs::write(root.join("health/snap.json"), b"{}").unwrap();
    // a future service's data, under a path no rule names: kept by default
    fs::create_dir_all(root.join("notes-to-be")).unwrap();
    fs::write(root.join("notes-to-be/n1.txt"), b"my private note").unwrap();
    Box_ { _tmp: tmp, root, drive, admin_password }
}

fn pass() -> Passphrase {
    Passphrase::new(PASS).unwrap()
}

fn backup(b: &Box_) -> archive::Created {
    archive::create(&Layout::at(&b.root), &b.drive, &pass(), "test").unwrap()
}

fn ca_fp(root: &Path) -> String {
    deltos_tls::fingerprint(&deltos_tls::pem_to_der(&fs::read_to_string(root.join("ca/ca.crt")).unwrap()).unwrap())
}

/// Throw the box's state away and provision it again: a fresh box.
fn wipe_and_reprovision(b: &Box_) {
    for d in ["identity", "ca", "tls", "network", "b4", "notes-to-be"] {
        let _ = fs::remove_dir_all(b.root.join(d));
    }
    fs::create_dir_all(b.root.join("identity")).unwrap();
    let reg = Register::open(&b.root.join("identity"), HOST).unwrap();
    reg.provision_first_admin("Fresh Admin", "fresh").unwrap();
    drop(reg);
    deltos_tls::ensure(&ca_paths(&b.root), 1_800_000_000).unwrap();
}

#[test]
fn round_trip_brings_back_people_stamp_wifi_and_notes() {
    let b = make_box();
    let original_fp = ca_fp(&b.root);
    let original_key = fs::read(b.root.join("ca/ca.key")).unwrap();
    let c = backup(&b);
    assert_eq!(c.packs_listed, 1);
    let excluded: Vec<_> = c.excluded.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(excluded, ["capability.json", "health", "packs"]);
    let listed = archive::list(&b.drive).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].backup_id, c.backup_id);

    wipe_and_reprovision(&b);
    assert_ne!(ca_fp(&b.root), original_fp, "a fresh box makes its own CA");
    let layout = Layout::at(&b.root);
    let r = restore::review(&layout, &b.drive.join(&c.backup_id), &c.backup_id, &pass()).unwrap();
    assert_eq!(r.ca_fingerprint.as_deref(), Some(original_fp.as_str()));
    assert_ne!(r.current_ca_fingerprint, r.ca_fingerprint);
    let p = r.people.as_ref().unwrap();
    assert_eq!(p.profiles, 1);
    assert_eq!(p.admins, ["ada"]);
    assert_eq!(p.roles.get("admin"), Some(&1));
    assert!(p.last_activity.is_some(), "the sign-in counts as activity");
    let n = r.network.as_ref().unwrap();
    assert_eq!((n.mode.as_str(), n.interface.as_str(), n.ssid.as_deref()), ("hotspot", "wlan0", Some(SSID)));
    assert_eq!(r.packs.len(), 1);
    assert!(r.packs[0].present, "the pack file is still on this box");
    assert_eq!(r.replaces, ["b4", "ca", "identity", "network", "notes-to-be", "tls"]);
    // nothing live changed by the review
    assert_ne!(ca_fp(&b.root), original_fp);

    let a = restore::apply(&layout, &r.review_id, &r.review_digest, &mut NoServices).unwrap();
    assert!(a.packs_to_reinstall.is_empty());
    assert_eq!(ca_fp(&b.root), original_fp, "the same stamp, so devices keep trusting the box");
    assert_eq!(fs::read(b.root.join("ca/ca.key")).unwrap(), original_key);
    assert_eq!(fs::read(b.root.join("notes-to-be/n1.txt")).unwrap(), b"my private note");
    // the CA service accepts what was restored and issues nothing new
    let o = deltos_tls::ensure(&ca_paths(&b.root), 1_800_000_000).unwrap();
    assert!(!o.created_ca);
    assert!(o.issued.is_empty(), "{o:?}");
    // the backup's admin signs in with their old password; the fresh one is gone
    let reg = Register::open(&b.root.join("identity"), HOST).unwrap();
    reg.sign_in_password(&https(), "ada", &b.admin_password).unwrap();
    assert!(reg.sign_in_password(&https(), "fresh", "whatever-it-was").is_err());
    let net = deltos_net::Registry::open(&b.root.join("network/registry.sqlite")).unwrap();
    assert_eq!(net.radio().unwrap().unwrap().passphrase.expose(), WIFI_PASS);
    // what was replaced is kept once; the staging folder is gone
    assert!(a.previous.join("identity/identity.sqlite").is_file());
    assert!(!layout.staging().join(format!("review-{}", r.review_id)).exists());
}

#[test]
fn sessions_and_tickets_are_not_kept() {
    let b = make_box();
    let c = backup(&b);
    let layout = Layout::at(&b.root);
    let r = restore::review(&layout, &b.drive.join(&c.backup_id), &c.backup_id, &pass()).unwrap();
    let db = layout.staging().join(format!("review-{}/state/identity/identity.sqlite", r.review_id));
    let conn = rusqlite::Connection::open(db).unwrap();
    let n: i64 = conn.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    let p: i64 = conn.query_row("SELECT count(*) FROM profiles", [], |r| r.get(0)).unwrap();
    assert_eq!(p, 1);
}

#[test]
fn nothing_readable_without_the_passphrase() {
    let b = make_box();
    let c = backup(&b);
    let bytes = fs::read(b.drive.join(&c.backup_id)).unwrap();
    assert!(bytes.starts_with(b"age-encryption.org/v1\n-> scrypt "));
    let hay = String::from_utf8_lossy(&bytes);
    for needle in ["SQLite format", SSID, WIFI_PASS, "BEGIN", "ada", "manifest"] {
        assert!(!hay.contains(needle), "{needle} is readable in the backup file");
    }
    // the only file on the drive is the backup
    assert_eq!(fs::read_dir(&b.drive).unwrap().count(), 1);
    let layout = Layout::at(&b.root);
    let wrong = Passphrase::new("not the passphrase at all").unwrap();
    let e = restore::review(&layout, &b.drive.join(&c.backup_id), &c.backup_id, &wrong).unwrap_err();
    assert!(matches!(e, Error::Unreadable), "{e}");
    assert_eq!(fs::read_dir(layout.staging()).unwrap().count(), 0, "a failed review leaves nothing staged");
}

#[test]
fn a_changed_byte_is_refused() {
    let b = make_box();
    let c = backup(&b);
    let path = b.drive.join(&c.backup_id);
    let mut bytes = fs::read(&path).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 0x01;
    fs::write(&path, &bytes).unwrap();
    let layout = Layout::at(&b.root);
    let e = restore::review(&layout, &path, &c.backup_id, &pass()).unwrap_err();
    assert!(matches!(e, Error::Damaged(_) | Error::Unreadable), "{e}");
    // and a cut-off file
    let short = &bytes[..bytes.len() - 100];
    fs::write(&path, short).unwrap();
    assert!(restore::review(&layout, &path, &c.backup_id, &pass()).is_err());
}

#[test]
fn newer_state_than_the_box_understands_is_refused() {
    let b = make_box();
    let conn = rusqlite::Connection::open(b.root.join("identity/identity.sqlite")).unwrap();
    conn.execute("UPDATE meta SET value = '99' WHERE key = 'schema_version'", []).unwrap();
    drop(conn);
    let c = backup(&b);
    let e = restore::review(&Layout::at(&b.root), &b.drive.join(&c.backup_id), &c.backup_id, &pass()).unwrap_err();
    assert!(matches!(e, Error::TooNew(_)), "{e}");
}

#[test]
fn half_a_ca_is_refused() {
    let b = make_box();
    fs::remove_file(b.root.join("ca/ca.crt")).unwrap();
    let c = backup(&b);
    let e = restore::review(&Layout::at(&b.root), &b.drive.join(&c.backup_id), &c.backup_id, &pass()).unwrap_err();
    assert!(matches!(e, Error::Damaged(_)), "{e}");
}

#[test]
fn apply_needs_the_reviewed_digest() {
    let b = make_box();
    let c = backup(&b);
    let layout = Layout::at(&b.root);
    let r = restore::review(&layout, &b.drive.join(&c.backup_id), &c.backup_id, &pass()).unwrap();
    let e = restore::apply(&layout, &r.review_id, &"0".repeat(64), &mut NoServices).unwrap_err();
    assert!(matches!(e, Error::Invalid(_)), "{e}");
    assert!(matches!(restore::apply(&layout, "../../etc", &r.review_digest, &mut NoServices), Err(Error::Invalid(_))));
    // a second review replaces the first
    let r2 = restore::review(&layout, &b.drive.join(&c.backup_id), &c.backup_id, &pass()).unwrap();
    assert!(matches!(restore::apply(&layout, &r.review_id, &r.review_digest, &mut NoServices), Err(Error::NotFound(_))));
    restore::apply(&layout, &r2.review_id, &r2.review_digest, &mut NoServices).unwrap();
}

struct FailStop;
impl Services for FailStop {
    fn stop(&mut self) -> Result<(), String> {
        Err("stop refused".into())
    }
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn a_failed_restore_changes_nothing() {
    let b = make_box();
    let c = backup(&b);
    wipe_and_reprovision(&b);
    let fresh_fp = ca_fp(&b.root);
    let layout = Layout::at(&b.root);
    let r = restore::review(&layout, &b.drive.join(&c.backup_id), &c.backup_id, &pass()).unwrap();
    assert!(matches!(restore::apply(&layout, &r.review_id, &r.review_digest, &mut FailStop), Err(Error::RestoreFailed(_))));
    assert_eq!(ca_fp(&b.root), fresh_fp);
    // a swap that fails half way is put back: "network" fails after b4,
    // ca and identity were already swapped in
    let r = restore::review(&layout, &b.drive.join(&c.backup_id), &c.backup_id, &pass()).unwrap();
    let mut faulty = layout.clone();
    faulty.fault_swap = Some("network".into());
    let e = restore::apply(&faulty, &r.review_id, &r.review_digest, &mut NoServices).unwrap_err();
    assert!(matches!(e, Error::RestoreFailed(_)), "{e}");
    assert_eq!(ca_fp(&b.root), fresh_fp, "the CA swap was undone");
    let reg = Register::open(&b.root.join("identity"), HOST).unwrap();
    assert!(reg.has_admin().unwrap());
    assert!(!b.root.join("network").exists(), "the fresh box had no network settings, and still has none");
    assert!(!b.root.join("notes-to-be").exists());
    // and the same review can still be applied afterwards
    restore::apply(&layout, &r.review_id, &r.review_digest, &mut NoServices).unwrap();
    assert_ne!(ca_fp(&b.root), fresh_fp);
}

#[test]
fn missing_packs_are_named_for_reinstall() {
    let b = make_box();
    let c = backup(&b);
    fs::remove_file(b.root.join("packs/science.wax")).unwrap();
    let layout = Layout::at(&b.root);
    let r = restore::review(&layout, &b.drive.join(&c.backup_id), &c.backup_id, &pass()).unwrap();
    assert!(!r.packs[0].present);
    assert!(r.warnings.iter().any(|w| w.contains("installed again")));
    let a = restore::apply(&layout, &r.review_id, &r.review_digest, &mut NoServices).unwrap();
    assert_eq!(a.packs_to_reinstall.len(), 1);
    assert!(!a.packs_to_reinstall[0].archive_uuid.is_empty());
}

#[test]
fn passphrase_rules() {
    assert!(Passphrase::new("short").is_err());
    assert!(Passphrase::new("x".repeat(257)).is_err());
    assert_eq!(format!("{:?}", pass()), "[redacted]");
}

#[test]
fn ids_are_names_not_paths() {
    assert!(archive::check_backup_id("deltos-backup-20261121T081500Z-0a1b2c3d.age").is_ok());
    for bad in ["../x.age", "deltos-backup-20261121T081500Z-0a1b2c3d.age/..", "deltos-backup-2026112XT081500Z-0a1b2c3d.age", "/etc/passwd"] {
        assert!(archive::check_backup_id(bad).is_err(), "{bad}");
    }
}

// ---- the privilege boundary ----

struct Id {
    secure: bool,
    has: bool,
    ticket_ok: std::cell::Cell<Option<String>>,
}
impl IdentityCheck for Id {
    fn session(&self, s: &str) -> Option<(String, bool)> {
        (s == "sess").then(|| ("p1".to_string(), self.secure))
    }
    fn has_permission(&self, _: &str, perm: &str) -> bool {
        self.has && perm == actions::PERMISSION
    }
    fn redeem_ticket(&self, t: &str, s: &str, action: &str, digest: &str) -> bool {
        // single use, bound to the exact digest
        t == "tk" && s == "sess" && action == "restore_backup" && self.ticket_ok.take().as_deref() == Some(digest)
    }
}

fn caller(https: bool, ticket: Option<&'static str>) -> Caller<'static> {
    Caller { session_ref: "sess", over_https: https, confirmation_ticket: ticket }
}

#[test]
fn backup_actions_need_a_secure_admin_and_restore_asks_again() {
    let id = Id { secure: true, has: true, ticket_ok: std::cell::Cell::new(None) };
    let start = || Action::StartBackup { drive: "usb-1".into(), passphrase: pass() };
    assert!(matches!(actions::authorize(start(), &caller(false, None), &id), Err(Refusal::SecureConnectionRequired)));
    let plain = Id { secure: false, ..Id { secure: true, has: true, ticket_ok: Default::default() } };
    assert!(matches!(actions::authorize(start(), &caller(true, None), &plain), Err(Refusal::SecureConnectionRequired)));
    let nope = Id { has: false, secure: true, ticket_ok: Default::default() };
    assert!(matches!(actions::authorize(start(), &caller(true, None), &nope), Err(Refusal::PermissionDenied)));
    assert!(actions::authorize(start(), &caller(true, None), &id).is_ok());
    let bad_drive = Action::StartBackup { drive: "../mnt".into(), passphrase: pass() };
    assert!(matches!(actions::authorize(bad_drive, &caller(true, None), &id), Err(Refusal::Invalid(_))));

    let restore = Action::RestoreBackup { review_id: "0123456789abcdef".into(), review_digest: "a".repeat(64) };
    assert!(matches!(actions::authorize(restore.clone(), &caller(true, None), &id), Err(Refusal::ConfirmationRequired)));
    id.ticket_ok.set(Some("wrong digest".into()));
    assert!(matches!(actions::authorize(restore.clone(), &caller(true, Some("tk")), &id), Err(Refusal::ConfirmationRequired)));
    id.ticket_ok.set(Some(restore.args_digest()));
    let a = actions::authorize(restore.clone(), &caller(true, Some("tk")), &id).unwrap();
    assert_eq!(a.by(), "p1");
    assert!(matches!(actions::authorize(restore, &caller(true, Some("tk")), &id), Err(Refusal::ConfirmationRequired)), "a ticket works once");
    assert!(!start().summary().contains(PASS));
    assert!(!start().canonical_args().contains(PASS));
}

struct OneDrive(PathBuf);
impl actions::Drives for OneDrive {
    fn mount_point(&self, id: &str, _: bool) -> Option<PathBuf> {
        (id == "usb-1").then(|| self.0.clone())
    }
}

#[test]
fn perform_runs_the_three_actions() {
    let b = make_box();
    let layout = Layout::at(&b.root);
    let drives = OneDrive(b.drive.clone());
    let a = actions::authorize_unchecked_for_tests(Action::StartBackup { drive: "usb-1".into(), passphrase: pass() });
    let Outcome::Created(c) = actions::perform(&layout, a, &drives, &mut NoServices).unwrap() else { panic!() };
    let a = actions::authorize_unchecked_for_tests(Action::StartBackup { drive: "usb-2".into(), passphrase: pass() });
    assert!(matches!(actions::perform(&layout, a, &drives, &mut NoServices), Err(Error::NotFound(_))));
    let a = actions::authorize_unchecked_for_tests(Action::ReviewBackup { drive: "usb-1".into(), backup_id: c.backup_id.clone(), passphrase: pass() });
    let Outcome::Reviewed(r) = actions::perform(&layout, a, &drives, &mut NoServices).unwrap() else { panic!() };
    let a = actions::authorize_unchecked_for_tests(Action::RestoreBackup { review_id: r.review_id.clone(), review_digest: r.review_digest.clone() });
    let Outcome::Restored(_) = actions::perform(&layout, a, &drives, &mut NoServices).unwrap() else { panic!() };
}
