//! `zim2wax` CLI.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use zim2wax::{convert, probe, ConvertOptions};

#[derive(Parser, Debug)]
#[command(name = "zim2wax", version, about = "Convert a ZIM archive into a .wax pack (DeltOS B1)")]
struct Args {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// ZIM in, .wax out. Writes <output>.build-report.json beside the pack.
    Convert {
        #[arg(short, long)]
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// REQUIRED — a ZIM carries no analog. One of: reference, education,
        /// media, tools, civic, health (Contract §11).
        #[arg(long)]
        category: String,
        /// REQUIRED — a ZIM carries no analog. The lowest board tier the pack
        /// runs on: pi_zero_2w, pi_4, pi_5, mini_pc (Contract §2). Never generic.
        #[arg(long)]
        min_hw_tier: String,
        /// License to record when the ZIM has NO License metadata (current
        /// Wikipedia ZIMs omit it and a blank license is a hard failure,
        /// Contract §11). Ignored when the ZIM states one. SPDX id or text.
        #[arg(long)]
        license: Option<String>,
        /// Credit line to record when the ZIM carries neither Creator nor
        /// Publisher (attribution is required, Contract §11). Ignored when the
        /// ZIM states either.
        #[arg(long)]
        attribution: Option<String>,
        /// minisign secret key; also $WAX_MINISIGN_KEY
        #[arg(long)]
        sign_key: Option<PathBuf>,
        /// Pin header created_at for reproducible builds; also $SOURCE_DATE_EPOCH
        #[arg(long)]
        created_at: Option<u64>,
        /// Pin archive_uuid (canonical hyphenated or bare 32-hex) — required to
        /// re-convert into an existing pack's lineage (Track B §8: B8 re-crawls
        /// must reuse the archive_uuid so A6 sees an update, not a new pack).
        #[arg(long)]
        archive_uuid: Option<String>,
    },
    /// Read-only: show a ZIM's metadata and what the manifest would derive.
    Probe {
        #[arg(short, long)]
        input: PathBuf,
    },
}

fn main() -> Result<()> {
    match Args::parse().cmd {
        Commands::Convert {
            input,
            output,
            category,
            min_hw_tier,
            license,
            attribution,
            sign_key,
            created_at,
            archive_uuid,
        } => cmd_convert(
            input, output, category, min_hw_tier, license, attribution, sign_key, created_at, archive_uuid,
        ),
        Commands::Probe { input } => cmd_probe(input),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_convert(
    input: PathBuf,
    output: PathBuf,
    category: String,
    min_hw_tier: String,
    license: Option<String>,
    attribution: Option<String>,
    sign_key: Option<PathBuf>,
    created_at: Option<u64>,
    archive_uuid: Option<String>,
) -> Result<()> {
    let opts = ConvertOptions {
        category,
        min_hw_tier,
        license_if_absent: license,
        attribution_if_absent: attribution,
        created_at: created_at.or_else(|| {
            std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|v| v.parse().ok())
        }),
        archive_uuid: archive_uuid.as_deref().map(wax_builder::parse_uuid).transpose()?,
        sign_key: wax_builder::sign::resolve_seckey(sign_key),
    };
    opts.validate()?;

    let r = convert(&input, &output, &opts)?;
    let w = &r.write;
    println!(
        "converted {} → {}",
        input.display(),
        output.display()
    );
    println!(
        "  dirents      : {} read; {} content + {} redirects emitted; {} hrefs rewritten",
        r.stats.dirents, r.stats.content_emitted, r.stats.redirects_emitted, r.stats.hrefs_rewritten
    );
    println!("  archive_uuid : {}", w.archive_uuid_text());
    println!("  name         : {}", r.derived.name);
    println!("  version      : {}", r.derived.version);
    println!("  entry_point  : {}", r.derived.entry_point);
    println!(
        "  icon         : {} ({})",
        zim2wax::ICON_PATH,
        r.derived.icon_source.as_deref().unwrap_or("generated placeholder")
    );
    match &r.derived.languages {
        Some(l) => println!("  languages    : {l}"),
        None if r.stats.language_unmapped => println!("  languages    : (omitted — ZIM Language has no BCP-47 mapping)"),
        None => println!("  languages    : (omitted — ZIM has no Language metadata)"),
    }
    println!(
        "  not copied   : {} search-index (X/) + {} well-known (W/) dirents, by design",
        r.stats.search_index_entries, r.stats.wellknown_entries
    );
    match &w.license {
        Some(l) if l.review_required() => println!(
            "  license      : {:?} — LICENSE REVIEW REQUIRED ({})",
            l.license(),
            if r.warnings.count("license_operator_supplied") > 0 {
                "operator-supplied; reviewed rather than trusted"
            } else {
                "not on the SPDX allowlist"
            }
        ),
        Some(l) => println!("  license      : {} (allowlisted, builds clean)", l.license()),
        None => {}
    }
    match &w.sidecar {
        Some(p) => println!("  signed       : {}", p.display()),
        None => println!("  signed       : no (pass --sign-key to sign)"),
    }
    if !r.warnings.is_empty() {
        println!("  warnings     :");
        for (code, n) in r.warnings.iter() {
            println!("    {code:<28} {n}");
        }
    }
    println!("  build report : {}", w.report_path.display());
    Ok(())
}

fn cmd_probe(input: PathBuf) -> Result<()> {
    let p = probe(&input)?;
    println!("ZIM {}", input.display());
    println!("  format       : {}.{}", p.version.0, p.version.1);
    println!("  dirents      : {}", p.dirents);
    println!("  main page    : {}", p.main_page.as_deref().unwrap_or("(none)"));
    println!("  illustration : {}", p.illustration.as_deref().unwrap_or("(none — placeholder would be generated)"));
    println!("  metadata:");
    for (k, v) in &p.metadata {
        let v = if v.len() > 80 { format!("{}…", &v[..80]) } else { v.clone() };
        println!("    {k:<14} = {v}");
    }
    println!("  manifest derivations (Track B §20):");
    println!("    name         ← Title      = {:?}", p.metadata.get("Title").map(String::as_str).unwrap_or(""));
    match &p.derived_version {
        Ok(v) => println!("    version      ← Date       = {v}"),
        Err(e) => println!("    version      ← Date       = ERROR: {e}"),
    }
    let attribution = p
        .metadata
        .get("Creator")
        .or(p.metadata.get("Publisher"))
        .map(String::as_str)
        .unwrap_or("");
    println!("    attribution  ← Creator/Publisher = {attribution:?}");
    println!("    license      ← License    = {:?}", p.metadata.get("License").map(String::as_str).unwrap_or(""));
    println!(
        "    languages    ← Language   = {}",
        p.languages.as_deref().unwrap_or("(omitted)")
    );
    println!("    entry_point  ← main page  = {}", p.main_page.as_deref().unwrap_or("(none)"));
    println!("    category, min_hw_tier      = supplied by --category / --min-hw-tier (no ZIM source)");
    Ok(())
}
