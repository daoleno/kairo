//! Home: get online and choose where traffic goes. The first run is a
//! welcome that takes the provider's link; after that, the connection is the
//! page: one switch, the route, and the apps you use: AI first, then a card
//! for each kind used this week, with what its check measured. Everything
//! else is Advanced.

use kairo_core::kinds::{self, Kind};
use kairo_core::{CoreStatus, Mode};

use crate::pages::kinds::{AppRow, Tone};
use nsh_base::{ButtonElement as Button, theme};
use nsh_core::prelude::*;

use crate::i18n::plural;
use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, size};

pub fn page() -> impl IntoElement {
    let store = store();
    // Only a change of this rebuilds the page (not every state update).
    let has_profile = computed(move || store.state.with(|s| s.profiles.active.is_some()));
    view()
        .flex_1()
        .child(toolbar::floating(Page::Overview, String::new, vec![]))
        .child(dynamic(move || {
            if has_profile.get() {
                untracked(home).into_element()
            } else {
                untracked(welcome).into_element()
            }
        }))
        .child(crate::pages::kinds::sheet_host())
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
                problem.set(Some(kairo_core::tr!("home.welcome.no_link")));
            }
        });
    };
    let label = move || -> SharedString {
        if busy.get() {
            t!("home.connecting").into()
        } else if !valid() && clip_has_text.get() {
            t!("home.welcome.paste_connect").into()
        } else {
            t!("home.connect").into()
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
                    .child(text(t!("home.welcome.title")).text_size(px(size::display())).font_weight(FontWeight::SEMIBOLD).role(AccessibilityRole::Header)),
            )
            .child(
                ui::field(link, t!("home.welcome.link_placeholder"))
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
                    .child(ui::link_btn(t!("home.welcome.import_file")).on_press(move || store.page.set(Page::Profiles))),
            ),
    )
}

fn is_link(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("https://") || s.starts_with("http://"))
        && s.len() > 10
        && !s.contains(char::is_whitespace)
}

/// What went wrong, in plain words, with the way out.
fn problem_box(
    message: impl Fn() -> String + Copy + 'static,
    retry: impl Fn() + Copy + 'static,
) -> View {
    let explained = move || explain(&message());
    view()
        .gap_3()
        .p_4()
        .rounded(px(18.0))
        .bg(move || theme().destructive.opacity(0.08))
        .child(
            text(move || explained().0)
                .text_size(px(size::body()))
                .font_weight(FontWeight::MEDIUM),
        )
        .child(when(
            move || explained().1.is_some(),
            move || ui::muted(move || explained().1.unwrap_or_default()),
        ))
        .child(
            view()
                .flex_row()
                .child(ui::capsule(t!("home.try_again"), false).on_press(move || retry())),
        )
}

/// A failure in the user's words: the headline and a hint. (It reads the
/// engine's error text, in either language; it is `tr!`, profiles' tests
/// reach it.)
pub fn explain(e: &str) -> (String, Option<String>) {
    use kairo_core::profile::{Failure, failure_of};
    use kairo_core::tr;
    if let Some(said) = kairo_core::profile::provider_notice(e) {
        return (
            tr!("home.problem.provider_says", said = said),
            Some(tr!("home.problem.provider_says_hint")),
        );
    }
    let low = e.to_lowercase();
    if low.contains("http 401")
        || low.contains("http 403")
        || low.contains("http 404")
        || low.contains("http 410")
    {
        return (
            tr!("home.problem.refused"),
            Some(tr!("home.problem.refused_hint", error = e)),
        );
    }
    let failure = failure_of(e);
    if failure == Some(Failure::Unreachable) || low.contains("dns") || low.contains("resolve") {
        return (
            tr!("home.problem.unreachable"),
            Some(match crate::ui::machine() {
                Some(m) => tr!("home.problem.check_online", machine = m),
                None if cfg!(target_os = "android") => tr!("home.problem.check_online_phone"),
                None => tr!("home.problem.check_online_computer"),
            }),
        );
    }
    if failure == Some(Failure::NodeList) {
        return (
            tr!("home.problem.node_list"),
            Some(tr!("home.problem.node_list_hint")),
        );
    }
    if failure == Some(Failure::BadConfig) {
        return (
            tr!("home.problem.bad_config"),
            Some(tr!("home.problem.bad_config_hint", error = e)),
        );
    }
    (e.to_owned(), None)
}

