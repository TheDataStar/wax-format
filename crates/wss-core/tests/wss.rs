//! Wax Service Shell engine, end to end: every importer, the reports, and
//! the edition lifecycle (refresh → update → rollback → gc).

mod common;

use common::*;
use std::fs;
use std::path::Path;
use wax_core::WaxReader;
use wss_core::report::NoteKind;
use wss_core::{convert, detect, manage, refresh, SourceKind};

fn write(dir: &Path, rel: &str, bytes: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn site(dir: &Path) {
    write(dir, "index.html", r#"<!doctype html><html lang="en"><head><title>Garden Science</title><link rel="stylesheet" href="style.css"></head><body><h1>Garden Science</h1><p>Start with <a href="plants/leaf.html">leaves</a>.</p><h2>Soil</h2><p>Worms aerate the soil.</p></body></html>"#);
    write(dir, "plants/leaf.html", "<html><head><title>Leaves</title></head><body><h1>Leaves</h1><p>Photosynthesis happens in chloroplasts.</p></body></html>");
    write(dir, "style.css", "body{background:url(img/bg.png)}");
    write(dir, "img/bg.png", png());
    write(dir, "media/clip.mp4", b"not really a video");
    write(dir, "_assets/clash.css", "p{}");
    write(dir, "docs/handout.pdf", pdf("Seed Handout", "Ms Rivera", &["Germination needs water", "Seedlings need light"]));
}

// ===========================================================================
// detection
// ===========================================================================

#[test]
fn sources_are_recognised_by_content() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("renamed.bin");
    fs::write(&p, pdf("T", "A", &["x"])).unwrap();
    assert_eq!(detect(p.to_str().unwrap()).unwrap(), SourceKind::Pdf);
    let e = d.path().join("book.zip");
    fs::write(&e, epub("B", None, &[("One", "x")], &[])).unwrap();
    assert_eq!(detect(e.to_str().unwrap()).unwrap(), SourceKind::Epub);
    assert_eq!(detect(committed_zim("small-ns6.1.zim").to_str().unwrap()).unwrap(), SourceKind::Zim);
    assert_eq!(detect(d.path().to_str().unwrap()).unwrap(), SourceKind::Folder);
    assert_eq!(detect("https://example.org/").unwrap(), SourceKind::Website);
    let junk = d.path().join("notes.txt");
    fs::write(&junk, "hello").unwrap();
    assert!(detect(junk.to_str().unwrap()).is_err());
}

// ===========================================================================
// ZIM: delegated to zim2wax, at level 19
// ===========================================================================

#[test]
fn zim_conversion_is_zim2wax_at_level_19_byte_for_byte() {
    let d = tempfile::tempdir().unwrap();
    let via_wss = d.path().join("wss.wax");
    let mut r = recipe();
    r.pack.version = None; // a ZIM states its own
    r.pack.attribution = None;
    r.pack.license = Some("CC-BY-SA-4.0".into());
    let o = convert(committed_zim("small-ns6.1.zim").to_str().unwrap(), &via_wss, &r).unwrap();
    assert_eq!(o.kind, SourceKind::Zim);
    assert_eq!(o.zstd_level, 19);
    assert!(o.indexed > 0);

    // The same ZIM through zim2wax directly, as a box would run it.
    let direct = d.path().join("direct.wax");
    zim2wax::convert(&committed_zim("small-ns6.1.zim"), &direct, &wss_core::importers::zim::options(&r).unwrap()).unwrap();
    assert_eq!(sha256_file(&via_wss), sha256_file(&direct), "one importer, three places");
    assert_eq!(build_report(&via_wss)["archive_filename"], "wss.wax");

    // A recipe that tries to override what the ZIM states is refused.
    let mut bad = r.clone();
    bad.pack.version = Some("2026.11.1".into());
    let err = convert(committed_zim("small-ns6.1.zim").to_str().unwrap(), &d.path().join("x.wax"), &bad).unwrap_err();
    assert!(err.to_string().contains("version cannot be set for a ZIM"), "{err}");
}

#[test]
fn level_19_is_smaller_than_level_3_on_text() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    // Varied prose (a fixed pseudo-random walk over a word list), so the
    // level matters the way it does on real articles.
    let words = ["water", "cycle", "ocean", "cloud", "rain", "river", "evaporates", "condenses", "falls", "land",
        "soil", "plants", "roots", "sun", "heat", "vapour", "air", "cold", "mountain", "snow", "melts", "flows",
        "sea", "salt", "fresh", "lake", "ground", "aquifer", "wells", "people", "drink", "farm"];
    let mut x: u32 = 12345;
    for i in 0..60 {
        let mut text = String::new();
        for _ in 0..600 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            text.push_str(words[(x >> 16) as usize % words.len()]);
            text.push(if (x >> 8).is_multiple_of(11) { '.' } else { ' ' });
        }
        write(&src, &format!("p{i}.html"), format!("<html><head><title>Page {i}</title></head><body><h1>Page {i}</h1><p>{text}</p></body></html>"));
    }
    write(&src, "index.html", "<html><head><title>Index</title></head><body>i</body></html>");
    let mut sizes = vec![];
    for level in [3, 19] {
        let mut r = recipe();
        r.build.zstd_level = level;
        let out = d.path().join(format!("l{level}.wax"));
        convert(src.to_str().unwrap(), &out, &r).unwrap();
        let i = manage::inspect(&out).unwrap();
        sizes.push(i.stored_bytes_by_codec.values().sum::<u64>());
    }
    assert!(sizes[1] < sizes[0], "level 19 {} should beat level 3 {}", sizes[1], sizes[0]);
}

