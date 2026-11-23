//! Installing, updating, rolling back and removing packs: the pack half of
//! `deltos-installd` (Track F §4), run inside `deltos-shelld` behind its
//! checks (`docs/shelld.md` §7).
//!
//! The same rule as the other families: **nothing here changes the box
//! except through [`perform`], which takes only an [`Authorized`], which only
//! [`authorize`] makes**, after a live secure session, `content.install`, and
//! for `remove_pack` a single-use ticket bound to this session, action and
//! argument digest.
//!
//! **Shelld never parses a pack.** A pack's index segments are SQLite
//! databases from outside the box, and parsing them is risky work on
//! untrusted input, which privilege boundary rule 18 gives to a helper: an
//! unprivileged user with no network that does one job on one input. So the
//! staged copy is handed to `deltos-pack-check` on its standard input, and
//! the helper answers where the segments lie and what the manifest says.
//! Shelld then reads the 128-byte header itself, hashes the header and those
//! byte ranges itself (the SPEC §8.1 digest), and checks the publisher's
//! signature over that digest itself. A helper that lied about the ranges
//! gets a digest the signature does not cover, and the pack is refused.
//!
//! The pipeline (contract §7.2's states): `fetching` (the pack and its
//! signature are copied into root's staging folder before anything reads
//! them), `verifying` (header, helper, digest, signature, the older-edition
//! check of Track A §6), `installing` (the fit check of contract §2.3, then a
//! rename into `/var/lib/deltos/packs/`), `installed`. `deltd` notices the
//! new file on its next scan, gives it a hostname, and serves it.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use deltos_apps::fit::{BoxArch, Capability};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wax_core::{RollbackPlan, WaxHeader};

pub use deltos_net::actions::{Caller, IdentityCheck, Refusal};

/// Installing, updating, rolling back and removing packs (contract §4.2,
/// privilege boundary §8).
pub const PERMISSION: &str = "content.install";
/// A pack file (SPEC §1).
pub const PACK_SUFFIX: &str = ".wax";
/// Its detached signature, beside it (SPEC §8.2).
pub const SIG_SUFFIX: &str = ".minisig";
/// The signature of the edition before the last update, kept so
/// `roll_back_pack` can check what it goes back to (`docs/shelld.md` §7.3).
pub const PREVIOUS_SIG_SUFFIX: &str = ".minisig.previous";
/// `pack_index` is 0–511 (contract §8): at most this many packs at once.
pub const MAX_PACKS: usize = 512;
/// SPEC §5.1's chain guard: no pack has more segments than this.
pub const MAX_SEGMENTS: usize = 64;
/// A minisign signature is a few hundred bytes.
const MAX_SIG_BYTES: u64 = 4096;
/// The helper's answer is a list of ranges and a manifest: small.
const MAX_REPORT_BYTES: u64 = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Signature(String),
    /// Track A §6: an older edition than this box has already had.
    #[error("{0}")]
    Older(String),
    #[error("{0}")]
    Edition(String),
    #[error("{0}")]
    Resources(String),
    #[error("{0}")]
    NoStorage(String),
    #[error("{0}")]
    Io(String),
}

impl Error {
    /// deltd API §7's codes (`edition_older` added with this module).
    pub fn code(&self) -> &'static str {
        match self {
            Error::Invalid(_) => "invalid_request",
            Error::NotFound(_) => "pack_not_found",
            Error::Signature(_) => "signature_invalid",
            Error::Older(_) => "edition_older",
            Error::Edition(_) => "edition_mismatch",
            Error::Resources(_) => "not_enough_resources",
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

/// The closed list of pack actions (privilege boundary §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `source` is an id from [`Sources::list`]; `archive_uuid` is what the
    /// admin chose, and the pack must carry the same.
    InstallPack { source: String, archive_uuid: String },
    RemovePack { archive_uuid: String },
    RollBackPack { archive_uuid: String },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::InstallPack { .. } => "install_pack",
            Action::RemovePack { .. } => "remove_pack",
            Action::RollBackPack { .. } => "roll_back_pack",
        }
    }

    pub fn permission(&self) -> &'static str {
        PERMISSION
    }

    /// Privilege boundary §8: only removing asks for the password again.
    pub fn needs_reconfirm(&self) -> bool {
        matches!(self, Action::RemovePack { .. })
    }

    /// Installs copy a whole pack, which can be gigabytes: a job.
    pub fn is_long(&self) -> bool {
        matches!(self, Action::InstallPack { .. })
    }

    pub fn canonical_args(&self) -> String {
        match self {
            Action::InstallPack { source, archive_uuid } => format!("source={source}\narchive_uuid={archive_uuid}"),
            Action::RemovePack { archive_uuid } | Action::RollBackPack { archive_uuid } => format!("archive_uuid={archive_uuid}"),
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
            Action::InstallPack { source, archive_uuid } => {
                check_source_id(source)?;
                check_uuid(archive_uuid).map(|_| ())
            }
            Action::RemovePack { archive_uuid } | Action::RollBackPack { archive_uuid } => check_uuid(archive_uuid).map(|_| ()),
        }
    }
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Contract §11's canonical text form of `archive_uuid`: lowercase,
/// hyphenated 8-4-4-4-12. Only that form is accepted in a request, so one
/// pack always has one spelling in the log and in a ticket's digest.
pub fn check_uuid(s: &str) -> Result<[u8; 16]> {
    let bad = || Error::Invalid(format!("{s:?} is not an archive_uuid (lowercase 8-4-4-4-12)"));
    let b = s.as_bytes();
    if b.len() != 36 {
        return Err(bad());
    }
    let mut out = [0u8; 16];
    let mut n = 0;
    let mut i = 0;
    while i < 36 {
        if [8, 13, 18, 23].contains(&i) {
            if b[i] != b'-' {
                return Err(bad());
            }
            i += 1;
            continue;
        }
        let d = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let (Some(h), Some(l)) = (d(b[i]), d(b.get(i + 1).copied().unwrap_or(b'x'))) else { return Err(bad()) };
        out[n] = h << 4 | l;
        n += 1;
        i += 2;
    }
    Ok(out)
}

