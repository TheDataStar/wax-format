//! One embedded SQLite index segment (SPEC §4).
//!
//! A segment's bytes are a raw SQLite database sitting at an offset inside the
//! archive. This module opens that byte range in place through the [`crate::vfs`]
//! window — nothing is copied — and answers queries against `entries`,
//! `segment_meta` and `manifest` one row (or one page of rows) at a time, so
//! a segment costs its SQLite page cache and nothing proportional to its row
//! count. Every SQLite / schema problem becomes a [`WaxError`]; nothing panics
//! (A4 fuzz target 2, SPEC §11.3).

use crate::model::Entry;
use crate::{Result, WaxError};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::path::Path;

/// The constant stored at `segment_meta.format` (SPEC §4.2).
pub const SEGMENT_FORMAT_TAG: &str = "wax-index-segment";

/// Per-segment SQLite page cache, in KiB. The one place a segment's memory
/// lives: a 1 MiB cache holds the whole interior of a Wikipedia-scale
/// `entries` B-tree plus the leaves recently touched, and at the SPEC §5.1
/// cap of 64 segments the worst case is 64 MiB.
pub const PAGE_CACHE_KIB: u32 = 1024;

/// Parsed `segment_meta` (SPEC §4.2).
#[derive(Debug, Clone)]
pub struct SegmentMeta {
    pub segment_index: u64,
    pub blob_region_offset: u64,
    pub blob_region_length: u64,
    pub created_at: i64,
    /// `(offset, length)` of the previous segment's database. `None` ⇒ base.
    pub prev_segment: Option<(u64, u64)>,
}

/// An opened index segment: a read-only connection onto a window of the
/// archive file.
pub struct Segment {
    /// Absolute file offset this segment's database starts at.
    pub disk_offset: u64,
    /// Length in bytes of this segment's SQLite database on disk.
    pub db_len: u64,
    pub meta: SegmentMeta,
    conn: Connection,
    /// The `SELECT` column list, with absent optional columns substituted
    /// (SPEC §5.2 column tolerance).
    columns: String,
    has_volume: bool,
}

