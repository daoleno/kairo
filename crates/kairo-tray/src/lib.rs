//! Kairo's tray / menu bar item. It shows the core's state and the speeds, and
//! offers the mode, the system proxy, TUN, quick switching of the main group's
//! node, the core's start/stop, the window and Quit.
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

/// One node of the quick-switch group.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub name: String,
    pub delay: Option<u32>,
}

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
    /// The quick-switch group: its name, members and current pick.
    pub group: Option<(String, Vec<Choice>, Option<String>)>,
}

impl Model {
    pub fn of(svc: &Service) -> Model {
        let state = svc.state().borrow().clone();
        let live = svc.live().borrow().clone();
        let proxies = svc.proxies().borrow().clone();
        let s = &state.settings;
        let group = proxies
            .groups
            .iter()
            .filter(|g| g.selectable() && g.name != "GLOBAL" && !g.hidden)
            .find(|g| s.tray_group.is_empty() || g.name == s.tray_group)
            .or_else(|| proxies.groups.iter().find(|g| g.selectable() && g.name != "GLOBAL" && !g.hidden))
            .map(|g| {
                (
                    g.name.clone(),
                    g.members.iter().map(|m| Choice { name: m.name.clone(), delay: m.delay }).collect(),
                    g.now.clone(),
                )
            });
        let status = match &state.core {
            CoreStatus::Running { .. } => format!("Running · ↑ {}  ↓ {}", kairo_core::speed(live.up), kairo_core::speed(live.down)),
            other => other.title().to_owned(),
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
            group,
        }
    }

    /// The menu label of a node: its name and last delay.
    pub fn choice_label(c: &Choice) -> String {
        match c.delay {
            Some(0) => format!("{}    timeout", c.name),
            Some(d) => format!("{}    {d} ms", c.name),
            None => c.name.clone(),
        }
    }
}

/// The actions of the menu, carried out on the service.
pub(crate) mod act {
    use super::*;

    pub fn mode(svc: &Service, m: Mode) {
        svc.fire(svc.set_mode(m));
    }
    pub fn sysproxy(svc: &Service, on: bool) {
        svc.fire(svc.set_system_proxy(on));
    }
    pub fn tun(svc: &Service, on: bool) {
        svc.fire(svc.set_tun(on));
    }
    pub fn select(svc: &Service, group: String, name: String) {
        svc.fire(svc.select(group, name));
    }
    pub fn toggle_core(svc: &Service, running: bool) {
        if running {
            let f = svc.stop_core();
            svc.fire(async move {
                f.await;
                Ok::<(), String>(())
            });
        } else {
            svc.fire(svc.start_core());
        }
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
        // The centre is the white route or the blue tile: opaque.
        let i = (16 * 32 + 16) * 4;
        assert_eq!(px[i], 255);
        let g = super::glyph_rgba(22);
        assert!(g.chunks_exact(4).any(|p| p[3] == 255));
    }

    #[test]
    fn labels() {
        use super::{Choice, Model};
        assert_eq!(Model::choice_label(&Choice { name: "a".into(), delay: Some(120) }), "a    120 ms");
        assert_eq!(Model::choice_label(&Choice { name: "a".into(), delay: Some(0) }), "a    timeout");
    }
}
