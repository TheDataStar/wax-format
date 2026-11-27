//! The box API on the WebOS hostname: `/api/v1/...` (deltd API §3, §6).
//!
//! Built here: the Library (§6.2) and Search (§6.3) groups, `/status` and
//! `/openapi.json`; the session group is [`crate::signin`]'s, the Personal
//! group and the outline and reader reads [`crate::me`]'s. Every endpoint
//! learns the person from `identityd`'s session cookie and nothing else.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::hostname::PackHost;
use crate::identity::{Identity, Session};
use crate::library::{hex, Library, Pack, Snapshot};
use crate::packzone::etag_matches;
use crate::search::{self, Skip};

/// Default and largest page sizes for lists. Bounds on one response's
/// work, not contract values.
pub const DEFAULT_LIMIT: usize = 20;
pub const MAX_LIMIT: usize = 100;

/// The machine-readable contract for callers (deltd API §3 rule 2).
pub const OPENAPI: &str = include_str!("openapi.json");

pub struct Api {
    pub lib: Arc<Library>,
    pub ident: Arc<dyn Identity>,
    /// The WebOS hostname, for the exact-origin check on writes.
    pub webos_host: String,
    /// `deltd` has an HTTPS listener (it has the box's certificates).
    pub https_available: bool,
    /// The box CA's public certificate (PEM), offered to devices that are
    /// to trust the box (docs/box-certificates.md §5). `None`: not offered.
    pub ca_cert: Option<std::path::PathBuf>,
    /// Each person's bookmarks, highlights, lists and history (deltd API
    /// §6.5). `None`: this server keeps none (the desktop app keeps its own).
    pub personal: Option<Arc<crate::personal::Store>>,
    /// The key holder, for the admin endpoints (deltd API §6.8). `None`:
    /// this server changes nothing about the box.
    pub shell: Option<Arc<dyn crate::admin::Shell>>,
}

/// An error in the RFC 9457 shape with a code from API §7's closed list.
pub struct Problem {
    status: StatusCode,
    code: &'static str,
    title: &'static str,
    detail: Option<String>,
}

impl Problem {
    pub fn new(status: StatusCode, code: &'static str, title: &'static str) -> Self {
        Problem { status, code, title, detail: None }
    }
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
    pub fn code(&self) -> &'static str {
        self.code
    }
    pub fn response(self) -> Response<Body> {
        let mut v = json!({ "status": self.status.as_u16(), "code": self.code, "title": self.title });
        if let Some(d) = self.detail {
            v["detail"] = Value::String(d);
        }
        let mut r = Response::builder().status(self.status);
        api_headers(r.headers_mut().unwrap());
        r.headers_mut()
            .unwrap()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/problem+json"));
        r.headers_mut().unwrap().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        r.body(Body::from(v.to_string())).unwrap()
    }
}

pub(crate) fn not_signed_in() -> Problem {
    Problem::new(StatusCode::UNAUTHORIZED, "not_signed_in", "Please sign in.")
}
pub(crate) fn invalid(d: impl Into<String>) -> Problem {
    Problem::new(StatusCode::BAD_REQUEST, "invalid_request", "The request is not valid.").detail(d)
}
pub(crate) fn pack_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "pack_not_found", "No such pack.")
}
fn not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "invalid_request", "No such endpoint.")
}

/// Rule 12's headers on every WebOS and API response.
pub fn api_headers(h: &mut HeaderMap) {
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
}

/// Rule 17: over HTTPS, managed devices never fall back to plain HTTP for
/// the WebOS hostname (not its subdomains).
pub fn hsts(h: &mut HeaderMap) {
    h.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static("max-age=31536000"));
}

/// A WebOS cookie (privilege boundary §4.1): host-only (no `Domain`),
/// `HttpOnly`, `SameSite=Strict`, `Path=/`; over HTTPS the `__Host-` name
/// and `Secure`. `max_age` `None` lasts until the browser closes; `Some(0)`
/// deletes it.
pub fn cookie(names: (&str, &str), https: bool, value: &str, max_age: Option<i64>) -> String {
    let mut c = if https {
        format!("{}={value}; Secure; HttpOnly; SameSite=Strict; Path=/", names.1)
    } else {
        format!("{}={value}; HttpOnly; SameSite=Strict; Path=/", names.0)
    };
    if let Some(a) = max_age {
        c.push_str(&format!("; Max-Age={a}"));
    }
    c
}

