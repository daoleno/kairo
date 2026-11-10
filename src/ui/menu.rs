//! Menus that open from a button: the platform's own pop-up on a desktop, a sheet
//! from the bottom of the screen on a phone (Android has no pop-up menu to ask for:
//! `popup_menu` does nothing there).
//!
//! A menu is a list of [`Choice`]s; [`open`] shows it the way the platform does.

use std::cell::Cell;
use std::rc::Rc;

use nsh_base::{button, theme};
use nsh_core::element::desktop::{MenuItem, popup_menu};
use nsh_core::prelude::*;

use super::{Icon, icon, size};

/// One line of a menu.
#[derive(Clone)]
pub struct Choice {
    title: String,
    checked: bool,
    kind: Kind,
}

#[derive(Clone)]
enum Kind {
    Action(Option<Rc<dyn Fn()>>),
    Separator,
    Submenu(Vec<Choice>),
}

impl Choice {
    pub fn new(title: impl Into<String>) -> Self {
        Choice {
            title: title.into(),
            checked: false,
            kind: Kind::Action(None),
        }
    }

    pub fn separator() -> Self {
        Choice {
            title: String::new(),
            checked: false,
            kind: Kind::Separator,
        }
    }

    pub fn submenu(title: impl Into<String>, items: impl IntoIterator<Item = Choice>) -> Self {
        Choice {
            title: title.into(),
            checked: false,
            kind: Kind::Submenu(items.into_iter().collect()),
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn on_select(mut self, f: impl Fn() + 'static) -> Self {
        self.kind = Kind::Action(Some(Rc::new(f)));
        self
    }

    /// The same line for the desktop's own menu.
    fn item(self) -> MenuItem {
        match self.kind {
            Kind::Separator => MenuItem::separator(),
            Kind::Submenu(items) => MenuItem::submenu(
                super::menu_text(self.title),
                items.into_iter().map(Choice::item),
            ),
            Kind::Action(handler) => {
                let item = MenuItem::new(super::menu_text(self.title)).checked(self.checked);
                match handler {
                    Some(f) => item.on_select(move || f()),
                    None => item,
                }
            }
        }
    }
}

/// The lines of a menu on show (equal only to themselves: a new menu is a change).
#[derive(Clone)]
struct Shown(Rc<Vec<Choice>>);

impl PartialEq for Shown {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

thread_local! {
    /// The sheet a phone shows, set by [`host`].
    static SHEET: Cell<Option<Signal<Option<Shown>>>> = const { Cell::new(None) };
}

/// Shows `items` as a menu: a pop-up on a desktop, a sheet on a phone.
pub fn open(items: Vec<Choice>) {
    if super::phone() {
        if let Some(sheet) = SHEET.with(Cell::get) {
            sheet.set(Some(Shown(Rc::new(items))));
        }
        return;
    }
    let items: Vec<MenuItem> = items.into_iter().map(Choice::item).collect();
    spawn(async move {
        popup_menu(items).await;
    });
}

/// Where the phone's sheet is drawn: over everything, once, by the shell.
pub fn host() -> impl IntoElement {
    let shown = signal(None::<Shown>);
    SHEET.with(|s| s.set(Some(shown)));
    dynamic(move || match shown.get() {
        None => view().into_element(),
        Some(items) => untracked(move || sheet(items.0.to_vec(), shown)).into_element(),
    })
}

fn sheet(items: Vec<Choice>, shown: Signal<Option<Shown>>) -> impl IntoElement {
    let close = move || shown.set(None);
    let rows: Vec<AnyElement> = items
        .into_iter()
        .map(|c| line(c, shown).into_element())
        .collect();
    view()
        .absolute()
        .inset_0()
        .child(
            overlay()
                .absolute()
                .inset_0()
                .bg(move || theme().foreground.opacity(0.28))
                .on_press(close),
        )
        .child(
            overlay()
                .absolute()
                .inset_0()
                .justify_end()
                .pointer_events(PointerEvents::BoxNone)
                .on_cancel(close)
                .child(
                    view()
                        .mx_2()
                        .mb(move || {
                            // Above the keyboard when it is up, else above the gesture bar.
                            let k = keyboard_height();
                            px(if k > px(0.0) { k.0 + 8.0 } else { safe_area().bottom.0 + 8.0 })
                        })
                        .py_2()
                        .rounded(px(20.0))
                        .bg(move || theme().background)
                        .border_1()
                        .border_color(move || theme().border)
                        .shadow_lg()
                        // Many choices scroll; the Cancel button below stays in reach.
                        .child(scroll().max_h(px(380.0)).child(view().children(rows)))
                        .child(
                            view()
                                .h(px(1.0))
                                .my_1()
                                .bg(move || theme().border),
                        )
                        .child(
                            button()
                                .w_full()
                                .h(px(super::touch_h(48.0)))
                                .flex_row()
                                .items_center()
                                .justify_center()
                                .on_press(close)
                                .child(
                                    text("Cancel")
                                        .text_size(px(size::body()))
                                        .font_weight(FontWeight::SEMIBOLD),
                                ),
                        ),
                ),
        )
}

fn line(c: Choice, shown: Signal<Option<Shown>>) -> impl IntoElement {
    let Choice {
        title,
        checked,
        kind,
    } = c;
    if let Kind::Separator = kind {
        return view()
            .h(px(1.0))
            .my_1()
            .bg(move || theme().border)
            .into_element();
    }
    let (press, more): (Rc<dyn Fn()>, bool) = match kind {
        Kind::Submenu(items) => (Rc::new(move || shown.set(Some(Shown(Rc::new(items.clone()))))), true),
        Kind::Action(handler) => (
            Rc::new(move || {
                shown.set(None);
                // Not under the sheet: it goes with the press, and what the choice starts
                // (a request to the service) must not.
                if let Some(f) = handler.clone() {
                    Owner::detached().run(|| f());
                }
            }),
            false,
        ),
        Kind::Separator => unreachable!(),
    };
    button()
        .w_full()
        .h(px(super::touch_h(48.0)))
        .flex_row()
        .items_center()
        .gap_3()
        .px_5()
        .accessibility_label(title.clone())
        .on_press(move || press())
        .child(
            view().w(px(20.0)).items_center().child(
                icon(Icon::Check, 16.0)
                    .text_color(move || theme().primary)
                    .style_when(move || !checked, |s| s.invisible()),
            ),
        )
        .child(
            // A country's flag is a picture, not the emoji (not every platform draws them).
            view().flex_1().min_w(px(0.0)).child(super::flagged(
                move || -> SharedString { title.clone().into() },
                size::body(),
            )),
        )
        .children(more.then(|| {
            icon(Icon::ChevronRight, 14.0).text_color(move || theme().muted_foreground)
        }))
        .into_element()
}
