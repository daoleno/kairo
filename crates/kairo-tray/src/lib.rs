//! Kairo's tray / menu bar item: whether it is connected, how the AI services
//! stand, a submenu for each kind of app in use (AI, Streaming, Games,
//! Downloads, Dev tools, Everything else) with a country each, the mode, and
//! the window, Settings and Quit.
//!
//! - Linux: a StatusNotifierItem (KDE Plasma, GNOME with the AppIndicator
//!   extension, most panels) through `ksni`.
//! - macOS: an `NSStatusItem` with an `NSMenu`.
//! - Windows: a notification-area icon (`Shell_NotifyIconW`) with a popup menu.

use std::sync::Arc;

use kairo_core::{CoreStatus, Mode, Service};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// What the app does for the tray.
#[derive(Clone)]
pub struct Hooks {
    /// Shows and focuses the main window.
    pub show: Arc<dyn Fn() + Send + Sync>,
    /// Shows the window on a page (`"apps"`, `"settings"`).
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
    /// Good, OK, Slow or "" (what [`health_word`] gives: shown through
    /// [`health_label`]).
    pub health: &'static str,
    pub checked: bool,
}

/// Whose exit a submenu chooses: a kind of app (Everything else is the
/// Location).
pub type ExitId = kairo_core::kinds::Kind;

/// The platform menus' ids for an exit's items: `EXIT_IDS` + 100 × the
/// kind's place in [`Kind::ALL`](kairo_core::kinds::Kind::ALL) + the
/// country's place, 99 for Fastest.
pub const EXIT_IDS: usize = 1000;

/// The id of a country (`Some(i)`) or of Fastest (`None`) in `kind`'s submenu.
pub fn exit_item(kind: ExitId, choice: Option<usize>) -> usize {
    let k = ExitId::ALL.iter().position(|x| *x == kind).unwrap_or(0);
    EXIT_IDS + 100 * k + choice.unwrap_or(99).min(99)
}

/// What an id of [`exit_item`] stands for.
pub fn exit_of_item(id: usize) -> Option<(ExitId, Option<usize>)> {
    let rel = id.checked_sub(EXIT_IDS)?;
    let kind = *ExitId::ALL.get(rel / 100)?;
    let slot = rel % 100;
    Some((kind, (slot != 99).then_some(slot)))
}

/// One exit's submenu.
#[derive(Debug, Clone, PartialEq)]
pub struct ExitMenu {
    pub id: ExitId,
    pub title: String,
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
    /// The language its words are in (another one is another menu).
    pub language: String,
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

/// A flag emoji is two regional-indicator letters, which Windows draws as
/// letters ("JP"): its menu leaves them out.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn without_flags(text: &str) -> String {
    text.chars()
        .filter(|c| !('\u{1F1E6}'..='\u{1F1FF}').contains(c))
        .collect::<String>()
        .trim_start()
        .to_owned()
}

/// How good a delay is: "Good", "OK", "Slow" or "" (a value; a menu shows
/// it through [`health_label`]).
pub fn health_word(delay: Option<u32>) -> &'static str {
    match delay {
        None => "",
        Some(0) => "Slow",
        Some(d) if d < 200 => "Good",
        Some(d) if d < 500 => "OK",
        Some(_) => "Slow",
    }
}

/// A [`health_word`] in the language shown.
pub fn health_label(health: &str) -> String {
    match health {
        "Good" => kairo_core::tr!("tray.health.good"),
        "OK" => kairo_core::tr!("tray.health.ok"),
        "Slow" => kairo_core::tr!("tray.health.slow"),
        other => other.to_owned(),
    }
}

