//! Installing, updating, rolling back and removing add-ons: the add-on half
//! of `deltos-installd` (F4), run inside `deltos-shelld` behind its checks
//! (`docs/base-addons-admin.md` §7, `docs/shelld.md` §4).
//!
//! The same rule as `deltos_net::actions` and `deltos_backup::actions`:
//! **nothing here changes the box except through [`perform`], which takes
//! only an [`Authorized`], which only [`authorize`] makes**, after a live
//! secure session, the action's permission, and for the re-confirmed ones a
//! single-use ticket bound to this session, action and argument digest.
//!
//! The pipeline is contract §7.2's: the bundle is **copied** into root's own
//! staging folder before anything reads it (so a file swapped after the
//! check cannot be installed), its minisign signature is checked against the
//! project key, it is unpacked with only plain files and folders allowed, its
//! manifest is checked against the id and version the admin chose and the fit
//! check is run, and only then is it renamed into place and registered.
//!
//! Starting and stopping the add-on's containers is G1's (the platform
//! add-on, stage 5), not built yet. Until then the register records whether
//! the admin wants each add-on running, and G1 will read it.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use deltos_apps::fit::{self, Capability, Verdict};
use deltos_apps::manifest::Kind;
use deltos_apps::AppManifest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use deltos_net::actions::{Caller, IdentityCheck, Refusal};

/// Install, update, roll back and remove (contract §4.2, proposed 2026-11-22).
pub const MANAGE: &str = "apps.manage";
/// Stopping and starting a service (contract §4.2).
pub const RESTART: &str = "service.restart";

/// What a bundle file is called (`docs/base-addons-admin.md` §7).
pub const BUNDLE_SUFFIX: &str = ".deltos-addon";
/// Its detached signature, beside it.
pub const SIG_SUFFIX: &str = ".minisig";
/// The manifest inside a bundle.
pub const MANIFEST: &str = "deltos-app.toml";
/// Every bundle carries its own licence text.
pub const LICENCE: &str = "LICENCE";
/// Most entries a bundle may hold. A bound, not a measurement: the draft
/// add-ons hold five or six files plus their image archives.
pub const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Signature(String),
    #[error("{0}")]
    DoesNotFit(String),
    /// The fit check cannot tell (an open floor): the admin must accept that
    /// with their password, by sending the request again with
    /// `accept_unmeasured` (`docs/base-addons-admin.md` §5.3).
    #[error("{0}")]
    Unmeasured(String),
    #[error("{0}")]
    NoStorage(String),
    #[error("{0}")]
    Io(String),
}

