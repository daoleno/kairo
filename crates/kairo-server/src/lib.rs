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
pub mod redact;

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
}

impl Options {
    pub fn secure(&self) -> bool {
        self.public_url.as_ref().is_some_and(|u| u.scheme() == "https")
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
}

impl Ctx {
    pub fn new(svc: Service, auth: Arc<auth::Auth>, opts: Options, csp: String) -> Ctx {
        Ctx { svc, auth, opts, csp, sent_links: Default::default() }
    }

    /// The secrets of the moment: the state's, and the links pages sent.
    pub fn secrets(&self, state: &kairo_core::State) -> redact::Secrets {
        let sent = self.sent_links.lock().unwrap_or_else(|e| e.into_inner());
        redact::Secrets::with_links(state, &sent)
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
        log::warn!("no password yet: open the page and set one with the setup code {code}");
        println!("Setup code: {code}");
    }
    control::spawn(ctx.clone());
    let app = http::router(ctx.clone()).into_make_service_with_connect_info::<SocketAddr>();
    let mut servers = Vec::new();
    for addr in &opts.listen {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("cannot listen on {addr}: {e}"))?;
        log::warn!("listening on http://{}", listener.local_addr()?);
        println!("Kairo is at http://{}", display_addr(listener.local_addr()?));
        if !addr.ip().is_loopback() && !opts.secure() {
            println!(
                "Plain HTTP on the network: the password and the session cross it readable. \
                 Reach it through HTTPS (tailscale serve, or a proxy with --public-url) or an SSH tunnel."
            );
        }
        let app = app.clone();
        servers.push(tokio::spawn(async move { axum::serve(listener, app).await }));
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
