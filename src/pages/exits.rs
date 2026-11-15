//! Apps: where each kind of app leaves, and every app. A card per kind on
//! the left (All apps first); the selected kind's picker on the right,
//! countries first and a server one level deeper, with the kind's apps under
//! it. All apps lists every app in use by kind, each with its exit, and what
//! Kairo saw but could not sort. Latency reads Good / OK / Slow;
//! milliseconds only show in the list of servers.

use kairo_core::kinds::{AppExit, Kind};
use kairo_core::places::{self, Place};
use kairo_core::routes::RouteKind;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::i18n::plural;
use crate::pages::kinds::{self as k, AppRow};
use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, Variant, icon, size};

// --- How a kind's exit is doing ------------------------------------------------------

/// How a delay reads: never as a number outside the server list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Good,
    Ok,
    Slow,
    Untested,
}

impl Health {
    pub fn of(delay: Option<u32>) -> Health {
        match delay {
            None => Health::Untested,
            Some(0) => Health::Slow,
            Some(d) if d < 200 => Health::Good,
            Some(d) if d < 500 => Health::Ok,
            Some(_) => Health::Slow,
        }
    }

    pub fn word(self) -> String {
        kairo_core::tr!(match self {
            Health::Good => "exits.health.good",
            Health::Ok => "exits.health.ok",
            Health::Slow => "exits.health.slow",
            Health::Untested => "exits.health.untested",
        })
    }

    pub fn tone(self) -> AdaptiveColor {
        match self {
            Health::Good => ui::success(),
            Health::Ok => ui::warning(),
            Health::Slow => theme().destructive,
            Health::Untested => theme().muted_foreground,
        }
    }

    /// The better of two (a country is as good as its best server).
    fn best(a: Health, b: Health) -> Health {
        fn rank(h: Health) -> u8 {
            match h {
                Health::Good => 0,
                Health::Ok => 1,
                Health::Slow => 2,
                Health::Untested => 3,
            }
        }
        if rank(a) <= rank(b) { a } else { b }
    }
}


/// The stats slot of a kind's quality line, for the kinds that keep one.
fn stats_slot(kind: Kind) -> Option<usize> {
    match kind {
        Kind::Ai => Some(0),
        Kind::Other => Some(1),
        Kind::Downloads => Some(2),
        _ => None,
    }
}

/// How the kind's exit is doing now.
fn health_of(kind: Kind) -> Health {
    Health::of(k::place_of(kind).and_then(|p| p.delay))
}

/// A kind's card: on Apps' rail, and on a phone above the picker.
pub fn card(
    kind: Kind,
    count: impl Fn() -> usize + Copy + 'static,
    selected: impl Fn() -> bool + Copy + 'static,
    press: impl Fn() + 'static,
) -> impl IntoElement {
    let store = store();
    let place = computed(move || {
        store.state.with(|_| ());
        store.proxies.with(|_| ());
        k::place_of(kind)
    });
    let health = move || place.with(|p| Health::of(p.as_ref().and_then(|p| p.delay)));
    nsh_base::button()
        .w_full()
        .accessibility_label(kind.title())
        .selected(selected)
        .on_press(press)
        .gap_1()
        .px_4()
        .py_3()
        .rounded(px(18.0))
        .cursor_pointer()
        .bg(move || theme().muted)
        .hover(|s| s.bg(move || theme().accent))
        .style_when(selected, |s| s.outline(px(2.0)).outline_color(move || theme().primary))
        .focus(|s| s.outline(px(3.0)).outline_color(move || theme().ring))
        // The kind's name, then its health: beside it while both fit, under it
        // when not (a narrow card); the name is never cut to make room.
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_x_2()
                .gap_y_0p5()
                .child(
                    ui::muted(kind.title())
                        .font_weight(FontWeight::MEDIUM)
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .number_of_lines(2),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .flex_shrink_0()
                        .child(when(move || kind == Kind::Ai, move || ui::badge(t!("exits.badge.stable"))))
                        .child(ui::dot(move || health().tone()))
                        .child(ui::muted(move || -> SharedString { health().word().into() }).text_size(px(size::small()))),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(dynamic(move || {
                    let f = place
                        .with(|p| p.as_ref().and_then(|p| p.flag.clone()))
                        .unwrap_or_else(|| "◎".into());
                    untracked(move || ui::flag(f, size::large() + 1.0))
                }))
                .child(
                    text(move || -> SharedString {
                        place
                            .with(|p| p.as_ref().map(k::place_country))
                            .unwrap_or_else(|| "—".into())
                            .into()
                    })
                    .text_size(px(size::large()))
                    .font_weight(FontWeight::SEMIBOLD)
                    .number_of_lines(2)
                    .flex_1(),
                ),
        )
        .child(
            ui::muted(move || -> SharedString {
                store.state.with(|_| ());
                store.proxies.with(|_| ());
                let n = count();
                let apps = t!(&plural("apps.count", n as u64), count = n);
                format!("{} · {apps}", k::subtitle(kind)).into()
            })
            .text_size(px(size::small()))
            .number_of_lines(3),
        )
}

