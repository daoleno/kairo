//! Kairo's tray / menu bar item: whether it is connected, how the AI services
//! stand, the two exits you switch most (Everything else, AI) with a country
//! each, the mode, and the window, Settings and Quit.
//!
//! - Linux: a StatusNotifierItem (KDE Plasma, GNOME with the AppIndicator
//!   extension, most panels) through `ksni`.
//! - macOS: an `NSStatusItem` with an `NSMenu`.

use std::sync::Arc;

use kairo_core::{CoreStatus, Mode, Service};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

/// What the app does for the tray.
#[derive(Clone)]
pub struct Hooks {
    /// Shows and focuses the main window.
    pub show: Arc<dyn Fn() + Send + Sync>,
    /// Shows the window on a page (`"exits"`, `"settings"`).
    pub page: Arc<dyn Fn(&'static str) + Send + Sync>,
    /// Quits the app (after restoring the system proxy).
    pub quit: Arc<dyn Fn() + Send + Sync>,
}

/// A country in an exit's menu: choosing it picks its fastest server.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// The server to go through (empty for nothing: see [`ExitMenu::fastest`]).
    pub name: String,
    pub label: String,
    /// Good, OK, Slow or "".
    pub health: &'static str,
    pub checked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitId {
    /// Everything else (the Location).
    Rest,
    Ai,
}

/// One exit's submenu.
#[derive(Debug, Clone, PartialEq)]
pub struct ExitMenu {
    pub id: ExitId,
    pub title: &'static str,
    /// Where it goes now ("🇭🇰 Hong Kong").
    pub place: String,
    /// `Some(on)` when it has "Fastest" (the AI exit never does).
    pub fastest: Option<bool>,
    pub choices: Vec<Choice>,
}

/// What the menu shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Model {
    pub status: String,
    pub running: bool,
    pub mode: Mode,
    pub up: u64,
    pub down: u64,
    pub has_profile: bool,
    /// "AI: all 6 see Japan", "AI: 2 problems".
    pub ai: String,
    pub exits: Vec<ExitMenu>,
}

/// A flag for a menu: Taiwan's is left out (Apple's emoji font omits it on
/// Macs set to some regions, and a menu cannot draw a picture there).
pub fn menu_flag(flag: &str) -> String {
    if flag == "🇹🇼" || flag.is_empty() {
        String::new()
    } else {
        format!("{flag}  ")
    }
}

/// How a delay reads in a menu.
pub fn health_word(delay: Option<u32>) -> &'static str {
    match delay {
        None => "",
        Some(0) => "Slow",
        Some(d) if d < 200 => "Good",
        Some(d) if d < 500 => "OK",
        Some(_) => "Slow",
    }
}

/// The AI line: what the last check found, in a few words.
pub fn ai_line(state: &kairo_core::State) -> String {
    use kairo_core::ai_check::{self, Api};
    let checks = &state.ai.checks;
    if !state.settings.ai.enabled || checks.is_empty() {
        return String::new();
    }
    let odd = ai_check::odd_ones(checks);
    let problems = checks
        .iter()
        .filter(|c| {
            !c.reachable
                || matches!(c.api, Api::Unexpected { .. } | Api::Unreachable { .. })
                || odd.as_ref().is_some_and(|(_, ids)| ids.contains(&c.service))
        })
        .count()
        + state.bypass.iter().map(|b| b.tool.clone()).collect::<std::collections::BTreeSet<_>>().len();
    if problems > 0 {
        return format!("AI: {problems} {}", if problems == 1 { "problem" } else { "problems" });
    }
    let codes: std::collections::BTreeSet<&str> =
        checks.iter().filter_map(|c| c.country.code()).collect();
    match (codes.len(), codes.iter().next()) {
        (1, Some(code)) => format!("AI: all {} see {}", checks.len(), ai_check::country_name(code)),
        _ => format!("AI: {} reachable", checks.len()),
    }
}

