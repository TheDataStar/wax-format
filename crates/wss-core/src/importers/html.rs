//! A folder (an HTML site, a folder of worksheets, a built client-side web
//! app) → pack.
//!
//! The folder's tree becomes the pack's tree unchanged, so every relative
//! link keeps working with no rewriting. Pages are indexed for search and cut
//! into passages; PDFs get a text page each ([`super::pdf`]); an EPUB inside
//! the folder is carried as a download. Files packs do not carry yet (audio,
//! video, archives) are listed in the report, never dropped quietly.
//!
//! A client-side web app is the same import with its built folder: the app's
//! files are served from the pack as they are. WSS does not yet run the app
//! to detect calls to a server (WSS spec: a headless-browser check, release
//! 2); an app that needs a server is a box app instead (contract §15).

use super::{guess_mime, html_lang, html_title, Staged};
use crate::recipe::Recipe;
use crate::report::NoteKind;
use crate::stage::{Body, Item};
use anyhow::{bail, Context, Result};
use std::path::Path;

/// Files a folder's root may hold as the pack icon, in preference order.
const ICON_NAMES: [&str; 3] = ["icon.png", "favicon.png", "apple-touch-icon.png"];

/// A source file name made safe as one pack path component: kept as is when
/// already valid, otherwise with the offending characters replaced.
pub fn sanitize_component(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c })
        .collect();
    match s.as_str() {
        "" | "." | ".." => "_".into(),
        _ => s,
    }
}

pub fn import(dir: &Path, recipe: &Recipe) -> Result<Staged> {
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    let mut staged = Staged::new("html")?;
    let lang = recipe.pack.languages.clone();

    let walk = walkdir::WalkDir::new(dir).follow_links(false).sort_by_file_name();
    for entry in walk {
        let entry = entry.with_context(|| format!("walking {}", dir.display()))?;
        if !entry.file_type().is_file() {
            if entry.file_type().is_symlink() {
                let rel = rel_path(dir, entry.path());
                staged.skip(NoteKind::InvalidPath, rel, "a symbolic link; links are not followed out of the folder");
            }
            continue;
        }
        let rel = rel_path(dir, entry.path());
        let bytes = std::fs::read(entry.path()).with_context(|| format!("reading {}", entry.path().display()))?;
        let mime = guess_mime(&rel);
        match mime.as_str() {
            "application/pdf" => {
                super::pdf::stage_pdf(&mut staged, &rel, &rel, &bytes, lang.as_deref())?;
            }
            "application/epub+zip" => {
                if staged.stage_file(&rel, &rel, &mime, None, &bytes)? {
                    staged.notes.add(
                        NoteKind::NotExpanded,
                        &rel,
                        "an EPUB inside a folder is kept as a download; convert it on its own to read it in the pack",
                    );
                }
            }
            _ => {
                let title = super::is_html_mime(&mime).then(|| html_title(&String::from_utf8_lossy(&bytes))).flatten();
                staged.stage_file(&rel, &rel, &mime, title.as_deref(), &bytes)?;
            }
        }
    }

    // The start page: index.html, else the only page there is.
    let entry = if staged.staging.contains("index.html")? {
        Some("index.html".to_string())
    } else {
        let pages = staged.staging.html_paths(2)?;
        (pages.len() == 1).then(|| pages[0].clone())
    };
    if let Some(e) = recipe.pack.entry_point.as_deref().or(entry.as_deref()) {
        if let Some(Item { body: Body::Data { bytes, .. }, .. }) = staged.staging.get(e)? {
            let html = String::from_utf8_lossy(&bytes);
            staged.facts.name = html_title(&html);
            staged.facts.languages = html_lang(&html).filter(|l| super::valid_language(l));
        }
    }
    staged.facts.entry_point = entry;
    for name in ICON_NAMES {
        if staged.staging.contains(name)? {
            staged.facts.icon = Some(name.to_string());
            break;
        }
    }
    Ok(staged)
}

/// `dir`-relative path with `/` separators.
fn rel_path(dir: &Path, p: &Path) -> String {
    p.strip_prefix(dir)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}
