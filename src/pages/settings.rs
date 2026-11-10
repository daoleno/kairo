//! Settings: two panes behind a segmented control in the toolbar. General is
//! what a person sets once (looks, login, how apps reach the proxy, AI,
//! updates); Advanced is ports, the controller, the core, delay tests, and the
//! Inspect views (the subscription's groups, rules, diagnostics, DNS).
//!
//! Switches apply at once; typed values apply together from the bar that
//! appears when one changes.

use std::cell::{Cell, RefCell};

use kairo_core::tun::Privilege;
use kairo_core::{CoreStatus, Settings, Theme, UpdateStatus};
use nsh_base::{button, theme};
use nsh_core::file_dialog::OpenDialog;
use nsh_core::prelude::*;

use crate::chrome::Glyph;
use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, Variant, size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Advanced,
}

thread_local! {
    /// The pane shown, kept while an Inspect view is open (Back returns to it).
    static TAB: Cell<Tab> = const { Cell::new(Tab::General) };
    /// Typed values not applied yet, kept while an Inspect view is open.
    static PENDING: RefCell<Option<Draft>> = const { RefCell::new(None) };
}

/// Settings › Advanced (where the Inspect views are opened from).
pub fn show_advanced() {
    TAB.with(|t| t.set(Tab::Advanced));
    store().page.set(Page::Settings);
}

/// The toolbar's Back button of an Inspect view.
pub fn back() -> toolbar::Spec {
    toolbar::button("back", "Settings", Glyph::Back, Icon::ChevronLeft, show_advanced)
}

/// The typed settings, as text while they are edited.
#[derive(Debug, Clone, PartialEq)]
struct Draft {
    mixed_port: String,
    controller_port: String,
    secret: String,
    bypass: String,
    test_url: String,
    test_timeout_ms: String,
    core_path: String,
}

impl Draft {
    fn of(s: &Settings) -> Draft {
        Draft {
            mixed_port: s.mixed_port.to_string(),
            controller_port: s.controller_port.to_string(),
            secret: s.secret.clone(),
            bypass: s.bypass.join(", "),
            test_url: s.test_url.clone(),
            test_timeout_ms: s.test_timeout_ms.to_string(),
            core_path: s.core_path.clone().unwrap_or_default(),
        }
    }

    fn apply(&self, base: &Settings) -> Result<Settings, String> {
        let port = |s: &str, what: &str| {
            s.trim()
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| format!("{what} must be a port (1–65535)"))
        };
        let mut next = base.clone();
        next.mixed_port = port(&self.mixed_port, "The proxy port")?;
        next.controller_port = port(&self.controller_port, "The controller port")?;
        if self.secret.trim().is_empty() {
            return Err("The controller secret can't be empty".into());
        }
        next.secret = self.secret.trim().to_owned();
        next.bypass = self
            .bypass
            .split([',', '\n'])
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        next.test_url = self.test_url.trim().to_owned();
        next.test_timeout_ms = self.test_timeout_ms.trim().parse().map_err(|_| {
            "The delay test timeout must be a number of milliseconds".to_string()
        })?;
        let core = self.core_path.trim();
        next.core_path = (!core.is_empty()).then(|| core.to_owned());
        next.validate()?;
        Ok(next)
    }
}

