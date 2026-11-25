//! DeltOS apps: the app manifest (contract §15), the fit check against the
//! box's measured resources (contract §2, §6, §9), and the capacity estimate
//! for the WebOS the box feeds to visitors' devices.
//!
//! Design: `docs/base-addons-admin.md`.

pub mod capacity;
pub mod fit;
pub mod manifest;
pub mod measure;

pub use manifest::{parse, AppManifest, ReadError};

use std::path::Path;

/// Read every `*.toml` manifest in a directory, sorted by file name.
pub fn read_dir(dir: &Path) -> Result<Vec<AppManifest>, String> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        out.push(parse(&text).map_err(|e| format!("{}: {e}", p.display()))?);
    }
    Ok(out)
}
