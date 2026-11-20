//! `deltd` — serves every installed pack and the Library API.
//!
//! There is no default WebOS hostname (it is still open, contract §13.2) and
//! no anonymous mode: an identity source is required.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use deltlib::{router, AppState, Catalog, Library, StaticIdentity};

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
    /// The WebOS hostname under deltos.lan. Required: the name is still open
    /// (contract §13.2), so deltd does not pick one.
    #[arg(long)]
    webos_host: String,
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:80")]
    listen: SocketAddr,
    /// DEVELOPMENT ONLY: a fixed table of sessions and pack partitions, until
    /// deltos-identityd exists.
    #[arg(long)]
    dev_sessions: PathBuf,
    /// Readers kept open per pack (each is one open file and a small cache).
    #[arg(long, default_value_t = 2)]
    readers_per_pack: usize,
    /// How often the packs directory is rescanned for new, updated or
    /// removed packs.
    #[arg(long, default_value_t = 5)]
    rescan_seconds: u64,
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
    let ident = Arc::new(StaticIdentity::load(&a.dev_sessions)?);
    eprintln!("deltd: WARNING using development sessions from {}", a.dev_sessions.display());

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

    let app = router(AppState::new(lib, ident, &a.webos_host));
    let listener = tokio::net::TcpListener::bind(a.listen).await?;
    eprintln!("deltd: listening on {} for {}", a.listen, a.webos_host);
    axum::serve(listener, app).await?;
    Ok(())
}