pub fn uuid_text(b: &[u8; 16]) -> String {
    let h = hex(b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// `<origin>/<file>.wax`: which listing it came from, and a plain file name.
pub fn check_source_id(s: &str) -> Result<()> {
    let bad = || Error::Invalid(format!("{s:?} is not a source id from the pack listing"));
    let (origin, file) = s.split_once('/').ok_or_else(bad)?;
    let plain = |x: &str| !x.is_empty() && !x.starts_with('.') && x.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if s.len() > 200 || !plain(origin) || !plain(file) || !file.ends_with(PACK_SUFFIX) || file.len() <= PACK_SUFFIX.len() {
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

/// shelld's checks. The ticket is redeemed only after every other check
/// passed, so a refused request never spends it.
pub fn authorize(action: Action, caller: &Caller, id: &dyn IdentityCheck) -> std::result::Result<Authorized, Refusal> {
    let (profile, secure) = id.session(caller.session_ref).ok_or(Refusal::NotSignedIn)?;
    if !caller.over_https || !secure {
        return Err(Refusal::SecureConnectionRequired);
    }
    if !id.has_permission(&profile, PERMISSION) {
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

/// Where packs live (contract §10).
#[derive(Debug, Clone)]
pub struct Layout {
    /// `/var/lib/deltos/packs`: installed packs and their signatures; what
    /// `deltd` serves.
    pub packs: PathBuf,
    /// `/var/lib/deltos/packs-data`: each pack's own writable storage.
    pub packs_data: PathBuf,
    /// `/var/lib/deltos/shelld/packs.json`: what shelld installed, and the
    /// newest edition of every pack this box has had (`docs/shelld.md` §7.4).
    pub register: PathBuf,
    /// `/var/lib/deltos/capability.json` (contract §9).
    pub capability: PathBuf,
}

impl Layout {
    pub fn under(root: &Path) -> Layout {
        Layout {
            packs: root.join("var/lib/deltos/packs"),
            packs_data: root.join("var/lib/deltos/packs-data"),
            register: root.join("var/lib/deltos/shelld/packs.json"),
            capability: root.join("var/lib/deltos/capability.json"),
        }
    }
    /// On the packs' own filesystem, so placing a pack is a rename. `deltd`
    /// serves only `*.wax` files at the top, so it never sees in here.
    fn staging(&self) -> PathBuf {
        self.packs.join(".staging")
    }
}

/// One edition, as the register remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edition {
    pub version: String,
    /// The header's `created_at` (SPEC §2), which every append raises.
    pub created_at: u64,
}

/// One pack shelld installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// Its file name in the packs folder.
    pub file: String,
    pub name: String,
    pub edition: Edition,
    /// The edition before the last update, while its signature is kept.
    pub previous: Option<Edition>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub installed_by: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Register {
    pub schema: u32,
    pub packs: BTreeMap<String, Installed>,
    /// Track A §6: the newest edition of each pack this box has had,
    /// kept after a removal, so an older one cannot come back as new.
    pub newest_seen: BTreeMap<String, Edition>,
}

impl Register {
    pub fn load(layout: &Layout) -> Result<Register> {
        match fs::read(&layout.register) {
            Ok(b) => {
                let r: Register = serde_json::from_slice(&b).map_err(|e| Error::Io(format!("{}: {e}", layout.register.display())))?;
                if r.schema != 1 {
                    return Err(Error::Io(format!("packs.json schema {} is newer than this shelld", r.schema)));
                }
                Ok(r)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Register { schema: 1, ..Default::default() }),
            Err(e) => Err(e.into()),
        }
    }

    /// Written whole, synced, renamed into place.
    fn save(&self, layout: &Layout) -> Result<()> {
        let dir = layout.register.parent().ok_or_else(|| Error::Io("register path has no folder".into()))?;
        fs::create_dir_all(dir)?;
        let tmp = dir.join(".packs.json.tmp");
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(self).map_err(|e| Error::Io(e.to_string()))?)?;
        f.sync_all()?;
        fs::rename(&tmp, &layout.register)?;
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    }
}

/// CalVer `YYYY.MM.N` (contract §11), as three numbers for ordering.
pub fn calver(v: &str) -> Option<(u32, u32, u32)> {
    wax_builder::config::check_calver(v).ok()?;
    let mut p = v.split('.').map(|x| x.parse::<u32>().ok());
    Some((p.next()??, p.next()??, p.next()??))
}

/// Track A §6: is `new` older than `seen` in either measure? Both the
/// manifest's version and the header's `created_at` must not go backwards.
fn is_older(new: &Edition, seen: &Edition) -> bool {
    calver(&new.version) < calver(&seen.version) || new.created_at < seen.created_at
}

fn newer_of(a: &Edition, b: &Edition) -> Edition {
    Edition {
        version: if calver(&a.version) >= calver(&b.version) { a.version.clone() } else { b.version.clone() },
        created_at: a.created_at.max(b.created_at),
    }
}

/// One pack an admin may install, from a listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Offered {
    pub source: String,
    pub bytes: u64,
    pub signed: bool,
}

/// Where packs can come from: each origin is a folder (a mounted drive's top
/// folder; later the catalog's download folder, a peer, an upload).
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

    fn resolve(&self, source: &str) -> Result<(PathBuf, PathBuf)> {
        check_source_id(source)?;
        let (origin, file) = source.split_once('/').unwrap();
        let dir = self
            .origins()
            .into_iter()
            .find(|(o, _)| o == origin)
            .map(|(_, d)| d)
            .ok_or_else(|| Error::NotFound(format!("no pack source {origin:?}; list the sources again")))?;
        let p = dir.join(file);
        match fs::symlink_metadata(&p) {
            Ok(m) if m.is_file() => Ok((p, dir.join(format!("{file}{SIG_SUFFIX}")))),
            _ => Err(Error::NotFound(format!("{source} is not there any more; list the sources again"))),
        }
    }
}

/// Folders, each under its own name.
pub struct Folders(pub Vec<(String, PathBuf)>);
impl Sources for Folders {
    fn origins(&self) -> Vec<(String, PathBuf)> {
        self.0.clone()
    }
}

/// The key packs must be signed with (Track F §8, "the project key").
/// `None` when the box has none: every install is then refused.
pub struct Trust {
    key: Option<minisign_verify::PublicKey>,
}

impl Trust {
    pub fn none() -> Trust {
        Trust { key: None }
    }
    /// A minisign `.pub` file (two lines, or the key alone).
    pub fn from_file(p: &Path) -> Result<Trust> {
        let text = fs::read_to_string(p).map_err(|e| Error::Io(format!("{}: {e}", p.display())))?;
        let key = minisign_verify::PublicKey::decode(&text)
            .or_else(|_| minisign_verify::PublicKey::from_base64(text.trim()))
            .map_err(|e| Error::Invalid(format!("{}: {e}", p.display())))?;
        Ok(Trust { key: Some(key) })
    }

    /// SPEC §8.2/§8.3: a prehashed minisign signature over the 32-byte
    /// digest, by the pack key, whose trusted comment (when it names one)
    /// names this archive.
    fn verify(&self, digest: &[u8; 32], sig: &Path, uuid: &[u8; 16]) -> Result<()> {
        let key = self.key.as_ref().ok_or_else(|| Error::Signature("this box has no pack signing key set, so it installs no pack".into()))?;
        let st = fs::read_to_string(sig).map_err(|_| Error::Signature("the pack has no signature beside it".into()))?;
        let s = minisign_verify::Signature::decode(&st).map_err(|e| Error::Signature(format!("the signature cannot be read: {e}")))?;
        // false: a legacy (not prehashed) signature is refused (SPEC §8.2, -H)
        key.verify(digest, &s, false).map_err(|_| Error::Signature("the pack is not signed by the pack key, or was changed after signing".into()))?;
        if let Some(tok) = s.trusted_comment().split_whitespace().find_map(|t| t.strip_prefix("archive_uuid=")) {
            let named = parse_any_uuid(tok).ok_or_else(|| Error::Signature(format!("the signature names {tok:?}, which is not a uuid")))?;
            if &named != uuid {
                return Err(Error::Signature(format!("the signature is for archive {tok}, not {}", uuid_text(uuid))));
            }
        }
        Ok(())
    }
}

/// SPEC §8.2: compare as values; a sidecar may carry the bare 32-hex form.
fn parse_any_uuid(s: &str) -> Option<[u8; 16]> {
    let h: String = s.chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase();
    if h.len() != 32 {
        return None;
    }
    check_uuid(&format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])).ok()
}

