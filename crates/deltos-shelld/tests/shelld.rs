//! deltos-shelld end to end: a real `identityd` register on its own socket,
//! shelld on another, and requests made the way `deltd` will make them.
//!
//! Privilege boundary §13's tests that belong to shelld: a connection from
//! anyone but deltd is refused; a request that skipped the box API's checks
//! still cannot get past shelld; a plain or missing session, a missing
//! permission and a missing, reused or mismatched ticket are each refused;
//! every action is in the log with the right person and no secret is.

use std::io::Write;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use deltos_identity::protocol::{self, Callers};
use deltos_identity::Register;
use deltos_shelld::addons::{self, Trust};
use deltos_shelld::client::Client;
use deltos_shelld::drives::DrivesDir;
use deltos_shelld::log::ActionLog;
use deltos_shelld::{Config, NetPaths, Shell};
use serde_json::{json, Value};

fn me() -> u32 {
    unsafe { libc::getuid() }
}


struct Box_ {
    dir: tempfile::TempDir,
    /// identityd as deltd sees it (sign-in, tickets).
    id: deltos_identity::Client,
    shelld: Client,
    admin: String,
    admin_pw: String,
    key: minisign::KeyPair,
}

impl Box_ {
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn sources(&self) -> PathBuf {
        self.root().join("incoming")
    }
    fn log(&self) -> String {
        std::fs::read_to_string(self.root().join("actions.log")).unwrap_or_default()
    }
    fn layout(&self) -> addons::Layout {
        addons::Layout::under(self.root())
    }

    fn act(&self, session: &str, action: &str, args: Value) -> Result<Value, Value> {
        self.shelld.call(&json!({ "call": "act", "action": action, "args": args, "session": session, "over_https": true }))
    }

    fn act_confirmed(&self, session: &str, pw: &str, action: &str, args: Value) -> Result<Value, Value> {
        let d = self.shelld.call(&json!({ "call": "describe", "action": action, "args": args })).unwrap();
        assert_eq!(d["needs_reconfirm"], true, "{action} should ask again");
        let ticket = self.ticket(session, pw, action, d["args_digest"].as_str().unwrap());
        self.shelld.call(&json!({ "call": "act", "action": action, "args": args, "session": session, "over_https": true, "ticket": ticket }))
    }

    fn ticket(&self, session: &str, pw: &str, action: &str, digest: &str) -> String {
        let t = self
            .id
            .call(&json!({ "call": "issue_ticket", "ctx": { "https": true }, "token": session, "password": pw, "action": action, "args_digest": digest }))
            .unwrap();
        t.as_str().unwrap().to_string()
    }

