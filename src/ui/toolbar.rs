//! A page's toolbar: its title, a subtitle and its tools (search, buttons,
//! choices), declared once.
//!
//! With the platform's chrome (`chrome::native`), the tools are the window's
//! own: an `NSToolbar` on macOS, a libadwaita header bar on Linux. The page
//! sends what they show through the bridge and gets their events back. In the
//! headless harness and without the chrome, NSH draws them in a header above
//! the page, as it always did.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use nsh_base::theme;
use nsh_core::prelude::*;

use super::{HEADER_HEIGHT, Icon, Variant, search_field, size, title};
use crate::chrome::{self, Glyph, Tool, ToolEvent, Toolbar};
use crate::store::Page;

type Shared<T> = Rc<dyn Fn() -> T>;

/// One tool, with what it reads and does.
pub enum Spec {
    Search { id: &'static str, placeholder: &'static str, value: Signal<String> },
    Button {
        id: &'static str,
        label: Shared<String>,
        glyph: Shared<Glyph>,
        icon: Shared<Icon>,
        enabled: Shared<bool>,
        prominent: bool,
        press: Rc<dyn Fn()>,
    },
    Choice {
        id: &'static str,
        label: &'static str,
        glyph: Glyph,
        options: Vec<&'static str>,
        selected: Shared<usize>,
        choose: Rc<dyn Fn(usize)>,
    },
}

/// A search field bound to `value`.
pub fn search(id: &'static str, placeholder: &'static str, value: Signal<String>) -> Spec {
    Spec::Search { id, placeholder, value }
}

/// A button showing `glyph` (and `icon` where NSH draws it), named `label`.
pub fn button(id: &'static str, label: &'static str, glyph: Glyph, icon: Icon, press: impl Fn() + 'static) -> Spec {
    Spec::Button {
        id,
        label: Rc::new(move || label.to_owned()),
        glyph: Rc::new(move || glyph),
        icon: Rc::new(move || icon),
        enabled: Rc::new(|| true),
        prominent: false,
        press: Rc::new(press),
    }
}

/// A button whose glyph and label follow state (Pause / Resume).
pub fn button_dyn(
    id: &'static str,
    label: impl Fn() -> &'static str + 'static,
    glyph: impl Fn() -> (Glyph, Icon) + 'static,
    press: impl Fn() + 'static,
) -> Spec {
    let glyph = Rc::new(glyph);
    let g = glyph.clone();
    Spec::Button {
        id,
        label: Rc::new(move || label().to_owned()),
        glyph: Rc::new(move || g().0),
        icon: Rc::new(move || glyph().1),
        enabled: Rc::new(|| true),
        prominent: false,
        press: Rc::new(press),
    }
}

/// A pull-down of `options`, the `selected()` one checked.
pub fn choice(
    id: &'static str,
    label: &'static str,
    glyph: Glyph,
    options: Vec<&'static str>,
    selected: impl Fn() -> usize + 'static,
    choose: impl Fn(usize) + 'static,
) -> Spec {
    Spec::Choice { id, label, glyph, options, selected: Rc::new(selected), choose: Rc::new(choose) }
}

impl Spec {
    /// Enabled only while `when()` holds.
    pub fn enabled(mut self, when: impl Fn() -> bool + 'static) -> Spec {
        if let Spec::Button { enabled, .. } = &mut self {
            *enabled = Rc::new(when);
        }
        self
    }

    /// The page's main action (drawn filled where NSH draws it).
    pub fn prominent(mut self) -> Spec {
        if let Spec::Button { prominent, .. } = &mut self {
            *prominent = true;
        }
        self
    }

    fn id(&self) -> &'static str {
        match self {
            Spec::Search { id, .. } | Spec::Button { id, .. } | Spec::Choice { id, .. } => id,
        }
    }

    fn tool(&self) -> Tool {
        match self {
            Spec::Search { id, placeholder, value } => Tool::Search { id, placeholder, value: value.get() },
            Spec::Button { id, label, glyph, enabled, .. } => Tool::Button { id, label: label(), glyph: glyph(), enabled: enabled() },
            Spec::Choice { id, label, glyph, options, selected, .. } => {
                Tool::Choice { id, label, glyph: *glyph, options: options.clone(), selected: selected() }
            }
        }
    }

    fn handler(&self) -> Handler {
        match self {
            Spec::Search { value, .. } => Handler::Search(*value),
            Spec::Button { press, .. } => Handler::Press(press.clone()),
            Spec::Choice { choose, .. } => Handler::Choose(choose.clone()),
        }
    }

    /// The tool as NSH draws it.
    fn element(self) -> AnyElement {
        match self {
            Spec::Search { placeholder, value, .. } => search_field(value, placeholder).w(px(220.0)).into_element(),
            Spec::Button { label, icon, enabled, prominent, press, .. } => {
                if prominent {
                    let l = label.clone();
                    super::btn_icon_dyn(move || icon(), move || -> SharedString { l().into() }, Variant::Primary)
                        .disabled(move || !enabled())
                        .on_press(move || press())
                        .into_element()
                } else {
                    let l = label.clone();
                    super::btn_icon_dyn(move || icon(), move || -> SharedString { l().into() }, Variant::Secondary)
                        .disabled(move || !enabled())
                        .on_press(move || press())
                        .into_element()
                }
            }
            Spec::Choice { options, selected, choose, .. } => {
                // Few choices, all in sight: a segmented control.
                let selected = stored(selected);
                let choose = stored(choose);
                super::segmented(options.iter().enumerate().map(|(i, o)| (i, *o)).collect(), move || selected.with(|f| f()), move |i| {
                    choose.with(|f| f(i))
                })
                .into_element()
            }
        }
    }
}

