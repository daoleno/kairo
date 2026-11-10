//! AI on Home: one tile per service, the country each one sees, and what to
//! look at, when something is off. The facts come from the checks (see
//! `kairo_core::ai_check`); this file decides how they read, and says nothing
//! the checks did not measure: a country is "seen" or "estimated", never
//! "fine", and nothing here promises how a service treats an account.

use kairo_core::activity::{Tally, Target};
use kairo_core::ai::{self, AiService};
use kairo_core::ai_check::{self, Api, Check, Country, Hold};
use kairo_core::places;
use kairo_core::service::{AiAlert, State};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::store::store;
use crate::ui::{self, Icon, icon, size};

// --- What the tiles say --------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Not checked yet.
    Idle,
    /// Reached, and what it sees agrees with the others.
    Ok,
    /// Something to look at.
    Look,
    /// Not reached.
    Bad,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub id: &'static str,
    pub name: &'static str,
    pub mono: &'static str,
    pub tone: Tone,
    /// The flag of the country it sees, or a neutral mark.
    pub flag: String,
    pub line1: String,
    pub line2: String,
}

fn flag(code: &str) -> String {
    places::flag_of(code)
}

/// The tools that talk to a service, most traffic first (at most two).
fn tools_of(tally: &Tally, service: &AiService) -> String {
    let mut by_tool: Vec<(&str, u64, u32)> = Vec::new();
    for (tool, target, f) in tally.flows() {
        // A connection the core could not name a process for is not a tool.
        if *target == Target::Service(service.id) && tool != "Unknown" {
            by_tool.push((tool, f.up + f.down, f.open));
        }
    }
    by_tool.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
    by_tool
        .into_iter()
        .take(2)
        .map(|(t, _, _)| t)
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn tiles(state: &State, tally: &Tally) -> Vec<Tile> {
    let checks = &state.ai.checks;
    let odd = ai_check::odd_ones(checks);
    ai::SERVICES
        .iter()
        .map(|s| {
            let tools = tools_of(tally, s);
            let base = Tile {
                id: s.id,
                name: s.name,
                mono: s.mono,
                tone: Tone::Idle,
                flag: "◎".into(),
                line1: if state.ai.checking {
                    "Checking…".into()
                } else {
                    "Not checked yet".into()
                },
                line2: tools.clone(),
            };
            let Some(c) = checks.iter().find(|c| c.service == s.id) else {
                return base;
            };
            let country = c.country.code().map(ai_check::country_name);
            let flag = c.country.code().map(flag).unwrap_or_else(|| "◎".into());
            let estimated = matches!(c.country, Country::Estimated { .. });
            if !c.reachable {
                return Tile {
                    tone: Tone::Bad,
                    flag,
                    line1: "Unreachable".into(),
                    line2: match &c.api {
                        Api::Unreachable { why } => why.clone(),
                        _ => "No answer".into(),
                    },
                    ..base
                };
            }
            let odd_one = odd.as_ref().is_some_and(|(_, ids)| ids.contains(&s.id));
            if odd_one {
                return Tile {
                    tone: Tone::Look,
                    flag,
                    line1: format!("Sees {}", country.unwrap_or_default()),
                    line2: "Not where the others are".into(),
                    ..base
                };
            }
            if let Api::Unexpected { status } = c.api {
                return Tile {
                    tone: Tone::Look,
                    flag,
                    line1: country
                        .map(|c| format!("Sees {c}"))
                        .unwrap_or_else(|| "Reachable".into()),
                    line2: format!("Its API answered HTTP {status}"),
                    ..base
                };
            }
            if let Api::Unreachable { why } = &c.api {
                return Tile {
                    tone: Tone::Look,
                    flag,
                    line1: "Site reached, API not".into(),
                    line2: why.clone(),
                    ..base
                };
            }
            Tile {
                tone: Tone::Ok,
                flag,
                line1: match (country, estimated) {
                    (Some(c), false) => format!("Sees {c}"),
                    (Some(c), true) => format!("≈ {c}"),
                    (None, _) => "Reachable".into(),
                },
                line2: match (estimated, tools.is_empty()) {
                    (_, false) => tools,
                    (false, true) => "Not in use".into(),
                    (true, true) => "Estimated".into(),
                },
                ..base
            }
        })
        .collect()
}

#[cfg(test)]
/// The line under the board's title.
pub fn summary(tiles: &[Tile], checking: bool) -> String {
    let look = tiles
        .iter()
        .filter(|t| matches!(t.tone, Tone::Look | Tone::Bad))
        .count();
    let ok = tiles.iter().filter(|t| t.tone == Tone::Ok).count();
    if look > 0 {
        return if look == 1 {
            "1 thing to look at".into()
        } else {
            format!("{look} things to look at")
        };
    }
    if ok == 0 {
        return if checking {
            "Checking…".into()
        } else {
            "Not checked yet".into()
        };
    }
    // What was measured, not a verdict: they answered.
    if ok == tiles.len() {
        format!("All {ok} reachable")
    } else {
        format!("{ok} of {} reachable", tiles.len())
    }
}

/// How long ago, for the line under a check.
pub fn ago(now: i64, at: i64) -> String {
    let d = (now - at).max(0);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        _ => format!("{} h ago", d / 3600),
    }
}

