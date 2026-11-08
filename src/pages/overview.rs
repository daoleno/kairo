//! Overview: is it working, how is it routing, what is flowing.

use kairo_core::{CoreStatus, Mode};
use nsh_base::theme;
use nsh_core::canvas::LineJoin;
use nsh_core::prelude::*;

use crate::store::{Page, store};
use crate::ui::{self, Icon, Variant, icon, size};

pub fn page() -> impl IntoElement {
    view()
        .flex_1()
        .child(ui::page_header("Overview", vec![]))
        .child(
            scroll().flex_1().child(
                view()
                    .p_5()
                    .gap_4()
                    .max_w(px(1100.0))
                    .child(status_card())
                    .child(traffic_card())
                    .child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .gap_4()
                            .child(profile_card().flex_1().min_w(px(280.0)))
                            .child(group_card().flex_1().min_w(px(280.0))),
                    ),
            ),
        )
}

fn status_card() -> impl IntoElement {
    let store = store();
    let core = move || store.state.with(|s| s.core.clone());
    let headline = move || -> SharedString {
        let s = store.state.get();
        match &s.core {
            CoreStatus::Running { version } => format!("Running · mihomo {version}").into(),
            CoreStatus::Missing => "The mihomo core is not installed".into(),
            CoreStatus::Stopped if s.profiles.active.is_none() => "Add a profile to start".into(),
            other => other.title().into(),
        }
    };
    let detail = move || -> SharedString {
        let s = store.state.get();
        if let Some(task) = &s.task {
            return match task.progress {
                Some(p) => format!("{} · {:.0}%", task.label, p * 100.0).into(),
                None => format!("{}…", task.label).into(),
            };
        }
        match &s.core {
            CoreStatus::Failed { message } => message.clone().into(),
            CoreStatus::Running { .. } => {
                let mut parts = vec![format!("Mixed port {}", s.settings.mixed_port)];
                if s.settings.allow_lan {
                    parts.push("LAN access on".into());
                }
                if let Some(w) = &s.warning {
                    parts.push(w.clone());
                }
                parts.join(" · ").into()
            }
            CoreStatus::Missing => "Kairo downloads the official build for this computer and verifies its checksum.".into(),
            _ => match s.profiles.active_profile() {
                Some(p) => p.name.clone().into(),
                None => "A subscription URL or a config file from your provider.".into(),
            },
        }
    };
    let tone = move || match core() {
        CoreStatus::Running { .. } => ui::success(),
        CoreStatus::Starting | CoreStatus::Stopping => ui::warning(),
        CoreStatus::Failed { .. } => theme().destructive,
        _ => theme().muted_foreground,
    };

    let action = dynamic(move || {
        let core = store.state.with(|s| s.core.clone());
        let has_profile = store.state.with(|s| s.profiles.active.is_some());
        let busy = store.state.with(|s| s.task.is_some());
        untracked(move || {
            let svc = store.svc();
            match core {
                CoreStatus::Missing => ui::btn_icon(Icon::Download, "Install Core", Variant::Primary)
                    .disabled(busy)
                    .on_press(move || store.run(svc.install_core()))
                    .into_element(),
                CoreStatus::Running { .. } => view()
                    .flex_row()
                    .gap_2()
                    .child(ui::btn_icon(Icon::Restart, "Restart", Variant::Secondary).on_press({
                        let svc = svc.clone();
                        move || store.run(svc.restart_core())
                    }))
                    .child(ui::btn_icon(Icon::Stop, "Stop", Variant::Secondary).on_press(move || {
                        spawn(svc.stop_core());
                    }))
                    .into_element(),
                CoreStatus::Starting | CoreStatus::Stopping => ui::btn("Starting…", Variant::Secondary).disabled(true).into_element(),
                _ if !has_profile => ui::btn_icon(Icon::Plus, "Add Profile…", Variant::Primary)
                    .on_press(move || store.page.set(Page::Profiles))
                    .into_element(),
                _ => ui::btn_icon(Icon::Play, "Start", Variant::Primary)
                    .on_press(move || store.run(svc.start_core()))
                    .into_element(),
            }
        })
    });

    let set_mode = move |m: Mode| store.run(store.svc().set_mode(m));
    let sysproxy = move || store.state.with(|s| s.settings.system_proxy);
    let tun = move || store.state.with(|s| s.settings.tun);
    ui::card()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .p_4()
                .child(view().size(px(10.0)).rounded_full().bg(tone))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap_0p5()
                        .child(text(headline).text_size(px(size::LARGE)).font_weight(FontWeight::SEMIBOLD))
                        .child(ui::muted(detail).number_of_lines(2)),
                )
                .child(action),
        )
        .child(ui::divider())
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_x_6()
                .gap_y_3()
                .px_4()
                .py_3()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(ui::body("Mode").text_color(move || theme().muted_foreground))
                        .child(
                            ui::segmented(
                                Mode::ALL.iter().map(|m| (*m, m.title())).collect(),
                                move || store.state.with(|s| s.settings.mode),
                                set_mode,
                            )
                            .w(px(240.0)),
                        ),
                )
                .child(view().flex_1())
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(ui::body("System Proxy"))
                        .child(ui::toggle(sysproxy, "System Proxy", move |on| store.run(store.svc().set_system_proxy(on)))),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(ui::body("TUN"))
                        .child(ui::toggle(tun, "TUN", move |on| {
                            let svc = store.svc();
                            let needs = store.state.with_untracked(|s| s.tun_privilege != Some(kairo_core::tun::Privilege::Granted));
                            if on && needs {
                                store.error("TUN needs permission once: Settings › TUN › Grant");
                                store.page.set(Page::Settings);
                            } else {
                                store.run(svc.set_tun(on));
                            }
                        })),
                ),
        )
}

