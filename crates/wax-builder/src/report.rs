//! The build report — Cross-Track Contract §11 "The build report".
//!
//! Every successful build writes `<archive-filename>.build-report.json` beside
//! the archive, **by default**. It is the Track A → Track B (catalog intake)
//! interface: intake reads `license_review_required` from it to route a pack to
//! human review, and treats an absent report as a failed intake rather than an
//! assumed-clean pack.
//!
//! It is *not* a trust artifact: it sits outside the signature and is the
//! builder's own claim about its run. Anything intake must trust is verified
//! against the signed archive itself.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Bump when a key is changed or removed; adding a key keeps it (§11).
pub const REPORT_VERSION: u32 = 1;

/// One warning line: a distinct code and how many times it occurred. §11: one
/// entry per code with a count, never one entry per occurrence.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Warning {
    pub code: String,
    pub count: u64,
}

/// Accumulates warnings by code. Converters push one code per skipped or
/// dropped thing; the report renders the totals.
#[derive(Debug, Default, Clone)]
pub struct Warnings(BTreeMap<String, u64>);

impl Warnings {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn bump(&mut self, code: &str) {
        *self.0.entry(code.to_string()).or_default() += 1;
    }
    pub fn add(&mut self, code: &str, n: u64) {
        if n > 0 {
            *self.0.entry(code.to_string()).or_default() += n;
        }
    }
    pub fn count(&self, code: &str) -> u64 {
        self.0.get(code).copied().unwrap_or(0)
    }
    pub fn total(&self) -> u64 {
        self.0.values().sum()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn to_vec(&self) -> Vec<Warning> {
        self.0
            .iter()
            .map(|(code, count)| Warning {
                code: code.clone(),
                count: *count,
            })
            .collect()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, u64)> {
        self.0.iter().map(|(k, v)| (k.as_str(), *v))
    }
}

/// The report exactly as §11 pins it. Field order matches the Contract's
/// example so a diff against it reads naturally.
#[derive(Debug, Clone, Serialize)]
pub struct BuildReport {
    pub report_version: u32,
    /// Canonical lowercase hyphenated (§11).
    pub archive_uuid: String,
    /// File name only, no directory.
    pub archive_filename: String,
    /// Unix epoch **milliseconds** UTC (§11, matching §5.2's timestamp rule).
    pub built_at: u64,
    /// `"<tool> <version>"`, e.g. `wax-builder 1.2.0`.
    pub builder_version: String,
    pub entry_count: u64,
    pub redirect_count: u64,
    pub skipped_count: u64,
    pub license: String,
    pub license_review_required: bool,
    pub signed: bool,
    pub warnings: Vec<Warning>,
}

impl BuildReport {
    /// `<archive-filename>.build-report.json`, beside the archive.
    pub fn path_for(archive: &Path) -> PathBuf {
        let name = archive
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        archive.with_file_name(format!("{name}.build-report.json"))
    }

    /// Write the report beside `archive`. Returns the path written.
    pub fn write(&self, archive: &Path) -> Result<PathBuf> {
        let path = Self::path_for(archive);
        let json = serde_json::to_string_pretty(self).context("serializing build report")?;
        std::fs::write(&path, json + "\n")
            .with_context(|| format!("writing build report {}", path.display()))?;
        Ok(path)
    }
}

/// Wall clock in Unix epoch milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
