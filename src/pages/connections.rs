//! Connections: every open connection, live; search, pause, close one or all,
//! and the selected one's details beside the table.

use std::sync::Arc;

use kairo_core::model::Connection;
use nsh_base::{button, theme};
use nsh_core::prelude::*;

use crate::store::{Snap, store};
use crate::ui::menu::{self, Choice};
use crate::ui::{self, Icon, Variant, icon, size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    Newest,
    Download,
    Upload,
    Total,
}

/// Column widths at the text's size (the host takes the rest).
fn w(points: f32) -> f32 {
    (points * size::scale()).round()
}
#[allow(non_snake_case)]
fn W_PROCESS() -> f32 {
    w(110.0)
}
#[allow(non_snake_case)]
fn W_TIME() -> f32 {
    w(64.0)
}
#[allow(non_snake_case)]
fn W_CHAIN() -> f32 {
    w(190.0)
}
#[allow(non_snake_case)]
fn W_SPEED() -> f32 {
    w(76.0)
}
#[allow(non_snake_case)]
fn W_TOTAL() -> f32 {
    w(76.0)
}
const W_CLOSE: f32 = 32.0;

/// The connection's exit and node, in words: `AI exit · Tokyo 03`.
pub fn goes_through(c: &Connection, names: &kairo_core::activity::ExitNames) -> String {
    use kairo_core::activity::{ExitKind, exit_of};
    let kind = exit_of(c, names);
    match kind {
        ExitKind::Direct | ExitKind::Blocked | ExitKind::Local => kind.title().to_owned(),
        _ => {
            let node = c.outbound();
            let node = kairo_core::places::split_flag(node).1.trim().to_owned();
            if node.is_empty() {
                kind.title().to_owned()
            } else {
                format!("{} · {node}", kind.title())
            }
        }
    }
}

/// How long ago, short: `41 m`, `1 h 12 m`, `3 d`.
pub fn age(start: &str, now: i64) -> String {
    let Some(t) = kairo_core::activity::parse_time(start) else {
        return String::new();
    };
    let s = (now - t).max(0);
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} m", s / 60),
        3600..=86399 => format!("{} h {} m", s / 3600, s % 3600 / 60),
        _ => format!("{} d", s / 86400),
    }
}

/// The open connections as a table, with the selected one's details beside it
/// (Activity › All Connections). `query` filters; `paused` holds a snapshot.
pub fn list(
    query: Signal<String>,
    paused: Signal<Option<Snap<kairo_core::model::Connections>>>,
) -> impl IntoElement {
    let store = store();
    let order = signal(Order::Newest);
    let selected = signal(None::<String>);

    let source = move || paused.get().unwrap_or_else(|| store.connections.get());
    let rows = computed(move || {
        let q = query.get().trim().to_lowercase();
        let snap = source();
        let mut list: Vec<Connection> = snap
            .0
            .list
            .iter()
            .filter(|c| c.matches(&q))
            .cloned()
            .collect();
        match order.get() {
            Order::Newest => {}
            Order::Download => list.sort_by(|a, b| b.down_speed.cmp(&a.down_speed)),
            Order::Upload => list.sort_by(|a, b| b.up_speed.cmp(&a.up_speed)),
            Order::Total => {
                list.sort_by(|a, b| (b.download + b.upload).cmp(&(a.download + a.upload)))
            }
        }
        Arc::new(list)
    });
    let count = move || rows.with(|r| r.len());
    // With the inspector open the table keeps the columns the inspector doesn't show.
    let compact = computed(move || selected.with(Option::is_some));
    // Cores name processes only where they can see them (often not at all).
    let processes = computed(move || {
        store
            .connections
            .with(|c| c.0.list.iter().any(|c| !c.process.is_empty()))
    });

    let phone = ui::phone();
    view()
        .flex_1()
        .flex_row()
        .min_h(px(0.0))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                // On a phone a connection's details take the whole screen.
                .style_when(move || phone && selected.with(Option::is_some), |s| s.hidden())
                .child(if phone {
                    sort_bar(order).into_element()
                } else {
                    header_row(order, compact, processes).into_element()
                })
                .child(
                    when(
                        move || count() > 0,
                        move || table(rows, selected, compact, processes),
                    )
                    .otherwise(move || {
                        ui::empty_state(
                            Icon::Connections,
                            move || -> SharedString {
                                if !store.state.with(|s| s.core.is_running()) {
                                    "Not connected".into()
                                } else if query.with(|q| !q.is_empty()) {
                                    "Nothing matches".into()
                                } else {
                                    "No connections".into()
                                }
                            },
                            None,
                        )
                    }),
                )
                .child(footer()),
        )
        .child(when(
            move || selected.with(Option::is_some),
            move || inspector(selected, source),
        ))
}