fn hold_line(h: Option<&Hold>, checking: bool) -> (Option<bool>, String) {
    match h {
        Some(Hold::Held { secs }) => (Some(true), format!("Open for {secs} s, no drop")),
        Some(Hold::Dropped { after }) => (Some(false), format!("Dropped after {after} s")),
        Some(Hold::Failed { why }) => (None, format!("Could not test: {why}")),
        None if checking => (None, "Testing for 90 s…".into()),
        None => (None, "Not tested yet".into()),
    }
}

// --- The board ---------------------------------------------------------------------

/// Whether the AI layer is on and in the config.
pub fn on() -> bool {
    let store = store();
    store
        .state
        .with(|s| s.settings.ai.enabled && !s.ai.group.is_empty() && s.core.is_running())
}

/// The app choices without the AI group: with the AI layer on, AI is chosen
/// on the board and its exit row, not twice.
pub fn without_ai(mut apps: Vec<places::AppChoice>) -> Vec<places::AppChoice> {
    let store = store();
    let group = store
        .state
        .with(|s| (s.settings.ai.enabled && s.core.is_running()).then(|| s.ai.group.clone()));
    if let Some(g) = group.filter(|g| !g.is_empty()) {
        apps.retain(|a| a.group != g);
    }
    apps
}

/// The place AI goes through now, for the exit row and the alert.
pub fn exit_place() -> Option<places::Place> {
    let store = store();
    // Follow both: the node comes from the service, the place from the groups.
    store.state.with(|_| ());
    store.proxies.with(|_| ());
    let name = store.svc().ai_node()?;
    store.proxies.with(|p| {
        p.0.groups
            .iter()
            .flat_map(|g| g.members.iter())
            .find(|m| m.name == name && !m.is_group)
            .map(places::place)
    })
}

/// The place Downloads go through now.
pub fn downloads_place() -> Option<places::Place> {
    let store = store();
    store.state.with(|_| ());
    store.proxies.with(|_| ());
    let name = store.state.with(|s| s.downloads.node.clone())?;
    store.proxies.with(|p| {
        p.0.groups
            .iter()
            .flat_map(|g| g.members.iter())
            .find(|m| m.name == name && !m.is_group)
            .map(places::place)
    })
}

/// Downloads follow the fastest place by themselves.
pub fn downloads_fastest() -> bool {
    store().state.with(|s| s.downloads.fastest)
}

pub fn set_downloads(name: String) {
    let store = store();
    store.run(store.svc().set_downloads_exit(name));
}

pub fn downloads_to_fastest() {
    let store = store();
    store.run(store.svc().downloads_follow_fastest());
}

fn tone_color(t: Tone) -> AdaptiveColor {
    match t {
        Tone::Ok => ui::success(),
        Tone::Look => ui::warning(),
        Tone::Bad => theme().destructive,
        Tone::Idle => theme().muted_foreground,
    }
}

/// A fix Kairo offers on a row, one button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// The service goes somewhere other than the AI exit: send it there.
    UseAiExit,
    /// The tool does not go through Kairo: take every app (TUN).
    UseEveryApp,
    /// The same, where TUN is not available (Windows for now): a proxy line
    /// for the terminal the tool starts from.
    CopyForTerminal,
}

/// One row of the table on Home.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: &'static str,
    pub name: &'static str,
    pub mono: &'static str,
    pub tone: Tone,
    pub status: String,
    pub sees_flag: String,
    pub sees: String,
    /// What it sees is not where the others are.
    pub odd: bool,
    pub goes: String,
    pub used_by: String,
    pub fix: Option<Fix>,
}

/// Where a service's traffic goes, in the exits' names, and whether that is a
/// problem: the AI exit is where AI belongs.
pub fn goes_through(state: &State, service: &AiService) -> (String, bool) {
    if state.settings.mode == kairo_core::Mode::Direct {
        return ("Direct".into(), true);
    }
    match site_choice(state, service) {
        Some(label) if label != "AI exit" => (label, true),
        _ => ("AI exit".into(), false),
    }
}