enum Handler {
    Search(Signal<String>),
    Press(Rc<dyn Fn()>),
    Choose(Rc<dyn Fn(usize)>),
}

thread_local! {
    /// The shown page's tools, by id.
    static HANDLERS: RefCell<(Option<Page>, HashMap<&'static str, Handler>)> = RefCell::new((None, HashMap::new()));
}

/// Carries out a tool's event from the chrome (the store calls it).
pub fn dispatch(event: ToolEvent) {
    // The window's own shortcuts (GTK accelerators), outside any page's tools.
    if let ToolEvent::Pressed("palette") = event {
        crate::palette::open();
        return;
    }
    let handler = HANDLERS.with(|h| {
        let h = h.borrow();
        let id = match &event {
            ToolEvent::Pressed(id) | ToolEvent::Search(id, _) | ToolEvent::Chose(id, _) => *id,
        };
        h.1.get(id).map(|handler| match handler {
            Handler::Search(s) => Handler::Search(*s),
            Handler::Press(f) => Handler::Press(f.clone()),
            Handler::Choose(f) => Handler::Choose(f.clone()),
        })
    });
    match (handler, event) {
        (Some(Handler::Search(value)), ToolEvent::Search(_, text)) => {
            if value.get_untracked() != text {
                value.set(text);
            }
        }
        (Some(Handler::Press(f)), ToolEvent::Pressed(_)) => f(),
        (Some(Handler::Choose(f)), ToolEvent::Chose(_, i)) => f(i),
        _ => {}
    }
}

/// Above a scrolling page whose content runs under a floating toolbar
/// (macOS): the space the toolbar takes at the top of that content, plus `gap`.
pub fn scroll_top(gap: f32) -> impl Fn() -> Points + Copy + 'static {
    let store = crate::store::store();
    move || px(store.insets.get().top + gap)
}

/// The toolbar of a page that scrolls under it: as [`toolbar`], without the
/// room above the page (its content leaves it, with [`scroll_top`]).
pub fn floating(page: Page, subtitle: impl Fn() -> String + 'static, tools: Vec<Spec>) -> View {
    toolbar_with(page, subtitle, tools, false)
}

/// The page's toolbar: in the window's chrome, or drawn above the page.
pub fn toolbar(page: Page, subtitle: impl Fn() -> String + 'static, tools: Vec<Spec>) -> View {
    toolbar_with(page, subtitle, tools, true)
}

fn toolbar_with(page: Page, subtitle: impl Fn() -> String + 'static, tools: Vec<Spec>, room: bool) -> View {
    if chrome::native() {
        let specs = Rc::new(tools);
        HANDLERS.with(|h| {
            let mut h = h.borrow_mut();
            h.0 = Some(page);
            h.1 = specs.iter().map(|s| (s.id(), s.handler())).collect();
        });
        {
            let specs = specs.clone();
            effect(move || {
                let next = Toolbar { page: Some(page), subtitle: subtitle(), tools: specs.iter().map(Spec::tool).collect() };
                chrome::bridge().toolbar.send_if_modified(|t| {
                    let changed = *t != next;
                    if changed {
                        *t = next;
                    }
                    changed
                });
            });
        }
        cleanup(move || {
            HANDLERS.with(|h| {
                let mut h = h.borrow_mut();
                if h.0 == Some(page) {
                    h.0 = None;
                    h.1.clear();
                }
            });
            chrome::bridge().toolbar.send_if_modified(|t| {
                let mine = t.page == Some(page);
                if mine {
                    *t = Toolbar::default();
                }
                mine
            });
        });
        // Room for the window's toolbar where it floats over the content.
        let store = crate::store::store();
        return view().flex_shrink_0().h(move || px(if room { store.insets.get().top } else { 0.0 }));
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
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_baseline()
                .gap_2()
                .child(title(page.title()).text_size(px(size::large() + 2.0)).number_of_lines(1))
                .child(
                    text(move || -> SharedString { subtitle().into() })
                        .text_size(px(size::small()))
                        .tabular_nums()
                        .number_of_lines(1)
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .children(tools.into_iter().map(Spec::element).collect::<Vec<_>>())
}
