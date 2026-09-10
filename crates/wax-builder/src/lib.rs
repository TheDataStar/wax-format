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
use std::collections::{BTreeMap, BTreeSet};
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
///
/// The manifest is validated against the B3 schema first (§16), including that
/// `icon` and `entry_point` name entries that actually exist in this pack, and
/// `total_size_bytes` is measured from the finished archive — see
/// [`MAX_SIZE_PASSES`] for why that takes more than one write.
pub fn build_pack(
    input: &Path,
    output: &Path,
    cfg: &PackConfig,
    opts: &WriteOptions,
) -> Result<WriteReport> {
    let (entries, stats) = assemble::collect_entries(input, cfg)?;
    let uuid = opts.mint_uuid();
    let n = entries.len();

    // Validate the manifest before writing anything. `icon`/`entry_point` are
    // checked against the paths this build will actually contain.
    let declares_manifest = !cfg.manifest.is_empty();
    if declares_manifest {
        let paths: BTreeSet<String> = entries.iter().map(|e| {
            wax_core::writer::normalize_path(&e.path).unwrap_or_else(|_| e.path.clone())
        }).collect();
        cfg.manifest
            .validate(Some(&paths))
            .context("invalid [manifest] in pack config")?;
    }

    // `is_signed` lives in the header and the header is inside the signed digest
    // (SPEC §8.1), so the flag has to be set before the archive is written —
    // not patched in afterwards.
    let mut flags = 0u16;
    if opts.sign_key.is_some() {
        flags |= flag::IS_SIGNED;
    }

    let make_writer = || {
        let mut w = WaxWriter::new(uuid).flags(flags);
        if let Some(t) = opts.effective_created_at() {
            w = w.created_at(t);
        }
        w
    };

    if !declares_manifest {
        // No manifest to carry a size, so a single write is enough.
        make_writer()
            .build(output, entries, &BTreeMap::new())
            .with_context(|| format!("writing {}", output.display()))?;
    } else {
        write_until_size_is_stable(output, entries, cfg, &make_writer)?;
    }

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

/// Cap on write passes when settling `total_size_bytes`.
///
/// `manifest.total_size_bytes` records the finished archive's own size, so the
/// value is self-referential: writing a longer number can grow the index
/// segment, which grows the file, which changes the number. The fix is to write,
/// measure, and rewrite until the size stops moving. SQLite's 4 KiB page
/// granularity absorbs the handful of bytes a longer decimal costs, so this
/// settles on pass 2 in practice; the cap exists so a pathological case fails
/// loudly instead of looping.
pub const MAX_SIZE_PASSES: usize = 6;

fn write_until_size_is_stable(
    output: &Path,
    entries: Vec<wax_core::EntryInput>,
    cfg: &PackConfig,
    make_writer: &dyn Fn() -> WaxWriter,
) -> Result<u64> {
    let mut claimed: u64 = 0;
    for pass in 1..=MAX_SIZE_PASSES {
        let manifest = cfg.manifest.to_rows(claimed);
        make_writer()
            .build(output, entries.clone(), &manifest)
            .with_context(|| format!("writing {}", output.display()))?;
        let actual = std::fs::metadata(output)
            .with_context(|| format!("measuring {}", output.display()))?
            .len();
        if actual == claimed {
            return Ok(actual);
        }
        if pass == MAX_SIZE_PASSES {
            bail!(
                "manifest total_size_bytes did not settle after {MAX_SIZE_PASSES} passes \
                 (last claimed {claimed}, actual {actual}). This should not happen; please \
                 report it with the pack config."
            );
        }
        claimed = actual;
    }
    unreachable!("loop returns or bails")
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
    // manifest change is a new pack version, not an append. `total_size_bytes`
    // is excluded from the comparison because the builder computes it — a
    // config never carries one, so it is re-derived from the archive.
    if !cfg.manifest.is_empty() {
        let existing = WaxReader::open(archive)?.manifest().clone();
        let existing_size = existing
            .get("total_size_bytes")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let declared = cfg.manifest.to_rows(existing_size);
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
