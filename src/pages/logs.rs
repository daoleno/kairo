//! Logs: the core's log, live, newest first; level, search, pause and clear.

use std::sync::Arc;

use kairo_core::LogLevel;
use kairo_core::model::LogEntry;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::chrome::Glyph;
use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, size};

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
        .child(toolbar::toolbar(
            Page::Logs,
            move || {
                let n = rows.with(|r| r.len());
                let lines = if n == 1 { "1 line".to_owned() } else { format!("{n} lines") };
                if paused.with(Option::is_some) { format!("{lines} · paused") } else { lines }
            },
            vec![
                toolbar::search("logs.search", "Search the log", query),
                toolbar::choice(
                    "logs.level",
                    "Level",
                    Glyph::Filter,
                    vec!["Debug", "Info", "Warning", "Error"],
                    move || match level.get() {
                        LogLevel::Debug => 0,
                        LogLevel::Info => 1,
                        LogLevel::Warning => 2,
                        _ => 3,
                    },
                    move |i| set_level([LogLevel::Debug, LogLevel::Info, LogLevel::Warning, LogLevel::Error][i.min(3)]),
                ),
                toolbar::button_dyn(
                    "logs.pause",
                    move || if paused.with(Option::is_some) { "Resume" } else { "Pause" },
                    move || if paused.with(Option::is_some) { (Glyph::Resume, Icon::Play) } else { (Glyph::Pause, Icon::Pause) },
                    move || {
                        if paused.with_untracked(Option::is_some) {
                            paused.set(None);
                        } else {
                            paused.set(Some(live.get_untracked()));
                        }
                    },
                ),
                toolbar::button("logs.clear", "Clear", Glyph::Clear, Icon::Trash, move || {
                    store.svc().clear_logs();
                    paused.set(None);
                }),
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
                                .text_size(px(size::small()))
                                .tabular_nums()
                                .text_color(move || theme().muted_foreground)
                                .w(px(60.0 * size::scale()))
                                .pt(px(1.0)),
                        )
                        .child(
                            text(move || -> SharedString {
                                match lvl().as_str() {
                                    "warning" => "WARN".into(),
                                    other => other.to_uppercase().into(),
                                }
                            })
                            .text_size(px(size::caption()))
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
                                .text_size(px(size::body()))
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
}
