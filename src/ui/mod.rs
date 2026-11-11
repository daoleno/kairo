//! Kairo's styled component layer over `nsh_base`: tokens, the type scale and
//! the controls every page composes. Pages never restyle base components.

pub mod charts;
pub mod icons;
pub mod menu;
pub mod toolbar;

use nsh_base::{ButtonElement as Button, Input, button, input, switch, theme};

use nsh_core::JsonValue;
use nsh_core::prelude::*;

pub use icons::{Icon, icon};

// --- Tokens ------------------------------------------------------------------------

fn token(name: &str, fallback: AdaptiveColor) -> AdaptiveColor {
    theme().color(name).unwrap_or(fallback)
}

pub fn success() -> AdaptiveColor {
    token("success", theme().primary)
}
pub fn warning() -> AdaptiveColor {
    token("warning", theme().destructive)
}
pub fn track() -> AdaptiveColor {
    token("track", theme().muted)
}
pub fn selection() -> AdaptiveColor {
    token("selection", theme().accent)
}
pub fn graph_up() -> AdaptiveColor {
    token("graph-up", theme().muted_foreground)
}
pub fn subtle() -> AdaptiveColor {
    token("subtle", theme().muted)
}

// Chart series: Ember only for AI (the focal series), the others stay quiet.
pub fn chart_ai() -> AdaptiveColor {
    token("chart-ai", theme().primary)
}
pub fn chart_rest() -> AdaptiveColor {
    token("chart-rest", theme().muted_foreground)
}
pub fn chart_downloads() -> AdaptiveColor {
    token("chart-downloads", theme().muted_foreground)
}
pub fn chart_direct() -> AdaptiveColor {
    token("chart-direct", theme().muted)
}
pub fn chart_neutral() -> AdaptiveColor {
    token("chart-neutral", theme().muted_foreground)
}
pub fn chart_grid() -> AdaptiveColor {
    token("chart-grid", theme().border)
}
pub fn chart_bad() -> AdaptiveColor {
    token("chart-bad", theme().destructive)
}

pub fn radius_control() -> Points {
    px(theme().number("radius-control").unwrap_or(6.0))
}
pub fn radius_small() -> Points {
    px(theme().number("radius-small").unwrap_or(4.0))
}

/// The type scale (points). Body text is 13, the native size of desktop UI.
pub mod size {
    use std::sync::atomic::{AtomicU32, Ordering};

    static SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0

    /// The desktop's text size relative to these (macOS: 1; Linux: its UI
    /// font's), set by the launcher before the root loads.
    pub fn set_scale(scale: f32) {
        SCALE.store(scale.clamp(0.75, 3.0).to_bits(), Ordering::Relaxed);
    }

    pub fn scale() -> f32 {
        f32::from_bits(SCALE.load(Ordering::Relaxed))
    }

    fn at(points: f32) -> f32 {
        (points * scale()).round()
    }

    pub fn caption() -> f32 {
        at(11.0)
    }
    pub fn small() -> f32 {
        at(12.0)
    }
    pub fn body() -> f32 {
        at(13.0)
    }
    pub fn large() -> f32 {
        at(15.0)
    }
    pub fn title() -> f32 {
        at(20.0)
    }
    pub fn display() -> f32 {
        at(26.0)
    }
}

/// The height of a toolbar / page header.
pub const HEADER_HEIGHT: f32 = 52.0;

/// Whether this is a phone (Android): one column, tabs, no hover.
pub fn phone() -> bool {
    use nsh_core::platform::{Idiom, Os};
    try_os() == Some(Os::Android) && idiom() == Idiom::Phone
}
/// The height of a control a thumb presses (Android's guideline is 48 dp for
/// the target, 40 for the drawn control); `desktop` where a pointer is exact.
pub fn touch_h(desktop: f32) -> f32 {
    if phone() { desktop.max(40.0) } else { desktop }
}
/// The least height of a row in a group (System Settings' and libadwaita's
/// rows are about this tall).
pub const ROW_MIN: f32 = 48.0;
/// The height of a dense table row.
pub const ROW_HEIGHT: f32 = 30.0;

