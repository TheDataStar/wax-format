//! What `deltd` needs from `deltos-identityd`, as an interface.
//!
//! `identityd` owns sessions, sign-in and pack partitions (privilege boundary
//! §4, contract §4 and §8). It is not built yet, and its local-socket protocol
//! is `crates/deltos-identity`'s (docs/identityd.md §4). [`IdentitydLink`] is the
//! live implementation over that socket; [`StaticIdentity`] is a fixed table
//! for tests and local development only.
//!
//! The rule this interface encodes: **`deltd` learns who the caller is only
//! from `identityd`**, never from a request field (deltd API §2).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde::Deserialize;

use crate::hostname::is_partition;

/// A live signed-in session, as `identityd` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Session {
    pub profile_id: String,
    /// The browser the session lives on (privilege boundary §4.4).
    pub browser_id: String,
    /// The Guest profile: sees only guest-accessible packs, stores nothing.
    pub guest: bool,
    /// Made over HTTPS (privilege boundary §4.1).
    pub secure: bool,
    /// Effective permissions (contract §4), looked up live: the names held
    /// at any scope. Scoped checks are identityd's own.
    pub permissions: BTreeSet<String>,
    /// The anti-forgery token every write must carry (privilege boundary
    /// §4.3). Empty means no write is ever accepted for this session.
    pub csrf: String,
    /// This person's pack partition on this browser, when the source
    /// reports it with the session (the live register does).
    pub pack_partition: String,
}

pub trait Identity: Send + Sync {
    /// The live session a `delt_session` / `__Host-delt_session` cookie
    /// value names, if any.
    fn session_by_token(&self, token: &str) -> Option<Session>;

    /// The live session a pack partition was issued under: `Some` only while
    /// that person is still signed in on the browser it was issued for
    /// (deltd API §4.4 rule 2).
    fn session_for_partition(&self, partition: &str) -> Option<Session>;

    /// The partition issued for this session's (browser, person), used to
    /// tell the WebOS which pack hostname to frame.
    fn partition_for(&self, session: &Session) -> Option<String>;

    /// The sign-in calls (deltd API §6.1), forwarded to `identityd` as they
    /// are (docs/identityd.md §4). Only the live register answers them.
    fn call(&self, _req: &serde_json::Value) -> Result<serde_json::Value, CallError> {
        Err(CallError::Unavailable("sign-in needs deltos-identityd; this is the development table".into()))
    }
}

pub use deltos_identity::CallError;

/// The live register, over its local socket.
///
/// Every request on the pack zone asks whether its partition is live, so
/// answers are kept for [`IdentitydLink::CACHE`] and **dropped entirely on any
/// call that can change a session** (every sign-in and sign-out passes
/// through `deltd`). A session that times out on its own is therefore still
/// served for at most that long.
pub struct IdentitydLink {
    client: deltos_identity::Client,
    cache: std::sync::Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    by_token: HashMap<String, (std::time::Instant, Option<(Session, String)>)>,
    by_partition: HashMap<String, (std::time::Instant, Option<Session>)>,
}

