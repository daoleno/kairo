//! `kairo-server`: Kairo for a machine with no screen in front of it. One
//! headless Kairo runs the engine (the same as the desktop app's, on the
//! same folder) and serves its UI, compiled for the browser, behind a
//! password. See `SECURITY-NOTES.md` for the design of the controls.
//!
//! - It listens on 127.0.0.1 unless told otherwise (`--listen`).
//! - The core's controller and ports are the engine's, on 127.0.0.1 as
//!   always; the server never forwards to them, it speaks its own protocol
//!   (`kairo_core::protocol`).
//! - The folder is locked: one Kairo (this, or the desktop app) at a time.
//! - The CLI reaches a running server through a socket in the folder that
//!   only its owner can open (`control`).
//!
//! It is Unix only for now: that socket, the signals and the file modes.

#![cfg(unix)]

pub mod auth;
pub mod control;
pub mod dispatch;
pub mod http;
pub mod link;
pub mod fetched;
pub mod redact;
pub mod serve;
pub mod tls;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kairo_core::Service;
use kairo_core::paths::Paths;

/// How the server runs.
#[derive(Debug, Clone)]
pub struct Options {
    /// Kairo's folder (the desktop app's by default: `KAIRO_HOME`, else the usual one).
    pub home: PathBuf,
    /// Where it listens.
    pub listen: Vec<SocketAddr>,
    /// The page's files (`nsh package --target web`).
    pub web: PathBuf,
    /// Names browsers reach it by, beyond addresses, `localhost` and the machine's name.
    pub hosts: Vec<String>,
    /// The address browsers use when it is behind an HTTPS proxy
    /// (`https://kairo.example.ts.net`): its name is allowed, and cookies are Secure.
    pub public_url: Option<url::Url>,
    /// HTTPS of its own on every listener (`--tls-cert`/`--tls-key`).
    pub tls: Option<Arc<rustls::ServerConfig>>,
    /// Plain HTTP on an address beyond loopback, knowingly (`--insecure-http`).
    pub insecure_http: bool,
}

impl Options {
    pub fn secure(&self) -> bool {
        self.tls.is_some() || self.public_url.as_ref().is_some_and(|u| u.scheme() == "https")
    }

    /// Why a listener on `addr` is not opened, if it is not: plain HTTP
    /// beyond loopback without `--insecure-http`.
    pub fn refused(&self, addr: &SocketAddr) -> Option<String> {
        (self.tls.is_none() && !addr.ip().is_loopback() && !self.insecure_http).then(|| {
            format!(
                "Not listening on {addr}: plain HTTP there would carry the password and the session readable \
                 across the network. Give --tls-cert and --tls-key (HTTPS), or --insecure-http to accept that."
            )
        })
    }

    pub fn web_dir(&self) -> &Path {
        &self.web
    }
}

/// The server's shared state.
pub struct Ctx {
    pub svc: Service,
    pub auth: Arc<auth::Auth>,
    pub opts: Options,
    /// The page's content security policy.
    pub csp: String,
    /// Subscription links pages sent, kept to scrub them from what goes out
    /// even when they never became a subscription (a failed add quotes its
    /// link in the error, to every page).
    pub sent_links: std::sync::Mutex<Vec<String>>,
    /// The provider links of the configs on disk, read again when a file changes.
    providers: std::sync::Mutex<ProviderLinks>,
}

/// The `proxy-providers`/`rule-providers` links of the running config, every
/// subscription and every override, and the files they were read from (by
/// size and time: read again only when one changed).
#[derive(Default)]
struct ProviderLinks {
    stamps: Vec<(PathBuf, Option<std::time::SystemTime>, u64)>,
    links: Vec<String>,
}

fn config_files(paths: &Paths) -> Vec<(PathBuf, Option<std::time::SystemTime>, u64)> {
    let mut files = vec![paths.runtime_config()];
    for dir in [paths.profiles_dir(), paths.overrides_dir()] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut found: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
                .collect();
            found.sort();
            files.extend(found);
        }
    }
    files
        .into_iter()
        .map(|p| {
            let meta = std::fs::metadata(&p).ok();
            let (time, len) = (meta.as_ref().and_then(|m| m.modified().ok()), meta.map(|m| m.len()).unwrap_or(0));
            (p, time, len)
        })
        .collect()
}

impl Ctx {
    pub fn new(svc: Service, auth: Arc<auth::Auth>, opts: Options, csp: String) -> Ctx {
        Ctx { svc, auth, opts, csp, sent_links: Default::default(), providers: Default::default() }
    }

    /// The secrets of the moment: the state's, the links pages sent, and the
    /// provider links of the configs.
    pub fn secrets(&self, state: &kairo_core::State) -> redact::Secrets {
        let mut links = self.sent_links.lock().unwrap_or_else(|e| e.into_inner()).clone();
        links.extend(self.provider_links());
        redact::Secrets::with_links(state, &links)
    }

    fn provider_links(&self) -> Vec<String> {
        let mut cache = self.providers.lock().unwrap_or_else(|e| e.into_inner());
        let stamps = config_files(self.svc.paths());
        if stamps != cache.stamps {
            let mut links = Vec::new();
            for (path, _, _) in &stamps {
                if let Ok(text) = std::fs::read_to_string(path) {
                    links.extend(kairo_core::config::provider_urls(&text));
                }
            }
            links.sort();
            links.dedup();
            *cache = ProviderLinks { stamps, links };
        }
        cache.links.clone()
    }

