//! Delt for the desktop: the window.
//!
//! Everything it does is in `delt-desktop`; this file only connects the
//! window to it:
//!
//! * the `deltpack:` scheme, answered by deltlib's pack zone in-process;
//! * commands the window's script calls (library, search, bookmarks,
//!   notes, history, settings, updates), each a thin wrapper;
//! * file and folder pickers, run here so the page never touches paths it
//!   was not given.
//!
//! Pack pages run in a sandboxed frame from another scheme, so they cannot
//! call any of these commands (Tauri gives commands to the window's own
//! origin only, and the capability file names no other).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use delt_desktop::store::{Bookmark, Note, Settings, ThemeChoice, Visit};
use delt_desktop::updates::{Applied, UpdatePlan};
use delt_desktop::{content, Desktop, ImportMode, PackInfo};
use serde_json::{json, Value};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

type D<'a> = State<'a, Arc<Desktop>>;
type R<T> = Result<T, String>;

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

/// Run blocking work (file copies, updates) off the window's thread.
async fn blocking<T: Send + 'static>(d: &Arc<Desktop>, f: impl FnOnce(&Desktop) -> R<T> + Send + 'static) -> R<T> {
    let d = Arc::clone(d);
    tauri::async_runtime::spawn_blocking(move || f(&d)).await.map_err(e)?
}

// ---- library ----

#[tauri::command]
fn packs(d: D<'_>) -> Vec<PackInfo> {
    d.packs()
}

#[tauri::command]
fn refresh(d: D<'_>) -> R<Value> {
    let r = d.refresh().map_err(e)?;
    Ok(json!({ "added": r.added, "updated": r.updated, "removed": r.removed,
               "damaged": r.damaged.iter().map(|p| p.display().to_string()).collect::<Vec<_>>() }))
}

#[tauri::command]
async fn import_pack(d: D<'_>, path: String, mode: ImportMode) -> R<PackInfo> {
    blocking(&d, move |d| d.import(&PathBuf::from(path), mode).map_err(e)).await
}

#[tauri::command]
async fn remove_pack(d: D<'_>, uuid: String) -> R<()> {
    blocking(&d, move |d| d.remove(&uuid).map_err(e)).await
}

#[tauri::command]
async fn set_library_dir(d: D<'_>, path: String) -> R<()> {
    blocking(&d, move |d| d.set_library_dir(&PathBuf::from(path)).map_err(e)).await
}

#[tauri::command]
fn library_dir(d: D<'_>) -> String {
    d.library_dir().display().to_string()
}

// ---- the box API, and search ----

#[tauri::command]
async fn api_get(d: D<'_>, path: String, query: Option<String>) -> R<Value> {
    let (status, body) = d.api_get(&path, query.as_deref()).await;
    Ok(json!({ "status": status, "body": body }))
}

#[tauri::command]
async fn search(d: D<'_>, q: String, limit: Option<usize>, cursor: Option<String>) -> R<Value> {
    let (status, body) = d.search(&q, limit.unwrap_or(20), cursor.as_deref()).await;
    Ok(json!({ "status": status, "body": body }))
}

// ---- updates ----

#[tauri::command]
async fn check_update(d: D<'_>, path: String) -> R<UpdatePlan> {
    blocking(&d, move |d| d.check_update(&PathBuf::from(path)).map_err(e)).await
}

#[tauri::command]
async fn find_updates(d: D<'_>, dir: String) -> R<Vec<UpdatePlan>> {
    blocking(&d, move |d| d.find_updates(&PathBuf::from(dir)).map_err(e)).await
}

/// Checks again and applies: a plan is never carried through the window.
#[tauri::command]
async fn apply_update(d: D<'_>, path: String) -> R<Applied> {
    blocking(&d, move |d| {
        let plan = d.check_update(&PathBuf::from(path)).map_err(e)?;
        d.apply_update(&plan).map_err(e)
    })
    .await
}

#[tauri::command]
async fn undo_update(d: D<'_>, uuid: String) -> R<Applied> {
    blocking(&d, move |d| d.undo_update(&uuid).map_err(e)).await
}

// ---- the person's own data ----

#[tauri::command]
fn bookmarks(d: D<'_>) -> R<Vec<Bookmark>> {
    d.store().bookmarks().map_err(e)
}

#[tauri::command]
fn add_bookmark(d: D<'_>, pack: String, path: String, title: String) -> R<Bookmark> {
    d.store().add_bookmark(&pack, &path, &title).map_err(e)
}

