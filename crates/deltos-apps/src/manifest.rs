//! The app manifest, `deltos-app.toml`, schema 1 (contract §15,
//! `docs/base-addons-admin.md` §5).
//!
//! One file per app. A base service and an add-on use the same format; `kind`
//! says which. Five platform parts read it and nothing else: the launcher, the
//! reverse proxy, backup, health monitoring and the add-on store.
//!
//! Reading is strict: an unknown key is an error, not a pass-through, the same
//! rule `wax-builder` applies to the pack manifest (contract §11).

use serde::{Deserialize, Serialize};
use std::fmt;

/// The only schema this build reads.
pub const SCHEMA: u32 = 1;

/// Every root under which an app may keep or back up data (contract §10:
/// DeltOS state lives in one tree).
pub const STATE_ROOT: &str = "/var/lib/deltos/";

/// The parsed manifest. Field names beyond the §2.3 resource group are this
/// document's to settle (contract §15.2, last paragraph).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppManifest {
    pub schema: u32,
    /// Lowercase id, 2 to 32 characters: `[a-z][a-z0-9-]*`, no trailing `-`.
    pub id: String,
    /// What the app does, in plain words ("Video and music", not "Jellyfin").
    pub name: String,
    /// Semantic version of this packaging of the app.
    pub version: String,
    pub kind: Kind,
    /// One sentence for the store and the dashboard.
    pub summary: String,
    /// Licence of the software, checked by the one licence check (§16).
    pub licence: String,
    /// The software behind it, when it is third-party. Shown to admins only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// The lowest deployment profile it may run on (contract §3).
    pub min_profile: Profile,
    /// Other apps that must be installed and running first, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<String>,
    pub resources: Resources,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<Load>,
    pub run: Run,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<Launcher>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<Reach>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defines: Vec<Permission>,
    pub sign_in: SignIn,
    pub health: Health,
    pub backup: Backup,
    #[serde(default)]
    pub participates: Participates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Part of base DeltOS: installed with the system, never removed by the store.
    Base,
    /// Optional, installed later from the add-on store or a drive.
    Addon,
}

/// Contract §3, in its total order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Kiosk,
    Classroom,
    CommunityHub,
    FieldOps,
}

impl Profile {
    pub fn wire(self) -> &'static str {
        match self {
            Profile::Kiosk => "kiosk",
            Profile::Classroom => "classroom",
            Profile::CommunityHub => "community_hub",
            Profile::FieldOps => "field_ops",
        }
    }
}

/// How much a number can be trusted. The rule "never invent a value" is
/// carried in the data: a floor nobody has measured says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// Measured on a machine of the class the app declares (its `arch`, or
    /// the minimum spec for `any`). `basis` says where and how.
    Measured,
    /// A real measurement on a different machine, or a contract §6
    /// provisional figure: a number to argue with, not evidence.
    Provisional,
    /// Nobody has measured it. The number fields must be absent.
    Open,
}

/// Contract §2.3, unchanged, plus how far to trust it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub evidence: Evidence,
    /// Steady-state RAM with nobody using it (§2.3 `min_ram_bytes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_ram_bytes: Option<u64>,
    /// Storage beyond its own install (§2.3 `min_storage_bytes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_storage_bytes: Option<u64>,
    pub arch: Arch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<Gpu>,
    /// Size of the install itself (program files, images). Counted against
    /// storage at install time, separately from `min_storage_bytes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_bytes: Option<u64>,
    /// Where and how the numbers were measured. Required unless `open`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    Aarch64,
    X86_64,
    Any,
}

