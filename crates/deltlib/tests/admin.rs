//! The admin endpoints (deltd API §6.8) end to end: the real router, the real
//! `deltos-identityd` register on its two sockets (one for deltd, one for
//! shelld), and the real `deltos-shelld` on its own. deltd checks first;
//! shelld checks everything again; the action log shows who did what.

use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use deltlib::admin::{Shell, ShelldLink};
use deltlib::{router, AppState, Catalog, ConnInfo, IdentitydLink, Library};
use deltos_identity::protocol::{self, Callers};
use deltos_identity::Register;
use deltos_shelld::drives::DrivesDir;
use deltos_shelld::log::ActionLog;
use deltos_shelld::packs::{self, Helper};
use deltos_shelld::{addons, Config, NetPaths};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const WEBOS: &str = "home.deltos.lan";
const LAPTOP: ConnInfo = ConnInfo { https: true, peer: Some(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 24))) };
const PLAIN: ConnInfo = ConnInfo { https: false, peer: Some(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 24))) };
const PACK_ID: &str = "10000000-0000-4000-8000-000000000001";

fn me() -> u32 {
    unsafe { libc::getuid() }
}

struct Box_ {
    dir: tempfile::TempDir,
    app: axum::Router,
    admin_pw: String,
}

/// One signed-in browser: its cookie and anti-forgery token.
struct Who {
    cookie: String,
    csrf: String,
}

fn boot_with(shell: bool) -> Box_ {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let packs_dir = root.join("packs");
    std::fs::create_dir(&packs_dir).unwrap();
    let lib = Arc::new(Library::new(&packs_dir, Catalog::in_memory().unwrap(), 1));
    let reg = Arc::new(Register::open(&root.join("identity"), WEBOS).unwrap());
    let (_, admin_pw) = reg.provision_first_admin("Admin", "admin").unwrap();
    let (id_deltd, id_shelld) = (root.join("id-deltd.sock"), root.join("id-shelld.sock"));
    let (r1, r2) = (Arc::clone(&reg), Arc::clone(&reg));
    let (l1, l2) = (UnixListener::bind(&id_deltd).unwrap(), UnixListener::bind(&id_shelld).unwrap());
    std::thread::spawn(move || protocol::serve(l1, r1, Callers { deltd: vec![me()], shelld: vec![] }));
    std::thread::spawn(move || protocol::serve(l2, r2, Callers { deltd: vec![], shelld: vec![me()] }));

    let sock = root.join("shelld.sock");
    if shell {
        // a hotspot box, as shelld's own tests set one up
        let mut nr = deltos_net::Registry::open(&root.join("registry.sqlite")).unwrap();
        nr.set_settings(&deltos_net::Settings {
            mode: deltos_net::Mode::Hotspot { interface: "wlan0".into(), box_address: "10.42.0.1".parse().unwrap(), prefix_len: 24 },
            uplink_interface: Some("eth0".into()),
            upstream_dns: vec![],
            uplink_sharing: false,
        })
        .unwrap();
        drop(nr);
        let lay = packs::Layout::under(root);
        std::fs::create_dir_all(&lay.packs).unwrap();
        let cfg = Config {
            identityd: deltos_identity::Client::with_connections(&id_shelld, 2),
            log: ActionLog::open(&root.join("actions.log")).unwrap(),
            deltd_uids: vec![me()],
            net: NetPaths { registry: root.join("registry.sqlite"), settings: deltos_net::Layout::new(root.join("run-network")) },
            backup: deltos_backup::Layout::at(&root.join("var/lib/deltos")),
            addons: addons::Layout::under(root),
            addon_folders: vec![],
            drives: DrivesDir(root.join("drives")),
            trust: addons::Trust::none(),
            packs: lay,
            pack_trust: packs::Trust::none(),
            pack_check: Helper { program: "/bin/false".into(), user: None },
            pack_data_owner: None,
            pack_folders: vec![("incoming".into(), root.join("incoming"))],
        };
        let s = deltos_shelld::Shell::new(cfg, Box::new(deltos_backup::restore::NoServices));
        let l = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || deltos_shelld::serve(l, s));
    }
    let link: Option<Arc<dyn Shell>> = shell.then(|| Arc::new(ShelldLink::new(&sock)) as Arc<dyn Shell>);
    let app = router(AppState::new(lib, Arc::new(IdentitydLink::new(&id_deltd)), WEBOS).with_https(true).with_shell(link));
    Box_ { dir, app, admin_pw }
}