/// What the helper reports about a pack (`deltos-pack-check check`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Every index segment's `(offset, length)`, oldest first.
    pub segments: Vec<(u64, u64)>,
    /// The pack's merged manifest (SPEC §5.6).
    pub manifest: BTreeMap<String, String>,
}

/// What the helper reports for a rollback (`deltos-pack-check rollback-plan`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanReport {
    pub prev_header: String,
    pub prev_end: u64,
    pub prev_ranges: Vec<(u64, u64)>,
}

/// How shelld asks the helper. `Helper` runs the real program.
pub trait Checker {
    /// Let the helper read the staging folder `dir` (and nothing else of
    /// root's): the folder becomes `0750 root:<helper's group>`.
    fn admit(&self, _dir: &Path) -> Result<()> {
        Ok(())
    }
    /// Run the helper's `mode` on `pack`; its JSON answer.
    fn run(&self, mode: &str, pack: &Path) -> Result<Vec<u8>>;
}

/// `deltos-pack-check`, run as its own unprivileged user. In the box's
/// unit shelld has no network, so neither has the helper it starts.
pub struct Helper {
    pub program: PathBuf,
    /// `(uid, gid)` to run as; `None` only in tests, which are not root.
    pub user: Option<(u32, u32)>,
}

impl Checker for Helper {
    fn admit(&self, dir: &Path) -> Result<()> {
        if let Some((_, gid)) = self.user {
            std::os::unix::fs::chown(dir, Some(0), Some(gid))?;
            set_mode(dir, 0o750)?;
        }
        Ok(())
    }