// --- Text --------------------------------------------------------------------------

/// Body text.
pub fn body(content: impl IntoReactive<SharedString>) -> Text {
    text(content).text_size(px(size::body()))
}

/// Secondary text in the muted color.
pub fn muted(content: impl IntoReactive<SharedString>) -> Text {
    text(content)
        .text_size(px(size::small()))
        .text_color(move || theme().muted_foreground)
}

/// A small section label above a group.
pub fn section_label(content: impl IntoReactive<SharedString>) -> Text {
    text(content)
        .text_size(px(size::small()))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(move || theme().muted_foreground)
        .role(AccessibilityRole::Header)
}

/// A page title.
pub fn title(content: impl IntoReactive<SharedString>) -> Text {
    text(content)
        .text_size(px(size::title()))
        .font_weight(FontWeight::SEMIBOLD)
        .role(AccessibilityRole::Header)
}

/// Numbers that line up.
pub fn figure(content: impl IntoReactive<SharedString>, pts: f32) -> Text {
    text(content).text_size(px(pts)).tabular_nums()
}

// --- Buttons -----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Ghost,
    Destructive,
}

fn base_button(variant: Variant) -> Button {
    let b = button()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_1p5()
        .px_3()
        .rounded(radius_control())
        .cursor_pointer()
        .focus(|s| s.outline(px(2.0)).outline_color(move || theme().ring))
        .disabled_style(|s| s.opacity(0.45));
    // A thumb's height, growing with the text size; a pointer's, fixed.
    let b = if phone() {
        b.min_h(px(touch_h(28.0))).px_4()
    } else {
        b.h(px(28.0))
    };
    match variant {
        Variant::Primary => b
            .bg(move || theme().primary)
            .text_color(move || theme().primary_foreground)
            .hover(|s| s.opacity(0.92))
            .active(|s| s.opacity(0.8)),
        Variant::Secondary => b
            .bg(move || theme().secondary)
            .border_1()
            .border_color(move || theme().border)
            .text_color(move || theme().secondary_foreground)
            .hover(|s| s.bg(move || theme().accent))
            .active(|s| s.opacity(0.75)),
        Variant::Ghost => b
            .text_color(move || theme().foreground)
            .hover(|s| s.bg(move || theme().accent))
            .active(|s| s.opacity(0.7)),
        Variant::Destructive => b
            .bg(move || theme().secondary)
            .border_1()
            .border_color(move || theme().border)
            .text_color(move || theme().destructive)
            .hover(|s| s.bg(move || theme().accent))
            .active(|s| s.opacity(0.75)),
    }
}

/// A text button.
pub fn btn(label: impl IntoReactive<SharedString>, variant: Variant) -> Button {
    base_button(variant).child(
        text(label)
            .text_size(px(size::body()))
            .font_weight(FontWeight::MEDIUM),
    )
}

/// A button with an icon before its label.
pub fn btn_icon(glyph: Icon, label: impl IntoReactive<SharedString>, variant: Variant) -> Button {
    base_button(variant).child(icon(glyph, 14.0)).child(
        text(label)
            .text_size(px(size::body()))
            .font_weight(FontWeight::MEDIUM),
    )
}

/// A button whose icon and label follow state.
pub fn btn_icon_dyn(
    glyph: impl Fn() -> Icon + 'static,
    label: impl IntoReactive<SharedString>,
    variant: Variant,
) -> Button {
    base_button(variant)
        .child(
            svg(move || icons::uri(glyph()))
                .size(px(14.0))
                .flex_shrink_0(),
        )
        .child(
            text(label)
                .text_size(px(size::body()))
                .font_weight(FontWeight::MEDIUM),
        )
}

/// An icon-only button; `label` names it for screen readers and tests.
pub fn icon_btn(glyph: Icon, label: &'static str) -> Button {
    button()
        .accessibility_label(label)
        .items_center()
        .justify_center()
        .size(px(touch_h(28.0)))
        .rounded(radius_control())
        .cursor_pointer()
        .text_color(move || theme().muted_foreground)
        .hover(|s| {
            s.bg(move || theme().accent)
                .text_color(move || theme().foreground)
        })
        .active(|s| s.opacity(0.7))
        .focus(|s| s.outline(px(2.0)).outline_color(move || theme().ring))
        .disabled_style(|s| s.opacity(0.4))
        .child(icon(glyph, 16.0))
}

