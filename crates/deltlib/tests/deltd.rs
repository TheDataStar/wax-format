//! deltd end to end: real packs on disk, the real router, requests in and
//! responses out. One test per row of deltd API §4.1, the §4.2 headers, the
//! §4.4 hostname checks, and the §11 CI list that applies to what is built.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use deltlib::{router, AppState, Catalog, Library, Session, StaticIdentity};
use http_body_util::BodyExt;
use tower::ServiceExt;
use wax_core::model::Compression;
use wax_core::writer::{EntryMeta, WaxWriter};

const WEBOS: &str = "library.deltos.lan";
const ALICE: &str = "aaaaaaaaaaaaaaaa"; // a learner's partition
const GUEST: &str = "6666666666666666";
const SCIENCE: [u8; 16] = [0x10, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1];
const FRENCH: [u8; 16] = [0x20, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 2];
const PLAIN: [u8; 16] = [0x30, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 3];
const SCIENCE_ID: &str = "10000000-0000-4000-8000-000000000001";
const FRENCH_ID: &str = "20000000-0000-4000-8000-000000000002";
const PLAIN_ID: &str = "30000000-0000-4000-8000-000000000003";

fn manifest(name: &str, lang: &str, guest: bool) -> BTreeMap<String, String> {
    let mut m = BTreeMap::from([
        ("name".to_string(), name.to_string()),
        ("languages".to_string(), lang.to_string()),
        ("category".to_string(), "reference".to_string()),
        ("entry_point".to_string(), "index.html".to_string()),
    ]);
    if guest {
        m.insert("guest_accessible".into(), "true".into());
    }
    m
}

fn html(sw: &mut wax_core::StreamingWriter, path: &str, title: &str, body: &str, c: Compression, index: bool) {
    let page = format!("<!doctype html><title>{title}</title><p>{body}</p>");
    sw.add_entry(EntryMeta::new(path).mime("text/html").title(title).compression(c), &mut page.as_bytes())
        .unwrap();
    if index {
        sw.index_text(path, Some(title), body).unwrap();
    }
}

/// A page well over `GZIP_MIN_BYTES`, as real articles are.
fn long_text() -> String {
    (0..60).map(|i| format!("Oxygen fact {i}: it makes up about a fifth of the air. ")).collect()
}

fn long_css() -> String {
    (0..60).map(|i| format!(".c{i} {{ color: #333; margin: 0 0 8px; }}\n")).collect()
}

/// A video-sized media entry: 1 MiB, stored verbatim.
fn media() -> Vec<u8> {
    (0..1024 * 1024u32).map(|i| (i % 251) as u8).collect()
}

fn science(dir: &Path) -> PathBuf {
    let out = dir.join("science.wax");
    let mut sw = WaxWriter::new(SCIENCE).created_at(1_700_000_000).create(&out, &manifest("Science", "en", true)).unwrap();
    html(&mut sw, "index.html", "Science", "Welcome to science.", Compression::Zstd, true);
    html(&mut sw, "Hydrogen.html", "Hydrogen", "Hydrogen is the lightest element.", Compression::Zstd, true);
    html(&mut sw, "Helium.html", "Helium", "Helium is a noble gas, lighter than air.", Compression::Zstd, true);
    html(&mut sw, "Carbon.html", "Carbon", "Carbon forms more compounds than any element.", Compression::Zstd, true);
    html(&mut sw, "Cafe.html", "Café chemistry", "Caffeine and the café.", Compression::Zstd, true);
    html(&mut sw, "Oxygen.html", "Oxygen", &long_text(), Compression::Zstd, false);
    sw.add_entry(EntryMeta::new("style.css").mime("text/css").compression(Compression::Zstd), &mut long_css().as_bytes())
        .unwrap();
    sw.add_redirect("H.html", "Hydrogen.html", None).unwrap();
    sw.add_entry(EntryMeta::new("media/clip.webm").mime("video/webm").compression(Compression::None), &mut &media()[..])
        .unwrap();
    sw.finish().unwrap();
    out
}

/// French pack, every page against the shared dictionary.
fn french(dir: &Path) -> PathBuf {
    let out = dir.join("french.wax");
    let mut sw = WaxWriter::new(FRENCH).created_at(1_700_000_000).create(&out, &manifest("Français", "fr", false)).unwrap();
    let samples: Vec<Vec<u8>> = (0..400)
        .map(|i| format!("<!doctype html><html><head><title>Page {i}</title></head><body><p>Les élèves de l'école numéro {i} étudient la chimie et la physique à Montréal.</p></body></html>").into_bytes())
        .collect();
    sw.train_dictionary(samples.iter().map(|s| s.as_slice())).unwrap();
    html(&mut sw, "index.html", "Accueil", "Bienvenue.", Compression::ZstdDict, true);
    html(&mut sw, "Ecole.html", "École", "Les élèves de l'école de Montréal.", Compression::ZstdDict, true);
    html(&mut sw, "Hydrogene.html", "Hydrogène", "L'hydrogène est l'élément le plus léger.", Compression::ZstdDict, true);
    sw.finish().unwrap();
    out
}

