//! `delt-desktop` — the core of Delt for the desktop, the reader for `.wax`
//! packs on a computer of one's own (the Kiwix-Desktop equivalent).
//!
//! It is **deltlib run in-process for one person**, plus what only a
//! desktop needs:
//!
//! * **The box's code, not a copy of it.** The library is a
//!   [`deltlib::Library`] over a folder; the window's library, search and
//!   completion calls are answered by [`deltlib::api::Api`] (deltd API v1)
//!   and pack pages by [`deltlib::packzone::serve`] — the very functions
//!   `deltd` runs. Nothing listens on a network port: the window reaches
//!   them by in-process calls and a private URL scheme ([`content`]).
//! * **Desktop-only parts:** adding and removing packs ([`Desktop::import`],
//!   [`Desktop::remove`]), bookmarks, private notes, reading history and
//!   settings ([`store`]), light/dark/colour-blind themes ([`theme`]), and
//!   applying and undoing edition updates ([`updates`]).
//!
//! The window itself (Tauri) is the separate `delt-desktop-app` crate, kept
//! out of the workspace so building the format and the box never needs a
//! web view toolkit.

pub mod content;
pub mod identity;
pub mod store;
pub mod theme;
pub mod updates;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use axum::http::{header, HeaderMap, HeaderValue, Method};
use deltlib::library::RefreshReport;
use deltlib::{Catalog, Identity, Library};
use serde::Serialize;
use serde_json::Value;

use crate::identity::{random_hex, LocalIdentity};
use crate::store::{Settings, Store};

