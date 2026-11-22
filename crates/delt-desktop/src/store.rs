//! The reader's own data: bookmarks, private notes, reading history and
//! settings, in one SQLite file in the user's data folder.
//!
//! * **Private.** Notes and history never leave this file: nothing here is
//!   sent anywhere, and no pack can read it (pack pages run in a sandboxed
//!   frame with no access to the window, see `content`).
//! * **Schema-versioned, and a newer schema is refused** rather than
//!   opened (contract §10's rule for state under `/var/lib/deltos`, applied here too: an older Delt
//!   must not rewrite data a newer one migrated).
//! * Timestamps are Unix epoch **milliseconds**, UTC (contract §5.2's rule).

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// The schema this build writes and understands.
pub const SCHEMA_VERSION: i64 = 1;

/// Recent pages kept in history; older visits are dropped.
pub const HISTORY_KEEP: i64 = 500;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("your reading data was saved by a newer Delt (data version {found}, this one understands {SCHEMA_VERSION}); update Delt to open it")]
    NewerSchema { found: i64 },
    #[error("reading data: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: i64,
    pub pack: String,
    pub path: String,
    pub title: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub pack: String,
    pub path: String,
    pub page_title: String,
    /// The passage the note is about, as the reader selected it (may be empty).
    pub quote: String,
    pub body: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visit {
    pub pack: String,
    pub path: String,
    pub title: String,
    pub visited_at: i64,
}

/// Theme choice. `System` follows the computer's light/dark setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// State colours that do not rely on telling red from green.
    pub colorblind: bool,
    /// The folder the library reads packs from; `None` = the default
    /// folder inside the data folder.
    pub library_dir: Option<String>,
}

/// Dark is the standard look (Commander, 2026-11-22). A person who already
/// saved a choice keeps it: only an unsaved setting takes this default.
impl Default for Settings {
    fn default() -> Self {
        Settings { theme: ThemeChoice::Dark, colorblind: false, library_dir: None }
    }
}