fn stat(label: &'static str, value: impl Fn() -> SharedString + 'static) -> impl IntoElement {
    view()
        .gap_0p5()
        .min_w(px(96.0))
        .child(ui::muted(label))
        .child(ui::figure(value, size::BODY).font_weight(FontWeight::MEDIUM))
}

fn speed_figure(glyph: Icon, label: &'static str, tone: fn() -> AdaptiveColor, value: impl Fn() -> u64 + 'static) -> impl IntoElement {
    view()
        .gap_1()
        .min_w(px(150.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .child(view().size(px(8.0)).rounded(px(2.0)).bg(tone))
                .child(ui::muted(label))
                .child(icon(glyph, 12.0).text_color(move || theme().muted_foreground)),
        )
        .child(ui::figure(move || -> SharedString { kairo_core::speed(value()).into() }, size::DISPLAY).font_weight(FontWeight::SEMIBOLD))
}

fn traffic_card() -> impl IntoElement {
    let store = store();
    let width = signal(0.0_f32);
    let height = 120.0_f32;
    let graph = canvas(move || {
        let w = width.get();
        let down = color(theme().primary);
        let up = color(ui::graph_up());
        let base = color(theme().border);
        store.live.with(|live| {
            let h = height;
            let points: Vec<(u64, u64)> = live.history.iter().map(|t| (t.down, t.up)).collect();
            let peak = points.iter().map(|(d, u)| (*d).max(*u)).max().unwrap_or(0).max(16 * 1024) as f32 * 1.15;
            let n = kairo_core::service::HISTORY;
            let x = |i: usize, len: usize| w - (len - 1 - i) as f32 * (w / (n - 1) as f32);
            let y = |v: u64| h - 1.0 - (v as f32 / peak) * (h - 6.0);
            let mut p = Painter::new()
                .stroke_color(base)
                .line_width(1.0)
                .begin_path()
                .move_to(0.0, h - 0.5)
                .line_to(w, h - 0.5)
                .stroke();
            if points.len() < 2 || w <= 0.0 {
                return p;
            }
            let len = points.len();
            // Download: a soft area under its line.
            p = p.begin_path().move_to(x(0, len), h);
            for (i, (d, _)) in points.iter().enumerate() {
                p = p.line_to(x(i, len), y(*d));
            }
            p = p.line_to(x(len - 1, len), h).close_path().fill_color(down.opacity(0.12)).fill();
            p = p.begin_path().move_to(x(0, len), y(points[0].0));
            for (i, (d, _)) in points.iter().enumerate().skip(1) {
                p = p.line_to(x(i, len), y(*d));
            }
            p = p.stroke_color(down).line_width(1.75).line_join(LineJoin::Round).stroke();
            p = p.begin_path().move_to(x(0, len), y(points[0].1));
            for (i, (_, u)) in points.iter().enumerate().skip(1) {
                p = p.line_to(x(i, len), y(*u));
            }
            p.stroke_color(up).line_width(1.5).line_join(LineJoin::Round).stroke()
        })
    })
    .h(px(height))
    .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0));

    ui::card()
        .p_4()
        .gap_4()
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_end()
                .gap_x_8()
                .gap_y_3()
                .child(speed_figure(Icon::ArrowDown, "Download", || theme().primary, move || store.live.with(|l| l.down)))
                .child(speed_figure(Icon::ArrowUp, "Upload", ui::graph_up, move || store.live.with(|l| l.up)))
                .child(view().flex_1())
                .child(stat("Downloaded", move || kairo_core::bytes(store.live.with(|l| l.download_total)).into()))
                .child(stat("Uploaded", move || kairo_core::bytes(store.live.with(|l| l.upload_total)).into()))
                .child(stat("Connections", move || store.live.with(|l| l.connections).to_string().into()))
                .child(stat("Memory", move || {
                    let m = store.live.with(|l| l.memory);
                    if m == 0 { "—".into() } else { kairo_core::bytes(m).into() }
                })),
        )
        .child(graph)
}

fn when_text(at: Option<i64>) -> String {
    let Some(at) = at else { return "never".into() };
    let ago = kairo_core::profile::now() - at;
    match ago {
        i64::MIN..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", ago / 60),
        3600..=86399 => format!("{} h ago", ago / 3600),
        _ => format!("{} d ago", ago / 86400),
    }
}

pub fn expiry_text(expire: Option<i64>) -> String {
    match expire {
        None => "No expiry".into(),
        Some(t) => {
            let days = (t - kairo_core::profile::now()) / 86400;
            let date = date_of(t);
            if days < 0 { format!("Expired {date}") } else { format!("Expires {date} · {days} d left") }
        }
    }
}