/// The least width of a kind's card in the grid above the kind (points).
const CARD_MIN: f32 = 150.0;
/// The room between the cards (points; `gap_2p5`).
const CARD_GAP: f32 = 10.0;

/// The columns of kind cards in `width` points, when they are above the kind
/// (a phone, a narrow window): two, three where three cards keep their least
/// width. Both divide the six kinds into full rows.
pub fn columns(width: f32) -> u16 {
    if width >= 3.0 * CARD_MIN + 2.0 * CARD_GAP { 3 } else { 2 }
}

// --- The page --------------------------------------------------------------------------

/// The content's width (beside the sidebar) below which Apps puts its cards
/// above the kind: the rail's 270 and a list that keeps its exit menus.
pub const STACK_BELOW: f32 = 820.0;

thread_local! {
    /// The kind shown (None: All apps).
    static SELECTED: std::cell::Cell<Option<Kind>> = const { std::cell::Cell::new(None) };
}

/// The kind the page opens on (None: All apps).
pub fn select(kind: Option<Kind>) {
    SELECTED.with(|s| s.set(kind));
}

pub fn page() -> impl IntoElement {
    let which = signal(SELECTED.with(|s| s.get()));
    effect(move || SELECTED.with(|s| s.set(which.get())));
    let query = signal(String::new());
    let adding = signal(false);
    let rows = k::all_rows(0);
    // Rebuilt only when the window crosses the width, not at every pixel.
    let stack = computed(move || ui::phone() || window().width.0 < STACK_BELOW);
    let count = move |kind: Option<Kind>| rows.with(|r| r.iter().filter(|a| kind.is_none_or(|k| a.kind == k)).count());
    view()
        .flex_1()
        .child(toolbar::floating(
            Page::Apps,
            move || match which.get() {
                Some(_) => t!("apps.page.subtitle"),
                None => {
                    let n = count(None);
                    t!(&plural("apps.count", n as u64), count = n)
                }
            },
            vec![toolbar::search("apps.search", t!("apps.search_placeholder"), query)],
        ))
        // Below this width the cards go above the kind (in rows), not beside it.
        .child(scroll().flex_1().child(dynamic(move || {
            let phone = stack.get();
            untracked(move || {
            let page = view()
                .px(px(if phone { 16.0 } else { 24.0 }))
                .pt(toolbar::scroll_top(20.0))
                .pb_8()
                .w_full()
                .max_w(px(1040.0))
                .self_center()
                .gap_4();
            let page = if phone { page } else { page.flex_row().items_start() };
            page.child(if phone {
                // Above the kind: All Apps on its own, then the kinds in equal columns
                // across the width.
                let width = signal(0.0f32);
                view()
                    .w_full()
                    .gap_3()
                    .child(view().flex_row().flex_wrap().gap_2().child(all_apps(which, move || count(None), true)))
                    .child(
                        view()
                            .w_full()
                            .grid()
                            .grid_cols(move || columns(width.get()))
                            .gap_2p5()
                            .on_layout(move |b: &Bounds<Points>| {
                                if (b.size.width.0 - width.get_untracked()).abs() > 0.5 {
                                    width.set(b.size.width.0);
                                }
                            })
                            .children(Kind::ALL.into_iter().map(move |kind| {
                                card(kind, move || count(Some(kind)), move || which.get() == Some(kind), move || which.set(Some(kind)))
                                    .into_element()
                            })),
                    )
                    .into_element()
            } else {
                view()
                    .flex_shrink_0()
                    .w(px(270.0))
                    .gap_2p5()
                    .child(all_apps(which, move || count(None), false))
                    .children(Kind::ALL.into_iter().map(move |kind| {
                        card(kind, move || count(Some(kind)), move || which.get() == Some(kind), move || which.set(Some(kind)))
                            .into_element()
                    }))
                    .into_element()
            })
            .child(view().flex_1().min_w(px(0.0)).gap_4().child(dynamic(move || {
                let sel = which.get();
                untracked(move || match sel {
                    Some(kind) => kind_view(kind, rows).into_element(),
                    None => all_view(rows, query, adding).into_element(),
                })
            })))
            .into_element()
            })
        })))
        .child(dynamic(move || {
            let open = adding.get();
            untracked(move || if open { add_sheet(adding).into_element() } else { view().into_element() })
        }))
        .child(k::sheet_host())
        .text_color(move || theme().foreground)
}