/// The update at the foot of the window: one round icon button, no words (they are the tooltip
/// and the accessibility label). A ring around the arrow fills with the download; ready is the
/// arrow on a disc of the accent.
pub fn update_button(b: crate::chrome::UpdateButton, on_press: impl Fn() + 'static) -> View {
    let ready = b.stage == crate::chrome::UpdateStage::Ready;
    let hint = view()
        .px_2()
        .py_1()
        .rounded(radius_small())
        .bg(move || theme().popover)
        .border_1()
        .border_color(move || theme().border)
        .child(
            text(b.label.clone())
                .text_size(px(size::small()))
                .text_color(move || theme().popover_foreground)
                .number_of_lines(1),
        )
        .into_element();
    // The hint goes on a view around the button (a button takes none itself).
    view().flex_shrink_0().tooltip(hint).child(
        button()
            .accessibility_label(b.label.clone())
            .disabled(!b.enabled)
            .items_center()
            .justify_center()
            .size(px(touch_h(32.0)))
            .flex_shrink_0()
            .rounded_full()
            .cursor_pointer()
            .bg(move || {
                if ready {
                    theme().primary.into()
                } else {
                    theme().background.opacity(0.0)
                }
            })
            .text_color(move || {
                if ready {
                    theme().primary_foreground
                } else {
                    theme().primary
                }
            })
            .hover(|s| s.opacity(0.85))
            .active(|s| s.opacity(0.7))
            .focus(|s| s.outline(px(2.0)).outline_color(move || theme().ring))
            .on_press(on_press)
            .child(svg(crate::chrome::update_icon_svg(&b)).size(px(22.0))),
    )
}

// --- Controls ----------------------------------------------------------------------

/// An on/off switch in the platform's proportions.
pub fn toggle(
    checked: impl Fn() -> bool + 'static,
    label: &'static str,
    on_change: impl Fn(bool) + 'static,
) -> AnyElement {
    if crate::chrome::native_controls() {
        // NSSwitch has no title: its label is the row's text beside it.
        let _ = label;
        return native("kairo/switch")
            .prop("on", move || JsonValue::Bool(checked()))
            .w(px(38.0))
            .h(px(22.0))
            .flex_shrink_0()
            .on_native_event(move |e: &NativeEvent| {
                if &*e.name == "change" {
                    on_change(e.data["on"].as_bool().unwrap_or(false));
                }
            })
            .into_element();
    }
    switch()
        .checked(checked)
        .accessibility_label(label)
        .on_change(move |v: &bool| on_change(*v))
        .cursor_pointer()
        .track_style(|s| {
            s.w(px(34.0))
                .h(px(20.0))
                .p(px(2.0))
                .rounded_full()
                .bg(move || track())
                .transition_colors()
                .checked(|s| s.bg(move || theme().primary))
        })
        .thumb_style(|s| {
            s.size(px(16.0))
                .rounded_full()
                .bg(move || AdaptiveColor::from(white()))
                .shadow_sm()
                .transition_transform()
                .checked(|s| s.translate_x(px(14.0)))
        })
        .into_element()
}