// --- Home ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum Conn {
    Off,
    Connecting(Phase),
    On,
    Failed(String),
}

/// What the switch is waiting for.
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Updating,
    Locating,
    Starting,
    Stopping,
}

impl Phase {
    fn label(self) -> String {
        match self {
            Phase::Updating => t!("home.state.updating"),
            Phase::Locating => t!("home.state.locating"),
            Phase::Starting => t!("home.connecting"),
            Phase::Stopping => t!("home.state.disconnecting"),
        }
    }
}

fn conn() -> Conn {
    let store = store();
    store.state.with(|s| match &s.core {
        CoreStatus::Running { .. } if s.task.is_some() => Conn::Connecting(Phase::Updating),
        CoreStatus::Running { .. } => {
            if s.locating {
                Conn::Connecting(Phase::Locating)
            } else {
                Conn::On
            }
        }
        CoreStatus::Starting => Conn::Connecting(Phase::Starting),
        CoreStatus::Stopping => Conn::Connecting(Phase::Stopping),
        CoreStatus::Failed { message } => Conn::Failed(message.clone()),
        CoreStatus::Stopped => Conn::Off,
    })
}

/// The place the main group goes through now.
fn home() -> impl IntoElement {
    scroll().flex_1().child(
        view()
            .px(px(if ui::phone() { 16.0 } else { 24.0 }))
            .pt(toolbar::scroll_top(20.0))
            .pb_8()
            .gap_4()
            .w_full()
            .max_w(px(1040.0))
            .self_center()
            .child(hero())
            .child(apps_you_use()),
    )
}

/// The switch, what it means now, and the two choices that shape it.
fn hero() -> impl IntoElement {
    let store = store();
    let state = computed(conn);
    let last_press = signal(None::<(bool, nsh_core::time::Instant)>);
    let on = move || {
        matches!(state.get(), Conn::On | Conn::Connecting(_))
            && !matches!(state.get(), Conn::Connecting(Phase::Stopping))
    };
    let headline = move || -> SharedString {
        match state.get() {
            Conn::Off => t!("home.state.off").into(),
            Conn::Connecting(what) => what.label().into(),
            Conn::On => t!("home.state.on").into(),
            Conn::Failed(_) => t!("home.state.failed").into(),
        }
    };
    let detail = move || -> SharedString {
        match state.get() {
            Conn::On | Conn::Connecting(_) | Conn::Off => plan_line().into(),
            Conn::Failed(m) => explain(&m).0.into(),
        }
    };
    let phone = ui::phone();
    ui::card_soft()
        .px(px(if phone { 16.0 } else { 24.0 }))
        .py_5()
        .gap_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_4()
                .child(big_switch(on, move |_| {
                    // A second press soon after the first undoes it, whatever the
                    // screen shows yet: the last press wins (the service orders
                    // them). Later, the screen is the truth.
                    let next = next_press(
                        last_press.get(),
                        on(),
                        matches!(state.get(), Conn::Failed(_)),
                    );
                    last_press.set(Some((next, nsh_core::time::Instant::now())));
                    store.run(store.svc().connect(next));
                }))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap_0p5()
                        .child(
                            text(headline)
                                .text_size(px(size::title() + 2.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .role(AccessibilityRole::Header),
                        )
                        .child(ui::muted(detail).tabular_nums().number_of_lines(3)),
                )
                .child(when(move || !phone, choices)),
        )
        // On a phone the choices go under the switch, the width being short.
        .child(when(move || phone, choices))
        .child(when(
            move || matches!(state.get(), Conn::Failed(_)),
            move || {
                // The reason is the line above; this is only the advice, if any.
                let hint = move || match state.get() {
                    Conn::Failed(m) => explain(&m).1.unwrap_or_default(),
                    _ => String::new(),
                };
                view()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        view()
                            .flex_1()
                            .child(when(move || !hint().is_empty(), move || ui::muted(hint))),
                    )
                    .child(fix_button(
                        move || match state.get() {
                            Conn::Failed(m) => m,
                            _ => String::new(),
                        },
                        true,
                    ))
            },
        ))
        // A problem it keeps running through (a port in use): said quietly,
        // with the way out.
        .child(when(
            move || matches!(state.get(), Conn::On) && store.state.with(|s| s.warning.is_some()),
            move || {
                let warning = move || store.state.with(|s| s.warning.clone().unwrap_or_default());
                view()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(ui::dot(ui::warning))
                    .child(ui::muted(move || -> SharedString { warning().into() }).flex_1())
                    .child(fix_button(warning, false))
            },
        ))
        // Something Kairo did for the user (moved past a port another
        // program holds): one quiet line.
        .child(when(
            move || matches!(state.get(), Conn::On) && store.state.with(|s| s.note.is_some()),
            move || {
                ui::muted(move || -> SharedString {
                    store
                        .state
                        .with(|s| s.note.clone().unwrap_or_default())
                        .into()
                })
            },
        ))
}

