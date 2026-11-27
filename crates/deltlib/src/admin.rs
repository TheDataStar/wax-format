//! The admin endpoints (deltd API §6.8): the Admin Dashboard's way to change
//! the box. `deltd` never changes the box itself. Each endpoint checks the
//! request, then hands one action from the closed list to `deltos-shelld`,
//! which checks it all again with `identityd` before doing anything
//! (privilege boundary §5).
//!
//! What `deltd` checks before it asks shelld anything:
//!
//! * a live session, from the cookie (never from a field);
//! * for a write, the exact `Origin` and the anti-forgery token (rule 7);
//! * **Secure**: an HTTPS request *and* a session made over HTTPS (§4.1);
//! * the permission the action names, held by the person (contract §4).
//!   shelld itself says which permission an action needs (`describe`), so
//!   the two sides can never disagree on it.
//!
//! An action that asks again (privilege boundary §6) takes the person's
//! password in `confirm_password`. `deltd` has `identityd` turn it into a
//! one-use ticket bound to the action and its exact arguments, and sends
//! shelld the ticket, never the password.
//!
//! The action arguments are passed on as the body gives them, with the path's
//! own values added: shelld reads them into each action's fixed shape and
//! refuses anything extra, so `deltd` keeps no second copy of those shapes.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode};
use serde_json::{json, Map, Value};

use crate::api::{invalid, not_signed_in, session_token, Api, Problem};
use crate::identity::{CallError, Session};
use crate::signin::{body_json, private_json, write_allowed, ConnInfo};

/// The key holder, as `deltd` sees it: one JSON call, one answer
/// (`docs/shelld.md` §3).
pub trait Shell: Send + Sync {
    fn call(&self, req: &Value) -> Result<Value, CallError>;
}

/// The live `deltos-shelld`, over its local socket. Same wire format as
/// `identityd`'s, so the same client.
pub struct ShelldLink(deltos_identity::Client);

impl ShelldLink {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        // shelld runs one action at a time; two connections let a job's
        // state be read while an action is being answered
        ShelldLink(deltos_identity::Client::with_connections(path, 2))
    }
}

impl Shell for ShelldLink {
    fn call(&self, req: &Value) -> Result<Value, CallError> {
        self.0.call(req)
    }
}

/// The field a re-confirm action's password comes in (deltd API §6.8).
pub const CONFIRM_FIELD: &str = "confirm_password";

/// What one endpoint asks shelld for.
enum Route {
    /// An action, with the path's values to add to the body's arguments.
    Act(&'static str, Vec<(&'static str, Value)>),
    /// A listing, and the permission `deltd` checks first.
    List(&'static str, &'static str),
    Job(String),
}

