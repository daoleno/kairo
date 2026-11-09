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
        if *target == Target::Service(service.id) {
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
                    line1: "Not reachable".into(),
                    line2: match &c.api {
                        Api::Unreachable { why } => why.clone(),
                        _ => "No answer through the exit".into(),
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
                    (Some(c), true) => format!("{c} (estimated)"),
                    (None, _) => "Reachable".into(),
                },
                line2: if tools.is_empty() {
                    "Not in use".into()
                } else {
                    tools
                },
                ..base
            }
        })
        .collect()
}

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
    format!("{ok} of {} look right", tiles.len())
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

/// The place AI goes through now, for the exit row and the alert.
pub fn exit_place() -> Option<places::Place> {
    let store = store();
    let name = store.svc().ai_node()?;
    store.proxies.with(|p| {
        p.0.groups
            .iter()
            .flat_map(|g| g.members.iter())
            .find(|m| m.name == name && !m.is_group)
            .map(places::place)
    })
}

fn tone_color(t: Tone) -> AdaptiveColor {
    match t {
        Tone::Ok => ui::success(),
        Tone::Look => ui::warning(),
        Tone::Bad => theme().destructive,
        Tone::Idle => theme().muted_foreground,
    }
}

/// What to show on Home between the switch and the choices.
pub fn board() -> impl IntoElement {
    let store = store();
    let selected = signal(None::<&'static str>);
    let tiles = computed(move || {
        store
            .state
            .with(|s| store.activity.with(|a| tiles(s, &a.0)))
    });
    let checking = move || store.state.with(|s| s.ai.checking);
    when(on, move || {
        view()
            .gap_3()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_1()
                    .child(ui::section_label("AI"))
                    .child(
                        ui::muted(move || -> SharedString {
                            summary(&tiles.get(), checking()).into()
                        })
                        .flex_1(),
                    )
                    .child(
                        ui::link_btn(move || -> SharedString {
                            if checking() {
                                "Checking…".into()
                            } else {
                                "Check again".into()
                            }
                        })
                        .disabled(checking)
                        .on_press(move || store.run(store.svc().check_ai())),
                    ),
            )
            .child(alerts(selected, tiles))
            .child(grid(selected, tiles))
            .child(when(
                move || selected.get().is_some(),
                move || detail(selected),
            ))
            .child(lanes())
    })
}

