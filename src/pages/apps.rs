//! Apps & Sites: the apps and sites that go their own way. Everything not
//! listed uses its usual exit. A choice is an exit (AI, Everything else,
//! Downloads), a country, Direct or Block, and Kairo makes it a rule that
//! comes before everything else it does. The subscription's own groups are
//! one row away, under Inspect.

use kairo_core::places;
use kairo_core::routes::{Route, RouteExit, RouteKind};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, Variant, size};

/// What a row's pop-up offers.
#[derive(Debug, Clone, PartialEq)]
enum Choice {
    Exit(RouteExit),
    /// Back to the usual exit: the row goes.
    Remove,
}

/// The words for an exit.
pub fn exit_title(exit: &RouteExit) -> String {
    match exit {
        RouteExit::Usual => "Usual".into(),
        RouteExit::Direct => "Direct".into(),
        RouteExit::Ai => "AI exit".into(),
        RouteExit::Everything => "Everything else".into(),
        RouteExit::Downloads => "Downloads".into(),
        RouteExit::Block => "Block".into(),
        RouteExit::Node(name) => {
            let p = places::split_flag(name);
            let country = places::place(&kairo_core::model::Node {
                name: name.clone(),
                kind: String::new(),
                udp: false,
                delay: None,
                alive: true,
                is_group: false,
            })
            .region;
            let flag =
                p.0.or_else(|| places::region_flag(country))
                    .unwrap_or_default();
            if country == "Other" {
                format!("{flag} {}", p.1.trim()).trim().to_owned()
            } else {
                format!("{flag} {country}").trim().to_owned()
            }
        }
    }
}

/// The pop-up's choices: the exits, a country (its fastest server), Direct,
/// Block, and the way back to usual.
fn choices(current: &RouteExit) -> Vec<(Choice, String)> {
    let store = store();
    let mut out = vec![
        (Choice::Exit(RouteExit::Ai), exit_title(&RouteExit::Ai)),
        (
            Choice::Exit(RouteExit::Everything),
            exit_title(&RouteExit::Everything),
        ),
        (
            Choice::Exit(RouteExit::Downloads),
            exit_title(&RouteExit::Downloads),
        ),
        (Choice::Exit(RouteExit::Direct), "Direct".into()),
    ];
    let preferred = store
        .state
        .with_untracked(|s| s.settings.tray_group.clone());
    store.proxies.with_untracked(|p| {
        if let Some(g) = places::main_group(&p.0.groups, &preferred) {
            let all = places::places(&p.0.groups, g);
            for (region, list) in places::by_region(&all) {
                let best = list
                    .iter()
                    .filter(|p| p.delay.is_some_and(|d| d > 0))
                    .min_by_key(|p| p.delay)
                    .or(list.first());
                if let Some(best) = best {
                    let exit = RouteExit::Node(best.name.clone());
                    let flag = best
                        .flag
                        .clone()
                        .or_else(|| places::region_flag(region))
                        .unwrap_or_default();
                    out.push((
                        Choice::Exit(exit),
                        format!("{flag} {region}").trim().to_owned(),
                    ));
                }
            }
        }
    });
    out.push((Choice::Exit(RouteExit::Block), "Block".into()));
    // A node chosen earlier that is not its country's fastest now.
    if let RouteExit::Node(_) = current
        && !out.iter().any(|(c, _)| *c == Choice::Exit(current.clone()))
    {
        out.push((Choice::Exit(current.clone()), exit_title(current)));
    }
    out.push((Choice::Remove, "Back to usual".into()));
    out
}

/// A letter in a soft square, for an app or a site.
fn letter(name: &str) -> impl IntoElement {
    let l: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "·".into());
    view()
        .size(px(28.0))
        .flex_shrink_0()
        .rounded(px(8.0))
        .items_center()
        .justify_center()
        .bg(ui::subtle)
        .child(
            text(l)
                .text_size(px(size::small()))
                .font_weight(FontWeight::SEMIBOLD),
        )
}

fn route_row(r: Route) -> impl IntoElement {
    let store = store();
    let (name, detail) = match &r.kind {
        RouteKind::App { name } => (name.clone(), None),
        RouteKind::Site { domain } => (domain.clone(), Some("and subdomains")),
    };
    let kind = stored(r.kind.clone());
    let id = stored(r.id.clone());
    let exit = stored(r.exit.clone());
    let current = r.exit.clone();
    ui::row_view()
        .child(letter(&name))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(ui::body(name).number_of_lines(1))
                .child(detail.map(ui::muted)),
        )
        .child(ui::popup_button(
            move || choices(&current),
            move || Choice::Exit(exit.with(|e| e.clone())),
            move |c: Choice| match c {
                Choice::Exit(e) => store.run(store.svc().set_route(kind.with(|k| k.clone()), e)),
                Choice::Remove => store.run(store.svc().remove_route(id.with(|i| i.clone()))),
            },
        ))
}

