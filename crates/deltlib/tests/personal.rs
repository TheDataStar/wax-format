//! deltd API §6.4 and §6.5 end to end: each person's bookmarks, highlights,
//! reading lists, history and export, and page outlines and reader mode,
//! through the real router against real packs on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use deltlib::personal::{Filter, Store};
use deltlib::{router, AppState, Catalog, Library, Session, StaticIdentity};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wax_core::model::Compression;
use wax_core::writer::{EntryMeta, WaxWriter};

const WEBOS: &str = "home.deltos.lan";
const ORIGIN: &str = "http://home.deltos.lan";
const ALICE_PART: &str = "aaaaaaaaaaaaaaaa";
const BOB_PART: &str = "bbbbbbbbbbbbbbbb";
const GUEST_PART: &str = "6666666666666666";
const UUID: [u8; 16] = [0x10, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1];
const OLD_UUID: [u8; 16] = [0x20, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 2];
const ID: &str = "10000000-0000-4000-8000-000000000001";
const OLD_ID: &str = "20000000-0000-4000-8000-000000000002";

fn manifest(name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("name".to_string(), name.to_string()),
        ("languages".to_string(), "en".to_string()),
        ("entry_point".to_string(), "index.html".to_string()),
        ("guest_accessible".to_string(), "true".to_string()),
    ])
}

fn page(title: &str, sections: &[(&str, &str)]) -> String {
    let mut s = format!("<html><head><title>{title}</title></head><body><p>Lead of {title}.</p>");
    for (h, body) in sections {
        s.push_str(&format!("<h2 id=\"{}\">{h}</h2><p>{body}</p>", h.replace(' ', "_")));
    }
    s.push_str("<img src=\"img/a.png\" alt=\"A picture\"></body></html>");
    s
}

fn add(sw: &mut wax_core::StreamingWriter, path: &str, title: &str, html: &str) {
    sw.add_entry(EntryMeta::new(path).mime("text/html").title(title).compression(Compression::Zstd), &mut html.as_bytes())
        .unwrap();
    sw.index_text(path, Some(title), &wax_core::text::html_to_text(html)).unwrap();
    sw.index_passages(path, html.as_bytes()).unwrap();
}

/// Edition 1: pages cut into passages, an alias, a style sheet.
fn science(dir: &Path) -> PathBuf {
    let out = dir.join("science.wax");
    let mut sw = WaxWriter::new(UUID).created_at(1_700_000_000).create(&out, &manifest("Science")).unwrap();
    add(&mut sw, "index.html", "Science", &page("Science", &[]));
    add(
        &mut sw,
        "Hydrogen.html",
        "Hydrogen",
        &page("Hydrogen", &[("History", "Hydrogen was named by Lavoisier."), ("Uses", "Hydrogen fuels rockets.")]),
    );
    add(&mut sw, "Helium.html", "Helium", &page("Helium", &[("Uses", "Helium fills balloons.")]));
    add(&mut sw, "Carbon.html", "Carbon", &page("Carbon", &[("Forms", "Diamond is carbon.")]));
    sw.add_redirect("H.html", "Hydrogen.html", None).unwrap();
    sw.add_entry(EntryMeta::new("style.css").mime("text/css").compression(Compression::Zstd), &mut &b"p{}"[..]).unwrap();
    sw.add_entry(EntryMeta::new("img/a.png").mime("image/png").compression(Compression::None), &mut &[7u8; 64][..])
        .unwrap();
    sw.finish().unwrap();
    out
}

/// Edition 2: Hydrogen's sentence about rockets moves to a new section and
/// the one about Lavoisier is rewritten; Carbon is deleted.
fn science_update(pack: &Path) {
    let mut sw = WaxWriter::new(UUID).created_at(1_700_100_000).open_append(pack).unwrap();
    add(
        &mut sw,
        "Hydrogen.html",
        "Hydrogen",
        &page("Hydrogen", &[("Discovery", "Cavendish found it in 1766."), ("Space", "Hydrogen fuels rockets.")]),
    );
    sw.add_tombstone("Carbon.html").unwrap();
    sw.finish().unwrap();
}

