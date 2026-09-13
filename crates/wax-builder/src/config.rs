//! Build-time pack configuration (`wax-pack.toml`).
//!
//! Supplies everything the directory walk cannot infer: the `manifest` rows, the
//! alias (redirect) map, per-entry titles, and the compression policy.
//!
//! # Manifest = Cross-Track Contract §11, enforced
//!
//! The B3 field list lives in exactly one place:
//! [`docs/cross-track-contract.md`](../../../docs/cross-track-contract.md) §11.
//! Track A §16 and Track B §2 cite it rather than restating it, and this module
//! enforces it — presence, requiredness, value domain and format.
//!
//! * **Eight required:** `name`, `icon`, `category`, `license`, `attribution`,
//!   `version`, `min_hw_tier`, `entry_point`. `icon` and `entry_point` must name
//!   entries that exist in the pack.
//! * **Five optional**, omitted when unset rather than defaulted:
//!   `guest_accessible` (bool), `runtime_ram_bytes`, `runtime_storage_bytes`,
//!   `languages` (BCP-47), `depends_on` (archive_uuids).
//! * **Closed enums:** [`CATEGORIES`]; [`MIN_HW_TIERS`], the Contract §2 board
//!   tiers — never `generic` (box-reported only) and never a Deployment Profile.
//! * **Formats:** `version` is CalVer `YYYY.MM.N`; each `languages` element is
//!   a well-formed BCP-47 tag; each `depends_on` element is a UUID, normalized
//!   to the canonical lowercase-hyphenated form on write.
//! * **Licensing has three outcomes**, not two ([`LicenseOutcome`]): blank fails
//!   the build; an allowlisted SPDX id builds clean; anything else builds with
//!   `license_review_required` in the build report — never in the manifest.
//! * **Nothing else.** Any key outside §11's table is a build error. `id` and
//!   `total_size_bytes` were removed from the schema and get dedicated messages.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// `category` domain (Contract §11). Closed set.
pub const CATEGORIES: [&str; 6] = ["reference", "education", "media", "tools", "civic", "health"];

/// `min_hw_tier` domain — the four board tiers of Contract §2. Closed set.
/// `generic` is a value a box reports, never one a pack declares.
pub const MIN_HW_TIERS: [&str; 4] = ["pi_zero_2w", "pi_4", "pi_5", "mini_pc"];

/// Deployment Profile names. A different axis entirely from the hardware tier;
/// an earlier draft used these for `min_hw_tier`, so they get a targeted error.
const DEPLOYMENT_PROFILES: [&str; 4] = ["kiosk", "classroom", "communityhub", "fieldops"];

/// SPDX identifiers that build clean (Contract §11). Literal, exact-match.
pub const LICENSE_ALLOWLIST: [&str; 10] = [
    "CC0-1.0",
    "CC-BY-4.0",
    "CC-BY-SA-3.0",
    "CC-BY-SA-4.0",
    "GFDL-1.3-or-later",
    "MIT",
    "Apache-2.0",
    "GPL-2.0-only",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
];

/// The eight required §11 fields, in table order.
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

/// The five optional §11 fields, in table order.
pub const OPTIONAL_FIELDS: [&str; 5] = [
    "guest_accessible",
    "runtime_ram_bytes",
    "runtime_storage_bytes",
    "languages",
    "depends_on",
];

/// One of the three licensing outcomes (Contract §11). The blank case is not
/// represented here because it is a hard failure, not an outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenseOutcome {
    /// The license is on [`LICENSE_ALLOWLIST`]; the pack builds clean.
    Clean { license: String },
    /// Free text, or a recognized-but-not-allowlisted identifier. The pack
    /// builds, and the build report carries `license_review_required` so the
    /// catalog's intake routes it to human review.
    ReviewRequired { license: String },
}

impl LicenseOutcome {
    pub fn review_required(&self) -> bool {
        matches!(self, LicenseOutcome::ReviewRequired { .. })
    }
    pub fn license(&self) -> &str {
        match self {
            LicenseOutcome::Clean { license } | LicenseOutcome::ReviewRequired { license } => license,
        }
    }
}

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

