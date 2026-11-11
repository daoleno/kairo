//! Kairo's launcher: the window, the tray and the process' lifecycle.
//!
//! NSH's standard runner ends the app when its window closes. A proxy client
//! lives in the tray, so Kairo owns its window: on Linux a `GtkApplicationWindow`
//! hosting NSH's `AppView`, on macOS NSH's `run_app` with `keep_alive_on_close`.
//! Closing the window hides it (with a tray); Quit, Ctrl/Cmd+Q, SIGTERM and
//! SIGINT stop the core and put the system proxy back first.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::sync::Arc;

use kairo_core::Service;
use kairo_core::paths::Paths;

#[cfg(target_os = "linux")]
mod gtk_chrome;
#[cfg(target_os = "macos")]
mod mac_chrome;
#[cfg(target_os = "macos")]
mod mac_theme;

fn root() -> nsh_engine_root::Root {
    #[cfg(target_os = "macos")]
    if nsh_app_kairo::chrome::native() {
        let theme = nsh_app_kairo::accent::apply(&mac_theme::theme_json(nsh_app_kairo::THEME));
        if let Ok(tokens) = nsh_base::tokens_of(Some(&theme)) {
            return nsh_engine_root::Root::of::<nsh_app_kairo::App>().tokens(tokens);
        }
    }
    nsh_engine_root::Root::of::<nsh_app_kairo::App>().tokens(nsh_app_kairo::tokens())
}

#[cfg(target_os = "linux")]
mod nsh_engine_root {
    pub use nsh_platform_linux::Root;
}
#[cfg(target_os = "macos")]
mod nsh_engine_root {
    pub use nsh_platform_macos::Root;
}
#[cfg(target_os = "windows")]
mod nsh_engine_root {
    pub use nsh_platform_windows::Root;
}

/// Restores the system proxy on a panic and on termination signals.
fn emergency_handlers(svc: &Service) {
    let paths: Paths = svc.paths().clone();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        Service::restore_system_proxy_now(&paths);
        hook(info);
    }));
    #[cfg(unix)]
    {
        let svc = svc.clone();
        std::thread::Builder::new()
            .name("kairo-signals".into())
            .spawn(move || {
                let mut signals = signal_hook::iterator::Signals::new([
                    signal_hook::consts::SIGTERM,
                    signal_hook::consts::SIGINT,
                    signal_hook::consts::SIGHUP,
                ])
                .expect("signal handlers");
                if let Some(sig) = signals.forever().next() {
                    log::warn!("signal {sig}: stopping");
                    svc.shutdown();
                    std::process::exit(0);
                }
            })
            .expect("signal thread");
    }
}

/// The guardian: a small process of Kairo's own that outlives it. It holds
/// the read end of a pipe whose write end only Kairo has; when Kairo is gone
/// however it went (a crash, SIGKILL, a segfault in a toolkit), the pipe
/// closes, and the guardian puts the system proxy back and stops the core.
/// After a clean quit there is nothing left to do and it just exits.
#[cfg(unix)]
mod guardian {
    use super::*;
    use std::io::Read;
    use std::process::{Command, Stdio};

    const FLAG: &str = "--guard";

    pub fn is_guardian() -> bool {
        std::env::args().nth(1).as_deref() == Some(FLAG)
    }

    /// Starts the guardian for this process.
    pub fn spawn() {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        match Command::new(exe)
            .arg(FLAG)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // The write end stays open for as long as this process lives
                // (and, close-on-exec, is not inherited by the core).
                std::mem::forget(child.stdin.take());
                std::mem::forget(child);
            }
            Err(e) => log::warn!("no guardian: {e}"),
        }
    }

    pub fn run() -> ! {
        // Its own session: Ctrl+C in a terminal stops Kairo, not the guardian.
        // At logout everything gets SIGTERM (and SIGHUP): the guardian stays
        // until Kairo itself is gone, then does what Kairo could not.
        unsafe {
            libc::setsid();
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            libc::signal(libc::SIGINT, libc::SIG_IGN);
        }
        let mut buf = [0u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
        let paths = Paths::new(Paths::default_home());
        if paths.sysproxy_state().exists() || paths.core_pid().exists() {
            log::warn!(
                "Kairo ended without cleaning up: restoring the system proxy and stopping the core"
            );
            Service::restore_system_proxy_now(&paths);
            if let Ok(rt) = kairo_core::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(kairo_core::supervisor::stop_stale(&paths));
            }
        }
        std::process::exit(0);
    }
}

