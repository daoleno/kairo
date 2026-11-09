//! Home: get online and choose where traffic goes. The first run is a
//! welcome that takes the provider's link; after that, the connection is the
//! page: one switch, the place traffic goes through, the route, the apps and
//! sites that go elsewhere, and what is flowing. Everything else is Advanced.

use kairo_core::places::{self, Place};
use kairo_core::{CoreStatus, Mode};
use nsh_base::{ButtonElement as Button, popover, theme};
use nsh_core::canvas::{LineCap, LineJoin};
use nsh_core::prelude::*;

use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, icon, size};

pub fn page() -> impl IntoElement {
    let store = store();
    // Only a change of this rebuilds the page (not every state update).
    let has_profile = computed(move || store.state.with(|s| s.profiles.active.is_some()));
    view().flex_1().child(toolbar::floating(Page::Overview, String::new, vec![])).child(dynamic(move || {
        if has_profile.get() { untracked(home).into_element() } else { untracked(welcome).into_element() }
    }))
}

// --- Welcome ------------------------------------------------------------------------

/// The first run: a link in, connected out.
fn welcome() -> impl IntoElement {
    let store = store();
    let link = signal(String::new());
    let busy = signal(false);
    let problem = signal(None::<String>);
    let clip_has_text = signal(false);
    spawn(async move {
        clip_has_text.set(clipboard_has_string().await);
    });
    let valid = move || link.with(|u| is_link(u));
    let connect = move |url: String| {
        busy.set(true);
        problem.set(None);
        let svc = store.svc();
        spawn(async move {
            let r = svc.add_remote(url, None).await;
            busy.set(false);
            match r {
                Ok(_) => store.run(store.svc().connect(true)),
                Err(e) => problem.set(Some(e)),
            }
        });
    };
    let primary = move || {
        if valid() {
            connect(link.get_untracked().trim().to_owned());
            return;
        }
        // Nothing typed: what the user copied.
        spawn(async move {
            let text = clipboard_string().await;
            let text = text.trim().to_owned();
            if is_link(&text) {
                link.set(text.clone());
                connect(text);
            } else {
                problem.set(Some("The clipboard holds no link. Copy the subscription link from your provider, or paste it here.".into()));
            }
        });
    };
    let label = move || -> SharedString {
        if busy.get() {
            "Connecting…".into()
        } else if !valid() && clip_has_text.get() {
            "Paste and Connect".into()
        } else {
            "Connect".into()
        }
    };
    scroll().flex_1().child(
        view()
            .px_8()
            .pt(toolbar::scroll_top(56.0))
            .pb_8()
            .gap_6()
            .w_full()
            .max_w(px(520.0))
            .self_center()
            .child(
                view()
                    .gap_2()
                    .child(text("Welcome to Kairo").text_size(px(size::display())).font_weight(FontWeight::SEMIBOLD).role(AccessibilityRole::Header))
                    .child(ui::muted("Paste the subscription link from your provider. Kairo finds the fastest location and connects.").text_size(px(size::body() + 1.0))),
            )
            .child(
                ui::field(link, "https://… (your provider's subscription link)")
                    .auto_focus(true)
                    .h(px(44.0))
                    .px_4()
                    .rounded_full()
                    .text_size(px(size::body() + 1.0))
                    .on_submit(move |_: &str| primary()),
            )
            .child(when(
                move || problem.with(|p| p.is_some()),
                move || problem_box(move || problem.get().unwrap_or_default(), move || primary()),
            ))
            .child(
                view().flex_row().items_center().gap_3().child(view().flex_1()).child(
                    ui::capsule(label, true)
                        .disabled(move || busy.get())
                        .on_press(move || primary()),
                ),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .self_center()
                    .mt_4()
                    .child(ui::muted("Have a config file instead?"))
                    .child(ui::link_btn("Import File…").on_press(move || store.page.set(Page::Profiles))),
            ),
    )
}

fn is_link(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("https://") || s.starts_with("http://")) && s.len() > 10 && !s.contains(char::is_whitespace)
}

