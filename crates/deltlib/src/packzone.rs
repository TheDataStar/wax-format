//! The pack zone (deltd API §4): a plain, fast file server for pack content.
//!
//! `GET` and `HEAD` on an entry's path, nothing else. Pages are served
//! exactly as built — never rewritten, never given a toolbar. Every row of
//! API §4.1 is here, and every response carries §4.2's headers.
//!
//! The zone never sees a sign-in: no cookie is read or set. Who may read is
//! decided only by the hostname's partition (API §4.4).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode};
use sha2::{Digest, Sha256};
use wax_core::{Compression, Entry, WaxError};

use crate::hostname::PackHost;
use crate::identity::Identity;
use crate::library::{hex, Library, Pack};

/// Media and other stored-verbatim entries are sent in pieces this size, so
/// a large video never sits whole in memory on a 2 GB box.
pub const STREAM_CHUNK: usize = 256 * 1024;

/// Settings the pack zone needs from the server.
#[derive(Clone)]
pub struct ZoneConfig {
    /// The WebOS hostname (contract §8; the name is still open, §13.2), for
    /// `frame-ancestors`.
    pub webos_host: String,
}

/// Serve one pack-zone request. `host` is the parsed pack hostname, `path`
/// the raw (still percent-encoded) URL path.
pub async fn serve(
    lib: Arc<Library>,
    ident: Arc<dyn Identity>,
    cfg: &ZoneConfig,
    host: Option<PackHost>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Response<Body> {
    // 1. Only issued, live hostnames (§4.4), checked before any lookup.
    let Some(host) = host else { return page(cfg, StatusCode::NOT_FOUND) };
    let snap = lib.snapshot();
    let Some(pack) = snap.by_index(host.pack_index).cloned() else {
        return page(cfg, StatusCode::NOT_FOUND);
    };
    if pack.slot.pack_generation != host.pack_generation {
        return page(cfg, StatusCode::NOT_FOUND);
    }
    let Some(session) = ident.session_for_partition(&host.pack_partition) else {
        return page(cfg, StatusCode::NOT_FOUND);
    };
    if session.guest && !pack.guest_accessible() {
        return page(cfg, StatusCode::NOT_FOUND);
    }
    drop(snap);

    // 2. Methods.
    if method != Method::GET && method != Method::HEAD {
        let mut r = page(cfg, StatusCode::METHOD_NOT_ALLOWED);
        r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return r;
    }
    let head = method == Method::HEAD;

    // 3. Path.
    let Ok(decoded) = percent_encoding::percent_decode_str(path).decode_utf8() else {
        return page(cfg, StatusCode::NOT_FOUND);
    };
    let entry_path = decoded.trim_start_matches('/').to_string();
    if entry_path.is_empty() {
        return match pack.manifest.get("entry_point") {
            Some(ep) => redirect(cfg, StatusCode::FOUND, ep),
            None => page(cfg, StatusCode::NOT_FOUND),
        };
    }

    // 4. Look it up.
    let lookup = {
        let p = Arc::clone(&pack);
        let ep = entry_path.clone();
        tokio::task::spawn_blocking(move || p.readers.with(|r| r.entry(&ep))).await
    };
    let entry = match lookup {
        Ok(Ok(e)) => e,
        Ok(Err(WaxError::EntryNotFound(_))) => return page(cfg, StatusCode::NOT_FOUND),
        Ok(Err(WaxError::EntryDeleted(_))) => return page(cfg, StatusCode::GONE),
        Ok(Err(e)) => {
            lib.health.record(&format!("{}: lookup {entry_path:?}: {e}", pack.archive_uuid));
            return page(cfg, StatusCode::INTERNAL_SERVER_ERROR);
        }
        Err(_) => return page(cfg, StatusCode::INTERNAL_SERVER_ERROR),
    };
    if let Some(to) = &entry.redirect_to {
        return redirect(cfg, StatusCode::MOVED_PERMANENTLY, to);
    }
    let codec = match Compression::parse(&entry.path, &entry.compression) {
        Ok(c) => c,
        Err(e) => {
            // refuse rather than guess (SPEC §3)
            lib.health.record(&format!("{}: {e}", pack.archive_uuid));
            return page(cfg, StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    serve_entry(lib, cfg, pack, entry, codec, headers, head).await
}

async fn serve_entry(
    lib: Arc<Library>,
    cfg: &ZoneConfig,
    pack: Arc<Pack>,
    entry: Entry,
    codec: Compression,
    headers: &HeaderMap,
    head: bool,
) -> Response<Body> {
    let mime = entry.mime.clone().unwrap_or_else(|| "application/octet-stream".into());
    let sha = entry.sha256.map(|s| hex(&s));
    let etag_plain = sha.as_ref().map(|h| format!("\"{h}\""));
    let etag_zst = sha.as_ref().map(|h| format!("\"{h}-zst\""));
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let range = range.filter(|_| if_range_ok(headers, etag_plain.as_deref()));

    // zstd passthrough: only a plain-zstd entry, only to a client that asks
    // for zstd, and only without a range (API §4.3). Browsers send zstd only
    // over HTTPS, so plain-HTTP visitors always get the decoded page.
    let passthrough = codec == Compression::Zstd && range.is_none() && accepts_zstd(headers);
    let etag = if passthrough { etag_zst.clone() } else { etag_plain.clone() };

    let mut b = Response::builder();
    {
        let h = b.headers_mut().expect("fresh builder");
        zone_headers(h, cfg);
        h.insert(header::CONTENT_TYPE, hv(&mime));
        h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        // HTML is revalidated on every load so a retired hostname stops
        // working at once; the rest is revalidated by ETag too, which costs
        // a 304 and can never serve an old edition.
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        if let Some(t) = &etag {
            h.insert(header::ETAG, hv(t));
        }
        if !passthrough {
            h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        }
    }

    if let (Some(t), Some(inm)) = (&etag, headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok())) {
        if etag_matches(inm, t) {
            return b.status(StatusCode::NOT_MODIFIED).body(Body::empty()).unwrap();
        }
    }

    let total = entry.uncompressed_length;
    let span = match range.as_deref().map(|r| parse_range(r, total)) {
        None | Some(RangeSpec::Ignore) => None,
        Some(RangeSpec::Unsatisfiable) => {
            let h = b.headers_mut().unwrap();
            h.insert(header::CONTENT_RANGE, hv(&format!("bytes */{total}")));
            return b.status(StatusCode::RANGE_NOT_SATISFIABLE).body(Body::empty()).unwrap();
        }
        Some(RangeSpec::Span(s, e)) => Some((s, e)),
    };
    let (status, start, len) = match span {
        Some((s, e)) => {
            b.headers_mut()
                .unwrap()
                .insert(header::CONTENT_RANGE, hv(&format!("bytes {s}-{e}/{total}")));
            (StatusCode::PARTIAL_CONTENT, s, e - s + 1)
        }
        None => (StatusCode::OK, 0, total),
    };

    // Stored verbatim: stream straight from the file.
    if codec == Compression::None {
        if entry.length != entry.uncompressed_length {
            lib.health.record(&format!("{}: {:?} stored length differs from its size", pack.archive_uuid, entry.path));
            return page(cfg, StatusCode::INTERNAL_SERVER_ERROR);
        }
        b.headers_mut().unwrap().insert(header::CONTENT_LENGTH, hv(&len.to_string()));
        let b = b.status(status);
        if head {
            return b.body(Body::empty()).unwrap();
        }
        // a whole-entry read is checked against its sha256 as it streams; a
        // byte range cannot be (the hash covers the whole entry)
        let verify = (span.is_none()).then_some(entry.sha256).flatten();
        return b.body(stream_stored(lib, pack, entry, start, len, verify)).unwrap();
    }

    // Compressed: decode on the box (checksum verified by the reader).
    let read = {
        let p = Arc::clone(&pack);
        let path = entry.path.clone();
        tokio::task::spawn_blocking(move || p.readers.with(|r| r.read(&path))).await
    };
    let plain = match read {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => {
            lib.health.record(&format!("{}: read {:?}: {e}", pack.archive_uuid, entry.path));
            return page(cfg, StatusCode::INTERNAL_SERVER_ERROR);
        }
        Err(_) => return page(cfg, StatusCode::INTERNAL_SERVER_ERROR),
    };
    if passthrough {
        // the decode above verified the checksum; send the stored frame
        let stored = {
            let p = Arc::clone(&pack);
            let (o, l) = (entry.offset, entry.length);
            tokio::task::spawn_blocking(move || p.read_stored(o, l)).await
        };
        let Ok(Ok(raw)) = stored else {
            return page(cfg, StatusCode::INTERNAL_SERVER_ERROR);
        };
        let h = b.headers_mut().unwrap();
        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static("zstd"));
        h.insert(header::CONTENT_LENGTH, hv(&raw.len().to_string()));
        let b = b.status(StatusCode::OK);
        return if head { b.body(Body::empty()) } else { b.body(Body::from(raw)) }.unwrap();
    }
    let body = plain[start as usize..(start + len) as usize].to_vec();
    b.headers_mut().unwrap().insert(header::CONTENT_LENGTH, hv(&len.to_string()));
    let b = b.status(status);
    if head { b.body(Body::empty()) } else { b.body(Body::from(body)) }.unwrap()
}

/// Stream `len` stored bytes from `start`, in [`STREAM_CHUNK`] pieces. With
/// `verify`, the whole entry is hashed as it goes and the transfer is cut off
/// — never completed — if the hash is wrong.
fn stream_stored(
    lib: Arc<Library>,
    pack: Arc<Pack>,
    entry: Entry,
    start: u64,
    len: u64,
    verify: Option<[u8; 32]>,
) -> Body {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(2);
    tokio::task::spawn_blocking(move || {
        let mut hasher = verify.map(|_| Sha256::new());
        let mut done = 0u64;
        while done < len {
            let n = (len - done).min(STREAM_CHUNK as u64);
            let chunk = match pack.read_stored(entry.offset + start + done, n) {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    return;
                }
            };
            done += n;
            if let Some(h) = hasher.as_mut() {
                h.update(&chunk);
                if done == len {
                    let got: [u8; 32] = hasher.take().unwrap().finalize().into();
                    if Some(got) != verify {
                        lib.health.record(&format!(
                            "{}: {:?} does not match its sha256; transfer cut off",
                            pack.archive_uuid, entry.path
                        ));
                        let _ = tx.blocking_send(Err(std::io::Error::other("checksum mismatch")));
                        return;
                    }
                }
            }
            if tx.blocking_send(Ok(chunk)).is_err() {
                return; // client went away
            }
        }
    });
    Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx))
}

