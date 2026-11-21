//! Backup and restore behind the privilege boundary.
//!
//! These are `deltos-shelld` actions (docs/privilege-boundary.md §8) and this
//! module is the shape shelld will call, exactly as `deltos_net::actions`:
//! **nothing here changes the box except through [`perform`], which takes
//! only an [`Authorized`], which only [`authorize`] makes**, after the same
//! three checks against `identityd`: a live **secure** session, the
//! `backup.manage` permission (contract §4.2), and for a restore a
//! single-use confirmation ticket bound to this session, this action and a
//! digest of its exact arguments (privilege boundary §6).
//!
//! The list is closed: `start_backup`, `review_backup`, `restore_backup`.
//! Drives and backups are named by ids from an earlier listing, never by a
//! path the caller writes (rule 6): shelld resolves a drive id to its
//! mount point through [`Drives`].
//!
//! Root at the box's own console is outside the boundary (rule 2: it
//! already owns the box). The `deltos-backup` command makes its
//! [`Authorized`] with [`authorize_console`], which refuses anyone else.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

pub use deltos_net::actions::{Caller, IdentityCheck, Refusal};

use crate::archive::{self, Created};
use crate::restore::{self, Applied, Review, Services};
use crate::{hex, Error, Layout, Passphrase, Result};

/// The permission every backup action needs (contract §4.2).
pub const PERMISSION: &str = "backup.manage";

#[derive(Debug, Clone)]
pub enum Action {
    /// Write an encrypted backup to a mounted drive.
    StartBackup { drive: String, passphrase: Passphrase },
    /// Open a backup with its passphrase and stage it for review; changes
    /// nothing live (Track F §9's review step).
    ReviewBackup { drive: String, backup_id: String, passphrase: Passphrase },
    /// Put the reviewed backup in place. Re-confirmed.
    RestoreBackup { review_id: String, review_digest: String },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::StartBackup { .. } => "start_backup",
            Action::ReviewBackup { .. } => "review_backup",
            Action::RestoreBackup { .. } => "restore_backup",
        }
    }

    /// Restoring replaces every person, password and the box's certificate:
    /// it asks again (privilege boundary §8).
    pub fn needs_reconfirm(&self) -> bool {
        matches!(self, Action::RestoreBackup { .. })
    }

    /// The canonical argument string a ticket is bound to. Passphrases
    /// appear only by digest, so nothing holding this string holds one.
    pub fn canonical_args(&self) -> String {
        let pd = |p: &Passphrase| hex(&Sha256::digest(p.expose().as_bytes()));
        match self {
            Action::StartBackup { drive, passphrase } => format!("drive={drive}\npassphrase_sha256={}", pd(passphrase)),
            Action::ReviewBackup { drive, backup_id, passphrase } => {
                format!("drive={drive}\nbackup_id={backup_id}\npassphrase_sha256={}", pd(passphrase))
            }
            Action::RestoreBackup { review_id, review_digest } => format!("review_id={review_id}\nreview_digest={review_digest}"),
        }
    }

    pub fn args_digest(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.name().as_bytes());
        h.update(b"\n");
        h.update(self.canonical_args().as_bytes());
        hex(&h.finalize())
    }

    /// For the action log (rule 12): secrets as `[redacted]`.
    pub fn summary(&self) -> String {
        match self {
            Action::StartBackup { drive, .. } => format!("start_backup drive={drive} passphrase=[redacted]"),
            Action::ReviewBackup { drive, backup_id, .. } => format!("review_backup drive={drive} backup_id={backup_id} passphrase=[redacted]"),
            Action::RestoreBackup { review_id, .. } => format!("restore_backup review_id={review_id}"),
        }
    }

    fn validate(&self) -> Result<()> {
        let drive_ok = |d: &str| {
            if !d.is_empty() && d.len() <= 64 && d.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                Ok(())
            } else {
                Err(Error::Invalid(format!("{d:?} is not a drive id")))
            }
        };
        match self {
            Action::StartBackup { drive, .. } => drive_ok(drive),
            Action::ReviewBackup { drive, backup_id, .. } => {
                drive_ok(drive)?;
                archive::check_backup_id(backup_id)
            }
            Action::RestoreBackup { review_id, review_digest } => {
                restore::check_review_id(review_id)?;
                if review_digest.len() == 64 && review_digest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                    Ok(())
                } else {
                    Err(Error::Invalid("review_digest".into()))
                }
            }
        }
    }
}

/// An action that passed every check.
#[derive(Debug)]
pub struct Authorized {
    action: Action,
    /// Profile id, or `console`.
    by: String,
}

impl Authorized {
    pub fn action(&self) -> &Action {
        &self.action
    }
    pub fn by(&self) -> &str {
        &self.by
    }
}

/// shelld's checks. The ticket is redeemed only after every other check
/// passed, so a refused request never spends it.
pub fn authorize(action: Action, caller: &Caller, id: &dyn IdentityCheck) -> std::result::Result<Authorized, Refusal> {
    let (profile, secure) = id.session(caller.session_ref).ok_or(Refusal::NotSignedIn)?;
    if !caller.over_https || !secure {
        return Err(Refusal::SecureConnectionRequired);
    }
    if !id.has_permission(&profile, PERMISSION) {
        return Err(Refusal::PermissionDenied);
    }
    action.validate().map_err(|e| Refusal::Invalid(e.to_string()))?;
    if action.needs_reconfirm() {
        let t = caller.confirmation_ticket.ok_or(Refusal::ConfirmationRequired)?;
        if !id.redeem_ticket(t, caller.session_ref, action.name(), &action.args_digest()) {
            return Err(Refusal::ConfirmationRequired);
        }
    }
    Ok(Authorized { action, by: profile })
}

/// Root at the box's own console (privilege boundary rule 2). Anyone else
/// is refused.
pub fn authorize_console(action: Action) -> std::result::Result<Authorized, Refusal> {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return Err(Refusal::PermissionDenied);
    }
    action.validate().map_err(|e| Refusal::Invalid(e.to_string()))?;
    Ok(Authorized { action, by: "console".into() })
}

/// Test-only: an [`Authorized`] without the checks.
#[doc(hidden)]
pub fn authorize_unchecked_for_tests(action: Action) -> Authorized {
    Authorized { action, by: "test".into() }
}

/// Drive ids to mount points, from shelld's own drive listing.
pub trait Drives {
    /// `writable` asks for the drive mounted for writing (rule 19: only
    /// for an action that writes a backup).
    fn mount_point(&self, drive_id: &str, writable: bool) -> Option<PathBuf>;
}

#[derive(Debug)]
pub enum Outcome {
    Created(Created),
    Reviewed(Box<Review>),
    Restored(Applied),
}

pub fn perform(layout: &Layout, a: Authorized, drives: &dyn Drives, services: &mut dyn Services) -> Result<Outcome> {
    let drive = |id: &str, w: bool| drives.mount_point(id, w).ok_or_else(|| Error::NotFound(format!("no drive {id}")));
    match a.action {
        Action::StartBackup { drive: d, passphrase } => Ok(Outcome::Created(archive::create(layout, &drive(&d, true)?, &passphrase, &a.by)?)),
        Action::ReviewBackup { drive: d, backup_id, passphrase } => {
            let path = drive(&d, false)?.join(&backup_id);
            Ok(Outcome::Reviewed(Box::new(restore::review(layout, &path, &backup_id, &passphrase)?)))
        }
        Action::RestoreBackup { review_id, review_digest } => Ok(Outcome::Restored(restore::apply(layout, &review_id, &review_digest, services)?)),
    }
}