/// What went wrong, in plain words, with the way out.
fn problem_box(message: impl Fn() -> String + Copy + 'static, retry: impl Fn() + Copy + 'static) -> View {
    let explained = move || explain(&message());
    view()
        .gap_3()
        .p_4()
        .rounded(px(18.0))
        .bg(move || theme().destructive.opacity(0.08))
        .child(text(move || explained().0).text_size(px(size::body())).font_weight(FontWeight::MEDIUM))
        .child(when(move || explained().1.is_some(), move || ui::muted(move || explained().1.unwrap_or_default())))
        .child(view().flex_row().child(ui::capsule("Try Again", false).on_press(move || retry())))
}

/// A failure in the user's words: the headline and a hint.
pub fn explain(e: &str) -> (String, Option<String>) {
    if let Some(said) = e.strip_prefix("Your provider says: ") {
        return (
            format!("Your provider says: “{said}”"),
            Some("The link works, but the provider sent a message instead of locations. Do what it asks (often: allow fetching on its website), then try again.".into()),
        );
    }
    let low = e.to_lowercase();
    if low.contains("http 401") || low.contains("http 403") || low.contains("http 404") || low.contains("http 410") {
        return (
            "The provider refused this link.".into(),
            Some(format!("It may have expired or been reset ({e}). Copy a fresh link from your provider.")),
        );
    }
    if low.contains("timed out") || low.contains("could not connect") || low.contains("dns") || low.contains("resolve") {
        return ("Kairo can't reach the provider.".into(), Some("Check that this Mac or PC is online, then try again.".into()));
    }
    if low.contains("base64 node list") {
        return (
            "This link gives a plain node list, not a Clash/mihomo config.".into(),
            Some("Ask your provider for the Clash or mihomo link (many have a “Clash” button next to it).".into()),
        );
    }
    if low.contains("not valid yaml") || low.contains("no proxies") {
        return ("This link doesn't give a config Kairo can use.".into(), Some(format!("Check you copied the whole link. ({e})")));
    }
    (e.to_owned(), None)
}

// --- Home ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum Conn {
    Off,
    Connecting(&'static str),
    On,
    Failed(String),
}

fn conn() -> Conn {
    let store = store();
    let testing = store.proxies.with(|p| !p.0.testing.is_empty());
    store.state.with(|s| match &s.core {
        CoreStatus::Running { .. } if s.task.is_some() => Conn::Connecting("Updating…"),
        CoreStatus::Running { .. } => {
            // Automatic and no place measured yet: the first test is running (or about to).
            if s.settings.auto_location && s.profiles.active.is_some() && (testing || current_place().is_none_or(|p| p.delay.is_none())) && current_place().is_none_or(|p| p.delay.is_none()) {
                Conn::Connecting("Finding the fastest location…")
            } else {
                Conn::On
            }
        }
        CoreStatus::Starting => Conn::Connecting("Connecting…"),
        CoreStatus::Stopping => Conn::Connecting("Disconnecting…"),
        CoreStatus::Failed { message } => Conn::Failed(message.clone()),
        CoreStatus::Stopped => Conn::Off,
    })
}

/// The place the main group goes through now.
fn current_place() -> Option<Place> {
    let store = store();
    let preferred = store.state.with(|s| s.settings.tray_group.clone());
    store.proxies.with(|p| {
        let g = places::main_group(&p.0.groups, &preferred)?;
        places::current(&p.0.groups, g)
    })
}

fn home() -> impl IntoElement {
    scroll().flex_1().child(
        view()
            .px_8()
            .pt(toolbar::scroll_top(28.0))
            .pb_8()
            .gap_5()
            .w_full()
            .max_w(px(600.0))
            .self_center()
            .child(hero())
            .child(choices())
            .child(traffic())
            .child(plan()),
    )
}