fn head(
    label: &'static str,
    w: Option<f32>,
    right: bool,
    sort: Option<(Signal<Order>, Order)>,
) -> impl IntoElement {
    let active = move || sort.is_some_and(|(s, o)| s.get() == o);
    let t = text(label)
        .text_size(px(size::small()))
        .font_weight(FontWeight::MEDIUM)
        .text_color(move || {
            if active() {
                theme().foreground
            } else {
                theme().muted_foreground
            }
        })
        .number_of_lines(1);
    let cell = button()
        .flex_row()
        .items_center()
        .gap_1()
        .when(right, |b| b.justify_end())
        .when_some(w, |b, w| b.w(px(w)))
        .when(w.is_none(), |b| b.flex_1().min_w(px(0.0)))
        .child(t)
        .child(
            icon(Icon::ArrowDown, 10.0)
                .text_color(move || theme().foreground)
                .style_when(move || !active(), |s| s.invisible()),
        );
    match sort {
        Some((s, o)) => cell
            .cursor_pointer()
            .on_press(move || {
                s.set(if s.get_untracked() == o {
                    Order::Newest
                } else {
                    o
                })
            })
            .into_element(),
        None => cell.disabled(true).into_element(),
    }
}

/// A phone's stand-in for the column headings: how the list is ordered.
fn sort_bar(order: Signal<Order>) -> impl IntoElement {
    let name = move || match order.get() {
        Order::Newest => "Newest",
        Order::Download => "Download speed",
        Order::Upload => "Upload speed",
        Order::Total => "Total",
    };
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_5()
        .py_1()
        .border_b_1()
        .border_color(move || theme().border)
        .child(ui::muted("Sorted by").flex_1())
        .child(
            button()
                .flex_row()
                .items_center()
                .gap_1()
                .h(px(ui::touch_h(40.0)))
                .accessibility_label("Sort connections")
                .on_press(move || {
                    let pick = |o: Order, title: &'static str| {
                        Choice::new(title)
                            .checked(order.get_untracked() == o)
                            .on_select(move || order.set(o))
                    };
                    menu::open(vec![
                        pick(Order::Newest, "Newest"),
                        pick(Order::Download, "Download speed"),
                        pick(Order::Upload, "Upload speed"),
                        pick(Order::Total, "Total"),
                    ]);
                })
                .child(
                    text(move || -> SharedString { name().into() })
                        .text_size(px(size::body()))
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(icon(Icon::ArrowDown, 12.0).text_color(move || theme().muted_foreground)),
        )
}

fn header_row(
    order: Signal<Order>,
    compact: Memo<bool>,
    processes: Memo<bool>,
) -> impl IntoElement {
    let wide = move || !compact.get();
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .h(px(28.0))
        .px_5()
        .border_b_1()
        .border_color(move || theme().border)
        .bg(move || ui::subtle())
        .child(head("Host", None, false, None))
        .child(when(
            move || wide() && processes.get(),
            move || head("Process", Some(W_PROCESS()), false, None),
        ))
        .child(head("Goes through", Some(W_CHAIN()), false, None))
        .child(head(
            "Down",
            Some(W_SPEED()),
            true,
            Some((order, Order::Download)),
        ))
        .child(when(wide, move || {
            head("Up", Some(W_SPEED()), true, Some((order, Order::Upload)))
        }))
        .child(head(
            "Total",
            Some(W_TOTAL()),
            true,
            Some((order, Order::Total)),
        ))
        .child(when(wide, move || {
            head("Time", Some(W_TIME()), false, None)
        }))
        .child(view().w(px(W_CLOSE)))
}

