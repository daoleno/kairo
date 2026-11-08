//! Rules: the running configuration's rules in order (searchable), and its
//! rule providers.

use std::sync::Arc;

use kairo_core::model::{Rule, RuleProvider};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::store::store;
use crate::ui::{self, Icon, Variant, size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Rules,
    Providers,
}

pub fn page() -> impl IntoElement {
    let store = store();
    let query = signal(String::new());
    let tab = signal(Tab::Rules);
    let rules = signal(Arc::new(Vec::<Rule>::new()));
    let providers = signal(Arc::new(Vec::<RuleProvider>::new()));
    let loading = signal(false);
    let running = computed(move || store.state.with(|s| s.core.is_running()));
    // The rules change with the running configuration: reload on a (re)start,
    // a profile switch or an update of the active profile.
    let generation = computed(move || {
        store.state.with(|s| {
            let active = s.profiles.active_profile().map(|p| (p.id.clone(), p.updated_at));
            (s.core.is_running(), active)
        })
    });
    let load = move || {
        if !running.get_untracked() {
            rules.set(Arc::new(Vec::new()));
            providers.set(Arc::new(Vec::new()));
            return;
        }
        loading.set(true);
        let svc = store.svc();
        store.run_then(svc.rules(), move |r| {
            loading.set(false);
            if let Ok(r) = r {
                rules.set(Arc::new(r));
            }
        });
        store.run_then(store.svc().rule_providers(), move |r| {
            if let Ok(p) = r {
                providers.set(Arc::new(p));
            }
        });
    };
    effect(move || {
        let _ = generation.get();
        untracked(load);
    });

    let shown = computed(move || {
        let q = query.get().trim().to_lowercase();
        let all = rules.get();
        let list: Vec<(usize, Rule)> = all
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                q.is_empty()
                    || r.payload.to_lowercase().contains(&q)
                    || r.kind.to_lowercase().contains(&q)
                    || r.proxy.to_lowercase().contains(&q)
            })
            .map(|(i, r)| (i, r.clone()))
            .collect();
        Arc::new(list)
    });

    view()
        .flex_1()
        .child(ui::page_header(
            move || -> SharedString {
                let (n, all) = (shown.with(|s| s.len()), rules.with(|r| r.len()));
                if n == all { format!("Rules  {all}").into() } else { format!("Rules  {n} of {all}").into() }
            },
            vec![
                ui::search_field(query, "Type, payload or target").w(px(240.0)).into_element(),
                ui::icon_btn(Icon::Refresh, "Reload Rules").disabled(move || !running.get() || loading.get()).on_press(load).into_element(),
            ],
        ))
        .child(when(
            move || providers.with(|p| !p.is_empty()),
            move || {
                view()
                    .flex_row()
                    .px_5()
                    .py_2()
                    .border_b_1()
                    .border_color(move || theme().border)
                    .child(ui::segmented(
                        vec![(Tab::Rules, "Rules"), (Tab::Providers, "Providers")],
                        move || tab.get(),
                        move |t| tab.set(t),
                    ))
            },
        ))
        .child(
            when(move || running.get(), move || {
                dynamic(move || {
                    let t = if providers.with(|p| p.is_empty()) { Tab::Rules } else { tab.get() };
                    untracked(move || match t {
                        Tab::Rules => rule_list(shown, query).into_element(),
                        Tab::Providers => provider_list(providers, query, load).into_element(),
                    })
                })
            })
            .otherwise(|| ui::empty_state(Icon::Rules, "The core is not running", "The running configuration's rules show here.", None)),
        )
}

const W_INDEX: f32 = 52.0;
const W_KIND: f32 = 150.0;
const W_TARGET: f32 = 200.0;