/// "Nebula · 37 of 100 GB · 22 days left": the subscription in use.
pub fn plan_line() -> String {
    let store = store();
    store.state.with(|s| {
        let Some(p) = s.profiles.active_profile() else {
            return String::new();
        };
        plan_text(
            &p.name,
            p.usage.as_ref().map(|u| (u.used(), u.total, u.expire)),
            kairo_core::profile::now(),
        )
    })
}

/// The plan line from its parts: name, (used, total, expiry), now.
pub fn plan_text(name: &str, usage: Option<(u64, u64, Option<i64>)>, now: i64) -> String {
    let mut parts = vec![name.to_owned()];
    if let Some((used, total, expire)) = usage {
        if total > 0 {
            parts.push(kairo_core::tr!(
                "home.plan.usage",
                used = kairo_core::bytes(used),
                total = kairo_core::bytes(total)
            ));
        }
        if let Some(t) = expire {
            let days = (t - now) / 86400;
            parts.push(if t < now {
                kairo_core::tr!("home.plan.expired")
            } else {
                kairo_core::tr!(&plural("home.plan.days_left", days as u64), count = days)
            });
        }
    }
    parts.join(" · ")
}

/// The ports a message names (`Port 7897 …`, `127.0.0.1:7874: bind …`).
fn ports_in(message: &str) -> Vec<u16> {
    message
        .split(|c: char| !c.is_ascii_digit())
        .filter(|d| (2..=5).contains(&d.len()))
        .filter_map(|d| d.parse().ok())
        .collect()
}

/// The way out of a failure, as one button.
/// `failed`: the core is down (a way out is always offered); otherwise it
/// runs with a warning, and a button shows only when Kairo can fix it.
fn fix_button(message: impl Fn() -> String + Copy + 'static, failed: bool) -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let m = message().to_lowercase();
        let (mixed, controller_port) = store
            .state
            .with(|s| (s.settings.mixed_port, s.settings.controller_port));
        // Only Kairo's own ports are Kairo's to move: a clash on a port a
        // profile's override opened is not fixed by changing the proxy port.
        let ours = ports_in(&m)
            .iter()
            .any(|p| *p == mixed || *p == controller_port);
        untracked(move || {
            if ours
                && (m.contains("address already in use")
                    || kairo_core::failure::port_taken(&m)
                    || m.contains("port") && m.contains("in use"))
            {
                let controller = m.contains("controller port");
                ui::capsule(t!("home.fix.free_port"), true)
                    .on_press(move || {
                        let mut next = store.svc().settings();
                        // The port that is busy moves; the other stays.
                        let (from_mixed, from_controller) = if controller {
                            (next.mixed_port, next.controller_port + 1)
                        } else {
                            (next.mixed_port + 1, next.controller_port)
                        };
                        let free = store.svc().free_ports(from_mixed, from_controller);
                        store.run_then(free, move |r| {
                            let Ok((mixed, controller_port)) = r else { return };
                            if controller {
                                next.controller_port = controller_port;
                            } else {
                                next.mixed_port = mixed;
                            }
                            store.run_then(store.svc().update_settings(next), move |r| {
                                if r.is_ok() {
                                    store.run(store.svc().connect(true));
                                }
                            });
                        });
                    })
                    .into_element()
            } else if kairo_core::failure::tun_needs_permission(&m)
                || m.contains("tun") && (m.contains("permission") || m.contains("privilege"))
            {
                ui::capsule(t!("home.fix.allow_tun"), true)
                    .on_press(move || store.run(store.svc().grant_tun()))
                    .into_element()
            } else if failed {
                ui::capsule(t!("home.try_again"), true)
                    .on_press(move || store.run(store.svc().connect(true)))
                    .into_element()
            } else {
                view().into_element()
            }
        })
    })
}

