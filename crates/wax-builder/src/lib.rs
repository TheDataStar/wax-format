//! `wax-builder` — component **A2**: assemble a directory tree into a signed
//! `.wax` archive.
//!
//! The on-disk contract is [`SPEC.md`](../../../SPEC.md) (WAX format v0.9);
//! this crate only orchestrates. Layout, the append-commit protocol and the
//! signable digest all live in `wax-core`.
//!
//! * [`config`] — `wax-pack.toml`: manifest rows, alias map, titles, compression policy.
//! * [`assemble`] — directory walk to `EntryInput`s, deterministically ordered.
//! * [`sign`] — A7 detached minisign sidecar (SPEC §8).
//!
//! [`build_pack`] and [`append_pack`] are the two write paths; both are exposed
//! as library functions so the conformance suite can drive them without a
//! subprocess.

pub mod assemble;
pub mod config;
pub mod report;
pub mod sign;

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use wax_core::header::flag;
use wax_core::{WaxReader, WaxWriter};

pub use assemble::WalkStats;
pub use config::PackConfig;
pub use report::{BuildReport, Warnings};

/// Options shared by [`build_pack`] and [`append_pack`].
#[derive(Debug, Default, Clone)]
pub struct WriteOptions {
    /// Pin the header's `created_at` (and each segment's `segment_meta.created_at`).
    /// Set this for reproducible builds; `None` uses wall-clock time.
    pub created_at: Option<u64>,
    /// Secret key for the A7 sidecar. `None` leaves the archive unsigned.
    pub sign_key: Option<PathBuf>,
    /// Pin the `archive_uuid` instead of minting a fresh UUIDv4.
    ///
    /// A fresh v4 UUID is random, so it is the one field that makes two builds
    /// of an identical tree differ byte-for-byte. Pin it (together with
    /// `created_at`) for reproducible builds; leave it `None` for the normal
    /// "this is a brand-new pack" case.
    pub archive_uuid: Option<[u8; 16]>,
}

impl WriteOptions {
    /// `created_at`, honouring `$SOURCE_DATE_EPOCH` when no explicit value is set.
    pub fn effective_created_at(&self) -> Option<u64> {
        if let Some(t) = self.created_at {
            return Some(t);
        }
        std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
    }

    /// The identity to stamp into a fresh build: the pinned value if given,
    /// otherwise a newly minted UUIDv4 (SPEC §2).
    pub fn mint_uuid(&self) -> [u8; 16] {
        self.archive_uuid
            .unwrap_or_else(|| *uuid::Uuid::new_v4().as_bytes())
    }
}

/// Parse an `archive_uuid` from either hyphenated UUID form or 32 bare hex
/// characters.
pub fn parse_uuid(s: &str) -> Result<[u8; 16]> {
    // Both the canonical hyphenated form and the bare 32-hex form are accepted
    // on input (Contract §11); only the canonical form is ever emitted.
    match uuid::Uuid::parse_str(s.trim()) {
        Ok(u) => Ok(*u.as_bytes()),
        Err(_) => bail!(
            "archive_uuid must be a UUID (canonical lowercase hyphenated, or bare 32 hex \
             digits), got {s:?}"
        ),
    }
}

/// What a build or append produced.
#[derive(Debug)]
pub struct WriteReport {
    pub archive: PathBuf,
    pub archive_uuid: [u8; 16],
    pub entries: usize,
    pub segments: usize,
    pub stats: WalkStats,
    pub sidecar: Option<PathBuf>,
    /// Licensing outcome (Contract §11). `None` when the pack declares no
    /// manifest. `license_review_required` is a build-report fact, never a
    /// manifest key — the catalog's intake reads it from here.
    pub license: Option<config::LicenseOutcome>,
    /// Redirect (alias) entries written.
    pub redirect_count: u64,
    /// Source items not written, as declared by the caller (a converter's
    /// skipped entries, dropped redirects, …).
    pub skipped_count: u64,
    /// Warnings by code, one entry per code (§11).
    pub warnings: Warnings,
    /// Where the §11 build report was written. Always written on success.
    pub report_path: PathBuf,
}

impl WriteReport {
    pub fn license_review_required(&self) -> bool {
        self.license.as_ref().is_some_and(|l| l.review_required())
    }

    /// Canonical text form of `archive_uuid`: lowercase hyphenated (§11).
    pub fn archive_uuid_text(&self) -> String {
        uuid_text(&self.archive_uuid)
    }
}

/// Canonical text form of an `archive_uuid` (Contract §11): lowercase
/// hyphenated 8-4-4-4-12. This is the only form any tool emits.
pub fn uuid_text(b: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*b).hyphenated().to_string()
}

