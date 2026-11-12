//! HTTPS of the server's own (`--tls-cert`/`--tls-key`): a certificate the
//! owner has (tailscale cert, Let's Encrypt, the network's own CA), never
//! one the server makes up. The files are read again when they change, so a
//! renewed certificate needs no restart.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// The certificate chain and key in `cert` and `key` (PEM).
fn load(cert: &Path, key: &Path) -> Result<Arc<CertifiedKey>, String> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .map_err(|e| format!("{}: {e}", cert.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", cert.display()))?;
    if chain.is_empty() {
        return Err(format!("{}: no certificate in it", cert.display()));
    }
    let key = PrivateKeyDer::from_pem_file(key).map_err(|e| format!("{}: {e}", key.display()))?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&key).map_err(|e| format!("the key: {e}"))?;
    let certified = CertifiedKey::new(chain, signer);
    certified.keys_match().map_err(|e| format!("the key is not the certificate's: {e}"))?;
    Ok(Arc::new(certified))
}

fn stamp(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The certificate, read again (at most once a minute) when its files change.
#[derive(Debug)]
struct Reloading {
    cert: PathBuf,
    key: PathBuf,
    inner: Mutex<Loaded>,
}

/// The certificate in use, its files' times when it was read, and the last look at them.
#[derive(Debug)]
struct Loaded {
    key: Arc<CertifiedKey>,
    stamps: (Option<SystemTime>, Option<SystemTime>),
    looked: Instant,
}

impl ResolvesServerCert for Reloading {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current())
    }
}

impl Reloading {
    fn new(cert: &Path, key: &Path) -> Result<Reloading, String> {
        let first = load(cert, key)?;
        Ok(Reloading {
            cert: cert.to_path_buf(),
            key: key.to_path_buf(),
            inner: Mutex::new(Loaded { key: first, stamps: (stamp(cert), stamp(key)), looked: Instant::now() }),
        })
    }

    fn current(&self) -> Arc<CertifiedKey> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.looked.elapsed() > Duration::from_secs(60) {
            inner.looked = Instant::now();
            let stamps = (stamp(&self.cert), stamp(&self.key));
            if stamps != inner.stamps {
                match load(&self.cert, &self.key) {
                    Ok(fresh) => {
                        log::warn!("the TLS certificate changed: using the new one");
                        inner.key = fresh;
                        inner.stamps = stamps;
                    }
                    // Half written (a renewal in progress): the old one until it reads.
                    Err(e) => log::warn!("the TLS certificate changed but does not read yet: {e}"),
                }
            }
        }
        inner.key.clone()
    }
}

/// The TLS setup of `cert` and `key`; an error says what is wrong with them.
pub fn config(cert: &Path, key: &Path) -> Result<Arc<rustls::ServerConfig>, String> {
    let resolver = Reloading::new(cert, key)?;
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// Handshakes at once, at most; each gets this long.
const HANDSHAKES: usize = 64;
const HANDSHAKE_TIME: Duration = Duration::from_secs(10);

/// A TLS listener for `axum::serve`: handshakes run beside the accepting, so
/// a slow or silent client holds up no one else.
pub struct TlsListener {
    ready: tokio::sync::mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
}

impl TlsListener {
    pub fn new(tcp: TcpListener, config: Arc<rustls::ServerConfig>) -> std::io::Result<TlsListener> {
        let local = tcp.local_addr()?;
        let (tx, ready) = tokio::sync::mpsc::channel(HANDSHAKES);
        let acceptor = TlsAcceptor::from(config);
        let slots = Arc::new(tokio::sync::Semaphore::new(HANDSHAKES));
        tokio::spawn(async move {
            loop {
                let (stream, peer) = match tcp.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        log::debug!("accept: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let Ok(slot) = slots.clone().acquire_owned().await else { return };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    match tokio::time::timeout(HANDSHAKE_TIME, acceptor.accept(stream)).await {
                        Ok(Ok(tls)) => {
                            let _ = tx.send((tls, peer)).await;
                        }
                        Ok(Err(e)) => log::debug!("TLS handshake from {peer}: {e}"),
                        Err(_) => log::debug!("TLS handshake from {peer}: timed out"),
                    }
                });
            }
        });
        Ok(TlsListener { ready, local })
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(conn) => conn,
            // The accepting task ended (never, short of a panic): wait for ever.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        Ok(self.local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_renewed_certificate_is_used_without_a_restart_and_a_half_written_one_is_not() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
        std::fs::copy(fixtures.join("test-cert.pem"), &cert).unwrap();
        std::fs::copy(fixtures.join("test-key.pem"), &key).unwrap();
        let resolver = Reloading::new(&cert, &key).unwrap();
        let first = resolver.current().cert[0].clone();
        let age = |r: &Reloading| r.inner.lock().unwrap().looked -= Duration::from_secs(61);

        // Half written: the old one stays.
        std::fs::write(&cert, "-----BEGIN CERTIFICATE-----\nMIIB").unwrap();
        age(&resolver);
        assert_eq!(resolver.current().cert[0], first);

        // Renewed: a new pair, used once a minute has passed since the last look.
        let made = std::process::Command::new("openssl")
            .args(["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes"])
            .args(["-days", "1", "-subj", "/CN=localhost", "-keyout"])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .output()
            .unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        assert_eq!(resolver.current().cert[0], first, "not looked at again within a minute");
        age(&resolver);
        let renewed = resolver.current().cert[0].clone();
        assert_ne!(renewed, first);
        assert_eq!(renewed, CertificateDer::pem_file_iter(&cert).unwrap().next().unwrap().unwrap());
    }
}