/// What the guardian does once Kairo is gone: if it ended without cleaning up,
/// puts the system proxy back and stops the core.
#[cfg(windows)]
mod guardian {
    use super::*;
    use std::process::{Command, Stdio};

    const FLAG: &str = "--guard";

    pub fn is_guardian() -> bool {
        std::env::args().nth(1).as_deref() == Some(FLAG)
    }

    /// Starts the guardian for this process: a second copy of Kairo that does
    /// nothing but wait for this one to end.
    pub fn spawn() {
        use std::os::windows::process::CommandExt;
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        let flags = 0x0000_0008 | 0x0000_0200 | 0x0800_0000;
        if let Err(e) = Command::new(exe)
            .arg(FLAG)
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
        {
            log::warn!("no guardian: {e}");
        }
    }

    pub fn run() -> ! {
        let parent = std::env::args()
            .nth(2)
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0);
        if parent != 0 {
            // INFINITE: until Kairo is gone, however it went.
            kairo_core::winapi::wait_for_exit(parent, u32::MAX);
        }
        let paths = Paths::new(Paths::default_home());
        if paths.sysproxy_state().exists() || paths.core_pid().exists() {
            log::warn!(
                "Kairo ended without cleaning up: restoring the system proxy and stopping the core"
            );
            Service::restore_system_proxy_now(&paths);
            if let Ok(rt) = kairo_core::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(kairo_core::supervisor::stop_stale(&paths));
            }
        }
        std::process::exit(0);
    }
}

/// `kairo.exe --tun-selftest [home]`: the TUN permission and the core's start and
/// stop through its task, without a window (a diagnostic, and how the Windows
/// test in a standard account is run). Prints `status`, `paths`, then starts the
/// core, prints READY when its controller answers, waits for the file
/// `<home>\stop-now` (or 90 s), stops it and prints STOPPED or STILL-RUNNING.
#[cfg(windows)]
mod tun_selftest {
    use super::*;
    use kairo_core::{elevated, supervisor::CoreProcess, tun::{self, Privilege}};
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    const CONFIG: &str = r#"mixed-port: 17891
allow-lan: false
mode: rule
log-level: info
find-process-mode: always
external-controller: 127.0.0.1:19091
secret: selftest-secret-0123456789
dns:
  enable: true
  listen: 127.0.0.1:1054
  enhanced-mode: fake-ip
  nameserver: [1.1.1.1, 8.8.8.8]
tun:
  enable: true
  stack: mixed
  auto-route: true
  auto-detect-interface: true
  dns-hijack: ["any:53", "tcp://any:53"]
  route-exclude-address: [10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 20.20.20.0/24]
proxies: []
rules:
  - MATCH,DIRECT
"#;

    const SECRET: &str = "selftest-secret-0123456789";