pub fn page() -> impl IntoElement {
    let store = store();
    let settings = computed(move || store.state.with(|s| s.settings.clone()));
    let tab = signal(TAB.with(Cell::get));
    let draft = signal(
        PENDING
            .with(|p| p.borrow_mut().take())
            .unwrap_or_else(|| Draft::of(&settings.get_untracked())),
    );
    // A change from elsewhere (the tray, a save) resets the fields it touches.
    let saved = signal(Draft::of(&settings.get_untracked()));
    effect(move || {
        let now = Draft::of(&settings.get());
        untracked(move || {
            let before = saved.get();
            draft.update(|d| {
                macro_rules! follow {
                    ($f:ident) => {
                        if d.$f == before.$f {
                            d.$f = now.$f.clone();
                        }
                    };
                }
                follow!(mixed_port);
                follow!(controller_port);
                follow!(secret);
                follow!(bypass);
                follow!(test_url);
                follow!(test_timeout_ms);
                follow!(core_path);
            });
            saved.set(now);
        });
    });
    // Leaving for an Inspect view keeps what was typed.
    cleanup(move || {
        let d = draft.get_untracked();
        let keep = d != saved.get_untracked();
        PENDING.with(|p| *p.borrow_mut() = keep.then_some(d));
    });
    let dirty = move || draft.with(|d| saved.with(|s| d != s));

    let apply = move || match draft.get_untracked().apply(&store.svc().settings()) {
        Ok(next) => store.run_then(store.svc().update_settings(next), move |r| {
            if r.is_ok() {
                store.info("Settings applied");
            }
        }),
        Err(e) => store.error(e),
    };

    view()
        .flex_1()
        .child(toolbar::floating(
            Page::Settings,
            String::new,
            vec![toolbar::choice(
                "settings.tab",
                "Settings",
                Glyph::Sliders,
                vec!["General", "Advanced"],
                move || match tab.get() {
                    Tab::General => 0,
                    Tab::Advanced => 1,
                },
                move |i| {
                    let t = if i == 0 { Tab::General } else { Tab::Advanced };
                    TAB.with(|c| c.set(t));
                    tab.set(t);
                },
            )],
        ))
        .child(
            scroll().flex_1().child(
                view()
                    .px_6()
                    .pt(ui::toolbar::scroll_top(20.0))
                    .pb_6()
                    .gap_5()
                    .w_full()
                    .max_w(px(760.0))
                    .self_center()
                    .child(dynamic(move || {
                        let t = tab.get();
                        untracked(move || match t {
                            Tab::General => general(settings).into_element(),
                            Tab::Advanced => advanced(settings, draft, apply).into_element(),
                        })
                    })),
            ),
        )
        .child(when(dirty, move || {
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(48.0))
                .px_5()
                .border_t_1()
                .border_color(move || theme().border)
                .bg(move || ui::subtle())
                .child(ui::dot(move || ui::warning()))
                .child(ui::body("Unapplied changes").flex_1())
                .child(ui::btn("Revert", Variant::Secondary).on_press(move || draft.set(saved.get_untracked())))
                .child(ui::btn("Apply", Variant::Primary).on_press(apply))
        }))
}

/// A switch that applies at once.
fn flag(settings: Memo<Settings>, get: fn(&Settings) -> bool, label: &'static str, put: fn(&mut Settings, bool)) -> AnyElement {
    let store = store();
    ui::toggle(
        move || settings.with(get),
        label,
        move |on| {
            let mut next = store.svc().settings();
            put(&mut next, on);
            store.run(store.svc().update_settings(next));
        },
    )
}

// --- General -------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Follow {
    System,
    Tun,
}

fn general(settings: Memo<Settings>) -> impl IntoElement {
    let store = store();
    let mac = cfg!(target_os = "macos");
    view()
        .gap_5()
        .child(ui::group(
            Some("General"),
            vec![
                ui::setting_row(
                    "Appearance",
                    None,
                    ui::segmented(
                        vec![(Theme::System, "System"), (Theme::Light, "Light"), (Theme::Dark, "Dark")],
                        move || settings.with(|s| s.theme),
                        move |t| {
                            let mut next = store.svc().settings();
                            next.theme = t;
                            store.run(store.svc().update_settings(next));
                        },
                    ),
                )
                .into_element(),
                ui::setting_row(
                    if mac { "Open at login" } else { "Launch at login" },
                    Some("Start Kairo when you log in."),
                    flag(settings, |s| s.launch_at_login, "Open at login", |s, on| s.launch_at_login = on),
                )
                .into_element(),
                ui::setting_row(
                    "Start minimized",
                    Some(if mac { "Open in the menu bar, without the window." } else { "Open in the tray, without the window." }),
                    flag(settings, |s| s.start_minimized, "Start minimized", |s, on| s.start_minimized = on),
                )
                .into_element(),
                ui::setting_row(
                    if mac { "Keep running in the menu bar" } else { "Keep running in the tray" },
                    Some("Closing the window leaves Kairo and the proxy running."),
                    flag(settings, |s| s.close_to_tray, "Keep running in the tray", |s, on| s.close_to_tray = on),
                )
                .into_element(),
            ],
        ))
        .child(connection_group(settings))
        .child(ai_group(settings))
        .child(about_group())
}

