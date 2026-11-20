//! Delt desktop end to end: real packs on disk, the real deltlib library,
//! API and pack zone, the person's own data, and edition updates applied
//! and undone on real files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::http::{HeaderMap, Method};
use delt_desktop::updates::{UpdateError, UpdateKind};
use delt_desktop::{Desktop, DesktopError, ImportMode};
use wax_core::model::Compression;
use wax_core::writer::{EntryMeta, WaxWriter};

const SCIENCE: [u8; 16] = [0x10, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1];
const HISTORY: [u8; 16] = [0x20, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 2];
const SCIENCE_ID: &str = "10000000-0000-4000-8000-000000000001";
const HISTORY_ID: &str = "20000000-0000-4000-8000-000000000002";

fn manifest(name: &str, version: Option<&str>) -> BTreeMap<String, String> {
    let mut m = BTreeMap::from([
        ("name".to_string(), name.to_string()),
        ("languages".to_string(), "en".to_string()),
        ("entry_point".to_string(), "index.html".to_string()),
    ]);
    if let Some(v) = version {
        m.insert("version".into(), v.into());
    }
    m
}

fn page(sw: &mut wax_core::StreamingWriter, path: &str, title: &str, sections: &[(&str, &str)]) {
    let mut html = format!("<!doctype html><html><head><title>{title}</title></head><body><h1>{title}</h1>");
    let mut text = String::new();
    for (h, p) in sections {
        html.push_str(&format!("<h2>{h}</h2><p>{p}</p>"));
        text.push_str(&format!("{h} {p} "));
    }
    html.push_str("<p><a href=\"/index.html\">Home</a></p></body></html>");
    sw.add_entry(EntryMeta::new(path).mime("text/html").title(title).compression(Compression::Zstd), &mut html.as_bytes())
        .unwrap();
    sw.index_text(path, Some(title), &text).unwrap();
    sw.index_passages(path, html.as_bytes()).unwrap();
}

fn science(dir: &Path, version: Option<&str>) -> PathBuf {
    let out = dir.join("science.wax");
    let mut sw = WaxWriter::new(SCIENCE).created_at(1_700_000_000).create(&out, &manifest("Science", version)).unwrap();
    page(&mut sw, "index.html", "Science", &[("Welcome", "Pages about the elements.")]);
    page(
        &mut sw,
        "Hydrogen.html",
        "Hydrogen",
        &[
            ("Discovery", "Henry Cavendish described it in 1766 as inflammable air."),
            ("Uses", "Hydrogen fuels rockets and makes ammonia for fertiliser."),
        ],
    );
    page(&mut sw, "Helium.html", "Helium", &[("Properties", "Helium is a noble gas, lighter than air.")]);
    sw.finish().unwrap();
    out
}

fn history(dir: &Path) -> PathBuf {
    let out = dir.join("history.wax");
    let mut sw = WaxWriter::new(HISTORY).created_at(1_700_000_000).create(&out, &manifest("History", Some("2026.09.1"))).unwrap();
    page(&mut sw, "index.html", "History", &[("Welcome", "Pages about the past.")]);
    page(&mut sw, "Rockets.html", "Rockets", &[("Early rockets", "Gunpowder rockets flew in China.")]);
    sw.finish().unwrap();
    out
}

/// The installed science pack, with one update appended (a new page and a
/// removed one), written to a separate file as a publisher would ship it.
fn appended_edition(installed: &Path, out: &Path) -> PathBuf {
    std::fs::copy(installed, out).unwrap();
    let mut sw = WaxWriter::new(SCIENCE).created_at(1_700_100_000).open_append(out).unwrap();
    sw.set_manifest(&manifest("Science", Some("2026.10.1")));
    page(&mut sw, "Carbon.html", "Carbon", &[("Chemistry", "Carbon forms more compounds than any element.")]);
    sw.add_tombstone("Helium.html").unwrap();
    sw.finish().unwrap();
    out.to_path_buf()
}

fn desktop() -> (tempfile::TempDir, tempfile::TempDir, Desktop) {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let d = Desktop::open(data.path().join("Delt")).unwrap();
    (data, src, d)
}

fn host_of(d: &Desktop, uuid: &str) -> String {
    let p = d.packs().into_iter().find(|p| p.archive_uuid == uuid).unwrap();
    p.origin.rsplit("//").next().unwrap().to_string()
}

async fn get(d: &Desktop, uuid: &str, path: &str) -> delt_desktop::content::Served {
    d.serve_content(&host_of(d, uuid), &Method::GET, path, &HeaderMap::new()).await
}