/// A segmented control: one of `options`, the selected one raised.
pub fn segmented<T: Copy + PartialEq + 'static>(
    options: Vec<(T, &'static str)>,
    selected: impl Fn() -> T + Copy + 'static,
    on_select: impl Fn(T) + Copy + 'static,
) -> View {
    if crate::chrome::native_controls() {
        let values: Vec<T> = options.iter().map(|o| o.0).collect();
        let labels: Vec<JsonValue> = options.iter().map(|o| JsonValue::from(o.1)).collect();
        let width = options
            .iter()
            .map(|o| o.1.len() as f32 * 7.5 + 26.0)
            .sum::<f32>()
            + 4.0;
        let index = {
            let values = values.clone();
            move || {
                JsonValue::from(
                    values
                        .iter()
                        .position(|v| *v == selected())
                        .map_or(-1, |i| i as i64),
                )
            }
        };
        return view().flex_shrink_0().child(
            native("kairo/segmented")
                .prop("labels", JsonValue::Array(labels))
                .prop("selected", index)
                .w(px(width))
                .h(px(24.0))
                .on_native_event(move |e: &NativeEvent| {
                    if let Some(v) = e.data["index"]
                        .as_u64()
                        .and_then(|i| values.get(i as usize))
                    {
                        on_select(*v);
                    }
                }),
        );
    }
    // On a phone the track never outgrows its parent: with the largest system font the
    // segments give way (their text ends in an ellipsis) instead of running off the screen.
    let phone = phone();
    let track_view = view().flex_row().p(px(3.0)).gap(px(2.0)).rounded_full().bg(move || track());
    let track_view = if phone { track_view.w_full().min_w(px(0.0)) } else { track_view.flex_shrink_0() };
    track_view
        .children(options.into_iter().map(move |(value, label)| {
            let is = move || selected() == value;
            button()
                .selected(is)
                .role(AccessibilityRole::Tab)
                .on_press(move || on_select(value))
                .flex_grow(1.0)
                .flex_shrink(if phone { 1.0 } else { 0.0 })
                .min_w(px(if phone { 0.0 } else { 56.0 }))
                .items_center()
                .justify_center()
                .min_h(px(touch_h(26.0)))
                .px(px(if phone { 6.0 } else { 14.0 }))
                .rounded_full()
                .cursor_pointer()
                .text_color(move || theme().muted_foreground)
                .hover(|s| s.text_color(move || theme().foreground))
                .style_when(is, |s| {
                    s.bg(move || theme().background)
                        .shadow_sm()
                        .text_color(move || theme().foreground)
                })
                .child(
                    text(label)
                        .text_size(px(if phone { size::small() } else { size::body() }))
                        .font_weight(FontWeight::MEDIUM)
                        .number_of_lines(1),
                )
        }))
}

/// A single-line text field bound to `value`.
pub fn field(value: Signal<String>, placeholder: &'static str) -> Input {
    input()
        .value(move || value.get())
        .on_change(move |t: &str| value.set(t.to_owned()))
        .placeholder(placeholder)
        .placeholder_color(|| color(theme().muted_foreground))
        .caret_color(|| color(theme().primary))
        .h(px(28.0))
        .px_2()
        .text_size(px(size::body()))
        .rounded(radius_control())
        .border_1()
        .border_color(move || theme().input)
        .bg(move || theme().background)
        .focus(|s| s.border_color(move || theme().primary))
}

/// A search field with its glyph.
pub fn search_field(value: Signal<String>, placeholder: &'static str) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .min_h(px(touch_h(28.0)))
        .px_2()
        .rounded(radius_control())
        .border_1()
        .border_color(move || theme().input)
        .bg(move || theme().background)
        .focus_within(|s| s.border_color(move || theme().primary))
        .child(icon(Icon::Search, 14.0).text_color(move || theme().muted_foreground))
        .child(
            input()
                .value(move || value.get())
                .on_change(move |t: &str| value.set(t.to_owned()))
                .placeholder(placeholder)
                .placeholder_color(|| color(theme().muted_foreground))
                .caret_color(|| color(theme().primary))
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(size::body())),
        )
        .child(when(
            move || value.with(|v| !v.is_empty()),
            move || {
                button()
                    .accessibility_label("Clear")
                    .on_press(move || value.set(String::new()))
                    .cursor_pointer()
                    .text_color(move || theme().muted_foreground)
                    .child(icon(Icon::Close, 12.0))
            },
        ))
}

// --- Surfaces ----------------------------------------------------------------------

/// A grouped surface with a hairline border.
pub fn card() -> View {
    view()
        .rounded(move || theme().radius)
        .border_1()
        .border_color(move || theme().border)
        .bg(move || theme().card)
        .overflow_hidden()
}