impl IdentitydLink {
    pub const CACHE: std::time::Duration = std::time::Duration::from_secs(5);
    /// Calls that read only; anything else empties the cache.
    const READS: &'static [&'static str] = &["session", "session_for_partition", "tap_list", "password_allowed", "list_devices", "list_members"];

    pub fn new(socket: impl Into<std::path::PathBuf>) -> Self {
        IdentitydLink { client: deltos_identity::Client::new(socket), cache: Default::default() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// identityd's session view as deltd's [`Session`], and its partition.
    pub fn convert(v: &serde_json::Value) -> Option<(Session, String)> {
        let o = v.as_object()?;
        let mut permissions: BTreeSet<String> = o
            .get("permissions")?
            .as_array()?
            .iter()
            .filter_map(|h| h.get("permission").and_then(|p| p.as_str()).map(str::to_string))
            .collect();
        if o.get("all_permissions").and_then(|b| b.as_bool()) == Some(true) {
            permissions.insert("*".into());
        }
        let s = Session {
            profile_id: o.get("profile_id")?.as_str()?.to_string(),
            browser_id: o.get("browser")?.as_str()?.to_string(),
            guest: o.get("guest")?.as_bool()?,
            secure: o.get("secure")?.as_bool()?,
            permissions,
            csrf: o.get("csrf")?.as_str()?.to_string(),
            pack_partition: o.get("partition")?.as_str()?.to_string(),
        };
        Some((s, o.get("partition")?.as_str()?.to_string()))
    }

    fn fresh<T: Clone>(e: Option<&(std::time::Instant, T)>) -> Option<T> {
        e.filter(|(t, _)| t.elapsed() < Self::CACHE).map(|(_, v)| v.clone())
    }

    fn lookup(&self, token: &str) -> Option<(Session, String)> {
        if let Some(v) = Self::fresh(self.lock().by_token.get(token)) {
            return v;
        }
        let r = match self.client.call(&serde_json::json!({ "call": "session", "token": token })) {
            Ok(v) => Self::convert(&v),
            // unreachable: nobody is signed in, and nothing is cached
            Err(_) => return None,
        };
        let mut c = self.lock();
        if c.by_token.len() > 50_000 {
            c.by_token.clear();
        }
        c.by_token.insert(token.to_string(), (std::time::Instant::now(), r.clone()));
        r
    }
}

impl Identity for IdentitydLink {
    fn session_by_token(&self, token: &str) -> Option<Session> {
        self.lookup(token).map(|(s, _)| s)
    }

    fn session_for_partition(&self, partition: &str) -> Option<Session> {
        if let Some(v) = Self::fresh(self.lock().by_partition.get(partition)) {
            return v;
        }
        let r = match self.client.call(&serde_json::json!({ "call": "session_for_partition", "partition": partition })) {
            Ok(v) => Self::convert(&v).map(|(s, _)| s),
            Err(_) => return None,
        };
        let mut c = self.lock();
        if c.by_partition.len() > 50_000 {
            c.by_partition.clear();
        }
        c.by_partition.insert(partition.to_string(), (std::time::Instant::now(), r.clone()));
        r
    }

    fn partition_for(&self, session: &Session) -> Option<String> {
        // the partition comes with the session
        Some(session.pack_partition.clone()).filter(|p| !p.is_empty())
    }

    fn call(&self, req: &serde_json::Value) -> Result<serde_json::Value, CallError> {
        let name = req.get("call").and_then(|c| c.as_str()).unwrap_or("");
        let r = self.client.call(req);
        if !Self::READS.contains(&name) {
            *self.lock() = Cache::default();
        }
        r
    }
}

/// A fixed table of sessions, loaded from JSON. **Development and tests
/// only**: it never expires anything and has no sign-in. `deltd` refuses to
/// start without an identity source, so there is no anonymous mode.
#[derive(Debug, Default)]
pub struct StaticIdentity {
    by_token: HashMap<String, (Session, String)>,
    by_partition: HashMap<String, Session>,
}

#[derive(Deserialize)]
struct StaticFile {
    sessions: Vec<StaticSession>,
}

#[derive(Deserialize)]
struct StaticSession {
    token: String,
    profile_id: String,
    browser_id: String,
    #[serde(default)]
    guest: bool,
    #[serde(default)]
    secure: bool,
    #[serde(default)]
    permissions: Vec<String>,
    partition: String,
    #[serde(default)]
    csrf: String,
}

impl StaticIdentity {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one session with its pack partition.
    pub fn add(&mut self, token: &str, session: Session, partition: &str) -> Result<(), String> {
        if !is_partition(partition) {
            return Err(format!("partition {partition:?} is not 16 lowercase hex characters (contract §8)"));
        }
        if token.is_empty() {
            return Err("empty session token".into());
        }
        self.by_partition.insert(partition.to_string(), session.clone());
        self.by_token.insert(token.to_string(), (session, partition.to_string()));
        Ok(())
    }

    /// Sign a partition's person out: its pack hostnames stop being served.
    pub fn sign_out(&mut self, token: &str) {
        if let Some((_, p)) = self.by_token.remove(token) {
            self.by_partition.remove(&p);
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let f: StaticFile = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut id = StaticIdentity::new();
        for s in f.sessions {
            id.add(
                &s.token,
                Session {
                    profile_id: s.profile_id,
                    browser_id: s.browser_id,
                    guest: s.guest,
                    secure: s.secure,
                    permissions: s.permissions.into_iter().collect(),
                    csrf: s.csrf,
                    pack_partition: s.partition.clone(),
                },
                &s.partition,
            )?;
        }
        Ok(id)
    }
}

impl Identity for StaticIdentity {
    fn session_by_token(&self, token: &str) -> Option<Session> {
        self.by_token.get(token).map(|(s, _)| s.clone())
    }
    fn session_for_partition(&self, partition: &str) -> Option<Session> {
        self.by_partition.get(partition).cloned()
    }
    fn partition_for(&self, session: &Session) -> Option<String> {
        self.by_token
            .values()
            .find(|(s, _)| s.profile_id == session.profile_id && s.browser_id == session.browser_id)
            .map(|(_, p)| p.clone())
    }
}