fn header<'a>(s: &'a delt_desktop::content::Served, k: &str) -> Option<&'a str> {
    s.headers.iter().find(|(n, _)| n == k).and_then(|(_, v)| std::str::from_utf8(v).ok())
}

#[tokio::test]
async fn import_list_and_remove() {
    let (_data, src, d) = desktop();
    assert!(d.packs().is_empty());
    let s = science(src.path(), Some("2026.09.1"));
    let info = d.import(&s, ImportMode::Copy).unwrap();
    assert_eq!(info.archive_uuid, SCIENCE_ID);
    assert_eq!(info.name, "Science");
    assert_eq!(info.version.as_deref(), Some("2026.09.1"));
    assert!(info.searchable);
    assert!(s.is_file(), "copy leaves the original");
    assert!(d.library_dir().join("science.wax").is_file());

    // the same pack again is an update, not a second copy
    assert!(matches!(d.import(&s, ImportMode::Copy), Err(DesktopError::AlreadyInstalled(_))));

    let h = history(src.path());
    d.import(&h, ImportMode::Move).unwrap();
    assert!(!h.exists(), "move takes the original");
    let names: Vec<String> = d.packs().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["History", "Science"]);

    // not a pack at all
    let junk = src.path().join("junk.wax");
    std::fs::write(&junk, b"not a pack").unwrap();
    assert!(matches!(d.import(&junk, ImportMode::Copy), Err(DesktopError::NotAPack(..))));

    d.remove(HISTORY_ID).unwrap();
    assert_eq!(d.packs().len(), 1);
    assert!(matches!(d.remove(HISTORY_ID), Err(DesktopError::NoSuchPack(_))));
}

