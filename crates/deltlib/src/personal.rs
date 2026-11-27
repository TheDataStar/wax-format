//! Each person's own reading data on the box: bookmarks, highlights with
//! notes, reading lists and history (deltd API §6.5).
//!
//! * **Private to the person** (decided 2026-11-15). Every call takes the
//!   person's `profile_id`, which the API takes from the live session and
//!   never from the request (deltd API §2). No call reads another person's
//!   rows, and nothing here takes a "whose" argument from outside: an id
//!   that belongs to someone else answers exactly like one that does not
//!   exist. Ids are random, so they say nothing about how many rows anyone
//!   else has.
//! * **One file**, `/var/lib/deltos/deltd/personal.sqlite` (contract §10,
//!   answered 2026-11-27 on the API doc's own recommendation), `deltd`'s
//!   alone. A backup carries it like the rest of the state tree
//!   (`docs/box-backup.md` §3); restore refuses a newer schema.
//! * **Schema-versioned, and a newer schema is refused** rather than opened
//!   (contract §10: rollback must never hand old code newly-migrated data).
//! * Timestamps are Unix epoch **milliseconds**, UTC (contract §5.2).
//!
//! A highlight's anchor is the pack, the page path, the passage (SPEC v0.13
//! §5.10: the page's path plus the passage's ordinal is its id) and the
//! quoted text. After an update the box re-finds the quote on the page; if
//! it cannot, the highlight is **flagged, never moved silently**
//! ([`Store::flag`], deltd API §6.5).

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// The schema this build writes and understands (`PRAGMA user_version`).
pub const PERSONAL_SCHEMA_VERSION: i64 = 1;

/// The file under `deltd`'s state directory (contract §10).
pub const PERSONAL_DB: &str = "personal.sqlite";

/// Pages kept in one person's history; older visits are dropped. The same
/// number the Delt desktop app keeps (`delt-desktop` `store::HISTORY_KEEP`).
pub const HISTORY_KEEP: i64 = 500;

