//! The box's own certificate authority (docs/box-certificates.md).
//!
//! Every box makes its own small CA the first time it starts, and from it
//! the two wildcard certificates contract §8 asks for: `*.deltos.lan` (the
//! WebOS, `home.deltos.lan`, and the box's services) and `*.deltos-packs.lan`
//! (pack content), never one certificate for both. `deltd` serves HTTPS
//! with them (`deltlib::tls`). A teacher's laptop or the kiosk trusts the CA
//! once and every certificate the box makes after that.
//!
//! **The CA can only vouch for the box's own names.** Its certificate
//! carries a name constraint permitting `deltos.lan` and `deltos-packs.lan`
//! and nothing else, so a device that installed it cannot be fooled by it
//! about any other website, even if the CA key were stolen.
//!
//! [`ensure`] is idempotent: it makes what is missing, renews what is close
//! to expiring, and otherwise changes nothing.

use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa,
    KeyPair, KeyUsagePurpose, NameConstraints, PKCS_ECDSA_P256_SHA256,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

/// The zones the CA may vouch for (contract §8), and so its name constraint.
pub const ZONES: [&str; 2] = ["deltos.lan", "deltos-packs.lan"];

const DAY: i64 = 86_400;
/// The CA lives 10 years: a device trusts it once for the box's life.
pub const CA_DAYS: i64 = 3650;
/// A server certificate lives 397 days, under the 398-day ceiling that
/// browsers apply, so no device's rule ever refuses one.
pub const LEAF_DAYS: i64 = 397;
/// Renewed when fewer than 30 days remain (checked at every boot and daily).
pub const RENEW_DAYS: i64 = 30;
/// Validity starts a day early, so a phone whose clock is a little behind
/// the box's does not see a certificate "not yet valid".
pub const BACKDATE_DAYS: i64 = 1;
/// The earliest time the box will believe. A box without a battery-backed
/// clock can boot thinking it is 1970; certificates made then would look
/// expired to every phone. Below this floor, the floor is used instead and
/// the certificates are renewed once the clock is right.
pub const CLOCK_FLOOR: i64 = 1_793_491_200; // 2026-11-01T00:00:00Z

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("certificate: {0}")]
    Cert(#[from] rcgen::Error),
    #[error("{0}")]
    Bad(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Where things are kept (contract §10, proposed 2026-11-21).
#[derive(Debug, Clone)]
pub struct Paths {
    /// The CA's key and certificate: root only (`/var/lib/deltos/ca/`).
    pub ca_dir: PathBuf,
    /// What `deltd` reads: the two certificates and keys, and the CA's
    /// public certificate for devices to install (`/var/lib/deltos/tls/`).
    pub tls_dir: PathBuf,
    /// The group that may read the server keys (`deltos-deltd`); `None`
    /// leaves the group alone (tests).
    pub key_group: Option<u32>,
}

/// What [`ensure`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub created_ca: bool,
    /// The server certificates that were (re)issued, by file stem.
    pub issued: Vec<String>,
    /// SHA-256 of the CA certificate (DER), colon-separated upper-case hex,
    /// as phones and browsers show it, to compare before trusting it.
    pub ca_fingerprint: String,
    /// The clock was below [`CLOCK_FLOOR`]; the floor was used.
    pub clock_untrusted: bool,
}

/// What was issued, kept beside the certificates so renewal needs no parser
/// of its own output.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Issued {
    ca_fingerprint: String,
    /// file stem -> not_after (unix seconds)
    leaves: std::collections::BTreeMap<String, i64>,
    /// file stem -> the clock was untrusted when it was made
    #[serde(default)]
    provisional: std::collections::BTreeMap<String, bool>,
}

/// The two server certificates: file stem and the names each covers.
pub fn leaves() -> [(&'static str, Vec<String>); 2] {
    [
        ("deltos.lan", vec!["*.deltos.lan".into(), "home.deltos.lan".into()]),
        ("deltos-packs.lan", vec!["*.deltos-packs.lan".into()]),
    ]
}

fn at(unix: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(unix).expect("a time within range")
}

pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

fn read(p: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(p) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(p.into(), e)),
    }
}

/// Write `data` to `path` through a temporary file and a rename, so `deltd`
/// never reads half a certificate, with `mode` set before any byte lands.
fn write_atomic(path: &Path, data: &[u8], mode: u32, group: Option<u32>) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .map_err(|e| Error::Io(tmp.clone(), e))?;
    // the mode exactly, whatever the caller's umask (a unit's UMask=0077
    // would otherwise leave deltd unable to read its own key)
    use std::os::unix::fs::PermissionsExt;
    f.set_permissions(std::fs::Permissions::from_mode(mode)).map_err(|e| Error::Io(tmp.clone(), e))?;
    f.write_all(data).and_then(|_| f.sync_all()).map_err(|e| Error::Io(tmp.clone(), e))?;
    if let Some(g) = group {
        std::os::unix::fs::fchown(&f, None, Some(g)).map_err(|e| Error::Io(tmp.clone(), e))?;
    }
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| Error::Io(path.into(), e))
}

