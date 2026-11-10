//! Home: get online and choose where traffic goes. The first run is a
//! welcome that takes the provider's link; after that, the connection is the
//! page: one switch, the place traffic goes through, the route, the apps and
//! sites that go elsewhere, and what is flowing. Everything else is Advanced.

use kairo_core::{CoreStatus, Mode};
use nsh_base::{ButtonElement as Button, theme};
use nsh_core::prelude::*;

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
                .child(ui::capsule("Try Again", false).on_press(move || retry())),
        )
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
    if low.contains("http 401")
        || low.contains("http 403")
        || low.contains("http 404")
        || low.contains("http 410")
    {
        return (
            "The provider refused this link.".into(),
            Some(format!(
                "It may have expired or been reset ({e}). Copy a fresh link from your provider."
            )),
        );
    }
    if low.contains("timed out")
        || low.contains("could not connect")
        || low.contains("dns")
        || low.contains("resolve")
    {
        return (
            "Kairo can't reach the provider.".into(),
            Some("Check that this Mac or PC is online, then try again.".into()),
        );
    }
    if low.contains("base64 node list") {
        return (
            "This link gives a plain node list, not a Clash/mihomo config.".into(),
            Some("Ask your provider for the Clash or mihomo link (many have a “Clash” button next to it).".into()),
        );
    }
    if low.contains("not valid yaml") || low.contains("no proxies") {
        return (
            "This link doesn't give a config Kairo can use.".into(),
            Some(format!("Check you copied the whole link. ({e})")),
        );
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
    store.state.with(|s| match &s.core {
        CoreStatus::Running { .. } if s.task.is_some() => Conn::Connecting("Updating…"),
        CoreStatus::Running { .. } => {
            if s.locating {
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
fn home() -> impl IntoElement {
    scroll().flex_1().child(
        view()
            .px_6()
            .pt(toolbar::scroll_top(20.0))
            .pb_8()
            .gap_4()
            .w_full()
            .max_w(px(1040.0))
            .self_center()
            .child(hero())
            .child(exits())
            .child(crate::pages::ai::board()),
    )
}

/// The switch, what it means now, and the two choices that shape it.
fn hero() -> impl IntoElement {
    let store = store();
    let state = computed(conn);
    let last_press = signal(None::<(bool, std::time::Instant)>);
    let on = move || {
        matches!(state.get(), Conn::On | Conn::Connecting(_))
            && !matches!(state.get(), Conn::Connecting("Disconnecting…"))
    };
    let headline = move || -> SharedString {
        match state.get() {
            Conn::Off => "Not Connected".into(),
            Conn::Connecting(what) => what.into(),
            Conn::On => "Connected".into(),
            Conn::Failed(_) => "Couldn't Connect".into(),
        }
    };
    let detail = move || -> SharedString {
        match state.get() {
            Conn::On | Conn::Connecting(_) => plan_line().into(),
            Conn::Off => "Turn on to go through your subscription.".into(),
            Conn::Failed(m) => explain(&m).0.into(),
        }
    };
    ui::card_soft()
        .px_6()
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
                    last_press.set(Some((next, std::time::Instant::now())));
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
                        .child(ui::muted(detail).tabular_nums().number_of_lines(2)),
                )
                .child(choices()),
        )
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
            parts.push(format!(
                "{} of {}",
                kairo_core::bytes(used),
                kairo_core::bytes(total)
            ));
        }
        if let Some(t) = expire {
            let days = (t - now) / 86400;
            parts.push(if t < now {
                "expired".into()
            } else if days == 1 {
                "1 day left".into()
            } else {
                format!("{days} days left")
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
                    || m.contains("used by")
                    || m.contains("port") && m.contains("in use"))
            {
                let controller = m.contains("controller port");
                ui::capsule("Use a Free Port", true)
                    .on_press(move || {
                        let mut next = store.svc().settings();
                        if controller {
                            next.controller_port = kairo_core::settings::free_port(
                                next.controller_port + 1,
                                &[next.mixed_port],
                            );
                        } else {
                            next.mixed_port = kairo_core::settings::free_port(
                                next.mixed_port + 1,
                                &[next.controller_port],
                            );
                        }
                        store.run_then(store.svc().update_settings(next), move |r| {
                            if r.is_ok() {
                                store.run(store.svc().connect(true));
                            }
                        });
                    })
                    .into_element()
            } else if m.contains("tun") && (m.contains("permission") || m.contains("privilege")) {
                ui::capsule("Allow TUN…", true)
                    .on_press(move || store.run(store.svc().grant_tun()))
                    .into_element()
            } else if failed {
                ui::capsule("Try Again", true)
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
    view()
        .gap_2()
        .flex_shrink_0()
        .child(labelled(
            "Mode",
            ui::segmented(
                vec![
                    (Mode::Rule, "Smart"),
                    (Mode::Global, "Everything"),
                    (Mode::Direct, "Direct"),
                ],
                mode,
                move |m| store.run(store.svc().set_mode(m)),
            ),
        ))
        .child(labelled(
            "Apps",
            ui::segmented(
                vec![(Via::System, "System settings"), (Via::Tun, "Every app")],
                via,
                move |v| set_via(v),
            ),
        ))
}

fn labelled(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .justify_end()
        .gap_3()
        .child(ui::muted(label))
        .child(control)
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
fn next_press(last: Option<(bool, std::time::Instant)>, shown_on: bool, failed: bool) -> bool {
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

/// The three exits, as cards; a press opens the Exits page.
fn exits() -> impl IntoElement {
    let store = store();
    view()
        .gap_2()
        .child(ui::section_label("Exits").px_1())
        .child(
            view()
                .flex_row()
                .gap_3()
                .children(crate::pages::exits::Exit::ALL.into_iter().map(move |e| {
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(crate::pages::exits::card_with(
                            e,
                            || false,
                            move || {
                                crate::pages::exits::select(e);
                                store.page.set(Page::Exits)
                            },
                            true,
                        ))
                })),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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
