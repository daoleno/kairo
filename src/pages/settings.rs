//! Settings: the app, the proxy's ports and reach, TUN, the controller, delay
//! tests and the core itself. Switches apply at once; typed values apply
//! together from the bar that appears when one changes.

use kairo_core::tun::Privilege;
use kairo_core::{Settings, Theme};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::store::store;
use crate::ui::{self, Icon, Variant, size};

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
        let port = |s: &str, what: &str| s.trim().parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| format!("{what} must be a port (1–65535)"));
        let mut next = base.clone();
        next.mixed_port = port(&self.mixed_port, "The proxy port")?;
        next.controller_port = port(&self.controller_port, "The controller port")?;
        if self.secret.trim().is_empty() {
            return Err("The controller secret can't be empty".into());
        }
        next.secret = self.secret.trim().to_owned();
        next.bypass = self.bypass.split([',', '\n']).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();
        next.test_url = self.test_url.trim().to_owned();
        next.test_timeout_ms = self.test_timeout_ms.trim().parse().map_err(|_| "The delay test timeout must be a number of milliseconds".to_string())?;
        let core = self.core_path.trim();
        next.core_path = (!core.is_empty()).then(|| core.to_owned());
        next.validate()?;
        Ok(next)
    }
}

pub fn page() -> impl IntoElement {
    let store = store();
    let settings = computed(move || store.state.with(|s| s.settings.clone()));
    let draft = signal(Draft::of(&settings.get_untracked()));
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
    let dirty = move || draft.with(|d| saved.with(|s| d != s));

    // A switch applies at once.
    let set = move |f: fn(&mut Settings, bool)| {
        move |on: bool| {
            let mut next = store.svc().settings();
            f(&mut next, on);
            store.run(store.svc().update_settings(next));
        }
    };
    let apply = move || match draft.get_untracked().apply(&store.svc().settings()) {
        Ok(next) => store.run_then(store.svc().update_settings(next), move |r| {
            if r.is_ok() {
                store.info("Settings applied");
            }
        }),
        Err(e) => store.error(e),
    };

    let text_field = move |get: fn(&Draft) -> String, put: fn(&mut Draft, String), placeholder: &'static str, w: f32| {
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
            .text_size(px(size::BODY))
            .rounded(ui::radius_control())
            .border_1()
            .border_color(move || theme().input)
            .bg(move || theme().background)
            .focus(|s| s.border_color(move || theme().primary))
    };

    let s = move |f: fn(&Settings) -> bool| move || settings.with(f);

    view()
        .flex_1()
        .child(ui::page_header("Settings", vec![]))
        .child(
            scroll().flex_1().child(
                view()
                    .p_5()
                    .gap_5()
                    .max_w(px(760.0))
                    // --- General ---
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
                                "Launch at login",
                                Some("Start Kairo when you log in."),
                                ui::toggle(s(|s| s.launch_at_login), "Launch at login", set(|s, on| s.launch_at_login = on)),
                            )
                            .into_element(),
                            ui::setting_row(
                                "Start minimized",
                                Some("Open in the tray, without the window."),
                                ui::toggle(s(|s| s.start_minimized), "Start minimized", set(|s, on| s.start_minimized = on)),
                            )
                            .into_element(),
                            ui::setting_row(
                                "Keep running in the tray",
                                Some("Closing the window leaves Kairo and the proxy running."),
                                ui::toggle(s(|s| s.close_to_tray), "Keep running in the tray", set(|s, on| s.close_to_tray = on)),
                            )
                            .into_element(),
                            ui::setting_row(
                                "Start the core with Kairo",
                                None,
                                ui::toggle(s(|s| s.auto_start_core), "Start the core with Kairo", set(|s, on| s.auto_start_core = on)),
                            )
                            .into_element(),
                            ui::setting_row(
                                "Tray quick switch",
                                Some("The group whose nodes the tray menu offers."),
                                ui::popup_button(
                                    move || {
                                        let mut v = vec![(String::new(), "First selector".to_owned())];
                                        v.extend(store.proxies.with_untracked(|p| {
                                            p.0.groups.iter().filter(|g| g.selectable() && !g.hidden && g.name != "GLOBAL").map(|g| (g.name.clone(), g.name.clone())).collect::<Vec<_>>()
                                        }));
                                        let cur = store.svc().settings().tray_group;
                                        if !cur.is_empty() && !v.iter().any(|(n, _)| *n == cur) {
                                            v.push((cur.clone(), cur));
                                        }
                                        v
                                    },
                                    move || settings.with(|s| s.tray_group.clone()),
                                    move |g: String| {
                                        let mut next = store.svc().settings();
                                        next.tray_group = g;
                                        store.run(store.svc().update_settings(next));
                                    },
                                ),
                            )
                            .into_element(),
                        ],
                    ))
                    // --- Proxy ---
                    .child(ui::group(
                        Some("Proxy"),
                        vec![
                            ui::setting_row(
                                "Proxy port",
                                Some("HTTP and SOCKS5 on one port; the system proxy points here."),
                                text_field(|d| d.mixed_port.clone(), |d, v| d.mixed_port = v, "7897", 90.0).text_right().test_id("mixed-port"),
                            )
                            .into_element(),
                            ui::setting_row(
                                "Allow LAN",
                                Some("Let other devices on your network use this proxy."),
                                ui::toggle(s(|s| s.allow_lan), "Allow LAN", set(|s, on| s.allow_lan = on)),
                            )
                            .into_element(),
                            ui::setting_row("IPv6", Some("Resolve and connect over IPv6."), ui::toggle(s(|s| s.ipv6), "IPv6", set(|s, on| s.ipv6 = on)))
                                .into_element(),
                            ui::setting_row(
                                "System proxy bypass",
                                Some("Hosts that skip the system proxy, separated by commas."),
                                text_field(|d| d.bypass.clone(), |d, v| d.bypass = v, "localhost, 127.0.0.1, ::1", 280.0),
                            )
                            .into_element(),
                            ui::setting_row(
                                move || -> SharedString {
                                    if store.state.with(|s| s.sysproxy_supported) { "System proxy".into() } else { "System proxy (not available on this desktop)".into() }
                                },
                                None,
                                ui::toggle(s(|s| s.system_proxy), "System proxy", move |on| store.run(store.svc().set_system_proxy(on))),
                            )
                            .into_element(),
                        ],
                    ))
                    // --- TUN ---
                    .child(tun_group())
                    // --- Controller ---
                    .child(ui::group(
                        Some("Controller"),
                        vec![
                            ui::setting_row(
                                "Controller port",
                                Some("mihomo's API, on 127.0.0.1 only."),
                                text_field(|d| d.controller_port.clone(), |d, v| d.controller_port = v, "9097", 90.0).text_right(),
                            )
                            .into_element(),
                            ui::setting_row(
                                "Secret",
                                Some("Required by every API request."),
                                view()
                                    .flex_row()
                                    .gap_1()
                                    .child(text_field(|d| d.secret.clone(), |d, v| d.secret = v, "secret", 260.0))
                                    .child(ui::icon_btn(Icon::Restart, "New Secret").on_press(move || draft.update(|d| d.secret = kairo_core::settings::new_secret()))),
                            )
                            .into_element(),
                        ],
                    ))
                    // --- Delay test ---
                    .child(ui::group(
                        Some("Delay Test"),
                        vec![
                            ui::setting_row(
                                "Test URL",
                                Some("Requested through each node; groups with their own URL use theirs."),
                                text_field(|d| d.test_url.clone(), |d, v| d.test_url = v, "https://www.gstatic.com/generate_204", 300.0),
                            )
                            .into_element(),
                            ui::setting_row(
                                "Timeout",
                                None,
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(text_field(|d| d.test_timeout_ms.clone(), |d, v| d.test_timeout_ms = v, "5000", 80.0).text_right())
                                    .child(ui::muted("ms")),
                            )
                            .into_element(),
                        ],
                    ))
                    // --- Core ---
                    .child(core_group(text_field(|d| d.core_path.clone(), |d, v| d.core_path = v, "Kairo's own (downloaded)", 300.0)))
                    .child(about_group()),
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

