//! The staging area every importer except ZIM writes into.
//!
//! An importer reads its source once and stages what the pack will hold. The
//! build then reads the staging area back **in path order**, which is what
//! makes a build deterministic whatever order the source yielded items in
//! (a filesystem walk, a zip directory, a crawl queue). Staging lives in a
//! temporary SQLite file, not in memory, so a large source does not grow the
//! heap (WSS spec: "memory stays bounded whatever the source size"; Track A
//! §18). Each item's bytes are read back one at a time.

use anyhow::{bail, Context, Result};
use rusqlite::{params, OptionalExtension};

/// How an entry is compressed. The build turns `Text` into `zstd-dict` when
/// the pack has a dictionary and plain `zstd` otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// Markup and other text: dictionary-eligible.
    Text,
    /// Compressible binary, no dictionary (TTF/OTF fonts, ADR-0003).
    Zstd,
    /// Stored verbatim: already entropy-coded media (ADR-0003), or a binary
    /// whose gain is unmeasured.
    None,
}

impl Codec {
    fn as_i64(self) -> i64 {
        match self {
            Codec::Text => 0,
            Codec::Zstd => 1,
            Codec::None => 2,
        }
    }
    fn from_i64(v: i64) -> Codec {
        match v {
            0 => Codec::Text,
            1 => Codec::Zstd,
            _ => Codec::None,
        }
    }
}

/// What the search index gets for an entry (SPEC §5.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Search {
    /// Not indexed.
    No,
    /// Index the entry's visible text, extracted from its HTML; cut passages
    /// at its headings (SPEC v0.13 §5.10).
    Html,
}

/// One staged item, as read back.
#[derive(Debug, Clone)]
pub struct Item {
    pub path: String,
    pub mime: Option<String>,
    pub title: Option<String>,
    pub body: Body,
    pub search: Search,
}

#[derive(Debug, Clone)]
pub enum Body {
    Data { bytes: Vec<u8>, codec: Codec },
    Redirect { to: String },
}

pub struct Staging {
    _tmp: tempfile::NamedTempFile,
    conn: rusqlite::Connection,
}

/// Rows read back per query while iterating.
const PAGE: i64 = 64;

impl Staging {
    pub fn create() -> Result<Self> {
        let tmp = tempfile::Builder::new().prefix("wss-stage").suffix(".sqlite").tempfile()?;
        let conn = rusqlite::Connection::open(tmp.path())?;
        conn.execute_batch(
            "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
             CREATE TABLE items (
                 path TEXT PRIMARY KEY NOT NULL,
                 mime TEXT, title TEXT,
                 redirect_to TEXT,
                 codec INTEGER NOT NULL DEFAULT 2,
                 search INTEGER NOT NULL DEFAULT 0,
                 bytes BLOB
             ) WITHOUT ROWID;",
        )?;
        Ok(Staging { _tmp: tmp, conn })
    }

    /// Stage content. Returns `false` (and stages nothing) when `path` is
    /// already staged: the first item at a path wins, and the importer
    /// reports the collision.
    pub fn add(
        &self,
        path: &str,
        mime: Option<&str>,
        title: Option<&str>,
        bytes: &[u8],
        codec: Codec,
        search: Search,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO items (path, mime, title, codec, search, bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![path, mime, title, codec.as_i64(), matches!(search, Search::Html) as i64, bytes],
        )?;
        Ok(n == 1)
    }

