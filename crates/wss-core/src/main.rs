//! `wss` — the Wax Service Shell command line.
//!
//! Every screen of the future desktop app maps to a command here (WSS spec:
//! "everything in the UI is also in the command line"). `--json` on any
//! command prints the engine's result as JSON for scripts and front ends.

use anyhow::{bail, Context, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;
use wss_core::{manage, Recipe};

#[derive(Parser, Debug)]
#[command(name = "wss", version, about = "Wax Service Shell: make and manage .wax packs")]
struct Cli {
    /// Print results as JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Convert a source (a .zim, .epub or .pdf file, a folder, or an http(s)
    /// URL) into a pack.
    Convert {
        /// The source.
        source: String,
        #[arg(short, long)]
        output: PathBuf,
        #[command(flatten)]
        recipe: RecipeArgs,
        /// Also save the recipe used, so the build can be repeated exactly.
        #[arg(long)]
        save_recipe: Option<PathBuf>,
    },
    /// Re-convert a source as the next edition of a pack and append only what
    /// changed. Category, resources, arch, gpu, name, license and attribution
    /// default to what the pack already declares; give the new --version.
    Refresh {
        /// The pack to update, in place.
        #[arg(short, long)]
        pack: PathBuf,
        /// The refreshed source.
        source: String,
        #[command(flatten)]
        recipe: RecipeArgs,
    },
    /// Say what kind of source something is.
    Detect { source: String },
    /// Show what a pack holds.
    Inspect { pack: PathBuf },
    /// Compare two editions of a pack.
    Diff {
        old: PathBuf,
        new: PathBuf,
        /// List every added, changed and removed path.
        #[arg(long)]
        list: bool,
    },
    /// Append a candidate edition, built elsewhere, to a pack as an update.
    Update {
        #[arg(short, long)]
        pack: PathBuf,
        #[arg(short, long)]
        candidate: PathBuf,
        /// zstd level for re-framed entries (ADR-0003: 19).
        #[arg(long, default_value_t = wss_core::recipe::DEFAULT_ZSTD_LEVEL)]
        zstd_level: i32,
        #[arg(long)]
        created_at: Option<u64>,
        /// minisign secret key; also $WAX_MINISIGN_KEY
        #[arg(long)]
        sign_key: Option<PathBuf>,
    },
    /// Undo the newest update, byte for byte.
    Rollback { pack: PathBuf },
    /// Reclaim space: an orphaned tail in place, or compact into a new base
    /// edition.
    Gc {
        pack: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Only reclaim an orphaned tail; keeps the signature valid.
        #[arg(long)]
        tail_only: bool,
        /// Leave the search index out if it cannot be carried exactly.
        #[arg(long)]
        drop_search: bool,
    },
    /// Sign a pack with a minisign secret key.
    Sign {
        pack: PathBuf,
        /// minisign secret key; also $WAX_MINISIGN_KEY
        #[arg(short, long)]
        key: Option<PathBuf>,
    },
    /// Read every entry back and check the signature.
    Verify {
        pack: PathBuf,
        /// minisign public key file; also $WAX_MINISIGN_PUBKEY
        #[arg(long)]
        pubkey: Option<PathBuf>,
        #[arg(long)]
        require_signature: bool,
    },
}

/// Recipe values from the command line. Each overrides the `--recipe` file.
#[derive(ClapArgs, Debug, Default)]
struct RecipeArgs {
    /// A recipe file (wss.toml) to start from.
    #[arg(long)]
    recipe: Option<PathBuf>,
    /// One of: reference, education, media, tools, civic, health. Required.
    #[arg(long)]
    category: Option<String>,
    /// Steady-state RAM the pack needs, in bytes. Required.
    #[arg(long)]
    min_ram_bytes: Option<i64>,
    /// Storage the pack needs beyond itself, in bytes. Required.
    #[arg(long)]
    min_storage_bytes: Option<i64>,
    /// aarch64, x86_64 or any (converted web content: any). Required.
    #[arg(long)]
    arch: Option<String>,
    /// required or preferred; omit when irrelevant.
    #[arg(long)]
    gpu: Option<String>,
    #[arg(long)]
    name: Option<String>,
    /// CalVer YYYY.MM.N. Required unless the source states it (a ZIM).
    #[arg(long)]
    version: Option<String>,
    /// SPDX id or licence text, used only when the source states none.
    #[arg(long)]
    license: Option<String>,
    /// Credit line, used only when the source names no author or publisher.
    #[arg(long)]
    attribution: Option<String>,
    /// Comma-separated BCP-47 tags.
    #[arg(long)]
    languages: Option<String>,
    #[arg(long)]
    entry_point: Option<String>,
    /// 1..=19; default 19 (ADR-0003).
    #[arg(long)]
    zstd_level: Option<i32>,
    #[arg(long)]
    no_search_index: bool,
    /// Pin created_at (Unix seconds); also $SOURCE_DATE_EPOCH.
    #[arg(long)]
    created_at: Option<u64>,
    #[arg(long)]
    archive_uuid: Option<String>,
    /// minisign secret key; also $WAX_MINISIGN_KEY
    #[arg(long)]
    sign_key: Option<PathBuf>,
    /// The previous edition, whose dictionary is carried forward.
    #[arg(long)]
    carry_dictionary_from: Option<PathBuf>,
    /// Website capture: stop after this many URLs.
    #[arg(long)]
    max_pages: Option<usize>,
    /// Website capture: links followed from the start page.
    #[arg(long)]
    max_depth: Option<usize>,
    /// Website capture: only follow pages under this path.
    #[arg(long)]
    path_prefix: Option<String>,
    /// Website capture: skip URLs whose path contains this (repeatable).
    #[arg(long)]
    exclude: Vec<String>,
    /// Website capture: pause between requests, in ms.
    #[arg(long)]
    delay_ms: Option<u64>,
}

impl RecipeArgs {
    fn recipe(self) -> Result<Recipe> {
        self.recipe_inheriting(None)
    }

    /// For `refresh`: what the operator declared for the pack's first
    /// edition (category, resources, arch, gpu, and the name, license and
    /// attribution fallbacks) is read back from the pack's manifest when not
    /// given again, so refreshing a pack needs only the source and the new
    /// version. Anything given on the command line or in `--recipe` wins.
    fn recipe_for_refresh(self, pack: &std::path::Path) -> Result<Recipe> {
        let r = wax_core::WaxReader::open(pack).with_context(|| format!("opening {}", pack.display()))?;
        let m = r.manifest().clone();
        self.recipe_inheriting(Some(&m))
    }

    fn recipe_inheriting(self, inherit: Option<&std::collections::BTreeMap<String, String>>) -> Result<Recipe> {
        let mut r = match &self.recipe {
            Some(p) => Recipe::from_toml(&std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?)
                .with_context(|| format!("parsing {}", p.display()))?,
            None => Recipe::default(),
        };
        macro_rules! set {
            ($dst:expr, $src:expr) => {
                if let Some(v) = $src {
                    $dst = Some(v);
                }
            };
        }
        let p = &mut r.pack;
        set!(p.category, self.category);
        set!(p.min_ram_bytes, self.min_ram_bytes);
        set!(p.min_storage_bytes, self.min_storage_bytes);
        set!(p.arch, self.arch);
        set!(p.gpu, self.gpu);
        set!(p.name, self.name);
        set!(p.version, self.version);
        set!(p.license, self.license);
        set!(p.attribution, self.attribution);
        set!(p.languages, self.languages);
        set!(p.entry_point, self.entry_point);
        if let Some(m) = inherit {
            let get = |k: &str| m.get(k).filter(|v| !v.is_empty()).cloned();
            let num = |k: &str| get(k).and_then(|v| v.parse::<i64>().ok());
            p.category = p.category.take().or_else(|| get("category"));
            p.min_ram_bytes = p.min_ram_bytes.or_else(|| num("min_ram_bytes"));
            p.min_storage_bytes = p.min_storage_bytes.or_else(|| num("min_storage_bytes"));
            p.arch = p.arch.take().or_else(|| get("arch"));
            p.gpu = p.gpu.take().or_else(|| get("gpu"));
            p.name = p.name.take().or_else(|| get("name"));
            p.license = p.license.take().or_else(|| get("license"));
            p.attribution = p.attribution.take().or_else(|| get("attribution"));
        }
        let b = &mut r.build;
        if let Some(l) = self.zstd_level {
            b.zstd_level = l;
        }
        if self.no_search_index {
            b.search_index = false;
        }
        set!(b.created_at, self.created_at);
        set!(b.archive_uuid, self.archive_uuid);
        set!(b.sign_key, self.sign_key);
        set!(b.carry_dictionary_from, self.carry_dictionary_from);
        let c = &mut r.crawl;
        if let Some(v) = self.max_pages {
            c.max_pages = v;
        }
        if let Some(v) = self.max_depth {
            c.max_depth = v;
        }
        set!(c.path_prefix, self.path_prefix);
        c.exclude.extend(self.exclude);
        if let Some(v) = self.delay_ms {
            c.delay_ms = v;
        }
        r.validate()?;
        Ok(r)
    }
}

fn emit<T: Serialize>(json: bool, value: &T, text: impl FnOnce(&T)) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        text(value);
    }
    Ok(())
}

