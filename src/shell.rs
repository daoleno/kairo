//! The window: a sidebar of destinations and the selected page beside it, with
//! the core's state and speeds at the sidebar's foot and messages at the bottom.

use kairo_core::CoreStatus;
use nsh_base::{button, theme};
use nsh_core::GlassEffect;
use nsh_core::element::desktop::title_bar;
use nsh_core::prelude::*;

use crate::pages;
use crate::store::{Page, store};
use crate::ui::{self, Icon, icon, size};

const SIDEBAR_WIDTH: f32 = 212.0;

fn page_icon(page: Page) -> Icon {
    match page {
        Page::Overview => Icon::Overview,
        Page::Activity => Icon::Activity,
        Page::Apps => Icon::Layers,
        Page::Proxies => Icon::Proxies,
        Page::Profiles => Icon::Profiles,
        Page::Connections => Icon::Connections,
        Page::Rules => Icon::Rules,
        Page::Logs => Icon::Logs,
        Page::Dns => Icon::Dns,
        Page::Settings => Icon::Settings,
    }
}

fn nav_item(page: Page) -> impl IntoElement {
    let store = store();
    let selected = move || store.page.get() == page;
    let count = move || match page {
        Page::Connections => {
            let n = store.live.with(|l| l.connections);
            (n > 0).then(|| n.to_string())
        }
        Page::Profiles => {
            let n = store.state.with(|s| s.profiles.profiles.len());
            (n > 0).then(|| n.to_string())
        }
        _ => None,
    };
    button()
        .selected(selected)
        .role(AccessibilityRole::Tab)
        .accessibility_label(page.title())
        .on_press(move || store.page.set(page))
        .flex_row()
        .items_center()
        .gap_2p5()
        .h(px(30.0))
        .px_2p5()
        .rounded(ui::radius_control())
        .cursor_pointer()
        .text_color(move || theme().sidebar_foreground)
        .hover(|s| s.bg(move || theme().sidebar_accent.opacity(0.6)))
        .style_when(selected, |s| s.bg(move || theme().sidebar_accent))
        .child(icon(page_icon(page), 16.0).text_color(move || {
            if selected() {
                theme().sidebar_primary
            } else {
                theme().muted_foreground
            }
        }))
        .child(
            text(page.title())
                .text_size(px(size::body()))
                .flex_1()
                .font_weight(FontWeight::MEDIUM),
        )
        .child(
            text(move || count().unwrap_or_default())
                .text_size(px(size::small()))
                .tabular_nums()
                .text_color(move || theme().muted_foreground),
        )
}

