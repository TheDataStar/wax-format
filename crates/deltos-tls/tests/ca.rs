//! The box's CA, checked with the same certificate verifier browsers'
//! rules come from (rustls/webpki), including the name constraint.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use deltos_tls::{ensure, pem_to_der, Paths, CLOCK_FLOOR, LEAF_DAYS, RENEW_DAYS};
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

const DAY: i64 = 86_400;
const NOW: i64 = CLOCK_FLOOR + 30 * DAY;

fn paths(d: &tempfile::TempDir) -> Paths {
    Paths { ca_dir: d.path().join("ca"), tls_dir: d.path().join("tls"), key_group: None }
}

fn der(p: &std::path::Path) -> CertificateDer<'static> {
    CertificateDer::from(pem_to_der(&std::fs::read_to_string(p).unwrap()).unwrap())
}

/// Verify `leaf` for `name` at `when` against the box's CA alone.
fn verify(ca: &CertificateDer<'static>, leaf: &CertificateDer<'static>, name: &str, when: i64) -> Result<(), rustls::Error> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca.clone()).unwrap();
    let v = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .unwrap();
    v.verify_server_cert(
        leaf,
        &[],
        &ServerName::try_from(name.to_string()).unwrap(),
        &[],
        UnixTime::since_unix_epoch(std::time::Duration::from_secs(when as u64)),
    )
    .map(|_| ())
}

#[test]
fn makes_a_ca_and_two_certificates_each_valid_only_for_its_zone() {
    let d = tempfile::tempdir().unwrap();
    let p = paths(&d);
    let o = ensure(&p, NOW).unwrap();
    assert!(o.created_ca);
    assert_eq!(o.issued, vec!["deltos.lan", "deltos-packs.lan"]);
    assert_eq!(o.ca_fingerprint.len(), 32 * 3 - 1);
    let ca = der(&p.tls_dir.join("ca.crt"));
    let lan = der(&p.tls_dir.join("deltos.lan.crt"));
    let packs = der(&p.tls_dir.join("deltos-packs.lan.crt"));
    verify(&ca, &lan, "home.deltos.lan", NOW).unwrap();
    verify(&ca, &lan, "wiki.deltos.lan", NOW).unwrap();
    verify(&ca, &packs, "pack-3-g1-0123456789abcdef.deltos-packs.lan", NOW).unwrap();
    // never one certificate for both zones (contract §8)
    assert!(verify(&ca, &lan, "pack-3-g1-0123456789abcdef.deltos-packs.lan", NOW).is_err());
    assert!(verify(&ca, &packs, "home.deltos.lan", NOW).is_err());
    // the public CA certificate is the CA's own
    assert_eq!(der(&p.ca_dir.join("ca.crt")), ca);
}

#[test]
fn the_ca_cannot_vouch_for_any_other_website() {
    // Even with the CA key in hand, a certificate for another site fails on
    // every device that trusted the box: the name constraint.
    let d = tempfile::tempdir().unwrap();
    let p = paths(&d);
    ensure(&p, NOW).unwrap();
    let ca_pem = std::fs::read_to_string(p.ca_dir.join("ca.crt")).unwrap();
    let ca_key = rcgen::KeyPair::from_pem(&std::fs::read_to_string(p.ca_dir.join("ca.key")).unwrap()).unwrap();
    let issuer = rcgen::CertificateParams::from_ca_cert_pem(&ca_pem).unwrap().self_signed(&ca_key).unwrap();
    let k = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut lp = rcgen::CertificateParams::new(vec!["bank.example.com".to_string()]).unwrap();
    lp.not_before = time::OffsetDateTime::from_unix_timestamp(NOW - DAY).unwrap();
    lp.not_after = time::OffsetDateTime::from_unix_timestamp(NOW + 30 * DAY).unwrap();
    lp.use_authority_key_identifier_extension = true;
    let evil = lp.signed_by(&k, &issuer, &ca_key).unwrap();
    let ca = CertificateDer::from(pem_to_der(&ca_pem).unwrap());
    let r = verify(&ca, &CertificateDer::from(evil.der().to_vec()), "bank.example.com", NOW);
    assert!(r.is_err(), "a box CA must not be able to vouch for bank.example.com: {r:?}");
}