fn cell(
    content: impl IntoReactive<SharedString>,
    w: Option<f32>,
    right: bool,
    muted: bool,
) -> Text {
    let t = text(content)
        .text_size(px(size::body()))
        .number_of_lines(1)
        .when(right, |t| t.text_right().tabular_nums())
        .when(muted, |t| t.text_color(move || theme().muted_foreground));
    match w {
        Some(w) => t.w(px(w)),
        None => t.flex_1().min_w(px(0.0)),
    }
}

fn table(
    rows: Memo<Arc<Vec<Connection>>>,
    selected: Signal<Option<String>>,
    compact: Memo<bool>,
    processes: Memo<bool>,
) -> impl IntoElement {
    let store = store();
    let narrow = move || compact.get();
    virtual_list(
        move || rows.with(|r| r.len()),
        move |at| {
            let conn = computed(move || rows.with(|r| r.get(at.index()).cloned()));
            let f = move |g: fn(&Connection) -> String| {
                move || -> SharedString {
                    conn.with(|c| c.as_ref().map(g).unwrap_or_default()).into()
                }
            };
            let id = move || conn.with(|c| c.as_ref().map(|c| c.id.clone()).unwrap_or_default());
            let is = move || selected.with(|s| s.as_deref() == Some(id().as_str()));
            if ui::phone() {
                return phone_row(conn, selected, id).into_element();
            }
            button()
                .on_press(move || {
                    let id = id();
                    selected.set(
                        if selected.get_untracked().as_deref() == Some(id.as_str()) {
                            None
                        } else {
                            Some(id)
                        },
                    );
                })
                .selected(is)
                .flex_row()
                .items_center()
                .gap_3()
                .h(px(ui::ROW_HEIGHT))
                .px_5()
                .cursor_pointer()
                .hover(|s| s.bg(move || theme().accent))
                .style_when(is, |s| s.bg(move || ui::selection()))
                .child(cell(
                    f(|c| format!("{}:{}", c.host, c.port)),
                    None,
                    false,
                    false,
                ))
                .child(
                    cell(
                        f(|c| {
                            if c.process.is_empty() {
                                "—".into()
                            } else {
                                c.process.clone()
                            }
                        }),
                        Some(W_PROCESS()),
                        false,
                        true,
                    )
                    .style_when(move || narrow() || !processes.get(), |s| s.hidden()),
                )
                .child(cell(
                    move || -> SharedString {
                        let names = store.activity.with(|a| a.0.names.clone());
                        conn.with(|c| {
                            c.as_ref()
                                .map(|c| goes_through(c, &names))
                                .unwrap_or_default()
                        })
                        .into()
                    },
                    Some(W_CHAIN()),
                    false,
                    false,
                ))
                .child(cell(
                    f(|c| {
                        if c.down_speed == 0 {
                            "—".into()
                        } else {
                            kairo_core::speed(c.down_speed)
                        }
                    }),
                    Some(W_SPEED()),
                    true,
                    false,
                ))
                .child(
                    cell(
                        f(|c| {
                            if c.up_speed == 0 {
                                "—".into()
                            } else {
                                kairo_core::speed(c.up_speed)
                            }
                        }),
                        Some(W_SPEED()),
                        true,
                        false,
                    )
                    .style_when(narrow, |s| s.hidden()),
                )
                .child(cell(
                    f(|c| kairo_core::bytes(c.download + c.upload)),
                    Some(W_TOTAL()),
                    true,
                    true,
                ))
                .child(
                    cell(
                        f(|c| age(&c.start, kairo_core::unix())),
                        Some(W_TIME()),
                        false,
                        true,
                    )
                    .style_when(narrow, |s| s.hidden()),
                )
                .child(
                    view().w(px(W_CLOSE)).items_end().child(
                        ui::icon_btn(Icon::Close, "Close Connection")
                            .size(px(22.0))
                            .on_press(move || store.run(store.svc().close_connection(id()))),
                    ),
                )
                .into_element()
        },
    )
    .keys(move || rows.with(|r| r.iter().map(|c| c.id.clone()).collect::<Vec<_>>()))
    .estimated_item_size(px(ui::ROW_HEIGHT))
    .flex_1()
}

