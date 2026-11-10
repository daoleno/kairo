//! Statistics: what Kairo has recorded of your own use, as charts. Traffic by
//! exit, apps and AI services (Activity), how reliable each AI service has
//! been (Activity, and a page of its own), how the exits have been doing
//! (Exits › Quality), what a subscription has left (Subscriptions), and a
//! small line on Home. All of it is recorded on this computer and goes
//! nowhere.

use kairo_core::ai;
use kairo_core::stats::{self, AiHour, Cell, DAY, HOUR, Range, Slot, Store as Stats};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::chrome::Glyph;
use crate::store::{Page, store};
use crate::ui::charts::{self, Col, Line, Mark, Row, Ser, Tone};
use crate::ui::{self, Icon, size, toolbar};

pub fn now() -> i64 {
    kairo_core::service::unix()
}

fn tz(t: i64) -> i64 {
    stats::local_offset(t)
}

// --- Dates (local time, no library) -------------------------------------------------

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// (month 0..11, day of month, weekday 0=Sun, hour) of a local time in seconds.
pub fn civil(local: i64) -> (usize, i64, usize, i64) {
    let days = local.div_euclid(DAY);
    let hour = local.rem_euclid(DAY) / HOUR;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let _ = era;
    let weekday = (days + 4).rem_euclid(7) as usize;
    ((m - 1) as usize, d, weekday, hour)
}

/// "Nov 10" for a UTC time, in local time.
pub fn day_text(t: i64) -> String {
    let (m, d, _, _) = civil(t + tz(t));
    format!("{} {d}", MONTHS[m])
}

/// "Tue, Nov 10".
fn day_long(t: i64) -> String {
    let (m, d, w, _) = civil(t + tz(t));
    format!("{}, {} {d}", WEEKDAYS[w], MONTHS[m])
}

fn hour_text(t: i64) -> String {
    let (_, _, _, h) = civil(t + tz(t));
    format!("{h:02}:00")
}

/// The unit (KB, MB, GB) a chart of at most `max` bytes is rounded in.
fn unit_of(max: u64) -> f64 {
    let kb = 1024.0;
    if max as f64 >= kb * kb * kb {
        kb * kb * kb
    } else if max as f64 >= kb * kb {
        kb * kb
    } else {
        kb
    }
}

fn bytes_f(v: f64) -> String {
    kairo_core::bytes(v.max(0.0) as u64)
}

fn ms_f(v: f64) -> String {
    format!("{} ms", v.round() as i64)
}

fn count_f(v: f64) -> String {
    format!("{}", v.round() as i64)
}

fn slot_color(s: Slot) -> fn() -> AdaptiveColor {
    match s {
        Slot::Ai => ui::chart_ai,
        Slot::Rest => ui::chart_rest,
        Slot::Downloads => ui::chart_downloads,
        Slot::Direct => ui::chart_direct,
    }
}

fn card() -> View {
    ui::card_soft().p_4().gap_2()
}

fn heading(title: &str, sub: impl Into<SharedString>) -> impl IntoElement {
    view()
        .flex_row()
        .items_baseline()
        .gap_2()
        .child(ui::body(title.to_owned()).font_weight(FontWeight::SEMIBOLD))
        .child(ui::muted(sub.into()).flex_1())
}

// --- Activity: the overview ---------------------------------------------------------

/// The charts above Activity's list: a range, traffic by exit, top apps and
/// AI services, and how reliable each AI service has been.
pub fn overview() -> impl IntoElement {
    let store = store();
    let range = signal(Range::Today);
    view()
        .gap_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .child(ui::segmented(
                    vec![
                        (Range::Today, "Today"),
                        (Range::Week, "7 days"),
                        (Range::Month, "30 days"),
                    ],
                    move || range.get(),
                    move |r| range.set(r),
                ))
                .child(view().flex_1()),
        )
        .child(dynamic(move || {
            let r = range.get();
            let snap = store.stats.get();
            untracked(move || content(&snap.0, r).into_element())
        }))
}

