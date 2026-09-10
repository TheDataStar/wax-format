//! Build-time pack configuration (`wax-pack.toml`).
//!
//! Supplies everything the directory walk cannot infer: the `manifest` rows, the
//! alias (redirect) map, per-entry titles, and the compression policy.
//!
//! # Manifest and the Track B boundary
//!
//! SPEC.md §5.6 / §6.4 deliberately leave `manifest` *content* to Track B: A3
//! fixes only the table shape (`key TEXT PRIMARY KEY, value TEXT`), its
//! segment-0-only location, and its immutability across appends. This module
//! therefore treats manifest rows as **opaque strings** and performs **no
//! validation**: it does not enforce which keys are required, does not check
//! value domains (e.g. what `min_hw_tier` may be), and passes unknown keys
//! straight through so Track B can add fields without a code change.
//!
//! [`ManifestConfig::B3_FIELDS`] names the six fields called out for Track B's
//! B3 field set; they are reported by `inspect` but never enforced here.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Parsed `wax-pack.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackConfig {
    #[serde(default)]
    pub manifest: ManifestConfig,
    #[serde(default)]
    pub build: BuildConfig,
    /// `alias path` -> `target path`. Chains are flattened at build time
    /// (SPEC §6.2); the on-disk graph is always depth 1.
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    /// `entry path` -> human-readable `entries.title`.
    #[serde(default)]
    pub titles: BTreeMap<String, String>,
}

/// Manifest rows. Known B3 fields are named for discoverability only — every
/// value is written verbatim as a string and nothing is validated here.
#[derive(Debug, Default, Deserialize)]
pub struct ManifestConfig {
    pub name: Option<String>,
    pub icon: Option<String>,
    pub category: Option<String>,
    pub license: Option<String>,
    pub version: Option<String>,
    pub min_hw_tier: Option<String>,
    /// Any other key/value pair, passed through untouched.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl ManifestConfig {
    /// The six fields named as Track B's B3 set. Presence is *reported*, never
    /// required — see the module docs.
    pub const B3_FIELDS: [&'static str; 6] =
        ["name", "icon", "category", "license", "version", "min_hw_tier"];

    /// Flatten to the `manifest` rows written into segment 0.
    ///
    /// Scalars become their natural string form; arrays/tables are rejected
    /// rather than guessing an encoding, because SPEC §5.6 types the column as
    /// `TEXT` and Track B has not defined a nesting convention.
    pub fn to_rows(&self) -> Result<BTreeMap<String, String>> {
        let mut rows = BTreeMap::new();
        let named: [(&str, &Option<String>); 6] = [
            ("name", &self.name),
            ("icon", &self.icon),
            ("category", &self.category),
            ("license", &self.license),
            ("version", &self.version),
            ("min_hw_tier", &self.min_hw_tier),
        ];
        for (k, v) in named {
            if let Some(v) = v {
                rows.insert(k.to_string(), v.clone());
            }
        }
        for (k, v) in &self.extra {
            let s = match v {
                toml::Value::String(s) => s.clone(),
                toml::Value::Integer(i) => i.to_string(),
                toml::Value::Float(f) => f.to_string(),
                toml::Value::Boolean(b) => b.to_string(),
                toml::Value::Datetime(d) => d.to_string(),
                other => bail!(
                    "manifest key `{k}` has type {} — the manifest column is TEXT and \
                     Track B has not defined a nesting convention, so arrays/tables are \
                     rejected rather than encoded by guesswork (SPEC §5.6)",
                    other.type_str()
                ),
            };
            rows.insert(k.clone(), s);
        }
        Ok(rows)
    }

    /// B3 fields that are absent. Informational only.
    pub fn missing_b3(&self) -> Vec<&'static str> {
        let present = |k: &str| match k {
            "name" => self.name.is_some(),
            "icon" => self.icon.is_some(),
            "category" => self.category.is_some(),
            "license" => self.license.is_some(),
            "version" => self.version.is_some(),
            "min_hw_tier" => self.min_hw_tier.is_some(),
            _ => false,
        };
        Self::B3_FIELDS.iter().copied().filter(|k| !present(k)).collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
    /// Default codec for entries that are not covered by `store_uncompressed`.
    #[serde(default = "default_compression")]
    pub compression: String,
    /// Lower-case extensions stored verbatim. These are formats that are
    /// already entropy-coded, where zstd costs CPU and usually grows the blob.
    #[serde(default = "default_store_uncompressed")]
    pub store_uncompressed: Vec<String>,
    /// Glob-free prefix excludes, matched against the normalized entry path.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Default for BuildConfig {
    fn default() -> Self {
        BuildConfig {
            compression: default_compression(),
            store_uncompressed: default_store_uncompressed(),
            exclude: Vec::new(),
        }
    }
}

fn default_compression() -> String {
    "zstd".to_string()
}

/// Already-compressed container/media formats.
fn default_store_uncompressed() -> Vec<String> {
    [
        "png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "mp3", "aac", "ogg", "opus", "flac",
        "mp4", "m4a", "m4v", "webm", "mkv", "zip", "gz", "bz2", "xz", "zst", "7z", "rar", "woff",
        "woff2", "pdf",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl PackConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading pack config {}", path.display()))?;
        let cfg: PackConfig = toml::from_str(&text)
            .with_context(|| format!("parsing pack config {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Look for `wax-pack.toml` in the source tree root, then next to it.
    pub fn discover(input: &Path, explicit: Option<&Path>) -> Result<Self> {
        if let Some(p) = explicit {
            return Self::load(p);
        }
        let candidate = input.join("wax-pack.toml");
        if candidate.is_file() {
            return Self::load(&candidate);
        }
        Ok(PackConfig::default())
    }

    fn validate(&self) -> Result<()> {
        match self.build.compression.as_str() {
            "zstd" | "none" => {}
            other => bail!(
                "build.compression = {other:?} is not a v0.9 codec; SPEC §3 defines \
                 exactly \"none\" and \"zstd\""
            ),
        }
        Ok(())
    }
}
