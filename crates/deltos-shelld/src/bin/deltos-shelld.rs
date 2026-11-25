//! `deltos-shelld`: the box's key holder (docs/shelld.md).
//!
//! Runs as root (privilege boundary §3: "privileged"), listening only on a
//! Unix socket that only `deltos-deltd` may open; no network door.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use clap::Parser;
use deltos_identity::protocol::uid_of;
use deltos_shelld::addons::{self, Trust};
use deltos_shelld::drives::DrivesDir;
use deltos_shelld::packs::{self, Trust as PackTrust};
use deltos_shelld::log::ActionLog;
use deltos_shelld::{Config, NetPaths, Shell};

#[derive(Parser)]
#[command(name = "deltos-shelld", about = "DeltOS key holder: the closed list of box-changing actions")]
struct Args {
    /// Where to listen. Its folder's group must be deltos-shellc (rule 1).
    #[arg(long, default_value = "/run/deltos/shelld/shelld.sock")]
    socket: PathBuf,
    /// identityd's socket, for the second check (privilege boundary §5 step 6).
    #[arg(long, default_value = "/run/deltos/identity/identityd.sock")]
    identityd: PathBuf,
    /// The append-only action log (rule 12). Required: its place is still
    /// open (privilege boundary §14), so it has no default.
    #[arg(long)]
    action_log: PathBuf,
    /// The only user served.
    #[arg(long, default_value = "deltos-deltd")]
    deltd_user: String,
    /// TESTS ONLY: also serve this uid.
    #[arg(long)]
    deltd_uid: Vec<u32>,
    /// The network registry and the settings folder it renders into.
    #[arg(long, default_value = "/var/lib/deltos/network/registry.sqlite")]
    net_registry: PathBuf,
    #[arg(long, default_value = "/run/deltos/network")]
    net_settings: PathBuf,
    /// The root everything else is under (tests: a temporary folder). Not
    /// "/" also skips setting owners on restore and stopping services.
    #[arg(long, default_value = "/")]
    root: PathBuf,
    /// Removable drives are mount points in here.
    #[arg(long, default_value = "/run/deltos/drives")]
    drives: PathBuf,
    /// The minisign public key add-on bundles must be signed with. Without
    /// it, every add-on install is refused (never let through unsigned).
    #[arg(long)]
    addon_key: Option<PathBuf>,
    /// Another folder add-on bundles are listed from, as NAME=DIR.
    #[arg(long)]
    addon_source: Vec<String>,
    /// The minisign public key packs must be signed with (Track F §8, "the
    /// project key"). Without it, every pack install is refused.
    #[arg(long)]
    pack_key: Option<PathBuf>,
    /// Another folder packs are listed from, as NAME=DIR.
    #[arg(long)]
    pack_source: Vec<String>,
    /// The helper that reads untrusted packs (rule 18).
    #[arg(long, default_value = "/usr/lib/deltos/bin/deltos-pack-check")]
    pack_check: PathBuf,
    /// The helper's own user (and group of the same name). Required when
    /// shelld runs as root: the helper never runs as root.
    #[arg(long, default_value = "deltos-packcheck")]
    pack_check_user: String,
}

/// The uid and primary gid of a user.
fn ids_of(name: &str) -> Option<(u32, u32)> {
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: getpwnam returns null or a pointer to a static passwd record,
    // read immediately and not kept.
    let p = unsafe { libc::getpwnam(c.as_ptr()) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { ((*p).pw_uid, (*p).pw_gid) })
    }
}

/// What a restore stops and starts (the same as `deltos-backup` at the console).
struct Systemd;
fn systemctl(args: &[&str]) -> Result<(), String> {
    let s = Command::new("systemctl").args(args).status().map_err(|e| e.to_string())?;
    if s.success() {
        Ok(())
    } else {
        Err(format!("systemctl {} failed ({s})", args.join(" ")))
    }
}
impl deltos_backup::Services for Systemd {
    fn stop(&mut self) -> Result<(), String> {
        systemctl(&["stop", "deltos-deltd.service", "deltos-identityd.service"])
    }
    fn start(&mut self) -> Result<(), String> {
        systemctl(&["restart", "deltos-tls.service", "deltos-net-render.service"])?;
        systemctl(&["start", "deltos-identityd.service", "deltos-deltd.service"])
    }
}