impl Model {
    pub fn of(svc: &Service) -> Model {
        use kairo_core::places;
        let state = svc.state().borrow().clone();
        let live = svc.live().borrow().clone();
        let proxies = svc.proxies().borrow().clone();
        let s = &state.settings;
        let main = places::main_group(&proxies.groups, &s.tray_group);
        let rest_place = main.and_then(|g| places::current(&proxies.groups, g));
        let ai_place = svc.ai_node().and_then(|n| {
            proxies
                .groups
                .iter()
                .flat_map(|g| g.members.iter())
                .find(|m| m.name == n && !m.is_group)
                .map(places::place)
        });
        let label_of = |p: &places::Place| {
            format!(
                "{}{}",
                menu_flag(p.flag.as_deref().unwrap_or("")),
                if p.region == "Other" { p.title.clone() } else { p.region.to_owned() }
            )
        };
        // Countries, the best server of each, quickest first.
        let countries = |now: Option<&places::Place>| -> Vec<Choice> {
            let Some(g) = main else { return vec![] };
            let all = places::places(&proxies.groups, g);
            let mut out: Vec<(u32, Choice)> = places::by_region(&all)
                .into_iter()
                .filter_map(|(region, list)| {
                    let best = list
                        .iter()
                        .filter(|p| p.delay.is_some_and(|d| d > 0))
                        .min_by_key(|p| p.delay)
                        .or(list.first())?;
                    let flag = best
                        .flag
                        .clone()
                        .or_else(|| places::region_flag(region))
                        .unwrap_or_default();
                    Some((
                        best.delay.filter(|d| *d > 0).unwrap_or(u32::MAX),
                        Choice {
                            name: best.name.clone(),
                            label: format!("{}{region}", menu_flag(&flag)),
                            health: health_word(best.delay),
                            checked: now.is_some_and(|n| list.iter().any(|p| p.name == n.name)),
                        },
                    ))
                })
                .collect();
            out.sort_by_key(|(d, _)| *d);
            out.into_iter().take(PLACES).map(|(_, c)| c).collect()
        };
        let mut exits = Vec::new();
        if main.is_some() {
            exits.push(ExitMenu {
                id: ExitId::Rest,
                title: "Everything else",
                place: rest_place.as_ref().map(label_of).unwrap_or_else(|| "—".into()),
                fastest: Some(s.auto_location),
                choices: countries(if s.auto_location { None } else { rest_place.as_ref() }),
            });
            if s.ai.enabled && !state.ai.group.is_empty() {
                exits.push(ExitMenu {
                    id: ExitId::Ai,
                    title: "AI exit",
                    place: ai_place.as_ref().map(label_of).unwrap_or_else(|| "—".into()),
                    fastest: None,
                    choices: countries(ai_place.as_ref()),
                });
            }
        }
        let status = match &state.core {
            CoreStatus::Running { .. } => "Connected".to_owned(),
            CoreStatus::Starting => "Connecting…".into(),
            CoreStatus::Failed { .. } => "Couldn't connect".into(),
            _ => "Not Connected".into(),
        };
        Model {
            status,
            running: state.core.is_running(),
            mode: s.mode,
            up: live.up,
            down: live.down,
            has_profile: state.profiles.active.is_some(),
            ai: if state.core.is_running() { ai_line(&state) } else { String::new() },
            exits,
        }
    }

    /// The menu label of a country: its name and how good it is.
    pub fn choice_label(c: &Choice) -> String {
        if c.health.is_empty() {
            c.label.clone()
        } else {
            format!("{}    {}", c.label, c.health)
        }
    }

    /// The mode in plain words.
    pub fn route_title(m: Mode) -> &'static str {
        match m {
            Mode::Rule => "Smart",
            Mode::Global => "Everything",
            Mode::Direct => "Direct",
        }
    }
}

/// How many countries a menu lists (the rest: All Servers… in the window).
pub const PLACES: usize = 12;

/// The actions of the menu, carried out on the service.
pub(crate) mod act {
    use super::*;

    pub fn mode(svc: &Service, m: Mode) {
        svc.fire(svc.set_mode(m));
    }
    /// Everything else: a server, or (empty) Fastest.
    pub fn locate(svc: &Service, name: String) {
        svc.fire(svc.set_location((!name.is_empty()).then_some(name)));
    }
    /// The AI exit: a server.
    pub fn ai_exit(svc: &Service, name: String) {
        svc.fire(svc.set_ai_exit(name));
    }
    pub fn test_again(svc: &Service) {
        svc.fire(svc.test_places());
    }
    pub fn connect(svc: &Service, on: bool) {
        svc.fire(svc.connect(on));
    }
}

