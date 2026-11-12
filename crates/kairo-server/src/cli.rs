//! The command line: serve, and the few things to do when locked out.

use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::path::PathBuf;

use kairo_server::{Options, PORT, control, default_home};

const HELP: &str = "\
Kairo for a machine with no screen: the proxy, and its UI as a web page.

Usage:
  kairo-server [serve] [options]   Run Kairo and serve its page
  kairo-server password            Set a new password (signs every browser out)
  kairo-server status [--json]     What runs, where, and the setup code if any
  kairo-server every-app off       Turn Every app (TUN) off
  kairo-server help

Options of serve:
  --listen ADDR      Where to listen (default 127.0.0.1:7979). Repeat for more.
                     An address beyond loopback needs HTTPS (--tls-cert, --tls-key)
                     or --insecure-http; without either it is not opened.
  --tls-cert FILE    HTTPS on every listener with this certificate chain (PEM) ...
  --tls-key FILE     ... and its key (PEM). Read again when they change.
  --insecure-http    Plain HTTP beyond loopback: the password crosses the network readable.
  --host NAME        A name browsers reach it by (beyond addresses, localhost, *.localhost
                     and this machine's name). Repeat for more.
  --public-url URL   Its address behind an HTTPS proxy (https://kairo.example.ts.net):
                     cookies become Secure and the name is allowed.
  --web DIR          The page's files (default: next to this program)

Kairo's folder: KAIRO_HOME, else the desktop app's.";

fn web_default() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    // A package: <prefix>/bin/kairo-server and <prefix>/share/kairo-server/web
    // (not share/kairo: that is Kairo's own folder, with the password in it).
    let packaged = dir.join("../share/kairo-server/web");
    if packaged.join("index.html").is_file() {
        return packaged;
    }
    dir.join("web")
}

fn fail(text: impl std::fmt::Display) -> ! {
    eprintln!("kairo-server: {text}");
    std::process::exit(1)
}

pub fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let home = std::env::var_os("KAIRO_HOME").map(PathBuf::from).unwrap_or_else(default_home);
    match args.first().map(String::as_str) {
        None | Some("serve") | Some("--listen") | Some("--host") | Some("--public-url") | Some("--web")
        | Some("--tls-cert") | Some("--tls-key") | Some("--insecure-http") => {
            let rest = if args.first().map(String::as_str) == Some("serve") { &args[1..] } else { &args[..] };
            serve(home, rest)
        }
        Some("password") => password(home),
        Some("status") => status(home, args.get(1).map(String::as_str) == Some("--json")),
        Some("every-app") if args.get(1).map(String::as_str) == Some("off") => every_app_off(home),
        Some("help") | Some("--help") | Some("-h") => println!("{HELP}"),
        Some(other) => fail(format!("unknown command {other:?}; see kairo-server help")),
    }
}

fn serve(home: PathBuf, args: &[String]) {
    let mut opts = Options {
        home: home.clone(),
        listen: Vec::new(),
        web: web_default(),
        hosts: Vec::new(),
        public_url: None,
        tls: None,
        insecure_http: false,
    };
    let (mut cert, mut key) = (None::<PathBuf>, None::<PathBuf>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().unwrap_or_else(|| fail(format!("{a} needs a value")));
        match a.as_str() {
            "--listen" => {
                let v = value();
                let addr: SocketAddr = v.parse().unwrap_or_else(|_| fail(format!("{v:?} is not an address and port")));
                opts.listen.push(addr);
            }
            "--host" => opts.hosts.push(value().to_ascii_lowercase()),
            "--public-url" => {
                let v = value();
                let u = url::Url::parse(&v).unwrap_or_else(|_| fail(format!("{v:?} is not a URL")));
                if let Some(h) = u.host_str() {
                    opts.hosts.push(h.to_ascii_lowercase());
                }
                opts.public_url = Some(u);
            }
            "--web" => opts.web = PathBuf::from(value()),
            "--tls-cert" => cert = Some(PathBuf::from(value())),
            "--tls-key" => key = Some(PathBuf::from(value())),
            "--insecure-http" => opts.insecure_http = true,
            other => fail(format!("unknown option {other:?}; see kairo-server help")),
        }
    }
    match (cert, key) {
        (Some(c), Some(k)) => {
            opts.tls = Some(kairo_server::tls::config(&c, &k).unwrap_or_else(|e| fail(format!("HTTPS: {e}"))));
        }
        (None, None) => {}
        _ => fail("--tls-cert and --tls-key go together"),
    }
    if opts.listen.is_empty() {
        opts.listen.push(SocketAddr::from(([127, 0, 0, 1], PORT)));
    }
    let lock = kairo_server::lock_home(&home).unwrap_or_else(|e| fail(e));
    // The engine: the same service as the desktop app's, on the same folder.
    let svc = kairo_core::Service::start(kairo_core::paths::Paths::new(home));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap_or_else(|e| fail(e));
    let result = rt.block_on(kairo_server::run(opts, svc.clone()));
    // The core stops and the system proxy is put back, as when the app quits.
    svc.shutdown();
    drop(lock);
    if let Err(e) = result {
        fail(e);
    }
}