/// The switch and what it means now.
fn hero() -> impl IntoElement {
    let store = store();
    let state = computed(conn);
    let on = move || matches!(state.get(), Conn::On | Conn::Connecting(_)) && !matches!(state.get(), Conn::Connecting("Disconnecting…"));
    let headline = move || -> SharedString {
        match state.get() {
            Conn::Off => "Not Connected".into(),
            Conn::Connecting(what) => what.into(),
            Conn::On => "Connected".into(),
            Conn::Failed(_) => "Couldn't Connect".into(),
        }
    };
    let detail = move || -> SharedString {
        let auto = store.state.with(|s| s.settings.auto_location);
        let sysproxy_off = store.state.with(|s| s.sysproxy_supported && !s.settings.system_proxy && !s.settings.tun);
        match state.get() {
            Conn::On => match current_place() {
                Some(p) => {
                    let mut parts = vec![format!("{}{}", p.flag.map(|f| f + " ").unwrap_or_default(), p.title)];
                    if let Some(d) = p.delay.filter(|d| *d > 0) {
                        parts.push(format!("{d} ms"));
                    }
                    if auto {
                        parts.push("automatic".into());
                    }
                    if sysproxy_off {
                        parts.push("apps opt in by port".into());
                    }
                    parts.join(" · ").into()
                }
                None => "Through your subscription".into(),
            },
            Conn::Off => "Turn on to go through your subscription.".into(),
            Conn::Connecting(_) => "".into(),
            Conn::Failed(m) => explain(&m).0.into(),
        }
    };
    view()
        .items_center()
        .gap_3()
        .py_4()
        .child(
            big_switch(on, move |next| store.run(store.svc().connect(next)))
                .disabled(move || matches!(state.get(), Conn::Connecting(_))),
        )
        .child(text(headline).text_size(px(size::display())).font_weight(FontWeight::SEMIBOLD).role(AccessibilityRole::Header).mt_2())
        .child(ui::muted(detail).text_size(px(size::body() + 1.0)).text_center().number_of_lines(2))
        .child(when(
            move || matches!(state.get(), Conn::Failed(_)),
            move || {
                let hint = move || match state.get() {
                    Conn::Failed(m) => explain(&m).1.unwrap_or(m),
                    _ => String::new(),
                };
                view()
                    .items_center()
                    .gap_3()
                    .child(ui::muted(hint).text_center().max_w(px(440.0)))
                    .child(fix_button(move || match state.get() {
                        Conn::Failed(m) => m,
                        _ => String::new(),
                    }))
            },
        ))
}

/// The way out of a failure, as one button.
fn fix_button(message: impl Fn() -> String + Copy + 'static) -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let m = message().to_lowercase();
        untracked(move || {
            if m.contains("address already in use") || m.contains("port") && m.contains("in use") {
                ui::capsule("Use a Free Port", true)
                    .on_press(move || {
                        let mut next = store.svc().settings();
                        next.mixed_port = kairo_core::settings::free_port(next.mixed_port + 1, &[next.controller_port]);
                        store.run_then(store.svc().update_settings(next), move |r| {
                            if r.is_ok() {
                                store.run(store.svc().connect(true));
                            }
                        });
                    })
                    .into_element()
            } else if m.contains("tun") && (m.contains("permission") || m.contains("privilege")) {
                ui::capsule("Allow TUN…", true).on_press(move || store.run(store.svc().grant_tun())).into_element()
            } else {
                ui::capsule("Try Again", true).on_press(move || store.run(store.svc().connect(true))).into_element()
            }
        })
    })
}

/// A large switch: the page's one decision.
fn big_switch(on: impl Fn() -> bool + Copy + 'static, set: impl Fn(bool) + Copy + 'static) -> Button {
    let (w, h, pad) = (104.0_f32, 60.0_f32, 5.0_f32);
    let knob = h - pad * 2.0;
    nsh_base::button()
        .role(AccessibilityRole::Switch)
        .accessibility_label("Connect")
        .selected(on)
        .on_press(move || set(!on()))
        .w(px(w))
        .h(px(h))
        .p(px(pad))
        .rounded_full()
        .cursor_pointer()
        .bg(move || if on() { theme().primary } else { ui::track() })
        .transition_colors()
        .focus(|s| s.outline(px(3.0)).outline_color(move || theme().ring))
        .disabled_style(|s| s.opacity(0.7))
        .child(
            view()
                .size(px(knob))
                .rounded_full()
                .bg(move || AdaptiveColor::from(white()))
                .shadow_md()
                .transition_transform()
                .translate_x(move || px(if on() { w - pad * 2.0 - knob } else { 0.0 })),
        )
}

/// Location, route, apps and sites.
fn choices() -> impl IntoElement {
    ui::rows(vec![location_row().into_element(), route_row().into_element(), apps_row().into_element()])
}

