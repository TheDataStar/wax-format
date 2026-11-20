//! Test sources built in code: a minimal PDF with a real text layer, a
//! minimal EPUB 3, and a tiny HTTP server for website capture.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use wss_core::Recipe;

pub const UUID: &str = "4a1b2c3d-4e5f-4607-8a99-aabbccddeeff";
pub const CREATED_AT: u64 = 1_789_000_000;

/// A recipe with every required fact, pinned for reproducible builds.
pub fn recipe() -> Recipe {
    let mut r = Recipe::default();
    r.pack.category = Some("education".into());
    r.pack.arch = Some("any".into());
    r.pack.min_ram_bytes = Some(2 * 1024 * 1024 * 1024);
    r.pack.min_storage_bytes = Some(32 * 1024 * 1024 * 1024);
    r.pack.version = Some("2026.11.1".into());
    r.pack.license = Some("CC-BY-4.0".into());
    r.pack.attribution = Some("Test School".into());
    r.build.created_at = Some(CREATED_AT);
    r.build.archive_uuid = Some(UUID.into());
    r.crawl.delay_ms = 0;
    r
}

/// The Contract §11 closed warning vocabulary.
pub const CONTRACT_CODES: [&str; 12] = [
    "unsupported_mimetype",
    "redirect_cycle",
    "redirect_dangling",
    "invalid_path",
    "reserved_prefix_collision",
    "license_operator_supplied",
    "attribution_operator_supplied",
    "icon_generated",
    "uncaptured_page",
    "external_reference",
    "no_searchable_text",
    "kept_unopened",
];

pub fn warning(v: &serde_json::Value, code: &str) -> u64 {
    v["warnings"].as_array().unwrap().iter().find(|w| w["code"] == code).map(|w| w["count"].as_u64().unwrap()).unwrap_or(0)
}

pub fn build_report(pack: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(wax_builder::BuildReport::path_for(pack)).unwrap()).unwrap()
}

pub fn assert_only_contract_codes(v: &serde_json::Value) {
    for w in v["warnings"].as_array().unwrap() {
        let c = w["code"].as_str().unwrap();
        assert!(CONTRACT_CODES.contains(&c), "{c} is not a Contract §11 code");
    }
}

pub fn committed_zim(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../zim2wax/tests/fixtures").join(name)
}

/// A one- or more-page PDF whose pages each show one line of text in
/// Helvetica, with a correct xref table, and an Info dictionary.
pub fn pdf(title: &str, author: &str, pages: &[&str]) -> Vec<u8> {
    let n = pages.len();
    // objects: 1 catalog, 2 pages, 3 font, 4 info, then per page: page, content
    let mut objs: Vec<String> = Vec::new();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 5 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into());
    objs.push(format!("<< /Title ({title}) /Author ({author}) >>"));
    for (i, text) in pages.iter().enumerate() {
        let content = format!("BT /F1 18 Tf 72 720 Td ({text}) Tj ET");
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            6 + 2 * i
        ));
        objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R /Info 4 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes(),
    );
    out
}

/// A minimal valid PNG (1×1).
pub fn png() -> Vec<u8> {
    zim2wax::png::placeholder_icon()
}

/// A minimal EPUB 3: two chapters, a nav document, a PNG cover, and the
/// given extra files (path, bytes).
pub fn epub(title: &str, rights: Option<&str>, chapters: &[(&str, &str)], extra: &[(&str, Vec<u8>)]) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let deflate = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("mimetype", stored).unwrap();
        z.write_all(b"application/epub+zip").unwrap();
        z.start_file("META-INF/container.xml", deflate).unwrap();
        z.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
        let mut manifest = String::from(r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="cover" href="img/cover.png" media-type="image/png" properties="cover-image"/>"#);
        let mut spine = String::new();
        let mut nav = String::new();
        for (i, (head, _)) in chapters.iter().enumerate() {
            manifest.push_str(&format!(r#"<item id="c{i}" href="text/ch{i}.xhtml" media-type="application/xhtml+xml"/>"#));
            spine.push_str(&format!(r#"<itemref idref="c{i}"/>"#));
            nav.push_str(&format!(r#"<li><a href="text/ch{i}.xhtml">{head}</a></li>"#));
        }
        let rights = rights.map(|r| format!("<dc:rights>{r}</dc:rights>")).unwrap_or_default();
        z.start_file("OEBPS/content.opf", deflate).unwrap();
        z.write_all(format!(r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:uuid:1</dc:identifier><dc:title>{title}</dc:title><dc:creator>Ada Writer</dc:creator><dc:language>en</dc:language>{rights}</metadata><manifest>{manifest}</manifest><spine>{spine}</spine></package>"#).as_bytes()).unwrap();
        z.start_file("OEBPS/nav.xhtml", deflate).unwrap();
        z.write_all(format!(r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body><nav epub:type="toc"><ol>{nav}</ol></nav></body></html>"#).as_bytes()).unwrap();
        for (i, (head, body)) in chapters.iter().enumerate() {
            z.start_file(format!("OEBPS/text/ch{i}.xhtml"), deflate).unwrap();
            z.write_all(format!(r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>{head}</title></head><body><h1>{head}</h1><p>{body}</p><img src="../img/cover.png"/></body></html>"#).as_bytes()).unwrap();
        }
        z.start_file("OEBPS/img/cover.png", stored).unwrap();
        z.write_all(&png()).unwrap();
        for (p, b) in extra {
            z.start_file(*p, deflate).unwrap();
            z.write_all(b).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

/// Serve `site` (path → (content type, body)) on a local port until the test
/// ends. Unknown paths are 404. Returns the base URL.
pub fn serve(site: Vec<(&'static str, &'static str, Vec<u8>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut line = String::new();
            let mut r = BufReader::new(s.try_clone().unwrap());
            if r.read_line(&mut line).is_err() {
                continue;
            }
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" || h == "\n" {
                    break;
                }
            }
            let target = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            let resp = match site.iter().find(|(p, _, _)| *p == target) {
                Some((_, ctype, body)) if ctype.starts_with("redirect:") => format!(
                    "HTTP/1.1 301 Moved\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    &ctype["redirect:".len()..]
                )
                .into_bytes()
                .into_iter()
                .chain(body.iter().copied().take(0))
                .collect::<Vec<u8>>(),
                Some((_, ctype, body)) => {
                    let mut v = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    v.extend_from_slice(body);
                    v
                }
                None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            };
            let _ = s.write_all(&resp);
        }
    });
    format!("http://{addr}")
}

pub fn sha256_file(p: &Path) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(std::fs::read(p).unwrap()).into()
}
