//! Exits: where traffic leaves. Three exits, each a place: AI (pinned, never
//! switched under an open session), Everything else (the Location, the
//! fastest by itself unless chosen) and Downloads. A card per exit on the left;
//! the selected exit's picker on the right: countries first, a server one level
//! deeper. Latency reads Good / OK / Slow; milliseconds only show in the list
//! of servers.

use kairo_core::places::{self, Place};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::i18n::plural;
use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, icon, size};

/// The three exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ai,
    Rest,
    Downloads,
}

impl Exit {
    pub const ALL: [Exit; 3] = [Exit::Ai, Exit::Rest, Exit::Downloads];

    pub fn title(self) -> String {
        kairo_core::tr!(match self {
            Exit::Ai => "exits.title.ai",
            Exit::Rest => "exits.title.rest",
            Exit::Downloads => "exits.title.downloads",
        })
    }

    /// Whether Kairo can keep it on the fastest place by itself. The AI exit
    /// is never switched automatically: a switch would cut a running session.
    pub fn has_fastest(self) -> bool {
        !matches!(self, Exit::Ai)
    }
}

// --- What an exit is now ---------------------------------------------------------------

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

/// The place `exit` goes through now.
pub fn place_of(exit: Exit) -> Option<Place> {
    let store = store();
    match exit {
        Exit::Ai => crate::pages::ai::exit_place(),
        Exit::Rest => {
            let preferred = store.state.with(|s| s.settings.tray_group.clone());
            store.proxies.with(|p| {
                let g = places::main_group(&p.0.groups, &preferred)?;
                places::current(&p.0.groups, g)
            })
        }
        Exit::Downloads => crate::pages::ai::downloads_place(),
    }
}

/// The line under an exit's country: which server, and why it is there.
pub fn subtitle(exit: Exit) -> String {
    let store = store();
    let place = place_of(exit);
    let node = place
        .as_ref()
        .map(|p| p.title.clone())
        .unwrap_or_else(|| "—".into());
    use kairo_core::tr;
    let how = |fastest: bool| {
        if fastest {
            tr!("exits.subtitle.fastest", node = node)
        } else {
            tr!("exits.subtitle.chosen", node = node)
        }
    };
    match exit {
        Exit::Ai => {
            let pinned = store
                .state
                .with(|s| s.settings.ai.exit.as_ref().map(|e| e.since));
            match pinned {
                Some(since) if pin_days(since) == 0 => tr!("exits.subtitle.pinned_today", node = node),
                Some(since) => tr!("exits.subtitle.pinned", node = node, when = pinned_for(since)),
                None => tr!("exits.subtitle.follows", node = node),
            }
        }
        Exit::Rest => how(store.state.with(|s| s.settings.auto_location)),
        Exit::Downloads => how(crate::pages::ai::downloads_fastest()),
    }
}

/// Whole days since a pin was made.
fn pin_days(since: i64) -> i64 {
    (kairo_core::unix() - since).max(0) / 86400
}

/// "today", "2 days", "3 weeks": how long ago a pin was made.
pub fn pinned_for(since: i64) -> String {
    let days = pin_days(since);
    match days {
        0 => kairo_core::tr!("exits.pinned_for.today"),
        1..=13 => kairo_core::tr!(&plural("exits.pinned_for.days", days as u64), count = days),
        _ => kairo_core::tr!("exits.pinned_for.weeks", count = days / 7),
    }
}

/// An exit's card: used on Home and on Exits.
pub fn card(
    exit: Exit,
    selected: impl Fn() -> bool + Copy + 'static,
    press: impl Fn() + 'static,
) -> impl IntoElement {
    card_with(exit, selected, press, false)
}

