//! `wax-builder` CLI (component A2). All logic lives in the library half of
//! this crate; this file is argument parsing and reporting.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use wax_builder::{
    WriteReport,
    append_pack, build_pack, config::PackConfig, sign, verify_pack, WriteOptions,
};
use wax_core::header::flag;
use wax_core::WaxReader;

#[derive(Parser, Debug)]
#[command(
    name = "wax-builder",
    version,
    about = "Assemble, append to, inspect and verify WAX archives (format v0.9, see SPEC.md)"
)]
struct Args {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Assemble a directory tree into a fresh single-segment .wax archive
    Build {
        #[arg(short, long)]
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Pack config (default: <input>/wax-pack.toml if present)
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// minisign secret key; also $WAX_MINISIGN_KEY
        #[arg(long)]
        sign_key: Option<PathBuf>,
        /// Pin header created_at for reproducible builds; also $SOURCE_DATE_EPOCH
        #[arg(long)]
        created_at: Option<u64>,
        /// Pin archive_uuid instead of minting a fresh UUIDv4 (canonical
        /// hyphenated or bare 32-hex accepted). Required for byte-identical rebuilds.
        #[arg(long)]
        archive_uuid: Option<String>,
    },
    /// Append a new segment (new tree) to an existing archive, then re-sign
    Append {
        #[arg(short, long)]
        archive: PathBuf,
        #[arg(short, long)]
        input: PathBuf,
        #[arg(short, long)]
        config: Option<PathBuf>,
        #[arg(long)]
        sign_key: Option<PathBuf>,
        #[arg(long)]
        created_at: Option<u64>,
    },
    /// Dump the header, manifest and segment chain (add --entries for the entry table)
    Inspect {
        #[arg(short, long)]
        archive: PathBuf,
        /// Also list every entry
        #[arg(short, long)]
        entries: bool,
    },
    /// Re-read every entry (checksum check) and verify the signature sidecar
    Verify {
        #[arg(short, long)]
        archive: PathBuf,
        /// minisign public key file; also $WAX_MINISIGN_PUBKEY
        #[arg(short, long)]
        pubkey: Option<PathBuf>,
        /// minisign public key as a base64 string
        #[arg(long)]
        pubkey_str: Option<String>,
        /// Fail if the signature cannot be checked
        #[arg(long)]
        require_signature: bool,
    },
    /// List every entry (shorthand for `inspect --entries`)
    Ls {
        #[arg(short, long)]
        archive: PathBuf,
    },
    /// Write one entry's bytes to stdout
    Read {
        #[arg(short, long)]
        archive: PathBuf,
        #[arg(short, long)]
        file: String,
    },
}

fn main() -> Result<()> {
    match Args::parse().cmd {
        Commands::Build {
            input,
            output,
            config,
            sign_key,
            created_at,
            archive_uuid,
        } => cmd_build(input, output, config, sign_key, created_at, archive_uuid),
        Commands::Append {
            archive,
            input,
            config,
            sign_key,
            created_at,
        } => cmd_append(archive, input, config, sign_key, created_at),
        Commands::Inspect { archive, entries } => cmd_inspect(archive, entries),
        Commands::Verify {
            archive,
            pubkey,
            pubkey_str,
            require_signature,
        } => cmd_verify(archive, pubkey, pubkey_str, require_signature),
        Commands::Ls { archive } => cmd_inspect(archive, true),
        Commands::Read { archive, file } => cmd_read(archive, file),
    }
}

fn cmd_build(
    input: PathBuf,
    output: PathBuf,
    config: Option<PathBuf>,
    sign_key: Option<PathBuf>,
    created_at: Option<u64>,
    archive_uuid: Option<String>,
) -> Result<()> {
    let cfg = PackConfig::discover(&input, config.as_deref())?;
    let opts = WriteOptions {
        created_at,
        sign_key: sign::resolve_seckey(sign_key),
        archive_uuid: archive_uuid.as_deref().map(wax_builder::parse_uuid).transpose()?,
    };
    let report = build_pack(&input, &output, &cfg, &opts)?;

    println!(
        "built {} — {} entries ({} files, {} aliases), {} segment",
        report.archive.display(),
        report.entries,
        report.stats.files,
        report.stats.aliases,
        report.segments
    );
    println!("  archive_uuid : {}", report.archive_uuid_text());
    match &report.sidecar {
        Some(p) => println!("  signed       : {}", p.display()),
        None => println!("  signed       : no (pass --sign-key to sign)"),
    }
    print_license(&report);
    println!("  build report : {}", report.report_path.display());
    Ok(())
}

