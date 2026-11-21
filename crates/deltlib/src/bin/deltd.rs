//! `deltd` — serves every installed pack and the Library API.
//!
//! There is no anonymous mode: an identity source is required, either the
//! live register (`--identity-socket`, deltos-identityd) or, for development
//! only, a fixed table (`--dev-sessions`).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use deltlib::{router, AppState, Catalog, Identity, IdentitydLink, Library, StaticIdentity};

#[derive(Parser)]
#[command(name = "deltd", about = "Serve every installed .wax pack and the deltd API v1")]
struct Args {
    /// The packs directory (contract §10: /var/lib/deltos/packs/).
    #[arg(long, default_value = "/var/lib/deltos/packs")]
    packs: PathBuf,
    /// The catalog directory, which holds the pack-hostname table
    /// (contract §10: /var/lib/deltos/b4/).
    #[arg(long, default_value = "/var/lib/deltos/b4")]
    catalog_dir: PathBuf,
    /// The WebOS hostname under deltos.lan (contract §8: home.deltos.lan).
    #[arg(long)]
    webos_host: String,
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:80")]
    listen: SocketAddr,
    /// deltos-identityd's socket: the live register of people and sign-ins.
    #[arg(long, conflicts_with = "dev_sessions", required_unless_present = "dev_sessions")]
    identity_socket: Option<PathBuf>,
    /// DEVELOPMENT ONLY: a fixed table of sessions and pack partitions, with
    /// no sign-in at all.
    #[arg(long)]
    dev_sessions: Option<PathBuf>,
    /// Also listen for HTTPS here (managed devices and the kiosk; contract
    /// §16). Needs --tls-cert-dir.
    #[arg(long, requires = "tls_cert_dir")]
    tls_listen: Option<SocketAddr>,
    /// The box's two certificates: deltos.lan.crt/.key and
    /// deltos-packs.lan.crt/.key (contract §8). If neither certificate is
    /// there yet, deltd serves plain HTTP only and says so; a pair that is
    /// there but broken stops it.
    #[arg(long, requires = "tls_listen")]
    tls_cert_dir: Option<PathBuf>,
    /// Readers kept open per pack (each is one open file and a small cache).
    #[arg(long, default_value_t = 2)]
    readers_per_pack: usize,
    /// How often the packs directory is rescanned for new, updated or
    /// removed packs.
    #[arg(long, default_value_t = 5)]
    rescan_seconds: u64,
    /// Answer phones' "is there internet?" checks (docs/box-network-plan.md
    /// section 6). Set it exactly when the box's DNS sends those hosts here,
    /// i.e. on a hotspot without internet sharing.
    #[arg(long)]
    answer_connectivity_checks: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = Args::parse();
    if !a.webos_host.ends_with(".deltos.lan") {
        return Err("--webos-host must be a name under deltos.lan (contract §8)".into());
    }
    std::fs::create_dir_all(&a.catalog_dir)?;
    let catalog = Catalog::open(&a.catalog_dir.join("deltd-pack-hostnames.sqlite"))?;
    let lib = Arc::new(Library::new(&a.packs, catalog, a.readers_per_pack));
    let r = lib.refresh()?;
    eprintln!("deltd: {} packs, {} damaged", r.added.len(), r.damaged.len());
    let ident: Arc<dyn Identity> = match (&a.identity_socket, &a.dev_sessions) {
        (Some(sock), _) => {
            eprintln!("deltd: people and sign-in from deltos-identityd at {}", sock.display());
            Arc::new(IdentitydLink::new(sock))
        }
        (None, Some(dev)) => {
            eprintln!("deltd: WARNING using development sessions from {}", dev.display());
            Arc::new(StaticIdentity::load(dev)?)
        }
        (None, None) => unreachable!("clap requires one"),
    };
    let tls = match &a.tls_cert_dir {
        Some(d) if !d.join("deltos.lan.crt").exists() && !d.join("deltos-packs.lan.crt").exists() => {
            eprintln!(
                "deltd: HTTPS off: no certificates in {} yet, so teachers and admins cannot sign in (docs/identityd.md §6)",
                d.display()
            );
            None
        }
        Some(d) => Some(deltlib::tls::acceptor(d)?),
        None => None,
    };

    let bg = Arc::clone(&lib);
    let every = Duration::from_secs(a.rescan_seconds.max(1));
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;
            let l = Arc::clone(&bg);
            match tokio::task::spawn_blocking(move || l.refresh()).await {
                Ok(Ok(r)) if r.added.len() + r.updated.len() + r.removed.len() > 0 => eprintln!(
                    "deltd: packs added {:?}, updated {:?}, removed {:?}",
                    r.added, r.updated, r.removed
                ),
                Ok(Err(e)) => eprintln!("deltd: rescan failed: {e}"),
                _ => {}
            }
        }
    });

    let app = router(
        AppState::new(lib, ident, &a.webos_host)
            .with_connectivity_checks(a.answer_connectivity_checks)
            .with_https(tls.is_some())
            .with_ca_cert(a.tls_cert_dir.as_ref().map(|d| d.join("ca.crt"))),
    );
    let listener = tokio::net::TcpListener::bind(a.listen).await?;
    eprintln!("deltd: listening on {} for {}", a.listen, a.webos_host);
    let plain = tokio::spawn(deltlib::tls::serve(listener, app.clone(), None));
    if let (Some(addr), Some(acc)) = (a.tls_listen, tls) {
        let l = tokio::net::TcpListener::bind(addr).await?;
        eprintln!("deltd: listening for HTTPS on {addr}");
        let secure = tokio::spawn(deltlib::tls::serve(l, app, Some(acc)));
        tokio::select! { r = plain => r??, r = secure => r?? }
    } else {
        plain.await??;
    }
    Ok(())
}