/// The routes the user set for this service's sites in Apps & Sites.
fn site_routes<'a>(state: &'a State, service: &AiService) -> Vec<&'a kairo_core::routes::Route> {
    state
        .settings
        .routes
        .iter()
        .filter(|r| match &r.kind {
            kairo_core::routes::RouteKind::Site { domain } => service
                .suffixes
                .iter()
                .any(|s| domain == s || domain.ends_with(&format!(".{s}"))),
            _ => false,
        })
        .collect()
}

/// What the user chose for this service's sites in Apps & Sites, if anything,
/// in the exits' names.
fn site_choice(state: &State, service: &AiService) -> Option<String> {
    use kairo_core::routes::RouteExit as E;
    site_routes(state, service).first().map(|r| match &r.exit {
        E::Ai | E::Usual => "AI exit".to_owned(),
        E::Everything => "Everything else".to_owned(),
        E::Downloads => "Downloads".to_owned(),
        E::Direct => "Direct".to_owned(),
        E::Block => "Blocked".to_owned(),
        E::Node(n) => places::split_flag(n).1.trim().to_owned(),
    })
}

pub fn rows(state: &State, tally: &Tally) -> Vec<Row> {
    let odd = ai_check::odd_ones(&state.ai.checks);
    tiles(state, tally)
        .into_iter()
        .map(|t| {
            let service = ai::SERVICES
                .iter()
                .find(|s| s.id == t.id)
                .expect("a tile is of a service");
            let check = state.ai.checks.iter().find(|c| c.service == t.id);
            let is_odd = odd.as_ref().is_some_and(|(_, ids)| ids.contains(&t.id));
            let (goes, off_exit) = goes_through(state, service);
            let country = check.and_then(|c| {
                c.country.code().map(|code| {
                    let name = ai_check::country_name(code);
                    match c.country {
                        Country::Estimated { .. } => format!("≈ {name}"),
                        _ => name.to_owned(),
                    }
                })
            });
            let (status, sees_flag, sees, tone) = match t.tone {
                Tone::Idle => (t.line1.clone(), "◎".to_owned(), "—".to_owned(), Tone::Idle),
                Tone::Bad => (
                    "Unreachable".to_owned(),
                    "◎".to_owned(),
                    "—".to_owned(),
                    Tone::Bad,
                ),
                Tone::Look if !check.is_some_and(|c| c.reachable) => (
                    "Unreachable".to_owned(),
                    "◎".to_owned(),
                    "—".to_owned(),
                    Tone::Bad,
                ),
                _ => {
                    let api_trouble = matches!(
                        check.map(|c| &c.api),
                        Some(Api::Unexpected { .. } | Api::Unreachable { .. })
                    );
                    (
                        if api_trouble {
                            "API not answering".to_owned()
                        } else {
                            "Reachable".to_owned()
                        },
                        t.flag.clone(),
                        country.clone().unwrap_or_else(|| "—".into()),
                        if api_trouble { Tone::Look } else { Tone::Ok },
                    )
                }
            };
            let bypassing = state.bypass.iter().any(|b| b.service == Some(t.id));
            let fix = if bypassing {
                if state.tun_privilege == Some(kairo_core::tun::Privilege::Unsupported) {
                    Some(Fix::CopyForTerminal)
                } else {
                    Some(Fix::UseEveryApp)
                }
            } else {
                (off_exit && state.settings.mode != kairo_core::Mode::Direct)
                    .then_some(Fix::UseAiExit)
            };
            let (goes, status, sees_flag, sees, tone) = if bypassing {
                (
                    "Direct".to_owned(),
                    "Not through Kairo".to_owned(),
                    "◎".to_owned(),
                    "your own network".to_owned(),
                    Tone::Bad,
                )
            } else {
                (goes, status, sees_flag, sees, tone)
            };
            Row {
                id: t.id,
                name: t.name,
                mono: t.mono,
                tone,
                status,
                sees_flag,
                sees,
                odd: is_odd,
                goes,
                used_by: if t.line2 == "Not where the others are"
                    || t.line2.starts_with("Its API")
                    || t.line2 == "No answer"
                    || t.line2 == "Estimated"
                    || t.line2 == "Not in use"
                {
                    String::new()
                } else {
                    t.line2.clone()
                },
                fix,
            }
        })
        .collect()
}

