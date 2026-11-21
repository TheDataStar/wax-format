//! `deltos-identityd` — the box's register of people and sign-ins, on a
//! local socket only `deltd` and `deltos-shelld` may open
//! (docs/identityd.md, docs/privilege-boundary.md §3).

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use deltos_identity::protocol::{self, uid_of, Callers};
use deltos_identity::Register;

#[derive(Parser)]
#[command(name = "deltos-identityd", about = "The box's register of people, roles and sign-ins")]
struct Args {
    /// The register's directory (contract §10).
    #[arg(long, default_value = "/var/lib/deltos/identity")]
    dir: PathBuf,
    /// The socket to listen on. Its directory should be setgid
    /// deltos-identc so the socket's group is (system/tmpfiles.d).
    #[arg(long, default_value = "/run/deltos/identity/identityd.sock")]
    socket: PathBuf,
    /// The WebOS hostname, written into sign-in cards' QR codes (contract §8).
    #[arg(long, default_value = "home.deltos.lan")]
    webos_host: String,
    /// The user deltd runs as: the only caller of the sign-in calls.
    #[arg(long, default_value = "deltos-deltd")]
    deltd_user: String,
    /// The user deltos-shelld runs as (skipped if it does not exist yet).
    #[arg(long, default_value = "deltos-shelld")]
    shelld_user: String,
    /// TESTS ONLY: also treat this uid as deltd.
    #[arg(long)]
    deltd_uid: Vec<u32>,
    /// TESTS ONLY: also treat this uid as shelld.
    #[arg(long)]
    shelld_uid: Vec<u32>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = Args::parse();
    if !a.webos_host.ends_with(".deltos.lan") {
        return Err("--webos-host must be a name under deltos.lan (contract §8)".into());
    }
    let reg = Arc::new(Register::open(&a.dir, &a.webos_host)?);
    let mut callers = Callers { deltd: a.deltd_uid.clone(), shelld: a.shelld_uid.clone() };
    match uid_of(&a.deltd_user) {
        Some(u) => callers.deltd.push(u),
        None if callers.deltd.is_empty() => {
            return Err(format!("no user {:?}: nothing could ever call identityd", a.deltd_user).into())
        }
        None => {}
    }
    if let Some(u) = uid_of(&a.shelld_user) {
        callers.shelld.push(u);
    }
    let _ = std::fs::remove_file(&a.socket);
    let listener = UnixListener::bind(&a.socket)?;
    std::fs::set_permissions(&a.socket, std::fs::Permissions::from_mode(0o660))?;
    eprintln!(
        "identityd: listening on {} (deltd uids {:?}, shelld uids {:?}); admin exists: {}",
        a.socket.display(),
        callers.deltd,
        callers.shelld,
        reg.has_admin()?
    );

    let sweeper = Arc::clone(&reg);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(600));
        if let Err(e) = sweeper.sweep() {
            eprintln!("identityd: sweep failed: {e}");
        }
    });
    notify_ready();
    protocol::serve(listener, reg, callers)?;
    Ok(())
}

/// Tell systemd the socket is ready (Type=notify), so deltd starts after it.
fn notify_ready() {
    let Some(path) = std::env::var_os("NOTIFY_SOCKET") else { return };
    let s = match std::os::unix::net::UnixDatagram::unbound() {
        Ok(s) => s,
        Err(_) => return,
    };
    let p = PathBuf::from(&path);
    if p.as_os_str().as_encoded_bytes().first() == Some(&b'@') {
        #[cfg(target_os = "linux")]
        {
            use std::os::linux::net::SocketAddrExt;
            if let Ok(addr) = std::os::unix::net::SocketAddr::from_abstract_name(&p.as_os_str().as_encoded_bytes()[1..]) {
                let _ = s.send_to_addr(b"READY=1", &addr);
            }
        }
    } else {
        let _ = s.send_to(b"READY=1", &p);
    }
}