/// `YYYY-MM-DD` of a unix time (local).
pub fn date_of(t: i64) -> String {
    let days = (t + kairo_core::service::local_offset_seconds()).div_euclid(86400);
    // Civil from days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

fn profile_card() -> View {
    let store = store();
    let active = move || store.state.with(|s| s.profiles.active_profile().cloned());
    ui::card().p_4().gap_3().child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(ui::section_label("Profile").flex_1())
            .child(ui::icon_btn(Icon::ChevronRight, "Open Profiles").size(px(22.0)).on_press(move || store.page.set(Page::Profiles))),
    )
    .child(dynamic(move || {
        let p = active();
        let updating = store.state.with(|s| p.as_ref().is_some_and(|p| s.updating.contains(&p.id)));
        untracked(move || match p {
            None => view()
                .gap_2()
                .child(ui::muted("No profile yet. Add a subscription URL or a config file."))
                .child(ui::btn_icon(Icon::Plus, "Add Profile…", Variant::Secondary).self_start().on_press(move || store.page.set(Page::Profiles)))
                .into_element(),
            Some(p) => {
                let usage = p.usage;
                let id = p.id.clone();
                view()
                    .gap_2()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(text(p.name.clone()).text_size(px(size::LARGE)).font_weight(FontWeight::SEMIBOLD).flex_1().number_of_lines(1))
                            .child(p.is_remote().then(|| {
                                ui::icon_btn(Icon::Refresh, "Update Profile").disabled(updating).on_press(move || {
                                    store.run_then(store.svc().update_profile(id.clone()), move |r| {
                                        if r.is_ok() {
                                            store.info("Profile updated");
                                        }
                                    })
                                })
                            })),
                    )
                    .child(usage.map(|u| {
                        let fraction = u.fraction().unwrap_or(0.0) as f32;
                        view()
                            .gap_1p5()
                            .child(ui::meter(move || fraction))
                            .child(
                                view()
                                    .flex_row()
                                    .child(ui::muted(if u.total > 0 {
                                        format!("{} of {}", kairo_core::bytes(u.used()), kairo_core::bytes(u.total))
                                    } else {
                                        format!("{} used", kairo_core::bytes(u.used()))
                                    }).tabular_nums().flex_1())
                                    .child(ui::muted(expiry_text(u.expire))),
                            )
                    }))
                    .child(ui::muted(format!(
                        "{} proxies · {} groups · {} rules · updated {}{}",
                        p.stats.proxies,
                        p.stats.groups,
                        p.stats.rules,
                        if updating { "now…".to_owned() } else { when_text(p.updated_at) },
                        p.last_error.as_ref().map(|e| format!(" · last update failed: {e}")).unwrap_or_default()
                    )))
                    .into_element()
            }
        })
    }))
}

fn group_card() -> View {
    let store = store();
    let main = move || {
        store.proxies.with(|p| {
            let tray = store.state.with(|s| s.settings.tray_group.clone());
            p.0.groups
                .iter()
                .filter(|g| g.selectable() && g.name != "GLOBAL" && !g.hidden)
                .find(|g| tray.is_empty() || g.name == tray)
                .or_else(|| p.0.groups.iter().find(|g| g.selectable() && g.name != "GLOBAL"))
                .cloned()
        })
    };
    ui::card()
        .p_4()
        .gap_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(ui::section_label("Proxy").flex_1())
                .child(ui::icon_btn(Icon::ChevronRight, "Open Proxies").size(px(22.0)).on_press(move || store.page.set(Page::Proxies))),
        )
        .child(dynamic(move || {
            let g = main();
            let mode = store.state.with(|s| s.settings.mode);
            untracked(move || match g {
                None => ui::muted("Groups appear when the core is running.").into_element(),
                Some(g) => {
                    let now = g.now.clone().unwrap_or_default();
                    let delay = g.members.iter().find(|m| m.name == now).and_then(|m| m.delay);
                    let name = g.name.clone();
                    let testing = move || store.proxies.with(|p| p.0.testing.contains(&name));
                    let group_name = g.name.clone();
                    view()
                        .gap_2()
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(text(now.clone()).text_size(px(size::LARGE)).font_weight(FontWeight::SEMIBOLD).flex_1().number_of_lines(1))
                                .child(text(ui::delay_text(delay)).text_size(px(size::BODY)).tabular_nums().text_color(move || ui::delay_tone(delay)))
                                .child(
                                    ui::icon_btn(Icon::Bolt, "Test Group")
                                        .disabled(testing)
                                        .on_press(move || store.run(store.svc().test_group(group_name.clone()))),
                                ),
                        )
                        .child(ui::muted(match mode {
                            Mode::Rule => format!("{} · {} · {} members", g.name, g.kind_title(), g.members.len()),
                            Mode::Global => "Global mode: everything goes through GLOBAL".into(),
                            Mode::Direct => "Direct mode: nothing goes through a proxy".into(),
                        }))
                        .into_element()
                }
            })
        }))
}