fn rule_list(shown: Memo<Arc<Vec<(usize, Rule)>>>, query: Signal<String>) -> impl IntoElement {
    view()
        .flex_1()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .h(px(28.0))
                .px_5()
                .border_b_1()
                .border_color(move || theme().border)
                .bg(move || ui::subtle())
                .child(head("#", Some(W_INDEX)))
                .child(head("Type", Some(W_KIND)))
                .child(head("Payload", None))
                .child(head("Target", Some(W_TARGET))),
        )
        .child(
            when(move || shown.with(|s| !s.is_empty()), move || {
                virtual_list(move || shown.with(|s| s.len()), move |at| {
                    let row = computed(move || shown.with(|s| s.get(at.index()).cloned()));
                    let f = move |g: fn(&(usize, Rule)) -> String| move || -> SharedString { row.with(|r| r.as_ref().map(g).unwrap_or_default()).into() };
                    view()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .h(px(ui::ROW_HEIGHT))
                        .px_5()
                        .hover(|s| s.bg(move || theme().accent))
                        .child(cell(f(|r| (r.0 + 1).to_string()), Some(W_INDEX)).tabular_nums().text_color(move || theme().muted_foreground))
                        .child(cell(f(|r| r.1.kind.clone()), Some(W_KIND)).text_color(move || theme().muted_foreground))
                        .child(cell(f(|r| if r.1.payload.is_empty() { "—".into() } else { r.1.payload.clone() }), None).selectable(true))
                        .child(cell(f(|r| r.1.proxy.clone()), Some(W_TARGET)).text_color(move || {
                            let target = row.with(|r| r.as_ref().map(|r| r.1.proxy.clone()).unwrap_or_default());
                            match target.as_str() {
                                "REJECT" | "REJECT-DROP" => theme().destructive,
                                "DIRECT" => theme().muted_foreground,
                                _ => theme().primary,
                            }
                        }))
                })
                .keys(move || shown.with(|s| s.iter().map(|r| r.0).collect::<Vec<_>>()))
                .estimated_item_size(px(ui::ROW_HEIGHT))
                .flex_1()
            })
            .otherwise(move || {
                ui::empty_state(
                    Icon::Rules,
                    "No rules",
                    move || -> SharedString {
                        if query.with(|q| q.is_empty()) { "This configuration has no rules.".into() } else { "Nothing matches the filter.".into() }
                    },
                    None,
                )
            }),
        )
}

fn head(label: &'static str, w: Option<f32>) -> Text {
    let t = text(label).text_size(px(size::SMALL)).font_weight(FontWeight::MEDIUM).text_color(move || theme().muted_foreground);
    match w {
        Some(w) => t.w(px(w)),
        None => t.flex_1().min_w(px(0.0)),
    }
}

fn cell(content: impl IntoReactive<SharedString>, w: Option<f32>) -> Text {
    let t = text(content).text_size(px(size::BODY)).number_of_lines(1);
    match w {
        Some(w) => t.w(px(w)),
        None => t.flex_1().min_w(px(0.0)),
    }
}

fn provider_list(providers: Signal<Arc<Vec<RuleProvider>>>, query: Signal<String>, reload: impl Fn() + Copy + 'static) -> impl IntoElement {
    let store = store();
    let updating = signal(std::collections::HashSet::<String>::new());
    let shown = computed(move || {
        let q = query.get().trim().to_lowercase();
        providers.with(|p| p.iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q)).cloned().collect::<Vec<_>>())
    });
    let update = move |name: String| {
        updating.update(|u| {
            u.insert(name.clone());
        });
        let n = name.clone();
        store.run_then(store.svc().update_rule_provider(name), move |_| {
            updating.update(|u| {
                u.remove(&n);
            });
            reload();
        });
    };
    scroll().flex_1().child(
        view()
            .p_5()
            .gap_3()
            .max_w(px(900.0))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .child(ui::muted("Rule sets the rules refer to, kept up to date by the core.").flex_1())
                    .child(ui::btn_icon(Icon::Refresh, "Update All", Variant::Secondary).on_press(move || {
                        for p in providers.get_untracked().iter() {
                            update(p.name.clone());
                        }
                    })),
            )
            .child(ui::card().child(each(
                move || shown.get(),
                |p: &RuleProvider| p.name.clone(),
                move |p, i| {
                    let name = p.with_untracked(|p| p.name.clone());
                    let busy = {
                        let name = name.clone();
                        let m = computed(move || updating.with(|u| u.contains(&name)));
                        move || m.get()
                    };
                    view()
                        .child((i.get_untracked() > 0).then(|| ui::divider().ml_4()))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .px_4()
                                .py_2p5()
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .gap_0p5()
                                        .child(ui::body(name.clone()).font_weight(FontWeight::MEDIUM))
                                        .child(ui::muted(move || -> SharedString {
                                            p.with(|p| {
                                                let mut parts = vec![p.behavior.clone(), p.format.clone(), p.vehicle.clone()];
                                                parts.retain(|s| !s.is_empty());
                                                parts.push(format!("{} rules", p.rule_count));
                                                if let Some(at) = p.updated_at.as_deref() {
                                                    parts.push(format!("updated {}", at.replace('T', " ").split('.').next().unwrap_or("")));
                                                }
                                                parts.join(" · ")
                                            })
                                            .into()
                                        })),
                                )
                                .child(ui::icon_btn(Icon::Refresh, "Update").disabled(busy).on_press(move || update(name.clone()))),
                        )
                },
            ))),
    )
}
