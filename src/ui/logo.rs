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

/// The mark on a square with no picture: a service's own one or two letters ("Cu"), else the first
/// letter of what it is given. A whole name ("MiniMax") never goes in: it would wrap in the square.
fn mark(letters: &str) -> String {
    let n = letters.chars().count();
    if (1..=2).contains(&n) && letters.chars().all(char::is_alphanumeric) {
        letters.to_owned()
    } else {
        letter_of(letters)
    }
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

/// The name to show for an app: what the system calls the installed app ("Firefox" for the program
/// `firefox`) once it has said, else the name as it is. The program's name stays the key.
pub fn app_name(name: String) -> impl Fn() -> SharedString + 'static {
    let rev = store().icons_rev;
    move || {
        let _ = rev.get();
        untracked(|| store().svc().app_label(&name)).unwrap_or_else(|| name.clone()).into()
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
                text(mark(&l))
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

#[cfg(test)]
mod tests {
    use super::mark;

    #[test]
    fn a_square_with_no_picture_holds_one_or_two_letters_never_a_name() {
        // The AI apps with no logo of their own gave their whole name.
        assert_eq!(mark("MiniMax"), "M");
        assert_eq!(mark("Yuanbao"), "Y");
        assert_eq!(mark("Ernie"), "E");
        assert_eq!(mark("zhipu"), "Z");
        // A service's own letters stay.
        assert_eq!(mark("Cu"), "Cu");
        assert_eq!(mark("C"), "C");
        // A name in Chinese, or one that starts with a sign.
        assert_eq!(mark("通义千问"), "通");
        assert_eq!(mark("@scope"), "S");
        assert_eq!(mark(""), "·");
        for name in ["MiniMax", "Yuanbao", "Ernie", "Zhipu", "通义千问", "example.com"] {
            assert!(mark(name).chars().count() <= 2, "{name}");
        }
    }
}