/// What the add dialog is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Adding {
    App,
    Site,
}

pub fn page() -> impl IntoElement {
    let store = store();
    let query = signal(String::new());
    let adding = signal(None::<Adding>);
    let routes = computed(move || store.state.with(|s| s.settings.routes.clone()));
    view()
        .flex_1()
        .child(toolbar::floating(
            Page::Apps,
            move || {
                let n = routes.with(|r| r.len());
                format!("{n} {}", if n == 1 { "choice" } else { "choices" })
            },
            vec![
                toolbar::search("apps.search", "Search apps and sites", query),
            ],
        ))
        .child(
            scroll().flex_1().child(
                view()
                    .px_6()
                    .pt(toolbar::scroll_top(20.0))
                    .pb_8()
                    .gap_4()
                    .w_full()
                    .max_w(px(780.0))
                    .self_center()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .px_1()
                            .child(
                                ui::muted("Everything not listed uses its usual exit.").flex_1(),
                            )
                            .child(
                                ui::capsule("Add App…", false)
                                    .h(px(30.0))
                                    .px_4()
                                    .on_press(move || adding.set(Some(Adding::App))),
                            )
                            .child(
                                ui::capsule("Add Site…", false)
                                    .h(px(30.0))
                                    .px_4()
                                    .on_press(move || adding.set(Some(Adding::Site))),
                            ),
                    )
                    .child(dynamic(move || {
                        let all = routes.get();
                        let q = query.get().to_lowercase();
                        untracked(move || {
                            let keep = |r: &Route| {
                                q.is_empty()
                                    || r.id.to_lowercase().contains(&q)
                                    || exit_title(&r.exit).to_lowercase().contains(&q)
                            };
                            let apps: Vec<Route> = all
                                .iter()
                                .filter(|r| matches!(r.kind, RouteKind::App { .. }) && keep(r))
                                .cloned()
                                .collect();
                            let sites: Vec<Route> = all
                                .iter()
                                .filter(|r| matches!(r.kind, RouteKind::Site { .. }) && keep(r))
                                .cloned()
                                .collect();
                            let mut out = view().gap_4();
                            if apps.is_empty() && sites.is_empty() {
                                out = out.child(ui::empty_state(
                                    Icon::Layers,
                                    "Nothing set apart",
                                    "Choose where one app or site goes: the AI exit, another country, direct, or blocked.",
                                    None,
                                ));
                            }
                            if !apps.is_empty() {
                                out = out.child(section("Apps", apps));
                            }
                            if !sites.is_empty() {
                                out = out.child(section("Sites", sites));
                            }
                            out.into_element()
                        })
                    }))
                    .child(subscription_row()),
            ),
        )
        .child(dynamic(move || {
            let what = adding.get();
            untracked(move || match what {
                Some(kind) => sheet(kind, adding).into_element(),
                None => view().into_element(),
            })
        }))
        .text_color(move || theme().foreground)
}

fn section(title: &'static str, routes: Vec<Route>) -> impl IntoElement {
    view()
        .gap_2()
        .child(ui::section_label(title).px_1())
        .child(ui::rows(
            routes
                .into_iter()
                .map(|r| route_row(r).into_element())
                .collect(),
        ))
}

