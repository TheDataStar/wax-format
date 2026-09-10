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
pub mod sign;

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use wax_core::header::flag;
use wax_core::{WaxReader, WaxWriter};

pub use assemble::WalkStats;
pub use config::PackConfig;

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
    let cleaned: String = s.chars().filter(|c| *c != '-').collect();
    if cleaned.len() != 32 || !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("archive_uuid must be 32 hex digits (hyphens optional), got {s:?}");
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&cleaned[i * 2..i * 2 + 2], 16)
            .expect("validated as hex above");
    }
    Ok(out)
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
}

/// Fresh single-segment build: `input` tree + `cfg` → `output` archive.
///
/// A new UUIDv4 `archive_uuid` is minted here (SPEC §2) unless
/// [`WriteOptions::archive_uuid`] pins one; appends reuse whatever is already
/// in the archive.
pub fn build_pack(
    input: &Path,
    output: &Path,
    cfg: &PackConfig,
    opts: &WriteOptions,
) -> Result<WriteReport> {
    let (entries, stats) = assemble::collect_entries(input, cfg)?;
    let manifest = cfg.manifest.to_rows()?;
    let uuid = opts.mint_uuid();
    let n = entries.len();

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
    writer
        .build(output, entries, &manifest)
        .with_context(|| format!("writing {}", output.display()))?;

    let sidecar = maybe_sign(output, opts)?;
    let segments = WaxReader::open(output)?.segment_count();

    Ok(WriteReport {
        archive: output.to_path_buf(),
        archive_uuid: uuid,
        entries: n,
        segments,
        stats,
        sidecar,
    })
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
    let declared = cfg.manifest.to_rows()?;
    if !declared.is_empty() {
        let existing = WaxReader::open(archive)?.manifest().clone();
        if declared != existing {
            bail!(
                "this config's [manifest] differs from the archive's. The manifest lives \
                 only in segment 0 and is immutable across appends (SPEC §5.6) — a manifest \
                 change is a new pack version, so rebuild with `build` instead of `append`."
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

    Ok(WriteReport {
        archive: archive.to_path_buf(),
        archive_uuid: uuid,
        entries: n,
        segments: reader.segment_count(),
        stats,
        sidecar,
    })
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