/// The card, with a small line of how the exit has been doing (Home's).
pub fn card_with(
    exit: Exit,
    selected: impl Fn() -> bool + Copy + 'static,
    press: impl Fn() + 'static,
    spark: bool,
) -> impl IntoElement {
    let store = store();
    let place = computed(move || {
        // Follow what the place is made of.
        store.state.with(|_| ());
        store.proxies.with(|_| ());
        place_of(exit)
    });
    let health = move || place.with(|p| Health::of(p.as_ref().and_then(|p| p.delay)));
    nsh_base::button()
        .w_full()
        .accessibility_label(exit.title())
        .selected(selected)
        .on_press(press)
        .gap_1p5()
        .px_4()
        .py_3()
        .rounded(px(20.0))
        .cursor_pointer()
        .bg(move || theme().muted)
        .hover(|s| s.bg(move || theme().accent))
        .style_when(selected, |s| {
            s.outline(px(2.0)).outline_color(move || theme().primary)
        })
        .focus(|s| s.outline(px(3.0)).outline_color(move || theme().ring))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    ui::muted(exit.title())
                        .font_weight(FontWeight::MEDIUM)
                        .flex_1()
                        .number_of_lines(1),
                )
                .child(when(move || exit == Exit::Ai, move || ui::badge(t!("exits.badge.stable"))))
                .child(ui::dot(move || health().tone()))
                .child(
                    ui::muted(move || -> SharedString { health().word().into() })
                        .text_size(px(size::small())),
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
                    untracked(move || ui::flag(f, size::large() + 3.0))
                }))
                .child(
                    text(move || -> SharedString {
                        place
                            .with(|p| p.as_ref().map(|p| place_country(p)))
                            .unwrap_or_else(|| "—".into())
                            .into()
                    })
                    .text_size(px(size::large() + 2.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .number_of_lines(if ui::phone() { 2 } else { 1 })
                    .flex_1(),
                )
                .child(when(
                    move || spark,
                    move || {
                        crate::pages::stats::home_spark(match exit {
                            Exit::Ai => 0,
                            Exit::Rest => 1,
                            Exit::Downloads => 2,
                        })
                    },
                )),
        )
        .child(
            ui::muted(move || -> SharedString {
                store.state.with(|_| ());
                store.proxies.with(|_| ());
                subtitle(exit).into()
            })
            .number_of_lines(if ui::phone() { 2 } else { 1 }),
        )
}

/// The country a place is in, in words ("Hong Kong"), from its region.
pub fn place_country(p: &Place) -> String {
    if p.region == "Other" {
        p.title.clone()
    } else {
        places::region_title(p.region)
    }
}

// --- The page --------------------------------------------------------------------------

thread_local! {
    static SELECTED: std::cell::Cell<Exit> = const { std::cell::Cell::new(Exit::Rest) };
}

/// The exit the page opens on (a card on Home).
pub fn select(exit: Exit) {
    SELECTED.with(|s| s.set(exit));
}

pub fn page() -> impl IntoElement {
    let which = signal(SELECTED.with(|s| s.get()));
    effect(move || SELECTED.with(|s| s.set(which.get())));
    view()
        .flex_1()
        .child(toolbar::floating(
            Page::Exits,
            || t!("exits.page.subtitle"),
            vec![],
        ))
        .child(
            scroll().flex_1().child({
                let phone = ui::phone();
                let page = view()
                    .px(px(if phone { 16.0 } else { 24.0 }))
                    .pt(toolbar::scroll_top(20.0))
                    .pb_8()
                    .w_full()
                    .max_w(px(1040.0))
                    .self_center()
                    .gap_4();
                // Cards beside the picker; on a phone, the cards above it.
                let page = if phone { page } else { page.flex_row().items_start() };
                page
                    .child({
                        let cards = view().flex_shrink_0().gap_3();
                        if phone { cards.w_full() } else { cards.w(px(280.0)) }
                    }.children(
                        Exit::ALL.into_iter().map(move |e| {
                            card(e, move || which.get() == e, move || which.set(e)).into_element()
                        }),
                    ))
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap_4()
                            .child(dynamic(move || {
                                let e = which.get();
                                untracked(move || picker(e).into_element())
                            }))
                            .child(when(move || which.get() == Exit::Downloads, mirror_card))
                            .child(dynamic(move || {
                                let e = which.get();
                                untracked(move || quality_card(e).into_element())
                            })),
                    )
            }),
        )
}

