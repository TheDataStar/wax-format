//! What `deltd` needs from `deltos-identityd`, as an interface.
//!
//! `identityd` owns sessions, sign-in and pack partitions (privilege boundary
//! §4, contract §4 and §8). It is not built yet, and its local-socket protocol
//! is not specified anywhere, so `deltd` depends on this trait rather than on
//! a wire format nobody has written. The live implementation is a later step;
//! [`StaticIdentity`] is a fixed table for tests and local development.
//!
//! The rule this interface encodes: **`deltd` learns who the caller is only
//! from `identityd`**, never from a request field (deltd API §2).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde::Deserialize;

use crate::hostname::is_partition;

/// A live signed-in session, as `identityd` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub profile_id: String,
    /// The browser the session lives on (privilege boundary §4.4).
    pub browser_id: String,
    /// The Guest profile: sees only guest-accessible packs, stores nothing.
    pub guest: bool,
    /// Made over HTTPS (privilege boundary §4.1).
    pub secure: bool,
    /// Effective permissions (contract §4), looked up live.
    pub permissions: BTreeSet<String>,
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
