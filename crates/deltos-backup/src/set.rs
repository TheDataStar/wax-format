//! What a backup holds: the backup set (docs/box-backup.md §3).
//!
//! The rule is **everything under the state tree, except a short named
//! list**. Track F §9 makes the whole tree the one backup boundary, and
//! contract §16 says backups cover everything people create; including by
//! default means a new kind of state (notes, reading progress, a Track H
//! service's data, once their paths are fixed) is backed up the day it
//! appears, while excluding by default would lose it silently. Every
//! exclusion here has a written reason.

/// What happens to one top-level name under the state tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Treatment {
    /// Copied into the backup and replaced on restore.
    Include,
    /// Pack files: listed (uuid, name, version, size), not copied.
    PackList,
    /// Left out, for the reason given.
    Exclude(&'static str),
}

/// The installed packs (contract §10).
pub const PACKS_DIR: &str = "packs";
/// Where a review unpacks a backup. Never itself backed up.
pub const STAGING_DIR: &str = ".restore-staging";
/// What the last restore replaced. Never itself backed up.
pub const PREVIOUS_DIR: &str = ".restore-previous";

/// The register of people (contract §10).
pub const IDENTITY_DIR: &str = "identity";
pub const IDENTITY_DB: &str = "identity/identity.sqlite";
/// The network registry (docs/box-radio-and-boot.md §3).
pub const NETWORK_DB: &str = "network/registry.sqlite";
/// deltd's pack-hostname catalog (contract §10's B4 directory).
pub const CATALOG_DB: &str = "b4/deltd-pack-hostnames.sqlite";
/// The box's certificate authority (docs/box-certificates.md).
pub const CA_DIR: &str = "ca";
pub const CA_KEY: &str = "ca/ca.key";
pub const CA_CERT: &str = "ca/ca.crt";

const EXCLUDED: &[(&str, &str)] = &[
    (
        "capability.json",
        "the measured RAM, storage, architecture and GPU of this hardware (contract §9); a restore onto other hardware must measure its own",
    ),
    ("health", "health snapshots of this hardware and its services; they describe the old box, not anything a person made"),
    (STAGING_DIR, "a restore's own working folder"),
    (PREVIOUS_DIR, "what the last restore replaced; already a copy"),
];

pub fn treatment(top: &str) -> Treatment {
    if top == PACKS_DIR {
        return Treatment::PackList;
    }
    for (name, why) in EXCLUDED {
        if *name == top {
            return Treatment::Exclude(why);
        }
    }
    Treatment::Include
}

/// SQLite's side files. A database is copied through SQLite itself
/// (`VACUUM INTO`), which takes a consistent copy while its program runs;
/// these are never copied on their own.
pub fn is_sqlite_side_file(name: &str) -> bool {
    name.ends_with("-wal") || name.ends_with("-shm") || name.ends_with("-journal")
}

/// Rows of the register that are never kept: live sessions and unspent
/// confirmation tickets. They are bound to browsers and minutes, not to
/// people; restoring them would bring back sign-ins nobody made on this
/// box. Everyone signs in again after a restore. Remembered devices and
/// sign-in cards are kept, so teachers' phones and students' cards still
/// work (docs/box-backup.md §3).
pub const IDENTITY_DROPPED_TABLES: &[&str] = &["sessions", "tickets"];
