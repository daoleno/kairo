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
use crate::i18n::plural;
use crate::store::{Page, store};
use crate::ui::charts::{self, Col, Line, Mark, Row, Ser, Tone};
use crate::ui::{self, Icon, size, toolbar};

pub fn now() -> i64 {
    kairo_core::unix()
}

fn tz(t: i64) -> i64 {
    stats::local_offset(t)
}

// --- Dates (local time, no library) -------------------------------------------------

/// The keys of the weekdays' short names, Sunday first.
const WEEKDAYS: [&str; 7] = [
    "stats.weekday.sun",
    "stats.weekday.mon",
    "stats.weekday.tue",
    "stats.weekday.wed",
    "stats.weekday.thu",
    "stats.weekday.fri",
    "stats.weekday.sat",
];
/// The keys of the months' short names ("Nov"; Chinese "11月").
const MONTHS: [&str; 12] = [
    "stats.month.jan",
    "stats.month.feb",
    "stats.month.mar",
    "stats.month.apr",
    "stats.month.may",
    "stats.month.jun",
    "stats.month.jul",
    "stats.month.aug",
    "stats.month.sep",
    "stats.month.oct",
    "stats.month.nov",
    "stats.month.dec",
];

/// "Mon" for weekday 0=Sun..6.
fn weekday(w: usize) -> String {
    t!(WEEKDAYS[w])
}

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
    t!("stats.date.day", month = t!(MONTHS[m]), day = d)
}

/// "Tue, Nov 10".
fn day_long(t: i64) -> String {
    let (m, d, w, _) = civil(t + tz(t));
    t!("stats.date.day_long", weekday = weekday(w), month = t!(MONTHS[m]), day = d)
}

/// "Tue, Nov 10, 14:00".
fn day_hour(t: i64) -> String {
    t!("stats.date.day_hour", day = day_long(t), hour = hour_text(t))
}

/// "Today", "7 days", "30 days".
fn range_title(r: Range) -> String {
    match r {
        Range::Today => t!("stats.range.today"),
        Range::Week => t!("stats.range.week"),
        Range::Month => t!("stats.range.month"),
    }
}