    /// One call to the controller on the user's port: (status, body).
    fn call(method: &str, path: &str, body: &str, secret: Option<&str>) -> Option<(u16, String)> {
        let mut s = std::net::TcpStream::connect_timeout(
            &"127.0.0.1:19091".parse().ok()?,
            Duration::from_millis(500),
        )
        .ok()?;
        s.set_read_timeout(Some(Duration::from_secs(20))).ok()?;
        let auth = secret.map(|x| format!("Authorization: Bearer {x}\r\n")).unwrap_or_default();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:19091\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).ok()?;
        let mut out = String::new();
        s.read_to_string(&mut out).ok()?;
        let status = out.split_whitespace().nth(1)?.parse().ok()?;
        let body = out.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
        Some((status, body))
    }

    fn controller_version() -> Option<String> {
        let (status, body) = call("GET", "/version", "", Some(SECRET))?;
        (status == 200 && body.contains("version")).then_some(body)
    }

    /// What the host's front of the controller must do, printed one line each.
    fn probes(paths: &Paths, name: &str) {
        let show = |label: &str, r: Option<(u16, String)>| {
            let r = r.map(|(s, b)| format!("{s} {}", b.chars().take(80).collect::<String>())).unwrap_or("no answer".into());
            println!("PROBE {label}: {r}");
        };
        show("GET /proxies (allowed)", call("GET", "/proxies", "", Some(SECRET)));
        show("GET /version without the secret", call("GET", "/version", "", None));
        show("GET /version with a wrong secret", call("GET", "/version", "", Some("not-the-secret-0123456789")));
        show("POST /restart", call("POST", "/restart", "{}", Some(SECRET)));
        show("POST /upgrade", call("POST", "/upgrade", "{}", Some(SECRET)));
        show("GET /storage/x", call("GET", "/storage/x", "", Some(SECRET)));
        show("PATCH /configs tcptun-config", call("PATCH", "/configs", r#"{"tcptun-config":"127.0.0.1:1,127.0.0.1:2"}"#, Some(SECRET)));
        show("PATCH /configs mode (allowed)", call("PATCH", "/configs", r#"{"mode":"rule"}"#, Some(SECRET)));
        // What Kairo does to change the config: write it to the inbox, ask to reload.
        // The path in the call is not read.
        if let Ok(text) = std::fs::read(paths.runtime_config()) {
            let _ = elevated::deliver_config(name, &text);
        }
        show(
            "PUT /configs path=C:\\evil.yaml (reloads the inbox)",
            call("PUT", "/configs?force=true", r#"{"path":"C:\\Windows\\win.ini","payload":"external-ui: C:/x"}"#, Some(SECRET)),
        );
        show("GET /version after the reload", call("GET", "/version", "", Some(SECRET)));
    }

    pub fn run(args: &[String]) -> i32 {
        let home = args
            .first()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(Paths::default_home);
        let paths = Paths::new(&home);
        println!("user {:?}", elevated::current_user());
        println!("config={} log={:?}", paths.runtime_config().display(), elevated::current().and_then(|(_, n)| elevated::log_path(&n)));
        let status = tun::status(&paths.core_bin());
        println!("status {status:?}");
        if status != Privilege::Granted {
            return 3;
        }
        let _ = paths.ensure();
        let _ = std::fs::create_dir_all(paths.runtime_dir());
        // `<home>\selftest.yaml`, when there is one, is the config to run.
        let config = std::fs::read_to_string(home.join("selftest.yaml")).unwrap_or_else(|_| CONFIG.to_owned());
        if let Err(e) = std::fs::write(paths.runtime_config(), config) {
            println!("cannot write the config: {e}");
            return 4;
        }
        let Ok(rt) = kairo_core::tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return 5;
        };
        rt.block_on(async {
            let mut p = match CoreProcess::spawn_elevated(&paths).await {
                Ok(p) => p,
                Err(e) => {
                    println!("spawn failed: {e}");
                    return 6;
                }
            };
            println!("started; running={}", p.running());
            let t = Instant::now();
            let mut version = None;
            while t.elapsed() < Duration::from_secs(20) {
                version = controller_version();
                if version.is_some() {
                    break;
                }
                kairo_core::tokio::time::sleep(Duration::from_millis(300)).await;
            }
            println!("READY {version:?} after {:?}", t.elapsed());
            if let Some((_, name)) = elevated::current() {
                probes(&paths, &name);
            }
            let stop_file = home.join("stop-now");
            let _ = std::fs::remove_file(&stop_file);
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(90) && !stop_file.exists() {
                kairo_core::tokio::time::sleep(Duration::from_millis(500)).await;
            }
            println!("core_alive before stop: {}", kairo_core::supervisor::core_alive(&paths));
            p.stop(&paths).await;
            println!("{}", if p.running() { "STILL-RUNNING" } else { "STOPPED" });
            println!("log tail: {:?}", p.tail().into_iter().rev().take(3).collect::<Vec<_>>());
            0
        })
    }
}

/// A windowed program has no console: its log goes to `logs/kairo.log`.
#[cfg(windows)]
fn init_logging() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"));
    let dir = Paths::default_home().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("kairo.log");
    // The guardian and the next Kairo append; a log of a megabyte starts over.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        builder.target(env_logger::Target::Pipe(Box::new(file)));
    }
    builder.init();
}

#[cfg(not(windows))]
fn init_logging() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
}

