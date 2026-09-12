//! A7 signing hook — detached minisign sidecar (SPEC §8).
//!
//! `wax-builder` shells out to the `minisign` binary rather than linking an
//! Ed25519 implementation, so the signature is produced by the same tool
//! operators verify with. Key *generation* is out of scope: a keypair is
//! supplied by flag or environment.
//!
//! What gets signed (SPEC §8.1) is **not** the archive file but the 32-byte
//! digest `SHA-256(header || segment[0] || ... || segment[N])`, which
//! `wax-core` computes via [`wax_core::WaxReader::signable_digest`]. That digest
//! is written to a temporary file and handed to `minisign -S`.
//!
//! ## Prehashing — deviation from SPEC §8.2's wording
//!
//! SPEC §8.2 says to sign "in prehashed mode (minisign `-H`)". In minisign 0.12
//! `-H` is a **verify-side** flag (`minisign -V [-H] ...`) meaning *require* the
//! signature to be prehashed; `minisign -S` has no `-H` option and produces
//! prehashed (`ED`) signatures by default, with `-l` selecting the legacy
//! non-prehashed form. The spec's *intent* — prehashed signatures — is met by
//! signing with plain `-S` (never `-l`) and verifying with `-V -H`, which is
//! what this module does.

use anyhow::{anyhow, bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use wax_core::WaxReader;

/// Environment overrides.
pub const ENV_MINISIGN: &str = "WAX_MINISIGN";
pub const ENV_SECKEY: &str = "WAX_MINISIGN_KEY";
pub const ENV_PUBKEY: &str = "WAX_MINISIGN_PUBKEY";

/// Locate the `minisign` executable: `$WAX_MINISIGN`, else `minisign` on PATH.
pub fn minisign_bin() -> String {
    std::env::var(ENV_MINISIGN).unwrap_or_else(|_| "minisign".to_string())
}

/// The sidecar path for an archive: `<archive>.minisig` (SPEC §8.2).
pub fn sidecar_path(archive: &Path) -> PathBuf {
    let mut s = archive.as_os_str().to_os_string();
    s.push(".minisig");
    PathBuf::from(s)
}

/// Trusted comment carried in the sidecar (SPEC §8.2: `archive_uuid` + `created_at`).
/// The uuid is written in its canonical lowercase-hyphenated form (Contract §11).
pub fn trusted_comment(uuid: &[u8; 16], created_at: u64) -> String {
    format!("wax archive_uuid={} created_at={created_at}", crate::uuid_text(uuid))
}

/// Recompute the signable digest for an archive on disk (SPEC §8.1).
/// Returns `(digest, archive_uuid, created_at)`.
pub fn signable_digest(archive: &Path) -> Result<([u8; 32], [u8; 16], u64)> {
    let mut r = WaxReader::open(archive)
        .with_context(|| format!("opening {} to compute its digest", archive.display()))?;
    let uuid = r.header().archive_uuid;
    let created_at = r.header().created_at;
    let digest = r.signable_digest()?;
    Ok((digest, uuid, created_at))
}

/// Sign `archive`, writing `<archive>.minisig`.
///
/// `seckey` is a path to a minisign secret key. A password-protected key makes
/// `minisign` prompt on the terminal; for unattended builds use a key created
/// with `minisign -G -W` (no password).
pub fn sign(archive: &Path, seckey: &Path) -> Result<PathBuf> {
    if !seckey.is_file() {
        bail!("secret key {} does not exist", seckey.display());
    }
    let (digest, uuid, created_at) = signable_digest(archive)?;
    let sidecar = sidecar_path(archive);

    let dir = tempfile::tempdir().context("creating temp dir for the digest")?;
    let msg = dir.path().join("wax.digest");
    std::fs::File::create(&msg)?.write_all(&digest)?;

    // NOTE: plain `-S` (no `-l`) is minisign's prehashed `ED` form — see the
    // module docs for why this differs from SPEC 8.2's literal `-H` wording.
    let out = Command::new(minisign_bin())
        .arg("-S")
        .arg("-s")
        .arg(seckey)
        .arg("-m")
        .arg(&msg)
        .arg("-x")
        .arg(&sidecar)
        .arg("-t")
        .arg(trusted_comment(&uuid, created_at))
        .arg("-c")
        .arg("WAX archive signature (see SPEC.md section 8)")
        .output()
        .map_err(|e| launch_error(e, "sign"))?;

    if !out.status.success() {
        bail!(
            "minisign -S failed ({}): {}{}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim(),
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }
    if !sidecar.is_file() {
        bail!(
            "minisign reported success but {} was not created",
            sidecar.display()
        );
    }
    Ok(sidecar)
}

/// Outcome of a sidecar check.
#[derive(Debug)]
pub struct SignatureReport {
    pub sidecar: PathBuf,
    pub trusted_comment: String,
    /// `archive_uuid` parsed out of the trusted comment, if present.
    pub comment_uuid: Option<String>,
    pub header_uuid: String,
}

/// Where the verifying public key comes from.
#[derive(Debug, Clone)]
pub enum PubKey {
    File(PathBuf),
    /// Raw base64 key string (minisign `-P`).
    Inline(String),
}

impl PubKey {
    /// Resolve from explicit flags, then `$WAX_MINISIGN_PUBKEY` (treated as a
    /// path if it names an existing file, otherwise as an inline key).
    pub fn resolve(file: Option<PathBuf>, inline: Option<String>) -> Option<PubKey> {
        if let Some(p) = file {
            return Some(PubKey::File(p));
        }
        if let Some(s) = inline {
            return Some(PubKey::Inline(s));
        }
        match std::env::var(ENV_PUBKEY) {
            Ok(v) if !v.is_empty() => {
                let p = PathBuf::from(&v);
                Some(if p.is_file() {
                    PubKey::File(p)
                } else {
                    PubKey::Inline(v)
                })
            }
            _ => None,
        }
    }
}

/// Verify `<archive>.minisig` against `archive` using `pubkey`.
///
/// Verification is `minisign -V -H` — `-H` *requires* the signature to be
/// prehashed, rejecting a legacy-format signature (SPEC §8.2).
pub fn verify(archive: &Path, pubkey: &PubKey) -> Result<SignatureReport> {
    let sidecar = sidecar_path(archive);
    if !sidecar.is_file() {
        bail!("no signature sidecar at {}", sidecar.display());
    }
    let (digest, uuid, _created_at) = signable_digest(archive)?;

    let dir = tempfile::tempdir().context("creating temp dir for the digest")?;
    let msg = dir.path().join("wax.digest");
    std::fs::File::create(&msg)?.write_all(&digest)?;

    let mut cmd = Command::new(minisign_bin());
    cmd.arg("-V").arg("-H");
    match pubkey {
        PubKey::File(p) => {
            if !p.is_file() {
                bail!("public key {} does not exist", p.display());
            }
            cmd.arg("-p").arg(p);
        }
        PubKey::Inline(s) => {
            cmd.arg("-P").arg(s);
        }
    }
    cmd.arg("-m").arg(&msg).arg("-x").arg(&sidecar);

    let out = cmd.output().map_err(|e| launch_error(e, "verify"))?;
    if !out.status.success() {
        bail!(
            "signature does not verify: {}{}",
            String::from_utf8_lossy(&out.stderr).trim(),
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }

    let comment = read_trusted_comment(&sidecar)?;
    let header_uuid = crate::uuid_text(&uuid);
    let comment_uuid = comment
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("archive_uuid=").map(|v| v.to_string()));

    // SPEC §8.3 step 3: bind the sidecar to this specific archive. Compare as
    // parsed UUIDs, not strings, so a sidecar written before the canonical
    // hyphenated form was pinned (bare 32-hex) still binds correctly.
    if let Some(cu) = &comment_uuid {
        let same = uuid::Uuid::parse_str(cu)
            .map(|u| *u.as_bytes() == uuid)
            .unwrap_or(false);
        if !same {
            bail!(
                "signature is for a different archive: trusted comment says \
                 archive_uuid={cu}, header says {header_uuid}"
            );
        }
    }

    Ok(SignatureReport {
        sidecar,
        trusted_comment: comment,
        comment_uuid,
        header_uuid,
    })
}

/// Resolve the secret key from `--sign-key` then `$WAX_MINISIGN_KEY`.
pub fn resolve_seckey(flag: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = flag {
        return Some(p);
    }
    match std::env::var(ENV_SECKEY) {
        Ok(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => None,
    }
}

/// The `trusted comment: ...` line of a minisign sidecar (line 3).
fn read_trusted_comment(sidecar: &Path) -> Result<String> {
    let text = std::fs::read_to_string(sidecar)
        .with_context(|| format!("reading {}", sidecar.display()))?;
    text.lines()
        .find_map(|l| l.strip_prefix("trusted comment:").map(|c| c.trim().to_string()))
        .ok_or_else(|| anyhow!("{} has no trusted comment line", sidecar.display()))
}

fn launch_error(e: std::io::Error, what: &str) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        anyhow!(
            "could not run `{}` to {what} — install minisign or set ${} to its full path",
            minisign_bin(),
            ENV_MINISIGN
        )
    } else {
        anyhow!("could not run `{}` to {what}: {e}", minisign_bin())
    }
}