impl Error {
    /// deltd API §7's codes (`does_not_fit` and `app_not_found` added with this crate).
    pub fn code(&self) -> &'static str {
        match self {
            Error::Invalid(_) => "invalid_request",
            Error::NotFound(_) => "app_not_found",
            Error::Signature(_) => "signature_invalid",
            Error::DoesNotFit(_) => "does_not_fit",
            Error::Unmeasured(_) => "confirmation_required",
            Error::NoStorage(_) => "not_enough_storage",
            Error::Io(_) => "internal",
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// The closed list of add-on actions (privilege boundary §8, proposed 2026-11-22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `source` is an id from [`Sources::list`]; `id` and `version` are what
    /// the admin chose, and the bundle must say the same.
    InstallAddon { source: String, id: String, version: String, accept_unmeasured: bool },
    UpdateAddon { source: String, id: String, version: String, accept_unmeasured: bool },
    /// `version` must be the retained previous one.
    RollBackAddon { id: String, version: String },
    RemoveAddon { id: String, keep_data: bool },
    SetAddonRunning { id: String, on: bool },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::InstallAddon { .. } => "install_addon",
            Action::UpdateAddon { .. } => "update_addon",
            Action::RollBackAddon { .. } => "roll_back_addon",
            Action::RemoveAddon { .. } => "remove_addon",
            Action::SetAddonRunning { .. } => "set_addon_running",
        }
    }

    pub fn permission(&self) -> &'static str {
        match self {
            Action::SetAddonRunning { .. } => RESTART,
            _ => MANAGE,
        }
    }

    /// Removing always asks again; installing or updating asks only when the
    /// admin accepts an add-on the fit check cannot vouch for (§5.3).
    pub fn needs_reconfirm(&self) -> bool {
        match self {
            Action::InstallAddon { accept_unmeasured, .. } | Action::UpdateAddon { accept_unmeasured, .. } => *accept_unmeasured,
            Action::RemoveAddon { .. } => true,
            _ => false,
        }
    }

    /// Installs and updates can take minutes (image archives): a job.
    pub fn is_long(&self) -> bool {
        matches!(self, Action::InstallAddon { .. } | Action::UpdateAddon { .. })
    }

    pub fn canonical_args(&self) -> String {
        match self {
            Action::InstallAddon { source, id, version, accept_unmeasured }
            | Action::UpdateAddon { source, id, version, accept_unmeasured } => {
                format!("source={source}\nid={id}\nversion={version}\naccept_unmeasured={accept_unmeasured}")
            }
            Action::RollBackAddon { id, version } => format!("id={id}\nversion={version}"),
            Action::RemoveAddon { id, keep_data } => format!("id={id}\nkeep_data={keep_data}"),
            Action::SetAddonRunning { id, on } => format!("id={id}\non={on}"),
        }
    }

    pub fn args_digest(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.name().as_bytes());
        h.update(b"\n");
        h.update(self.canonical_args().as_bytes());
        hex(&h.finalize())
    }

    /// Nothing secret in any of them, so the log line is the arguments.
    pub fn summary(&self) -> String {
        format!("{} {}", self.name(), self.canonical_args().replace('\n', " "))
    }

    fn validate(&self) -> Result<()> {
        match self {
            Action::InstallAddon { source, id, version, .. } | Action::UpdateAddon { source, id, version, .. } => {
                check_source_id(source)?;
                check_id(id)?;
                check_version(version)
            }
            Action::RollBackAddon { id, version } => {
                check_id(id)?;
                check_version(version)
            }
            Action::RemoveAddon { id, .. } | Action::SetAddonRunning { id, .. } => check_id(id),
        }
    }
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// An app id, as the manifest allows it (`[a-z][a-z0-9-]*`, 2 to 32).
pub fn check_id(id: &str) -> Result<()> {
    let b = id.as_bytes();
    let ok = (2..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("{id:?} is not an app id")))
    }
}

/// `MAJOR.MINOR.PATCH`, as the manifest requires.
pub fn check_version(v: &str) -> Result<()> {
    parse_version(v).map(|_| ()).ok_or_else(|| Error::Invalid(format!("{v:?} is not a version (MAJOR.MINOR.PATCH)")))
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut p = v.split('.');
    let mut n = || -> Option<u64> {
        let s = p.next()?;
        if s.is_empty() || s.len() > 9 || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0')) {
            return None;
        }
        s.parse().ok()
    };
    let t = (n()?, n()?, n()?);
    p.next().is_none().then_some(t)
}

/// `<origin>/<file>`: which listing it came from, and a plain bundle file name.
pub fn check_source_id(s: &str) -> Result<()> {
    let bad = || Error::Invalid(format!("{s:?} is not a source id from the add-on listing"));
    let (origin, file) = s.split_once('/').ok_or_else(bad)?;
    let plain = |x: &str| !x.is_empty() && !x.starts_with('.') && x.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if s.len() > 160 || !plain(origin) || !plain(file) || !file.ends_with(BUNDLE_SUFFIX) {
        return Err(bad());
    }
    Ok(())
}

/// An action that passed every check. Only [`authorize`] makes one.
#[derive(Debug)]
pub struct Authorized {
    action: Action,
    by: String,
}

impl Authorized {
    pub fn action(&self) -> &Action {
        &self.action
    }
    pub fn by(&self) -> &str {
        &self.by
    }
}

/// The session checks without an action: listings are reads, but they show
/// what the box runs, so they need the same secure session and permission.
pub fn check_reader(caller: &Caller, id: &dyn IdentityCheck) -> std::result::Result<String, Refusal> {
    let (profile, secure) = id.session(caller.session_ref).ok_or(Refusal::NotSignedIn)?;
    if !caller.over_https || !secure {
        return Err(Refusal::SecureConnectionRequired);
    }
    if !id.has_permission(&profile, MANAGE) {
        return Err(Refusal::PermissionDenied);
    }
    Ok(profile)
}

