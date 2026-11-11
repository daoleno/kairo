//! Subscriptions: the links (and files) Kairo gets its locations from. Add one,
//! update them, switch between them, and see how much of the plan is used.
//!
//! The text on this page (usage lines, "updated 2 h ago", the allowance per
//! day, plain-words provider errors) is made by pure functions that take the
//! time as an argument, so they are tested without a window.

use std::rc::Rc;

use kairo_core::profile::{Profile, Source, Usage};
use nsh_base::{ButtonElement as Button, alert_dialog, button, theme};
use nsh_core::file_dialog::{FileFilter, OpenDialog};
use nsh_core::prelude::*;

use crate::chrome::Glyph;
use crate::store::{Page, store};
use crate::ui::menu::{self, Choice};
use crate::ui::toolbar;
use crate::ui::{self, Icon, Variant, icon, size};

// --- The words -----------------------------------------------------------------------

const DAY: i64 = 86_400;

/// The host of a link, and nothing else: a link carries the account's token.
pub fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "Subscription".to_owned())
}

/// Where a subscription comes from, for its row: the host, or "Imported file".
pub fn source_label(source: &Source) -> String {
    match source {
        Source::Remote { url, .. } => host_of(url),
        Source::Local { .. } => "Imported file".to_owned(),
    }
}

/// "Updated 2 h ago".
pub fn updated_ago(at: Option<i64>, now: i64) -> String {
    let Some(at) = at else {
        return "Never updated".into();
    };
    match now - at {
        i64::MIN..=59 => "Updated just now".into(),
        s @ 60..=3599 => format!("Updated {} min ago", s / 60),
        s @ 3600..=86399 => format!("Updated {} h ago", s / 3600),
        s => format!("Updated {} d ago", s / DAY),
    }
}

/// "Every 12 hours", "Every day", "Only by hand".
pub fn interval_label(hours: u32) -> String {
    match hours {
        0 => "Only by hand".into(),
        1 => "Every hour".into(),
        24 => "Every day".into(),
        168 => "Every week".into(),
        h if h % 24 == 0 => format!("Every {} days", h / 24),
        h => format!("Every {h} hours"),
    }
}

/// "Updated 2 h ago · every 12 h"; a file is not updated by itself.
pub fn schedule_line(source: &Source, updated_at: Option<i64>, now: i64) -> String {
    match source {
        Source::Remote { interval_hours, .. } => {
            let ago = updated_ago(updated_at, now);
            match interval_hours {
                0 => format!("{ago} · by hand"),
                h => format!("{ago} · every {h} h"),
            }
        }
        Source::Local { .. } => "Not updated automatically".into(),
    }
}

/// `Dec 2`, in the local time of `offset` seconds from UTC.
pub fn month_day(t: i64, offset: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // Civil from days (Howard Hinnant).
    let z = (t + offset).div_euclid(DAY) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{} {day}", MONTHS[(month - 1) as usize])
}

/// A size with at most one decimal, none when whole: "37 GB", "2.9 GB".
pub fn size_text(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        return format!("{n} B");
    }
    let number = if v >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    };
    format!("{number} {}", UNITS[i])
}

/// "37 of 100 GB": the unit once when both share it.
pub fn used_of(used: u64, total: u64) -> String {
    let (a, b) = (size_text(used), size_text(total));
    match (a.split_once(' '), b.split_once(' ')) {
        (Some((number, unit)), Some((_, unit_b))) if unit == unit_b => format!("{number} of {b}"),
        _ => format!("{a} of {b}"),
    }
}

/// Whole days until `expire`, a started day counting as one; `None` when there
/// is no expiry or it has passed.
pub fn days_left(expire: Option<i64>, now: i64) -> Option<i64> {
    let left = expire? - now;
    (left > 0).then(|| (left + DAY - 1) / DAY)
}

/// What is left of the plan; `None` without a limit.
pub fn left_bytes(usage: &Usage) -> Option<u64> {
    (usage.total > 0).then(|| usage.total.saturating_sub(usage.used()))
}

/// How much can be used each day for the rest to last until the plan expires.
pub fn allowance_per_day(left: u64, days: i64) -> Option<u64> {
    (days > 0).then(|| left / days as u64)
}

/// "37 of 100 GB · expires Dec 2".
pub fn usage_line(usage: Option<&Usage>, now: i64, offset: i64) -> String {
    let Some(u) = usage else {
        return "No usage data".into();
    };
    let amount = if u.total > 0 {
        used_of(u.used(), u.total)
    } else {
        format!("{} used", size_text(u.used()))
    };
    let expiry = match u.expire {
        None => "no expiry".to_owned(),
        Some(t) if t <= now => format!("expired {}", month_day(t, offset)),
        Some(t) => format!("expires {}", month_day(t, offset)),
    };
    format!("{amount} · {expiry}")
}

/// One of the four tiles of the usage section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    pub title: String,
    pub value: String,
    pub detail: String,
}

fn tile(title: impl Into<String>, value: impl Into<String>, detail: impl Into<String>) -> Tile {
    Tile {
        title: title.into(),
        value: value.into(),
        detail: detail.into(),
    }
}

