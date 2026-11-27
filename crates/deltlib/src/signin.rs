//! Sign-in and the people endpoints (deltd API §6.1), forwarded to
//! `identityd`, plus the write rules every state-changing request passes
//! (deltd API §3 rule 6, privilege boundary §7 rule 7).
//!
//! `deltd` adds only what it alone knows: whether the request came over
//! HTTPS, whether it came from the box's own screen, the device's address,
//! and the cookies the browser sent. Every decision about a person is
//! `identityd`'s (docs/identityd.md §4).

use std::net::IpAddr;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode};
use serde_json::{json, Value};

use crate::api::{api_headers, cookie, Api, Problem};
use crate::identity::{CallError, Session};

/// How the request reached `deltd`. Set per connection by the listener;
/// a request with none is plain HTTP from nowhere in particular.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnInfo {
    pub https: bool,
    pub peer: Option<IpAddr>,
}

impl ConnInfo {
    /// The box's own screen: a browser on the box itself, whose connection
    /// comes from loopback (docs/identityd.md §5: the kiosk resolves
    /// `home.deltos.lan` to 127.0.0.1).
    pub fn kiosk(&self) -> bool {
        self.peer.is_some_and(|p| p.is_loopback() || p.to_canonical().is_loopback())
    }
}

const SESSION: (&str, &str) = ("delt_session", "__Host-delt_session");
const BROWSER: (&str, &str) = ("delt_browser", "__Host-delt_browser");
const DEVICE: (&str, &str) = ("delt_device", "__Host-delt_device");

/// The browser id cookie lives 400 days, the longest browsers keep one.
const BROWSER_MAX_AGE: i64 = 400 * 86400;

pub(crate) fn problem_of(e: CallError) -> Problem {
    match e {
        CallError::Unavailable(d) => {
            Problem::new(StatusCode::SERVICE_UNAVAILABLE, "sign_in_unavailable", "Signing in is not available right now.")
                .detail(d)
        }
        CallError::Refused { code, detail } => {
            let (status, code, title): (StatusCode, &'static str, &'static str) = match code.as_str() {
                "invalid_request" => (StatusCode::BAD_REQUEST, "invalid_request", "The request is not valid."),
                "not_signed_in" => (StatusCode::UNAUTHORIZED, "not_signed_in", "Please sign in."),
                "sign_in_failed" => (StatusCode::UNAUTHORIZED, "sign_in_failed", "That did not work. Check and try again."),
                "permission_denied" => (StatusCode::FORBIDDEN, "permission_denied", "You are not allowed to do that."),
                "guest_not_allowed" => (StatusCode::FORBIDDEN, "guest_not_allowed", "Not available to Guest."),
                "secure_connection_required" => (
                    StatusCode::FORBIDDEN,
                    "secure_connection_required",
                    "This needs a secure (HTTPS) connection, from a device with the box's certificate.",
                ),
                "confirmation_required" => {
                    (StatusCode::FORBIDDEN, "confirmation_required", "Please enter your password again.")
                }
                "too_many_requests" => {
                    (StatusCode::TOO_MANY_REQUESTS, "too_many_requests", "Too many tries. Wait a few minutes.")
                }
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong."),
            };
            let p = Problem::new(status, code, title);
            match detail {
                Some(d) => p.detail(d),
                None => p,
            }
        }
    }
}

fn rejected() -> Problem {
    Problem::new(StatusCode::FORBIDDEN, "request_rejected", "The request was refused.")
}

fn invalid(d: &str) -> Problem {
    Problem::new(StatusCode::BAD_REQUEST, "invalid_request", "The request is not valid.").detail(d.to_string())
}

/// A cookie's value by its plain or `__Host-` name, preferring the `__Host-`
/// one (only an HTTPS response can have set it).
fn read_cookie(headers: &HeaderMap, names: (&str, &str)) -> Option<String> {
    let mut plain = None;
    for v in headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()) {
        for kv in v.split(';') {
            let Some((k, val)) = kv.trim().split_once('=') else { continue };
            let k = k.trim();
            if k == names.1 {
                return Some(val.trim().to_string());
            }
            if k == names.0 {
                plain = Some(val.trim().to_string());
            }
        }
    }
    plain
}