/// The session cookie's value: `__Host-delt_session` over HTTPS,
/// `delt_session` otherwise (deltd API §6.1).
pub fn session_token(headers: &HeaderMap) -> Option<String> {
    let mut plain = None;
    for v in headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()) {
        for kv in v.split(';') {
            let Some((k, val)) = kv.trim().split_once('=') else { continue };
            match k.trim() {
                "__Host-delt_session" => return Some(val.trim().to_string()),
                "delt_session" => plain = Some(val.trim().to_string()),
                _ => {}
            }
        }
    }
    plain
}

pub(crate) fn query_map(q: Option<&str>) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for kv in q.unwrap_or("").split('&').filter(|s| !s.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let dec = |s: &str| {
            percent_encoding::percent_decode_str(&s.replace('+', " "))
                .decode_utf8_lossy()
                .into_owned()
        };
        m.insert(dec(k), dec(v));
    }
    m
}

pub(crate) fn limit_of(q: &BTreeMap<String, String>) -> Result<usize, Problem> {
    match q.get("limit") {
        None => Ok(DEFAULT_LIMIT),
        Some(s) => match s.parse::<usize>() {
            Ok(n) if (1..=MAX_LIMIT).contains(&n) => Ok(n),
            _ => Err(invalid(format!("limit must be 1–{MAX_LIMIT}"))),
        },
    }
}

/// Offset cursors for ranked lists ("o<n>"). Opaque to callers.
fn offset_of(q: &BTreeMap<String, String>) -> Result<usize, Problem> {
    match q.get("cursor") {
        None => Ok(0),
        Some(c) => c
            .strip_prefix('o')
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| invalid("cursor is not one this server issued")),
    }
}

pub(crate) fn visible(p: &Pack, s: &Session) -> bool {
    !s.guest || p.guest_accessible()
}

/// Packs the person may see, after the `packs`, `lang`/`language` and
/// `category` filters.
fn chosen<'a>(snap: &'a Snapshot, s: &Session, q: &BTreeMap<String, String>) -> Result<Vec<&'a Arc<Pack>>, Problem> {
    let wanted: Option<Vec<&str>> = q.get("packs").map(|v| v.split(',').map(str::trim).filter(|x| !x.is_empty()).collect());
    let lang = q.get("lang").or_else(|| q.get("language")).map(|l| l.to_ascii_lowercase());
    let cat = q.get("category");
    let mut out = Vec::new();
    for p in snap.by_uuid.values() {
        if !visible(p, s) {
            continue;
        }
        if let Some(w) = &wanted {
            if !w.contains(&p.archive_uuid.as_str()) {
                continue;
            }
        }
        if let Some(l) = &lang {
            let ok = p.languages().iter().any(|pl| {
                let pl = pl.to_ascii_lowercase();
                pl == *l || pl.starts_with(&format!("{l}-"))
            });
            if !ok {
                continue;
            }
        }
        if let Some(c) = cat {
            if p.manifest.get("category") != Some(c) {
                continue;
            }
        }
        out.push(p);
    }
    if let Some(w) = &wanted {
        for id in w {
            if !snap.by_uuid.get(*id).is_some_and(|p| visible(p, s)) {
                return Err(pack_not_found().detail(format!("pack {id}")));
            }
        }
    }
    Ok(out)
}

fn search_status(p: &Pack) -> Value {
    match p.search_tokenizer.as_deref() {
        None => json!({ "available": false, "reason": Skip::NoIndex.code() }),
        Some(t) if t == "unicode61" || t == "trigram" || t == wax_core::tokenize::WAX_BIGRAM => {
            json!({ "available": true, "tokenizer": t })
        }
        Some(t) => json!({ "available": false, "tokenizer": t, "reason": Skip::UnknownTokenizer.code() }),
    }
}

fn pack_summary(p: &Pack) -> Value {
    json!({
        "archive_uuid": p.archive_uuid,
        "name": p.name(),
        "version": p.manifest.get("version"),
        "category": p.manifest.get("category"),
        "languages": p.languages(),
        "guest_accessible": p.guest_accessible(),
        "live_entry_count": p.live_entry_count,
        "search": search_status(p),
    })
}

impl Api {
    /// Answer one request on the WebOS hostname.
    pub async fn handle(
        &self,
        method: &Method,
        path: &str,
        query: Option<&str>,
        headers: &HeaderMap,
        body: &[u8],
        conn: &crate::signin::ConnInfo,
    ) -> Response<Body> {
        let mut r = self.handle_inner(method, path, query, headers, body, conn).await;
        if conn.https {
            hsts(r.headers_mut());
        }
        r
    }