    pub fn add_redirect(&self, path: &str, to: &str, title: Option<&str>) -> Result<bool> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO items (path, title, redirect_to) VALUES (?1, ?2, ?3)",
            params![path, title, to],
        )?;
        Ok(n == 1)
    }

    /// Replace an already-staged item's bytes (an importer that rewrites links
    /// after it has seen every page).
    pub fn replace_bytes(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let n = self.conn.execute("UPDATE items SET bytes = ?2 WHERE path = ?1", params![path, bytes])?;
        if n != 1 {
            bail!("staging: no item at {path:?} to replace");
        }
        Ok(())
    }

    pub fn contains(&self, path: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM items WHERE path = ?1", [path], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn get(&self, path: &str) -> Result<Option<Item>> {
        Ok(self
            .conn
            .query_row(
                "SELECT path, mime, title, redirect_to, codec, search, bytes FROM items WHERE path = ?1",
                [path],
                row_to_item,
            )
            .optional()?)
    }

    pub fn len(&self) -> Result<u64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get::<_, i64>(0))? as u64)
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Paths of staged content (not redirects) whose mime is HTML, in path
    /// order. Used to pick an entry point.
    pub fn html_paths(&self, limit: usize) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(
            "SELECT path FROM items WHERE redirect_to IS NULL AND (mime = 'text/html' OR mime = 'application/xhtml+xml') ORDER BY path LIMIT ?1",
        )?;
        let rows = st.query_map([limit as i64], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Visit every item in path byte order, one at a time.
    pub fn for_each(&self, mut f: impl FnMut(Item) -> Result<()>) -> Result<()> {
        let mut after = String::new();
        let mut first = true;
        loop {
            let mut st = self.conn.prepare_cached(
                "SELECT path, mime, title, redirect_to, codec, search, bytes FROM items
                 WHERE ?1 OR path > ?2 ORDER BY path LIMIT ?3",
            )?;
            let page: Vec<Item> = st
                .query_map(params![first, after, PAGE], row_to_item)?
                .collect::<std::result::Result<_, _>>()
                .context("reading the staging area")?;
            if page.is_empty() {
                return Ok(());
            }
            first = false;
            after = page.last().unwrap().path.clone();
            for item in page {
                f(item)?;
            }
        }
    }

    /// Dictionary training samples (SPEC §5.9): `Text` items in path order,
    /// each truncated to `each`, at most `max` samples and `budget` bytes.
    /// Path order is content-derived, so two builds of the same source train
    /// the same dictionary.
    pub fn dict_samples(&self, max: usize, budget: usize, each: usize) -> Result<Vec<Vec<u8>>> {
        let mut st = self.conn.prepare(
            "SELECT substr(bytes, 1, ?1) FROM items WHERE redirect_to IS NULL AND codec = 0 AND length(bytes) > 0 ORDER BY path",
        )?;
        let mut rows = st.query([each as i64])?;
        let mut out = Vec::new();
        let mut used = 0usize;
        while let Some(r) = rows.next()? {
            let b: Vec<u8> = r.get(0)?;
            if out.len() >= max || used + b.len() > budget {
                break;
            }
            used += b.len();
            out.push(b);
        }
        Ok(out)
    }
}

fn row_to_item(r: &rusqlite::Row) -> rusqlite::Result<Item> {
    let redirect: Option<String> = r.get(3)?;
    let body = match redirect {
        Some(to) => Body::Redirect { to },
        None => Body::Data {
            bytes: r.get::<_, Option<Vec<u8>>>(6)?.unwrap_or_default(),
            codec: Codec::from_i64(r.get(4)?),
        },
    };
    Ok(Item {
        path: r.get(0)?,
        mime: r.get(1)?,
        title: r.get(2)?,
        body,
        search: if r.get::<_, i64>(5)? == 1 { Search::Html } else { Search::No },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterates_in_path_order_and_first_wins() {
        let s = Staging::create().unwrap();
        for p in ["b.html", "a.html", "c/d.css"] {
            assert!(s.add(p, Some("text/html"), None, p.as_bytes(), Codec::Text, Search::Html).unwrap());
        }
        assert!(!s.add("a.html", None, None, b"other", Codec::None, Search::No).unwrap());
        assert!(s.add_redirect("x.html", "a.html", None).unwrap());
        let mut seen = vec![];
        s.for_each(|i| {
            seen.push(i.path);
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, ["a.html", "b.html", "c/d.css", "x.html"]);
        match s.get("a.html").unwrap().unwrap().body {
            Body::Data { bytes, .. } => assert_eq!(bytes, b"a.html"),
            _ => panic!(),
        }
        assert_eq!(s.dict_samples(2, 1 << 20, 3).unwrap(), vec![b"a.h".to_vec(), b"b.h".to_vec()]);
    }

    #[test]
    fn pages_through_more_than_one_page() {
        let s = Staging::create().unwrap();
        for i in 0..(PAGE * 2 + 5) {
            s.add(&format!("p{i:04}"), None, None, b"x", Codec::None, Search::No).unwrap();
        }
        let mut n = 0;
        s.for_each(|_| {
            n += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(n, PAGE * 2 + 5);
    }
}