/// A large switch: the page's one decision.
fn big_switch(
    on: impl Fn() -> bool + Copy + 'static,
    set: impl Fn(bool) + Copy + 'static,
) -> Button {
    let (w, h, pad) = (58.0_f32, 34.0_f32, 3.0_f32);
    let knob = h - pad * 2.0;
    nsh_base::button()
        .role(AccessibilityRole::Switch)
        .accessibility_label(t!("home.connect"))
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

/// Route and the way apps reach Kairo: two small controls.
fn choices() -> impl IntoElement {
    let store = store();
    let mode = move || store.state.with(|s| s.settings.mode);
    let via = move || {
        store.state.with(|s| {
            if s.settings.tun {
                Via::Tun
            } else {
                Via::System
            }
        })
    };
    let phone = ui::phone();
    let remote = store.svc().is_remote();
    view()
        .gap_2()
        .flex_shrink_0()
        .child(labelled(
            t!("home.mode.title"),
            ui::segmented(
                vec![
                    (Mode::Rule, t!("home.mode.smart")),
                    (Mode::Global, t!("home.mode.global")),
                    (Mode::Direct, t!("home.mode.direct")),
                ],
                mode,
                move |m| store.run(store.svc().set_mode(m)),
            ),
        ))
        // On Android every app is in the VPN: nothing to choose (a server's
        // page, phone-sized or not, chooses for the server).
        .child(when(
            move || !phone || remote,
            move || {
                // A server's page switches the server's apps.
                labelled(
                    match ui::machine() {
                        Some(m) => SharedString::from(t!("home.via.apps_on", machine = m)),
                        None => t!("home.via.apps").into(),
                    },
                    ui::segmented(
                        vec![(Via::System, t!("home.via.system")), (Via::Tun, t!("home.via.tun"))],
                        via,
                        move |v| set_via(v),
                    ),
                )
            },
        ))
}

fn labelled(label: impl IntoReactive<SharedString>, control: impl IntoElement) -> impl IntoElement {
    let phone = ui::phone();
    // On a phone the label and its control wrap rather than push the label off the edge.
    let row = view().flex_row().flex_wrap().items_center().gap_3();
    let row = if phone { row.justify_start() } else { row.justify_end() };
    row.child(ui::muted(label)).child(control)
}

/// How apps reach Kairo: those that follow the system's proxy settings, or every
/// app (TUN).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    System,
    Tun,
}

/// What a press of the big switch asks for. A second press soon after the
/// first undoes it, whatever the screen shows yet (the core takes a moment to
/// stop or start): the last press wins. After a while, or after a failure,
/// the screen is the truth.
fn next_press(last: Option<(bool, nsh_core::time::Instant)>, shown_on: bool, failed: bool) -> bool {
    match last {
        Some((v, at)) if at.elapsed() < std::time::Duration::from_secs(3) && !failed => !v,
        _ => !shown_on,
    }
}

/// Switches to `via`; TUN asks for its permission the first time.
pub fn set_via(via: Via) {
    let store = store();
    let svc = store.svc();
    match via {
        Via::System => store.run(async move {
            svc.set_tun(false).await?;
            if svc.snapshot().sysproxy_supported {
                svc.set_system_proxy(true).await?;
            }
            Ok(())
        }),
        Via::Tun => {
            let granted = store.state.with_untracked(|s| s.tun_privilege)
                == Some(kairo_core::tun::Privilege::Granted);
            store.run(async move {
                if !granted {
                    svc.grant_tun().await?;
                }
                svc.set_tun(true).await
            })
        }
    }
}