pub struct Store {
    db: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Store> {
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        let found: Option<i64> = db
            .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get::<_, String>(0))
            .optional()?
            .and_then(|v| v.parse().ok());
        match found {
            Some(v) if v > SCHEMA_VERSION => return Err(StoreError::NewerSchema { found: v }),
            Some(_) => {}
            None => {
                db.execute_batch(
                    "BEGIN;
                     CREATE TABLE bookmarks(
                         id INTEGER PRIMARY KEY, pack TEXT NOT NULL, path TEXT NOT NULL,
                         title TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE(pack, path));
                     CREATE TABLE notes(
                         id INTEGER PRIMARY KEY, pack TEXT NOT NULL, path TEXT NOT NULL,
                         page_title TEXT NOT NULL, quote TEXT NOT NULL, body TEXT NOT NULL,
                         created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                     CREATE INDEX notes_page ON notes(pack, path);
                     CREATE TABLE history(
                         pack TEXT NOT NULL, path TEXT NOT NULL, title TEXT NOT NULL,
                         visited_at INTEGER NOT NULL, PRIMARY KEY(pack, path));
                     CREATE INDEX history_recent ON history(visited_at);
                     CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                     CREATE TABLE secrets(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                     COMMIT;",
                )?;
                db.execute(
                    "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)",
                    params![SCHEMA_VERSION.to_string()],
                )?;
            }
        }
        Ok(Store { db })
    }

    /// A per-install random value, made once and then kept.
    pub fn install_secret(&self, key: &str, make: impl FnOnce() -> String) -> Result<String> {
        if let Some(v) = self
            .db
            .query_row("SELECT value FROM secrets WHERE key=?1", params![key], |r| r.get(0))
            .optional()?
        {
            return Ok(v);
        }
        let v = make();
        self.db.execute("INSERT INTO secrets(key, value) VALUES (?1, ?2)", params![key, v])?;
        Ok(v)
    }

    // ---- bookmarks ----

    /// Bookmark a page; bookmarking it again keeps the first one.
    pub fn add_bookmark(&self, pack: &str, path: &str, title: &str) -> Result<Bookmark> {
        self.db.execute(
            "INSERT OR IGNORE INTO bookmarks(pack, path, title, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![pack, path, title, now_ms()],
        )?;
        Ok(self
            .db
            .query_row(
                "SELECT id, pack, path, title, created_at FROM bookmarks WHERE pack=?1 AND path=?2",
                params![pack, path],
                bookmark_row,
            )?)
    }

    pub fn remove_bookmark(&self, id: i64) -> Result<bool> {
        Ok(self.db.execute("DELETE FROM bookmarks WHERE id=?1", params![id])? > 0)
    }

    pub fn is_bookmarked(&self, pack: &str, path: &str) -> Result<Option<i64>> {
        Ok(self
            .db
            .query_row("SELECT id FROM bookmarks WHERE pack=?1 AND path=?2", params![pack, path], |r| r.get(0))
            .optional()?)
    }

    /// Newest first.
    pub fn bookmarks(&self) -> Result<Vec<Bookmark>> {
        let mut st = self
            .db
            .prepare("SELECT id, pack, path, title, created_at FROM bookmarks ORDER BY created_at DESC, id DESC")?;
        let rows = st.query_map([], bookmark_row)?.collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    // ---- notes ----

    pub fn add_note(&self, pack: &str, path: &str, page_title: &str, quote: &str, body: &str) -> Result<Note> {
        if body.trim().is_empty() && quote.trim().is_empty() {
            return Err(StoreError::Invalid("a note needs some text".into()));
        }
        let t = now_ms();
        self.db.execute(
            "INSERT INTO notes(pack, path, page_title, quote, body, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![pack, path, page_title, quote, body, t],
        )?;
        self.note(self.db.last_insert_rowid())?.ok_or_else(|| StoreError::Invalid("note vanished".into()))
    }

    pub fn update_note(&self, id: i64, body: &str) -> Result<Option<Note>> {
        self.db.execute("UPDATE notes SET body=?2, updated_at=?3 WHERE id=?1", params![id, body, now_ms()])?;
        self.note(id)
    }

    pub fn remove_note(&self, id: i64) -> Result<bool> {
        Ok(self.db.execute("DELETE FROM notes WHERE id=?1", params![id])? > 0)
    }

    pub fn note(&self, id: i64) -> Result<Option<Note>> {
        Ok(self
            .db
            .query_row(&format!("{NOTE_COLS} WHERE id=?1"), params![id], note_row)
            .optional()?)
    }

    /// The notes on one page, oldest first (reading order), or every note,
    /// newest first.
    pub fn notes(&self, page: Option<(&str, &str)>) -> Result<Vec<Note>> {
        let rows = match page {
            Some((pack, path)) => {
                let mut st = self.db.prepare(&format!(
                    "{NOTE_COLS} WHERE pack=?1 AND path=?2 ORDER BY created_at, id"
                ))?;
                let r = st.query_map(params![pack, path], note_row)?.collect::<std::result::Result<_, _>>()?;
                r
            }
            None => {
                let mut st = self.db.prepare(&format!("{NOTE_COLS} ORDER BY updated_at DESC, id DESC"))?;
                let r = st.query_map([], note_row)?.collect::<std::result::Result<_, _>>()?;
                r
            }
        };
        Ok(rows)
    }

    /// Every note as one Markdown document, grouped by page: the person's
    /// own copy to keep or print. Nothing is sent anywhere.
    pub fn export_notes_markdown(&self, pack_name: impl Fn(&str) -> Option<String>) -> Result<String> {
        let mut st = self.db.prepare(&format!("{NOTE_COLS} ORDER BY pack, path, created_at, id"))?;
        let notes: Vec<Note> = st.query_map([], note_row)?.collect::<std::result::Result<_, _>>()?;
        let mut out = String::from("# My notes\n");
        let mut last: Option<(String, String)> = None;
        for n in notes {
            let key = (n.pack.clone(), n.path.clone());
            if last.as_ref() != Some(&key) {
                let pack = pack_name(&n.pack).unwrap_or_else(|| n.pack.clone());
                out.push_str(&format!("\n## {} — {}\n\n`{}`\n", n.page_title, pack, n.path));
                last = Some(key);
            }
            out.push('\n');
            if !n.quote.trim().is_empty() {
                for line in n.quote.lines() {
                    out.push_str(&format!("> {line}\n"));
                }
                out.push('\n');
            }
            if !n.body.trim().is_empty() {
                out.push_str(n.body.trim_end());
                out.push('\n');
            }
        }
        Ok(out)
    }

    // ---- history ----

    pub fn record_visit(&self, pack: &str, path: &str, title: &str) -> Result<()> {
        self.db.execute(
            // never earlier than the last visit, so two visits in one
            // millisecond still come back in the order they happened
            "INSERT INTO history(pack, path, title, visited_at)
             VALUES (?1, ?2, ?3, MAX(?4, (SELECT COALESCE(MAX(visited_at), 0) + 1 FROM history)))
             ON CONFLICT(pack, path) DO UPDATE SET title=excluded.title, visited_at=excluded.visited_at",
            params![pack, path, title, now_ms()],
        )?;
        self.db.execute(
            "DELETE FROM history WHERE rowid NOT IN
               (SELECT rowid FROM history ORDER BY visited_at DESC LIMIT ?1)",
            params![HISTORY_KEEP],
        )?;
        Ok(())
    }

    pub fn history(&self, limit: usize) -> Result<Vec<Visit>> {
        let mut st = self.db.prepare(
            "SELECT pack, path, title, visited_at FROM history ORDER BY visited_at DESC, rowid DESC LIMIT ?1",
        )?;
        let rows = st
            .query_map(params![limit as i64], |r| {
                Ok(Visit { pack: r.get(0)?, path: r.get(1)?, title: r.get(2)?, visited_at: r.get(3)? })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    pub fn clear_history(&self) -> Result<()> {
        self.db.execute("DELETE FROM history", [])?;
        Ok(())
    }

    // ---- settings ----

    pub fn settings(&self) -> Result<Settings> {
        let raw: Option<String> = self
            .db
            .query_row("SELECT value FROM settings WHERE key='settings'", [], |r| r.get(0))
            .optional()?;
        Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
    }

    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        let v = serde_json::to_string(s).map_err(|e| StoreError::Invalid(e.to_string()))?;
        self.db.execute(
            "INSERT INTO settings(key, value) VALUES ('settings', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![v],
        )?;
        Ok(())
    }
}

const NOTE_COLS: &str = "SELECT id, pack, path, page_title, quote, body, created_at, updated_at FROM notes";

fn bookmark_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Bookmark> {
    Ok(Bookmark { id: r.get(0)?, pack: r.get(1)?, path: r.get(2)?, title: r.get(3)?, created_at: r.get(4)? })
}

fn note_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: r.get(0)?,
        pack: r.get(1)?,
        path: r.get(2)?,
        page_title: r.get(3)?,
        quote: r.get(4)?,
        body: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmarks_are_unique_per_page() {
        let s = Store::in_memory().unwrap();
        let a = s.add_bookmark("p", "A.html", "A").unwrap();
        let again = s.add_bookmark("p", "A.html", "A renamed").unwrap();
        assert_eq!(a, again);
        s.add_bookmark("p", "B.html", "B").unwrap();
        assert_eq!(s.bookmarks().unwrap().len(), 2);
        assert_eq!(s.is_bookmarked("p", "A.html").unwrap(), Some(a.id));
        assert!(s.remove_bookmark(a.id).unwrap());
        assert_eq!(s.is_bookmarked("p", "A.html").unwrap(), None);
    }

    #[test]
    fn notes_round_trip_and_export() {
        let s = Store::in_memory().unwrap();
        assert!(s.add_note("p", "A.html", "A", "", "  ").is_err());
        let n = s.add_note("p", "A.html", "Alpha", "the quoted line", "my thought").unwrap();
        s.add_note("p", "A.html", "Alpha", "", "second").unwrap();
        s.add_note("q", "B.html", "Beta", "", "other pack").unwrap();
        assert_eq!(s.notes(Some(("p", "A.html"))).unwrap().len(), 2);
        assert_eq!(s.notes(None).unwrap().len(), 3);
        let u = s.update_note(n.id, "changed").unwrap().unwrap();
        assert_eq!(u.body, "changed");
        assert!(u.updated_at >= n.updated_at);
        let md = s.export_notes_markdown(|p| (p == "p").then(|| "Science".to_string())).unwrap();
        assert!(md.contains("## Alpha — Science"), "{md}");
        assert!(md.contains("> the quoted line\n\nchanged\n"), "{md}");
        assert!(md.contains("## Beta — q"), "{md}");
        assert!(s.remove_note(n.id).unwrap());
        assert_eq!(s.notes(None).unwrap().len(), 2);
    }

    #[test]
    fn history_keeps_one_row_per_page_newest_first() {
        let s = Store::in_memory().unwrap();
        s.record_visit("p", "A.html", "A").unwrap();
        s.record_visit("p", "B.html", "B").unwrap();
        s.record_visit("p", "A.html", "A").unwrap();
        let h = s.history(10).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].path, "A.html");
        s.clear_history().unwrap();
        assert!(s.history(10).unwrap().is_empty());
    }

    #[test]
    fn settings_default_and_persist() {
        let s = Store::in_memory().unwrap();
        assert_eq!(s.settings().unwrap(), Settings::default());
        assert_eq!(Settings::default().theme, ThemeChoice::Dark, "dark is the standard look");
        let mine = Settings { theme: ThemeChoice::Light, colorblind: true, library_dir: Some("/x".into()) };
        s.save_settings(&mine).unwrap();
        assert_eq!(s.settings().unwrap(), mine);
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("user.sqlite");
        drop(Store::open(&p).unwrap());
        let db = Connection::open(&p).unwrap();
        db.execute("UPDATE meta SET value='99' WHERE key='schema_version'", []).unwrap();
        drop(db);
        assert!(matches!(Store::open(&p), Err(StoreError::NewerSchema { found: 99 })));
    }

    #[test]
    fn install_secrets_are_made_once() {
        let s = Store::in_memory().unwrap();
        let a = s.install_secret("k", || "first".into()).unwrap();
        let b = s.install_secret("k", || "second".into()).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("first", "first"));
    }
}
