//! Progress events (deltd API §8, contract §5.2): `POST /progress-events`
//! from the WebOS, and `entry_viewed` recorded by `deltd` as it serves a
//! page. They are kept by `deltos-identityd` (contract §10: `identity/`),
//! which takes the person from the live session it is given, assigns the
//! time and order, and refuses Guest.
//!
//! * **The person** always comes from a live session: the API request's
//!   cookie, or, for `entry_viewed`, the session the pack hostname's
//!   partition was issued under. Never from the event.
//! * **The pack** always comes from a pack hostname, never from the event:
//!   the WebOS names the frame the event is about, and that hostname's
//!   partition must be the same person's on the same browser (privilege
//!   boundary §7 rule 8).
//! * **The time** is the box's clock, assigned by `identityd`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Response, StatusCode};
use serde_json::{json, Value};

use crate::api::{invalid, not_signed_in, session_token, Api, Problem};
use crate::hostname::PackHost;
use crate::identity::Identity;
use crate::library::{hex, Pack};
use crate::signin::{body_json, private_json, problem_of, write_allowed, ConnInfo};

/// The one event source a pack's events have (contract §5.2).
pub const WAX_PACK: &str = "wax_pack";

/// Event types the WebOS may send. `entry_viewed` is the box's own
/// (deltd API §8.2); any other type is passed on and stored verbatim, as
/// contract §5.2 asks, so a newer WebOS is never refused.
pub const BOX_ONLY: &[&str] = &["entry_viewed"];

fn rejected() -> Problem {
    Problem::new(StatusCode::FORBIDDEN, "request_rejected", "The request was rejected.")
}

impl Api {
    /// `POST /api/v1/progress-events`: `{event_id, event_type,
    /// pack_hostname, payload?}`.
    pub(crate) async fn progress_events(&self, headers: &HeaderMap, body: &[u8], conn: &ConnInfo) -> Result<Response<Body>, Problem> {
        let token = session_token(headers).ok_or_else(not_signed_in)?;
        let session = self.ident.session_by_token(&token).ok_or_else(not_signed_in)?;
        write_allowed(headers, conn, &self.webos_host, Some(&session))?;
        if session.guest {
            return Err(Problem::new(StatusCode::FORBIDDEN, "guest_not_allowed", "Nothing is recorded for Guest."));
        }
        let b = body_json(headers, body)?;
        let text = |k: &str| b.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| invalid(format!("{k} is required")));
        let event_id = text("event_id")?;
        let event_type = text("event_type")?;
        if BOX_ONLY.contains(&event_type) {
            return Err(invalid(format!("{event_type} is recorded by the box itself")));
        }
        let host = PackHost::parse(text("pack_hostname")?).ok_or_else(|| invalid("pack_hostname is not a pack hostname"))?;
        // the frame must be this person's, on this browser
        let owner = self.ident.session_for_partition(&host.pack_partition).ok_or_else(rejected)?;
        if owner.profile_id != session.profile_id || owner.browser_id != session.browser_id {
            return Err(rejected());
        }
        let snap = self.lib.snapshot();
        let pack = snap
            .by_index(host.pack_index)
            .filter(|p| p.slot.pack_generation == host.pack_generation)
            .ok_or_else(|| Problem::new(StatusCode::NOT_FOUND, "pack_not_found", "No such pack."))?;
        let payload = match b.get("payload") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return Err(invalid("payload must be a JSON object")),
        };
        let req = json!({
            "call": "record_progress",
            "by": { "token": token },
            "event": { "event_source": WAX_PACK, "event_id": event_id, "pack_id": pack.archive_uuid,
                       "event_type": event_type, "payload": payload },
        });
        let ident = Arc::clone(&self.ident);
        let v = tokio::task::spawn_blocking(move || ident.call(&req))
            .await
            .map_err(|_| Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong."))?
            .map_err(problem_of)?;
        Ok(private_json(StatusCode::OK, &json!({ "recorded": v["inserted"], "seq": v["seq"] }), conn, vec![]))
    }
}

/// `entry_viewed` for a page served on `partition`'s pack hostname (deltd
/// API §8.2), sent off the request's path. Nothing is recorded when the
/// partition's session has ended, for Guest, or when there is no
/// `identityd` (the desktop app); a failure loses one event and nothing
/// else.
pub fn record_entry_viewed(ident: &Arc<dyn Identity>, partition: &str, pack: &Pack, path: &str) {
    let mut id = [0u8; 16];
    if getrandom::getrandom(&mut id).is_err() {
        return;
    }
    // contract §5.2's 4 KiB payload: a path that long is left out
    let payload = if path.len() <= 3_000 { json!({ "path": path }) } else { json!({}) };
    let req = json!({
        "call": "record_progress",
        "by": { "partition": partition },
        "event": { "event_source": WAX_PACK, "event_id": hex(&id), "pack_id": pack.archive_uuid,
                   "event_type": "entry_viewed", "payload": payload },
    });
    let ident = Arc::clone(ident);
    tokio::task::spawn_blocking(move || {
        let _ = ident.call(&req);
    });
}
