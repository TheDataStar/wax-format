//! shelld's own questions to `identityd` (privilege boundary §5 step 6, §6):
//! it never takes the box API's word for who the person is.
//!
//! `identityd` lets shelld make three calls and nothing else (`session`,
//! `has_permission`, `redeem_ticket`; `deltos_identity::protocol::SHELLD_CALLS`),
//! and checks shelld's user id on the connection.

use std::cell::{Cell, RefCell};

use deltos_identity::{CallError, Client};
use deltos_net::actions::IdentityCheck;
use serde_json::json;

/// One request's questions about one session.
pub struct Ask<'a> {
    client: &'a Client,
    /// The person the session belongs to, from the first `session` answer.
    /// A permission question about anyone else is answered no.
    profile: RefCell<Option<String>>,
    token: RefCell<Option<String>>,
    unavailable: Cell<bool>,
}

impl<'a> Ask<'a> {
    pub fn new(client: &'a Client) -> Self {
        Ask { client, profile: RefCell::new(None), token: RefCell::new(None), unavailable: Cell::new(false) }
    }

    /// `identityd` could not be reached: a "not signed in" refusal is then
    /// reported as `sign_in_unavailable` instead.
    pub fn was_unavailable(&self) -> bool {
        self.unavailable.get()
    }

    fn call(&self, req: serde_json::Value) -> Option<serde_json::Value> {
        match self.client.call(&req) {
            Ok(v) => Some(v),
            Err(CallError::Unavailable(_)) => {
                self.unavailable.set(true);
                None
            }
            Err(CallError::Refused { .. }) => None,
        }
    }
}

impl IdentityCheck for Ask<'_> {
    fn session(&self, session_ref: &str) -> Option<(String, bool)> {
        let v = self.call(json!({ "call": "session", "token": session_ref }))?;
        let profile = v.get("profile_id")?.as_str()?.to_string();
        let secure = v.get("secure").and_then(|s| s.as_bool()).unwrap_or(false);
        self.profile.borrow_mut().replace(profile.clone());
        // the token itself is what has_permission asks about
        self.token.borrow_mut().replace(session_ref.to_string());
        Some((profile, secure))
    }

    fn has_permission(&self, profile_id: &str, permission: &str) -> bool {
        if self.profile.borrow().as_deref() != Some(profile_id) {
            return false;
        }
        let Some(token) = self.token.borrow().clone() else { return false };
        // box-wide: no action on the list is scoped to one class yet
        self.call(json!({ "call": "has_permission", "token": token, "permission": permission }))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    fn redeem_ticket(&self, ticket: &str, session_ref: &str, action: &str, args_digest: &str) -> bool {
        self.call(json!({ "call": "redeem_ticket", "token": session_ref, "ticket": ticket, "action": action, "args_digest": args_digest }))
            .is_some()
    }
}