    /// Wait for a job to finish; its last state.
    fn wait(&self, session: &str, job: &Value) -> Value {
        let id = job["job"].as_str().unwrap_or_else(|| panic!("not a job: {job}"));
        let start = Instant::now();
        loop {
            let j = self.shelld.call(&json!({ "call": "job", "job": id, "session": session, "over_https": true })).unwrap();
            let s = j["state"].as_str().unwrap();
            if j["finished_at"].is_u64() {
                return j;
            }
            assert!(start.elapsed() < Duration::from_secs(30), "job stuck in {s}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Write a signed bundle into the incoming folder.
    fn bundle(&self, file: &str, manifest: &str, extra: &[(&str, &[u8])]) {
        let mut b = tar::Builder::new(Vec::new());
        let mut add = |name: &str, data: &[u8]| {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, name, data).unwrap();
        };
        add("deltos-app.toml", manifest.as_bytes());
        add("compose.yaml", b"services: {}\n");
        add("LICENCE", b"MIT\n");
        for (n, d) in extra {
            add(n, d);
        }
        let bytes = b.into_inner().unwrap();
        self.write_signed(file, &bytes);
    }

    fn write_signed(&self, file: &str, bytes: &[u8]) {
        std::fs::create_dir_all(self.sources()).unwrap();
        std::fs::write(self.sources().join(file), bytes).unwrap();
        let sig = minisign::sign(Some(&self.key.pk), &self.key.sk, std::io::Cursor::new(bytes), None, None).unwrap();
        std::fs::write(self.sources().join(format!("{file}.minisig")), sig.into_string()).unwrap();
    }
}

fn manifest(id: &str, version: &str, evidence: &str) -> String {
    let res = if evidence == "open" {
        "evidence = \"open\"\narch = \"any\"".to_string()
    } else {
        format!("evidence = \"{evidence}\"\nmin_ram_bytes = 67108864\nmin_storage_bytes = 1048576\narch = \"any\"\nbasis = \"test\"")
    };
    format!(
        r#"schema = 1
id = "{id}"
name = "Test notes"
version = "{version}"
kind = "addon"
summary = "A test add-on."
licence = "MIT"
min_profile = "classroom"
depends = ["platform"]

[resources]
{res}

[run]
form = "compose"
entry = "compose.yaml"

[sign_in]
method = "none"

[health]
probe = "compose"
target = "app"

[backup]
paths = ["/var/lib/deltos/apps/{id}/"]
"#
    )
}

fn start_with(deltd_uids: Vec<u32>, trust: Option<bool>) -> Box_ {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let reg = Arc::new(Register::open(&root.join("identity"), "home.deltos.lan").unwrap());
    let (_, admin_pw) = reg.provision_first_admin("Admin", "admin").unwrap();
    // one socket each way: my uid is "deltd" on one and "shelld" on the other
    let id_deltd = root.join("id-deltd.sock");
    let id_shelld = root.join("id-shelld.sock");
    let (r1, r2) = (Arc::clone(&reg), Arc::clone(&reg));
    let l1 = UnixListener::bind(&id_deltd).unwrap();
    let l2 = UnixListener::bind(&id_shelld).unwrap();
    std::thread::spawn(move || protocol::serve(l1, r1, Callers { deltd: vec![me()], shelld: vec![] }));
    std::thread::spawn(move || protocol::serve(l2, r2, Callers { deltd: vec![], shelld: vec![me()] }));

    // the network: a hotspot box
    let net_reg = root.join("registry.sqlite");
    let mut nr = deltos_net::Registry::open(&net_reg).unwrap();
    nr.set_settings(&deltos_net::Settings {
        mode: deltos_net::Mode::Hotspot { interface: "wlan0".into(), box_address: "10.42.0.1".parse().unwrap(), prefix_len: 24 },
        uplink_interface: Some("eth0".into()),
        upstream_dns: vec![],
        uplink_sharing: false,
    })
    .unwrap();
    drop(nr);

    // the box's own measurement (contract §9)
    let layout = addons::Layout::under(root);
    std::fs::create_dir_all(layout.capability.parent().unwrap()).unwrap();
    std::fs::write(
        &layout.capability,
        json!({ "profile": "classroom", "ram_bytes": 8u64 << 30, "storage_bytes": 64u64 << 30, "arch": "x86_64", "gpu": "none", "os_ram_bytes": 300u64 << 20 }).to_string(),
    )
    .unwrap();

    let key = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let trust = match trust {
        Some(true) => Trust::from_base64(&key.pk.to_base64()).unwrap(),
        Some(false) => Trust::from_base64(&minisign::KeyPair::generate_unencrypted_keypair().unwrap().pk.to_base64()).unwrap(),
        None => Trust::none(),
    };
    let cfg = Config {
        identityd: deltos_identity::Client::with_connections(&id_shelld, 2),
        log: ActionLog::open(&root.join("actions.log")).unwrap(),
        deltd_uids,
        net: NetPaths { registry: net_reg, settings: deltos_net::Layout::new(root.join("run-network")) },
        backup: deltos_backup::Layout::at(&root.join("var/lib/deltos")),
        addons: layout,
        addon_folders: vec![("incoming".into(), root.join("incoming"))],
        drives: DrivesDir(root.join("drives")),
        trust,
    };
    let shell = Shell::new(cfg, Box::new(deltos_backup::restore::NoServices));
    let sock = root.join("shelld.sock");
    let l = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || deltos_shelld::serve(l, shell));

    let id = deltos_identity::Client::with_connections(&id_deltd, 2);
    let admin = sign_in(&id, "admin", &admin_pw, true);
    Box_ { shelld: Client::new(&sock), id, admin, admin_pw, key, dir }
}

fn start() -> Box_ {
    let b = start_with(vec![me()], Some(true));
    platform(&b);
    b
}

fn sign_in(id: &deltos_identity::Client, name: &str, pw: &str, https: bool) -> String {
    let s = id
        .call(&json!({ "call": "sign_in_password", "ctx": { "https": https, "peer": "10.42.0.9" }, "sign_in_name": name, "password": pw }))
        .unwrap();
    s["session_token"].as_str().unwrap().to_string()
}

/// Every compose add-on stands on the platform add-on (G1 + G2): install
/// the real draft manifest's bundle first.
fn platform(b: &Box_) {
    let m = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../deltos-apps/addons/platform.toml")).unwrap();
    b.bundle("platform-0.1.0.deltos-addon", &m, &[]);
    let j = b.act(&b.admin, "install_addon", json!({ "source": "incoming/platform-0.1.0.deltos-addon", "id": "platform", "version": "0.1.0" })).unwrap();
    let j = b.wait(&b.admin, &j);
    assert_eq!(j["state"], "done", "{j}");
}

fn code(r: Result<Value, Value>) -> String {
    match r {
        Ok(v) => panic!("expected a refusal, got {v}"),
        Err(e) => e["code"].as_str().unwrap().to_string(),
    }
}

#[test]
fn a_caller_that_is_not_deltd_is_disconnected() {
    let b = start_with(vec![me().wrapping_add(1)], Some(true));
    let e = b.shelld.call(&json!({ "call": "list", "listing": "actions" }));
    assert_eq!(e.unwrap_err()["code"], "unavailable");
}

#[test]
fn every_check_refuses_on_its_own() {
    let b = start();
    let args = json!({ "name": "printer.deltos.lan", "target": "10.42.0.20", "requested_by": "p1" });
    // no live session
    assert_eq!(code(b.act(&"0".repeat(64), "approve_name", args.clone())), "not_signed_in");
    // the right session over plain HTTP (a box API that forgot to check)
    let r = b.shelld.call(&json!({ "call": "act", "action": "approve_name", "args": args, "session": b.admin, "over_https": false }));
    assert_eq!(code(r), "secure_connection_required");
    // a secure session without the permission: a teacher
    b.id.call(&json!({ "call": "create_profile", "ctx": { "https": true }, "token": b.admin, "profile": {
        "display_name": "Teacher", "kind": "passworded", "sign_in_name": "teacher", "password": "teacher-password-1",
        "roles": [{ "role": "teacher" }] } }))
        .unwrap();
    let teacher = sign_in(&b.id, "teacher", "teacher-password-1", true);
    assert_eq!(code(b.act(&teacher, "approve_name", args.clone())), "permission_denied");
    assert_eq!(code(b.act(&teacher, "remove_addon", json!({ "id": "notes", "keep_data": true }))), "permission_denied");
    // not on the list; on the list but not built; an argument outside the shape
    assert_eq!(code(b.act(&b.admin, "run_command", json!({ "cmd": "id" }))), "invalid_request");
    let e = b.act(&b.admin, "install_pack", json!({})).unwrap_err();
    assert!(e["detail"].as_str().unwrap().contains("not built yet"), "{e}");
    let mut extra = args.clone();
    extra["also"] = json!(1);
    assert_eq!(code(b.act(&b.admin, "approve_name", extra)), "invalid_request");
    // and the admin, over HTTPS, may
    b.act(&b.admin, "approve_name", args).unwrap();
    let names = std::fs::read_to_string(b.root().join("run-network/names.d/names")).unwrap_or_default();
    assert!(names.contains("printer"), "{names}");
    // every one of those is in the log, with who asked
    let log = b.log();
    let results: Vec<&str> = ["not_signed_in", "secure_connection_required", "permission_denied", "invalid_request", "ok"].to_vec();
    let approve: Vec<_> = log.lines().filter(|l| l.contains("\"action\":\"approve_name\"")).collect();
    assert_eq!(approve.len(), 5, "{log}");
    for r in results {
        assert!(approve.iter().any(|l| l.contains(&format!("\"result\":\"{r}\""))), "{r} not logged:\n{log}");
    }
    assert!(approve[2].contains("\"by\":\"") && !approve[2].contains("\"by\":\"unknown\""), "the teacher is named: {}", approve[2]);
    assert!(log.contains("\"action\":\"run_command\"") && log.contains("\"action\":\"install_pack\""));
}

#[test]
fn a_ticket_works_once_for_exactly_its_action_and_arguments() {
    let b = start();
    let on = json!({ "on": true });
    // asks again: no ticket, refused
    assert_eq!(code(b.act(&b.admin, "set_uplink_sharing", on.clone())), "confirmation_required");
    // a ticket for other arguments
    let off_digest = b.shelld.call(&json!({ "call": "describe", "action": "set_uplink_sharing", "args": { "on": false } })).unwrap();
    let t = b.ticket(&b.admin, &b.admin_pw, "set_uplink_sharing", off_digest["args_digest"].as_str().unwrap());
    let r = b.shelld.call(&json!({ "call": "act", "action": "set_uplink_sharing", "args": on, "session": b.admin, "over_https": true, "ticket": t }));
    assert_eq!(code(r), "confirmation_required");
    // the right one, then the same one again
    let d = b.shelld.call(&json!({ "call": "describe", "action": "set_uplink_sharing", "args": on })).unwrap();
    let t = b.ticket(&b.admin, &b.admin_pw, "set_uplink_sharing", d["args_digest"].as_str().unwrap());
    let req = json!({ "call": "act", "action": "set_uplink_sharing", "args": on, "session": b.admin, "over_https": true, "ticket": t });
    b.shelld.call(&req).unwrap();
    assert_eq!(code(b.shelld.call(&req)), "confirmation_required");
}

#[test]
fn no_secret_reaches_the_log() {
    let b = start();
    let pass = "a backup passphrase 9";
    // no drive is mounted, so it fails, but it is logged either way
    let j = b.act(&b.admin, "start_backup", json!({ "drive": "usb1", "passphrase": pass })).unwrap();
    let j = b.wait(&b.admin, &j);
    assert_eq!(j["state"], "failed", "{j}");
    let wifi = "hotspot password 7";
    let r = b.act_confirmed(&b.admin, &b.admin_pw, "configure_ap", json!({ "ssid": "Library", "passphrase": wifi, "country": "KE", "channel": 6 }));
    r.unwrap();
    let log = b.log();
    assert!(!log.contains(pass) && !log.contains(wifi) && !log.contains(&b.admin_pw), "{log}");
    assert!(log.contains("[redacted]"));
    assert!(log.contains("configure_ap"));
}

#[test]
fn an_add_on_installs_updates_rolls_back_stops_and_is_removed() {
    let b = start();
    b.bundle("notes-0.1.0.deltos-addon", &manifest("notes", "0.1.0", "provisional"), &[("images/app.tar", b"oci")]);
    let l = b.shelld.call(&json!({ "call": "list", "listing": "addon_sources", "session": b.admin, "over_https": true })).unwrap();
    assert_eq!(l["bundles"][0]["source"], "incoming/notes-0.1.0.deltos-addon", "{l}");
    assert_eq!(l["bundles"][0]["signed"], true);

    let j = b.act(&b.admin, "install_addon", json!({ "source": "incoming/notes-0.1.0.deltos-addon", "id": "notes", "version": "0.1.0" })).unwrap();
    let j = b.wait(&b.admin, &j);
    assert_eq!(j["state"], "done", "{j}");
    assert_eq!(j["result"]["verdict"], "fits_estimated");
    let lay = b.layout();
    assert!(lay.programs.join("notes/0.1.0/images/app.tar").is_file());
    assert!(lay.data.join("notes").is_dir());
    assert!(!lay.programs.join(".staging").read_dir().unwrap().any(|_| true), "staging is left empty");
    // installing it again is refused: update it instead
    let j = b.act(&b.admin, "install_addon", json!({ "source": "incoming/notes-0.1.0.deltos-addon", "id": "notes", "version": "0.1.0" })).unwrap();
    assert_eq!(b.wait(&b.admin, &j)["error"]["code"], "invalid_request");

    // update, then go back
    b.bundle("notes-0.2.0.deltos-addon", &manifest("notes", "0.2.0", "provisional"), &[]);
    let j = b.act(&b.admin, "update_addon", json!({ "source": "incoming/notes-0.2.0.deltos-addon", "id": "notes", "version": "0.2.0" })).unwrap();
    assert_eq!(b.wait(&b.admin, &j)["state"], "done");
    let apps = b.shelld.call(&json!({ "call": "list", "listing": "addons", "session": b.admin, "over_https": true })).unwrap();
    let notes = apps["apps"].as_array().unwrap().iter().find(|a| a["id"] == "notes").unwrap().clone();
    assert_eq!((notes["version"].as_str(), notes["previous"].as_str()), (Some("0.2.0"), Some("0.1.0")));
    b.act(&b.admin, "roll_back_addon", json!({ "id": "notes", "version": "0.1.0" })).unwrap();
    assert_eq!(addons::Register::load(&lay).unwrap().apps["notes"].version, "0.1.0");

    // stop it
    b.act(&b.admin, "set_addon_running", json!({ "id": "notes", "on": false })).unwrap();
    assert!(!addons::Register::load(&lay).unwrap().apps["notes"].wanted_running);

    // the platform cannot go while notes stands on it
    let r = b.act_confirmed(&b.admin, &b.admin_pw, "remove_addon", json!({ "id": "platform", "keep_data": true })).unwrap_err();
    assert!(r["detail"].as_str().unwrap().contains("needed by notes"), "{r}");

    // removing asks again; keeping its data leaves the data folder
    std::fs::write(lay.data.join("notes/note.txt"), "kept").unwrap();
    assert_eq!(code(b.act(&b.admin, "remove_addon", json!({ "id": "notes", "keep_data": true }))), "confirmation_required");
    b.act_confirmed(&b.admin, &b.admin_pw, "remove_addon", json!({ "id": "notes", "keep_data": true })).unwrap();
    assert!(!lay.programs.join("notes").exists());
    assert!(lay.data.join("notes/note.txt").is_file());
    assert!(!addons::Register::load(&lay).unwrap().apps.contains_key("notes"));

    // put back, then removed with its data
    let j = b.act(&b.admin, "install_addon", json!({ "source": "incoming/notes-0.2.0.deltos-addon", "id": "notes", "version": "0.2.0" })).unwrap();
    assert_eq!(b.wait(&b.admin, &j)["state"], "done");
    b.act_confirmed(&b.admin, &b.admin_pw, "remove_addon", json!({ "id": "notes", "keep_data": false })).unwrap();
    assert!(!lay.data.join("notes").exists());

    let log = b.log();
    for a in ["install_addon", "update_addon", "roll_back_addon", "set_addon_running", "remove_addon"] {
        assert!(log.lines().any(|l| l.contains(a) && l.contains("\"result\":\"ok\"")), "{a} missing from the log:\n{log}");
    }
}

#[test]
fn a_bundle_that_is_not_signed_by_the_key_is_never_installed() {
    let install = |b: &Box_, file: &str| {
        let j = b.act(&b.admin, "install_addon", json!({ "source": format!("incoming/{file}"), "id": "notes", "version": "0.1.0" })).unwrap();
        b.wait(&b.admin, &j)
    };
    // signed by another key
    let b = start_with(vec![me()], Some(false));
    b.bundle("notes-0.1.0.deltos-addon", &manifest("notes", "0.1.0", "provisional"), &[]);
    assert_eq!(install(&b, "notes-0.1.0.deltos-addon")["state"], "failed_signature");
    // no key on the box at all
    let b = start_with(vec![me()], None);
    b.bundle("notes-0.1.0.deltos-addon", &manifest("notes", "0.1.0", "provisional"), &[]);
    assert_eq!(install(&b, "notes-0.1.0.deltos-addon")["state"], "failed_signature");
    // the right key, but changed after signing
    let b = start();
    b.bundle("notes-0.1.0.deltos-addon", &manifest("notes", "0.1.0", "provisional"), &[]);
    let p = b.sources().join("notes-0.1.0.deltos-addon");
    std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(b"x").unwrap();
    assert_eq!(install(&b, "notes-0.1.0.deltos-addon")["state"], "failed_signature");
    // no signature beside it
    b.bundle("notes-0.1.0.deltos-addon", &manifest("notes", "0.1.0", "provisional"), &[]);
    std::fs::remove_file(b.sources().join("notes-0.1.0.deltos-addon.minisig")).unwrap();
    assert_eq!(install(&b, "notes-0.1.0.deltos-addon")["state"], "failed_signature");
    assert!(!addons::Register::load(&b.layout()).unwrap().apps.contains_key("notes"));
}

#[test]
fn a_signed_bundle_still_cannot_write_outside_itself() {
    let b = start();
    let raw = |name: &str, kind: tar::EntryType, data: &[u8], link: Option<&str>| {
        let mut out = tar::Builder::new(Vec::new());
        let m = manifest("notes", "0.1.0", "provisional");
        for (n, d) in [("deltos-app.toml", m.as_bytes()), ("compose.yaml", b"x".as_slice()), ("LICENCE", b"MIT".as_slice())] {
            let mut h = tar::Header::new_gnu();
            h.set_size(d.len() as u64);
            h.set_cksum();
            out.append_data(&mut h, n, d).unwrap();
        }
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(kind);
        h.set_size(data.len() as u64);
        // write the name raw, so the builder cannot refuse it for us
        let bytes = name.as_bytes();
        h.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
        if let Some(l) = link {
            h.set_link_name(l).unwrap();
        }
        h.set_cksum();
        out.append(&h, data).unwrap();
        out.into_inner().unwrap()
    };
    let cases = [
        ("../escape", tar::EntryType::Regular, None),
        ("link", tar::EntryType::Symlink, Some("/etc/passwd")),
        ("dev", tar::EntryType::Char, None),
    ];
    for (name, kind, link) in cases {
        b.write_signed("notes-0.1.0.deltos-addon", &raw(name, kind, b"", link));
        let j = b.act(&b.admin, "install_addon", json!({ "source": "incoming/notes-0.1.0.deltos-addon", "id": "notes", "version": "0.1.0" })).unwrap();
        let j = b.wait(&b.admin, &j);
        assert_eq!(j["error"]["code"], "invalid_request", "{name}: {j}");
    }
    assert!(!b.layout().programs.join("escape").exists() && !b.root().join("usr/lib/deltos/escape").exists());
    assert!(!addons::Register::load(&b.layout()).unwrap().apps.contains_key("notes"));
}

#[test]
fn an_unmeasured_add_on_installs_only_when_the_admin_accepts_it_with_their_password() {
    let b = start();
    b.bundle("notes-0.1.0.deltos-addon", &manifest("notes", "0.1.0", "open"), &[]);
    let args = json!({ "source": "incoming/notes-0.1.0.deltos-addon", "id": "notes", "version": "0.1.0" });
    let j = b.wait(&b.admin, &b.act(&b.admin, "install_addon", args.clone()).unwrap());
    assert_eq!(j["error"]["code"], "confirmation_required", "{j}");
    // accepting it without the password is refused before any job starts
    let mut yes = args.clone();
    yes["accept_unmeasured"] = json!(true);
    assert_eq!(code(b.act(&b.admin, "install_addon", yes.clone())), "confirmation_required");
    let j = b.act_confirmed(&b.admin, &b.admin_pw, "install_addon", yes).unwrap();
    let j = b.wait(&b.admin, &j);
    assert_eq!((j["state"].as_str(), j["result"]["verdict"].as_str()), (Some("done"), Some("cannot_tell")), "{j}");
}

#[test]
fn an_add_on_that_does_not_fit_is_refused_with_its_reasons() {
    let b = start();
    let big = manifest("notes", "0.1.0", "measured").replace("min_ram_bytes = 67108864", "min_ram_bytes = 68719476736");
    b.bundle("notes-0.1.0.deltos-addon", &big, &[]);
    let j = b.act(&b.admin, "install_addon", json!({ "source": "incoming/notes-0.1.0.deltos-addon", "id": "notes", "version": "0.1.0" })).unwrap();
    let j = b.wait(&b.admin, &j);
    assert_eq!(j["error"]["code"], "does_not_fit", "{j}");
    // and the bundle must be the one chosen
    b.bundle("other-0.1.0.deltos-addon", &manifest("notes", "0.1.0", "provisional"), &[]);
    let j = b.act(&b.admin, "install_addon", json!({ "source": "incoming/other-0.1.0.deltos-addon", "id": "other", "version": "0.1.0" })).unwrap();
    assert_eq!(b.wait(&b.admin, &j)["error"]["code"], "invalid_request");
    // a path for a source is refused before anything is read
    let r = b.act(&b.admin, "install_addon", json!({ "source": "../etc/x.deltos-addon", "id": "notes", "version": "0.1.0" }));
    assert_eq!(code(r), "invalid_request");
}

#[test]
fn another_person_cannot_read_someone_s_job() {
    let b = start();
    b.bundle("notes-0.1.0.deltos-addon", &manifest("notes", "0.1.0", "provisional"), &[]);
    let j = b.act(&b.admin, "install_addon", json!({ "source": "incoming/notes-0.1.0.deltos-addon", "id": "notes", "version": "0.1.0" })).unwrap();
    b.wait(&b.admin, &j);
    let other = sign_in(&b.id, "admin", &b.admin_pw, true);
    // same person, new session: fine
    b.shelld.call(&json!({ "call": "job", "job": j["job"], "session": other, "over_https": true })).unwrap();
    b.id.call(&json!({ "call": "create_profile", "ctx": { "https": true }, "token": b.admin, "profile": {
        "display_name": "Second admin", "kind": "passworded", "sign_in_name": "admin2", "password": "second-admin-pass",
        "roles": [{ "role": "admin" }] } }))
        .unwrap();
    let a2 = sign_in(&b.id, "admin2", "second-admin-pass", true);
    let r = b.shelld.call(&json!({ "call": "job", "job": j["job"], "session": a2, "over_https": true }));
    assert_eq!(code(r), "invalid_request");
}