/// The line under the table's title.
pub fn table_summary(rows: &[Row], checking: bool) -> (String, bool) {
    let problems = rows
        .iter()
        .filter(|r| matches!(r.tone, Tone::Bad | Tone::Look) || r.fix.is_some())
        .count();
    if problems > 0 {
        return (
            if problems == 1 {
                "1 problem".into()
            } else {
                format!("{problems} problems")
            },
            true,
        );
    }
    let ok = rows.iter().filter(|r| r.tone == Tone::Ok).count();
    if ok == 0 {
        return (
            if checking {
                "Checking…".into()
            } else {
                "Not checked yet".into()
            },
            false,
        );
    }
    // What was measured: where they answered and what they see.
    let flags: std::collections::BTreeSet<&str> = rows
        .iter()
        .filter(|r| r.tone == Tone::Ok && r.sees != "—")
        .map(|r| r.sees.as_str())
        .collect();
    let text = match (ok == rows.len(), flags.len()) {
        (true, 1) => format!("All {ok} see {}", flags.iter().next().unwrap_or(&"")),
        (true, _) => format!("All {ok} reachable"),
        (false, _) => format!("{ok} of {} reachable", rows.len()),
    };
    (text, false)
}

/// What to show on Home under the exits: the AI services, one row each.
pub fn board() -> impl IntoElement {
    let store = store();
    let selected = signal(None::<&'static str>);
    let rows = computed(move || store.state.with(|s| store.activity.with(|a| rows(s, &a.0))));
    let checking = move || store.state.with(|s| s.ai.checking);
    when(on, move || {
        view().gap_3().child(alerts()).child(
            crate::ui::card_soft()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .px_5()
                        .pt_4()
                        .pb_2()
                        .child(ui::body("AI").font_weight(FontWeight::SEMIBOLD))
                        .child(
                            text(move || -> SharedString {
                                table_summary(&rows.get(), checking()).0.into()
                            })
                            .text_size(px(size::body()))
                            .text_color(move || {
                                if table_summary(&rows.get(), checking()).1 {
                                    ui::warning()
                                } else {
                                    theme().muted_foreground
                                }
                            })
                            .flex_1(),
                        )
                        .child(
                            ui::capsule(
                                move || -> SharedString {
                                    if checking() {
                                        "Checking…".into()
                                    } else {
                                        "Check Now".into()
                                    }
                                },
                                false,
                            )
                            .h(px(30.0))
                            .px_4()
                            .disabled(checking)
                            .on_press(move || store.run(store.svc().check_ai())),
                        ),
                )
                .child(when(|| !ui::phone(), header_row))
                .child(dynamic(move || {
                    let rows = rows.get();
                    untracked(move || {
                        view()
                            .children(rows.into_iter().map(move |r| table_row(r, selected)))
                            .into_element()
                    })
                }))
                .child(when(
                    move || selected.get().is_some(),
                    move || view().p_4().child(detail(selected)),
                ))
                .child(lanes()),
        )
    })
}

const COLS: [f32; 5] = [0.22, 0.20, 0.18, 0.17, 0.23];

fn cell(i: usize) -> View {
    view()
        .w(percentage(COLS[i]))
        .flex_shrink_0()
        .min_w(px(0.0))
        .flex_row()
        .items_center()
        .gap_2()
        .pr_2()
}

fn header_row() -> impl IntoElement {
    let head = |i: usize, t: &'static str| {
        cell(i).child(
            ui::muted(t)
                .text_size(px(size::small()))
                .font_weight(FontWeight::MEDIUM),
        )
    };
    view()
        .flex_row()
        .items_center()
        .px_5()
        .h(px(28.0))
        .child(head(0, "Service"))
        .child(head(1, "Status"))
        .child(head(2, "Sees"))
        .child(head(3, "Goes through"))
        .child(head(4, "Used by").justify_end())
}

