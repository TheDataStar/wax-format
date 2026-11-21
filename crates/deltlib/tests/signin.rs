//! deltd with the live register: the real `deltos-identityd` register on a
//! real Unix socket, the real router, and a real TLS listener. These are the
//! privilege boundary §13 tests that sign-in can answer before shelld exists.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use deltlib::{router, AppState, Catalog, ConnInfo, IdentitydLink, Library};
use deltos_identity::protocol::{self, Callers};
use deltos_identity::Register;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wax_core::model::Compression;
use wax_core::writer::{EntryMeta, WaxWriter};

const WEBOS: &str = "home.deltos.lan";
const PACK: [u8; 16] = [0x10, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1];
const PACK_ID: &str = "10000000-0000-4000-8000-000000000001";

const PHONE: ConnInfo = ConnInfo { https: false, peer: Some(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 23))) };
const LAPTOP: ConnInfo = ConnInfo { https: true, peer: Some(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 24))) };
const KIOSK: ConnInfo = ConnInfo { https: true, peer: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)) };

fn pack(dir: &Path) {
    let m = BTreeMap::from([
        ("name".to_string(), "Science".to_string()),
        ("languages".to_string(), "en".to_string()),
        ("entry_point".to_string(), "index.html".to_string()),
    ]);
    let mut sw = WaxWriter::new(PACK).created_at(1_700_000_000).create(dir.join("science.wax"), &m).unwrap();
    let page = "<!doctype html><title>Science</title><p>Evaporation.</p>";
    sw.add_entry(EntryMeta::new("index.html").mime("text/html").title("Science").compression(Compression::Zstd), &mut page.as_bytes())
        .unwrap();
    sw.finish().unwrap();
}

struct Box_ {
    _dir: tempfile::TempDir,
    app: axum::Router,
    lib: Arc<Library>,
    admin_pw: String,
    sock: PathBuf,
}

/// A browser: keeps the cookies the box sets, sends them back, and sends
/// the anti-forgery token once it has one.
#[derive(Default, Clone)]
struct Browser {
    cookies: BTreeMap<String, String>,
    csrf: Option<String>,
}

impl Browser {
    fn cookie_header(&self) -> String {
        self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
    }
    fn keep(&mut self, r: &Response<Body>) {
        for c in r.headers().get_all("set-cookie") {
            let c = c.to_str().unwrap();
            let (kv, _) = c.split_once(';').unwrap();
            let (k, v) = kv.split_once('=').unwrap();
            if c.contains("Max-Age=0") {
                self.cookies.remove(k);
            } else {
                self.cookies.insert(k.into(), v.into());
            }
        }
    }
}

fn boot() -> Box_ {
    let dir = tempfile::tempdir().unwrap();
    let packs = dir.path().join("packs");
    std::fs::create_dir(&packs).unwrap();
    pack(&packs);
    let lib = Arc::new(Library::new(&packs, Catalog::in_memory().unwrap(), 2));
    lib.refresh().unwrap();
    let reg = Arc::new(Register::open(&dir.path().join("identity"), WEBOS).unwrap());
    let (_, admin_pw) = reg.provision_first_admin("Admin", "admin").unwrap();
    let sock = dir.path().join("identityd.sock");
    let l = UnixListener::bind(&sock).unwrap();
    let me = unsafe { libc::getuid() };
    std::thread::spawn(move || protocol::serve(l, reg, Callers { deltd: vec![me], shelld: vec![] }));
    let app = router(AppState::new(Arc::clone(&lib), Arc::new(IdentitydLink::new(&sock)), WEBOS).with_https(true));
    Box_ { _dir: dir, app, lib, admin_pw, sock }
}

fn origin(conn: &ConnInfo) -> String {
    format!("{}://{WEBOS}", if conn.https { "https" } else { "http" })
}