/// How apps reach the proxy: the system's proxy settings, or every app through
/// TUN (which needs a one-time permission for the core).
fn connection_group(settings: Memo<Settings>) -> impl IntoElement {
    let store = store();
    let privilege = computed(move || store.state.with(|s| s.tun_privilege));
    let granting = signal(false);
    let allow = move |then_tun: bool| {
        granting.set(true);
        store.run_then(store.svc().grant_tun(), move |r| {
            granting.set(false);
            if r.is_ok() {
                store.info("Permission granted");
                if then_tun {
                    store.run(store.svc().set_tun(true));
                }
            }
        });
    };
    let follow = move |f: Follow| match f {
        Follow::System => {
            let mut next = store.svc().settings();
            next.tun = false;
            if store.state.with_untracked(|s| s.sysproxy_supported) {
                next.system_proxy = true;
            }
            store.run(store.svc().update_settings(next));
        }
        Follow::Tun => match privilege.get_untracked() {
            Some(Privilege::Granted) => store.run(store.svc().set_tun(true)),
            Some(Privilege::Needed) => allow(true),
            Some(Privilege::Unsupported) => store.error("Every app (TUN) is not available on this system"),
            None => store.error("Install the core to use Every app (TUN)"),
        },
    };
    ui::group(
        Some("Connection"),
        vec![
            ui::setting_row_dyn(
                "Apps follow",
                move || -> SharedString {
                    if settings.with(|s| s.tun) {
                        "Every app's traffic goes through Kairo, including apps that ignore the system settings.".into()
                    } else if store.state.with(|s| s.sysproxy_supported) {
                        "Apps that follow the system's proxy settings go through Kairo.".into()
                    } else {
                        "This desktop has no system proxy setting Kairo can use; choose Every app (TUN).".into()
                    }
                },
                ui::segmented(
                    vec![(Follow::System, "System settings"), (Follow::Tun, "Every app (TUN)")],
                    move || if settings.with(|s| s.tun) { Follow::Tun } else { Follow::System },
                    follow,
                ),
            )
            .into_element(),
            ui::setting_row_dyn(
                "Every app permission",
                move || -> SharedString {
                    match privilege.get() {
                        None => "Install the core to use Every app (TUN).".into(),
                        Some(Privilege::Granted) => "The core may route every app's traffic.".into(),
                        Some(Privilege::Needed) if cfg!(target_os = "linux") => {
                            "A one-time permission: the core gets the cap_net_admin capability (you'll be asked for your password).".into()
                        }
                        Some(Privilege::Needed) => "A one-time permission: the core is made to run as root (you'll be asked for your password).".into(),
                        Some(Privilege::Unsupported) => "Not available on this platform.".into(),
                    }
                },
                dynamic(move || {
                    let p = privilege.get();
                    untracked(move || match p {
                        Some(Privilege::Granted) => ui::muted("Granted").into_element(),
                        Some(Privilege::Needed) => ui::btn_icon(Icon::Shield, "Allow…", Variant::Secondary)
                            .disabled(move || granting.get())
                            .on_press(move || allow(false))
                            .into_element(),
                        _ => ui::muted("Unavailable").into_element(),
                    })
                }),
            )
            .into_element(),
        ],
    )
}

/// What Kairo does for AI tools; each switch rebuilds the rules (open
/// connections keep running). Which exit they use is chosen under Exits.
fn ai_group(settings: Memo<Settings>) -> impl IntoElement {
    let store = store();
    let ai = move |f: fn(&kairo_core::ai::AiSettings) -> bool| move || settings.with(|s| f(&s.ai));
    let set = move |f: fn(&mut kairo_core::ai::AiSettings, bool)| {
        move |on: bool| {
            let mut next = store.svc().settings().ai;
            f(&mut next, on);
            store.run(store.svc().update_ai(next));
        }
    };
    ui::group(
        Some("AI"),
        vec![
            ui::setting_row(
                "AI tools use one exit",
                Some("Claude, ChatGPT, Gemini, Cursor, Copilot and the rest go through the exit you pick under Exits, and stay there when the Location changes."),
                ui::toggle(ai(|a| a.enabled), "AI tools use one exit", set(|a, v| a.enabled = v)),
            )
            .into_element(),
            ui::setting_row(
                "Domestic AI goes direct",
                Some("Kimi, DeepSeek, Doubao and Tongyi skip the proxy."),
                ui::toggle(ai(|a| a.domestic_direct), "Domestic AI goes direct", set(|a, v| a.domestic_direct = v)),
            )
            .into_element(),
        ],
    )
}