#[tauri::command]
fn remove_bookmark(d: D<'_>, id: i64) -> R<bool> {
    d.store().remove_bookmark(id).map_err(e)
}

#[tauri::command]
fn bookmark_of(d: D<'_>, pack: String, path: String) -> R<Option<i64>> {
    d.store().is_bookmarked(&pack, &path).map_err(e)
}

#[tauri::command]
fn notes(d: D<'_>, pack: Option<String>, path: Option<String>) -> R<Vec<Note>> {
    let page = match (&pack, &path) {
        (Some(a), Some(b)) => Some((a.as_str(), b.as_str())),
        _ => None,
    };
    d.store().notes(page).map_err(e)
}

#[tauri::command]
fn add_note(d: D<'_>, pack: String, path: String, page_title: String, quote: String, body: String) -> R<Note> {
    d.store().add_note(&pack, &path, &page_title, &quote, &body).map_err(e)
}

#[tauri::command]
fn update_note(d: D<'_>, id: i64, body: String) -> R<Option<Note>> {
    d.store().update_note(id, &body).map_err(e)
}

#[tauri::command]
fn remove_note(d: D<'_>, id: i64) -> R<bool> {
    d.store().remove_note(id).map_err(e)
}

#[tauri::command]
fn history(d: D<'_>, limit: Option<usize>) -> R<Vec<Visit>> {
    d.store().history(limit.unwrap_or(100)).map_err(e)
}

#[tauri::command]
fn clear_history(d: D<'_>) -> R<()> {
    d.store().clear_history().map_err(e)
}

#[tauri::command]
fn settings(d: D<'_>) -> R<Settings> {
    d.settings().map_err(e)
}

#[tauri::command]
fn save_appearance(d: D<'_>, theme: ThemeChoice, colorblind: bool) -> R<Settings> {
    d.save_appearance(theme, colorblind).map_err(e)
}

#[tauri::command]
fn theme_css() -> String {
    delt_desktop::theme::css()
}

// ---- pickers (paths come only from these) ----

#[tauri::command]
async fn pick_pack(app: tauri::AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .add_filter("WAX packs", &["wax"])
        .blocking_pick_file()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.display().to_string())
}

#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Option<String> {
    app.dialog().file().blocking_pick_folder().and_then(|p| p.into_path().ok()).map(|p| p.display().to_string())
}

/// Ask where to save the notes, then write them there as Markdown.
#[tauri::command]
async fn export_notes(app: tauri::AppHandle, d: D<'_>) -> R<Option<String>> {
    let md = d.export_notes().map_err(e)?;
    let Some(p) = app
        .dialog()
        .file()
        .add_filter("Markdown", &["md"])
        .set_file_name("My notes.md")
        .blocking_save_file()
        .and_then(|p| p.into_path().ok())
    else {
        return Ok(None);
    };
    std::fs::write(&p, md).map_err(e)?;
    Ok(Some(p.display().to_string()))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let d = Desktop::open(dir).map_err(|x| Box::new(x) as Box<dyn std::error::Error>)?;
            app.manage(Arc::new(d));
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol(content::SCHEME, |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                let d = Arc::clone(app.state::<Arc<Desktop>>().inner());
                let host = request.uri().host().unwrap_or_default().to_string();
                let path = request.uri().path().to_string();
                let served = d.serve_content(&host, request.method(), &path, request.headers()).await;
                if let Some(nav) = &served.navigated {
                    let _ = app.emit("delt-navigated", nav);
                }
                let mut b = tauri::http::Response::builder().status(served.status);
                for (k, v) in served.headers {
                    b = b.header(k, v);
                }
                let resp = b.body(served.body).unwrap_or_else(|_| {
                    tauri::http::Response::builder().status(500).body(Vec::new()).expect("a bare response")
                });
                responder.respond(resp);
            });
        })
        .invoke_handler(tauri::generate_handler![
            packs,
            refresh,
            import_pack,
            remove_pack,
            set_library_dir,
            library_dir,
            api_get,
            search,
            check_update,
            find_updates,
            apply_update,
            undo_update,
            bookmarks,
            add_bookmark,
            remove_bookmark,
            bookmark_of,
            notes,
            add_note,
            update_note,
            remove_note,
            history,
            clear_history,
            settings,
            save_appearance,
            theme_css,
            pick_pack,
            pick_folder,
            export_notes,
        ])
        .run(tauri::generate_context!())
        .expect("Delt could not start");
}