fn content(st: &Stats, range: Range) -> impl IntoElement {
    let t = now();
    let z = tz(t);
    if st.collecting(t) {
        let ago = if st.first_at == 0 {
            "just now".to_owned()
        } else {
            let m = ((t - st.first_at) / 60).max(0);
            format!("{m} minute{} ago", if m == 1 { "" } else { "s" })
        };
        return card()
            .child(charts::collecting(
                "Collecting data",
                &format!(
                    "Kairo started recording {ago}. The first chart appears after an hour of use. It is recorded on this computer and never sent anywhere."
                ),
            ))
            .into_element();
    }
    let series = st.traffic(range, t, z);
    let ai_total: u64 = series.buckets.iter().map(|b| b.x[0]).sum();
    let hourly = series.per == stats::Per::Hour;
    let cols: Vec<Col> = series
        .buckets
        .iter()
        .map(|b| Col {
            vals: b.x.iter().map(|v| *v as f64).collect(),
            tip: if hourly {
                format!(
                    "Today, {}{}",
                    hour_text(b.start),
                    if b.start <= t && t < b.start + HOUR {
                        " (so far)"
                    } else {
                        ""
                    }
                )
            } else {
                format!(
                    "{}{}",
                    day_long(b.start),
                    if b.start <= t && t < b.start + DAY {
                        " (so far)"
                    } else {
                        ""
                    }
                )
            },
            open: b.open,
        })
        .collect();
    let labels: Vec<charts::XLabel> = series
        .buckets
        .iter()
        .enumerate()
        .filter_map(|(i, b)| match range {
            Range::Today if i % 3 == 0 => {
                Some((i, 3, hour_text(b.start).trim_end_matches(":00").to_owned()))
            }
            Range::Week => {
                let (_, d, w, _) = civil(b.start + z);
                Some((i, 1, format!("{} {d}", WEEKDAYS[w])))
            }
            Range::Month if i % 5 == 0 => Some((i, 5, day_text(b.start))),
            _ => None,
        })
        .collect();
    let ser: Vec<Ser> = Slot::ALL
        .iter()
        .map(|s| Ser {
            name: s.title().to_owned(),
            color: slot_color(*s),
        })
        .collect();
    let when = match range {
        Range::Today => "today",
        Range::Week => "in 7 days",
        Range::Month => "in 30 days",
    };
    let share = if series.total > 0 {
        format!(
            " ({}%)",
            (ai_total as f64 / series.total as f64 * 100.0).round()
        )
    } else {
        String::new()
    };
    let traffic = card()
        .child(heading(
            "Traffic",
            format!(
                "{} {when} · AI {}{share}",
                kairo_core::bytes(series.total),
                kairo_core::bytes(ai_total)
            ),
        ))
        .child(ui::muted(if hourly {
            "Per hour, by the exit it left through"
        } else {
            "Per day, by the exit it left through"
        }))
        .child(charts::legend(
            Slot::ALL
                .iter()
                .map(|s| (s.title().to_owned(), (slot_color(*s))()))
                .collect(),
        ))
        .child(if series.total == 0 {
            view()
                .py_4()
                .child(ui::muted("No traffic in this range yet."))
                .into_element()
        } else {
            charts::columns(
                ser,
                cols,
                bytes_f,
                labels,
                170.0,
                "Move over a column for exact values".into(),
                unit_of(series.buckets.iter().map(|b| b.total()).max().unwrap_or(0)),
            )
            .into_element()
        })
        .child(clear_footer());
    let apps = st.top_apps(range, t, z);
    let apps_total: u64 = apps.iter().map(|a| a.1).sum();
    let services = st.top_services(range, t, z);
    let ai_sum: u64 = services.iter().map(|a| a.1).sum();
    let top = view()
        .flex_row()
        .gap_3()
        .child(
            card()
                .flex_1()
                .min_w(px(0.0))
                .child(heading("Top apps", range.title()))
                .child(charts::ranked(
                    apps.iter()
                        .take(6)
                        .map(|(n, v)| Row {
                            name: n.clone(),
                            value: *v as f64,
                            label: kairo_core::bytes(*v),
                            color: ui::chart_neutral,
                            lead: None,
                        })
                        .collect(),
                    "No traffic yet.",
                )),
        )
        .child(
            card()
                .flex_1()
                .min_w(px(0.0))
                .child(heading("AI services", "share of AI traffic"))
                .child(charts::ranked(
                    services
                        .iter()
                        .take(6)
                        .filter_map(|(id, v)| {
                            let s = ai::SERVICES.iter().find(|s| s.id == id)?;
                            let pct = if ai_sum > 0 { *v * 100 / ai_sum } else { 0 };
                            Some(Row {
                                name: s.name.to_owned(),
                                value: *v as f64,
                                label: format!("{} · {pct}%", kairo_core::bytes(*v)),
                                color: ui::chart_ai,
                                lead: Some(crate::pages::ai::mono(s.mono).into_element()),
                            })
                        })
                        .collect(),
                    "No AI traffic yet.",
                )),
        );
    let _ = apps_total;
    view()
        .gap_3()
        .child(traffic)
        .child(top)
        .child(reliability(st, range, t))
        .into_element()
}