/// The AI line: what the last check found, in a few words.
pub fn ai_line(state: &kairo_core::State) -> String {
    use kairo_core::ai_check::{self, Api};
    let checks = &state.ai.checks;
    if state.settings.kinds_off || checks.is_empty() {
        return String::new();
    }
    let odd = ai_check::odd_ones(checks);
    let problems = checks
        .iter()
        .filter(|c| {
            !c.reachable
                || matches!(c.api, Api::Unexpected { .. } | Api::Unreachable { .. })
                || odd
                    .as_ref()
                    .is_some_and(|(_, ids)| ids.contains(&c.service))
        })
        .count()
        + state
            .bypass
            .iter()
            .map(|b| b.tool.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
    if problems > 0 {
        return if problems == 1 {
            kairo_core::tr!("tray.ai.problem", count = problems)
        } else {
            kairo_core::tr!("tray.ai.problems", count = problems)
        };
    }
    let codes: std::collections::BTreeSet<&str> =
        checks.iter().filter_map(|c| c.country.code()).collect();
    match (codes.len(), codes.iter().next()) {
        (1, Some(code)) => kairo_core::tr!(
            "tray.ai.all_see",
            count = checks.len(),
            country = ai_check::country_name(code)
        ),
        _ => kairo_core::tr!("tray.ai.reachable", count = checks.len()),
    }
}

/// The menu's first line: the connection as a person says it. With no
/// subscription it is "Not Set Up" whatever the core does (it runs, all
/// direct), as Home says; never "Connected".
pub fn status_of(core: &CoreStatus, has_profile: bool) -> String {
    if !has_profile {
        return kairo_core::tr!("model.view.not_set_up");
    }
    match core {
        CoreStatus::Running { .. } => kairo_core::tr!("tray.status.connected"),
        CoreStatus::Starting => kairo_core::tr!("tray.status.connecting"),
        CoreStatus::Failed { .. } => kairo_core::tr!("tray.status.failed"),
        _ => kairo_core::tr!("tray.status.off"),
    }
}

impl Model {
    /// Connected as a person means it: the core runs and there is a
    /// subscription to go through (the tick, the bright icon, the tooltip).
    pub fn connected(&self) -> bool {
        self.running && self.has_profile
    }

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
                if p.region == "Other" {
                    p.title.clone()
                } else {
                    places::region_title(p.region)
                }
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
                            label: format!("{}{}", menu_flag(&flag), places::region_title(region)),
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
                id: ExitId::Other,
                title: kairo_core::tr!("tray.exit.rest"),
                place: rest_place
                    .as_ref()
                    .map(label_of)
                    .unwrap_or_else(|| "—".into()),
                fastest: Some(s.auto_location),
                choices: countries(if s.auto_location {
                    None
                } else {
                    rest_place.as_ref()
                }),
            });
            if !s.kinds_off && !state.ai.group.is_empty() {
                exits.push(ExitMenu {
                    id: ExitId::Ai,
                    title: kairo_core::tr!("tray.exit.ai"),
                    place: ai_place
                        .as_ref()
                        .map(label_of)
                        .unwrap_or_else(|| "—".into()),
                    fastest: None,
                    choices: countries(ai_place.as_ref()),
                });
            }
            // The other kinds, those with an app in use.
            if !s.kinds_off {
                let used: std::collections::BTreeSet<ExitId> = svc
                    .apps_in_use()
                    .iter()
                    .filter_map(|id| {
                        s.apps
                            .iter()
                            .find(|a| a.id == *id)
                            .map(|a| a.kind)
                            .or_else(|| kairo_core::kinds::catalog_app(id).map(|a| a.kind))
                            .or_else(|| id.starts_with("game:").then_some(ExitId::Games))
                    })
                    .collect();
                for kind in ExitId::ROUTED.into_iter().filter(|k| used.contains(k)) {
                    let Some(exit) = s.kinds.exit(kind) else { continue };
                    let place = if exit.subscription {
                        None
                    } else {
                        svc.kind_node(kind).and_then(|n| {
                            proxies
                                .groups
                                .iter()
                                .flat_map(|g| g.members.iter())
                                .find(|m| m.name == n && !m.is_group)
                                .map(places::place)
                        })
                    };
                    let fastest = !exit.subscription && exit.exit.is_none() && exit.fastest;
                    exits.push(ExitMenu {
                        id: kind,
                        title: kind.title(),
                        place: if exit.subscription {
                            kairo_core::tr!("tray.exit.subscription")
                        } else {
                            place.as_ref().map(label_of).unwrap_or_else(|| "—".into())
                        },
                        fastest: Some(fastest),
                        choices: countries(if fastest { None } else { place.as_ref() }),
                    });
                }
            }
        }
        Model {
            status: status_of(&state.core, state.profiles.active.is_some()),
            running: state.core.is_running(),
            mode: s.mode,
            up: live.up,
            down: live.down,
            has_profile: state.profiles.active.is_some(),
            ai: if state.core.is_running() {
                ai_line(&state)
            } else {
                String::new()
            },
            exits,
            language: kairo_core::i18n::language(),
        }
    }

    /// The menu label of a country: its name and how good it is.
    pub fn choice_label(c: &Choice) -> String {
        if c.health.is_empty() {
            c.label.clone()
        } else {
            format!("{}    {}", c.label, health_label(c.health))
        }
    }

    /// The mode in plain words.
    pub fn route_title(m: Mode) -> String {
        kairo_core::tr!(match m {
            Mode::Rule => "tray.mode.rule",
            Mode::Global => "tray.mode.global",
            Mode::Direct => "tray.mode.direct",
        })
    }

    /// The line of an exit's submenu: the exit and where it goes now.
    pub fn exit_line(e: &ExitMenu) -> String {
        kairo_core::tr!("tray.exit.line", exit = e.title, place = e.place)
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
    /// A kind's exit: a server.
    pub fn pick(svc: &Service, kind: ExitId, name: String) {
        match kind {
            ExitId::Other => locate(svc, name),
            ExitId::Ai => ai_exit(svc, name),
            k => svc.fire(svc.set_kind_exit(k, name)),
        }
    }
    /// A kind's exit back on the fastest server.
    pub fn fastest(svc: &Service, kind: ExitId) {
        match kind {
            ExitId::Other => locate(svc, String::new()),
            ExitId::Ai => {}
            k => svc.fire(svc.kind_follow_fastest(k)),
        }
    }
    /// An item of an exit's submenu (see [`exit_item`]): Windows and macOS
    /// number their items; Linux's menu calls back each item's own action.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub fn exit_item(svc: &Service, model: &Model, id: usize) -> bool {
        let Some((kind, slot)) = exit_of_item(id) else {
            return false;
        };
        match slot {
            None => fastest(svc, kind),
            Some(i) => {
                let name = model
                    .exits
                    .iter()
                    .find(|e| e.id == kind)
                    .and_then(|e| e.choices.get(i))
                    .map(|c| c.name.clone());
                if let Some(name) = name {
                    pick(svc, kind, name);
                }
            }
        }
        true
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
    #[cfg(windows)]
    {
        windows::start(svc, hooks)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (svc, hooks);
        Err("no tray on this platform yet".into())
    }
}