/// The tiles of a subscription's usage, from what the provider's header says:
/// Used, Left, Recent pace (measured later: "—" until then) and what to use per
/// day to last until the plan expires. No projection is made without a measured
/// pace.
pub fn usage_tiles(u: &Usage, now: i64, offset: i64) -> [Tile; 4] {
    let days = days_left(u.expire, now);
    let left = left_bytes(u);
    let used = tile(
        "Used",
        size_text(u.used()),
        if u.total > 0 {
            format!("of {}", size_text(u.total))
        } else {
            "no limit".to_owned()
        },
    );
    let until = match (u.expire, days) {
        (None, _) => "no expiry".to_owned(),
        (Some(t), None) => format!("expired {}", month_day(t, offset)),
        (Some(t), Some(1)) => format!("1 day to {}", month_day(t, offset)),
        (Some(t), Some(d)) => format!("{d} days to {}", month_day(t, offset)),
    };
    let left_tile = tile(
        "Left",
        left.map_or_else(|| "No limit".to_owned(), size_text),
        until,
    );
    let pace = tile("Recent pace", "—", "not measured yet");
    let allowance = match (left, days, u.expire) {
        (Some(left), Some(d), Some(t)) => {
            let per_day = allowance_per_day(left, d).unwrap_or(0);
            tile(
                format!("To last until {}", month_day(t, offset)),
                format!("≤ {}/day", size_text(per_day)),
                format!(
                    "{} over {d} day{}",
                    size_text(left),
                    if d == 1 { "" } else { "s" }
                ),
            )
        }
        _ => tile("To last until expiry", "—", ""),
    };
    [used, left_tile, pace, allowance]
}

/// Replaces every link in `text` (and `url`, the subscription's own) by its
/// host: an error message must never carry a token.
pub fn scrub(text: &str, url: Option<&str>) -> String {
    let mut text = text.to_owned();
    if let Some(url) = url.filter(|u| !u.is_empty()) {
        text = text.replace(url, &host_of(url));
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = ["http://", "https://"]
        .iter()
        .filter_map(|p| rest.find(p))
        .min()
    {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '”' | ',' | '>'))
            .unwrap_or(tail.len());
        out.push_str(&host_of(&tail[..end]));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn reason(code: u16) -> &'static str {
    match code {
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Server Error",
    }
}

/// An update's failure in the user's words: the headline and a hint, with no
/// link in either. (The engine says "The server answered HTTP 503"; the rest is
/// `home::explain`.)
pub fn plain_error(err: &str, url: Option<&str>) -> (String, Option<String>) {
    let e = scrub(err, url);
    let code = e
        .strip_prefix("The server answered HTTP ")
        .and_then(|n| n.trim().parse::<u16>().ok());
    if let Some(code) = code.filter(|c| *c >= 500 || *c == 429) {
        return (
            format!("Your provider says: “{code} {}”", reason(code)),
            Some(
                "Its server is having trouble. This usually passes: try again in a little while."
                    .into(),
            ),
        );
    }
    crate::pages::home::explain(&e)
}

/// The rest of the banner after the subscription's name:
/// "couldn't update · Your provider says: “503 Service Unavailable”. Using the saved copy."
pub fn banner_text(headline: &str) -> String {
    format!(
        "couldn't update · {}. Using the saved copy.",
        headline.trim().trim_end_matches('.')
    )
}

/// How often a subscription updates, the choices of its menu: the usual
/// ones, and the current when it is another.
fn interval_choices(current: u32) -> Vec<u32> {
    let mut v = vec![6, 12, 24, 72, 168, 0];
    if !v.contains(&current) {
        v.insert(0, current);
    }
    v
}

fn now() -> i64 {
    kairo_core::profile::now()
}

fn offset() -> i64 {
    kairo_core::service::local_offset_seconds()
}

// --- The page ------------------------------------------------------------------------

/// What the dialog over the page is for.
#[derive(Debug, Clone, PartialEq)]
enum Dialog {
    Add,
    Rename { id: String, name: String },
    Link { id: String, host: String },
}

/// The "as the provider says" choice of the update interval when adding.
const PROVIDER_CHOICE: u32 = u32::MAX;