#[tokio::test]
async fn the_box_api_answers_the_window() {
    let (_data, src, d) = desktop();
    d.import(&science(src.path(), None), ImportMode::Copy).unwrap();
    d.import(&history(src.path()), ImportMode::Copy).unwrap();

    let (st, packs) = d.api_get("/packs", None).await;
    assert_eq!(st, 200, "{packs}");
    assert_eq!(packs["items"].as_array().unwrap().len(), 2);

    let (st, sug) = d.api_get("/suggest", Some("q=hyd")).await;
    assert_eq!(st, 200);
    assert_eq!(sug["items"][0]["title"], "Hydrogen");

    // one query across both packs, with the best passage as the snippet
    let (st, res) = d.search("rockets", 10, None).await;
    assert_eq!(st, 200, "{res}");
    let items = res["items"].as_array().unwrap();
    let paths: Vec<&str> = items.iter().map(|i| i["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"Rockets.html") && paths.contains(&"Hydrogen.html"), "{paths:?}");
    let hyd = items.iter().find(|i| i["path"] == "Hydrogen.html").unwrap();
    let snip = hyd["snippet"].as_str().unwrap();
    assert!(snip.contains("fuels rockets"), "{snip}");
    assert!(!snip.contains("Cavendish"), "the matching passage, not the first: {snip}");

    // a passage that only repeats the title is passed over
    let rk = items.iter().find(|i| i["path"] == "Rockets.html").unwrap();
    assert_eq!(rk["snippet"], "Early rockets Gunpowder rockets flew in China. Home", "{rk}");

    // paging uses the API's own cursor
    let (_, first) = d.search("rockets", 1, None).await;
    let next = first["next"].as_str().unwrap().to_string();
    let (st, second) = d.search("rockets", 1, Some(&next)).await;
    assert_eq!(st, 200);
    assert_ne!(first["items"][0]["path"], second["items"][0]["path"]);
}

#[tokio::test]
async fn pages_are_served_by_the_pack_zone_and_remembered() {
    let (_data, src, d) = desktop();
    d.import(&science(src.path(), None), ImportMode::Copy).unwrap();

    let s = get(&d, SCIENCE_ID, "/Hydrogen.html").await;
    assert_eq!(s.status, 200);
    assert!(String::from_utf8_lossy(&s.body).contains("inflammable air"));
    assert_eq!(header(&s, "content-type"), Some("text/html"));
    let csp = header(&s, "content-security-policy").unwrap();
    assert!(csp.starts_with("default-src 'self'"), "{csp}");
    assert!(csp.ends_with(&format!("frame-ancestors {}", delt_desktop::content::WINDOW_ORIGINS)), "{csp}");
    assert!(header(&s, "etag").is_some());
    let nav = s.navigated.unwrap();
    assert_eq!((nav.pack.as_str(), nav.path.as_str(), nav.title.as_str()), (SCIENCE_ID, "Hydrogen.html", "Hydrogen"));
    let hist = d.store().history(10).unwrap();
    assert_eq!(hist[0].title, "Hydrogen");

    // the pack's root sends to its entry point; unknown pages are 404
    let root = get(&d, SCIENCE_ID, "/").await;
    assert_eq!(root.status, 302);
    assert!(root.navigated.is_none());
    assert_eq!(get(&d, SCIENCE_ID, "/Nope.html").await.status, 404);

    // a host that names no pack, or another generation, serves nothing
    let none = d.serve_content("pack-99-g1.localhost", &Method::GET, "/index.html", &HeaderMap::new()).await;
    assert_eq!(none.status, 404);
    let stale = d.serve_content("pack-0-g99.localhost", &Method::GET, "/index.html", &HeaderMap::new()).await;
    assert_eq!(stale.status, 404);

    // a subresource fetch is not a navigation
    let mut h = HeaderMap::new();
    h.insert("sec-fetch-dest", "script".parse().unwrap());
    let sub = d.serve_content(&host_of(&d, SCIENCE_ID), &Method::GET, "/Helium.html", &h).await;
    assert_eq!(sub.status, 200);
    assert!(sub.navigated.is_none());
}

#[tokio::test]
async fn an_appended_edition_is_applied_in_place_and_undone() {
    let (_data, src, d) = desktop();
    d.import(&science(src.path(), Some("2026.09.1")), ImportMode::Copy).unwrap();
    let installed = d.library_dir().join("science.wax");
    let before = std::fs::read(&installed).unwrap();
    let newer = appended_edition(&installed, &src.path().join("science-2026.10.wax"));

    let plan = d.check_update(&newer).unwrap();
    let UpdateKind::Append { new_updates, bytes } = plan.kind.clone() else { panic!("{plan:?}") };
    assert_eq!(new_updates, 1);
    assert!(bytes > 0 && bytes < std::fs::metadata(&newer).unwrap().len());
    assert!(!plan.signature_sidecar);
    // the appended edition says its own version (SPEC v0.13 §5.6.1)
    assert_eq!(plan.installed_version.as_deref(), Some("2026.09.1"));
    assert_eq!(plan.candidate_version.as_deref(), Some("2026.10.1"));
    // the folder scan finds the same plan
    let found = d.find_updates(src.path()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].candidate, newer);

    let done = d.apply_update(&plan).unwrap();
    assert!(done.can_undo);
    assert_eq!(done.version.as_deref(), Some("2026.10.1"));
    assert_eq!(d.packs()[0].version.as_deref(), Some("2026.10.1"));
    assert_eq!(std::fs::read(&installed).unwrap(), std::fs::read(&newer).unwrap(), "byte for byte the new edition");
    // the running library already serves it: new page in, removed page gone
    assert_eq!(get(&d, SCIENCE_ID, "/Carbon.html").await.status, 200);
    assert_eq!(get(&d, SCIENCE_ID, "/Helium.html").await.status, 410);
    let (_, res) = d.search("compounds", 10, None).await;
    assert_eq!(res["items"][0]["path"], "Carbon.html", "{res}");
    // and the update cannot be applied twice
    assert!(matches!(d.check_update(&newer), Err(DesktopError::Update(UpdateError::AlreadyCurrent))));
    // a plan made before a change is refused
    assert!(matches!(d.apply_update(&plan), Err(DesktopError::Update(UpdateError::Stale))));

    let undone = d.undo_update(SCIENCE_ID).unwrap();
    assert!(!undone.can_undo);
    assert_eq!(undone.version.as_deref(), Some("2026.09.1"), "undo brings the old manifest back");
    assert_eq!(std::fs::read(&installed).unwrap(), before, "back byte for byte");
    assert_eq!(get(&d, SCIENCE_ID, "/Helium.html").await.status, 200);
    assert!(matches!(d.undo_update(SCIENCE_ID), Err(DesktopError::Update(UpdateError::NothingToUndo(_)))));

    // the older edition is recognised as older from the newer side too
    let (_d2, _s2, other) = desktop();
    other.import(&newer, ImportMode::Copy).unwrap();
    let old_copy = src.path().join("science-old.wax");
    std::fs::write(&old_copy, &before).unwrap();
    assert!(matches!(other.check_update(&old_copy), Err(DesktopError::Update(UpdateError::Older))));
}

#[tokio::test]
async fn a_rebuilt_edition_replaces_the_file_only_when_newer() {
    let (_data, src, d) = desktop();
    d.import(&science(src.path(), Some("2026.09.1")), ImportMode::Copy).unwrap();

    let rebuilt_dir = tempfile::tempdir().unwrap();
    let rebuilt = rebuild(rebuilt_dir.path(), "2026.10.4");
    std::fs::write(delt_desktop::updates::sidecar_of(&rebuilt), b"untrusted comment: test\n").unwrap();
    let plan = d.check_update(&rebuilt).unwrap();
    assert!(matches!(plan.kind, UpdateKind::Replace { .. }), "{plan:?}");
    assert!(plan.signature_sidecar);
    assert_eq!(plan.candidate_version.as_deref(), Some("2026.10.4"));
    d.apply_update(&plan).unwrap();
    let info = d.packs().into_iter().next().unwrap();
    assert_eq!(info.version.as_deref(), Some("2026.10.4"));
    assert!(!info.can_undo_update);
    assert!(delt_desktop::updates::sidecar_of(&d.library_dir().join("science.wax")).is_file());
    assert_eq!(get(&d, SCIENCE_ID, "/Oxygen.html").await.status, 200);

    let older_dir = tempfile::tempdir().unwrap();
    let older = rebuild(older_dir.path(), "2026.09.2");
    assert!(matches!(d.check_update(&older), Err(DesktopError::Update(UpdateError::Older))));

    // without a version on both sides, newer is unknown: refused, not guessed
    let nov_dir = tempfile::tempdir().unwrap();
    let nov = science(nov_dir.path(), None);
    assert!(matches!(d.check_update(&nov), Err(DesktopError::Update(UpdateError::CannotOrder(_)))));

    // another pack entirely is not an update of anything installed
    assert!(matches!(d.check_update(&history(src.path())), Err(DesktopError::Update(UpdateError::DifferentArchive))));
}

fn rebuild(dir: &Path, version: &str) -> PathBuf {
    let out = dir.join("science.wax");
    let mut sw = WaxWriter::new(SCIENCE).created_at(1_700_200_000).create(&out, &manifest("Science", Some(version))).unwrap();
    page(&mut sw, "index.html", "Science", &[("Welcome", "Pages about the elements, rebuilt.")]);
    page(&mut sw, "Oxygen.html", "Oxygen", &[("Breathing", "Oxygen is what we breathe.")]);
    sw.finish().unwrap();
    out
}

#[tokio::test]
async fn bookmarks_notes_and_settings_survive_a_restart() {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let origin = {
        let d = Desktop::open(data.path()).unwrap();
        d.import(&science(src.path(), None), ImportMode::Copy).unwrap();
        d.store().add_bookmark(SCIENCE_ID, "Hydrogen.html", "Hydrogen").unwrap();
        d.store().add_note(SCIENCE_ID, "Hydrogen.html", "Hydrogen", "inflammable air", "Ask about Cavendish").unwrap();
        d.save_appearance(delt_desktop::store::ThemeChoice::Dark, true).unwrap();
        d.packs()[0].origin.clone()
    };
    let d = Desktop::open(data.path()).unwrap();
    assert_eq!(d.packs().len(), 1);
    assert_eq!(d.store().bookmarks().unwrap()[0].title, "Hydrogen");
    let md = d.export_notes().unwrap();
    assert!(md.contains("## Hydrogen — Science") && md.contains("> inflammable air") && md.contains("Ask about Cavendish"), "{md}");
    let s = d.settings().unwrap();
    assert_eq!(s.theme, delt_desktop::store::ThemeChoice::Dark);
    assert!(s.colorblind);
    // the pack keeps its address across a restart, so bookmarks still open
    assert_eq!(d.packs()[0].origin, origin);
    let (_, packs) = d.api_get("/packs", None).await;
    assert_eq!(packs["items"][0]["archive_uuid"], SCIENCE_ID, "{packs}");
}

#[tokio::test]
async fn the_library_folder_can_be_moved_to_a_drive_of_packs() {
    let (_data, src, d) = desktop();
    science(src.path(), None);
    history(src.path());
    d.set_library_dir(src.path()).unwrap();
    assert_eq!(d.packs().len(), 2);
    assert!(d.set_library_dir(&src.path().join("missing")).is_err());
    assert_eq!(d.settings().unwrap().library_dir.as_deref(), Some(src.path().to_string_lossy().as_ref()));
}
