//! A2b conformance — B3 manifest schema enforcement.
//!
//! Source of truth: `docs/track-a-refinement.md` §16, which reproduces Track B's
//! B3 field table and states it is exhaustive, not illustrative.

use std::path::PathBuf;
use tempfile::TempDir;
use wax_builder::config::PackConfig;
use wax_builder::{append_pack, build_pack, WriteOptions};
use wax_core::WaxReader;

const PINNED: u64 = 1_700_000_000;
const PINNED_UUID: [u8; 16] = [
    0x4a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f, 0x46, 0x07, 0x8a, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];

/// A complete, valid `[manifest]`. `icon` and `entry_point` name entries that
/// [`manifest_tree`] actually contains.
const VALID_MANIFEST: &str = r#"
[manifest]
name = "Community Science Library"
icon = "img/icon.svg"
category = "reference"
license = "CC-BY-SA-4.0"
attribution = "Nairobi Community Trust"
version = "2026.09.1"
min_ram_bytes = 2147483648
min_storage_bytes = 34359738368
arch = "any"
entry_point = "index.html"
"#;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn tree(files: &[(&str, &[u8])]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }
    dir
}

fn manifest_tree() -> TempDir {
    tree(&[
        ("index.html", b"<h1>Home</h1>"),
        ("img/icon.svg", b"<svg/>"),
        ("articles/a.html", b"<p>a</p>"),
    ])
}

fn cfg_from(toml_src: &str) -> PackConfig {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("wax-pack.toml");
    std::fs::write(&p, toml_src).unwrap();
    PackConfig::load(&p).unwrap()
}

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