/// What the updater is doing, in a line.
fn update_text(u: &UpdateStatus) -> String {
    match u {
        UpdateStatus::Idle => "Updates download by themselves".into(),
        UpdateStatus::Checking => "Checking for updates…".into(),
        UpdateStatus::UpToDate { checked } => format!("Kairo is up to date · checked at {checked}"),
        UpdateStatus::Downloading {
            version,
            progress: Some(p),
        } => format!("Downloading Kairo {version} · {:.0}%", p * 100.0),
        UpdateStatus::Downloading { version, .. } => format!("Downloading Kairo {version}…"),
        UpdateStatus::Ready { version } => format!("Kairo {version} is ready"),
        UpdateStatus::Manual { version, .. } => {
            format!("Kairo {version} is out (this copy cannot update itself)")
        }
        UpdateStatus::Installing { version } => format!("Installing Kairo {version}…"),
        UpdateStatus::Failed { message } => format!("Update failed: {message}"),
    }
}

/// Check for Updates, Restart to Update (once downloaded), or the release page
/// (a copy that cannot update itself). Downloading is the sidebar's.
pub fn update_button() -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let u = store.state.with(|s| s.update.clone());
        untracked(move || match u {
            UpdateStatus::Ready { .. } => ui::btn_icon(Icon::Restart, "Restart to Update", Variant::Primary)
                .on_press(move || store.run(store.svc().restart_to_update()))
                .into_element(),
            UpdateStatus::Manual { url, .. } => ui::btn_icon(Icon::External, "Release Page", Variant::Secondary)
                .on_press(move || {
                    let url = url.clone();
                    spawn(async move {
                        open_url(url).await;
                    });
                })
                .into_element(),
            UpdateStatus::Checking | UpdateStatus::Downloading { .. } | UpdateStatus::Installing { .. } => {
                ui::btn("Check for Updates", Variant::Secondary).disabled(true).into_element()
            }
            _ => ui::btn("Check for Updates", Variant::Secondary)
                .on_press(move || {
                    store.run_then(store.svc().check_for_update(), move |r| {
                        if let Ok(None) = r {
                            store.info("Kairo is up to date");
                        }
                    })
                })
                .into_element(),
        })
    })
}

fn about_group() -> impl IntoElement {
    let store = store();
    ui::group(
        Some("About"),
        vec![
            ui::setting_row(
                concat!("Kairo ", env!("CARGO_PKG_VERSION")),
                Some("A native mihomo client. Free software under the GPL-3.0, like the mihomo core it ships with."),
                view(),
            )
            .into_element(),
            ui::setting_row(move || -> SharedString { update_text(&store.state.with(|s| s.update.clone())).into() }, None, update_button())
                .into_element(),
        ],
    )
}

// --- Advanced ------------------------------------------------------------------------

/// A text field bound to one value of the draft; Return applies.
fn text_field(
    draft: Signal<Draft>,
    apply: impl Fn() + Copy + 'static,
    get: fn(&Draft) -> String,
    put: fn(&mut Draft, String),
    placeholder: &'static str,
    w: f32,
) -> nsh_base::Input {
    nsh_base::input()
        .value(move || draft.with(get))
        .on_change(move |t: &str| draft.update(|d| put(d, t.to_owned())))
        .on_submit(move |_: &str| apply())
        .placeholder(placeholder)
        .placeholder_color(|| color(theme().muted_foreground))
        .caret_color(|| color(theme().primary))
        .h(px(28.0))
        .px_2()
        .w(px(w))
        .text_size(px(size::body()))
        .rounded(ui::radius_control())
        .border_1()
        .border_color(move || theme().input)
        .bg(move || theme().background)
        .focus(|s| s.border_color(move || theme().primary))
}

