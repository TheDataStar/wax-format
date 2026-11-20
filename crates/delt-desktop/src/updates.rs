//! Edition updates: bring an installed pack up to a newer edition the
//! person has on disk (a USB stick, a download folder), and undo it.
//!
//! Two shapes of newer edition exist (SPEC §7.4):
//!
//! * **An appended edition** — the publisher appended an update to the
//!   edition installed here. Its first new segment records the digest of
//!   the archive it extends (`prev_state_digest`). When that equals this
//!   copy's own digest, the update is applied in place following SPEC
//!   §7.4's procedure: cut back to the logical end, write the new bytes
//!   from there, then the new header. Only the new part is written, so
//!   updating a 90 GB pack needs room for the update, not a second copy.
//!   It can be undone ([`undo_last_update`], SPEC §7.5).
//! * **A rebuilt edition** — the same archive, rebuilt from scratch, so it
//!   does not extend this copy byte for byte. It replaces the file whole,
//!   and only when its manifest `version` (CalVer `YYYY.MM.N`, contract
//!   §11) is newer. Without a version on both, which one is newer is not
//!   known, and the update is refused rather than guessed.
//!
//! **Signatures are reported, not judged.** Which publisher keys a desktop
//! should trust is not decided (the box's equivalent is open too:
//! architecture Q2, contract §13.2). A plan says whether the new edition
//! carries a signature sidecar; it is copied along with the update, and
//! nothing claims it was checked.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use wax_core::{ReadOptions, WaxReader, HEADER_LEN};

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("{0} is not a pack Delt can open: {1}")]
    Unreadable(PathBuf, String),
    #[error("this file is a different pack, not an edition of the installed one")]
    DifferentArchive,
    #[error("the installed pack is already this edition")]
    AlreadyCurrent,
    #[error("this edition is older than the installed one")]
    Older,
    #[error("Delt cannot tell which edition is newer: {0}")]
    CannotOrder(String),
    #[error("the installed pack changed since this update was checked; check it again")]
    Stale,
    #[error("the update did not produce the expected edition and was undone: {0}")]
    Mismatch(String),
    #[error("this pack has no update that can be undone: {0}")]
    NothingToUndo(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Wax(#[from] wax_core::WaxError),
}

pub type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpdateKind {
    /// Applied in place; `bytes` is what will be written.
    Append { new_updates: usize, bytes: u64 },
    /// The whole file is replaced; `bytes` is the new file's size.
    Replace { bytes: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdatePlan {
    pub archive_uuid: String,
    pub name: String,
    pub installed: PathBuf,
    pub candidate: PathBuf,
    pub installed_version: Option<String>,
    pub candidate_version: Option<String>,
    pub kind: UpdateKind,
    /// The new edition carries a `.minisig` sidecar (SPEC §8.2). Not checked:
    /// see the module note.
    pub signature_sidecar: bool,
    /// The installed copy's digest when checked, so a plan cannot be applied
    /// to a file that changed since.
    #[serde(skip)]
    base_digest: [u8; 32],
}

fn open(p: &Path) -> Result<WaxReader> {
    WaxReader::open_with(p, ReadOptions { verify_checksums: false, lookup_cache_entries: 0 })
        .map_err(|e| UpdateError::Unreadable(p.to_path_buf(), e.to_string()))
}

/// CalVer `YYYY.MM.N` (contract §11), ordered by its three numbers.
pub fn calver(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.trim().split('.');
    let y: u32 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let n: u32 = it.next()?.parse().ok()?;
    (it.next().is_none() && (1..=12).contains(&m) && n >= 1).then_some((y, m, n))
}

pub fn sidecar_of(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".minisig");
    PathBuf::from(s)
}

/// Decide whether `candidate` updates the pack at `installed`, and how.
pub fn inspect(installed: &Path, candidate: &Path) -> Result<UpdatePlan> {
    let a = open(installed)?;
    let b = open(candidate)?;
    if a.header().archive_uuid != b.header().archive_uuid {
        return Err(UpdateError::DifferentArchive);
    }
    let da = a.signable_digest()?;
    let db = b.signable_digest()?;
    if da == db {
        return Err(UpdateError::AlreadyCurrent);
    }
    let (na, nb) = (a.segment_count(), b.segment_count());
    let va = a.manifest().get("version").cloned();
    let vb = b.manifest().get("version").cloned();
    let kind = if nb > na && b.prev_state_digest(na) == Some(da) {
        UpdateKind::Append { new_updates: nb - na, bytes: b.file_size().saturating_sub(a.logical_end()) }
    } else if na > nb && a.prev_state_digest(nb) == Some(db) {
        return Err(UpdateError::Older);
    } else {
        let (Some(x), Some(y)) = (va.as_deref(), vb.as_deref()) else {
            return Err(UpdateError::CannotOrder("one of the two editions has no version".into()));
        };
        let (Some(x), Some(y)) = (calver(x), calver(y)) else {
            return Err(UpdateError::CannotOrder(format!("a version is not YYYY.MM.N ({x:?}, {y:?})")));
        };
        match y.cmp(&x) {
            std::cmp::Ordering::Greater => UpdateKind::Replace { bytes: b.file_size() },
            std::cmp::Ordering::Less => return Err(UpdateError::Older),
            std::cmp::Ordering::Equal => {
                return Err(UpdateError::CannotOrder(
                    "both editions say the same version but their contents differ".into(),
                ))
            }
        }
    };
    Ok(UpdatePlan {
        archive_uuid: deltlib::library::uuid_text(&a.header().archive_uuid),
        name: a.manifest().get("name").cloned().unwrap_or_default(),
        installed: installed.to_path_buf(),
        candidate: candidate.to_path_buf(),
        installed_version: va,
        candidate_version: vb,
        kind,
        signature_sidecar: sidecar_of(candidate).is_file(),
        base_digest: da,
    })
}

/// What applying a plan did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Applied {
    pub archive_uuid: String,
    pub version: Option<String>,
    pub bytes_written: u64,
    pub can_undo: bool,
}

/// Apply a plan made by [`inspect`]. The installed copy is checked again
/// first; if it changed, nothing is written.
pub fn apply(plan: &UpdatePlan) -> Result<Applied> {
    let a = open(&plan.installed)?;
    if a.signable_digest()? != plan.base_digest {
        return Err(UpdateError::Stale);
    }
    let b = open(&plan.candidate)?;
    let want = b.signable_digest()?;
    let end = a.logical_end();
    drop(a);
    drop(b);

    let written = match plan.kind {
        UpdateKind::Append { .. } => {
            let n = append_tail(&plan.installed, &plan.candidate, end)?;
            let got = open(&plan.installed)?.signable_digest()?;
            if got != want {
                // the new part did not describe what the publisher's edition
                // does; put the installed edition back exactly (SPEC §7.5)
                let undo = wax_core::rollback_last_update(&plan.installed).map(|_| ()).map_err(|e| e.to_string());
                return Err(UpdateError::Mismatch(match undo {
                    Ok(()) => "the result's digest differs from the new edition's".into(),
                    Err(e) => format!("the result's digest differs, and undoing it failed: {e}"),
                }));
            }
            n
        }
        UpdateKind::Replace { .. } => replace_whole(&plan.installed, &plan.candidate)?,
    };
    let side = sidecar_of(&plan.candidate);
    if side.is_file() {
        std::fs::copy(&side, sidecar_of(&plan.installed))?;
    }
    let r = open(&plan.installed)?;
    Ok(Applied {
        archive_uuid: plan.archive_uuid.clone(),
        version: r.manifest().get("version").cloned(),
        bytes_written: written,
        can_undo: undo_available(&r),
    })
}

/// SPEC §7.4's apply procedure with the new edition itself as the delta:
/// cut to `end`, write the new edition's bytes from `end`, then its header.
fn append_tail(installed: &Path, candidate: &Path, end: u64) -> Result<u64> {
    let mut src = File::open(candidate)?;
    let mut header = [0u8; HEADER_LEN];
    src.read_exact(&mut header)?;
    let total = src.metadata()?.len();
    let mut dst = OpenOptions::new().read(true).write(true).open(installed)?;
    dst.set_len(end)?; // an orphaned tail and the old mirror (§2.4) go
    dst.sync_all()?;
    src.seek(SeekFrom::Start(end))?;
    dst.seek(SeekFrom::Start(end))?;
    let n = std::io::copy(&mut (&mut src).take(total - end), &mut dst)?;
    dst.sync_all()?;
    dst.seek(SeekFrom::Start(0))?;
    dst.write_all(&header)?;
    dst.sync_all()?;
    Ok(n)
}

/// Copy beside the installed file under a name the library ignores, then
/// rename over it, so the library only ever sees one whole edition.
fn replace_whole(installed: &Path, candidate: &Path) -> Result<u64> {
    let part = installed.with_extension("wax-part");
    let n = std::fs::copy(candidate, &part)?;
    File::open(&part)?.sync_all()?;
    std::fs::rename(&part, installed)?;
    Ok(n)
}

fn undo_available(r: &WaxReader) -> bool {
    let n = r.segment_count();
    n > 1 && r.prev_header(n - 1).is_some() && r.prev_state_digest(n - 1).is_some()
}

/// Can the newest update of this pack be undone (SPEC §7.5)?
pub fn can_undo(installed: &Path) -> bool {
    open(installed).map(|r| undo_available(&r)).unwrap_or(false)
}

/// Undo the newest appended update (SPEC §7.5): the file goes back byte for
/// byte to the edition before it.
pub fn undo_last_update(installed: &Path) -> Result<Applied> {
    let r = open(installed)?;
    if !undo_available(&r) {
        return Err(UpdateError::NothingToUndo(
            "it has no appended update, or its newest one was written before undo was possible (WAX v0.12)".into(),
        ));
    }
    let uuid = deltlib::library::uuid_text(&r.header().archive_uuid);
    drop(r);
    wax_core::rollback_last_update(installed)?;
    let r = open(installed)?;
    Ok(Applied {
        archive_uuid: uuid,
        version: r.manifest().get("version").cloned(),
        bytes_written: 0,
        can_undo: undo_available(&r),
    })
}

/// Every `.wax` directly inside `dir` that updates one of `installed`
/// (`(archive_uuid, path)`). Files that are not updates are skipped.
pub fn find_updates(dir: &Path, installed: &[(String, PathBuf)]) -> std::io::Result<Vec<UpdatePlan>> {
    let mut out = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "wax") && p.is_file())
        .collect();
    files.sort();
    for f in files {
        let Ok(r) = open(&f) else { continue };
        let uuid = deltlib::library::uuid_text(&r.header().archive_uuid);
        drop(r);
        if let Some((_, path)) = installed.iter().find(|(u, p)| *u == uuid && !same_file(p, &f)) {
            if let Ok(plan) = inspect(path, &f) {
                out.push(plan);
            }
        }
    }
    Ok(out)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calver_orders_by_numbers() {
        assert_eq!(calver("2026.09.1"), Some((2026, 9, 1)));
        assert!(calver("2026.10.1") > calver("2026.09.30"));
        assert_eq!(calver("2026.13.1"), None);
        assert_eq!(calver("2026.09.0"), None);
        assert_eq!(calver("1.2"), None);
        assert_eq!(calver("2026.09.1.4"), None);
    }
}
