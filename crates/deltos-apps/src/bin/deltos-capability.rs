//! `deltos-capability`: the box measures itself and writes its capability
//! record (contract §9; `docs/box-capability.md`).
//!
//! * `provision --profile P`: once, when the box is set up. The profile is
//!   chosen (contract §3) and refused on a box below its floor.
//! * `measure`: at every boot (`deltos-capability.service`), before any
//!   DeltOS service starts. Keeps the chosen profile; reads the hardware
//!   again.

use clap::{Parser, Subcommand, ValueEnum};
use deltos_apps::manifest::Profile;
use deltos_apps::measure::{self, Machine};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "deltos-capability",
    about = "Measure this box and write its capability record"
)]
struct Cli {
    /// The record (contract §10).
    #[arg(long, default_value = "/var/lib/deltos/capability.json")]
    record: PathBuf,
    /// The state tree, whose drive is the box's storage.
    #[arg(long, default_value = "/var/lib/deltos")]
    state: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum P {
    Kiosk,
    Classroom,
    CommunityHub,
    FieldOps,
}

#[derive(Subcommand)]
enum Cmd {
    /// First setup only: choose the profile and write the first record.
    Provision {
        /// No default: what the box runs is the person's choice (contract §3).
        #[arg(long, value_enum)]
        profile: P,
    },
    /// Measure again, keeping the profile (every boot).
    Measure,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let m = Machine::this(&cli.state);
    let print = |c: &deltos_apps::fit::Capability| {
        println!("{}", serde_json::to_string(c).unwrap_or_default())
    };
    let res = match cli.cmd {
        Cmd::Provision { profile } => {
            let profile = match profile {
                P::Kiosk => Profile::Kiosk,
                P::Classroom => Profile::Classroom,
                P::CommunityHub => Profile::CommunityHub,
                P::FieldOps => Profile::FieldOps,
            };
            measure::provision(&m, &cli.record, profile).map(|(c, running)| {
                print(&c);
                if !running.is_empty() {
                    eprintln!("os_ram_bytes not measured: DeltOS services are running ({}); the next boot measures it", running.join(", "));
                }
            })
        }
        Cmd::Measure => measure::remeasure(&m, &cli.record).map(|(c, running, below)| {
            print(&c);
            if !running.is_empty() {
                eprintln!("os_ram_bytes kept from the last measurement: DeltOS services are running ({})", running.join(", "));
            }
            if let Some(b) = below {
                eprintln!("WARNING: {b}. The record says what this box is; the fit check refuses what it cannot run.");
            }
        }),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("deltos-capability: {e}");
            ExitCode::FAILURE
        }
    }
}
