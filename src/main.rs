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

#[cfg(target_os = "macos")]
mod mac_chrome;

fn root() -> nsh_engine_root::Root {
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
        let Ok(exe) = std::env::current_exe() else { return };
        match Command::new(exe).arg(FLAG).stdin(Stdio::piped()).stdout(Stdio::null()).spawn() {
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
        unsafe {
            libc::setsid();
        }
        let mut buf = [0u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
        let paths = Paths::new(Paths::default_home());
        if paths.sysproxy_state().exists() || paths.core_pid().exists() {
            log::warn!("Kairo ended without cleaning up: restoring the system proxy and stopping the core");
            Service::restore_system_proxy_now(&paths);
            if let Ok(rt) = kairo_core::tokio::runtime::Builder::new_current_thread().enable_all().build() {
                rt.block_on(kairo_core::supervisor::stop_stale(&paths));
            }
        }
        std::process::exit(0);
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    #[cfg(unix)]
    if guardian::is_guardian() {
        guardian::run();
    }
    // Started by "Restart to Update": the Kairo it replaces must be gone first
    // (single instance, the core's ports).
    kairo_core::update::wait_for_previous();
    let minimized = std::env::args().any(|a| a == "--minimized");
    platform::run(minimized);
}

/// Starts the engine of the one running Kairo: the guardian, the service
/// (which recovers from a crash and starts the core) and the handlers.
fn start_primary() -> Service {
    #[cfg(unix)]
    guardian::spawn();
    let svc = Service::global();
    emergency_handlers(&svc);
    svc
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use nsh_platform_linux::gtk4 as gtk;
    use nsh_platform_linux::{AppOptions, AppView, Appearance};
    use gtk::prelude::*;
    use gtk::{gio, glib};
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
        let app = gtk::Application::builder().application_id(APP_ID).build();
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
                let window = gtk::ApplicationWindow::builder()
                    .application(app)
                    .title("Kairo")
                    .icon_name(APP_ID)
                    .default_width(1180)
                    .default_height(760)
                    .build();
                window.set_size_request(860, 560);
                let view = AppView::new(AppOptions {
                    app_id: APP_ID.into(),
                    title: "Kairo".into(),
                    size: (1180, 760),
                    appearance: Appearance::System,
                    ..AppOptions::default()
                });
                window.set_child(Some(&view.widget()));
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
            let Some(mtm) = MainThreadMarker::new() else { return };
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
        let hooks = kairo_tray::Hooks { show: Arc::new(present), quit };
        let tray_ok = kairo_tray::start(svc.clone(), hooks).is_ok();
        let settings = svc.settings();
        if (minimized || settings.start_minimized) && tray_ok {
            dispatch2::DispatchQueue::main().exec_async(|| {
                let Some(mtm) = MainThreadMarker::new() else { return };
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
    use super::*;
    use nsh_platform_windows::{AppOptions, AppSource};

    pub fn run(_minimized: bool) {
        let svc = super::start_primary();
        nsh_platform_windows::run_app(
            AppSource::Rust(super::root()),
            AppOptions { title: "Kairo".to_owned(), size: (1180, 760), ..Default::default() },
        );
        svc.shutdown();
    }
}