/// The endpoints of §6.8 that shelld has an action for. `None`: not one of
/// these paths at all. `Some(Err)`: the path is, the method is not (its
/// allowed methods).
fn route(method: &Method, segs: &[&str]) -> Option<Result<Route, &'static str>> {
    use Route::*;
    // a path value as the browser meant it (`aa%3Abb…` is a MAC address)
    let s = |v: &str| Value::String(percent_encoding::percent_decode_str(v).decode_utf8_lossy().into_owned());
    let m = method.as_str();
    let r = match (segs, m) {
        // packs (docs/shelld.md §7)
        (["installs"], "GET") => List("packs", "content.install"),
        (["installs"], "POST") => Act("install_pack", vec![]),
        (["installs"], _) => return Some(Err("GET, POST")),
        (["installs", "sources"], "GET") => List("pack_sources", "content.install"),
        (["installs", "sources"], _) => return Some(Err("GET")),
        (["packs", id], "DELETE") => Act("remove_pack", vec![("archive_uuid", s(id))]),
        (["packs", id, "rollback"], "POST") => Act("roll_back_pack", vec![("archive_uuid", s(id))]),
        (["packs", _, "rollback"], _) => return Some(Err("POST")),
        // drives and backups (docs/box-backup.md)
        (["drives"], "GET") => List("drives", "backup.manage"),
        (["drives"], _) => return Some(Err("GET")),
        (["backups"], "POST") => Act("start_backup", vec![]),
        (["backups"], _) => return Some(Err("POST")),
        (["backups", id, "review"], "POST") => Act("review_backup", vec![("backup_id", s(id))]),
        (["backups", _, "restore"], "POST") => Act("restore_backup", vec![]),
        (["backups", _, "review" | "restore"], _) => return Some(Err("POST")),
        // the box's network (docs/box-network-plan.md §7, docs/box-radio-and-boot.md)
        (["network", "leases"], "GET") => List("leases", "network.configure"),
        (["network", "leases"], _) => return Some(Err("GET")),
        (["network", "hotspot"], "PUT") => Act("configure_ap", vec![]),
        (["network", "hotspot"], _) => return Some(Err("PUT")),
        (["network", "names"], "POST") => Act("approve_name", vec![]),
        (["network", "names"], _) => return Some(Err("POST")),
        (["network", "names", name], "DELETE") => Act("remove_name", vec![("name", s(name))]),
        (["network", "names", _], _) => return Some(Err("DELETE")),
        (["network", "reservations", mac], "PUT") => Act("set_address_reservation", vec![("mac", s(mac))]),
        (["network", "reservations", mac], "DELETE") => Act("remove_address_reservation", vec![("mac", s(mac))]),
        (["network", "reservations", _], _) => return Some(Err("PUT, DELETE")),
        (["network", "uplink-sharing"], "PUT") => Act("set_uplink_sharing", vec![]),
        (["network", "uplink-sharing"], _) => return Some(Err("PUT")),
        // add-ons (docs/base-addons-admin.md §6, docs/shelld.md §4)
        (["apps"], "GET") => List("addons", "apps.manage"),
        (["apps"], "POST") => Act("install_addon", vec![]),
        (["apps"], _) => return Some(Err("GET, POST")),
        (["apps", "sources"], "GET") => List("addon_sources", "apps.manage"),
        (["apps", "sources"], _) => return Some(Err("GET")),
        (["apps", id], "DELETE") => Act("remove_addon", vec![("id", s(id))]),
        (["apps", _], _) => return Some(Err("DELETE")),
        (["apps", id, "update"], "POST") => Act("update_addon", vec![("id", s(id))]),
        (["apps", id, "rollback"], "POST") => Act("roll_back_addon", vec![("id", s(id))]),
        (["apps", _, "update" | "rollback"], _) => return Some(Err("POST")),
        (["apps", id, "running"], "PUT") => Act("set_addon_running", vec![("id", s(id))]),
        (["apps", _, "running"], _) => return Some(Err("PUT")),
        // the action log, and jobs
        (["audit"], "GET") => List("action_log", "admin.console"),
        (["audit"], _) => return Some(Err("GET")),
        (["jobs", id], "GET") => Job(id.to_string()),
        (["jobs", _], _) => return Some(Err("GET")),
        _ => return None,
    };
    Some(Ok(r))
}

fn unavailable() -> Problem {
    Problem::new(StatusCode::SERVICE_UNAVAILABLE, "admin_unavailable", "Changing the box is not available right now.")
}

fn secure_required() -> Problem {
    Problem::new(
        StatusCode::FORBIDDEN,
        "secure_connection_required",
        "This needs a secure (HTTPS) connection, from a device with the box's certificate.",
    )
}

fn denied() -> Problem {
    Problem::new(StatusCode::FORBIDDEN, "permission_denied", "You are not allowed to do that.")
}

fn holds(s: &Session, permission: &str) -> bool {
    s.permissions.contains("*") || s.permissions.contains(permission)
}