/// The logo as ARGB32 (network byte order) pixels, `size` pixels square.
/// `dim` greys it (the core is stopped).
pub fn icon_argb(size: u32, dim: bool) -> Vec<u8> {
    let rgba = render(include_str!("../../../assets/icon.svg"), size);
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        let (mut r, mut g, mut b, a) = (px[0], px[1], px[2], px[3]);
        if dim {
            let l = ((r as u32 * 30 + g as u32 * 59 + b as u32 * 11) / 100) as u8;
            (r, g, b) = (l, l, l);
        }
        out.extend_from_slice(&[a, r, g, b]);
    }
    out
}

/// The menu bar glyph (black on transparent) as straight RGBA.
pub fn glyph_rgba(size: u32) -> Vec<u8> {
    render(include_str!("../../../assets/glyph.svg"), size)
}

/// Renders an SVG to a PNG of `size` × `size` pixels.
pub fn render_png(svg: &str, size: u32) -> Vec<u8> {
    use resvg::{tiny_skia, usvg};
    let tree =
        usvg::Tree::from_str(svg, &usvg::Options::default()).expect("the bundled svg parses");
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let s = tree.size();
    let scale = size as f32 / s.width().max(s.height());
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().expect("png")
}

/// Renders an SVG to straight (not premultiplied) RGBA.
pub fn render(svg: &str, size: u32) -> Vec<u8> {
    use resvg::{tiny_skia, usvg};
    let tree =
        usvg::Tree::from_str(svg, &usvg::Options::default()).expect("the bundled svg parses");
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let s = tree.size();
    let scale = size as f32 / s.width().max(s.height());
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut data = pixmap.take();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    data
}

/// Starts the tray. `Err` when the desktop has no tray (the app then keeps its
/// window instead of hiding to the tray).
pub fn start(svc: Service, hooks: Hooks) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::start(svc, hooks)
    }
    #[cfg(target_os = "macos")]
    {
        macos::start(svc, hooks)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (svc, hooks);
        Err("no tray on this platform yet".into())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_a_png() {
        let png = super::render_png(include_str!("../../../assets/glyph.svg"), 36);
        assert_eq!(&png[1..4], b"PNG");
        let dim = super::render_png(
            &include_str!("../../../assets/glyph.svg").replacen(
                "<svg ",
                "<svg opacity=\"0.45\" ",
                1,
            ),
            36,
        );
        assert_ne!(png, dim);
    }

    #[test]
    fn renders_the_icon() {
        let px = super::icon_argb(32, false);
        assert_eq!(px.len(), 32 * 32 * 4);
        // The centre is the white route or the graphite tile: opaque.
        let i = (16 * 32 + 16) * 4;
        assert_eq!(px[i], 255);
        let g = super::glyph_rgba(22);
        assert!(g.chunks_exact(4).any(|p| p[3] == 255));
    }

    #[test]
    fn labels() {
        use super::{Choice, Model};
        let c = |health| Choice {
            name: "x".into(),
            label: "🇭🇰  Hong Kong".into(),
            health,
            checked: false,
        };
        assert_eq!(Model::choice_label(&c("Good")), "🇭🇰  Hong Kong    Good");
        assert_eq!(Model::choice_label(&c("")), "🇭🇰  Hong Kong");
    }

    #[test]
    fn delays_read_as_words_and_taiwan_has_no_flag_in_a_menu() {
        assert_eq!(super::health_word(None), "");
        assert_eq!(super::health_word(Some(0)), "Slow");
        assert_eq!(super::health_word(Some(60)), "Good");
        assert_eq!(super::health_word(Some(300)), "OK");
        assert_eq!(super::health_word(Some(900)), "Slow");
        assert_eq!(super::menu_flag("🇹🇼"), "");
        assert_eq!(super::menu_flag("🇯🇵"), "🇯🇵  ");
    }
}