fn advanced(settings: Memo<Settings>, draft: Signal<Draft>, apply: impl Fn() + Copy + 'static) -> impl IntoElement {
    let store = store();
    let field = move |get: fn(&Draft) -> String, put: fn(&mut Draft, String), placeholder: &'static str, w: f32| {
        text_field(draft, apply, get, put, placeholder, w)
    };
    let home = store.svc().paths().home.clone();
    let revealed = signal(false);

    view()
        .gap_5()
        .child(ui::group(
            Some("Advanced"),
            vec![
                ui::setting_row(
                    "Port (HTTP + SOCKS)",
                    Some("One port for both; the system proxy points here."),
                    field(|d| d.mixed_port.clone(), |d, v| d.mixed_port = v, "7897", 90.0).text_right().test_id("mixed-port"),
                )
                .into_element(),
                ui::setting_row(
                    "Allow other devices on the network",
                    Some("Let other devices on your network use this proxy."),
                    flag(settings, |s| s.allow_lan, "Allow other devices on the network", |s, on| s.allow_lan = on),
                )
                .into_element(),
                ui::setting_row(
                    "IPv6",
                    Some("Resolve and connect over IPv6."),
                    flag(settings, |s| s.ipv6, "IPv6", |s, on| s.ipv6 = on),
                )
                .into_element(),
                ui::setting_row(
                    "Skip the proxy for",
                    Some("Hosts that skip the system proxy, separated by commas."),
                    field(|d| d.bypass.clone(), |d, v| d.bypass = v, "localhost, 127.0.0.1, ::1", 280.0),
                )
                .into_element(),
                ui::setting_row(
                    "Controller",
                    Some("mihomo's API, on 127.0.0.1 only; every request needs the secret."),
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(field(|d| d.controller_port.clone(), |d, v| d.controller_port = v, "9097", 76.0).text_right())
                        .child(
                            when(move || revealed.get(), move || field(|d| d.secret.clone(), |d, v| d.secret = v, "secret", 240.0))
                                .otherwise(move || secret_mask(revealed)),
                        )
                        .child(ui::btn("New Secret", Variant::Secondary).on_press(move || {
                            draft.update(|d| d.secret = kairo_core::settings::new_secret());
                            revealed.set(true);
                        })),
                )
                .into_element(),
                core_row(draft, apply),
                geo_row(),
                ui::setting_row(
                    "Delay test",
                    Some("The address requested through each node, and how long to wait for it."),
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(field(|d| d.test_url.clone(), |d, v| d.test_url = v, "https://www.gstatic.com/generate_204", 280.0))
                        .child(field(|d| d.test_timeout_ms.clone(), |d, v| d.test_timeout_ms = v, "5000", 70.0).text_right())
                        .child(ui::muted("ms")),
                )
                .into_element(),
                ui::setting_row(
                    "Config folder",
                    Some("Kairo's settings, profiles and the core's working files."),
                    ui::btn(
                        if cfg!(target_os = "macos") { "Open in Finder" } else { "Open Folder" },
                        Variant::Secondary,
                    )
                    .on_press(move || {
                        let home = home.clone();
                        spawn(async move {
                            let _ = open_path(home).await;
                        });
                    }),
                )
                .into_element(),
            ],
        ))
        .child(inspect_group())
}

/// The secret, hidden until asked for.
fn secret_mask(revealed: Signal<bool>) -> impl IntoElement {
    button()
        .accessibility_label("Show secret")
        .on_press(move || revealed.set(true))
        .flex_row()
        .items_center()
        .h(px(28.0))
        .px_2()
        .w(px(240.0))
        .rounded(ui::radius_control())
        .border_1()
        .border_color(move || theme().input)
        .bg(move || theme().background)
        .cursor_pointer()
        .child(
            text("••••••••")
                .text_size(px(size::body()))
                .text_color(move || theme().muted_foreground),
        )
}