    fn run(&self, mode: &str, pack: &Path) -> Result<Vec<u8>> {
        use std::os::unix::process::CommandExt;
        // The helper is named the one file. Staging is `0711` (it can pass
        // through, not list) and only the install's own folder lets its
        // group in, so the pack is all of root's it can read.
        let mut cmd = Command::new(&self.program);
        cmd.arg(mode).arg(pack).env_clear().current_dir("/").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some((uid, gid)) = self.user {
            // The standard library drops root's supplementary groups before
            // it changes the uid and gid. Checked here, after the change and
            // before the helper starts: if any group but its own is left,
            // the helper does not run.
            cmd.uid(uid).gid(gid);
            // SAFETY: getgroups is async-signal-safe and writes into `g`.
            unsafe {
                cmd.pre_exec(move || {
                    let mut g = [0 as libc::gid_t; 64];
                    let n = libc::getgroups(g.len() as libc::c_int, g.as_mut_ptr());
                    if n < 0 || g[..n as usize].iter().any(|x| *x != gid) || libc::getuid() != uid || libc::getgid() != gid {
                        return Err(std::io::Error::from_raw_os_error(libc::EPERM));
                    }
                    Ok(())
                });
            }
        }
        let mut child = cmd.spawn().map_err(|e| Error::Io(format!("the pack checker cannot start ({}): {e}", self.program.display())))?;
        let mut out = Vec::new();
        child.stdout.take().unwrap().take(MAX_REPORT_BYTES + 1).read_to_end(&mut out)?;
        let mut err = Vec::new();
        child.stderr.take().unwrap().take(4096).read_to_end(&mut err)?;
        let status = child.wait()?;
        if out.len() as u64 > MAX_REPORT_BYTES {
            return Err(Error::Invalid("the pack checker's answer is too long".into()));
        }
        if !status.success() {
            let why = String::from_utf8_lossy(&err).trim().to_string();
            return Err(Error::Invalid(format!("the pack cannot be read: {why}")));
        }
        Ok(out)
    }
}

/// What the box measured about itself, and the free space now.
pub trait Measure {
    fn capability(&self) -> Result<Capability>;
    fn free_storage_bytes(&self) -> Result<u64>;
}

/// The box's own record and `statvfs` on the packs folder.
pub struct Measured<'a>(pub &'a Layout);
impl Measure for Measured<'_> {
    fn capability(&self) -> Result<Capability> {
        let p = &self.0.capability;
        let t = fs::read_to_string(p).map_err(|e| Error::Resources(format!("the box has not measured itself yet ({}: {e})", p.display())))?;
        serde_json::from_str(&t).map_err(|e| Error::Io(format!("{}: {e}", p.display())))
    }
    fn free_storage_bytes(&self) -> Result<u64> {
        let mut dir = self.0.packs.clone();
        while !dir.exists() {
            if !dir.pop() {
                break;
            }
        }
        crate::addons::free_bytes(&dir).map_err(|e| Error::Io(e.to_string()))
    }
}

/// What an action did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "done", rename_all = "snake_case")]
pub enum Outcome {
    Installed { archive_uuid: String, name: String, version: String },
    Updated { archive_uuid: String, name: String, version: String, previous: String, can_roll_back: bool },
    RolledBack { archive_uuid: String, version: String },
    Removed { archive_uuid: String },
}

/// Everything `perform` needs from the box.
pub struct Env<'a> {
    pub layout: &'a Layout,
    pub sources: &'a dyn Sources,
    pub trust: &'a Trust,
    pub checker: &'a dyn Checker,
    pub measure: &'a dyn Measure,
    /// Who owns each pack's writable storage (`deltd`'s user); `None` in tests.
    pub data_owner: Option<(u32, u32)>,
}

/// Perform an authorized action. `progress` hears contract §7.2's states.
pub fn perform(env: &Env, a: Authorized, progress: &mut dyn FnMut(&str)) -> Result<Outcome> {
    let by = a.by;
    match a.action {
        Action::InstallPack { source, archive_uuid } => install(env, &by, &source, &archive_uuid, progress),
        Action::RemovePack { archive_uuid } => remove(env, &archive_uuid),
        Action::RollBackPack { archive_uuid } => roll_back(env, &archive_uuid),
    }
}