/// All Apps, a filter over the kinds: the rail's first row, or a chip above
/// the kinds' cards (`chip`).
fn all_apps(which: Signal<Option<Kind>>, count: impl Fn() -> usize + 'static, chip: bool) -> AnyElement {
    nsh_base::button()
        .accessibility_label(t!("apps.all"))
        .selected(move || which.get().is_none())
        .on_press(move || which.set(None))
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .h(px(ui::touch_h(if chip { 36.0 } else { 44.0 })))
        .rounded(px(if chip { 999.0 } else { 18.0 }))
        .cursor_pointer()
        .bg(move || theme().muted)
        .hover(|s| s.bg(move || theme().accent))
        .style_when(move || which.get().is_none(), |s| {
            s.outline(px(2.0)).outline_color(move || theme().primary)
        })
        .child(
            ui::body(t!("apps.all"))
                .font_weight(FontWeight::SEMIBOLD)
                .number_of_lines(1)
                .when(!chip, |t| t.flex_1()),
        )
        .child(ui::muted(move || -> SharedString { count().to_string().into() }))
        .into_element()
}

/// A kind: its picker, what goes with it, and its apps.
fn kind_view(kind: Kind, rows: Memo<Vec<AppRow>>) -> impl IntoElement {
    let store = store();
    let tun = move || store.state.with(|s| s.settings.tun);
    view()
        .gap_4()
        .child(picker(kind))
        .child(when(move || kind == Kind::Downloads, mirror_card))
        .child(when(
            move || kind == Kind::Games && !tun(),
            move || {
                ui::card()
                    .rounded(px(22.0))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .px_4()
                            .py_3()
                            .when(ui::phone(), |v| v.flex_wrap())
                            .child(
                                view()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .gap_0p5()
                                    .child(ui::body(t!("apps.games.title")))
                                    .child(ui::muted(t!("apps.games.body"))),
                            )
                            .child(
                                ui::capsule(t!("kinds.fix.every_app"), true)
                                    .h(px(30.0))
                                    .px_4()
                                    .on_press(|| crate::pages::home::set_via(crate::pages::home::Via::Tun)),
                            ),
                    )
            },
        ))
        .child(dynamic(move || {
            let list: Vec<AppRow> = rows.with(|r| r.iter().filter(|a| a.kind == kind).cloned().collect());
            untracked(move || {
                if list.is_empty() {
                    return ui::muted(t!("apps.kind.none", kind = kind.in_sentence()))
                        .px_1()
                        .into_element();
                }
                view()
                    .gap_2()
                    .child(ui::section_label(t!("apps.kind.section", kind = kind.in_sentence()).trim().to_owned()).px_1())
                    .child(ui::rows(list.into_iter().map(k::app_row).collect()))
                    .into_element()
            })
        }))
        .child(stats_slot(kind).map(quality_card))
}

/// Every app, by kind; what was seen and not sorted; the subscription's groups.
fn all_view(rows: Memo<Vec<AppRow>>, query: Signal<String>, adding: Signal<bool>) -> impl IntoElement {
    let store = store();
    view()
        .gap_4()
        .child(
            view()
                .flex_row()
                .items_center()
                .child(view().flex_1())
                .child(ui::capsule(t!("apps.add_app"), false).h(px(ui::touch_h(30.0))).px_4().on_press(move || adding.set(true))),
        )
        .child(dynamic(move || {
            let q = query.get().to_lowercase();
            let all = rows.get();
            untracked(move || {
                let keep = |r: &AppRow| {
                    q.is_empty()
                        || r.name.to_lowercase().contains(&q)
                        || r.id.to_lowercase().contains(&q)
                        || r.programs.iter().any(|p| p.to_lowercase().contains(&q))
                };
                let mut out = view().gap_4();
                let mut any = false;
                for kind in Kind::ALL {
                    let list: Vec<AppRow> = all.iter().filter(|r| r.kind == kind && keep(r)).cloned().collect();
                    if list.is_empty() {
                        continue;
                    }
                    any = true;
                    out = out.child(
                        view()
                            .gap_2()
                            .child(ui::section_label(kind.title()).px_1())
                            .child(ui::rows(list.into_iter().map(k::app_row).collect())),
                    );
                }
                if !any {
                    out = out.child(ui::empty_state(Icon::Layers, t!("apps.empty"), None));
                }
                out.into_element()
            })
        }))
        .child(unsorted())
        .child(subscription_row())
        .child(when(
            move || store.state.with(|s| s.settings.kinds_off),
            || ui::muted(t!("apps.kinds_off")).px_1(),
        ))
}