fn clear_footer() -> impl IntoElement {
    let store = store();
    let sure = signal(false);
    view()
        .flex_row()
        .items_center()
        .child(
            ui::muted("Recorded on this computer · kept 30 days")
                .flex_1()
                .text_size(px(size::small())),
        )
        .child(
            ui::link_btn(move || -> SharedString {
                if sure.get() {
                    "Press again to clear".into()
                } else {
                    "Clear History…".into()
                }
            })
            .on_press(move || {
                if sure.get_untracked() {
                    sure.set(false);
                    store.svc().clear_stats();
                    store.info("History cleared.");
                } else {
                    sure.set(true);
                }
            }),
        )
}

// --- AI reliability ---------------------------------------------------------------------

fn tone_of(c: Cell) -> Tone {
    match c {
        Cell::None => Tone::None,
        Cell::Ok => Tone::Good,
        Cell::Part => Tone::Part,
        Cell::Down => Tone::Bad,
    }
}

/// Cells of `group` hours each, the worst of each.
fn cells(hours: &[(i64, Option<AiHour>)], group: usize) -> Vec<(Tone, String)> {
    hours
        .chunks(group)
        .map(|ch| {
            let worst = ch
                .iter()
                .map(|(_, h)| Cell::of(h.as_ref()))
                .fold(Cell::None, Cell::worst);
            let (from, to) = (ch[0].0, ch[ch.len() - 1].0 + HOUR);
            let when = if group == 1 {
                format!("{}, {}", day_long(from), hour_text(from))
            } else if group >= 24 {
                day_long(from)
            } else {
                format!("{}, {}–{}", day_long(from), hour_text(from), hour_text(to))
            };
            let (c, p): (u32, u32) = ch
                .iter()
                .filter_map(|(_, h)| h.as_ref())
                .fold((0, 0), |(c, p), h| (c + h.checks, p + h.passed));
            let what = match worst {
                Cell::None => "not checked".to_owned(),
                Cell::Ok => "reachable".to_owned(),
                Cell::Part => format!("some checks failed ({p} of {c} passed)"),
                Cell::Down => format!("unreachable ({p} of {c} passed)"),
            };
            (tone_of(worst), format!("{when} · {what}"))
        })
        .collect()
}

