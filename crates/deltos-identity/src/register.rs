//! The register: people, roles, sessions, sign-in cards, remembered devices,
//! pack partitions and re-confirmation tickets, in one SQLite database
//! (contract §10: `/var/lib/deltos/identity/`).
//!
//! Every rule here is one of `docs/privilege-boundary.md` §4, §6 and §10,
//! contract §4 and §5.1, or a value from [`crate::policy`]. Each public
//! method is one call of the socket protocol ([`crate::protocol`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::policy;
use crate::secret;

/// The database's schema version (contract §10: refuse a newer one).
pub const SCHEMA_VERSION: i64 = 1;

/// The built-in role bundles (contract §4.2's seed values). `admin` holds
/// every permission, including ones apps add later, as a property of the
/// bundle rather than a list.
const SEED_ROLES: &[(&str, bool, &[&str])] = &[
    ("admin", true, &[]),
    ("teacher", false, &["profile.manage", "progress.view", "progress.view.self"]),
    ("student", false, &["progress.view.self"]),
    ("guest", false, &[]),
];

/// Every error has one of deltd API §7's codes (plus `sign_in_failed`,
/// added there in the same change).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid_request: {0}")]
    Invalid(String),
    #[error("not_signed_in")]
    NotSignedIn,
    #[error("sign_in_failed")]
    SignInFailed,
    #[error("permission_denied")]
    PermissionDenied,
    #[error("guest_not_allowed")]
    GuestNotAllowed,
    #[error("secure_connection_required")]
    SecureRequired,
    #[error("confirmation_required")]
    ConfirmationRequired,
    #[error("too_many_requests")]
    TooMany,
    #[error("internal: {0}")]
    Internal(String),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::Invalid(_) => "invalid_request",
            Error::NotSignedIn => "not_signed_in",
            Error::SignInFailed => "sign_in_failed",
            Error::PermissionDenied => "permission_denied",
            Error::GuestNotAllowed => "guest_not_allowed",
            Error::SecureRequired => "secure_connection_required",
            Error::ConfirmationRequired => "confirmation_required",
            Error::TooMany => "too_many_requests",
            Error::Internal(_) => "internal",
        }
    }
    pub fn detail(&self) -> Option<String> {
        match self {
            Error::Invalid(d) | Error::Internal(d) => Some(d.clone()),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Internal(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// How a person signs in (contract §5.1's archetypes; Guest is passwordless
/// and one per box).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Passworded,
    Passwordless,
    Guest,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Passworded => "passworded",
            Kind::Passwordless => "passwordless",
            Kind::Guest => "guest",
        }
    }
    fn parse(s: &str) -> Kind {
        match s {
            "passworded" => Kind::Passworded,
            "guest" => Kind::Guest,
            _ => Kind::Passwordless,
        }
    }
}

/// Where a request came from, as `deltd` saw it. `deltd` is the only
/// program that can know the transport and the peer (privilege boundary
/// §3, "where TLS ends").
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ctx {
    /// The request arrived over HTTPS.
    #[serde(default)]
    pub https: bool,
    /// The request came from the box's own screen (deltd: a loopback peer).
    #[serde(default)]
    pub kiosk: bool,
    /// The device's address, for rate limits only. Never stored.
    #[serde(default)]
    pub peer: String,
    /// The browser id cookie, if the browser sent one.
    #[serde(default)]
    pub browser: Option<String>,
    /// The session cookie the browser presented, if any: ended by any new
    /// sign-in on this browser (rule 11, and no session fixation).
    #[serde(default)]
    pub session: Option<String>,
}

/// A role grant: a bundle, at a scope (`None` = box-wide; `Some(class)` =
/// that class or group only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Grant {
    pub role: String,
    #[serde(default)]
    pub scope: Option<String>,
}

/// One held permission and its scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Held {
    pub permission: String,
    pub scope: Option<String>,
}

/// A live session, as every caller sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionView {
    pub profile_id: String,
    pub display_name: String,
    pub kind: Kind,
    pub guest: bool,
    /// Made over HTTPS with a password (privilege boundary §4.1).
    pub secure: bool,
    pub kiosk: bool,
    /// The browser it lives on: the browser id's hash, never the id.
    pub browser: String,
    /// The anti-forgery token (privilege boundary §4.3).
    pub csrf: String,
    /// This person's pack partition on this browser (contract §8).
    pub partition: String,
    pub expires_at: i64,
    pub idle_seconds: i64,
    /// `true` when a role holds every permission (`admin`).
    pub all_permissions: bool,
    pub permissions: Vec<Held>,
}

impl SessionView {
    /// Whether the person holds `permission` at `scope` (a box-wide grant
    /// covers every scope; contract §4.2, privilege boundary rule 4).
    pub fn holds(&self, permission: &str, scope: Option<&str>) -> bool {
        self.all_permissions
            || self
                .permissions
                .iter()
                .any(|h| h.permission == permission && (h.scope.is_none() || h.scope.as_deref() == scope))
    }
}

/// What a successful sign-in hands back for `deltd` to set as cookies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedIn {
    pub session_token: String,
    /// The browser id to keep (new when the browser had none or an unknown one).
    pub browser_token: String,
    /// A new remembered-device credential (card sign-in only).
    pub device_token: Option<String>,
    pub session: SessionView,
}