fn ca_params(not_before: i64, box_id: &str) -> Result<CertificateParams> {
    let mut p = CertificateParams::new(Vec::<String>::new())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, "DeltOS");
    dn.push(DnType::CommonName, format!("DeltOS box {box_id}"));
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(1));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    p.name_constraints = Some(NameConstraints {
        permitted_subtrees: ZONES.iter().map(|z| GeneralSubtree::DnsName(z.to_string())).collect(),
        excluded_subtrees: vec![],
    });
    p.not_before = at(not_before);
    p.not_after = at(not_before + CA_DAYS * DAY);
    Ok(p)
}

/// Make what is missing and renew what is near its end. `now` is the
/// box's clock (unix seconds).
pub fn ensure(paths: &Paths, now: i64) -> Result<Outcome> {
    let clock_untrusted = now < CLOCK_FLOOR;
    let now = now.max(CLOCK_FLOOR);
    let start = now - BACKDATE_DAYS * DAY;
    for d in [&paths.ca_dir, &paths.tls_dir] {
        std::fs::create_dir_all(d).map_err(|e| Error::Io(d.clone(), e))?;
    }
    let ca_key_path = paths.ca_dir.join("ca.key");
    let ca_crt_path = paths.ca_dir.join("ca.crt");

    // The CA: made once, kept for the box's life.
    let (ca_key, ca_pem, created_ca) = match (read(&ca_key_path)?, read(&ca_crt_path)?) {
        (Some(k), Some(c)) => (KeyPair::from_pem(&k)?, c, false),
        (None, None) => {
            let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
            let id: String = fingerprint(&key.public_key_der()).replace(':', "")[..8].to_ascii_lowercase();
            let cert = ca_params(start, &id)?.self_signed(&key)?;
            write_atomic(&ca_key_path, key.serialize_pem().as_bytes(), 0o600, None)?;
            write_atomic(&ca_crt_path, cert.pem().as_bytes(), 0o644, None)?;
            (key, cert.pem(), true)
        }
        _ => {
            return Err(Error::Bad(format!(
                "{} holds only half of the CA (key or certificate); refusing to guess. Restore it, or remove both to start a new CA (every device must then trust the new one)",
                paths.ca_dir.display()
            )))
        }
    };
    let ca_der = pem_to_der(&ca_pem)?;
    let ca_fp = fingerprint(&ca_der);
    // the issuer: the stored CA's own name and key
    let issuer = CertificateParams::from_ca_cert_pem(&ca_pem)?.self_signed(&ca_key)?;

    let state_path = paths.tls_dir.join("issued.json");
    let mut state: Issued =
        read(&state_path)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if state.ca_fingerprint != ca_fp {
        // a new CA: every server certificate must be reissued under it
        state = Issued { ca_fingerprint: ca_fp.clone(), ..Default::default() };
    }

    let mut issued = Vec::new();
    for (stem, names) in leaves() {
        let crt = paths.tls_dir.join(format!("{stem}.crt"));
        let key = paths.tls_dir.join(format!("{stem}.key"));
        let fresh = matches!(state.leaves.get(stem), Some(&na) if na - now > RENEW_DAYS * DAY)
            && !(state.provisional.get(stem) == Some(&true) && !clock_untrusted)
            && crt.exists()
            && key.exists();
        if fresh {
            continue;
        }
        let k = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut p = CertificateParams::new(names)?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, format!("*.{stem}"));
        p.distinguished_name = dn;
        p.is_ca = IsCa::NoCa;
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.use_authority_key_identifier_extension = true;
        p.not_before = at(start);
        let not_after = start + LEAF_DAYS * DAY;
        p.not_after = at(not_after);
        let cert = p.signed_by(&k, &issuer, &ca_key)?;
        // key first, then the certificate, each replaced whole
        write_atomic(&key, k.serialize_pem().as_bytes(), 0o640, paths.key_group)?;
        write_atomic(&crt, cert.pem().as_bytes(), 0o644, None)?;
        state.leaves.insert(stem.to_string(), not_after);
        state.provisional.insert(stem.to_string(), clock_untrusted);
        issued.push(stem.to_string());
    }
    // the CA's public certificate, for devices to install
    let public = paths.tls_dir.join("ca.crt");
    if read(&public)?.as_deref() != Some(ca_pem.as_str()) {
        write_atomic(&public, ca_pem.as_bytes(), 0o644, None)?;
    }
    write_atomic(&state_path, serde_json::to_string_pretty(&state).unwrap().as_bytes(), 0o644, None)?;
    Ok(Outcome { created_ca, issued, ca_fingerprint: ca_fp, clock_untrusted })
}

/// The first certificate in a PEM text, as DER.
pub fn pem_to_der(pem: &str) -> Result<Vec<u8>> {
    let b64: String = pem
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN CERTIFICATE-----"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .collect();
    base64_decode(&b64).ok_or_else(|| Error::Bad("not a PEM certificate".into()))
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        if c == b'=' {
            break;
        }
        let v = T.iter().position(|&t| t == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A group's id by name.
pub fn gid_of(name: &str) -> Option<u32> {
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: getgrnam returns null or a pointer to a static record, read at once.
    let g = unsafe { libc::getgrnam(c.as_ptr()) };
    (!g.is_null()).then(|| unsafe { (*g).gr_gid })
}
