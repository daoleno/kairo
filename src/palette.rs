//! ⌘K / Ctrl+K: everything Kairo does, one search away (pages, the
//! everyday choices, the power actions and every location), run with Enter.

use kairo_core::Mode;
use kairo_core::places;
use nsh_base::{input, theme};
use nsh_core::prelude::*;

use crate::store::{Page, Store, store};
use crate::ui::{self, Icon, icon, size};

#[derive(Clone)]
struct Command {
    title: String,
    detail: String,
    glyph: Icon,
    run: std::rc::Rc<dyn Fn(Store)>,
}

fn cmd(
    title: impl Into<String>,
    detail: impl Into<String>,
    glyph: Icon,
    run: impl Fn(Store) + 'static,
) -> Command {
    Command {
        title: title.into(),
        detail: detail.into(),
        glyph,
        run: std::rc::Rc::new(run),
    }
}

fn commands(store: Store) -> Vec<Command> {
    let running = store.state.with(|s| s.core.is_running());
    let has_profile = store.state.with(|s| s.profiles.active.is_some());
    let mut out = Vec::new();
    if has_profile {
        out.push(cmd(
            if running { "Disconnect" } else { "Connect" },
            "Connection",
            Icon::Shield,
            move |s| s.run(s.svc().connect(!running)),
        ));
    }
    for (m, title) in [
        (Mode::Rule, "Route: Smart"),
        (Mode::Global, "Route: All Traffic"),
        (Mode::Direct, "Route: Direct"),
    ] {
        out.push(cmd(title, "Connection", Icon::Rules, move |s| {
            s.run(s.svc().set_mode(m))
        }));
    }
    if has_profile && running {
        out.push(cmd(
            "Check AI services",
            "Which country each sees, and whether its API answers",
            Icon::Activity,
            |s| {
                s.page.set(Page::Overview);
                s.run(s.svc().check_ai())
            },
        ));
    }
    out.push(cmd(
        "Location: Fastest",
        "Kairo keeps the fastest place",
        Icon::Bolt,
        |s| s.run(s.svc().set_location(None)),
    ));
    out.push(cmd(
        "Test Locations",
        "Measure every place's delay",
        Icon::Bolt,
        |s| s.run(s.svc().test_places()),
    ));
    for page in Page::ALL {
        out.push(cmd(
            format!("Go to {}", page.title()),
            if page.inspect() { "Inspect" } else { "Page" },
            Icon::ChevronRight,
            move |s| s.page.set(page),
        ));
    }
    out.push(cmd(
        "Add Subscription…",
        "Subscriptions",
        Icon::Plus,
        |s| s.page.set(Page::Profiles),
    ));
    out.push(cmd(
        "Update Subscriptions",
        "Subscriptions",
        Icon::Refresh,
        |s| {
            s.run_then(s.svc().update_all_profiles(), move |r| {
                if r.is_ok() {
                    s.info("Subscriptions updated");
                }
            })
        },
    ));
    out.push(cmd("Restart Core", "Advanced", Icon::Restart, |s| {
        s.run(s.svc().restart_core())
    }));
    out.push(cmd("Close All Connections", "Advanced", Icon::Close, |s| {
        s.run(s.svc().close_all_connections())
    }));
    out.push(cmd(
        "Update GeoIP and GeoSite",
        "Advanced",
        Icon::Download,
        |s| s.run(s.svc().update_geo()),
    ));
    out.push(cmd("Check for Updates", "Kairo", Icon::Download, |s| {
        s.run_then(s.svc().check_for_update(), move |r| {
            if let Ok(None) = r {
                s.info("Kairo is up to date");
            }
        })
    }));
    // Every place, by delay.
    let preferred = store.state.with(|s| s.settings.tray_group.clone());
    let mut list = store.proxies.with(|p| {
        places::main_group(&p.0.groups, &preferred)
            .map(|g| places::places(&p.0.groups, g))
            .unwrap_or_default()
    });
    list.sort_by_key(|p| match p.delay {
        Some(d) if d > 0 => d,
        _ => u32::MAX,
    });
    for p in list {
        let name = p.name.clone();
        let flag = p
            .flag
            .clone()
            .or_else(|| places::region_flag(p.region))
            .unwrap_or_default();
        out.push(cmd(
            format!("{flag} {}", p.title).trim().to_owned(),
            format!(
                "Location · {}{}",
                p.region,
                p.delay
                    .filter(|d| *d > 0)
                    .map(|d| format!(" · {d} ms"))
                    .unwrap_or_default()
            ),
            Icon::Globe,
            move |s| s.run(s.svc().set_location(Some(name.clone()))),
        ));
    }
    out
}