/// One installed pack file, found by reading headers (shelld reads only the
/// 128 bytes; packs baked into the system image are found the same way).
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub archive_uuid: String,
    pub file: String,
    pub bytes: u64,
}

/// Every `*.wax` file in the packs folder, by its header's uuid.
pub fn installed_files(layout: &Layout) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    let rd = match fs::read_dir(&layout.packs) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e.into()),
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.ends_with(PACK_SUFFIX) {
            continue;
        }
        let Ok(md) = fs::symlink_metadata(e.path()) else { continue };
        if !md.is_file() {
            continue;
        }
        if let Ok((h, _)) = read_header(&e.path()) {
            out.push(Found { archive_uuid: uuid_text(&h.archive_uuid), file: name, bytes: md.len() });
        }
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(out)
}

fn read_header(p: &Path) -> Result<(WaxHeader, [u8; 128])> {
    let mut f = fs::File::open(p)?;
    let mut raw = [0u8; 128];
    f.read_exact(&mut raw).map_err(|_| Error::Invalid("the file is too short to be a pack".into()))?;
    let h = WaxHeader::parse(&raw).map_err(|e| Error::Invalid(format!("not a pack: {e}")))?;
    Ok((h, raw))
}

/// The SPEC §8.1 digest of `header ‖ ranges`, read from `file` by shelld
/// itself. The ranges must be in order, apart, inside the file, after the
/// header, and the last one must be where `header` points.
fn digest_of(file: &Path, header_raw: &[u8; 128], header: &WaxHeader, ranges: &[(u64, u64)]) -> Result<[u8; 32]> {
    let mut f = fs::File::open(file)?;
    let size = f.metadata()?.len();
    if ranges.is_empty() || ranges.len() > MAX_SEGMENTS {
        return Err(Error::Invalid(format!("the pack lists {} segments", ranges.len())));
    }
    if ranges.last() != Some(&(header.index_offset, header.index_length)) {
        return Err(Error::Invalid("the newest segment is not where the header points".into()));
    }
    let mut floor = 128u64;
    for &(o, l) in ranges {
        let end = o.checked_add(l).ok_or_else(|| Error::Invalid("a segment runs past the end".into()))?;
        if o < floor || end > size || l == 0 {
            return Err(Error::Invalid("the segments overlap or lie outside the file".into()));
        }
        floor = end;
    }
    let mut h = Sha256::new();
    h.update(header_raw);
    let mut buf = vec![0u8; 1 << 16];
    for &(o, l) in ranges {
        f.seek(SeekFrom::Start(o))?;
        let mut left = l;
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            f.read_exact(&mut buf[..n])?;
            h.update(&buf[..n]);
            left -= n as u64;
        }
    }
    Ok(h.finalize().into())
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

fn set_mode(p: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn sync_dir(p: &Path) {
    if let Ok(d) = fs::File::open(p) {
        let _ = d.sync_all();
    }
}

/// Contract §2.3 and §11: what a pack needs, read from its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needs {
    pub min_ram_bytes: u64,
    pub min_storage_bytes: u64,
    pub arch: String,
    pub gpu_required: bool,
    pub runtime_storage_bytes: u64,
}

fn num(m: &BTreeMap<String, String>, k: &str) -> Result<Option<u64>> {
    match m.get(k) {
        None => Ok(None),
        Some(v) => v.trim().parse::<u64>().map(Some).map_err(|_| Error::Invalid(format!("the manifest's {k} is not a number: {v:?}"))),
    }
}

/// Read the requirement, mapping a retired `min_hw_tier` to its historical
/// floor (contract §11; never guessing one it cannot map).
pub fn needs(m: &BTreeMap<String, String>) -> Result<Needs> {
    let (ram, storage) = match (num(m, "min_ram_bytes")?, num(m, "min_storage_bytes")?) {
        (Some(r), Some(s)) => (r, s),
        (None, None) => match m.get("min_hw_tier") {
            Some(t) => {
                let (r, s) = wax_builder::config::legacy_tier_floor(t)
                    .ok_or_else(|| Error::Invalid(format!("the pack's retired min_hw_tier {t:?} cannot be mapped to a floor (contract §11)")))?;
                (r as u64, s as u64)
            }
            None => return Err(Error::Invalid("the manifest declares no min_ram_bytes or min_storage_bytes (contract §11)".into())),
        },
        _ => return Err(Error::Invalid("the manifest declares only one of min_ram_bytes and min_storage_bytes".into())),
    };
    let arch = match m.get("arch").map(String::as_str) {
        Some(a @ ("any" | "aarch64" | "x86_64")) => a.to_string(),
        None if m.contains_key("min_hw_tier") => "any".into(),
        Some(a) => return Err(Error::Invalid(format!("the manifest's arch {a:?} is not aarch64, x86_64 or any"))),
        None => return Err(Error::Invalid("the manifest declares no arch (contract §11)".into())),
    };
    let gpu_required = match m.get("gpu").map(String::as_str) {
        None | Some("preferred") => false,
        Some("required") => true,
        Some(g) => return Err(Error::Invalid(format!("the manifest's gpu {g:?} is not required or preferred"))),
    };
    Ok(Needs { min_ram_bytes: ram, min_storage_bytes: storage, arch, gpu_required, runtime_storage_bytes: num(m, "runtime_storage_bytes")?.unwrap_or(0) })
}

