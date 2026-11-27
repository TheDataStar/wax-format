//! deltd API §8 end to end: progress events from the WebOS and the box's
//! own `entry_viewed`, always credited to a live session's person and always
//! naming the pack from a pack hostname.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use deltlib::{router, AppState, Catalog, Identity, Library, Session, StaticIdentity};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wax_core::model::Compression;
use wax_core::writer::{EntryMeta, WaxWriter};

const WEBOS: &str = "home.deltos.lan";
const ORIGIN: &str = "http://home.deltos.lan";
const ALICE: &str = "aaaaaaaaaaaaaaaa";
const BOB: &str = "bbbbbbbbbbbbbbbb";
const ALICE_OTHER_BROWSER: &str = "cccccccccccccccc";
const GUEST: &str = "6666666666666666";
const UUID: [u8; 16] = [0x10, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1];
const ID: &str = "10000000-0000-4000-8000-000000000001";

struct Box_ {
    _dir: tempfile::TempDir,
    lib: Arc<Library>,
    ident: Arc<StaticIdentity>,
    app: axum::Router,
}

fn session(profile: &str, browser: &str, guest: bool) -> Session {
    Session { profile_id: profile.into(), browser_id: browser.into(), guest, csrf: format!("csrf-{profile}"), ..Default::default() }
}

fn boot() -> Box_ {
    let dir = tempfile::tempdir().unwrap();
    let packs = dir.path().join("packs");
    std::fs::create_dir(&packs).unwrap();
    let m = BTreeMap::from([("name".to_string(), "Science".to_string()), ("guest_accessible".to_string(), "true".to_string())]);
    let mut sw = WaxWriter::new(UUID).created_at(1_700_000_000).create(packs.join("s.wax"), &m).unwrap();
    for (p, mime) in [("A.html", "text/html"), ("B.html", "text/html"), ("s.css", "text/css")] {
        sw.add_entry(EntryMeta::new(p).mime(mime).compression(Compression::Zstd), &mut &b"<p>x</p>"[..]).unwrap();
    }
    sw.finish().unwrap();
    let lib = Arc::new(Library::new(&packs, Catalog::in_memory().unwrap(), 1));
    lib.refresh().unwrap();
    let mut id = StaticIdentity::new();
    id.add("alice", session("alice", "b1", false), ALICE).unwrap();
    id.add("alice2", session("alice", "b2", false), ALICE_OTHER_BROWSER).unwrap();
    id.add("bob", session("bob", "b3", false), BOB).unwrap();
    id.add("guest", session("guest", "b4", true), GUEST).unwrap();
    let ident = Arc::new(id);
    let dynid: Arc<dyn Identity> = Arc::clone(&ident) as Arc<dyn Identity>;
    let app = router(AppState::new(Arc::clone(&lib), dynid, WEBOS));
    Box_ { _dir: dir, lib, ident, app }
}