fn reliability(st: &Stats, range: Range, t: i64) -> impl IntoElement {
    let store = store();
    let (days, group) = match range {
        Range::Today => (1, 1),
        Range::Week => (7, 6),
        Range::Month => (30, 24),
    };
    let from = range.from(t, tz(t));
    let bytes = st.top_services(range, t, tz(t));
    let rows: Vec<AnyElement> = ai::SERVICES
        .iter()
        .map(|s| {
            let mut hours = st.ai_hours(s.id, t, days);
            if range == Range::Today {
                hours.retain(|(h, _)| *h >= from);
            }
            let up = stats::uptime(&hours);
            let ms = stats::median(
                hours
                    .iter()
                    .filter_map(|(_, h)| h.as_ref())
                    .flat_map(|h| h.api_ms.clone())
                    .collect(),
            );
            let data = bytes
                .iter()
                .find(|(id, _)| id == s.id)
                .map(|x| kairo_core::bytes(x.1))
                .unwrap_or_else(|| "–".into());
            let id = s.id;
            let cells = cells(&hours, group);
            let tones: Vec<Tone> = cells.iter().map(|c| c.0).collect();
            nsh_base::button()
                .w_full()
                .accessibility_label(format!("{}: open its week", s.name))
                .on_press(move || {
                    store.ai_service.set(id);
                    store.page.set(Page::AiService);
                })
                .flex_row()
                .items_center()
                .gap_3()
                .px_1()
                .min_h(px(36.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .hover(|st| st.bg(move || theme().accent.opacity(0.5)))
                .child(
                    view()
                        .w(px(120.0))
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(crate::pages::ai::mono(s.mono))
                        .child(ui::body(s.name).number_of_lines(1)),
                )
                .child(view().flex_1().child(charts::ribbon_bare(tones, 14.0)))
                .child(
                    ui::muted(up.map_or("–".to_owned(), |u| format!("{u:.1}%")))
                        .tabular_nums()
                        .w(px(52.0)),
                )
                .child(
                    ui::muted(ms.map_or("–".to_owned(), |m| format!("{m} ms")))
                        .tabular_nums()
                        .w(px(84.0)),
                )
                .child(ui::muted(data).tabular_nums().w(px(70.0)))
                .into_element()
        })
        .collect();
    card()
        .child(heading(
            "AI services",
            format!(
                "reachability and API response time · {}",
                match range {
                    Range::Today => "today",
                    Range::Week => "last 7 days",
                    Range::Month => "last 30 days",
                }
            ),
        ))
        .child(charts::legend(vec![
            ("Reachable".into(), ui::success()),
            ("Some checks failed".into(), ui::warning()),
            ("Unreachable".into(), ui::chart_bad()),
        ]))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .px_1()
                .child(ui::muted("Service").w(px(120.0)))
                .child(ui::muted("Reachable").flex_1())
                .child(ui::muted("Up").w(px(52.0)))
                .child(ui::muted("API median").w(px(84.0)))
                .child(ui::muted("Data").w(px(70.0))),
        )
        .child(view().children(rows))
        .child(ui::muted(
            "From Kairo's own checks every 10 minutes. API time is how long the API took to answer an unauthenticated request. Open a row for its week.",
        ))
}

// --- One AI service -------------------------------------------------------------------

fn stat(label: &str, value: String, detail: String) -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .p_3()
        .rounded(px(14.0))
        .bg(move || theme().muted)
        .gap_0p5()
        .child(ui::muted(label.to_owned()))
        .child(
            text(value)
                .text_size(px(size::title()))
                .font_weight(FontWeight::SEMIBOLD)
                .number_of_lines(2),
        )
        .child(ui::muted(detail).number_of_lines(2))
}

/// The page of one AI service: its week of checks and API times.
pub fn ai_service() -> impl IntoElement {
    let store = store();
    view()
        .flex_1()
        .child(toolbar::floating(
            Page::AiService,
            move || {
                let id = store.ai_service.get();
                ai::SERVICES
                    .iter()
                    .find(|s| s.id == id)
                    .map_or(String::new(), |s| s.name.to_owned())
            },
            vec![toolbar::button(
                "back",
                "Activity",
                Glyph::Back,
                Icon::ChevronLeft,
                move || store.page.set(Page::Activity),
            )],
        ))
        .child(
            scroll().flex_1().child(
                view()
                    .px_6()
                    .pt(toolbar::scroll_top(20.0))
                    .pb_6()
                    .gap_3()
                    .w_full()
                    .max_w(px(900.0))
                    .self_center()
                    .child(dynamic(move || {
                        let id = store.ai_service.get();
                        let snap = store.stats.get();
                        let state = store.state.get();
                        untracked(move || service_content(&snap.0, &state, id).into_element())
                    })),
            ),
        )
}