fn main() {
    init_logging();
    #[cfg(any(unix, windows))]
    if guardian::is_guardian() {
        guardian::run();
    }
    // Elevated copy of Kairo (UAC): the one-time TUN permission, then it exits
    // (kairo_core::elevated).
    #[cfg(windows)]
    {
        let a: Vec<String> = std::env::args().collect();
        match a.get(1).map(String::as_str) {
            Some("--grant-tun") => std::process::exit(kairo_core::elevated::run_grant(&a[2..])),
            Some("--revoke-tun") => std::process::exit(kairo_core::elevated::run_revoke(&a[2..])),
            Some("--tun-selftest") => std::process::exit(tun_selftest::run(&a[2..])),
            // The app's own calls for the permission and for taking it away (the
            // UAC path), for the acceptance run; the product's switch stays shut.
            Some("--tun-grant-selftest") | Some("--tun-revoke-selftest") => {
                let grant = a[1] == "--tun-grant-selftest";
                let r = if grant { kairo_core::elevated::grant(&Paths::new(Paths::default_home())) } else { kairo_core::elevated::revoke() };
                println!("{} {:?}", if grant { "GRANT" } else { "REVOKE" }, r);
                println!("status {:?}", kairo_core::tun::status(&Paths::new(Paths::default_home()).core_bin()));
                std::process::exit(if r.is_ok() { 0 } else { 1 });
            }
            _ => {}
        }
    }
    // At login (see kairo_core::autostart::install_restore): put the system
    // proxy back if a Kairo that is gone left it pointing at a dead port.
    if std::env::args().nth(1).as_deref() == Some("--restore-network") {
        let paths = Paths::new(Paths::default_home());
        kairo_core::sysproxy::restore_if_dead(&paths);
        std::process::exit(0);
    }
    // Started by "Restart to Update": the Kairo it replaces must be gone first
    // (single instance, the core's ports).
    kairo_core::update::wait_for_previous();
    let minimized = std::env::args().any(|a| a == "--minimized");
    // `--page <name>` opens on that page (screenshots, docs).
    let args: Vec<String> = std::env::args().collect();
    if let Some(page) = args
        .iter()
        .position(|a| a == "--page")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| nsh_app_kairo::store::Page::named(n))
    {
        nsh_app_kairo::chrome::select(page);
    }
    platform::run(minimized);
}