/// Sites no app claims, with enough traffic: a kind makes one an app.
fn unsorted() -> impl IntoElement {
    let store = store();
    let seen = computed(move || {
        store.activity.with(|a| {
            a.0.unsorted(kairo_core::kinds::USED_BYTES)
                .into_iter()
                .take(8)
                .map(|(n, b)| (n.to_owned(), b))
                .collect::<Vec<_>>()
        })
    });
    dynamic(move || {
        let list = seen.get();
        untracked(move || {
            if list.is_empty() {
                return view().into_element();
            }
            view()
                .gap_2()
                .child(ui::section_label(t!("apps.unsorted")).px_1())
                .child(ui::rows(
                    list.into_iter()
                        .map(|(site, bytes)| {
                            let domain = stored(site.clone());
                            ui::row_view()
                                .child(k::logo_of(&site, &site, Some(&RouteKind::Site { domain: site.clone() }), 24.0))
                                .child(ui::body(site.clone()).number_of_lines(1).flex_1())
                                .child(ui::muted(t!("apps.so_far", bytes = kairo_core::bytes(bytes))))
                                .child(ui::popup_button(
                                    || {
                                        std::iter::once((None, t!("apps.sort_as")))
                                            .chain(Kind::ALL.into_iter().map(|k| (Some(k), k.title().to_owned())))
                                            .collect()
                                    },
                                    || None,
                                    move |k: Option<Kind>| {
                                        if let Some(kind) = k {
                                            k::set_app(
                                                String::new(),
                                                Some(RouteKind::Site { domain: domain.with(|d| d.clone()) }),
                                                kind,
                                                AppExit::Kind,
                                            )
                                        }
                                    },
                                ))
                                .into_element()
                        })
                        .collect(),
                ))
                .into_element()
        })
    })
}

/// The subscription's own per-app groups: a line, and the way to them.
fn subscription_row() -> impl IntoElement {
    let store = store();
    let summary = move || -> String {
        let preferred = store.state.with(|s| s.settings.tray_group.clone());
        store.proxies.with(|p| {
            let Some(main) = places::main_group(&p.0.groups, &preferred) else {
                return String::new();
            };
            let apps = crate::pages::ai::without_ai(places::app_choices(&p.0.groups, &main.name));
            if apps.is_empty() {
                return String::new();
            }
            let names: Vec<&str> = apps.iter().take(3).map(|a| a.group.as_str()).collect();
            let more = apps.len().saturating_sub(3);
            if more == 0 {
                names.join(", ")
            } else {
                t!("apps.subscription.more", names = names.join(", "), count = more)
            }
        })
    };
    when(
        move || !summary().is_empty(),
        move || {
            view()
                .gap_2()
                .child(ui::section_label(t!("apps.subscription.title")).px_1())
                .child(ui::rows(vec![
                    ui::row_view()
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(ui::body(move || -> SharedString { summary().into() }).number_of_lines(2)),
                        )
                        .child(ui::capsule(t!("apps.subscription.groups"), false).h(px(30.0)).px_4().on_press(move || store.page.set(Page::Proxies)))
                        .into_element(),
                ]))
        },
    )
}

// --- Adding ----------------------------------------------------------------------------

/// Add App: a program or a site, its kind and its exit.
fn add_sheet(adding: Signal<bool>) -> impl IntoElement {
    let close = move || adding.set(false);
    view()
        .absolute()
        .inset_0()
        .child(
            overlay()
                .absolute()
                .inset_0()
                .bg(move || theme().foreground.opacity(0.28))
                .on_press(close),
        )
        .child(
            overlay()
                .absolute()
                .inset_0()
                .items_center()
                .justify_center()
                .px_4()
                .pointer_events(nsh_core::PointerEvents::BoxNone)
                .on_cancel(close)
                .child(add_card(adding)),
        )
}