/// shelld's checks. The ticket is redeemed only after every other check
/// passed, so a refused request never spends it.
pub fn authorize(action: Action, caller: &Caller, id: &dyn IdentityCheck) -> std::result::Result<Authorized, Refusal> {
    let (profile, secure) = id.session(caller.session_ref).ok_or(Refusal::NotSignedIn)?;
    if !caller.over_https || !secure {
        return Err(Refusal::SecureConnectionRequired);
    }
    if !id.has_permission(&profile, action.permission()) {
        return Err(Refusal::PermissionDenied);
    }
    action.validate().map_err(|e| Refusal::Invalid(e.to_string()))?;
    if action.needs_reconfirm() {
        let t = caller.confirmation_ticket.ok_or(Refusal::ConfirmationRequired)?;
        if !id.redeem_ticket(t, caller.session_ref, action.name(), &action.args_digest()) {
            return Err(Refusal::ConfirmationRequired);
        }
    }
    Ok(Authorized { action, by: profile })
}

/// Test-only: an [`Authorized`] without the checks.
#[doc(hidden)]
pub fn authorize_unchecked_for_tests(action: Action) -> Authorized {
    Authorized { action, by: "test".into() }
}

/// Where add-ons live (contract §10, proposed 2026-11-22; `docs/shelld.md` §4.2).
#[derive(Debug, Clone)]
pub struct Layout {
    /// `/usr/lib/deltos/apps`: base manifests as `<id>.toml`, and each
    /// add-on's unpacked bundles as `<id>/<version>/`.
    pub programs: PathBuf,
    /// `/var/lib/deltos/apps`: each add-on's own data as `<id>/`, and the
    /// register of installed add-ons, `installed.json`.
    pub data: PathBuf,
    /// `/var/lib/deltos/capability.json` (contract §9).
    pub capability: PathBuf,
}

impl Layout {
    pub fn under(root: &Path) -> Layout {
        Layout {
            programs: root.join("usr/lib/deltos/apps"),
            data: root.join("var/lib/deltos/apps"),
            capability: root.join("var/lib/deltos/capability.json"),
        }
    }
    pub fn box_default() -> Layout {
        Layout::under(Path::new("/"))
    }
    fn register(&self) -> PathBuf {
        self.data.join("installed.json")
    }
    /// On the programs' own filesystem, so placing a bundle is a rename.
    fn staging(&self) -> PathBuf {
        self.programs.join(".staging")
    }
    fn bundle_dir(&self, id: &str, version: &str) -> PathBuf {
        self.programs.join(id).join(version)
    }
}

/// One installed add-on in the register.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub version: String,
    /// The version before the last update, kept so it can be rolled back to.
    pub previous: Option<String>,
    /// Whether the admin wants it running; G1 (stage 5) acts on it.
    pub wanted_running: bool,
    pub installed_by: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Register {
    pub schema: u32,
    pub apps: BTreeMap<String, Installed>,
}

impl Register {
    pub fn load(layout: &Layout) -> Result<Register> {
        match fs::read(layout.register()) {
            Ok(b) => {
                let r: Register = serde_json::from_slice(&b).map_err(|e| Error::Io(format!("{}: {e}", layout.register().display())))?;
                if r.schema != 1 {
                    return Err(Error::Io(format!("installed.json schema {} is newer than this shelld", r.schema)));
                }
                Ok(r)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Register { schema: 1, apps: BTreeMap::new() }),
            Err(e) => Err(e.into()),
        }
    }

    /// Written whole to a temporary file, synced, then renamed over the old
    /// one, so a power cut leaves either the old register or the new one.
    fn save(&self, layout: &Layout) -> Result<()> {
        fs::create_dir_all(&layout.data)?;
        let tmp = layout.data.join(".installed.json.tmp");
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(self).map_err(|e| Error::Io(e.to_string()))?)?;
        f.sync_all()?;
        fs::rename(&tmp, layout.register())?;
        if let Ok(d) = fs::File::open(&layout.data) {
            let _ = d.sync_all();
        }
        Ok(())
    }
}