/// Starts the engine of the one running Kairo: the guardian, the service
/// (which recovers from a crash and starts the core) and the handlers.
fn start_primary() -> Service {
    #[cfg(any(unix, windows))]
    guardian::spawn();
    let svc = Service::global();
    emergency_handlers(&svc);
    svc
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use gtk::prelude::*;
    use gtk::{gio, glib};
    use nsh_platform_linux::gtk4 as gtk;
    use nsh_platform_linux::{AppOptions, AppView, Appearance};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    const APP_ID: &str = "dev.kairo.app";

    fn application() -> Option<gtk::Application> {
        gio::Application::default().and_then(|a| a.downcast::<gtk::Application>().ok())
    }

    fn present() {
        if let Some(app) = application() {
            app.activate();
        }
    }

    pub fn run(minimized: bool) {
        glib::set_prgname(Some(APP_ID));
        glib::set_application_name("Kairo");
        // One Kairo per user: a second launch shows the first one's window and
        // leaves (before it touches the core or the system proxy).
        let native = std::env::var("KAIRO_SPIKE").as_deref() != Ok("nsh");
        nsh_app_kairo::chrome::set_native(native);
        let app: gtk::Application = libadwaita::Application::builder()
            .application_id(APP_ID)
            .build()
            .upcast();
        if let Err(e) = app.register(None::<&gio::Cancellable>) {
            log::warn!("no session bus registration ({e}); running on our own");
        }
        if app.is_remote() {
            // Forwards the activation to the first Kairo and returns.
            app.run_with_args::<&str>(&[]);
            return;
        }
        let svc = super::start_primary();
        let tray_ok = Rc::new(Cell::new(false));
        let view_slot: Rc<RefCell<Option<Rc<AppView>>>> = Rc::default();
        let first = Rc::new(Cell::new(true));

        let quit_svc = svc.clone();
        let quit = Arc::new(move || {
            let svc = quit_svc.clone();
            glib::MainContext::default().invoke(move || {
                svc.shutdown();
                if let Some(app) = application() {
                    app.quit();
                }
            });
        });
        let quit_action = gio::SimpleAction::new("quit", None);
        {
            let quit = quit.clone();
            quit_action.connect_activate(move |_, _| quit());
        }
        app.add_action(&quit_action);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        svc.set_quit_hook(quit.clone());

        app.connect_activate({
            let svc = svc.clone();
            let tray_ok = tray_ok.clone();
            let quit = quit.clone();
            move |app| {
                if let Some(window) = app.windows().first() {
                    window.set_visible(true);
                    window.present();
                    return;
                }
                // KAIRO_WINDOW=WxH: a window of another size (screenshots of long pages).
                let (win_w, win_h) = std::env::var("KAIRO_WINDOW")
                    .ok()
                    .and_then(|v| {
                        let (w, h) = v.split_once('x')?;
                        Some((w.parse::<i32>().ok()?, h.parse::<i32>().ok()?))
                    })
                    .unwrap_or((1180, 760));
                let window: gtk::ApplicationWindow = if native {
                    libadwaita::ApplicationWindow::builder()
                        .application(app)
                        .title("Kairo")
                        .icon_name(APP_ID)
                        .default_width(win_w)
                        .default_height(win_h)
                        .build()
                        .upcast()
                } else {
                    gtk::ApplicationWindow::builder()
                        .application(app)
                        .title("Kairo")
                        .icon_name(APP_ID)
                        .default_width(win_w)
                        .default_height(win_h)
                        .build()
                };
                window.set_size_request(860, 560);
                let view = AppView::new(AppOptions {
                    app_id: APP_ID.into(),
                    title: "Kairo".into(),
                    size: (1180, 760),
                    appearance: Appearance::System,
                    ..AppOptions::default()
                });
                if let Some(settings) = gtk::Settings::default() {
                    nsh_app_kairo::ui::size::set_scale(crate::gtk_chrome::font_scale(&settings));
                }
                if let Some(adw_window) = window.downcast_ref::<libadwaita::ApplicationWindow>() {
                    crate::gtk_chrome::build(adw_window, view.clone(), svc.clone());
                } else {
                    window.set_child(Some(&view.widget()));
                }
                view.load(nsh_platform_linux::App::Rust(super::root()));
                *view_slot.borrow_mut() = Some(view);

                // Ctrl+W closes (hides) the window, as in every GTK app.
                let close = gtk::ShortcutController::new();
                close.set_propagation_phase(gtk::PropagationPhase::Bubble);
                close.add_shortcut(gtk::Shortcut::new(
                    gtk::ShortcutTrigger::parse_string("<Control>w"),
                    Some(gtk::NamedAction::new("window.close")),
                ));
                window.add_controller(close);

                let svc_close = svc.clone();
                let tray_close = tray_ok.clone();
                let quit_close = quit.clone();
                window.connect_close_request(move |w| {
                    if tray_close.get() && svc_close.settings().close_to_tray {
                        w.set_visible(false);
                    } else {
                        quit_close();
                    }
                    glib::Propagation::Stop
                });

                // The tray, once (it outlives the window).
                if first.replace(false) {
                    let hooks = kairo_tray::Hooks {
                        show: Arc::new(|| {
                            glib::MainContext::default().invoke(present);
                        }),
                        page: Arc::new(|name| {
                            glib::MainContext::default().invoke(move || {
                                if let Some(page) = nsh_app_kairo::store::Page::named(name) {
                                    nsh_app_kairo::chrome::select(page);
                                }
                                present();
                            });
                        }),
                        quit: quit.clone(),
                    };
                    match kairo_tray::start(svc.clone(), hooks) {
                        Ok(()) => tray_ok.set(true),
                        Err(e) => log::warn!("no tray: {e}; closing the window quits"),
                    }
                    let hide = (minimized || svc.settings().start_minimized) && tray_ok.get();
                    if hide {
                        return;
                    }
                }
                window.present();
            }
        });
        app.run_with_args::<&str>(&[]);
        svc.shutdown();
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use nsh_platform_macos::{AppOptions, AppSource, Appearance, Components};
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    fn present() {
        dispatch2::DispatchQueue::main().exec_async(|| {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let app = NSApplication::sharedApplication(mtm);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            for window in app.windows().iter() {
                window.makeKeyAndOrderFront(None);
            }
        });
    }

    pub fn run(minimized: bool) {
        let svc = super::start_primary();
        let quit_svc = svc.clone();
        let quit = Arc::new(move || {
            let svc = quit_svc.clone();
            // AppKit wants the main thread; the tray and the updater call from others.
            dispatch2::DispatchQueue::main().exec_async(move || {
                svc.shutdown();
                nsh_platform_macos::quit_app();
            });
        });
        svc.set_quit_hook(quit.clone());
        let hooks = kairo_tray::Hooks {
            show: Arc::new(present),
            page: Arc::new(|name| {
                if let Some(page) = nsh_app_kairo::store::Page::named(name) {
                    nsh_app_kairo::chrome::select(page);
                }
                present();
            }),
            quit,
        };
        let tray_ok = kairo_tray::start(svc.clone(), hooks).is_ok();
        let settings = svc.settings();
        if (minimized || settings.start_minimized) && tray_ok {
            dispatch2::DispatchQueue::main().exec_async(|| {
                let Some(mtm) = MainThreadMarker::new() else {
                    return;
                };
                for window in NSApplication::sharedApplication(mtm).windows().iter() {
                    window.orderOut(None);
                }
            });
        }
        let spike = std::env::var("KAIRO_SPIKE").unwrap_or_default();
        let native = spike != "nsh";
        nsh_app_kairo::chrome::set_native(native);
        nsh_app_kairo::chrome::set_glass_sidebar(!native);
        nsh_app_kairo::chrome::set_native_controls(spike != "nshcontrols" && spike != "nsh");
        let components = Components::default();
        crate::mac_chrome::controls::register(&components);
        if native {
            crate::mac_chrome::install(svc.clone());
        }
        nsh_platform_macos::run_app(
            AppSource::Rust(super::root()),
            AppOptions {
                components,
                title: "Kairo".to_owned(),
                size: (1180, 760),
                appearance: Appearance::System,
                keep_alive_on_close: tray_ok && settings.close_to_tray,
                ..Default::default()
            },
        );
        svc.shutdown();
    }
}

#[cfg(target_os = "windows")]
mod platform {
    //! The window is Kairo's own, not NSH's runner's: closing it hides it (with
    //! a tray icon), a second launch shows the first one's window, and logging
    //! off puts the system proxy back first.

    use super::*;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

    use nsh_platform_windows::{App, AppOptions, AppView, Appearance};
    use windows::Win32::Foundation::{
        ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM,
    };
    use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext,
    };
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, CallWindowProcW, DispatchMessageW, FindWindowW, GWLP_WNDPROC,
        GetMessageW, GetWindowRect, GetWindowThreadProcessId, IsIconic, MSG, PostMessageW,
        SPI_GETWORKAREA, SW_HIDE, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
        SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, SystemParametersInfoW,
        TranslateMessage, WM_APP, WM_CLOSE, WM_ENDSESSION,
    };
    use windows::core::{HSTRING, w};

    const APP_ID: &str = "dev.kairo.app";
    /// Posted to the window to show it (by the tray, or by a second launch).
    const WM_SHOW: u32 = WM_APP + 10;

    static SERVICE: OnceLock<Service> = OnceLock::new();
    static TRAY_OK: AtomicBool = AtomicBool::new(false);
    static QUITTING: AtomicBool = AtomicBool::new(false);
    static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);
    static ORIGINAL_PROC: AtomicIsize = AtomicIsize::new(0);

    fn window() -> HWND {
        HWND(MAIN_WINDOW.load(Ordering::SeqCst) as *mut std::ffi::c_void)
    }

    /// One Kairo per user (and per `KAIRO_HOME`, so isolated test runs can
    /// run beside it). `false` when another one is running.
    fn first_instance() -> bool {
        let name = match std::env::var("KAIRO_HOME") {
            Ok(h) if !h.is_empty() => {
                let sum: u32 = h
                    .bytes()
                    .fold(5381u32, |a, b| a.wrapping_mul(33) ^ u32::from(b));
                format!("Local\\{APP_ID}.{sum:08x}")
            }
            _ => format!("Local\\{APP_ID}"),
        };
        // SAFETY: a named mutex kept for the life of the process.
        unsafe {
            let handle = CreateMutexW(None, true, &HSTRING::from(name));
            let exists = GetLastError() == ERROR_ALREADY_EXISTS;
            // Never closed: it is what the next launch finds.
            std::mem::forget(handle);
            !exists
        }
    }

    /// A second launch: asks the first one to show its window.
    fn show_the_first() {
        // SAFETY: plain calls on another process's window.
        unsafe {
            let Ok(hwnd) = FindWindowW(None, w!("Kairo")) else {
                return;
            };
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let _ = AllowSetForegroundWindow(pid);
            let _ = PostMessageW(Some(hwnd), WM_SHOW, WPARAM(0), LPARAM(0));
        }
    }

    fn show_window(hwnd: HWND) {
        // SAFETY: plain calls on this process's window.
        unsafe {
            let _ = ShowWindow(
                hwnd,
                if IsIconic(hwnd).as_bool() {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            let _ = SetForegroundWindow(hwnd);
        }
    }

    /// Our window procedure, in front of NSH's: closing hides to the tray,
    /// the show message brings the window back, logging off cleans up.
    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_SHOW => {
                show_window(hwnd);
                return LRESULT(0);
            }
            WM_CLOSE if !QUITTING.load(Ordering::SeqCst) => {
                let to_tray = TRAY_OK.load(Ordering::SeqCst)
                    && SERVICE.get().is_some_and(|s| s.settings().close_to_tray);
                if to_tray {
                    // SAFETY: a plain call on this window.
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_HIDE);
                    }
                    return LRESULT(0);
                }
                // Closing the window is quitting: say so before the engine goes.
                if let Some(svc) = SERVICE.get() {
                    svc.shutdown();
                }
                QUITTING.store(true, Ordering::SeqCst);
            }
            WM_ENDSESSION if wparam.0 != 0 => {
                // Logging off or shutting down: the system proxy goes back now,
                // the process is ended soon after.
                if let Some(svc) = SERVICE.get() {
                    svc.shutdown();
                }
            }
            _ => {}
        }
        // SAFETY: NSH's procedure, which this one replaced.
        unsafe {
            let original: windows::Win32::UI::WindowsAndMessaging::WNDPROC =
                std::mem::transmute(ORIGINAL_PROC.load(Ordering::SeqCst));
            CallWindowProcW(original, hwnd, msg, wparam, lparam)
        }
    }

    fn quit_app(svc: &Service) {
        svc.shutdown();
        QUITTING.store(true, Ordering::SeqCst);
        // SAFETY: posting to the window of the UI thread.
        unsafe {
            let _ = PostMessageW(Some(window()), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// The first window size in points: 1180x760, or less on a small screen so
    /// the window fits above the taskbar.
    fn window_size() -> (i32, i32) {
        let mut work = RECT::default();
        // SAFETY: fills a RECT; the process is DPI aware by now, so it is in pixels.
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut work as *mut _ as *mut core::ffi::c_void),
                Default::default(),
            )
        }
        .is_ok();
        if !ok {
            return (1180, 760);
        }
        // SAFETY: no arguments.
        let scale = f64::from(unsafe { GetDpiForSystem() }.max(96)) / 96.0;
        let fit =
            |pixels: i32, wanted: i32| ((f64::from(pixels) / scale) as i32 - 24).clamp(640, wanted);
        (
            fit(work.right - work.left, 1180),
            fit(work.bottom - work.top, 760),
        )
    }

    /// Puts the window inside the screen's work area (above the taskbar),
    /// smaller if it has to be: on a small screen the bottom of the sidebar,
    /// where the update button is, would otherwise be under the taskbar.
    fn fit_to_work_area(hwnd: HWND) {
        let mut work = RECT::default();
        let mut rect = RECT::default();
        // SAFETY: fill RECTs; the window is ours.
        unsafe {
            if SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut work as *mut _ as *mut core::ffi::c_void),
                Default::default(),
            )
            .is_err()
                || GetWindowRect(hwnd, &mut rect).is_err()
            {
                return;
            }
            let w = (rect.right - rect.left).min(work.right - work.left);
            let h = (rect.bottom - rect.top).min(work.bottom - work.top);
            let x = rect.left.clamp(work.left, work.right - w);
            let y = rect.top.clamp(work.top, work.bottom - h);
            if (x, y, w, h)
                != (
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                )
            {
                let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }

    /// The UI thread: the window, the tray and the message loop.
    fn ui(svc: Service, minimized: bool) {
        // SAFETY: process setup before any window exists (what NSH's runner does).
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let _ = OleInitialize(None);
            let _ = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID));
        }
        nsh_app_kairo::chrome::set_native(false);
        let view = match AppView::new(
            None,
            AppOptions {
                app_id: APP_ID.into(),
                title: "Kairo".into(),
                size: window_size(),
                appearance: Appearance::System,
                limits: nsh_platform_windows::Limits::default(),
                ..AppOptions::default()
            },
        ) {
            Ok(v) => v,
            Err(e) => {
                nsh_platform_windows::fatal_error(&format!("Cannot open a window: {e}"));
                return;
            }
        };
        view.load(App::Rust(super::root()));
        let hwnd = view.hwnd();
        fit_to_work_area(hwnd);
        MAIN_WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);
        // SAFETY: replaces the window's procedure with one that forwards to it.
        unsafe {
            let original = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc as *const () as isize);
            ORIGINAL_PROC.store(original, Ordering::SeqCst);
        }

        let quit_svc = svc.clone();
        let quit: Arc<dyn Fn() + Send + Sync> = Arc::new(move || quit_app(&quit_svc));
        svc.set_quit_hook(quit.clone());
        let hooks = kairo_tray::Hooks {
            show: Arc::new(|| {
                // SAFETY: posting to the window of the UI thread.
                unsafe {
                    let _ = PostMessageW(Some(window()), WM_SHOW, WPARAM(0), LPARAM(0));
                }
            }),
            page: Arc::new(|name| {
                if let Some(page) = nsh_app_kairo::store::Page::named(name) {
                    nsh_app_kairo::chrome::select(page);
                }
                // SAFETY: as above.
                unsafe {
                    let _ = PostMessageW(Some(window()), WM_SHOW, WPARAM(0), LPARAM(0));
                }
            }),
            quit,
        };
        match kairo_tray::start(svc.clone(), hooks) {
            Ok(()) => TRAY_OK.store(true, Ordering::SeqCst),
            Err(e) => log::warn!("no tray: {e}; closing the window quits"),
        }
        let hide = (minimized || svc.settings().start_minimized) && TRAY_OK.load(Ordering::SeqCst);
        if !hide {
            view.show();
        }
        let mut msg = MSG::default();
        // SAFETY: the standard message loop.
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        view.stop();
        drop(view);
        // SAFETY: pairs the OleInitialize above.
        unsafe { OleUninitialize() };
    }

    pub fn run(minimized: bool) {
        // Leftovers of the update that brought this Kairo here.
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            kairo_core::update::remove_leftovers(dir);
        }
        if !first_instance() {
            show_the_first();
            return;
        }
        let svc = super::start_primary();
        let _ = SERVICE.set(svc.clone());
        // Deep view trees recurse in drawing: NSH's runner gives its thread 8 MB too.
        let thread_svc = svc.clone();
        let ui = std::thread::Builder::new()
            .name("nsh-ui".into())
            .stack_size(8 << 20)
            .spawn(move || ui(thread_svc, minimized));
        match ui {
            Ok(handle) => {
                let _ = handle.join();
            }
            Err(e) => {
                nsh_platform_windows::fatal_error(&format!("Cannot start the UI thread: {e}"))
            }
        }
        kairo_tray::stop();
        svc.shutdown();
    }
}