/// Contract §2.3's comparison rule: every declared field met by the
/// capability record. Storage is what is free now (the staged copy already
/// counted), against the larger of the two "beyond the archive" floors
/// (`docs/shelld.md` §7.2 says why the larger, not the sum).
pub fn fits(n: &Needs, cap: &Capability, free_after_copy: u64) -> std::result::Result<(), Vec<String>> {
    let mut why = Vec::new();
    if n.min_ram_bytes > cap.ram_bytes {
        why.push(format!("it needs {} bytes of memory; this box has {}", n.min_ram_bytes, cap.ram_bytes));
    }
    let box_arch = match cap.arch {
        BoxArch::Aarch64 => "aarch64",
        BoxArch::X86_64 => "x86_64",
    };
    if n.arch != "any" && n.arch != box_arch {
        why.push(format!("it is built for {}; this box is {box_arch}", n.arch));
    }
    if n.gpu_required && cap.gpu == "none" {
        why.push("it needs a graphics processor; this box has none".into());
    }
    let room = n.min_storage_bytes.max(n.runtime_storage_bytes);
    if room > free_after_copy {
        why.push(format!("it needs {room} bytes of storage beyond the pack; {free_after_copy} are free"));
    }
    if why.is_empty() {
        Ok(())
    } else {
        Err(why)
    }
}

