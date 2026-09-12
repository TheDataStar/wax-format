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
min_hw_tier = "pi_zero_2w"
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
required_field_case!(missing_min_hw_tier_is_rejected, "min_hw_tier");
required_field_case!(missing_entry_point_is_rejected, "entry_point");

#[test]
fn an_empty_required_value_counts_as_missing() {
    let msg = build_err(&manifest_with("name", r#"name = "   ""#));
    assert!(msg.contains("missing required field") && msg.contains("name"), "{msg}");
}

#[test]
fn all_eight_required_fields_are_reported_together() {
    // a manifest with only optional keys names every missing required field
    let msg = build_err("[manifest]\nlanguages = \"en\"\n");
    for f in [
        "name",
        "icon",
        "category",
        "license",
        "attribution",
        "version",
        "min_hw_tier",
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
fn bad_min_hw_tier_is_rejected() {
    let msg = build_err(&manifest_with("min_hw_tier", r#"min_hw_tier = "banana""#));
    assert!(msg.contains("min_hw_tier") && msg.contains("not permitted"), "{msg}");
    assert!(msg.contains("pi_zero_2w"), "error should list the domain: {msg}");
}

#[test]
fn every_permitted_min_hw_tier_is_accepted() {
    for t in ["pi_zero_2w", "pi_4", "pi_5", "mini_pc"] {
        let cfg = manifest_with("min_hw_tier", &format!(r#"min_hw_tier = "{t}""#));
        let (_dir, archive) = build_ok(&cfg);
        let r = WaxReader::open(&archive).unwrap();
        assert_eq!(r.manifest().get("min_hw_tier").map(String::as_str), Some(t));
    }
}

#[test]
fn deployment_profile_names_are_rejected_with_a_targeted_message() {
    // §16: an earlier draft used these; they are a different axis entirely.
    for profile in ["Kiosk", "Classroom", "Community Hub", "Field Ops", "community_hub"] {
        let msg = build_err(&manifest_with(
            "min_hw_tier",
            &format!(r#"min_hw_tier = "{profile}""#),
        ));
        assert!(
            msg.contains("Deployment Profile"),
            "{profile:?} should be diagnosed as a Deployment Profile name, got: {msg}"
        );
        assert!(msg.contains("pi_zero_2w"), "should point at the real tiers: {msg}");
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
        "runtime_ram_bytes",
        "runtime_storage_bytes",
        "languages",
        "depends_on",
    ] {
        assert!(
            m.get(k).is_none(),
            "{k} is unset and must be omitted, not written as 0/empty (got {:?})",
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
        "the configured spelling is written through verbatim"
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
fn depends_on_accepts_the_32_hex_spelling_inspect_prints() {
    // `wax-builder inspect` shows archive_uuid as bare hex; that must be usable
    let bare = UUID_A.replace('-', "");
    let cfg = format!("{VALID_MANIFEST}depends_on = \"{bare}\"\n");
    let (_dir, archive) = build_ok(&cfg);
    assert_eq!(
        WaxReader::open(&archive).unwrap().manifest().get("depends_on").map(String::as_str),
        Some(bare.as_str())
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
        ("min_hw_tier", "pi_zero_2w"),
        ("entry_point", "index.html"),
    ];
    for (k, v) in expected {
        assert_eq!(m.get(k).map(String::as_str), Some(v), "manifest.{k}");
    }
    // and nothing else: no computed fields, no defaults for unset optionals
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