/// What a caller of [`build_from_entries`] contributes to the §11 build
/// report beyond what the writer itself can see.
#[derive(Debug, Default, Clone)]
pub struct BuildContext {
    /// `"<tool> <version>"` for `builder_version`, e.g. `zim2wax 0.1.0`.
    /// Defaults to this crate's own name and version.
    pub builder_version: Option<String>,
    /// Warnings the caller accumulated while producing `entries`.
    pub warnings: Warnings,
    /// Source items the caller skipped (not present in `entries`).
    pub skipped_count: u64,
}

/// Fresh single-segment build: `input` tree + `cfg` — `output` archive.
///
/// Thin wrapper over [`build_from_entries`]: walks the tree, then hands the
/// entries and the config's `[manifest]` to the entries-based build.
pub fn build_pack(
    input: &Path,
    output: &Path,
    cfg: &PackConfig,
    opts: &WriteOptions,
) -> Result<WriteReport> {
    let (entries, stats) = assemble::collect_entries(input, cfg)?;
    let mut report = build_from_entries(
        output,
        entries,
        &cfg.manifest,
        opts,
        BuildContext::default(),
    )?;
    report.stats = stats;
    Ok(report)
}

/// Build a fresh single-segment archive from already-assembled entries.
///
/// This is the primitive a converter (zim2wax, warc2wax) calls: it owns entry
/// production, and this function owns manifest validation (Contract §11),
/// the write, signing, and the §11 build report, which is written beside the
/// archive on every success.
///
/// A new UUIDv4 `archive_uuid` is minted (SPEC §2) unless
/// [`WriteOptions::archive_uuid`] pins one.
pub fn build_from_entries(
    output: &Path,
    entries: Vec<wax_core::EntryInput>,
    manifest: &config::ManifestConfig,
    opts: &WriteOptions,
    ctx: BuildContext,
) -> Result<WriteReport> {
    let uuid = opts.mint_uuid();
    let n = entries.len();
    let redirect_count = entries
        .iter()
        .filter(|e| matches!(e.content, wax_core::EntryContent::Redirect { .. }))
        .count() as u64;

    // Validate the manifest before writing anything. `icon`/`entry_point` are
    // checked against the paths this build will actually contain.
    let declares_manifest = !manifest.is_empty();
    let license = if declares_manifest {
        let paths: BTreeSet<String> = entries
            .iter()
            .map(|e| wax_core::writer::normalize_path(&e.path).unwrap_or_else(|_| e.path.clone()))
            .collect();
        Some(
            manifest
                .validate(Some(&paths))
                .context("invalid manifest")?,
        )
    } else {
        None
    };

    // `is_signed` lives in the header and the header is inside the signed digest
    // (SPEC §8.1), so the flag has to be set before the archive is written —
    // not patched in afterwards.
    let mut flags = 0u16;
    if opts.sign_key.is_some() {
        flags |= flag::IS_SIGNED;
    }

    let mut writer = WaxWriter::new(uuid).flags(flags);
    if let Some(t) = opts.effective_created_at() {
        writer = writer.created_at(t);
    }
    let rows = if declares_manifest {
        manifest.to_rows()
    } else {
        BTreeMap::new()
    };
    writer
        .build(output, entries, &rows)
        .with_context(|| format!("writing {}", output.display()))?;

    let sidecar = maybe_sign(output, opts)?;
    let segments = WaxReader::open(output)?.segment_count();

    let mut report = WriteReport {
        archive: output.to_path_buf(),
        archive_uuid: uuid,
        entries: n,
        segments,
        stats: WalkStats::default(),
        sidecar,
        license,
        redirect_count,
        skipped_count: ctx.skipped_count,
        warnings: ctx.warnings,
        report_path: PathBuf::new(),
    };
    report.report_path = write_build_report(&report, ctx.builder_version.as_deref())?;
    Ok(report)
}

/// Render and write the §11 build report for `report`, beside its archive.
fn write_build_report(report: &WriteReport, builder_version: Option<&str>) -> Result<PathBuf> {
    let br = BuildReport {
        report_version: report::REPORT_VERSION,
        archive_uuid: report.archive_uuid_text(),
        archive_filename: report
            .archive
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        built_at: report::now_ms(),
        builder_version: builder_version
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))),
        entry_count: report.entries as u64,
        redirect_count: report.redirect_count,
        skipped_count: report.skipped_count,
        license: report
            .license
            .as_ref()
            .map(|l| l.license().to_string())
            .unwrap_or_default(),
        license_review_required: report.license_review_required(),
        signed: report.sidecar.is_some(),
        warnings: report.warnings.to_vec(),
    };
    br.write(&report.archive)
}

