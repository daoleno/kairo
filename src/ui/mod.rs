//! Kairo's styled component layer over `nsh_base`: tokens, the type scale and
//! the controls every page composes. Pages never restyle base components.

pub mod icons;
pub mod toolbar;

use nsh_base::{button, input, switch, theme, ButtonElement as Button, Input};

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
    text(content).text_size(px(size::small())).text_color(move || theme().muted_foreground)
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
    text(content).text_size(px(size::title())).font_weight(FontWeight::SEMIBOLD).role(AccessibilityRole::Header)
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
        .h(px(28.0))
        .px_3()
        .rounded(radius_control())
        .cursor_pointer()
        .focus(|s| s.outline(px(2.0)).outline_color(move || theme().ring))
        .disabled_style(|s| s.opacity(0.45));
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
    base_button(variant).child(text(label).text_size(px(size::body())).font_weight(FontWeight::MEDIUM))
}

/// A button with an icon before its label.
pub fn btn_icon(glyph: Icon, label: impl IntoReactive<SharedString>, variant: Variant) -> Button {
    base_button(variant)
        .child(icon(glyph, 14.0))
        .child(text(label).text_size(px(size::body())).font_weight(FontWeight::MEDIUM))
}

/// A button whose icon and label follow state.
pub fn btn_icon_dyn(glyph: impl Fn() -> Icon + 'static, label: impl IntoReactive<SharedString>, variant: Variant) -> Button {
    base_button(variant)
        .child(svg(move || icons::uri(glyph())).size(px(14.0)).flex_shrink_0())
        .child(text(label).text_size(px(size::body())).font_weight(FontWeight::MEDIUM))
}

/// An icon-only button; `label` names it for screen readers and tests.
pub fn icon_btn(glyph: Icon, label: &'static str) -> Button {
    button()
        .accessibility_label(label)
        .items_center()
        .justify_center()
        .size(px(28.0))
        .rounded(radius_control())
        .cursor_pointer()
        .text_color(move || theme().muted_foreground)
        .hover(|s| s.bg(move || theme().accent).text_color(move || theme().foreground))
        .active(|s| s.opacity(0.7))
        .focus(|s| s.outline(px(2.0)).outline_color(move || theme().ring))
        .disabled_style(|s| s.opacity(0.4))
        .child(icon(glyph, 16.0))
}

// --- Controls ----------------------------------------------------------------------

/// An on/off switch in the platform's proportions.
pub fn toggle(checked: impl Fn() -> bool + 'static, label: &'static str, on_change: impl Fn(bool) + 'static) -> AnyElement {
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
        let width = options.iter().map(|o| o.1.len() as f32 * 7.5 + 26.0).sum::<f32>() + 4.0;
        let index = {
            let values = values.clone();
            move || JsonValue::from(values.iter().position(|v| *v == selected()).map_or(-1, |i| i as i64))
        };
        return view().flex_shrink_0().child(
            native("kairo/segmented")
                .prop("labels", JsonValue::Array(labels))
                .prop("selected", index)
                .w(px(width))
                .h(px(24.0))
                .on_native_event(move |e: &NativeEvent| {
                    if let Some(v) = e.data["index"].as_u64().and_then(|i| values.get(i as usize)) {
                        on_select(*v);
                    }
                }),
        );
    }
    view()
        .flex_row()
        .flex_shrink_0()
        .p(px(2.0))
        .gap(px(2.0))
        .rounded(radius_control())
        .bg(move || theme().muted)
        .border_1()
        .border_color(move || theme().border)
        .children(options.into_iter().map(move |(value, label)| {
            let is = move || selected() == value;
            button()
                .selected(is)
                .role(AccessibilityRole::Tab)
                .on_press(move || on_select(value))
                .flex_grow(1.0)
                .flex_shrink_0()
                .min_w(px(56.0))
                .items_center()
                .justify_center()
                .h(px(24.0))
                .px_3()
                .rounded(radius_small())
                .cursor_pointer()
                .text_color(move || theme().muted_foreground)
                .hover(|s| s.text_color(move || theme().foreground))
                .style_when(is, |s| s.bg(move || theme().background).shadow_sm().text_color(move || theme().foreground))
                .child(text(label).text_size(px(size::body())).font_weight(FontWeight::MEDIUM).number_of_lines(1))
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
        .h(px(28.0))
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
    view()
        .flex_row()
        .items_center()
        .gap_4()
        .px_4()
        .py_2p5()
        .min_h(px(ROW_MIN))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap_0p5()
                .child(body(label))
                .child(detail.map(|d| muted(d))),
        )
        .child(control)
}

/// A row whose description follows state.
pub fn setting_row_dyn(label: &'static str, detail: impl Fn() -> SharedString + 'static, control: impl IntoElement) -> View {
    view()
        .flex_row()
        .items_center()
        .gap_4()
        .px_4()
        .py_2p5()
        .min_h(px(ROW_MIN))
        .child(view().flex_1().min_w(px(0.0)).gap_0p5().child(body(label)).child(muted(detail)))
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
    view().gap_2().child(label.map(|l| section_label(l).px_1())).child(inner)
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
        .child(text(content).text_size(px(size::caption())).font_weight(FontWeight::MEDIUM).text_color(move || theme().muted_foreground))
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
        .child(view().h_full().rounded_full().bg(move || theme().primary).w(move || percentage(fraction().clamp(0.0, 1.0))))
}

/// What a page shows when it has nothing: a glyph, a title, a line and an action.
pub fn empty_state(glyph: Icon, heading: &'static str, detail: impl IntoReactive<SharedString>, action: Option<AnyElement>) -> View {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .p_8()
        .child(icon(glyph, 32.0).text_color(move || theme().muted_foreground).mb_1())
        .child(text(heading).text_size(px(size::large())).font_weight(FontWeight::SEMIBOLD))
        .child(muted(detail).text_center().max_w(px(360.0)))
        .child(action.map(|a| view().mt_2().child(a)))
}

/// A page's header: its title on the left, its actions on the right.
pub fn page_header(heading: impl IntoReactive<SharedString>, actions: Vec<AnyElement>) -> View {
    if crate::chrome::native() {
        // The window's toolbar shows the title.
        let empty = actions.is_empty();
        return view()
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .gap_2()
            .px_5()
            .style_when(move || !empty, |s| s.h(px(44.0)))
            .children(actions);
    }
    view()
        .h(px(HEADER_HEIGHT))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap_2()
        .px_5()
        .border_b_1()
        .border_color(move || theme().border)
        .window_drag(true)
        .child(title(heading).text_size(px(size::large() + 2.0)).flex_1().number_of_lines(1))
        .children(actions)
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
            options().into_iter().find(|(v, _)| *v == current).map(|(_, t)| t).unwrap_or_default().into()
        }
    };
    base_button(Variant::Secondary)
        .min_w(px(120.0))
        .justify_between()
        .on_press(move || {
            let current = selected();
            let items = options()
                .into_iter()
                .map(|(value, title)| {
                    let checked = value == current;
                    nsh_core::element::desktop::MenuItem::new(title).checked(checked).on_select(move || on_select(value.clone()))
                })
                .collect();
            spawn(async move {
                nsh_core::element::desktop::popup_menu(items).await;
            });
        })
        .child(text(title).text_size(px(size::body())).number_of_lines(1))
        .child(icon(Icon::ChevronDown, 12.0).text_color(move || theme().muted_foreground))
}