/// Takes the tray icon away (Windows keeps a ghost of it until the pointer
/// passes over otherwise). A no-op elsewhere.
pub fn stop() {
    #[cfg(windows)]
    windows::stop();
}

#[cfg(test)]
mod tests {
    /// With no subscription the core still runs (all direct), but the menu
    /// says what Home says, "Not Set Up", with no tick and a dim icon.
    #[test]
    fn no_subscription_is_not_set_up_never_connected() {
        use super::{Model, status_of};
        use kairo_core::CoreStatus;
        let running = CoreStatus::Running { version: "v1".into() };
        let t = |k: &str| kairo_core::i18n::translate(k, &[]);
        assert_eq!(status_of(&running, false), t("model.view.not_set_up"));
        assert_eq!(status_of(&CoreStatus::Stopped, false), t("model.view.not_set_up"));
        assert_eq!(status_of(&running, true), t("tray.status.connected"));
        assert_eq!(status_of(&CoreStatus::Starting, true), t("tray.status.connecting"));
        assert_eq!(status_of(&CoreStatus::Failed { message: "x".into() }, true), t("tray.status.failed"));
        assert_eq!(status_of(&CoreStatus::Stopped, true), t("tray.status.off"));
        let m = |running, has_profile| Model { running, has_profile, ..Default::default() };
        assert!(!m(true, false).connected());
        assert!(m(true, true).connected());
        assert!(!m(false, true).connected());
        // Both languages have the words.
        for lang in ["en", "zh"] {
            let said = kairo_core::i18n::text_in(lang, "model.view.not_set_up", &[]);
            assert!(!said.is_empty() && said != "model.view.not_set_up", "{lang}");
        }
        assert_eq!(kairo_core::i18n::text_in("zh", "model.view.not_set_up", &[]), "未设置");
    }

    #[test]
    fn flags_are_left_out_of_menu_text() {
        assert_eq!(super::without_flags("🇯🇵  Tokyo 01"), "Tokyo 01");
        assert_eq!(super::without_flags("Tokyo"), "Tokyo");
    }

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