fn cmd_append(
    archive: PathBuf,
    input: PathBuf,
    config: Option<PathBuf>,
    sign_key: Option<PathBuf>,
    created_at: Option<u64>,
) -> Result<()> {
    let cfg = PackConfig::discover(&input, config.as_deref())?;
    let opts = WriteOptions {
        created_at,
        sign_key: sign::resolve_seckey(sign_key),
        // append always reuses the archive's existing identity (SPEC §2)
        archive_uuid: None,
    };
    let report = append_pack(&archive, &input, &cfg, &opts)?;

    println!(
        "appended to {} — {} new entries, now {} segments",
        report.archive.display(),
        report.entries,
        report.segments
    );
    println!(
        "  archive_uuid : {} (preserved)",
        report.archive_uuid_text()
    );
    print_license(&report);
    match &report.sidecar {
        Some(p) => println!("  re-signed    : {}", p.display()),
        None => {
            let sidecar = sign::sidecar_path(&archive);
            if sidecar.is_file() {
                println!(
                    "  WARNING      : {} is now STALE — the digest covers the new header \
                     and segment chain (SPEC 8.1). Re-run with --sign-key.",
                    sidecar.display()
                );
            } else {
                println!("  signed       : no");
            }
        }
    }
    Ok(())
}

fn cmd_inspect(archive: PathBuf, list_entries: bool) -> Result<()> {
    let reader = WaxReader::open(&archive)
        .with_context(|| format!("opening {}", archive.display()))?;
    let h = reader.header();

    println!("format version : {}.{}", h.version_major, h.version_minor);
    println!("archive_uuid   : {}", wax_builder::uuid_text(&h.archive_uuid));
    println!("created_at     : {}", h.created_at);
    println!(
        "flags          : 0x{:04x}{}",
        h.flags,
        describe_flags(h.flags)
    );
    println!("index_offset   : {}", h.index_offset);
    println!("index_length   : {}", h.index_length);
    println!("blob_section   : {}", h.blob_section_length);
    println!("search_index   : offset={} length={}", h.search_index_offset, h.search_index_length);
    println!("segments       : {}", reader.segment_count());
    println!("entries        : {}", reader.entry_count()?);
    println!("file_size      : {}", reader.file_size());

    let sidecar = sign::sidecar_path(&archive);
    println!(
        "sidecar        : {}",
        if sidecar.is_file() {
            sidecar.display().to_string()
        } else {
            "absent".to_string()
        }
    );

    let manifest = reader.manifest();
    if manifest.is_empty() {
        println!("manifest       : (empty)");
    } else {
        println!("manifest:");
        for (k, v) in manifest {
            println!("  {k} = {v}");
        }
    }
    if !manifest.is_empty() {
        // Resolve the hardware requirement, mapping a retired `min_hw_tier`
        // where a pack predates the measured-resource migration.
        let resolved = wax_builder::config::resolve_hw_requirement(manifest);
        if let Some(hw) = &resolved {
            let gib = |b: i64| b as f64 / (1024.0 * 1024.0 * 1024.0);
            match &hw.from_legacy_tier {
                None => println!(
                    "  -> requires {:.0} GiB RAM, {:.0} GiB storage, arch {}{}",
                    gib(hw.min_ram_bytes),
                    gib(hw.min_storage_bytes),
                    hw.arch,
                    hw.gpu.as_deref().map(|g| format!(", gpu {g}")).unwrap_or_default()
                ),
                // Each retired tier maps to its own historical floor, so the
                // resolved requirement is the one the pack was built against
                // — nothing is lost, and there is nothing to warn about.
                Some(tier) => println!(
                    "  -> legacy `min_hw_tier = {tier}` mapped to {:.0} GiB RAM, \
                     {:.0} GiB storage, arch {} (pack predates the measured-resource \
                     migration; the value is mapped on read, not rewritten)",
                    gib(hw.min_ram_bytes),
                    gib(hw.min_storage_bytes),
                    hw.arch
                ),
            }
        } else if let Some(tier) = manifest.get("min_hw_tier") {
            // The one case that is still a genuine problem: a `min_hw_tier`
            // outside the four retired names. It cannot be mapped, and
            // guessing a floor for it is exactly what the mapping rule exists
            // to prevent.
            println!(
                "  WARNING: `min_hw_tier = {tier}` is not one of the four retired tiers and \
                 cannot be mapped to a resource floor. This pack's hardware requirement is \
                 unknown — it is NOT being treated as runnable anywhere. Re-build it with \
                 declared resource fields (docs/cross-track-contract.md §2.3, §11)."
            );
        }

        // Absent-field warning. A pack whose legacy tier resolved is not
        // missing its resource fields in any meaningful sense, so saying so
        // adds nothing once the mapping above has reported what it resolved
        // to. A tier that did NOT resolve still gets the warning.
        let mapped = resolved.as_ref().is_some_and(|h| h.from_legacy_tier.is_some());
        let missing: Vec<&str> = wax_builder::config::REQUIRED_FIELDS
            .iter()
            .copied()
            .filter(|k| !manifest.contains_key(*k))
            .filter(|k| {
                !(mapped && matches!(*k, "min_ram_bytes" | "min_storage_bytes" | "arch"))
            })
            .collect();
        if !missing.is_empty() {
            println!(
                "  WARNING: required B3 field(s) absent: {} \
                 (pack predates schema enforcement, or was not built by wax-builder)",
                missing.join(", ")
            );
        }
    }

    if list_entries {
        println!();
        println!(
            "{:<52} {:>10} {:>10}  {:<22} {:<6} REDIRECT",
            "PATH", "SIZE", "STORED", "MIME", "CODEC"
        );
        for e in reader.entries() {
            let e = e?;
            println!(
                "{:<52} {:>10} {:>10}  {:<22} {:<6} {}",
                e.path,
                e.uncompressed_length,
                e.length,
                e.mime.as_deref().unwrap_or("-"),
                e.compression,
                e.redirect_to.as_deref().unwrap_or("")
            );
        }
    }
    Ok(())
}

