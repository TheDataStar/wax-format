//! Directory tree → `Vec<EntryInput>` (SPEC §6.1, §6.2) and the build/append
//! orchestration on top of `wax-core`'s writer.

use crate::config::PackConfig;
use anyhow::{bail, Context, Result};
use std::path::Path;
use walkdir::WalkDir;
use wax_core::writer::normalize_path;
use wax_core::{Compression, EntryInput};

/// Config filenames that are build inputs, not archive content.
const SKIP_FILENAMES: [&str; 1] = ["wax-pack.toml"];

/// A summary of what the walk produced, for CLI reporting.
#[derive(Debug, Default)]
pub struct WalkStats {
    pub files: usize,
    pub aliases: usize,
    pub bytes_in: u64,
    pub skipped: Vec<String>,
}

/// Walk `input`, producing entries in **ascending path order**.
///
/// Deterministic ordering matters: `wax-core`'s writer lays blobs down in the
/// order it is handed them, so a stable sort is what makes two builds of the
/// same tree byte-identical (see the determinism conformance test).
pub fn collect_entries(
    input: &Path,
    cfg: &PackConfig,
) -> Result<(Vec<EntryInput>, WalkStats)> {
    if !input.is_dir() {
        bail!("input {} is not a directory", input.display());
    }
    let mut stats = WalkStats::default();
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();

    for dirent in WalkDir::new(input).sort_by_file_name() {
        let dirent = dirent.with_context(|| format!("walking {}", input.display()))?;
        if !dirent.file_type().is_file() {
            continue;
        }
        let name = dirent.file_name().to_string_lossy().to_string();
        if SKIP_FILENAMES.contains(&name.as_str()) {
            stats.skipped.push(name);
            continue;
        }
        let rel = dirent
            .path()
            .strip_prefix(input)
            .expect("walkdir yields paths under the root")
            .to_string_lossy()
            .to_string();
        let path = normalize_path(&rel)
            .with_context(|| format!("normalizing {}", dirent.path().display()))?;

        if cfg.build.exclude.iter().any(|p| path.starts_with(p.as_str())) {
            stats.skipped.push(path);
            continue;
        }
        files.push((path, dirent.path().to_path_buf()));
    }

    // Stable, locale-independent ordering by the *normalized* path.
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut entries = Vec::with_capacity(files.len() + cfg.aliases.len());
    for (path, disk) in files {
        let bytes = std::fs::read(&disk)
            .with_context(|| format!("reading {}", disk.display()))?;
        stats.bytes_in += bytes.len() as u64;

        let codec = choose_compression(&path, cfg);
        let mut e = EntryInput::data(path.clone(), bytes, codec);
        if let Some(m) = mime_guess::from_path(&disk).first_raw() {
            e = e.with_mime(m);
        }
        if let Some(t) = cfg.titles.get(&path) {
            e = e.with_title(t.clone());
        }
        entries.push(e);
        stats.files += 1;
    }

    // Aliases become redirect entries; `wax-core` flattens declared chains so
    // the emitted graph is exactly one hop (SPEC §6.2).
    for (alias, target) in &cfg.aliases {
        let alias = normalize_path(alias)
            .with_context(|| format!("normalizing alias source {alias:?}"))?;
        let target = normalize_path(target)
            .with_context(|| format!("normalizing alias target {target:?}"))?;
        entries.push(EntryInput::redirect(alias, target));
        stats.aliases += 1;
    }

    if entries.is_empty() {
        bail!("no files found under {}", input.display());
    }
    Ok((entries, stats))
}

/// Per-entry codec choice (SPEC §3 defines the value set `{none, zstd}`).
///
/// Policy: honour `build.compression` as the default, but store formats that
/// are already entropy-coded verbatim — zstd on a JPEG burns CPU and typically
/// grows the blob. This is extension-based and therefore deterministic; there is
/// deliberately no "compress and keep whichever is smaller" pass (see README).
fn choose_compression(path: &str, cfg: &PackConfig) -> Compression {
    if cfg.build.compression == "none" {
        return Compression::None;
    }
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !ext.is_empty() && cfg.build.store_uncompressed.contains(&ext) {
        Compression::None
    } else {
        Compression::Zstd
    }
}