/// `VALID_MANIFEST` with the line for `key` removed.
fn manifest_without(key: &str) -> String {
    VALID_MANIFEST
        .lines()
        .filter(|l| !l.trim_start().starts_with(&format!("{key} ")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `VALID_MANIFEST` with an extra line appended. Needed for keys that are not
/// in the canonical manifest at all — an optional field, or a retired one.
fn manifest_plus(extra: &str) -> String {
    format!("{}{extra}\n", VALID_MANIFEST)
}

/// `VALID_MANIFEST` with the line for `key` replaced by `replacement`.
fn manifest_with(key: &str, replacement: &str) -> String {
    VALID_MANIFEST
        .lines()
        .map(|l| {
            if l.trim_start().starts_with(&format!("{key} ")) {
                replacement.to_string()
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build with `cfg_src` and return the rendered error.
fn build_err(cfg_src: &str) -> String {
    let src = manifest_tree();
    let dst = tempfile::tempdir().unwrap();
    let cfg = cfg_from(cfg_src);
    let err = build_pack(src.path(), &out(&dst, "p.wax"), &cfg, &pinned())
        .expect_err("build should have been rejected");
    format!("{err:#}")
}

/// Build with `cfg_src`, expecting success; returns the archive path + its dir.
fn build_ok(cfg_src: &str) -> (TempDir, PathBuf) {
    let src = manifest_tree();
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    let cfg = cfg_from(cfg_src);
    build_pack(src.path(), &archive, &cfg, &pinned()).expect("build should have succeeded");
    // keep `src` alive only until the build finishes; `dst` owns the archive
    drop(src);
    (dst, archive)
}

// ---------------------------------------------------------------------------
// required fields — one case each
// ---------------------------------------------------------------------------

macro_rules! required_field_case {
    ($test:ident, $field:literal) => {
        #[test]
        fn $test() {
            let msg = build_err(&manifest_without($field));
            assert!(
                msg.contains("missing required field") && msg.contains($field),
                "expected a missing-{} error, got: {msg}",
                $field
            );
        }
    };
}

required_field_case!(missing_name_is_rejected, "name");
required_field_case!(missing_icon_is_rejected, "icon");
required_field_case!(missing_category_is_rejected, "category");
required_field_case!(missing_license_is_rejected, "license");
required_field_case!(missing_attribution_is_rejected, "attribution");
required_field_case!(missing_version_is_rejected, "version");
required_field_case!(missing_min_ram_bytes_is_rejected, "min_ram_bytes");
required_field_case!(missing_min_storage_bytes_is_rejected, "min_storage_bytes");
required_field_case!(missing_arch_is_rejected, "arch");
required_field_case!(missing_entry_point_is_rejected, "entry_point");

#[test]
fn an_empty_required_value_counts_as_missing() {
    let msg = build_err(&manifest_with("name", r#"name = "   ""#));
    assert!(msg.contains("missing required field") && msg.contains("name"), "{msg}");
}

#[test]
fn all_ten_required_fields_are_reported_together() {
    // a manifest with only optional keys names every missing required field
    let msg = build_err("[manifest]\nlanguages = \"en\"\n");
    for f in [
        "name",
        "icon",
        "category",
        "license",
        "attribution",
        "version",
        "min_ram_bytes",
        "min_storage_bytes",
        "arch",
        "entry_point",
    ] {
        assert!(msg.contains(f), "error should name {f}: {msg}");
    }
}

// ---------------------------------------------------------------------------
// closed enums
// ---------------------------------------------------------------------------

#[test]
fn bad_category_is_rejected() {
    let msg = build_err(&manifest_with("category", r#"category = "banana""#));
    assert!(msg.contains("category") && msg.contains("not permitted"), "{msg}");
    assert!(msg.contains("reference"), "error should list the domain: {msg}");
}

#[test]
fn every_permitted_category_is_accepted() {
    for c in ["reference", "education", "media", "tools", "civic", "health"] {
        let cfg = manifest_with("category", &format!(r#"category = "{c}""#));
        let (_dir, archive) = build_ok(&cfg);
        let r = WaxReader::open(&archive).unwrap();
        assert_eq!(r.manifest().get("category").map(String::as_str), Some(c));
    }
}

#[test]
fn bad_arch_is_rejected() {
    let msg = build_err(&manifest_with("arch", "arch = \"banana\""));
    assert!(msg.contains("arch") && msg.contains("not permitted"), "{msg}");
    assert!(msg.contains("aarch64"), "error should list the domain: {msg}");
}

#[test]
fn every_permitted_arch_is_accepted() {
    for a in ["aarch64", "x86_64", "any"] {
        let cfg = manifest_with("arch", &format!("arch = \"{a}\""));
        let (_dir, archive) = build_ok(&cfg);
        let r = WaxReader::open(&archive).unwrap();
        assert_eq!(r.manifest().get("arch").map(String::as_str), Some(a));
    }
}

#[test]
fn a_retired_board_tier_in_arch_is_diagnosed_as_such() {
    // The four names are gone as a vocabulary, so someone reaching for the old
    // field may put a board name in `arch`. Say what happened.
    for tier in ["pi_zero_2w", "pi_4", "pi_5", "mini_pc"] {
        let msg = build_err(&manifest_with("arch", &format!("arch = \"{tier}\"")));
        assert!(
            msg.contains("retired hardware tier"),
            "{tier:?} should be diagnosed as a retired tier, got: {msg}"
        );
        assert!(msg.contains("min_ram_bytes"), "should name the replacement: {msg}");
    }
}

#[test]
fn the_two_resource_floors_must_be_positive() {
    for field in ["min_ram_bytes", "min_storage_bytes"] {
        let msg = build_err(&manifest_with(field, &format!("{field} = 0")));
        assert!(msg.contains(field) && msg.contains("positive"), "{msg}");
        assert!(msg.contains("gates nothing"), "should say why zero is wrong: {msg}");
    }
}

#[test]
fn gpu_is_a_closed_set_when_present() {
    for ok in ["required", "preferred"] {
        let cfg = manifest_plus(&format!("gpu = \"{ok}\""));
        let (_dir, archive) = build_ok(&cfg);
        let r = WaxReader::open(&archive).unwrap();
        assert_eq!(r.manifest().get("gpu").map(String::as_str), Some(ok));
    }
    let msg = build_err(&manifest_plus("gpu = \"none\""));
    assert!(msg.contains("gpu") && msg.contains("not permitted"), "{msg}");
    assert!(msg.contains("Omit the key"), "should say to omit, not write none: {msg}");
}

#[test]
fn deployment_profile_names_are_rejected_with_a_targeted_message() {
    // §16: an earlier draft used these for the hardware field; they are a
    // different axis entirely. Now checked against `arch`.
    for profile in ["Kiosk", "Classroom", "Community Hub", "Field Ops", "community_hub"] {
        let msg = build_err(&manifest_with("arch", &format!("arch = \"{profile}\"")));
        assert!(
            msg.contains("Deployment Profile"),
            "{profile:?} should be diagnosed as a Deployment Profile name, got: {msg}"
        );
        assert!(msg.contains("aarch64"), "should point at the real domain: {msg}");
    }
}

// ---------------------------------------------------------------------------
// total_size_bytes — removed from the schema (§17 / Track B §19)
// ---------------------------------------------------------------------------

#[test]
fn total_size_bytes_in_config_is_rejected_as_removed() {
    let cfg = format!("{VALID_MANIFEST}total_size_bytes = 12345\n");
    let msg = build_err(&cfg);
    assert!(msg.contains("total_size_bytes"), "{msg}");
    assert!(
        msg.contains("removed from the B3 schema"),
        "error should say the field was removed, not computed: {msg}"
    );
    assert!(
        msg.contains("packs.size"),
        "error should point at the catalog as where size lives: {msg}"
    );
}

#[test]
fn total_size_bytes_is_never_written() {
    // the field does not exist; no archive built from a valid manifest carries it
    let (_dir, archive) = build_ok(VALID_MANIFEST);
    let r = WaxReader::open(&archive).unwrap();
    assert!(
        r.manifest().get("total_size_bytes").is_none(),
        "total_size_bytes must not appear in a manifest (got {:?})",
        r.manifest().get("total_size_bytes")
    );
}

// ---------------------------------------------------------------------------
// id — removed from the schema
// ---------------------------------------------------------------------------

#[test]
fn id_in_config_is_rejected() {
    let cfg = format!("{VALID_MANIFEST}id = \"science-lib\"\n");
    let msg = build_err(&cfg);
    assert!(msg.contains("`id`"), "{msg}");
    assert!(
        msg.contains("archive_uuid"),
        "error should explain archive_uuid is the identity: {msg}"
    );
}

#[test]
fn id_is_never_written_even_when_other_keys_are_valid() {
    // belt and braces: no archive should ever carry an `id` manifest row
    let (_dir, archive) = build_ok(VALID_MANIFEST);
    let r = WaxReader::open(&archive).unwrap();
    assert!(r.manifest().get("id").is_none());
}

// ---------------------------------------------------------------------------
// unknown keys — the table is exhaustive
// ---------------------------------------------------------------------------

#[test]
fn unknown_manifest_keys_are_rejected() {
    let cfg = format!("{VALID_MANIFEST}some_future_key = \"value\"\n");
    let msg = build_err(&cfg);
    assert!(msg.contains("unknown manifest key"), "{msg}");
    assert!(msg.contains("some_future_key"), "{msg}");
    assert!(msg.contains("exhaustive"), "error should cite §16's wording: {msg}");
}

// ---------------------------------------------------------------------------
// icon / entry_point must name real entries
// ---------------------------------------------------------------------------

#[test]
fn icon_pointing_at_a_missing_entry_is_rejected() {
    let msg = build_err(&manifest_with("icon", r#"icon = "img/nope.svg""#));
    assert!(msg.contains("icon") && msg.contains("does not name an entry"), "{msg}");
}

#[test]
fn entry_point_pointing_at_a_missing_entry_is_rejected() {
    let msg = build_err(&manifest_with("entry_point", r#"entry_point = "missing.html""#));
    assert!(
        msg.contains("entry_point") && msg.contains("does not name an entry"),
        "{msg}"
    );
}

#[test]
fn a_data_uri_icon_is_rejected() {
    let msg = build_err(&manifest_with(
        "icon",
        r#"icon = "data:image/svg+xml;base64,PHN2Zy8+""#,
    ));
    assert!(msg.contains("data") && msg.contains("URI"), "{msg}");
}

#[test]
fn a_root_level_icon_is_valid() {
    // §17 corrects §16's "not a filename alone": a root-level icon.svg is both a
    // bare filename and a perfectly valid archive path. The check is existence,
    // not depth.
    let src = tree(&[
        ("index.html", b"<h1>Home</h1>"),
        ("icon.svg", b"<svg/>"),
    ]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    let cfg = cfg_from(&manifest_with("icon", r#"icon = "icon.svg""#));
    build_pack(src.path(), &archive, &cfg, &pinned()).expect("root-level icon must build");
    let r = WaxReader::open(&archive).unwrap();
    assert_eq!(r.manifest().get("icon").map(String::as_str), Some("icon.svg"));
}

#[test]
fn a_leading_slash_or_dot_slash_is_tolerated() {
    // natural things to write in a config; both resolve to the same entry
    for spelling in ["/index.html", "./index.html"] {
        let cfg = manifest_with("entry_point", &format!(r#"entry_point = "{spelling}""#));
        let (_dir, archive) = build_ok(&cfg);
        let r = WaxReader::open(&archive).unwrap();
        assert_eq!(
            r.manifest().get("entry_point").map(String::as_str),
            Some(spelling),
            "the configured spelling is preserved verbatim in the manifest"
        );
    }
}

// ---------------------------------------------------------------------------
// optional fields — omitted, never defaulted
// ---------------------------------------------------------------------------

#[test]
fn unset_optional_fields_are_omitted_not_zeroed() {
    let (_dir, archive) = build_ok(VALID_MANIFEST);
    let r = WaxReader::open(&archive).unwrap();
    let m = r.manifest();
    for k in [
        "guest_accessible",
        "runtime_ram_bytes",
        "runtime_storage_bytes",
        "languages",
        "depends_on",
    ] {
        assert!(
            m.get(k).is_none(),
            "{k} is unset and must be omitted, not written as 0/false/empty (got {:?})",
            m.get(k)
        );
    }
}

#[test]
fn set_optional_fields_round_trip() {
    let cfg = format!(
        "{VALID_MANIFEST}runtime_ram_bytes = 268435456\n\
         runtime_storage_bytes = 1073741824\n\
         languages = \"en,sw,fr\"\n\
         depends_on = \"{UUID_A},{UUID_B}\"\n"
    );
    let (_dir, archive) = build_ok(&cfg);
    let r = WaxReader::open(&archive).unwrap();
    let m = r.manifest();
    assert_eq!(m.get("runtime_ram_bytes").map(String::as_str), Some("268435456"));
    assert_eq!(m.get("runtime_storage_bytes").map(String::as_str), Some("1073741824"));
    assert_eq!(m.get("languages").map(String::as_str), Some("en,sw,fr"));
    assert_eq!(
        m.get("depends_on").map(String::as_str),
        Some(format!("{UUID_A},{UUID_B}").as_str())
    );
}

// ---------------------------------------------------------------------------
// depends_on — comma-separated archive_uuid values (§17 / Track B §19)
// ---------------------------------------------------------------------------

const UUID_A: &str = "4a1b2c3d-4e5f-4607-8a99-aabbccddeeff";
const UUID_B: &str = "0f1e2d3c-4b5a-4968-8776-655443322110";

#[test]
fn depends_on_accepts_hyphenated_uuids() {
    let cfg = format!("{VALID_MANIFEST}depends_on = \"{UUID_A},{UUID_B}\"\n");
    let (_dir, archive) = build_ok(&cfg);
    let r = WaxReader::open(&archive).unwrap();
    assert_eq!(
        r.manifest().get("depends_on").map(String::as_str),
        Some(format!("{UUID_A},{UUID_B}").as_str()),
        "canonical input is written unchanged"
    );
}

#[test]
fn depends_on_accepts_a_single_uuid() {
    let cfg = format!("{VALID_MANIFEST}depends_on = \"{UUID_A}\"\n");
    let (_dir, archive) = build_ok(&cfg);
    assert_eq!(
        WaxReader::open(&archive).unwrap().manifest().get("depends_on").map(String::as_str),
        Some(UUID_A)
    );
}

#[test]
fn depends_on_accepts_bare_hex_but_writes_the_canonical_form() {
    // Contract 11: bare 32-hex is accepted on input, normalized on write,
    // never emitted.
    let bare = UUID_A.replace('-', "");
    let cfg = format!("{VALID_MANIFEST}depends_on = \"{bare}\"\n");
    let (_dir, archive) = build_ok(&cfg);
    assert_eq!(
        WaxReader::open(&archive).unwrap().manifest().get("depends_on").map(String::as_str),
        Some(UUID_A),
        "bare input must be written in canonical hyphenated form"
    );
}

#[test]
fn depends_on_normalizes_case_and_whitespace_on_write() {
    let upper = UUID_A.to_uppercase();
    let cfg = format!("{VALID_MANIFEST}depends_on = \" {upper} ,{UUID_B}\"\n");
    let (_dir, archive) = build_ok(&cfg);
    assert_eq!(
        WaxReader::open(&archive).unwrap().manifest().get("depends_on").map(String::as_str),
        Some(format!("{UUID_A},{UUID_B}").as_str()),
        "written form is lowercase, hyphenated, no padding"
    );
}

#[test]
fn depends_on_tolerates_whitespace_around_commas() {
    let cfg = format!("{VALID_MANIFEST}depends_on = \"{UUID_A} , {UUID_B}\"\n");
    let (_dir, _archive) = build_ok(&cfg);
}

#[test]
fn depends_on_rejects_a_pack_name() {
    // the old "pack ids" wording; a name is not an identity (§16)
    let cfg = format!("{VALID_MANIFEST}depends_on = \"base-fonts\"\n");
    let msg = build_err(&cfg);
    assert!(msg.contains("depends_on"), "{msg}");
    assert!(msg.contains("not an archive_uuid"), "{msg}");
    assert!(msg.contains("base-fonts"), "should name the offending element: {msg}");
}

#[test]
fn depends_on_rejects_one_bad_element_among_good_ones() {
    let cfg = format!("{VALID_MANIFEST}depends_on = \"{UUID_A},not-a-uuid,{UUID_B}\"\n");
    let msg = build_err(&cfg);
    assert!(msg.contains("not-a-uuid"), "{msg}");
}

#[test]
fn depends_on_rejects_an_empty_element() {
    for raw in [
        format!("{UUID_A},,{UUID_B}"),
        format!("{UUID_A},"),
        format!(",{UUID_A}"),
        String::new(),
    ] {
        let cfg = format!("{VALID_MANIFEST}depends_on = \"{raw}\"\n");
        let msg = build_err(&cfg);
        assert!(msg.contains("empty element"), "{raw:?} -> {msg}");
    }
}

#[test]
fn depends_on_does_not_check_that_the_pack_exists() {
    // a well-formed UUID that no pack has ever carried must still build:
    // resolution is the catalog's job (§17)
    let cfg = format!("{VALID_MANIFEST}depends_on = \"00000000-0000-4000-8000-000000000000\"\n");
    let (_dir, _archive) = build_ok(&cfg);
}

// ---------------------------------------------------------------------------
// guest_accessible — optional boolean, default false (Contract §11)
// ---------------------------------------------------------------------------

#[test]
fn guest_accessible_true_is_written() {
    let cfg = format!("{VALID_MANIFEST}guest_accessible = true\n");
    let (_dir, archive) = build_ok(&cfg);
    let r = WaxReader::open(&archive).unwrap();
    assert_eq!(r.manifest().get("guest_accessible").map(String::as_str), Some("true"));
}

#[test]
fn guest_accessible_explicit_false_is_written() {
    // an explicit false is an authored statement and is preserved; only an
    // *unset* value is omitted
    let cfg = format!("{VALID_MANIFEST}guest_accessible = false\n");
    let (_dir, archive) = build_ok(&cfg);
    let r = WaxReader::open(&archive).unwrap();
    assert_eq!(r.manifest().get("guest_accessible").map(String::as_str), Some("false"));
}

#[test]
fn guest_accessible_must_be_a_boolean() {
    // TOML typing: a string "yes" is not a bool and fails at parse
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("wax-pack.toml");
    std::fs::write(&p, format!("{VALID_MANIFEST}guest_accessible = \"yes\"\n")).unwrap();
    let err = PackConfig::load(&p).unwrap_err();
    assert!(format!("{err:#}").contains("guest_accessible"), "{err:#}");
}

// ---------------------------------------------------------------------------
// version — CalVer YYYY.MM.N (Contract §11)
// ---------------------------------------------------------------------------

#[test]
fn version_accepts_calver() {
    for v in ["2026.09.1", "2026.01.1", "2026.12.42", "1999.06.7"] {
        let cfg = manifest_with("version", &format!(r#"version = "{v}""#));
        let (_dir, archive) = build_ok(&cfg);
        assert_eq!(
            WaxReader::open(&archive).unwrap().manifest().get("version").map(String::as_str),
            Some(v)
        );
    }
}

#[test]
fn version_rejects_semver_and_other_shapes() {
    for (v, why) in [
        ("1.0.0", "semver"),
        ("2026.9.1", "month not zero-padded"),
        ("2026.13.1", "month out of range"),
        ("2026.00.1", "month zero"),
        ("2026.09", "two components"),
        ("2026.09.1.2", "four components"),
        ("26.09.1", "two-digit year"),
        ("2026.09.01", "leading zero on release"),
        ("2026.09.0", "release counter starts at 1"),
        ("2026.09.x", "non-numeric release"),
        ("v2026.09.1", "prefix"),
    ] {
        let msg = build_err(&manifest_with("version", &format!(r#"version = "{v}""#)));
        assert!(
            msg.contains("not CalVer") && msg.contains("YYYY.MM.N"),
            "{v:?} ({why}) should be rejected as not CalVer, got: {msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// languages — BCP-47 per element (Contract §11)
// ---------------------------------------------------------------------------

#[test]
fn languages_accepts_well_formed_bcp47() {
    let cfg = format!("{VALID_MANIFEST}languages = \"en, pt-BR ,zh-Hans,sw,fr-CA,es-419\"\n");
    let (_dir, archive) = build_ok(&cfg);
    assert_eq!(
        WaxReader::open(&archive).unwrap().manifest().get("languages").map(String::as_str),
        Some("en,pt-BR,zh-Hans,sw,fr-CA,es-419"),
        "elements are trimmed; tags are written as given"
    );
}

#[test]
fn languages_rejects_malformed_tags() {
    for bad in [
        "english",       // well-formed 7-letter subtag, but not a registered language
        "zz",            // well-formed, not in the registry
        "en_US",         // underscore is not a subtag separator
        "e",             // one-letter primary subtag
        "en-",
        "-en",
        "en--US",
        "zh-Hans-",
        "toolongsubtag",
    ] {
        let cfg = format!("{VALID_MANIFEST}languages = \"en,{bad}\"\n");
        let msg = build_err(&cfg);
        assert!(
            msg.contains("BCP-47") && msg.contains(bad),
            "{bad:?} should be rejected as malformed BCP-47, got: {msg}"
        );
    }
}

#[test]
fn languages_rejects_an_empty_element() {
    for raw in ["en,,fr", "en,", ",en", ""] {
        let cfg = format!("{VALID_MANIFEST}languages = \"{raw}\"\n");
        let msg = build_err(&cfg);
        assert!(msg.contains("empty element"), "{raw:?} -> {msg}");
    }
}

// ---------------------------------------------------------------------------
// license — three outcomes (Contract §11)
// ---------------------------------------------------------------------------

#[test]
fn blank_license_is_a_hard_failure() {
    for blank in [r#"license = """#, r#"license = "   ""#] {
        let msg = build_err(&manifest_with("license", blank));
        assert!(msg.contains("missing required field") && msg.contains("license"), "{msg}");
    }
    let msg = build_err(&manifest_without("license"));
    assert!(msg.contains("license"), "{msg}");
}

#[test]
fn every_allowlisted_license_builds_clean() {
    let src = manifest_tree();
    let dst = tempfile::tempdir().unwrap();
    for id in wax_builder::config::LICENSE_ALLOWLIST {
        let cfg = cfg_from(&manifest_with("license", &format!(r#"license = "{id}""#)));
        let archive = out(&dst, &format!("{}.wax", id.replace(['.', '+'], "_")));
        let report = build_pack(src.path(), &archive, &cfg, &pinned()).unwrap();
        assert!(
            !report.license_review_required(),
            "{id} is allowlisted and must build clean"
        );
        assert_eq!(report.license.as_ref().unwrap().license(), id);
    }
}

#[test]
fn free_text_license_builds_with_review_required() {
    let src = manifest_tree();
    let dst = tempfile::tempdir().unwrap();
    for text in [
        "All rights reserved, see COPYING",
        "Creative Commons Attribution-ShareAlike",
        "public domain",
    ] {
        let cfg = cfg_from(&manifest_with("license", &format!(r#"license = "{text}""#)));
        let archive = out(&dst, "p.wax");
        let report = build_pack(src.path(), &archive, &cfg, &pinned())
            .expect("free-text license must still build");
        assert!(report.license_review_required(), "{text:?} must route to review");
        // and the manifest carries the text verbatim, with no review flag inside it
        let r = WaxReader::open(&archive).unwrap();
        assert_eq!(r.manifest().get("license").map(String::as_str), Some(text));
        assert!(r.manifest().get("license_review_required").is_none());
    }
}

#[test]
fn recognized_but_not_allowlisted_spdx_id_routes_to_review() {
    // valid SPDX ids that Contract 11 deliberately leaves off the allowlist
    let src = manifest_tree();
    let dst = tempfile::tempdir().unwrap();
    for id in ["GPL-2.0-or-later", "BSD-3-Clause", "CC-BY-NC-4.0", "LGPL-3.0-only", "MPL-2.0"] {
        let cfg = cfg_from(&manifest_with("license", &format!(r#"license = "{id}""#)));
        let report = build_pack(src.path(), &out(&dst, "p.wax"), &cfg, &pinned()).unwrap();
        assert!(report.license_review_required(), "{id} is not allowlisted");
    }
}

#[test]
fn allowlist_match_is_case_insensitive_and_written_canonically() {
    // Contract §11: matching is case-insensitive, normalized to SPDX's canonical
    // casing on write. "Literal" constrains which ids are allowed, not their case.
    let src = manifest_tree();
    let dst = tempfile::tempdir().unwrap();
    for (spelling, canonical) in [
        ("mit", "MIT"),
        ("Mit", "MIT"),
        ("cc-by-sa-4.0", "CC-BY-SA-4.0"),
        ("CC-by-SA-4.0", "CC-BY-SA-4.0"),
        ("apache-2.0", "Apache-2.0"),
        ("gpl-3.0-or-later", "GPL-3.0-or-later"),
    ] {
        let cfg = cfg_from(&manifest_with("license", &format!(r#"license = "{spelling}""#)));
        let archive = out(&dst, "p.wax");
        let report = build_pack(src.path(), &archive, &cfg, &pinned()).unwrap();
        assert!(!report.license_review_required(), "{spelling:?} must build clean");
        assert_eq!(report.license.as_ref().unwrap().license(), canonical);
        let r = WaxReader::open(&archive).unwrap();
        assert_eq!(
            r.manifest().get("license").map(String::as_str),
            Some(canonical),
            "{spelling:?} must be written in canonical casing"
        );
    }
}

#[test]
fn near_misses_that_are_not_case_variants_still_route_to_review() {
    let src = manifest_tree();
    let dst = tempfile::tempdir().unwrap();
    for near_miss in ["CC-BY-SA", "Apache 2.0", "GPL-3.0", "MIT License", "cc0"] {
        let cfg = cfg_from(&manifest_with("license", &format!(r#"license = "{near_miss}""#)));
        let report = build_pack(src.path(), &out(&dst, "p.wax"), &cfg, &pinned()).unwrap();
        assert!(report.license_review_required(), "{near_miss:?} is not on the allowlist");
    }
}

#[test]
fn append_report_carries_the_archives_licensing_outcome() {
    let src = manifest_tree();
    let extra = tree(&[("articles/b.html", b"<p>b</p>")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    let cfg = cfg_from(&manifest_with("license", r#"license = "see COPYING""#));
    build_pack(src.path(), &archive, &cfg, &pinned()).unwrap();
    let report = append_pack(&archive, extra.path(), &cfg, &pinned()).unwrap();
    assert!(report.license_review_required());
}

// ---------------------------------------------------------------------------
// min_hw_tier — retired; a build that still declares it gets a migration path
// ---------------------------------------------------------------------------

#[test]
fn retired_min_hw_tier_is_rejected_with_a_migration_message() {
    // Replaces the old `generic is box-reported` case: there is no tier axis
    // left for `generic` to be excluded from. What matters now is that a
    // wax-pack.toml carried over from before the migration fails with the
    // fields to use, not with "unknown key".
    for spelling in ["pi_zero_2w", "pi_4", "pi_5", "mini_pc", "generic"] {
        let msg = build_err(&manifest_plus(&format!("min_hw_tier = \"{spelling}\"")));
        assert!(
            msg.contains("was retired"),
            "{spelling:?} should be diagnosed as the retired field, got: {msg}"
        );
        for field in ["min_ram_bytes", "min_storage_bytes", "arch"] {
            assert!(msg.contains(field), "migration message should name {field}: {msg}");
        }
        assert!(
            msg.contains("still open"),
            "message should say existing packs are unaffected: {msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// the build report — Contract §11 "The build report"
// ---------------------------------------------------------------------------

fn read_report(archive: &std::path::Path) -> serde_json::Value {
    let p = wax_builder::BuildReport::path_for(archive);
    assert!(p.is_file(), "build report must be written by default at {}", p.display());
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

#[test]
fn build_report_is_written_by_default_beside_the_archive() {
    let (_dir, archive) = build_ok(VALID_MANIFEST);
    let p = wax_builder::BuildReport::path_for(&archive);
    assert_eq!(
        p.file_name().unwrap().to_string_lossy(),
        "p.wax.build-report.json",
        "name is <archive-filename>.build-report.json"
    );
    assert_eq!(p.parent(), archive.parent());
    assert!(p.is_file());
}

#[test]
fn build_report_has_exactly_the_contract_fields() {
    let (_dir, archive) = build_ok(VALID_MANIFEST);
    let v = read_report(&archive);
    let obj = v.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "archive_filename",
            "archive_uuid",
            "builder_version",
            "built_at",
            "entry_count",
            "license",
            "license_review_required",
            "redirect_count",
            "report_version",
            "signed",
            "skipped_count",
            "warnings",
        ]
    );
    assert_eq!(v["report_version"], 1);
    assert_eq!(v["archive_uuid"], "4a1b2c3d-4e5f-4607-8a99-aabbccddeeff");
    assert_eq!(v["archive_filename"], "p.wax");
    assert!(v["built_at"].as_u64().unwrap() > 1_600_000_000_000, "epoch milliseconds");
    assert!(v["builder_version"].as_str().unwrap().starts_with("wax-builder "));
    assert_eq!(v["entry_count"], 3);
    assert_eq!(v["redirect_count"], 0);
    assert_eq!(v["skipped_count"], 0);
    assert_eq!(v["license"], "CC-BY-SA-4.0");
    assert_eq!(v["license_review_required"], false);
    assert_eq!(v["signed"], false);
    assert_eq!(v["warnings"], serde_json::json!([]));
}

#[test]
fn build_report_carries_review_required_for_free_text() {
    let src = manifest_tree();
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    let cfg = cfg_from(&manifest_with("license", r#"license = "see COPYING""#));
    build_pack(src.path(), &archive, &cfg, &pinned()).unwrap();
    let v = read_report(&archive);
    assert_eq!(v["license"], "see COPYING");
    assert_eq!(v["license_review_required"], true);
}

#[test]
fn build_report_counts_redirects_and_caller_warnings() {
    use wax_builder::{build_from_entries, BuildContext, Warnings};
    use wax_core::{Compression, EntryInput};
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    let mut warnings = Warnings::new();
    warnings.add("unsupported_mimetype", 3);
    warnings.bump("redirect_cycle");
    warnings.bump("redirect_cycle");
    let entries = vec![
        EntryInput::data("index.html", b"<h1/>".to_vec(), Compression::None),
        EntryInput::data("icon.svg", b"<svg/>".to_vec(), Compression::None),
        EntryInput::redirect("home.html", "index.html"),
    ];
    let cfg = cfg_from(&manifest_with("icon", r#"icon = "icon.svg""#));
    let report = build_from_entries(
        &archive,
        entries,
        &cfg.manifest,
        &pinned(),
        BuildContext {
            builder_version: Some("zim2wax 0.1.0".to_string()),
            warnings,
            skipped_count: 5,
            force_license_review: false,
        },
    )
    .unwrap();
    assert_eq!(report.redirect_count, 1);
    let v = read_report(&archive);
    assert_eq!(v["builder_version"], "zim2wax 0.1.0");
    assert_eq!(v["entry_count"], 3);
    assert_eq!(v["redirect_count"], 1);
    assert_eq!(v["skipped_count"], 5);
    // one entry per code with a count, sorted by code
    assert_eq!(
        v["warnings"],
        serde_json::json!([
            {"code": "redirect_cycle", "count": 2},
            {"code": "unsupported_mimetype", "count": 3}
        ])
    );
}

#[test]
fn a_caller_can_force_license_review_on_an_allowlisted_id() {
    // Contract §11: an operator-supplied license is reviewed, not trusted
    use wax_builder::{build_from_entries, BuildContext};
    use wax_core::{Compression, EntryInput};
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    let entries = vec![
        EntryInput::data("index.html", b"<h1/>".to_vec(), Compression::None),
        EntryInput::data("icon.svg", b"<svg/>".to_vec(), Compression::None),
    ];
    let cfg = cfg_from(&manifest_with("icon", r#"icon = "icon.svg""#)); // license = CC-BY-SA-4.0
    let report = build_from_entries(
        &archive,
        entries,
        &cfg.manifest,
        &pinned(),
        BuildContext {
            force_license_review: true,
            ..BuildContext::default()
        },
    )
    .unwrap();
    assert!(report.license_review_required());
    assert_eq!(report.license.as_ref().unwrap().license(), "CC-BY-SA-4.0");
    let v = read_report(&archive);
    assert_eq!(v["license"], "CC-BY-SA-4.0");
    assert_eq!(v["license_review_required"], true);
    // the manifest still carries the canonical id; the review flag is report-only
    let r = WaxReader::open(&archive).unwrap();
    assert_eq!(r.manifest().get("license").map(String::as_str), Some("CC-BY-SA-4.0"));
}

#[test]
fn append_refreshes_the_build_report() {
    let src = manifest_tree();
    let extra = tree(&[("articles/b.html", b"<p>b</p>")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    let cfg = cfg_from(VALID_MANIFEST);
    build_pack(src.path(), &archive, &cfg, &pinned()).unwrap();
    assert_eq!(read_report(&archive)["entry_count"], 3);
    append_pack(&archive, extra.path(), &cfg, &pinned()).unwrap();
    assert_eq!(read_report(&archive)["entry_count"], 4, "report reflects the appended state");
}

// ---------------------------------------------------------------------------
// full round-trip
// ---------------------------------------------------------------------------

#[test]
fn a_valid_manifest_round_trips_unchanged() {
    let (_dir, archive) = build_ok(VALID_MANIFEST);
    let r = WaxReader::open(&archive).unwrap();
    let m = r.manifest();

    let expected = [
        ("name", "Community Science Library"),
        ("icon", "img/icon.svg"),
        ("category", "reference"),
        ("license", "CC-BY-SA-4.0"),
        ("attribution", "Nairobi Community Trust"),
        ("version", "2026.09.1"),
        ("min_ram_bytes", "2147483648"),
        ("min_storage_bytes", "34359738368"),
        ("arch", "any"),
        ("entry_point", "index.html"),
    ];
    for (k, v) in expected {
        assert_eq!(m.get(k).map(String::as_str), Some(v), "manifest.{k}");
    }
    // and nothing else: no computed fields, no defaults for unset optionals
    // (guest_accessible included - "default false" is the reader's default,
    // not a row the builder writes)
    assert_eq!(
        m.len(),
        expected.len(),
        "manifest should hold exactly the 8 required fields, got {:?}",
        m.keys().collect::<Vec<_>>()
    );
}

#[test]
fn an_absent_manifest_section_still_builds() {
    // wax-core treats an empty manifest as valid and opaque (§15); enforcement
    // applies to a pack that declares one.
    let src = manifest_tree();
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    build_pack(src.path(), &archive, &PackConfig::default(), &pinned()).unwrap();
    assert!(WaxReader::open(&archive).unwrap().manifest().is_empty());
}

// ---------------------------------------------------------------------------
// append interaction
// ---------------------------------------------------------------------------

#[test]
fn append_accepts_an_unchanged_manifest() {
    let src = manifest_tree();
    let extra = tree(&[("articles/b.html", b"<p>b</p>")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");
    let cfg = cfg_from(VALID_MANIFEST);

    build_pack(src.path(), &archive, &cfg, &pinned()).unwrap();
    append_pack(&archive, extra.path(), &cfg, &pinned())
        .expect("an unchanged manifest must not block an append");
    assert_eq!(WaxReader::open(&archive).unwrap().segment_count(), 2);
}

#[test]
fn append_still_rejects_a_genuinely_changed_manifest() {
    let src = manifest_tree();
    let extra = tree(&[("articles/b.html", b"<p>b</p>")]);
    let dst = tempfile::tempdir().unwrap();
    let archive = out(&dst, "p.wax");

    build_pack(src.path(), &archive, &cfg_from(VALID_MANIFEST), &pinned()).unwrap();
    let renamed = manifest_with("name", r#"name = "Renamed Pack""#);
    let err = append_pack(&archive, extra.path(), &cfg_from(&renamed), &pinned()).unwrap_err();
    assert!(format!("{err:#}").contains("immutable across appends"), "{err:#}");
}
