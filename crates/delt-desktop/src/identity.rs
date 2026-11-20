//! The one person a desktop reader serves.
//!
//! `deltd` asks `identityd` who is signed in (deltlib [`Identity`]). On a
//! desktop there is no sign-in: the computer's own account is the person,
//! and nothing listens on the network, so the answer is fixed. This is the
//! desktop's equivalent of the box's session lookup, not a development
//! table: it is the real and only identity a desktop reader has.
//!
//! The session token and pack partition are random per install and never
//! leave the process (the window reaches the API through in-process calls,
//! never a cookie).

use std::collections::BTreeSet;

use deltlib::{Identity, Session};

/// The desktop's single, always-signed-in owner.
pub struct LocalIdentity {
    token: String,
    partition: String,
    session: Session,
}

impl LocalIdentity {
    /// `partition` must be 16 lowercase hex characters (deltd API §4.4),
    /// which [`random_hex`] produces.
    pub fn new(token: String, partition: String) -> Self {
        LocalIdentity {
            token,
            partition,
            session: Session {
                profile_id: "desktop-owner".into(),
                browser_id: "desktop-window".into(),
                guest: false,
                // in-process calls never cross a network, so nothing can
                // overhear them; the box's HTTPS rule has no wire to apply to
                secure: true,
                // reading only: the desktop API surface is the Library and
                // Search groups, which need a session and nothing more
                permissions: BTreeSet::new(),
            },
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn partition(&self) -> &str {
        &self.partition
    }
}

impl Identity for LocalIdentity {
    fn session_by_token(&self, token: &str) -> Option<Session> {
        (token == self.token).then(|| self.session.clone())
    }

    fn session_for_partition(&self, partition: &str) -> Option<Session> {
        (partition == self.partition).then(|| self.session.clone())
    }

    fn partition_for(&self, _session: &Session) -> Option<String> {
        Some(self.partition.clone())
    }
}

/// `n` random bytes as lowercase hex.
pub fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("the operating system's random source");
    deltlib::library::hex(&b)
}
