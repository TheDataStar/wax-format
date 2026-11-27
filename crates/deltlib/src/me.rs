//! The Personal group (deltd API §6.5) and the page reads behind Reading
//! (§6.4): `/me/bookmarks`, `/me/highlights`, `/me/lists`, `/me/history`,
//! `/me/needs-attention`, `/me/export`, and `/packs/{uuid}/outline` and
//! `/reader`.
//!
//! Every `/me` endpoint needs a live session that is not Guest (Guest stores
//! nothing, contract §5.1), takes the person from that session only, and
//! answers `Cache-Control: no-store` (rule 10). Every write is checked like
//! every other write: exact `Origin` and the session's anti-forgery token
//! (rule 6). None takes a person argument, so no permission, `admin`
//! included, reads another person's data through it.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode};
use serde_json::{json, Value};
use wax_core::passage::Passage;
use wax_core::WaxError;

use crate::api::{invalid, limit_of, not_signed_in, page_of, session_token, Api, Problem};
use crate::identity::Session;
use crate::library::{Pack, Snapshot};
use crate::personal::{self, Cursor, Filter, ListItem, NewHighlight, Store, StoreError};
use crate::reading::{self, MAX_READABLE_BYTES};
use crate::signin::{body_json, private_json, write_allowed, ConnInfo};

fn guest_not_allowed() -> Problem {
    Problem::new(StatusCode::FORBIDDEN, "guest_not_allowed", "Guest cannot keep anything on the box.")
}
fn item_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "item_not_found", "No such item.")
}
fn internal() -> Problem {
    Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong.")
}
fn store_problem(e: StoreError) -> Problem {
    match e {
        StoreError::Invalid(d) => invalid(d),
        e => internal().detail(e.to_string()),
    }
}

/// One page of a pack, read for a highlight check or reader mode.
pub(crate) struct PageData {
    /// The page's own path, after a redirect.
    pub path: String,
    pub title: String,
    pub content: Vec<u8>,
    /// `None` when the page carries no passages (packs before v0.13).
    pub passages: Option<Vec<Passage>>,
}

/// Read `path` from `p`: a page (HTML) only, at most [`MAX_READABLE_BYTES`].
/// `content: false` checks it exists and reads its title only.
pub(crate) async fn load_page(p: Arc<Pack>, path: String, content: bool) -> Result<PageData, Problem> {
    tokio::task::spawn_blocking(move || {
        p.readers.with(|r| {
            let to_problem = |e: WaxError| match e {
                WaxError::EntryNotFound(_) => Problem::new(StatusCode::NOT_FOUND, "page_not_found", "No page at that path."),
                WaxError::EntryDeleted(_) => Problem::new(StatusCode::GONE, "page_removed", "This page was removed in an update."),
                e => Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "pack_damaged", "This pack could not be read.").detail(e.to_string()),
            };
            let e = r.resolve(&path).map_err(to_problem)?.entry;
            if !reading::is_page(e.mime.as_deref()) {
                return Err(invalid(format!("{} is not a page", e.path)));
            }
            let title = e.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| e.path.clone());
            if !content {
                return Ok(PageData { path: e.path, title, content: vec![], passages: None });
            }
            if e.uncompressed_length > MAX_READABLE_BYTES {
                return Err(invalid(format!("{} is too large to read this way", e.path)));
            }
            let bytes = r.read(&e.path).map_err(to_problem)?;
            // A page without passages, or with a rule this build does not
            // know, is one section; entries and search are unaffected.
            let passages = r.passages(&e.path).ok();
            Ok(PageData { path: e.path, title, content: bytes, passages })
        })
    })
    .await
    .map_err(|_| internal())?
}

fn field<'a>(b: &'a Value, k: &str) -> Result<&'a str, Problem> {
    b.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| invalid(format!("{k} is required")))
}

fn opt_field<'a>(b: &'a Value, k: &str) -> Result<Option<&'a str>, Problem> {
    match b.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(invalid(format!("{k} must be text"))),
    }
}

fn cursor_of(q: &BTreeMap<String, String>) -> Result<Option<Cursor>, Problem> {
    match q.get("cursor") {
        None => Ok(None),
        Some(c) => Cursor::parse(c).map(Some).ok_or_else(|| invalid("cursor is not one this server issued")),
    }
}

fn filter_of(q: &BTreeMap<String, String>) -> Filter {
    Filter { pack: q.get("pack").cloned(), path: q.get("path").cloned() }
}