pub fn page() -> impl IntoElement {
    let store = store();
    let dialog = signal(None::<Dialog>);
    // KAIRO_EXPAND=1 opens the active subscription's usage (screenshots).
    let expanded = signal(
        std::env::var_os("KAIRO_EXPAND")
            .filter(|v| !v.is_empty())
            .and_then(|_| store.state.with_untracked(|s| s.profiles.active.clone())),
    );
    let profiles = computed(move || store.state.with(|s| s.profiles.profiles.clone()));
    let failing = computed(move || {
        store.state.with(|s| {
            s.profiles
                .profiles
                .iter()
                .filter(|p| p.failure_shown())
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let any_remote =
        move || profiles.with(|p| p.iter().any(|p| matches!(p.source, Source::Remote { .. })));
    let updating_all = move || store.state.with(|s| !s.updating.is_empty());
    view()
        .flex_1()
        .child(toolbar::floating(
            Page::Profiles,
            move || {
                let n = store.state.with(|s| s.profiles.profiles.len());
                match n {
                    0 => String::new(),
                    1 => "1 subscription".to_owned(),
                    n => format!("{n} subscriptions"),
                }
            },
            vec![
                toolbar::button(
                    "profiles.update-all",
                    "Update All",
                    Glyph::Refresh,
                    Icon::Refresh,
                    move || store.run(store.svc().update_all_profiles()),
                )
                .enabled(move || any_remote() && !updating_all()),
                toolbar::button(
                    "profiles.add",
                    "Add",
                    Glyph::Add,
                    Icon::Plus,
                    move || dialog.set(Some(Dialog::Add)),
                )
                .enabled(move || dialog.with(|d| d.is_none()))
                .prominent(),
            ],
        ))
        .child(
            scroll().flex_1().child(
                view()
                    .px(px(if ui::phone() { 16.0 } else { 24.0 }))
                    .pt(toolbar::scroll_top(if ui::phone() { 12.0 } else { 20.0 }))
                    .pb_6()
                    .gap_3()
                    .w_full()
                    .max_w(px(900.0))
                    .self_center()
                    .child(each(
                        move || failing.get(),
                        |p: &Profile| p.id.clone(),
                        move |p, _| banner(p),
                    ))
                    .child(
                        when(
                            move || profiles.with(|p| !p.is_empty()),
                            move || {
                                if ui::phone() {
                                    return view()
                                        .gap_3()
                                        .child(each(
                                            move || profiles.get(),
                                            |p: &Profile| p.id.clone(),
                                            move |p, _| phone_row(p, expanded, dialog),
                                        ))
                                        .into_element();
                                }
                                ui::card().child(each(
                                    move || profiles.get(),
                                    |p: &Profile| p.id.clone(),
                                    move |p, i| {
                                        let first = i.get_untracked() == 0;
                                        view()
                                            .child((!first).then(|| ui::divider().ml(px(44.0))))
                                            .child(row(p, expanded, dialog))
                                    },
                                ))
                                .into_element()
                            },
                        )
                        .otherwise(move || {
                            ui::empty_state(
                                Icon::Profiles,
                                "No subscriptions yet",
                                Some(
                                    ui::btn_icon(Icon::Plus, "Add", Variant::Primary)
                                        .on_press(move || dialog.set(Some(Dialog::Add)))
                                        .into_element(),
                                ),
                            )
                        }),
                    ),
            ),
        )
        .child(dynamic(move || match dialog.get() {
            None => view().into_element(),
            Some(d) => untracked(move || sheet(d, dialog)).into_element(),
        }))
}

// --- Provider errors -----------------------------------------------------------------

/// "Backup couldn't update · Your provider says: “503 Service Unavailable”.
/// Using the saved copy." with Retry.
fn banner(p: ReadSignal<Profile>) -> impl IntoElement {
    let store = store();
    let id = p.get_untracked().id.clone();
    let updating = {
        let id = id.clone();
        let m = computed(move || store.state.with(|s| s.updating.contains(&id)));
        move || m.get()
    };
    let said =
        move || p.with(|p| plain_error(p.last_error.as_deref().unwrap_or_default(), p.url()));
    let close_id = id.clone();
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_3()
        .rounded(move || theme().radius)
        .bg(move || ui::warning().opacity(0.14))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap_0p5()
                .child(
                    view()
                        .flex_row()
                        .gap_1()
                        .child(
                            text(move || -> SharedString { p.with(|p| p.name.clone()).into() })
                                .text_size(px(size::body()))
                                .font_weight(FontWeight::SEMIBOLD)
                                .number_of_lines(1),
                        )
                        .child(
                            text(move || -> SharedString { banner_text(&said().0).into() })
                                .text_size(px(size::body()))
                                .flex_1(),
                        ),
                )
                .child(when(
                    move || said().1.is_some(),
                    move || ui::muted(move || said().1.unwrap_or_default()),
                )),
        )
        .child(
            ui::btn(
                move || -> SharedString {
                    if updating() {
                        "Retrying…".into()
                    } else {
                        "Retry".into()
                    }
                },
                Variant::Secondary,
            )
            .disabled(updating)
            .on_press(move || {
                // The banner says why; no second message.
                let f = store.svc().update_profile(id.clone());
                spawn(async move {
                    let _ = f.await;
                });
            }),
        )
        // Closing hides it until the next failed update says it again.
        .child(
            ui::icon_btn(Icon::Close, "Close")
                .on_press(move || store.svc().dismiss_failure(&close_id)),
        )
}

// --- The list ------------------------------------------------------------------------

fn row(
    p: ReadSignal<Profile>,
    expanded: Signal<Option<String>>,
    dialog: Signal<Option<Dialog>>,
) -> impl IntoElement {
    let store = store();
    let id = p.get_untracked().id.clone();
    let active = {
        let id = id.clone();
        let m = computed(move || {
            store
                .state
                .with(|s| s.profiles.active.as_deref() == Some(id.as_str()))
        });
        move || m.get()
    };
    let updating = {
        let id = id.clone();
        let m = computed(move || store.state.with(|s| s.updating.contains(&id)));
        move || m.get()
    };
    let open = {
        let id = id.clone();
        let m = computed(move || expanded.with(|e| e.as_deref() == Some(id.as_str())));
        move || m.get()
    };
    let remote = move || p.with(|p| p.is_remote());
    let has_usage = move || p.with(|p| p.usage.is_some());
    let limited = move || p.with(|p| p.usage.is_some_and(|u| u.total > 0));
    let toggle = {
        let id = id.clone();
        move || {
            if open() {
                expanded.set(None)
            } else {
                expanded.set(Some(id.clone()))
            }
        }
    };
    let id_use = id.clone();
    let id_update = id.clone();
    let id_menu = id.clone();

    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .px_4()
                .py_3()
                .min_h(px(66.0))
                .child(
                    view().w(px(16.0)).items_center().child(
                        icon(Icon::Check, 14.0)
                            .text_color(move || theme().primary)
                            .style_when(move || !active(), |s| s.invisible()),
                    ),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap_0p5()
                        .child(
                            view()
                                .flex_row()
                                .items_baseline()
                                .gap_1p5()
                                .child(
                                    text(move || -> SharedString {
                                        p.with(|p| p.name.clone()).into()
                                    })
                                    .text_size(px(size::body()))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .number_of_lines(1),
                                )
                                .child(
                                    ui::muted(move || -> SharedString {
                                        p.with(|p| source_label(&p.source)).into()
                                    })
                                    .number_of_lines(1),
                                ),
                        )
                        .child(ui::muted(move || -> SharedString {
                            p.with(|p| usage_line(p.usage.as_ref(), now(), offset()))
                                .into()
                        }))
                        .child(ui::muted(move || -> SharedString {
                            if updating() {
                                return "Updating…".into();
                            }
                            p.with(|p| schedule_line(&p.source, p.updated_at, now()))
                                .into()
                        })),
                )
                .child(when(limited, move || {
                    view().w(px(120.0)).child(ui::meter(move || {
                        p.with(|p| p.usage.and_then(|u| u.fraction()).unwrap_or(0.0) as f32)
                    }))
                }))
                .child(when(active, || in_use_tag()).otherwise(move || {
                    let id = id_use.clone();
                    ui::btn("Use", Variant::Secondary)
                        .h(px(26.0))
                        .on_press(move || store.run(store.svc().activate(id.clone())))
                }))
                // The groups are the active subscription's own: one press from where it is.
                .child(when(active, || groups_button().h(px(26.0))))
                .child(when(remote, move || {
                    let id = id_update.clone();
                    ui::btn("Update", Variant::Secondary)
                        .h(px(26.0))
                        .disabled(updating)
                        .on_press(move || store.run(store.svc().update_profile(id.clone())))
                }))
                .child(when(has_usage, move || {
                    let toggle = toggle.clone();
                    button()
                        .accessibility_label("Subscription usage")
                        .on_press(move || toggle())
                        .items_center()
                        .justify_center()
                        .size(px(26.0))
                        .rounded(ui::radius_control())
                        .cursor_pointer()
                        .text_color(move || theme().muted_foreground)
                        .hover(|s| s.bg(move || theme().accent))
                        .child(
                            icon(Icon::ChevronDown, 14.0)
                                .rotate(move || if open() { deg(180.0) } else { deg(0.0) }),
                        )
                }))
                .child(
                    ui::btn("···", Variant::Secondary)
                        .accessibility_label("More")
                        .h(px(26.0))
                        .on_press(move || menu(id_menu.clone(), p, dialog)),
                ),
        )
        .child(when(
            move || open() && has_usage(),
            move || usage_section(p),
        ))
}

/// A subscription on a phone: a card, not a table row. The name and what it
/// is on top, its numbers under it (the meter as wide as the card), then the
/// actions in a row that wraps, and the usage opening below.
fn phone_row(
    p: ReadSignal<Profile>,
    expanded: Signal<Option<String>>,
    dialog: Signal<Option<Dialog>>,
) -> impl IntoElement {
    let store = store();
    let id = p.get_untracked().id.clone();
    let active = {
        let id = id.clone();
        let m = computed(move || {
            store
                .state
                .with(|s| s.profiles.active.as_deref() == Some(id.as_str()))
        });
        move || m.get()
    };
    let updating = {
        let id = id.clone();
        let m = computed(move || store.state.with(|s| s.updating.contains(&id)));
        move || m.get()
    };
    let open = {
        let id = id.clone();
        let m = computed(move || expanded.with(|e| e.as_deref() == Some(id.as_str())));
        move || m.get()
    };
    let remote = move || p.with(|p| p.is_remote());
    let has_usage = move || p.with(|p| p.usage.is_some());
    let limited = move || p.with(|p| p.usage.is_some_and(|u| u.total > 0));
    let id_use = id.clone();
    let id_update = id.clone();
    let id_menu = id.clone();
    let id_toggle = id.clone();
    ui::card_soft()
        .child(
            view()
                .px_4()
                .py_3()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    text(move || -> SharedString {
                                        p.with(|p| p.name.clone()).into()
                                    })
                                    .text_size(px(size::large()))
                                    .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(
                                    ui::muted(move || -> SharedString {
                                        p.with(|p| source_label(&p.source)).into()
                                    })
                                    .number_of_lines(1),
                                ),
                        )
                        .child(when(active, || in_use_tag()))
                        .child(
                            ui::icon_btn(Icon::More, "More")
                                .mr(px(-8.0))
                                .on_press(move || menu(id_menu.clone(), p, dialog)),
                        ),
                )
                .child(ui::muted(move || -> SharedString {
                    p.with(|p| usage_line(p.usage.as_ref(), now(), offset()))
                        .into()
                }))
                .child(when(limited, move || {
                    ui::meter(move || {
                        p.with(|p| p.usage.and_then(|u| u.fraction()).unwrap_or(0.0) as f32)
                    })
                }))
                .child(ui::muted(move || -> SharedString {
                    if updating() {
                        return "Updating…".into();
                    }
                    p.with(|p| schedule_line(&p.source, p.updated_at, now()))
                        .into()
                }))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_2()
                        .child(when(move || !active(), move || {
                            let id = id_use.clone();
                            ui::btn("Use", Variant::Primary)
                                .on_press(move || store.run(store.svc().activate(id.clone())))
                        }))
                        .child(when(active, || groups_button()))
                        .child(when(remote, move || {
                            let id = id_update.clone();
                            ui::btn_icon(Icon::Refresh, "Update", Variant::Secondary)
                                .disabled(updating)
                                .on_press(move || {
                                    store.run(store.svc().update_profile(id.clone()))
                                })
                        }))
                        .child(when(has_usage, move || {
                            let id = id_toggle.clone();
                            ui::btn(
                                move || -> SharedString {
                                    if open() { "Hide usage" } else { "Usage" }.into()
                                },
                                Variant::Secondary,
                            )
                            .on_press(move || {
                                if open() {
                                    expanded.set(None)
                                } else {
                                    expanded.set(Some(id.clone()))
                                }
                            })
                        })),
                ),
        )
        .child(when(
            move || open() && has_usage(),
            move || usage_section(p),
        ))
}