/// A phone's row: the service and its status on one line, where it sees and goes
/// through (or the fix) under it.
fn phone_row(r: Row, selected: Signal<Option<&'static str>>) -> impl IntoElement {
    let id = r.id;
    let tone = r.tone;
    let problem = matches!(tone, Tone::Bad | Tone::Look) || r.fix.is_some();
    let store = store();
    nsh_base::button()
        .w_full()
        .accessibility_label(format!(
            "{}: {} · sees {} · {}",
            r.name, r.status, r.sees, r.goes
        ))
        .on_press(move || {
            selected.set(if selected.get_untracked() == Some(id) {
                None
            } else {
                Some(id)
            })
        })
        .gap_1()
        .px_4()
        .py_2p5()
        .border_t_1()
        .border_color(move || theme().border.opacity(0.7))
        .bg(move || {
            if problem {
                tone_color(if tone == Tone::Ok { Tone::Look } else { tone }).opacity(0.12)
            } else {
                AdaptiveColor::from(black().opacity(0.0))
            }
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(mono(r.mono))
                .child(ui::body(r.name).number_of_lines(1).flex_1())
                .child(ui::dot(move || tone_color(tone)))
                .child(ui::body(r.status.clone()).number_of_lines(1)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .pl_8()
                .child(ui::flag(r.sees_flag.clone(), size::body()))
                .child(
                    ui::muted(format!("{} · {}", r.sees, r.goes))
                        .text_size(px(size::small()))
                        .number_of_lines(1)
                        .flex_1(),
                )
                .children(r.fix.map(|fix| {
                    ui::capsule(
                        match fix {
                            Fix::UseAiExit => "Use AI Exit",
                            Fix::UseEveryApp => "Use Every App",
                            Fix::CopyForTerminal => "Copy for Terminal",
                        },
                        true,
                    )
                    .h(px(28.0))
                    .px_3p5()
                    .on_press(move || apply_fix(store, id, fix))
                })),
        )
}

fn table_row(r: Row, selected: Signal<Option<&'static str>>) -> impl IntoElement {
    if ui::phone() {
        return phone_row(r, selected).into_element();
    }
    table_row_wide(r, selected).into_element()
}

fn table_row_wide(r: Row, selected: Signal<Option<&'static str>>) -> impl IntoElement {
    let id = r.id;
    let tone = r.tone;
    let problem = matches!(tone, Tone::Bad | Tone::Look) || r.fix.is_some();
    let store = store();
    nsh_base::button()
        .w_full()
        .accessibility_label(format!(
            "{}: {} · sees {} · {}",
            r.name, r.status, r.sees, r.goes
        ))
        .on_press(move || {
            selected.set(if selected.get_untracked() == Some(id) {
                None
            } else {
                Some(id)
            })
        })
        .flex_row()
        .items_center()
        .px_5()
        .min_h(px(42.0))
        .border_t_1()
        .border_color(move || theme().border.opacity(0.7))
        .cursor_pointer()
        .bg(move || {
            if problem {
                tone_color(if tone == Tone::Ok { Tone::Look } else { tone }).opacity(0.12)
            } else {
                AdaptiveColor::from(black().opacity(0.0))
            }
        })
        .hover(|s| s.bg(move || theme().accent.opacity(0.5)))
        .child(
            cell(0)
                .child(mono(r.mono))
                .child(ui::body(r.name).number_of_lines(1)),
        )
        .child(
            cell(1)
                .child(ui::dot(move || tone_color(tone)))
                .child(ui::body(r.status.clone()).number_of_lines(1)),
        )
        .child(
            cell(2)
                .child(ui::flag(r.sees_flag.clone(), size::large()))
                .child(
                    ui::body(r.sees.clone())
                        .number_of_lines(1)
                        .font_weight(if r.odd {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(move || {
                            if r.odd {
                                ui::warning()
                            } else {
                                theme().foreground
                            }
                        }),
                ),
        )
        .child(
            cell(3).child(
                ui::muted(r.goes.clone())
                    .text_size(px(size::body()))
                    .number_of_lines(1),
            ),
        )
        .child(
            cell(4).justify_end().child(match r.fix {
                Some(fix) => ui::capsule(
                    match fix {
                        Fix::UseAiExit => "Use AI Exit",
                        Fix::UseEveryApp => "Use Every App",
                        Fix::CopyForTerminal => "Copy for Terminal",
                    },
                    true,
                )
                .h(px(28.0))
                .px_3p5()
                .on_press(move || apply_fix(store, id, fix))
                .into_element(),
                None => ui::muted(r.used_by.clone())
                    .number_of_lines(1)
                    .into_element(),
            }),
        )
}

/// Carries out a row's fix.
fn apply_fix(store: crate::store::Store, id: &'static str, fix: Fix) {
    match fix {
        Fix::UseAiExit => use_ai_exit(store, id),
        Fix::UseEveryApp => crate::pages::home::set_via(crate::pages::home::Via::Tun),
        Fix::CopyForTerminal => {
            set_clipboard(store.svc().terminal_env());
            store.info("Copied. Paste it in the terminal before you start the tool.");
        }
    }
}

/// Sends the service's sites to the AI exit (drops the choices that took it
/// elsewhere).
fn use_ai_exit(store: crate::store::Store, id: &'static str) {
    let Some(service) = ai::SERVICES.iter().find(|s| s.id == id) else {
        return;
    };
    let ids: Vec<String> = store.state.with_untracked(|s| {
        site_routes(s, service)
            .into_iter()
            .map(|r| r.id.clone())
            .collect()
    });
    for rid in ids {
        store.run(store.svc().remove_route(rid));
    }
}

/// What happened to the exit, and the route being Direct.
fn alerts() -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let alert = store.state.with(|s| s.ai.alert.clone());
        let direct = store
            .state
            .with(|s| s.settings.mode == kairo_core::Mode::Direct);
        untracked(move || {
            let mut out = view().gap_2();
            if direct {
                out = out.child(banner(
                    "Mode is Direct".to_owned(),
                    "Everything goes out directly, so AI tools are not using the exit and nothing is checked."
                        .to_owned(),
                    vec![Some(
                        ui::capsule("Use Smart", true)
                            .on_press(move || {
                                store.run(store.svc().set_mode(kairo_core::Mode::Rule))
                            })
                            .into_element(),
                    )],
                ));
            }
            if let Some(AiAlert::ExitGone { was }) = alert {
                let place = exit_place();
                let now = place
                    .as_ref()
                    .map(|p| {
                        format!(
                            "{}{}",
                            p.flag.clone().map(|f| f + " ").unwrap_or_default(),
                            p.title
                        )
                    })
                    .unwrap_or_else(|| "Everything else".into());
                let pin = place.map(|p| p.name);
                out = out.child(banner(
                    format!("{was} is no longer in your subscription"),
                    format!(
                        "AI now follows Everything else ({now}). Sessions already open keep running."
                    ),
                    vec![
                        pin.map(|name| {
                            ui::capsule("Pin it", true)
                                .on_press(move || store.run(store.svc().set_ai_exit(name.clone())))
                                .into_element()
                        }),
                        Some(
                            ui::capsule("OK", false)
                                .on_press(move || store.svc().dismiss_ai_alert())
                                .into_element(),
                        ),
                    ],
                ));
            }
            out.into_element()
        })
    })
}

fn banner(title: String, detail: String, actions: Vec<Option<AnyElement>>) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_3()
        .rounded(px(18.0))
        .bg(move || ui::warning().opacity(0.14))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap_0p5()
                .child(ui::body(title).font_weight(FontWeight::SEMIBOLD))
                .child(ui::muted(detail)),
        )
        .children(actions.into_iter().flatten())
}

