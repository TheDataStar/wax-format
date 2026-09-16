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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wax_core::header::flag;
use wax_core::{WaxReader, WaxWriter};

pub use assemble::WalkStats;
pub use config::PackConfig;
pub use report::{BuildReport, Warnings};
pub use wax_core::{EntryMeta, EntryStats};

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
    /// Report `license_review_required` regardless of the allowlist. Contract
    /// §11: an operator-supplied license is reviewed rather than trusted, even
    /// when the id it names would otherwise build clean.
    pub force_license_review: bool,
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
/// Thin wrapper over [`PackStream`] for callers that already hold their
/// entries in memory. Converters should use [`PackStream`] directly so that
/// peak memory does not scale with the archive (Track A §18).
pub fn build_from_entries(
    output: &Path,
    entries: Vec<wax_core::EntryInput>,
    manifest: &config::ManifestConfig,
    opts: &WriteOptions,
    ctx: BuildContext,
) -> Result<WriteReport> {
    let mut stream = PackStream::create(output, manifest, opts, ctx)?;
    for e in entries {
        match e.content {
            wax_core::EntryContent::Data { bytes, compression } => {
                let meta = wax_core::EntryMeta {
                    path: e.path,
                    mime: e.mime,
                    title: e.title,
                    compression: Some(compression),
                };
                stream.add_entry(meta, &mut std::io::Cursor::new(bytes))?;
            }
            wax_core::EntryContent::Redirect { to } => {
                stream.add_redirect(e.path, to, e.title)?;
            }
        }
    }
    stream.finish()
}

/// A pack being built by streaming (Track A §18): the primitive a converter
/// drives. Owns manifest validation (Contract §11), signing, and the §11
/// build report; entries are handed to [`PackStream::add_entry`] one at a time
/// as a `Read` and never held in memory.
///
/// Manifest format rules are checked at `create`, before any bytes are
/// written; the two rules that need the archive's contents (§11: `icon` and
/// `entry_point` must name real entries) are point lookups in the index at
/// `finish`, so no path set is ever held in memory.
pub struct PackStream {
    output: PathBuf,
    writer: wax_core::StreamingWriter,
    manifest: Option<config::ManifestConfig>,
    license: Option<config::LicenseOutcome>,
    opts: WriteOptions,
    ctx: BuildContext,
    archive_uuid: [u8; 16],
}

impl PackStream {
    pub fn create(
        output: &Path,
        manifest: &config::ManifestConfig,
        opts: &WriteOptions,
        ctx: BuildContext,
    ) -> Result<Self> {
        let uuid = opts.mint_uuid();
        let declares_manifest = !manifest.is_empty();
        // Format/domain validation now; the existence checks come at finish.
        let license = if declares_manifest {
            Some(manifest.validate(None).context("invalid manifest")?)
        } else {
            None
        };

        // `is_signed` lives in the header and the header is inside the signed
        // digest (SPEC §8.1), so the flag has to be set before writing.
        let mut flags = 0u16;
        if opts.sign_key.is_some() {
            flags |= flag::IS_SIGNED;
        }
        let mut w = WaxWriter::new(uuid).flags(flags);
        if let Some(t) = opts.effective_created_at() {
            w = w.created_at(t);
        }
        let rows = if declares_manifest {
            manifest.to_rows()
        } else {
            BTreeMap::new()
        };
        let writer = w
            .create(output, &rows)
            .with_context(|| format!("creating {}", output.display()))?;

        Ok(PackStream {
            output: output.to_path_buf(),
            writer,
            manifest: declares_manifest.then(|| clone_manifest(manifest)),
            license,
            opts: WriteOptions {
                created_at: opts.created_at,
                sign_key: opts.sign_key.clone(),
                archive_uuid: Some(uuid),
            },
            ctx,
            archive_uuid: uuid,
        })
    }

    /// Stream one content entry from `src`.
    pub fn add_entry(
        &mut self,
        meta: wax_core::EntryMeta,
        src: &mut dyn std::io::Read,
    ) -> Result<wax_core::EntryStats> {
        Ok(self.writer.add_entry(meta, src)?)
    }

    /// Add a redirect (no blob). Chains are flattened at `finish`.
    pub fn add_redirect(
        &mut self,
        path: impl Into<String>,
        to: impl Into<String>,
        title: Option<String>,
    ) -> Result<()> {
        Ok(self.writer.add_redirect(path, to, title)?)
    }

