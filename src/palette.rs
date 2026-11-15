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
            if running { t!("menu.disconnect") } else { t!("menu.connect") },
            t!("menu.connection"),
            Icon::Shield,
            move |s| s.run(s.svc().connect(!running)),
        ));
    }
    for (m, title) in [
        (Mode::Rule, t!("palette.mode.rule")),
        (Mode::Global, t!("palette.mode.global")),
        (Mode::Direct, t!("palette.mode.direct")),
    ] {
        out.push(cmd(title, t!("menu.connection"), Icon::Rules, move |s| {
            s.run(s.svc().set_mode(m))
        }));
    }
    if has_profile && running {
        out.push(cmd(
            t!("palette.check_ai"),
            "AI",
            Icon::Activity,
            |s| {
                s.page.set(Page::Overview);
                s.run(s.svc().check_ai())
            },
        ));
    }
    if has_profile && running {
        out.push(cmd(t!("palette.check_apps"), Page::Apps.title(), Icon::Activity, |s| {
            s.run(s.svc().check_ai());
            s.run(s.svc().check_apps())
        }));
    }
    // The kinds, and the apps in use: each opens where it is chosen.
    for kind in kairo_core::kinds::Kind::ALL {
        out.push(cmd(kind.title(), t!("palette.group.kind"), Icon::Layers, move |_| {
            crate::pages::kinds::open_kind(kind)
        }));
    }
    let apps = {
        let week = crate::pages::kinds::week(store);
        store.state.with(|s| store.activity.with(|a| crate::pages::kinds::rows(s, &a.0, &week, 0)))
    };
    for r in apps {
        let id = r.id.clone();
        out.push(cmd(r.name.clone(), t!("palette.app", kind = r.kind.title()), Icon::Layers, move |s| {
            crate::pages::exits::select(None);
            s.page.set(Page::Apps);
            s.app_sheet.set(Some(id.clone()))
        }));
    }
    out.push(cmd(
        t!("palette.rest_fastest"),
        Page::Apps.title(),
        Icon::Bolt,
        |s| s.run(s.svc().set_location(None)),
    ));
    out.push(cmd(
        t!("palette.test_exits"),
        Page::Apps.title(),
        Icon::Bolt,
        |s| s.run(s.svc().test_places()),
    ));
    for page in Page::ALL {
        out.push(cmd(
            t!("palette.go_to", page = page.title()),
            if page.inspect() { t!("palette.group.inspect") } else { t!("palette.group.page") },
            Icon::ChevronRight,
            move |s| s.page.set(page),
        ));
    }
    out.push(cmd(
        t!("palette.add_subscription"),
        Page::Profiles.title(),
        Icon::Plus,
        |s| s.page.set(Page::Profiles),
    ));
    out.push(cmd(
        t!("palette.all_connections"),
        Page::Activity.title(),
        Icon::Connections,
        |s| {
            s.all_connections.set(true);
            s.page.set(Page::Activity)
        },
    ));
    out.push(cmd(
        t!("palette.use_tun"),
        t!("menu.connection"),
        Icon::Shield,
        |_| crate::pages::home::set_via(crate::pages::home::Via::Tun),
    ));
    out.push(cmd(
        t!("palette.copy_proxy"),
        t!("menu.connection"),
        Icon::Link,
        crate::ui::copy_terminal_env,
    ));
    out.push(cmd(
        t!("palette.update_subscriptions"),
        Page::Profiles.title(),
        Icon::Refresh,
        |s| {
            s.run_update_then(s.svc().update_all_profiles(), move |r| {
                if r.is_ok() {
                    s.info(kairo_core::tr!("palette.subscriptions_updated"));
                }
            })
        },
    ));
    let advanced = t!("palette.group.advanced");
    out.push(cmd(t!("menu.core.restart"), advanced.clone(), Icon::Restart, |s| {
        s.run(s.svc().restart_core())
    }));
    out.push(cmd(t!("palette.close_all"), advanced.clone(), Icon::Close, |s| {
        s.run(s.svc().close_all_connections())
    }));
    out.push(cmd(
        t!("palette.update_geo"),
        advanced,
        Icon::Download,
        |s| s.run(s.svc().update_geo()),
    ));
    out.push(cmd(t!("palette.check_updates"), "Kairo", Icon::Download, |s| {
        s.run_then(s.svc().check_for_update(), move |r| {
            if let Ok(None) = r {
                s.info(kairo_core::tr!("menu.up_to_date"));
            }
        })
    }));
    // Every place, by delay.
    let ai_on = store
        .state
        .with(|s| !s.settings.kinds_off && !s.ai.group.is_empty());
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
    let (rest_exit, ai_exit) = (t!("palette.exit.rest"), t!("palette.exit.ai"));
    for p in list {
        let name = p.name.clone();
        let flag = p
            .flag
            .clone()
            .or_else(|| places::region_flag(p.region))
            .unwrap_or_default();
        let detail = |exit: &str| {
            format!(
                "{exit} · {}{}",
                places::region_title(p.region),
                p.delay
                    .filter(|d| *d > 0)
                    .map(|d| format!(" · {d} ms"))
                    .unwrap_or_default()
            )
        };
        let title = format!("{flag} {}", p.title).trim().to_owned();
        let for_ai = name.clone();
        out.push(cmd(
            title.clone(),
            detail(&rest_exit),
            Icon::Globe,
            move |s| s.run(s.svc().set_location(Some(name.clone()))),
        ));
        if ai_on {
            out.push(cmd(title, detail(&ai_exit), Icon::Globe, move |s| {
                s.run(s.svc().set_ai_exit(for_ai.clone()))
            }));
        }
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
                                .placeholder(t!("palette.placeholder"))
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
                                            ui::flagged({ let t = c.title.clone(); move || t.clone().into() }, size::body()).flex_1(),
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