/// §4.2's headers that every pack-zone response carries, errors included.
fn zone_headers(h: &mut HeaderMap, cfg: &ZoneConfig) {
    let w = &cfg.webos_host;
    let csp = format!(
        "default-src 'self' data: blob:; form-action 'self'; base-uri 'self'; \
         frame-ancestors http://{w} https://{w}"
    );
    h.insert(header::CONTENT_SECURITY_POLICY, hv(&csp));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("same-origin"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("same-origin"));
}

fn hv(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static("invalid"))
}

/// A small error page. The zone does not know the person, so it cannot pick
/// their language; translating these is part of the translatable-shell work
/// (contract §16) and they are English for now.
pub fn page(cfg: &ZoneConfig, status: StatusCode) -> Response<Body> {
    let msg = match status {
        StatusCode::NOT_FOUND => "This page is not here.",
        StatusCode::GONE => "This page was removed in an update.",
        StatusCode::METHOD_NOT_ALLOWED => "Pages can only be read.",
        _ => "This page could not be read.",
    };
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>{}</title><p>{msg}</p>",
        status.canonical_reason().unwrap_or("Error")
    );
    let mut r = Response::builder().status(status);
    let h = r.headers_mut().unwrap();
    zone_headers(h, cfg);
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r.body(Body::from(html)).unwrap()
}

