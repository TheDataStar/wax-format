//! A7 signing hook — detached minisign sidecar (SPEC §8).
//!
//! Signing and checking are done **in Rust**, with the `minisign` crate (the
//! same format and algorithm as the `minisign` program, by the same author),
//! so building a signed pack needs no outside program. The box's key holder
//! already checks signatures this way (`deltos-shelld`, `minisign-verify`).
//! The sidecar is byte-compatible with the program's: `minisign -V -H`
//! verifies what this writes, and this verifies what `minisign -S` writes
//! (the interoperability tests run both ways whenever the program is
//! installed).
//!
//! What gets signed (SPEC §8.1) is **not** the archive file but the 32-byte
//! digest `SHA-256(header || segment[0] || ... || segment[N])`, which
//! `wax-core` computes via [`wax_core::WaxReader::signable_digest`].
//!
//! ## Prehashing — SPEC §8.2's wording
//!
//! SPEC §8.2 says to sign "in prehashed mode (minisign `-H`)". In minisign
//! `-H` is a **verify-side** flag meaning *require* a prehashed signature;
//! signing produces prehashed (`ED`) signatures by default. So: every
//! signature written here is prehashed, and [`verify`] refuses a legacy,
//! non-prehashed one, exactly as `minisign -V -H` does.
//!
//! ## Keys
//!
//! A secret key is a minisign key file. One made without a password
//! (`minisign -G -W`, or [`write_unencrypted_keypair`]) is read as is, for
//! unattended builds. One with a password is unlocked by asking on the
//! terminal, as the `minisign` program does; with no terminal it is refused.

use anyhow::{anyhow, bail, Context, Result};
use ct_codecs::{Base64, Decoder, Encoder};
use std::io::{Cursor, IsTerminal};
use std::path::{Path, PathBuf};

/// Environment overrides.
pub const ENV_SECKEY: &str = "WAX_MINISIGN_KEY";
pub const ENV_PUBKEY: &str = "WAX_MINISIGN_PUBKEY";
/// Where the interoperability tests look for the `minisign` program. Not
/// used to sign or verify.
pub const ENV_MINISIGN: &str = "WAX_MINISIGN";

/// The `minisign` program, for the interoperability tests only:
/// `$WAX_MINISIGN`, else `minisign` on PATH.
pub fn minisign_bin() -> String {
    std::env::var(ENV_MINISIGN).unwrap_or_else(|_| "minisign".to_string())
}

/// The untrusted comment written into every sidecar.
pub const UNTRUSTED_COMMENT: &str = "WAX archive signature (see SPEC.md section 8)";

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
    // Streams header + segment bytes; never loads the entry table (Track A §18).
    wax_core::reader::signable_digest_of(archive)
        .with_context(|| format!("computing the digest of {}", archive.display()))
}

/// Sign `archive`, writing `<archive>.minisig`.
///
/// `seckey` is a path to a minisign secret key (see the module docs on
/// passwords).
pub fn sign(archive: &Path, seckey: &Path) -> Result<PathBuf> {
    if !seckey.is_file() {
        bail!("secret key {} does not exist", seckey.display());
    }
    let sk = load_secret_key(seckey)?;
    let (digest, uuid, created_at) = signable_digest(archive)?;
    let sidecar = sidecar_path(archive);
    let sig = minisign::sign(
        None,
        &sk,
        Cursor::new(digest.to_vec()),
        Some(&trusted_comment(&uuid, created_at)),
        Some(UNTRUSTED_COMMENT),
    )
    .map_err(|e| anyhow!("signing failed: {e}"))?;
    // written beside the archive, then renamed, so a reader never sees half
    let tmp = sidecar.with_extension("minisig.tmp");
    std::fs::write(&tmp, sig.into_string()).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &sidecar).with_context(|| format!("writing {}", sidecar.display()))?;
    Ok(sidecar)
}

/// Read a minisign secret key: as is when it has no password, else by
/// asking on the terminal.
fn load_secret_key(path: &Path) -> Result<minisign::SecretKey> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading secret key {}", path.display()))?;
    match minisign::SecretKey::from_box(text.clone().into(), Some(String::new())) {
        Ok(k) => Ok(k),
        // the crate's only signal that the empty password was wrong
        Err(e) if e.to_string().contains("Wrong password") => {
            if !std::io::stdin().is_terminal() {
                bail!(
                    "secret key {} has a password and there is no terminal to ask for it; \
                     use a key made without one (minisign -G -W) for unattended builds",
                    path.display()
                );
            }
            minisign::SecretKey::from_box(text.into(), None).map_err(|e| anyhow!("secret key {}: {e}", path.display()))
        }
        Err(e) => Err(anyhow!("secret key {}: {e}", path.display())),
    }
}

/// Make a minisign keypair with no password at `seckey` and `pubkey`, in
/// the program's own file format (as `minisign -G -W`). For unattended
/// builds and tests; an existing file is never overwritten.
pub fn write_unencrypted_keypair(seckey: &Path, pubkey: &Path) -> Result<()> {
    for p in [seckey, pubkey] {
        if p.exists() {
            bail!("{} already exists", p.display());
        }
    }
    // An empty password gives a key with its checksum and no encryption;
    // marking its key-derivation algorithm "none" (two zero bytes, SPEC of
    // the minisign key format) is what `minisign -G -W` writes, so the
    // program reads it without asking for a password too.
    let kp = minisign::KeyPair::generate_encrypted_keypair(Some(String::new())).map_err(|e| anyhow!("making a key: {e}"))?;
    let boxed = kp.sk.to_box(None).map_err(|e| anyhow!("{e}"))?.into_string();
    let mut lines: Vec<String> = boxed.lines().map(str::to_string).collect();
    let mut raw = Base64::decode_to_vec(lines[1].trim(), None).map_err(|e| anyhow!("{e}"))?;
    raw[2] = 0;
    raw[3] = 0;
    lines[1] = Base64::encode_to_string(&raw).map_err(|e| anyhow!("{e}"))?;
    let sk = lines.join("\n") + "\n";
    let pk = kp.pk.to_box().map_err(|e| anyhow!("{e}"))?.into_string();
    std::fs::write(seckey, sk).with_context(|| format!("writing {}", seckey.display()))?;
    std::fs::write(pubkey, pk).with_context(|| format!("writing {}", pubkey.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(seckey, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
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
/// A legacy, non-prehashed signature is refused, as `minisign -V -H` does
/// (SPEC §8.2).
pub fn verify(archive: &Path, pubkey: &PubKey) -> Result<SignatureReport> {
    let sidecar = sidecar_path(archive);
    if !sidecar.is_file() {
        bail!("no signature sidecar at {}", sidecar.display());
    }
    let (digest, uuid, _created_at) = signable_digest(archive)?;
    let pk = match pubkey {
        PubKey::File(p) => {
            if !p.is_file() {
                bail!("public key {} does not exist", p.display());
            }
            minisign::PublicKey::from_file(p).map_err(|e| anyhow!("public key {}: {e}", p.display()))?
        }
        PubKey::Inline(s) => minisign::PublicKey::from_base64(s.trim()).map_err(|e| anyhow!("public key: {e}"))?,
    };
    let sig = minisign::SignatureBox::from_file(&sidecar).map_err(|e| anyhow!("signature does not verify: {e}"))?;
    if !sig.is_prehashed() {
        bail!("signature does not verify: a legacy (not prehashed) signature is refused (SPEC 8.2)");
    }
    minisign::verify(&pk, &sig, Cursor::new(digest.to_vec()), true, false, false)
        .map_err(|e| anyhow!("signature does not verify: {e}"))?;

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