/// No search index at all.
fn plain(dir: &Path) -> PathBuf {
    let out = dir.join("plain.wax");
    let mut sw = WaxWriter::new(PLAIN).created_at(1_700_000_000).create(&out, &manifest("Plain", "en", false)).unwrap();
    html(&mut sw, "index.html", "Plain", "Hydrogen, but unsearchable.", Compression::Zstd, false);
    sw.finish().unwrap();
    out
}

struct Box_ {
    _dir: tempfile::TempDir,
    packs: PathBuf,
    lib: Arc<Library>,
    app: axum::Router,
}

fn session(profile: &str, guest: bool) -> Session {
    Session { profile_id: profile.into(), browser_id: format!("browser-{profile}"), guest, secure: false, ..Default::default() }
}

fn boot(with_ident: impl FnOnce(&mut StaticIdentity)) -> Box_ {
    let dir = tempfile::tempdir().unwrap();
    let packs = dir.path().join("packs");
    std::fs::create_dir(&packs).unwrap();
    science(&packs);
    french(&packs);
    plain(&packs);
    let lib = Arc::new(Library::new(&packs, Catalog::in_memory().unwrap(), 2));
    lib.refresh().unwrap();
    let mut id = StaticIdentity::new();
    id.add("alice-token", session("alice", false), ALICE).unwrap();
    id.add("guest-token", session("guest", true), GUEST).unwrap();
    with_ident(&mut id);
    let app = router(AppState::new(Arc::clone(&lib), Arc::new(id), WEBOS));
    Box_ { _dir: dir, packs, lib, app }
}

impl Box_ {
    fn host(&self, uuid: &str, partition: &str) -> String {
        let snap = self.lib.snapshot();
        let p = &snap.by_uuid[uuid];
        format!("pack-{}-g{}-{partition}.deltos-packs.lan", p.slot.pack_index, p.slot.pack_generation)
    }
    async fn send(&self, req: Request<Body>) -> Response<Body> {
        self.app.clone().oneshot(req).await.unwrap()
    }
    async fn get(&self, host: &str, path: &str, headers: &[(&str, &str)]) -> Response<Body> {
        let mut b = Request::builder().uri(path).header("host", host);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        self.send(b.body(Body::empty()).unwrap()).await
    }
    async fn api(&self, path: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
        let cookie = token.map(|t| format!("delt_session={t}"));
        let mut h = vec![];
        if let Some(c) = &cookie {
            h.push(("cookie", c.as_str()));
        }
        let r = self.get(WEBOS, path, &h).await;
        let st = r.status();
        let b = body(r).await;
        (st, serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null))
    }
}

async fn body(r: Response<Body>) -> Vec<u8> {
    r.into_body().collect().await.unwrap().to_bytes().to_vec()
}

fn hdr<'a>(r: &'a Response<Body>, k: &str) -> Option<&'a str> {
    r.headers().get(k).and_then(|v| v.to_str().ok())
}

fn assert_zone_headers(r: &Response<Body>) {
    assert!(r.headers().get("set-cookie").is_none(), "pack zone never sets a cookie");
    let csp = hdr(r, "content-security-policy").expect("CSP");
    assert!(csp.contains("default-src 'self' data: blob:"), "{csp}");
    assert!(csp.contains("form-action 'self'") && csp.contains("base-uri 'self'"), "{csp}");
    assert!(csp.contains(&format!("frame-ancestors http://{WEBOS} https://{WEBOS}")), "{csp}");
    assert_eq!(hdr(r, "cross-origin-resource-policy"), Some("same-origin"));
    assert_eq!(hdr(r, "x-content-type-options"), Some("nosniff"));
    assert_eq!(hdr(r, "referrer-policy"), Some("same-origin"));
}

// --- §4.1: what a request finds -------------------------------------------

#[tokio::test]
async fn a_normal_entry_is_served_exactly_as_built() {
    let b = boot(|_| {});
    let h = b.host(SCIENCE_ID, ALICE);
    let r = b.get(&h, "/Hydrogen.html", &[("cookie", "delt_session=alice-token")]).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_zone_headers(&r);
    assert_eq!(hdr(&r, "content-type"), Some("text/html"));
    assert_eq!(hdr(&r, "cache-control"), Some("no-cache"));
    assert_eq!(hdr(&r, "vary"), Some("Accept-Encoding"));
    assert_eq!(hdr(&r, "accept-ranges"), Some("bytes"));
    assert!(hdr(&r, "content-encoding").is_none(), "plain HTTP visitors get decoded bytes");
    assert_eq!(body(r).await, b"<!doctype html><title>Hydrogen</title><p>Hydrogen is the lightest element.</p>");
}