fn cmd_verify(
    archive: PathBuf,
    pubkey: Option<PathBuf>,
    pubkey_str: Option<String>,
    require_signature: bool,
) -> Result<()> {
    let pk = sign::PubKey::resolve(pubkey, pubkey_str);
    let report = verify_pack(&archive, pk.as_ref(), require_signature)?;

    println!(
        "entries        : {} read OK, {} redirects",
        report.entries_checked, report.redirects
    );
    for (path, err) in &report.bad_entries {
        println!("  FAIL {path}: {err}");
    }
    match (&report.signature, &report.signature_error) {
        (Some(s), _) => {
            println!("signature      : VALID");
            println!("  sidecar      : {}", s.sidecar.display());
            println!("  trusted      : {}", s.trusted_comment);
            println!("  archive_uuid : {} (matches header)", s.header_uuid);
        }
        (None, Some(e)) => println!("signature      : FAILED — {e}"),
        (None, None) => {
            let sidecar = sign::sidecar_path(&archive);
            if sidecar.is_file() {
                println!(
                    "signature      : present but unchecked (no public key; pass --pubkey)"
                );
            } else {
                println!("signature      : none");
            }
        }
    }

    if report.ok() {
        println!("result         : OK");
        Ok(())
    } else {
        bail!("verification failed");
    }
}

fn cmd_read(archive: PathBuf, file: String) -> Result<()> {
    use std::io::Write;
    let reader = WaxReader::open(&archive)?;
    let data = reader.read(&file)?;
    std::io::stdout().write_all(&data)?;
    Ok(())
}

fn describe_flags(flags: u16) -> String {
    let mut set = Vec::new();
    if flags & flag::HAS_SEARCH_INDEX != 0 {
        set.push("has_search_index");
    }
    if flags & flag::HAS_DELTA_BASE != 0 {
        set.push("has_delta_base");
    }
    if flags & flag::IS_MULTI_VOLUME != 0 {
        set.push("is_multi_volume");
    }
    if flags & flag::IS_SIGNED != 0 {
        set.push("is_signed");
    }
    if set.is_empty() {
        String::new()
    } else {
        format!(" [{}]", set.join(" "))
    }
}

/// One line per build/append stating the Contract §11 licensing outcome.
fn print_license(report: &WriteReport) {
    match &report.license {
        None => {}
        Some(l) if l.review_required() => println!(
            "  license      : {:?} — LICENSE REVIEW REQUIRED (not on the SPDX allowlist; \
             the pack builds, and the catalog's intake routes it to human review)",
            l.license()
        ),
        Some(l) => println!("  license      : {} (allowlisted, builds clean)", l.license()),
    }
}