fn location_row() -> impl IntoElement {
    let store = store();
    let open = signal(false);
    let place = computed(current_place);
    let auto = move || store.state.with(|s| s.settings.auto_location);
    let value = move || -> SharedString {
        match place.get() {
            Some(p) => format!("{} · {}", p.title, ui::delay_text(p.delay)).into(),
            None => "—".into(),
        }
    };
    popover()
        .open(move || Some(open.get()))
        .on_open_change(move |e: &nsh_base::PopoverEvent| {
            if let nsh_base::PopoverEvent::OpenChange(next, _) = e {
                open.set(*next);
                if *next {
                    test_if_stale();
                }
            }
        })
        .placement(Placement::Bottom)
        .align(Align::End)
        .offset(px(6.0))
        .popup_class("transition-opacity duration-150")
        .popup_starting_class(Some(SharedString::from("opacity-0")))
        .trigger(
            ui::row_view()
                .cursor_pointer()
                .child(text(move || place.get().and_then(|p| p.flag).unwrap_or_else(|| "◎".into())).text_size(px(size::large() + 2.0)))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap_0p5()
                        .child(ui::body("Location"))
                        .child(ui::muted(move || -> SharedString { if auto() { "Fastest, automatically".into() } else { "Chosen by you".into() } })),
                )
                .child(ui::muted(value).tabular_nums().number_of_lines(1))
                .child(icon(Icon::ChevronDown, 12.0).text_color(move || theme().muted_foreground)),
        )
        .child(location_picker(move || open.set(false)))
}

/// The locations were never tested, or long ago: test them now.
fn test_if_stale() {
    let store = store();
    let (untested, total, testing) = store.proxies.with_untracked(|p| {
        let preferred = store.state.with_untracked(|s| s.settings.tray_group.clone());
        let Some(g) = places::main_group(&p.0.groups, &preferred) else { return (0, 0, true) };
        let all = places::places(&p.0.groups, g);
        (all.iter().filter(|p| p.delay.is_none()).count(), all.len(), !p.0.testing.is_empty())
    });
    if total > 0 && untested * 2 >= total && !testing {
        spawn(store.svc().test_places());
    }
}

fn location_picker(close: impl Fn() + Copy + 'static) -> impl IntoElement {
    let store = store();
    let query = signal(String::new());
    let preferred = move || store.state.with(|s| s.settings.tray_group.clone());
    let auto = move || store.state.with(|s| s.settings.auto_location);
    let regions = move || {
        let q = query.get().to_lowercase();
        store.proxies.with(|p| {
            let Some(g) = places::main_group(&p.0.groups, &preferred()) else { return (None, vec![], false) };
            let current = places::current(&p.0.groups, g).map(|c| c.name);
            let all: Vec<Place> = places::places(&p.0.groups, g).into_iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.region.to_lowercase().contains(&q)).collect();
            (current, places::by_region(&all), !p.0.testing.is_empty())
        })
    };
    let pick = move |name: Option<String>| {
        close();
        store.run(store.svc().set_location(name));
    };
    view()
        .w(px(360.0))
        .max_h(move || px((nsh_core::element::window().height.0 - 340.0).clamp(220.0, 520.0)))
        .rounded(px(22.0))
        .bg(move || theme().popover)
        .shadow_xl()
        .border_1()
        .border_color(move || theme().border.opacity(0.6))
        .overflow_hidden()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .p_3()
                .child(ui::search_field(query, "Search locations").flex_1())
                .child(
                    ui::icon_btn(Icon::Bolt, "Test Again").on_press(move || {
                        spawn(store.svc().test_places());
                    }),
                ),
        )
        .child(
            scroll().flex_shrink(1.0).child(
                view()
                    .px_2()
                    .pb_2()
                    .child(place_row(None, move || auto(), move || {
                        let fastest = store.proxies.with(|p| places::main_group(&p.0.groups, &preferred()).and_then(|g| places::fastest(&p.0.groups, g)));
                        fastest.map(|f| format!("{} · {}", f.title, ui::delay_text(f.delay))).unwrap_or_default()
                    }, pick))
                    .child(dynamic(move || {
                        let (now, regions, testing) = regions();
                        let manual = !auto();
                        untracked(move || {
                            view().children(regions.into_iter().map(move |(region, list)| {
                                let now = now.clone();
                                view()
                                    .child(ui::section_label(region).px_3().pt_3().pb_1())
                                    .children(list.into_iter().map(move |p| {
                                        let is_now = manual && now.as_deref() == Some(p.name.as_str());
                                        let name = p.name.clone();
                                        let delay = if testing && p.delay.is_none() { "…".to_owned() } else { ui::delay_text(p.delay).to_string() };
                                        place_line(p, is_now, delay).on_press(move || pick(Some(name.clone())))
                                    }))
                            }))
                        })
                    })),
            ),
        )
}

