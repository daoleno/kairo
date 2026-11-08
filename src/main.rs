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

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let minimized = std::env::args().any(|a| a == "--minimized");
    let svc = Service::global();
    emergency_handlers(&svc);
    platform::run(svc, minimized);
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

    pub fn run(svc: Service, minimized: bool) {
        glib::set_prgname(Some(APP_ID));
        glib::set_application_name("Kairo");
        // One Kairo per user: a second launch shows the first one's window.
        let app = gtk::Application::builder().application_id(APP_ID).build();
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
    use nsh_platform_macos::{AppOptions, AppSource, Appearance};
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

    pub fn run(svc: Service, minimized: bool) {
        let quit_svc = svc.clone();
        let quit = Arc::new(move || {
            quit_svc.shutdown();
            nsh_platform_macos::quit_app();
        });
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
        nsh_platform_macos::run_app(
            AppSource::Rust(super::root()),
            AppOptions {
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

    pub fn run(svc: Service, _minimized: bool) {
        nsh_platform_windows::run_app(
            AppSource::Rust(super::root()),
            AppOptions { title: "Kairo".to_owned(), size: (1180, 760), ..Default::default() },
        );
        svc.shutdown();
    }
}