fn redirect(cfg: &ZoneConfig, status: StatusCode, to: &str) -> Response<Body> {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
        .add(b' ').add(b'"').add(b'#').add(b'%').add(b'<').add(b'>').add(b'?').add(b'`').add(b'{').add(b'}');
    let loc = format!("/{}", percent_encoding::utf8_percent_encode(to.trim_start_matches('/'), SET));
    let mut r = Response::builder().status(status);
    let h = r.headers_mut().unwrap();
    zone_headers(h, cfg);
    h.insert(header::LOCATION, hv(&loc));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r.body(Body::empty()).unwrap()
}

/// Whether `Accept-Encoding` admits `zstd` with a non-zero q.
pub fn accepts_zstd(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|item| {
            let mut parts = item.split(';');
            let name = parts.next().unwrap_or("").trim();
            if !name.eq_ignore_ascii_case("zstd") {
                return false;
            }
            !parts.any(|p| {
                let p = p.trim();
                p.strip_prefix("q=").or_else(|| p.strip_prefix("Q=")).is_some_and(|q| q.trim().parse::<f32>().map(|q| q <= 0.0).unwrap_or(true))
            })
        })
}

/// `If-None-Match` against one strong tag (weak comparison, RFC 9110 §13.1.2).
pub fn etag_matches(header: &str, tag: &str) -> bool {
    let tag = tag.trim_start_matches("W/");
    header.split(',').map(str::trim).any(|t| t == "*" || t.trim_start_matches("W/") == tag)
}

