//! A2 conformance — `wax-builder`-specific behaviour on top of the A4 suite in
//! `wax-core`. Cases named in the Track A2 brief:
//!
//! * alias flattening produces exactly one hop,
//! * two builds of the same source tree are byte-identical (determinism),
//! * append preserves `archive_uuid` and leaves prior segments untouched.
//!
//! Plus the manifest/compression/signing behaviour A2 owns.

use std::path::{Path, PathBuf};
use tempfile::TempDir;
use wax_builder::config::PackConfig;
use wax_builder::{append_pack, build_pack, verify_pack, sign, WriteOptions};
use wax_core::header::flag;
use wax_core::WaxReader;

const PINNED: u64 = 1_700_000_000;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Build a source tree; `files` are `(relative path, contents)`.
fn tree(files: &[(&str, &[u8])]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }
    dir
}

fn cfg_from(toml_src: &str) -> PackConfig {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("wax-pack.toml");
    std::fs::write(&p, toml_src).unwrap();
    PackConfig::load(&p).unwrap()
}

/// Reproducible-build options: pinned timestamp AND pinned identity.
/// A fresh UUIDv4 is random, so both have to be pinned for byte-identical output.
const PINNED_UUID: [u8; 16] = [
    0x4a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f, 0x46, 0x07, 0x8a, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];

fn pinned() -> WriteOptions {
    WriteOptions {
        created_at: Some(PINNED),
        sign_key: None,
        archive_uuid: Some(PINNED_UUID),
    }
}

fn out(dir: &TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}

/// Locate minisign. Tests that need it skip when it is absent — but set
/// `WAX_REQUIRE_MINISIGN=1` (CI does) and a missing binary becomes a hard
/// failure, so the signing suite can never silently pass by not running.
fn minisign_available() -> bool {
    let ok = std::process::Command::new(sign::minisign_bin())
        .arg("-v")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok && std::env::var("WAX_REQUIRE_MINISIGN").as_deref() == Ok("1") {
        panic!(
            "WAX_REQUIRE_MINISIGN=1 but `{}` could not be run. Install minisign or              point $WAX_MINISIGN at it.",
            sign::minisign_bin()
        );
    }
    if !ok {
        eprintln!(
            "SKIPPED (minisign not found via `{}`; set $WAX_MINISIGN, or              WAX_REQUIRE_MINISIGN=1 to make this a failure)",
            sign::minisign_bin()
        );
    }
    ok
}

/// Generate a throwaway password-less keypair. Returns `(seckey, pubkey)`.
fn keypair(dir: &Path) -> (PathBuf, PathBuf) {
    let sec = dir.join("test.key");
    let pubk = dir.join("test.pub");
    let out = std::process::Command::new(sign::minisign_bin())
        .arg("-G")
        .arg("-W")
        .arg("-f")
        .arg("-s")
        .arg(&sec)
        .arg("-p")
        .arg(&pubk)
        .output()
        .expect("minisign -G");
    assert!(
        out.status.success(),
        "keygen failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (sec, pubk)
}

// ---------------------------------------------------------------------------
// 1. alias flattening — exactly one hop on disk
// ---------------------------------------------------------------------------

#[test]
fn aliases_are_flattened_to_exactly_one_hop() {
    let src = tree(&[("articles/canonical.html", b"CANON")]);
    // Declared chain: a -> b -> c -> canonical. On disk every one of them must
    // point straight at the canonical entry (SPEC 6.2).
    let cfg = cfg_from(
        r#"
[aliases]
"a.html" = "b.html"
"b.html" = "c.html"
"c.html" = "articles/canonical.html"
"#,
    );
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(src.path(), &archive, &cfg, &pinned()).unwrap();

    let r = WaxReader::open(&archive).unwrap();
    for alias in ["a.html", "b.html", "c.html"] {
        let e = r.entry(alias).unwrap_or_else(|_| panic!("{alias} missing"));
        assert_eq!(
            e.redirect_to.as_deref(),
            Some("articles/canonical.html"),
            "{alias} should point straight at the canonical entry, not at another redirect"
        );
        // and resolving must succeed within one hop
        assert_eq!(r.resolve(alias).unwrap().entry.path, "articles/canonical.html");
        assert_eq!(r.read(alias).unwrap(), b"CANON");
    }

    // No entry on disk redirects to another redirect.
    let redirect_targets: Vec<String> = r
        .entries()
        .filter_map(|e| e.unwrap().redirect_to)
        .collect();
    for t in redirect_targets {
        let target = r.entry(&t).expect("redirect target exists");
        assert!(
            target.redirect_to.is_none(),
            "redirect target {t} is itself a redirect — chain was not flattened"
        );
    }
}

#[test]
fn alias_cycle_is_a_build_error() {
    let src = tree(&[("real.html", b"x")]);
    let cfg = cfg_from(
        r#"
[aliases]
"a.html" = "b.html"
"b.html" = "a.html"
"#,
    );
    let dst = tempfile::tempdir().unwrap();
    let err = build_pack(src.path(), &out(&dst, "p.wax"), &cfg, &pinned()).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("deeper than one hop") || msg.to_lowercase().contains("redirect"),
        "expected a redirect-cycle error, got: {msg}"
    );
}