/// A pack built without passages (as every pack before v0.13).
fn old(dir: &Path) -> PathBuf {
    let out = dir.join("old.wax");
    let mut sw = WaxWriter::new(OLD_UUID).created_at(1_700_000_000).create(&out, &manifest("Old")).unwrap();
    let html = page("Old", &[("Part", "Old text here.")]);
    sw.add_entry(EntryMeta::new("index.html").mime("text/html").title("Old").compression(Compression::Zstd), &mut html.as_bytes())
        .unwrap();
    sw.finish().unwrap();
    out
}

struct Box_ {
    _dir: tempfile::TempDir,
    packs: PathBuf,
    lib: Arc<Library>,
    store: Arc<Store>,
    app: axum::Router,
}

fn session(profile: &str, guest: bool) -> Session {
    Session {
        profile_id: profile.into(),
        browser_id: format!("browser-{profile}"),
        guest,
        csrf: format!("csrf-{profile}"),
        ..Default::default()
    }
}

fn boot() -> Box_ {
    let dir = tempfile::tempdir().unwrap();
    let packs = dir.path().join("packs");
    std::fs::create_dir(&packs).unwrap();
    science(&packs);
    old(&packs);
    let lib = Arc::new(Library::new(&packs, Catalog::in_memory().unwrap(), 2));
    lib.refresh().unwrap();
    let mut id = StaticIdentity::new();
    id.add("alice", session("alice", false), ALICE_PART).unwrap();
    id.add("bob", session("bob", false), BOB_PART).unwrap();
    id.add("guest", session("guest", true), GUEST_PART).unwrap();
    let store = Arc::new(Store::in_memory().unwrap());
    let app = router(AppState::new(Arc::clone(&lib), Arc::new(id), WEBOS).with_personal(Some(Arc::clone(&store))));
    Box_ { _dir: dir, packs, lib, store, app }
}

async fn body(r: Response<Body>) -> Vec<u8> {
    r.into_body().collect().await.unwrap().to_bytes().to_vec()
}

impl Box_ {
    async fn raw(&self, method: &str, path: &str, who: Option<&str>, origin: Option<&str>, csrf: Option<&str>, b: Option<Value>) -> Response<Body> {
        let mut r = Request::builder().method(method).uri(path).header("host", WEBOS);
        if let Some(w) = who {
            r = r.header("cookie", format!("delt_session={w}"));
        }
        if let Some(o) = origin {
            r = r.header("origin", o);
        }
        if let Some(c) = csrf {
            r = r.header("x-delt-csrf", c);
        }
        let req = match b {
            Some(v) => r.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
            None => r.body(Body::empty()).unwrap(),
        };
        self.app.clone().oneshot(req).await.unwrap()
    }
    /// A request as the WebOS sends it for `who`.
    async fn call(&self, method: &str, path: &str, who: &str, b: Option<Value>) -> (StatusCode, Value) {
        let csrf = format!("csrf-{who}");
        let r = self.raw(method, path, Some(who), Some(ORIGIN), Some(&csrf), b).await;
        let st = r.status();
        (st, serde_json::from_slice(&body(r).await).unwrap_or(Value::Null))
    }
    async fn get(&self, path: &str, who: &str) -> (StatusCode, Value) {
        self.call("GET", path, who, None).await
    }
    fn pack_host(&self, part: &str) -> String {
        let snap = self.lib.snapshot();
        let p = &snap.by_uuid[ID];
        format!("pack-{}-g{}-{part}.deltos-packs.lan", p.slot.pack_index, p.slot.pack_generation)
    }
    async fn read_page(&self, part: &str, path: &str) -> StatusCode {
        let r = Request::builder().uri(path).header("host", self.pack_host(part)).body(Body::empty()).unwrap();
        self.app.clone().oneshot(r).await.unwrap().status()
    }
}

// --- who may use /me ---------------------------------------------------------