    async fn handle_inner(
        &self,
        method: &Method,
        path: &str,
        query: Option<&str>,
        headers: &HeaderMap,
        body: &[u8],
        conn: &crate::signin::ConnInfo,
    ) -> Response<Body> {
        let Some(rest) = path.strip_prefix("/api/v1/") else {
            return not_found().response();
        };
        if rest == "openapi.json" {
            return self.ok(json_bytes(OPENAPI.as_bytes().to_vec()), None, headers, false);
        }
        if rest == "certificate" || rest == "certificate/ca.crt" {
            return self.certificate(method, rest == "certificate/ca.crt");
        }
        {
            let segs: Vec<&str> = rest.split('/').collect();
            let q = query_map(query);
            if let Some(r) = self.people(method, &segs, &q, headers, body, conn).await {
                return r;
            }
            if let Some(r) = self.me(method, &segs, &q, headers, body, conn).await {
                return r;
            }
            if segs == ["progress-events"] {
                if *method != Method::POST {
                    let mut r = Problem::new(StatusCode::METHOD_NOT_ALLOWED, "invalid_request", "Method not allowed.").response();
                    r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST"));
                    return r;
                }
                return self.progress_events(headers, body, conn).await.unwrap_or_else(Problem::response);
            }
            if let Some(r) = self.admin(method, &segs, headers, body, conn).await {
                return r;
            }
        }
        if method != Method::GET && method != Method::HEAD {
            let mut r = Problem::new(StatusCode::METHOD_NOT_ALLOWED, "invalid_request", "Method not allowed.").response();
            r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
            return r;
        }
        let Some(session) = session_token(headers).and_then(|t| self.ident.session_by_token(&t)) else {
            return not_signed_in().response();
        };
        let q = query_map(query);
        let snap = self.lib.snapshot();
        let segs: Vec<&str> = rest.split('/').collect();
        let result = match segs.as_slice() {
            ["status"] => Ok(self.status(&snap, &session)),
            ["packs"] => self.packs(&snap, &session, &q),
            ["packs", id] => self.pack(&snap, &session, id),
            ["packs", id, "pages"] => self.pages(&snap, &session, id, &q).await,
            ["packs", id, "outline"] => self.outline(&snap, &session, id, &q).await,
            ["packs", id, "reader"] => self.reader(&snap, &session, id, &q).await,
            ["search"] => self.search(&snap, &session, &q, None).await,
            ["search", "packs", id] => self.search(&snap, &session, &q, Some(id)).await,
            ["suggest"] => self.suggest(&snap, &session, &q).await,
            _ => Err(not_found()),
        };
        match result {
            Ok(v) => {
                // Exact caching (rule 10): the tag covers the packs' states, the
                // request, and whether the caller is Guest.
                let mut h = Sha256::new();
                for p in snap.by_uuid.values() {
                    h.update(p.state_token.as_bytes());
                }
                h.update(path.as_bytes());
                h.update(query.unwrap_or("").as_bytes());
                h.update([session.guest as u8]);
                let tag = format!("W/\"{}\"", &hex(&h.finalize())[..32]);
                self.ok(json_bytes(serde_json::to_vec(&v).unwrap()), Some(tag), headers, method == Method::HEAD)
            }
            Err(p) => p.response(),
        }
    }

