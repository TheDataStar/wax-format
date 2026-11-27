//! F10: backup and restore of the box's own state (docs/box-backup.md).
//!
//! A backup is one file on a drive: the box's state tree `/var/lib/deltos/`
//! (Track E §4, contract §10), **minus** the pack files themselves and what
//! describes only this hardware, as a tar stream compressed with zstd and
//! **encrypted with a passphrase the admin types** ([age] format, scrypt).
//! Nothing about the box is written to the drive in the clear: without the
//! passphrase the file says only that it is an age file and when it was
//! made (its name).
//!
//! Restore is two steps (Track F §9's staged review):
//! 1. [`restore::review`] decrypts into a staging folder beside the live
//!    state, checks every file against the backup's own list, refuses state
//!    newer than this box's programs understand (contract §10), and returns
//!    a [`restore::Review`]: how many people and which roles, the
//!    certificate authority's fingerprint, the Wi-Fi name, which packs to
//!    reinstall. Nothing live changes.
//! 2. [`restore::apply`] swaps the staged folders in, one component at a
//!    time, putting everything back if any step fails, and keeps the
//!    replaced state in one "previous" folder.
//!
//! Both are admin actions over HTTPS behind the privilege boundary
//! ([`actions`]); the `deltos-backup` command is the same code for root at
//! the box's own console, which already owns the box (privilege boundary
//! rule 2).
//!
//! [age]: https://age-encryption.org/v1

pub mod actions;
pub mod archive;
pub mod owners;
pub mod restore;
pub mod set;

use std::path::{Path, PathBuf};

pub use actions::{Action, Authorized, Caller, IdentityCheck, Refusal};
pub use archive::{create, list, BackupInfo, Created, Manifest};
pub use restore::{apply, review, Applied, Review, Services};

/// The state tree (contract §10).
pub const STATE_ROOT: &str = "/var/lib/deltos";

/// Version of the backup's own layout (the manifest and the tar paths).
/// A restore refuses a newer one, like every other state (contract §10).
pub const FORMAT_VERSION: u32 = 1;

/// The scrypt work factor every backup is written with: `N = 2^18`, 256 MiB
/// of memory. It is age's own documented default ("roughly 1 second on a
/// modern machine"), fixed rather than measured on the writing box, so a
/// backup made on a fast mini PC still opens on a 2 GB Pi (contract §2.1
/// minimum spec). A restore accepts no higher factor than this, so a
/// hostile file cannot make the box spend more memory than that.
pub const SCRYPT_LOG_N: u8 = 18;

/// Passphrase floor and ceiling: the box's own password rule
/// (`deltos_identity::policy`), because a person types it the same way, and
/// a copy of the file can be attacked offline for as long as anyone likes.
pub const PASSPHRASE_MIN_CHARS: usize = deltos_identity::policy::PASSWORD_MIN_CHARS;
pub const PASSPHRASE_MAX_BYTES: usize = deltos_identity::policy::PASSWORD_MAX_BYTES;

/// A passphrase. Never printed, never logged (privilege boundary rule 12).
#[derive(Clone)]
pub struct Passphrase(String);

impl Passphrase {
    pub fn new(s: impl Into<String>) -> Result<Self> {
        let s = s.into();
        if s.chars().count() < PASSPHRASE_MIN_CHARS {
            return Err(Error::Invalid(format!("the passphrase must be at least {PASSPHRASE_MIN_CHARS} characters")));
        }
        if s.len() > PASSPHRASE_MAX_BYTES {
            return Err(Error::Invalid(format!("the passphrase must be at most {PASSPHRASE_MAX_BYTES} bytes")));
        }
        Ok(Passphrase(s))
    }
    pub(crate) fn secret(&self) -> age::secrecy::SecretString {
        age::secrecy::SecretString::from(self.0.clone())
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

/// Where things are.
#[derive(Debug, Clone)]
pub struct Layout {
    /// The state tree, normally [`STATE_ROOT`].
    pub root: PathBuf,
    /// Set owners and modes from the backup's list on restore. Needs root;
    /// off in tests.
    pub set_owners: bool,
    /// Test-only: make the swap of this component fail, to prove the
    /// swaps already made are put back.
    #[doc(hidden)]
    pub fault_swap: Option<String>,
}

impl Layout {
    pub fn system() -> Self {
        Layout { root: PathBuf::from(STATE_ROOT), set_owners: true, fault_swap: None }
    }
    pub fn at(root: &Path) -> Self {
        Layout { root: root.to_path_buf(), set_owners: false, fault_swap: None }
    }
    /// Where a review unpacks a backup: inside the state tree, so a restore
    /// is a rename on one filesystem, never a copy.
    pub fn staging(&self) -> PathBuf {
        self.root.join(set::STAGING_DIR)
    }
    /// What the last restore replaced.
    pub fn previous(&self) -> PathBuf {
        self.root.join(set::PREVIOUS_DIR)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid_request: {0}")]
    Invalid(String),
    /// Wrong passphrase, or the file is not a backup (the two cannot be told
    /// apart, by design of the encryption).
    #[error("backup_unreadable: the passphrase is wrong or the file is not a DeltOS backup")]
    Unreadable,
    /// The backup opened but its contents are not what its own list says.
    #[error("backup_damaged: {0}")]
    Damaged(String),
    /// State newer than this box's programs understand (contract §10).
    #[error("backup_too_new: {0}")]
    TooNew(String),
    #[error("backup_not_found: {0}")]
    NotFound(String),
    #[error("restore_failed: {0}")]
    RestoreFailed(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
}

impl Error {
    /// The deltd API §7-style code.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Invalid(_) => "invalid_request",
            Error::Unreadable => "backup_unreadable",
            Error::Damaged(_) => "backup_damaged",
            Error::TooNew(_) => "backup_too_new",
            Error::NotFound(_) => "backup_not_found",
            Error::RestoreFailed(_) => "restore_failed",
            Error::Io(_) | Error::Sql(_) => "internal",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