fn add_card(adding: Signal<bool>) -> impl IntoElement {
    let store = store();
    let value = signal(String::new());
    let site = signal(false);
    let kind = signal(Kind::Other);
    let exit = signal(AppExit::Kind);
    let problem = signal(None::<String>);
    let go = move || {
        let v = value.get_untracked().trim().to_owned();
        if v.is_empty() {
            problem.set(Some(t!("apps.add.name_first")));
            return;
        }
        // A name with a dot and no spaces is a site, else a program.
        let m = if site.get_untracked() {
            RouteKind::Site { domain: v }
        } else {
            RouteKind::App { name: v }
        };
        let svc = store.svc();
        let (kd, ex) = (kind.get_untracked(), exit.get_untracked());
        spawn(async move {
            match svc.set_app(String::new(), Some(m), kd, ex).await {
                Ok(()) => adding.set(false),
                Err(e) => problem.set(Some(e)),
            }
        });
    };
    // Programs Activity has seen, to pick from (no browser: a browser is never an app).
    let seen = computed(move || {
        store.activity.with(|a| {
            a.0.apps()
                .into_iter()
                .map(|(n, _)| n.to_owned())
                .filter(|n| n != "Unknown" && !kairo_core::kinds::is_browser(n))
                .take(8)
                .collect::<Vec<_>>()
        })
    });
    view()
        .w_full()
        .max_w(px(480.0))
        .p_5()
        .gap_3()
        .rounded(px(16.0))
        .bg(move || theme().background)
        .border_1()
        .border_color(move || theme().border)
        .child(
            text(t!("apps.add.app_title"))
                .text_size(px(size::large() + 3.0))
                .font_weight(FontWeight::SEMIBOLD)
                .role(AccessibilityRole::Header),
        )
        .child(ui::segmented(
            vec![(false, t!("apps.add.program")), (true, t!("apps.add.site"))],
            move || site.get(),
            move |v| site.set(v),
        ))
        .child(
            ui::field(value, t!("apps.add.placeholder"))
                .auto_focus(true)
                .on_submit(move |_: &str| go()),
        )
        .child(when(
            move || !site.get() && !seen.with(|s| s.is_empty()),
            move || {
                view().flex_row().flex_wrap().gap_2().children(seen.get().into_iter().map(move |n| {
                    let name = n.clone();
                    ui::btn(n, Variant::Secondary).h(px(26.0)).on_press(move || value.set(name.clone()))
                }))
            },
        ))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .child(ui::muted(t!("kinds.sheet.kind")))
                .child(view().flex_1())
                .child(ui::popup_button(
                    || Kind::ALL.into_iter().map(|k| (k, k.title().to_owned())).collect(),
                    move || kind.get(),
                    move |k| kind.set(k),
                )),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .child(ui::muted(t!("kinds.sheet.exit")))
                .child(view().flex_1())
                .child(ui::popup_button(
                    move || k::exit_choices(kind.get(), &AppExit::Kind),
                    move || exit.get(),
                    move |e| exit.set(e),
                )),
        )
        .child(when(
            move || problem.with(|p| p.is_some()),
            move || {
                ui::muted(move || -> SharedString { problem.get().unwrap_or_default().into() })
                    .text_color(move || theme().destructive)
            },
        ))
        .child(
            view()
                .flex_row()
                .gap_2()
                .justify_end()
                .child(ui::btn(t!("apps.add.cancel"), Variant::Secondary).on_press(move || adding.set(false)))
                .child(ui::btn(t!("apps.add.add"), Variant::Primary).on_press(go)),
        )
}

/// How the selected kind has been doing: response time, failed tests, switches.
fn quality_card(idx: usize) -> impl IntoElement {
    ui::card()
        .rounded(px(22.0))
        .child(view().px_4().py_3().child(ui::body(t!("exits.quality")).font_weight(FontWeight::SEMIBOLD)))
        .child(ui::divider())
        .child(crate::pages::stats::quality(idx))
}

// --- The mirror ------------------------------------------------------------------------

/// What the mirror's speed test says, in words.
pub fn mirror_text(test: &kairo_core::downloads::MirrorTest) -> String {
    use kairo_core::downloads::MirrorTest as T;
    use kairo_core::tr;
    let secs = |ms: u64| format!("{:.1} s", ms as f64 / 1000.0);
    match test {
        T::Untested => tr!("exits.mirror.off"),
        T::Running => tr!("exits.mirror.testing"),
        T::Passed {
            mirror_ms,
            direct_ms: Some(d),
        } => tr!("exits.mirror.passed", mirror = secs(*mirror_ms), original = secs(*d)),
        T::Passed {
            mirror_ms,
            direct_ms: None,
        } => tr!("exits.mirror.passed_no_original", mirror = secs(*mirror_ms)),
        T::Failed { why } => tr!("exits.mirror.failed", why = why),
    }
}