/// Append a new `(blob region, index segment)` pair to an existing archive
/// (SPEC §7.1), then re-sign, because the digest covers the new header and the
/// whole segment chain (SPEC §8.1).
///
/// The `archive_uuid` is **preserved** — an append is a new state of the same
/// logical pack, which is what a delta update matches against (SPEC §2).
///
/// Note this is the *segment-append* path, not the A6 delta/patch engine.
pub fn append_pack(
    archive: &Path,
    input: &Path,
    cfg: &PackConfig,
    opts: &WriteOptions,
) -> Result<WriteReport> {
    if !archive.is_file() {
        bail!("archive {} does not exist", archive.display());
    }
    // `manifest` is segment-0-only and immutable across appends (SPEC §5.6): a
    // manifest change is a new pack version, not an append.
    if !cfg.manifest.is_empty() {
        let existing = WaxReader::open(archive)?.manifest().clone();
        let declared = cfg.manifest.to_rows();
        if declared != existing {
            bail!(
                "this config's [manifest] differs from the archive's. The manifest lives \n                 only in segment 0 and is immutable across appends (SPEC §5.6) — a manifest \n                 change is a new pack version, so rebuild with `build` instead of `append`."
            );
        }
    }

    let (entries, stats) = assemble::collect_entries(input, cfg)?;
    let n = entries.len();

    let uuid = WaxReader::open(archive)?.header().archive_uuid;
    let mut writer = WaxWriter::new(uuid);
    if let Some(t) = opts.effective_created_at() {
        writer = writer.created_at(t);
    }
    writer
        .append(archive, entries)
        .with_context(|| format!("appending to {}", archive.display()))?;

    let sidecar = maybe_sign(archive, opts)?;
    let reader = WaxReader::open(archive)?;
    debug_assert_eq!(reader.header().archive_uuid, uuid);

    // The manifest is immutable across appends, so the licensing outcome is
    // whatever the archive already carries.
    let license = reader
        .manifest()
        .get("license")
        .map(|l| config::classify_license(l));

    let redirect_count = reader.entries().filter(|e| e.is_redirect()).count() as u64;
    let total = reader.entries().count();
    let mut report = WriteReport {
        archive: archive.to_path_buf(),
        archive_uuid: uuid,
        entries: total,
        segments: reader.segment_count(),
        stats,
        sidecar,
        license,
        redirect_count,
        skipped_count: 0,
        warnings: Warnings::new(),
        report_path: PathBuf::new(),
    };
    let _ = n;
    // An append changes the archive, so the report beside it is refreshed.
    report.report_path = write_build_report(&report, None)?;
    Ok(report)
}

fn maybe_sign(archive: &Path, opts: &WriteOptions) -> Result<Option<PathBuf>> {
    match &opts.sign_key {
        None => Ok(None),
        Some(key) => sign::sign(archive, key).map(Some),
    }
}

/// Result of `wax-builder verify`.
#[derive(Debug)]
pub struct VerifyReport {
    pub entries_checked: usize,
    pub redirects: usize,
    pub bad_entries: Vec<(String, String)>,
    pub signature: Option<sign::SignatureReport>,
    pub signature_error: Option<String>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.bad_entries.is_empty() && self.signature_error.is_none()
    }
}

/// Read every entry (which verifies its `sha256`, SPEC §3.1) and optionally
/// check the A7 sidecar.
pub fn verify_pack(
    archive: &Path,
    pubkey: Option<&sign::PubKey>,
    require_signature: bool,
) -> Result<VerifyReport> {
    let mut reader = WaxReader::open(archive)
        .with_context(|| format!("opening {}", archive.display()))?;

    let paths: Vec<String> = reader.paths().map(|s| s.to_string()).collect();
    let redirects = reader.entries().filter(|e| e.is_redirect()).count();

    let mut bad = Vec::new();
    let mut checked = 0usize;
    for p in &paths {
        match reader.read(p) {
            Ok(_) => checked += 1,
            Err(e) => bad.push((p.clone(), e.to_string())),
        }
    }

    let (signature, signature_error) = match pubkey {
        Some(pk) => match sign::verify(archive, pk) {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e.to_string())),
        },
        None => {
            let sidecar = sign::sidecar_path(archive);
            if require_signature {
                (None, Some(format!(
                    "--require-signature was given but no public key was supplied \
                     (use --pubkey/--pubkey-str or ${})",
                    sign::ENV_PUBKEY
                )))
            } else if sidecar.is_file() {
                (None, None) // sidecar present but unchecked; reported by the CLI
            } else if reader.header().has_flag(flag::IS_SIGNED) {
                (None, Some(format!(
                    "header sets is_signed but {} is missing",
                    sidecar.display()
                )))
            } else {
                (None, None)
            }
        }
    };

    Ok(VerifyReport {
        entries_checked: checked,
        redirects,
        bad_entries: bad,
        signature,
        signature_error,
    })
}
