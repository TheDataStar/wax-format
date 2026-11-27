//! Restore: review first, then apply (Track F §9).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::archive::{self, by_path, Manifest, PackRecord, MANIFEST_NAME, STATE_PREFIX};
use crate::set;
use crate::{hex, owners, Error, Layout, Passphrase, Result, FORMAT_VERSION};

/// The largest manifest a review reads (a box's state is thousands of
/// files at most; this bounds memory on a hostile file).
const MANIFEST_MAX: u64 = 16 * 1024 * 1024;
const REVIEW_PREFIX: &str = "review-";

/// Who holds what in the backup's register (Track F §9: "profile count,
/// roles, last activity").
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct People {
    pub profiles: u64,
    /// `passworded` / `passwordless` / `guest` -> count
    pub by_kind: BTreeMap<String, u64>,
    /// role name -> how many people hold it (any scope)
    pub roles: BTreeMap<String, u64>,
    /// Sign-in names of the box-wide admins: after a restore, these are the
    /// accounts that can sign in to the admin screens.
    pub admins: Vec<String>,
    pub last_activity: Option<i64>,
    pub remembered_devices: u64,
    pub sign_in_cards: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Network {
    /// `hotspot` or `joined`.
    pub mode: String,
    pub interface: String,
    /// The hotspot's name. Its password is never shown.
    pub ssid: Option<String>,
    pub country: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackStatus {
    #[serde(flatten)]
    pub pack: PackRecord,
    /// A pack with this archive uuid is installed on this box now.
    pub present: bool,
}

/// What a restore would do, for the admin to confirm.
#[derive(Debug, Clone, Serialize)]
pub struct Review {
    pub review_id: String,
    /// Binds the confirmation to exactly this backup's contents.
    pub review_digest: String,
    pub backup_id: String,
    pub created_at: i64,
    pub made_by: String,
    /// Top-level parts of the state tree the restore replaces.
    pub replaces: Vec<String>,
    /// Parts on this box the backup does not have; left as they are.
    pub keeps: Vec<String>,
    pub people: Option<People>,
    /// The backup's certificate authority (the box's "stamp"), as phones
    /// show it, and this box's current one.
    pub ca_fingerprint: Option<String>,
    pub current_ca_fingerprint: Option<String>,
    pub network: Option<Network>,
    pub packs: Vec<PackStatus>,
    pub files: usize,
    pub warnings: Vec<String>,
}

/// What [`apply`] did.
#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    pub replaced: Vec<String>,
    /// Where the replaced state now is.
    pub previous: PathBuf,
    pub packs_to_reinstall: Vec<PackRecord>,
    /// The services did not come back; the state is restored all the same.
    pub service_error: Option<String>,
}

/// Stopping and starting the services whose state is replaced.
pub trait Services {
    fn stop(&mut self) -> std::result::Result<(), String>;
    fn start(&mut self) -> std::result::Result<(), String>;
}

/// No services (tests, or an image tree).
pub struct NoServices;
impl Services for NoServices {
    fn stop(&mut self) -> std::result::Result<(), String> {
        Ok(())
    }
    fn start(&mut self) -> std::result::Result<(), String> {
        Ok(())
    }
}

fn safe_rel(s: &str) -> Result<PathBuf> {
    let p = Path::new(s);
    if s.is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(Error::Damaged(format!("{s:?} is not a path inside the state tree")));
    }
    Ok(p.to_path_buf())
}

pub fn check_review_id(id: &str) -> Result<()> {
    if id.len() == 16 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(Error::Invalid(format!("{id:?} is not a review id")))
    }
}

fn damaged(e: io::Error) -> Error {
    Error::Damaged(format!("the file ends early or was changed ({e})"))
}

