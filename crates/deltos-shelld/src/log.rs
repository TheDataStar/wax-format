//! The action log (privilege boundary rule 12): who, what, when, the
//! arguments in summary and the result, one JSON object per line, appended
//! and synced before the answer goes back. Only `deltos-shelld` writes it;
//! secrets never reach it (each action's `summary` redacts them).
//!
//! Its place under `/var/lib/deltos/` is open (privilege boundary §14,
//! contract §13.2), so it is a required command-line argument, not a default.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Unix seconds.
    pub at: u64,
    /// The person's profile id, or `unknown` when the session was refused.
    pub by: String,
    pub action: String,
    /// Redacted summary of the arguments.
    pub args: String,
    /// `ok`, `started` (a job), or a deltd API §7 error code.
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
}

/// Opened for each entry, not held: a restore replaces the state tree,
/// this file included, and the next entry must go to the file now in place.
pub struct ActionLog {
    path: PathBuf,
    lock: Mutex<()>,
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl ActionLog {
    /// Opened for appending only, created `0640` (root, and the group of its
    /// folder) if new.
    pub fn open(path: &Path) -> std::io::Result<ActionLog> {
        Self::append(path)?;
        Ok(ActionLog { path: path.to_path_buf(), lock: Mutex::new(()) })
    }

    fn append(path: &Path) -> std::io::Result<File> {
        OpenOptions::new().append(true).create(true).mode(0o640).open(path)
    }

    /// Appended and synced. A log that cannot be written stops the action
    /// being reported as done: the caller turns the error into `internal`.
    pub fn write(&self, e: &Entry) -> std::io::Result<()> {
        let mut line = serde_json::to_string(e).map_err(std::io::Error::other)?;
        line.push('\n');
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = Self::append(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.sync_data()
    }

    /// The newest `n` entries, newest last (for `admin.console`).
    pub fn tail(&self, n: usize) -> std::io::Result<Vec<Entry>> {
        let f = File::open(&self.path)?;
        let mut out = std::collections::VecDeque::with_capacity(n.min(1024));
        for line in BufReader::new(f).lines() {
            let line = line?;
            if let Ok(e) = serde_json::from_str::<Entry>(&line) {
                if out.len() == n {
                    out.pop_front();
                }
                out.push_back(e);
            }
        }
        Ok(out.into_iter().collect())
    }
}