/// One connection on a phone: where it goes and what it is, the total and the speed, close.
fn phone_row(
    conn: Memo<Option<Connection>>,
    selected: Signal<Option<String>>,
    id: impl Fn() -> String + Copy + 'static,
) -> impl IntoElement {
    let store = store();
    let line = move |g: fn(&Connection, &kairo_core::activity::ExitNames) -> String| {
        move || -> SharedString {
            let names = store.activity.with(|a| a.0.names.clone());
            conn.with(|c| c.as_ref().map(|c| g(c, &names)).unwrap_or_default())
                .into()
        }
    };
    button()
        .on_press(move || {
            let id = id();
            selected.set(Some(id));
        })
        .w_full()
        .flex_row()
        .items_center()
        .gap_3()
        .pl_5()
        .pr_3()
        .py_2()
        .border_b_1()
        .border_color(move || theme().border)
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text(line(|c, _| format!("{}:{}", c.host, c.port)))
                        .text_size(px(size::body()))
                        .number_of_lines(1),
                )
                .child(
                    ui::muted(line(|c, names| {
                        let mut s = goes_through(c, names);
                        if !c.process.is_empty() {
                            s = format!("{} · {s}", c.process);
                        }
                        s
                    }))
                    .text_size(px(size::small()))
                    .number_of_lines(2),
                ),
        )
        .child(
            view()
                .items_end()
                .child(
                    text(line(|c, _| kairo_core::bytes(c.download + c.upload)))
                        .text_size(px(size::body()))
                        .tabular_nums(),
                )
                .child(
                    ui::muted(line(|c, _| {
                        if c.down_speed == 0 {
                            age(&c.start, kairo_core::unix())
                        } else {
                            format!("↓ {}", kairo_core::speed(c.down_speed))
                        }
                    }))
                    .text_size(px(size::small()))
                    .tabular_nums(),
                ),
        )
        .child(
            ui::icon_btn(Icon::Close, "Close Connection")
                .on_press(move || store.run(store.svc().close_connection(id()))),
        )
}

fn footer() -> impl IntoElement {
    let store = store();
    let phone = ui::phone();
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_x_5()
        .when(!phone, |v| v.h(px(28.0)))
        .when(phone, |v| v.py_1p5())
        .px_5()
        .border_t_1()
        .border_color(move || theme().border)
        .child(
            ui::muted(move || -> SharedString {
                format!("↓ {}", kairo_core::speed(store.live.with(|l| l.down))).into()
            })
            .tabular_nums(),
        )
        .child(
            ui::muted(move || -> SharedString {
                format!("↑ {}", kairo_core::speed(store.live.with(|l| l.up))).into()
            })
            .tabular_nums(),
        )
        .child(view().flex_1().min_w(px(0.0)))
        // Alone on its line on a phone, where it wraps instead of running off the screen.
        .child(view().when(phone, |v| v.w_full()).child(ui::muted(move || -> SharedString {
            store
                .live
                .with(|l| {
                    let memory = if l.memory > 0 {
                        format!(" · Core memory {}", kairo_core::bytes(l.memory))
                    } else {
                        String::new()
                    };
                    format!(
                        "Downloaded {} · Uploaded {}{memory}",
                        kairo_core::bytes(l.download_total),
                        kairo_core::bytes(l.upload_total)
                    )
                })
                .into()
        })))
}

fn detail_row(label: &'static str, value: String) -> impl IntoElement {
    let has_flag = kairo_core::places::find_flag(&value).1.is_some();
    view().gap_0p5().child(ui::muted(label)).child(if has_flag {
        // A node's flag is a picture (not every platform draws the emoji).
        let shown: SharedString = value.into();
        ui::flagged(move || shown.clone(), size::body()).into_element()
    } else {
        text(if value.is_empty() {
            "—".to_owned()
        } else {
            value
        })
        .text_size(px(size::body()))
        .selectable(true)
        .into_element()
    })
}