/// The core: its version and state, Restart, and a core of one's own.
fn core_row(draft: Signal<Draft>, apply: impl Fn() + Copy + 'static) -> AnyElement {
    let store = store();
    let _ = apply;
    let line = move || -> SharedString {
        store.state.with(|s| {
            let version = match &s.core_version {
                Some(v) => format!("mihomo {v}"),
                None => "mihomo".to_owned(),
            };
            let state = match &s.core {
                CoreStatus::Running { .. } => "running",
                CoreStatus::Starting => "starting",
                CoreStatus::Stopping => "stopping",
                CoreStatus::Stopped => "stopped",
                CoreStatus::Failed { .. } => "stopped with an error",
            };
            let custom = if s.settings.core_path.is_some() { " · your own core" } else { "" };
            format!("{version} · {state}{custom}").into()
        })
    };
    let custom = move || draft.with(|d| !d.core_path.trim().is_empty());
    ui::setting_row_dyn(
        "Core",
        line,
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                ui::btn("Restart", Variant::Secondary)
                    .disabled(move || !store.state.with(|s| s.core.is_running()))
                    .on_press(move || store.run(store.svc().restart_core())),
            )
            .child(when(custom, move || {
                ui::btn("Use Kairo's", Variant::Secondary).on_press(move || draft.update(|d| d.core_path.clear()))
            }))
            .child(ui::btn("Custom…", Variant::Secondary).on_press(move || {
                spawn(async move {
                    let picked = pick_paths(OpenDialog::files().title("Choose a mihomo binary").button_label("Choose")).await;
                    if let Ok(Some(paths)) = picked
                        && let Some(path) = paths.first()
                    {
                        draft.update(|d| d.core_path = path.display().to_string());
                    }
                });
            })),
    )
    .into_element()
}

/// The geo databases: when they were fetched, and a button to fetch them again.
fn geo_row() -> AnyElement {
    let store = store();
    let tick = signal(0u32);
    let updating = signal(false);
    let running = computed(move || store.state.with(|s| s.core.is_running()));
    let detail = move || -> SharedString {
        let _ = tick.get();
        let paths = store.svc().paths().clone();
        match geo_mtime(&paths) {
            Some(secs) => format!(
                "Updated {}",
                date_text(secs + kairo_core::service::local_offset_seconds(), kairo_core::service::unix() + kairo_core::service::local_offset_seconds())
            )
            .into(),
            None => "Not downloaded yet".into(),
        }
    };
    ui::setting_row_dyn(
        "GeoIP and GeoSite",
        detail,
        ui::btn(
            move || -> SharedString { if updating.get() { "Updating…".into() } else { "Update".into() } },
            Variant::Secondary,
        )
        .disabled(move || updating.get() || !running.get())
        .on_press(move || {
            updating.set(true);
            store.run_then(store.svc().update_geo(), move |r| {
                updating.set(false);
                tick.update(|t| *t += 1);
                if r.is_ok() {
                    store.info("GeoIP and GeoSite updated");
                }
            });
        }),
    )
    .into_element()
}

/// When the newest geo file was written (unix seconds).
fn geo_mtime(paths: &kairo_core::paths::Paths) -> Option<i64> {
    [kairo_core::geo::MMDB, kairo_core::geo::GEOSITE]
        .into_iter()
        .filter_map(|f| std::fs::metadata(paths.runtime_dir().join(f.name)).ok()?.modified().ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .max()
}

/// A day as "Nov 8" (with the year when it is not this one). Both times are
/// local seconds.
fn date_text(local_secs: i64, local_now: i64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (year, month, day) = civil(local_secs.div_euclid(86_400));
    let (now_year, _, _) = civil(local_now.div_euclid(86_400));
    let name = MONTHS[(month as usize).clamp(1, 12) - 1];
    if year == now_year {
        format!("{name} {day}")
    } else {
        format!("{name} {day}, {year}")
    }
}

/// The proleptic Gregorian date of a day count since 1970-01-01.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// 1214 as "1,214".
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The count and a Show button that opens `page`.
fn open(store: crate::store::Store, label: &'static str, page: Page, count: impl Fn() -> SharedString + 'static) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .child(ui::muted(count))
        .child(ui::btn(label, Variant::Secondary).on_press(move || store.page.set(page)))
}

