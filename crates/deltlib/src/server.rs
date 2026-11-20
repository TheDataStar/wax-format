//! `deltd`'s front door: one listener, routed by `Host` (Track A §20.3), so
//! pack isolation holds at every profile, with or without a reverse proxy.
//!
//! * the WebOS hostname → the API ([`crate::api`]);
//! * a pack hostname in `deltos-packs.lan` → the pack zone ([`crate::packzone`]);
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

#[derive(Clone)]
pub struct AppState {
    pub lib: Arc<Library>,
    pub ident: Arc<dyn Identity>,
    pub zone: ZoneConfig,
}

impl AppState {
    pub fn new(lib: Arc<Library>, ident: Arc<dyn Identity>, webos_host: &str) -> Self {
        AppState { lib, ident, zone: ZoneConfig { webos_host: webos_host.to_ascii_lowercase() } }
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
    let (parts, _body) = req.into_parts();
    let path = parts.uri.path().to_string();

    if host == st.zone.webos_host {
        let api = Api { lib: Arc::clone(&st.lib), ident: Arc::clone(&st.ident) };
        return api.handle(&parts.method, &path, parts.uri.query(), &parts.headers).await;
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
    refused()
}

/// The answer to a hostname `deltd` does not serve: says nothing more.
fn refused() -> Response<Body> {
    let mut r = Response::builder().status(StatusCode::NOT_FOUND);
    api_headers(r.headers_mut().unwrap());
    r.body(Body::empty()).unwrap()
}