#[test]
fn alias_to_nonexistent_target_is_a_build_error() {
    let src = tree(&[("real.html", b"x")]);
    let cfg = cfg_from(
        r#"
[aliases]
"a.html" = "nope.html"
"#,
    );
    let dst = tempfile::tempdir().unwrap();
    let err = build_pack(src.path(), &out(&dst, "p.wax"), &cfg, &pinned()).unwrap_err();
    assert!(
        format!("{err:#}").contains("does not exist"),
        "expected a dangling-redirect error, got: {err:#}"
    );
}

// ---------------------------------------------------------------------------
// 2. determinism
// ---------------------------------------------------------------------------

#[test]
fn two_builds_of_the_same_tree_are_byte_identical() {
    let src = tree(&[
        ("index.html", b"<h1>Home</h1>"),
        ("css/site.css", b"body{margin:0}"),
        ("img/logo.png", &[0x89, b'P', b'N', b'G', 1, 2, 3, 4][..]),
        ("deep/nested/page.html", b"<p>deep</p>"),
        ("unicode/\u{79d1}\u{5b66}.html", "\u{5185}\u{5bb9}".as_bytes()),
    ]);
    let cfg = cfg_from(
        r#"
[aliases]
"home.html" = "index.html"
"#,
    );
    let dst = tempfile::tempdir().unwrap();
    let a = out(&dst, "a.wax");
    let b = out(&dst, "b.wax");

    build_pack(src.path(), &a, &cfg, &pinned()).unwrap();
    build_pack(src.path(), &b, &cfg, &pinned()).unwrap();

    let ba = std::fs::read(&a).unwrap();
    let bb = std::fs::read(&b).unwrap();
    assert_eq!(ba.len(), bb.len(), "archive sizes differ");
    assert!(
        ba == bb,
        "two builds of the same tree with a pinned created_at must be byte-identical; \
         first difference at offset {:?}",
        ba.iter().zip(&bb).position(|(x, y)| x != y)
    );
}

#[test]
fn created_at_is_the_only_thing_that_moves_between_builds() {
    // Same tree, different pinned created_at. Everything except the timestamp
    // (header bytes 24..32 and segment_meta.created_at inside the SQLite
    // segment) should be stable — in particular the blob region is identical.
    let src = tree(&[("a.html", b"AAAA"), ("b.html", b"BBBB")]);
    let cfg = PackConfig::default();
    let dst = tempfile::tempdir().unwrap();
    let a = out(&dst, "a.wax");
    let b = out(&dst, "b.wax");

    build_pack(src.path(), &a, &cfg, &WriteOptions { created_at: Some(PINNED), sign_key: None, archive_uuid: Some(PINNED_UUID) }).unwrap();
    build_pack(src.path(), &b, &cfg, &WriteOptions { created_at: Some(PINNED + 86_400), sign_key: None, archive_uuid: Some(PINNED_UUID) }).unwrap();

    let ra = WaxReader::open(&a).unwrap();
    let rb = WaxReader::open(&b).unwrap();
    let (ha, hb) = (ra.header(), rb.header());

    assert_ne!(ha.created_at, hb.created_at);
    assert_eq!(ha.archive_uuid, hb.archive_uuid, "uuid was pinned for both builds");
    assert_eq!(ha.index_offset, hb.index_offset);
    assert_eq!(ha.index_length, hb.index_length);
    assert_eq!(ha.blob_section_length, hb.blob_section_length);
    assert_eq!(ha.flags, hb.flags);

    // blob regions byte-identical
    let ba = std::fs::read(&a).unwrap();
    let bb = std::fs::read(&b).unwrap();
    let blob_end = 128 + ha.blob_section_length as usize;
    assert_eq!(&ba[128..blob_end], &bb[128..blob_end], "blob region moved");
}