/// A soft, borderless rounded surface (Home's cards and tables).
pub fn card_soft() -> View {
    view()
        .rounded(px(20.0))
        .bg(move || theme().muted)
        .overflow_hidden()
}

/// The picture of a flag (an emoji of two regional indicators), as a data URI cropped to the
/// flag's own 36:26 box. Windows has no flag emoji (it draws "JP"), and the emoji fonts of the
/// other platforms differ and clip, so every platform draws the same bundled pictures.
fn flag_uri(flag: &str) -> Option<String> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static URIS: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
    }
    URIS.with(|cache| {
        cache
            .borrow_mut()
            .entry(flag.to_owned())
            .or_insert_with(|| {
                let code = kairo_core::places::flag_code(flag)?.to_ascii_lowercase();
                let svg = crate::flags::svg(&code)?;
                let svg = svg
                    .replace("viewBox=\"0 0 36 36\"", "viewBox='0 5 36 26'")
                    .replace('%', "%25")
                    .replace('#', "%23")
                    .replace('"', "'");
                Some(format!("data:image/svg+xml,{svg}"))
            })
            .clone()
    })
}

/// A flag: the picture of its country's flag; a neutral mark's text when there is none.
pub fn flag(flag: impl Into<String>, size: f32) -> AnyElement {
    let flag = flag.into();
    match flag_uri(&flag) {
        // A hairline keeps the white-fielded flags (Japan, Singapore) from melting into a light card.
        Some(uri) => view()
            .rounded(px(size * 0.16))
            .border_1()
            .border_color(move || theme().border)
            .overflow_hidden()
            .flex_shrink_0()
            .child(svg(uri).w(px(size * 1.35)).h(px(size * 0.975)))
            .into_element(),
        None => text(flag).text_size(px(size)).into_element(),
    }
}

/// A label that may start with a flag emoji ("🇯🇵 Tokyo 01"): the flag as its picture, then the text.
pub fn flagged(label: impl Fn() -> SharedString + 'static, size: f32) -> View {
    flagged_with(label, size, false)
}

/// A line of small muted text that may carry a flag anywhere in it ("Select · 🇺🇸 Los Angeles 01").
pub fn flagged_muted(label: impl Fn() -> SharedString + 'static) -> View {
    flagged_with(label, size::small(), true)
}

fn flagged_with(label: impl Fn() -> SharedString + 'static, size: f32, muted: bool) -> View {
    let words = move |s: String| {
        let t = text(s).text_size(px(size)).number_of_lines(1);
        if muted {
            t.text_color(move || theme().muted_foreground)
        } else {
            t
        }
    };
    view()
        .flex_row()
        .items_center()
        .gap_1p5()
        .min_w(px(0.0))
        .child(dynamic(move || {
            let full = label();
            let (before, flag_emoji, after) = kairo_core::places::find_flag(&full);
            let (before, after) = (before.trim().to_owned(), after.trim().to_owned());
            // A flag with no picture stays what it was: text.
            let (flag_emoji, before, after) = match flag_emoji {
                Some(f) if flag_uri(&f).is_none() => (None, full.trim().to_owned(), String::new()),
                other => (other, before, after),
            };
            untracked(move || {
                view()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .min_w(px(0.0))
                    .children((!before.is_empty()).then(|| words(before)))
                    .child(flag_emoji.map(|f| flag(f, size + 1.0)))
                    .children((!after.is_empty()).then(|| words(after)))
                    .into_element()
            })
        }))
}

/// A menu's text: Windows' menus cannot draw a flag picture and would show its two letters.
pub fn menu_text(title: String) -> String {
    if cfg!(windows) {
        kairo_core::places::split_flag(&title).1.trim().to_owned()
    } else {
        title
    }
}

/// A hairline separator.
pub fn divider() -> View {
    view().h(px(1.0)).bg(move || theme().border)
}

