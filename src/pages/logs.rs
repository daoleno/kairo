//! Logs: the core's log, live, newest first; level, search, pause and clear.

use std::sync::Arc;

use kairo_core::LogLevel;
use kairo_core::model::LogEntry;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::store::store;
use crate::ui::{self, Icon, Variant, size};

fn rank(level: &str) -> u8 {
    match level {
        "debug" => 0,
        "info" => 1,
        "warning" => 2,
        "error" => 3,
        _ => 1,
    }
}

pub fn page() -> impl IntoElement {
    let store = store();
    let query = signal(String::new());
    let paused = signal(None::<Arc<Vec<LogEntry>>>);
    let level = computed(move || store.state.with(|s| s.settings.log_level));
    let live = computed(move || {
        let _ = store.log_seq.get();
        let mut all = store.svc().logs();
        all.reverse();
        Arc::new(all)
    });
    let rows = computed(move || {
        let q = query.get().trim().to_lowercase();
        let min = rank(level.get().as_str());
        let src = paused.get().unwrap_or_else(|| live.get());
        let list: Vec<LogEntry> =
            src.iter().filter(|l| rank(&l.level) >= min && (q.is_empty() || l.payload.to_lowercase().contains(&q))).cloned().collect();
        Arc::new(list)
    });
    let set_level = move |l: LogLevel| {
        let mut next = store.svc().settings();
        next.log_level = l;
        store.run(store.svc().update_settings(next));
    };

    view()
        .flex_1()
        .child(ui::page_header(
            "Logs",
            vec![
                ui::search_field(query, "Search the log").w(px(220.0)).into_element(),
                ui::segmented(
                    vec![(LogLevel::Debug, "Debug"), (LogLevel::Info, "Info"), (LogLevel::Warning, "Warning"), (LogLevel::Error, "Error")],
                    move || level.get(),
                    set_level,
                )
                .into_element(),
                ui::btn_icon_dyn(
                    move || if paused.with(Option::is_some) { Icon::Play } else { Icon::Pause },
                    move || -> SharedString { if paused.with(Option::is_some) { "Resume".into() } else { "Pause".into() } },
                    Variant::Secondary,
                )
                .on_press(move || {
                    if paused.with_untracked(Option::is_some) {
                        paused.set(None);
                    } else {
                        paused.set(Some(live.get_untracked()));
                    }
                })
                .into_element(),
                ui::icon_btn(Icon::Trash, "Clear")
                    .on_press(move || {
                        store.svc().clear_logs();
                        paused.set(None);
                    })
                    .into_element(),
            ],
        ))
        .child(
            when(move || rows.with(|r| !r.is_empty()), move || {
                virtual_list(move || rows.with(|r| r.len()), move |at| {
                    let entry = computed(move || rows.with(|r| r.get(at.index()).cloned()));
                    let lvl = move || entry.with(|e| e.as_ref().map(|e| e.level.clone()).unwrap_or_default());
                    view()
                        .flex_row()
                        .items_start()
                        .gap_3()
                        .px_5()
                        .py(px(5.0))
                        .hover(|s| s.bg(move || theme().accent))
                        .child(
                            text(move || -> SharedString { entry.with(|e| e.as_ref().map(|e| e.time.clone()).unwrap_or_default()).into() })
                                .text_size(px(size::SMALL))
                                .tabular_nums()
                                .text_color(move || theme().muted_foreground)
                                .w(px(60.0))
                                .pt(px(1.0)),
                        )
                        .child(
                            text(move || -> SharedString {
                                match lvl().as_str() {
                                    "warning" => "WARN".into(),
                                    other => other.to_uppercase().into(),
                                }
                            })
                            .text_size(px(size::CAPTION))
                            .font_weight(FontWeight::SEMIBOLD)
                            .w(px(44.0))
                            .pt(px(2.0))
                            .text_color(move || match lvl().as_str() {
                                "error" => theme().destructive,
                                "warning" => ui::warning(),
                                "debug" => theme().muted_foreground,
                                _ => theme().primary,
                            }),
                        )
                        .child(
                            text(move || -> SharedString { entry.with(|e| e.as_ref().map(|e| e.payload.clone()).unwrap_or_default()).into() })
                                .text_size(px(size::BODY))
                                .flex_1()
                                .min_w(px(0.0))
                                .number_of_lines(3)
                                .selectable(true),
                        )
                })
                .keys(move || rows.with(|r| r.iter().map(|e| e.seq).collect::<Vec<_>>()))
                .estimated_item_size(px(26.0))
                .flex_1()
            })
            .otherwise(move || {
                ui::empty_state(
                    Icon::Logs,
                    "No log lines",
                    move || -> SharedString {
                        if !store.state.with(|s| s.core.is_running()) {
                            "The core's log shows here while it runs.".into()
                        } else if query.with(|q| !q.is_empty()) {
                            "Nothing matches the search.".into()
                        } else {
                            "Lines at the chosen level and above appear as the core writes them.".into()
                        }
                    },
                    None,
                )
            }),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .h(px(28.0))
                .px_5()
                .border_t_1()
                .border_color(move || theme().border)
                .child(ui::muted(move || -> SharedString {
                    let n = rows.with(|r| r.len());
                    if paused.with(Option::is_some) { format!("{n} lines · paused").into() } else { format!("{n} lines").into() }
                })),
        )
}