impl Arch {
    pub fn wire(self) -> &'static str {
        match self {
            Arch::Aarch64 => "aarch64",
            Arch::X86_64 => "x86_64",
            Arch::Any => "any",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Gpu {
    Required,
    Preferred,
}

/// How cost grows with use: what the capacity estimate needs beyond the idle
/// floor (`docs/base-addons-admin.md` §4).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Load {
    pub evidence: Evidence,
    /// Extra RAM for each person actively using the app at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram_bytes_per_active_user: Option<u64>,
    /// Highest RAM seen under the heaviest load measured (high-water mark).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_ram_bytes: Option<u64>,
    /// Measured ceiling of people at once on the floor hardware, if one was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_active_users: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
}

/// How it runs. Base services are systemd units; add-ons are Docker Compose
/// projects under G1 (Track G §2: one authoring format).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub form: RunForm,
    /// The systemd unit (form `systemd`) or the compose file inside the
    /// add-on bundle (form `compose`).
    pub entry: String,
    /// Network ports it listens on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listens: Vec<Listen>,
    /// Linux capabilities it needs, `CAP_*`. Empty for almost every add-on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Hardware it needs handed to it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunForm {
    Systemd,
    Compose,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Listen {
    pub port: u16,
    pub proto: Proto,
    /// `loopback`: only the box (behind the proxy). `lan`: the box's networks.
    pub bind: Bind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Proto {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Bind {
    Loopback,
    Lan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Device {
    /// A GPU render node, e.g. for video transcoding.
    Gpu,
    /// The Wi-Fi radio (base networking only).
    WifiRadio,
    /// USB drives the admin mounted (content.install's mount action).
    RemovableStorage,
    /// Audio output or input, e.g. community radio.
    Audio,
    /// Serial or SDR hardware (Field Ops modules).
    Serial,
}

/// Where it shows up in the DeltOS launcher (the six groups of the UI review).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Launcher {
    pub group: Group,
    /// Icon file inside the bundle (SVG). The icon set is open (contract §13.2).
    pub icon: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Learn,
    Create,
    Talk,
    WatchAndListen,
    Share,
    Manage,
}

/// How a person reaches it (contract §8 service zone, §15 Reachability).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reach {
    /// `<host>.deltos.lan`. Absent for an app reached inside the WebOS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Permissions required to reach it. Never a role list.
    pub requires: Vec<String>,
    /// Only over HTTPS with a session made over HTTPS (contract §4.2).
    #[serde(default)]
    pub secure_only: bool,
}

/// A permission the app adds to the catalogue (contract §4.1).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Permission {
    /// `<app-ish>.<verb>`, lowercase, dots and underscores.
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub secure_only: bool,
    /// Which shipped bundles get it on install (contract §4.2). `admin` holds
    /// every permission anyway and is not listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seed_bundles: Vec<String>,
}

/// How people are signed in to it (Track H §2's identity bridge).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignIn {
    pub method: SignInMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInMethod {
    /// Part of the WebOS: uses the WebOS session through the box API.
    Webos,
    /// Track H §2: an adapter creates accounts through the app's admin API.
    AdapterApi,
    /// Track H §2: an adapter writes the app's account files.
    AdapterFiles,
    /// After F12: the proxy's checked identity headers.
    ForwardAuth,
    /// Nobody signs in (a headless service).
    None,
}

/// The probe whose result feeds contract §7.1's health states.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    pub probe: Probe,
    /// The unit name, `host:port`, or URL path, depending on `probe`.
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Probe {
    /// The systemd unit's own state (and watchdog, where it has one).
    Systemd,
    /// An HTTP GET that must answer 2xx.
    Http,
    /// A TCP connect.
    Tcp,
    /// The compose healthcheck (G1).
    Compose,
}

/// What F10 keeps (contract §15 Durability).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Backup {
    /// Copied into the backup.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Listed by name and size, not copied (large media, like packs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listed: Vec<String>,
    /// Never backed up (caches, rebuildable indexes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

/// Contract §15 Participation: each opt-in, each independent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Participates {
    #[serde(default)]
    pub search: bool,
    #[serde(default)]
    pub notifications: bool,
    #[serde(default)]
    pub progress: bool,
}

/// One thing wrong with a manifest. Every problem is reported, not just the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub field: String,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("not a valid app manifest: {0}")]
    Syntax(#[from] toml::de::Error),
    #[error("app manifest has {} problem(s):\n  {}", .0.len(), .0.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("\n  "))]
    Invalid(Vec<Problem>),
}

