//! Pack installs through deltos-shelld, end to end: a real `identityd`
//! register on its own socket, shelld on another, the real
//! `deltos-pack-check` helper, and real packs signed the way SPEC §8 says.
//!
//! `docs/shelld.md` §7.6 lists what each test shows.

use std::collections::BTreeMap;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use deltos_identity::protocol::{self, Callers};
use deltos_identity::Register;
use deltos_shelld::client::Client;
use deltos_shelld::drives::DrivesDir;
use deltos_shelld::log::ActionLog;
use deltos_shelld::packs::{self, Checker, Env, Helper, Measured};
use deltos_shelld::{addons, Config, NetPaths, Shell};
use serde_json::{json, Value};
use wax_core::writer::{EntryMeta, WaxWriter};

const SCIENCE: [u8; 16] = [0x10, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1];
const SCIENCE_ID: &str = "10000000-0000-4000-8000-000000000001";
const ATLAS: [u8; 16] = [0x20, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 2];
const ATLAS_ID: &str = "20000000-0000-4000-8000-000000000002";

fn me() -> u32 {
    unsafe { libc::getuid() }
}

fn manifest(name: &str, version: &str, arch: &str, ram: u64) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("name".to_string(), name.to_string()),
        ("icon".into(), "icon.svg".into()),
        ("category".into(), "reference".into()),
        ("license".into(), "CC-BY-SA-4.0".into()),
        ("attribution".into(), "Test authors".into()),
        ("version".into(), version.to_string()),
        ("min_ram_bytes".into(), ram.to_string()),
        ("min_storage_bytes".into(), "1048576".into()),
        ("arch".into(), arch.to_string()),
        ("entry_point".into(), "index.html".into()),
    ])
}

/// Edition one of a pack: a few pages.
fn build(path: &Path, uuid: [u8; 16], m: &BTreeMap<String, String>, created_at: u64) {
    let mut sw = WaxWriter::new(uuid).created_at(created_at).create(path, m).unwrap();
    for (p, body) in [("index.html", "Welcome."), ("Water.html", "Evaporation and rain."), ("icon.svg", "<svg/>")] {
        sw.add_entry(EntryMeta::new(p).mime("text/html"), &mut body.as_bytes()).unwrap();
    }
    sw.finish().unwrap();
}

/// A newer edition, appended (SPEC §7.4), replacing the manifest.
fn append(path: &Path, uuid: [u8; 16], m: &BTreeMap<String, String>, created_at: u64) {
    let mut sw = WaxWriter::new(uuid).created_at(created_at).open_append(path).unwrap();
    sw.set_manifest(m);
    sw.add_entry(EntryMeta::new("Ice.html").mime("text/html"), &mut &b"Ice is frozen water."[..]).unwrap();
    sw.finish().unwrap();
}

/// SPEC §8.2: minisign over the 32-byte digest, prehashed, with the
/// trusted comment `wax-builder` writes.
fn sign(path: &Path, key: &minisign::KeyPair) {
    let (digest, uuid, created_at) = wax_core::reader::signable_digest_of(path).unwrap();
    let comment = format!("wax archive_uuid={} created_at={created_at}", packs::uuid_text(&uuid));
    let sig = minisign::sign(Some(&key.pk), &key.sk, std::io::Cursor::new(digest.to_vec()), Some(&comment), None).unwrap();
    std::fs::write(sidecar(path), sig.into_string()).unwrap();
}

fn sidecar(p: &Path) -> PathBuf {
    PathBuf::from(format!("{}.minisig", p.display()))
}