#[tokio::test]
async fn me_needs_a_person_not_guest_and_checked_writes() {
    let b = boot();
    let r = b.raw("GET", "/api/v1/me/bookmarks", None, None, None, None).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let (st, v) = b.get("/api/v1/me/history", "guest").await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("guest_not_allowed")));
    let (st, v) = b.call("POST", "/api/v1/me/bookmarks", "guest", Some(json!({"pack": ID, "path": "index.html"}))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("guest_not_allowed")));

    let add = json!({"pack": ID, "path": "Helium.html"});
    for (origin, csrf) in [(None, Some("csrf-alice")), (Some("http://evil.example"), Some("csrf-alice")), (Some(ORIGIN), None), (Some(ORIGIN), Some("csrf-bob"))] {
        let r = b.raw("POST", "/api/v1/me/bookmarks", Some("alice"), origin, csrf, Some(add.clone())).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{origin:?} {csrf:?}");
        let v: Value = serde_json::from_slice(&body(r).await).unwrap();
        assert_eq!(v["code"], "request_rejected");
    }
    assert!(b.store.bookmarks("alice", &Filter::default(), None, 10).unwrap().is_empty(), "nothing written");

    let r = b.raw("GET", "/api/v1/me/bookmarks", Some("alice"), None, None, None).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store", "rule 10");
    let r = b.raw("PUT", "/api/v1/me/bookmarks", Some("alice"), Some(ORIGIN), Some("csrf-alice"), None).await;
    assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);
}

// --- bookmarks -----------------------------------------------------------------

