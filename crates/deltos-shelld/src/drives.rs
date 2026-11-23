//! Removable drives, by id (rule 6: a drive is named by an id from a
//! listing, never by a path the caller writes).
//!
//! A drive is a folder under the drives folder (normally `/run/deltos/drives`)
//! that is a mount point. Mounting them is `mount_removable`, not built yet,
//! because the filesystem types it may mount are Track E's to name and Track E
//! names none (privilege boundary rule 19; `docs/shelld.md` §6). Until then a
//! drive mounted there by root at the console is listed and usable.

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Drive {
    pub id: String,
    pub writable: bool,
    pub free_bytes: Option<u64>,
}

pub struct DrivesDir(pub PathBuf);

fn plain(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn read_only(p: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return true };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: statvfs writes into `s`, which is a statvfs.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return true;
    }
    s.f_flag & libc::ST_RDONLY != 0
}

impl DrivesDir {
    pub fn list(&self) -> Vec<Drive> {
        let Ok(top) = std::fs::metadata(&self.0) else { return Vec::new() };
        let Ok(rd) = std::fs::read_dir(&self.0) else { return Vec::new() };
        let mut out: Vec<Drive> = rd
            .flatten()
            .filter_map(|e| {
                let id = e.file_name().to_str()?.to_string();
                let m = std::fs::symlink_metadata(e.path()).ok()?;
                // a mount point: a real folder on another device
                (plain(&id) && m.is_dir() && m.dev() != top.dev()).then(|| Drive {
                    writable: !read_only(&e.path()),
                    free_bytes: crate::addons::free_bytes(&e.path()).ok(),
                    id,
                })
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Where an addon listing looks on each drive: its top folder.
    pub fn origins(&self) -> Vec<(String, PathBuf)> {
        self.list().into_iter().map(|d| (format!("drive-{}", d.id), self.0.join(d.id))).collect()
    }
}

impl deltos_backup::actions::Drives for DrivesDir {
    fn mount_point(&self, drive_id: &str, writable: bool) -> Option<PathBuf> {
        self.list().into_iter().find(|d| d.id == drive_id && (d.writable || !writable)).map(|d| self.0.join(d.id))
    }
}