/// What happened to the exit, and which service sees another country.
fn alerts(selected: Signal<Option<&'static str>>, tiles: Memo<Vec<Tile>>) -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let alert = store.state.with(|s| s.ai.alert.clone());
        let odd = store.state.with(|s| ai_check::odd_ones(&s.ai.checks));
        let tiles = tiles.get();
        untracked(move || {
            let mut out = view().gap_2();
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
                    .unwrap_or_else(|| "your Location".into());
                let pin = place.map(|p| p.name);
                out = out.child(banner(
                    format!("{was} is no longer in your subscription"),
                    format!(
                        "AI now follows your Location ({now}). Sessions already open keep running."
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
            if let Some((majority, ids)) = odd {
                let names: Vec<&str> = tiles
                    .iter()
                    .filter(|t| ids.contains(&t.id))
                    .map(|t| t.name)
                    .collect();
                let first = ids.first().copied();
                out = out.child(banner(
                    format!(
                        "{} {} a different country",
                        names.join(" and "),
                        if names.len() == 1 { "sees" } else { "see" }
                    ),
                    format!(
                        "The others see {}. One exit can look different to services that share it.",
                        ai_check::country_name(&majority)
                    ),
                    vec![Some(
                        ui::capsule("Details", false)
                            .on_press(move || selected.set(first))
                            .into_element(),
                    )],
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

fn grid(selected: Signal<Option<&'static str>>, tiles: Memo<Vec<Tile>>) -> impl IntoElement {
    dynamic(move || {
        let tiles = tiles.get();
        untracked(move || {
            let mut rows = view().gap_3();
            for chunk in tiles.chunks(3) {
                let mut row = view().flex_row().gap_3();
                for t in chunk {
                    row = row.child(tile(t.clone(), selected));
                }
                // A short last row keeps the tiles' width.
                for _ in chunk.len()..3 {
                    row = row.child(view().flex_1());
                }
                rows = rows.child(row);
            }
            rows.into_element()
        })
    })
}

fn tile(t: Tile, selected: Signal<Option<&'static str>>) -> impl IntoElement {
    let id = t.id;
    let tone = t.tone;
    let on = move || selected.get() == Some(id);
    let look = matches!(tone, Tone::Look | Tone::Bad);
    nsh_base::button()
        .accessibility_label(format!("{}: {} {}", t.name, t.line1, t.line2))
        .selected(on)
        .on_press(move || selected.set(if on() { None } else { Some(id) }))
        .flex_1()
        .min_w(px(0.0))
        .gap_3()
        .p_3p5()
        .rounded(px(20.0))
        .cursor_pointer()
        .bg(move || {
            if look {
                tone_color(tone).opacity(0.14)
            } else {
                theme().muted
            }
        })
        .hover(|s| s.bg(move || theme().accent))
        .style_when(on, |s| {
            s.outline(px(2.0)).outline_color(move || theme().primary)
        })
        .focus(|s| s.outline(px(3.0)).outline_color(move || theme().ring))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(mono(t.mono))
                .child(
                    ui::body(t.name)
                        .font_weight(FontWeight::SEMIBOLD)
                        .flex_1()
                        .number_of_lines(1),
                )
                .child(ui::dot(move || tone_color(tone))),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(text(t.flag).text_size(px(28.0)))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            ui::body(t.line1)
                                .font_weight(if look {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .number_of_lines(1),
                        )
                        .child(
                            ui::muted(t.line2)
                                .text_size(px(size::small()))
                                .number_of_lines(1),
                        ),
                ),
        )
}

/// The service's initial, in a soft circle.
pub fn mono(s: &'static str) -> impl IntoElement {
    view()
        .size(px(28.0))
        .flex_shrink_0()
        .rounded_full()
        .items_center()
        .justify_center()
        .bg(move || ui::subtle())
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
                .gap_3()
                .child(lane(
                    "Stays on this computer",
                    "Ollama, LM Studio and your own network are never proxied.",
                ))
                .child(lane(
                    if domestic {
                        "Goes direct"
                    } else {
                        "Domestic AI"
                    },
                    if domestic {
                        "Kimi, DeepSeek, Doubao and Tongyi go straight out."
                    } else {
                        "Kimi, DeepSeek, Doubao and Tongyi follow your subscription."
                    },
                ))
                .into_element()
        })
    })
}

fn lane(title: &'static str, detail: &'static str) -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .gap_0p5()
        .px_4()
        .py_3()
        .rounded(px(18.0))
        .border_1()
        .border_color(move || theme().border)
        .child(ui::body(title).font_weight(FontWeight::MEDIUM))
        .child(ui::muted(detail).text_size(px(size::small())))
}

// --- A service in detail ------------------------------------------------------------

fn detail(selected: Signal<Option<&'static str>>) -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let id = selected.get();
        let Some(service) = id.and_then(|id| ai::SERVICES.iter().find(|s| s.id == id)) else {
            return view().into_element();
        };
        let (check, hold, checking, tile) = store.state.with(|s| {
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
                store
                    .activity
                    .with(|a| tiles(s, &a.0))
                    .into_iter()
                    .find(|t| t.id == service.id),
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
            let tile = tile.unwrap();
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
                        .child(text(tile.flag).text_size(px(26.0)))
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
                .text_color(move || ui::success())
                .into_element(),
            Some(false) => ui::dot(move || ui::warning()).into_element(),
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
    fn all_the_same_country_reads_as_right_and_says_what_it_saw() {
        let t = tiles(&state_with(all("JP")), &Tally::default());
        assert!(t.iter().all(|t| t.tone == Tone::Ok));
        assert_eq!(t[0].line1, "Sees Japan");
        assert_eq!(t[0].flag, "🇯🇵");
        assert_eq!(
            summary(&t, false),
            format!("{0} of {0} look right", t.len())
        );
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
        assert_eq!(c.line1, "Hong Kong (estimated)");
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
            (Tone::Bad, "Not reachable")
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
