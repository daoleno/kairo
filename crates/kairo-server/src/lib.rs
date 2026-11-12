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
    let auth = Arc::new(auth::Auth::open(web_state_dir(&opts.home))?);
    let index = std::fs::read_to_string(opts.web.join("index.html")).map_err(|e| {
        anyhow::anyhow!("the page's files are not in {}: {e}", opts.web.display())
    })?;
    let ctx = Arc::new(Ctx { svc, auth: auth.clone(), opts: opts.clone(), csp: http::csp_for(&index) });
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
