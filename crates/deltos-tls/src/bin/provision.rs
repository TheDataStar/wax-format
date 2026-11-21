//! `deltos-tls-provision` — make the box's CA and server certificates the
//! first time, and renew the server certificates when they near their end
//! (docs/box-certificates.md). Run at every boot and daily, as root.

use std::path::PathBuf;
use std::process::Command;

use clap::Parser;
use deltos_tls::{ensure, gid_of, Paths};

#[derive(Parser)]
#[command(name = "deltos-tls-provision", about = "Make and renew the box's certificates")]
struct Args {
    #[arg(long, default_value = "/var/lib/deltos/ca")]
    ca_dir: PathBuf,
    #[arg(long, default_value = "/var/lib/deltos/tls")]
    tls_dir: PathBuf,
    /// The group that reads the server keys.
    #[arg(long, default_value = "deltos-deltd")]
    key_group: String,
    /// Restart this unit if anything was (re)issued, so it serves the new
    /// certificates (deltd reads them at start). Queued, not waited for: at
    /// boot this runs before deltd, and a stopped unit is left stopped.
    #[arg(long)]
    restart: Option<String>,
}

fn main() {
    let a = Args::parse();
    let Some(gid) = gid_of(&a.key_group) else {
        eprintln!("deltos-tls-provision: no group {:?}; install the system users first", a.key_group);
        std::process::exit(1);
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let paths = Paths { ca_dir: a.ca_dir, tls_dir: a.tls_dir, key_group: Some(gid) };
    match ensure(&paths, now) {
        Ok(o) => {
            if o.created_ca {
                println!("Made this box's certificate authority.");
            }
            if o.clock_untrusted {
                println!("WARNING: the clock is before 2026-11-01; certificates were dated from then and will be renewed once the clock is set.");
            }
            if !o.issued.is_empty() {
                println!("Issued: {}", o.issued.join(", "));
            }
            println!("Box certificate fingerprint (SHA-256), to compare on a device before trusting it:\n  {}", o.ca_fingerprint);
            if let (false, Some(unit)) = (o.issued.is_empty(), a.restart) {
                let _ = Command::new("systemctl").args(["--no-block", "try-restart", &unit]).status();
            }
        }
        Err(e) => {
            eprintln!("deltos-tls-provision: {e}");
            std::process::exit(1);
        }
    }
}