/// Parse and check. Returns the manifest only when it has no problems.
pub fn parse(text: &str) -> Result<AppManifest, ReadError> {
    let m: AppManifest = toml::from_str(text)?;
    let problems = m.check();
    if problems.is_empty() {
        Ok(m)
    } else {
        Err(ReadError::Invalid(problems))
    }
}

fn is_id(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[b.len() - 1] != b'-'
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn is_permission(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.as_bytes()[0].is_ascii_lowercase()
                && p.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        })
}

fn is_semver(s: &str) -> bool {
    let core = s.split(['-', '+']).next().unwrap_or("");
    let n: Vec<&str> = core.split('.').collect();
    n.len() == 3
        && n.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|c| c.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        })
}

/// The contract §8 service names that are already given out, and the WebOS's own.
const RESERVED_HOSTS: [&str; 2] = ["home", "www"];

/// The shipped bundles (contract §4.2) other than `admin`.
const SEED_BUNDLES: [&str; 3] = ["teacher", "student", "guest"];

impl AppManifest {
    /// Every problem, in field order. Empty means valid.
    pub fn check(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut bad = |field: &str, message: String| {
            out.push(Problem {
                field: field.into(),
                message,
            })
        };

        if self.schema != SCHEMA {
            bad(
                "schema",
                format!("this build reads schema {SCHEMA}, not {}", self.schema),
            );
        }
        if !is_id(&self.id) {
            bad(
                "id",
                "2 to 32 characters: lowercase letters, digits and '-', starting with a letter"
                    .into(),
            );
        }
        if self.name.trim().is_empty() {
            bad("name", "must say what the app does".into());
        }
        if !is_semver(&self.version) {
            bad(
                "version",
                format!("'{}' is not MAJOR.MINOR.PATCH", self.version),
            );
        }
        if self.summary.trim().is_empty() {
            bad("summary", "one sentence is required".into());
        }
        if self.licence.trim().is_empty() {
            bad(
                "licence",
                "blank: nothing DeltOS distributes may have no licence (contract §11, §16)".into(),
            );
        }
        for d in &self.depends {
            if !is_id(d) {
                bad("depends", format!("'{d}' is not an app id"));
            } else if d == &self.id {
                bad("depends", "an app cannot depend on itself".into());
            }
        }

        // Resources: §2.3 plus evidence.
        let r = &self.resources;
        match r.evidence {
            Evidence::Open => {
                if r.min_ram_bytes.is_some() || r.min_storage_bytes.is_some() {
                    bad("resources", "evidence is 'open', so no floor number may be written: an unmeasured number is an invented one".into());
                }
            }
            Evidence::Measured | Evidence::Provisional => {
                if r.min_ram_bytes.is_none() {
                    bad(
                        "resources.min_ram_bytes",
                        "required unless evidence is 'open' (contract §2.3)".into(),
                    );
                }
                if r.min_storage_bytes.is_none() {
                    bad(
                        "resources.min_storage_bytes",
                        "required unless evidence is 'open' (contract §2.3)".into(),
                    );
                }
                if r.basis.as_deref().is_none_or(|b| b.trim().is_empty()) {
                    bad(
                        "resources.basis",
                        "say where and how the numbers were measured".into(),
                    );
                }
            }
        }
        if r.min_ram_bytes == Some(0) || r.min_storage_bytes == Some(0) {
            bad(
                "resources",
                "omission is not zero (contract §2.3): leave a field out rather than write 0"
                    .into(),
            );
        }
        if let Some(l) = &self.load {
            match l.evidence {
                Evidence::Open => {
                    if l.ram_bytes_per_active_user.is_some()
                        || l.max_active_users.is_some()
                        || l.peak_ram_bytes.is_some()
                    {
                        bad(
                            "load",
                            "evidence is 'open', so no number may be written".into(),
                        );
                    }
                }
                _ => {
                    if l.ram_bytes_per_active_user.is_none()
                        && l.max_active_users.is_none()
                        && l.peak_ram_bytes.is_none()
                    {
                        bad(
                            "load",
                            "give at least one measured number, or mark it 'open'".into(),
                        );
                    }
                    if l.basis.as_deref().is_none_or(|b| b.trim().is_empty()) {
                        bad(
                            "load.basis",
                            "say where and how the numbers were measured".into(),
                        );
                    }
                }
            }
        }

        // Run.
        if self.run.entry.trim().is_empty() {
            bad(
                "run.entry",
                "name the systemd unit or the compose file".into(),
            );
        }
        let platform = self.id == PLATFORM_ID;
        match (self.kind, self.run.form) {
            // The platform add-on is the orchestrator itself, so it cannot run under it.
            (Kind::Addon, RunForm::Systemd) if platform => {}
            (Kind::Addon, RunForm::Compose) if platform => bad("run.form", "the platform is the orchestrator and cannot run under itself: form 'systemd'".into()),
            (Kind::Addon, RunForm::Systemd) => bad("run.form", "an add-on runs under the orchestrator (form 'compose', Track G §2); only base services are systemd units".into()),
            (Kind::Addon, RunForm::Compose) if !self.depends.iter().any(|d| d == PLATFORM_ID) => {
                bad("depends", format!("a compose add-on needs the '{PLATFORM_ID}' platform (G1 + G2) installed first"))
            }
            _ => {}
        }
        if self.run.form == RunForm::Compose
            && !self.run.entry.ends_with(".yaml")
            && !self.run.entry.ends_with(".yml")
        {
            bad(
                "run.entry",
                "a compose entry is the compose file inside the bundle (.yaml)".into(),
            );
        }
        if self.run.form == RunForm::Systemd && !self.run.entry.ends_with(".service") {
            bad("run.entry", "a systemd entry is a .service unit".into());
        }
        for c in &self.run.capabilities {
            if !c.starts_with("CAP_")
                || c.len() < 5
                || !c[4..].bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
            {
                bad(
                    "run.capabilities",
                    format!("'{c}' is not a Linux capability name"),
                );
            }
            if self.kind == Kind::Addon && !platform {
                bad("run.capabilities", format!("'{c}': an add-on gets no Linux capabilities; ask for a base action instead (privilege boundary §8)"));
            }
        }
        let mut seen = Vec::new();
        for l in &self.run.listens {
            if l.port == 0 {
                bad("run.listens", "port 0 is not a port".into());
            }
            if seen.contains(&(l.port, l.proto)) {
                bad("run.listens", format!("port {} is listed twice", l.port));
            }
            seen.push((l.port, l.proto));
            if self.kind == Kind::Addon && l.bind == Bind::Lan {
                bad("run.listens", format!("port {}: an add-on listens on loopback only and is reached through the proxy (contract §8)", l.port));
            }
        }
        if self.kind == Kind::Addon {
            for d in &self.run.devices {
                if *d == Device::WifiRadio {
                    bad(
                        "run.devices",
                        "the Wi-Fi radio belongs to base networking (E3)".into(),
                    );
                }
            }
            if self.min_profile == Profile::Kiosk {
                bad("min_profile", "Kiosk runs no Track G or Track H service (contract §3), so an add-on starts at 'classroom'".into());
            }
        }
        if self.run.devices.contains(&Device::Gpu) && r.gpu.is_none() {
            bad(
                "resources.gpu",
                "the app asks for the GPU device, so declare gpu 'required' or 'preferred'".into(),
            );
        }

        // Launcher and reach.
        if let Some(lc) = &self.launcher {
            if lc.icon.trim().is_empty()
                || lc.icon.contains("://")
                || lc.icon.starts_with('/')
                || lc.icon.contains("..")
            {
                bad(
                    "launcher.icon",
                    "a relative path to a file inside the bundle".into(),
                );
            }
            if self.reach.is_none() {
                bad(
                    "reach",
                    "an app in the launcher must say who may reach it".into(),
                );
            }
        }
        if let Some(re) = &self.reach {
            if let Some(h) = &re.host {
                if !is_id(h) {
                    bad(
                        "reach.host",
                        format!("'{h}' is not a valid name under deltos.lan"),
                    );
                } else if RESERVED_HOSTS.contains(&h.as_str()) {
                    bad(
                        "reach.host",
                        format!("'{h}.deltos.lan' is reserved (contract §8)"),
                    );
                }
                if self.run.form == RunForm::Compose
                    && !self.run.listens.iter().any(|l| l.proto == Proto::Tcp)
                {
                    bad(
                        "run.listens",
                        "a reachable add-on must name the loopback TCP port the proxy forwards to"
                            .into(),
                    );
                }
            }
            if re.requires.is_empty() {
                bad("reach.requires", "name at least one permission; there is no open door (contract §4.3 default-deny)".into());
            }
            for p in &re.requires {
                if !is_permission(p) {
                    bad("reach.requires", format!("'{p}' is not a permission name"));
                }
            }
        }
        let mut names = Vec::new();
        for p in &self.defines {
            if !is_permission(&p.name) {
                bad(
                    "defines.name",
                    format!(
                        "'{}' is not a permission name (lowercase words joined by '.')",
                        p.name
                    ),
                );
            }
            if names.contains(&&p.name) {
                bad("defines.name", format!("'{}' is defined twice", p.name));
            }
            names.push(&p.name);
            if p.summary.trim().is_empty() {
                bad(
                    "defines.summary",
                    format!(
                        "'{}' needs a plain-words summary for the role editor",
                        p.name
                    ),
                );
            }
            for b in &p.seed_bundles {
                if b == "admin" {
                    bad(
                        "defines.seed_bundles",
                        "admin holds every permission already (contract §4.2); do not list it"
                            .into(),
                    );
                } else if !SEED_BUNDLES.contains(&b.as_str()) {
                    bad(
                        "defines.seed_bundles",
                        format!("'{b}' is not a shipped bundle (teacher, student, guest)"),
                    );
                }
                if p.secure_only && b == "guest" {
                    bad(
                        "defines.seed_bundles",
                        format!(
                            "'{}' is secure-only and guests never have a secure session",
                            p.name
                        ),
                    );
                }
            }
        }
        if self.kind == Kind::Addon
            && self.sign_in.method == SignInMethod::Webos
            && self.reach.as_ref().and_then(|r| r.host.as_ref()).is_some()
        {
            bad("sign_in.method", "an app on its own host cannot use the WebOS session (contract §8: host-only cookie)".into());
        }

        // Health.
        if self.health.target.trim().is_empty() {
            bad(
                "health.target",
                "nothing may run unwatched (contract §15.2)".into(),
            );
        }
        if self.health.probe == Probe::Compose && self.run.form != RunForm::Compose {
            bad(
                "health.probe",
                "'compose' is only for compose add-ons".into(),
            );
        }

        // Backup paths stay inside the state tree.
        for (field, list) in [
            ("backup.paths", &self.backup.paths),
            ("backup.listed", &self.backup.listed),
            ("backup.exclude", &self.backup.exclude),
        ] {
            for p in list {
                if !p.starts_with(STATE_ROOT) || p.contains("/../") || p.ends_with("/..") {
                    bad(
                        field,
                        format!("'{p}' is outside {STATE_ROOT} (contract §10: one state tree)"),
                    );
                }
            }
        }
        out
    }

    /// The licence outcome from the one check DeltOS uses (contract §16).
    pub fn licence_review_required(&self) -> bool {
        wax_builder::config::classify_license(&self.licence).review_required()
    }
}

/// The id of the platform add-on every compose add-on stands on: G1
/// (orchestrator) and G2 (reverse proxy), costed together in contract §6.
pub const PLATFORM_ID: &str = "platform";