/// A row of a settings group: title and description on the left, a control on the right.
pub fn setting_row(
    label: impl IntoReactive<SharedString>,
    detail: Option<&'static str>,
    control: impl IntoElement,
) -> View {
    let phone = phone();
    // On a phone a wide control wraps under its label instead of squeezing the label.
    let row = view().flex_row().items_center().gap_4().px_4().py_2p5().min_h(px(ROW_MIN));
    let row = if phone { row.flex_wrap().gap_y_2() } else { row };
    row
        .child(
            view()
                .flex_1()
                .min_w(px(if phone { 150.0 } else { 0.0 }))
                .gap_0p5()
                .child(body(label))
                .child(detail.map(|d| muted(d))),
        )
        .child(control)
}

/// A row whose description follows state.
pub fn setting_row_dyn(
    label: &'static str,
    detail: impl Fn() -> SharedString + 'static,
    control: impl IntoElement,
) -> View {
    let phone = phone();
    // On a phone a wide control wraps under its label instead of squeezing the label.
    let row = view().flex_row().items_center().gap_4().px_4().py_2p5().min_h(px(ROW_MIN));
    let row = if phone { row.flex_wrap().gap_y_2() } else { row };
    row
        .child(
            view()
                .flex_1()
                .min_w(px(if phone { 150.0 } else { 0.0 }))
                .gap_0p5()
                .child(body(label))
                .child({
                    let detail = std::rc::Rc::new(detail);
                    let shown = detail.clone();
                    when(
                        move || !shown().is_empty(),
                        move || {
                            let detail = detail.clone();
                            muted(move || detail())
                        },
                    )
                }),
        )
        .child(control)
}

/// A settings group: a label above a card of rows separated by hairlines.
pub fn group(label: Option<&'static str>, rows: Vec<AnyElement>) -> View {
    let count = rows.len();
    let mut inner = card();
    for (i, row) in rows.into_iter().enumerate() {
        inner = inner.child(row);
        if i + 1 < count {
            inner = inner.child(view().h(px(1.0)).ml_4().bg(move || theme().border));
        }
    }
    view()
        .gap_2()
        .child(label.map(|l| section_label(l).px_1()))
        .child(inner)
}

/// Rows in one rounded group: no outline, a soft fill, hairlines inset
/// between rows (the grouped lists of the platforms' settings).
pub fn rows(rows: Vec<AnyElement>) -> View {
    let count = rows.len();
    let mut inner = view()
        .rounded(px(20.0))
        .bg(move || theme().muted)
        .overflow_hidden();
    for (i, row) in rows.into_iter().enumerate() {
        inner = inner.child(row);
        if i + 1 < count {
            inner = inner.child(
                view()
                    .h(px(1.0))
                    .ml_5()
                    .bg(move || theme().border.opacity(0.7)),
            );
        }
    }
    inner
}

/// A row of [`rows`].
pub fn row_view() -> View {
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .px_5()
        .py_3()
        .min_h(px(58.0))
}

/// A capsule button: the prominent one filled with the accent, the others
/// a soft fill.
pub fn capsule(label: impl IntoReactive<SharedString>, prominent: bool) -> Button {
    button()
        .flex_row()
        .items_center()
        .justify_center()
        .h(px(38.0))
        .px_5()
        .rounded_full()
        .cursor_pointer()
        .bg(move || {
            if prominent {
                theme().primary
            } else {
                theme().secondary
            }
        })
        .text_color(move || {
            if prominent {
                theme().primary_foreground
            } else {
                theme().foreground
            }
        })
        .hover(|s| s.opacity(0.9))
        .focus(|s| s.outline(px(3.0)).outline_color(move || theme().ring))
        .disabled_style(|s| s.opacity(0.5))
        .child(
            text(label)
                .text_size(px(size::body() + 1.0))
                .font_weight(FontWeight::SEMIBOLD)
                .number_of_lines(1),
        )
}

/// A quiet text button (a link's look).
pub fn link_btn(label: impl IntoReactive<SharedString>) -> Button {
    button()
        .cursor_pointer()
        .px_1()
        .rounded(radius_small())
        .text_color(move || theme().foreground)
        .hover(|s| s.opacity(0.7))
        .focus(|s| s.outline(px(2.0)).outline_color(move || theme().ring))
        .disabled_style(|s| s.opacity(0.4))
        .child(
            text(label)
                .text_size(px(size::small()))
                .font_weight(FontWeight::MEDIUM),
        )
}