/// A new profile.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NewProfile {
    pub display_name: String,
    pub kind: Kind,
    #[serde(default)]
    pub sign_in_name: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub class_or_group: Option<String>,
    #[serde(default)]
    pub roles: Vec<Grant>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Card {
    pub profile_id: String,
    /// `XXXXX-XXXXX`
    pub code: String,
    /// What the QR code holds: the WebOS address with the code in the
    /// fragment, which browsers never send to a server or write to its logs.
    pub qr_text: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub device_id: String,
    pub created_at: i64,
    pub last_used: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    pub profile_id: String,
    pub display_name: String,
    pub kind: Kind,
    pub sign_in_name: Option<String>,
    pub roles: Vec<Grant>,
    pub card_expires_at: Option<i64>,
    pub devices: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TapEntry {
    pub profile_id: String,
    pub display_name: String,
    pub class_or_group: Option<String>,
    pub guest: bool,
}

/// A clock, replaceable in tests.
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }
}

/// Failed attempts per key within the window (in memory: a restart forgets
/// them, which only ever helps a person who mistyped).
#[derive(Default)]
struct Limiter {
    failures: HashMap<String, Vec<i64>>,
}

impl Limiter {
    fn blocked(&mut self, key: &str, now: i64) -> bool {
        let v = self.failures.entry(key.to_string()).or_default();
        v.retain(|t| now - t < policy::RATE_WINDOW);
        v.len() as u32 >= policy::RATE_MAX_FAILURES
    }
    fn fail(&mut self, key: &str, now: i64) {
        self.failures.entry(key.to_string()).or_default().push(now);
        if self.failures.len() > 10_000 {
            self.failures.retain(|_, v| v.iter().any(|t| now - t < policy::RATE_WINDOW));
        }
    }
}

pub struct Register {
    db: Mutex<Connection>,
    limiter: Mutex<Limiter>,
    card_key: Vec<u8>,
    clock: Box<dyn Clock>,
    webos_origin_http: String,
}