#[tokio::test]
async fn root_alias_missing_and_methods() {
    let b = boot(|_| {});
    let h = b.host(SCIENCE_ID, ALICE);
    let r = b.get(&h, "/", &[]).await;
    assert_eq!((r.status(), hdr(&r, "location")), (StatusCode::FOUND, Some("/index.html")));
    let r = b.get(&h, "/H.html", &[]).await;
    assert_eq!((r.status(), hdr(&r, "location")), (StatusCode::MOVED_PERMANENTLY, Some("/Hydrogen.html")));
    let r = b.get(&h, "/Nope.html", &[]).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert_zone_headers(&r);
    for m in ["POST", "PUT", "DELETE", "OPTIONS", "PATCH"] {
        let r = b.send(Request::builder().method(m).uri("/Hydrogen.html").header("host", &h).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED, "{m}");
        assert_eq!(hdr(&r, "allow"), Some("GET, HEAD"));
    }
    let r = b.send(Request::builder().method("HEAD").uri("/Hydrogen.html").header("host", &h).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(hdr(&r, "content-length").is_some());
    assert!(body(r).await.is_empty());
}

#[tokio::test]
async fn a_page_removed_by_an_update_answers_410_and_search_follows_the_edition() {
    let b = boot(|_| {});
    let h = b.host(SCIENCE_ID, ALICE);
    let (_, before) = b.api("/api/v1/search?q=compounds", Some("alice-token")).await;
    assert_eq!(before["items"][0]["path"], "Carbon.html");

    // Edition 2, as an append: Carbon deleted, Helium rewritten, Boron added.
    let mut sw = WaxWriter::new(SCIENCE).created_at(1_700_100_000).open_append(b.packs.join("science.wax")).unwrap();
    sw.add_tombstone("Carbon.html").unwrap();
    html(&mut sw, "Helium.html", "Helium", "Helium fills party balloons.", Compression::Zstd, true);
    html(&mut sw, "Boron.html", "Boron", "Boron is a metalloid.", Compression::Zstd, true);
    sw.finish().unwrap();
    let rep = b.lib.refresh().unwrap();
    assert_eq!(rep.updated, vec![SCIENCE_ID.to_string()]);
    assert_eq!(b.host(SCIENCE_ID, ALICE), h, "an update keeps the pack's hostname, so its storage");

    let r = b.get(&h, "/Carbon.html", &[]).await;
    assert_eq!(r.status(), StatusCode::GONE);
    assert_zone_headers(&r);
    let (_, after) = b.api("/api/v1/search?q=compounds", Some("alice-token")).await;
    assert_eq!(after["items"].as_array().unwrap().len(), 0, "a deleted page never appears: {after}");
    let (_, boron) = b.api("/api/v1/search?q=metalloid", Some("alice-token")).await;
    assert_eq!(boron["items"][0]["path"], "Boron.html", "the new edition is searchable at once");
    let (_, old) = b.api("/api/v1/search?q=noble", Some("alice-token")).await;
    assert_eq!(old["items"].as_array().unwrap().len(), 0, "the superseded text is gone: {old}");
}

#[tokio::test]
async fn a_corrupt_entry_is_500_never_the_bad_bytes() {
    let b = boot(|_| {});
    let h = b.host(SCIENCE_ID, ALICE);
    let (hyd, clip) = {
        let snap = b.lib.snapshot();
        let p = &snap.by_uuid[SCIENCE_ID];
        p.readers.with(|r| (r.entry("Hydrogen.html").unwrap(), r.entry("media/clip.webm").unwrap()))
    };
    let path = b.packs.join("science.wax");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[(hyd.offset + hyd.length / 2) as usize] ^= 0xff;
    bytes[(clip.offset + 1000) as usize] ^= 0xff;
    // rewrite in place: same header, so the open pack keeps its handles
    std::fs::write(&path, &bytes).unwrap();
    let before = b.lib.health.count();

    let r = b.get(&h, "/Hydrogen.html", &[]).await;
    assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!String::from_utf8_lossy(&body(r).await).contains("lightest"));

    // verbatim media streams, so the check lands at the end: the transfer is cut off
    let r = b.get(&h, "/media/clip.webm", &[]).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.into_body().collect().await.is_err(), "a corrupt stream must not complete");
    assert!(b.lib.health.count() >= before + 2, "each raises a health event");
}

// --- §4.2: caching, encodings, ranges ---------------------------------------