/// A short, neutral state or count.
pub fn badge(content: impl IntoReactive<SharedString>) -> View {
    view()
        .px_1p5()
        .py(px(1.0))
        .rounded(radius_small())
        .bg(move || theme().muted)
        .border_1()
        .border_color(move || theme().border)
        .child(
            text(content)
                .text_size(px(size::caption()))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().muted_foreground),
        )
}

/// A small status dot.
pub fn dot(tone: impl Fn() -> AdaptiveColor + 'static) -> View {
    view().size(px(8.0)).rounded_full().bg(tone)
}

/// The color that says how good a delay is.
pub fn delay_tone(delay: Option<u32>) -> AdaptiveColor {
    match delay {
        None => theme().muted_foreground,
        Some(0) => theme().destructive,
        Some(d) if d < 300 => success(),
        Some(d) if d < 800 => warning(),
        Some(_) => theme().destructive,
    }
}

pub fn delay_text(delay: Option<u32>) -> SharedString {
    match delay {
        None => "—".into(),
        Some(0) => "Timeout".into(),
        Some(d) => format!("{d} ms").into(),
    }
}

/// A thin bar showing `fraction` (0..=1).
pub fn meter(fraction: impl Fn() -> f32 + 'static) -> View {
    view()
        .h(px(4.0))
        .rounded_full()
        .bg(move || track())
        .overflow_hidden()
        .child(
            view()
                .h_full()
                .rounded_full()
                .bg(move || theme().primary)
                .w(move || percentage(fraction().clamp(0.0, 1.0))),
        )
}

/// What a page shows when it has nothing: a glyph, what is so (no explaining line) and an action.
pub fn empty_state(
    glyph: Icon,
    heading: impl IntoReactive<SharedString>,
    action: Option<AnyElement>,
) -> View {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .p_8()
        .child(
            icon(glyph, 32.0)
                .text_color(move || theme().muted_foreground)
                .mb_1(),
        )
        .child(
            text(heading)
                .text_center()
                .text_size(px(size::large()))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(action.map(|a| view().mt_2().child(a)))
}

/// A pop-up button: the current choice and a chevron; a press opens the
/// platform's menu of `options` (`(value, title)`), the current one checked.
pub fn popup_button<T: Clone + PartialEq + 'static>(
    options: impl Fn() -> Vec<(T, String)> + 'static,
    selected: impl Fn() -> T + Copy + 'static,
    on_select: impl Fn(T) + Copy + 'static,
) -> Button {
    let options = std::rc::Rc::new(options);
    let title = {
        let options = options.clone();
        move || -> SharedString {
            let current = selected();
            options()
                .into_iter()
                .find(|(v, _)| *v == current)
                .map(|(_, t)| t)
                .unwrap_or_default()
                .into()
        }
    };
    base_button(Variant::Secondary)
        .min_w(px(120.0))
        .rounded_full()
        .px_3p5()
        .justify_between()
        .on_press(move || {
            let current = selected();
            let items = options()
                .into_iter()
                .map(|(value, title)| {
                    let checked = value == current;
                    menu::Choice::new(title)
                        .checked(checked)
                        .on_select(move || on_select(value.clone()))
                })
                .collect();
            menu::open(items);
        })
        .child(flagged(title, size::body()))
        .child(icon(Icon::ChevronDown, 12.0).text_color(move || theme().muted_foreground))
}

#[cfg(test)]
mod flag_tests {
    #[test]
    fn a_flag_becomes_a_cropped_picture() {
        let uri = super::flag_uri("🇯🇵").expect("Japan");
        assert!(uri.starts_with("data:image/svg+xml,<svg"));
        assert!(uri.contains("viewBox='0 5 36 26'"));
        assert!(!uri.contains('#') && !uri.contains('"'));
        assert!(super::flag_uri("🇹🇼").is_some());
        assert!(super::flag_uri("◎").is_none());
    }
}