/// The manifests the box runs now: the base ones from the system image and
/// every installed add-on's current one. `except` leaves one add-on out (the
/// one being updated, which the fit check must not count twice).
pub fn installed_manifests(layout: &Layout, reg: &Register, except: Option<&str>) -> Result<Vec<AppManifest>> {
    let mut out = Vec::new();
    if layout.programs.is_dir() {
        out = deltos_apps::read_dir(&layout.programs).map_err(Error::Io)?;
    }
    for (id, i) in &reg.apps {
        if Some(id.as_str()) == except {
            continue;
        }
        out.push(read_manifest(&layout.bundle_dir(id, &i.version))?);
    }
    Ok(out)
}

fn read_manifest(dir: &Path) -> Result<AppManifest> {
    let p = dir.join(MANIFEST);
    let text = fs::read_to_string(&p).map_err(|e| Error::Io(format!("{}: {e}", p.display())))?;
    deltos_apps::parse(&text).map_err(|e| Error::Invalid(format!("{}: {e}", p.display())))
}

/// One bundle an admin may install, from a listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Offered {
    pub source: String,
    pub bytes: u64,
    pub signed: bool,
}

/// Where bundles can come from: each origin is a folder (a mounted drive, or
/// later the store's download folder and the upload staging folder).
/// Names, never paths, cross the boundary (rule 6).
pub trait Sources {
    fn origins(&self) -> Vec<(String, PathBuf)>;

    fn list(&self) -> Vec<Offered> {
        let mut out = Vec::new();
        for (origin, dir) in self.origins() {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let source = format!("{origin}/{name}");
                if check_source_id(&source).is_err() {
                    continue;
                }
                let Ok(md) = fs::symlink_metadata(e.path()) else { continue };
                if !md.is_file() {
                    continue;
                }
                let signed = dir.join(format!("{name}{SIG_SUFFIX}")).is_file();
                out.push(Offered { source, bytes: md.len(), signed });
            }
        }
        out.sort_by(|a, b| a.source.cmp(&b.source));
        out
    }

    /// The bundle and its signature, for an id [`Sources::list`] handed out.
    fn resolve(&self, source: &str) -> Result<(PathBuf, PathBuf)> {
        check_source_id(source)?;
        let (origin, file) = source.split_once('/').unwrap();
        let dir = self
            .origins()
            .into_iter()
            .find(|(o, _)| o == origin)
            .map(|(_, d)| d)
            .ok_or_else(|| Error::NotFound(format!("no add-on source {origin:?}; list the sources again")))?;
        let p = dir.join(file);
        match fs::symlink_metadata(&p) {
            Ok(m) if m.is_file() => Ok((p, dir.join(format!("{file}{SIG_SUFFIX}")))),
            _ => Err(Error::NotFound(format!("{source} is not there any more; list the sources again"))),
        }
    }
}

/// Folders given on the command line, each under its own name.
pub struct Folders(pub Vec<(String, PathBuf)>);
impl Sources for Folders {
    fn origins(&self) -> Vec<(String, PathBuf)> {
        self.0.clone()
    }
}

/// The key bundles must be signed with. `None` when the box has none
/// configured: every install is then refused, never let through unsigned.
pub struct Trust {
    key: Option<minisign_verify::PublicKey>,
}

impl Trust {
    pub fn none() -> Trust {
        Trust { key: None }
    }
    pub fn from_base64(b64: &str) -> Result<Trust> {
        let key = minisign_verify::PublicKey::from_base64(b64.trim()).map_err(|e| Error::Invalid(format!("add-on key: {e}")))?;
        Ok(Trust { key: Some(key) })
    }
    /// A minisign `.pub` file (two lines, or the key alone).
    pub fn from_file(p: &Path) -> Result<Trust> {
        let text = fs::read_to_string(p).map_err(|e| Error::Io(format!("{}: {e}", p.display())))?;
        let key = minisign_verify::PublicKey::decode(&text)
            .or_else(|_| minisign_verify::PublicKey::from_base64(text.trim()))
            .map_err(|e| Error::Invalid(format!("{}: {e}", p.display())))?;
        Ok(Trust { key: Some(key) })
    }