impl Box_ {
    fn host(&self, part: &str) -> String {
        let snap = self.lib.snapshot();
        let p = &snap.by_uuid[ID];
        format!("pack-{}-g{}-{part}.deltos-packs.lan", p.slot.pack_index, p.slot.pack_generation)
    }
    async fn post(&self, who: &str, csrf: Option<&str>, b: Value) -> (StatusCode, Value) {
        let mut r = Request::builder()
            .method("POST")
            .uri("/api/v1/progress-events")
            .header("host", WEBOS)
            .header("origin", ORIGIN)
            .header("cookie", format!("delt_session={who}"))
            .header("content-type", "application/json");
        if let Some(c) = csrf {
            r = r.header("x-delt-csrf", c);
        }
        let resp = self.app.clone().oneshot(r.body(Body::from(b.to_string())).unwrap()).await.unwrap();
        let st = resp.status();
        let v = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap_or(Value::Null);
        (st, v)
    }
    async fn send(&self, who: &str, b: Value) -> (StatusCode, Value) {
        let csrf = format!("csrf-{}", if who.starts_with("alice") { "alice" } else { who });
        self.post(who, Some(&csrf), b).await
    }
    async fn read(&self, part: &str, path: &str) -> StatusCode {
        let r = Request::builder().uri(path).header("host", self.host(part)).body(Body::empty()).unwrap();
        self.app.clone().oneshot(r).await.unwrap().status()
    }
    async fn wait(&self, n: usize) -> Vec<(String, Value)> {
        for _ in 0..100 {
            let v = self.ident.recorded_progress();
            if v.len() >= n {
                return v;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        self.ident.recorded_progress()
    }
}

#[tokio::test]
async fn the_webos_reports_for_the_signed_in_person_and_the_box_names_the_pack() {
    let b = boot();
    let ev = json!({"event_id": "w1", "event_type": "pack_opened", "pack_hostname": b.host(ALICE),
                    "pack_id": "someone-elses", "profile_id": "bob", "payload": {"why": "test"}});
    let (st, v) = b.send("alice", ev.clone()).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!((v["recorded"].as_bool(), v["seq"].as_i64()), (Some(true), Some(1)));
    let (_, again) = b.send("alice", ev).await;
    assert_eq!(again["recorded"], false, "a retry records nothing new");
    let rec = b.ident.recorded_progress();
    assert_eq!(rec.len(), 1);
    assert_eq!(rec[0].0, "alice", "the session's person, never the event's");
    assert_eq!(rec[0].1["pack_id"], ID, "the pack from the hostname, never the event");
    assert_eq!(rec[0].1["event_source"], "wax_pack");
    assert_eq!(rec[0].1["payload"], json!({"why": "test"}));

    // a quiz score relayed from the frame: an unknown-to-rollups type still passes
    let (st, _) = b.send("alice", json!({"event_id": "w2", "event_type": "quiz_scored", "pack_hostname": b.host(ALICE), "payload": {"score": 7}})).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = b.send("alice", json!({"event_id": "w3", "event_type": "new_kind", "pack_hostname": b.host(ALICE)})).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn only_ones_own_frame_on_ones_own_browser_counts() {
    let b = boot();
    let ev = |part: &str, id: &str| json!({"event_id": id, "event_type": "unit_completed", "pack_hostname": b.host(part)});
    let (st, v) = b.send("alice", ev(BOB, "x1")).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("request_rejected")), "Bob's frame");
    let (st, _) = b.send("alice", ev(ALICE_OTHER_BROWSER, "x2")).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "Alice's frame on another browser");
    let (st, _) = b.send("alice", ev("dddddddddddddddd", "x3")).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "a partition nobody holds");
    let (st, v) = b.send("guest", ev(GUEST, "x4")).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("guest_not_allowed")));
    let (st, _) = b.post("alice", None, ev(ALICE, "x5")).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "no anti-forgery token");
    let (st, v) = b.send("alice", json!({"event_id": "x6", "event_type": "entry_viewed", "pack_hostname": b.host(ALICE)})).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_request")), "the box records entry_viewed itself");
    let (st, _) = b.send("alice", json!({"event_id": "x7", "event_type": "pack_opened", "pack_hostname": "example.com"})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = b.send("alice", json!({"event_type": "pack_opened", "pack_hostname": b.host(ALICE)})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "event_id is required");
    assert!(b.ident.recorded_progress().is_empty(), "nothing was recorded");
}

#[tokio::test]
async fn pages_served_are_entry_viewed_for_their_reader() {
    let b = boot();
    assert_eq!(b.read(ALICE, "/A.html").await, StatusCode::OK);
    assert_eq!(b.read(ALICE, "/s.css").await, StatusCode::OK);
    assert_eq!(b.read(BOB, "/B.html").await, StatusCode::OK);
    assert_eq!(b.read(GUEST, "/A.html").await, StatusCode::OK);
    let rec = b.wait(2).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let mut rec2 = b.ident.recorded_progress();
    assert_eq!(rec.len(), rec2.len(), "style sheets and Guest record nothing");
    rec2.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(rec2.iter().map(|(p, e)| (p.as_str(), e["payload"]["path"].as_str().unwrap())).collect::<Vec<_>>(), [("alice", "A.html"), ("bob", "B.html")]);
    assert!(rec2.iter().all(|(_, e)| e["event_type"] == "entry_viewed" && e["pack_id"] == ID));
    assert_ne!(rec2[0].1["event_id"], rec2[1].1["event_id"]);
}
