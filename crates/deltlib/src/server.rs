//! `deltd`'s front door: one listener, routed by `Host` (Track A §20.3), so
//! pack isolation holds at every profile, with or without a reverse proxy.
//!
//! * the WebOS hostname → the API ([`crate::api`]);
//! * a pack hostname in `deltos-packs.lan` → the pack zone ([`crate::packzone`]);
//! * a phone's internet check, when switched on ([`AppState::answer_connectivity_checks`]),
//!   answered from `deltos_net::connectivity`'s closed list;
//! * **anything else is refused before routing** (privilege boundary rule 16),
//!   which is what stops DNS rebinding.
//!
//! No CORS request is answered on any surface (deltd API §2).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, Response, StatusCode};
use axum::Router;

use crate::api::{api_headers, Api};
use crate::hostname::{normalize_host, PackHost, PACK_ZONE};
use crate::identity::Identity;
use crate::library::Library;
use crate::packzone::{self, ZoneConfig};
use crate::signin::ConnInfo;

/// The largest request body the API reads (sign-in and profile JSON).
pub const MAX_API_BODY: usize = 16 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub lib: Arc<Library>,
    pub ident: Arc<dyn Identity>,
    pub zone: ZoneConfig,
    /// Answer phones' internet checks (docs/box-network-plan.md §6). On
    /// exactly when the box's DNS sends those hosts here, which is when
    /// hotspot devices get no internet through the box.
    pub answer_connectivity_checks: bool,
    /// An HTTPS listener runs beside the plain one.
    pub https_available: bool,
}

impl AppState {
    pub fn new(lib: Arc<Library>, ident: Arc<dyn Identity>, webos_host: &str) -> Self {
        AppState {
            lib,
            ident,
            zone: ZoneConfig { webos_host: webos_host.to_ascii_lowercase() },
            answer_connectivity_checks: false,
            https_available: false,
        }
    }

    pub fn with_https(mut self, on: bool) -> Self {
        self.https_available = on;
        self
    }

    pub fn with_connectivity_checks(mut self, on: bool) -> Self {
        self.answer_connectivity_checks = on;
        self
    }
}

pub fn router(state: AppState) -> Router {
    Router::new().fallback(dispatch).with_state(state)
}

async fn dispatch(State(st): State<AppState>, req: Request) -> Response<Body> {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().host())
        .and_then(normalize_host);
    let Some(host) = host else { return refused() };
    let conn = req.extensions().get::<ConnInfo>().copied().unwrap_or_default();
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();

    if host == st.zone.webos_host {
        // API bodies are small JSON; anything bigger is refused unread
        let Ok(body) = axum::body::to_bytes(body, MAX_API_BODY).await else {
            let mut r = Response::builder().status(StatusCode::PAYLOAD_TOO_LARGE);
            api_headers(r.headers_mut().unwrap());
            return r.body(Body::empty()).unwrap();
        };
        let api = Api {
            lib: Arc::clone(&st.lib),
            ident: Arc::clone(&st.ident),
            webos_host: st.zone.webos_host.clone(),
            https_available: st.https_available,
        };
        return api.handle(&parts.method, &path, parts.uri.query(), &parts.headers, &body, &conn).await;
    }
    if host.ends_with(&format!(".{PACK_ZONE}")) {
        return packzone::serve(
            Arc::clone(&st.lib),
            Arc::clone(&st.ident),
            &st.zone,
            PackHost::parse(&host),
            &parts.method,
            &path,
            &parts.headers,
        )
        .await;
    }
    if st.answer_connectivity_checks && deltos_net::connectivity::is_check_host(&host) {
        return connectivity_answer(&host, &parts.method, &path);
    }
    refused()
}

/// A phone's internet check: only the listed path on the listed host, only
/// GET/HEAD, a fixed body, no cookie, never cached.
fn connectivity_answer(host: &str, method: &axum::http::Method, path: &str) -> Response<Body> {
    use axum::http::{HeaderValue, Method};
    let Some(c) = deltos_net::connectivity::lookup(host, path) else { return refused() };
    if method != Method::GET && method != Method::HEAD {
        return refused();
    }
    let mut r = Response::builder().status(StatusCode::from_u16(c.status).unwrap_or(StatusCode::NO_CONTENT));
    let h = r.headers_mut().unwrap();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if !c.content_type.is_empty() {
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static(c.content_type));
    }
    let body = if *method == Method::HEAD { Body::empty() } else { Body::from(c.body) };
    r.body(body).unwrap()
}

/// The answer to a hostname `deltd` does not serve: says nothing more.
fn refused() -> Response<Body> {
    let mut r = Response::builder().status(StatusCode::NOT_FOUND);
    api_headers(r.headers_mut().unwrap());
    r.body(Body::empty()).unwrap()
}
