//! Kairo's tray / menu bar item: whether it is connected and through where,
//! and the three everyday choices without the window: Connect, the location
//! (Fastest or a place, quickest first) and the route. Then the window and
//! Quit. The system proxy and TUN live in Settings (Connect sets the proxy).
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
    /// Quits the app (after restoring the system proxy).
    pub quit: Arc<dyn Fn() + Send + Sync>,
}

/// A location in the menu: `name` empty is Fastest (automatic).
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub name: String,
    pub label: String,
    pub delay: Option<u32>,
}

/// How many places the menu lists (the rest: All Locations… in the window).
pub const PLACES: usize = 15;

/// What the menu shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Model {
    pub status: String,
    pub running: bool,
    pub mode: Mode,
    pub sysproxy: bool,
    pub sysproxy_supported: bool,
    pub tun: bool,
    pub tun_allowed: bool,
    pub up: u64,
    pub down: u64,
    pub has_profile: bool,
    /// The location: its group, the choices (Fastest first) and the one in use
    /// (empty: Fastest).
    pub group: Option<(String, Vec<Choice>, Option<String>)>,
    /// The location's title for the submenu ("Fastest · Tokyo 03").
    pub place: String,
}

impl Model {
    pub fn of(svc: &Service) -> Model {
        let state = svc.state().borrow().clone();
        let live = svc.live().borrow().clone();
        let proxies = svc.proxies().borrow().clone();
        let s = &state.settings;
        let main = kairo_core::places::main_group(&proxies.groups, &s.tray_group);
        let now_place = main.and_then(|g| kairo_core::places::current(&proxies.groups, g));
        let label_of = |p: &kairo_core::places::Place| format!("{}{}", p.flag.clone().map(|f| f + "  ").unwrap_or_default(), p.title);
        let group = main.map(|g| {
            let mut places = kairo_core::places::places(&proxies.groups, g);
            places.sort_by_key(|p| match p.delay {
                Some(d) if d > 0 => d,
                _ => u32::MAX,
            });
            let mut choices = vec![Choice { name: String::new(), label: "Fastest".into(), delay: None }];
            choices.extend(places.into_iter().take(PLACES).map(|p| Choice { label: label_of(&p), name: p.name, delay: p.delay }));
            let now = if s.auto_location { Some(String::new()) } else { now_place.as_ref().map(|p| p.name.clone()) };
            (g.name.clone(), choices, now)
        });
        let place = match (&now_place, s.auto_location) {
            (Some(p), true) => format!("Fastest · {}", label_of(p)),
            (Some(p), false) => label_of(p),
            (None, _) => "—".into(),
        };
        let status = match &state.core {
            CoreStatus::Running { .. } => match &now_place {
                Some(p) => format!(
                    "Connected · {}{}",
                    label_of(p),
                    p.delay.filter(|d| *d > 0).map(|d| format!(" · {d} ms")).unwrap_or_default()
                ),
                None => "Connected".into(),
            },
            CoreStatus::Starting => "Connecting…".into(),
            CoreStatus::Failed { .. } => "Couldn't connect".into(),
            _ => "Not Connected".into(),
        };
        Model {
            status,
            running: state.core.is_running(),
            mode: s.mode,
            sysproxy: s.system_proxy,
            sysproxy_supported: state.sysproxy_supported,
            tun: s.tun,
            tun_allowed: state.tun_privilege == Some(kairo_core::tun::Privilege::Granted),
            up: live.up,
            down: live.down,
            has_profile: state.profiles.active.is_some(),
            group,
            place,
        }
    }

    /// The menu label of a location: its name and last delay.
    pub fn choice_label(c: &Choice) -> String {
        match c.delay {
            Some(0) => format!("{}    no answer", c.label),
            Some(d) => format!("{}    {d} ms", c.label),
            None => c.label.clone(),
        }
    }

    /// The route's title (the mode in plain words).
    pub fn route_title(m: Mode) -> &'static str {
        match m {
            Mode::Rule => "Smart Route",
            Mode::Global => "All Traffic",
            Mode::Direct => "Direct",
        }
    }
}

/// The actions of the menu, carried out on the service.
pub(crate) mod act {
    use super::*;

    pub fn mode(svc: &Service, m: Mode) {
        svc.fire(svc.set_mode(m));
    }
    /// A location (empty: Fastest).
    pub fn locate(svc: &Service, name: String) {
        svc.fire(svc.set_location((!name.is_empty()).then_some(name)));
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
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("the bundled svg parses");
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let s = tree.size();
    let scale = size as f32 / s.width().max(s.height());
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().expect("png")
}

/// Renders an SVG to straight (not premultiplied) RGBA.
pub fn render(svg: &str, size: u32) -> Vec<u8> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("the bundled svg parses");
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let s = tree.size();
    let scale = size as f32 / s.width().max(s.height());
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
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
        let dim = super::render_png(&include_str!("../../../assets/glyph.svg").replacen("<svg ", "<svg opacity=\"0.45\" ", 1), 36);
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
        assert_eq!(Model::choice_label(&Choice { name: "x".into(), label: "a".into(), delay: Some(120) }), "a    120 ms");
        assert_eq!(Model::choice_label(&Choice { name: "x".into(), label: "a".into(), delay: Some(0) }), "a    no answer");
    }
}