/// Unpack into `dir/state/`, checking everything against the manifest.
fn unpack(path: &Path, passphrase: &Passphrase, dir: &Path) -> Result<(Manifest, Vec<u8>)> {
    let mut ar = archive::open(path, passphrase)?;
    let mut it = ar.entries().map_err(damaged)?;
    let mut first = it.next().ok_or_else(|| Error::Damaged("empty".into()))?.map_err(damaged)?;
    if first.path().map_err(damaged)?.to_string_lossy() != MANIFEST_NAME || first.size() > MANIFEST_MAX {
        return Err(Error::Damaged("the first member is not the backup's list".into()));
    }
    let mut mj = Vec::new();
    first.read_to_end(&mut mj).map_err(damaged)?;
    drop(first);
    let m: Manifest = serde_json::from_slice(&mj).map_err(|e| Error::Damaged(format!("the backup's list: {e}")))?;
    if m.format != "deltos-backup" {
        return Err(Error::Damaged("not a DeltOS backup".into()));
    }
    if m.format_version > FORMAT_VERSION {
        return Err(Error::TooNew(format!(
            "the backup is format {}, newer than this box understands ({FORMAT_VERSION}); update the box first",
            m.format_version
        )));
    }
    let want = by_path(&m);
    for e in &m.entries {
        let rel = safe_rel(&e.path)?;
        if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            let ps = parent.to_string_lossy();
            if want.get(ps.as_ref()).map(|p| p.kind != "dir").unwrap_or(true) {
                return Err(Error::Damaged(format!("{} has no folder in the list", e.path)));
            }
        }
        if e.kind != "dir" && e.kind != "file" {
            return Err(Error::Damaged(format!("{}: unknown kind {}", e.path, e.kind)));
        }
    }
    let state = dir.join("state");
    fs::create_dir(&state)?;
    let mut seen = BTreeSet::new();
    for ent in it {
        let mut ent = ent.map_err(damaged)?;
        let name = ent.path().map_err(damaged)?.to_string_lossy().into_owned();
        let rel_s = name
            .strip_prefix(STATE_PREFIX)
            .map(|s| s.trim_end_matches('/'))
            .ok_or_else(|| Error::Damaged(format!("{name}: outside the state")))?
            .to_string();
        let e = want.get(rel_s.as_str()).ok_or_else(|| Error::Damaged(format!("{rel_s} is not in the backup's list")))?;
        if !seen.insert(rel_s.clone()) {
            return Err(Error::Damaged(format!("{rel_s} appears twice")));
        }
        let rel = safe_rel(&rel_s)?;
        let et = ent.header().entry_type();
        let dst = state.join(&rel);
        match (e.kind.as_str(), et) {
            ("dir", tar::EntryType::Directory) => {
                fs::create_dir(&dst).map_err(|er| Error::Damaged(format!("{rel_s}: {er}")))?;
                fs::set_permissions(&dst, fs::Permissions::from_mode(0o700))?;
            }
            ("file", tar::EntryType::Regular) => {
                if ent.size() != e.size {
                    return Err(Error::Damaged(format!("{rel_s}: size differs from the list")));
                }
                let mut out = fs::OpenOptions::new().write(true).create_new(true).open(&dst).map_err(|er| Error::Damaged(format!("{rel_s}: {er}")))?;
                fs::set_permissions(&dst, fs::Permissions::from_mode(0o600))?;
                let mut h = Sha256::new();
                let mut buf = vec![0u8; 64 * 1024];
                let mut n = 0u64;
                loop {
                    let k = ent.read(&mut buf).map_err(damaged)?;
                    if k == 0 {
                        break;
                    }
                    h.update(&buf[..k]);
                    out.write_all(&buf[..k])?;
                    n += k as u64;
                }
                out.sync_all()?;
                if n != e.size || hex(&h.finalize()) != e.sha256 {
                    return Err(Error::Damaged(format!("{rel_s}: contents differ from the list")));
                }
            }
            _ => return Err(Error::Damaged(format!("{rel_s}: wrong kind of member"))),
        }
    }
    if seen.len() != m.entries.len() {
        let missing: Vec<_> = m.entries.iter().filter(|e| !seen.contains(&e.path)).map(|e| e.path.clone()).collect();
        return Err(Error::Damaged(format!("missing from the file: {}", missing.join(", "))));
    }
    Ok((m, mj))
}