/// Opens the subscription's groups (the Groups page).
fn groups_button() -> Button {
    let store = store();
    ui::btn_icon(Icon::Layers, "Groups", Variant::Secondary)
        .on_press(move || store.page.set(Page::Proxies))
}

fn in_use_tag() -> View {
    view()
        .px_2()
        .h(px(22.0))
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(move || theme().primary.opacity(0.12))
        .child(
            text("In use")
                .text_size(px(size::small()))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().primary),
        )
}

/// ⋯: Rename, Change Link, Update Interval, Remove.
fn menu(id: String, p: ReadSignal<Profile>, dialog: Signal<Option<Dialog>>) {
    let store = store();
    let (name, source) = p.with_untracked(|p| (p.name.clone(), p.source.clone()));
    let mut items = Vec::new();
    items.push(Choice::new("Rename…").on_select({
        let (id, name) = (id.clone(), name.clone());
        move || {
            dialog.set(Some(Dialog::Rename {
                id: id.clone(),
                name: name.clone(),
            }))
        }
    }));
    if let Source::Remote {
        url,
        interval_hours,
    } = &source
    {
        let host = host_of(url);
        items.push(Choice::new("Change Link…").on_select({
            let id = id.clone();
            move || {
                dialog.set(Some(Dialog::Link {
                    id: id.clone(),
                    host: host.clone(),
                }))
            }
        }));
        items.push(Choice::submenu(
            "Update Interval",
            interval_choices(*interval_hours).into_iter().map(|h| {
                let id = id.clone();
                Choice::new(interval_label(h))
                    .checked(h == *interval_hours)
                    .on_select(move || {
                        store
                            .svc()
                            .rename_profile(id.clone(), String::new(), Some(h), None)
                    })
            }),
        ));
    }
    items.push(Choice::separator());
    items.push(Choice::new("Remove…").on_select({
        let id = id.clone();
        move || remove(id.clone(), name.clone())
    }));
    menu::open(items);
}