fn print_notes(n: &wss_core::report::Notes) {
    if n.is_empty() {
        println!("  not carried  : nothing; every item was carried");
        return;
    }
    println!("  report       : {} items need a look", n.total());
    for (kind, count) in &n.counts {
        println!("    {count:>6} × {}", kind.plain());
        for note in n.notes.iter().filter(|x| x.kind == *kind).take(5) {
            println!("             {}: {}", note.item, note.detail);
        }
        if *count > 5 {
            println!("             … and {} more in the import report", count - 5);
        }
    }
}

fn print_convert(o: &wss_core::ConvertOutcome) {
    println!("converted ({} importer) → {}", o.kind.importer(), o.archive.display());
    println!("  archive_uuid : {}", o.archive_uuid);
    println!("  entries      : {} content, {} redirects", o.entries, o.redirects);
    println!("  search       : {} pages indexed, {} cut into passages", o.indexed, o.passages);
    println!("  compression  : zstd level {}, dictionary {}", o.zstd_level, o.dictionary);
    match (&o.license, o.license_review_required) {
        (Some(l), true) => println!("  license      : {l} — LICENSE REVIEW REQUIRED"),
        (Some(l), false) => println!("  license      : {l}"),
        (None, _) => {}
    }
    match &o.signature_sidecar {
        Some(p) => println!("  signed       : {}", p.display()),
        None => println!("  signed       : no (pass --sign-key to sign)"),
    }
    for (code, n) in &o.warnings {
        println!("  warning      : {code} × {n}");
    }
    print_notes(&o.notes);
    println!("  build report : {}", o.build_report.display());
    println!("  import report: {}", o.import_report.display());
}

