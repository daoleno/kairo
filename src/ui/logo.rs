//! The picture of an app, a site or an AI service: its real icon, never a tinted mark.
//!
//! In this order: an installed app's own icon from the system; a site's own icon (fetched from
//! the site and kept on disk); a bundled logo of a well-known name (`assets/logos`); and, last, a
//! letter. What is on disk is shown at once; what is not is looked for in the background and the
//! picture appears when it is kept (a fetch never makes the page wait).

use std::path::PathBuf;

use nsh_base::theme;
use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;

use crate::logos::{self, Named};
use crate::store::store;
use crate::ui::{self, size};

/// What a picture is of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// An app, by the program's name as Activity and Apps & Sites call it.
    App(String),
    /// A site, by its domain.
    Site(String),
    /// An AI service, by its id (`claude`), with the letters to show when nothing else is.
    Service(&'static str, &'static str),
}

/// What is known now about a subject's picture.
#[derive(Debug, Clone, PartialEq)]
enum Shown {
    /// A file on disk (a PNG, or an SVG), with whether it needs a white tile (a brand's mark or a
    /// site's small icon; an app's own icon is already a shape of its own).
    File { path: PathBuf, tile: bool },
    /// A bundled SVG.
    Bundled(&'static str),
    Letter(String),
}

fn letter_of(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "·".into())
}

fn resolve(subject: &Subject) -> Shown {
    let svc = store().svc();
    match subject {
        Subject::App(name) => {
            if let Some(path) = svc.app_icon(name) {
                return Shown::File { path, tile: false };
            }
            match logos::bundled(Named::App, name) {
                Some(svg) => Shown::Bundled(svg),
                None => Shown::Letter(letter_of(name)),
            }
        }
        Subject::Site(domain) => {
            if let Some(path) = svc.site_icon(domain) {
                return Shown::File { path, tile: true };
            }
            match logos::bundled(Named::Site, domain) {
                Some(svg) => Shown::Bundled(svg),
                None => Shown::Letter(letter_of(domain)),
            }
        }
        Subject::Service(id, letters) => match logos::bundled(Named::Service, id) {
            Some(svg) => Shown::Bundled(svg),
            None => Shown::Letter((*letters).to_owned()),
        },
    }
}

/// A square of `side` points holding the subject's picture.
pub fn logo(subject: Subject, side: f32) -> AnyElement {
    let rev = store().icons_rev;
    // The picture could not be shown (a file that does not decode): the letter instead.
    let broken = signal(false);
    dynamic(move || {
        // A kept picture makes this run again.
        let _ = rev.get();
        let subject = subject.clone();
        let broken_now = broken.get();
        untracked(move || {
            let mut shown = resolve(&subject);
            if broken_now && matches!(shown, Shown::File { .. }) {
                let name = match &subject {
                    Subject::App(n) | Subject::Site(n) => n.clone(),
                    Subject::Service(_, l) => (*l).to_owned(),
                };
                shown = match &subject {
                    Subject::Service(..) => Shown::Letter(name),
                    _ => Shown::Letter(letter_of(&name)),
                };
            }
            draw(shown, side, broken).into_element()
        })
    })
    .into_element()
}

/// The white tile a brand's mark sits on: marks are black or dark in places, and the tile is what
/// keeps them readable in dark mode without recolouring them.
fn tile(side: f32) -> View {
    view()
        .size(px(side))
        .flex_shrink_0()
        .rounded(px(side * 0.28))
        .items_center()
        .justify_center()
        .overflow_hidden()
        .bg(|| AdaptiveColor::from(white()))
        .border_1()
        .border_color(move || theme().border)
}

fn draw(shown: Shown, side: f32, broken: Signal<bool>) -> View {
    let inner = (side * 0.68).round();
    match shown {
        Shown::File { path, tile: false } => view()
            .size(px(side))
            .flex_shrink_0()
            .child(file_picture(&path, side, broken)),
        Shown::File { path, tile: true } => tile(side).child(file_picture(&path, inner, broken)),
        Shown::Bundled(markup) => tile(side).child(
            svg(ImageSrc::svg_markup(markup))
                .size(px(inner))
                .flex_shrink_0(),
        ),
        Shown::Letter(l) => view()
            .size(px(side))
            .flex_shrink_0()
            .rounded(px(side * 0.28))
            .items_center()
            .justify_center()
            .bg(ui::subtle)
            .child(
                text(l)
                    .text_size(px(size::small()))
                    .font_weight(FontWeight::SEMIBOLD),
            ),
    }
}

fn file_picture(path: &std::path::Path, side: f32, broken: Signal<bool>) -> AnyElement {
    let uri = path.to_string_lossy().into_owned();
    if path.extension().is_some_and(|e| e == "svg") {
        return svg(uri).size(px(side)).flex_shrink_0().into_element();
    }
    image(uri)
        .size(px(side))
        .object_fit(ResizeMode::Contain)
        .flex_shrink_0()
        .on_error(move |_: &str| broken.set(true))
        .into_element()
}
