//! The recipe: every choice that shapes a pack (WSS product spec, "Core
//! concepts"). A recipe, not a click history: the same recipe over the same
//! sources gives the same bytes, so it is what a GUI saves, a SaaS job queue
//! stores and a scheduled refresh re-runs.
//!
//! Serialised as TOML (`wss.toml`). The CLI builds one from its flags and can
//! also read one with `--recipe`.
//!
//! ## What has no default, on purpose
//!
//! The project rule is "refuse rather than guess". So `category`,
//! `min_ram_bytes`, `min_storage_bytes` and `arch` (Contract §2.3, §11) are
//! required for every source, and `license`, `attribution`, `name` and
//! `version` are required whenever the source itself does not state them.
//! [`Recipe::validate`] checks the source-independent part before any work
//! starts; each importer checks the rest once it knows what its source says.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use wax_builder::config::{ARCHES, CATEGORIES, GPU_MODES};

/// ADR-0003: build at zstd level 19 off-box (16% smaller text than level 3,
/// no decode cost). WSS runs on a desktop or server, so this is its default.
pub const DEFAULT_ZSTD_LEVEL: i32 = 19;

/// The whole recipe.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    #[serde(default)]
    pub pack: PackFacts,
    #[serde(default)]
    pub build: BuildSettings,
    #[serde(default)]
    pub crawl: CrawlSettings,
}

/// What the pack declares about itself (Contract §11 manifest fields).
///
/// `license`, `attribution`, `name` and `languages` are used **only when the
/// source states none** — a ZIM's `License`, an EPUB's `dc:rights`. A value
/// the source states always wins, and an operator-supplied license is always
/// reviewed rather than trusted (Contract §11).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackFacts {
    pub name: Option<String>,
    /// CalVer `YYYY.MM.N` (Contract §11). Required unless the source states
    /// an edition date (ZIM `Date`).
    pub version: Option<String>,
    /// One of the Contract §11 categories. Required.
    pub category: Option<String>,
    pub license: Option<String>,
    pub attribution: Option<String>,
    /// Comma-separated BCP-47 tags.
    pub languages: Option<String>,
    /// Path inside the pack to open first. Defaults per importer: the source's
    /// own start page when it has one (`index.html`, the generated contents
    /// page of an EPUB, the crawl's start URL).
    pub entry_point: Option<String>,
    /// Contract §2.3. Required.
    pub min_ram_bytes: Option<i64>,
    /// Contract §2.3. Required.
    pub min_storage_bytes: Option<i64>,
    /// Contract §2.3: `aarch64`, `x86_64` or `any`. Required.
    pub arch: Option<String>,
    /// Contract §2.3: `required` or `preferred`; omit when irrelevant.
    pub gpu: Option<String>,
    pub guest_accessible: Option<bool>,
}

/// How the pack is built. None of these change what a reader must understand.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct BuildSettings {
    /// zstd level, 1..=19 (SPEC §3 window bound). Default 19 (ADR-0003).
    pub zstd_level: i32,
    /// Build the embedded search index and passages (SPEC §5.7, §5.10).
    pub search_index: bool,
    /// Pin the header's `created_at` for reproducible builds. Also
    /// `$SOURCE_DATE_EPOCH`.
    pub created_at: Option<u64>,
    /// Pin the pack identity. Required to build a later edition of an
    /// existing pack (its `archive_uuid` never changes, SPEC §2).
    pub archive_uuid: Option<String>,
    /// minisign secret key. Also `$WAX_MINISIGN_KEY`.
    pub sign_key: Option<PathBuf>,
    /// The previous edition, whose shared dictionary is carried forward
    /// verbatim (SPEC §5.9). Set automatically by `wss update`.
    pub carry_dictionary_from: Option<PathBuf>,
}

impl Default for BuildSettings {
    fn default() -> Self {
        BuildSettings {
            zstd_level: DEFAULT_ZSTD_LEVEL,
            search_index: true,
            created_at: None,
            archive_uuid: None,
            sign_key: None,
            carry_dictionary_from: None,
        }
    }
}

/// Website capture (the URL importer). These are tool settings the person
/// chooses, not format or contract values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct CrawlSettings {
    /// Stop after this many fetched URLs.
    pub max_pages: usize,
    /// Links followed away from the start page.
    pub max_depth: usize,
    /// Only URLs whose path starts with this are captured. Default: the
    /// directory of the start URL.
    pub path_prefix: Option<String>,
    /// Skip URLs whose path contains any of these.
    pub exclude: Vec<String>,
    /// Minimum pause between requests, in milliseconds. A site's robots.txt
    /// `Crawl-delay` raises it, never lowers it.
    pub delay_ms: u64,
    /// Per-request timeout, in seconds.
    pub timeout_secs: u64,
}