// ===========================================================================
// folders
// ===========================================================================

#[test]
fn a_folder_becomes_a_searchable_pack_and_reports_what_it_left_out() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("site");
    site(&src);
    let out = d.path().join("garden.wax");
    let o = convert(src.to_str().unwrap(), &out, &recipe()).unwrap();
    assert_eq!(o.kind, SourceKind::Folder);

    let r = WaxReader::open(&out).unwrap();
    let m = r.manifest();
    assert_eq!(m["name"], "Garden Science", "from the start page's <title>");
    assert_eq!(m["entry_point"], "index.html");
    assert_eq!(m["languages"], "en");
    assert_eq!(m["version"], "2026.11.1");
    assert_eq!(m["icon"], "_assets/icon.png");
    // the tree is kept, so relative links work unchanged
    assert!(r.read("plants/leaf.html").unwrap().starts_with(b"<html>"));
    assert_eq!(r.read("img/bg.png").unwrap(), png());
    // the PDF is kept and gets a searchable text page
    assert!(r.contains("docs/handout.pdf").unwrap());
    let tp = "_assets/wss/pdf/docs/handout.pdf.html";
    let page = String::from_utf8(r.read(tp).unwrap()).unwrap();
    assert!(page.contains("Germination needs water") && page.contains("id=\"page-2\""), "{page}");
    // search over pages and the PDF text, with passages
    assert_eq!(r.search_tokenizer(), Some("wax-bigram-16"));
    assert_eq!(r.search("chloroplasts", 5).unwrap()[0].path, "plants/leaf.html");
    assert_eq!(r.search("seedlings", 5).unwrap()[0].path, tp);
    assert!(r.passages("index.html").unwrap().len() >= 2);

    // nothing dropped silently
    assert!(!r.contains("media/clip.mp4").unwrap());
    assert!(!r.contains("_assets/clash.css").unwrap());
    assert_eq!(o.notes.count(NoteKind::UnsupportedType), 1);
    assert_eq!(o.notes.count(NoteKind::ReservedPrefix), 1);
    let v = build_report(&out);
    assert_only_contract_codes(&v);
    assert_eq!(v["skipped_count"], 2);
    assert_eq!(v["archive_filename"], "garden.wax");
    // an operator-supplied license is reviewed, not trusted (Contract §11)
    assert_eq!(v["license_review_required"], true);
    assert!(o.warnings.contains_key("license_operator_supplied"));
    assert!(o.warnings.contains_key("icon_generated"));
    let report: serde_json::Value = serde_json::from_str(&fs::read_to_string(&o.import_report).unwrap()).unwrap();
    assert_eq!(report["notes"]["counts"]["unsupported_type"], 1);
    assert!(!out.with_extension("wax.partial").exists());

    assert!(manage::verify(&out, None, false).unwrap().ok);
}

#[test]
fn the_same_folder_builds_the_same_bytes() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("site");
    site(&src);
    let (a, b) = (d.path().join("a.wax"), d.path().join("b.wax"));
    convert(src.to_str().unwrap(), &a, &recipe()).unwrap();
    convert(src.to_str().unwrap(), &b, &recipe()).unwrap();
    assert_eq!(sha256_file(&a), sha256_file(&b));
}

