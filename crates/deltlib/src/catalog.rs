//! `pack_index` and `pack_generation` allocation (contract §8).
//!
//! The contract has the on-device catalog allocate both at install time; the
//! privilege boundary's `retire_pack_hostnames` names "deltd's catalog" as the
//! one that frees an index. Until B4 exists this small table is that catalog's
//! hostname half, stored under the catalog's directory (contract §10).
//!
//! Rules carried here:
//! * an installed pack keeps its index and generation for as long as it is
//!   installed, across every update, so its browser storage carries over;
//! * a removed pack's index is freed, and the next pack given that index gets
//!   a **higher** generation, so it never inherits the removed pack's storage;
//! * there are 512 indexes (0–511); a 513th concurrent pack is refused.
//!
//! State carries a schema version and a newer one is refused (contract §10).

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::hostname::MAX_PACK_INDEX;

/// Schema version of this table, in `PRAGMA user_version`.
pub const CATALOG_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("catalog schema version {found} is newer than this deltd understands ({CATALOG_SCHEMA_VERSION}); refusing to open it")]
    NewerSchema { found: i64 },
    #[error("every pack_index 0–{MAX_PACK_INDEX} is in use (contract §8)")]
    Full,
    #[error("catalog database: {0}")]
    Sql(#[from] rusqlite::Error),
}

pub struct Catalog {
    conn: Connection,
}

/// Where one installed pack sits in the pack zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub pack_index: u16,
    pub pack_generation: u64,
}

impl Catalog {
    pub fn open(path: &Path) -> Result<Self, CatalogError> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, CatalogError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, CatalogError> {
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v > CATALOG_SCHEMA_VERSION {
            return Err(CatalogError::NewerSchema { found: v });
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS pack_slots (
                 pack_index      INTEGER PRIMARY KEY CHECK (pack_index BETWEEN 0 AND 511),
                 last_generation INTEGER NOT NULL CHECK (last_generation >= 1),
                 archive_uuid    TEXT UNIQUE          -- NULL while the index is free
             );",
        )?;
        conn.execute_batch(&format!("PRAGMA user_version = {CATALOG_SCHEMA_VERSION};"))?;
        Ok(Catalog { conn })
    }

    /// The slot of an installed pack, allocating one on first sight.
    pub fn assign(&mut self, archive_uuid: &str) -> Result<Slot, CatalogError> {
        if let Some(s) = self.slot_of(archive_uuid)? {
            return Ok(s);
        }
        let tx = self.conn.transaction()?;
        // lowest free index: a freed one (reused at a higher generation), or a never-used one
        let freed: Option<(i64, i64)> = tx
            .query_row(
                "SELECT pack_index, last_generation FROM pack_slots
                  WHERE archive_uuid IS NULL ORDER BY pack_index LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let unused: Option<i64> = {
            let mut st = tx.prepare("SELECT pack_index FROM pack_slots ORDER BY pack_index")?;
            let used: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            (0..=MAX_PACK_INDEX as i64).find(|i| used.binary_search(i).is_err())
        };
        let slot = match (freed, unused) {
            (Some((fi, _)), Some(u)) if u < fi => {
                tx.execute("INSERT INTO pack_slots VALUES (?1, 1, ?2)", params![u, archive_uuid])?;
                Slot { pack_index: u as u16, pack_generation: 1 }
            }
            (Some((fi, fg)), _) => {
                tx.execute(
                    "UPDATE pack_slots SET last_generation = ?2, archive_uuid = ?3 WHERE pack_index = ?1",
                    params![fi, fg + 1, archive_uuid],
                )?;
                Slot { pack_index: fi as u16, pack_generation: (fg + 1) as u64 }
            }
            (None, Some(u)) => {
                tx.execute("INSERT INTO pack_slots VALUES (?1, 1, ?2)", params![u, archive_uuid])?;
                Slot { pack_index: u as u16, pack_generation: 1 }
            }
            (None, None) => return Err(CatalogError::Full),
        };
        tx.commit()?;
        Ok(slot)
    }

    pub fn slot_of(&self, archive_uuid: &str) -> Result<Option<Slot>, CatalogError> {
        Ok(self
            .conn
            .query_row(
                "SELECT pack_index, last_generation FROM pack_slots WHERE archive_uuid = ?1",
                [archive_uuid],
                |r| {
                    Ok(Slot {
                        pack_index: r.get::<_, i64>(0)? as u16,
                        pack_generation: r.get::<_, i64>(1)? as u64,
                    })
                },
            )
            .optional()?)
    }

    /// Free a removed pack's index (`retire_pack_hostnames`). Its hostnames
    /// stop resolving at once; the index returns at a higher generation.
    pub fn retire(&mut self, archive_uuid: &str) -> Result<(), CatalogError> {
        self.conn.execute(
            "UPDATE pack_slots SET archive_uuid = NULL WHERE archive_uuid = ?1",
            [archive_uuid],
        )?;
        Ok(())
    }

    /// Every installed pack's uuid.
    pub fn installed(&self) -> Result<Vec<String>, CatalogError> {
        let mut st = self
            .conn
            .prepare("SELECT archive_uuid FROM pack_slots WHERE archive_uuid IS NOT NULL")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_slots_and_higher_generation_on_reuse() {
        let mut c = Catalog::in_memory().unwrap();
        let a = c.assign("a").unwrap();
        let b = c.assign("b").unwrap();
        assert_eq!((a.pack_index, a.pack_generation), (0, 1));
        assert_eq!((b.pack_index, b.pack_generation), (1, 1));
        assert_eq!(c.assign("a").unwrap(), a, "stable while installed");
        c.retire("a").unwrap();
        let n = c.assign("c").unwrap();
        assert_eq!((n.pack_index, n.pack_generation), (0, 2), "reused index, higher generation");
        c.retire("c").unwrap();
        let again = c.assign("a").unwrap();
        assert_eq!((again.pack_index, again.pack_generation), (0, 3), "a reinstall is a new generation too");
    }

    #[test]
    fn full_at_512() {
        let mut c = Catalog::in_memory().unwrap();
        for i in 0..512 {
            c.assign(&format!("p{i}")).unwrap();
        }
        assert!(matches!(c.assign("one-more"), Err(CatalogError::Full)));
        c.retire("p7").unwrap();
        assert_eq!(c.assign("one-more").unwrap(), Slot { pack_index: 7, pack_generation: 2 });
    }

    #[test]
    fn refuses_a_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cat.sqlite");
        drop(Catalog::open(&p).unwrap());
        Connection::open(&p).unwrap().execute_batch("PRAGMA user_version = 99;").unwrap();
        assert!(matches!(Catalog::open(&p), Err(CatalogError::NewerSchema { found: 99 })));
    }
}