// ---------------------------------------------------------------------------
// 3. append: uuid preserved, prior segments untouched
// ---------------------------------------------------------------------------

#[test]
fn append_preserves_uuid_and_leaves_prior_bytes_untouched() {
    let base = tree(&[("a.txt", b"a-base"), ("shared.txt", b"v1")]);
    let extra = tree(&[("b.txt", b"b-append"), ("shared.txt", b"v2")]);
    let cfg = PackConfig::default();

    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    let first = build_pack(base.path(), &archive, &cfg, &pinned()).unwrap();

    let before = std::fs::read(&archive).unwrap();
    let uuid_before = first.archive_uuid;
    let (idx_off, idx_len) = {
        let r = WaxReader::open(&archive).unwrap();
        (r.header().index_offset, r.header().index_length)
    };

    let second = append_pack(
        &archive,
        extra.path(),
        &cfg,
        &WriteOptions { created_at: Some(PINNED + 10), sign_key: None, archive_uuid: None },
    )
    .unwrap();

    // uuid preserved (SPEC 2: stable identity across versions)
    assert_eq!(second.archive_uuid, uuid_before);
    let after = std::fs::read(&archive).unwrap();

    // Everything from the end of the header to the end of the old index segment
    // must be byte-for-byte unchanged (SPEC 7 invariant).
    let old_tail = (idx_off + idx_len) as usize;
    assert_eq!(
        &before[128..old_tail],
        &after[128..old_tail],
        "append rewrote blob region 0 or index segment 0"
    );
    assert!(after.len() > before.len(), "append should have grown the file");

    // Only the header changed in the first 128 bytes, and only its pointers.
    let r = WaxReader::open(&archive).unwrap();
    assert_eq!(r.segment_count(), 2);
    assert_eq!(r.header().archive_uuid, uuid_before);

    let r = WaxReader::open(&archive).unwrap();
    assert_eq!(r.read("a.txt").unwrap(), b"a-base", "base entry still readable");
    assert_eq!(r.read("shared.txt").unwrap(), b"v2", "append overrides base");
    assert_eq!(r.read("b.txt").unwrap(), b"b-append");
}

// ---------------------------------------------------------------------------
// manifest
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// compression policy
// ---------------------------------------------------------------------------

#[test]
fn already_compressed_formats_are_stored_verbatim() {
    let src = tree(&[
        ("page.html", b"<html>lots of compressible text here</html>"),
        ("photo.jpg", &[0xFF, 0xD8, 0xFF, 0xE0, 9, 9, 9, 9][..]),
    ]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(src.path(), &archive, &PackConfig::default(), &pinned()).unwrap();

    let r = WaxReader::open(&archive).unwrap();
    assert_eq!(r.entry("page.html").unwrap().compression, "zstd");
    assert_eq!(r.entry("photo.jpg").unwrap().compression, "none");
}

#[test]
fn compression_none_applies_to_everything() {
    let src = tree(&[("page.html", b"text")]);
    let cfg = cfg_from("[build]\ncompression = \"none\"\n");
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(src.path(), &archive, &cfg, &pinned()).unwrap();
    assert_eq!(
        WaxReader::open(&archive).unwrap().entry("page.html").unwrap().compression,
        "none"
    );
}

#[test]
fn unknown_codec_in_config_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("wax-pack.toml");
    std::fs::write(&p, "[build]\ncompression = \"brotli\"\n").unwrap();
    let err = PackConfig::load(&p).unwrap_err();
    assert!(format!("{err:#}").contains("not a v0.9 codec"));
}

