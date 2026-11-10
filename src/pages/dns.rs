//! DNS (Settings › Advanced › Inspect): ask the core's resolver (the same DNS the proxy uses) and see the
//! answers; flush the fake-IP cache.

use kairo_core::model::{DnsAnswer, dns_type_name};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::store::{Page, store};
use crate::ui::{self, Icon, Variant, size};

#[derive(Debug, Clone, PartialEq)]
struct Lookup {
    id: u64,
    name: String,
    kind: &'static str,
    status: i32,
    millis: u128,
    answers: Vec<DnsAnswer>,
    error: Option<String>,
}

fn status_name(code: i32) -> &'static str {
    match code {
        0 => "NOERROR",
        1 => "FORMERR",
        2 => "SERVFAIL",
        3 => "NXDOMAIN",
        4 => "NOTIMP",
        5 => "REFUSED",
        _ => "ERROR",
    }
}

const KINDS: [&str; 6] = ["A", "AAAA", "CNAME", "MX", "TXT", "NS"];

pub fn page() -> impl IntoElement {
    let store = store();
    let name = signal(String::new());
    let kind = signal("A");
    let busy = signal(false);
    let history = signal(Vec::<Lookup>::new());
    let next_id = signal(0u64);
    let running = computed(move || store.state.with(|s| s.core.is_running()));

    let query = move || {
        let n = name.get_untracked().trim().trim_end_matches('.').to_owned();
        if n.is_empty() || busy.get_untracked() || !running.get_untracked() {
            return;
        }
        busy.set(true);
        let k = kind.get_untracked();
        let id = next_id.get_untracked() + 1;
        next_id.set(id);
        let started = std::time::Instant::now();
        let svc = store.svc();
        spawn(async move {
            let r = svc.dns_query(n.clone(), k.to_owned()).await;
            let millis = started.elapsed().as_millis();
            let lookup = match r {
                Ok(resp) => Lookup {
                    id,
                    name: n,
                    kind: k,
                    status: resp.status,
                    millis,
                    answers: resp.answer.unwrap_or_default(),
                    error: None,
                },
                Err(e) => Lookup {
                    id,
                    name: n,
                    kind: k,
                    status: -1,
                    millis,
                    answers: vec![],
                    error: Some(e),
                },
            };
            history.update(|h| {
                h.insert(0, lookup);
                h.truncate(20);
            });
            busy.set(false);
        });
    };

    view()
        .flex_1()
        .child(ui::toolbar::floating(
            Page::Dns,
            String::new,
            vec![crate::pages::settings::back()],
        ))
        .child(
            scroll().flex_1().child(
                view()
                    .px_6()
                    .pt(ui::toolbar::scroll_top(20.0))
                    .pb_6()
                    .gap_4()
                    .w_full()
                    .max_w(px(900.0))
                    .self_center()
                    .child(
                        ui::card()
                            .p_4()
                            .gap_3()
                            .child(ui::section_label("Look Up"))
                            .child(ui::muted("Queries go to the core's resolver, so the answers are the ones proxied apps get (fake-IP included)."))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    // A phone: the name, the record types and the button each on a line.
                                    .when(ui::phone(), |v| v.flex_col().items_stretch())
                                    .child(ui::field(name, "example.com").flex_1().on_submit(move |_: &str| query()))
                                    .child(view().when(ui::phone(), |v| v.w_full()).child(ui::segmented(KINDS.iter().map(|k| (*k, *k)).collect(), move || kind.get(), move |k| kind.set(k))))
                                    .child(
                                        ui::btn_icon(Icon::Search, "Query", Variant::Primary)
                                            .when(ui::phone(), |b| b.w_full())
                                            .disabled(move || busy.get() || !running.get() || name.with(|n| n.trim().is_empty()))
                                            .on_press(query),
                                    ),
                            )
                            .child(when(move || !running.get(), || ui::muted("Start the core to query its resolver."))),
                    )
                    .child(each(
                        move || history.get(),
                        |l: &Lookup| l.id,
                        move |l, _| result_card(l.get_untracked()),
                    ))
                    .child(
                        ui::group(
                            Some("Fake-IP"),
                            vec![
                                ui::setting_row(
                                    "Flush the fake-IP cache",
                                    Some("Forget the fake addresses handed out so far; apps get new ones on their next lookup."),
                                    ui::btn("Flush", Variant::Secondary).disabled(move || !running.get()).on_press(move || {
                                        store.run_then(store.svc().flush_fakeip(), move |r| {
                                            if r.is_ok() {
                                                store.info("Fake-IP cache flushed");
                                            }
                                        })
                                    }),
                                )
                                .into_element(),
                            ],
                        ),
                    ),
            ),
        )
}

fn result_card(l: Lookup) -> impl IntoElement {
    let ok = l.error.is_none() && l.status == 0;
    let summary = match &l.error {
        Some(e) => e.clone(),
        None => format!(
            "{} · {} {} · {} ms",
            status_name(l.status),
            l.answers.len(),
            if l.answers.len() == 1 {
                "answer"
            } else {
                "answers"
            },
            l.millis
        ),
    };
    let mut card = ui::card().child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2p5()
            .when(ui::phone(), |v| v.flex_wrap())
            .child(ui::dot(move || {
                if ok {
                    ui::success()
                } else {
                    theme().destructive
                }
            }))
            .child(
                ui::body(format!("{}  {}", l.name, l.kind))
                    .font_weight(FontWeight::SEMIBOLD)
                    .selectable(true),
            )
            .child(view().flex_1())
            .child(ui::muted(summary)),
    );
    if !l.answers.is_empty() {
        card = card.child(ui::divider()).child(
            view()
                .flex_row()
                .gap_3()
                .px_4()
                .py_1p5()
                .style_when(ui::phone, |s| s.hidden())
                .bg(move || ui::subtle())
                .child(col_head("Name", None))
                .child(col_head("Type", Some(64.0)))
                .child(col_head("TTL", Some(64.0)))
                .child(col_head("Data", None)),
        );
        for a in l.answers {
            if ui::phone() {
                // The record on two lines: what it is, then its data.
                card = card.child(
                    view()
                        .px_4()
                        .py_2()
                        .gap_0p5()
                        .child(
                            ui::muted(format!(
                                "{} · {} · {} s",
                                a.name.trim_end_matches('.'),
                                dns_type_name(a.kind),
                                a.ttl
                            ))
                            .number_of_lines(1),
                        )
                        .child(ui::body(a.data.clone()).selectable(true)),
                );
                continue;
            }
            card = card.child(
                view()
                    .flex_row()
                    .gap_3()
                    .px_4()
                    .py_1p5()
                    .child(
                        ui::body(a.name.trim_end_matches('.').to_owned())
                            .flex_1()
                            .min_w(px(0.0))
                            .number_of_lines(1)
                            .selectable(true),
                    )
                    .child(
                        ui::body(dns_type_name(a.kind))
                            .w(px(64.0))
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(
                        ui::body(format!("{} s", a.ttl))
                            .w(px(64.0))
                            .tabular_nums()
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(
                        ui::body(a.data.clone())
                            .flex_1()
                            .min_w(px(0.0))
                            .selectable(true),
                    ),
            );
        }
    }
    card
}

fn col_head(label: &'static str, w: Option<f32>) -> Text {
    let t = text(label)
        .text_size(px(size::small()))
        .font_weight(FontWeight::MEDIUM)
        .text_color(move || theme().muted_foreground);
    match w {
        Some(w) => t.w(px(w)),
        None => t.flex_1().min_w(px(0.0)),
    }
}