#[tokio::test]
async fn one_etag_per_encoding_and_zstd_passthrough() {
    let b = boot(|_| {});
    let h = b.host(SCIENCE_ID, ALICE);
    let plain = b.get(&h, "/Hydrogen.html", &[]).await;
    let tag = hdr(&plain, "etag").unwrap().to_string();
    let plain_bytes = body(plain).await;

    let z = b.get(&h, "/Hydrogen.html", &[("accept-encoding", "gzip, br, zstd")]).await;
    assert_eq!(hdr(&z, "content-encoding"), Some("zstd"));
    let ztag = hdr(&z, "etag").unwrap().to_string();
    assert_eq!(ztag, format!("{}-zst\"", tag.trim_end_matches('"')), "same hash, distinct tag");
    assert!(hdr(&z, "accept-ranges").is_none());
    let decoded = zstd_decode(&body(z).await);
    assert_eq!(decoded, plain_bytes);

    let r = b.get(&h, "/Hydrogen.html", &[("if-none-match", &tag)]).await;
    assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
    let r = b.get(&h, "/Hydrogen.html", &[("if-none-match", &ztag)]).await;
    assert_eq!(r.status(), StatusCode::OK, "the zstd tag does not validate the plain bytes");
    let r = b.get(&h, "/Hydrogen.html", &[("if-none-match", &ztag), ("accept-encoding", "zstd")]).await;
    assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn gzip_for_browsers_that_cannot_take_zstd() {
    use std::io::Read;
    let gunzip = |b: &[u8]| {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(b).read_to_end(&mut out).unwrap();
        out
    };
    let b = boot(|_| {});
    let h = b.host(SCIENCE_ID, ALICE);
    let plain = b.get(&h, "/Oxygen.html", &[]).await;
    assert!(hdr(&plain, "content-encoding").is_none(), "nothing asked for, nothing encoded");
    let tag = hdr(&plain, "etag").unwrap().to_string();
    let plain_bytes = body(plain).await;
    assert!(plain_bytes.len() as u64 >= deltlib::packzone::GZIP_MIN_BYTES);

    // what a phone on plain HTTP sends
    let g = b.get(&h, "/Oxygen.html", &[("accept-encoding", "gzip, deflate")]).await;
    assert_eq!(hdr(&g, "content-encoding"), Some("gzip"));
    assert_eq!(hdr(&g, "vary"), Some("Accept-Encoding"));
    assert!(hdr(&g, "accept-ranges").is_none());
    let gtag = hdr(&g, "etag").unwrap().to_string();
    assert_eq!(gtag, format!("{}-gz\"", tag.trim_end_matches('"')), "same hash, distinct tag");
    let len: usize = hdr(&g, "content-length").unwrap().parse().unwrap();
    let packed = body(g).await;
    assert_eq!(packed.len(), len);
    assert!(packed.len() * 3 < plain_bytes.len(), "{} vs {}", packed.len(), plain_bytes.len());
    assert_eq!(gunzip(&packed), plain_bytes);

    // tags never cross encodings
    let r = b.get(&h, "/Oxygen.html", &[("if-none-match", &gtag), ("accept-encoding", "gzip")]).await;
    assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
    let r = b.get(&h, "/Oxygen.html", &[("if-none-match", &gtag)]).await;
    assert_eq!(r.status(), StatusCode::OK, "the gzip tag does not validate the plain bytes");
    let r = b.get(&h, "/Oxygen.html", &[("if-none-match", &tag), ("accept-encoding", "gzip")]).await;
    assert_eq!(r.status(), StatusCode::OK, "nor the plain tag the gzip bytes");

    // zstd still wins where the browser takes it (HTTPS); q=0 refuses gzip
    let z = b.get(&h, "/Oxygen.html", &[("accept-encoding", "gzip, zstd")]).await;
    assert_eq!(hdr(&z, "content-encoding"), Some("zstd"));
    let r = b.get(&h, "/Oxygen.html", &[("accept-encoding", "gzip;q=0")]).await;
    assert!(hdr(&r, "content-encoding").is_none());

    // stylesheets too; HEAD gives the same length and no body
    let c = b.get(&h, "/style.css", &[("accept-encoding", "gzip")]).await;
    assert_eq!(hdr(&c, "content-encoding"), Some("gzip"));
    let clen = hdr(&c, "content-length").unwrap().to_string();
    assert!(String::from_utf8(gunzip(&body(c).await)).unwrap().starts_with(".c0 {"));
    let r = b.send(Request::builder().method("HEAD").uri("/style.css").header("host", &h).header("accept-encoding", "gzip").body(Body::empty()).unwrap()).await;
    assert_eq!((hdr(&r, "content-encoding"), hdr(&r, "content-length")), (Some("gzip"), Some(clen.as_str())));
    assert!(body(r).await.is_empty());

    // not: small pages, ranges, media stored verbatim
    let r = b.get(&h, "/Hydrogen.html", &[("accept-encoding", "gzip")]).await;
    assert!(hdr(&r, "content-encoding").is_none(), "under GZIP_MIN_BYTES");
    let r = b.get(&h, "/Oxygen.html", &[("accept-encoding", "gzip"), ("range", "bytes=0-14")]).await;
    assert_eq!((r.status(), hdr(&r, "content-encoding")), (StatusCode::PARTIAL_CONTENT, None));
    let r = b.get(&h, "/media/clip.webm", &[("accept-encoding", "gzip")]).await;
    assert!(hdr(&r, "content-encoding").is_none());
}

fn zstd_decode(b: &[u8]) -> Vec<u8> {
    zstd::stream::decode_all(b).unwrap()
}

#[tokio::test]
async fn shared_dictionary_entries_are_always_decoded_on_the_box() {
    let b = boot(|_| {});
    let h = b.host(FRENCH_ID, ALICE);
    let r = b.get(&h, "/Ecole.html", &[("accept-encoding", "zstd")]).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(hdr(&r, "content-encoding").is_none(), "a browser cannot use the pack's dictionary");
    assert!(String::from_utf8(body(r).await).unwrap().contains("Les élèves de l'école de Montréal."));
}

#[tokio::test]
async fn byte_ranges_on_media_and_text() {
    let b = boot(|_| {});
    let h = b.host(SCIENCE_ID, ALICE);
    let m = media();
    let r = b.get(&h, "/media/clip.webm", &[("range", "bytes=1000-1999")]).await;
    assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(hdr(&r, "content-range"), Some("bytes 1000-1999/1048576"));
    assert_eq!(body(r).await, &m[1000..2000]);
    let r = b.get(&h, "/media/clip.webm", &[("range", "bytes=-10")]).await;
    assert_eq!(body(r).await, &m[m.len() - 10..]);
    let r = b.get(&h, "/media/clip.webm", &[]).await;
    assert_eq!(hdr(&r, "content-length"), Some("1048576"));
    assert_eq!(body(r).await, m, "whole media streams and verifies");
    let r = b.get(&h, "/media/clip.webm", &[("range", "bytes=2000000-")]).await;
    assert_eq!(r.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(hdr(&r, "content-range"), Some("bytes */1048576"));
    // a text entry's range comes from its decoded bytes, even if zstd is offered
    let r = b.get(&h, "/Hydrogen.html", &[("range", "bytes=0-14"), ("accept-encoding", "zstd")]).await;
    assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
    assert!(hdr(&r, "content-encoding").is_none());
    assert_eq!(body(r).await, b"<!doctype html>");
    // If-Range with a stale tag: the whole entry
    let r = b.get(&h, "/Hydrogen.html", &[("range", "bytes=0-14"), ("if-range", "\"stale\"")]).await;
    assert_eq!(r.status(), StatusCode::OK);
}

// --- §4.4: only issued, live hostnames ---------------------------------------

#[tokio::test]
async fn only_issued_live_hostnames_are_served() {
    let b = boot(|id| id.add("bob-token", session("bob", false), "bbbbbbbbbbbbbbbb").unwrap());
    let snap = b.lib.snapshot();
    let p = &snap.by_uuid[SCIENCE_ID];
    let (i, g) = (p.slot.pack_index, p.slot.pack_generation);
    let ok = format!("pack-{i}-g{g}-{ALICE}.deltos-packs.lan");
    assert_eq!(b.get(&ok, "/index.html", &[]).await.status(), StatusCode::OK);
    for bad in [
        format!("pack-{i}-g{g}-cccccccccccccccc.deltos-packs.lan"), // never issued
        format!("pack-{i}-g{}-{ALICE}.deltos-packs.lan", g + 1),     // not the current generation
        format!("pack-{}-g1-{ALICE}.deltos-packs.lan", 400),         // no pack there
        format!("pack-{i}-g{g}-{ALICE}.deltos.lan"),                  // wrong zone
        "evil.example.com".to_string(),                              // DNS rebinding
        "192.168.4.1".to_string(),
    ] {
        let r = b.get(&bad, "/index.html", &[]).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{bad}");
        assert!(r.headers().get("set-cookie").is_none());
    }
    // Guest sees only guest-accessible packs: Science is, French is not.
    assert_eq!(b.get(&b.host(SCIENCE_ID, GUEST), "/index.html", &[]).await.status(), StatusCode::OK);
    assert_eq!(b.get(&b.host(FRENCH_ID, GUEST), "/index.html", &[]).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn signing_out_shuts_that_persons_pack_hostnames() {
    let dir = tempfile::tempdir().unwrap();
    let packs = dir.path().join("packs");
    std::fs::create_dir(&packs).unwrap();
    science(&packs);
    let lib = Arc::new(Library::new(&packs, Catalog::in_memory().unwrap(), 1));
    lib.refresh().unwrap();
    let mut id = StaticIdentity::new();
    id.add("alice-token", session("alice", false), ALICE).unwrap();
    let host = {
        let s = lib.snapshot();
        let p = &s.by_uuid[SCIENCE_ID];
        format!("pack-{}-g{}-{ALICE}.deltos-packs.lan", p.slot.pack_index, p.slot.pack_generation)
    };
    let live = router(AppState::new(Arc::clone(&lib), Arc::new(id), WEBOS));
    let req = || Request::builder().uri("/index.html").header("host", &host).body(Body::empty()).unwrap();
    assert_eq!(live.clone().oneshot(req()).await.unwrap().status(), StatusCode::OK);
    let mut id = StaticIdentity::new();
    id.add("alice-token", session("alice", false), ALICE).unwrap();
    id.sign_out("alice-token");
    let after = router(AppState::new(lib, Arc::new(id), WEBOS));
    assert_eq!(after.oneshot(req()).await.unwrap().status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_new_pack_at_a_reused_index_gets_a_higher_generation() {
    let b = boot(|_| {});
    let old = b.lib.snapshot().by_uuid[PLAIN_ID].slot;
    std::fs::remove_file(b.packs.join("plain.wax")).unwrap();
    let rep = b.lib.refresh().unwrap();
    assert_eq!(rep.removed, vec![PLAIN_ID.to_string()]);
    let gone = format!("pack-{}-g{}-{ALICE}.deltos-packs.lan", old.pack_index, old.pack_generation);
    assert_eq!(b.get(&gone, "/index.html", &[]).await.status(), StatusCode::NOT_FOUND);

    let out = b.packs.join("other.wax");
    let mut sw = WaxWriter::new([0x40, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 4])
        .create(&out, &manifest("Other", "en", false))
        .unwrap();
    html(&mut sw, "index.html", "Other", "Something else.", Compression::Zstd, false);
    sw.finish().unwrap();
    b.lib.refresh().unwrap();
    let new = b.lib.snapshot().by_uuid["40000000-0000-4000-8000-000000000004"].slot;
    assert_eq!(new.pack_index, old.pack_index);
    assert!(new.pack_generation > old.pack_generation);
    assert_eq!(b.get(&gone, "/index.html", &[]).await.status(), StatusCode::NOT_FOUND, "the old name never serves the new pack");
}

// --- API: Library and Search ------------------------------------------------

#[tokio::test]
async fn api_needs_a_session_and_sends_safe_headers() {
    let b = boot(|_| {});
    let (st, v) = b.api("/api/v1/packs", None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(v["code"], "not_signed_in");
    let (st, _) = b.api("/api/v1/packs", Some("forged")).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let r = b.get(WEBOS, "/api/v1/packs", &[("cookie", "delt_session=alice-token")]).await;
    assert_eq!(hdr(&r, "content-security-policy"), Some("default-src 'none'; frame-ancestors 'none'"));
    assert_eq!(hdr(&r, "referrer-policy"), Some("no-referrer"));
    assert_eq!(hdr(&r, "cache-control"), Some("private, no-cache"));
    assert!(hdr(&r, "access-control-allow-origin").is_none(), "no CORS, ever");
    let tag = hdr(&r, "etag").unwrap().to_string();
    let r = b.get(WEBOS, "/api/v1/packs", &[("cookie", "delt_session=alice-token"), ("if-none-match", &tag)]).await;
    assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
    let r = b.send(Request::builder().method("POST").uri("/api/v1/packs").header("host", WEBOS).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);
    let (st, v) = b.api("/api/v1/nothing-here", Some("alice-token")).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("invalid_request")));
}

#[tokio::test]
async fn library_lists_what_the_person_may_see() {
    let b = boot(|_| {});
    let (_, v) = b.api("/api/v1/packs", Some("alice-token")).await;
    let ids: Vec<&str> = v["items"].as_array().unwrap().iter().map(|p| p["archive_uuid"].as_str().unwrap()).collect();
    assert_eq!(ids, [SCIENCE_ID, FRENCH_ID, PLAIN_ID]);
    let (_, g) = b.api("/api/v1/packs", Some("guest-token")).await;
    assert_eq!(g["items"].as_array().unwrap().len(), 1, "Guest sees only guest-accessible packs");
    let (st, e) = b.api(&format!("/api/v1/packs/{FRENCH_ID}"), Some("guest-token")).await;
    assert_eq!((st, e["code"].as_str()), (StatusCode::FORBIDDEN, Some("guest_not_allowed")));
    let (_, fr) = b.api("/api/v1/packs?language=fr", Some("alice-token")).await;
    assert_eq!(fr["items"][0]["archive_uuid"], FRENCH_ID);
    // cursor paging
    let (_, p1) = b.api("/api/v1/packs?limit=2", Some("alice-token")).await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 2);
    let next = p1["next"].as_str().unwrap();
    let (_, p2) = b.api(&format!("/api/v1/packs?limit=2&cursor={next}"), Some("alice-token")).await;
    assert_eq!(p2["items"][0]["archive_uuid"], PLAIN_ID);
    assert!(p2["next"].is_null());

    let (_, one) = b.api(&format!("/api/v1/packs/{SCIENCE_ID}"), Some("alice-token")).await;
    assert_eq!(one["pack_hostname"], b.host(SCIENCE_ID, ALICE), "this person's hostname on this browser");
    assert_eq!(one["search"]["tokenizer"], "unicode61");
    let (_, pl) = b.api(&format!("/api/v1/packs/{PLAIN_ID}"), Some("alice-token")).await;
    assert_eq!(pl["search"]["available"], false);

    let (_, pages) = b.api(&format!("/api/v1/packs/{SCIENCE_ID}/pages?prefix=H"), Some("alice-token")).await;
    let paths: Vec<&str> = pages["items"].as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["H.html", "Helium.html", "Hydrogen.html"]);
    let (_, pg) = b.api(&format!("/api/v1/packs/{SCIENCE_ID}/pages?prefix=H&limit=1&cursor=H.html"), Some("alice-token")).await;
    assert_eq!(pg["items"][0]["path"], "Helium.html");
}

#[tokio::test]
async fn search_is_one_query_across_packs_accent_insensitive() {
    let b = boot(|_| {});
    // "hydrogene" (no accent) finds French "hydrogène"; "hydrogen" finds English
    let (_, v) = b.api("/api/v1/search?q=hydrogene", Some("alice-token")).await;
    assert_eq!(v["items"][0]["path"], "Hydrogene.html", "{v}");
    let (_, v) = b.api("/api/v1/search?q=eleves%20montreal", Some("alice-token")).await;
    assert_eq!(v["items"][0]["pack"], FRENCH_ID);
    let (_, v) = b.api("/api/v1/search?q=element", Some("alice-token")).await;
    let packs: Vec<&str> = v["items"].as_array().unwrap().iter().map(|h| h["pack"].as_str().unwrap()).collect();
    assert!(packs.contains(&SCIENCE_ID) && packs.contains(&FRENCH_ID), "fused across packs: {v}");
    assert_eq!(v["items"][0]["position_in_pack"], 0);
    assert_eq!(v["items"][1]["position_in_pack"], 0, "each pack's best hit comes first (RRF)");
    // the unsearchable pack is listed, never silently left out
    let skipped = v["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1);
    assert_eq!((skipped[0]["pack"].as_str(), skipped[0]["reason"].as_str()), (Some(PLAIN_ID), Some("no_index")));
    // FTS syntax is inert
    let (st, _) = b.api("/api/v1/search?q=%22NEAR(hydrogen%20OR%20*%20%3A", Some("alice-token")).await;
    assert_eq!(st, StatusCode::OK);
    // Guest searches only what Guest may see
    let (_, g) = b.api("/api/v1/search?q=element", Some("guest-token")).await;
    assert!(g["items"].as_array().unwrap().iter().all(|h| h["pack"] == SCIENCE_ID));
    // one pack; a pack with no index is a 409
    let (_, one) = b.api(&format!("/api/v1/search/packs/{SCIENCE_ID}?q=element"), Some("alice-token")).await;
    assert!(one["items"].as_array().unwrap().iter().all(|h| h["pack"] == SCIENCE_ID));
    let (st, e) = b.api(&format!("/api/v1/search/packs/{PLAIN_ID}?q=hydrogen"), Some("alice-token")).await;
    assert_eq!((st, e["code"].as_str()), (StatusCode::CONFLICT, Some("search_unavailable")));
    // paging
    let (_, p1) = b.api("/api/v1/search?q=element&limit=1", Some("alice-token")).await;
    let (_, p2) = b.api(&format!("/api/v1/search?q=element&limit=1&cursor={}", p1["next"].as_str().unwrap()), Some("alice-token")).await;
    assert_ne!(p1["items"][0], p2["items"][0]);
    let (st, _) = b.api("/api/v1/search?q=", Some("alice-token")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_chinese_pack_is_searchable_with_wax_bigram_16() {
    // SPEC v0.13 §5.7.2: CJK packs were built without an index until the
    // format could answer CJK queries; now they declare wax-bigram-16.
    let b = boot(|_| {});
    let out = b.packs.join("chinese.wax");
    let id = [0x40, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 4];
    let mut m = manifest("中文", "zh", false);
    m.insert("name".into(), "中文".into());
    let mut sw = WaxWriter::new(id).created_at(1_700_000_000).create(&out, &m).unwrap();
    sw.set_search_tokenizer(wax_core::tokenize::WAX_BIGRAM).unwrap();
    html(&mut sw, "index.html", "首页", "欢迎。", Compression::Zstd, true);
    html(&mut sw, "Beijing.html", "北京", "北京大学是中国的大学。北京有山和水。", Compression::Zstd, true);
    sw.finish().unwrap();
    b.lib.refresh().unwrap();
    for q in ["北京大学", "大学", "水"] {
        let (_, v) = b.api(&format!("/api/v1/search?q={}", urlenc(q)), Some("alice-token")).await;
        assert_eq!(v["items"][0]["path"], "Beijing.html", "{q}: {v}");
    }
    let (_, one) = b.api("/api/v1/packs/40000000-0000-4000-8000-000000000004", Some("alice-token")).await;
    assert_eq!(one["search"]["tokenizer"], "wax-bigram-16");
    assert_eq!(one["search"]["available"], true);
}

fn urlenc(s: &str) -> String {
    s.bytes().map(|b| format!("%{b:02X}")).collect()
}

#[tokio::test]
async fn suggestions_exact_first_then_accent_insensitive() {
    let b = boot(|_| {});
    let (_, v) = b.api("/api/v1/suggest?q=Hy", Some("alice-token")).await;
    let titles: Vec<&str> = v["items"].as_array().unwrap().iter().map(|s| s["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Hydrogen", "Hydrogène"]);
    let (_, v) = b.api("/api/v1/suggest?q=ecol", Some("alice-token")).await;
    assert_eq!(v["items"][0]["title"], "École", "accent-insensitive: {v}");
    assert_eq!(v["items"][0]["exact"], false);
    let (_, v) = b.api("/api/v1/suggest?q=caf", Some("alice-token")).await;
    assert_eq!(v["items"][0]["title"], "Café chemistry");
    let (_, s) = b.api("/api/v1/status", Some("alice-token")).await;
    assert_eq!((s["api"].as_str(), s["packs"].as_u64()), (Some("v1"), Some(3)));
}

#[tokio::test]
async fn openapi_lists_every_route_the_api_answers() {
    let b = boot(|_| {});
    let r = b.get(WEBOS, "/api/v1/openapi.json", &[]).await;
    assert_eq!(r.status(), StatusCode::OK);
    let doc: serde_json::Value = serde_json::from_slice(&body(r).await).unwrap();
    let paths = doc["paths"].as_object().unwrap();
    for p in ["/api/v1/status", "/api/v1/packs", "/api/v1/packs/{uuid}", "/api/v1/packs/{uuid}/pages",
              "/api/v1/search", "/api/v1/search/packs/{uuid}", "/api/v1/suggest", "/api/v1/openapi.json"] {
        assert!(paths.contains_key(p), "{p}");
    }
    for p in paths.keys() {
        let probe = p.replace("{uuid}", SCIENCE_ID);
        let (st, _) = b.api(&format!("{probe}?q=x"), Some("alice-token")).await;
        assert_ne!(st, StatusCode::NOT_FOUND, "{p} is documented but not answered");
    }
}

// --- phones' internet checks (box-network-plan §6) ---------------------------

#[tokio::test]
async fn connectivity_checks_only_when_on_and_only_the_listed_paths() {
    let b = boot(|_| {});
    let r = b.get("connectivitycheck.gstatic.com", "/generate_204", &[]).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND, "off by default");

    let dir = tempfile::tempdir().unwrap();
    let lib = Arc::new(Library::new(dir.path(), Catalog::in_memory().unwrap(), 1));
    let app = router(AppState::new(lib, Arc::new(StaticIdentity::new()), WEBOS).with_connectivity_checks(true));
    let get = |host: &'static str, path: &'static str, m: &'static str| {
        let app = app.clone();
        async move { app.oneshot(Request::builder().method(m).uri(path).header("host", host).body(Body::empty()).unwrap()).await.unwrap() }
    };
    let r = get("connectivitycheck.gstatic.com", "/generate_204", "GET").await;
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    assert!(r.headers().get("set-cookie").is_none());
    let r = get("captive.apple.com", "/hotspot-detect.html", "GET").await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(String::from_utf8(body(r).await).unwrap().contains("<TITLE>Success</TITLE>"));
    let r = get("www.msftconnecttest.com", "/connecttest.txt", "GET").await;
    assert_eq!(body(r).await, b"Microsoft Connect Test");
    for (h, p, m) in [
        ("connectivitycheck.gstatic.com", "/other", "GET"),
        ("connectivitycheck.gstatic.com", "/generate_204", "POST"),
        ("captive.apple.com", "/", "GET"),
        ("example.com", "/generate_204", "GET"),
    ] {
        assert_eq!(get(h, p, m).await.status(), StatusCode::NOT_FOUND, "{h}{p} {m}");
    }
}