fn tun_group() -> impl IntoElement {
    let store = store();
    let privilege = computed(move || store.state.with(|s| s.tun_privilege));
    let granting = signal(false);
    let tun_on = move || store.state.with(|s| s.settings.tun);
    let stack = move || store.state.with(|s| s.settings.tun_stack.clone());
    let set_stack = move |v: String| {
        let mut next = store.svc().settings();
        next.tun_stack = v;
        store.run(store.svc().update_settings(next));
    };
    let detail = move || -> SharedString {
        match privilege.get() {
            None => "Install the core to use TUN.".into(),
            Some(Privilege::Granted) => "Routes every app's traffic through the core, including apps that ignore the system proxy.".into(),
            Some(Privilege::Needed) if cfg!(target_os = "linux") => {
                "TUN needs a one-time permission: the core gets the cap_net_admin capability (you'll be asked for your password).".into()
            }
            Some(Privilege::Needed) => "TUN needs a one-time permission: the core is made to run as root (you'll be asked for your password).".into(),
            Some(Privilege::Unsupported) => "TUN is not available on this platform.".into(),
        }
    };
    ui::group(
        Some("TUN Mode"),
        vec![
            view()
                .flex_row()
                .items_center()
                .gap_4()
                .px_4()
                .py_2p5()
                .min_h(px(44.0))
                .child(view().flex_1().min_w(px(0.0)).gap_0p5().child(ui::body("TUN mode")).child(ui::muted(detail)))
                .child(
                    when(
                        move || privilege.get() == Some(Privilege::Granted),
                        move || ui::toggle(tun_on, "TUN mode", move |on| store.run(store.svc().set_tun(on))),
                    )
                    .otherwise(move || {
                        when(
                            move || privilege.get() == Some(Privilege::Needed),
                            move || {
                                ui::btn_icon(Icon::Shield, "Grant Permission…", Variant::Secondary).disabled(move || granting.get()).on_press(move || {
                                    granting.set(true);
                                    store.run_then(store.svc().grant_tun(), move |r| {
                                        granting.set(false);
                                        if r.is_ok() {
                                            store.info("TUN permission granted");
                                        }
                                    });
                                })
                            },
                        )
                    }),
                )
                .into_element(),
            ui::setting_row(
                "Stack",
                Some("Mixed suits most; gVisor is the most compatible, System the fastest."),
                ui::segmented(vec![("mixed", "Mixed"), ("gvisor", "gVisor"), ("system", "System")], move || -> &'static str {
                    match stack().as_str() {
                        "gvisor" => "gvisor",
                        "system" => "system",
                        _ => "mixed",
                    }
                }, move |v: &'static str| set_stack(v.to_owned())),
            )
            .into_element(),
        ],
    )
}

