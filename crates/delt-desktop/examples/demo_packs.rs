//! Write small demo packs for trying the desktop window by hand:
//!
//! ```text
//! cargo run -p delt-desktop --example demo_packs -- OUT_DIR
//! ```
//!
//! `OUT_DIR/science.wax` and `OUT_DIR/history.wax` to add, and
//! `OUT_DIR/updates/science.wax`, a newer edition of the science pack (one
//! page added, one removed) to try "Find updates in a folder".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wax_core::model::Compression;
use wax_core::writer::{EntryMeta, WaxWriter};

const SCIENCE: [u8; 16] = [0x10, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1];
const HISTORY: [u8; 16] = [0x20, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 2];

fn manifest(name: &str, desc: &str, version: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("name".into(), name.into()),
        ("description".into(), desc.into()),
        ("version".into(), version.into()),
        ("languages".into(), "en".into()),
        ("entry_point".into(), "index.html".into()),
    ])
}

const STYLE: &str = "body{font:17px/1.6 Georgia,serif;max-width:40em;margin:2em auto;padding:0 1em;color:#222}a{color:#0645ad}";

fn page(sw: &mut wax_core::StreamingWriter, path: &str, title: &str, sections: &[(&str, &str)], links: &[(&str, &str)]) {
    let mut html = format!("<!doctype html><html><head><meta charset=utf-8><title>{title}</title><link rel=stylesheet href=\"/style.css\"></head><body><h1>{title}</h1>");
    let mut text = String::new();
    for (h, p) in sections {
        html.push_str(&format!("<h2>{h}</h2><p>{p}</p>"));
        text.push_str(&format!("{h} {p} "));
    }
    if !links.is_empty() {
        html.push_str("<h2>See also</h2><ul>");
        for (href, t) in links {
            html.push_str(&format!("<li><a href=\"/{href}\">{t}</a></li>"));
        }
        html.push_str("</ul>");
    }
    html.push_str("</body></html>");
    sw.add_entry(EntryMeta::new(path).mime("text/html").title(title).compression(Compression::Zstd), &mut html.as_bytes())
        .unwrap();
    sw.index_text(path, Some(title), &text).unwrap();
    sw.index_passages(path, html.as_bytes()).unwrap();
}

fn science(out: &Path) -> PathBuf {
    let f = out.join("science.wax");
    let mut sw = WaxWriter::new(SCIENCE).create(&f, &manifest("Science", "The elements, one page each.", "2026.09.1")).unwrap();
    sw.add_entry(EntryMeta::new("style.css").mime("text/css"), &mut STYLE.as_bytes()).unwrap();
    page(&mut sw, "index.html", "Science", &[("Welcome", "Pages about the elements of the periodic table.")],
         &[("Hydrogen.html", "Hydrogen"), ("Helium.html", "Helium")]);
    page(&mut sw, "Hydrogen.html", "Hydrogen", &[
        ("Discovery", "Henry Cavendish described hydrogen in 1766 as inflammable air."),
        ("Uses", "Hydrogen fuels rockets and makes ammonia for fertiliser."),
    ], &[("Helium.html", "Helium")]);
    page(&mut sw, "Helium.html", "Helium", &[("Properties", "Helium is a noble gas, lighter than air.")], &[("index.html", "Science")]);
    sw.finish().unwrap();
    f
}

fn history(out: &Path) {
    let f = out.join("history.wax");
    let mut sw = WaxWriter::new(HISTORY).create(&f, &manifest("History", "Short pages about the past.", "2026.09.1")).unwrap();
    sw.add_entry(EntryMeta::new("style.css").mime("text/css"), &mut STYLE.as_bytes()).unwrap();
    page(&mut sw, "index.html", "History", &[("Welcome", "Pages about the past.")], &[("Rockets.html", "Rockets")]);
    page(&mut sw, "Rockets.html", "Rockets", &[("Early rockets", "Gunpowder rockets flew in China in the thirteenth century.")], &[]);
    sw.finish().unwrap();
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("usage: demo_packs OUT_DIR"));
    std::fs::create_dir_all(out.join("updates")).unwrap();
    let s = science(&out);
    history(&out);
    let upd = out.join("updates/science.wax");
    std::fs::copy(&s, &upd).unwrap();
    // an append cannot change the manifest (SPEC §5.6: base segment only),
    // so this edition still says 2026.09.1; see NOTES.md
    let mut sw = WaxWriter::new(SCIENCE).open_append(&upd).unwrap();
    page(&mut sw, "Carbon.html", "Carbon", &[("Chemistry", "Carbon forms more compounds than any other element.")], &[]);
    sw.add_tombstone("Helium.html").unwrap();
    sw.finish().unwrap();
    println!("wrote {}", out.display());
}