fn notify_ready() {
    let Some(path) = std::env::var_os("NOTIFY_SOCKET") else { return };
    if let Ok(s) = std::os::unix::net::UnixDatagram::unbound() {
        let _ = s.send_to(b"READY=1", PathBuf::from(path));
    }
}

fn main() -> ExitCode {
    let a = Args::parse();
    match run(a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("deltos-shelld: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(a: Args) -> Result<(), String> {
    let system = a.root == std::path::Path::new("/");
    let mut deltd = a.deltd_uid.clone();
    match uid_of(&a.deltd_user) {
        Some(u) => deltd.push(u),
        None if deltd.is_empty() => return Err(format!("no user {:?}: nothing could ever call shelld", a.deltd_user)),
        None => {}
    }
    let mut folders = Vec::new();
    for s in &a.addon_source {
        let (n, d) = s.split_once('=').ok_or_else(|| format!("--addon-source {s:?}: NAME=DIR"))?;
        folders.push((n.to_string(), PathBuf::from(d)));
    }
    let trust = match &a.addon_key {
        Some(p) => Trust::from_file(p).map_err(|e| e.to_string())?,
        None => {
            eprintln!("deltos-shelld: no --addon-key: add-on installs will be refused");
            Trust::none()
        }
    };
    let mut pack_folders = Vec::new();
    for s in &a.pack_source {
        let (n, d) = s.split_once('=').ok_or_else(|| format!("--pack-source {s:?}: NAME=DIR"))?;
        pack_folders.push((n.to_string(), PathBuf::from(d)));
    }
    let pack_trust = match &a.pack_key {
        Some(p) => PackTrust::from_file(p).map_err(|e| e.to_string())?,
        None => {
            eprintln!("deltos-shelld: no --pack-key: pack installs will be refused");
            PackTrust::none()
        }
    };
    let am_root = unsafe { libc::geteuid() } == 0;
    let helper_user = match ids_of(&a.pack_check_user) {
        Some((0, _)) => return Err(format!("--pack-check-user {:?} is root; the pack checker never runs as root", a.pack_check_user)),
        Some(ids) => Some(ids),
        None if am_root => return Err(format!("no user {:?}: the pack checker would run as root", a.pack_check_user)),
        None => None,
    };
    let data_owner = if am_root { ids_of(&a.deltd_user) } else { None };
    let log = ActionLog::open(&a.action_log).map_err(|e| format!("{}: {e}", a.action_log.display()))?;
    let backup = if system {
        deltos_backup::Layout::system()
    } else {
        deltos_backup::Layout::at(&a.root.join("var/lib/deltos"))
    };
    let cfg = Config {
        identityd: deltos_identity::Client::with_connections(&a.identityd, 2),
        log,
        deltd_uids: deltd,
        net: NetPaths { registry: a.net_registry, settings: deltos_net::Layout::new(a.net_settings) },
        backup,
        addons: addons::Layout::under(&a.root),
        addon_folders: folders,
        drives: DrivesDir(a.drives),
        trust,
        packs: packs::Layout::under(&a.root),
        pack_trust,
        pack_check: packs::Helper { program: a.pack_check, user: if am_root { helper_user } else { None } },
        pack_data_owner: data_owner,
        pack_folders,
    };
    let services: Box<dyn deltos_backup::Services + Send> =
        if system { Box::new(Systemd) } else { Box::new(deltos_backup::restore::NoServices) };
    let shell = Shell::new(cfg, services);

    let _ = std::fs::remove_file(&a.socket);
    let listener = UnixListener::bind(&a.socket).map_err(|e| format!("{}: {e}", a.socket.display()))?;
    std::fs::set_permissions(&a.socket, std::fs::Permissions::from_mode(0o660)).map_err(|e| e.to_string())?;
    eprintln!("deltos-shelld: listening on {}", a.socket.display());
    notify_ready();
    deltos_shelld::serve(listener, shell).map_err(|e| e.to_string())
}