/// The mirror switch, under the Downloads exit: off until a speed test passes.
fn mirror_card() -> impl IntoElement {
    let store = store();
    let info = computed(move || store.state.with(|s| (s.downloads.clone(), s.settings.kinds.mirror)));
    let running =
        move || info.with(|(d, _)| matches!(d.mirror_test, kairo_core::downloads::MirrorTest::Running));
    let can_switch = move || info.with(|(d, _)| d.mirror_test.passed());
    ui::card()
        .rounded(px(22.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_4()
                .px_4()
                .py_3()
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap_0p5()
                        .child(ui::body(t!("exits.mirror.title")))
                        .child(ui::muted(move || -> SharedString {
                            info.with(|(d, _)| mirror_text(&d.mirror_test)).into()
                        })),
                )
                .child(
                    ui::btn(t!("exits.mirror.test"), Variant::Secondary)
                        .disabled(running)
                        .on_press(move || store.run(store.svc().test_mirror())),
                )
                .child(
                    view().opacity(move || if can_switch() || info.with(|(_, on)| *on) { 1.0_f32 } else { 0.4_f32 }).child(
                        ui::toggle(
                            move || info.with(|(_, on)| *on),
                            t!("exits.mirror.use"),
                            move |on| {
                                if can_switch() || !on {
                                    store.run(store.svc().set_mirror(on));
                                } else {
                                    store.error(kairo_core::downloads::mirror_refused());
                                }
                            },
                        ),
                    ),
                ),
        )
        .child(when(
            move || info.with(|(_, on)| *on),
            move || {
                let lines = kairo_core::downloads::mirror_env_lines().join("\n");
                let copy = lines.clone();
                view()
                    .gap_2()
                    .px_4()
                    .pb_3()
                    .child(ui::divider())
                    .child(text(lines).text_size(px(size::small())).selectable(true))
                    .child(
                        view().flex_row().child(
                            ui::btn(t!("exits.mirror.copy"), Variant::Secondary).on_press(move || {
                                ui::copy(store, copy.clone(), t!("exits.copied"));
                            }),
                        ),
                    )
            },
        ))
}

// --- The picker ------------------------------------------------------------------------

/// A country: its servers and how good the best of them is.
#[derive(Debug, Clone, PartialEq)]
struct Country {
    region: &'static str,
    flag: String,
    servers: Vec<Place>,
}

impl Country {
    fn health(&self) -> Health {
        self.servers
            .iter()
            .map(|p| Health::of(p.delay))
            .fold(Health::Untested, Health::best)
    }
}


fn countries(kind: Kind) -> Vec<Country> {
    let store = store();
    let preferred = store.state.with(|s| s.settings.tray_group.clone());
    let here = k::place_of(kind).map(|p| p.region);
    store.proxies.with(|p| {
        let Some(g) = places::main_group(&p.0.groups, &preferred) else {
            return vec![];
        };
        let all = places::places(&p.0.groups, g);
        let mut list: Vec<Country> = places::by_region(&all)
            .into_iter()
            .map(|(region, servers)| Country {
                region,
                flag: servers
                    .iter()
                    .find_map(|s| s.flag.clone())
                    .or_else(|| places::region_flag(region))
                    .unwrap_or_else(|| "◎".into()),
                servers,
            })
            .collect();
        // Where it is now first, then the quickest countries.
        list.sort_by_key(|c| {
            (
                Some(c.region) != here,
                c.servers
                    .iter()
                    .filter_map(|s| s.delay.filter(|d| *d > 0))
                    .min()
                    .unwrap_or(u32::MAX),
            )
        });
        list
    })
}