/// Apps you use: the AI table, then a card for each other kind used in the
/// last 7 days (at least [`kinds::USED_BYTES`], or a connection open now),
/// at most three apps each, troubles first.
fn apps_you_use() -> impl IntoElement {
    let store = store();
    let rows = crate::pages::kinds::all_rows(kinds::USED_BYTES);
    let checking = move || store.state.with(|s| s.ai.checking || s.kinds.checking);
    let ai_used = move || rows.with(|r| r.iter().any(|a| a.kind == Kind::Ai));
    let any = move || rows.with(|r| !r.is_empty());
    view()
        .gap_3()
        .child(when(any, move || {
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .px_1()
                .child(ui::section_label(t!("home.apps.title")).flex_1())
                .child(
                    ui::capsule(
                        move || -> SharedString { if checking() { t!("kinds.checking").into() } else { t!("kinds.check_now").into() } },
                        false,
                    )
                    .h(px(28.0))
                    .px_4()
                    .disabled(checking)
                    .on_press(move || {
                        store.run(store.svc().check_ai());
                        store.run(store.svc().check_apps());
                    }),
                )
        }))
        .child(when(ai_used, crate::pages::ai::board))
        .child(dynamic(move || {
            let all = rows.get();
            untracked(move || {
                let cards: Vec<AnyElement> = [Kind::Streaming, Kind::Games, Kind::Downloads, Kind::DevTools]
                    .into_iter()
                    .filter_map(|k| {
                        let mut list: Vec<AppRow> = all.iter().filter(|r| r.kind == k).cloned().collect();
                        if list.is_empty() {
                            return None;
                        }
                        // Troubles first; the rest by traffic (as they come).
                        list.sort_by_key(|r| !(matches!(r.tone, Tone::Bad | Tone::Look) || r.fix.is_some()));
                        Some(kind_card(k, list))
                    })
                    .collect();
                let phone = ui::phone();
                let mut out = view().gap_3();
                let mut cards = cards.into_iter().peekable();
                while cards.peek().is_some() {
                    if phone {
                        out = out.child(cards.next());
                        continue;
                    }
                    let a = cards.next();
                    let b = cards.next();
                    out = out.child(
                        view()
                            .flex_row()
                            .items_start()
                            .gap_3()
                            .child(view().flex_1().min_w(px(0.0)).child(a))
                            .child(view().flex_1().min_w(px(0.0)).child(b)),
                    );
                }
                out.into_element()
            })
        }))
}

/// The summary in a kind card's head, and whether it is a problem.
fn kind_summary(rows: &[AppRow]) -> (String, Tone) {
    let problems = rows.iter().filter(|r| matches!(r.tone, Tone::Bad | Tone::Look) || r.fix.is_some()).count();
    let bad = rows.iter().any(|r| r.tone == Tone::Bad);
    if problems > 0 {
        let text = kairo_core::tr!(&plural("home.apps.problems", problems as u64), count = problems);
        return (text, if bad { Tone::Bad } else { Tone::Look });
    }
    let ok = rows.iter().filter(|r| r.tone == Tone::Ok).count();
    match ok {
        0 => (kairo_core::tr!("kinds.not_checked"), Tone::Idle),
        n if n == rows.len() => (kairo_core::tr!("home.apps.all_reachable"), Tone::Ok),
        n => (kairo_core::tr!("home.apps.some_reachable", n = n, count = rows.len()), Tone::Ok),
    }
}

