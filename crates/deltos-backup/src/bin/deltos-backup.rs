//! `deltos-backup`: backup and restore at the box's own console, as root
//! (docs/box-backup.md §6). The same code `deltos-shelld` runs for the
//! admin actions; root already owns the box (privilege boundary rule 2).
//!
//!   deltos-backup create  --to DIR            write a backup into DIR
//!   deltos-backup list    DIR                 the backups in DIR
//!   deltos-backup review  --from DIR ID       check and stage one; prints a review id
//!   deltos-backup restore REVIEW_ID [--yes]   put the reviewed backup in place
//!
//! The passphrase is asked for on the terminal without echo, or read from
//! the first line of standard input with `--passphrase-stdin`. It is never
//! an argument (arguments are visible to every user in the process list).

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};
use deltos_backup::actions::{self, Action, Outcome};
use deltos_backup::{restore, Layout, Passphrase, Services};

#[derive(Parser)]
#[command(name = "deltos-backup", about = "Back up and restore the DeltOS box's own state (root, at the box)")]
struct Cli {
    /// The state tree. Anything but the default also skips owners and services (testing).
    #[arg(long, default_value = deltos_backup::STATE_ROOT)]
    root: PathBuf,
    /// Do not stop and start the box's services around a restore.
    #[arg(long)]
    no_services: bool,
    /// Print results as JSON.
    #[arg(long)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Create {
        #[arg(long)]
        to: PathBuf,
        #[arg(long)]
        passphrase_stdin: bool,
    },
    List {
        dir: PathBuf,
    },
    Review {
        #[arg(long)]
        from: PathBuf,
        backup_id: String,
        #[arg(long)]
        passphrase_stdin: bool,
    },
    Restore {
        review_id: String,
        /// Skip typing "restore" to confirm.
        #[arg(long)]
        yes: bool,
    },
}

/// The drive is the folder named on the command line.
struct Folder(PathBuf);
impl actions::Drives for Folder {
    fn mount_point(&self, _: &str, _: bool) -> Option<PathBuf> {
        Some(self.0.clone())
    }
}

/// The services whose state a restore replaces (system/systemd).
struct Systemd;
fn systemctl(args: &[&str]) -> Result<(), String> {
    let s = Command::new("systemctl").args(args).status().map_err(|e| e.to_string())?;
    if s.success() {
        Ok(())
    } else {
        Err(format!("systemctl {} failed ({s})", args.join(" ")))
    }
}
impl Services for Systemd {
    fn stop(&mut self) -> Result<(), String> {
        systemctl(&["stop", "deltos-deltd.service", "deltos-identityd.service"])
    }
    fn start(&mut self) -> Result<(), String> {
        // certificates first (checks the restored CA, renews if due), then
        // the network settings (the path watcher restarts the radio and
        // DNS/DHCP if they changed), then sign-in and the box API
        systemctl(&["restart", "deltos-tls.service", "deltos-net-render.service"])?;
        systemctl(&["start", "deltos-identityd.service", "deltos-deltd.service"])
    }
}

fn read_tty(prompt: &str) -> io::Result<String> {
    use std::os::fd::AsRawFd;
    let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let fd = tty.as_raw_fd();
    // SAFETY: termios calls on an fd we hold open.
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let old = t;
    t.c_lflag &= !libc::ECHO;
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &t) };
    let mut w = &tty;
    write!(w, "{prompt}")?;
    let mut line = String::new();
    let r = io::BufReader::new(&tty).read_line(&mut line);
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &old) };
    writeln!(w)?;
    r?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

fn passphrase(from_stdin: bool, twice: bool) -> Result<Passphrase, String> {
    let p = if from_stdin {
        let mut l = String::new();
        io::stdin().lock().read_line(&mut l).map_err(|e| e.to_string())?;
        l.trim_end_matches(['\n', '\r']).to_string()
    } else {
        let a = read_tty("Backup passphrase: ").map_err(|e| format!("no terminal to ask on ({e}); use --passphrase-stdin"))?;
        if twice {
            let b = read_tty("Type it again: ").map_err(|e| e.to_string())?;
            if a != b {
                return Err("the two passphrases differ".into());
            }
        }
        a
    };
    Passphrase::new(p).map_err(|e| e.to_string())
}

fn when(t: i64) -> String {
    let s = deltos_backup::archive::utc_stamp_pub(t);
    format!("{}-{}-{} {}:{}:{} UTC", &s[0..4], &s[4..6], &s[6..8], &s[9..11], &s[11..13], &s[13..15])
}