fn picker(kind: Kind) -> impl IntoElement {
    let store = store();
    let open = signal(None::<&'static str>);
    let all = signal(false);
    let list = computed(move || {
        store.proxies.with(|_| ());
        countries(kind)
    });
    let testing = move || store.proxies.with(|p| !p.0.testing.is_empty());
    let current = move || {
        store.state.with(|_| ());
        store.proxies.with(|_| ());
        k::place_of(kind).map(|p| p.name)
    };
    let sub = move || store.state.with(|s| k::on_subscription(s, kind));
    let fastest = move || store.state.with(|s| k::on_fastest(s, kind));
    let follows = move || store.state.with(|s| k::follows_location(s, kind));
    // What the servers' rows show, each memoized: the rows are made again only when
    // one of these changes, not at every change of the state (a press on a row made
    // again in the meantime is lost).
    let now_m = computed(current);
    let manual_m = computed(move || !fastest() && !sub() && !follows());
    effect(move || {
        store.state.with(|_| ());
        store.proxies.with(|_| ());
        let here = k::place_of(kind);
        untracked(move || {
            if open.get().is_none()
                && let Some(p) = here
            {
                open.set(Some(p.region));
            }
        });
    });
    let phone = ui::phone();
    let head = view().flex_row().items_center().gap_2().px_4().py_3();
    let head = if phone { head.flex_wrap() } else { head };
    ui::card()
        .rounded(px(22.0))
        .child(
            head.child(ui::body(kind.title()).font_weight(FontWeight::SEMIBOLD))
                .child(view().flex_1())
                .child(
                    ui::btn_icon(Icon::Bolt, t!("exits.test_again"), Variant::Secondary)
                        .disabled(testing)
                        .on_press(move || {
                            spawn(store.svc().test_places());
                        }),
                ),
        )
        .child(ui::divider())
        .child(when(
            move || Kind::ROUTED.contains(&kind),
            move || {
                row_button(
                    sub,
                    move || k::to_subscription(kind),
                    icon(Icon::Layers, 15.0).text_color(move || theme().muted_foreground).into_element(),
                    ui::body(t!("kinds.as_subscription")),
                    ui::muted(move || -> SharedString {
                        if sub() {
                            k::place_of(kind).map(|p| t!("exits.fastest.now", node = p.title)).unwrap_or_default().into()
                        } else {
                            t!("apps.picker.no_rule").into()
                        }
                    }),
                    None,
                    None::<fn() -> Health>,
                )
            },
        ))
        .child(when(
            move || kind != Kind::Ai,
            move || {
                row_button(
                    move || fastest() && !sub(),
                    move || k::use_fastest(kind),
                    icon(Icon::Bolt, 15.0).text_color(move || theme().primary).into_element(),
                    ui::body(t!("exits.fastest.title")).font_weight(FontWeight::MEDIUM),
                    ui::muted(move || -> SharedString {
                        store.state.with(|_| ());
                        store.proxies.with(|_| ());
                        match k::place_of(kind) {
                            Some(p) if fastest() && !sub() => t!("exits.fastest.now", node = p.title).into(),
                            _ => t!("exits.fastest.automatic").into(),
                        }
                    }),
                    None,
                    Some(move || health_of(kind)),
                )
            },
        ))
        .child(when(
            move || kind != Kind::Other,
            move || {
                row_button(
                    move || follows() && !sub(),
                    move || k::follow_location(kind),
                    icon(Icon::Globe, 15.0).text_color(move || theme().muted_foreground).into_element(),
                    ui::body(t!("kinds.exit.same_as", kind = Kind::Other.in_sentence()).trim().to_owned()),
                    ui::muted(""),
                    None,
                    None::<fn() -> Health>,
                )
            },
        ))
        // Each country, and each server of an open one, is made once and kept (by its
        // region, by its name) while readings come and go: a press on a row is never
        // lost to a row made again under it.
        .child(each(
            move || list.get(),
            |c: &Country| c.region,
            move |c, _| country_rows(kind, c, now_m, manual_m, all, open),
        ))
        .child(ui::divider())
        .child(
            nsh_base::button()
                .on_press(move || all.set(!all.get_untracked()))
                .h(px(40.0))
                .px_4()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(move || theme().accent))
                .child(ui::muted(move || -> SharedString {
                    if all.get() { t!("exits.servers.fewer").into() } else { t!("exits.servers.all").into() }
                })),
        )
}

/// A row of the picker: a leading mark, a title, a count or note, the health
/// of what it stands for, and a chevron when it opens.
fn row_button(
    chosen: impl Fn() -> bool + Copy + 'static,
    press: impl Fn() + 'static,
    lead: AnyElement,
    title: Text,
    note: Text,
    chevron: Option<Box<dyn Fn() -> bool>>,
    health: Option<impl Fn() -> Health + Copy + 'static>,
) -> impl IntoElement {
    nsh_base::button()
        .on_press(press)
        .flex_row()
        .items_center()
        .gap_3()
        .h(px(40.0))
        .px_4()
        .cursor_pointer()
        .hover(|s| s.bg(move || theme().accent))
        .style_when(chosen, |s| s.bg(ui::selection))
        .child(view().w(px(22.0)).items_center().child(lead))
        .child(title)
        .child(note.flex_1().number_of_lines(1))
        .child(health.map(|h| {
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                .child(ui::dot(move || h().tone()))
                .child(
                    ui::muted(move || -> SharedString { h().word().into() })
                        .text_size(px(size::small())),
                )
        }))
        .child(chevron.map(|open| {
            dynamic(move || {
                let glyph = if open() { Icon::ChevronDown } else { Icon::ChevronRight };
                untracked(move || icon(glyph, 12.0).text_color(move || theme().muted_foreground))
            })
        }))
}