    fn verify(&self, bundle: &Path, sig: &Path) -> Result<()> {
        let key = self.key.as_ref().ok_or_else(|| Error::Signature("this box has no add-on signing key set, so it installs no add-on".into()))?;
        let st = fs::read_to_string(sig).map_err(|_| Error::Signature("the bundle has no signature beside it".into()))?;
        if st.len() > 4096 {
            return Err(Error::Signature("the signature file is not a minisign signature".into()));
        }
        let s = minisign_verify::Signature::decode(&st).map_err(|e| Error::Signature(format!("the signature cannot be read: {e}")))?;
        let mut v = key.verify_stream(&s).map_err(|e| Error::Signature(format!("not signed by the add-on key: {e}")))?;
        let mut f = fs::File::open(bundle)?;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            v.update(&buf[..n]);
        }
        v.finalize().map_err(|_| Error::Signature("the bundle does not match its signature".into()))
    }
}

/// What the box measured about itself, and the free space now.
pub trait Measure {
    fn capability(&self) -> Result<Capability>;
    /// Free bytes on the filesystem the add-on's data goes on.
    fn free_storage_bytes(&self) -> Result<u64>;
}

/// The box's own record and `statvfs`.
pub struct Measured<'a>(pub &'a Layout);
impl Measure for Measured<'_> {
    fn capability(&self) -> Result<Capability> {
        let p = &self.0.capability;
        let t = fs::read_to_string(p).map_err(|e| Error::DoesNotFit(format!("the box has not measured itself yet ({}: {e})", p.display())))?;
        serde_json::from_str(&t).map_err(|e| Error::Io(format!("{}: {e}", p.display())))
    }
    fn free_storage_bytes(&self) -> Result<u64> {
        let mut dir = self.0.data.clone();
        while !dir.exists() {
            if !dir.pop() {
                break;
            }
        }
        free_bytes(&dir)
    }
}

pub fn free_bytes(p: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|e| Error::Io(e.to_string()))?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: statvfs writes into `s`, which is a statvfs.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(s.f_bavail as u64 * s.f_frsize as u64)
}

/// What an action did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "done", rename_all = "snake_case")]
pub enum Outcome {
    Installed { id: String, version: String, verdict: String },
    Updated { id: String, version: String, previous: String, verdict: String },
    RolledBack { id: String, version: String },
    Removed { id: String, data_kept: bool },
    RunningSet { id: String, wanted_running: bool },
}

/// Perform an authorized action. `progress` hears contract §7.2's states.
pub fn perform(
    layout: &Layout,
    a: Authorized,
    sources: &dyn Sources,
    trust: &Trust,
    measure: &dyn Measure,
    progress: &mut dyn FnMut(&str),
) -> Result<Outcome> {
    let by = a.by;
    match a.action {
        Action::InstallAddon { source, id, version, accept_unmeasured } => {
            place(layout, &by, &source, &id, &version, accept_unmeasured, false, sources, trust, measure, progress)
        }
        Action::UpdateAddon { source, id, version, accept_unmeasured } => {
            place(layout, &by, &source, &id, &version, accept_unmeasured, true, sources, trust, measure, progress)
        }
        Action::RollBackAddon { id, version } => {
            let mut reg = Register::load(layout)?;
            let i = reg.apps.get_mut(&id).ok_or_else(|| Error::NotFound(format!("{id} is not installed")))?;
            if i.previous.as_deref() != Some(version.as_str()) {
                return Err(Error::Invalid(format!("{id} has no earlier version {version} kept to go back to")));
            }
            if !layout.bundle_dir(&id, &version).join(MANIFEST).is_file() {
                return Err(Error::Io(format!("{id} {version} is in the register but its files are missing")));
            }
            let now = std::mem::replace(&mut i.version, version.clone());
            i.previous = Some(now);
            reg.save(layout)?;
            Ok(Outcome::RolledBack { id, version })
        }
        Action::RemoveAddon { id, keep_data } => {
            let mut reg = Register::load(layout)?;
            if !reg.apps.contains_key(&id) {
                return Err(Error::NotFound(format!("{id} is not installed")));
            }
            let others = installed_manifests(layout, &reg, Some(&id))?;
            let needed_by: Vec<_> = others.iter().filter(|m| m.depends.contains(&id)).map(|m| m.id.clone()).collect();
            if !needed_by.is_empty() {
                return Err(Error::Invalid(format!("{id} is needed by {}; remove those first", needed_by.join(", "))));
            }
            // out of the register first: a crash part-way leaves files but
            // never a registered add-on with half its files gone
            reg.apps.remove(&id);
            reg.save(layout)?;
            remove_tree(&layout.programs.join(&id))?;
            if !keep_data {
                remove_tree(&layout.data.join(&id))?;
            }
            Ok(Outcome::Removed { id, data_kept: keep_data })
        }
        Action::SetAddonRunning { id, on } => {
            let mut reg = Register::load(layout)?;
            let i = reg.apps.get_mut(&id).ok_or_else(|| Error::NotFound(format!("{id} is not installed")))?;
            i.wanted_running = on;
            reg.save(layout)?;
            Ok(Outcome::RunningSet { id, wanted_running: on })
        }
    }
}