fn install(env: &Env, by: &str, source: &str, want: &str, progress: &mut dyn FnMut(&str)) -> Result<Outcome> {
    let layout = env.layout;
    let want_raw = check_uuid(want)?;

    progress("fetching");
    let (pack, sig) = env.sources.resolve(source)?;
    let size = fs::metadata(&pack)?.len();
    let free = env.measure.free_storage_bytes()?;
    if size > free {
        return Err(Error::NoStorage(format!("the pack is {size} bytes; {free} bytes are free")));
    }
    let staging = layout.staging();
    fs::create_dir_all(&staging)?;
    // through, not readable: an install's folder is reached only by its random name
    set_mode(&staging, 0o711)?;
    let work = Staged(staging.join(random_name()));
    fs::create_dir(&work.0)?;
    set_mode(&work.0, 0o700)?;
    env.checker.admit(&work.0)?;
    let copy = work.0.join("pack.wax");
    let copy_sig = work.0.join("pack.wax.minisig");
    match fs::symlink_metadata(&sig) {
        Ok(m) if m.is_file() && m.len() <= MAX_SIG_BYTES => {
            fs::copy(&sig, &copy_sig)?;
        }
        _ => return Err(Error::Signature(format!("{source} has no signature beside it ({source}{SIG_SUFFIX})"))),
    }
    fs::copy(&pack, &copy)?;
    // readable by deltd once placed; until then only inside the install's own folder
    set_mode(&copy, 0o644)?;
    set_mode(&copy_sig, 0o644)?;

    progress("verifying");
    let (h, raw) = read_header(&copy)?;
    if h.archive_uuid != want_raw {
        return Err(Error::Invalid(format!("the file holds pack {}, not the {want} that was chosen", uuid_text(&h.archive_uuid))));
    }
    h.validate(fs::metadata(&copy)?.len()).map_err(|e| Error::Invalid(format!("not a pack: {e}")))?;
    let out = env.checker.run("check", &copy)?;
    let rep: Report = serde_json::from_slice(&out).map_err(|e| Error::Invalid(format!("the pack checker's answer cannot be read: {e}")))?;
    let digest = digest_of(&copy, &raw, &h, &rep.segments)?;
    env.trust.verify(&digest, &copy_sig, &h.archive_uuid)?;

    let m = &rep.manifest;
    let version = m.get("version").cloned().unwrap_or_default();
    if calver(&version).is_none() {
        return Err(Error::Invalid(format!("the pack's version {version:?} is not CalVer YYYY.MM.N (contract §11)")));
    }
    let name = m.get("name").cloned().unwrap_or_default();
    let edition = Edition { version: version.clone(), created_at: h.created_at };
    let mut reg = Register::load(layout)?;
    if let Some(seen) = reg.newest_seen.get(want) {
        if is_older(&edition, seen) {
            return Err(Error::Older(format!(
                "this box has already had edition {} (written at {}) of this pack; {} (written at {}) is older, so it is refused as a rollback",
                seen.version, seen.created_at, edition.version, edition.created_at
            )));
        }
    }
    let files = installed_files(layout)?;
    let current = files.iter().find(|f| f.archive_uuid == want).cloned();
    if let Some(c) = &current {
        let (ch, _) = read_header(&layout.packs.join(&c.file))?;
        let cur = reg.packs.get(want).map(|i| i.edition.clone());
        let same = ch.created_at == h.created_at && cur.as_ref().is_none_or(|e| e.version == version);
        if same {
            return Err(Error::Invalid(format!("{name} {version} is already installed")));
        }
        if let Some(cur) = &cur {
            if !(calver(&version) > calver(&cur.version) || (calver(&version) == calver(&cur.version) && h.created_at > cur.created_at)) {
                return Err(Error::Older(format!("{name} {version} is not newer than the installed {}", cur.version)));
            }
        } else if h.created_at <= ch.created_at {
            return Err(Error::Older(format!("{name} {version} is not newer than the edition installed")));
        }
    } else if files.len() >= MAX_PACKS {
        return Err(Error::Resources(format!("this box already has {MAX_PACKS} packs, the most it can serve (contract §8)")));
    }
    let deps: Vec<String> = m
        .get("depends_on")
        .map(|d| d.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

    progress("installing");
    let n = needs(m)?;
    let cap = env.measure.capability()?;
    let mut why = match fits(&n, &cap, env.measure.free_storage_bytes()?) {
        Ok(()) => Vec::new(),
        Err(w) => w,
    };
    for d in &deps {
        let id = parse_any_uuid(d).map(|u| uuid_text(&u)).unwrap_or_else(|| d.clone());
        if !files.iter().any(|f| f.archive_uuid == id) {
            why.push(format!("it needs pack {id} installed first"));
        }
    }
    if !why.is_empty() {
        let lacks_storage = why.iter().all(|w| w.contains("storage"));
        let msg = format!("{name} {version} does not fit this box: {}", why.join("; "));
        return Err(if lacks_storage { Error::NoStorage(msg) } else { Error::Resources(msg) });
    }

    // place: the new signature beside the new file, then the file, so a pack
    // is never in place without the signature it was checked with
    let file = current.as_ref().map(|c| c.file.clone()).unwrap_or_else(|| format!("{want}{PACK_SUFFIX}"));
    let dest = layout.packs.join(&file);
    let dest_sig = layout.packs.join(format!("{file}{SIG_SUFFIX}"));
    let dest_prev = layout.packs.join(format!("{file}{PREVIOUS_SIG_SUFFIX}"));
    let previous = if current.is_some() {
        // keep the old edition's signature, for roll_back_pack
        match fs::rename(&dest_sig, &dest_prev) {
            Ok(()) => reg.packs.get(want).map(|i| i.edition.clone()).or(Some(Edition { version: String::new(), created_at: 0 })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = fs::remove_file(&dest_prev);
                None
            }
            Err(e) => return Err(e.into()),
        }
    } else {
        None
    };
    fs::rename(&copy_sig, &dest_sig)?;
    fs::rename(&copy, &dest)?;
    sync_dir(&layout.packs);
    let data = layout.packs_data.join(want);
    if !data.exists() {
        fs::create_dir_all(&data)?;
        set_mode(&data, 0o750)?;
        if let Some((u, g)) = env.data_owner {
            std::os::unix::fs::chown(&data, Some(u), Some(g))?;
        }
    }

    let seen = match reg.newest_seen.get(want) {
        Some(s) => newer_of(s, &edition),
        None => edition.clone(),
    };
    reg.newest_seen.insert(want.into(), seen);
    let can_roll_back = previous.is_some();
    let prev_version = previous.as_ref().map(|p| p.version.clone());
    reg.packs.insert(
        want.into(),
        Installed { file, name: name.clone(), edition, previous, depends_on: deps, installed_by: by.into() },
    );
    reg.save(layout)?;
    progress("installed");
    Ok(match current {
        None => Outcome::Installed { archive_uuid: want.into(), name, version },
        Some(_) => Outcome::Updated { archive_uuid: want.into(), name, version, previous: prev_version.unwrap_or_default(), can_roll_back },
    })
}

fn find(layout: &Layout, uuid: &str) -> Result<Found> {
    check_uuid(uuid)?;
    installed_files(layout)?
        .into_iter()
        .find(|f| f.archive_uuid == uuid)
        .ok_or_else(|| Error::NotFound(format!("no installed pack {uuid}")))
}

fn remove(env: &Env, uuid: &str) -> Result<Outcome> {
    let layout = env.layout;
    let f = find(layout, uuid)?;
    let mut reg = Register::load(layout)?;
    let needed_by: Vec<String> = reg
        .packs
        .iter()
        .filter(|(u, i)| u.as_str() != uuid && i.depends_on.iter().any(|d| parse_any_uuid(d).map(|x| uuid_text(&x)).as_deref() == Some(uuid)))
        .map(|(_, i)| i.name.clone())
        .collect();
    if !needed_by.is_empty() {
        return Err(Error::Invalid(format!("this pack is needed by {}; remove those first", needed_by.join(", "))));
    }
    // the file first: deltd stops serving it on its next scan, and frees its
    // pack_index there (privilege boundary §9, retire_pack_hostnames)
    fs::remove_file(layout.packs.join(&f.file))?;
    for suffix in [SIG_SUFFIX, PREVIOUS_SIG_SUFFIX] {
        match fs::remove_file(layout.packs.join(format!("{}{suffix}", f.file))) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    sync_dir(&layout.packs);
    match fs::remove_dir_all(layout.packs_data.join(uuid)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    // the newest edition seen is kept: an older one cannot come back as new
    reg.packs.remove(uuid);
    reg.save(layout)?;
    Ok(Outcome::Removed { archive_uuid: uuid.into() })
}

fn roll_back(env: &Env, uuid: &str) -> Result<Outcome> {
    let layout = env.layout;
    let f = find(layout, uuid)?;
    let file = layout.packs.join(&f.file);
    let prev_sig = layout.packs.join(format!("{}{PREVIOUS_SIG_SUFFIX}", f.file));
    if !prev_sig.is_file() {
        return Err(Error::Edition("no earlier edition's signature is kept for this pack, so there is nothing checked to go back to".into()));
    }
    let (h, _) = read_header(&file)?;
    let out = env.checker.run("rollback-plan", &file)?;
    let rep: PlanReport = serde_json::from_slice(&out).map_err(|e| Error::Invalid(format!("the pack checker's answer cannot be read: {e}")))?;
    let mut prev_raw = [0u8; 128];
    let bytes = unhex(&rep.prev_header).filter(|b| b.len() == 128).ok_or_else(|| Error::Invalid("the earlier header is not 128 bytes".into()))?;
    prev_raw.copy_from_slice(&bytes);
    let prev = WaxHeader::parse(&prev_raw).map_err(|e| Error::Invalid(format!("the earlier header: {e}")))?;
    if prev.archive_uuid != h.archive_uuid {
        return Err(Error::Invalid("the earlier header is for another pack".into()));
    }
    if prev.index_offset.checked_add(prev.index_length) != Some(rep.prev_end) {
        return Err(Error::Invalid("the earlier header does not end where the plan cuts".into()));
    }
    // what the earlier edition will be, checked against its kept signature
    // by shelld itself before a byte is written
    let digest = digest_of(&file, &prev_raw, &prev, &rep.prev_ranges)?;
    env.trust.verify(&digest, &prev_sig, &h.archive_uuid).map_err(|e| Error::Signature(format!("the earlier edition does not match its kept signature: {e}")))?;
    let plan = RollbackPlan { prev_header: prev_raw, prev_end: rep.prev_end, prev_ranges: rep.prev_ranges };
    wax_core::apply_rollback(&file, &plan).map_err(|e| Error::Edition(format!("the rollback could not be applied: {e}")))?;
    let sig = layout.packs.join(format!("{}{SIG_SUFFIX}", f.file));
    fs::rename(&prev_sig, &sig)?;
    sync_dir(&layout.packs);

    let mut reg = Register::load(layout)?;
    let version = match reg.packs.get_mut(uuid) {
        Some(i) => {
            let back = i.previous.take().unwrap_or(Edition { version: String::new(), created_at: prev.created_at });
            i.edition = Edition { version: back.version.clone(), created_at: prev.created_at };
            back.version
        }
        None => String::new(),
    };
    // newest_seen is not lowered: the admin chose this, and the newer edition may come back
    reg.save(layout)?;
    Ok(Outcome::RolledBack { archive_uuid: uuid.into(), version })
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_only_in_their_canonical_form() {
        let u = "4c0cfba1-3e77-4b1e-9a02-1f9b3c5d7e01";
        assert_eq!(uuid_text(&check_uuid(u).unwrap()), u);
        for bad in ["4C0CFBA1-3E77-4B1E-9A02-1F9B3C5D7E01", "4c0cfba13e774b1e9a021f9b3c5d7e01", "4c0cfba1-3e77-4b1e-9a02-1f9b3c5d7e0", "../etc/passwd"] {
            assert!(check_uuid(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_any_uuid("4c0cfba13e774b1e9a021f9b3c5d7e01"), check_uuid(u).ok());
    }

    #[test]
    fn source_ids_are_names_not_paths() {
        assert!(check_source_id("drive-usb1/science.wax").is_ok());
        for bad in ["drive-usb1/../x.wax", "/etc/x.wax", "drive-usb1/x.zim", "drive-usb1/.wax", "x.wax", "drive-usb1/sub/x.wax"] {
            assert!(check_source_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn older_in_either_measure_is_older() {
        let seen = Edition { version: "2026.09.3".into(), created_at: 100 };
        assert!(is_older(&Edition { version: "2026.09.2".into(), created_at: 200 }, &seen));
        assert!(is_older(&Edition { version: "2026.10.1".into(), created_at: 99 }, &seen));
        assert!(!is_older(&seen, &seen), "the same edition again is not older");
        assert!(!is_older(&Edition { version: "2026.09.10".into(), created_at: 100 }, &seen), "N is a number, not text");
    }

    #[test]
    fn a_retired_tier_maps_to_its_own_floor_and_an_unknown_one_is_refused() {
        let m = BTreeMap::from([("min_hw_tier".to_string(), "pi_5".to_string())]);
        let n = needs(&m).unwrap();
        assert_eq!((n.min_ram_bytes, n.arch.as_str()), (4 * 1024 * 1024 * 1024, "any"));
        let m = BTreeMap::from([("min_hw_tier".to_string(), "pi_6".to_string())]);
        assert!(needs(&m).is_err());
    }
}