fn service_content(st: &Stats, state: &kairo_core::State, id: &'static str) -> impl IntoElement {
    let t = now();
    let Some(s) = ai::SERVICES.iter().find(|s| s.id == id) else {
        return view().into_element();
    };
    let hours = st.ai_hours(id, t, 7);
    let up = stats::uptime(&hours);
    let (checks, passed): (u32, u32) = hours
        .iter()
        .filter_map(|(_, h)| h.as_ref())
        .fold((0, 0), |(c, p), h| (c + h.checks, p + h.passed));
    let samples: Vec<u32> = hours
        .iter()
        .filter_map(|(_, h)| h.as_ref())
        .flat_map(|h| h.api_ms.clone())
        .collect();
    let med = stats::median(samples.clone());
    let p95 = stats::percentile(samples, 0.95);
    let (holds, held): (u32, u32) = hours
        .iter()
        .filter_map(|(_, h)| h.as_ref())
        .fold((0, 0), |(c, p), h| (c + h.holds, p + h.held));
    // The countries, in order, and when each began.
    let mut countries: Vec<(i64, String)> = Vec::new();
    for (h, a) in &hours {
        if let Some(c) = a.as_ref().and_then(|a| a.country.clone())
            && countries.last().is_none_or(|l| l.1 != c)
        {
            countries.push((*h, c));
        }
    }
    let now_check = state.ai.checks.iter().find(|c| c.service == id);
    let reachable_now = now_check.map(|c| c.reachable);
    let (state_text, state_tone) = match reachable_now {
        Some(true) => ("Reachable now", ui::success()),
        Some(false) => ("Unreachable now", ui::chart_bad()),
        None => ("Not checked yet", theme().muted_foreground),
    };
    let days: Vec<Vec<(i64, Option<AiHour>)>> = hours.chunks(24).map(|c| c.to_vec()).collect();
    let grid_rows: Vec<(String, Vec<(Tone, String)>)> = days
        .iter()
        .map(|d| {
            let (_, _, w, _) = civil(d[0].0 + tz(d[0].0));
            (WEEKDAYS[w].to_owned(), cells(d, 1))
        })
        .collect();
    let api_vals: Vec<Option<f64>> = hours
        .iter()
        .map(|(_, h)| stats::api_median(h).map(f64::from))
        .collect();
    let tips: Vec<String> = hours
        .iter()
        .map(|(h, _)| format!("{}, {}", day_long(*h), hour_text(*h)))
        .collect();
    let labels: Vec<charts::XLabel> = hours
        .iter()
        .enumerate()
        .filter(|(_, (h, _))| (h + tz(*h)).rem_euclid(DAY) / HOUR == 0)
        .map(|(i, (h, _))| {
            let (_, _, w, _) = civil(h + tz(*h));
            (i, 24, WEEKDAYS[w].to_owned())
        })
        .collect();
    let line = charts::lines(
        vec![Line {
            name: "API response".into(),
            color: ui::chart_neutral,
            vals: api_vals.clone(),
            area: true,
            dashed: false,
        }],
        tips,
        ms_f,
        labels,
        vec![],
        None,
        170.0,
        "Move over the line for the hour's median".into(),
    );
    let goes = {
        let g = crate::pages::ai::goes_through(state, s);
        g.0
    };
    view()
        .gap_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .child(crate::pages::ai::mono(s.mono))
                .child(
                    view()
                        .flex_1()
                        .gap_0p5()
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    ui::body(s.name)
                                        .text_size(px(size::title() + 2.0))
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(ui::dot(move || state_tone))
                                .child(ui::muted(state_text)),
                        )
                        .child(ui::muted(format!("Goes through {goes}"))),
                ),
        )
        .child(
            view()
                .flex_row()
                .gap_3()
                .child(stat(
                    "Reachable, 7 days",
                    up.map_or("–".into(), |u| format!("{u:.1}%")),
                    if checks == 0 {
                        "no checks yet".into()
                    } else {
                        format!("{} of {checks} checks failed", checks - passed)
                    },
                ))
                .child(stat(
                    "API response, median",
                    med.map_or("–".into(), |m| format!("{m} ms")),
                    p95.map_or("no answers yet".into(), |p| format!("95% under {p} ms")),
                ))
                .child(stat(
                    "Country it sees",
                    countries
                        .last()
                        .map_or("–".into(), |c| kairo_core::ai_check::country_name(&c.1)),
                    match countries.len() {
                        0 => "no answer yet".into(),
                        1 => "no change this week".into(),
                        n => format!("{} changes this week", n - 1),
                    },
                ))
                .child(stat(
                    "Held open 90 s",
                    if holds == 0 {
                        "–".into()
                    } else {
                        format!("{held} of {holds}")
                    },
                    if holds == 0 {
                        "hourly test".into()
                    } else {
                        format!("hourly test · {} dropped", holds - held)
                    },
                )),
        )
        .child(
            card()
                .child(heading("Was it reachable?", "each square is one hour"))
                .child(charts::legend(vec![
                    ("All checks passed".into(), ui::success()),
                    ("Some failed".into(), ui::warning()),
                    ("Mostly failed".into(), ui::chart_bad()),
                ]))
                .child(charts::grid(
                    grid_rows,
                    "Move over a square for its hour".into(),
                )),
        )
        .child(
            card()
                .child(heading("API response time", "hourly median, last 7 days"))
                .child(if api_vals.iter().all(Option::is_none) {
                    ui::muted("No API answers recorded yet.").into_element()
                } else {
                    line.into_element()
                }),
        )
        .child(
            card()
                .child(heading("Country it sees", ""))
                .child(view().gap_1().children(if countries.is_empty() {
                    vec![ui::muted("Nothing recorded yet.").into_element()]
                } else {
                    countries
                        .iter()
                        .rev()
                        .enumerate()
                        .map(|(i, (at, c))| {
                            view()
                                .flex_row()
                                .gap_3()
                                .child(
                                    ui::muted(format!("{} {}", day_text(*at), hour_text(*at)))
                                        .w(px(120.0)),
                                )
                                .child(ui::body(kairo_core::ai_check::country_name(c)))
                                .child(ui::muted(if i == 0 { "now" } else { "" }))
                                .into_element()
                        })
                        .collect()
                })),
        )
        .into_element()
}