/// "The subscription's groups", Rules, Diagnostics and DNS: read-only views
/// (and lookups) for the few who need them.
fn inspect_group() -> impl IntoElement {
    let store = store();
    let running = computed(move || store.state.with(|s| s.core.is_running()));
    let groups = computed(move || store.proxies.with(|p| p.0.groups.iter().filter(|g| !g.hidden && g.name != "GLOBAL").count()));
    // How many rules run: asked of the core while it is up.
    let rules = signal(None::<usize>);
    effect(move || {
        if running.get() {
            untracked(move || {
                let svc = store.svc();
                spawn(async move {
                    if let Ok(r) = svc.rules().await {
                        rules.set(Some(r.len()));
                    }
                });
            });
        } else {
            rules.set(None);
        }
    });
    ui::group(
        Some("Inspect"),
        vec![
            ui::setting_row(
                "The subscription's groups",
                Some("Browse the groups the subscription defines, with their nodes."),
                open(store, "Show", Page::Proxies, move || -> SharedString {
                    match groups.get() {
                        0 => "".into(),
                        1 => "1 group".into(),
                        n => format!("{n} groups").into(),
                    }
                }),
            )
            .into_element(),
            ui::setting_row(
                "Rules",
                Some("The routing rules that are running. Change routes under Apps & Sites."),
                open(store, "Show", Page::Rules, move || -> SharedString {
                    match rules.get() {
                        Some(n) => format!("{} · read-only", thousands(n)).into(),
                        None => "read-only".into(),
                    }
                }),
            )
            .into_element(),
            ui::setting_row(
                "Diagnostics",
                Some("The core's log, and a summary to copy for a bug report."),
                open(store, "Show", Page::Logs, || "logs".into()),
            )
            .into_element(),
            ui::setting_row(
                "DNS",
                Some("Ask the core's resolver, or forget the fake addresses it handed out."),
                ui::btn("Look Up…", Variant::Secondary).on_press(move || store.page.set(Page::Dns)),
            )
            .into_element(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> Draft {
        Draft::of(&Settings {
            secret: "abc".into(),
            ..Settings::default()
        })
    }

    #[test]
    fn a_draft_of_the_settings_applies_to_the_same_settings() {
        let base = Settings {
            secret: "abc".into(),
            ..Settings::default()
        };
        assert_eq!(Draft::of(&base).apply(&base).unwrap(), base);
    }

    #[test]
    fn ports_are_checked() {
        let base = Settings::default();
        for bad in ["", "0", "70000", "http", "-1"] {
            let mut d = draft();
            d.mixed_port = bad.into();
            assert!(d.apply(&base).is_err(), "{bad}");
        }
        let mut d = draft();
        d.mixed_port = " 8080 ".into();
        assert_eq!(d.apply(&base).unwrap().mixed_port, 8080);
        // The two ports must differ.
        d.controller_port = "8080".into();
        assert!(d.apply(&base).is_err());
    }

    #[test]
    fn the_secret_can_not_be_empty() {
        let mut d = draft();
        d.secret = "  ".into();
        assert!(d.apply(&Settings::default()).is_err());
        d.secret = " s3 ".into();
        assert_eq!(d.apply(&Settings::default()).unwrap().secret, "s3");
    }

    #[test]
    fn the_bypass_list_splits_on_commas_and_lines() {
        let mut d = draft();
        d.bypass = "localhost, 127.0.0.1\n*.local,, ".into();
        assert_eq!(d.apply(&Settings::default()).unwrap().bypass, ["localhost", "127.0.0.1", "*.local"]);
    }

    #[test]
    fn the_delay_test_is_checked() {
        let base = Settings::default();
        let mut d = draft();
        d.test_timeout_ms = "five".into();
        assert!(d.apply(&base).is_err());
        d.test_timeout_ms = "50".into();
        assert!(d.apply(&base).is_err(), "too short");
        d.test_timeout_ms = "3000".into();
        d.test_url = "not a url".into();
        assert!(d.apply(&base).is_err());
        d.test_url = "https://example.com/204".into();
        let next = d.apply(&base).unwrap();
        assert_eq!((next.test_timeout_ms, next.test_url.as_str()), (3000, "https://example.com/204"));
    }

    #[test]
    fn an_empty_core_path_means_kairos_own() {
        let mut d = draft();
        d.core_path = "  ".into();
        assert_eq!(d.apply(&Settings::default()).unwrap().core_path, None);
        d.core_path = "/opt/mihomo".into();
        assert_eq!(d.apply(&Settings::default()).unwrap().core_path.as_deref(), Some("/opt/mihomo"));
    }

    #[test]
    fn dates_read_the_way_people_say_them() {
        // 2025-11-08 12:00 and 2024-02-29.
        let nov8 = 20_400 * 86_400 + 43_200;
        assert_eq!(civil(20_400), (2025, 11, 8));
        assert_eq!(date_text(nov8, nov8 + 86_400), "Nov 8");
        assert_eq!(civil(19_782), (2024, 2, 29));
        assert_eq!(date_text(19_782 * 86_400, nov8), "Feb 29, 2024");
        assert_eq!(civil(0), (1970, 1, 1));
    }

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1214), "1,214");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