/// shelld's and identityd's error codes, as deltd API §7's closed list.
/// A code that is not on the list is reported as `internal`, with the
/// code kept in the detail.
pub(crate) fn problem_of(e: CallError) -> Problem {
    let (code, detail) = match e {
        CallError::Unavailable(d) => return unavailable().detail(d),
        CallError::Refused { code, detail } => (code, detail),
    };
    let (status, code, title): (StatusCode, &'static str, &'static str) = match code.as_str() {
        "invalid_request" => (StatusCode::BAD_REQUEST, "invalid_request", "The request is not valid."),
        "not_signed_in" => (StatusCode::UNAUTHORIZED, "not_signed_in", "Please sign in."),
        "permission_denied" => (StatusCode::FORBIDDEN, "permission_denied", "You are not allowed to do that."),
        "secure_connection_required" => return with(secure_required(), detail),
        "confirmation_required" => (StatusCode::FORBIDDEN, "confirmation_required", "Please enter your password again."),
        "too_many_requests" => (StatusCode::TOO_MANY_REQUESTS, "too_many_requests", "Too many tries. Wait a few minutes."),
        "sign_in_unavailable" => {
            (StatusCode::SERVICE_UNAVAILABLE, "sign_in_unavailable", "Signing in is not available right now.")
        }
        "pack_not_found" => (StatusCode::NOT_FOUND, "pack_not_found", "No such pack."),
        "app_not_found" => (StatusCode::NOT_FOUND, "app_not_found", "No such add-on."),
        "signature_invalid" => (StatusCode::UNPROCESSABLE_ENTITY, "signature_invalid", "The signature check failed."),
        "does_not_fit" => (StatusCode::CONFLICT, "does_not_fit", "This does not fit on this box."),
        "edition_mismatch" => (StatusCode::CONFLICT, "edition_mismatch", "This update does not build on the edition installed."),
        "edition_older" => (StatusCode::CONFLICT, "edition_older", "This is an older edition than the box has had."),
        "not_enough_resources" => {
            (StatusCode::SERVICE_UNAVAILABLE, "not_enough_resources", "This box does not have what this needs.")
        }
        "not_enough_storage" => (StatusCode::INSUFFICIENT_STORAGE, "not_enough_storage", "There is not enough space."),
        "backup_unreadable" => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "backup_unreadable",
            "The passphrase is wrong, or this is not a DeltOS backup.",
        ),
        "backup_damaged" => (StatusCode::UNPROCESSABLE_ENTITY, "backup_damaged", "This backup is damaged."),
        "backup_too_new" => (StatusCode::CONFLICT, "backup_too_new", "This backup is from newer DeltOS than this box runs."),
        "backup_not_found" => (StatusCode::NOT_FOUND, "backup_not_found", "No such backup or review."),
        "restore_failed" => {
            (StatusCode::INTERNAL_SERVER_ERROR, "restore_failed", "The restore did not finish; the box was put back as it was.")
        }
        "internal" => (StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong."),
        other => {
            let d = format!("{other}: {}", detail.unwrap_or_default());
            return Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong.").detail(d);
        }
    };
    with(Problem::new(status, code, title), detail)
}

fn with(p: Problem, detail: Option<String>) -> Problem {
    match detail {
        Some(d) => p.detail(d),
        None => p,
    }
}

impl Api {
    /// Answer an admin endpoint, or `None` when the path is not one.
    pub(crate) async fn admin(
        &self,
        method: &Method,
        segs: &[&str],
        headers: &HeaderMap,
        body: &[u8],
        conn: &ConnInfo,
    ) -> Option<Response<Body>> {
        let r = match route(method, segs)? {
            Ok(r) => r,
            Err(allow) => {
                let mut r = Problem::new(StatusCode::METHOD_NOT_ALLOWED, "invalid_request", "Method not allowed.").response();
                r.headers_mut().insert(header::ALLOW, HeaderValue::from_static(allow));
                return Some(r);
            }
        };
        Some(self.admin_inner(method, r, headers, body, conn).await.unwrap_or_else(Problem::response))
    }