/// Rule 7: a write must come from the WebOS's own origin, for this
/// transport, exactly; and, once signed in, carry the session's token.
pub fn write_allowed(headers: &HeaderMap, conn: &ConnInfo, webos_host: &str, session: Option<&Session>) -> Result<(), Problem> {
    let want = format!("{}://{}", if conn.https { "https" } else { "http" }, webos_host);
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin != Some(want.as_str()) {
        return Err(rejected());
    }
    if let Some(s) = session {
        let sent = headers.get("x-delt-csrf").and_then(|v| v.to_str().ok()).unwrap_or("");
        if s.csrf.is_empty() || !constant_eq(sent.as_bytes(), s.csrf.as_bytes()) {
            return Err(rejected());
        }
    }
    Ok(())
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The JSON body, if any. A body must be `application/json` (rule 3).
pub(crate) fn body_json(headers: &HeaderMap, body: &[u8]) -> Result<Value, Problem> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let ct = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    if ct.split(';').next().map(str::trim) != Some("application/json") {
        return Err(invalid("a request body must be application/json"));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(invalid("the body is not a JSON object")),
    }
}

/// What a browser sees about its own session: never the browser id.
fn session_json(v: &Value) -> Value {
    json!({
        "profile_id": v["profile_id"],
        "display_name": v["display_name"],
        "kind": v["kind"],
        "guest": v["guest"],
        "secure": v["secure"],
        "kiosk": v["kiosk"],
        "expires_at": v["expires_at"],
        "idle_seconds": v["idle_seconds"],
        "all_permissions": v["all_permissions"],
        "permissions": v["permissions"],
        "csrf_token": v["csrf"],
    })
}

pub(crate) fn private_json(status: StatusCode, v: &Value, conn: &ConnInfo, cookies: Vec<String>) -> Response<Body> {
    let mut r = Response::builder().status(status);
    let h = r.headers_mut().unwrap();
    api_headers(h);
    if conn.https {
        crate::api::hsts(h);
    }
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    for c in cookies {
        if let Ok(v) = HeaderValue::from_str(&c) {
            h.append(header::SET_COOKIE, v);
        }
    }
    r.body(Body::from(v.to_string())).unwrap()
}

impl Api {
    pub(crate) fn ctx(&self, headers: &HeaderMap, conn: &ConnInfo) -> Value {
        json!({
            "https": conn.https,
            "kiosk": conn.kiosk(),
            "peer": conn.peer.map(|p| p.to_string()).unwrap_or_default(),
            "browser": read_cookie(headers, BROWSER),
            "session": read_cookie(headers, SESSION),
        })
    }

    fn forward(&self, req: Value) -> Result<Value, Problem> {
        self.ident.call(&req).map_err(problem_of)
    }