fn remove(id: String, name: String) {
    let store = store();
    let active = store
        .state
        .with_untracked(|s| s.profiles.active.as_deref() == Some(id.as_str()));
    let description = if active {
        "Kairo deletes its saved copy. It is the one in use: Kairo switches to another subscription, or disconnects when there is none."
    } else {
        "Kairo deletes its saved copy. You can add it again later."
    };
    spawn(async move {
        let answer = alert_dialog()
            .title(format!("Remove “{name}”?"))
            .description(description)
            .destructive("remove", "Remove")
            .cancel("cancel", "Cancel")
            .show()
            .await;
        if answer.is_ok_and(|a| a.action.as_deref() == Some("remove")) {
            store.run(store.svc().delete_profile(id));
        }
    });
}

// --- Usage ---------------------------------------------------------------------------

/// "Subscription usage": what the provider's header says. The history and the
/// pace come later, from what Kairo records.
fn usage_section(p: ReadSignal<Profile>) -> impl IntoElement {
    let store = store();
    let tiles = computed(move || {
        let (id, usage) = p.with(|p| (p.id.clone(), p.usage));
        let mut tiles = usage.map(|u| usage_tiles(&u, now(), offset()))?;
        // The pace comes from what Kairo has recorded (a day of readings).
        let pace = store
            .stats
            .with(|st| kairo_core::stats::project(&st.0.usage_of(&id), now()).and_then(|p| p.pace));
        if let Some(pace) = pace {
            tiles[2] = tile(
                "Recent pace",
                format!("{}/day", size_text(pace as u64)),
                "over the last days",
            );
        }
        Some(tiles)
    });
    let one = move |i: usize| {
        let get = move |f: fn(&Tile) -> &String| -> SharedString {
            tiles
                .with(|t| t.as_ref().map(|t| f(&t[i]).clone()).unwrap_or_default())
                .into()
        };
        view()
            .flex_1()
            .min_w(px(0.0))
            .gap_0p5()
            .p_3()
            .rounded(px(10.0))
            .bg(move || ui::subtle())
            .child(ui::muted(move || get(|t| &t.title)).number_of_lines(1))
            .child(
                ui::figure(move || get(|t| &t.value), 20.0)
                    .font_weight(FontWeight::SEMIBOLD)
                    .number_of_lines(1),
            )
            .child(ui::muted(move || get(|t| &t.detail)).number_of_lines(1))
    };
    view()
        .px_4()
        .pb_4()
        .gap_2()
        .child(ui::section_label("Subscription usage"))
        .child(if ui::phone() {
            // Two by two: four across is too narrow for a figure and its words.
            view()
                .gap_2()
                .child(view().flex_row().gap_2().child(one(0)).child(one(1)))
                .child(view().flex_row().gap_2().child(one(2)).child(one(3)))
        } else {
            view()
                .flex_row()
                .gap_3()
                .child(one(0))
                .child(one(1))
                .child(one(2))
                .child(one(3))
        })
        .child(dynamic(move || {
            let id = p.with(|p| p.id.clone());
            untracked(move || crate::pages::stats::usage(&id).into_element())
        }))
}

