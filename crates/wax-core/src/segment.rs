//! One embedded SQLite index segment (SPEC §4).
//!
//! A segment's bytes are a raw SQLite database. This module copies a byte range
//! to a temp file, opens it read-only, and reads `segment_meta` + `entries`.
//! Every SQLite / schema problem becomes a [`WaxError`]; nothing panics
//! (A4 fuzz target 2, SPEC §11.3).

use crate::model::Entry;
use crate::{Result, WaxError};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::io::Write;
use tempfile::NamedTempFile;

/// The constant stored at `segment_meta.format` (SPEC §4.2).
pub const SEGMENT_FORMAT_TAG: &str = "wax-index-segment";

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

/// An opened index segment. Holds its temp file alive for the connection's life.
pub struct Segment {
    /// Absolute file offset this segment's database starts at (for diagnostics).
    pub disk_offset: u64,
    /// Length in bytes of this segment's SQLite database on disk.
    pub db_len: u64,
    pub meta: SegmentMeta,
    conn: Connection,
    has_redirect_to: bool,
    has_title: bool,
    _tmp: NamedTempFile,
}

impl Segment {
    /// Open a segment from `bytes` (already sliced out of the archive).
    /// `disk_offset` is only used in error messages.
    pub fn open(disk_offset: u64, bytes: &[u8]) -> Result<Self> {
        let mut tmp = NamedTempFile::new()?;
        tmp.write_all(bytes)?;
        tmp.flush()?;

        let conn = Connection::open_with_flags(
            tmp.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| WaxError::NotAnIndexSegment {
            offset: disk_offset,
            reason: format!("not a SQLite database: {e}"),
        })?;

        // Touching the schema forces SQLite to actually parse the header/pages;
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

        Ok(Segment {
            disk_offset,
            db_len: bytes.len() as u64,
            meta,
            has_redirect_to: cols.iter().any(|c| c == "redirect_to"),
            has_title: cols.iter().any(|c| c == "title"),
            conn,
            _tmp: tmp,
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

    /// All `entries` rows in this segment, `ORDER BY path` (SPEC §5.5).
    pub fn entries(&self) -> Result<Vec<Entry>> {
        let title_col = if self.has_title { "title" } else { "NULL" };
        let redir_col = if self.has_redirect_to {
            "redirect_to"
        } else {
            "NULL"
        };
        // volume_id may be absent in an odd build; COALESCE via a subselect-free
        // approach: check column presence like the others.
        let has_volume = Self::entry_columns(&self.conn)?
            .iter()
            .any(|c| c == "volume_id");
        let vol_col = if has_volume { "volume_id" } else { "0" };

        let sql = format!(
            "SELECT path, {title_col} AS title, offset, length, uncompressed_length, \
             mime, compression, sha256, {vol_col} AS volume_id, {redir_col} AS redirect_to \
             FROM entries ORDER BY path ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            let sha: Option<Vec<u8>> = r.get("sha256")?;
            Ok(RawRow {
                path: r.get("path")?,
                title: r.get("title")?,
                offset: r.get::<_, i64>("offset")?,
                length: r.get::<_, i64>("length")?,
                uncompressed_length: r.get::<_, i64>("uncompressed_length")?,
                mime: r.get("mime")?,
                compression: r.get("compression")?,
                sha256: sha,
                volume_id: r.get::<_, i64>("volume_id")?,
                redirect_to: r.get("redirect_to")?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_entry(self.disk_offset)?);
        }
        Ok(out)
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