fn core_group(core_path_field: impl IntoElement) -> impl IntoElement {
    let store = store();
    let checking = signal(false);
    let version = move || -> SharedString {
        store.state.with(|s| match (&s.core_version, &s.core) {
            (Some(v), _) => format!("mihomo {v}").into(),
            (None, kairo_core::CoreStatus::Missing) => "Not installed".into(),
            _ => "Unknown".into(),
        })
    };
    let update = move || store.state.with(|s| s.core_update.clone());
    let busy = move || store.state.with(|s| s.task.is_some()) || checking.get();
    ui::group(
        Some("Core"),
        vec![
            ui::setting_row(
                version,
                Some("Downloaded from MetaCubeX/mihomo's GitHub releases and checked against the published SHA-256."),
                view()
                    .flex_row()
                    .gap_2()
                    .child(
                        when(
                            move || update().is_some(),
                            move || {
                                ui::btn_icon(Icon::Download, move || -> SharedString { format!("Update to {}", update().unwrap_or_default()).into() }, Variant::Primary)
                                    .disabled(busy)
                                    .on_press(move || store.run(store.svc().install_core()))
                            },
                        )
                        .otherwise(move || {
                            ui::btn(
                                move || -> SharedString {
                                    if store.state.with(|s| s.core_version.is_none()) { "Install".into() } else { "Check for Updates".into() }
                                },
                                Variant::Secondary,
                            )
                            .disabled(busy)
                            .on_press(move || {
                                if store.state.with_untracked(|s| s.core_version.is_none()) {
                                    store.run(store.svc().install_core());
                                    return;
                                }
                                checking.set(true);
                                store.run_then(store.svc().check_core_update(), move |r| {
                                    checking.set(false);
                                    if let Ok(None) = r {
                                        store.info("The core is up to date");
                                    }
                                });
                            })
                        }),
                    ),
            )
            .into_element(),
            ui::setting_row(
                "Task",
                None,
                ui::muted(move || -> SharedString {
                    store.state.with(|s| match &s.task {
                        Some(t) => match t.progress {
                            Some(p) => format!("{} · {:.0}%", t.label, p * 100.0).into(),
                            None => format!("{}…", t.label).into(),
                        },
                        None => "Idle".into(),
                    })
                }),
            )
            .style_when(move || store.state.with(|s| s.task.is_none()), |s| s.hidden())
            .into_element(),
            ui::setting_row("Custom core", Some("Path to a mihomo binary of your own; empty uses Kairo's."), core_path_field).into_element(),
            ui::setting_row(
                "GeoIP and GeoSite data",
                Some("Download the latest databases the rules use."),
                ui::btn("Update", Variant::Secondary).disabled(move || !store.state.with(|s| s.core.is_running())).on_press(move || {
                    store.run_then(store.svc().update_geo(), move |r| {
                        if r.is_ok() {
                            store.info("Geo databases updated");
                        }
                    })
                }),
            )
            .into_element(),
        ],
    )
}

fn about_group() -> impl IntoElement {
    let store = store();
    let home = store.svc().paths().home.display().to_string();
    let home_url = format!("file://{home}");
    ui::group(
        Some("About"),
        vec![
            ui::setting_row(
                concat!("Kairo ", env!("CARGO_PKG_VERSION")),
                Some("A native mihomo client, free software under the GPL-3.0. mihomo is © MetaCubeX, GPL-3.0, and is downloaded when you install the core."),
                view(),
            )
            .into_element(),
            ui::setting_row(
                "Data folder",
                None,
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(ui::muted(home).number_of_lines(1).max_w(px(320.0)).selectable(true))
                    .child(ui::icon_btn(Icon::External, "Open Data Folder").on_press(move || {
                        let url = home_url.clone();
                        spawn(async move {
                            open_url(url).await;
                        });
                    })),
            )
            .into_element(),
        ],
    )
}