fn sqlite_version(path: &Path, sql: &str) -> Result<Option<i64>> {
    if !path.is_file() {
        return Ok(None);
    }
    let c = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let v: rusqlite::Result<String> = c.query_row(sql, [], |r| r.get::<_, rusqlite::types::Value>(0)).map(|v| match v {
        rusqlite::types::Value::Integer(i) => i.to_string(),
        rusqlite::types::Value::Text(t) => t,
        _ => String::new(),
    });
    match v {
        Ok(s) => s.parse().map(Some).map_err(|_| Error::Damaged(format!("{}: schema version {s:?} is not a number", path.display()))),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Contract §10: refuse state newer than the programs on this box.
fn check_versions(state: &Path) -> Result<()> {
    let checks: [(&str, &str, i64, &str); 4] = [
        (set::IDENTITY_DB, "SELECT value FROM meta WHERE key='schema_version'", deltos_identity::register::SCHEMA_VERSION, "the register of people"),
        (set::NETWORK_DB, "PRAGMA user_version", deltos_net::registry::REGISTRY_SCHEMA_VERSION, "the network settings"),
        (set::CATALOG_DB, "PRAGMA user_version", deltlib::catalog::CATALOG_SCHEMA_VERSION, "the pack catalog"),
        (set::PERSONAL_DB, "PRAGMA user_version", deltlib::personal::PERSONAL_SCHEMA_VERSION, "people's bookmarks, notes and history"),
    ];
    for (rel, sql, ours, what) in checks {
        if let Some(v) = sqlite_version(&state.join(rel), sql)? {
            if v > ours {
                return Err(Error::TooNew(format!("{what} in the backup is schema {v}, newer than this box's {ours}; update the box first")));
            }
        }
    }
    Ok(())
}

fn ca_fingerprint(dir: &Path) -> Result<Option<String>> {
    let key = dir.join(set::CA_KEY).exists();
    let crt = dir.join(set::CA_CERT);
    match (key, crt.exists()) {
        (false, false) => Ok(None),
        (true, true) => {
            let pem = fs::read_to_string(&crt).map_err(|e| Error::Damaged(format!("ca.crt: {e}")))?;
            let der = deltos_tls::pem_to_der(&pem).map_err(|e| Error::Damaged(format!("ca.crt: {e}")))?;
            Ok(Some(deltos_tls::fingerprint(&der)))
        }
        // docs/box-certificates.md §2: half a CA is refused, never completed
        _ => Err(Error::Damaged("the certificate authority is half there (key without certificate, or the reverse)".into())),
    }
}

fn people(db: &Path, last_activity: Option<i64>) -> Result<Option<People>> {
    if !db.is_file() {
        return Ok(None);
    }
    let c = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut p = People { last_activity, ..Default::default() };
    p.profiles = c.query_row("SELECT count(*) FROM profiles", [], |r| r.get::<_, i64>(0))? as u64;
    let mut s = c.prepare("SELECT kind, count(*) FROM profiles GROUP BY kind ORDER BY kind")?;
    for r in s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (k, n) = r?;
        p.by_kind.insert(k, n as u64);
    }
    let mut s = c.prepare("SELECT role, count(DISTINCT profile_id) FROM profile_roles GROUP BY role ORDER BY role")?;
    for r in s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (k, n) = r?;
        p.roles.insert(k, n as u64);
    }
    let mut s = c.prepare(
        "SELECT p.sign_in_name FROM profile_roles r JOIN profiles p ON p.profile_id = r.profile_id
         WHERE r.role = 'admin' AND r.scope = '' AND p.sign_in_name IS NOT NULL ORDER BY p.sign_in_name",
    )?;
    for r in s.query_map([], |r| r.get::<_, String>(0))? {
        p.admins.push(r?);
    }
    p.remembered_devices = c.query_row("SELECT count(*) FROM devices", [], |r| r.get::<_, i64>(0))? as u64;
    p.sign_in_cards = c.query_row("SELECT count(*) FROM cards", [], |r| r.get::<_, i64>(0))? as u64;
    Ok(Some(p))
}

fn network(db: &Path, probe: &Path) -> Result<Option<Network>> {
    if !db.is_file() {
        return Ok(None);
    }
    // the registry's own code reads it, from a throwaway copy, so the
    // staged file is restored exactly as it was backed up
    fs::copy(db, probe)?;
    let out = (|| {
        let reg = deltos_net::Registry::open(probe).map_err(|e| Error::Damaged(format!("network settings: {e}")))?;
        let s = reg.settings().map_err(|e| Error::Damaged(format!("network settings: {e}")))?;
        let (mode, interface) = match s.mode {
            deltos_net::Mode::Hotspot { interface, .. } => ("hotspot", interface),
            deltos_net::Mode::Joined { interface, .. } => ("joined", interface),
        };
        let radio = reg.radio().map_err(|e| Error::Damaged(format!("network settings: {e}")))?;
        Ok(Some(Network {
            mode: mode.into(),
            interface,
            ssid: radio.as_ref().map(|r| r.ssid.clone()),
            country: radio.as_ref().map(|r| r.country.clone()),
        }))
    })();
    let _ = fs::remove_file(probe);
    out
}

fn live_pack_uuids(root: &Path) -> BTreeSet<String> {
    let dir = root.join(set::PACKS_DIR);
    let mut out = BTreeSet::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "wax").unwrap_or(false) {
                if let Ok(r) = wax_core::WaxReader::open(&p) {
                    out.insert(deltlib::library::uuid_text(&r.header().archive_uuid));
                }
            }
        }
    }
    out
}