#[test]
fn missing_facts_stop_the_build_with_a_question() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("site");
    site(&src);
    let out = d.path().join("x.wax");
    for (strip, needle) in [("license", "license is required"), ("attribution", "attribution is required"), ("version", "version is required"), ("category", "category is required")] {
        let mut r = recipe();
        match strip {
            "license" => r.pack.license = None,
            "attribution" => r.pack.attribution = None,
            "version" => r.pack.version = None,
            _ => r.pack.category = None,
        }
        let e = convert(src.to_str().unwrap(), &out, &r).unwrap_err().to_string();
        assert!(e.contains(needle), "{strip}: {e}");
        assert!(!out.exists(), "a failed build leaves nothing at the output path");
    }
    // a folder with no obvious start page asks for one
    let multi = d.path().join("multi");
    write(&multi, "a.html", "<p>a</p>");
    write(&multi, "b.html", "<p>b</p>");
    let mut r = recipe();
    r.pack.name = Some("Multi".into());
    let e = convert(multi.to_str().unwrap(), &out, &r).unwrap_err().to_string();
    assert!(e.contains("entry_point is required"), "{e}");
    r.pack.entry_point = Some("b.html".into());
    convert(multi.to_str().unwrap(), &out, &r).unwrap();
}

// ===========================================================================
// EPUB
// ===========================================================================