/// The §11 manifest fields. Unknown keys land in `unknown` and are rejected by
/// [`ManifestConfig::validate`] rather than silently written.
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

    // --- optional; omitted entirely when unset, never defaulted ---
    /// Author's default for Guest-profile visibility. The admin override lives
    /// in the catalog, not here (§11).
    pub guest_accessible: Option<bool>,
    pub runtime_ram_bytes: Option<i64>,
    pub runtime_storage_bytes: Option<i64>,
    /// Comma-separated BCP-47 tags.
    pub languages: Option<String>,
    /// Comma-separated `archive_uuid` values. Normalized on write.
    pub depends_on: Option<String>,

    // --- explicitly removed from the schema; captured so they can be
    //     rejected with a specific message rather than a generic "unknown" ---
    pub id: Option<toml::Value>,
    pub total_size_bytes: Option<toml::Value>,

    /// Anything not in the §11 table.
    #[serde(flatten)]
    pub unknown: BTreeMap<String, toml::Value>,
}

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
    /// is valid at the `wax-core` layer (Track A §15) — enforcement applies to a
    /// pack that declares a manifest.
    pub fn is_empty(&self) -> bool {
        self.required_pairs().iter().all(|(_, v)| v.is_none())
            && self.guest_accessible.is_none()
            && self.runtime_ram_bytes.is_none()
            && self.runtime_storage_bytes.is_none()
            && self.languages.is_none()
            && self.depends_on.is_none()
            && self.id.is_none()
            && self.total_size_bytes.is_none()
            && self.unknown.is_empty()
    }

    /// Enforce Contract §11. `archive_paths`, when supplied, additionally checks
    /// that `icon` and `entry_point` name entries that actually exist.
    ///
    /// Returns the licensing outcome on success; a blank license is an error.
    pub fn validate(&self, archive_paths: Option<&BTreeSet<String>>) -> Result<LicenseOutcome> {
        // Removed keys first — they produce the most specific advice.
        if self.id.is_some() {
            bail!(
                "manifest key `id` was removed from the B3 schema: it duplicated \
                 `archive_uuid` without adding meaning, and `archive_uuid` (the header \
                 field) is the only identity a pack carries. Remove it from wax-pack.toml \
                 (see docs/cross-track-contract.md §11)."
            );
        }
        if self.total_size_bytes.is_some() {
            bail!(
                "manifest key `total_size_bytes` was removed from the B3 schema: a pack's \
                 size is measurable by anyone holding the file and is not the author's to \
                 declare, and a copy inside the archive is both self-referential and \
                 permanently stale after an append. Archive size lives in the on-device \
                 catalog (`packs.size`, populated at publish time). Remove it from \
                 wax-pack.toml (see docs/cross-track-contract.md §11, track-a-refinement.md §17)."
            );
        }
        if !self.unknown.is_empty() {
            let mut keys: Vec<&str> = self.unknown.keys().map(String::as_str).collect();
            keys.sort_unstable();
            bail!(
                "unknown manifest key(s): {}. Contract §11's field table is exhaustive — \
                 eight required, five optional, nothing else; permitted keys are: {}",
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
                 (docs/cross-track-contract.md §11)",
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
            if squashed == "generic" {
                bail!(
                    "manifest min_hw_tier {tier:?}: `generic` is a tier a box reports about \
                     itself, never one a pack declares — a floor of \"declared\" cannot gate \
                     anything (docs/cross-track-contract.md §2). Declare the lowest board \
                     tier the pack runs on: {}",
                    MIN_HW_TIERS.join(", ")
                );
            }
            if DEPLOYMENT_PROFILES.contains(&squashed.as_str()) {
                bail!(
                    "manifest min_hw_tier {tier:?} is a Deployment Profile name, not a \
                     hardware tier — they are different axes (docs/cross-track-contract.md \
                     §2–§3). Use one of the board tiers: {}",
                    MIN_HW_TIERS.join(", ")
                );
            }
            bail!(
                "manifest min_hw_tier {tier:?} is not permitted; must be one of: {}",
                MIN_HW_TIERS.join(", ")
            );
        }

        // version: CalVer YYYY.MM.N (Contract §11).
        let version = self.version.as_deref().unwrap_or_default();
        if let Err(why) = check_calver(version) {
            bail!(
                "manifest version {version:?} is not CalVer: {why}. The form is YYYY.MM.N, \
                 e.g. 2026.09.1 — a four-digit year, a two-digit month 01–12, and a release \
                 number (docs/cross-track-contract.md §11). Semver is not accepted."
            );
        }

        // languages: each element a well-formed BCP-47 tag (Contract §11).
        if let Some(raw) = &self.languages {
            for (i, element) in raw.split(',').enumerate() {
                let element = element.trim();
                if element.is_empty() {
                    bail!(
                        "manifest languages has an empty element at position {} — it must be \
                         a comma-separated list of BCP-47 tags with no blanks",
                        i + 1
                    );
                }
                // Well-formed (RFC 5646 syntax) *and* valid (primary language
                // subtag is in the IANA registry): "english" is syntactically a
                // legal 7-letter subtag but names no registered language.
                let tag = language_tags::LanguageTag::parse(element).map_err(|e| {
                    anyhow::anyhow!(
                        "manifest languages element {element:?} is not a well-formed BCP-47 \
                         tag ({e}). Examples: en, pt-BR, zh-Hans (docs/cross-track-contract.md §11)"
                    )
                })?;
                if let Err(e) = tag.validate() {
                    bail!(
                        "manifest languages element {element:?} is not a valid BCP-47 tag \
                         ({e}). Use registered subtags: en, pt-BR, zh-Hans, sw, es-419 \
                         (docs/cross-track-contract.md §11)"
                    );
                }
            }
        }

        // depends_on: each element an archive_uuid (Contract §11). Shape only —
        // the referenced pack's existence is resolved catalog-side, not here.
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
                         other identifiers are not accepted (docs/cross-track-contract.md §11)"
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
                         must be a path within the archive (docs/cross-track-contract.md §11)"
                    );
                }
            }
        }

        Ok(self.license_outcome())
    }

    /// Classify the license per Contract §11. Assumes presence was already
    /// checked (a blank license is a hard failure in [`validate`]).
    pub fn license_outcome(&self) -> LicenseOutcome {
        classify_license(self.license.as_deref().unwrap_or_default())
    }

    /// Flatten to the `manifest` rows written into segment 0.
    ///
    /// Optional fields that are unset are **omitted**, not written as `0` /
    /// `""` / `false`. `depends_on` elements are normalized to the canonical
    /// lowercase-hyphenated UUID form (§11); other values are written as
    /// configured. Call only after [`validate`] has succeeded.
    pub fn to_rows(&self) -> BTreeMap<String, String> {
        let mut rows = BTreeMap::new();
        for (k, v) in self.required_pairs() {
            if let Some(v) = v {
                rows.insert(k.to_string(), v.clone());
            }
        }
        // An allowlisted license is written in SPDX's canonical casing (§11).
        if let Some(l) = &self.license {
            rows.insert("license".to_string(), classify_license(l).license().to_string());
        }
        if let Some(v) = self.guest_accessible {
            rows.insert("guest_accessible".to_string(), v.to_string());
        }
        if let Some(v) = self.runtime_ram_bytes {
            rows.insert("runtime_ram_bytes".to_string(), v.to_string());
        }
        if let Some(v) = self.runtime_storage_bytes {
            rows.insert("runtime_storage_bytes".to_string(), v.to_string());
        }
        if let Some(v) = &self.languages {
            let cleaned: Vec<String> = v.split(',').map(|e| e.trim().to_string()).collect();
            rows.insert("languages".to_string(), cleaned.join(","));
        }
        if let Some(v) = &self.depends_on {
            let canonical: Vec<String> = v
                .split(',')
                .map(|e| e.trim())
                .filter_map(|e| uuid::Uuid::parse_str(e).ok())
                .map(|u| u.hyphenated().to_string())
                .collect();
            rows.insert("depends_on".to_string(), canonical.join(","));
        }
        rows
    }
}