#[test]
fn nothing_changes_until_renewal_is_due_then_only_the_leaves_renew() {
    let d = tempfile::tempdir().unwrap();
    let p = paths(&d);
    let first = ensure(&p, NOW).unwrap();
    let ca_before = std::fs::read(p.ca_dir.join("ca.crt")).unwrap();
    let leaf_before = std::fs::read(p.tls_dir.join("deltos.lan.crt")).unwrap();
    let again = ensure(&p, NOW + DAY).unwrap();
    assert!(!again.created_ca && again.issued.is_empty(), "{again:?}");
    assert_eq!(std::fs::read(p.tls_dir.join("deltos.lan.crt")).unwrap(), leaf_before);
    // inside the renewal window: new leaves, same CA, still trusted by a device that trusted it once
    let later = NOW + (LEAF_DAYS - RENEW_DAYS) * DAY;
    let renewed = ensure(&p, later).unwrap();
    assert_eq!(renewed.issued.len(), 2);
    assert_eq!(renewed.ca_fingerprint, first.ca_fingerprint);
    assert_eq!(std::fs::read(p.ca_dir.join("ca.crt")).unwrap(), ca_before);
    let ca = der(&p.tls_dir.join("ca.crt"));
    verify(&ca, &der(&p.tls_dir.join("deltos.lan.crt")), "home.deltos.lan", later + 300 * DAY).unwrap();
}

#[test]
fn a_box_that_boots_thinking_it_is_1970_still_makes_usable_certificates() {
    let d = tempfile::tempdir().unwrap();
    let p = paths(&d);
    let o = ensure(&p, 0).unwrap();
    assert!(o.clock_untrusted);
    let ca = der(&p.tls_dir.join("ca.crt"));
    // a phone with the right time accepts them
    verify(&ca, &der(&p.tls_dir.join("deltos.lan.crt")), "home.deltos.lan", CLOCK_FLOOR + 10 * DAY).unwrap();
    // once the clock is right, they are reissued from the real time
    let fixed = ensure(&p, NOW).unwrap();
    assert!(!fixed.clock_untrusted);
    assert_eq!(fixed.issued.len(), 2);
    assert!(!fixed.created_ca);
}

#[test]
fn keys_are_private_and_half_a_ca_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let p = paths(&d);
    ensure(&p, NOW).unwrap();
    let mode = |f: &std::path::Path| std::fs::metadata(f).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&p.ca_dir.join("ca.key")), 0o600);
    assert_eq!(mode(&p.tls_dir.join("deltos.lan.key")), 0o640);
    assert_eq!(mode(&p.tls_dir.join("deltos-packs.lan.key")), 0o640);
    assert_eq!(mode(&p.tls_dir.join("ca.crt")), 0o644);
    assert!(!p.tls_dir.join("ca.key").exists(), "the CA key never sits where deltd can read");
    std::fs::remove_file(p.ca_dir.join("ca.crt")).unwrap();
    assert!(ensure(&p, NOW).is_err(), "half a CA: refuse, never silently start a new one");
}

#[test]
fn modes_hold_under_a_strict_umask() {
    // deltos-tls.service runs with UMask=0077; deltd must still read its keys
    let d = tempfile::tempdir().unwrap();
    let p = paths(&d);
    let old = unsafe { libc::umask(0o077) };
    let r = ensure(&p, NOW);
    unsafe { libc::umask(old) };
    r.unwrap();
    let mode = |f: &std::path::Path| std::fs::metadata(f).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&p.tls_dir.join("deltos.lan.key")), 0o640);
    assert_eq!(mode(&p.tls_dir.join("ca.crt")), 0o644);
}

#[test]
fn deltd_loads_what_was_made() {
    let d = tempfile::tempdir().unwrap();
    let p = paths(&d);
    ensure(&p, NOW).unwrap();
    for stem in ["deltos.lan", "deltos-packs.lan"] {
        let k = std::fs::read_to_string(p.tls_dir.join(format!("{stem}.key"))).unwrap();
        assert!(k.contains("PRIVATE KEY"), "{stem}");
    }
}