/// The service's initial, in a soft circle.
pub fn mono(s: &'static str) -> impl IntoElement {
    view()
        .size(px(28.0))
        .flex_shrink_0()
        .rounded_full()
        .items_center()
        .justify_center()
        .bg(ui::subtle)
        .child(
            text(s)
                .text_size(px(size::small()))
                .font_weight(FontWeight::SEMIBOLD),
        )
}

/// What stays here, and what goes direct: two facts, not controls.
fn lanes() -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let domestic = store.state.with(|s| s.settings.ai.domestic_direct);
        untracked(move || {
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .px_5()
                .h(px(36.0))
                .border_t_1()
                .border_color(move || theme().border.opacity(0.7))
                .child(
                    ui::muted(if domestic {
                        "Direct: Kimi, DeepSeek, Doubao, Tongyi"
                    } else {
                        "Kimi, DeepSeek, Doubao, Tongyi follow your subscription"
                    })
                    .flex_1()
                    .number_of_lines(1),
                )
                .child(when(|| !cfg!(target_os = "android"), || {
                    ui::muted("On this computer: Ollama, LM Studio").number_of_lines(1)
                }))
                .into_element()
        })
    })
}

// --- A service in detail ------------------------------------------------------------

fn detail(selected: Signal<Option<&'static str>>) -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let id = selected.get();
        let Some(service) = id.and_then(|id| ai::SERVICES.iter().find(|s| s.id == id)) else {
            return view().into_element();
        };
        let (check, hold, checking) = store.state.with(|s| {
            (
                s.ai.checks
                    .iter()
                    .find(|c| c.service == service.id)
                    .cloned(),
                s.ai.holds
                    .iter()
                    .find(|(k, _)| k == service.id)
                    .map(|(_, h)| h.clone()),
                s.ai.checking,
            )
        });
        untracked(move || {
            let now = kairo_core::service::unix();
            let rows: Vec<AnyElement> = match &check {
                None => vec![
                    ui::muted(if checking {
                        "Checking…"
                    } else {
                        "Not checked yet. Check again to see."
                    })
                    .px_1()
                    .into_element(),
                ],
                Some(c) => checks_rows(service, c, hold.as_ref(), checking, now),
            };
            let hosts = service
                .suffixes
                .iter()
                .take(3)
                .copied()
                .collect::<Vec<_>>()
                .join(" · ");
            let flag_text = check
                .as_ref()
                .and_then(|c| c.country.code().map(flag))
                .unwrap_or_else(|| "◎".into());
            ui::card()
                .p_4()
                .gap_3()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(mono(service.mono))
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(ui::body(service.name).font_weight(FontWeight::SEMIBOLD))
                                .child(ui::muted(hosts).text_size(px(size::small()))),
                        )
                        .child(text(flag_text).text_size(px(26.0)))
                        .child(ui::icon_btn(Icon::Close, "Close").on_press(move || selected.set(None))),
                )
                .children(rows)
                .child(
                    ui::muted("Kairo shows what the service sees. Where you are, and the service's own terms, still apply.")
                        .text_size(px(size::small())),
                )
                .into_element()
        })
    })
}