// --- Exits: quality --------------------------------------------------------------------

/// How an exit has been doing: response time, failed tests, and every switch.
/// `exit` is 0 (AI), 1 (everything else) or 2 (downloads).
pub fn quality(exit: usize) -> impl IntoElement {
    let store = store();
    let week = signal(exit == 0);
    view()
        .gap_3()
        .p_4()
        .child(
            view()
                .flex_row()
                .items_center()
                .child(ui::muted("From Kairo's own tests every 10 minutes").flex_1())
                .child(ui::segmented(
                    vec![(false, "24 hours"), (true, "7 days")],
                    move || week.get(),
                    move |w| week.set(w),
                )),
        )
        .child(dynamic(move || {
            let w = week.get();
            let snap = store.stats.get();
            untracked(move || quality_content(&snap.0, exit, w).into_element())
        }))
}

fn quality_content(st: &Stats, exit: usize, week: bool) -> impl IntoElement {
    let t = now();
    let key = stats::EXITS[exit];
    let hours = if week { 7 * 24 } else { 24 };
    let data = st.exit_hours(key, t, hours);
    if data.iter().all(|(_, h)| h.is_none()) {
        return charts::collecting(
            "Collecting data",
            "Kairo tests the server each exit is on every ten minutes. The first chart appears after an hour of use.",
        )
        .into_element();
    }
    let since = data[0].0;
    let meds: Vec<Option<f64>> = data
        .iter()
        .map(|(_, h)| stats::exit_median(h).map(f64::from))
        .collect();
    let all: Vec<u32> = data
        .iter()
        .filter_map(|(_, h)| h.as_ref())
        .flat_map(|h| h.ms.clone())
        .collect();
    let (tests, fails): (u32, u32) = data
        .iter()
        .filter_map(|(_, h)| h.as_ref())
        .fold((0, 0), |(a, b), h| (a + h.tests, b + h.fails));
    let switches = st.switches_of(key, since);
    let by_you = switches.iter().filter(|s| !s.auto).count();
    let tips: Vec<String> = data
        .iter()
        .map(|(h, _)| {
            if week {
                format!("{}, {}", day_long(*h), hour_text(*h))
            } else {
                format!("Today, {}", hour_text(*h))
            }
        })
        .collect();
    let labels: Vec<charts::XLabel> = data
        .iter()
        .enumerate()
        .filter_map(|(i, (h, _))| {
            let local = h + tz(*h);
            if week {
                (local.rem_euclid(DAY) / HOUR == 0).then(|| {
                    let (_, _, w, _) = civil(local);
                    (i, 24, WEEKDAYS[w].to_owned())
                })
            } else {
                (i % 3 == 0).then(|| (i, 3, hour_text(*h).trim_end_matches(":00").to_owned()))
            }
        })
        .collect();
    let marks: Vec<Mark> = switches
        .iter()
        .filter_map(|s| {
            let i = data
                .iter()
                .position(|(h, _)| s.at >= *h && s.at < *h + HOUR)?;
            Some(Mark {
                at: i,
                text: format!(
                    "{} {} → {}",
                    if s.auto { "Automatic:" } else { "You:" },
                    s.from,
                    s.to
                ),
            })
        })
        .collect();
    let color: fn() -> AdaptiveColor = if exit == 0 {
        ui::chart_ai
    } else {
        ui::chart_rest
    };
    let line = charts::lines(
        vec![Line {
            name: "Response".into(),
            color,
            vals: meds,
            area: true,
            dashed: false,
        }],
        tips.clone(),
        ms_f,
        labels.clone(),
        marks,
        None,
        150.0,
        "Move over the line for the hour's median".into(),
    );
    let fail_cols: Vec<Col> = data
        .iter()
        .enumerate()
        .map(|(i, (_, h))| Col {
            vals: vec![h.as_ref().map_or(0.0, |h| f64::from(h.fails))],
            tip: tips[i].clone(),
            open: h.is_none(),
        })
        .collect();
    let failed = charts::columns(
        vec![Ser {
            name: "Failed tests".into(),
            color: ui::chart_bad,
        }],
        fail_cols,
        count_f,
        vec![],
        80.0,
        "Failed tests per hour".into(),
        1.0,
    );
    let node_now = data
        .iter()
        .rev()
        .find_map(|(_, h)| h.as_ref().map(|h| h.node.clone()))
        .unwrap_or_default();
    view()
        .gap_3()
        .child(
            view()
                .flex_row()
                .gap_3()
                .child(stat(
                    "Typical response",
                    stats::median(all.clone()).map_or("–".into(), |m| format!("{m} ms")),
                    "median".into(),
                ))
                .child(stat(
                    "Failed tests",
                    fails.to_string(),
                    format!("of {tests}"),
                ))
                .child(stat(
                    "Changes",
                    switches.len().to_string(),
                    if switches.is_empty() {
                        "none".into()
                    } else {
                        format!("{by_you} by you · {} automatic", switches.len() - by_you)
                    },
                ))
                .child(stat("Right now", node_now, String::new())),
        )
        .child(heading("Response time", "hourly median"))
        .child(line)
        .child(heading("Failed tests", "per hour"))
        .child(failed)
        .child(heading(
            if exit == 0 {
                "When the AI exit changed"
            } else {
                "When it switched"
            },
            "",
        ))
        .child(view().gap_1().children(if switches.is_empty() {
            vec![ui::muted("No change in this time.").into_element()]
        } else {
            switches
                .iter()
                .map(|s| {
                    view()
                        .flex_row()
                        .gap_3()
                        .child(
                            ui::muted(format!("{} {}", day_text(s.at), hour_text(s.at)))
                                .w(px(120.0)),
                        )
                        .child(
                            ui::body(format!("{} → {}", s.from, s.to))
                                .flex_1()
                                .number_of_lines(1),
                        )
                        .child(ui::muted(if s.auto { "automatic" } else { "you" }))
                        .into_element()
                })
                .collect()
        }))
        .into_element()
}