fn inspector(
    selected: Signal<Option<String>>,
    source: impl Fn() -> Snap<kairo_core::model::Connections> + Copy + 'static,
) -> impl IntoElement {
    let store = store();
    let conn = computed(move || {
        let id = selected.get()?;
        source().0.list.iter().find(|c| c.id == id).cloned()
    });
    let phone = ui::phone();
    view()
        .when(!phone, |v| v.w(px(300.0)).flex_shrink_0().border_l_1())
        .when(phone, |v| v.flex_1().min_w(px(0.0)))
        .border_color(move || theme().border)
        .bg(move || ui::subtle())
        .child(
            view()
                .flex_row()
                .items_center()
                .h(px(40.0))
                .px_4()
                .border_b_1()
                .border_color(move || theme().border)
                .child(ui::section_label("Details").flex_1())
                .child(
                    ui::icon_btn(Icon::Close, "Close Details")
                        .size(px(22.0))
                        .on_press(move || selected.set(None)),
                ),
        )
        .child(dynamic(move || {
            let c = conn.get();
            untracked(move || match c {
                None => ui::empty_state(
                    Icon::Connections,
                    "Closed",
                    None,
                )
                .into_element(),
                Some(c) => {
                    let id = c.id.clone();
                    scroll()
                        .flex_1()
                        .child(
                            view()
                                .p_4()
                                .gap_3()
                                .child(detail_row("Host", format!("{}:{}", c.host, c.port)))
                                .child(detail_row("Destination IP", c.destination_ip.clone()))
                                .child(detail_row("Source", c.source.clone()))
                                .child(detail_row("Network", c.network.to_uppercase()))
                                .child(detail_row("Inbound", c.inbound.clone()))
                                .child(detail_row(
                                    "Process",
                                    if c.process_path.is_empty() {
                                        c.process.clone()
                                    } else {
                                        c.process_path.clone()
                                    },
                                ))
                                .child(detail_row(
                                    "Rule",
                                    if c.rule_payload.is_empty() {
                                        c.rule.clone()
                                    } else {
                                        format!("{} ({})", c.rule, c.rule_payload)
                                    },
                                ))
                                .child(detail_row(
                                    "Chain",
                                    c.chains
                                        .iter()
                                        .rev()
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join(" → "),
                                ))
                                .child(detail_row(
                                    "Traffic",
                                    format!(
                                        "↓ {}  ↑ {}",
                                        kairo_core::bytes(c.download),
                                        kairo_core::bytes(c.upload)
                                    ),
                                ))
                                .child(detail_row(
                                    "Started",
                                    c.start
                                        .replace('T', " ")
                                        .split('.')
                                        .next()
                                        .unwrap_or("")
                                        .to_owned(),
                                ))
                                .child(always_buttons(&c))
                                .child(
                                    ui::btn_icon(
                                        Icon::Close,
                                        "Close Connection",
                                        Variant::Destructive,
                                    )
                                    .self_start()
                                    .mt_2()
                                    .on_press(move || {
                                        store.run(store.svc().close_connection(id.clone()))
                                    }),
                                ),
                        )
                        .into_element()
                }
            })
        }))
}

/// "Always Direct · Always AI Exit · Block" for the connection's site.
fn always_buttons(c: &Connection) -> impl IntoElement {
    use kairo_core::routes::{RouteExit, RouteKind};
    let store = store();
    let site = kairo_core::activity::site_of(&c.host);
    let one = move |label: &'static str, exit: RouteExit| {
        let site = site.clone();
        ui::btn(label, Variant::Secondary).on_press(move || {
            store.run_then(
                store.svc().set_route(
                    RouteKind::Site {
                        domain: site.clone(),
                    },
                    exit.clone(),
                ),
                move |r| {
                    if r.is_ok() {
                        store.info("Saved in Apps & Sites");
                    }
                },
            )
        })
    };
    view().gap_2().child(ui::muted("For this site")).child(
        view()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .child(one("Always Direct", RouteExit::Direct))
            .child(one("Always AI Exit", RouteExit::Ai))
            .child(one("Block", RouteExit::Block)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_short() {
        let start = "2026-11-09T15:00:00Z";
        let t = kairo_core::activity::parse_time(start).unwrap();
        assert_eq!(age(start, t + 30), "30 s");
        assert_eq!(age(start, t + 41 * 60), "41 m");
        assert_eq!(age(start, t + 3600 + 12 * 60), "1 h 12 m");
        assert_eq!(age(start, t + 3 * 86400), "3 d");
        assert_eq!(age("nope", t), "");
    }
}