fn checks_rows(
    service: &'static AiService,
    c: &Check,
    hold: Option<&Hold>,
    checking: bool,
    now: i64,
) -> Vec<AnyElement> {
    let mut out = Vec::new();
    out.push(check_row(
        Some(c.reachable),
        "Reachable",
        if c.reachable {
            "Through the exit".into()
        } else {
            "No answer".into()
        },
    ));
    let (ok, line) = match &c.api {
        Api::Answers { ms } => (Some(true), format!("Answers in {ms} ms")),
        Api::Unexpected { status } => (
            Some(false),
            format!("Answered HTTP {status}, not what it gives without a key"),
        ),
        Api::Unreachable { why } => (Some(false), why.clone()),
        Api::None => (None, "Kairo has no way to ask this one".into()),
    };
    if service.api.is_some() || !matches!(c.api, Api::None) {
        out.push(check_row(ok, "API", line));
    }
    let (ok, line) = match &c.country {
        Country::Seen { code, warp } => (
            Some(true),
            format!(
                "Sees {}{}",
                ai_check::country_name(code),
                if *warp {
                    " · through Cloudflare WARP"
                } else {
                    ""
                }
            ),
        ),
        Country::Estimated { code } => (
            None,
            format!(
                "About {} · estimated: this service does not say",
                ai_check::country_name(code)
            ),
        ),
        Country::Unknown => (None, "Not known".into()),
    };
    out.push(check_row(ok, "Country", line));
    let (ok, line) = hold_line(hold, checking);
    out.push(check_row(ok, "A connection held open", line));
    out.push(
        ui::muted(format!("Checked {}", ago(now, c.at)))
            .text_size(px(size::small()))
            .px_1()
            .into_element(),
    );
    out
}

