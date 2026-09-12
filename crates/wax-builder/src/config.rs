//! Build-time pack configuration (`wax-pack.toml`).
//!
//! Supplies everything the directory walk cannot infer: the `manifest` rows, the
//! alias (redirect) map, per-entry titles, and the compression policy.
//!
//! # Manifest = Track B's B3 schema, enforced
//!
//! [`docs/track-a-refinement.md`](../../../docs/track-a-refinement.md) §16
//! reproduces Track B's B3 field table and states it is **exhaustive, not
//! illustrative** — `wax-builder` validates against it rather than passing keys
//! through. That is the change from the previous pass, where any key (and any
//! value) was accepted verbatim.
//!
//! * Required: `name`, `icon`, `category`, `license`, `attribution`, `version`,
//!   `min_hw_tier`, `entry_point`. `icon` and `entry_point` must name entries
//!   that exist in the pack (§17).
//! * Closed enums: [`CATEGORIES`], [`MIN_HW_TIERS`] — the latter is the
//!   Cross-Track Contract §2 tier axis.
//! * Optional and omitted-when-unset: `runtime_ram_bytes`,
//!   `runtime_storage_bytes`, `languages`, `depends_on`. `depends_on` is
//!   comma-separated `archive_uuid` values, each of which must parse as a UUID
//!   (§17; Track B §19) — whether the referenced pack exists is the catalog's
//!   question, not the builder's.
//! * Two keys are **removed from the schema** and rejected with a dedicated
//!   message: `id` (`archive_uuid` is the only identity a pack carries, §16)
//!   and `total_size_bytes` (self-referential and stale-on-append; the
//!   catalog's `packs.size` carries archive size instead, §17 / Track B §19).

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// `category` domain (§16). Closed set.
pub const CATEGORIES: [&str; 6] = ["reference", "education", "media", "tools", "civic", "health"];

/// `min_hw_tier` domain — Track E's E6 hardware-tier names (§16). Closed set.
pub const MIN_HW_TIERS: [&str; 4] = ["pi_zero_2w", "pi_4", "pi_5", "mini_pc"];

/// Deployment Profile names. A different axis entirely from the hardware tier;
/// an earlier draft used these for `min_hw_tier`, so they get a targeted error
/// rather than a generic "not in the list".
const DEPLOYMENT_PROFILES: [&str; 4] = ["kiosk", "classroom", "communityhub", "fieldops"];

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

/// The B3 manifest fields (§16). Unknown keys land in `unknown` and are rejected
/// by [`ManifestConfig::validate`] rather than silently written.
#[derive(Debug, Default, Deserialize)]
pub struct ManifestConfig {
    // --- required ---
    pub name: Option<String>,
    pub icon: Option<String>,
    pub category: Option<String>,
    pub license: Option<String>,
    pub attribution: Option<String>,
    pub version: Option<String>,
    pub min_hw_tier: Option<String>,
    pub entry_point: Option<String>,

    // --- optional; omitted entirely when unset, never defaulted to 0/"" ---
    pub runtime_ram_bytes: Option<i64>,
    pub runtime_storage_bytes: Option<i64>,
    pub languages: Option<String>,
    /// Comma-separated `archive_uuid` values. Validated for shape only.
    pub depends_on: Option<String>,

    // --- explicitly removed from the schema; captured so they can be
    //     rejected with a specific message rather than a generic "unknown" ---
    pub id: Option<toml::Value>,
    pub total_size_bytes: Option<toml::Value>,

    /// Anything not in the B3 table.
    #[serde(flatten)]
    pub unknown: BTreeMap<String, toml::Value>,
}

/// The eight required B3 fields, in table order.
pub const REQUIRED_FIELDS: [&str; 8] = [
    "name",
    "icon",
    "category",
    "license",
    "attribution",
    "version",
    "min_hw_tier",
    "entry_point",
];