fn remove_tree(p: &Path) -> Result<()> {
    match fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// A staging folder removed however the install ends.
struct Staged(PathBuf);
impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn random_name() -> String {
    let mut b = [0u8; 12];
    getrandom::getrandom(&mut b).expect("the operating system's random source");
    hex(&b)
}

#[allow(clippy::too_many_arguments)]
fn place(
    layout: &Layout,
    by: &str,
    source: &str,
    id: &str,
    version: &str,
    accept_unmeasured: bool,
    update: bool,
    sources: &dyn Sources,
    trust: &Trust,
    measure: &dyn Measure,
    progress: &mut dyn FnMut(&str),
) -> Result<Outcome> {
    let reg = Register::load(layout)?;
    let current = reg.apps.get(id).cloned();
    match (&current, update) {
        (Some(c), false) => return Err(Error::Invalid(format!("{id} {} is already installed; update it instead", c.version))),
        (None, true) => return Err(Error::NotFound(format!("{id} is not installed, so it cannot be updated"))),
        (Some(c), true) if parse_version(version) <= parse_version(&c.version) => {
            return Err(Error::Invalid(format!("{id} {version} is not newer than the installed {}", c.version)))
        }
        _ => {}
    }

    progress("fetching");
    let (bundle, sig) = sources.resolve(source)?;
    let size = fs::metadata(&bundle)?.len();
    let free = measure.free_storage_bytes()?;
    // the copy, then the unpacked files: at least twice the bundle
    if size.saturating_mul(2) > free {
        return Err(Error::NoStorage(format!("the bundle is {size} bytes and unpacking it needs twice that; {free} bytes are free")));
    }
    fs::create_dir_all(layout.staging())?;
    let work = Staged(layout.staging().join(random_name()));
    fs::create_dir(&work.0)?;
    let copy = work.0.join("bundle");
    let copy_sig = work.0.join("bundle.minisig");
    fs::copy(&bundle, &copy)?;
    match fs::symlink_metadata(&sig) {
        Ok(m) if m.is_file() && m.len() <= 4096 => {
            fs::copy(&sig, &copy_sig)?;
        }
        _ => return Err(Error::Signature(format!("{source} has no signature beside it ({source}{SIG_SUFFIX})"))),
    }

    progress("verifying");
    trust.verify(&copy, &copy_sig)?;

    progress("installing");
    let tree = work.0.join("tree");
    fs::create_dir(&tree)?;
    unpack(&copy, &tree)?;
    fs::remove_file(&copy)?;
    let m = read_manifest(&tree)?;
    if m.kind != Kind::Addon {
        return Err(Error::Invalid(format!("{} is a base service, which comes with the system image", m.id)));
    }
    if m.id != id || m.version != version {
        return Err(Error::Invalid(format!("the bundle holds {} {}, not the {id} {version} that was chosen", m.id, m.version)));
    }
    for need in [LICENCE.to_string(), m.run.entry.clone()]
        .into_iter()
        .chain(m.launcher.as_ref().map(|l| l.icon.clone()))
    {
        if m.run.form == deltos_apps::manifest::RunForm::Systemd && need == m.run.entry {
            continue; // a unit file is named, not carried
        }
        let rel = Path::new(&need);
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) || !tree.join(rel).is_file() {
            return Err(Error::Invalid(format!("the bundle is missing {need}")));
        }
    }

    let cap = measure.capability()?;
    let installed = installed_manifests(layout, &reg, if update { Some(id) } else { None })?;
    let f = fit::check(&cap, &installed, std::slice::from_ref(&m), measure.free_storage_bytes()?);
    let verdict = match f.verdict {
        Verdict::Fits => "fits",
        Verdict::FitsProvisionally => "fits_estimated",
        Verdict::CannotTell => "cannot_tell",
        Verdict::DoesNotFit => {
            let why = serde_json::to_string(&f.blockers).unwrap_or_default();
            return Err(Error::DoesNotFit(format!("{id} {version} does not fit this box: {why}")));
        }
    };
    if f.verdict == Verdict::CannotTell && !accept_unmeasured {
        return Err(Error::Unmeasured(format!(
            "nobody has measured enough to tell whether {id} fits this box (RAM {}, storage {}); accept that with your password to install it anyway",
            sum_text(&f.ram),
            sum_text(&f.storage)
        )));
    }

    // place: rename into programs/<id>/<version>, then register
    let dest = layout.bundle_dir(id, version);
    fs::create_dir_all(dest.parent().unwrap())?;
    remove_tree(&dest)?; // a leftover from an install that crashed before registering
    fs::rename(&tree, &dest)?;
    let data = layout.data.join(id);
    if !data.exists() {
        fs::create_dir_all(&data)?;
        set_mode(&data, 0o750)?;
    }
    let mut reg = Register::load(layout)?;
    let (outcome, drop_version) = match current {
        None => {
            reg.apps.insert(
                id.into(),
                Installed { version: version.into(), previous: None, wanted_running: true, installed_by: by.into() },
            );
            (Outcome::Installed { id: id.into(), version: version.into(), verdict: verdict.into() }, None)
        }
        Some(c) => {
            let old_prev = c.previous.clone();
            reg.apps.insert(
                id.into(),
                Installed { version: version.into(), previous: Some(c.version.clone()), wanted_running: c.wanted_running, installed_by: by.into() },
            );
            (
                Outcome::Updated { id: id.into(), version: version.into(), previous: c.version, verdict: verdict.into() },
                old_prev,
            )
        }
    };
    reg.save(layout)?;
    // keep the current and the previous version, no more (G1 keeps two)
    if let Some(v) = drop_version {
        if v != version {
            remove_tree(&layout.bundle_dir(id, &v))?;
        }
    }
    progress("installed");
    Ok(outcome)
}