    /// Run a forwarded call off the async threads: a password check takes
    /// tens of milliseconds on purpose.
    async fn forward_blocking(&self, req: Value) -> Result<Value, Problem> {
        let ident = std::sync::Arc::clone(&self.ident);
        tokio::task::spawn_blocking(move || ident.call(&req).map_err(problem_of))
            .await
            .map_err(|_| Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong."))?
    }

    /// The cookies a successful sign-in sets.
    fn signed_in_cookies(&self, v: &Value, conn: &ConnInfo) -> Vec<String> {
        let kiosk = v["session"]["kiosk"].as_bool().unwrap_or(false);
        let secure = v["session"]["secure"].as_bool().unwrap_or(false);
        let session_age = (!kiosk).then(|| deltos_identity::policy::session_limits(kiosk, secure).1);
        let mut c = vec![
            cookie(SESSION, conn.https, v["session_token"].as_str().unwrap_or(""), session_age),
            cookie(BROWSER, conn.https, v["browser_token"].as_str().unwrap_or(""), Some(BROWSER_MAX_AGE)),
        ];
        if let Some(d) = v["device_token"].as_str() {
            c.push(cookie(DEVICE, conn.https, d, Some(deltos_identity::policy::DEVICE_ABSOLUTE)));
        }
        c
    }

    /// Answer a sign-in or people endpoint, or `None` if the path is not one.
    #[allow(clippy::too_many_arguments)]
    pub async fn people(
        &self,
        method: &Method,
        segs: &[&str],
        query: &std::collections::BTreeMap<String, String>,
        headers: &HeaderMap,
        body: &[u8],
        conn: &ConnInfo,
    ) -> Option<Response<Body>> {
        let known = matches!(
            segs,
            ["session"]
                | ["sign-in", "profiles"]
                | ["sign-in", "check"]
                | ["me", "sessions"]
                | ["me", "device"]
                | ["me", "password"]
                | ["profiles"]
                | ["profiles", _, "password"]
                | ["classes", _, "members"]
                | ["classes", _, "members", _, "sign-in-card"]
                | ["classes", _, "members", _, "devices"]
                | ["classes", _, "members", _, "devices", _]
        );
        if !known {
            return None;
        }
        Some(match self.people_inner(method, segs, query, headers, body, conn).await {
            Ok(r) => r,
            Err(p) => {
                let mut r = p.response();
                if conn.https {
                    crate::api::hsts(r.headers_mut());
                }
                r
            }
        })
    }

    async fn people_inner(
        &self,
        method: &Method,
        segs: &[&str],
        query: &std::collections::BTreeMap<String, String>,
        headers: &HeaderMap,
        body: &[u8],
        conn: &ConnInfo,
    ) -> Result<Response<Body>, Problem> {
        let token = read_cookie(headers, SESSION);
        let ctx = self.ctx(headers, conn);
        // the session the write rules check against (None before sign-in)
        let session = token.as_deref().and_then(|t| self.ident.session_by_token(t));
        let ok = |v: Value| private_json(StatusCode::OK, &v, conn, vec![]);
        let need_token = || token.clone().ok_or_else(|| Problem::new(StatusCode::UNAUTHORIZED, "not_signed_in", "Please sign in."));

        match (method.clone(), segs) {
            (Method::GET | Method::HEAD, ["session"]) => {
                let Some(t) = token.clone() else {
                    return Err(Problem::new(StatusCode::UNAUTHORIZED, "not_signed_in", "Please sign in."));
                };
                let v = self.forward(json!({ "call": "session", "token": t }))?;
                if v.is_null() {
                    return Err(Problem::new(StatusCode::UNAUTHORIZED, "not_signed_in", "Please sign in."));
                }
                Ok(ok(session_json(&v)))
            }
            (Method::POST, ["session"]) => {
                // Sign-in is checked like every write (rule 7), but has no
                // session yet, so no anti-forgery token to compare.
                write_allowed(headers, conn, &self.webos_host, None)?;
                let b = body_json(headers, body)?;
                let req = if let Some(code) = b.get("sign_in_code").and_then(Value::as_str) {
                    json!({ "call": "sign_in_card", "ctx": ctx, "code": code })
                } else if let Some(name) = b.get("sign_in_name").and_then(Value::as_str) {
                    let pw = b.get("password").and_then(Value::as_str).unwrap_or("");
                    json!({ "call": "sign_in_password", "ctx": ctx, "sign_in_name": name, "password": pw })
                } else if let Some(pid) = b.get("profile_id").and_then(Value::as_str) {
                    json!({ "call": "sign_in_tap", "ctx": ctx, "profile_id": pid })
                } else if let Some(d) = read_cookie(headers, DEVICE) {
                    json!({ "call": "sign_in_device", "ctx": ctx, "device": d })
                } else {
                    return Err(invalid("give sign_in_name and password, sign_in_code, or profile_id (kiosk)"));
                };
                let v = self.forward_blocking(req).await?;
                let cookies = self.signed_in_cookies(&v, conn);
                Ok(private_json(StatusCode::OK, &session_json(&v["session"]), conn, cookies))
            }
            (Method::DELETE, ["session"]) => {
                write_allowed(headers, conn, &self.webos_host, session.as_ref())?;
                let forget = query.get("forget_device").is_some_and(|v| v == "true");
                let device = forget.then(|| read_cookie(headers, DEVICE)).flatten();
                self.forward(json!({ "call": "sign_out", "token": need_token()?, "forget_device": device }))?;
                let mut c = vec![cookie(SESSION, conn.https, "", Some(0))];
                if forget {
                    c.push(cookie(DEVICE, conn.https, "", Some(0)));
                }
                Ok(private_json(StatusCode::OK, &json!({ "signed_out": true }), conn, c))
            }
            (Method::DELETE, ["me", "device"]) => {
                // Forgetting this device also ends the session it started.
                write_allowed(headers, conn, &self.webos_host, session.as_ref())?;
                let device = read_cookie(headers, DEVICE);
                self.forward(json!({ "call": "sign_out", "token": need_token()?, "forget_device": device }))?;
                let c = vec![cookie(SESSION, conn.https, "", Some(0)), cookie(DEVICE, conn.https, "", Some(0))];
                Ok(private_json(StatusCode::OK, &json!({ "forgotten": true }), conn, c))
            }
            (Method::DELETE, ["me", "sessions"]) => {
                write_allowed(headers, conn, &self.webos_host, session.as_ref())?;
                self.forward(json!({ "call": "sign_out_everywhere", "token": need_token()? }))?;
                Ok(private_json(StatusCode::OK, &json!({ "signed_out": true }), conn, vec![cookie(SESSION, conn.https, "", Some(0))]))
            }
            (Method::GET | Method::HEAD, ["sign-in", "profiles"]) => {
                let v = self.forward(json!({ "call": "tap_list", "ctx": ctx }))?;
                Ok(ok(json!({ "items": v, "next": null })))
            }
            (Method::GET | Method::HEAD, ["sign-in", "check"]) => {
                let name = query.get("sign_in_name").ok_or_else(|| invalid("sign_in_name is required"))?;
                self.forward(json!({ "call": "password_allowed", "ctx": ctx, "sign_in_name": name }))?;
                Ok(ok(json!({ "password_allowed": true })))
            }
            (Method::PUT, ["me", "password"]) => {
                write_allowed(headers, conn, &self.webos_host, session.as_ref())?;
                let b = body_json(headers, body)?;
                let req = json!({ "call": "change_password", "ctx": ctx, "token": need_token()?,
                                  "current": b.get("current_password"), "new": b.get("new_password") });
                self.forward_blocking(req).await?;
                Ok(private_json(StatusCode::OK, &json!({ "changed": true, "signed_out": true }), conn, vec![cookie(SESSION, conn.https, "", Some(0))]))
            }
            (Method::POST, ["profiles"]) => {
                write_allowed(headers, conn, &self.webos_host, session.as_ref())?;
                let b = body_json(headers, body)?;
                let id = self
                    .forward_blocking(json!({ "call": "create_profile", "ctx": ctx, "token": need_token()?, "profile": b }))
                    .await?;
                Ok(private_json(StatusCode::CREATED, &json!({ "profile_id": id }), conn, vec![]))
            }
            (Method::PUT, ["profiles", pid, "password"]) => {
                write_allowed(headers, conn, &self.webos_host, session.as_ref())?;
                let b = body_json(headers, body)?;
                let req = json!({ "call": "set_password", "ctx": ctx, "token": need_token()?,
                                  "profile_id": pid, "new": b.get("new_password") });
                self.forward_blocking(req).await?;
                Ok(ok(json!({ "changed": true })))
            }
            (Method::GET | Method::HEAD, ["classes", class, "members"]) => {
                let v = self.forward(json!({ "call": "list_members", "ctx": ctx, "token": need_token()?, "class": class }))?;
                Ok(ok(json!({ "items": v, "next": null })))
            }
            (Method::POST, ["classes", class, "members", pid, "sign-in-card"]) => {
                write_allowed(headers, conn, &self.webos_host, session.as_ref())?;
                let v = self.forward(json!({ "call": "issue_card", "ctx": ctx, "token": need_token()?,
                                             "profile_id": pid, "class": class }))?;
                Ok(private_json(StatusCode::CREATED, &v, conn, vec![]))
            }
            (Method::GET | Method::HEAD, ["classes", class, "members", pid, "devices"]) => {
                let v = self.forward(json!({ "call": "list_devices", "ctx": ctx, "token": need_token()?,
                                             "profile_id": pid, "class": class }))?;
                Ok(ok(json!({ "items": v, "next": null })))
            }
            (Method::DELETE, ["classes", class, "members", pid, "devices", device]) => {
                write_allowed(headers, conn, &self.webos_host, session.as_ref())?;
                self.forward(json!({ "call": "remove_device", "ctx": ctx, "token": need_token()?,
                                     "profile_id": pid, "class": class, "device_id": device }))?;
                Ok(ok(json!({ "removed": true })))
            }
            _ => Err(Problem::new(StatusCode::METHOD_NOT_ALLOWED, "invalid_request", "Method not allowed.")),
        }
    }
}
