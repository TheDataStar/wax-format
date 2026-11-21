//! `deltos-identity-provision` — make the box's first admin (Track F §2).
//!
//! Works **only while the register has no admin**, and refuses ever after,
//! so it is never a way around the checked calls. The password is
//! generated (never a default), printed once on standard output, and only
//! its hash is kept. Run it as the register's owner (`deltos-identity`),
//! for example `runuser -u deltos-identity -- deltos-identity-provision ...`.

use std::path::PathBuf;

use clap::Parser;
use deltos_identity::Register;

#[derive(Parser)]
#[command(name = "deltos-identity-provision", about = "Make the box's first admin, once")]
struct Args {
    #[arg(long, default_value = "/var/lib/deltos/identity")]
    dir: PathBuf,
    /// The admin's name as people see it.
    #[arg(long)]
    display_name: String,
    /// What the admin types to sign in: 1–32 of a-z 0-9 . _ -
    #[arg(long)]
    sign_in_name: String,
    #[arg(long, default_value = "home.deltos.lan")]
    webos_host: String,
}

fn main() {
    let a = Args::parse();
    let r = Register::open(&a.dir, &a.webos_host).and_then(|reg| reg.provision_first_admin(&a.display_name, &a.sign_in_name));
    match r {
        Ok((id, pw)) => {
            println!("First admin created: {} ({})", a.sign_in_name, id);
            println!("Password, shown this once and never stored: {pw}");
            println!("Sign in over HTTPS only: admin passwords are refused over plain HTTP.");
        }
        Err(e) => {
            eprintln!("deltos-identity-provision: {e}");
            std::process::exit(1);
        }
    }
}