    /// The box CA's public certificate, for a device that does not trust the
    /// box yet (so: no session, and over plain HTTP too). Nothing secret: a
    /// device must still compare the fingerprint with the one the box shows
    /// on its own screen before trusting it (docs/box-certificates.md §5).
    fn certificate(&self, method: &Method, der_file: bool) -> Response<Body> {
        use rustls::pki_types::pem::PemObject;
        if method != Method::GET && method != Method::HEAD {
            let mut r = Problem::new(StatusCode::METHOD_NOT_ALLOWED, "invalid_request", "Method not allowed.").response();
            r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
            return r;
        }
        let Some(der) = self
            .ca_cert
            .as_ref()
            .and_then(|p| rustls::pki_types::CertificateDer::from_pem_file(p).ok())
        else {
            return Problem::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "certificate_unavailable",
                "This box has not made its certificates yet.",
            )
            .response();
        };
        let mut r = Response::builder();
        let h = r.headers_mut().unwrap();
        api_headers(h);
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        let body = if der_file {
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-x509-ca-cert"));
            h.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment; filename=\"deltos-box-ca.crt\""),
            );
            der.to_vec()
        } else {
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let fp = hex(&Sha256::digest(der.as_ref()))
                .as_bytes()
                .chunks(2)
                .map(|c| std::str::from_utf8(c).unwrap().to_ascii_uppercase())
                .collect::<Vec<_>>()
                .join(":");
            serde_json::to_vec(&json!({
                "fingerprint_sha256": fp,
                "download": "/api/v1/certificate/ca.crt",
                "https": self.https_available
            }))
            .unwrap()
        };
        let body = if *method == Method::HEAD { Body::empty() } else { Body::from(body) };
        r.status(StatusCode::OK).body(body).unwrap()
    }

    fn ok(&self, body: Vec<u8>, etag: Option<String>, headers: &HeaderMap, head: bool) -> Response<Body> {
        let mut r = Response::builder();
        let h = r.headers_mut().unwrap();
        api_headers(h);
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        // depends on the session: never shared, always revalidated
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
        h.insert(header::VARY, HeaderValue::from_static("Cookie"));
        if let Some(t) = &etag {
            h.insert(header::ETAG, HeaderValue::from_str(t).unwrap());
            if let Some(inm) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
                if etag_matches(inm, t) {
                    return r.status(StatusCode::NOT_MODIFIED).body(Body::empty()).unwrap();
                }
            }
        }
        if head {
            return r.status(StatusCode::OK).body(Body::empty()).unwrap();
        }
        r.status(StatusCode::OK).body(Body::from(body)).unwrap()
    }

    fn status(&self, snap: &Snapshot, _s: &Session) -> Value {
        json!({
            "service": "deltd",
            "version": env!("CARGO_PKG_VERSION"),
            "api": "v1",
            "format": format!("{}.{}", wax_core::FORMAT_VERSION_MAJOR, wax_core::FORMAT_VERSION_MINOR),
            "packs": snap.by_uuid.len(),
            "damaged_packs": snap.damaged.len(),
            "health_events": self.lib.health.count(),
            "features": {
                "search": true,
                "suggest": true,
                "snippets": false,
                "zstd_passthrough": true,
                "https": self.https_available
            }
        })
    }

    fn packs(&self, snap: &Snapshot, s: &Session, q: &BTreeMap<String, String>) -> Result<Value, Problem> {
        let limit = limit_of(q)?;
        let after = q.get("cursor").cloned();
        let items: Vec<Value> = chosen(snap, s, q)?
            .into_iter()
            .filter(|p| after.as_ref().is_none_or(|a| p.archive_uuid.as_str() > a.as_str()))
            .take(limit + 1)
            .map(|p| pack_summary(p))
            .collect();
        Ok(page_of(items, limit, |v| v["archive_uuid"].as_str().unwrap().to_string()))
    }

    pub(crate) fn find<'a>(&self, snap: &'a Snapshot, s: &Session, id: &str) -> Result<&'a Arc<Pack>, Problem> {
        match snap.by_uuid.get(id) {
            Some(p) if visible(p, s) => Ok(p),
            Some(_) if s.guest => Err(Problem::new(StatusCode::FORBIDDEN, "guest_not_allowed", "Not available to Guest.")),
            _ => Err(pack_not_found()),
        }
    }

    fn pack(&self, snap: &Snapshot, s: &Session, id: &str) -> Result<Value, Problem> {
        let p = self.find(snap, s, id)?;
        let mut v = pack_summary(p);
        v["manifest"] = json!(p.manifest);
        v["size_bytes"] = json!(p.file_size);
        v["segment_count"] = json!(p.segment_count);
        v["pack_hostname"] = match self.ident.partition_for(s) {
            Some(part) => json!(PackHost {
                pack_index: p.slot.pack_index,
                pack_generation: p.slot.pack_generation,
                pack_partition: part
            }
            .to_string()),
            None => Value::Null,
        };
        Ok(v)
    }

    async fn pages(&self, snap: &Snapshot, s: &Session, id: &str, q: &BTreeMap<String, String>) -> Result<Value, Problem> {
        let p = Arc::clone(self.find(snap, s, id)?);
        let limit = limit_of(q)?;
        let prefix = q.get("prefix").cloned().unwrap_or_default();
        let cursor = q.get("cursor").cloned();
        let rows = tokio::task::spawn_blocking(move || {
            p.readers.with(|r| -> wax_core::Result<Vec<Value>> {
                let mut out = Vec::new();
                // the prefix itself, which a strictly-after seek would skip
                if cursor.is_none() && !prefix.is_empty() {
                    if let Ok(e) = r.entry(&prefix) {
                        out.push(page_row(&e));
                    }
                }
                let start = cursor.or_else(|| (!prefix.is_empty()).then(|| prefix.clone()));
                for e in r.entries_after(start.as_deref()) {
                    let e = e?;
                    if !e.path.starts_with(&prefix) || out.len() > limit {
                        break;
                    }
                    out.push(page_row(&e));
                }
                Ok(out)
            })
        })
        .await
        .map_err(|_| Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong."))?
        .map_err(|e| Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "pack_damaged", "This pack could not be read.").detail(e.to_string()))?;
        Ok(page_of(rows, limit, |v| v["path"].as_str().unwrap().to_string()))
    }

    async fn search(
        &self,
        snap: &Snapshot,
        s: &Session,
        q: &BTreeMap<String, String>,
        one: Option<&str>,
    ) -> Result<Value, Problem> {
        let text = q.get("q").map(|t| t.trim().to_string()).unwrap_or_default();
        if text.is_empty() {
            return Err(invalid("q is required"));
        }
        let limit = limit_of(q)?;
        let offset = offset_of(q)?;
        let packs: Vec<Arc<Pack>> = match one {
            Some(id) => vec![Arc::clone(self.find(snap, s, id)?)],
            None => chosen(snap, s, q)?.into_iter().cloned().collect(),
        };
        let want = offset + limit + 1;
        let t = text.clone();
        let (lists, skipped) = tokio::task::spawn_blocking(move || {
            let mut lists = Vec::new();
            let mut skipped = Vec::new();
            for p in &packs {
                match p.readers.with(|r| search::search_pack(r, &p.archive_uuid, &t, want)) {
                    Ok(h) => lists.push(h),
                    Err(k) => skipped.push(json!({ "pack": p.archive_uuid, "name": p.name(), "reason": k.code() })),
                }
            }
            (lists, skipped)
        })
        .await
        .map_err(|_| Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong."))?;
        if one.is_some() && lists.is_empty() {
            let reason = skipped.first().and_then(|v| v["reason"].as_str()).unwrap_or("no_index").to_string();
            return Err(Problem::new(StatusCode::CONFLICT, "search_unavailable", "This pack cannot be searched.").detail(reason));
        }
        let fused = search::fuse(lists);
        let more = fused.len() > offset + limit;
        let items: Vec<Value> = fused
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|h| {
                let name = snap.by_uuid.get(&h.pack).map(|p| p.name().to_string());
                json!({ "pack": h.pack, "pack_name": name, "path": h.path, "title": h.title,
                        "position_in_pack": h.position, "snippet": null })
            })
            .collect();
        Ok(json!({
            "query": text,
            "items": items,
            "skipped": skipped,
            "next": if more { Value::String(format!("o{}", offset + limit)) } else { Value::Null },
        }))
    }

    async fn suggest(&self, snap: &Snapshot, s: &Session, q: &BTreeMap<String, String>) -> Result<Value, Problem> {
        let text = q.get("q").cloned().unwrap_or_default();
        if text.trim().is_empty() {
            return Err(invalid("q is required"));
        }
        let limit = limit_of(q)?;
        let packs: Vec<Arc<Pack>> = chosen(snap, s, q)?.into_iter().cloned().collect();
        let t = text.clone();
        let mut all = tokio::task::spawn_blocking(move || {
            let mut all = Vec::new();
            for p in &packs {
                if let Ok(c) = p.readers.with(|r| search::suggest_pack(r, &p.archive_uuid, &t, limit)) {
                    all.extend(c);
                }
            }
            all
        })
        .await
        .map_err(|_| Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong."))?;
        // exact matches first, then by accent-insensitive title, then pack
        all.sort_by(|a, b| {
            b.exact
                .cmp(&a.exact)
                .then_with(|| search::base_fold(&a.title).cmp(&search::base_fold(&b.title)))
                .then_with(|| a.pack.cmp(&b.pack))
                .then_with(|| a.path.cmp(&b.path))
        });
        all.truncate(limit);
        let items: Vec<Value> = all
            .into_iter()
            .map(|c| json!({ "pack": c.pack, "path": c.path, "title": c.title, "exact": c.exact }))
            .collect();
        Ok(json!({ "query": text, "items": items }))
    }
}

fn page_row(e: &wax_core::Entry) -> Value {
    json!({ "path": e.path, "title": e.title, "redirect_to": e.redirect_to })
}

/// `items` + `next` from up to `limit + 1` rows (the extra row says there is more).
pub(crate) fn page_of(mut items: Vec<Value>, limit: usize, key: impl Fn(&Value) -> String) -> Value {
    let more = items.len() > limit;
    items.truncate(limit);
    let next = if more { items.last().map(|v| Value::String(key(v))).unwrap_or(Value::Null) } else { Value::Null };
    json!({ "items": items, "next": next })
}

pub(crate) fn json_bytes(b: Vec<u8>) -> Vec<u8> {
    b
}