/// A country's row and, while it is open, its servers.
fn country_rows(
    kind: Kind,
    c: ReadSignal<Country>,
    now: Memo<Option<String>>,
    manual: Memo<bool>,
    all: Signal<bool>,
    open: Signal<Option<&'static str>>,
) -> impl IntoElement {
    let region = c.with_untracked(|c| c.region);
    let is_open = move || all.get() || open.get() == Some(region);
    let holds_now = move || {
        manual.get() && now.with(|n| n.as_deref().is_some_and(|n| c.with(|c| c.servers.iter().any(|s| s.name == n))))
    };
    view()
        .child(row_button(
            move || holds_now() && !is_open(),
            move || open.set(if is_open() && !all.get_untracked() { None } else { Some(region) }),
            ui::flag(c.with_untracked(|c| c.flag.clone()), size::large()),
            ui::body(places::region_title(region)).font_weight(FontWeight::MEDIUM),
            ui::muted(move || -> SharedString { c.with(|c| c.servers.len()).to_string().into() }),
            Some(Box::new(is_open)),
            Some(move || c.with(|c| c.health())),
        ))
        .child(when(is_open, move || {
            view().pb_1().child(each(
                move || c.with(|c| c.servers.clone()),
                |s: &Place| s.name.clone(),
                move |s, _| server_row(kind, s, now, manual),
            ))
        }))
}

/// A server: its name, how it speaks, and its delay (the one place ms show).
fn server_row(kind: Kind, p: ReadSignal<Place>, now: Memo<Option<String>>, manual: Memo<bool>) -> nsh_base::ButtonElement {
    let name = p.with_untracked(|p| p.name.clone());
    let chosen = {
        let name = name.clone();
        computed(move || manual.get() && now.with(|n| n.as_deref() == Some(name.as_str())))
    };
    let delay = move || p.with(|p| p.delay);
    let pick = name.clone();
    nsh_base::button()
        .selected(move || chosen.get())
        .accessibility_label(name)
        .on_press(move || k::pick(kind, pick.clone()))
        .flex_row()
        .items_center()
        .gap_3()
        .h(px(34.0))
        .pl(px(58.0))
        .pr_4()
        .cursor_pointer()
        .hover(|s| s.bg(move || theme().accent))
        .style_when(move || chosen.get(), |s| s.bg(ui::selection))
        .child(
            view()
                .w(px(16.0))
                .child(when(move || chosen.get(), move || icon(Icon::Check, 14.0).text_color(move || theme().primary))),
        )
        .child(ui::body(move || -> SharedString { p.with(|p| p.title.clone()).into() }).number_of_lines(1))
        .child(view().flex_1())
        .child(
            text(move || ui::delay_text(delay()))
                .text_size(px(size::small()))
                .tabular_nums()
                .text_color(move || ui::delay_tone(delay())),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_cards_are_two_columns_on_a_phone_three_where_they_fit() {
        // Phones (the width inside 16-point margins): two equal columns.
        for phone in [328.0, 360.0, 380.0, 392.0] {
            assert_eq!(columns(phone), 2, "{phone}");
        }
        // A wider window below the rail's width, a phone on its side: three.
        assert_eq!(columns(3.0 * CARD_MIN + 2.0 * CARD_GAP), 3);
        assert_eq!(columns(700.0), 3);
        // Unmeasured yet: two, not none.
        assert_eq!(columns(0.0), 2);
        // Both counts fill their rows with the six kinds.
        assert!([2, 3].iter().all(|c| Kind::ALL.len() % c == 0));
    }

    #[test]
    fn delays_read_as_words() {
        assert_eq!(Health::of(None), Health::Untested);
        assert_eq!(Health::of(Some(0)), Health::Slow);
        assert_eq!(Health::of(Some(48)), Health::Good);
        assert_eq!(Health::of(Some(320)), Health::Ok);
        assert_eq!(Health::of(Some(900)), Health::Slow);
    }

    #[test]
    fn a_country_is_as_good_as_its_best_server() {
        let place = |d| Place {
            name: "n".into(),
            title: "n".into(),
            flag: None,
            region: "Japan",
            delay: d,
            is_group: false,
        };
        let c = Country {
            region: "Japan",
            flag: "🇯🇵".into(),
            servers: vec![place(Some(0)), place(Some(300)), place(Some(80))],
        };
        assert_eq!(c.health(), Health::Good);
        let none = Country {
            servers: vec![place(None)],
            ..c.clone()
        };
        assert_eq!(none.health(), Health::Untested);
    }
}