    /// Remembers a link a page sent (the last few hundred).
    pub fn remember_link(&self, url: &str) {
        let mut sent = self.sent_links.lock().unwrap_or_else(|e| e.into_inner());
        let url = url.trim().to_owned();
        if !url.is_empty() && !sent.contains(&url) {
            if sent.len() >= 256 {
                sent.remove(0);
            }
            sent.push(url);
        }
    }
}

/// The default port.
pub const PORT: u16 = 7979;

/// The machine's name.
pub fn machine_name() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: gethostname writes at most `len` bytes into the buffer.
        if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
            let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            return String::from_utf8_lossy(&buf[..end]).into_owned();
        }
    }
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// Whether one folder is the other or inside it (as they are on disk).
pub fn overlaps(a: &Path, b: &Path) -> bool {
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let (a, b) = (real(a), real(b));
    a.starts_with(&b) || b.starts_with(&a)
}

/// The auth folder of a Kairo folder.
pub fn web_state_dir(home: &Path) -> PathBuf {
    home.join("web")
}

/// Holds the folder's lock for as long as it lives.
pub struct HomeLock {
    #[allow(dead_code)]
    file: std::fs::File,
}

/// Takes the folder's lock: `Err` names who has it.
pub fn lock_home(home: &Path) -> Result<HomeLock, String> {
    std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
    let path = home.join("kairo.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: flock on a file descriptor we own.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(format!(
                "another Kairo uses {} (a running kairo-server, or the desktop app)",
                home.display()
            ));
        }
    }
    use std::io::Write;
    let mut f = &file;
    let _ = f.write_all(format!("{}\n", std::process::id()).as_bytes());
    Ok(HomeLock { file })
}

/// Whether a server (or another holder) has the folder's lock now.
pub fn home_locked(home: &Path) -> bool {
    match lock_home(home) {
        Ok(lock) => {
            drop(lock);
            false
        }
        Err(_) => home.join("kairo.lock").exists(),
    }
}

/// Runs the server until a signal stops it.
pub async fn run(opts: Options, svc: Service) -> anyhow::Result<()> {
    if overlaps(&opts.web, &opts.home) {
        anyhow::bail!(
            "the page's folder {} and Kairo's folder {} overlap: the password and sessions would be served as files",
            opts.web.display(),
            opts.home.display()
        );
    }
    let auth = Arc::new(auth::Auth::open(web_state_dir(&opts.home))?);
    let index = std::fs::read_to_string(opts.web.join("index.html")).map_err(|e| {
        anyhow::anyhow!("the page's files are not in {}: {e}", opts.web.display())
    })?;
    let ctx = Arc::new(Ctx::new(svc, auth.clone(), opts.clone(), http::csp_for(&index)));
    if let Some(code) = auth.setup_code() {
        // Not into the log, nor into a journal (stdout under systemd is one):
        // a terminal sees it; `kairo-server status` shows it to its owner.
        use std::io::IsTerminal;
        log::warn!("no password yet: open the page and set one with the setup code (`kairo-server status` shows it)");
        if std::io::stdout().is_terminal() {
            println!("Setup code: {code}");
        }
    }
    control::spawn(ctx.clone());
    let app = http::router(ctx.clone());
    let mut servers = Vec::new();
    for &addr in &opts.listen {
        if let Some(why) = opts.refused(&addr) {
            log::error!("{why}");
            println!("{why}");
            continue;
        }
        let (app, tls) = (app.clone(), opts.tls.clone());
        let scheme = if tls.is_some() { "https" } else { "http" };
        // One address that cannot be had (not up yet, taken) never stops the
        // server, the other addresses or the core: it is tried again.
        servers.push(tokio::spawn(async move {
            let mut said = false;
            let listener = loop {
                match bind(addr) {
                    Ok(l) => break l,
                    Err(e) => {
                        if !said {
                            log::error!("cannot listen on {addr} ({e}): trying again every 10 s");
                            println!("Cannot listen on {addr} yet ({e}): trying again every 10 s");
                            said = true;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    }
                }
            };
            let local = listener.local_addr().unwrap_or(addr);
            log::warn!("listening on {scheme}://{local}");
            println!("Kairo is at {scheme}://{}", display_addr(local));
            if !addr.ip().is_loopback() && tls.is_none() {
                println!(
                    "Plain HTTP on the network (--insecure-http): the password and the session cross it readable."
                );
            }
            match tls {
                Some(config) => match tls::TlsListener::new(listener, config) {
                    Ok(l) => serve::serve(l, app).await,
                    Err(e) => log::error!("the listener on {addr} stopped: {e}"),
                },
                None => serve::serve(listener, app).await,
            }
        }));
    }
    let stop = async {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signals");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    };
    stop.await;
    log::warn!("stopping");
    auth.flush();
    for s in servers {
        s.abort();
    }
    Ok(())
}

/// A listening socket on `addr`. On Linux an address this machine does not
/// have yet is bound all the same (IP_FREEBIND): it answers once it comes up.
fn bind(addr: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if !addr.ip().is_unspecified() && !addr.ip().is_loopback() {
        let _ = if addr.is_ipv4() { socket.set_freebind_v4(true) } else { socket.set_freebind_v6(true) };
    }
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    tokio::net::TcpListener::from_std(socket.into())
}

/// An address as a browser takes it (`localhost` for 127.0.0.1).
fn display_addr(a: SocketAddr) -> String {
    if a.ip().is_loopback() && a.is_ipv4() {
        format!("localhost:{}", a.port())
    } else {
        a.to_string()
    }
}

/// The Kairo folder of this user (the desktop app's).
pub fn default_home() -> PathBuf {
    Paths::default_home()
}