/// A kind of traffic as the charts name it (`Slot` is data).
fn slot_title(s: Slot) -> String {
    match s {
        Slot::Ai => t!("stats.slot.ai"),
        Slot::Rest => t!("stats.slot.rest"),
        Slot::Downloads => t!("stats.slot.downloads"),
        Slot::Direct => t!("stats.slot.direct"),
    }
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
    let sub: SharedString = sub.into();
    // On a phone the line under the title, so neither is squeezed beside the other.
    let phone = ui::phone();
    view()
        .when(!phone, |v| v.flex_row().items_baseline().gap_2())
        .child(ui::body(title.to_owned()).font_weight(FontWeight::SEMIBOLD))
        .child((!sub.is_empty()).then(|| ui::muted(sub).when(!phone, |t| t.flex_1())))
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
                        (Range::Today, range_title(Range::Today)),
                        (Range::Week, range_title(Range::Week)),
                        (Range::Month, range_title(Range::Month)),
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
        let started = if st.first_at == 0 {
            t!("stats.collecting.just_now")
        } else {
            let m = ((t - st.first_at) / 60).max(0);
            t!(&plural("stats.collecting.ago", m as u64), count = m)
        };
        return card()
            .child(charts::collecting(
                &t!("stats.collecting.title"),
                &started,
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
                if b.start <= t && t < b.start + HOUR {
                    t!("stats.tip.today_hour_so_far", hour = hour_text(b.start))
                } else {
                    t!("stats.tip.today_hour", hour = hour_text(b.start))
                }
            } else if b.start <= t && t < b.start + DAY {
                t!("stats.tip.day_so_far", day = day_long(b.start))
            } else {
                day_long(b.start)
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
                Some((i, 1, t!("stats.date.weekday_day", weekday = weekday(w), day = d)))
            }
            Range::Month if i % 5 == 0 => Some((i, 5, day_text(b.start))),
            _ => None,
        })
        .collect();
    let ser: Vec<Ser> = Slot::ALL
        .iter()
        .map(|s| Ser {
            name: slot_title(*s),
            color: slot_color(*s),
        })
        .collect();
    let total = kairo_core::bytes(series.total);
    let ai = kairo_core::bytes(ai_total);
    let summary = if series.total > 0 {
        let share = (ai_total as f64 / series.total as f64 * 100.0).round();
        match range {
            Range::Today => t!("stats.traffic.summary.today_share", total = total, ai = ai, share = share),
            Range::Week => t!("stats.traffic.summary.week_share", total = total, ai = ai, share = share),
            Range::Month => t!("stats.traffic.summary.month_share", total = total, ai = ai, share = share),
        }
    } else {
        match range {
            Range::Today => t!("stats.traffic.summary.today", total = total, ai = ai),
            Range::Week => t!("stats.traffic.summary.week", total = total, ai = ai),
            Range::Month => t!("stats.traffic.summary.month", total = total, ai = ai),
        }
    };
    let traffic = card()
        .child(heading(&t!("stats.traffic.title"), summary))
        .child(ui::muted(if hourly {
            t!("stats.traffic.per_hour")
        } else {
            t!("stats.traffic.per_day")
        }))
        .child(charts::legend(
            Slot::ALL
                .iter()
                .map(|s| (slot_title(*s), (slot_color(*s))()))
                .collect(),
        ))
        .child(if series.total == 0 {
            view()
                .py_4()
                .child(ui::muted(t!("stats.traffic.empty")))
                .into_element()
        } else {
            charts::columns(
                ser,
                cols,
                bytes_f,
                labels,
                170.0,
                String::new(),
                unit_of(series.buckets.iter().map(|b| b.total()).max().unwrap_or(0)),
                0.0,
            )
            .into_element()
        })
        .child(clear_footer());
    let apps = st.top_apps(range, t, z);
    let apps_total: u64 = apps.iter().map(|a| a.1).sum();
    let services = st.top_services(range, t, z);
    let ai_sum: u64 = services.iter().map(|a| a.1).sum();
    // Side by side on a desktop, one above the other on a phone.
    let top = view()
        .when(!ui::phone(), |v| v.flex_row())
        .gap_3()
        .child(
            card()
                .when(!ui::phone(), |v| v.flex_1().min_w(px(0.0)))
                .child(heading(&t!("stats.top_apps.title"), range_title(range)))
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
                    &t!("stats.top_apps.empty"),
                )),
        )
        .child(
            card()
                .when(!ui::phone(), |v| v.flex_1().min_w(px(0.0)))
                .child(heading(&t!("stats.ai_services"), t!("stats.top_services.sub")))
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
                                lead: Some(crate::pages::ai::mono(s.id, s.mono).into_element()),
                            })
                        })
                        .collect(),
                    &t!("stats.top_services.empty"),
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
        // On a phone the button goes under the note when they do not fit side by side.
        .when(ui::phone(), |v| v.flex_wrap().gap_y_1())
        .child(view().flex_1())
        .child(
            ui::link_btn(move || -> SharedString {
                if sure.get() {
                    t!("stats.clear.confirm").into()
                } else {
                    t!("stats.clear.button").into()
                }
            })
            .on_press(move || {
                if sure.get_untracked() {
                    sure.set(false);
                    store.svc().clear_stats();
                    store.info(t!("stats.clear.done"));
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
                day_hour(from)
            } else if group >= 24 {
                day_long(from)
            } else {
                t!("stats.date.day_hours", day = day_long(from), from = hour_text(from), to = hour_text(to))
            };
            let (c, p): (u32, u32) = ch
                .iter()
                .filter_map(|(_, h)| h.as_ref())
                .fold((0, 0), |(c, p), h| (c + h.checks, p + h.passed));
            let what = match worst {
                Cell::None => t!("stats.check.none"),
                Cell::Ok => t!("stats.check.ok"),
                Cell::Part => t!("stats.check.part", passed = p, checks = c),
                Cell::Down => t!("stats.check.down", passed = p, checks = c),
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
            let up_text = up.map_or("–".to_owned(), |u| format!("{u:.1}%"));
            let ms_text = ms.map_or("–".to_owned(), |m| format!("{m} ms"));
            let row = nsh_base::button()
                .w_full()
                .accessibility_label(t!("stats.ai.open_week", name = s.name))
                .on_press(move || {
                    store.ai_service.set(id);
                    store.page.set(Page::AiService);
                })
                .px_1()
                .rounded(px(8.0))
                .cursor_pointer()
                .hover(|st| st.bg(move || theme().accent.opacity(0.5)));
            if ui::phone() {
                // A phone has no room for columns: the name, the ribbon, then the numbers in words.
                return row
                    .py_2()
                    .gap_1p5()
                    .items_start()
                    .child(
                        view()
                            .w_full()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(crate::pages::ai::mono(s.id, s.mono))
                            .child(ui::body(s.name).flex_1().min_w(px(0.0)).number_of_lines(1)),
                    )
                    .child(
                        view()
                            .w_full()
                            .flex_row()
                            .child(view().flex_1().min_w(px(0.0)).child(charts::ribbon_bare(tones, 14.0))),
                    )
                    .child(
                        ui::muted(t!("stats.ai.phone_line", up = up_text, ms = ms_text, data = data))
                            .w_full()
                            .text_size(px(size::small())),
                    )
                    .into_element();
            }
            row.flex_row()
                .items_center()
                .gap_3()
                .min_h(px(36.0))
                .child(
                    view()
                        .w(px(120.0))
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(crate::pages::ai::mono(s.id, s.mono))
                        .child(ui::body(s.name).number_of_lines(1)),
                )
                .child(view().flex_1().child(charts::ribbon_bare(tones, 14.0)))
                .child(ui::muted(up_text).tabular_nums().w(px(52.0)))
                .child(ui::muted(ms_text).tabular_nums().w(px(84.0)))
                .child(ui::muted(data).tabular_nums().w(px(70.0)))
                .into_element()
        })
        .collect();
    card()
        .child(heading(
            &t!("stats.ai_services"),
            match range {
                Range::Today => t!("stats.ai.range.today"),
                Range::Week => t!("stats.ai.range.week"),
                Range::Month => t!("stats.ai.range.month"),
            },
        ))
        .child(charts::legend(vec![
            (t!("stats.ai.reachable"), ui::success()),
            (t!("stats.ai.some_failed"), ui::warning()),
            (t!("stats.ai.unreachable"), ui::chart_bad()),
        ]))
        .child((!ui::phone()).then(|| {
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .px_1()
                .child(ui::muted(t!("stats.ai.column.service")).w(px(120.0)))
                .child(ui::muted(t!("stats.ai.reachable")).flex_1())
                .child(ui::muted(t!("stats.ai.column.up")).w(px(52.0)))
                .child(ui::muted(t!("stats.ai.column.api")).w(px(84.0)))
                .child(ui::muted(t!("stats.ai.column.data")).w(px(70.0)))
        }))
        .child(view().children(rows))
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
        .child({
            // A value may be a place ("🇺🇸 Los Angeles 01"): its flag is a picture.
            let (flag, rest) = kairo_core::places::split_flag(&value);
            let rest = rest.trim().to_owned();
            view()
                .flex_row()
                .items_center()
                .gap_1p5()
                // On a phone the tile is narrow: the flag goes above the name, which then has the width.
                .when(ui::phone(), |v| v.flex_col().items_start().gap_1())
                .child(flag.filter(|f| !f.is_empty()).map(|f| ui::flag(f, size::title() - 2.0)))
                .child(
                    text(rest)
                        .text_size(px(size::title()))
                        .font_weight(FontWeight::SEMIBOLD)
                        .number_of_lines(2)
                        .min_w(px(0.0)),
                )
        })
        .child(ui::muted(detail).number_of_lines(2))
}

/// The figures side by side on a desktop, two to a line on a phone.
fn stats_row(items: Vec<AnyElement>) -> AnyElement {
    if !ui::phone() {
        return view().flex_row().gap_3().children(items).into_element();
    }
    let mut lines = Vec::new();
    let mut items = items.into_iter();
    while let Some(first) = items.next() {
        let mut line = view().flex_row().gap_3().child(first);
        line = match items.next() {
            Some(second) => line.child(second),
            None => line.child(view().flex_1()),
        };
        lines.push(line.into_element());
    }
    view().gap_3().children(lines).into_element()
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
                Page::Activity.title(),
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
        Some(true) => (t!("stats.service.reachable_now"), ui::success()),
        Some(false) => (t!("stats.service.unreachable_now"), ui::chart_bad()),
        None => (t!("stats.service.not_checked"), theme().muted_foreground),
    };
    let days: Vec<Vec<(i64, Option<AiHour>)>> = hours.chunks(24).map(|c| c.to_vec()).collect();
    let grid_rows: Vec<(String, Vec<(Tone, String)>)> = days
        .iter()
        .map(|d| {
            let (_, _, w, _) = civil(d[0].0 + tz(d[0].0));
            (weekday(w), cells(d, 1))
        })
        .collect();
    let api_vals: Vec<Option<f64>> = hours
        .iter()
        .map(|(_, h)| stats::api_median(h).map(f64::from))
        .collect();
    let tips: Vec<String> = hours
        .iter()
        .map(|(h, _)| day_hour(*h))
        .collect();
    let labels: Vec<charts::XLabel> = hours
        .iter()
        .enumerate()
        .filter(|(_, (h, _))| (h + tz(*h)).rem_euclid(DAY) / HOUR == 0)
        .map(|(i, (h, _))| {
            let (_, _, w, _) = civil(h + tz(*h));
            (i, 24, weekday(w))
        })
        .collect();
    let line = charts::lines(
        vec![Line {
            name: t!("stats.service.api_response"),
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
        String::new(),
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
                .child(crate::pages::ai::mono(s.id, s.mono))
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
                        .child(ui::muted(t!("stats.service.goes", goes = goes))),
                ),
        )
        .child(stats_row(vec![
            stat(
                    &t!("stats.service.reachable_week"),
                    up.map_or("–".into(), |u| format!("{u:.1}%")),
                    if checks == 0 {
                        t!("stats.service.no_checks")
                    } else {
                        t!("stats.service.checks_failed", failed = checks - passed, checks = checks)
                    },
                )
            .into_element(),
            stat(
                    &t!("stats.service.api_median"),
                    med.map_or("–".into(), |m| format!("{m} ms")),
                    p95.map_or(t!("stats.service.no_answers"), |p| t!("stats.service.p95", ms = p)),
                )
            .into_element(),
            stat(
                    &t!("stats.service.country"),
                    countries
                        .last()
                        .map_or("–".into(), |c| kairo_core::ai_check::country_name(&c.1)),
                    match countries.len() {
                        0 => t!("stats.service.no_answer"),
                        1 => t!("stats.service.no_change"),
                        n => t!("stats.service.changes", count = n - 1),
                    },
                )
            .into_element(),
            stat(
                    &t!("stats.service.held"),
                    if holds == 0 {
                        "–".into()
                    } else {
                        t!("stats.service.held_of", held = held, holds = holds)
                    },
                    if holds == 0 {
                        t!("stats.service.hourly")
                    } else {
                        t!("stats.service.hourly_dropped", count = holds - held)
                    },
                )
            .into_element(),
        ]))
        .child(
            card()
                .child(heading(&t!("stats.service.was_reachable"), ""))
                .child(charts::legend(vec![
                    (t!("stats.service.all_passed"), ui::success()),
                    (t!("stats.service.some_failed"), ui::warning()),
                    (t!("stats.service.mostly_failed"), ui::chart_bad()),
                ]))
                .child(charts::grid(
                    grid_rows,
                    String::new(),
                )),
        )
        .child(
            card()
                .child(heading(&t!("stats.service.api_time"), t!("stats.service.api_time_sub")))
                .child(if api_vals.iter().all(Option::is_none) {
                    ui::muted(t!("stats.service.no_api")).into_element()
                } else {
                    line.into_element()
                }),
        )
        .child(
            card()
                .child(heading(&t!("stats.service.country"), ""))
                .child(view().gap_1().children(if countries.is_empty() {
                    vec![ui::muted(t!("stats.service.nothing")).into_element()]
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
                                .child(ui::muted(if i == 0 { t!("stats.service.now") } else { String::new() }))
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
                .when(!ui::phone(), |v| v.flex_row().items_center())
                .when(ui::phone(), |v| v.gap_2())
                .child(view().when(!ui::phone(), |t| t.flex_1()))
                .child(ui::segmented(
                    vec![(false, t!("stats.range.day")), (true, range_title(Range::Week))],
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
            &t!("stats.collecting.title"),
            &t!("stats.quality.no_readings"),
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
                day_hour(*h)
            } else {
                t!("stats.tip.today_hour", hour = hour_text(*h))
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
                    (i, 24, weekday(w))
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
                text: if s.auto {
                    t!("stats.quality.switch_auto", from = s.from, to = s.to)
                } else {
                    t!("stats.quality.switch_you", from = s.from, to = s.to)
                },
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
            name: t!("stats.quality.response"),
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
        String::new(),
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
            name: t!("stats.quality.failed"),
            color: ui::chart_bad,
        }],
        fail_cols,
        count_f,
        vec![],
        80.0,
        t!("stats.quality.failed_per_hour"),
        1.0,
        2.0,
    );
    let node_now = data
        .iter()
        .rev()
        .find_map(|(_, h)| h.as_ref().map(|h| h.node.clone()))
        .unwrap_or_default();
    view()
        .gap_3()
        .child(stats_row(vec![
            stat(
                &t!("stats.quality.typical"),
                stats::median(all.clone()).map_or("–".into(), |m| format!("{m} ms")),
                t!("stats.quality.median"),
            )
            .into_element(),
            stat(&t!("stats.quality.failed"), fails.to_string(), t!("stats.quality.of_tests", tests = tests)).into_element(),
            stat(
                &t!("stats.quality.changes"),
                switches.len().to_string(),
                if switches.is_empty() {
                    t!("stats.quality.none")
                } else {
                    t!("stats.quality.by", you = by_you, auto = switches.len() - by_you)
                },
            )
            .into_element(),
            stat(&t!("stats.quality.right_now"), node_now, String::new()).into_element(),
        ]))
        .child(heading(&t!("stats.quality.response_time"), t!("stats.quality.hourly_median")))
        .child(line)
        .child(heading(&t!("stats.quality.failed"), t!("stats.quality.per_hour")))
        .child(failed)
        .child(heading(
            &if exit == 0 {
                t!("stats.quality.ai_changed")
            } else {
                t!("stats.quality.switched")
            },
            "",
        ))
        .child(view().gap_1().children(if switches.is_empty() {
            vec![ui::muted(t!("stats.quality.no_change")).into_element()]
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
                        .child(ui::muted(if s.auto {
                            t!("stats.quality.automatic")
                        } else {
                            t!("stats.quality.you")
                        }))
                        .into_element()
                })
                .collect()
        }))
        .into_element()
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
        return ui::muted(t!("stats.usage.none")).into_element();
    };
    let proj = stats::project(&samples, t);
    let left = last.total.saturating_sub(last.used);
    let pace = proj.as_ref().and_then(|p| p.pace);
    let budget = proj.as_ref().and_then(|p| p.budget);
    let runs_out = proj.as_ref().and_then(|p| p.runs_out);
    let expire = last.expire;
    let days_left = expire.map(|e| ((e - t) as f64 / DAY as f64).ceil().max(0.0) as i64);
    let pace_text = match (pace, runs_out, expire) {
        (Some(_), Some(r), Some(e)) if r < e => {
            let days = (e - r) / DAY;
            Some(t!(
                &plural("stats.usage.runs_out_before", days.max(0) as u64),
                date = day_text(r),
                count = days
            ))
        }
        (Some(_), Some(_), Some(_)) => Some(t!("stats.usage.lasts")),
        (Some(_), Some(r), None) => Some(t!("stats.usage.runs_out", date = day_text(r))),
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
                text: t!("stats.usage.expires"),
            }]
        })
        .unwrap_or_default();
    let mut series = vec![Line {
        name: t!("stats.usage.used"),
        color: ui::chart_ai,
        vals: used.clone(),
        area: true,
        dashed: false,
    }];
    if pace.is_some() {
        series.push(Line {
            name: t!("stats.usage.at_pace"),
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
        String::new(),
    );
    view()
        .gap_2()
        .child(heading(
            &t!("stats.usage.title"),
            if pace.is_some() {
                t!("stats.usage.legend")
            } else {
                String::new()
            },
        ))
        .child(pace_text.map(|t| {
            ui::body(t)
                .text_color(move || ui::warning())
                .text_size(px(size::small()))
        }))
        .child(line)
        .into_element()
}

#[cfg(test)]
mod tests {
    use kairo_core::i18n::text_in;

    #[test]
    fn a_bare_percent_stays_beside_an_argument() {
        let ms = 420;
        assert_eq!(text_in("en", "stats.service.p95", &[("ms", &ms)]), "95% under 420 ms");
        assert_eq!(text_in("zh", "stats.service.p95", &[("ms", &ms)]), "95% 低于 420 ms");
        let share = 42.0f64;
        let args: [(&str, &dyn std::fmt::Display); 3] = [("total", &"1.2 GB"), ("ai", &"300 MB"), ("share", &share)];
        assert_eq!(text_in("en", "stats.traffic.summary.today_share", &args), "1.2 GB today · AI 300 MB (42%)");
        assert_eq!(text_in("zh", "stats.traffic.summary.today_share", &args), "今天 1.2 GB · AI 300 MB（42%）");
    }

    #[test]
    fn chinese_dates_have_a_numeric_month() {
        let day = 10;
        let month = text_in("zh", "stats.month.nov", &[]);
        assert_eq!(text_in("zh", "stats.date.day", &[("month", &month), ("day", &day)]), "11月10日");
        let month = text_in("en", "stats.month.nov", &[]);
        assert_eq!(text_in("en", "stats.date.day", &[("month", &month), ("day", &day)]), "Nov 10");
    }
}
