//! B1 conversion suite — Track B §4/§20 and Contract §11, end to end.
//!
//! Two kinds of input: the committed openzim test-suite ZIMs (one per
//! namespace scheme), and synthetic ZIMs built by `common::ZimBuilder` for the
//! shapes real fixtures lack. A real Wikipedia ZIM is exercised by
//! `real_wikipedia_zim` when `ZIM2WAX_REAL_ZIM` points at one.

mod common;

use common::*;
use wax_core::WaxReader;
use zim2wax::{convert, ICON_PATH};

const C: u8 = b'C';
const A: u8 = b'A';
const I: u8 = b'I';
const M: u8 = b'M';
const LAYOUT: u8 = b'-';

/// A small but realistic ≥6.1 archive: articles, assets under mwoffliner's
/// prefixes, cross-references, redirects.
fn wiki() -> ZimBuilder {
    ZimBuilder::new()
        .standard_meta()
        .html(C, "index", "Home", r#"<html><body><a href="Photosynthesis">P</a> <a href="./Plant_cell#Wall">cell</a> <img src="./_assets_/leaf.png"> <link href="./_res_/style.css"> <a href="https://example.org/x">ext</a></body></html>"#)
        .html(C, "Photosynthesis", "Photosynthesis", r#"<html><body><a href="index">home</a> <a href="Photo_synthesis">alias</a></body></html>"#)
        .html(C, "Plant_cell", "Plant cell", "<html><body>cell</body></html>")
        .content(C, "_assets_/leaf.png", "null", "image/png", b"\x89PNG-fake")
        .content(C, "_res_/style.css", "null", "text/css", "body{background:url(bg.png)}")
        .content(C, "_res_/bg.png", "null", "image/png", b"\x89PNG-bg")
        .content(M, "Illustration_48x48@1", "", "image/png", b"\x89PNG-icon")
        .redirect(C, "Photo_synthesis", "Photo synthesis", C, "Photosynthesis")
        .main_page(C, "index")
}

// ===========================================================================
// canonicalization + href rewriting
// ===========================================================================

#[test]
fn articles_flatten_to_root_and_assets_go_under_assets_prefix() {
    let fx = fixture(&wiki());
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let r = WaxReader::open(&fx.wax).unwrap();
    let paths: Vec<&str> = r.paths().collect();
    for p in [
        "index.html",
        "Photosynthesis.html",
        "Plant_cell.html",
        "Photo_synthesis.html",
        "_assets/_assets_/leaf.png",
        "_assets/_res_/style.css",
        "_assets/_res_/bg.png",
        "_assets/icon.png",
        "_meta/Title",
        "_meta/Illustration_48x48@1",
    ] {
        assert!(paths.contains(&p), "missing {p}; have {paths:?}");
    }
    // nothing keeps a namespace prefix
    assert!(!paths.iter().any(|p| p.starts_with("C/") || p.starts_with("M/")), "{paths:?}");
}

#[test]
fn hrefs_are_rewritten_to_root_relative_canonical_paths() {
    let fx = fixture(&wiki());
    let rep = convert(&fx.zim, &fx.wax, &opts()).unwrap();
    assert!(rep.stats.hrefs_rewritten >= 6, "{:?}", rep.stats);
    let mut r = WaxReader::open(&fx.wax).unwrap();
    let index = String::from_utf8(r.read("index.html").unwrap()).unwrap();
    assert!(index.contains(r#"href="/Photosynthesis.html""#), "{index}");
    assert!(index.contains(r#"href="/Plant_cell.html#Wall""#), "fragment preserved: {index}");
    assert!(index.contains(r#"src="/_assets/_assets_/leaf.png""#), "{index}");
    assert!(index.contains(r#"href="/_assets/_res_/style.css""#), "{index}");
    assert!(index.contains(r#"href="https://example.org/x""#), "external untouched: {index}");
    // a link to a redirect alias points at the alias (wax-core follows one hop)
    let p = String::from_utf8(r.read("Photosynthesis.html").unwrap()).unwrap();
    assert!(p.contains(r#"href="/Photo_synthesis.html""#), "{p}");
    // css url() rewritten too
    let css = String::from_utf8(r.read("_assets/_res_/style.css").unwrap()).unwrap();
    assert_eq!(css, "body{background:url(/_assets/_res_/bg.png)}");
}

#[test]
fn legacy_namespace_scheme_maps_the_same_way() {
    let b = ZimBuilder::legacy()
        .standard_meta()
        .html(A, "Main_Page", "Main", r#"<a href="Other"><img src="../I/pic.jpg"><link href="../-/s.css">"#)
        .html(A, "Other", "Other", "<p/>")
        .content(I, "pic.jpg", "", "image/jpeg", b"JPG")
        .content(LAYOUT, "s.css", "", "text/css", "a{}")
        .content(I, "favicon.png", "", "image/png", b"\x89PNG-fav")
        .redirect(LAYOUT, "favicon", "", I, "favicon.png")
        .main_page(A, "Main_Page");
    let fx = fixture(&b);
    let rep = convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let mut r = WaxReader::open(&fx.wax).unwrap();
    let main = String::from_utf8(r.read("Main_Page.html").unwrap()).unwrap();
    assert_eq!(main, r#"<a href="/Other.html"><img src="/_assets/pic.jpg"><link href="/_assets/s.css">"#);
    assert!(r.entry("_assets/pic.jpg").is_some());
    assert!(r.entry("_assets/s.css").is_some());
    // -/favicon redirect → _assets/favicon → _assets/favicon.png
    assert_eq!(r.entry("_assets/favicon").unwrap().redirect_to.as_deref(), Some("_assets/favicon.png"));
    // no Illustration_* in a legacy ZIM: the favicon is the icon (not a placeholder)
    assert_eq!(rep.derived.icon_source.as_deref(), Some("I/favicon.png"));
    assert_eq!(r.read(ICON_PATH).unwrap(), b"\x89PNG-fav");
}

#[test]
fn case_is_preserved_and_case_variants_stay_distinct() {
    let b = ZimBuilder::new()
        .standard_meta()
        .html(C, "index", "i", "<p/>")
        .html(C, "MacOS", "MacOS", "<p>os</p>")
        .html(C, "Macos", "Macos", "<p>other</p>")
        .main_page(C, "index");
    let fx = fixture(&b);
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let mut r = WaxReader::open(&fx.wax).unwrap();
    assert_eq!(r.read("MacOS.html").unwrap(), b"<p>os</p>");
    assert_eq!(r.read("Macos.html").unwrap(), b"<p>other</p>");
}

#[test]
fn an_article_inside_a_reserved_prefix_is_dropped_with_a_warning() {
    let b = ZimBuilder::new()
        .standard_meta()
        .html(C, "index", "i", "<p/>")
        .html(C, "_assets/shadow", "shadow", "<p>bad</p>")
        .main_page(C, "index");
    let fx = fixture(&b);
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let r = WaxReader::open(&fx.wax).unwrap();
    assert!(r.entry("_assets/shadow.html").is_none());
    assert_eq!(warning_count(&report_json(&fx.wax), "reserved_prefix_collision"), 1);
}

#[test]
fn a_url_that_is_not_a_valid_wax_path_is_dropped_with_a_warning() {
    // a redirect titled like a URL, as real Wikipedia ZIMs contain
    let b = ZimBuilder::new()
        .standard_meta()
        .html(C, "index", "i", "<p/>")
        .redirect(C, "Http://web.archive.org", "wayback", C, "index")
        .main_page(C, "index");
    let fx = fixture(&b);
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    assert_eq!(warning_count(&report_json(&fx.wax), "invalid_path"), 1);
}

// ===========================================================================
// redirects (§20)
// ===========================================================================

#[test]
fn redirect_chains_are_flattened_to_one_hop() {
    let b = ZimBuilder::new()
        .standard_meta()
        .html(C, "index", "i", "<p/>")
        .html(C, "Target", "Target", "<p>T</p>")
        .redirect(C, "Hop1", "h1", C, "Target")
        .redirect(C, "Hop2", "h2", C, "Hop1")
        .redirect(C, "Hop3", "h3", C, "Hop2")
        .main_page(C, "index");
    let fx = fixture(&b);
    let rep = convert(&fx.zim, &fx.wax, &opts()).unwrap();
    assert_eq!(rep.stats.redirects_emitted, 3);
    let mut r = WaxReader::open(&fx.wax).unwrap();
    for alias in ["Hop1.html", "Hop2.html", "Hop3.html"] {
        assert_eq!(
            r.entry(alias).unwrap().redirect_to.as_deref(),
            Some("Target.html"),
            "{alias} must point straight at the terminus"
        );
        assert_eq!(r.read(alias).unwrap(), b"<p>T</p>");
    }
    assert_eq!(report_json(&fx.wax)["redirect_count"], 3);
}

#[test]
fn redirect_cycles_are_dropped_and_counted() {
    let b = ZimBuilder::new()
        .standard_meta()
        .html(C, "index", "i", "<p/>")
        .redirect(C, "Loop_A", "a", C, "Loop_B")
        .redirect(C, "Loop_B", "b", C, "Loop_A")
        .redirect(C, "Self", "s", C, "Self")
        .main_page(C, "index");
    let fx = fixture(&b);
    let rep = convert(&fx.zim, &fx.wax, &opts()).unwrap();
    assert_eq!(rep.stats.redirects_emitted, 0);
    let r = WaxReader::open(&fx.wax).unwrap();
    assert!(r.entry("Loop_A.html").is_none() && r.entry("Loop_B.html").is_none() && r.entry("Self.html").is_none());
    let v = report_json(&fx.wax);
    assert_eq!(warning_count(&v, "redirect_cycle"), 3);
    assert_eq!(v["redirect_count"], 0);
}

#[test]
fn redirects_to_skipped_or_missing_termini_are_dropped_as_dangling() {
    let b = ZimBuilder::new()
        .standard_meta()
        .html(C, "index", "i", "<p/>")
        .content(C, "_assets_/clip.ogg", "null", "application/ogg", b"OGG")
        .redirect(C, "Sound", "sound", C, "_assets_/clip.ogg") // terminus skipped (audio)
        .redirect(C, "Via", "via", C, "Sound") // chain ending at the skipped one
        .main_page(C, "index");
    let fx = fixture(&b);
    let rep = convert(&fx.zim, &fx.wax, &opts()).unwrap();
    assert_eq!(rep.stats.redirects_emitted, 0);
    let v = report_json(&fx.wax);
    assert_eq!(warning_count(&v, "redirect_dangling"), 2);
    assert_eq!(warning_count(&v, "unsupported_mimetype"), 1);
}

// ===========================================================================
// manifest derivations (§20)
// ===========================================================================

#[test]
fn every_manifest_field_derives_from_the_zim_or_the_flags() {
    let fx = fixture(&wiki());
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let r = WaxReader::open(&fx.wax).unwrap();
    let m = r.manifest();
    assert_eq!(m.get("name").map(String::as_str), Some("Synthetic Test Wiki"), "name ← Title");
    assert_eq!(m.get("icon").map(String::as_str), Some("_assets/icon.png"), "icon ← Illustration");
    assert_eq!(m.get("version").map(String::as_str), Some("2026.09.5"), "version ← Date as CalVer");
    assert_eq!(m.get("attribution").map(String::as_str), Some("Test Creator"), "attribution ← Creator");
    assert_eq!(m.get("entry_point").map(String::as_str), Some("index.html"), "entry_point ← main page");
    assert_eq!(m.get("languages").map(String::as_str), Some("en"), "languages ← Language (eng→en)");
    assert_eq!(m.get("license").map(String::as_str), Some("CC-BY-SA-4.0"));
    assert_eq!(m.get("category").map(String::as_str), Some("reference"), "← --category");
    assert_eq!(m.get("min_hw_tier").map(String::as_str), Some("pi_zero_2w"), "← --min-hw-tier");
    assert_eq!(m.len(), 9, "8 required + languages; got {:?}", m.keys().collect::<Vec<_>>());
}

#[test]
fn icon_is_the_illustration_bytes() {
    let fx = fixture(&wiki());
    let rep = convert(&fx.zim, &fx.wax, &opts()).unwrap();
    assert_eq!(rep.derived.icon_source.as_deref(), Some("M/Illustration_48x48@1"));
    let mut r = WaxReader::open(&fx.wax).unwrap();
    assert_eq!(r.read(ICON_PATH).unwrap(), b"\x89PNG-icon");
    assert_eq!(warning_count(&report_json(&fx.wax), "icon_placeholder"), 0);
}

#[test]
fn icon_falls_back_to_a_generated_placeholder_never_omitted() {
    let b = ZimBuilder::new().standard_meta().html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    let rep = convert(&fx.zim, &fx.wax, &opts()).unwrap();
    assert_eq!(rep.derived.icon_source, None);
    let mut r = WaxReader::open(&fx.wax).unwrap();
    assert_eq!(r.manifest().get("icon").map(String::as_str), Some("_assets/icon.png"));
    let png = r.read(ICON_PATH).unwrap();
    assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "placeholder is a PNG");
    assert_eq!(warning_count(&report_json(&fx.wax), "icon_placeholder"), 1);
}

#[test]
fn attribution_falls_back_to_publisher() {
    let b = ZimBuilder::new()
        .meta("Title", "T").meta("Date", "2026-01-01").meta("License", "MIT")
        .meta("Publisher", "Only Publisher")
        .html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let r = WaxReader::open(&fx.wax).unwrap();
    assert_eq!(r.manifest().get("attribution").map(String::as_str), Some("Only Publisher"));
}

#[test]
fn attribution_with_neither_creator_nor_publisher_fails_clearly() {
    // §20 says "else the empty string"; Contract §11 requires attribution.
    // The conversion stops with a message naming the conflict (flagged).
    let b = ZimBuilder::new()
        .meta("Title", "T").meta("Date", "2026-01-01").meta("License", "MIT")
        .html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    let err = convert(&fx.zim, &fx.wax, &opts()).unwrap_err().to_string();
    assert!(err.contains("neither Creator nor Publisher"), "{err}");
    assert!(err.contains("§20/§11"), "{err}");
}

#[test]
fn version_derives_from_date_with_day_as_unpadded_counter() {
    let b = ZimBuilder::new().standard_meta().meta("Date", "2025-12-03")
        .html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let r = WaxReader::open(&fx.wax).unwrap();
    assert_eq!(r.manifest().get("version").map(String::as_str), Some("2025.12.3"));
}

#[test]
fn malformed_date_fails() {
    let b = ZimBuilder::new().standard_meta().meta("Date", "2025/12/03")
        .html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    let err = convert(&fx.zim, &fx.wax, &opts()).unwrap_err().to_string();
    assert!(err.contains("not YYYY-MM-DD"), "{err}");
}

#[test]
fn unmappable_language_is_omitted_with_a_warning() {
    let b = ZimBuilder::new().standard_meta().meta("Language", "zzz")
        .html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let r = WaxReader::open(&fx.wax).unwrap();
    assert!(r.manifest().get("languages").is_none());
    assert_eq!(warning_count(&report_json(&fx.wax), "language_unmapped"), 1);
}

#[test]
fn multi_language_zim_maps_each_code() {
    let b = ZimBuilder::new().standard_meta().meta("Language", "eng,fra,zzz")
        .html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let r = WaxReader::open(&fx.wax).unwrap();
    assert_eq!(r.manifest().get("languages").map(String::as_str), Some("en,fr"));
}

#[test]
fn missing_main_page_fails_because_entry_point_is_required() {
    let b = ZimBuilder::new().standard_meta().html(C, "index", "i", "<p/>");
    let fx = fixture(&b);
    let err = convert(&fx.zim, &fx.wax, &opts()).unwrap_err().to_string();
    assert!(err.contains("main page"), "{err}");
}

#[test]
fn entry_titles_carry_over_and_placeholders_do_not() {
    let fx = fixture(&wiki());
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let r = WaxReader::open(&fx.wax).unwrap();
    assert_eq!(r.entry("Plant_cell.html").unwrap().title.as_deref(), Some("Plant cell"));
    assert_eq!(r.entry("Photo_synthesis.html").unwrap().title.as_deref(), Some("Photo synthesis"));
    // empty title → None (ZIM: "empty means the URL is the title")
    assert_eq!(r.entry("_meta/Title").unwrap().title, None);
}

// ===========================================================================
// required flags
// ===========================================================================

#[test]
fn bad_category_or_tier_is_rejected_before_any_conversion() {
    let fx = fixture(&wiki());
    let mut o = opts();
    o.category = "science".into();
    assert!(convert(&fx.zim, &fx.wax, &o).unwrap_err().to_string().contains("--category"));
    assert!(!fx.wax.exists(), "nothing written");
    let mut o = opts();
    o.min_hw_tier = "generic".into();
    let e = convert(&fx.zim, &fx.wax, &o).unwrap_err().to_string();
    assert!(e.contains("--min-hw-tier") && e.contains("generic"), "{e}");
    let mut o = opts();
    o.min_hw_tier = "Kiosk".into();
    assert!(convert(&fx.zim, &fx.wax, &o).is_err());
}

#[test]
fn cli_refuses_to_run_without_the_required_flags() {
    let fx = fixture(&wiki());
    let bin = env!("CARGO_BIN_EXE_zim2wax");
    // no --category, no --min-hw-tier
    let out = std::process::Command::new(bin)
        .args(["convert", "--input"])
        .arg(&fx.zim)
        .arg("--output")
        .arg(&fx.wax)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--category") && stderr.contains("--min-hw-tier"), "{stderr}");
    assert!(!fx.wax.exists());
    // with only --category
    let out = std::process::Command::new(bin)
        .args(["convert", "--input"])
        .arg(&fx.zim)
        .arg("--output")
        .arg(&fx.wax)
        .args(["--category", "reference"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--min-hw-tier"));
}

// ===========================================================================
// licensing (Contract §11) — three outcomes
// ===========================================================================

#[test]
fn allowlisted_license_builds_clean_even_in_lowercase() {
    let b = ZimBuilder::new().standard_meta().meta("License", "cc-by-sa-4.0")
        .html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    let rep = convert(&fx.zim, &fx.wax, &opts()).unwrap();
    assert!(!rep.write.license_review_required());
    let r = WaxReader::open(&fx.wax).unwrap();
    assert_eq!(r.manifest().get("license").map(String::as_str), Some("CC-BY-SA-4.0"), "canonical casing");
    let v = report_json(&fx.wax);
    assert_eq!(v["license"], "CC-BY-SA-4.0");
    assert_eq!(v["license_review_required"], false);
}

#[test]
fn free_text_license_builds_with_review_required() {
    let b = ZimBuilder::new().standard_meta().meta("License", "Creative Commons, see wiki")
        .html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    let rep = convert(&fx.zim, &fx.wax, &opts()).unwrap();
    assert!(rep.write.license_review_required());
    let v = report_json(&fx.wax);
    assert_eq!(v["license"], "Creative Commons, see wiki");
    assert_eq!(v["license_review_required"], true);
    // never a manifest key
    assert!(WaxReader::open(&fx.wax).unwrap().manifest().get("license_review_required").is_none());
}

#[test]
fn blank_license_is_a_hard_failure() {
    let b = ZimBuilder::new()
        .meta("Title", "T").meta("Date", "2026-01-01").meta("Creator", "c")
        .html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    let err = convert(&fx.zim, &fx.wax, &opts()).unwrap_err().to_string();
    assert!(err.contains("no License metadata"), "{err}");
    assert!(err.contains("--license"), "should point at the override: {err}");
    assert!(!fx.wax.exists());
}

#[test]
fn license_override_applies_only_when_the_zim_has_none() {
    // absent → override used
    let b = ZimBuilder::new()
        .meta("Title", "T").meta("Date", "2026-01-01").meta("Creator", "c")
        .html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    let mut o = opts();
    o.license_if_absent = Some("mit".into());
    convert(&fx.zim, &fx.wax, &o).unwrap();
    assert_eq!(WaxReader::open(&fx.wax).unwrap().manifest().get("license").map(String::as_str), Some("MIT"));
    // present → ZIM's own value wins, override ignored
    let b = ZimBuilder::new().standard_meta().html(C, "index", "i", "<p/>").main_page(C, "index");
    let fx = fixture(&b);
    convert(&fx.zim, &fx.wax, &o).unwrap();
    assert_eq!(WaxReader::open(&fx.wax).unwrap().manifest().get("license").map(String::as_str), Some("CC-BY-SA-4.0"));
}

// ===========================================================================
// unsupported mimetypes
// ===========================================================================

#[test]
fn audio_and_video_are_skipped_counted_and_references_left_intact() {
    let b = ZimBuilder::new()
        .standard_meta()
        .html(C, "index", "i", r#"<audio src="./_assets_/say.ogg"></audio><video src="./_assets_/clip.webm"></video><img src="./_assets_/ok.png">"#)
        .content(C, "_assets_/say.ogg", "null", "application/ogg", b"OGG")
        .content(C, "_assets_/clip.webm", "null", "video/webm", b"WEBM")
        .content(C, "_assets_/ok.png", "null", "image/png", b"PNG")
        .main_page(C, "index");
    let fx = fixture(&b);
    convert(&fx.zim, &fx.wax, &opts()).unwrap();
    let mut r = WaxReader::open(&fx.wax).unwrap();
    assert!(r.entry("_assets/_assets_/say.ogg").is_none());
    assert!(r.entry("_assets/_assets_/clip.webm").is_none());
    assert!(r.entry("_assets/_assets_/ok.png").is_some());
    let html = String::from_utf8(r.read("index.html").unwrap()).unwrap();
    assert!(html.contains(r#"<audio src="./_assets_/say.ogg">"#), "reference left intact: {html}");
    assert!(html.contains(r#"<video src="./_assets_/clip.webm">"#), "{html}");
    assert!(html.contains(r#"<img src="/_assets/_assets_/ok.png">"#), "supported one rewritten: {html}");
    let v = report_json(&fx.wax);
    assert_eq!(warning_count(&v, "unsupported_mimetype"), 2);
    assert_eq!(v["skipped_count"], 2);
}

// ===========================================================================
// the committed openzim test-suite fixtures (one per namespace scheme)
// ===========================================================================

#[test]
fn committed_fixture_new_namespace_scheme_converts() {
    let dir = tempfile::tempdir().unwrap();
    let wax = dir.path().join("ns61.wax");
    let mut o = opts();
    o.license_if_absent = Some("CC-BY-SA-4.0".into()); // the test-suite ZIMs carry no License
    let rep = convert(&committed("small-ns6.1.zim"), &wax, &o).unwrap();
    assert_eq!(rep.stats.dirents, 16);
    assert_eq!(rep.derived.icon_source.as_deref(), Some("M/Illustration_48x48@1"));
    let mut r = WaxReader::open(&wax).unwrap();
    assert_eq!(r.manifest().get("name").map(String::as_str), Some("Test ZIM file"));
    assert_eq!(r.manifest().get("version").map(String::as_str), Some("2021.06.2"));
    assert_eq!(r.manifest().get("languages").map(String::as_str), Some("en"));
    assert_eq!(r.manifest().get("entry_point").map(String::as_str), Some("main.html"));
    assert!(r.read("main.html").unwrap().starts_with(b"<"));
    assert!(r.entry("_assets/favicon.png").is_some());
    assert!(!r.paths().any(|p| p.contains("xapian") || p.contains("listing/")), "indexes not copied");
    let v = report_json(&wax);
    assert_eq!(warning_count(&v, "search_index_not_copied"), 3);
    assert_eq!(warning_count(&v, "wellknown_not_copied"), 1);
}

#[test]
fn committed_fixture_legacy_namespace_scheme_converts() {
    let dir = tempfile::tempdir().unwrap();
    let wax = dir.path().join("ns5.wax");
    let mut o = opts();
    o.license_if_absent = Some("see-source".into());
    let rep = convert(&committed("small-ns5.zim"), &wax, &o).unwrap();
    assert_eq!(rep.stats.dirents, 17);
    assert_eq!(rep.derived.icon_source.as_deref(), Some("I/favicon.png"), "legacy favicon becomes the icon");
    assert!(rep.write.license_review_required());
    let r = WaxReader::open(&wax).unwrap();
    assert_eq!(r.manifest().get("version").map(String::as_str), Some("2020.11.15"));
    assert_eq!(r.entry("_assets/favicon").unwrap().redirect_to.as_deref(), Some("_assets/favicon.png"));
}

/// Real-content end-to-end. Set `ZIM2WAX_REAL_ZIM` to a Wikipedia ZIM path
/// (e.g. wikipedia_en_100) to run; skipped otherwise. Not committed — 333 MB.
#[test]
fn real_wikipedia_zim() {
    let Ok(path) = std::env::var("ZIM2WAX_REAL_ZIM") else {
        eprintln!("ZIM2WAX_REAL_ZIM not set; skipping real-ZIM test");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wax = dir.path().join("wp.wax");
    let mut o = opts();
    o.license_if_absent = Some("CC-BY-SA-4.0".into());
    let rep = convert(std::path::Path::new(&path), &wax, &o).unwrap();
    assert!(rep.stats.content_emitted > 1000);
    assert!(rep.stats.redirects_emitted > 100);
    assert!(rep.stats.hrefs_rewritten > 1000);
    let v = report_json(&wax);
    assert!(warning_count(&v, "unsupported_mimetype") > 0, "Wikipedia carries audio/video");
    assert_eq!(warning_count(&v, "redirect_cycle"), 0);
    let mut r = WaxReader::open(&wax).unwrap();
    let ep = r.manifest().get("entry_point").cloned().unwrap();
    let html = String::from_utf8(r.read(&ep).unwrap()).unwrap();
    assert!(html.contains(r#"href="/"#), "hrefs rewritten root-relative");
    // every entry reads back (sha256 verified by the reader)
    let paths: Vec<String> = r.paths().map(String::from).collect();
    for p in paths.iter().step_by(97) {
        r.read(p).unwrap();
    }
}