/// "Fastest": Kairo keeps it on the fastest place.
fn place_row(_: Option<()>, chosen: impl Fn() -> bool + 'static, detail: impl Fn() -> String + 'static, pick: impl Fn(Option<String>) + 'static) -> Button {
    nsh_base::button()
        .on_press(move || pick(None))
        .flex_row()
        .items_center()
        .gap_3()
        .h(px(40.0))
        .px_3()
        .rounded(px(12.0))
        .cursor_pointer()
        .hover(|s| s.bg(move || theme().accent))
        .child(icon(Icon::Bolt, 15.0).text_color(move || theme().primary))
        .child(ui::body("Fastest").font_weight(FontWeight::MEDIUM).flex_1())
        .child(ui::muted(detail).tabular_nums())
        .child(view().w(px(16.0)).child(when(chosen, || icon(Icon::Check, 14.0).text_color(move || theme().primary))))
}

fn place_line(p: Place, chosen: bool, delay: String) -> Button {
    let tone = ui::delay_tone(p.delay);
    nsh_base::button()
        .selected(chosen)
        .accessibility_label(p.name.clone())
        .flex_row()
        .items_center()
        .gap_3()
        .h(px(36.0))
        .px_3()
        .rounded(px(12.0))
        .cursor_pointer()
        .hover(|s| s.bg(move || theme().accent))
        .style_when(move || chosen, |s| s.bg(move || ui::selection()))
        .child(text(p.flag.clone().or_else(|| places::region_flag(p.region)).unwrap_or_else(|| "◎".into())).w(px(20.0)))
        .child(ui::body(p.title.clone()).flex_1().number_of_lines(1))
        .child(text(delay).text_size(px(size::small())).tabular_nums().text_color(move || tone))
        .child(view().w(px(16.0)).child(chosen.then(|| icon(Icon::Check, 14.0).text_color(move || theme().primary))))
}

fn route_row() -> impl IntoElement {
    let store = store();
    let mode = move || store.state.with(|s| s.settings.mode);
    let detail = move || -> SharedString {
        let place = current_place().map(|p| p.title).unwrap_or_else(|| "your location".into());
        match mode() {
            Mode::Rule => format!("Local sites go direct; the rest through {place}.").into(),
            Mode::Global => format!("Everything goes through {place}.").into(),
            Mode::Direct => "Everything goes direct, as without Kairo.".into(),
        }
    };
    ui::row_view()
        .child(view().flex_1().min_w(px(0.0)).gap_0p5().child(ui::body("Route")).child(ui::muted(detail).number_of_lines(2)))
        .child(ui::segmented(vec![(Mode::Rule, "Smart"), (Mode::Global, "All"), (Mode::Direct, "Direct")], mode, move |m| {
            store.run(store.svc().set_mode(m))
        }))
}

fn apps_row() -> impl IntoElement {
    let store = store();
    let summary = move || -> SharedString {
        let preferred = store.state.with(|s| s.settings.tray_group.clone());
        store.proxies.with(|p| {
            let Some(main) = places::main_group(&p.0.groups, &preferred) else { return "".into() };
            let apps = places::app_choices(&p.0.groups, &main.name);
            if apps.is_empty() {
                return "Your subscription has no per-app choices.".into();
            }
            let own = apps.iter().filter(|a| !a.follows_main).count();
            let names: Vec<&str> = apps.iter().take(3).map(|a| a.group.as_str()).collect();
            if own == 0 {
                format!("{} and {} more follow your location", names.join(", "), apps.len().saturating_sub(3)).into()
            } else {
                format!("{} follow your location · {own} go their own way", apps.len() - own).into()
            }
        })
    };
    nsh_base::button()
        .on_press(move || store.page.set(Page::Apps))
        .child(
            ui::row_view()
                .cursor_pointer()
                .child(view().flex_1().min_w(px(0.0)).gap_0p5().child(ui::body("Apps & Sites")).child(ui::muted(summary).number_of_lines(1)))
                .child(icon(Icon::ChevronRight, 13.0).text_color(move || theme().muted_foreground)),
        )
}