fn print_update(u: &manage::UpdateReport) {
    let d = &u.diff;
    if !u.appended {
        println!("up to date: the source has not changed ({} entries unchanged); nothing was appended", d.unchanged);
    } else {
        println!(
            "update appended: {} added, {} changed, {} removed, {} unchanged",
            d.added.len(),
            d.changed.len(),
            d.removed.len(),
            d.unchanged
        );
        println!("  boxes download : {} bytes (pack is now {} bytes)", u.update_bytes, u.file_bytes);
        println!("  segments       : {} ({} more updates before a new base edition)", u.segments, u.segment_budget_left);
        match &u.signature_sidecar {
            Some(p) => println!("  signed         : {}", p.display()),
            None => println!("  signed         : no"),
        }
    }
    for c in &u.cautions {
        println!("  note           : {c}");
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let json = cli.json;
    match cli.cmd {
        Cmd::Convert { source, output, recipe, save_recipe } => {
            let r = recipe.recipe()?;
            let o = wss_core::convert(&source, &output, &r)?;
            if let Some(p) = save_recipe {
                std::fs::write(&p, r.to_toml()?)?;
            }
            emit(json, &o, print_convert)
        }
        Cmd::Refresh { pack, source, recipe } => {
            let r = recipe.recipe_for_refresh(&pack)?;
            let o = wss_core::refresh(&pack, &source, &r)?;
            emit(json, &o, |o| print_update(&o.update))
        }
        Cmd::Detect { source } => {
            let k = wss_core::detect(&source)?;
            emit(json, &k, |k| println!("{} ({} importer)", serde_json::to_string(k).unwrap().trim_matches('"'), k.importer()))
        }
        Cmd::Inspect { pack } => {
            let i = manage::inspect(&pack)?;
            emit(json, &i, |i| {
                println!("{}", i.path.display());
                println!("  format       : WAX {} ({} bytes, {} segment(s))", i.format_version, i.file_bytes, i.segments);
                println!("  archive_uuid : {}", i.archive_uuid);
                println!("  entries      : {} content, {} redirects", i.entries, i.redirects);
                println!("  stored       : {:?}, {} bytes uncompressed", i.stored_bytes_by_codec, i.uncompressed_bytes);
                println!("  dictionary   : {}", i.dictionary_bytes.map(|b| format!("{b} bytes")).unwrap_or_else(|| "none".into()));
                println!("  search       : {}", i.search_tokenizer.as_deref().unwrap_or("none"));
                println!(
                    "  signed       : {}",
                    match (&i.signature_sidecar, i.signed_flag) {
                        (Some(p), _) => format!("sidecar {}", p.display()),
                        (None, true) => "flag set but the sidecar is missing".into(),
                        (None, false) => "no".into(),
                    }
                );
                if i.header_from_mirror {
                    println!("  header       : slot A torn, opened from its mirror; run `wss gc --tail-only`");
                }
                println!("  manifest:");
                for (k, v) in &i.manifest {
                    println!("    {k:<18} = {v}");
                }
            })
        }
        Cmd::Diff { old, new, list } => {
            let d = manage::diff(&old, &new)?;
            emit(json, &d, |d| {
                println!(
                    "{} added, {} changed, {} removed, {} unchanged ({} bytes of new content)",
                    d.added.len(),
                    d.changed.len(),
                    d.removed.len(),
                    d.unchanged,
                    d.changed_bytes
                );
                for (k, (a, b)) in &d.manifest_changes {
                    println!("  manifest {k}: {} → {}", a.as_deref().unwrap_or("(none)"), b.as_deref().unwrap_or("(none)"));
                }
                if list {
                    for p in &d.added {
                        println!("  + {p}");
                    }
                    for p in &d.changed {
                        println!("  ~ {p}");
                    }
                    for p in &d.removed {
                        println!("  - {p}");
                    }
                }
            })
        }
        Cmd::Update { pack, candidate, zstd_level, created_at, sign_key } => {
            let key = wax_builder::sign::resolve_seckey(sign_key);
            let u = manage::update(&pack, &candidate, zstd_level, created_at, key.as_deref())?;
            emit(json, &u, print_update)
        }
        Cmd::Rollback { pack } => {
            let s = manage::rollback(&pack)?;
            println!(
                "rolled back {}: {} segment(s), {} bytes. The previous edition's signature, if kept, verifies again.",
                pack.display(),
                s.segments,
                s.file_length
            );
            Ok(())
        }
        Cmd::Gc { pack, output, tail_only, drop_search } => {
            let g = manage::gc(&pack, output.as_deref(), tail_only, drop_search)?;
            emit(json, &g, |g| {
                println!("{} → {} bytes ({} segment(s) before)", g.bytes_before, g.bytes_after, g.segments_before);
                if g.new_base_edition {
                    println!("  this is a new base edition: sign it again, and earlier updates no longer apply to it");
                }
                if g.search_dropped {
                    println!("  the search index could not be carried exactly and was left out");
                }
            })
        }
        Cmd::Sign { pack, key } => {
            let Some(key) = wax_builder::sign::resolve_seckey(key) else {
                bail!("no secret key: pass --key or set $WAX_MINISIGN_KEY");
            };
            let p = manage::sign(&pack, &key)?;
            println!("signed: {}", p.display());
            Ok(())
        }
        Cmd::Verify { pack, pubkey, require_signature } => {
            let pk = wax_builder::sign::PubKey::resolve(pubkey, None);
            let v = manage::verify(&pack, pk.as_ref(), require_signature)?;
            emit(json, &v, |v| {
                println!("{}: {} entries read back", if v.ok { "ok" } else { "FAILED" }, v.entries_checked);
                for (p, e) in &v.bad_entries {
                    println!("  bad: {p}: {e}");
                }
                if let Some(s) = &v.signature {
                    println!("  signature: valid ({s})");
                }
                if let Some(e) = &v.signature_error {
                    println!("  signature: {e}");
                }
                if let Some(n) = &v.header_note {
                    println!("  note: {n}");
                }
            })?;
            if !v.ok {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}