#[test]
fn an_epub_becomes_a_book_with_contents_chapter_links_and_search() {
    let d = tempfile::tempdir().unwrap();
    let book = d.path().join("rivers.epub");
    fs::write(&book, epub("Rivers of the World", Some("CC-BY-4.0"), &[("The Nile", "The Nile flows north."), ("The Amazon", "The Amazon carries the most water.")], &[])).unwrap();
    let out = d.path().join("rivers.wax");
    let mut r = recipe();
    r.pack.license = None;
    r.pack.attribution = None;
    let o = convert(book.to_str().unwrap(), &out, &r).unwrap();
    assert_eq!(o.kind, SourceKind::Epub);
    let rd = WaxReader::open(&out).unwrap();
    let m = rd.manifest();
    assert_eq!(m["name"], "Rivers of the World");
    assert_eq!(m["attribution"], "Ada Writer");
    assert_eq!(m["license"], "CC-BY-4.0");
    assert_eq!(m["languages"], "en");
    assert_eq!(m["entry_point"], "_assets/wss/contents.html");
    assert_eq!(m["icon"], "OEBPS/img/cover.png", "the PNG cover is the icon");
    // the book stated its license: nothing operator-supplied, no review
    let v = build_report(&out);
    assert_eq!(v["license_review_required"], false);
    assert_only_contract_codes(&v);

    let contents = String::from_utf8(rd.read("_assets/wss/contents.html").unwrap()).unwrap();
    assert!(contents.contains(r#"<a href="/OEBPS/text/ch0.xhtml">The Nile</a>"#), "{contents}");
    let ch0 = String::from_utf8(rd.read("OEBPS/text/ch0.xhtml").unwrap()).unwrap();
    assert!(ch0.contains(r#"href="/OEBPS/text/ch1.xhtml" rel="next""#) && !ch0.contains("rel=\"prev\""), "{ch0}");
    assert_eq!(rd.entry("OEBPS/text/ch1.xhtml").unwrap().title.as_deref(), Some("The Amazon"));
    assert_eq!(rd.search("amazon", 5).unwrap()[0].path, "OEBPS/text/ch1.xhtml");
    assert!(!rd.contains("mimetype").unwrap() && !rd.contains("META-INF/container.xml").unwrap());
}

#[test]
fn a_drm_protected_epub_is_refused_but_font_obfuscation_is_carried() {
    let d = tempfile::tempdir().unwrap();
    let enc = |alg: &str| {
        format!(r#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="{alg}"/><enc:CipherData><enc:CipherReference URI="OEBPS/font.ttf"/></enc:CipherData></enc:EncryptedData></encryption>"#).into_bytes()
    };
    let drm = d.path().join("drm.epub");
    fs::write(&drm, epub("Locked", Some("CC0-1.0"), &[("One", "x")], &[("META-INF/encryption.xml", enc("http://www.w3.org/2001/04/xmlenc#aes128-cbc")), ("OEBPS/font.ttf", vec![0; 64])])).unwrap();
    let e = convert(drm.to_str().unwrap(), &d.path().join("drm.wax"), &recipe()).unwrap_err().to_string();
    assert!(e.contains("DRM"), "{e}");

    let obf = d.path().join("obf.epub");
    fs::write(&obf, epub("Fonts", Some("CC0-1.0"), &[("One", "x")], &[("META-INF/encryption.xml", enc("http://www.idpf.org/2008/embedding")), ("OEBPS/font.ttf", vec![0; 64])])).unwrap();
    let o = convert(obf.to_str().unwrap(), &d.path().join("obf.wax"), &recipe()).unwrap();
    assert_eq!(o.notes.count(NoteKind::ObfuscatedFont), 1);
    assert_eq!(warning(&build_report(&d.path().join("obf.wax")), "kept_unopened"), 1);
}

// ===========================================================================
// PDF
// ===========================================================================

#[test]
fn a_pdf_is_kept_whole_with_a_text_page_per_page() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("volcanoes.pdf");
    let bytes = pdf("Volcanoes", "Dr Lava", &["Magma rises through the crust", "Basalt cools quickly"]);
    fs::write(&file, &bytes).unwrap();
    let out = d.path().join("v.wax");
    let mut r = recipe();
    r.pack.attribution = None;
    let o = convert(file.to_str().unwrap(), &out, &r).unwrap();
    assert_eq!(o.kind, SourceKind::Pdf);
    let rd = WaxReader::open(&out).unwrap();
    assert_eq!(rd.manifest()["name"], "Volcanoes");
    assert_eq!(rd.manifest()["attribution"], "Dr Lava", "the PDF's Author");
    assert_eq!(rd.read("volcanoes.pdf").unwrap(), bytes, "the original is kept byte for byte");
    assert_eq!(rd.entry("volcanoes.pdf").unwrap().compression, "none");
    let entry = rd.manifest()["entry_point"].clone();
    assert_eq!(entry, "_assets/wss/pdf/volcanoes.pdf.html");
    let hit = &rd.search_passages("basalt", 3).unwrap()[0];
    assert_eq!(hit.path, entry);
    assert!(o.notes.is_empty(), "{:?}", o.notes);
}

#[test]
fn a_pdf_without_text_is_carried_and_reported() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("scan.pdf");
    fs::write(&file, pdf("Scan", "Archive", &[""])).unwrap();
    let out = d.path().join("s.wax");
    let o = convert(file.to_str().unwrap(), &out, &recipe()).unwrap();
    assert_eq!(o.notes.count(NoteKind::NoSearchableText), 1);
    assert!(WaxReader::open(&out).unwrap().contains("scan.pdf").unwrap());
    let v = build_report(&out);
    assert_eq!(warning(&v, "no_searchable_text"), 1);
    assert_eq!(v["skipped_count"], 0, "carried, so not skipped");
    assert_only_contract_codes(&v);
}

// ===========================================================================
// website capture
// ===========================================================================

#[test]
fn a_website_is_captured_politely_and_works_offline() {
    let base = serve(vec![
        ("/robots.txt", "text/plain", b"User-agent: *\nDisallow: /docs/private/\n".to_vec()),
        ("/docs/", "text/html", br#"<html lang="en"><head><title>Field Guide</title><link rel="stylesheet" href="/static/site.css"></head><body><h1>Field Guide</h1><a href="birds.html#robin">Birds</a> <a href="private/notes.html">private</a> <a href="https://elsewhere.example/x">elsewhere</a> <a href="/outside.html">outside the prefix</a> <a href="list.html?page=2">page 2</a> <a href="old.html">old</a></body></html>"#.to_vec()),
        ("/docs/birds.html", "text/html; charset=utf-8", br#"<html><head><title>Birds</title></head><body><h2 id="robin">Robin</h2><p>Robins sing at dawn.</p><img src="/static/robin.png"><a href="./">home</a></body></html>"#.to_vec()),
        ("/docs/private/notes.html", "text/html", b"<p>secret</p>".to_vec()),
        ("/docs/list.html?page=2", "text/html", b"<html><body><p>Second page of the list</p></body></html>".to_vec()),
        ("/docs/old.html", "redirect:/docs/birds.html", vec![]),
        ("/outside.html", "text/html", b"<p>outside</p>".to_vec()),
        ("/static/site.css", "text/css", b"body{background:url(bg.png)}".to_vec()),
        ("/static/bg.png", "image/png", png()),
        ("/static/robin.png", "image/png", png()),
    ]);
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("guide.wax");
    let o = convert(&format!("{base}/docs/"), &out, &recipe()).unwrap();
    assert_eq!(o.kind, SourceKind::Website);
    let r = WaxReader::open(&out).unwrap();
    assert_eq!(r.manifest()["entry_point"], "docs/index.html");
    assert_eq!(r.manifest()["name"], "Field Guide");
    for p in ["docs/index.html", "docs/birds.html", "static/site.css", "static/bg.png", "static/robin.png"] {
        assert!(r.contains(p).unwrap(), "missing {p}");
    }
    assert!(r.paths().any(|p| p.unwrap().starts_with("docs/list__q")), "query pages get their own file");
    assert!(!r.contains("docs/private/notes.html").unwrap(), "robots.txt is obeyed");
    assert!(!r.contains("outside.html").unwrap(), "pages outside the prefix are not followed");
    // links point at pack paths; other sites are left alone
    let home = String::from_utf8(r.read("docs/index.html").unwrap()).unwrap();
    assert!(home.contains(r#"href="/docs/birds.html#robin""#), "{home}");
    assert!(home.contains(r#"href="/static/site.css""#), "{home}");
    assert!(home.contains("https://elsewhere.example/x"));
    let birds = String::from_utf8(r.read("docs/birds.html").unwrap()).unwrap();
    assert!(birds.contains(r#"href="/docs/index.html""#), "{birds}");
    // a redirect becomes an alias
    assert_eq!(r.resolve("docs/old.html").unwrap().entry.path, "docs/birds.html");
    assert_eq!(r.search("robins", 5).unwrap()[0].path, "docs/birds.html");

    assert!(o.notes.count(NoteKind::UncapturedPage) >= 1);
    assert_eq!(o.notes.count(NoteKind::ExternalReference), 1);
    let report: serde_json::Value = serde_json::from_str(&fs::read_to_string(&o.import_report).unwrap()).unwrap();
    assert_eq!(report["url_map"][format!("{base}/docs/birds.html")], "docs/birds.html");
    let v = build_report(&out);
    assert_only_contract_codes(&v);
    // robots.txt-blocked page, and the out-of-prefix page is simply not followed
    assert_eq!(warning(&v, "uncaptured_page"), o.notes.count(NoteKind::UncapturedPage));
    assert_eq!(v["skipped_count"].as_u64().unwrap(), o.notes.count(NoteKind::UncapturedPage));
    assert_eq!(warning(&v, "external_reference"), 1, "one reference to elsewhere.example");
}

#[test]
fn a_crawl_stops_at_its_page_limit_and_says_so() {
    let base = serve(vec![
        ("/a.html", "text/html", br#"<a href="b.html">b</a><a href="c.html">c</a>"#.to_vec()),
        ("/b.html", "text/html", b"<p>b</p>".to_vec()),
        ("/c.html", "text/html", b"<p>c</p>".to_vec()),
    ]);
    let d = tempfile::tempdir().unwrap();
    let mut r = recipe();
    r.pack.name = Some("Limited".into());
    r.crawl.max_pages = 2;
    let o = convert(&format!("{base}/a.html"), &d.path().join("l.wax"), &r).unwrap();
    assert_eq!(o.entries, 3, "two pages plus the generated icon");
    assert!(o.notes.notes.iter().any(|n| n.detail.contains("page limit")));
    let v = build_report(&d.path().join("l.wax"));
    assert_eq!(warning(&v, "uncaptured_page"), 1, "c.html was never fetched");
    assert_eq!(v["skipped_count"], 1);
}

// ===========================================================================
// editions: refresh, update, rollback, gc
// ===========================================================================

#[test]
fn a_refresh_appends_only_what_changed_and_rolls_back_exactly() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("site");
    site(&src);
    write(&src, "old.html", "<html><head><title>Old</title></head><body><p>Obsolete trilobite page</p></body></html>");
    let pack = d.path().join("garden.wax");
    convert(src.to_str().unwrap(), &pack, &recipe()).unwrap();
    let first = sha256_file(&pack);
    let first_size = fs::metadata(&pack).unwrap().len();

    // Unchanged source: nothing to publish.
    let same = refresh(&pack, src.to_str().unwrap(), &recipe()).unwrap();
    assert!(!same.update.appended);
    assert_eq!(sha256_file(&pack), first);

    // Change one page, add one, remove one.
    write(&src, "plants/leaf.html", "<html><head><title>Leaves</title></head><body><h1>Leaves</h1><p>Stomata let leaves breathe.</p></body></html>");
    write(&src, "plants/root.html", "<html><head><title>Roots</title></head><body><p>Roots drink groundwater.</p></body></html>");
    fs::remove_file(src.join("old.html")).unwrap();
    let mut r = recipe();
    r.build.created_at = Some(CREATED_AT + 86_400);
    let out = refresh(&pack, src.to_str().unwrap(), &r).unwrap();
    let u = &out.update;
    assert!(u.appended);
    assert_eq!(u.diff.added, ["plants/root.html"]);
    assert_eq!(u.diff.changed, ["plants/leaf.html"]);
    assert_eq!(u.diff.removed, ["old.html"]);
    assert!(u.update_bytes < first_size, "boxes download the difference, not the pack");
    assert_eq!(u.segments, 2);
    assert!(u.cautions.is_empty(), "{:?}", u.cautions);

    let rd = WaxReader::open(&pack).unwrap();
    assert!(rd.prev_state_digest(1).is_some(), "the update says which edition it extends");
    assert!(!rd.contains("old.html").unwrap());
    assert!(rd.search("trilobite", 5).unwrap().is_empty(), "removed pages leave search");
    assert!(rd.search("chloroplasts", 5).unwrap().is_empty(), "changed pages are re-indexed");
    assert_eq!(rd.search("stomata", 5).unwrap()[0].path, "plants/leaf.html");
    assert_eq!(rd.search("groundwater", 5).unwrap()[0].path, "plants/root.html");
    assert!(rd.passages("plants/leaf.html").is_ok());
    assert!(manage::verify(&pack, None, false).unwrap().ok);
    drop(rd);

    // The update applied to a fresh build is the same as building fresh,
    // entry for entry.
    let fresh = d.path().join("fresh.wax");
    convert(src.to_str().unwrap(), &fresh, &r).unwrap();
    assert!(manage::diff(&pack, &fresh).unwrap().is_empty());

    // Roll back: byte for byte the first edition.
    manage::rollback(&pack).unwrap();
    assert_eq!(sha256_file(&pack), first);

    // Compact a two-segment pack into a new base edition.
    refresh(&pack, src.to_str().unwrap(), &r).unwrap();
    let g = manage::gc(&pack, None, false, false).unwrap();
    assert!(g.new_base_edition && g.segments_before == 2);
    assert_eq!(WaxReader::open(&pack).unwrap().segment_count(), 1);
    assert!(manage::diff(&pack, &fresh).unwrap().is_empty());
}

#[test]
fn a_refresh_that_changes_the_manifest_needs_a_new_base_edition() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("site");
    site(&src);
    let pack = d.path().join("p.wax");
    convert(src.to_str().unwrap(), &pack, &recipe()).unwrap();
    let before = sha256_file(&pack);
    let mut r = recipe();
    r.pack.category = Some("reference".into());
    let e = refresh(&pack, src.to_str().unwrap(), &r).unwrap_err().to_string();
    assert!(e.contains("new base edition") && e.contains("category"), "{e}");
    assert_eq!(sha256_file(&pack), before, "a refused update leaves the pack untouched");

    // A new version alone is allowed, but the person is told it cannot be
    // recorded in an append (a SPEC gap, reported rather than worked around).
    write(&src, "index.html", "<html lang=\"en\"><head><title>Garden Science</title></head><body><p>new</p></body></html>");
    let mut r = recipe();
    r.pack.version = Some("2026.12.1".into());
    let u = refresh(&pack, src.to_str().unwrap(), &r).unwrap().update;
    assert!(u.appended);
    assert!(u.cautions.iter().any(|c| c.contains("cannot record a new version")), "{:?}", u.cautions);
    assert_eq!(WaxReader::open(&pack).unwrap().manifest()["version"], "2026.11.1");
}

#[test]
fn a_zim_refresh_carries_the_dictionary_and_appends_nothing_when_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let mut r = recipe();
    r.pack.version = None;
    r.pack.attribution = None;
    r.pack.license = Some("CC-BY-SA-4.0".into());
    let zim = committed_zim("small-ns6.1.zim");
    let pack = d.path().join("z.wax");
    convert(zim.to_str().unwrap(), &pack, &r).unwrap();
    let u = refresh(&pack, zim.to_str().unwrap(), &r).unwrap().update;
    assert!(!u.appended, "{:?}", u.diff);
}

// ===========================================================================
// the command line
// ===========================================================================

#[test]
fn the_cli_converts_inspects_and_diffs_with_json() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("site");
    site(&src);
    let out = d.path().join("cli.wax");
    let recipe_file = d.path().join("wss.toml");
    let run = |args: &[&str]| {
        let o = std::process::Command::new(env!("CARGO_BIN_EXE_wss")).args(args).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    let json = run(&[
        "--json", "convert", src.to_str().unwrap(), "-o", out.to_str().unwrap(),
        "--category", "education", "--min-ram-bytes", "2147483648", "--min-storage-bytes", "34359738368",
        "--arch", "any", "--version", "2026.11.1", "--license", "CC-BY-4.0", "--attribution", "Test School",
        "--created-at", "1789000000", "--archive-uuid", UUID, "--save-recipe", recipe_file.to_str().unwrap(),
    ]);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["kind"], "folder");
    assert_eq!(v["zstd_level"], 19);

    // The saved recipe rebuilds the same bytes.
    let again = d.path().join("again.wax");
    run(&["convert", src.to_str().unwrap(), "-o", again.to_str().unwrap(), "--recipe", recipe_file.to_str().unwrap()]);
    assert_eq!(sha256_file(&out), sha256_file(&again));

    let info: serde_json::Value = serde_json::from_str(&run(&["--json", "inspect", out.to_str().unwrap()])).unwrap();
    assert_eq!(info["manifest"]["name"], "Garden Science");
    let diff: serde_json::Value = serde_json::from_str(&run(&["--json", "diff", out.to_str().unwrap(), again.to_str().unwrap()])).unwrap();
    assert_eq!(diff["added"].as_array().unwrap().len() + diff["changed"].as_array().unwrap().len(), 0);
    assert!(run(&["verify", out.to_str().unwrap()]).starts_with("ok"));

    // Missing required flags fail with the question, not a guess.
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_wss"))
        .args(["convert", src.to_str().unwrap(), "-o", d.path().join("n.wax").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("category is required"));
}

// ===========================================================================
// signing (needs the minisign binary; skips loudly without it)
// ===========================================================================

fn minisign() -> Option<String> {
    let bin = wax_builder::sign::minisign_bin();
    if std::process::Command::new(&bin).arg("-v").output().is_ok() {
        return Some(bin);
    }
    if std::env::var("WAX_REQUIRE_MINISIGN").as_deref() == Ok("1") {
        panic!("WAX_REQUIRE_MINISIGN=1 but minisign ({bin}) is not runnable");
    }
    eprintln!("SKIPPED (minisign not found): signing tests");
    None
}

#[test]
fn signed_builds_and_updates_verify() {
    let Some(bin) = minisign() else { return };
    let d = tempfile::tempdir().unwrap();
    let (sk, pk) = (d.path().join("k.key"), d.path().join("k.pub"));
    let st = std::process::Command::new(&bin).args(["-G", "-W", "-s"]).arg(&sk).arg("-p").arg(&pk).status().unwrap();
    assert!(st.success());
    let src = d.path().join("site");
    site(&src);
    let pack = d.path().join("s.wax");
    let mut r = recipe();
    r.build.sign_key = Some(sk.clone());
    let o = convert(src.to_str().unwrap(), &pack, &r).unwrap();
    assert_eq!(o.signature_sidecar.as_deref(), Some(wax_builder::sign::sidecar_path(&pack).as_path()));
    let key = wax_builder::sign::PubKey::File(pk.clone());
    assert!(manage::verify(&pack, Some(&key), true).unwrap().ok);

    write(&src, "plants/new.html", "<p>new</p>");
    let u = refresh(&pack, src.to_str().unwrap(), &r).unwrap().update;
    assert!(u.signature_sidecar.is_some());
    assert!(manage::verify(&pack, Some(&key), true).unwrap().ok);

    // ZIM builds put their signature beside the published pack too.
    let mut zr = recipe();
    zr.pack.version = None;
    zr.pack.attribution = None;
    zr.build.sign_key = Some(sk);
    let zp = d.path().join("z.wax");
    convert(committed_zim("small-ns6.1.zim").to_str().unwrap(), &zp, &zr).unwrap();
    assert!(wax_builder::sign::sidecar_path(&zp).is_file());
    assert!(manage::verify(&zp, Some(&key), true).unwrap().ok);
}