    /// Point lookup: has this path been added?
    pub fn has_path(&self, path: &str) -> Result<bool> {
        Ok(self.writer.has_path(path)?)
    }

    /// Caller-side warnings accumulated while producing entries.
    pub fn warnings_mut(&mut self) -> &mut Warnings {
        &mut self.ctx.warnings
    }

    /// Record how many source items were dropped (for `skipped_count`).
    pub fn set_skipped(&mut self, n: u64) {
        self.ctx.skipped_count = n;
    }

    /// Report `license_review_required` regardless of the allowlist.
    pub fn force_license_review(&mut self) {
        self.ctx.force_license_review = true;
    }

    /// Validate the contents-dependent manifest rules, commit the archive,
    /// sign, and write the §11 build report.
    pub fn finish(self) -> Result<WriteReport> {
        let PackStream {
            output,
            writer,
            manifest,
            license,
            opts,
            ctx,
            archive_uuid,
        } = self;

        // §11: icon and entry_point must resolve to real entries. Two point
        // lookups in the index, done before the archive is committed.
        if let Some(m) = &manifest {
            for (key, value) in [("icon", m.icon.as_deref()), ("entry_point", m.entry_point.as_deref())] {
                let value = value.unwrap_or_default();
                let normalized = assemble::normalize_for_lookup(value);
                if !writer.resolves(&normalized)? {
                    bail!(
                        "invalid manifest: {key} {value:?} does not name an entry in this pack. \
                         It must be a path within the archive (docs/cross-track-contract.md §11)"
                    );
                }
            }
        }

        let stats = writer
            .finish()
            .with_context(|| format!("writing {}", output.display()))?;

        let sidecar = maybe_sign(&output, &opts)?;

        let license = if ctx.force_license_review {
            license.map(|l| config::LicenseOutcome::ReviewRequired {
                license: l.license().to_string(),
            })
        } else {
            license
        };

        let mut report = WriteReport {
            archive: output,
            archive_uuid,
            entries: (stats.entries + stats.redirects) as usize,
            // a fresh build is exactly one segment; no reader open needed
            segments: 1,
            stats: WalkStats::default(),
            sidecar,
            license,
            redirect_count: stats.redirects,
            skipped_count: ctx.skipped_count,
            warnings: ctx.warnings,
            report_path: PathBuf::new(),
        };
        report.report_path = write_build_report(&report, ctx.builder_version.as_deref())?;
        Ok(report)
    }
}

fn clone_manifest(m: &config::ManifestConfig) -> config::ManifestConfig {
    config::ManifestConfig {
        name: m.name.clone(),
        icon: m.icon.clone(),
        category: m.category.clone(),
        license: m.license.clone(),
        attribution: m.attribution.clone(),
        version: m.version.clone(),
        min_ram_bytes: m.min_ram_bytes,
        min_storage_bytes: m.min_storage_bytes,
        arch: m.arch.clone(),
        entry_point: m.entry_point.clone(),
        guest_accessible: m.guest_accessible,
        gpu: m.gpu.clone(),
        runtime_ram_bytes: m.runtime_ram_bytes,
        runtime_storage_bytes: m.runtime_storage_bytes,
        languages: m.languages.clone(),
        depends_on: m.depends_on.clone(),
        id: m.id.clone(),
        total_size_bytes: m.total_size_bytes.clone(),
        min_hw_tier: m.min_hw_tier.clone(),
        unknown: m.unknown.clone(),
    }
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

    // One streaming pass over the merged index: bounded memory however many
    // entries the archive now carries.
    let mut redirect_count = 0u64;
    let mut total = 0usize;
    for e in reader.entries() {
        if e?.is_redirect() {
            redirect_count += 1;
        }
        total += 1;
    }
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
/// check the A7 sidecar. Entries are streamed from the index and read one at
/// a time; memory is bounded by the largest single entry, not the count.
pub fn verify_pack(
    archive: &Path,
    pubkey: Option<&sign::PubKey>,
    require_signature: bool,
) -> Result<VerifyReport> {
    let reader = WaxReader::open(archive)
        .with_context(|| format!("opening {}", archive.display()))?;

    let mut bad = Vec::new();
    let mut checked = 0usize;
    let mut redirects = 0usize;
    for entry in reader.entries() {
        let entry = entry.with_context(|| format!("listing {}", archive.display()))?;
        if entry.is_redirect() {
            redirects += 1;
        }
        match reader.read(&entry.path) {
            Ok(_) => checked += 1,
            Err(e) => bad.push((entry.path, e.to_string())),
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