fn tops(m: &Manifest) -> Vec<String> {
    let mut t: BTreeSet<String> = BTreeSet::new();
    for e in &m.entries {
        t.insert(e.path.split('/').next().unwrap_or_default().to_string());
    }
    t.into_iter().collect()
}

/// Remove earlier reviews: one staged restore at a time.
fn clear_reviews(layout: &Layout) {
    if let Ok(rd) = fs::read_dir(layout.staging()) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(REVIEW_PREFIX) {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Step 1: decrypt, check and stage a backup; change nothing live.
pub fn review(layout: &Layout, backup: &Path, backup_id: &str, passphrase: &Passphrase) -> Result<Review> {
    archive::check_backup_id(backup_id)?;
    if !backup.is_file() {
        return Err(Error::NotFound(format!("no backup {backup_id}")));
    }
    clear_reviews(layout);
    let dir = archive::private_dir(&layout.staging(), REVIEW_PREFIX)?;
    let review_id = dir.file_name().unwrap().to_string_lossy()[REVIEW_PREFIX.len()..].to_string();
    let out = (|| {
        let (m, mj) = unpack(backup, passphrase, &dir)?;
        let state = dir.join("state");
        for t in tops(&m) {
            if set::treatment(&t) != set::Treatment::Include {
                return Err(Error::Damaged(format!("{t} is never part of a backup")));
            }
        }
        check_versions(&state)?;
        if layout.set_owners {
            for e in &m.entries {
                if owners::uid_of(&e.owner).is_none() || owners::gid_of(&e.group).is_none() {
                    return Err(Error::Invalid(format!(
                        "{} belongs to {}:{}, which this box does not have; install the same DeltOS services first",
                        e.path, e.owner, e.group
                    )));
                }
            }
        }
        let ca = ca_fingerprint(&state)?;
        let current_ca = ca_fingerprint(&layout.root).ok().flatten();
        let people = people(&state.join(set::IDENTITY_DB), m.identity_last_activity)?;
        let network = network(&state.join(set::NETWORK_DB), &dir.join("network-probe.sqlite"))?;
        let live = live_pack_uuids(&layout.root);
        let packs: Vec<PackStatus> =
            m.packs.iter().map(|p| PackStatus { present: !p.archive_uuid.is_empty() && live.contains(&p.archive_uuid), pack: p.clone() }).collect();
        let replaces = tops(&m);
        let mut keeps = Vec::new();
        for e in fs::read_dir(&layout.root)?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if set::treatment(&n) == set::Treatment::Include && !replaces.contains(&n) {
                keeps.push(n);
            }
        }
        keeps.sort();
        let mut warnings = vec!["Everyone signs in again afterwards: sign-ins in progress are not kept in a backup.".to_string()];
        if let Some(p) = &people {
            if replaces.iter().any(|t| t == set::IDENTITY_DIR) {
                warnings.push(format!(
                    "The people on this box now are replaced by the backup's {}. Sign in afterwards as one of the backup's admins: {}.",
                    p.profiles,
                    if p.admins.is_empty() { "(none with a password)".to_string() } else { p.admins.join(", ") }
                ));
                if p.admins.is_empty() {
                    warnings.push("The backup has no admin who can sign in with a password; nobody could change the box afterwards.".into());
                }
            }
        }
        match (&ca, &current_ca) {
            (Some(a), Some(b)) if a != b => warnings.push(format!(
                "The box's certificate changes back to the backup's ({a}). Devices that trusted the backed-up box trust it again; a device set up on this box since then must set up again."
            )),
            (None, _) => warnings.push("The backup has no certificate authority; this box keeps its own.".into()),
            _ => {}
        }
        let missing = packs.iter().filter(|p| !p.present).count();
        if missing > 0 {
            warnings.push(format!("{missing} pack(s) in the backup are not on this box and must be installed again (pack files are never in a backup)."));
        }
        let mut digest = Sha256::new();
        digest.update(backup_id.as_bytes());
        digest.update(b"\n");
        digest.update(&mj);
        let review_digest = hex(&digest.finalize());
        let mut f = File::create(dir.join(MANIFEST_NAME))?;
        f.write_all(&mj)?;
        f.sync_all()?;
        fs::write(dir.join("backup-id"), backup_id)?;
        Ok(Review {
            review_id: review_id.clone(),
            review_digest,
            backup_id: backup_id.into(),
            created_at: m.created_at,
            made_by: m.made_by.clone(),
            replaces,
            keeps,
            people,
            ca_fingerprint: ca,
            current_ca_fingerprint: current_ca,
            network,
            packs,
            files: m.entries.iter().filter(|e| e.kind == "file").count(),
            warnings,
        })
    })();
    if out.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    out
}

/// The review a digest was computed over, read back from staging.
fn staged(layout: &Layout, review_id: &str) -> Result<(PathBuf, Manifest, String)> {
    check_review_id(review_id)?;
    let dir = layout.staging().join(format!("{REVIEW_PREFIX}{review_id}"));
    let mj = fs::read(dir.join(MANIFEST_NAME)).map_err(|_| Error::NotFound(format!("no review {review_id}; review the backup again")))?;
    let backup_id = fs::read_to_string(dir.join("backup-id"))?;
    let m: Manifest = serde_json::from_slice(&mj).map_err(|e| Error::Damaged(e.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(backup_id.as_bytes());
    digest.update(b"\n");
    digest.update(&mj);
    Ok((dir, m, hex(&digest.finalize())))
}

/// The digest of a staged review, for root at the console, who confirms by
/// typing rather than with a ticket.
pub fn staged_digest(layout: &Layout, review_id: &str) -> Result<String> {
    Ok(staged(layout, review_id)?.2)
}

/// Owners and modes from the list, deepest first so a folder's own mode is
/// set after its contents are written.
fn set_owners(layout: &Layout, state: &Path, m: &Manifest) -> Result<()> {
    let mut es: Vec<_> = m.entries.iter().collect();
    es.sort_by_key(|e| std::cmp::Reverse(e.path.matches('/').count()));
    for e in es {
        let p = state.join(&e.path);
        if layout.set_owners {
            let uid = owners::uid_of(&e.owner).ok_or_else(|| Error::RestoreFailed(format!("no user {}", e.owner)))?;
            let gid = owners::gid_of(&e.group).ok_or_else(|| Error::RestoreFailed(format!("no group {}", e.group)))?;
            owners::chown(&p, uid, gid)?;
        }
        fs::set_permissions(&p, fs::Permissions::from_mode(e.mode & 0o7777))?;
    }
    Ok(())
}

/// Step 2: put a reviewed backup in place. `expected_digest` is the
/// review's digest the admin confirmed; a different staged backup is
/// refused.
pub fn apply(layout: &Layout, review_id: &str, expected_digest: &str, services: &mut dyn Services) -> Result<Applied> {
    let (dir, m, digest) = staged(layout, review_id)?;
    if digest != expected_digest {
        return Err(Error::Invalid("the staged backup is not the one reviewed; review it again".into()));
    }
    let state = dir.join("state");
    set_owners(layout, &state, &m).map_err(|e| Error::RestoreFailed(format!("nothing was changed: {e}")))?;
    let comps = tops(&m);
    let prev = layout.previous();
    if prev.exists() {
        fs::remove_dir_all(&prev)?;
    }
    fs::create_dir(&prev)?;
    fs::set_permissions(&prev, fs::Permissions::from_mode(0o700))?;
    services.stop().map_err(|e| Error::RestoreFailed(format!("could not stop the services: {e}")))?;

    // (component, live was moved aside)
    let mut done: Vec<(String, bool)> = Vec::new();
    let mut fail: Option<String> = None;
    for c in &comps {
        let live = layout.root.join(c);
        let moved = live.exists();
        if moved {
            if let Err(e) = fs::rename(&live, prev.join(c)) {
                fail = Some(format!("{c}: {e}"));
                break;
            }
        }
        let swapped = if layout.fault_swap.as_deref() == Some(c.as_str()) {
            Err(io::Error::other("injected fault"))
        } else {
            fs::rename(state.join(c), &live)
        };
        if let Err(e) = swapped {
            if moved {
                let _ = fs::rename(prev.join(c), &live);
            }
            fail = Some(format!("{c}: {e}"));
            break;
        }
        done.push((c.clone(), moved));
    }
    if let Some(f) = fail {
        // put back everything already swapped, newest first
        for (c, moved) in done.into_iter().rev() {
            let live = layout.root.join(&c);
            let _ = fs::rename(&live, state.join(&c));
            if moved {
                let _ = fs::rename(prev.join(&c), &live);
            }
        }
        let _ = services.start();
        return Err(Error::RestoreFailed(format!("nothing was changed: {f}")));
    }
    File::open(&layout.root)?.sync_all()?;
    let _ = fs::remove_dir_all(&dir);
    let live = live_pack_uuids(&layout.root);
    let packs_to_reinstall = m.packs.iter().filter(|p| p.archive_uuid.is_empty() || !live.contains(&p.archive_uuid)).cloned().collect();
    let service_error = services.start().err();
    Ok(Applied { replaced: comps, previous: prev, packs_to_reinstall, service_error })
}