fn sum_text(s: &fit::Sum) -> String {
    match s {
        fit::Sum::Known { needed, have, .. } => format!("{needed} of {have} bytes"),
        fit::Sum::Unknown { unmeasured, .. } => format!("not measured: {}", unmeasured.join(", ")),
    }
}

fn set_mode(p: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Unpack a verified bundle: plain files and folders only, every path inside
/// `into`, every file `0644` and folder `0755` whatever the archive says (no
/// set-user-id, no links, no device files), at most [`MAX_ENTRIES`].
fn unpack(bundle: &Path, into: &Path) -> Result<()> {
    let mut ar = tar::Archive::new(fs::File::open(bundle)?);
    let mut n = 0usize;
    for e in ar.entries().map_err(|e| Error::Invalid(format!("the bundle is not a tar file: {e}")))? {
        let mut e = e.map_err(|e| Error::Invalid(format!("the bundle is damaged: {e}")))?;
        n += 1;
        if n > MAX_ENTRIES {
            return Err(Error::Invalid(format!("the bundle holds more than {MAX_ENTRIES} entries")));
        }
        let path = e.path().map_err(|e| Error::Invalid(e.to_string()))?.into_owned();
        let mut rel = PathBuf::new();
        for c in path.components() {
            match c {
                Component::Normal(x) => rel.push(x),
                Component::CurDir => {}
                _ => return Err(Error::Invalid(format!("the bundle names a path outside itself: {}", path.display()))),
            }
        }
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = into.join(&rel);
        match e.header().entry_type() {
            tar::EntryType::Directory => {
                fs::create_dir_all(&out)?;
                set_mode(&out, 0o755)?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if let Some(p) = out.parent() {
                    fs::create_dir_all(p)?;
                }
                if fs::symlink_metadata(&out).is_ok() {
                    return Err(Error::Invalid(format!("the bundle holds {} twice", rel.display())));
                }
                let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&out)?;
                std::io::copy(&mut e, &mut f)?;
                f.sync_all()?;
                set_mode(&out, 0o644)?;
            }
            t => return Err(Error::Invalid(format!("the bundle holds {} of a kind not allowed ({t:?}); only files and folders", rel.display()))),
        }
    }
    Ok(())
}