// --- The dialog ----------------------------------------------------------------------

/// The dialog over the page: a dimmed backdrop (a press on it, or Escape,
/// closes) and the card in the middle.
fn sheet(d: Dialog, dialog: Signal<Option<Dialog>>) -> impl IntoElement {
    let close = move || dialog.set(None);
    let card = match d {
        Dialog::Add => add_card(dialog).into_element(),
        Dialog::Rename { id, name } => rename_card(id, name, dialog).into_element(),
        Dialog::Link { id, host } => link_card(id, host, dialog).into_element(),
    };
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
                .child(card),
        )
}

fn card_frame(title: &'static str, detail: Option<String>) -> View {
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
        .child(detail.map(ui::muted))
}

fn form_row(label: &'static str, control: impl IntoElement) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .child(ui::muted(label).w(px(52.0)))
        .child(view().flex_1().min_w(px(0.0)).items_end().child(control))
}

/// A failure, in the provider's words.
fn problem_box(problem: Signal<Option<String>>, url: Signal<String>) -> impl IntoElement {
    let said = move || {
        plain_error(
            &problem.get().unwrap_or_default(),
            Some(url.get_untracked().trim()),
        )
    };
    when(
        move || problem.with(|p| p.is_some()),
        move || {
            view()
                .gap_1()
                .p_3()
                .rounded(px(10.0))
                .bg(move || theme().destructive.opacity(0.08))
                .child(
                    text(move || said().0)
                        .text_size(px(size::body()))
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(when(
                    move || said().1.is_some(),
                    move || ui::muted(move || said().1.unwrap_or_default()),
                ))
        },
    )
}

fn add_card(dialog: Signal<Option<Dialog>>) -> impl IntoElement {
    let store = store();
    let url = signal(String::new());
    let name = signal(String::new());
    let hours = signal(PROVIDER_CHOICE);
    let busy = signal(false);
    let problem = signal(None::<String>);
    let valid = move || {
        url.with(|u| {
            let u = u.trim();
            u.starts_with("http://") || u.starts_with("https://")
        })
    };
    let submit = Rc::new(move || {
        if !valid() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        problem.set(None);
        let n = name.get_untracked().trim().to_owned();
        let f = store.svc().add_remote(
            url.get_untracked().trim().to_owned(),
            (!n.is_empty()).then_some(n),
        );
        spawn(async move {
            let r = f.await;
            busy.set(false);
            match r {
                Ok(id) => {
                    let h = hours.get_untracked();
                    if h != PROVIDER_CHOICE {
                        store.svc().rename_profile(id, String::new(), Some(h), None);
                    }
                    store.info("Subscription added");
                    dialog.set(None);
                }
                Err(e) => problem.set(Some(e)),
            }
        });
    });
    let import = move || {
        let svc = store.svc();
        spawn(async move {
            let picked = pick_paths(
                OpenDialog::files()
                    .filter(FileFilter::new(
                        "Clash / mihomo config",
                        ["yaml", "yml", "txt"],
                    ))
                    .title("Import a Config"),
            )
            .await;
            match picked {
                Ok(Some(paths)) => {
                    if let Some(path) = paths.into_iter().next() {
                        store.run_then(svc.add_file(path), move |r| {
                            if r.is_ok() {
                                store.info("Config imported");
                                dialog.set(None);
                            }
                        });
                    }
                }
                Ok(None) => {}
                Err(e) => store.error(format!("No file dialog: {e}")),
            }
        });
    };
    let on_submit = submit.clone();
    let on_press = submit.clone();
    card_frame("Add a Subscription", None)
    .child(form_row(
        "Link",
        ui::field(url, "https://example.com/subscribe?token=…")
            .w_full()
            .on_submit(move |_: &str| on_submit()),
    ))
    .child(form_row("Name", {
        let on_submit = submit.clone();
        ui::field(name, "From the provider")
            .w_full()
            .on_submit(move |_: &str| on_submit())
    }))
    .child(form_row(
        "Update",
        ui::popup_button(
            move || {
                let mut v = vec![(PROVIDER_CHOICE, "As the provider says".to_owned())];
                v.extend([6, 12, 24, 72, 168, 0].map(|h| (h, interval_label(h))));
                v
            },
            move || hours.get(),
            move |h| hours.set(h),
        ),
    ))
    .child(problem_box(problem, url))
    .child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .mt_1()
            .child(ui::btn("Import a File", Variant::Secondary).on_press(import))
            .child(view().flex_1())
            .child(ui::muted(move || -> SharedString {
                if busy.get() {
                    "Downloading…".into()
                } else {
                    "".into()
                }
            }))
            .child(ui::btn("Cancel", Variant::Secondary).on_press(move || dialog.set(None)))
            .child(
                ui::btn(
                    move || -> SharedString {
                        if problem.with(|p| p.is_some()) {
                            "Try Again".into()
                        } else {
                            "Add".into()
                        }
                    },
                    Variant::Primary,
                )
                .disabled(move || !valid() || busy.get())
                .on_press(move || on_press()),
            ),
    )
}