fn core_footer() -> impl IntoElement {
    let store = store();
    let status = move || store.state.with(|s| s.core.clone());
    let tone = move || match status() {
        CoreStatus::Running { .. } => ui::success(),
        CoreStatus::Starting | CoreStatus::Stopping => ui::warning(),
        CoreStatus::Failed { .. } => theme().destructive,
        _ => theme().muted_foreground,
    };
    view()
        .gap_2()
        .p_3()
        .mx_2()
        .mb_2()
        .rounded(move || theme().radius)
        .bg(move || theme().sidebar_accent.opacity(0.45))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(ui::dot(tone))
                .child(
                    text(move || store.state.with(|s| s.plain()))
                        .text_size(px(size::small()))
                        .font_weight(FontWeight::MEDIUM)
                        .flex_1(),
                )
                .child(
                    text(move || store.state.with(|s| s.settings.mode.route()))
                        .text_size(px(size::small()))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(
            view()
                .flex_row()
                .gap_3()
                .child(speed(Icon::ArrowUp, move || store.live.with(|l| l.up)))
                .child(speed(Icon::ArrowDown, move || store.live.with(|l| l.down))),
        )
        // A downloaded Kairo update: one tap installs it and restarts.
        .child(when(
            move || {
                store
                    .state
                    .with(|s| matches!(s.update, kairo_core::UpdateStatus::Ready { .. }))
            },
            pages::settings::update_button,
        ))
}

fn speed(glyph: Icon, value: impl Fn() -> u64 + 'static) -> impl IntoElement {
    view()
        .flex_1()
        .flex_row()
        .items_center()
        .gap_1()
        .child(icon(glyph, 12.0).text_color(move || theme().muted_foreground))
        .child(
            text(move || kairo_core::speed(value()))
                .text_size(px(size::small()))
                .tabular_nums()
                .number_of_lines(1),
        )
}

fn sidebar() -> impl IntoElement {
    let groups: [&[Page]; 3] = [
        &[Page::Overview, Page::Activity, Page::Apps],
        &[
            Page::Proxies,
            Page::Profiles,
            Page::Connections,
            Page::Rules,
            Page::Logs,
        ],
        &[Page::Settings],
    ];
    let glass = crate::chrome::glass_sidebar();
    view()
        .w(px(SIDEBAR_WIDTH))
        .flex_shrink_0()
        .style_when(
            move || !glass,
            |s| {
                s.bg(move || theme().sidebar)
                    .border_r_1()
                    .border_color(move || theme().sidebar_border)
            },
        )
        .style_when(
            move || glass,
            |s| s.m_2().rounded(px(18.0)).glass_effect(GlassEffect::Regular),
        )
        .window_drag(true)
        // Room for macOS's traffic lights when the title bar is hidden.
        .pt(move || {
            if title_bar().top > px(0.0) {
                title_bar().top + px(6.0)
            } else {
                px(12.0)
            }
        })
        .child(
            view()
                .flex_1()
                .px_2()
                .gap_4()
                .children(groups.into_iter().map(|pages| {
                    view()
                        .gap_0p5()
                        .children(pages.iter().map(|p| nav_item(*p)))
                })),
        )
        .child(core_footer())
}

fn toast() -> impl IntoElement {
    let store = store();
    when(
        move || store.toast.with(|t| t.is_some()),
        move || {
            let message = move || {
                store
                    .toast
                    .with(|t| t.as_ref().map(|t| t.1.clone()).unwrap_or_default())
            };
            let error = move || store.toast.with(|t| t.as_ref().is_some_and(|t| t.2));
            view()
                .absolute()
                .bottom_5()
                .left_0()
                .right_0()
                .items_center()
                .pointer_events(PointerEvents::BoxNone)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .max_w(px(560.0))
                        .px_4()
                        .py_2p5()
                        .rounded(move || theme().radius)
                        .bg(move || theme().popover)
                        .border_1()
                        .border_color(move || theme().border)
                        .shadow_lg()
                        .child(ui::dot(move || {
                            if error() {
                                theme().destructive
                            } else {
                                theme().primary
                            }
                        }))
                        .child(text(message).text_size(px(size::body())).flex_shrink(1.0))
                        .child(
                            ui::icon_btn(Icon::Close, "Dismiss")
                                .size(px(20.0))
                                .on_press(move || store.toast.set(None)),
                        ),
                )
        },
    )
}

pub fn shell() -> impl IntoElement {
    let store = store();
    view()
        .flex_1()
        .flex_row()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(when(|| !crate::chrome::native(), sidebar))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                // Clear of a sidebar floating over the content (macOS).
                .pl(move || px(store.insets.get().left))
                .child(dynamic(move || {
                    let page = store.page.get();
                    untracked(|| match page {
                        Page::Overview => pages::home::page().into_element(),
                        Page::Activity => pages::activity::page().into_element(),
                        Page::Apps => pages::apps::page().into_element(),
                        Page::Proxies => pages::proxies::page().into_element(),
                        Page::Profiles => pages::profiles::page().into_element(),
                        Page::Connections => pages::connections::page().into_element(),
                        Page::Rules => pages::rules::page().into_element(),
                        Page::Logs => pages::logs::page().into_element(),
                        Page::Dns => pages::dns::page().into_element(),
                        Page::Settings => pages::settings::page().into_element(),
                    })
                })),
        )
        .child(toast())
        .child(crate::palette::palette())
}