    async fn admin_inner(&self, method: &Method, r: Route, headers: &HeaderMap, body: &[u8], conn: &ConnInfo) -> Result<Response<Body>, Problem> {
        let token = session_token(headers).ok_or_else(not_signed_in)?;
        let session = self.ident.session_by_token(&token).ok_or_else(not_signed_in)?;
        if method != Method::GET && method != Method::HEAD {
            write_allowed(headers, conn, &self.webos_host, Some(&session))?;
        }
        if !conn.https || !session.secure {
            return Err(secure_required());
        }
        let shell = self.shell.clone().ok_or_else(unavailable)?;
        let ask = |req: Value| {
            let shell = Arc::clone(&shell);
            async move {
                tokio::task::spawn_blocking(move || shell.call(&req))
                    .await
                    .map_err(|_| Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong."))?
                    .map_err(problem_of)
            }
        };
        match r {
            Route::List(listing, permission) => {
                if !holds(&session, permission) {
                    return Err(denied());
                }
                let v = ask(json!({ "call": "list", "listing": listing, "session": token, "over_https": true })).await?;
                Ok(private_json(StatusCode::OK, &v, conn, vec![]))
            }
            Route::Job(id) => {
                let v = ask(json!({ "call": "job", "job": id, "session": token, "over_https": true })).await;
                // another person's job, or none: the same answer
                let v = v.map_err(|p| if p.code() == "invalid_request" { job_not_found() } else { p })?;
                Ok(private_json(StatusCode::OK, &v, conn, vec![]))
            }
            Route::Act(action, from_path) => {
                let mut args = match body_json(headers, body)? {
                    Value::Object(m) => m,
                    _ => Map::new(),
                };
                let password = match args.remove(CONFIRM_FIELD) {
                    None | Some(Value::Null) => None,
                    Some(Value::String(p)) => Some(p),
                    Some(_) => return Err(invalid(format!("{CONFIRM_FIELD} must be text"))),
                };
                for (k, v) in from_path {
                    if args.contains_key(k) {
                        return Err(invalid(format!("{k} comes from the path, not the body")));
                    }
                    args.insert(k.to_string(), v);
                }
                let args = Value::Object(args);
                let d = ask(json!({ "call": "describe", "action": action, "args": args })).await?;
                let permission = d["permission"].as_str().unwrap_or("");
                if permission.is_empty() || !holds(&session, permission) {
                    return Err(denied());
                }
                let mut req = json!({ "call": "act", "action": action, "args": args, "session": token, "over_https": true });
                if d["needs_reconfirm"] == true {
                    let pw = password.filter(|p| !p.is_empty()).ok_or_else(|| {
                        Problem::new(StatusCode::FORBIDDEN, "confirmation_required", "Please enter your password again.")
                    })?;
                    let ticket = self
                        .identity_call(json!({
                            "call": "issue_ticket", "ctx": self.ctx(headers, conn), "token": token, "password": pw,
                            "action": action, "args_digest": d["args_digest"],
                        }))
                        .await?;
                    req["ticket"] = ticket;
                }
                let v = ask(req).await?;
                match v.get("job").and_then(Value::as_str) {
                    Some(job) if d["long"] == true => {
                        let mut r = private_json(StatusCode::ACCEPTED, &json!({ "job": job }), conn, vec![]);
                        if let Ok(l) = HeaderValue::from_str(&format!("/api/v1/jobs/{job}")) {
                            r.headers_mut().insert(header::LOCATION, l);
                        }
                        Ok(r)
                    }
                    _ => Ok(private_json(StatusCode::OK, &v, conn, vec![])),
                }
            }
        }
    }

    async fn identity_call(&self, req: Value) -> Result<Value, Problem> {
        let ident = Arc::clone(&self.ident);
        tokio::task::spawn_blocking(move || ident.call(&req))
            .await
            .map_err(|_| Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong."))?
            .map_err(problem_of)
    }
}

fn job_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "job_not_found", "No such job.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_names_a_built_action_or_listing() {
        let built = [
            "install_pack", "remove_pack", "roll_back_pack", "start_backup", "review_backup", "restore_backup",
            "configure_ap", "approve_name", "remove_name", "set_address_reservation", "remove_address_reservation",
            "set_uplink_sharing", "install_addon", "update_addon", "roll_back_addon", "remove_addon", "set_addon_running",
        ];
        let paths: &[(&str, &[&str])] = &[
            ("POST", &["installs"]),
            ("DELETE", &["packs", "x"]),
            ("POST", &["packs", "x", "rollback"]),
            ("POST", &["backups"]),
            ("POST", &["backups", "x", "review"]),
            ("POST", &["backups", "x", "restore"]),
            ("PUT", &["network", "hotspot"]),
            ("POST", &["network", "names"]),
            ("DELETE", &["network", "names", "x"]),
            ("PUT", &["network", "reservations", "x"]),
            ("DELETE", &["network", "reservations", "x"]),
            ("PUT", &["network", "uplink-sharing"]),
            ("POST", &["apps"]),
            ("POST", &["apps", "x", "update"]),
            ("POST", &["apps", "x", "rollback"]),
            ("DELETE", &["apps", "x"]),
            ("PUT", &["apps", "x", "running"]),
        ];
        let mut seen: Vec<&str> = paths
            .iter()
            .map(|(m, p)| match route(&Method::from_bytes(m.as_bytes()).unwrap(), p) {
                Some(Ok(Route::Act(a, _))) => a,
                _ => panic!("{m} {p:?} is not an action"),
            })
            .collect();
        seen.sort();
        let mut want = built.to_vec();
        want.sort();
        assert_eq!(seen, want, "one endpoint per built action");
        assert!(route(&Method::GET, &["packs", "x"]).is_none(), "reading a pack stays the library's");
        assert!(matches!(route(&Method::GET, &["network", "hotspot"]), Some(Err("PUT"))));
    }

    #[test]
    fn unknown_codes_never_leak_as_codes() {
        let p = problem_of(CallError::Refused { code: "made_up".into(), detail: Some("x".into()) });
        assert_eq!(p.code(), "internal");
        assert_eq!(problem_of(CallError::Unavailable("gone".into())).code(), "admin_unavailable");
        assert_eq!(problem_of(CallError::Refused { code: "backup_unreadable".into(), detail: None }).code(), "backup_unreadable");
    }
}