fn read_password(prompt: &str) -> String {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    #[cfg(unix)]
    let echo = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        let tty = libc::isatty(0) == 1 && libc::tcgetattr(0, &mut t) == 0;
        if tty {
            let mut quiet = t;
            quiet.c_lflag &= !libc::ECHO;
            libc::tcsetattr(0, libc::TCSANOW, &quiet);
        }
        tty.then_some(t)
    };
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    #[cfg(unix)]
    if let Some(t) = echo {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) };
        eprintln!();
    }
    line.trim_end_matches(['\n', '\r']).to_owned()
}

fn password(home: PathBuf) {
    let auth = kairo_server::auth::Auth::open(kairo_server::web_state_dir(&home)).unwrap_or_else(|e| fail(e));
    let first = read_password("New password: ");
    let again = read_password("Again: ");
    if first != again {
        fail("the passwords differ");
    }
    match auth.reset_password(&first) {
        Ok(()) => println!("Password set. Every browser signs in again."),
        Err(e) => fail(e.message()),
    }
}

fn status(home: PathBuf, as_json: bool) {
    let answer = control::ask(&home, "status");
    if as_json {
        // For scripts: the running server's answer, or what is known without one.
        let s = answer.unwrap_or_else(|| {
            let code = std::fs::read_to_string(kairo_server::web_state_dir(&home).join("setup-code")).ok();
            serde_json::json!({
                "running": false,
                "home": home,
                "setup_code": code.map(|c| c.trim().to_owned()),
            })
        });
        println!("{s}");
        return;
    }
    match answer {
        Some(s) => {
            let line = |k: &str| s.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
            println!("Kairo is running ({})", line("version"));
            for a in s.get("listen").and_then(|v| v.as_array()).into_iter().flatten() {
                let scheme = if s["https"].as_bool() == Some(true) { "https" } else { "http" };
                let refused = s["refused"].as_array().is_some_and(|r| r.contains(a));
                let note = if refused { "  (not opened: plain HTTP beyond loopback)" } else { "" };
                println!("  Page:        {scheme}://{}{note}", a.as_str().unwrap_or_default());
            }
            if let Some(u) = s.get("public_url").and_then(|v| v.as_str()) {
                println!("  Public:      {u}");
            }
            println!("  Folder:      {}", line("home"));
            println!("  Connection:  {}", line("core"));
            println!("  Every app:   {}", if s["every_app"].as_bool() == Some(true) { "on" } else { "off" });
            println!("  System proxy: {}", if s["system_proxy"].as_bool() == Some(true) { "on" } else { "off" });
            match s.get("setup_code").and_then(|v| v.as_str()) {
                Some(code) => println!("  No password yet. Setup code: {code}"),
                None => println!("  Password:    set"),
            }
        }
        None => {
            println!("Kairo is not running (folder {}).", home.display());
            let code = kairo_server::web_state_dir(&home).join("setup-code");
            if let Ok(c) = std::fs::read_to_string(code) {
                println!("  No password yet. Setup code: {}", c.trim());
            }
        }
    }
}

fn every_app_off(home: PathBuf) {
    match control::ask(&home, "every-app off") {
        Some(a) if a.get("ok").is_some() => println!("Every app is off."),
        Some(a) => fail(a.get("error").and_then(|e| e.as_str()).unwrap_or("it did not turn off")),
        None => {
            // Not running: the setting, for the next start.
            let paths = kairo_core::paths::Paths::new(home);
            let mut s = kairo_core::Settings::load(&paths);
            s.tun = false;
            s.save(&paths).unwrap_or_else(|e| fail(e));
            println!("Every app is off (Kairo is not running; it starts with it off).");
        }
    }
}