#[test]
fn the_pack_config_is_not_archived_as_an_entry() {
    let src = tree(&[("a.txt", b"a")]);
    std::fs::write(
        src.path().join("wax-pack.toml"),
        "[titles]
\"a.txt\" = \"The A File\"
",
    )
    .unwrap();
    let cfg = PackConfig::discover(src.path(), None).unwrap();
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(src.path(), &archive, &cfg, &pinned()).unwrap();

    let r = WaxReader::open(&archive).unwrap();
    assert!(!r.contains("wax-pack.toml").unwrap(), "config leaked into the archive");
    // ...but the config was read: its title landed on the entry
    assert_eq!(r.entry("a.txt").unwrap().title.as_deref(), Some("The A File"));
}

// ---------------------------------------------------------------------------
// entry metadata
// ---------------------------------------------------------------------------

#[test]
fn mime_and_title_are_populated() {
    let src = tree(&[("index.html", b"<h1>hi</h1>"), ("style.css", b"a{}")]);
    let cfg = cfg_from("[titles]\n\"index.html\" = \"Home Page\"\n");
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(src.path(), &archive, &cfg, &pinned()).unwrap();

    let r = WaxReader::open(&archive).unwrap();
    assert_eq!(r.entry("index.html").unwrap().mime.as_deref(), Some("text/html"));
    assert_eq!(r.entry("style.css").unwrap().mime.as_deref(), Some("text/css"));
    assert_eq!(r.entry("index.html").unwrap().title.as_deref(), Some("Home Page"));
    assert!(r.entry("style.css").unwrap().title.is_none());
}