fn run(cli: Cli) -> Result<(), String> {
    let mut layout = Layout::at(&cli.root);
    let system = cli.root.as_os_str() == deltos_backup::STATE_ROOT;
    layout.set_owners = system;
    let console = |a: Action| actions::authorize_console(a).map_err(|r| format!("{r} (run as root at the box)"));
    let print = |v: &dyn erased::Json| if cli.json { println!("{}", v.json()) };
    match cli.cmd {
        Cmd::List { dir } => {
            let l = deltos_backup::list(&dir).map_err(|e| e.to_string())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&l).unwrap());
            } else if l.is_empty() {
                println!("no backups in {}", dir.display());
            } else {
                for b in l {
                    println!("{}  {} bytes", b.backup_id, b.bytes);
                }
            }
        }
        Cmd::Create { to, passphrase_stdin } => {
            let p = passphrase(passphrase_stdin, true)?;
            let a = console(Action::StartBackup { drive: "console".into(), passphrase: p })?;
            eprintln!("deltos-backup: {}", a.action().summary());
            let Outcome::Created(c) = actions::perform(&layout, a, &Folder(to.clone()), &mut restore::NoServices).map_err(|e| e.to_string())? else { unreachable!() };
            print(&c);
            if !cli.json {
                println!("wrote {} ({} bytes, {} files, {} packs listed)", to.join(&c.backup_id).display(), c.bytes, c.files, c.packs_listed);
                for s in &c.skipped {
                    println!("  skipped {}: {}", s.path, s.reason);
                }
                println!("Keep the passphrase somewhere safe: without it nobody, including you, can open this backup.");
            }
        }
        Cmd::Review { from, backup_id, passphrase_stdin } => {
            let p = passphrase(passphrase_stdin, false)?;
            let a = console(Action::ReviewBackup { drive: "console".into(), backup_id, passphrase: p })?;
            eprintln!("deltos-backup: {}", a.action().summary());
            let Outcome::Reviewed(r) = actions::perform(&layout, a, &Folder(from), &mut restore::NoServices).map_err(|e| e.to_string())? else { unreachable!() };
            print(&*r);
            if !cli.json {
                println!("Backup {} made {} by {}", r.backup_id, when(r.created_at), r.made_by);
                if let Some(p) = &r.people {
                    let kinds: Vec<String> = p.by_kind.iter().map(|(k, n)| format!("{n} {k}")).collect();
                    let roles: Vec<String> = p.roles.iter().map(|(k, n)| format!("{k} {n}")).collect();
                    println!("  people: {} ({}); roles: {}", p.profiles, kinds.join(", "), roles.join(", "));
                    println!("  admins who can sign in: {}", if p.admins.is_empty() { "none".into() } else { p.admins.join(", ") });
                    println!("  remembered devices {}, sign-in cards {}; last activity {}", p.remembered_devices, p.sign_in_cards, p.last_activity.map(when).unwrap_or_else(|| "none".into()));
                }
                if let Some(f) = &r.ca_fingerprint {
                    println!("  certificate (stamp): {f}");
                }
                if let Some(f) = &r.current_ca_fingerprint {
                    println!("  this box's now:      {f}");
                }
                if let Some(n) = &r.network {
                    println!("  network: {} on {}{}", n.mode, n.interface, n.ssid.as_ref().map(|s| format!(", Wi-Fi name {s:?}")).unwrap_or_default());
                }
                for p in &r.packs {
                    println!("  pack {} {} ({}){}", p.pack.name, p.pack.version, p.pack.archive_uuid, if p.present { "" } else { " - reinstall" });
                }
                println!("  replaces: {}", r.replaces.join(", "));
                if !r.keeps.is_empty() {
                    println!("  keeps as it is: {}", r.keeps.join(", "));
                }
                for w in &r.warnings {
                    println!("  ! {w}");
                }
                println!("To restore it: deltos-backup restore {}", r.review_id);
            }
        }
        Cmd::Restore { review_id, yes } => {
            let digest = restore::staged_digest(&layout, &review_id).map_err(|e| e.to_string())?;
            if !yes {
                eprint!("This replaces the people, settings and certificate on this box. Type \"restore\" to go ahead: ");
                let mut l = String::new();
                io::stdin().lock().read_line(&mut l).map_err(|e| e.to_string())?;
                if l.trim() != "restore" {
                    return Err("not confirmed; nothing changed".into());
                }
            }
            let a = console(Action::RestoreBackup { review_id, review_digest: digest })?;
            eprintln!("deltos-backup: {}", a.action().summary());
            let mut sys = Systemd;
            let mut none = restore::NoServices;
            let svc: &mut dyn Services = if system && !cli.no_services { &mut sys } else { &mut none };
            let Outcome::Restored(r) = actions::perform(&layout, a, &Folder(PathBuf::new()), svc).map_err(|e| e.to_string())? else { unreachable!() };
            print(&r);
            if !cli.json {
                println!("restored: {}; the replaced state is in {}", r.replaced.join(", "), r.previous.display());
                for p in &r.packs_to_reinstall {
                    println!("  reinstall pack {} {} ({})", p.name, p.version, p.archive_uuid);
                }
            }
            if let Some(e) = r.service_error {
                return Err(format!("restored, but the services did not start: {e}"));
            }
        }
    }
    Ok(())
}

mod erased {
    pub trait Json {
        fn json(&self) -> String;
    }
    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> String {
            serde_json::to_string_pretty(self).unwrap()
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("deltos-backup: {e}");
            ExitCode::FAILURE
        }
    }
}