/// Readers kept open per pack. A desktop has one reader at a time, plus
/// search running beside the page.
pub const READERS_PER_PACK: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error(transparent)]
    Store(#[from] store::StoreError),
    #[error(transparent)]
    Update(#[from] updates::UpdateError),
    #[error("pack catalog: {0}")]
    Catalog(#[from] deltlib::catalog::CatalogError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0} is not a pack Delt can open: {1}")]
    NotAPack(PathBuf, String),
    #[error("this pack is already in your library (as {0}); to bring it up to date, use Update instead")]
    AlreadyInstalled(String),
    #[error("no pack with id {0} is in the library")]
    NoSuchPack(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, DesktopError>;

/// How a pack gets into the library folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Leave the original where it is.
    Copy,
    /// Move it in (a rename when on the same drive, otherwise copy and delete).
    Move,
}

/// One pack, as the window's library screen shows it.
#[derive(Debug, Clone, Serialize)]
pub struct PackInfo {
    pub archive_uuid: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub languages: Vec<String>,
    pub file: String,
    pub file_size: u64,
    pub searchable: bool,
    pub entry_point: Option<String>,
    /// Where the window loads this pack's pages from.
    pub origin: String,
    pub can_undo_update: bool,
}

pub struct Desktop {
    data_dir: PathBuf,
    library_dir: RwLock<PathBuf>,
    lib: RwLock<Arc<Library>>,
    ident: Arc<LocalIdentity>,
    store: Mutex<Store>,
}

impl Desktop {
    /// Open (or create) Delt's data in `data_dir`: `user.sqlite` (the
    /// person's own data), `catalog.sqlite` (pack addresses) and, unless the
    /// settings name another, the library folder `packs/`.
    pub fn open(data_dir: impl Into<PathBuf>) -> Result<Desktop> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)?;
        let store = Store::open(&data_dir.join("user.sqlite"))?;
        let token = store.install_secret("session_token", || random_hex(32))?;
        let partition = store.install_secret("pack_partition", || random_hex(8))?;
        let settings = store.settings()?;
        let library_dir = settings
            .library_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("packs"));
        std::fs::create_dir_all(&library_dir)?;
        let lib = Self::make_library(&data_dir, &library_dir)?;
        let d = Desktop {
            data_dir,
            library_dir: RwLock::new(library_dir),
            lib: RwLock::new(lib),
            ident: Arc::new(LocalIdentity::new(token, partition)),
            store: Mutex::new(store),
        };
        d.refresh()?;
        Ok(d)
    }

    fn make_library(data_dir: &Path, library_dir: &Path) -> Result<Arc<Library>> {
        let catalog = Catalog::open(&data_dir.join("catalog.sqlite"))?;
        Ok(Arc::new(Library::new(library_dir, catalog, READERS_PER_PACK)))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn library_dir(&self) -> PathBuf {
        self.library_dir.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn library(&self) -> Arc<Library> {
        Arc::clone(&self.lib.read().unwrap_or_else(|p| p.into_inner()))
    }

    pub fn identity(&self) -> Arc<dyn Identity> {
        Arc::clone(&self.ident) as Arc<dyn Identity>
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Rescan the library folder: new files, changed editions, removals.
    pub fn refresh(&self) -> Result<RefreshReport> {
        Ok(self.library().refresh()?)
    }

    /// Read packs from another folder from now on (a drive of packs, say).
    pub fn set_library_dir(&self, dir: &Path) -> Result<()> {
        if !dir.is_dir() {
            return Err(DesktopError::Invalid(format!("{} is not a folder", dir.display())));
        }
        let lib = Self::make_library(&self.data_dir, dir)?;
        lib.refresh()?;
        *self.lib.write().unwrap_or_else(|p| p.into_inner()) = lib;
        *self.library_dir.write().unwrap_or_else(|p| p.into_inner()) = dir.to_path_buf();
        let store = self.store();
        let mut s = store.settings()?;
        s.library_dir = Some(dir.to_string_lossy().into_owned());
        store.save_settings(&s)?;
        Ok(())
    }

    pub fn settings(&self) -> Result<Settings> {
        Ok(self.store().settings()?)
    }

    /// Save theme and colour-blind choices. The library folder changes only
    /// through [`Desktop::set_library_dir`].
    pub fn save_appearance(&self, theme: store::ThemeChoice, colorblind: bool) -> Result<Settings> {
        let store = self.store();
        let mut s = store.settings()?;
        s.theme = theme;
        s.colorblind = colorblind;
        store.save_settings(&s)?;
        Ok(s)
    }

    /// The packs in the library, by name.
    pub fn packs(&self) -> Vec<PackInfo> {
        let snap = self.library().snapshot();
        let mut v: Vec<PackInfo> = snap
            .by_uuid
            .values()
            .map(|p| {
                let m = |k: &str| p.manifest.get(k).cloned();
                PackInfo {
                    archive_uuid: p.archive_uuid.clone(),
                    name: p.name().to_string(),
                    title: m("title"),
                    description: m("description"),
                    version: m("version"),
                    languages: p.languages(),
                    file: p.path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
                    file_size: p.file_size,
                    searchable: p.search_tokenizer.is_some(),
                    entry_point: m("entry_point"),
                    origin: content::pack_origin(p.slot.pack_index, p.slot.pack_generation),
                    can_undo_update: updates::can_undo(&p.path),
                }
            })
            .collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.archive_uuid.cmp(&b.archive_uuid)));
        v
    }

    fn pack_path(&self, uuid: &str) -> Result<PathBuf> {
        self.library()
            .snapshot()
            .by_uuid
            .get(uuid)
            .map(|p| p.path.clone())
            .ok_or_else(|| DesktopError::NoSuchPack(uuid.to_string()))
    }

    pub fn pack_name(&self, uuid: &str) -> Option<String> {
        self.library().snapshot().by_uuid.get(uuid).map(|p| p.name().to_string())
    }

    /// Ask the deltd API v1 (`GET` only: Library and Search groups,
    /// `/status`) exactly as a browser on the box would, as the desktop's
    /// one person. `path` is below `/api/v1/` (`/packs`, `/search`, …).
    /// Returns the status and the JSON body.
    pub async fn api_get(&self, path: &str, query: Option<&str>) -> (u16, Value) {
        let api = deltlib::api::Api {
            lib: self.library(),
            ident: self.identity(),
            webos_host: String::new(),
            https_available: false,
        };
        let mut h = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&format!("__Host-delt_session={}", self.ident.token())) {
            h.insert(header::COOKIE, v);
        }
        let full = format!("/api/v1/{}", path.trim_start_matches('/'));
        let resp = api.handle(&Method::GET, &full, query, &h, &[], &deltlib::ConnInfo::default()).await;
        let status = resp.status().as_u16();
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap_or_default();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    /// Search across every pack (deltd's `/search`), with each hit's best
    /// matching passage as its snippet where the pack has passages (SPEC
    /// §5.10). The box API leaves `snippet` empty; the desktop fills it for
    /// the hits it shows.
    /// `cursor` is the `next` value of the previous page, passed back as is.
    pub async fn search(&self, q: &str, limit: usize, cursor: Option<&str>) -> (u16, Value) {
        let mut query = format!("q={}&limit={limit}", encode(q));
        if let Some(c) = cursor {
            query.push_str(&format!("&cursor={}", encode(c)));
        }
        let (status, mut v) = self.api_get("/search", Some(&query)).await;
        if status != 200 {
            return (status, v);
        }
        let lib = self.library();
        let words = q.to_string();
        if let Some(items) = v.get_mut("items").and_then(Value::as_array_mut) {
            let snap = lib.snapshot();
            for it in items.iter_mut() {
                let (Some(pack), Some(path)) = (it["pack"].as_str(), it["path"].as_str()) else { continue };
                let Some(p) = snap.by_uuid.get(pack) else { continue };
                let snippet = p.readers.with(|r| snippet_for(r, path, &words));
                if let Some(s) = snippet {
                    it["snippet"] = Value::String(s);
                }
            }
        }
        (status, v)
    }

    /// Serve a `deltpack:` request and record the visit when it was a page.
    pub async fn serve_content(&self, host: &str, method: &Method, path: &str, headers: &HeaderMap) -> content::Served {
        let served = content::serve(self.library(), self.identity(), self.ident.partition(), host, method, path, headers).await;
        if let Some(n) = &served.navigated {
            let _ = self.store().record_visit(&n.pack, &n.path, &n.title);
        }
        served
    }

    /// Put a pack into the library folder. A pack already in the library
    /// (same `archive_uuid`) is refused: that is an update.
    pub fn import(&self, file: &Path, mode: ImportMode) -> Result<PackInfo> {
        let r = wax_core::WaxReader::open(file).map_err(|e| DesktopError::NotAPack(file.to_path_buf(), e.to_string()))?;
        let uuid = deltlib::library::uuid_text(&r.header().archive_uuid);
        drop(r);
        if let Some(p) = self.library().snapshot().by_uuid.get(&uuid) {
            return Err(DesktopError::AlreadyInstalled(p.name().to_string()));
        }
        let dir = self.library_dir();
        let dest = free_name(&dir, file);
        if file.parent().map(|p| same_dir(p, &dir)).unwrap_or(false) {
            // already in the library folder: nothing to place
        } else {
            match mode {
                ImportMode::Move if std::fs::rename(file, &dest).is_ok() => {}
                _ => {
                    let part = dest.with_extension("wax-part");
                    std::fs::copy(file, &part)?;
                    std::fs::File::open(&part)?.sync_all()?;
                    std::fs::rename(&part, &dest)?;
                    if mode == ImportMode::Move {
                        std::fs::remove_file(file)?;
                    }
                }
            }
            let side = updates::sidecar_of(file);
            if side.is_file() {
                std::fs::copy(&side, updates::sidecar_of(&dest))?;
                if mode == ImportMode::Move {
                    let _ = std::fs::remove_file(&side);
                }
            }
        }
        self.refresh()?;
        self.packs()
            .into_iter()
            .find(|p| p.archive_uuid == uuid)
            .ok_or_else(|| DesktopError::NotAPack(dest, "it was placed in the library but did not open".into()))
    }

    /// Delete a pack's file (and signature sidecar) from the library folder.
    /// Bookmarks and notes about it are kept: they are the person's, and
    /// come back to life if the pack is added again.
    pub fn remove(&self, uuid: &str) -> Result<()> {
        let path = self.pack_path(uuid)?;
        std::fs::remove_file(&path)?;
        let _ = std::fs::remove_file(updates::sidecar_of(&path));
        self.refresh()?;
        Ok(())
    }

    /// Check whether `candidate` is a newer edition of an installed pack.
    pub fn check_update(&self, candidate: &Path) -> Result<updates::UpdatePlan> {
        let r = wax_core::WaxReader::open(candidate)
            .map_err(|e| DesktopError::NotAPack(candidate.to_path_buf(), e.to_string()))?;
        let uuid = deltlib::library::uuid_text(&r.header().archive_uuid);
        drop(r);
        let installed = self.pack_path(&uuid).map_err(|_| updates::UpdateError::DifferentArchive)?;
        Ok(updates::inspect(&installed, candidate)?)
    }

    /// Newer editions of installed packs found directly inside `dir`.
    pub fn find_updates(&self, dir: &Path) -> Result<Vec<updates::UpdatePlan>> {
        let installed: Vec<(String, PathBuf)> = self
            .library()
            .snapshot()
            .by_uuid
            .values()
            .map(|p| (p.archive_uuid.clone(), p.path.clone()))
            .collect();
        Ok(updates::find_updates(dir, &installed)?)
    }

    pub fn apply_update(&self, plan: &updates::UpdatePlan) -> Result<updates::Applied> {
        let done = updates::apply(plan)?;
        self.refresh()?;
        Ok(done)
    }

    pub fn undo_update(&self, uuid: &str) -> Result<updates::Applied> {
        let path = self.pack_path(uuid)?;
        let done = updates::undo_last_update(&path)?;
        self.refresh()?;
        Ok(done)
    }

    /// Every note, as Markdown.
    pub fn export_notes(&self) -> Result<String> {
        let names = |u: &str| self.pack_name(u);
        Ok(self.store().export_notes_markdown(names)?)
    }
}

/// The text of the passage that best matches `q` in one page, trimmed for
/// a results list.
fn snippet_for(r: &wax_core::WaxReader, path: &str, q: &str) -> Option<String> {
    let tok = r.search_tokenizer()?.to_string();
    let expr = deltlib::search::match_expr(q, &tok, false).ok()?;
    let resolved = r.resolve(path).ok()?.entry;
    let title = resolved.title.clone().unwrap_or_default();
    let content = r.read(&resolved.path).ok()?;
    let text_of = |p: &wax_core::Passage| {
        let span = content.get(p.start as usize..p.end as usize)?;
        let t = wax_core::text::html_to_text(&String::from_utf8_lossy(span));
        Some(t.split_whitespace().collect::<Vec<_>>().join(" "))
    };
    // the best match, unless all it says is the page's own title (the
    // passage before the first heading often is): then the next best
    let best = r.best_passages(path, &expr, SNIPPET_CANDIDATES).ok()?;
    let text = best
        .iter()
        .filter_map(text_of)
        .find(|t| !t.is_empty() && *t != title)?;
    Some(clip(&text, SNIPPET_CHARS))
}

/// Passages considered for one snippet.
const SNIPPET_CANDIDATES: usize = 3;

/// Characters of passage text a result shows.
pub const SNIPPET_CHARS: usize = 220;

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let cut: String = s.chars().take(n).collect();
    let cut = cut.rsplit_once(' ').map(|(a, _)| a.to_string()).unwrap_or(cut);
    format!("{cut}…")
}

fn encode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// `dir/<file name>`, numbered if that name is taken.
fn free_name(dir: &Path, file: &Path) -> PathBuf {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "pack".into());
    let mut p = dir.join(format!("{stem}.wax"));
    if file.parent().map(|d| same_dir(d, dir)).unwrap_or(false) {
        return file.to_path_buf();
    }
    let mut i = 2;
    while p.exists() {
        p = dir.join(format!("{stem}-{i}.wax"));
        i += 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_keeps_whole_words() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("one two three four", 9), "one two…");
    }
}
