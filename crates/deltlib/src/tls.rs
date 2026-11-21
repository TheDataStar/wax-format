//! `deltd`'s listeners: plain HTTP for visitors and HTTPS for managed
//! devices (contract §16), each connection tagged with how it arrived
//! ([`ConnInfo`]), so the API knows for certain whether a request came over
//! HTTPS and from where. Nothing in a request header is ever believed for
//! this (privilege boundary §3, "where TLS ends").
//!
//! **Certificates** (contract §8: two wildcards, never one): a directory
//! holding `deltos.lan.crt`/`.key` for `*.deltos.lan` and
//! `deltos-packs.lan.crt`/`.key` for `*.deltos-packs.lan`. The pack-zone
//! certificate is chosen only for a pack-zone name, so the certificate that
//! fronts the WebOS never fronts untrusted pack content. Who issues them
//! (the box's internal CA) is Track G's, not `deltd`'s.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use hyper_util::rt::{TokioIo, TokioTimer};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

use crate::hostname::PACK_ZONE;
use crate::signin::ConnInfo;

/// How long a client may take to finish the TLS handshake, and to send a
/// request's headers.
const HANDSHAKE: Duration = Duration::from_secs(10);
const HEADERS: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct ByZone {
    lan: Arc<CertifiedKey>,
    packs: Arc<CertifiedKey>,
}

impl ResolvesServerCert for ByZone {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name().unwrap_or("").to_ascii_lowercase();
        if name.ends_with(&format!(".{PACK_ZONE}")) {
            Some(Arc::clone(&self.packs))
        } else {
            Some(Arc::clone(&self.lan))
        }
    }
}

fn load_pair(dir: &Path, stem: &str) -> Result<Arc<CertifiedKey>, String> {
    let crt = dir.join(format!("{stem}.crt"));
    let key = dir.join(format!("{stem}.key"));
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&crt)
        .map_err(|e| format!("{}: {e}", crt.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", crt.display()))?;
    if chain.is_empty() {
        return Err(format!("{}: no certificate", crt.display()));
    }
    let k = PrivateKeyDer::from_pem_file(&key).map_err(|e| format!("{}: {e}", key.display()))?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&k).map_err(|e| format!("{}: {e}", key.display()))?;
    Ok(Arc::new(CertifiedKey::new(chain, signer)))
}

/// The TLS settings from a certificate directory.
pub fn acceptor(dir: &Path) -> Result<TlsAcceptor, String> {
    let r = ByZone { lan: load_pair(dir, "deltos.lan")?, packs: load_pair(dir, "deltos-packs.lan")? };
    let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(r));
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(cfg)))
}

/// Serve `app` on `listener`, over TLS when `tls` is given, tagging every
/// request with its [`ConnInfo`]. Runs until the listener fails.
pub async fn serve(listener: TcpListener, app: Router, tls: Option<TlsAcceptor>) -> std::io::Result<()> {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                // out of file descriptors and the like: wait, do not spin
                eprintln!("deltd: accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let app = app.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            let info = ConnInfo { https: tls.is_some(), peer: Some(peer.ip()) };
            match tls {
                Some(acc) => {
                    if let Ok(Ok(s)) = tokio::time::timeout(HANDSHAKE, acc.accept(stream)).await {
                        serve_conn(TokioIo::new(s), app, info, peer).await
                    }
                }
                None => serve_conn(TokioIo::new(stream), app, info, peer).await,
            }
        });
    }
}

async fn serve_conn<I>(io: I, app: Router, info: ConnInfo, _peer: SocketAddr)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let svc = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
        req.extensions_mut().insert(info);
        app.clone().oneshot(req)
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADERS)
        .serve_connection(io, svc)
        .await;
}