fn check_row(ok: Option<bool>, title: &'static str, value: String) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .child(match ok {
            Some(true) => icon(Icon::Check, 15.0)
                .text_color(ui::success)
                .into_element(),
            Some(false) => ui::dot(ui::warning).into_element(),
            None => ui::dot(move || theme().muted_foreground.opacity(0.5)).into_element(),
        })
        .child(ui::body(title).w(px(170.0)).flex_shrink_0())
        .child(ui::muted(value).flex_1().min_w(px(0.0)))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kairo_core::activity::parse_time;
    use kairo_core::model::Connection;

    fn state_with(checks: Vec<Check>) -> State {
        let mut s = State::default();
        s.ai.checks = checks;
        s
    }

    fn check(service: &'static str, code: &str, warp: bool, api: Api) -> Check {
        Check {
            service,
            reachable: true,
            api,
            country: Country::Seen {
                code: code.into(),
                warp,
            },
            at: 100,
        }
    }

    fn all(code: &str) -> Vec<Check> {
        ai::SERVICES
            .iter()
            .map(|s| check(s.id, code, false, Api::Answers { ms: 300 }))
            .collect()
    }

    #[test]
    fn nothing_checked_says_so() {
        let t = tiles(&State::default(), &Tally::default());
        assert_eq!(t.len(), ai::SERVICES.len());
        assert!(t.iter().all(|t| t.tone == Tone::Idle));
        assert_eq!(summary(&t, false), "Not checked yet");
        assert_eq!(summary(&t, true), "Checking…");
    }

    #[test]
    fn all_the_same_country_says_what_it_saw_and_no_more() {
        let t = tiles(&state_with(all("JP")), &Tally::default());
        assert!(t.iter().all(|t| t.tone == Tone::Ok));
        assert_eq!(t[0].line1, "Sees Japan");
        assert_eq!(t[0].flag, "🇯🇵");
        assert_eq!(summary(&t, false), format!("All {} reachable", t.len()));
    }

    #[test]
    fn one_service_seeing_another_country_is_the_one_to_look_at() {
        let mut checks = all("JP");
        checks
            .iter_mut()
            .find(|c| c.service == "gemini")
            .unwrap()
            .country = Country::Seen {
            code: "HK".into(),
            warp: false,
        };
        let t = tiles(&state_with(checks), &Tally::default());
        let g = t.iter().find(|t| t.id == "gemini").unwrap();
        assert_eq!(g.tone, Tone::Look);
        assert_eq!(g.line1, "Sees Hong Kong");
        assert_eq!(g.line2, "Not where the others are");
        assert_eq!(summary(&t, false), "1 thing to look at");
        assert_eq!(t.iter().filter(|t| t.tone == Tone::Ok).count(), t.len() - 1);
    }

    #[test]
    fn an_estimate_is_marked_and_never_makes_a_difference() {
        let mut checks = all("JP");
        let c = checks.iter_mut().find(|c| c.service == "cursor").unwrap();
        c.country = Country::Estimated { code: "HK".into() };
        let t = tiles(&state_with(checks), &Tally::default());
        let c = t.iter().find(|t| t.id == "cursor").unwrap();
        assert_eq!(c.tone, Tone::Ok);
        assert_eq!(c.line1, "≈ Hong Kong");
        assert_eq!(c.line2, "Estimated");
    }

    #[test]
    fn unreachable_and_odd_api_are_told_apart() {
        let mut checks = all("JP");
        {
            let c = checks.iter_mut().find(|c| c.service == "claude").unwrap();
            c.reachable = false;
            c.api = Api::Unreachable {
                why: "no answer in 12 s".into(),
            };
            c.country = Country::Unknown;
        }
        checks
            .iter_mut()
            .find(|c| c.service == "chatgpt")
            .unwrap()
            .api = Api::Unexpected { status: 500 };
        let t = tiles(&state_with(checks), &Tally::default());
        let claude = t.iter().find(|t| t.id == "claude").unwrap();
        assert_eq!(
            (claude.tone, claude.line1.as_str()),
            (Tone::Bad, "Unreachable")
        );
        assert_eq!(claude.line2, "no answer in 12 s");
        let chatgpt = t.iter().find(|t| t.id == "chatgpt").unwrap();
        assert_eq!(chatgpt.tone, Tone::Look);
        assert_eq!(chatgpt.line2, "Its API answered HTTP 500");
        assert_eq!(summary(&t, false), "2 things to look at");
    }

    #[test]
    fn the_tools_using_a_service_show_under_it() {
        let mut tally = Tally::default();
        let start = "2026-11-09T15:00:00Z";
        let now = parse_time(start).unwrap();
        let conn = |id: &str, host: &str, process: &str, path: &str| Connection {
            id: id.into(),
            host: host.into(),
            port: "443".into(),
            network: "tcp".into(),
            inbound: String::new(),
            process: process.into(),
            process_path: path.into(),
            source: String::new(),
            destination_ip: String::new(),
            rule: String::new(),
            rule_payload: String::new(),
            chains: vec![],
            upload: 10,
            download: 100,
            up_speed: 0,
            down_speed: 0,
            start: start.into(),
        };
        tally.ingest(
            &[
                conn("1", "api.anthropic.com", "claude", "/bin/claude"),
                conn(
                    "2",
                    "claude.ai",
                    "Claude",
                    "/Applications/Claude.app/Contents/MacOS/Claude",
                ),
            ],
            now + 5,
        );
        let t = tiles(&state_with(all("JP")), &tally);
        assert_eq!(t[0].id, "claude");
        assert!(t[0].line2.contains("Claude"), "{}", t[0].line2);
        // Services nobody uses say so.
        assert_eq!(
            t.iter().find(|t| t.id == "grok").unwrap().line2,
            "Not in use"
        );
    }

    #[test]
    fn says_how_long_ago() {
        assert_eq!(ago(100, 100), "just now");
        assert_eq!(ago(400, 100), "5 min ago");
        assert_eq!(ago(100 + 7200, 100), "2 h ago");
        assert_eq!(ago(50, 100), "just now");
    }

    #[test]
    fn the_hold_reads_plainly() {
        assert_eq!(
            hold_line(Some(&Hold::Held { secs: 90 }), false),
            (Some(true), "Open for 90 s, no drop".into())
        );
        assert_eq!(
            hold_line(Some(&Hold::Dropped { after: 14 }), false).0,
            Some(false)
        );
        assert_eq!(hold_line(None, true).1, "Testing for 90 s…");
        assert_eq!(hold_line(None, false).1, "Not tested yet");
    }
}
