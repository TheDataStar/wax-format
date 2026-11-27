//! Pack pages in the desktop window, served by deltlib's own pack zone.
//!
//! The window loads pack pages from a private URL scheme, `deltpack:`, that
//! only this process answers: nothing listens on a network port, so no other
//! program on the computer can read a pack through Delt.
//!
//! * **Every request goes through [`deltlib::packzone::serve`]**, the same
//!   code the box runs (deltd API §4): the same 404/410 rows, ETags, byte
//!   ranges, dictionary decoding and headers. Nothing is forked.
//! * **One host per pack**, `pack-<pack_index>-g<pack_generation>.localhost`,
//!   because converted pages link root-relative (`/A/Foo`, zim2wax §20), so
//!   each pack needs its own root. The index and generation are the box's
//!   own (deltlib's catalog), so a removed pack's address never serves a
//!   newer pack. The partition the box would carry in the hostname is the
//!   desktop's single one ([`crate::identity`]), added here.
//! * **Only the framing rule changes**: the pack zone allows framing by the
//!   WebOS host; here the frame's parent is the desktop window, so
//!   `frame-ancestors` names the window's origin instead.
//!
//! On Windows the web view cannot register a real scheme, so Tauri maps
//! `deltpack://h/` to `http://deltpack.h/` and back; [`pack_origin`] gives
//! the form the window must use.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode};
use deltlib::hostname::PackHost;
use deltlib::packzone::{self, ZoneConfig};
use deltlib::{Identity, Library};

/// The URL scheme pack pages are served from.
pub const SCHEME: &str = "deltpack";

/// The desktop window's own origins (Tauri's, per platform).
pub const WINDOW_ORIGINS: &str = "tauri://localhost http://tauri.localhost https://tauri.localhost";

/// The largest response the window is handed in one piece. Media is asked
/// for in ranges by the web view; a whole-file request above this is cut to
/// a range answer by the web view's own retry.
pub const MAX_BODY: usize = 512 * 1024 * 1024;

/// The host for one pack.
pub fn pack_host(pack_index: u16, pack_generation: u64) -> String {
    format!("pack-{pack_index}-g{pack_generation}.localhost")
}

/// `deltpack://pack-…localhost`, or its Windows form.
pub fn pack_origin(pack_index: u16, pack_generation: u64) -> String {
    let host = pack_host(pack_index, pack_generation);
    if cfg!(windows) {
        format!("http://{SCHEME}.{host}")
    } else {
        format!("{SCHEME}://{host}")
    }
}

/// Parse a host made by [`pack_host`] (either platform form).
pub fn parse_host(host: &str, partition: &str) -> Option<PackHost> {
    let h = host.to_ascii_lowercase();
    let h = h.strip_prefix(&format!("{SCHEME}.")).unwrap_or(&h);
    let rest = h.strip_suffix(".localhost")?.strip_prefix("pack-")?;
    let (idx, gen) = rest.split_once("-g")?;
    let canonical = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    if !canonical(idx) || !canonical(gen) {
        return None;
    }
    Some(PackHost {
        pack_index: idx.parse().ok().filter(|i| *i <= deltlib::hostname::MAX_PACK_INDEX)?,
        pack_generation: gen.parse().ok()?,
        pack_partition: partition.to_string(),
    })
}

/// A page the window has just shown, for history and the reading chrome.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Navigated {
    pub pack: String,
    pub path: String,
    pub title: String,
}

pub struct Served {
    pub status: u16,
    pub headers: Vec<(String, Vec<u8>)>,
    pub body: Vec<u8>,
    /// Set when this was a page (HTML) answered whole: the reader moved.
    pub navigated: Option<Navigated>,
}