/// Words in any order, each a prefix of a word of the title or detail.
fn matches(c: &Command, q: &str) -> bool {
    let hay = format!("{} {}", c.title, c.detail).to_lowercase();
    q.split_whitespace()
        .all(|w| hay.contains(&w.to_lowercase()))
}

thread_local! {
    static OPEN: std::cell::Cell<Option<Signal<bool>>> = const { std::cell::Cell::new(None) };
}

/// Opens the palette (a menu item or the chrome's shortcut).
pub fn open() {
    if let Some(open) = OPEN.with(|o| o.get()) {
        open.set(true);
    }
}

pub fn palette() -> impl IntoElement {
    let store = store();
    let open = signal(false);
    OPEN.with(|o| o.set(Some(open)));
    on_key("mod+k", move |_| open.set(!open.get_untracked()));
    when(move || open.get(), move || panel(open, store))
}

fn panel(open: Signal<bool>, store: Store) -> impl IntoElement {
    let query = signal(String::new());
    let selected = signal(0usize);
    let all = std::rc::Rc::new(untracked(|| commands(store)));
    let shown = {
        let all = all.clone();
        move || {
            let q = query.get();
            all.iter()
                .filter(|c| q.trim().is_empty() || matches(c, &q))
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
        }
    };
    let shown = std::rc::Rc::new(shown);
    let run = {
        let shown = shown.clone();
        move || {
            let list = shown();
            if let Some(c) = list.get(selected.get_untracked()).or(list.first()) {
                open.set(false);
                (c.run)(store);
            }
        }
    };
    let run = std::rc::Rc::new(run);
    // The search field has the focus, and a field keeps Escape and the
    // arrows for itself unless asked: these are the palette's.
    let always = || KeyOptions {
        when: nsh_core::keymap::When::Always,
        ..Default::default()
    };
    on_key_with("escape", always(), move |_| open.set(false));
    {
        let shown = shown.clone();
        on_key_with(
            "down",
            KeyOptions {
                repeat: true,
                ..always()
            },
            move |_| {
                selected.set((selected.get_untracked() + 1).min(shown().len().saturating_sub(1)))
            },
        );
    }
    on_key_with(
        "up",
        KeyOptions {
            repeat: true,
            ..always()
        },
        move |_| selected.set(selected.get_untracked().saturating_sub(1)),
    );
    let submit = run.clone();
    view()
        .absolute()
        .inset_0()
        .items_center()
        .pt(px(96.0))
        .bg(move || AdaptiveColor::from(black()).opacity(0.12))
        .on_press(move || open.set(false))
        .child(
            view()
                .w(px(560.0))
                .max_w(percentage(0.92))
                .max_h(px(440.0))
                .rounded(px(22.0))
                .bg(move || theme().popover)
                .shadow_2xl()
                .border_1()
                .border_color(move || theme().border.opacity(0.6))
                .overflow_hidden()
                .on_press(|| {})
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .h(px(52.0))
                        .child(
                            icon(Icon::Search, 16.0).text_color(move || theme().muted_foreground),
                        )
                        .child(
                            input()
                                .value(move || query.get())
                                .on_change(move |t: &str| {
                                    query.set(t.to_owned());
                                    selected.set(0);
                                })
                                .on_submit(move |_: &str| submit())
                                .placeholder("Search Kairo: pages, actions, locations")
                                .placeholder_color(|| color(theme().muted_foreground))
                                .caret_color(|| color(theme().primary))
                                .auto_focus(true)
                                .flex_1()
                                .text_size(px(size::large())),
                        ),
                )
                .child(view().h(px(1.0)).bg(move || theme().border.opacity(0.6)))
                .child(
                    scroll()
                        .flex_shrink(1.0)
                        .child(view().p_2().child(dynamic(move || {
                            let list = shown();
                            let sel = selected.get();
                            untracked(move || {
                                view().children(list.into_iter().enumerate().map(move |(i, c)| {
                                    let is = i == sel;
                                    let run_one = c.run.clone();
                                    nsh_base::button()
                                        .on_press(move || {
                                            open.set(false);
                                            run_one(store);
                                        })
                                        .flex_row()
                                        .items_center()
                                        .gap_3()
                                        .h(px(40.0))
                                        .px_3()
                                        .rounded(px(12.0))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(move || theme().accent))
                                        .style_when(move || is, |s| s.bg(move || ui::selection()))
                                        .child(icon(c.glyph, 15.0).text_color(move || {
                                            if is {
                                                theme().primary
                                            } else {
                                                theme().muted_foreground
                                            }
                                        }))
                                        .child(
                                            ui::body(c.title.clone()).flex_1().number_of_lines(1),
                                        )
                                        .child(
                                            ui::muted(c.detail.clone())
                                                .text_size(px(size::small()))
                                                .number_of_lines(1),
                                        )
                                }))
                            })
                        }))),
                ),
        )
}