// --- Home -----------------------------------------------------------------------------

/// A small line of how an exit has been doing the last 24 hours (nothing
/// before there are two tests to draw).
pub fn home_spark(exit: usize) -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let snap = store.stats.get();
        untracked(move || {
            let vals: Vec<Option<f64>> = snap
                .0
                .exit_hours(stats::EXITS[exit], now(), 24)
                .iter()
                .map(|(_, h)| stats::exit_median(h).map(f64::from))
                .collect();
            if vals.iter().flatten().count() < 2 {
                return view().into_element();
            }
            let c: fn() -> AdaptiveColor = if exit == 0 {
                ui::chart_ai
            } else {
                ui::chart_neutral
            };
            charts::spark(vals, 84.0, 26.0, c).into_element()
        })
    })
}

// --- Subscriptions: usage -----------------------------------------------------------------

/// What a subscription has used and where its pace leads.
pub fn usage(profile: &str) -> impl IntoElement {
    let store = store();
    let id = profile.to_owned();
    dynamic(move || {
        let snap = store.stats.get();
        let id = id.clone();
        untracked(move || usage_content(&snap.0, &id).into_element())
    })
}

fn plural_days(n: i64) -> String {
    format!("{n} day{}", if n == 1 { "" } else { "s" })
}

fn gb(b: f64) -> f64 {
    b / (1u64 << 30) as f64
}

fn gb_f(v: f64) -> String {
    if v >= 100.0 {
        format!("{:.0} GB", v)
    } else if v >= 10.0 {
        format!("{:.0} GB", v)
    } else {
        format!("{:.1} GB", v)
    }
}