/// A kind on Home: where it leaves, and its apps (three at most).
fn kind_card(kind: Kind, rows: Vec<AppRow>) -> AnyElement {
    let store = store();
    let (summary, tone) = kind_summary(&rows);
    let more = rows.len().saturating_sub(3);
    let place = crate::pages::kinds::place_of(kind);
    let flag = place.as_ref().and_then(|p| p.flag.clone()).unwrap_or_else(|| "◎".into());
    ui::card_soft()
        .child(
            nsh_base::button()
                .accessibility_label(kind.title())
                .on_press(move || crate::pages::kinds::open_kind(kind))
                .flex_row()
                .items_center()
                .gap_2()
                .px_5()
                .pt_4()
                .pb_2()
                .cursor_pointer()
                .child(ui::body(kind.title()).font_weight(FontWeight::SEMIBOLD))
                .child(ui::flag(flag, size::body() + 2.0))
                .child(ui::muted(crate::pages::kinds::subtitle(kind)).text_size(px(size::small())).number_of_lines(1).flex_1())
                .child(ui::dot(move || crate::pages::kinds::tone_color(tone)))
                .child(ui::muted(summary).text_size(px(size::small()))),
        )
        .children(rows.into_iter().take(3).map(move |r| {
            let id = r.id.clone();
            let t = r.tone;
            nsh_base::button()
                .accessibility_label(r.name.clone())
                .on_press(move || store.app_sheet.set(Some(id.clone())))
                .flex_row()
                .items_center()
                .gap_3()
                .px_5()
                .min_h(px(ui::touch_h(40.0)))
                .cursor_pointer()
                .hover(|s| s.bg(move || theme().accent))
                .child(r.logo(22.0))
                .child(ui::body(r.name.clone()).number_of_lines(1).flex_1().min_w(px(64.0)))
                .child(ui::dot(move || crate::pages::kinds::tone_color(t)))
                .child(ui::body(r.status.clone()).text_size(px(size::small())).number_of_lines(1).flex_shrink(1.0))
                .child(r.fix.clone().map(|f| {
                    let fx = f.clone();
                    ui::capsule(f.title(), matches!(f, crate::pages::kinds::Fix::UseEveryApp))
                        .h(px(24.0))
                        .px_3()
                        .on_press(move || fx.apply(store))
                }))
                .into_element()
        }))
        .child((more > 0).then(|| {
            ui::muted(t!("home.apps.more", count = more)).text_size(px(size::small())).px_5().pb_3()
        }))
        .child(view().h(px(8.0)))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn subscription_failures_are_explained_in_either_language() {
        use kairo_core::i18n::text_in;
        for lang in ["en", "zh"] {
            let error = text_in(lang, "model.profile.download", &[("error", &text_in(lang, "model.profile.timed_out", &[]))]);
            assert_eq!(explain(&error).0, kairo_core::tr!("home.problem.unreachable"), "{error}");
            let error = text_in(lang, "model.profile.node_list", &[]);
            assert_eq!(explain(&error).0, kairo_core::tr!("home.problem.node_list"), "{error}");
            let error = text_in(lang, "model.profile.yaml", &[("error", &"mapping values are not allowed")]);
            assert_eq!(explain(&error).0, kairo_core::tr!("home.problem.bad_config"), "{error}");
            let error = text_in(lang, "model.profile.no_proxies", &[]);
            assert_eq!(explain(&error).0, kairo_core::tr!("home.problem.bad_config"), "{error}");
            let error = text_in(lang, "model.profile.notice", &[("notice", &"Plan expired")]);
            assert_eq!(
                explain(&error).0,
                kairo_core::tr!("home.problem.provider_says", said = "Plan expired"),
                "{error}"
            );
        }
        // Not Kairo's words: the text as it is.
        assert_eq!(explain("something else").0, "something else");
    }

    #[test]
    fn a_quick_second_press_undoes_the_first_whatever_the_screen_shows() {
        // Pressed off a moment ago; the screen still says on (the core has not
        // stopped yet): the second press asks for on, not off again.
        let first = (false, Instant::now() - Duration::from_millis(130));
        assert!(next_press(Some(first), true, false));
        // And once more: on, then off, before the screen caught up either time.
        let second = (true, Instant::now() - Duration::from_millis(130));
        assert!(!next_press(Some(second), false, false));
    }

    #[test]
    fn later_or_after_a_failure_the_screen_is_the_truth() {
        let old = (false, Instant::now() - Duration::from_secs(10));
        assert!(!next_press(Some(old), true, false));
        assert!(next_press(None, false, false));
        let recent = (false, Instant::now() - Duration::from_millis(130));
        // A failed connect is on screen as failed (off): pressing asks for on.
        assert!(next_press(Some(recent), false, true));
    }
}