impl Default for CrawlSettings {
    fn default() -> Self {
        CrawlSettings {
            max_pages: 1000,
            max_depth: 8,
            path_prefix: None,
            exclude: Vec::new(),
            delay_ms: 1000,
            timeout_secs: 30,
        }
    }
}

impl Recipe {
    /// Read a `wss.toml`.
    pub fn from_toml(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// The source-independent checks, with the Contract's domains in the
    /// message, before any source is opened.
    pub fn validate(&self) -> Result<()> {
        let p = &self.pack;
        match p.category.as_deref() {
            None => bail!(
                "category is required: one of {} (Contract §11). No source states it, so WSS \
                 never picks one for you.",
                CATEGORIES.join(", ")
            ),
            Some(c) if !CATEGORIES.contains(&c) => {
                bail!("category {c:?} is not permitted; must be one of: {} (Contract §11)", CATEGORIES.join(", "))
            }
            _ => {}
        }
        match p.arch.as_deref() {
            None => bail!(
                "arch is required: one of {} (Contract §2.3). Converted web content is normally \
                 architecture-independent: any.",
                ARCHES.join(", ")
            ),
            Some(a) if !ARCHES.contains(&a) => {
                bail!("arch {a:?} is not permitted; must be one of: {} (Contract §2.3)", ARCHES.join(", "))
            }
            _ => {}
        }
        if let Some(g) = p.gpu.as_deref() {
            if !GPU_MODES.contains(&g) {
                bail!("gpu {g:?} is not permitted; must be one of: {} (Contract §2.3)", GPU_MODES.join(", "));
            }
        }
        for (key, v) in [("min_ram_bytes", p.min_ram_bytes), ("min_storage_bytes", p.min_storage_bytes)] {
            match v {
                None => bail!(
                    "{key} is required (Contract §2.3): the pack's resource floor in bytes. No \
                     source states it, so WSS never guesses it."
                ),
                Some(n) if n <= 0 => bail!("{key} is {n}; it must be a positive byte count (Contract §2.3)"),
                _ => {}
            }
        }
        if let Some(v) = p.version.as_deref() {
            if let Err(why) = wax_builder::config::check_calver(v) {
                bail!("version {v:?} is not CalVer YYYY.MM.N: {why} (Contract §11)");
            }
        }
        wax_core::writer::check_zstd_level(self.build.zstd_level)?;
        if let Some(u) = self.build.archive_uuid.as_deref() {
            wax_builder::parse_uuid(u)?;
        }
        Ok(())
    }

    /// `created_at`, honouring `$SOURCE_DATE_EPOCH`.
    pub fn created_at(&self) -> Option<u64> {
        self.build
            .created_at
            .or_else(|| std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|v| v.trim().parse().ok()))
    }

    pub fn archive_uuid(&self) -> Result<Option<[u8; 16]>> {
        self.build.archive_uuid.as_deref().map(wax_builder::parse_uuid).transpose()
    }

    pub fn sign_key(&self) -> Option<PathBuf> {
        wax_builder::sign::resolve_seckey(self.build.sign_key.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Recipe {
        let mut r = Recipe::default();
        r.pack.category = Some("education".into());
        r.pack.arch = Some("any".into());
        r.pack.min_ram_bytes = Some(1);
        r.pack.min_storage_bytes = Some(1);
        r
    }

    #[test]
    fn defaults_follow_adr_0003() {
        assert_eq!(Recipe::default().build.zstd_level, 19);
        assert!(Recipe::default().build.search_index);
    }

    #[test]
    fn required_facts_have_no_default() {
        assert!(ok().validate().is_ok());
        for strip in 0..4 {
            let mut r = ok();
            match strip {
                0 => r.pack.category = None,
                1 => r.pack.arch = None,
                2 => r.pack.min_ram_bytes = None,
                _ => r.pack.min_storage_bytes = None,
            }
            assert!(r.validate().unwrap_err().to_string().contains("required"));
        }
        let mut r = ok();
        r.pack.version = Some("2026.1.1".into());
        assert!(r.validate().is_err());
        let mut r = ok();
        r.build.zstd_level = 22;
        assert!(r.validate().is_err());
    }

    #[test]
    fn toml_round_trip() {
        let mut r = ok();
        r.crawl.exclude = vec!["/private/".into()];
        let back = Recipe::from_toml(&r.to_toml().unwrap()).unwrap();
        assert_eq!(back, r);
        assert!(Recipe::from_toml("[pack]\nbogus = 1\n").is_err());
    }
}