fn boot() -> Box_ {
    boot_with(true)
}

impl Box_ {
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn log(&self) -> String {
        std::fs::read_to_string(self.root().join("actions.log")).unwrap_or_default()
    }
    fn incoming(&self) -> PathBuf {
        let p = self.root().join("incoming");
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    async fn call(&self, conn: ConnInfo, method: &str, path: &str, who: Option<&Who>, csrf: bool, body: Option<Value>) -> (StatusCode, Value, axum::http::HeaderMap) {
        let origin = format!("{}://{WEBOS}", if conn.https { "https" } else { "http" });
        let mut r = Request::builder().method(method).uri(path).header("host", WEBOS);
        if let Some(w) = who {
            r = r.header("cookie", &w.cookie);
            if csrf && method != "GET" {
                r = r.header("x-delt-csrf", &w.csrf);
            }
        }
        if method != "GET" {
            r = r.header("origin", origin);
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
        let (parts, b) = resp.into_parts();
        let v = serde_json::from_slice(&b.collect().await.unwrap().to_bytes()).unwrap_or(Value::Null);
        (parts.status, v, parts.headers)
    }

    async fn sign_in(&self, name: &str, pw: &str) -> Who {
        let (st, v, h) = self.call(LAPTOP, "POST", "/api/v1/session", None, false, Some(json!({ "sign_in_name": name, "password": pw }))).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let cookie = h
            .get_all("set-cookie")
            .iter()
            .map(|c| c.to_str().unwrap().split(';').next().unwrap().to_string())
            .collect::<Vec<_>>()
            .join("; ");
        Who { cookie, csrf: v["csrf_token"].as_str().unwrap().to_string() }
    }

    async fn admin(&self) -> Who {
        let pw = self.admin_pw.clone();
        self.sign_in("admin", &pw).await
    }

    async fn teacher(&self, admin: &Who) -> Who {
        let (st, v, _) = self
            .call(LAPTOP, "POST", "/api/v1/profiles", Some(admin), true, Some(json!({
                "display_name": "Teacher", "kind": "passworded", "sign_in_name": "teacher",
                "password": "teacher-password-1", "roles": [{ "role": "teacher" }] })))
            .await;
        assert!(st.is_success(), "{st} {v}");
        self.sign_in("teacher", "teacher-password-1").await
    }
}

#[tokio::test]
async fn an_admin_changes_the_box_through_shelld_and_the_log_names_them() {
    let b = boot();
    let a = b.admin().await;
    let name = json!({ "name": "printer.deltos.lan", "target": "10.42.0.20", "requested_by": "p1" });
    let (st, v, h) = b.call(LAPTOP, "POST", "/api/v1/network/names", Some(&a), true, Some(name)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(h["cache-control"], "no-store");
    let names = std::fs::read_to_string(b.root().join("run-network/names.d/names")).unwrap_or_default();
    assert!(names.contains("printer"), "shelld did it: {names}");

    let (st, v, _) = b.call(LAPTOP, "GET", "/api/v1/audit", Some(&a), true, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let e = v["entries"].as_array().unwrap();
    let last = e.iter().rev().find(|x| x["action"] == "approve_name").expect("logged");
    assert_eq!(last["result"], "ok");
    assert!(last["by"].as_str().is_some_and(|p| p != "unknown"), "{last}");

    // the path's own value goes to the action, and cannot be overridden from the body
    let (st, _, _) = b.call(LAPTOP, "DELETE", "/api/v1/network/names/printer.deltos.lan", Some(&a), true, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, v, _) =
        b.call(LAPTOP, "DELETE", "/api/v1/network/names/printer.deltos.lan", Some(&a), true, Some(json!({ "name": "other.deltos.lan" }))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_request")));
    // a path value is percent-decoded before it is an argument
    let (st, v, _) = b
        .call(LAPTOP, "PUT", "/api/v1/network/reservations/02%3A00%3A00%3A00%3A00%3A01", Some(&a), true, Some(json!({ "address": "10.42.0.50", "label": "printer" })))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(b.log().contains("02:00:00:00:00:01"), "{}", b.log());
    // an argument outside the action's shape is shelld's to refuse
    let (st, v, _) = b.call(LAPTOP, "PUT", "/api/v1/network/uplink-sharing", Some(&a), true, Some(json!({ "on": true, "also": 1, "confirm_password": b.admin_pw }))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_request")), "{v}");
}

#[tokio::test]
async fn deltd_refuses_before_shelld_is_asked() {
    let b = boot();
    let a = b.admin().await;
    let t = b.teacher(&a).await;
    let before = b.log().lines().count();
    let args = json!({ "name": "printer.deltos.lan", "target": "10.42.0.20", "requested_by": "p1" });
    // no session; no anti-forgery token; the HTTPS session over plain HTTP; a teacher
    let (st, v, _) = b.call(LAPTOP, "POST", "/api/v1/network/names", None, true, Some(args.clone())).await;
    assert_eq!(v["code"], "not_signed_in", "{st}");
    let (_, v, _) = b.call(LAPTOP, "POST", "/api/v1/network/names", Some(&a), false, Some(args.clone())).await;
    assert_eq!(v["code"], "request_rejected");
    let (_, v, _) = b.call(PLAIN, "POST", "/api/v1/network/names", Some(&a), true, Some(args.clone())).await;
    assert_eq!(v["code"], "secure_connection_required");
    let (_, v, _) = b.call(PLAIN, "GET", "/api/v1/audit", Some(&a), true, None).await;
    assert_eq!(v["code"], "secure_connection_required");
    let (st, v, _) = b.call(LAPTOP, "POST", "/api/v1/network/names", Some(&t), true, Some(args)).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("permission_denied")));
    let (_, v, _) = b.call(LAPTOP, "GET", "/api/v1/audit", Some(&t), true, None).await;
    assert_eq!(v["code"], "permission_denied");
    assert_eq!(b.log().lines().count(), before, "none of these reached shelld:\n{}", b.log());
    // a wrong method says which ones are right
    let (st, _, h) = b.call(LAPTOP, "GET", "/api/v1/network/hotspot", Some(&a), true, None).await;
    assert_eq!((st, h["allow"].to_str().unwrap()), (StatusCode::METHOD_NOT_ALLOWED, "PUT"));
}

#[tokio::test]
async fn an_action_that_asks_again_needs_the_password_and_sends_only_a_ticket() {
    let b = boot();
    let a = b.admin().await;
    let on = |pw: Option<&str>| match pw {
        Some(p) => json!({ "on": true, "confirm_password": p }),
        None => json!({ "on": true }),
    };
    let (st, v, _) = b.call(LAPTOP, "PUT", "/api/v1/network/uplink-sharing", Some(&a), true, Some(on(None))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("confirmation_required")), "{v}");
    let (st, v, _) = b.call(LAPTOP, "PUT", "/api/v1/network/uplink-sharing", Some(&a), true, Some(on(Some("not-the-password")))).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
    assert!(matches!(v["code"].as_str(), Some("confirmation_required")), "{v}");
    let pw = b.admin_pw.clone();
    let (st, v, _) = b.call(LAPTOP, "PUT", "/api/v1/network/uplink-sharing", Some(&a), true, Some(on(Some(&pw)))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let log = b.log();
    assert!(!log.contains(&pw), "the password never reaches shelld or its log");
    assert!(log.lines().any(|l| l.contains("set_uplink_sharing") && l.contains("\"result\":\"ok\"")), "{log}");

    // removing a pack asks again too; with the password it gets as far as the pack
    let path = format!("/api/v1/packs/{PACK_ID}");
    let (_, v, _) = b.call(LAPTOP, "DELETE", &path, Some(&a), true, None).await;
    assert_eq!(v["code"], "confirmation_required");
    let (st, v, _) = b.call(LAPTOP, "DELETE", &path, Some(&a), true, Some(json!({ "confirm_password": pw }))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("pack_not_found")), "{v}");
}

#[tokio::test]
async fn a_long_action_is_a_job_only_its_starter_can_read() {
    let b = boot();
    let a = b.admin().await;
    std::fs::write(b.incoming().join("science.wax"), b"not really a pack").unwrap();
    let (st, v, h) =
        b.call(LAPTOP, "POST", "/api/v1/installs", Some(&a), true, Some(json!({ "source": "incoming/science.wax", "archive_uuid": PACK_ID }))).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{v}");
    let job = v["job"].as_str().unwrap().to_string();
    assert_eq!(h["location"].to_str().unwrap(), format!("/api/v1/jobs/{job}"));
    let path = format!("/api/v1/jobs/{job}");
    let start = Instant::now();
    let done = loop {
        let (st, v, _) = b.call(LAPTOP, "GET", &path, Some(&a), true, None).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        if v["finished_at"].is_u64() {
            break v;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "stuck: {v}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_ne!(done["state"], "done", "no pack key on this box, so nothing installs: {done}");
    let t = b.teacher(&a).await;
    // a teacher is not the starter: the job is not there for them
    let (st, v, _) = b.call(LAPTOP, "GET", &path, Some(&t), true, None).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("job_not_found")), "{v}");
    let (st, v, _) = b.call(LAPTOP, "GET", "/api/v1/installs/sources", Some(&a), true, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(v["packs"].is_array(), "{v}");
}

#[tokio::test]
async fn without_shelld_the_admin_endpoints_say_so() {
    let b = boot_with(false);
    let a = b.admin().await;
    let (st, v, _) = b.call(LAPTOP, "GET", "/api/v1/audit", Some(&a), true, None).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("admin_unavailable")), "{v}");
}

#[tokio::test]
async fn the_machine_readable_contract_lists_every_admin_endpoint() {
    let b = boot_with(false);
    let (_, v, _) = b.call(LAPTOP, "GET", "/api/v1/openapi.json", None, false, None).await;
    let paths = v["paths"].as_object().unwrap();
    for (p, m) in [
        ("installs", "post"), ("installs/sources", "get"), ("packs/{uuid}", "delete"), ("packs/{uuid}/rollback", "post"),
        ("drives", "get"), ("backups", "post"), ("backups/{id}/review", "post"), ("backups/{id}/restore", "post"),
        ("network/leases", "get"), ("network/hotspot", "put"), ("network/names", "post"), ("network/names/{name}", "delete"),
        ("network/reservations/{mac}", "put"), ("network/reservations/{mac}", "delete"), ("network/uplink-sharing", "put"),
        ("apps", "get"), ("apps", "post"), ("apps/sources", "get"), ("apps/{id}", "delete"), ("apps/{id}/update", "post"),
        ("apps/{id}/rollback", "post"), ("apps/{id}/running", "put"), ("audit", "get"), ("jobs/{id}", "get"),
    ] {
        assert!(paths.get(&format!("/api/v1/{p}")).and_then(|x| x.get(m)).is_some(), "{m} /api/v1/{p} is not in openapi.json");
    }
}