fn keyed(items: Vec<Value>, limit: usize, time: &'static str) -> Value {
    page_of(items, limit, move |v| {
        Cursor { at: v[time].as_i64().unwrap_or(0), id: v["id"].as_str().unwrap_or("").to_string() }.text()
    })
}

fn to_values<T: serde::Serialize>(rows: Vec<T>) -> Vec<Value> {
    rows.into_iter().map(|r| serde_json::to_value(r).unwrap()).collect()
}

impl Api {
    /// Answer a `/me` endpoint of the Personal group, or `None` if the path
    /// is not one.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn me(
        &self,
        method: &Method,
        segs: &[&str],
        q: &BTreeMap<String, String>,
        headers: &HeaderMap,
        body: &[u8],
        conn: &ConnInfo,
    ) -> Option<Response<Body>> {
        let known = matches!(
            segs,
            ["me", "bookmarks"]
                | ["me", "bookmarks", _]
                | ["me", "highlights"]
                | ["me", "highlights", _]
                | ["me", "lists"]
                | ["me", "lists", _]
                | ["me", "history"]
                | ["me", "needs-attention"]
                | ["me", "export"]
        );
        if !known {
            return None;
        }
        Some(match self.me_inner(method, segs, q, headers, body, conn).await {
            Ok(r) => r,
            Err(p) => p.response(),
        })
    }

    async fn me_inner(
        &self,
        method: &Method,
        segs: &[&str],
        q: &BTreeMap<String, String>,
        headers: &HeaderMap,
        body: &[u8],
        conn: &ConnInfo,
    ) -> Result<Response<Body>, Problem> {
        let session = session_token(headers).and_then(|t| self.ident.session_by_token(&t)).ok_or_else(not_signed_in)?;
        let write = !matches!(*method, Method::GET | Method::HEAD);
        if write {
            write_allowed(headers, conn, &self.webos_host, Some(&session))?;
        }
        if session.guest {
            return Err(guest_not_allowed());
        }
        let store = self
            .personal
            .clone()
            .ok_or_else(|| internal().detail("this server keeps no personal data"))?;
        let me = session.profile_id.as_str();
        let snap = self.lib.snapshot();
        let ok = |v: Value| private_json(StatusCode::OK, &v, conn, vec![]);
        let created = |v: Value| private_json(StatusCode::CREATED, &v, conn, vec![]);
        let b = if write { body_json(headers, body)? } else { json!({}) };
        let sp = store_problem;

        match (method.clone(), segs) {
            // ---- bookmarks ----
            (Method::GET | Method::HEAD, ["me", "bookmarks"]) => {
                let limit = limit_of(q)?;
                let rows = store.bookmarks(me, &filter_of(q), cursor_of(q)?.as_ref(), limit).map_err(sp)?;
                Ok(ok(keyed(to_values(rows), limit, "created_at")))
            }
            (Method::POST, ["me", "bookmarks"]) => {
                let p = Arc::clone(self.find(&snap, &session, field(&b, "pack")?)?);
                let page = load_page(Arc::clone(&p), field(&b, "path")?.to_string(), false).await?;
                let title = opt_field(&b, "title")?.unwrap_or(&page.title);
                let bm = store.add_bookmark(me, &p.archive_uuid, &page.path, title).map_err(sp)?;
                Ok(created(json!(bm)))
            }
            (Method::PATCH, ["me", "bookmarks", id]) => {
                let bm = store.rename_bookmark(me, id, field(&b, "title")?).map_err(sp)?.ok_or_else(item_not_found)?;
                Ok(ok(json!(bm)))
            }
            (Method::DELETE, ["me", "bookmarks", id]) => {
                if !store.remove_bookmark(me, id).map_err(sp)? {
                    return Err(item_not_found());
                }
                Ok(ok(json!({ "removed": true })))
            }

            // ---- highlights ----
            (Method::GET | Method::HEAD, ["me", "highlights"]) => {
                let f = filter_of(q);
                let packs = match &f.pack {
                    Some(p) => vec![p.clone()],
                    None => store.highlighted_packs(me).map_err(sp)?,
                };
                self.reanchor(&store, &snap, me, packs).await?;
                let limit = limit_of(q)?;
                let rows = store.highlights(me, &f, cursor_of(q)?.as_ref(), limit).map_err(sp)?;
                Ok(ok(keyed(to_values(rows), limit, "created_at")))
            }
            (Method::POST, ["me", "highlights"]) => {
                let p = Arc::clone(self.find(&snap, &session, field(&b, "pack")?)?);
                let quote = field(&b, "quote")?;
                let note = opt_field(&b, "note")?.unwrap_or("");
                let hint = match b.get("passage") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| invalid("passage must be a section number"))?),
                };
                let page = load_page(Arc::clone(&p), field(&b, "path")?.to_string(), true).await?;
                let (passage, anchor) = personal::find_quote(&page.content, page.passages.as_deref(), quote, hint)
                    .ok_or_else(|| invalid("the quote is not on this page"))?;
                let h = store
                    .add_highlight(
                        me,
                        &NewHighlight {
                            pack: &p.archive_uuid,
                            path: &page.path,
                            page_title: &page.title,
                            passage,
                            anchor: anchor.as_deref(),
                            quote,
                            note,
                            pack_state: &p.state_token,
                        },
                    )
                    .map_err(sp)?;
                Ok(created(json!(h)))
            }
            (Method::PATCH, ["me", "highlights", id]) => {
                let note = opt_field(&b, "note")?.ok_or_else(|| invalid("note is required"))?;
                let h = store.set_note(me, id, note).map_err(sp)?.ok_or_else(item_not_found)?;
                Ok(ok(json!(h)))
            }
            (Method::DELETE, ["me", "highlights", id]) => {
                if !store.remove_highlight(me, id).map_err(sp)? {
                    return Err(item_not_found());
                }
                Ok(ok(json!({ "removed": true })))
            }
            (Method::GET | Method::HEAD, ["me", "needs-attention"]) => {
                let packs = store.highlighted_packs(me).map_err(sp)?;
                self.reanchor(&store, &snap, me, packs).await?;
                let rows = store.needs_attention(me).map_err(sp)?;
                Ok(ok(json!({ "items": to_values(rows), "next": null })))
            }

            // ---- reading lists ----
            (Method::GET | Method::HEAD, ["me", "lists"]) => {
                let limit = limit_of(q)?;
                let rows = store.lists(me, cursor_of(q)?.as_ref(), limit).map_err(sp)?;
                Ok(ok(keyed(to_values(rows), limit, "updated_at")))
            }
            (Method::GET | Method::HEAD, ["me", "lists", id]) => {
                let l = store.list(me, id).map_err(sp)?.ok_or_else(item_not_found)?;
                Ok(ok(json!(l)))
            }
            (Method::POST, ["me", "lists"]) => {
                let items = self.list_items(&snap, &session, b.get("items")).await?.unwrap_or_default();
                let l = store.create_list(me, field(&b, "name")?, &items).map_err(sp)?;
                Ok(created(json!(l)))
            }
            (Method::PATCH, ["me", "lists", id]) => {
                let items = self.list_items(&snap, &session, b.get("items")).await?;
                let l = store
                    .update_list(me, id, opt_field(&b, "name")?, items.as_deref())
                    .map_err(sp)?
                    .ok_or_else(item_not_found)?;
                Ok(ok(json!(l)))
            }
            (Method::DELETE, ["me", "lists", id]) => {
                if !store.remove_list(me, id).map_err(sp)? {
                    return Err(item_not_found());
                }
                Ok(ok(json!({ "removed": true })))
            }

            // ---- history ----
            (Method::GET | Method::HEAD, ["me", "history"]) => {
                let limit = limit_of(q)?;
                let before = match q.get("cursor") {
                    None => None,
                    Some(c) => Some(c.parse::<i64>().map_err(|_| invalid("cursor is not one this server issued"))?),
                };
                let rows = store.history(me, before, limit).map_err(sp)?;
                Ok(ok(page_of(to_values(rows), limit, |v| v["visited_at"].as_i64().unwrap_or(0).to_string())))
            }
            (Method::DELETE, ["me", "history"]) => {
                let n = store.clear_history(me).map_err(sp)?;
                Ok(ok(json!({ "cleared": n })))
            }

            // ---- export ----
            (Method::GET | Method::HEAD, ["me", "export"]) => {
                let mut v = store.export(me).map_err(sp)?;
                v["pack_names"] = json!(snap
                    .by_uuid
                    .values()
                    .map(|p| (p.archive_uuid.clone(), p.name().to_string()))
                    .collect::<BTreeMap<_, _>>());
                let mut r = ok(v);
                r.headers_mut().insert(
                    header::CONTENT_DISPOSITION,
                    HeaderValue::from_static("attachment; filename=\"my-reading-data.json\""),
                );
                Ok(r)
            }

            _ => {
                let mut r = Problem::new(StatusCode::METHOD_NOT_ALLOWED, "invalid_request", "Method not allowed.").response();
                let allow = match segs {
                    ["me", "bookmarks" | "highlights" | "lists"] => "GET, HEAD, POST",
                    ["me", "lists", _] => "GET, HEAD, PATCH, DELETE",
                    ["me", _, _] => "PATCH, DELETE",
                    ["me", "history"] => "GET, HEAD, DELETE",
                    _ => "GET, HEAD",
                };
                r.headers_mut().insert(header::ALLOW, HeaderValue::from_static(allow));
                Ok(r)
            }
        }
    }

    /// A list's items from a request: each `{pack, path}` must be a page the
    /// person can open now; its title is the page's own.
    async fn list_items(&self, snap: &Snapshot, s: &Session, v: Option<&Value>) -> Result<Option<Vec<ListItem>>, Problem> {
        let arr = match v {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::Array(a)) => a,
            Some(_) => return Err(invalid("items must be a list")),
        };
        if arr.len() > personal::MAX_LIST_ITEMS {
            return Err(invalid(format!("a list holds at most {} pages", personal::MAX_LIST_ITEMS)));
        }
        let mut out = Vec::with_capacity(arr.len());
        for it in arr {
            let p = Arc::clone(self.find(snap, s, field(it, "pack")?)?);
            let page = load_page(Arc::clone(&p), field(it, "path")?.to_string(), false).await?;
            out.push(ListItem { pack: p.archive_uuid.clone(), path: page.path, title: page.title });
        }
        Ok(Some(out))
    }

    /// Re-find this person's highlights in packs updated since they were
    /// last checked (deltd API §6.5): found again, the anchor follows the
    /// quote; not found, or the page is gone, the highlight is flagged and
    /// left where it was. A pack no longer installed is left alone, so its
    /// highlights come back as they were if it is installed again.
    async fn reanchor(&self, store: &Arc<Store>, snap: &Snapshot, me: &str, packs: Vec<String>) -> Result<(), Problem> {
        for id in packs {
            let Some(p) = snap.by_uuid.get(&id) else { continue };
            let stale = store.stale_highlights(me, &id, &p.state_token).map_err(store_problem)?;
            let mut by_path: BTreeMap<String, Vec<personal::Highlight>> = BTreeMap::new();
            for h in stale {
                by_path.entry(h.path.clone()).or_default().push(h);
            }
            for (path, hs) in by_path {
                match load_page(Arc::clone(p), path, true).await {
                    Ok(page) => {
                        for h in hs {
                            match personal::find_quote(&page.content, page.passages.as_deref(), &h.quote, h.passage) {
                                Some((o, a)) => store.reanchor(&h.id, o, a.as_deref(), &p.state_token),
                                None => store.flag(&h.id, &p.state_token),
                            }
                            .map_err(store_problem)?;
                        }
                    }
                    Err(e) if matches!(e.code(), "page_not_found" | "page_removed" | "invalid_request") => {
                        for h in hs {
                            store.flag(&h.id, &p.state_token).map_err(store_problem)?;
                        }
                    }
                    // a damaged pack: leave the anchors as they are and try again next time
                    Err(_) => {}
                }
            }
        }
        Ok(())
    }

    /// `GET /packs/{uuid}/outline?path=`: the page's sections.
    pub(crate) async fn outline(&self, snap: &Snapshot, s: &Session, id: &str, q: &BTreeMap<String, String>) -> Result<Value, Problem> {
        let p = Arc::clone(self.find(snap, s, id)?);
        let path = q.get("path").filter(|v| !v.is_empty()).ok_or_else(|| invalid("path is required"))?;
        let page = load_page(Arc::clone(&p), path.clone(), true).await?;
        Ok(json!({
            "pack": p.archive_uuid,
            "path": page.path,
            "title": page.title,
            "has_passages": page.passages.is_some(),
            "sections": reading::outline(&page.content, page.passages.as_deref()),
        }))
    }

    /// `GET /packs/{uuid}/reader?path=`: the page's text and images without
    /// its layout.
    pub(crate) async fn reader(&self, snap: &Snapshot, s: &Session, id: &str, q: &BTreeMap<String, String>) -> Result<Value, Problem> {
        let p = Arc::clone(self.find(snap, s, id)?);
        let path = q.get("path").filter(|v| !v.is_empty()).ok_or_else(|| invalid("path is required"))?;
        let page = load_page(Arc::clone(&p), path.clone(), true).await?;
        Ok(json!({
            "pack": p.archive_uuid,
            "path": page.path,
            "title": page.title,
            "languages": p.languages(),
            "has_passages": page.passages.is_some(),
            "sections": reading::readable(&page.content, page.passages.as_deref(), &page.path),
        }))
    }
}