/// Bounds on what one request may store. Not contract values: they keep a
/// single row within the API's 16 KiB request body
/// (`server::MAX_API_BODY`) and the box's work per row bounded.
pub const MAX_TITLE_CHARS: usize = 300;
pub const MAX_QUOTE_CHARS: usize = 2_000;
pub const MAX_NOTE_CHARS: usize = 10_000;
pub const MAX_LIST_ITEMS: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("personal data was saved by a newer deltd (schema {found}, this one understands {PERSONAL_SCHEMA_VERSION})")]
    NewerSchema { found: i64 },
    #[error("personal data: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn new_id() -> String {
    let mut b = [0u8; 8];
    getrandom::getrandom(&mut b).expect("the OS random source failed");
    crate::library::hex(&b)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bookmark {
    pub id: String,
    pub pack: String,
    pub path: String,
    pub title: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Highlight {
    pub id: String,
    pub pack: String,
    pub path: String,
    pub page_title: String,
    /// The passage's ordinal (SPEC v0.13 §5.10), or `None` for a page that
    /// carries no passages (the quote then anchors to the whole page).
    pub passage: Option<u32>,
    /// The passage heading's `id`, for linking as `#anchor`.
    pub anchor: Option<String>,
    pub quote: String,
    pub note: String,
    /// The pack's state token (its header hash) when the anchor was last
    /// checked; a different current token means the pack was updated since.
    #[serde(skip)]
    pub pack_state: String,
    /// The quote could not be found after an update.
    pub needs_attention: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListItem {
    pub pack: String,
    pub path: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadingList {
    pub id: String,
    pub name: String,
    pub items: Vec<ListItem>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Visit {
    pub pack: String,
    pub path: String,
    pub title: String,
    pub visited_at: i64,
}

/// A position in a newest-first list: `(time, id)`. Callers see it as an
/// opaque string ([`Cursor::parse`], [`Cursor::text`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub at: i64,
    pub id: String,
}

impl Cursor {
    pub fn parse(s: &str) -> Option<Cursor> {
        let (at, id) = s.split_once('.')?;
        Some(Cursor { at: at.parse().ok()?, id: id.to_string() })
    }
    pub fn text(&self) -> String {
        format!("{}.{}", self.at, self.id)
    }
}

/// Which highlights or bookmarks: all, one pack's, or one page's.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub pack: Option<String>,
    pub path: Option<String>,
}

/// What a new highlight carries; the API checks the quote is on the page.
pub struct NewHighlight<'a> {
    pub pack: &'a str,
    pub path: &'a str,
    pub page_title: &'a str,
    pub passage: Option<u32>,
    pub anchor: Option<&'a str>,
    pub quote: &'a str,
    pub note: &'a str,
    pub pack_state: &'a str,
}

pub struct Store {
    db: Mutex<Connection>,
}

fn check_len(what: &str, s: &str, max: usize) -> Result<()> {
    if s.chars().count() > max {
        return Err(StoreError::Invalid(format!("{what} is longer than {max} characters")));
    }
    Ok(())
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Store> {
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        let found: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if found > PERSONAL_SCHEMA_VERSION {
            return Err(StoreError::NewerSchema { found });
        }
        if found == 0 {
            db.execute_batch(
                "BEGIN;
                 CREATE TABLE bookmarks(
                     id TEXT PRIMARY KEY, profile_id TEXT NOT NULL, pack TEXT NOT NULL, path TEXT NOT NULL,
                     title TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE(profile_id, pack, path));
                 CREATE INDEX bookmarks_recent ON bookmarks(profile_id, created_at, id);
                 CREATE TABLE highlights(
                     id TEXT PRIMARY KEY, profile_id TEXT NOT NULL, pack TEXT NOT NULL, path TEXT NOT NULL,
                     page_title TEXT NOT NULL, passage INTEGER, anchor TEXT, quote TEXT NOT NULL,
                     note TEXT NOT NULL, pack_state TEXT NOT NULL, needs_attention INTEGER NOT NULL DEFAULT 0,
                     created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                 CREATE INDEX highlights_recent ON highlights(profile_id, created_at, id);
                 CREATE INDEX highlights_page ON highlights(profile_id, pack, path);
                 CREATE TABLE lists(
                     id TEXT PRIMARY KEY, profile_id TEXT NOT NULL, name TEXT NOT NULL,
                     created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                 CREATE INDEX lists_recent ON lists(profile_id, updated_at, id);
                 CREATE TABLE list_items(
                     list_id TEXT NOT NULL REFERENCES lists(id) ON DELETE CASCADE, position INTEGER NOT NULL,
                     pack TEXT NOT NULL, path TEXT NOT NULL, title TEXT NOT NULL, PRIMARY KEY(list_id, position));
                 CREATE TABLE history(
                     profile_id TEXT NOT NULL, pack TEXT NOT NULL, path TEXT NOT NULL, title TEXT NOT NULL,
                     visited_at INTEGER NOT NULL, PRIMARY KEY(profile_id, pack, path));
                 CREATE INDEX history_recent ON history(profile_id, visited_at);
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        Ok(Store { db: Mutex::new(db) })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---- bookmarks ----

    /// Bookmark a page; bookmarking it again returns the first one.
    pub fn add_bookmark(&self, profile: &str, pack: &str, path: &str, title: &str) -> Result<Bookmark> {
        check_len("title", title, MAX_TITLE_CHARS)?;
        let db = self.db();
        db.execute(
            "INSERT OR IGNORE INTO bookmarks(id, profile_id, pack, path, title, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![new_id(), profile, pack, path, title, now_ms()],
        )?;
        Ok(db.query_row(
            &format!("{BOOKMARK_COLS} WHERE profile_id=?1 AND pack=?2 AND path=?3"),
            params![profile, pack, path],
            bookmark_row,
        )?)
    }

    pub fn rename_bookmark(&self, profile: &str, id: &str, title: &str) -> Result<Option<Bookmark>> {
        check_len("title", title, MAX_TITLE_CHARS)?;
        let db = self.db();
        db.execute("UPDATE bookmarks SET title=?3 WHERE profile_id=?1 AND id=?2", params![profile, id, title])?;
        Ok(db
            .query_row(&format!("{BOOKMARK_COLS} WHERE profile_id=?1 AND id=?2"), params![profile, id], bookmark_row)
            .optional()?)
    }

    pub fn remove_bookmark(&self, profile: &str, id: &str) -> Result<bool> {
        Ok(self.db().execute("DELETE FROM bookmarks WHERE profile_id=?1 AND id=?2", params![profile, id])? > 0)
    }

    /// Newest first, `limit + 1` rows at most (the extra one says there is more).
    pub fn bookmarks(&self, profile: &str, f: &Filter, after: Option<&Cursor>, limit: usize) -> Result<Vec<Bookmark>> {
        let db = self.db();
        let (sql, args) = keyset(BOOKMARK_COLS, "created_at", profile, f, after, limit);
        let mut st = db.prepare(&sql)?;
        let rows = st
            .query_map(rusqlite::params_from_iter(args.iter()), bookmark_row)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    // ---- highlights ----

    pub fn add_highlight(&self, profile: &str, h: &NewHighlight<'_>) -> Result<Highlight> {
        if h.quote.trim().is_empty() {
            return Err(StoreError::Invalid("a highlight needs the quoted text".into()));
        }
        check_len("quote", h.quote, MAX_QUOTE_CHARS)?;
        check_len("note", h.note, MAX_NOTE_CHARS)?;
        check_len("page_title", h.page_title, MAX_TITLE_CHARS)?;
        let id = new_id();
        let t = now_ms();
        let db = self.db();
        db.execute(
            "INSERT INTO highlights(id, profile_id, pack, path, page_title, passage, anchor, quote, note,
                                    pack_state, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
            params![id, profile, h.pack, h.path, h.page_title, h.passage, h.anchor, h.quote, h.note, h.pack_state, t],
        )?;
        Ok(db.query_row(&format!("{HIGHLIGHT_COLS} WHERE id=?1"), params![id], highlight_row)?)
    }

    pub fn highlight(&self, profile: &str, id: &str) -> Result<Option<Highlight>> {
        Ok(self
            .db()
            .query_row(&format!("{HIGHLIGHT_COLS} WHERE profile_id=?1 AND id=?2"), params![profile, id], highlight_row)
            .optional()?)
    }

    pub fn set_note(&self, profile: &str, id: &str, note: &str) -> Result<Option<Highlight>> {
        check_len("note", note, MAX_NOTE_CHARS)?;
        self.db().execute(
            "UPDATE highlights SET note=?3, updated_at=?4 WHERE profile_id=?1 AND id=?2",
            params![profile, id, note, now_ms()],
        )?;
        self.highlight(profile, id)
    }

    pub fn remove_highlight(&self, profile: &str, id: &str) -> Result<bool> {
        Ok(self.db().execute("DELETE FROM highlights WHERE profile_id=?1 AND id=?2", params![profile, id])? > 0)
    }

    pub fn highlights(&self, profile: &str, f: &Filter, after: Option<&Cursor>, limit: usize) -> Result<Vec<Highlight>> {
        let db = self.db();
        let (sql, args) = keyset(HIGHLIGHT_COLS, "created_at", profile, f, after, limit);
        let mut st = db.prepare(&sql)?;
        let rows = st
            .query_map(rusqlite::params_from_iter(args.iter()), highlight_row)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// This person's highlights in `pack` last checked against a different
    /// edition than `current_state`.
    pub fn stale_highlights(&self, profile: &str, pack: &str, current_state: &str) -> Result<Vec<Highlight>> {
        let db = self.db();
        let mut st = db.prepare(&format!("{HIGHLIGHT_COLS} WHERE profile_id=?1 AND pack=?2 AND pack_state<>?3"))?;
        let rows = st
            .query_map(params![profile, pack, current_state], highlight_row)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// Packs this person has highlights in.
    pub fn highlighted_packs(&self, profile: &str) -> Result<Vec<String>> {
        let db = self.db();
        let mut st = db.prepare("SELECT DISTINCT pack FROM highlights WHERE profile_id=?1 ORDER BY pack")?;
        let rows = st.query_map(params![profile], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// The quote was found again in `state`'s edition, at this passage.
    pub fn reanchor(&self, id: &str, passage: Option<u32>, anchor: Option<&str>, state: &str) -> Result<()> {
        self.db().execute(
            "UPDATE highlights SET passage=?2, anchor=?3, pack_state=?4, needs_attention=0 WHERE id=?1",
            params![id, passage, anchor, state],
        )?;
        Ok(())
    }

    /// The quote is no longer on the page in `state`'s edition: keep the
    /// highlight where it was and flag it for the person.
    pub fn flag(&self, id: &str, state: &str) -> Result<()> {
        self.db()
            .execute("UPDATE highlights SET pack_state=?2, needs_attention=1 WHERE id=?1", params![id, state])?;
        Ok(())
    }

    pub fn needs_attention(&self, profile: &str) -> Result<Vec<Highlight>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "{HIGHLIGHT_COLS} WHERE profile_id=?1 AND needs_attention=1 ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = st.query_map(params![profile], highlight_row)?.collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    // ---- reading lists ----

    pub fn create_list(&self, profile: &str, name: &str, items: &[ListItem]) -> Result<ReadingList> {
        check_list(name, items)?;
        let id = new_id();
        let t = now_ms();
        let mut db = self.db();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO lists(id, profile_id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, profile, name, t],
        )?;
        put_items(&tx, &id, items)?;
        tx.commit()?;
        drop(db);
        Ok(self.list(profile, &id)?.expect("just made"))
    }

    pub fn list(&self, profile: &str, id: &str) -> Result<Option<ReadingList>> {
        let db = self.db();
        let Some(mut l) = db
            .query_row(&format!("{LIST_COLS} WHERE profile_id=?1 AND id=?2"), params![profile, id], list_row)
            .optional()?
        else {
            return Ok(None);
        };
        l.items = items_of(&db, &l.id)?;
        Ok(Some(l))
    }

    /// Rename, replace the items, or both. `None` leaves that part alone.
    pub fn update_list(&self, profile: &str, id: &str, name: Option<&str>, items: Option<&[ListItem]>) -> Result<Option<ReadingList>> {
        if let Some(n) = name {
            check_list(n, &[])?;
        }
        if let Some(i) = items {
            check_list("x", i)?;
        }
        let mut db = self.db();
        let tx = db.transaction()?;
        let n = tx.execute(
            "UPDATE lists SET name=COALESCE(?3, name), updated_at=?4 WHERE profile_id=?1 AND id=?2",
            params![profile, id, name, now_ms()],
        )?;
        if n == 0 {
            return Ok(None);
        }
        if let Some(i) = items {
            tx.execute("DELETE FROM list_items WHERE list_id=?1", params![id])?;
            put_items(&tx, id, i)?;
        }
        tx.commit()?;
        drop(db);
        self.list(profile, id)
    }

    pub fn remove_list(&self, profile: &str, id: &str) -> Result<bool> {
        Ok(self.db().execute("DELETE FROM lists WHERE profile_id=?1 AND id=?2", params![profile, id])? > 0)
    }

    /// Most recently changed first.
    pub fn lists(&self, profile: &str, after: Option<&Cursor>, limit: usize) -> Result<Vec<ReadingList>> {
        let db = self.db();
        let (sql, args) = keyset(LIST_COLS, "updated_at", profile, &Filter::default(), after, limit);
        let mut st = db.prepare(&sql)?;
        let mut rows: Vec<ReadingList> = st
            .query_map(rusqlite::params_from_iter(args.iter()), list_row)?
            .collect::<std::result::Result<_, _>>()?;
        for l in &mut rows {
            l.items = items_of(&db, &l.id)?;
        }
        Ok(rows)
    }

    // ---- history ----

    /// One row per page, moved to the top on each visit. Times only ever
    /// increase per person, so two visits in one millisecond keep their order
    /// and the time alone is a stable cursor.
    pub fn record_visit(&self, profile: &str, pack: &str, path: &str, title: &str) -> Result<()> {
        let title: String = title.chars().take(MAX_TITLE_CHARS).collect();
        let db = self.db();
        db.execute(
            "INSERT INTO history(profile_id, pack, path, title, visited_at)
             VALUES (?1, ?2, ?3, ?4,
                     MAX(?5, (SELECT COALESCE(MAX(visited_at), 0) + 1 FROM history WHERE profile_id=?1)))
             ON CONFLICT(profile_id, pack, path) DO UPDATE SET title=excluded.title, visited_at=excluded.visited_at",
            params![profile, pack, path, title, now_ms()],
        )?;
        db.execute(
            "DELETE FROM history WHERE profile_id=?1 AND visited_at <
               (SELECT visited_at FROM history WHERE profile_id=?1 ORDER BY visited_at DESC LIMIT 1 OFFSET ?2)",
            params![profile, HISTORY_KEEP - 1],
        )?;
        Ok(())
    }

    /// Newest first; `before` is the `visited_at` of the last row already seen.
    pub fn history(&self, profile: &str, before: Option<i64>, limit: usize) -> Result<Vec<Visit>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT pack, path, title, visited_at FROM history
             WHERE profile_id=?1 AND visited_at < ?2 ORDER BY visited_at DESC LIMIT ?3",
        )?;
        let rows = st
            .query_map(params![profile, before.unwrap_or(i64::MAX), (limit + 1) as i64], |r| {
                Ok(Visit { pack: r.get(0)?, path: r.get(1)?, title: r.get(2)?, visited_at: r.get(3)? })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    pub fn clear_history(&self, profile: &str) -> Result<u64> {
        Ok(self.db().execute("DELETE FROM history WHERE profile_id=?1", params![profile])? as u64)
    }

    // ---- export ----

    /// Everything this person keeps here, as one document they can take with
    /// them (deltd API §6.5 `/me/export`).
    pub fn export(&self, profile: &str) -> Result<serde_json::Value> {
        let all = usize::MAX / 2;
        let f = Filter::default();
        let bookmarks = self.bookmarks(profile, &f, None, all)?;
        let highlights = self.highlights(profile, &f, None, all)?;
        let lists = self.lists(profile, None, all)?;
        let history = self.history(profile, None, all)?;
        Ok(serde_json::json!({
            "format": "deltos-personal-export",
            "format_version": 1,
            "exported_at": now_ms(),
            "bookmarks": bookmarks,
            "highlights": highlights,
            "lists": lists,
            "history": history,
        }))
    }
}

fn check_list(name: &str, items: &[ListItem]) -> Result<()> {
    if name.trim().is_empty() {
        return Err(StoreError::Invalid("a list needs a name".into()));
    }
    check_len("name", name, MAX_TITLE_CHARS)?;
    if items.len() > MAX_LIST_ITEMS {
        return Err(StoreError::Invalid(format!("a list holds at most {MAX_LIST_ITEMS} pages")));
    }
    for i in items {
        check_len("title", &i.title, MAX_TITLE_CHARS)?;
    }
    Ok(())
}

fn put_items(tx: &rusqlite::Transaction<'_>, id: &str, items: &[ListItem]) -> Result<()> {
    let mut st = tx.prepare("INSERT INTO list_items(list_id, position, pack, path, title) VALUES (?1, ?2, ?3, ?4, ?5)")?;
    for (k, i) in items.iter().enumerate() {
        st.execute(params![id, k as i64, i.pack, i.path, i.title])?;
    }
    Ok(())
}

fn items_of(db: &Connection, id: &str) -> Result<Vec<ListItem>> {
    let mut st = db.prepare("SELECT pack, path, title FROM list_items WHERE list_id=?1 ORDER BY position")?;
    let rows = st
        .query_map(params![id], |r| Ok(ListItem { pack: r.get(0)?, path: r.get(1)?, title: r.get(2)? }))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

/// A newest-first page of `cols` for one person, after `after`.
fn keyset(cols: &str, time: &str, profile: &str, f: &Filter, after: Option<&Cursor>, limit: usize) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value as V;
    let mut sql = format!("{cols} WHERE profile_id=?1");
    let mut args = vec![V::Text(profile.to_string())];
    if let Some(p) = &f.pack {
        args.push(V::Text(p.clone()));
        sql.push_str(&format!(" AND pack=?{}", args.len()));
    }
    if let Some(p) = &f.path {
        args.push(V::Text(p.clone()));
        sql.push_str(&format!(" AND path=?{}", args.len()));
    }
    if let Some(c) = after {
        args.push(V::Integer(c.at));
        let a = args.len();
        args.push(V::Text(c.id.clone()));
        sql.push_str(&format!(" AND ({time} < ?{a} OR ({time} = ?{a} AND id < ?{}))", a + 1));
    }
    args.push(V::Integer(limit.saturating_add(1).min(i64::MAX as usize) as i64));
    sql.push_str(&format!(" ORDER BY {time} DESC, id DESC LIMIT ?{}", args.len()));
    (sql, args)
}

const BOOKMARK_COLS: &str = "SELECT id, pack, path, title, created_at FROM bookmarks";
const HIGHLIGHT_COLS: &str = "SELECT id, pack, path, page_title, passage, anchor, quote, note, pack_state, needs_attention, created_at, updated_at FROM highlights";
const LIST_COLS: &str = "SELECT id, name, created_at, updated_at FROM lists";

fn bookmark_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Bookmark> {
    Ok(Bookmark { id: r.get(0)?, pack: r.get(1)?, path: r.get(2)?, title: r.get(3)?, created_at: r.get(4)? })
}

fn highlight_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Highlight> {
    Ok(Highlight {
        id: r.get(0)?,
        pack: r.get(1)?,
        path: r.get(2)?,
        page_title: r.get(3)?,
        passage: r.get(4)?,
        anchor: r.get(5)?,
        quote: r.get(6)?,
        note: r.get(7)?,
        pack_state: r.get(8)?,
        needs_attention: r.get::<_, i64>(9)? != 0,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
    })
}

fn list_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReadingList> {
    Ok(ReadingList { id: r.get(0)?, name: r.get(1)?, items: vec![], created_at: r.get(2)?, updated_at: r.get(3)? })
}

/// Text compared the same way on both sides: visible text only, every run of
/// whitespace one space, trimmed.
pub fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where `quote` is on a page: the first passage whose visible text holds
/// it, preferring `hint` (the passage it was in before). `None` passages
/// means the page has no passage table: then the whole page is searched and
/// the answer's ordinal is `None`. The outer `None` means not found.
pub fn find_quote(content: &[u8], passages: Option<&[wax_core::passage::Passage]>, quote: &str, hint: Option<u32>) -> Option<(Option<u32>, Option<String>)> {
    let q = normalize(quote);
    if q.is_empty() {
        return None;
    }
    let text_of = |s: &[u8]| normalize(&wax_core::text::html_to_text(&String::from_utf8_lossy(s)));
    let Some(ps) = passages else {
        return text_of(content).contains(&q).then_some((None, None));
    };
    let holds = |p: &wax_core::passage::Passage| {
        let (s, e) = (p.start as usize, (p.end as usize).min(content.len()));
        s <= e && text_of(&content[s..e]).contains(&q)
    };
    if let Some(p) = hint.and_then(|h| ps.get(h as usize)).filter(|p| holds(p)) {
        return Some((Some(p.ordinal), p.anchor.clone()));
    }
    ps.iter().find(|p| holds(p)).map(|p| (Some(p.ordinal), p.anchor.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hl<'a>(quote: &'a str, state: &'a str) -> NewHighlight<'a> {
        NewHighlight {
            pack: "p",
            path: "A.html",
            page_title: "A",
            passage: Some(1),
            anchor: Some("x"),
            quote,
            note: "",
            pack_state: state,
        }
    }

    #[test]
    fn people_never_see_each_others_rows() {
        let s = Store::in_memory().unwrap();
        let b = s.add_bookmark("alice", "p", "A.html", "A").unwrap();
        let h = s.add_highlight("alice", &hl("quoted", "s1")).unwrap();
        let l = s.create_list("alice", "Mine", &[]).unwrap();
        s.record_visit("alice", "p", "A.html", "A").unwrap();
        let f = Filter::default();
        assert!(s.bookmarks("bob", &f, None, 10).unwrap().is_empty());
        assert!(s.highlights("bob", &f, None, 10).unwrap().is_empty());
        assert!(s.lists("bob", None, 10).unwrap().is_empty());
        assert!(s.history("bob", None, 10).unwrap().is_empty());
        // Bob cannot change or remove Alice's rows by id.
        assert_eq!(s.rename_bookmark("bob", &b.id, "x").unwrap(), None);
        assert!(!s.remove_bookmark("bob", &b.id).unwrap());
        assert_eq!(s.set_note("bob", &h.id, "x").unwrap(), None);
        assert!(!s.remove_highlight("bob", &h.id).unwrap());
        assert_eq!(s.update_list("bob", &l.id, Some("x"), None).unwrap(), None);
        assert!(!s.remove_list("bob", &l.id).unwrap());
        assert_eq!(s.clear_history("bob").unwrap(), 0);
        assert_eq!(s.bookmarks("alice", &f, None, 10).unwrap(), vec![b]);
        assert_eq!(s.history("alice", None, 10).unwrap().len(), 1);
    }

    #[test]
    fn bookmarks_are_unique_per_page_and_page_with_a_cursor() {
        let s = Store::in_memory().unwrap();
        let a = s.add_bookmark("al", "p", "A.html", "A").unwrap();
        assert_eq!(s.add_bookmark("al", "p", "A.html", "again").unwrap(), a);
        for i in 0..5 {
            s.add_bookmark("al", "p", &format!("{i}.html"), "x").unwrap();
        }
        let f = Filter::default();
        let mut seen = vec![];
        let mut after: Option<Cursor> = None;
        loop {
            let mut page = s.bookmarks("al", &f, after.as_ref(), 2).unwrap();
            let more = page.len() > 2;
            page.truncate(2);
            after = page.last().map(|b| Cursor { at: b.created_at, id: b.id.clone() });
            seen.extend(page.into_iter().map(|b| b.path));
            if !more {
                break;
            }
        }
        seen.sort();
        assert_eq!(seen, ["0.html", "1.html", "2.html", "3.html", "4.html", "A.html"]);
        let one = Filter { pack: Some("p".into()), path: Some("A.html".into()) };
        assert_eq!(s.bookmarks("al", &one, None, 10).unwrap().len(), 1);
    }

    #[test]
    fn highlights_are_flagged_not_moved() {
        let s = Store::in_memory().unwrap();
        assert!(s.add_highlight("al", &hl("  ", "s1")).is_err());
        let h = s.add_highlight("al", &hl("quoted", "s1")).unwrap();
        assert_eq!(s.stale_highlights("al", "p", "s1").unwrap(), vec![]);
        assert_eq!(s.stale_highlights("al", "p", "s2").unwrap().len(), 1);
        s.flag(&h.id, "s2").unwrap();
        let f = s.highlight("al", &h.id).unwrap().unwrap();
        assert!(f.needs_attention);
        assert_eq!((f.passage, f.anchor.as_deref()), (Some(1), Some("x")), "kept where it was");
        assert_eq!(s.needs_attention("al").unwrap().len(), 1);
        s.reanchor(&h.id, Some(3), None, "s3").unwrap();
        let r = s.highlight("al", &h.id).unwrap().unwrap();
        assert!(!r.needs_attention);
        assert_eq!(r.passage, Some(3));
        let n = s.set_note("al", &h.id, "my thought").unwrap().unwrap();
        assert_eq!(n.note, "my thought");
        assert!(s.set_note("al", &h.id, &"x".repeat(MAX_NOTE_CHARS + 1)).is_err());
    }

    #[test]
    fn lists_keep_their_order_and_replace_items() {
        let s = Store::in_memory().unwrap();
        let it = |p: &str| ListItem { pack: "p".into(), path: p.into(), title: p.into() };
        assert!(s.create_list("al", " ", &[]).is_err());
        let l = s.create_list("al", "Week 1", &[it("B"), it("A")]).unwrap();
        assert_eq!(l.items, vec![it("B"), it("A")]);
        let u = s.update_list("al", &l.id, None, Some(&[it("C")])).unwrap().unwrap();
        assert_eq!((u.name.as_str(), u.items.clone()), ("Week 1", vec![it("C")]));
        let r = s.update_list("al", &l.id, Some("Week 2"), None).unwrap().unwrap();
        assert_eq!((r.name.as_str(), r.items), ("Week 2", vec![it("C")]));
        let too_many: Vec<_> = (0..=MAX_LIST_ITEMS).map(|i| it(&i.to_string())).collect();
        assert!(s.create_list("al", "big", &too_many).is_err());
        assert!(s.remove_list("al", &l.id).unwrap());
        let db = s.db();
        let left: i64 = db.query_row("SELECT COUNT(*) FROM list_items", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "items go with their list");
    }

    #[test]
    fn history_is_one_row_per_page_newest_first_and_bounded() {
        let s = Store::in_memory().unwrap();
        for i in 0..(HISTORY_KEEP + 3) {
            s.record_visit("al", "p", &format!("{i}.html"), "x").unwrap();
        }
        s.record_visit("al", "p", "5.html", "x").unwrap();
        let h = s.history("al", None, 1000).unwrap();
        assert_eq!(h.len() as i64, HISTORY_KEEP);
        assert_eq!(h[0].path, "5.html");
        assert!(h.windows(2).all(|w| w[0].visited_at > w[1].visited_at));
        assert!(!h.iter().any(|v| v.path == "0.html"), "the oldest visits went");
        let next = s.history("al", Some(h[9].visited_at), 5).unwrap();
        assert_eq!(next[0].path, h[10].path);
        assert_eq!(s.clear_history("al").unwrap() as i64, HISTORY_KEEP);
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(PERSONAL_DB);
        drop(Store::open(&p).unwrap());
        Connection::open(&p).unwrap().execute_batch("PRAGMA user_version = 99").unwrap();
        assert!(matches!(Store::open(&p), Err(StoreError::NewerSchema { found: 99 })));
    }

    #[test]
    fn quotes_are_found_by_visible_text() {
        let page = b"<p>Intro text.</p><h2 id=\"a\">Alpha</h2><p>The   <b>quick</b>\nfox.</p><h2 id=\"b\">Beta</h2><p>The quick fox.</p>";
        let starts = wax_core::passage::html_heading_starts(page);
        let ps = wax_core::passage::describe(page, &starts);
        assert_eq!(find_quote(page, Some(&ps), "quick fox", None), Some((Some(1), Some("a".into()))));
        assert_eq!(find_quote(page, Some(&ps), "quick fox", Some(2)), Some((Some(2), Some("b".into()))), "the hint wins");
        assert_eq!(find_quote(page, Some(&ps), "slow fox", None), None);
        assert_eq!(find_quote(page, None, "Intro text", None), Some((None, None)));
    }
}