/// `If-Range`: the range applies only if the tag still matches (strong).
fn if_range_ok(headers: &HeaderMap, etag: Option<&str>) -> bool {
    match headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok()) {
        None => true,
        Some(v) => etag.is_some_and(|t| v.trim() == t),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RangeSpec {
    /// Serve the whole entry (absent, malformed, or several ranges).
    Ignore,
    Unsatisfiable,
    /// Inclusive byte span.
    Span(u64, u64),
}

/// One `bytes=` range against an entry of `total` bytes. Several ranges are
/// answered with the whole entry, which RFC 9110 §14.2 allows.
pub fn parse_range(v: &str, total: u64) -> RangeSpec {
    let Some(spec) = v.trim().strip_prefix("bytes=") else { return RangeSpec::Ignore };
    if spec.contains(',') {
        return RangeSpec::Ignore;
    }
    let Some((a, z)) = spec.trim().split_once('-') else { return RangeSpec::Ignore };
    let (a, z) = (a.trim(), z.trim());
    let num = |s: &str| (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<u64>().ok()).flatten();
    match (a.is_empty(), z.is_empty()) {
        (true, true) => RangeSpec::Ignore,
        (true, false) => match num(z) {
            None => RangeSpec::Ignore,
            Some(0) => RangeSpec::Unsatisfiable,
            Some(_) if total == 0 => RangeSpec::Unsatisfiable,
            Some(n) => RangeSpec::Span(total.saturating_sub(n), total - 1),
        },
        (false, _) => {
            let Some(s) = num(a) else { return RangeSpec::Ignore };
            let e = if z.is_empty() { Some(u64::MAX) } else { num(z) };
            let Some(e) = e else { return RangeSpec::Ignore };
            if e < s {
                return RangeSpec::Ignore;
            }
            if s >= total {
                return RangeSpec::Unsatisfiable;
            }
            RangeSpec::Span(s, e.min(total - 1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), RangeSpec::Span(0, 9));
        assert_eq!(parse_range("bytes=90-", 100), RangeSpec::Span(90, 99));
        assert_eq!(parse_range("bytes=-10", 100), RangeSpec::Span(90, 99));
        assert_eq!(parse_range("bytes=-1000", 100), RangeSpec::Span(0, 99));
        assert_eq!(parse_range("bytes=50-1000", 100), RangeSpec::Span(50, 99));
        assert_eq!(parse_range("bytes=100-", 100), RangeSpec::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 100), RangeSpec::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), RangeSpec::Ignore);
        assert_eq!(parse_range("bytes=9-0", 100), RangeSpec::Ignore);
        assert_eq!(parse_range("items=0-1", 100), RangeSpec::Ignore);
        assert_eq!(parse_range("bytes=a-1", 100), RangeSpec::Ignore);
    }

    #[test]
    fn zstd_negotiation() {
        let h = |v: &str| {
            let mut m = HeaderMap::new();
            m.insert(header::ACCEPT_ENCODING, HeaderValue::from_str(v).unwrap());
            m
        };
        assert!(accepts_zstd(&h("gzip, deflate, br, zstd")));
        assert!(accepts_zstd(&h("zstd;q=0.5")));
        assert!(!accepts_zstd(&h("zstd;q=0")));
        assert!(!accepts_zstd(&h("gzip, br")));
        assert!(!accepts_zstd(&HeaderMap::new()));
    }

    #[test]
    fn etag_comparison() {
        assert!(etag_matches("\"abc\"", "\"abc\""));
        assert!(etag_matches("W/\"abc\", \"x\"", "\"abc\""));
        assert!(etag_matches("*", "\"abc\""));
        assert!(!etag_matches("\"abc-zst\"", "\"abc\""));
    }
}
