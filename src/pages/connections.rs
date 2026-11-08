//! Connections: every open connection, live; search, pause, close one or all,
//! and the selected one's details beside the table.

use std::sync::Arc;

use kairo_core::model::Connection;
use nsh_base::{button, theme};
use nsh_core::prelude::*;

use crate::store::{Snap, store};
use crate::ui::{self, Icon, Variant, icon, size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    Newest,
    Download,
    Upload,
    Total,
}

/// Column widths (the host takes the rest).
const W_PROCESS: f32 = 120.0;
const W_RULE: f32 = 150.0;
const W_CHAIN: f32 = 170.0;
const W_SPEED: f32 = 84.0;
const W_TOTAL: f32 = 84.0;
const W_CLOSE: f32 = 32.0;

pub fn page() -> impl IntoElement {
    let store = store();
    let query = signal(String::new());
    let paused = signal(None::<Snap<kairo_core::model::Connections>>);
    let order = signal(Order::Newest);
    let selected = signal(None::<String>);

    let source = move || paused.get().unwrap_or_else(|| store.connections.get());
    let rows = computed(move || {
        let q = query.get().trim().to_lowercase();
        let snap = source();
        let mut list: Vec<Connection> = snap.0.list.iter().filter(|c| c.matches(&q)).cloned().collect();
        match order.get() {
            Order::Newest => {}
            Order::Download => list.sort_by(|a, b| b.down_speed.cmp(&a.down_speed)),
            Order::Upload => list.sort_by(|a, b| b.up_speed.cmp(&a.up_speed)),
            Order::Total => list.sort_by(|a, b| (b.download + b.upload).cmp(&(a.download + a.upload))),
        }
        Arc::new(list)
    });
    let count = move || rows.with(|r| r.len());
    // With the inspector open the table keeps the columns the inspector doesn't show.
    let compact = computed(move || selected.with(Option::is_some));
    let total = move || store.connections.with(|c| c.0.list.len());

    view()
        .flex_1()
        .child(ui::page_header(
            move || -> SharedString {
                let (shown, all) = (count(), total());
                if shown == all { format!("Connections  {all}").into() } else { format!("Connections  {shown} of {all}").into() }
            },
            vec![
                ui::search_field(query, "Host, process, rule or node").w(px(240.0)).into_element(),
                ui::btn_icon_dyn(
                    move || if paused.with(Option::is_some) { Icon::Play } else { Icon::Pause },
                    move || -> SharedString { if paused.with(Option::is_some) { "Resume".into() } else { "Pause".into() } },
                    Variant::Secondary,
                )
                .on_press(move || {
                    if paused.with_untracked(Option::is_some) {
                        paused.set(None);
                    } else {
                        paused.set(Some(store.connections.get_untracked()));
                    }
                })
                .into_element(),
                ui::btn_icon(Icon::Close, "Close All", Variant::Destructive)
                    .disabled(move || total() == 0)
                    .on_press(move || {
                        let svc = store.svc();
                        store.run_then(svc.close_all_connections(), move |_| paused.set(None));
                    })
                    .into_element(),
            ],
        ))
        .child(
            view()
                .flex_1()
                .flex_row()
                .min_h(px(0.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(header_row(order, compact))
                        .child(when(move || count() > 0, move || table(rows, selected, compact)).otherwise(move || {
                            ui::empty_state(
                                Icon::Connections,
                                "No connections",
                                move || -> SharedString {
                                    if !store.state.with(|s| s.core.is_running()) {
                                        "Connections appear while the core is running.".into()
                                    } else if query.with(|q| !q.is_empty()) {
                                        "Nothing matches the filter.".into()
                                    } else {
                                        "Apps that use the proxy show up here.".into()
                                    }
                                },
                                None,
                            )
                        }))
                        .child(footer()),
                )
                .child(when(move || selected.with(Option::is_some), move || inspector(selected, source))),
        )
}

fn head(label: &'static str, w: Option<f32>, right: bool, sort: Option<(Signal<Order>, Order)>) -> impl IntoElement {
    let active = move || sort.is_some_and(|(s, o)| s.get() == o);
    let t = text(label)
        .text_size(px(size::SMALL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(move || if active() { theme().foreground } else { theme().muted_foreground })
        .number_of_lines(1);
    let cell = button()
        .flex_row()
        .items_center()
        .gap_1()
        .when(right, |b| b.justify_end())
        .when_some(w, |b, w| b.w(px(w)))
        .when(w.is_none(), |b| b.flex_1().min_w(px(0.0)))
        .child(t)
        .child(icon(Icon::ArrowDown, 10.0).text_color(move || theme().foreground).style_when(move || !active(), |s| s.invisible()));
    match sort {
        Some((s, o)) => cell.cursor_pointer().on_press(move || s.set(if s.get_untracked() == o { Order::Newest } else { o })).into_element(),
        None => cell.disabled(true).into_element(),
    }
}

fn header_row(order: Signal<Order>, compact: Memo<bool>) -> impl IntoElement {
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
        .child(when(wide, move || head("Process", Some(W_PROCESS), false, None)))
        .child(when(wide, move || head("Rule", Some(W_RULE), false, None)))
        .child(head("Node", Some(W_CHAIN), false, None))
        .child(head("Down", Some(W_SPEED), true, Some((order, Order::Download))))
        .child(when(wide, move || head("Up", Some(W_SPEED), true, Some((order, Order::Upload)))))
        .child(head("Total", Some(W_TOTAL), true, Some((order, Order::Total))))
        .child(view().w(px(W_CLOSE)))
}

fn cell(content: impl IntoReactive<SharedString>, w: Option<f32>, right: bool, muted: bool) -> Text {
    let t = text(content)
        .text_size(px(size::BODY))
        .number_of_lines(1)
        .when(right, |t| t.text_right().tabular_nums())
        .when(muted, |t| t.text_color(move || theme().muted_foreground));
    match w {
        Some(w) => t.w(px(w)),
        None => t.flex_1().min_w(px(0.0)),
    }
}

fn table(rows: Memo<Arc<Vec<Connection>>>, selected: Signal<Option<String>>, compact: Memo<bool>) -> impl IntoElement {
    let store = store();
    let narrow = move || compact.get();
    virtual_list(move || rows.with(|r| r.len()), move |at| {
        let conn = computed(move || rows.with(|r| r.get(at.index()).cloned()));
        let f = move |g: fn(&Connection) -> String| move || -> SharedString { conn.with(|c| c.as_ref().map(g).unwrap_or_default()).into() };
        let id = move || conn.with(|c| c.as_ref().map(|c| c.id.clone()).unwrap_or_default());
        let is = move || selected.with(|s| s.as_deref() == Some(id().as_str()));
        button()
            .on_press(move || {
                let id = id();
                selected.set(if selected.get_untracked().as_deref() == Some(id.as_str()) { None } else { Some(id) });
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
            .child(cell(f(|c| format!("{}:{}", c.host, c.port)), None, false, false))
            .child(
                cell(f(|c| if c.process.is_empty() { "—".into() } else { c.process.clone() }), Some(W_PROCESS), false, true)
                    .style_when(narrow, |s| s.hidden()),
            )
            .child(
                cell(
                    f(|c| if c.rule_payload.is_empty() { c.rule.clone() } else { format!("{} {}", c.rule, c.rule_payload) }),
                    Some(W_RULE),
                    false,
                    true,
                )
                .style_when(narrow, |s| s.hidden()),
            )
            .child(cell(f(|c| c.outbound().to_owned()), Some(W_CHAIN), false, false))
            .child(cell(f(|c| if c.down_speed == 0 { "—".into() } else { kairo_core::speed(c.down_speed) }), Some(W_SPEED), true, false))
            .child(
                cell(f(|c| if c.up_speed == 0 { "—".into() } else { kairo_core::speed(c.up_speed) }), Some(W_SPEED), true, false)
                    .style_when(narrow, |s| s.hidden()),
            )
            .child(cell(f(|c| kairo_core::bytes(c.download + c.upload)), Some(W_TOTAL), true, true))
            .child(
                view().w(px(W_CLOSE)).items_end().child(
                    ui::icon_btn(Icon::Close, "Close Connection").size(px(22.0)).on_press(move || store.run(store.svc().close_connection(id()))),
                ),
            )
    })
    .keys(move || rows.with(|r| r.iter().map(|c| c.id.clone()).collect::<Vec<_>>()))
    .estimated_item_size(px(ui::ROW_HEIGHT))
    .flex_1()
}

fn footer() -> impl IntoElement {
    let store = store();
    view()
        .flex_row()
        .items_center()
        .gap_5()
        .h(px(28.0))
        .px_5()
        .border_t_1()
        .border_color(move || theme().border)
        .child(ui::muted(move || -> SharedString { format!("↓ {}", kairo_core::speed(store.live.with(|l| l.down))).into() }).tabular_nums())
        .child(ui::muted(move || -> SharedString { format!("↑ {}", kairo_core::speed(store.live.with(|l| l.up))).into() }).tabular_nums())
        .child(view().flex_1())
        .child(ui::muted(move || -> SharedString {
            store.live.with(|l| format!("Downloaded {} · Uploaded {}", kairo_core::bytes(l.download_total), kairo_core::bytes(l.upload_total))).into()
        }))
}

fn detail_row(label: &'static str, value: String) -> impl IntoElement {
    view()
        .gap_0p5()
        .child(ui::muted(label))
        .child(text(if value.is_empty() { "—".to_owned() } else { value }).text_size(px(size::BODY)).selectable(true))
}

fn inspector(selected: Signal<Option<String>>, source: impl Fn() -> Snap<kairo_core::model::Connections> + Copy + 'static) -> impl IntoElement {
    let store = store();
    let conn = computed(move || {
        let id = selected.get()?;
        source().0.list.iter().find(|c| c.id == id).cloned()
    });
    view()
        .w(px(300.0))
        .flex_shrink_0()
        .border_l_1()
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
                .child(ui::icon_btn(Icon::Close, "Close Details").size(px(22.0)).on_press(move || selected.set(None))),
        )
        .child(dynamic(move || {
            let c = conn.get();
            untracked(move || match c {
                None => ui::empty_state(Icon::Connections, "Closed", "This connection has ended.", None).into_element(),
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
                                .child(detail_row("Process", if c.process_path.is_empty() { c.process.clone() } else { c.process_path.clone() }))
                                .child(detail_row(
                                    "Rule",
                                    if c.rule_payload.is_empty() { c.rule.clone() } else { format!("{} ({})", c.rule, c.rule_payload) },
                                ))
                                .child(detail_row("Chain", c.chains.iter().rev().cloned().collect::<Vec<_>>().join(" → ")))
                                .child(detail_row(
                                    "Traffic",
                                    format!("↓ {}  ↑ {}", kairo_core::bytes(c.download), kairo_core::bytes(c.upload)),
                                ))
                                .child(detail_row("Started", c.start.replace('T', " ").split('.').next().unwrap_or("").to_owned()))
                                .child(
                                    ui::btn_icon(Icon::Close, "Close Connection", Variant::Destructive)
                                        .self_start()
                                        .mt_2()
                                        .on_press(move || store.run(store.svc().close_connection(id.clone()))),
                                ),
                        )
                        .into_element()
                }
            })
        }))
}