const SCHEMA: &str = r#"
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE profiles(
    profile_id     TEXT PRIMARY KEY,
    display_name   TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('passworded','passwordless','guest')),
    sign_in_name   TEXT UNIQUE,
    password_hash  TEXT,
    class_or_group TEXT,
    created_at     INTEGER NOT NULL,
    CHECK ((kind = 'passworded') = (password_hash IS NOT NULL)),
    CHECK ((kind = 'passworded') = (sign_in_name IS NOT NULL))
);
CREATE UNIQUE INDEX one_guest ON profiles(kind) WHERE kind = 'guest';
CREATE TABLE roles(
    name            TEXT PRIMARY KEY,
    built_in        INTEGER NOT NULL,
    all_permissions INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE role_permissions(
    role       TEXT NOT NULL REFERENCES roles(name) ON DELETE CASCADE,
    permission TEXT NOT NULL,
    PRIMARY KEY (role, permission)
);
CREATE TABLE profile_roles(
    profile_id TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
    role       TEXT NOT NULL REFERENCES roles(name),
    scope      TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (profile_id, role, scope)
);
CREATE TABLE browsers(
    browser_hash TEXT PRIMARY KEY,
    created_at   INTEGER NOT NULL
);
CREATE TABLE partitions(
    browser_hash TEXT NOT NULL,
    profile_id   TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
    partition    TEXT NOT NULL UNIQUE,
    PRIMARY KEY (browser_hash, profile_id)
);
CREATE TABLE sessions(
    session_hash TEXT PRIMARY KEY,
    profile_id   TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
    browser_hash TEXT NOT NULL,
    secure       INTEGER NOT NULL,
    kiosk        INTEGER NOT NULL,
    csrf         TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    last_active  INTEGER NOT NULL,
    idle_limit   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL
);
CREATE INDEX sessions_by_person ON sessions(profile_id, browser_hash);
CREATE TABLE devices(
    device_id       TEXT PRIMARY KEY,
    credential_hash TEXT NOT NULL UNIQUE,
    profile_id      TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
    created_at      INTEGER NOT NULL,
    last_used       INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL
);
CREATE TABLE cards(
    profile_id TEXT PRIMARY KEY REFERENCES profiles(profile_id) ON DELETE CASCADE,
    code_mac   TEXT NOT NULL UNIQUE,
    issued_at  INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    issued_by  TEXT NOT NULL
);
CREATE TABLE tickets(
    ticket_hash  TEXT PRIMARY KEY,
    session_hash TEXT NOT NULL,
    action       TEXT NOT NULL,
    args_digest  TEXT NOT NULL,
    expires_at   INTEGER NOT NULL
);
"#;

fn valid_text(s: &str, what: &str, max: usize) -> Result<()> {
    if s.trim().is_empty() || s.len() > max || s.chars().any(char::is_control) {
        return Err(Error::Invalid(format!("{what} must be 1–{max} bytes of text")));
    }
    Ok(())
}

fn valid_sign_in_name(s: &str) -> Result<()> {
    let ok = (1..=32).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
    if !ok {
        return Err(Error::Invalid("sign_in_name must be 1–32 of a-z 0-9 . _ -".into()));
    }
    Ok(())
}

fn valid_password(p: &str) -> Result<()> {
    if p.chars().count() < policy::PASSWORD_MIN_CHARS || p.len() > policy::PASSWORD_MAX_BYTES {
        return Err(Error::Invalid(format!(
            "password must be at least {} characters and at most {} bytes",
            policy::PASSWORD_MIN_CHARS,
            policy::PASSWORD_MAX_BYTES
        )));
    }
    Ok(())
}

fn new_profile_id() -> String {
    // a UUIDv4, as contract §5.1's example shows
    let mut b = secret::random_bytes(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = secret::hex(&b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

impl Register {
    /// Open (or create) the register in `dir`: `identity.sqlite` and the
    /// card key `card-key`, both created owner-only.
    pub fn open(dir: &Path, webos_host: &str) -> Result<Self> {
        Self::open_with_clock(dir, webos_host, Box::new(SystemClock))
    }

    pub fn open_with_clock(dir: &Path, webos_host: &str, clock: Box<dyn Clock>) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| Error::Internal(format!("{}: {e}", dir.display())))?;
        let card_key = load_or_make_key(&dir.join("card-key"))?;
        let db_path: PathBuf = dir.join("identity.sqlite");
        let fresh = !db_path.exists();
        if fresh {
            create_private(&db_path)?;
        }
        let c = Connection::open(&db_path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        let has_meta: bool =
            c.query_row("SELECT count(*) FROM sqlite_master WHERE name='meta'", [], |r| r.get::<_, i64>(0))? > 0;
        if !has_meta {
            c.execute_batch(SCHEMA)?;
            c.execute("INSERT INTO meta VALUES ('schema_version', ?1)", [SCHEMA_VERSION.to_string()])?;
            for (name, all, perms) in SEED_ROLES {
                c.execute("INSERT INTO roles VALUES (?1, 1, ?2)", params![name, *all as i64])?;
                for p in *perms {
                    c.execute("INSERT INTO role_permissions VALUES (?1, ?2)", params![name, p])?;
                }
            }
        } else {
            let v: i64 = c
                .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get::<_, String>(0))?
                .parse()
                .map_err(|_| Error::Internal("schema_version is not a number".into()))?;
            if v > SCHEMA_VERSION {
                return Err(Error::Internal(format!(
                    "the identity register is schema {v}, newer than this program's {SCHEMA_VERSION}; refusing to open it (contract §10)"
                )));
            }
        }
        Ok(Register {
            db: Mutex::new(c),
            limiter: Mutex::new(Limiter::default()),
            card_key,
            clock,
            webos_origin_http: format!("http://{}", webos_host.to_ascii_lowercase()),
        })
    }

    fn now(&self) -> i64 {
        self.clock.now()
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn rate_check(&self, keys: &[String]) -> Result<()> {
        let now = self.now();
        let mut l = self.limiter.lock().unwrap_or_else(|p| p.into_inner());
        if keys.iter().any(|k| l.blocked(k, now)) {
            return Err(Error::TooMany);
        }
        Ok(())
    }

    fn rate_fail(&self, keys: &[String]) {
        let now = self.now();
        let mut l = self.limiter.lock().unwrap_or_else(|p| p.into_inner());
        for k in keys {
            l.fail(k, now);
        }
    }

    // ---------------------------------------------------------------- reads

    /// The live session a session cookie names. Ends it if it has expired.
    pub fn session(&self, token: &str) -> Result<Option<SessionView>> {
        if !secret::is_token(token) {
            return Ok(None);
        }
        let h = secret::token_hash(token);
        let c = self.db();
        self.live_session(&c, &h, true)
    }

    fn live_session(&self, c: &Connection, session_hash: &str, touch: bool) -> Result<Option<SessionView>> {
        let now = self.now();
        let row = c
            .query_row(
                "SELECT s.profile_id, p.display_name, p.kind, s.browser_hash, s.secure, s.kiosk, s.csrf,
                        s.last_active, s.idle_limit, s.expires_at
                 FROM sessions s JOIN profiles p USING (profile_id) WHERE s.session_hash = ?1",
                [session_hash],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)? != 0,
                        r.get::<_, i64>(5)? != 0,
                        r.get::<_, String>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, i64>(8)?,
                        r.get::<_, i64>(9)?,
                    ))
                },
            )
            .optional()?;
        let Some((pid, name, kind, browser, secure, kiosk, csrf, last, idle, expires)) = row else {
            return Ok(None);
        };
        if now >= expires || now - last >= idle {
            c.execute("DELETE FROM sessions WHERE session_hash = ?1", [session_hash])?;
            return Ok(None);
        }
        if touch && now - last >= policy::TOUCH_EVERY {
            c.execute("UPDATE sessions SET last_active = ?1 WHERE session_hash = ?2", params![now, session_hash])?;
        }
        let partition: String = c
            .query_row(
                "SELECT partition FROM partitions WHERE browser_hash = ?1 AND profile_id = ?2",
                params![browser, pid],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default();
        let (all, perms) = effective(c, &pid)?;
        let kind = Kind::parse(&kind);
        Ok(Some(SessionView {
            profile_id: pid,
            display_name: name,
            kind,
            guest: kind == Kind::Guest,
            secure,
            kiosk,
            browser,
            csrf,
            partition,
            expires_at: expires.min(now.max(last) + idle),
            idle_seconds: idle,
            all_permissions: all,
            permissions: perms,
        }))
    }

    /// The live session a pack partition was issued under: `Some` only
    /// while its person is signed in on the browser it was issued for
    /// (deltd API §4.4 rule 2).
    pub fn session_for_partition(&self, partition: &str) -> Result<Option<SessionView>> {
        if partition.len() != 16 || !partition.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            return Ok(None);
        }
        let c = self.db();
        let Some((browser, pid)): Option<(String, String)> = c
            .query_row("SELECT browser_hash, profile_id FROM partitions WHERE partition = ?1", [partition], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?
        else {
            return Ok(None);
        };
        let hashes: Vec<String> = c
            .prepare(
                "SELECT session_hash FROM sessions WHERE browser_hash = ?1 AND profile_id = ?2 ORDER BY last_active DESC",
            )?
            .query_map(params![browser, pid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for h in hashes {
            if let Some(s) = self.live_session(&c, &h, false)? {
                return Ok(Some(s));
            }
        }
        Ok(None)
    }

    /// The tap-your-name list: passwordless people and Guest. **Kiosk only**
    /// (privilege boundary §10); refused anywhere else.
    pub fn tap_list(&self, ctx: &Ctx) -> Result<Vec<TapEntry>> {
        if !ctx.kiosk {
            return Err(Error::PermissionDenied);
        }
        let c = self.db();
        let mut st = c.prepare(
            "SELECT profile_id, display_name, class_or_group, kind FROM profiles
             WHERE kind IN ('passwordless','guest') ORDER BY kind = 'guest', class_or_group, display_name",
        )?;
        let v = st
            .query_map([], |r| {
                Ok(TapEntry {
                    profile_id: r.get(0)?,
                    display_name: r.get(1)?,
                    class_or_group: r.get(2)?,
                    guest: r.get::<_, String>(3)? == "guest",
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    // ------------------------------------------------------------- sign-in

    /// Step one of a password sign-in, before any password is sent: whether
    /// this name may sign in with a password over this connection. Lets the
    /// WebOS refuse to show a password box over plain HTTP to a teacher or
    /// admin (contract §5.1), so their password never travels in the clear.
    pub fn password_allowed(&self, ctx: &Ctx, sign_in_name: &str) -> Result<()> {
        let c = self.db();
        let pid: Option<String> = c
            .query_row(
                "SELECT profile_id FROM profiles WHERE sign_in_name = ?1 AND kind = 'passworded'",
                [sign_in_name],
                |r| r.get(0),
            )
            .optional()?;
        match pid {
            Some(pid) if !ctx.https && holds_secure_only(&c, &pid)? => Err(Error::SecureRequired),
            _ => Ok(()),
        }
    }

    /// Sign in with a name and password. Over plain HTTP, refused for anyone
    /// holding a secure-only permission before the password is even
    /// checked (contract §5.1). Makes a secure session over HTTPS.
    pub fn sign_in_password(&self, ctx: &Ctx, sign_in_name: &str, password: &str) -> Result<SignedIn> {
        let keys = [format!("peer:{}", ctx.peer), format!("name:{sign_in_name}")];
        self.rate_check(&keys)?;
        let row: Option<(String, String)> = self
            .db()
            .query_row(
                "SELECT profile_id, password_hash FROM profiles WHERE sign_in_name = ?1 AND kind = 'passworded'",
                [sign_in_name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((pid, hash)) = row else {
            secret::verify_password(password, secret::dummy_hash());
            self.rate_fail(&keys);
            return Err(Error::SignInFailed);
        };
        if !ctx.https && holds_secure_only(&self.db(), &pid)? {
            return Err(Error::SecureRequired);
        }
        // the slow hash runs outside the database lock
        if !secret::verify_password(password, &hash) {
            self.rate_fail(&keys);
            return Err(Error::SignInFailed);
        }
        self.start_session(ctx, &pid, ctx.https, None)
    }

    /// Sign in a passwordless person with their sign-in card: makes a
    /// remembered device and a plain session (privilege boundary §10).
    pub fn sign_in_card(&self, ctx: &Ctx, code: &str) -> Result<SignedIn> {
        let keys = [format!("peer:{}", ctx.peer)];
        self.rate_check(&keys)?;
        let Some(code) = secret::normalize_card_code(code) else {
            self.rate_fail(&keys);
            return Err(Error::SignInFailed);
        };
        let mac = secret::card_mac(&self.card_key, &code);
        let now = self.now();
        let pid: Option<String> = self
            .db()
            .query_row(
                "SELECT c.profile_id FROM cards c JOIN profiles p USING (profile_id)
                 WHERE c.code_mac = ?1 AND c.expires_at > ?2 AND p.kind IN ('passwordless','guest')",
                params![mac, now],
                |r| r.get(0),
            )
            .optional()?;
        let Some(pid) = pid else {
            self.rate_fail(&keys);
            return Err(Error::SignInFailed);
        };
        let device = secret::token();
        self.db().execute(
            "INSERT INTO devices VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            params![secret::short_id(), secret::token_hash(&device), pid, now, now + policy::DEVICE_ABSOLUTE],
        )?;
        self.start_session(ctx, &pid, false, Some(device))
    }

    /// Start a new plain session for a remembered device's person.
    pub fn sign_in_device(&self, ctx: &Ctx, device: &str) -> Result<SignedIn> {
        let keys = [format!("peer:{}", ctx.peer)];
        self.rate_check(&keys)?;
        let now = self.now();
        let pid: Option<String> = if secret::is_token(device) {
            let c = self.db();
            let h = secret::token_hash(device);
            let pid = c
                .query_row(
                    "SELECT d.profile_id FROM devices d JOIN profiles p USING (profile_id)
                     WHERE d.credential_hash = ?1 AND d.expires_at > ?2 AND d.last_used > ?3
                       AND p.kind IN ('passwordless','guest')",
                    params![h, now, now - policy::DEVICE_UNUSED],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            if pid.is_some() {
                c.execute("UPDATE devices SET last_used = ?1 WHERE credential_hash = ?2", params![now, h])?;
            } else {
                c.execute(
                    "DELETE FROM devices WHERE credential_hash = ?1 AND (expires_at <= ?2 OR last_used <= ?3)",
                    params![h, now, now - policy::DEVICE_UNUSED],
                )?;
            }
            pid
        } else {
            None
        };
        let Some(pid) = pid else {
            self.rate_fail(&keys);
            return Err(Error::NotSignedIn);
        };
        self.start_session(ctx, &pid, false, None)
    }

    /// Tap-your-name, on the kiosk only, for passwordless people and Guest.
    pub fn sign_in_tap(&self, ctx: &Ctx, profile_id: &str) -> Result<SignedIn> {
        if !ctx.kiosk {
            return Err(Error::PermissionDenied);
        }
        let ok: bool = self
            .db()
            .query_row(
                "SELECT count(*) FROM profiles WHERE profile_id = ?1 AND kind IN ('passwordless','guest')",
                [profile_id],
                |r| r.get::<_, i64>(0),
            )?
            > 0;
        if !ok {
            return Err(Error::SignInFailed);
        }
        self.start_session(ctx, profile_id, false, None)
    }

    fn start_session(&self, ctx: &Ctx, pid: &str, secure: bool, device_token: Option<String>) -> Result<SignedIn> {
        let now = self.now();
        let c = self.db();
        // Any session this browser presented ends first: switching person
        // never overlaps (rule 11), and no planted id survives (§4.2).
        if let Some(old) = ctx.session.as_deref().filter(|t| secret::is_token(t)) {
            c.execute("DELETE FROM sessions WHERE session_hash = ?1", [secret::token_hash(old)])?;
        }
        let known_browser = match ctx.browser.as_deref().filter(|t| secret::is_token(t)) {
            Some(b) => c
                .query_row("SELECT count(*) FROM browsers WHERE browser_hash = ?1", [secret::token_hash(b)], |r| {
                    r.get::<_, i64>(0)
                })?
                > 0,
            None => false,
        };
        let browser_token = match (known_browser, ctx.browser.clone()) {
            (true, Some(b)) => b,
            _ => {
                let b = secret::token();
                c.execute("INSERT INTO browsers VALUES (?1, ?2)", params![secret::token_hash(&b), now])?;
                b
            }
        };
        let bh = secret::token_hash(&browser_token);
        let have: bool = c.query_row(
            "SELECT count(*) FROM partitions WHERE browser_hash = ?1 AND profile_id = ?2",
            params![bh, pid],
            |r| r.get::<_, i64>(0),
        )? > 0;
        if !have {
            c.execute("INSERT INTO partitions VALUES (?1, ?2, ?3)", params![bh, pid, secret::short_id()])?;
        }
        let token = secret::token();
        let (idle, abs) = policy::session_limits(ctx.kiosk, secure);
        c.execute(
            "INSERT INTO sessions VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9)",
            params![
                secret::token_hash(&token),
                pid,
                bh,
                secure as i64,
                ctx.kiosk as i64,
                secret::token(),
                now,
                idle,
                now + abs
            ],
        )?;
        let session = self
            .live_session(&c, &secret::token_hash(&token), false)?
            .ok_or_else(|| Error::Internal("new session not found".into()))?;
        Ok(SignedIn { session_token: token, browser_token, device_token, session })
    }

    // ------------------------------------------------------------ sign-out

    /// Sign out of this browser; with `forget_device`, also forget the
    /// remembered device the browser presented.
    pub fn sign_out(&self, token: &str, forget_device: Option<&str>) -> Result<()> {
        let s = self.session(token)?.ok_or(Error::NotSignedIn)?;
        let c = self.db();
        c.execute("DELETE FROM sessions WHERE session_hash = ?1", [secret::token_hash(token)])?;
        if let Some(d) = forget_device.filter(|d| secret::is_token(d)) {
            c.execute(
                "DELETE FROM devices WHERE credential_hash = ?1 AND profile_id = ?2",
                params![secret::token_hash(d), s.profile_id],
            )?;
        }
        Ok(())
    }

    /// Sign out everywhere: every session of this person ends.
    pub fn sign_out_everywhere(&self, token: &str) -> Result<()> {
        let s = self.session(token)?.ok_or(Error::NotSignedIn)?;
        if s.guest {
            return Err(Error::GuestNotAllowed);
        }
        self.db().execute("DELETE FROM sessions WHERE profile_id = ?1", [&s.profile_id])?;
        Ok(())
    }

    // ------------------------------------------------- managing other people

    /// A secure session over HTTPS, or the reason not (privilege boundary
    /// §4.1; card issue, device removal and profile changes are secure only).
    fn secure_session(&self, ctx: &Ctx, token: &str) -> Result<SessionView> {
        let s = self.session(token)?.ok_or(Error::NotSignedIn)?;
        if !(ctx.https && s.secure) {
            return Err(Error::SecureRequired);
        }
        Ok(s)
    }

    fn profile_row(&self, pid: &str) -> Result<(Kind, Option<String>)> {
        self.db()
            .query_row("SELECT kind, class_or_group FROM profiles WHERE profile_id = ?1", [pid], |r| {
                Ok((Kind::parse(&r.get::<_, String>(0)?), r.get(1)?))
            })
            .optional()?
            .ok_or_else(|| Error::Invalid("no such profile".into()))
    }

    /// `profile.manage` for the class the target person is in (a box-wide
    /// grant covers every class; Guest has no class, so only box-wide).
    /// `class`, when the caller names one (deltd's `/classes/{id}/...`
    /// paths), must be the class the person is actually in.
    fn may_manage(&self, s: &SessionView, target: &str, in_class: Option<&str>) -> Result<(Kind, Option<String>)> {
        let (kind, class) = self.profile_row(target)?;
        if in_class.is_some() && in_class != class.as_deref() {
            return Err(Error::Invalid("that person is not in that class".into()));
        }
        if !s.holds("profile.manage", class.as_deref()) || (class.is_none() && !s.holds("profile.manage", None)) {
            return Err(Error::PermissionDenied);
        }
        Ok((kind, class))
    }

    /// Issue a new sign-in card for a passwordless person; the old one stops
    /// working at once.
    pub fn issue_card(&self, ctx: &Ctx, token: &str, profile_id: &str, class: Option<&str>) -> Result<Card> {
        let s = self.secure_session(ctx, token)?;
        let (kind, _) = self.may_manage(&s, profile_id, class)?;
        if kind == Kind::Passworded {
            return Err(Error::Invalid("a sign-in card is only for a person without a password".into()));
        }
        let code = secret::card_code();
        let now = self.now();
        let expires = now + policy::CARD_VALID;
        self.db().execute(
            "INSERT INTO cards VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(profile_id) DO UPDATE SET code_mac = excluded.code_mac, issued_at = excluded.issued_at,
                 expires_at = excluded.expires_at, issued_by = excluded.issued_by",
            params![profile_id, secret::card_mac(&self.card_key, &code), now, expires, s.profile_id],
        )?;
        Ok(Card {
            profile_id: profile_id.into(),
            code: secret::display_card_code(&code),
            qr_text: format!("{}/#card={}", self.webos_origin_http, code),
            expires_at: expires,
        })
    }

    pub fn list_devices(&self, ctx: &Ctx, token: &str, profile_id: &str, class: Option<&str>) -> Result<Vec<Device>> {
        let s = self.secure_session(ctx, token)?;
        self.may_manage(&s, profile_id, class)?;
        let c = self.db();
        let mut st = c.prepare(
            "SELECT device_id, created_at, last_used, expires_at FROM devices WHERE profile_id = ?1 ORDER BY created_at",
        )?;
        let v = st
            .query_map([profile_id], |r| {
                Ok(Device { device_id: r.get(0)?, created_at: r.get(1)?, last_used: r.get(2)?, expires_at: r.get(3)? })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Remove a remembered device: it can start no new session, and every
    /// session it started on its browser ends at once.
    pub fn remove_device(
        &self,
        ctx: &Ctx,
        token: &str,
        profile_id: &str,
        class: Option<&str>,
        device_id: &str,
    ) -> Result<()> {
        let s = self.secure_session(ctx, token)?;
        self.may_manage(&s, profile_id, class)?;
        let n = self
            .db()
            .execute("DELETE FROM devices WHERE device_id = ?1 AND profile_id = ?2", params![device_id, profile_id])?;
        if n == 0 {
            return Err(Error::Invalid("no such device".into()));
        }
        // The device's sessions are this person's plain sessions; a lost
        // phone must stop working now, not when its session runs out.
        self.db().execute("DELETE FROM sessions WHERE profile_id = ?1 AND secure = 0", [profile_id])?;
        Ok(())
    }

    /// Create a profile. A teacher (`profile.manage` for a class) may add a
    /// passwordless student to that class; anything else needs
    /// `admin.console` (docs/identityd.md §5).
    pub fn create_profile(&self, ctx: &Ctx, token: &str, p: &NewProfile) -> Result<String> {
        let s = self.secure_session(ctx, token)?;
        let student_only = p.kind == Kind::Passwordless
            && p.roles.iter().all(|g| g.role == "student" && g.scope.is_none())
            && p.class_or_group.is_some();
        let allowed = s.holds("admin.console", None)
            || (student_only && s.holds("profile.manage", p.class_or_group.as_deref()));
        if !allowed {
            return Err(Error::PermissionDenied);
        }
        self.insert_profile(p)
    }

    /// The profile insert itself, shared with first-boot provisioning.
    pub fn insert_profile(&self, p: &NewProfile) -> Result<String> {
        valid_text(&p.display_name, "display_name", 100)?;
        if let Some(cl) = &p.class_or_group {
            valid_text(cl, "class_or_group", 64)?;
        }
        let hash = match p.kind {
            Kind::Passworded => {
                let n = p.sign_in_name.as_deref().ok_or_else(|| Error::Invalid("sign_in_name is required".into()))?;
                valid_sign_in_name(n)?;
                let pw = p.password.as_deref().ok_or_else(|| Error::Invalid("password is required".into()))?;
                valid_password(pw)?;
                Some(secret::hash_password(pw))
            }
            _ => {
                if p.sign_in_name.is_some() || p.password.is_some() {
                    return Err(Error::Invalid("a person without a password has no sign_in_name or password".into()));
                }
                None
            }
        };
        let id = new_profile_id();
        let mut c = self.db();
        let tx = c.transaction()?;
        for g in &p.roles {
            let exists: bool =
                tx.query_row("SELECT count(*) FROM roles WHERE name = ?1", [&g.role], |r| r.get::<_, i64>(0))? > 0;
            if !exists {
                return Err(Error::Invalid(format!("no role {:?}", g.role)));
            }
        }
        let r = tx.execute(
            "INSERT INTO profiles VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, p.display_name, p.kind.as_str(), p.sign_in_name, hash, p.class_or_group, self.now()],
        );
        if let Err(rusqlite::Error::SqliteFailure(f, _)) = &r {
            if f.code == rusqlite::ErrorCode::ConstraintViolation {
                return Err(Error::Invalid("that sign_in_name is taken, or Guest already exists".into()));
            }
        }
        r?;
        for g in &p.roles {
            tx.execute(
                "INSERT OR IGNORE INTO profile_roles VALUES (?1, ?2, ?3)",
                params![id, g.role, g.scope.clone().unwrap_or_default()],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// The people in a class, for the teacher who manages it.
    pub fn list_members(&self, ctx: &Ctx, token: &str, class: &str) -> Result<Vec<Member>> {
        let s = self.secure_session(ctx, token)?;
        if !s.holds("profile.manage", Some(class)) {
            return Err(Error::PermissionDenied);
        }
        let c = self.db();
        let now = self.now();
        let mut st = c.prepare(
            "SELECT p.profile_id, p.display_name, p.kind, p.sign_in_name,
                    (SELECT expires_at FROM cards WHERE profile_id = p.profile_id AND expires_at > ?2),
                    (SELECT count(*) FROM devices WHERE profile_id = p.profile_id)
             FROM profiles p WHERE p.class_or_group = ?1 ORDER BY p.display_name",
        )?;
        type Row = (String, String, String, Option<String>, Option<i64>, i64);
        let rows: Vec<Row> = st
            .query_map(params![class, now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::new();
        for (pid, name, kind, sin, card, devices) in rows {
            out.push(Member {
                roles: grants(&c, &pid)?,
                profile_id: pid,
                display_name: name,
                kind: Kind::parse(&kind),
                sign_in_name: sin,
                card_expires_at: card,
                devices: devices as usize,
            });
        }
        Ok(out)
    }

    /// Change one's own password. Every session of the person ends,
    /// this one included (privilege boundary §4.2).
    pub fn change_password(&self, ctx: &Ctx, token: &str, current: &str, new: &str) -> Result<()> {
        let s = self.secure_session(ctx, token)?;
        valid_password(new)?;
        self.check_password(ctx, &s.profile_id, current)?;
        self.set_password_hash(&s.profile_id, new)
    }

    /// An admin sets another person's password (a reset).
    pub fn set_password(&self, ctx: &Ctx, token: &str, profile_id: &str, new: &str) -> Result<()> {
        let s = self.secure_session(ctx, token)?;
        if !s.holds("admin.console", None) {
            return Err(Error::PermissionDenied);
        }
        if self.profile_row(profile_id)?.0 != Kind::Passworded {
            return Err(Error::Invalid("that person signs in without a password".into()));
        }
        valid_password(new)?;
        self.set_password_hash(profile_id, new)
    }

    fn set_password_hash(&self, pid: &str, new: &str) -> Result<()> {
        let h = secret::hash_password(new);
        let c = self.db();
        c.execute("UPDATE profiles SET password_hash = ?1 WHERE profile_id = ?2", params![h, pid])?;
        c.execute("DELETE FROM sessions WHERE profile_id = ?1", [pid])?;
        Ok(())
    }

    fn check_password(&self, ctx: &Ctx, pid: &str, password: &str) -> Result<()> {
        let keys = [format!("peer:{}", ctx.peer), format!("profile:{pid}")];
        self.rate_check(&keys)?;
        let hash: Option<String> = self
            .db()
            .query_row("SELECT password_hash FROM profiles WHERE profile_id = ?1", [pid], |r| r.get(0))
            .optional()?
            .flatten();
        // a passwordless identity can never re-confirm (privilege boundary §6)
        let ok = match &hash {
            Some(h) => secret::verify_password(password, h),
            None => false,
        };
        if !ok {
            self.rate_fail(&keys);
            return Err(Error::ConfirmationRequired);
        }
        Ok(())
    }

    // ------------------------------------------------------ re-confirmation

    /// Check the password again and issue a single-use ticket bound to this
    /// session, this action and these exact arguments (privilege boundary §6).
    pub fn issue_ticket(&self, ctx: &Ctx, token: &str, password: &str, action: &str, args_digest: &str) -> Result<String> {
        let s = self.secure_session(ctx, token)?;
        valid_text(action, "action", 64)?;
        if args_digest.len() != 64 || !args_digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid("args_digest must be a SHA-256 in hex".into()));
        }
        self.check_password(ctx, &s.profile_id, password)?;
        let t = secret::token();
        let now = self.now();
        let c = self.db();
        c.execute("DELETE FROM tickets WHERE expires_at <= ?1", [now])?;
        c.execute(
            "INSERT INTO tickets VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                secret::token_hash(&t),
                secret::token_hash(token),
                action,
                args_digest.to_ascii_lowercase(),
                now + policy::TICKET_VALID
            ],
        )?;
        Ok(t)
    }

    /// Redeem a ticket (`deltos-shelld` only): `Ok` once, for exactly the
    /// session, action and arguments it was issued for, within its minute.
    pub fn redeem_ticket(&self, token: &str, ticket: &str, action: &str, args_digest: &str) -> Result<()> {
        if !secret::is_token(ticket) || !secret::is_token(token) {
            return Err(Error::ConfirmationRequired);
        }
        let now = self.now();
        let c = self.db();
        // deleted whether it matches or not: a ticket is tried once
        let row: Option<(String, String, String, i64)> = c
            .query_row(
                "DELETE FROM tickets WHERE ticket_hash = ?1 RETURNING session_hash, action, args_digest, expires_at",
                [secret::token_hash(ticket)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        match row {
            Some((sh, a, d, exp))
                if sh == secret::token_hash(token)
                    && a == action
                    && d.eq_ignore_ascii_case(args_digest)
                    && now < exp
                    && self.live_session(&c, &sh, false)?.is_some() =>
            {
                Ok(())
            }
            _ => Err(Error::ConfirmationRequired),
        }
    }

    // ------------------------------------------------------- first boot

    /// Whether any profile holds the `admin` role.
    pub fn has_admin(&self) -> Result<bool> {
        Ok(self.db().query_row("SELECT count(*) FROM profile_roles WHERE role = 'admin'", [], |r| r.get::<_, i64>(0))?
            > 0)
    }

    /// Track F §2's first admin: only while no admin exists. Returns the
    /// profile id and the generated password, shown once and never stored.
    pub fn provision_first_admin(&self, display_name: &str, sign_in_name: &str) -> Result<(String, String)> {
        if self.has_admin()? {
            return Err(Error::Invalid("this box already has an admin; provisioning works only once".into()));
        }
        let pw = secret::generated_password();
        let id = self.insert_profile(&NewProfile {
            display_name: display_name.into(),
            kind: Kind::Passworded,
            sign_in_name: Some(sign_in_name.into()),
            password: Some(pw.clone()),
            class_or_group: None,
            roles: vec![Grant { role: "admin".into(), scope: None }],
        })?;
        Ok((id, pw))
    }

    /// Housekeeping: drop expired sessions, devices, cards and tickets.
    pub fn sweep(&self) -> Result<usize> {
        let now = self.now();
        let c = self.db();
        let mut n = c.execute(
            "DELETE FROM sessions WHERE expires_at <= ?1 OR last_active + idle_limit <= ?1",
            [now],
        )?;
        n += c.execute(
            "DELETE FROM devices WHERE expires_at <= ?1 OR last_used <= ?2",
            params![now, now - policy::DEVICE_UNUSED],
        )?;
        n += c.execute("DELETE FROM cards WHERE expires_at <= ?1", [now])?;
        n += c.execute("DELETE FROM tickets WHERE expires_at <= ?1", [now])?;
        Ok(n)
    }
}

fn grants(c: &Connection, pid: &str) -> Result<Vec<Grant>> {
    let mut st = c.prepare("SELECT role, scope FROM profile_roles WHERE profile_id = ?1 ORDER BY role, scope")?;
    let v = st
        .query_map([pid], |r| {
            let s: String = r.get(1)?;
            Ok(Grant { role: r.get(0)?, scope: (!s.is_empty()).then_some(s) })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

/// Effective permissions: the union of the person's grants, each at its
/// grant's scope, looked up live (privilege boundary §4.2).
fn effective(c: &Connection, pid: &str) -> Result<(bool, Vec<Held>)> {
    let mut all = false;
    let mut set: BTreeSet<Held> = BTreeSet::new();
    let mut st = c.prepare(
        "SELECT r.all_permissions, rp.permission, pr.scope FROM profile_roles pr
         JOIN roles r ON r.name = pr.role LEFT JOIN role_permissions rp ON rp.role = pr.role
         WHERE pr.profile_id = ?1",
    )?;
    let rows = st.query_map([pid], |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?)))?;
    for row in rows {
        let (a, perm, scope) = row?;
        all |= a;
        if let Some(p) = perm {
            set.insert(Held { permission: p, scope: (!scope.is_empty()).then_some(scope) });
        }
    }
    // a box-wide grant makes the scoped copies of the same permission redundant
    let wide: BTreeMap<String, ()> =
        set.iter().filter(|h| h.scope.is_none()).map(|h| (h.permission.clone(), ())).collect();
    let v = set.into_iter().filter(|h| h.scope.is_none() || !wide.contains_key(&h.permission)).collect();
    Ok((all, v))
}

fn holds_secure_only(c: &Connection, pid: &str) -> Result<bool> {
    let (all, perms) = effective(c, pid)?;
    Ok(all || perms.iter().any(|h| policy::is_secure_only(&h.permission)))
}

#[cfg(unix)]
fn create_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
        .map_err(|e| Error::Internal(format!("{}: {e}", path.display())))
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> Result<()> {
    std::fs::File::create(path).map(|_| ()).map_err(|e| Error::Internal(format!("{}: {e}", path.display())))
}

fn load_or_make_key(path: &Path) -> Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(k) if k.len() == 32 => Ok(k),
        Ok(_) => Err(Error::Internal(format!("{} is not a 32-byte key", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let k = secret::random_bytes(32);
            create_private(path)?;
            std::fs::write(path, &k).map_err(|e| Error::Internal(format!("{}: {e}", path.display())))?;
            Ok(k)
        }
        Err(e) => Err(Error::Internal(format!("{}: {e}", path.display()))),
    }
}