fn usage_content(st: &Stats, id: &str) -> impl IntoElement {
    let t = now();
    let samples = st.usage_of(id);
    let Some(last) = samples.last() else {
        return ui::muted(
            "Kairo records the usage your provider reports each time the subscription updates.",
        )
        .into_element();
    };
    let proj = stats::project(&samples, t);
    let left = last.total.saturating_sub(last.used);
    let pace = proj.as_ref().and_then(|p| p.pace);
    let budget = proj.as_ref().and_then(|p| p.budget);
    let runs_out = proj.as_ref().and_then(|p| p.runs_out);
    let expire = last.expire;
    let days_left = expire.map(|e| ((e - t) as f64 / DAY as f64).ceil().max(0.0) as i64);
    let pace_text = match (pace, runs_out, expire) {
        (Some(_), Some(r), Some(e)) if r < e => Some(format!(
            "At this pace it runs out {}, {} before it expires.",
            day_text(r),
            plural_days((e - r) / DAY)
        )),
        (Some(_), Some(_), Some(_)) => Some("At this pace it lasts until it expires.".to_owned()),
        (Some(_), Some(r), None) => Some(format!("At this pace it runs out {}.", day_text(r))),
        _ => None,
    };
    let _ = (left, budget, days_left);
    // Used so far, per day (the last sample of each day), and the pace after it.
    let z = tz(t);
    let first_day = stats::local_midnight(samples[0].at, z);
    let today = stats::local_midnight(t, z);
    let end_day = match (runs_out, expire) {
        (Some(r), Some(e)) => stats::local_midnight(r.min(e), z),
        (Some(r), None) => stats::local_midnight(r, z),
        (None, Some(e)) => stats::local_midnight(e, z),
        (None, None) => today,
    }
    .max(today)
    .min(today + 45 * DAY);
    let n = (((end_day - first_day) / DAY) + 1).clamp(2, 75) as usize;
    let mut used: Vec<Option<f64>> = vec![None; n];
    let mut carry: Option<f64> = None;
    for (i, slot) in used.iter_mut().enumerate() {
        let d0 = first_day + i as i64 * DAY;
        if d0 > today {
            break;
        }
        if let Some(s) = samples
            .iter()
            .filter(|s| s.at >= d0 && s.at < d0 + DAY)
            .next_back()
        {
            carry = Some(gb(s.used as f64));
        }
        *slot = carry;
    }
    let mut proj_vals: Vec<Option<f64>> = vec![None; n];
    if let Some(p) = pace {
        let today_i = ((today - first_day) / DAY) as usize;
        let base = gb(last.used as f64);
        for (i, v) in proj_vals.iter_mut().enumerate().skip(today_i.min(n - 1)) {
            *v = Some((base + gb(p) * (i - today_i.min(n - 1)) as f64).min(gb(last.total as f64)));
        }
    }
    let tips: Vec<String> = (0..n)
        .map(|i| day_long(first_day + i as i64 * DAY))
        .collect();
    let labels: Vec<charts::XLabel> = (0..n)
        .filter(|i| i % 5 == 0)
        .map(|i| (i, 5, day_text(first_day + i as i64 * DAY)))
        .collect();
    let marks: Vec<Mark> = expire
        .map(|e| ((stats::local_midnight(e, z) - first_day) / DAY) as usize)
        .filter(|i| *i < n)
        .map(|i| {
            vec![Mark {
                at: i,
                text: "Expires".into(),
            }]
        })
        .unwrap_or_default();
    let mut series = vec![Line {
        name: "Used".into(),
        color: ui::chart_ai,
        vals: used.clone(),
        area: true,
        dashed: false,
    }];
    if pace.is_some() {
        series.push(Line {
            name: "At this pace".into(),
            color: ui::chart_ai,
            vals: proj_vals,
            area: false,
            dashed: true,
        });
    }
    let line = charts::lines(
        series,
        tips,
        |v| gb_f(v),
        labels,
        marks,
        Some(gb(last.total as f64)),
        170.0,
        "Move over the line for the day's use".into(),
    );
    view()
        .gap_2()
        .child(heading(
            "Used so far",
            if pace.is_some() {
                "solid: recorded · dashed: if the recent pace continues"
            } else {
                "recorded each time the subscription updates"
            },
        ))
        .child(pace_text.map(|t| {
            ui::body(t)
                .text_color(move || ui::warning())
                .text_size(px(size::small()))
        }))
        .child(line)
        .child(ui::muted(if pace.is_some() {
            "The provider reports total use; Kairo records it every time the subscription updates."
        } else {
            "A projection appears once Kairo has a day of readings."
        }))
        .into_element()
}