/// The subscription's own per-app groups: a line, and the way to them.
fn subscription_row() -> impl IntoElement {
    let store = store();
    let summary = move || -> (String, String) {
        let preferred = store.state.with(|s| s.settings.tray_group.clone());
        let profile = store
            .state
            .with(|s| s.profiles.active_profile().map(|p| p.name.clone()))
            .unwrap_or_else(|| "Your subscription".into());
        store.proxies.with(|p| {
            let Some(main) = places::main_group(&p.0.groups, &preferred) else {
                return (String::new(), String::new());
            };
            let apps = crate::pages::ai::without_ai(places::app_choices(&p.0.groups, &main.name));
            if apps.is_empty() {
                return (String::new(), String::new());
            }
            let names: Vec<&str> = apps.iter().take(3).map(|a| a.group.as_str()).collect();
            let more = apps.len().saturating_sub(3);
            let title = if more == 0 {
                names.join(", ")
            } else {
                format!("{} and {more} more groups", names.join(", "))
            };
            (
                title,
                format!("{profile}'s own choices · used for anything above set to “usual”"),
            )
        })
    };
    when(
        move || !summary().0.is_empty(),
        move || {
            view()
                .gap_2()
                .child(ui::section_label("From your subscription").px_1())
                .child(ui::rows(vec![
                    ui::row_view()
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    ui::body(move || -> SharedString { summary().0.into() })
                                        .number_of_lines(1),
                                )
                                .child(
                                    ui::muted(move || -> SharedString { summary().1.into() })
                                        .number_of_lines(1),
                                ),
                        )
                        .child(
                            ui::capsule("Show", false)
                                .h(px(30.0))
                                .px_4()
                                .on_press(move || store.page.set(Page::Proxies)),
                        )
                        .into_element(),
                ]))
        },
    )
}

// --- Adding ----------------------------------------------------------------------------

/// The dialog over the page: a dimmed backdrop and the card in the middle.
fn sheet(kind: Adding, adding: Signal<Option<Adding>>) -> impl IntoElement {
    let close = move || adding.set(None);
    view()
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
                .pointer_events(nsh_core::PointerEvents::BoxNone)
                .on_cancel(close)
                .child(add_card(kind, adding)),
        )
}

fn add_card(kind: Adding, adding: Signal<Option<Adding>>) -> impl IntoElement {
    let store = store();
    let value = signal(String::new());
    let exit = signal(Choice::Exit(match kind {
        Adding::App => RouteExit::Direct,
        Adding::Site => RouteExit::Ai,
    }));
    let problem = signal(None::<String>);
    let title = match kind {
        Adding::App => "Add an app",
        Adding::Site => "Add a site",
    };
    let detail = match kind {
        Adding::App => "The app's name as it shows in Activity.",
        Adding::Site => "A site and its subdomains, like example.com.",
    };
    let go = move || {
        let v = value.get_untracked().trim().to_owned();
        if v.is_empty() {
            problem.set(Some("Type a name first.".into()));
            return;
        }
        let Choice::Exit(chosen) = exit.get_untracked() else {
            return;
        };
        let rk = match kind {
            Adding::App => RouteKind::App { name: v },
            Adding::Site => RouteKind::Site { domain: v },
        };
        let svc = store.svc();
        spawn(async move {
            match svc.set_route(rk, chosen).await {
                Ok(()) => adding.set(None),
                Err(e) => problem.set(Some(e)),
            }
        });
    };
    // Apps Activity has seen, to pick from.
    let seen = computed(move || {
        store.activity.with(|a| {
            a.0.apps()
                .into_iter()
                .map(|(n, _)| n.to_owned())
                .filter(|n| n != "Unknown")
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
            text(title)
                .text_size(px(size::large() + 3.0))
                .font_weight(FontWeight::SEMIBOLD)
                .role(AccessibilityRole::Header),
        )
        .child(ui::muted(detail))
        .child(
            ui::field(
                value,
                match kind {
                    Adding::App => "Telegram",
                    Adding::Site => "example.com",
                },
            )
            .auto_focus(true)
            .on_submit(move |_: &str| go()),
        )
        .child(when(
            move || kind == Adding::App && !seen.with(|s| s.is_empty()),
            move || {
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .children(seen.get().into_iter().map(move |n| {
                        let name = n.clone();
                        ui::btn(n, Variant::Secondary)
                            .h(px(26.0))
                            .on_press(move || value.set(name.clone()))
                    }))
            },
        ))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .child(ui::muted("Goes through"))
                .child(view().flex_1())
                .child(ui::popup_button(
                    move || {
                        choices(&RouteExit::Usual)
                            .into_iter()
                            .filter(|(c, _)| *c != Choice::Remove)
                            .collect()
                    },
                    move || exit.get(),
                    move |c| exit.set(c),
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
                .child(ui::btn("Cancel", Variant::Secondary).on_press(move || adding.set(None)))
                .child(ui::btn("Add", Variant::Primary).on_press(go)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exits_are_named_for_people() {
        assert_eq!(exit_title(&RouteExit::Ai), "AI exit");
        assert_eq!(exit_title(&RouteExit::Everything), "Everything else");
        assert_eq!(exit_title(&RouteExit::Block), "Block");
        assert_eq!(
            exit_title(&RouteExit::Node("🇺🇸 Seattle 02".into())),
            "🇺🇸 United States"
        );
    }
}