#[tokio::test]
async fn bookmarks_are_one_persons_own() {
    let b = boot();
    let (st, bm) = b.call("POST", "/api/v1/me/bookmarks", "alice", Some(json!({"pack": ID, "path": "H.html"}))).await;
    assert_eq!(st, StatusCode::CREATED, "{bm}");
    assert_eq!((bm["path"].as_str(), bm["title"].as_str()), (Some("Hydrogen.html"), Some("Hydrogen")), "the alias is followed");
    let (_, again) = b.call("POST", "/api/v1/me/bookmarks", "alice", Some(json!({"pack": ID, "path": "Hydrogen.html"}))).await;
    assert_eq!(again["id"], bm["id"], "one bookmark per page");
    let id = bm["id"].as_str().unwrap();

    let (st, v) = b.call("POST", "/api/v1/me/bookmarks", "alice", Some(json!({"pack": ID, "path": "Nope.html"}))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("page_not_found")));
    let (st, v) = b.call("POST", "/api/v1/me/bookmarks", "alice", Some(json!({"pack": ID, "path": "style.css"}))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_request")));
    let (st, v) = b.call("POST", "/api/v1/me/bookmarks", "alice", Some(json!({"pack": "nope", "path": "x"}))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("pack_not_found")));

    // Bob sees none of it and cannot touch it.
    let (_, v) = b.get("/api/v1/me/bookmarks", "bob").await;
    assert_eq!(v["items"], json!([]));
    let (st, v) = b.call("DELETE", &format!("/api/v1/me/bookmarks/{id}"), "bob", None).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("item_not_found")));
    let (st, _) = b.call("PATCH", &format!("/api/v1/me/bookmarks/{id}"), "bob", Some(json!({"title": "mine"}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (_, v) = b.call("PATCH", &format!("/api/v1/me/bookmarks/{id}"), "alice", Some(json!({"title": "H2"}))).await;
    assert_eq!(v["title"], "H2");
    b.call("POST", "/api/v1/me/bookmarks", "alice", Some(json!({"pack": ID, "path": "Helium.html"}))).await;
    let (_, v) = b.get("/api/v1/me/bookmarks?limit=1", "alice").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let next = v["next"].as_str().unwrap().to_string();
    let (_, v2) = b.get(&format!("/api/v1/me/bookmarks?limit=1&cursor={next}"), "alice").await;
    assert_eq!(v2["items"].as_array().unwrap().len(), 1);
    assert_ne!(v["items"][0]["id"], v2["items"][0]["id"]);
    assert_eq!(v2["next"], Value::Null);
    let (_, one) = b.get(&format!("/api/v1/me/bookmarks?pack={ID}&path=Hydrogen.html"), "alice").await;
    assert_eq!(one["items"].as_array().unwrap().len(), 1, "is this page bookmarked?");

    let (st, _) = b.call("DELETE", &format!("/api/v1/me/bookmarks/{id}"), "alice", None).await;
    assert_eq!(st, StatusCode::OK);
    let (_, v) = b.get("/api/v1/me/bookmarks", "alice").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
}

// --- highlights and updates ----------------------------------------------------------

#[tokio::test]
async fn highlights_follow_their_quote_and_are_flagged_when_it_is_gone() {
    let b = boot();
    let post = |path: &str, quote: &str| json!({"pack": ID, "path": path, "quote": quote, "note": "remember"});
    let (st, v) = b.call("POST", "/api/v1/me/highlights", "alice", Some(post("Hydrogen.html", "not on the page"))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_request")));

    let (st, rockets) = b.call("POST", "/api/v1/me/highlights", "alice", Some(post("Hydrogen.html", "fuels  rockets"))).await;
    assert_eq!(st, StatusCode::CREATED, "{rockets}");
    assert_eq!((rockets["passage"].as_u64(), rockets["anchor"].as_str()), (Some(2), Some("Uses")));
    assert_eq!(rockets["note"], "remember");
    let (_, lavoisier) = b.call("POST", "/api/v1/me/highlights", "alice", Some(post("H.html", "named by Lavoisier"))).await;
    assert_eq!((lavoisier["path"].as_str(), lavoisier["passage"].as_u64()), (Some("Hydrogen.html"), Some(1)));
    let (_, diamond) = b.call("POST", "/api/v1/me/highlights", "alice", Some(post("Carbon.html", "Diamond is carbon"))).await;
    let (_, bobs) = b.call("POST", "/api/v1/me/highlights", "bob", Some(post("Helium.html", "fills balloons"))).await;

    let (_, v) = b.get(&format!("/api/v1/me/highlights?pack={ID}&path=Hydrogen.html"), "alice").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let (_, v) = b.get("/api/v1/me/needs-attention", "alice").await;
    assert_eq!(v["items"], json!([]));

    // The pack is updated on the box.
    science_update(&b.packs.join("science.wax"));
    b.lib.refresh().unwrap();

    let (_, v) = b.get("/api/v1/me/needs-attention", "alice").await;
    let flagged: Vec<&str> = v["items"].as_array().unwrap().iter().map(|h| h["id"].as_str().unwrap()).collect();
    let mut want = vec![lavoisier["id"].as_str().unwrap(), diamond["id"].as_str().unwrap()];
    want.sort();
    let mut got = flagged.clone();
    got.sort();
    assert_eq!(got, want, "a rewritten sentence and a deleted page are flagged");
    let (_, all) = b.get("/api/v1/me/highlights", "alice").await;
    let by_id = |id: &Value| all["items"].as_array().unwrap().iter().find(|h| h["id"] == *id).unwrap().clone();
    let r = by_id(&rockets["id"]);
    assert_eq!((r["passage"].as_u64(), r["anchor"].as_str(), r["needs_attention"].as_bool()), (Some(2), Some("Space"), Some(false)), "found again, in its new section");
    let l = by_id(&lavoisier["id"]);
    assert_eq!((l["passage"].as_u64(), l["anchor"].as_str()), (Some(1), Some("History")), "flagged, never moved");

    // Bob's highlight is his alone and untouched.
    let (_, v) = b.get("/api/v1/me/highlights", "bob").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["id"], bobs["id"]);

    let path = format!("/api/v1/me/highlights/{}", diamond["id"].as_str().unwrap());
    let (_, v) = b.call("PATCH", &path, "alice", Some(json!({"note": "page gone"}))).await;
    assert_eq!(v["note"], "page gone");
    assert_eq!(b.call("PATCH", &path, "bob", Some(json!({"note": "x"}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(b.call("DELETE", &path, "alice", None).await.0, StatusCode::OK);
    let (_, v) = b.get("/api/v1/me/needs-attention", "alice").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
}

// --- reading lists ---------------------------------------------------------------

#[tokio::test]
async fn reading_lists_keep_order_and_check_their_pages() {
    let b = boot();
    let items = json!([{"pack": ID, "path": "Helium.html"}, {"pack": ID, "path": "H.html"}]);
    let (st, l) = b.call("POST", "/api/v1/me/lists", "alice", Some(json!({"name": "Week 1", "items": items}))).await;
    assert_eq!(st, StatusCode::CREATED, "{l}");
    let titles: Vec<&str> = l["items"].as_array().unwrap().iter().map(|i| i["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Helium", "Hydrogen"]);
    assert_eq!(l["items"][1]["path"], "Hydrogen.html");
    let (st, v) = b.call("POST", "/api/v1/me/lists", "alice", Some(json!({"name": "Bad", "items": [{"pack": ID, "path": "Nope.html"}]}))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("page_not_found")));
    assert_eq!(b.call("POST", "/api/v1/me/lists", "alice", Some(json!({"name": " "}))).await.0, StatusCode::BAD_REQUEST);

    let path = format!("/api/v1/me/lists/{}", l["id"].as_str().unwrap());
    let (_, v) = b.call("PATCH", &path, "alice", Some(json!({"items": [{"pack": ID, "path": "Carbon.html"}]}))).await;
    assert_eq!((v["name"].as_str(), v["items"].as_array().unwrap().len()), (Some("Week 1"), 1));
    let (_, v) = b.call("PATCH", &path, "alice", Some(json!({"name": "Week 2"}))).await;
    assert_eq!((v["name"].as_str(), v["items"][0]["title"].as_str()), (Some("Week 2"), Some("Carbon")));
    assert_eq!(b.get(&path, "bob").await.0, StatusCode::NOT_FOUND);
    let (_, all) = b.get("/api/v1/me/lists", "alice").await;
    assert_eq!(all["items"].as_array().unwrap().len(), 1);
    assert_eq!(b.call("DELETE", &path, "alice", None).await.0, StatusCode::OK);
    assert_eq!(b.get(&path, "alice").await.0, StatusCode::NOT_FOUND);
}

// --- history -----------------------------------------------------------------------

async fn wait_for_history(store: &Store, who: &str, n: usize) -> Vec<deltlib::personal::Visit> {
    for _ in 0..100 {
        let h = store.history(who, None, 50).unwrap();
        if h.len() >= n {
            return h;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    store.history(who, None, 50).unwrap()
}

#[tokio::test]
async fn pages_read_become_history_and_can_be_cleared() {
    let b = boot();
    assert_eq!(b.read_page(ALICE_PART, "/Helium.html").await, StatusCode::OK);
    assert_eq!(b.read_page(ALICE_PART, "/style.css").await, StatusCode::OK);
    assert_eq!(b.read_page(ALICE_PART, "/img/a.png").await, StatusCode::OK);
    assert_eq!(b.read_page(ALICE_PART, "/Hydrogen.html").await, StatusCode::OK);
    assert_eq!(b.read_page(GUEST_PART, "/Helium.html").await, StatusCode::OK);
    let h = wait_for_history(&b.store, "alice", 2).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let h2 = b.store.history("alice", None, 50).unwrap();
    assert_eq!(h, h2, "style sheets and images are not pages read");
    let (_, v) = b.get("/api/v1/me/history", "alice").await;
    let paths: Vec<&str> = v["items"].as_array().unwrap().iter().map(|i| i["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["Hydrogen.html", "Helium.html"], "newest first");
    assert_eq!(v["items"][0]["title"], "Hydrogen");
    let (_, p1) = b.get("/api/v1/me/history?limit=1", "alice").await;
    let (_, p2) = b.get(&format!("/api/v1/me/history?limit=1&cursor={}", p1["next"].as_str().unwrap()), "alice").await;
    assert_eq!(p2["items"][0]["path"], "Helium.html");
    assert!(b.store.history("guest", None, 50).unwrap().is_empty(), "Guest stores nothing");

    let (_, v) = b.call("DELETE", "/api/v1/me/history", "alice", None).await;
    assert_eq!(v["cleared"], 2);
    let (_, v) = b.get("/api/v1/me/history", "alice").await;
    assert_eq!(v["items"], json!([]));
}

// --- export ------------------------------------------------------------------------

#[tokio::test]
async fn export_is_everything_of_ones_own_as_a_file() {
    let b = boot();
    b.call("POST", "/api/v1/me/bookmarks", "alice", Some(json!({"pack": ID, "path": "Helium.html"}))).await;
    b.call("POST", "/api/v1/me/highlights", "alice", Some(json!({"pack": ID, "path": "Helium.html", "quote": "fills balloons"}))).await;
    b.call("POST", "/api/v1/me/lists", "alice", Some(json!({"name": "L"}))).await;
    b.call("POST", "/api/v1/me/bookmarks", "bob", Some(json!({"pack": ID, "path": "Carbon.html"}))).await;
    let r = b.raw("GET", "/api/v1/me/export", Some("alice"), None, None, None).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers().get("content-disposition").unwrap().to_str().unwrap().starts_with("attachment"));
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    let v: Value = serde_json::from_slice(&body(r).await).unwrap();
    assert_eq!(v["format"], "deltos-personal-export");
    assert_eq!(v["bookmarks"].as_array().unwrap().len(), 1, "Bob's are not in Alice's file");
    assert_eq!(v["highlights"][0]["quote"], "fills balloons");
    assert_eq!(v["lists"][0]["name"], "L");
    assert_eq!(v["pack_names"][ID], "Science");
    assert!(v["highlights"][0].get("pack_state").is_none());
}

// --- outline and reader --------------------------------------------------------------

#[tokio::test]
async fn outline_and_reader_come_from_the_pages_sections() {
    let b = boot();
    let (st, o) = b.get(&format!("/api/v1/packs/{ID}/outline?path=H.html"), "guest").await;
    assert_eq!(st, StatusCode::OK, "{o}");
    assert_eq!(o["path"], "Hydrogen.html");
    let heads: Vec<Value> = o["sections"].as_array().unwrap().iter().map(|s| s["heading"].clone()).collect();
    assert_eq!(heads, [Value::Null, json!("History"), json!("Uses")]);
    assert_eq!(o["sections"][1]["level"], 2);
    assert_eq!(o["sections"][1]["anchor"], "History");

    let (_, r) = b.get(&format!("/api/v1/packs/{ID}/reader?path=Hydrogen.html"), "alice").await;
    assert_eq!(r["sections"][0]["paragraphs"], json!(["Lead of Hydrogen."]));
    assert_eq!(r["sections"][1]["paragraphs"], json!(["Hydrogen was named by Lavoisier."]));
    assert_eq!(r["sections"][2]["images"], json!([{"path": "img/a.png", "alt": "A picture"}]));
    assert_eq!(r["languages"], json!(["en"]));

    let (st, v) = b.get(&format!("/api/v1/packs/{ID}/reader?path=img/a.png"), "alice").await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_request")));
    let (st, v) = b.get(&format!("/api/v1/packs/{ID}/outline"), "alice").await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_request")));
    let (st, v) = b.get(&format!("/api/v1/packs/{ID}/outline?path=Nope.html"), "alice").await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("page_not_found")));

    let (_, o) = b.get(&format!("/api/v1/packs/{OLD_ID}/outline?path=index.html"), "alice").await;
    assert_eq!(o["has_passages"], false);
    assert_eq!(o["sections"].as_array().unwrap().len(), 1, "a pack before v0.13: the page is one section");
    let (_, r) = b.get(&format!("/api/v1/packs/{OLD_ID}/reader?path=index.html"), "alice").await;
    assert_eq!(r["sections"][0]["paragraphs"], json!(["Lead of Old.", "Part", "Old text here."]));

    science_update(&b.packs.join("science.wax"));
    b.lib.refresh().unwrap();
    let (st, v) = b.get(&format!("/api/v1/packs/{ID}/outline?path=Carbon.html"), "alice").await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::GONE, Some("page_removed")));
}