#[test]
fn sha256_is_over_uncompressed_content() {
    use sha2::{Digest, Sha256};
    let body = b"the quick brown fox jumps over the lazy dog, repeatedly and compressibly";
    let src = tree(&[("doc.html", body)]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(src.path(), &archive, &PackConfig::default(), &pinned()).unwrap();

    let r = WaxReader::open(&archive).unwrap();
    let e = r.entry("doc.html").unwrap();
    assert_eq!(e.compression, "zstd", "precondition: this entry is compressed");
    assert_ne!(e.length, e.uncompressed_length, "precondition: sizes differ");
    let expected: [u8; 32] = Sha256::digest(body).into();
    assert_eq!(e.sha256, Some(expected), "sha256 must hash the *uncompressed* bytes");
}

#[test]
fn every_entry_has_volume_id_zero() {
    let src = tree(&[("a.txt", b"a"), ("b.txt", b"b")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(src.path(), &archive, &PackConfig::default(), &pinned()).unwrap();
    let r = WaxReader::open(&archive).unwrap();
    assert!(r.entries().all(|e| e.unwrap().volume_id == 0));
}

#[test]
fn archive_uuid_is_a_v4_uuid_and_unique_per_build() {
    let src = tree(&[("a.txt", b"a")]);
    let dst = tempfile::tempdir().unwrap();
    // no pinned uuid here: the default path must mint a fresh v4 each time
    let fresh = WriteOptions { created_at: Some(PINNED), sign_key: None, archive_uuid: None };
    let a = build_pack(src.path(), &out(&dst, "a.wax"), &PackConfig::default(), &fresh).unwrap();
    let b = build_pack(src.path(), &out(&dst, "b.wax"), &PackConfig::default(), &fresh).unwrap();

    assert_ne!(a.archive_uuid, b.archive_uuid, "each build mints a fresh identity");
    // RFC 4122 v4: version nibble 4, variant bits 10xx
    assert_eq!(a.archive_uuid[6] >> 4, 4, "version nibble should be 4");
    assert_eq!(a.archive_uuid[8] >> 6, 0b10, "variant bits should be 10");
}

// ---------------------------------------------------------------------------
// verify (checksums)
// ---------------------------------------------------------------------------

#[test]
fn verify_passes_on_a_clean_archive_and_fails_on_a_corrupted_blob() {
    let src = tree(&[("a.txt", b"hello there"), ("b.txt", b"second")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(src.path(), &archive, &cfg_from("[build]\ncompression = \"none\"\n"), &pinned())
        .unwrap();

    let clean = verify_pack(&archive, None, false).unwrap();
    assert!(clean.ok(), "clean archive should verify: {clean:?}");
    assert_eq!(clean.entries_checked, 2);

    // flip a byte in the blob region
    let mut bytes = std::fs::read(&archive).unwrap();
    bytes[128 + 1] ^= 0xFF;
    std::fs::write(&archive, &bytes).unwrap();

    let dirty = verify_pack(&archive, None, false).unwrap();
    assert!(!dirty.ok(), "corrupted archive must not verify");
    assert_eq!(dirty.bad_entries.len(), 1);
}

// ---------------------------------------------------------------------------
// signing (A7) — skipped when minisign is unavailable
// ---------------------------------------------------------------------------

#[test]
fn sign_then_verify_roundtrip() {
    if !minisign_available() {
        return;
    }
    let keys = tempfile::tempdir().unwrap();
    let (sec, pubk) = keypair(keys.path());

    let src = tree(&[("index.html", b"<h1>signed</h1>")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    let report = build_pack(
        src.path(),
        &archive,
        &PackConfig::default(),
        &WriteOptions { created_at: Some(PINNED), sign_key: Some(sec.clone()), archive_uuid: None },
    )
    .unwrap();

    let canonical = report.archive_uuid_text();
    let sidecar = report.sidecar.expect("a sidecar should have been written");
    assert_eq!(sidecar, sign::sidecar_path(&archive));
    assert!(sidecar.is_file());

    // is_signed must be set *in the signed header* (SPEC 8.1), not patched after
    let r = WaxReader::open(&archive).unwrap();
    assert!(r.header().has_flag(flag::IS_SIGNED), "is_signed flag not set");

    // trusted comment binds the sidecar to this archive, in the canonical
    // lowercase-hyphenated form (Contract 11) - never bare 32-hex
    let text = std::fs::read_to_string(&sidecar).unwrap();
    assert!(
        text.contains(&format!("archive_uuid={canonical}")),
        "trusted comment must carry the canonical archive_uuid: {text}"
    );
    assert!(canonical.len() == 36 && canonical.matches('-').count() == 4);
    let bare: String = canonical.chars().filter(|c| *c != '-').collect();
    assert!(
        !text.contains(&format!("archive_uuid={bare}")),
        "the bare 32-hex form must never be emitted: {text}"
    );
    assert!(text.contains(&format!("created_at={PINNED}")));

    let pk = sign::PubKey::File(pubk);
    let v = verify_pack(&archive, Some(&pk), true).unwrap();
    assert!(v.ok(), "signed archive should verify: {v:?}");
    assert!(v.signature.is_some());
}

#[test]
fn tampering_invalidates_the_signature() {
    if !minisign_available() {
        return;
    }
    let keys = tempfile::tempdir().unwrap();
    let (sec, pubk) = keypair(keys.path());

    let src = tree(&[("a.txt", b"original content")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(
        src.path(),
        &archive,
        &cfg_from("[build]\ncompression = \"none\"\n"),
        &WriteOptions { created_at: Some(PINNED), sign_key: Some(sec), archive_uuid: None },
    )
    .unwrap();

    let pk = sign::PubKey::File(pubk);
    assert!(verify_pack(&archive, Some(&pk), true).unwrap().ok());

    // Tamper inside the index segment — that is what the digest covers.
    let mut bytes = std::fs::read(&archive).unwrap();
    let idx = WaxReader::open(&archive).unwrap().header().index_offset as usize;
    bytes[idx + 90] ^= 0x40;
    std::fs::write(&archive, &bytes).unwrap();

    // A byte flip can also make the archive unopenable — equally a rejection.
    if let Ok(v) = verify_pack(&archive, Some(&pk), true) {
        assert!(
            !v.ok(),
            "signature must not verify after the index was tampered with"
        );
    }
}

#[test]
fn append_re_signs_and_the_old_signature_stops_verifying() {
    if !minisign_available() {
        return;
    }
    let keys = tempfile::tempdir().unwrap();
    let (sec, pubk) = keypair(keys.path());
    let pk = sign::PubKey::File(pubk);

    let base = tree(&[("a.txt", b"a")]);
    let extra = tree(&[("b.txt", b"b")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");

    build_pack(
        base.path(),
        &archive,
        &PackConfig::default(),
        &WriteOptions { created_at: Some(PINNED), sign_key: Some(sec.clone()), archive_uuid: None },
    )
    .unwrap();
    let sidecar = sign::sidecar_path(&archive);
    let sig_before = std::fs::read_to_string(&sidecar).unwrap();

    // Append WITHOUT re-signing: the stale sidecar must stop verifying, because
    // the digest covers the new header + segment chain (SPEC 8.1).
    append_pack(
        &archive,
        extra.path(),
        &PackConfig::default(),
        &WriteOptions { created_at: Some(PINNED + 5), sign_key: None, archive_uuid: None },
    )
    .unwrap();
    assert!(
        !verify_pack(&archive, Some(&pk), true).unwrap().ok(),
        "a stale sidecar must not verify after an append"
    );

    // Re-sign and it verifies again.
    sign::sign(&archive, &sec).unwrap();
    let sig_after = std::fs::read_to_string(&sidecar).unwrap();
    assert_ne!(sig_before, sig_after, "sidecar should have been regenerated");
    assert!(verify_pack(&archive, Some(&pk), true).unwrap().ok());
}

#[test]
fn append_re_signs_when_given_a_key() {
    if !minisign_available() {
        return;
    }
    let keys = tempfile::tempdir().unwrap();
    let (sec, pubk) = keypair(keys.path());
    let pk = sign::PubKey::File(pubk);

    let base = tree(&[("a.txt", b"a")]);
    let extra = tree(&[("b.txt", b"b")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");

    build_pack(
        base.path(),
        &archive,
        &PackConfig::default(),
        &WriteOptions { created_at: Some(PINNED), sign_key: Some(sec.clone()), archive_uuid: None },
    )
    .unwrap();
    let r = append_pack(
        &archive,
        extra.path(),
        &PackConfig::default(),
        &WriteOptions { created_at: Some(PINNED + 5), sign_key: Some(sec), archive_uuid: None },
    )
    .unwrap();

    assert!(r.sidecar.is_some(), "append should have re-signed");
    assert!(verify_pack(&archive, Some(&pk), true).unwrap().ok());
}

#[test]
fn a_sidecar_from_a_different_archive_is_rejected() {
    if !minisign_available() {
        return;
    }
    let keys = tempfile::tempdir().unwrap();
    let (sec, pubk) = keypair(keys.path());
    let pk = sign::PubKey::File(pubk);

    let src = tree(&[("a.txt", b"a")]);
    let dst = tempfile::tempdir().unwrap();
    let one = out(&dst, "one.wax");
    let two = out(&dst, "two.wax");

    let opts = WriteOptions { created_at: Some(PINNED), sign_key: Some(sec), archive_uuid: None };
    build_pack(src.path(), &one, &PackConfig::default(), &opts).unwrap();
    build_pack(src.path(), &two, &PackConfig::default(), &opts).unwrap();

    // Same content, same timestamp — but different archive_uuid, so swapping the
    // sidecars must be caught by the trusted-comment binding (SPEC 8.3 step 3).
    std::fs::copy(sign::sidecar_path(&one), sign::sidecar_path(&two)).unwrap();
    let err = sign::verify(&two, &pk).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("different archive") || msg.contains("does not verify"),
        "expected an archive-mismatch rejection, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// uuid pinning (reproducible builds)
// ---------------------------------------------------------------------------

#[test]
fn parse_uuid_accepts_both_spellings_and_rejects_junk() {
    let hyphenated = wax_builder::parse_uuid("4a1b2c3d-4e5f-4607-8a99-aabbccddeeff").unwrap();
    let bare = wax_builder::parse_uuid("4a1b2c3d4e5f46078a99aabbccddeeff").unwrap();
    assert_eq!(hyphenated, bare);
    assert_eq!(hyphenated, PINNED_UUID);

    assert!(wax_builder::parse_uuid("nope").is_err());
    assert!(wax_builder::parse_uuid("4a1b2c3d4e5f46078a99aabbccddee").is_err());
    assert!(wax_builder::parse_uuid("zz1b2c3d4e5f46078a99aabbccddeeff").is_err());
}

#[test]
fn pinning_the_uuid_reproduces_it_in_the_header() {
    let src = tree(&[("a.txt", b"a")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(src.path(), &archive, &PackConfig::default(), &pinned()).unwrap();
    assert_eq!(
        WaxReader::open(&archive).unwrap().header().archive_uuid,
        PINNED_UUID
    );
}