impl Box_ {
    async fn req(
        &self,
        b: &mut Browser,
        conn: ConnInfo,
        method: &str,
        path: &str,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> (StatusCode, Value, Response<Body>) {
        let mut r = Request::builder().method(method).uri(path).header("host", WEBOS);
        if !b.cookies.is_empty() {
            r = r.header("cookie", b.cookie_header());
        }
        if method != "GET" {
            r = r.header("origin", origin(&conn));
            if let Some(t) = &b.csrf {
                r = r.header("x-delt-csrf", t);
            }
        }
        for (k, v) in extra {
            r = r.header(*k, *v);
        }
        let body = match body {
            Some(v) => {
                r = r.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let mut req = r.body(body).unwrap();
        req.extensions_mut().insert(conn);
        let resp = self.app.clone().oneshot(req).await.unwrap();
        b.keep(&resp);
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if let Some(t) = v.get("csrf_token").and_then(Value::as_str) {
            b.csrf = Some(t.to_string());
        }
        (parts.status, v, Response::from_parts(parts, Body::empty()))
    }

    async fn admin(&self) -> Browser {
        let mut b = Browser::default();
        let (s, v, _) = self
            .req(&mut b, LAPTOP, "POST", "/api/v1/session", Some(json!({ "sign_in_name": "admin", "password": self.admin_pw })), &[])
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        b
    }

    /// Admin makes a teacher of class 5a; the teacher signs in on a laptop.
    async fn teacher(&self) -> Browser {
        let mut a = self.admin().await;
        let p = json!({ "display_name": "Ms Okafor", "kind": "passworded", "sign_in_name": "okafor",
                        "password": "teacher password 1", "class_or_group": "5a",
                        "roles": [{ "role": "teacher", "scope": "5a" }] });
        let (s, v, _) = self.req(&mut a, LAPTOP, "POST", "/api/v1/profiles", Some(p), &[]).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        let mut t = Browser::default();
        let (s, _, _) = self
            .req(&mut t, LAPTOP, "POST", "/api/v1/session", Some(json!({ "sign_in_name": "okafor", "password": "teacher password 1" })), &[])
            .await;
        assert_eq!(s, StatusCode::OK);
        t
    }

    async fn student(&self, t: &mut Browser, name: &str) -> String {
        let p = json!({ "display_name": name, "kind": "passwordless", "class_or_group": "5a",
                        "roles": [{ "role": "student" }] });
        let (s, v, _) = self.req(t, LAPTOP, "POST", "/api/v1/profiles", Some(p), &[]).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        v["profile_id"].as_str().unwrap().to_string()
    }

    async fn card(&self, t: &mut Browser, pid: &str) -> String {
        let (s, v, _) = self.req(t, LAPTOP, "POST", &format!("/api/v1/classes/5a/members/{pid}/sign-in-card"), None, &[]).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        v["code"].as_str().unwrap().to_string()
    }

    fn pack_host(&self, partition: &str) -> String {
        let snap = self.lib.snapshot();
        let p = &snap.by_uuid[PACK_ID];
        format!("pack-{}-g{}-{partition}.deltos-packs.lan", p.slot.pack_index, p.slot.pack_generation)
    }

    async fn pack_page(&self, host: &str) -> StatusCode {
        let req = Request::builder().uri("/index.html").header("host", host).body(Body::empty()).unwrap();
        self.app.clone().oneshot(req).await.unwrap().status()
    }
}

#[tokio::test]
async fn a_student_signs_in_on_a_phone_with_a_card_reads_and_signs_out() {
    let b = boot();
    let mut t = b.teacher().await;
    let amara = b.student(&mut t, "Amara").await;
    let code = b.card(&mut t, &amara).await;

    let mut phone = Browser::default();
    let (s, v, r) = b.req(&mut phone, PHONE, "POST", "/api/v1/session", Some(json!({ "sign_in_code": code })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["secure"], false);
    assert_eq!(v["profile_id"], amara.as_str());
    assert!(v.get("browser").is_none(), "the browser id's hash is never shown");
    // the three cookies, each host-only, HttpOnly, SameSite=Strict
    let set: Vec<String> = r.headers().get_all("set-cookie").iter().map(|c| c.to_str().unwrap().to_string()).collect();
    for name in ["delt_session=", "delt_browser=", "delt_device="] {
        let c = set.iter().find(|c| c.starts_with(name)).unwrap_or_else(|| panic!("no {name} in {set:?}"));
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict") && c.contains("Path=/"), "{c}");
        assert!(!c.to_ascii_lowercase().contains("domain="), "{c}");
        assert!(!c.contains("Secure"), "plain HTTP: {c}");
    }
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    assert!(r.headers().get("strict-transport-security").is_none(), "no HSTS over plain HTTP");

    // reading works, and the pack hostname is this person's on this phone
    let (s, v, _) = b.req(&mut phone, PHONE, "GET", &format!("/api/v1/packs/{PACK_ID}"), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let host = v["pack_hostname"].as_str().unwrap().to_string();
    let partition = host.split('-').nth(3).unwrap().split('.').next().unwrap().to_string();
    assert_eq!(host, b.pack_host(&partition));
    assert_eq!(b.pack_page(&host).await, StatusCode::OK);

    // signing out shuts that hostname, at once
    let (s, _, _) = b.req(&mut phone, PHONE, "DELETE", "/api/v1/session", None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_ne!(b.pack_page(&host).await, StatusCode::OK);
    let (s, _, _) = b.req(&mut phone, PHONE, "GET", "/api/v1/packs", None, &[]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // the phone is remembered: a new session with no card, same partition
    phone.csrf = None;
    let (s, v, _) = b.req(&mut phone, PHONE, "POST", "/api/v1/session", None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["profile_id"], amara.as_str());
    assert_eq!(b.pack_page(&host).await, StatusCode::OK, "same browser, same person: same partition");

    // the teacher removes the device: the phone is out now
    let (s, v, _) = b.req(&mut t, LAPTOP, "GET", &format!("/api/v1/classes/5a/members/{amara}/devices"), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let dev = v["items"][0]["device_id"].as_str().unwrap().to_string();
    let (s, _, _) = b.req(&mut t, LAPTOP, "DELETE", &format!("/api/v1/classes/5a/members/{amara}/devices/{dev}"), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_ne!(b.pack_page(&host).await, StatusCode::OK);
    phone.csrf = None;
    let (s, _, _) = b.req(&mut phone, PHONE, "POST", "/api/v1/session", None, &[]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn teacher_and_admin_passwords_are_refused_over_plain_http() {
    let b = boot();
    let mut phone = Browser::default();
    let (s, v, _) = b
        .req(&mut phone, PHONE, "POST", "/api/v1/session", Some(json!({ "sign_in_name": "admin", "password": b.admin_pw })), &[])
        .await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("secure_connection_required")));
    // the WebOS can ask first, before any password is typed
    let (s, v, _) = b.req(&mut phone, PHONE, "GET", "/api/v1/sign-in/check?sign_in_name=admin", None, &[]).await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("secure_connection_required")));
    let (s, v, _) = b.req(&mut phone, LAPTOP, "GET", "/api/v1/sign-in/check?sign_in_name=admin", None, &[]).await;
    assert_eq!((s, &v["password_allowed"]), (StatusCode::OK, &json!(true)));
}

#[tokio::test]
async fn https_sign_in_sets_host_prefixed_secure_cookies_and_hsts() {
    let b = boot();
    let mut a = Browser::default();
    let (s, v, r) = b
        .req(&mut a, LAPTOP, "POST", "/api/v1/session", Some(json!({ "sign_in_name": "admin", "password": b.admin_pw })), &[])
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["secure"], true);
    assert_eq!(v["all_permissions"], true);
    let set: Vec<String> = r.headers().get_all("set-cookie").iter().map(|c| c.to_str().unwrap().to_string()).collect();
    assert!(set.iter().any(|c| c.starts_with("__Host-delt_session=") && c.contains("; Secure;")), "{set:?}");
    assert!(set.iter().all(|c| c.starts_with("__Host-")), "{set:?}");
    assert_eq!(r.headers().get("strict-transport-security").unwrap(), "max-age=31536000");
    let (s, v, _) = b.req(&mut a, LAPTOP, "GET", "/api/v1/session", None, &[]).await;
    assert_eq!((s, &v["secure"]), (StatusCode::OK, &json!(true)));
}

#[tokio::test]
async fn a_plain_session_replayed_over_https_cannot_do_secure_things() {
    let b = boot();
    let mut t = b.teacher().await;
    let amara = b.student(&mut t, "Amara").await;
    let code = b.card(&mut t, &amara).await;
    let mut phone = Browser::default();
    b.req(&mut phone, PHONE, "POST", "/api/v1/session", Some(json!({ "sign_in_code": code })), &[]).await;
    // someone copies the plain cookie off the Wi-Fi and replays it over HTTPS
    let (s, v, _) = b.req(&mut phone, LAPTOP, "POST", &format!("/api/v1/classes/5a/members/{amara}/sign-in-card"), None, &[]).await;
    // (its Origin is now the https one, the token is right: only the session's kind refuses it)
    assert_eq!((s, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("secure_connection_required")), "{v}");
    // and a secure session over plain HTTP is refused the same way
    let (s, v, _) = b.req(&mut t, PHONE, "POST", &format!("/api/v1/classes/5a/members/{amara}/sign-in-card"), None, &[]).await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("secure_connection_required")), "{v}");
}

#[tokio::test]
async fn writes_without_the_right_origin_or_token_are_rejected() {
    let b = boot();
    let mut t = b.teacher().await;
    let amara = b.student(&mut t, "Amara").await;
    let path = format!("/api/v1/classes/5a/members/{amara}/sign-in-card");
    let pack_origin = format!("http://{}", b.pack_host("aaaaaaaaaaaaaaaa"));
    let cases: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("missing origin", None, t.csrf.as_deref()),
        ("other site", Some("https://evil.example"), t.csrf.as_deref()),
        ("plain-http origin on https", Some("http://home.deltos.lan"), t.csrf.as_deref()),
        ("pack origin", Some(pack_origin.as_str()), t.csrf.as_deref()),
        ("missing token", Some("https://home.deltos.lan"), None),
        ("wrong token", Some("https://home.deltos.lan"), Some("0000")),
    ];
    for (what, origin, csrf) in cases {
        let mut r = Request::builder().method("POST").uri(&path).header("host", WEBOS).header("cookie", t.cookie_header());
        if let Some(o) = origin {
            r = r.header("origin", o);
        }
        if let Some(c) = csrf {
            r = r.header("x-delt-csrf", c);
        }
        let mut req = r.body(Body::empty()).unwrap();
        req.extensions_mut().insert(LAPTOP);
        let resp = b.app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{what}");
        let v: Value = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(v["code"], "request_rejected", "{what}");
    }
    // sign-in itself needs the WebOS origin too
    let mut nobody = Browser::default();
    let req = {
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/v1/session")
            .header("host", WEBOS)
            .header("origin", "https://evil.example")
            .header("content-type", "application/json")
            .body(Body::from(json!({ "sign_in_name": "admin", "password": b.admin_pw }).to_string()))
            .unwrap();
        req.extensions_mut().insert(LAPTOP);
        req
    };
    assert_eq!(b.app.clone().oneshot(req).await.unwrap().status(), StatusCode::FORBIDDEN);
    let _ = &mut nobody;
}

#[tokio::test]
async fn the_tap_your_name_list_is_kiosk_only_and_switching_ends_the_first_person() {
    let b = boot();
    let mut t = b.teacher().await;
    let amara = b.student(&mut t, "Amara").await;
    let kofi = b.student(&mut t, "Kofi").await;
    let mut phone = Browser::default();
    let (s, _, _) = b.req(&mut phone, PHONE, "GET", "/api/v1/sign-in/profiles", None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = b.req(&mut phone, LAPTOP, "GET", "/api/v1/sign-in/profiles", None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = b.req(&mut phone, PHONE, "POST", "/api/v1/session", Some(json!({ "profile_id": amara })), &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let mut kiosk = Browser::default();
    let (s, v, _) = b.req(&mut kiosk, KIOSK, "GET", "/api/v1/sign-in/profiles", None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let (s, v, r) = b.req(&mut kiosk, KIOSK, "POST", "/api/v1/session", Some(json!({ "profile_id": amara })), &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((v["kiosk"].as_bool(), v["secure"].as_bool()), (Some(true), Some(false)));
    let sc = r.headers().get_all("set-cookie").iter().find(|c| c.to_str().unwrap().starts_with("__Host-delt_session=")).unwrap().to_str().unwrap().to_string();
    assert!(!sc.contains("Max-Age"), "a kiosk session cookie ends with the browser: {sc}");
    let first = kiosk.clone();
    let (s, v, _) = b.req(&mut kiosk, KIOSK, "POST", "/api/v1/session", Some(json!({ "profile_id": kofi })), &[]).await;
    assert_eq!((s, v["profile_id"].as_str()), (StatusCode::OK, Some(kofi.as_str())));
    let mut stale = first;
    let (s, _, _) = b.req(&mut stale, KIOSK, "GET", "/api/v1/packs", None, &[]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "Amara's session ended when Kofi signed in");
}

#[tokio::test]
async fn sign_in_answers_plainly_when_identityd_is_down() {
    let b = boot();
    std::fs::remove_file(&b.sock).unwrap();
    let mut phone = Browser::default();
    let (s, v, _) = b.req(&mut phone, PHONE, "POST", "/api/v1/session", Some(json!({ "sign_in_code": "ABCDE-FGHJK" })), &[]).await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("sign_in_unavailable")));
    let (s, _, _) = b.req(&mut phone, PHONE, "GET", "/api/v1/packs", None, &[("cookie", "delt_session=abc")]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "no register, nobody signed in");
}

/// A real HTTPS listener: the WebOS name gets the deltos.lan certificate,
/// a pack name the deltos-packs.lan one, and the request is known to be HTTPS.
#[tokio::test]
async fn the_https_listener_picks_the_certificate_by_zone_and_marks_requests_secure() {
    use rustls::pki_types::{CertificateDer, ServerName};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let b = boot();
    let certs = b._dir.path().join("tls");
    std::fs::create_dir(&certs).unwrap();
    let mut ders = BTreeMap::new();
    for (stem, name) in [("deltos.lan", "*.deltos.lan"), ("deltos-packs.lan", "*.deltos-packs.lan")] {
        let c = rcgen::generate_simple_self_signed(vec![name.to_string()]).unwrap();
        std::fs::write(certs.join(format!("{stem}.crt")), c.cert.pem()).unwrap();
        std::fs::write(certs.join(format!("{stem}.key")), c.key_pair.serialize_pem()).unwrap();
        ders.insert(stem, c.cert.der().clone());
    }
    let acc = deltlib::tls::acceptor(&certs).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(deltlib::tls::serve(l, b.app.clone(), Some(acc)));

    async fn fetch(addr: std::net::SocketAddr, sni: &str, root: &CertificateDer<'static>, req: String) -> String {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(root.clone()).unwrap();
        let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let conn = tokio_rustls::TlsConnector::from(Arc::new(cfg));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut s = conn.connect(ServerName::try_from(sni.to_string()).unwrap(), tcp).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.ok();
        String::from_utf8_lossy(&out).into_owned()
    }

    let body = json!({ "sign_in_name": "admin", "password": b.admin_pw }).to_string();
    let req = format!(
        "POST /api/v1/session HTTP/1.1\r\nHost: {WEBOS}\r\nOrigin: https://{WEBOS}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    // verified against the deltos.lan certificate only
    let resp = fetch(addr, WEBOS, &ders["deltos.lan"], req).await;
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("\"secure\":true"), "deltd knew it was HTTPS from the connection: {resp}");
    assert!(resp.contains("__Host-delt_session="), "{resp}");
    // a pack name is answered with the pack-zone certificate
    let host = b.pack_host("aaaaaaaaaaaaaaaa");
    let resp = fetch(addr, &host, &ders["deltos-packs.lan"], format!("GET /index.html HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")).await;
    assert!(resp.starts_with("HTTP/1.1 "), "{resp}");
}

/// The box's own CA, end to end: `deltos-tls` makes the certificates, deltd
/// loads them, a phone downloads the CA over plain HTTP, its fingerprint is
/// the one the box printed, and a client trusting only that file reaches
/// both zones over HTTPS.
#[tokio::test]
async fn a_device_that_trusts_the_downloaded_box_ca_reaches_both_zones() {
    use rustls::pki_types::{CertificateDer, ServerName};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let b = boot();
    let tls = b._dir.path().join("tls");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let made = deltos_tls::ensure(
        &deltos_tls::Paths { ca_dir: b._dir.path().join("ca"), tls_dir: tls.clone(), key_group: None },
        now.max(deltos_tls::CLOCK_FLOOR),
    )
    .unwrap();
    let app = router(
        AppState::new(Arc::clone(&b.lib), Arc::new(IdentitydLink::new(&b.sock)), WEBOS)
            .with_https(true)
            .with_ca_cert(Some(tls.join("ca.crt"))),
    );
    let get = |path: &str| {
        let mut r = Request::builder().uri(path).header("host", WEBOS).body(Body::empty()).unwrap();
        r.extensions_mut().insert(PHONE);
        app.clone().oneshot(r)
    };
    // over plain HTTP, no session: the fingerprint and the file
    let r = get("/api/v1/certificate").await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v: Value = serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(v["fingerprint_sha256"], made.ca_fingerprint);
    let r = get("/api/v1/certificate/ca.crt").await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()["content-type"], "application/x-x509-ca-cert");
    let der = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    assert_eq!(deltos_tls::fingerprint(&der), made.ca_fingerprint, "what the phone got is what the box printed");
    let ca = CertificateDer::from(der);

    let acc = deltlib::tls::acceptor(&tls).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(deltlib::tls::serve(l, app.clone(), Some(acc)));
    for host in [WEBOS.to_string(), b.pack_host("aaaaaaaaaaaaaaaa")] {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca.clone()).unwrap();
        let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut s = tokio_rustls::TlsConnector::from(Arc::new(cfg))
            .connect(ServerName::try_from(host.clone()).unwrap(), tcp)
            .await
            .unwrap_or_else(|e| panic!("{host}: {e}"));
        s.write_all(format!("GET /api/v1/certificate HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.ok();
        assert!(String::from_utf8_lossy(&out).starts_with("HTTP/1.1 "), "{host}");
    }
}