/// Contract §11 licensing outcome for a license string.
///
/// Allowlist matching is **case-insensitive**; a match is reported in SPDX's
/// canonical casing (the allowlist spelling), which is also what
/// [`ManifestConfig::to_rows`] writes. SPDX itself treats identifiers as
/// case-insensitive, and exact matching would have sent every
/// `cc-by-sa-4.0`-licensed Wikipedia pack to a moderator over letter case.
pub fn classify_license(raw: &str) -> LicenseOutcome {
    let trimmed = raw.trim();
    match LICENSE_ALLOWLIST
        .iter()
        .find(|id| id.eq_ignore_ascii_case(trimmed))
    {
        Some(canonical) => LicenseOutcome::Clean {
            license: canonical.to_string(),
        },
        None => LicenseOutcome::ReviewRequired {
            license: trimmed.to_string(),
        },
    }
}

/// Every key §11 permits, for error messages.
pub fn allowed_keys() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = REQUIRED_FIELDS.to_vec();
    v.extend(OPTIONAL_FIELDS);
    v.sort_unstable();
    v
}

/// CalVer `YYYY.MM.N` (Contract §11): four-digit year, zero-padded month 01—12,
/// release counter starting at 1 with no leading zero.
pub fn check_calver(s: &str) -> std::result::Result<(), &'static str> {
    let mut parts = s.split('.');
    let (Some(y), Some(m), Some(n), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("expected exactly three dot-separated components");
    };
    if y.len() != 4 || !y.bytes().all(|b| b.is_ascii_digit()) {
        return Err("year must be four digits");
    }
    if m.len() != 2 || !m.bytes().all(|b| b.is_ascii_digit()) {
        return Err("month must be two digits");
    }
    match m.parse::<u8>() {
        Ok(1..=12) => {}
        _ => return Err("month must be 01–12"),
    }
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return Err("release number must be digits");
    }
    if n.starts_with('0') {
        return Err(if n == "0" {
            "release number starts at 1"
        } else {
            "release number must not have a leading zero"
        });
    }
    Ok(())
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