/// Answer one `deltpack:` request.
pub async fn serve(
    lib: Arc<Library>,
    ident: Arc<dyn Identity>,
    partition: &str,
    host: &str,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Served {
    let cfg = ZoneConfig { webos_host: "tauri.localhost".into(), personal: None };
    let ph = parse_host(host, partition);
    let pack = ph.as_ref().and_then(|h| lib.snapshot().by_index(h.pack_index).cloned());
    let resp = packzone::serve(Arc::clone(&lib), ident, &cfg, ph, method, path, headers).await;
    let mut served = into_served(resp).await;

    // A page the frame now shows: answered whole (200) or confirmed unchanged
    // from the web view's cache (304, which carries no content type), and a
    // web page by the entry's own type. EPUB chapters are XHTML
    // (`application/xhtml+xml`), so `text/html` alone missed every chapter.
    let dest_ok = headers
        .get("sec-fetch-dest")
        .and_then(|v| v.to_str().ok())
        .is_none_or(|d| d == "iframe" || d == "document");
    if matches!(served.status, 200 | 304) && *method == Method::GET && dest_ok {
        if let Some(p) = pack {
            let entry_path = percent_decode(path.trim_start_matches('/'));
            let entry = p.readers.with(|r| r.entry(&entry_path).ok());
            let mime = entry.as_ref().and_then(|e| e.mime.clone()).or_else(|| {
                served.headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| String::from_utf8_lossy(v).into_owned())
            });
            if mime.as_deref().is_some_and(is_page_type) {
                let title = entry.and_then(|e| e.title).unwrap_or_else(|| entry_path.clone());
                served.navigated = Some(Navigated { pack: p.archive_uuid.clone(), path: entry_path, title });
            }
        }
    }
    served
}

/// The types a reader shows as a page of its own.
fn is_page_type(mime: &str) -> bool {
    let base = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    base == "text/html" || base == "application/xhtml+xml"
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

async fn into_served(resp: Response<Body>) -> Served {
    let (parts, body) = resp.into_parts();
    let body = axum::body::to_bytes(body, MAX_BODY).await.map(|b| b.to_vec());
    let (status, body) = match body {
        Ok(b) => (parts.status, b),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, b"This page is too large to show.".to_vec()),
    };
    let mut hs = parts.headers;
    reframe(&mut hs);
    Served {
        status: status.as_u16(),
        headers: hs.iter().map(|(k, v)| (k.as_str().to_string(), v.as_bytes().to_vec())).collect(),
        body,
        navigated: None,
    }
}

/// Replace the pack zone's `frame-ancestors` with the desktop window's.
fn reframe(h: &mut HeaderMap) {
    let Some(csp) = h.get(header::CONTENT_SECURITY_POLICY).and_then(|v| v.to_str().ok()) else { return };
    let kept: Vec<&str> = csp
        .split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty() && !d.starts_with("frame-ancestors"))
        .collect();
    let csp = format!("{}; frame-ancestors {WINDOW_ORIGINS}", kept.join("; "));
    if let Ok(v) = HeaderValue::from_str(&csp) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_round_trip_on_both_platform_forms() {
        let h = pack_host(7, 3);
        assert_eq!(h, "pack-7-g3.localhost");
        let p = parse_host(&h, "abcdabcdabcdabcd").unwrap();
        assert_eq!((p.pack_index, p.pack_generation), (7, 3));
        let w = parse_host("deltpack.pack-7-g3.localhost", "abcdabcdabcdabcd").unwrap();
        assert_eq!(w.pack_index, 7);
    }

    #[test]
    fn non_canonical_hosts_are_refused() {
        for h in ["pack-07-g3.localhost", "pack-7-g03.localhost", "pack-7.localhost", "pack-x-g1.localhost",
                  "pack-600-g1.localhost", "pack-7-g3.example", "tauri.localhost", ""] {
            assert!(parse_host(h, "p").is_none(), "{h}");
        }
    }

    #[test]
    fn decoding_paths() {
        assert_eq!(percent_decode("Caf%C3%A9.html"), "Café.html");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn only_framing_changes_in_the_policy() {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'self' data: blob:; base-uri 'self'; frame-ancestors http://x https://x"),
        );
        reframe(&mut h);
        assert_eq!(
            h[header::CONTENT_SECURITY_POLICY],
            format!("default-src 'self' data: blob:; base-uri 'self'; frame-ancestors {WINDOW_ORIGINS}").as_str()
        );
    }
}