fn rename_card(id: String, current: String, dialog: Signal<Option<Dialog>>) -> impl IntoElement {
    let store = store();
    let name = signal(current);
    let valid = move || name.with(|n| !n.trim().is_empty());
    let submit = Rc::new(move || {
        if !valid() {
            return;
        }
        store.svc().rename_profile(
            id.clone(),
            name.get_untracked().trim().to_owned(),
            None,
            None,
        );
        dialog.set(None);
    });
    let on_submit = submit.clone();
    card_frame("Rename", None)
    .child(form_row(
        "Name",
        ui::field(name, "Name")
            .w_full()
            .on_submit(move |_: &str| on_submit()),
    ))
    .child(
        view()
            .flex_row()
            .gap_2()
            .mt_1()
            .child(view().flex_1())
            .child(ui::btn("Cancel", Variant::Secondary).on_press(move || dialog.set(None)))
            .child(
                ui::btn("Save", Variant::Primary)
                    .disabled(move || !valid())
                    .on_press(move || submit()),
            ),
    )
}

fn link_card(id: String, host: String, dialog: Signal<Option<Dialog>>) -> impl IntoElement {
    let store = store();
    // Empty on purpose: the current link carries the account's token.
    let url = signal(String::new());
    let valid = move || {
        url.with(|u| {
            let u = u.trim();
            u.starts_with("http://") || u.starts_with("https://")
        })
    };
    let submit = Rc::new(move || {
        if !valid() {
            return;
        }
        let svc = store.svc();
        svc.rename_profile(
            id.clone(),
            String::new(),
            None,
            Some(url.get_untracked().trim().to_owned()),
        );
        // Fetch from the new link at once, to see that it works.
        let f = svc.update_profile(id.clone());
        dialog.set(None);
        spawn(async move {
            let _ = f.await;
        });
    });
    let on_submit = submit.clone();
    card_frame("Change Link", Some(format!("Now: {host}")))
    .child(form_row(
        "Link",
        ui::field(url, "https://example.com/subscribe?token=…")
            .w_full()
            .on_submit(move |_: &str| on_submit()),
    ))
    .child(
        view()
            .flex_row()
            .gap_2()
            .mt_1()
            .child(view().flex_1())
            .child(ui::btn("Cancel", Variant::Secondary).on_press(move || dialog.set(None)))
            .child(
                ui::btn("Save and Update", Variant::Primary)
                    .disabled(move || !valid())
                    .on_press(move || submit()),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_000_000; // 2026-10-26 07:33 UTC
    const GB: u64 = 1024 * 1024 * 1024;

    fn usage(used_gb: u64, total_gb: u64, expire: Option<i64>) -> Usage {
        Usage {
            upload: 0,
            download: used_gb * GB,
            total: total_gb * GB,
            expire,
        }
    }

    #[test]
    fn updated_ago_in_the_largest_unit() {
        assert_eq!(updated_ago(None, NOW), "Never updated");
        assert_eq!(updated_ago(Some(NOW - 10), NOW), "Updated just now");
        assert_eq!(updated_ago(Some(NOW - 5 * 60), NOW), "Updated 5 min ago");
        assert_eq!(
            updated_ago(Some(NOW - 2 * 3600 - 59), NOW),
            "Updated 2 h ago"
        );
        assert_eq!(updated_ago(Some(NOW - 26 * 3600), NOW), "Updated 1 d ago");
        // A clock that went back is just now, not negative.
        assert_eq!(updated_ago(Some(NOW + 500), NOW), "Updated just now");
    }

    #[test]
    fn schedule_says_how_it_is_kept_current() {
        let remote = |h| Source::Remote {
            url: "https://x.example/a".into(),
            interval_hours: h,
        };
        assert_eq!(
            schedule_line(&remote(12), Some(NOW - 7200), NOW),
            "Updated 2 h ago · every 12 h"
        );
        assert_eq!(
            schedule_line(&remote(0), Some(NOW - 7200), NOW),
            "Updated 2 h ago · by hand"
        );
        assert_eq!(
            schedule_line(&Source::Local { origin: None }, Some(NOW), NOW),
            "Not updated automatically"
        );
        assert_eq!(interval_label(12), "Every 12 hours");
        assert_eq!(interval_label(24), "Every day");
        assert_eq!(interval_label(72), "Every 3 days");
        assert_eq!(interval_label(0), "Only by hand");
    }

    #[test]
    fn dates_are_month_and_day() {
        // 2026-12-02 12:00 UTC
        let t = 1_796_212_800;
        assert_eq!(month_day(t, 0), "Dec 2");
        // Near midnight the offset decides the day.
        assert_eq!(month_day(1_796_169_600 + 3600, 0), "Dec 2");
        assert_eq!(month_day(1_796_169_600 + 3600, -7200), "Dec 1");
        assert_eq!(month_day(0, 0), "Jan 1");
    }

    #[test]
    fn sizes_drop_needless_decimals() {
        assert_eq!(size_text(37 * GB), "37 GB");
        assert_eq!(size_text(41 * GB / 10), "4.1 GB");
        assert_eq!(size_text(0), "0 B");
        assert_eq!(used_of(37 * GB, 100 * GB), "37 of 100 GB");
        assert_eq!(used_of(500 * 1024 * 1024, 50 * GB), "500 MB of 50 GB");
    }

    #[test]
    fn the_usage_line_follows_the_header() {
        let off = 0;
        let expire = NOW + 22 * DAY;
        let line = usage_line(Some(&usage(37, 100, Some(expire))), NOW, off);
        assert_eq!(
            line,
            format!("37 of 100 GB · expires {}", month_day(expire, off))
        );
        assert_eq!(usage_line(None, NOW, off), "No usage data");
        assert_eq!(
            usage_line(Some(&usage(37, 0, None)), NOW, off),
            "37 GB used · no expiry"
        );
        assert!(usage_line(Some(&usage(1, 10, Some(NOW - DAY))), NOW, off).contains("expired"));
    }

    #[test]
    fn the_allowance_is_what_is_left_over_the_days_left() {
        assert_eq!(days_left(Some(NOW + 22 * DAY), NOW), Some(22));
        // A started day counts, so the last day never divides by zero.
        assert_eq!(days_left(Some(NOW + 60), NOW), Some(1));
        assert_eq!(days_left(Some(NOW - 1), NOW), None);
        assert_eq!(days_left(None, NOW), None);
        assert_eq!(allowance_per_day(63 * GB, 22), Some(63 * GB / 22));
        assert_eq!(allowance_per_day(63 * GB, 0), None);
        assert_eq!(left_bytes(&usage(37, 100, None)), Some(63 * GB));
        // Over the plan: nothing left, not a wrap-around.
        assert_eq!(left_bytes(&usage(120, 100, None)), Some(0));
        assert_eq!(left_bytes(&usage(37, 0, None)), None);
    }

    #[test]
    fn the_tiles_have_no_pace_and_no_projection() {
        let off = 0;
        let expire = NOW + 22 * DAY;
        let tiles = usage_tiles(&usage(37, 100, Some(expire)), NOW, off);
        assert_eq!(
            (tiles[0].value.as_str(), tiles[0].detail.as_str()),
            ("37 GB", "of 100 GB")
        );
        assert_eq!(tiles[1].value, "63 GB");
        assert!(tiles[1].detail.starts_with("22 days to "));
        assert_eq!(tiles[2].value, "—");
        assert!(tiles[3].title.starts_with("To last until "));
        assert_eq!(tiles[3].value, "≤ 2.9 GB/day");
        assert_eq!(tiles[3].detail, "63 GB over 22 days");
        for t in &tiles {
            assert!(!t.detail.contains("at this rate") && !t.detail.contains("runs out"));
        }
    }

    #[test]
    fn the_tiles_without_a_limit_or_an_expiry_say_so() {
        let tiles = usage_tiles(&usage(37, 0, None), NOW, 0);
        assert_eq!(tiles[1].value, "No limit");
        assert_eq!(tiles[3].value, "—");
        let tiles = usage_tiles(&usage(37, 100, None), NOW, 0);
        assert_eq!(tiles[1].detail, "no expiry");
        assert_eq!(tiles[3].value, "—");
        let tiles = usage_tiles(&usage(37, 100, Some(NOW - 5)), NOW, 0);
        assert!(tiles[1].detail.starts_with("expired "));
        assert_eq!(tiles[3].value, "—");
        let tiles = usage_tiles(&usage(1, 100, Some(NOW + 3600)), NOW, 0);
        assert_eq!(tiles[3].detail, "99 GB over 1 day");
    }

    #[test]
    fn a_link_shows_only_its_host() {
        assert_eq!(
            host_of("https://sub.example.net/api/v1/client/9f2c?token=SECRET"),
            "sub.example.net"
        );
        assert_eq!(host_of("not a link, SECRET"), "Subscription");
        let remote = Source::Remote {
            url: "https://cdn.example.org/x?token=SECRET".into(),
            interval_hours: 24,
        };
        assert_eq!(source_label(&remote), "cdn.example.org");
        assert_eq!(
            source_label(&Source::Local { origin: None }),
            "Imported file"
        );
    }

    #[test]
    fn errors_never_carry_a_link() {
        let url = "https://sub.example.net/api?token=SECRET";
        let said = scrub(
            &format!("error sending request for url ({url}): timed out"),
            Some(url),
        );
        assert_eq!(
            said,
            "error sending request for url (sub.example.net): timed out"
        );
        // Another link than the subscription's, and one without a host.
        let said = scrub(
            "redirected to http://other.example/x?token=SECRET and then http:// failed",
            None,
        );
        assert!(!said.contains("SECRET") && !said.contains("/x"), "{said}");
        assert!(said.contains("other.example"));
        let (headline, hint) = plain_error(&format!("Download failed: bad url {url}"), Some(url));
        assert!(!headline.contains("SECRET"));
        assert!(!hint.unwrap_or_default().contains("SECRET"));
    }

    #[test]
    fn a_server_error_is_the_providers() {
        let (headline, hint) = plain_error("The server answered HTTP 503", None);
        assert_eq!(headline, "Your provider says: “503 Service Unavailable”");
        assert!(hint.is_some());
        assert_eq!(
            banner_text(&headline),
            "couldn't update · Your provider says: “503 Service Unavailable”. Using the saved copy."
        );
        // A refused link is a different story, and a headline's full stop is not doubled.
        let (headline, _) = plain_error("The server answered HTTP 404", None);
        assert_eq!(headline, "The provider refused this link.");
        assert_eq!(
            banner_text(&headline),
            "couldn't update · The provider refused this link. Using the saved copy."
        );
    }

    #[test]
    fn the_menu_offers_the_current_interval() {
        assert_eq!(interval_choices(12), vec![6, 12, 24, 72, 168, 0]);
        assert_eq!(interval_choices(5), vec![5, 6, 12, 24, 72, 168, 0]);
    }
}