impl Segment {
    /// Open the database occupying `[disk_offset, disk_offset + db_len)` of
    /// `archive`. The caller has already bounds-checked the range against the
    /// header and file size (SPEC §2.2, §5.1).
    pub fn open_at(archive: &Path, disk_offset: u64, db_len: u64) -> Result<Self> {
        if !crate::vfs::ensure_registered() {
            return Err(WaxError::Sqlite(rusqlite::Error::InvalidQuery));
        }
        let uri = crate::vfs::segment_uri(archive, disk_offset, db_len).ok_or_else(|| {
            WaxError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "archive path is not valid UTF-8",
            ))
        })?;
        let conn = Connection::open_with_flags(
            &uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| WaxError::NotAnIndexSegment {
            offset: disk_offset,
            reason: format!("cannot open as SQLite: {e}"),
        })?;
        conn.execute_batch(&format!("PRAGMA cache_size=-{PAGE_CACHE_KIB}; PRAGMA query_only=1;"))
            .map_err(|e| WaxError::NotAnIndexSegment {
                offset: disk_offset,
                reason: format!("not a SQLite database: {e}"),
            })?;

        // Touching the schema forces SQLite to actually read the header/pages;
        // a truncated or corrupt db fails here rather than later.
        let table_present = |name: &str| -> Result<bool> {
            Ok(conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                    [name],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|e| WaxError::NotAnIndexSegment {
                    offset: disk_offset,
                    reason: format!("cannot read schema: {e}"),
                })?
                .is_some())
        };

        if !table_present("segment_meta")? {
            return Err(WaxError::NotAnIndexSegment {
                offset: disk_offset,
                reason: "no segment_meta table".into(),
            });
        }
        if !table_present("entries")? {
            return Err(WaxError::Schema {
                detail: format!("segment at {disk_offset} has no `entries` table"),
            });
        }

        let meta = Self::read_meta(disk_offset, &conn)?;

        // Column tolerance for unknown/older minor versions (SPEC §5.2, §5.4).
        let cols = Self::entry_columns(&conn)?;
        for required in ["path", "offset", "length", "uncompressed_length", "compression"] {
            if !cols.iter().any(|c| c == required) {
                return Err(WaxError::Schema {
                    detail: format!("`entries` missing required column `{required}`"),
                });
            }
        }
        let has = |name: &str| cols.iter().any(|c| c == name);
        let columns = format!(
            "path, {title} AS title, offset, length, uncompressed_length, mime, compression, \
             sha256, {vol} AS volume_id, {redir} AS redirect_to",
            title = if has("title") { "title" } else { "NULL" },
            vol = if has("volume_id") { "COALESCE(volume_id, 0)" } else { "0" },
            redir = if has("redirect_to") { "redirect_to" } else { "NULL" },
        );

        Ok(Segment {
            disk_offset,
            db_len,
            meta,
            conn,
            columns,
            has_volume: has("volume_id"),
        })
    }

    fn entry_columns(conn: &Connection) -> Result<Vec<String>> {
        let mut stmt = conn.prepare("PRAGMA table_info(entries)")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn read_meta(disk_offset: u64, conn: &Connection) -> Result<SegmentMeta> {
        let get = |key: &str| -> Result<Option<String>> {
            Ok(conn
                .query_row(
                    "SELECT value FROM segment_meta WHERE key=?1",
                    [key],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten())
        };
        let need = |key: &str| -> Result<String> {
            get(key)?.ok_or_else(|| WaxError::NotAnIndexSegment {
                offset: disk_offset,
                reason: format!("segment_meta missing `{key}`"),
            })
        };
        let parse_u64 = |key: &str, s: &str| -> Result<u64> {
            s.trim().parse::<u64>().map_err(|_| WaxError::NotAnIndexSegment {
                offset: disk_offset,
                reason: format!("segment_meta.{key} is not a u64: {s:?}"),
            })
        };

        let tag = need("format")?;
        if tag != SEGMENT_FORMAT_TAG {
            return Err(WaxError::NotAnIndexSegment {
                offset: disk_offset,
                reason: format!("segment_meta.format = {tag:?}"),
            });
        }

        let segment_index = parse_u64("segment_index", &need("segment_index")?)?;
        let blob_region_offset = parse_u64("blob_region_offset", &need("blob_region_offset")?)?;
        let blob_region_length = parse_u64("blob_region_length", &need("blob_region_length")?)?;
        let created_at = need("created_at")?
            .trim()
            .parse::<i64>()
            .unwrap_or(0);

        let prev_off = get("prev_segment_offset")?;
        let prev_len = get("prev_segment_length")?;
        let prev_segment = match (prev_off, prev_len) {
            (Some(o), Some(l)) => Some((
                parse_u64("prev_segment_offset", &o)?,
                parse_u64("prev_segment_length", &l)?,
            )),
            (None, None) => None,
            _ => {
                return Err(WaxError::BrokenSegmentChain {
                    detail: format!(
                        "segment at {disk_offset} has only one of prev_segment_offset/length"
                    ),
                })
            }
        };

        Ok(SegmentMeta {
            segment_index,
            blob_region_offset,
            blob_region_length,
            created_at,
            prev_segment,
        })
    }

    fn row_to_raw(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
        Ok(RawRow {
            path: r.get("path")?,
            title: r.get("title")?,
            offset: r.get::<_, i64>("offset")?,
            length: r.get::<_, i64>("length")?,
            uncompressed_length: r.get::<_, i64>("uncompressed_length")?,
            mime: r.get("mime")?,
            compression: r.get("compression")?,
            sha256: r.get("sha256")?,
            volume_id: r.get::<_, i64>("volume_id")?,
            redirect_to: r.get("redirect_to")?,
        })
    }

    /// Point lookup of one path in this segment (an index probe; SPEC §5.2
    /// `path` is the primary key). `COLLATE BINARY` pins byte-order
    /// comparison even if a crafted segment declared another collation.
    pub fn lookup(&self, path: &str) -> Result<Option<Entry>> {
        let sql = format!(
            "SELECT {} FROM entries WHERE path = ?1 COLLATE BINARY",
            self.columns
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let raw = stmt.query_row([path], Self::row_to_raw).optional()?;
        raw.map(|r| r.into_entry(self.disk_offset)).transpose()
    }

    /// The next `limit` entries in ascending byte order of `path`, strictly
    /// after `after` (`None` ⇒ from the start). Keyset pagination: each call
    /// is one index range scan and holds `limit` rows, however many the
    /// segment has (SPEC §5.5 ordering).
    pub fn page(&self, after: Option<&str>, limit: usize) -> Result<Vec<Entry>> {
        let sql = match after {
            None => format!(
                "SELECT {} FROM entries ORDER BY path COLLATE BINARY LIMIT ?1",
                self.columns
            ),
            Some(_) => format!(
                "SELECT {} FROM entries WHERE path COLLATE BINARY > ?2 \
                 ORDER BY path COLLATE BINARY LIMIT ?1",
                self.columns
            ),
        };
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let limit = limit as i64;
        let mut out = Vec::with_capacity(limit as usize);
        let mut push = |r: rusqlite::Result<RawRow>| -> Result<()> {
            out.push(r?.into_entry(self.disk_offset)?);
            Ok(())
        };
        match after {
            None => {
                let rows = stmt.query_map([limit], Self::row_to_raw)?;
                for r in rows {
                    push(r)?;
                }
            }
            Some(a) => {
                let rows = stmt.query_map(rusqlite::params![limit, a], Self::row_to_raw)?;
                for r in rows {
                    push(r)?;
                }
            }
        }
        Ok(out)
    }

    /// Number of rows in `entries`.
    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Some entry whose `volume_id` is not 0, if any. A table scan in storage
    /// order (deliberately unordered: an `ORDER BY path` would walk the index
    /// and fetch every row at random) — bounded memory, O(rows) time — so the
    /// reader can keep rejecting multi-volume rows at open (SPEC §5.2) without
    /// holding the rows.
    pub fn first_nonzero_volume(&self) -> Result<Option<(String, i64)>> {
        if !self.has_volume {
            return Ok(None); // no column ⇒ every row is volume 0
        }
        Ok(self
            .conn
            .query_row(
                "SELECT path, COALESCE(volume_id, 0) FROM entries                  WHERE COALESCE(volume_id, 0) != 0 LIMIT 1",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?)
    }

    /// `manifest` as an opaque string map. Caller decides whether to read it
    /// (only segment 0, SPEC §5.6).
    pub fn manifest(&self) -> Result<Vec<(String, String)>> {
        let present = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='manifest'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !present {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM manifest ORDER BY key ASC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// `EXPLAIN QUERY PLAN` for the point lookup — test hook that pins "this
    /// is an index probe, not a scan".
    #[doc(hidden)]
    pub fn lookup_plan(&self) -> Result<String> {
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT {} FROM entries WHERE path = ?1 COLLATE BINARY",
            self.columns
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(["x"], |r| r.get::<_, String>(3))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out.join("\n"))
    }

    /// `EXPLAIN QUERY PLAN` for a continuation page — same purpose.
    #[doc(hidden)]
    pub fn page_plan(&self) -> Result<String> {
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT {} FROM entries WHERE path COLLATE BINARY > ?2 \
             ORDER BY path COLLATE BINARY LIMIT ?1",
            self.columns
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![10i64, "x"], |r| r.get::<_, String>(3))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out.join("\n"))
    }
}

struct RawRow {
    path: String,
    title: Option<String>,
    offset: i64,
    length: i64,
    uncompressed_length: i64,
    mime: Option<String>,
    compression: Option<String>,
    sha256: Option<Vec<u8>>,
    volume_id: i64,
    redirect_to: Option<String>,
}

impl RawRow {
    fn into_entry(self, seg_offset: u64) -> Result<Entry> {
        let clamp = |v: i64| -> Result<u64> {
            u64::try_from(v).map_err(|_| WaxError::Schema {
                detail: format!(
                    "segment at {seg_offset}: entry {:?} has a negative integer field",
                    self.path
                ),
            })
        };
        let sha256 = match self.sha256 {
            None => None,
            Some(v) if v.len() == 32 => {
                let mut a = [0u8; 32];
                a.copy_from_slice(&v);
                Some(a)
            }
            Some(v) => {
                return Err(WaxError::Schema {
                    detail: format!(
                        "segment at {seg_offset}: entry {:?} sha256 is {} bytes, expected 32",
                        self.path,
                        v.len()
                    ),
                })
            }
        };
        Ok(Entry {
            offset: clamp(self.offset)?,
            length: clamp(self.length)?,
            uncompressed_length: clamp(self.uncompressed_length)?,
            title: self.title,
            mime: self.mime,
            compression: self.compression.unwrap_or_else(|| "none".to_string()),
            sha256,
            volume_id: self.volume_id,
            redirect_to: self.redirect_to,
            path: self.path,
        })
    }
}
