//! `deltos-apps`: check app manifests, run the fit check, and print the
//! capacity estimate (docs/base-addons-admin.md).

use clap::{Parser, Subcommand};
use deltos_apps::{capacity, fit, parse, read_dir};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "deltos-apps",
    about = "DeltOS app manifests, fit check and capacity estimate"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check one or more deltos-app.toml files; prints every problem.
    Check { files: Vec<PathBuf> },
    /// May this box install these add-ons? Prints the fit as JSON.
    Fit {
        /// The box capability record (contract §9).
        #[arg(long)]
        capability: PathBuf,
        /// Directory of the manifests already installed (base and add-ons).
        #[arg(long)]
        installed: PathBuf,
        /// Free bytes on the state filesystem, measured now.
        #[arg(long)]
        free_storage_bytes: u64,
        /// The add-on manifest(s) to install together.
        set: Vec<PathBuf>,
    },
    /// People-at-once estimate from a measured-figures file and a scenario file.
    Capacity {
        #[arg(long)]
        measured: PathBuf,
        #[arg(long)]
        scenario: PathBuf,
    },
}

fn read(p: &PathBuf) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn run() -> Result<bool, String> {
    match Cli::parse().cmd {
        Cmd::Check { files } => {
            let mut ok = true;
            for f in &files {
                match parse(&read(f)?) {
                    Ok(m) => {
                        let review = if m.licence_review_required() {
                            " (licence goes to review)"
                        } else {
                            ""
                        };
                        println!(
                            "{}: ok, {} {} {:?}{review}",
                            f.display(),
                            m.id,
                            m.version,
                            m.resources.evidence
                        );
                    }
                    Err(e) => {
                        ok = false;
                        println!("{}: {e}", f.display());
                    }
                }
            }
            Ok(ok)
        }
        Cmd::Fit {
            capability,
            installed,
            free_storage_bytes,
            set,
        } => {
            let cap: fit::Capability = serde_json::from_str(&read(&capability)?)
                .map_err(|e| format!("{}: {e}", capability.display()))?;
            let inst = read_dir(&installed)?;
            let mut s = Vec::new();
            for f in &set {
                s.push(parse(&read(f)?).map_err(|e| format!("{}: {e}", f.display()))?);
            }
            let r = fit::check(&cap, &inst, &s, free_storage_bytes);
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
            Ok(r.verdict != fit::Verdict::DoesNotFit)
        }
        Cmd::Capacity { measured, scenario } => {
            let m: capacity::Measured = toml::from_str(&read(&measured)?)
                .map_err(|e| format!("{}: {e}", measured.display()))?;
            let s: capacity::Scenario = toml::from_str(&read(&scenario)?)
                .map_err(|e| format!("{}: {e}", scenario.display()))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&capacity::estimate(&m, &s)).unwrap()
            );
            Ok(true)
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("deltos-apps: {e}");
            ExitCode::from(2)
        }
    }
}