impl ManifestConfig {
    fn required_pairs(&self) -> [(&'static str, &Option<String>); 8] {
        [
            ("name", &self.name),
            ("icon", &self.icon),
            ("category", &self.category),
            ("license", &self.license),
            ("attribution", &self.attribution),
            ("version", &self.version),
            ("min_hw_tier", &self.min_hw_tier),
            ("entry_point", &self.entry_point),
        ]
    }

    /// True when no manifest key was supplied at all. An empty `manifest` table
    /// is valid at the `wax-core` layer (§15) — enforcement applies to a pack
    /// that declares a manifest.
    pub fn is_empty(&self) -> bool {
        self.required_pairs().iter().all(|(_, v)| v.is_none())
            && self.total_size_bytes.is_none()
            && self.runtime_ram_bytes.is_none()
            && self.runtime_storage_bytes.is_none()
            && self.languages.is_none()
            && self.depends_on.is_none()
            && self.id.is_none()
            && self.unknown.is_empty()
    }

    /// Enforce §16. `archive_paths`, when supplied, additionally checks that
    /// `icon` and `entry_point` name entries that actually exist in the pack.
    pub fn validate(&self, archive_paths: Option<&BTreeSet<String>>) -> Result<()> {
        // Rejected keys first — they produce the most specific advice.
        if self.id.is_some() {
            bail!(
                "manifest key `id` was removed from the B3 schema: it duplicated \
                 `archive_uuid` without adding meaning, and `archive_uuid` (the header \
                 field) is the only identity a pack carries. Remove it from wax-pack.toml \
                 (see docs/track-a-refinement.md §16)."
            );
        }
        if self.total_size_bytes.is_some() {
            bail!(
                "manifest key `total_size_bytes` was removed from the B3 schema: a pack's \
                 size is measurable by anyone holding the file and is not the author's to \
                 declare, and a copy inside the archive is both self-referential and \
                 permanently stale after an append. Archive size lives in the on-device \
                 catalog (`packs.size`, populated at publish time). Remove it from \
                 wax-pack.toml (see docs/track-a-refinement.md §17, track-b-refinement.md §19)."
            );
        }
        if !self.unknown.is_empty() {
            let mut keys: Vec<&str> = self.unknown.keys().map(String::as_str).collect();
            keys.sort_unstable();
            bail!(
                "unknown manifest key(s): {}. The B3 field table is exhaustive, not a \
                 sample (docs/track-a-refinement.md §16); permitted keys are: {}",
                keys.join(", "),
                allowed_keys().join(", ")
            );
        }

        // Required presence.
        let missing: Vec<&str> = self
            .required_pairs()
            .iter()
            .filter(|(_, v)| v.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true))
            .map(|(k, _)| *k)
            .collect();
        if !missing.is_empty() {
            bail!(
                "manifest is missing required field(s): {}. All eight of {} are required \
                 by the B3 schema (docs/track-a-refinement.md §16)",
                missing.join(", "),
                REQUIRED_FIELDS.join(", ")
            );
        }

        // Closed enums.
        let category = self.category.as_deref().unwrap_or_default();
        if !CATEGORIES.contains(&category) {
            bail!(
                "manifest category {category:?} is not permitted; must be one of: {}",
                CATEGORIES.join(", ")
            );
        }
        let tier = self.min_hw_tier.as_deref().unwrap_or_default();
        if !MIN_HW_TIERS.contains(&tier) {
            let squashed: String = tier
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect();
            if DEPLOYMENT_PROFILES.contains(&squashed.as_str()) {
                bail!(
                    "manifest min_hw_tier {tier:?} is a Deployment Profile name, not a \
                     hardware tier — they are different axes. Use one of Track E's E6 \
                     tier names: {}",
                    MIN_HW_TIERS.join(", ")
                );
            }
            bail!(
                "manifest min_hw_tier {tier:?} is not permitted; must be one of: {}",
                MIN_HW_TIERS.join(", ")
            );
        }

        // depends_on: each element is an archive_uuid (§17). Shape only — the
        // referenced pack's existence is resolved catalog-side, not here.
        if let Some(raw) = &self.depends_on {
            for (i, element) in raw.split(',').enumerate() {
                let element = element.trim();
                if element.is_empty() {
                    bail!(
                        "manifest depends_on has an empty element at position {} — it must be \
                         a comma-separated list of archive_uuid values with no blanks",
                        i + 1
                    );
                }
                if uuid::Uuid::parse_str(element).is_err() {
                    bail!(
                        "manifest depends_on element {element:?} is not an archive_uuid. Each \
                         comma-separated element must be a UUID (the depended-on pack's \
                         archive_uuid, as shown by `wax-builder inspect`); pack names and \
                         other identifiers are not accepted (docs/track-a-refinement.md §17)"
                    );
                }
            }
        }

        // icon / entry_point name entries inside the pack.
        if let Some(paths) = archive_paths {
            for (key, value) in [
                ("icon", self.icon.as_deref().unwrap_or_default()),
                ("entry_point", self.entry_point.as_deref().unwrap_or_default()),
            ] {
                if let Some(scheme) = uri_scheme(value) {
                    bail!(
                        "manifest {key} {value:?} looks like a {scheme} URI; it must be a \
                         path to an entry inside the archive"
                    );
                }
                let normalized = crate::assemble::normalize_for_lookup(value);
                if !paths.contains(&normalized) {
                    bail!(
                        "manifest {key} {value:?} does not name an entry in this pack. It \
                         must be a path within the archive (docs/track-a-refinement.md §16)"
                    );
                }
            }
        }
        Ok(())
    }

    /// Flatten to the `manifest` rows written into segment 0.
    ///
    /// Optional fields that are unset are **omitted**, not written as `0` / `""`
    /// (§16). Values are written exactly as configured — `depends_on` in
    /// particular is not re-serialized; see the note on UUID spelling in the
    /// README.
    pub fn to_rows(&self) -> BTreeMap<String, String> {
        let mut rows = BTreeMap::new();
        for (k, v) in self.required_pairs() {
            if let Some(v) = v {
                rows.insert(k.to_string(), v.clone());
            }
        }
        if let Some(v) = self.runtime_ram_bytes {
            rows.insert("runtime_ram_bytes".to_string(), v.to_string());
        }
        if let Some(v) = self.runtime_storage_bytes {
            rows.insert("runtime_storage_bytes".to_string(), v.to_string());
        }
        if let Some(v) = &self.languages {
            rows.insert("languages".to_string(), v.clone());
        }
        if let Some(v) = &self.depends_on {
            rows.insert("depends_on".to_string(), v.clone());
        }
        rows
    }
}

/// Every key the B3 table permits, for error messages.
pub fn allowed_keys() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = REQUIRED_FIELDS.to_vec();
    v.extend([
        "runtime_ram_bytes",
        "runtime_storage_bytes",
        "languages",
        "depends_on",
    ]);
    v.sort_unstable();
    v
}

fn uri_scheme(value: &str) -> Option<&'static str> {
    let lower = value.to_ascii_lowercase();
    for scheme in ["data:", "http://", "https://", "file://"] {
        if lower.starts_with(scheme) {
            return Some(match scheme {
                "data:" => "data",
                "file://" => "file",
                _ => "http",
            });
        }
    }
    None
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
    /// Path prefixes, matched against the normalized entry path, to exclude.
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
        cfg.validate_build()?;
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

    fn validate_build(&self) -> Result<()> {
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