struct Box_ {
    dir: tempfile::TempDir,
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
    fn incoming(&self) -> PathBuf {
        let p = self.root().join("incoming");
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn layout(&self) -> packs::Layout {
        packs::Layout::under(self.root())
    }
    fn log(&self) -> String {
        std::fs::read_to_string(self.root().join("actions.log")).unwrap_or_default()
    }
    fn act(&self, action: &str, args: Value) -> Result<Value, Value> {
        self.shelld.call(&json!({ "call": "act", "action": action, "args": args, "session": self.admin, "over_https": true }))
    }
    fn act_confirmed(&self, action: &str, args: Value) -> Result<Value, Value> {
        let d = self.shelld.call(&json!({ "call": "describe", "action": action, "args": args })).unwrap();
        assert_eq!(d["needs_reconfirm"], true, "{action} should ask again");
        let t = self
            .id
            .call(&json!({ "call": "issue_ticket", "ctx": { "https": true }, "token": self.admin, "password": self.admin_pw, "action": action, "args_digest": d["args_digest"] }))
            .unwrap();
        self.shelld.call(&json!({ "call": "act", "action": action, "args": args, "session": self.admin, "over_https": true, "ticket": t }))
    }
    fn list(&self, listing: &str) -> Value {
        self.shelld.call(&json!({ "call": "list", "listing": listing, "session": self.admin, "over_https": true })).unwrap()
    }
    /// Install from the incoming folder and wait for the job to end.
    fn install(&self, file: &str, uuid: &str) -> Value {
        let j = self.act("install_pack", json!({ "source": format!("incoming/{file}"), "archive_uuid": uuid })).unwrap();
        let id = j["job"].as_str().unwrap_or_else(|| panic!("not a job: {j}")).to_string();
        let start = Instant::now();
        loop {
            let s = self.shelld.call(&json!({ "call": "job", "job": id, "session": self.admin, "over_https": true })).unwrap();
            if s["finished_at"].is_u64() {
                return s;
            }
            assert!(start.elapsed() < Duration::from_secs(30), "job stuck: {s}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// A signed pack in the incoming folder.
    fn offer(&self, file: &str, uuid: [u8; 16], m: &BTreeMap<String, String>, created_at: u64) -> PathBuf {
        let p = self.incoming().join(file);
        build(&p, uuid, m, created_at);
        sign(&p, &self.key);
        p
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Key {
    Right,
    Other,
    None,
}

fn start_with(key: Key, arch: &str) -> Box_ {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let reg = Arc::new(Register::open(&root.join("identity"), "home.deltos.lan").unwrap());
    let (_, admin_pw) = reg.provision_first_admin("Admin", "admin").unwrap();
    let id_deltd = root.join("id-deltd.sock");
    let id_shelld = root.join("id-shelld.sock");
    let (r1, r2) = (Arc::clone(&reg), Arc::clone(&reg));
    let l1 = UnixListener::bind(&id_deltd).unwrap();
    let l2 = UnixListener::bind(&id_shelld).unwrap();
    std::thread::spawn(move || protocol::serve(l1, r1, Callers { deltd: vec![me()], shelld: vec![] }));
    std::thread::spawn(move || protocol::serve(l2, r2, Callers { deltd: vec![], shelld: vec![me()] }));

    let lay = packs::Layout::under(root);
    std::fs::create_dir_all(&lay.packs).unwrap();
    std::fs::write(
        &lay.capability,
        json!({ "profile": "classroom", "ram_bytes": 2u64 << 30, "storage_bytes": 32u64 << 30, "arch": arch, "gpu": "none" }).to_string(),
    )
    .unwrap();

    let k = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let pub_file = root.join("pack-key.pub");
    let trust = match key {
        Key::Right => {
            std::fs::write(&pub_file, k.pk.to_box().unwrap().into_string()).unwrap();
            packs::Trust::from_file(&pub_file).unwrap()
        }
        Key::Other => {
            let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
            std::fs::write(&pub_file, other.pk.to_box().unwrap().into_string()).unwrap();
            packs::Trust::from_file(&pub_file).unwrap()
        }
        Key::None => packs::Trust::none(),
    };
    let cfg = Config {
        identityd: deltos_identity::Client::with_connections(&id_shelld, 2),
        log: ActionLog::open(&root.join("actions.log")).unwrap(),
        deltd_uids: vec![me()],
        net: NetPaths { registry: root.join("registry.sqlite"), settings: deltos_net::Layout::new(root.join("run-network")) },
        backup: deltos_backup::Layout::at(&root.join("var/lib/deltos")),
        addons: addons::Layout::under(root),
        addon_folders: vec![],
        drives: DrivesDir(root.join("drives")),
        trust: addons::Trust::none(),
        packs: lay,
        pack_trust: trust,
        pack_check: Helper { program: env!("CARGO_BIN_EXE_deltos-pack-check").into(), user: None },
        pack_data_owner: None,
        pack_folders: vec![("incoming".into(), root.join("incoming"))],
    };
    let shell = Shell::new(cfg, Box::new(deltos_backup::restore::NoServices));
    let sock = root.join("shelld.sock");
    let l = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || deltos_shelld::serve(l, shell));

    let id = deltos_identity::Client::with_connections(&id_deltd, 2);
    let s = id
        .call(&json!({ "call": "sign_in_password", "ctx": { "https": true, "peer": "10.42.0.9" }, "sign_in_name": "admin", "password": admin_pw }))
        .unwrap();
    let admin = s["session_token"].as_str().unwrap().to_string();
    Box_ { shelld: Client::new(&sock), id, admin, admin_pw, key: k, dir }
}

fn start() -> Box_ {
    start_with(Key::Right, "aarch64")
}

const MIB: u64 = 1 << 20;

#[test]
fn a_pack_installs_updates_rolls_back_and_is_removed() {
    let b = start();
    let lay = b.layout();
    let src = b.offer("science.wax", SCIENCE, &manifest("Science", "2026.09.1", "any", 64 * MIB), 1_700_000_000);
    let ed1 = std::fs::read(&src).unwrap();
    let ed1_sig = std::fs::read(sidecar(&src)).unwrap();

    let l = b.list("pack_sources");
    assert_eq!(l["packs"][0]["source"], "incoming/science.wax", "{l}");
    assert_eq!(l["packs"][0]["signed"], true);

    let j = b.install("science.wax", SCIENCE_ID);
    assert_eq!(j["state"], "done", "{j}");
    assert_eq!(j["result"]["done"], "installed");
    let placed = lay.packs.join(format!("{SCIENCE_ID}.wax"));
    assert_eq!(std::fs::read(&placed).unwrap(), ed1, "the file deltd will serve is the one checked");
    assert_eq!(std::fs::read(sidecar(&placed)).unwrap(), ed1_sig);
    assert!(lay.packs_data.join(SCIENCE_ID).is_dir(), "its writable storage is made at install (contract §10)");
    assert!(!lay.packs.join(".staging").read_dir().unwrap().any(|_| true), "staging is left empty");
    let ls = b.list("packs");
    assert_eq!((ls["packs"][0]["archive_uuid"].as_str(), ls["packs"][0]["version"].as_str()), (Some(SCIENCE_ID), Some("2026.09.1")), "{ls}");
    assert_eq!(ls["packs"][0]["can_roll_back"], false);

    // the same edition again is refused
    assert_eq!(b.install("science.wax", SCIENCE_ID)["error"]["code"], "invalid_request");

    // a newer edition, appended, then gone back from
    append(&src, SCIENCE, &manifest("Science", "2026.10.1", "any", 64 * MIB), 1_700_100_000);
    sign(&src, &b.key);
    let ed2 = std::fs::read(&src).unwrap();
    let j = b.install("science.wax", SCIENCE_ID);
    assert_eq!(j["state"], "done", "{j}");
    assert_eq!((j["result"]["done"].as_str(), j["result"]["previous"].as_str()), (Some("updated"), Some("2026.09.1")));
    assert_eq!(std::fs::read(&placed).unwrap(), ed2);
    assert!(b.list("packs")["packs"][0]["can_roll_back"] == true);

    let r = b.act("roll_back_pack", json!({ "archive_uuid": SCIENCE_ID })).unwrap();
    assert_eq!(r["version"], "2026.09.1", "{r}");
    assert_eq!(std::fs::read(&placed).unwrap(), ed1, "byte for byte the first edition (SPEC §7.5)");
    assert_eq!(std::fs::read(sidecar(&placed)).unwrap(), ed1_sig, "and its own signature is back beside it");
    // only one step is kept
    let e = b.act("roll_back_pack", json!({ "archive_uuid": SCIENCE_ID })).unwrap_err();
    assert_eq!(e["code"], "edition_mismatch", "{e}");
    // and the newer edition may come back
    assert_eq!(b.install("science.wax", SCIENCE_ID)["state"], "done");

    // removing asks for the password again
    std::fs::write(lay.packs_data.join(SCIENCE_ID).join("saved"), "x").unwrap();
    assert_eq!(b.act("remove_pack", json!({ "archive_uuid": SCIENCE_ID })).unwrap_err()["code"], "confirmation_required");
    let r = b.act_confirmed("remove_pack", json!({ "archive_uuid": SCIENCE_ID })).unwrap();
    assert_eq!(r["done"], "removed");
    assert!(!placed.exists() && !sidecar(&placed).exists());
    assert!(!lay.packs_data.join(SCIENCE_ID).exists(), "its storage goes with it (contract §10)");
    assert_eq!(b.act_confirmed("remove_pack", json!({ "archive_uuid": SCIENCE_ID })).unwrap_err()["code"], "pack_not_found");

    // Track A §6: after the box has had 2026.10.1, 2026.09.1 cannot come back as new
    std::fs::write(&src, &ed1).unwrap();
    std::fs::write(sidecar(&src), &ed1_sig).unwrap();
    let j = b.install("science.wax", SCIENCE_ID);
    assert_eq!((j["state"].as_str(), j["error"]["code"].as_str()), (Some("failed_rollback"), Some("edition_older")), "{j}");

    let log = b.log();
    for a in ["install_pack", "roll_back_pack", "remove_pack"] {
        assert!(log.lines().any(|l| l.contains(a) && l.contains("\"result\":\"ok\"")), "{a} missing from the log:\n{log}");
    }
    assert!(log.lines().any(|l| l.contains("install_pack") && l.contains("edition_older")), "{log}");
}

#[test]
fn a_pack_not_signed_by_the_pack_key_is_never_installed() {
    let m = manifest("Science", "2026.09.1", "any", 64 * MIB);
    for key in [Key::Other, Key::None] {
        let b = start_with(key, "aarch64");
        b.offer("science.wax", SCIENCE, &m, 1_700_000_000);
        let j = b.install("science.wax", SCIENCE_ID);
        assert_eq!(j["state"], "failed_signature", "{j}");
        assert!(!b.layout().packs.join(format!("{SCIENCE_ID}.wax")).exists());
    }
    let b = start();
    // changed after signing: one byte of the index segment
    let p = b.offer("science.wax", SCIENCE, &m, 1_700_000_000);
    let (_, _, _) = wax_core::reader::signable_digest_of(&p).unwrap();
    let r = WaxReaderRanges::of(&p);
    let mut bytes = std::fs::read(&p).unwrap();
    let (o, l) = r[0];
    bytes[(o + l - 1) as usize] ^= 1;
    std::fs::write(&p, &bytes).unwrap();
    let j = b.install("science.wax", SCIENCE_ID);
    assert!(j["state"] == "failed_signature" || j["error"]["code"] == "invalid_request", "{j}");
    assert!(!b.layout().packs.join(format!("{SCIENCE_ID}.wax")).exists());
    // no signature beside it
    let p = b.offer("science.wax", SCIENCE, &m, 1_700_000_000);
    std::fs::remove_file(sidecar(&p)).unwrap();
    assert_eq!(b.install("science.wax", SCIENCE_ID)["state"], "failed_signature");
    // another pack's genuine signature beside it
    let atlas = b.offer("atlas.wax", ATLAS, &manifest("Atlas", "2026.09.1", "any", 64 * MIB), 1_700_000_000);
    b.offer("science.wax", SCIENCE, &m, 1_700_000_000);
    std::fs::copy(sidecar(&atlas), sidecar(&b.incoming().join("science.wax"))).unwrap();
    assert_eq!(b.install("science.wax", SCIENCE_ID)["state"], "failed_signature");
}

/// The segment ranges, as an honest reader sees them.
struct WaxReaderRanges;
impl WaxReaderRanges {
    fn of(p: &Path) -> Vec<(u64, u64)> {
        wax_core::WaxReader::open(p).unwrap().segment_ranges()
    }
}

/// A helper that lies, as one taken over by a crafted pack might.
struct Liar(Vec<u8>);
impl Checker for Liar {
    fn run(&self, _mode: &str, _pack: &Path) -> packs::Result<Vec<u8>> {
        Ok(self.0.clone())
    }
}

#[test]
fn shelld_checks_what_the_helper_says_rather_than_believing_it() {
    let b = start();
    let p = b.offer("science.wax", SCIENCE, &manifest("Science", "2026.09.1", "any", 64 * MIB), 1_700_000_000);
    let honest = WaxReaderRanges::of(&p);
    let size = std::fs::metadata(&p).unwrap().len();
    let pub_file = b.root().join("right.pub");
    std::fs::write(&pub_file, b.key.pk.to_box().unwrap().into_string()).unwrap();
    let trust = packs::Trust::from_file(&pub_file).unwrap();
    let lay = b.layout();
    let sources = packs::Folders(vec![("incoming".into(), b.incoming())]);
    let lies: Vec<Vec<(u64, u64)>> = vec![
        vec![],                                    // no segments
        vec![(128, 512)],                          // not where the header points
        vec![(0, 128), honest[0]],                 // the header counted twice
        vec![(honest[0].0, size)],                 // past the end
        vec![honest[0], honest[0]],                // overlapping
    ];
    for ranges in lies {
        let liar = Liar(json!({ "segments": ranges, "manifest": manifest("Science", "2026.09.1", "any", 1) }).to_string().into_bytes());
        let env = Env { layout: &lay, sources: &sources, trust: &trust, checker: &liar, measure: &Measured(&lay), data_owner: None };
        let a = packs::authorize_unchecked_for_tests(packs::Action::InstallPack { source: "incoming/science.wax".into(), archive_uuid: SCIENCE_ID.into() });
        let e = packs::perform(&env, a, &mut |_| {}).unwrap_err();
        assert!(matches!(e.code(), "invalid_request" | "signature_invalid"), "{ranges:?}: {e}");
        assert!(!lay.packs.join(format!("{SCIENCE_ID}.wax")).exists());
    }
    // the honest helper's answer, through the same path, installs
    let real = Helper { program: env!("CARGO_BIN_EXE_deltos-pack-check").into(), user: None };
    let env = Env { layout: &lay, sources: &sources, trust: &trust, checker: &real, measure: &Measured(&lay), data_owner: None };
    let a = packs::authorize_unchecked_for_tests(packs::Action::InstallPack { source: "incoming/science.wax".into(), archive_uuid: SCIENCE_ID.into() });
    packs::perform(&env, a, &mut |_| {}).unwrap();
}

#[test]
fn a_pack_that_does_not_fit_or_is_not_the_one_chosen_is_refused() {
    let b = start_with(Key::Right, "aarch64");
    // built for another architecture
    b.offer("pc.wax", SCIENCE, &manifest("PC only", "2026.09.1", "x86_64", 64 * MIB), 1_700_000_000);
    let j = b.install("pc.wax", SCIENCE_ID);
    assert_eq!(j["error"]["code"], "not_enough_resources", "{j}");
    assert!(j["error"]["detail"].as_str().unwrap().contains("x86_64"), "{j}");
    // needs more memory than the box has
    b.offer("big.wax", SCIENCE, &manifest("Big", "2026.09.1", "any", 8u64 << 30), 1_700_000_000);
    let j = b.install("big.wax", SCIENCE_ID);
    assert_eq!(j["error"]["code"], "not_enough_resources", "{j}");
    // a file that is not the pack the admin chose
    b.offer("atlas.wax", ATLAS, &manifest("Atlas", "2026.09.1", "any", 64 * MIB), 1_700_000_000);
    let j = b.install("atlas.wax", SCIENCE_ID);
    assert_eq!(j["error"]["code"], "invalid_request", "{j}");
    // a path, not a source id; an id in the bare form
    assert_eq!(b.act("install_pack", json!({ "source": "/etc/passwd.wax", "archive_uuid": SCIENCE_ID })).unwrap_err()["code"], "invalid_request");
    assert_eq!(
        b.act("install_pack", json!({ "source": "incoming/atlas.wax", "archive_uuid": ATLAS_ID.replace("-", "") })).unwrap_err()["code"],
        "invalid_request"
    );
    // a pack that stands on another one waits for it
    let mut m = manifest("Atlas notes", "2026.09.1", "any", 64 * MIB);
    m.insert("depends_on".into(), SCIENCE_ID.into());
    b.offer("notes.wax", ATLAS, &m, 1_700_000_000);
    let j = b.install("notes.wax", ATLAS_ID);
    assert!(j["error"]["detail"].as_str().unwrap().contains(SCIENCE_ID), "{j}");
    // a pack with no earlier edition kept cannot be rolled back
    let j = b.install("atlas.wax", ATLAS_ID);
    assert_eq!(j["state"], "done", "{j}");
    assert_eq!(b.act("roll_back_pack", json!({ "archive_uuid": ATLAS_ID })).unwrap_err()["code"], "edition_mismatch");
    assert!(!b.layout().packs.join(format!("{SCIENCE_ID}.wax")).exists());
}

#[test]
fn pack_actions_and_listings_need_a_secure_session_with_content_install() {
    let b = start();
    b.offer("science.wax", SCIENCE, &manifest("Science", "2026.09.1", "any", 64 * MIB), 1_700_000_000);
    let r = b.shelld.call(&json!({ "call": "act", "action": "install_pack", "args": { "source": "incoming/science.wax", "archive_uuid": SCIENCE_ID }, "session": b.admin, "over_https": false }));
    assert_eq!(r.unwrap_err()["code"], "secure_connection_required");
    let r = b.shelld.call(&json!({ "call": "list", "listing": "pack_sources", "session": b.admin, "over_https": false }));
    assert_eq!(r.unwrap_err()["code"], "secure_connection_required");
    // a teacher does not hold content.install by default (contract §4.2)
    b.id.call(&json!({ "call": "create_profile", "ctx": { "https": true }, "token": b.admin, "profile": {
        "display_name": "Teacher", "kind": "passworded", "sign_in_name": "teacher", "password": "teacher-password-1",
        "roles": [{ "role": "teacher" }] } }))
        .unwrap();
    let s = b
        .id
        .call(&json!({ "call": "sign_in_password", "ctx": { "https": true, "peer": "10.42.0.9" }, "sign_in_name": "teacher", "password": "teacher-password-1" }))
        .unwrap();
    let t = s["session_token"].as_str().unwrap();
    let r = b.shelld.call(&json!({ "call": "act", "action": "install_pack", "args": { "source": "incoming/science.wax", "archive_uuid": SCIENCE_ID }, "session": t, "over_https": true }));
    assert_eq!(r.unwrap_err()["code"], "permission_denied");
    let r = b.shelld.call(&json!({ "call": "list", "listing": "packs", "session": t, "over_https": true }));
    assert_eq!(r.unwrap_err()["code"], "permission_denied");
    // the extra argument is an error, not ignored
    assert_eq!(
        b.act("remove_pack", json!({ "archive_uuid": SCIENCE_ID, "keep_data": true })).unwrap_err()["code"],
        "invalid_request"
    );
}

/// On a box shelld is root and the helper is not: the helper can read the
/// install's own staging folder and nothing else there. Needs root to change
/// user; skips loudly otherwise (`DELTOS_REQUIRE_ROOT_TESTS=1` makes that a
/// failure).
#[test]
fn the_helper_runs_as_another_user_and_reads_only_its_own_staging_folder() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if unsafe { libc::geteuid() } != 0 {
        assert!(std::env::var_os("DELTOS_REQUIRE_ROOT_TESTS").is_none(), "DELTOS_REQUIRE_ROOT_TESTS is set but this is not root");
        eprintln!("SKIPPED (not root): the_helper_runs_as_another_user_and_reads_only_its_own_staging_folder");
        return;
    }
    let b = start();
    b.offer("science.wax", SCIENCE, &manifest("Science", "2026.09.1", "any", 64 * MIB), 1_700_000_000);
    let pub_file = b.root().join("right.pub");
    std::fs::write(&pub_file, b.key.pk.to_box().unwrap().into_string()).unwrap();
    let trust = packs::Trust::from_file(&pub_file).unwrap();
    let lay = b.layout();
    // the test's own folders must let "nobody" pass through to the packs
    for d in [b.root().to_path_buf(), b.root().join("var"), b.root().join("var/lib"), b.root().join("var/lib/deltos"), lay.packs.clone()] {
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let sources = packs::Folders(vec![("incoming".into(), b.incoming())]);
    let nobody = Helper { program: env!("CARGO_BIN_EXE_deltos-pack-check").into(), user: Some((65534, 65534)) };

    // another install's folder, not admitted, is out of its reach
    let other = lay.packs.join(".staging/other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::set_permissions(lay.packs.join(".staging"), std::fs::Permissions::from_mode(0o711)).unwrap();
    std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o700)).unwrap();
    let probe = other.join("p.wax");
    std::fs::copy(b.incoming().join("science.wax"), &probe).unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o644)).unwrap();
    let e = nobody.run("check", &probe).unwrap_err();
    assert!(e.to_string().contains("cannot be read"), "{e}");
    std::fs::remove_dir_all(&other).unwrap();

    // its own install's folder it can read, and the install goes through
    let env = Env { layout: &lay, sources: &sources, trust: &trust, checker: &nobody, measure: &Measured(&lay), data_owner: Some((65534, 65534)) };
    let a = packs::authorize_unchecked_for_tests(packs::Action::InstallPack { source: "incoming/science.wax".into(), archive_uuid: SCIENCE_ID.into() });
    packs::perform(&env, a, &mut |_| {}).unwrap();
    assert_eq!(std::fs::metadata(lay.packs_data.join(SCIENCE_ID)).unwrap().uid(), 65534, "its storage belongs to deltd's user");
    let st = std::fs::metadata(lay.packs.join(".staging")).unwrap();
    assert_eq!(st.mode() & 0o777, 0o711);
}