/// How the selected exit has been doing: response time, failed tests, switches.
fn quality_card(exit: Exit) -> impl IntoElement {
    let idx = match exit {
        Exit::Ai => 0,
        Exit::Rest => 1,
        Exit::Downloads => 2,
    };
    ui::card()
        .rounded(px(22.0))
        .child(
            view()
                .px_4()
                .py_3()
                .child(ui::body(t!("exits.quality")).font_weight(FontWeight::SEMIBOLD)),
        )
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
    let info = computed(move || store.state.with(|s| s.downloads.clone()));
    let running =
        move || info.with(|d| matches!(d.mirror_test, kairo_core::downloads::MirrorTest::Running));
    let can_switch = move || info.with(|d| d.mirror_test.passed());
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
                            info.with(|d| mirror_text(&d.mirror_test)).into()
                        })),
                )
                .child(
                    ui::btn(t!("exits.mirror.test"), Variant::Secondary)
                        .disabled(running)
                        .on_press(move || store.run(store.svc().test_mirror())),
                )
                .child(
                    view().opacity(move || if can_switch() || info.with(|d| d.mirror) { 1.0_f32 } else { 0.4_f32 }).child(
                        ui::toggle(
                            move || info.with(|d| d.mirror),
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
            move || info.with(|d| d.mirror),
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

fn countries(exit: Exit) -> Vec<Country> {
    let store = store();
    let preferred = store.state.with(|s| s.settings.tray_group.clone());
    store.proxies.with(|p| {
        let Some(g) = places::main_group(&p.0.groups, &preferred) else {
            return vec![];
        };
        let all = places::places(&p.0.groups, g);
        let here = place_of(exit).map(|p| p.region);
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

/// Picks `name` for `exit`.
fn pick(exit: Exit, name: String) {
    let store = store();
    match exit {
        Exit::Ai => store.run(store.svc().set_ai_exit(name)),
        Exit::Rest => store.run(store.svc().set_location(Some(name))),
        Exit::Downloads => crate::pages::ai::set_downloads(name),
    }
}

/// "Fastest" for the exits that have it.
fn use_fastest(exit: Exit) {
    let store = store();
    match exit {
        Exit::Ai => {}
        Exit::Rest => store.run(store.svc().set_location(None)),
        Exit::Downloads => crate::pages::ai::downloads_to_fastest(),
    }
}

/// Whether `exit` is on "Fastest" now.
fn on_fastest(exit: Exit) -> bool {
    let store = store();
    match exit {
        Exit::Ai => false,
        Exit::Rest => store.state.with(|s| s.settings.auto_location),
        Exit::Downloads => crate::pages::ai::downloads_fastest(),
    }
}

fn picker(exit: Exit) -> impl IntoElement {
    let store = store();
    let open = signal(None::<&'static str>);
    let all = signal(false);
    let list = computed(move || {
        store.proxies.with(|_| ());
        countries(exit)
    });
    let testing = move || store.proxies.with(|p| !p.0.testing.is_empty());
    // The exit's server now, to check it in the list.
    let current = move || {
        store.state.with(|_| ());
        store.proxies.with(|_| ());
        place_of(exit).map(|p| p.name)
    };
    // Open the country the exit is in, once it is known.
    effect(move || {
        store.state.with(|_| ());
        store.proxies.with(|_| ());
        let here = place_of(exit);
        untracked(move || {
            if open.get().is_none()
                && let Some(p) = here
            {
                open.set(Some(p.region));
            }
        });
    });
    // On a phone the title and the button wrap instead of pushing the button off the card,
    // and the one-line description (no room for it) is left out.
    let phone = ui::phone();
    let head = view().flex_row().items_center().gap_2().px_4().py_3();
    let head = if phone { head.flex_wrap() } else { head };
    ui::card()
        .rounded(px(22.0))
        .child(
            head.child(ui::body(exit.title()).font_weight(FontWeight::SEMIBOLD))
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
            move || exit.has_fastest(),
            move || {
                row_button(
                    move || on_fastest(exit),
                    move || use_fastest(exit),
                    icon(Icon::Bolt, 15.0)
                        .text_color(move || theme().primary)
                        .into_element(),
                    ui::body(t!("exits.fastest.title")).font_weight(FontWeight::MEDIUM),
                    ui::muted(move || -> SharedString {
                        store.state.with(|_| ());
                        store.proxies.with(|_| ());
                        match place_of(exit) {
                            Some(p) if on_fastest(exit) => t!("exits.fastest.now", node = p.title).into(),
                            _ => t!("exits.fastest.automatic").into(),
                        }
                    }),
                    None,
                    Some(move || Health::of(place_of(exit).and_then(|p| p.delay))),
                )
            },
        ))
        .child(when(
            move || exit == Exit::Ai && store.state.with(|s| s.settings.ai.exit.is_some()),
            move || {
                row_button(
                    move || false,
                    move || store.run(store.svc().ai_follow_location()),
                    icon(Icon::Globe, 15.0)
                        .text_color(move || theme().muted_foreground)
                        .into_element(),
                    ui::body(t!("exits.follow_rest")),
                    ui::muted(""),
                    None,
                    None::<fn() -> Health>,
                )
            },
        ))
        .child(dynamic(move || {
            let countries = list.get();
            let now = current();
            let expand_all = all.get();
            let open_now = open.get();
            let manual = !on_fastest(exit);
            untracked(move || {
                view()
                    .children(countries.into_iter().map(move |c| {
                        let is_open = expand_all || open_now == Some(c.region);
                        let region = c.region;
                        let health = c.health();
                        let count = c.servers.len();
                        let holds_now = manual
                            && now
                                .as_deref()
                                .is_some_and(|n| c.servers.iter().any(|s| s.name == n));
                        let servers = c.servers.clone();
                        let now = now.clone();
                        view()
                            .child(row_button(
                                move || holds_now && !is_open,
                                move || {
                                    open.set(if is_open && !expand_all {
                                        None
                                    } else {
                                        Some(region)
                                    })
                                },
                                ui::flag(c.flag.clone(), size::large()),
                                ui::body(places::region_title(region)).font_weight(FontWeight::MEDIUM),
                                ui::muted(format!("{count}")),
                                Some(is_open),
                                Some(move || health),
                            ))
                            .child(when(
                                move || is_open,
                                move || {
                                    let servers = servers.clone();
                                    let now = now.clone();
                                    view().pb_1().children(servers.into_iter().map(move |s| {
                                        let chosen =
                                            manual && now.as_deref() == Some(s.name.as_str());
                                        let name = s.name.clone();
                                        server_row(s, chosen)
                                            .on_press(move || pick(exit, name.clone()))
                                    }))
                                },
                            ))
                            .into_element()
                    }))
                    .into_element()
            })
        }))
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
                    if all.get() {
                        t!("exits.servers.fewer").into()
                    } else {
                        t!("exits.servers.all").into()
                    }
                })),
        )
}

use crate::ui::Variant;

/// A row of the picker: a leading mark, a title, a count or note, the health
/// of what it stands for, and a chevron when it opens.
fn row_button(
    chosen: impl Fn() -> bool + Copy + 'static,
    press: impl Fn() + 'static,
    lead: AnyElement,
    title: Text,
    note: Text,
    chevron: Option<bool>,
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
            icon(
                if open {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                12.0,
            )
            .text_color(move || theme().muted_foreground)
        }))
}

/// A server: its name, how it speaks, and its delay (the one place ms show).
fn server_row(p: Place, chosen: bool) -> nsh_base::ButtonElement {
    let tone = ui::delay_tone(p.delay);
    nsh_base::button()
        .selected(chosen)
        .accessibility_label(p.name.clone())
        .flex_row()
        .items_center()
        .gap_3()
        .h(px(34.0))
        .pl(px(58.0))
        .pr_4()
        .cursor_pointer()
        .hover(|s| s.bg(move || theme().accent))
        .style_when(move || chosen, |s| s.bg(ui::selection))
        .child(
            view()
                .w(px(16.0))
                .child(chosen.then(|| icon(Icon::Check, 14.0).text_color(move || theme().primary))),
        )
        .child(ui::body(p.title.clone()).number_of_lines(1))
        .child(view().flex_1())
        .child(
            text(ui::delay_text(p.delay))
                .text_size(px(size::small()))
                .tabular_nums()
                .text_color(move || tone),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_pin_is_dated_in_days() {
        let now = kairo_core::unix();
        assert_eq!(pinned_for(now), "today");
        assert_eq!(pinned_for(now - 5 * 86400), "5 days");
        assert_eq!(pinned_for(now - 21 * 86400), "3 weeks");
    }
}