/// What is flowing: speeds and a soft line.
fn traffic() -> impl IntoElement {
    let store = store();
    let width = signal(0.0_f32);
    let height = 64.0_f32;
    let graph = canvas(move || {
        let w = width.get();
        let line = color(theme().primary);
        let quiet = color(ui::track());
        store.live.with(|live| {
            let h = height;
            let points: Vec<u64> = live.history.iter().map(|t| t.down + t.up).collect();
            let mut p = Painter::new();
            if points.len() < 2 || w <= 0.0 {
                return p;
            }
            let peak = points.iter().copied().max().unwrap_or(0).max(32 * 1024) as f32 * 1.2;
            // Nothing flowing: a quiet baseline, not the accent.
            let line = if points.iter().all(|v| *v == 0) { quiet } else { line };
            let n = kairo_core::service::HISTORY;
            let len = points.len();
            let x = |i: usize| w - (len - 1 - i) as f32 * (w / (n - 1) as f32);
            let y = |v: u64| h - 3.0 - (v as f32 / peak) * (h - 8.0);
            // A smooth line: midpoints joined by quadratic curves.
            p = p.begin_path().move_to(x(0), y(points[0]));
            for i in 1..len {
                let (px0, py0) = (x(i - 1), y(points[i - 1]));
                let (px1, py1) = (x(i), y(points[i]));
                p = p.quadratic_curve_to(point(px(px0), px(py0)), point(px((px0 + px1) / 2.0), px((py0 + py1) / 2.0)));
            }
            p = p.line_to(x(len - 1), y(points[len - 1]));
            p.stroke_color(line).line_width(2.5).line_cap(LineCap::Round).line_join(LineJoin::Round).stroke()
        })
    })
    .h(px(height))
    .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0));
    view()
        .gap_2()
        .px_1()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_4()
                .child(speed(Icon::ArrowDown, move || store.live.with(|l| l.down)))
                .child(speed(Icon::ArrowUp, move || store.live.with(|l| l.up)))
                .child(view().flex_1())
                .child(ui::link_btn("Activity").on_press(move || store.page.set(Page::Connections))),
        )
        .child(graph)
}

fn speed(glyph: Icon, value: impl Fn() -> u64 + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_1()
        .child(icon(glyph, 12.0).text_color(move || theme().muted_foreground))
        .child(text(move || kairo_core::speed(value())).text_size(px(size::small())).tabular_nums().text_color(move || theme().muted_foreground))
}

/// The subscription: usage, expiry and a failed update, quietly.
fn plan() -> impl IntoElement {
    let store = store();
    let profile = move || store.state.with(|s| s.profiles.active_profile().cloned());
    let updating = move || store.state.with(|s| profile().is_some_and(|p| s.updating.contains(&p.id)));
    dynamic(move || {
        let Some(p) = profile() else { return None };
        let busy = updating();
        Some(untracked(move || {
            let id = p.id.clone();
            let mut parts = vec![p.name.clone()];
            if let Some(u) = p.usage {
                if u.total > 0 {
                    parts.push(format!("{} of {}", kairo_core::bytes(u.used()), kairo_core::bytes(u.total)));
                }
                if let Some(t) = u.expire {
                    let days = (t - kairo_core::profile::now()) / 86400;
                    parts.push(if days < 0 { "expired".into() } else { format!("{days} days left") });
                }
            }
            let failed = p.last_error.clone();
            view()
                .gap_1()
                .px_1()
                .child(ui::muted(parts.join(" · ")).text_size(px(size::small())).tabular_nums())
                .child(failed.map(move |e| {
                    let id = id.clone();
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            ui::muted(format!("Couldn't update the subscription — using the saved copy. {}", explain(&e).0))
                                .text_size(px(size::small()))
                                .flex_1()
                                .number_of_lines(2),
                        )
                        .child(ui::link_btn(if busy { "Updating…" } else { "Retry" }).disabled(busy).on_press(move || {
                            store.run(store.svc().update_profile(id.clone()))
                        }))
                }))
        }))
    })
}
