//! The window chrome the platform draws, and what crosses between it and the
//! NSH content.
//!
//! On macOS the launcher builds the window from AppKit's own parts: a split
//! view controller whose sidebar item AppKit draws in Liquid Glass, a source
//! list of the pages, and a unified toolbar. On Linux it builds a libadwaita
//! split view with header bars. NSH draws only the page content.
//!
//! The two sides run on different threads (AppKit's or GTK's main thread, and
//! NSH's engine thread), so they share `watch` channels: the selected page
//! goes both ways; the page's toolbar goes to the chrome, and what is done
//! with it comes back as events.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use kairo_core::tokio::sync::{mpsc, watch};

use crate::store::Page;

static NATIVE: AtomicBool = AtomicBool::new(false);

/// Whether the platform draws the sidebar and the toolbar. The launcher says
/// so before it loads the root; the headless test harness does not.
pub fn native() -> bool {
    NATIVE.load(Ordering::Relaxed)
}

pub fn set_native(on: bool) {
    NATIVE.store(on, Ordering::Relaxed);
}

static CONTROLS: AtomicBool = AtomicBool::new(false);

/// Whether switches and segmented controls are the platform's own (the
/// launcher registered them as native components).
pub fn native_controls() -> bool {
    CONTROLS.load(Ordering::Relaxed)
}

pub fn set_native_controls(on: bool) {
    CONTROLS.store(on, Ordering::Relaxed);
}

static GLASS_SIDEBAR: AtomicBool = AtomicBool::new(false);

/// Spike: NSH draws the sidebar as a floating glass panel.
pub fn glass_sidebar() -> bool {
    GLASS_SIDEBAR.load(Ordering::Relaxed)
}

pub fn set_glass_sidebar(on: bool) {
    GLASS_SIDEBAR.store(on, Ordering::Relaxed);
}

/// A toolbar glyph, named for each platform's own symbol set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Refresh,
    Add,
    Pause,
    Resume,
    CloseAll,
    Clear,
    Bolt,
    Sort,
    Filter,
    Restart,
    Stop,
    Start,
    Back,
    Copy,
    Sliders,
}

impl Glyph {
    /// The SF Symbol.
    pub fn sf(self) -> &'static str {
        match self {
            Glyph::Refresh => "arrow.clockwise",
            Glyph::Add => "plus",
            Glyph::Pause => "pause",
            Glyph::Resume => "play",
            Glyph::CloseAll => "xmark.circle",
            Glyph::Clear => "trash",
            Glyph::Bolt => "bolt",
            Glyph::Sort => "arrow.up.arrow.down",
            Glyph::Filter => "line.3.horizontal.decrease",
            Glyph::Restart => "arrow.counterclockwise",
            Glyph::Stop => "stop",
            Glyph::Start => "play",
            Glyph::Back => "chevron.backward",
            Glyph::Copy => "doc.on.doc",
            Glyph::Sliders => "slider.horizontal.3",
        }
    }

    /// The freedesktop icon names, best first (Adwaita's, then Breeze's).
    pub fn icon(self) -> &'static [&'static str] {
        match self {
            Glyph::Refresh => &["view-refresh-symbolic"],
            Glyph::Add => &["list-add-symbolic"],
            Glyph::Pause => &["media-playback-pause-symbolic"],
            Glyph::Resume | Glyph::Start => &["media-playback-start-symbolic"],
            Glyph::CloseAll => &["window-close-symbolic", "process-stop-symbolic"],
            Glyph::Clear => &["edit-clear-all-symbolic", "user-trash-symbolic"],
            Glyph::Bolt => &[
                "power-profile-performance-symbolic",
                "speedometer-symbolic",
                "system-run-symbolic",
            ],
            Glyph::Sort => &[
                "view-sort-ascending-symbolic",
                "view-sort-descending-symbolic",
                "view-list-symbolic",
            ],
            Glyph::Filter => &[
                "view-filter-symbolic",
                "filter-symbolic",
                "view-list-symbolic",
            ],
            Glyph::Restart => &["view-refresh-symbolic"],
            Glyph::Stop => &["media-playback-stop-symbolic", "process-stop-symbolic"],
            Glyph::Back => &["go-previous-symbolic"],
            Glyph::Copy => &["edit-copy-symbolic"],
            Glyph::Sliders => &["preferences-system-symbolic", "emblem-system-symbolic"],
        }
    }
}

/// One item of a page's toolbar, as the chrome draws it.
#[derive(Debug, Clone, PartialEq)]
pub enum Tool {
    /// A search field. `value` is what the page holds (the chrome sets it
    /// only when they differ, so typing is never interrupted).
    Search {
        id: &'static str,
        placeholder: &'static str,
        value: String,
    },
    /// A button: a glyph with a label for VoiceOver, Orca and tooltips.
    Button {
        id: &'static str,
        label: String,
        glyph: Glyph,
        enabled: bool,
    },
    /// A pull-down of choices, the selected one checked.
    Choice {
        id: &'static str,
        label: &'static str,
        glyph: Glyph,
        options: Vec<&'static str>,
        selected: usize,
    },
}

impl Tool {
    pub fn id(&self) -> &'static str {
        match self {
            Tool::Search { id, .. } | Tool::Button { id, .. } | Tool::Choice { id, .. } => id,
        }
    }
}

/// The toolbar of the page shown: its subtitle and its tools.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Toolbar {
    pub page: Option<Page>,
    pub subtitle: String,
    pub tools: Vec<Tool>,
}

/// What the chrome reports of a tool.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolEvent {
    Pressed(&'static str),
    Search(&'static str, String),
    Chose(&'static str, usize),
}

/// Where the platform's chrome floats over the content (points): the
/// toolbar at the top, the sidebar at the left. The content's background runs
/// under them; what it shows starts inside.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Insets {
    pub top: f32,
    pub left: f32,
}

pub struct Bridge {
    pub page: watch::Sender<Page>,
    pub insets: watch::Sender<Insets>,
    pub toolbar: watch::Sender<Toolbar>,
    pub events: mpsc::UnboundedSender<ToolEvent>,
    events_rx: Mutex<Option<mpsc::UnboundedReceiver<ToolEvent>>>,
}

impl Bridge {
    /// The tools' events, for the one who acts on them (the store).
    pub fn take_events(&self) -> Option<mpsc::UnboundedReceiver<ToolEvent>> {
        self.events_rx.lock().ok()?.take()
    }
}

pub fn bridge() -> &'static Bridge {
    static BRIDGE: OnceLock<Bridge> = OnceLock::new();
    BRIDGE.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        Bridge {
            page: watch::channel(Page::Overview).0,
            insets: watch::channel(Insets::default()).0,
            toolbar: watch::channel(Toolbar::default()).0,
            events: tx,
            events_rx: Mutex::new(Some(rx)),
        }
    })
}

/// Selects `page` from the chrome (a sidebar click, a menu).
pub fn select(page: Page) {
    bridge()
        .page
        .send_if_modified(|p| std::mem::replace(p, page) != page);
}

/// Reports a tool's event from the chrome.
pub fn tool_event(event: ToolEvent) {
    let _ = bridge().events.send(event);
}

/// The chrome's new insets (macOS: the content's safe area).
pub fn set_insets(insets: Insets) {
    bridge()
        .insets
        .send_if_modified(|i| std::mem::replace(i, insets) != insets);
}

// --- Kairo's own update, as the sidebar's foot shows it -----------------------------

/// Where the update is, as the one icon button at the foot draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStage {
    /// A newer Kairo exists: the arrow, ready to be pressed.
    Download,
    /// It is coming: the arrow with a ring that fills (or runs, when the size is not known).
    Downloading,
    /// It is here: the arrow on a filled disc; a press restarts into it.
    Ready,
    /// It is being put in place.
    Installing,
}

/// The one button of the update at the sidebar's foot: an icon, whose words
/// are only its tooltip and its accessibility label. `None`: nothing to offer.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateButton {
    /// "Download Kairo 0.3.2", "Downloading… 42%": for the tooltip and screen readers.
    pub label: String,
    pub enabled: bool,
    pub stage: UpdateStage,
    /// How much of the download is done (0 to 1), when it is known.
    pub progress: Option<f32>,
}

/// What the foot offers for the updater's state: the download arrow when a newer
/// Kairo exists, its progress as a ring while it comes, then the arrow on a disc
/// (restart to update).
pub fn update_button(u: &kairo_core::UpdateStatus) -> Option<UpdateButton> {
    use kairo_core::UpdateStatus as U;
    let button = |label: String, enabled: bool, stage: UpdateStage, progress: Option<f32>| {
        Some(UpdateButton {
            label,
            enabled,
            stage,
            progress,
        })
    };
    match u {
        U::Available { version } | U::Manual { version, .. } => button(
            format!("Download Kairo {version}"),
            true,
            UpdateStage::Download,
            None,
        ),
        // Android installs it where it is: no restart of ours in it.
        U::Ready { version } if cfg!(target_os = "android") => button(
            format!("Update to {version}"),
            true,
            UpdateStage::Ready,
            None,
        ),
        U::Downloading {
            progress: Some(p), ..
        } => button(
            format!("Downloading… {:.0}%", p * 100.0),
            false,
            UpdateStage::Downloading,
            Some(p.clamp(0.0, 1.0)),
        ),
        U::Downloading { .. } => {
            button("Downloading…".into(), false, UpdateStage::Downloading, None)
        }
        U::Ready { version } => button(
            format!("Restart to Update to {version}"),
            true,
            UpdateStage::Ready,
            None,
        ),
        U::Installing { .. } => button("Installing…".into(), false, UpdateStage::Installing, None),
        _ => None,
    }
}

/// The update icon as an SVG (24 points, `currentColor`): the download arrow inside a ring. The
/// ring is the whole circle thin while a download can start, an arc that fills with the
/// download's progress, or dots when the size is not known; no ring when it is ready (the
/// button's disc is the sign).
pub fn update_icon_svg(b: &UpdateButton) -> String {
    const R: f32 = 10.5;
    let circ = 2.0 * std::f32::consts::PI * R;
    let ring = match (b.stage, b.progress) {
        (UpdateStage::Ready, _) => String::new(),
        (UpdateStage::Download, _) => {
            format!("<circle cx='12' cy='12' r='{R}' stroke-width='1.5'/>")
        }
        (_, Some(p)) => format!(
            "<circle cx='12' cy='12' r='{R}' stroke-width='1.5' opacity='.25'/>\
             <circle cx='12' cy='12' r='{R}' stroke-width='2' stroke-linecap='round' \
             stroke-dasharray='{:.2} {:.2}' transform='rotate(-90 12 12)'/>",
            (p.clamp(0.0, 1.0) * circ).max(0.01),
            circ
        ),
        (_, None) => format!(
            "<circle cx='12' cy='12' r='{R}' stroke-width='1.75' stroke-linecap='round' \
             stroke-dasharray='1 4.4'/>"
        ),
    };
    format!(
        "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-linecap='round' stroke-linejoin='round'>{ring}<g stroke-width='1.75'><path d='M12 7v8.5'/><path d='M8.5 12.2l3.5 3.5 3.5-3.5'/></g></svg>"
    )
}

/// The button pressed: does what it says for the state the updater is in now.
pub fn update_pressed(svc: &kairo_core::Service) {
    use kairo_core::UpdateStatus as U;
    let rt = svc.runtime();
    match svc.snapshot().update {
        U::Available { .. } => {
            rt.spawn(svc.download_update());
        }
        U::Ready { .. } => {
            rt.spawn(svc.restart_to_update());
        }
        U::Manual { url, .. } => open_in_browser(&url),
        _ => {}
    }
}

/// Opens a web page in the desktop's browser.
pub fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        // Explorer hands the address to the default browser.
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod update_tests {
    use super::*;
    use kairo_core::UpdateStatus as U;

    #[test]
    fn the_foot_offers_an_arrow_then_a_ring_then_a_disc() {
        let v = || "0.3.1".to_owned();
        let b = |u: U| update_button(&u);
        assert_eq!(b(U::Idle), None);
        assert_eq!(b(U::Checking), None);
        assert_eq!(
            b(U::UpToDate {
                checked: "10:00".into()
            }),
            None
        );
        assert_eq!(
            b(U::Available { version: v() }),
            Some(UpdateButton {
                label: "Download Kairo 0.3.1".into(),
                enabled: true,
                stage: UpdateStage::Download,
                progress: None,
            })
        );
        assert_eq!(
            b(U::Downloading {
                version: v(),
                progress: Some(0.42)
            }),
            Some(UpdateButton {
                label: "Downloading… 42%".into(),
                enabled: false,
                stage: UpdateStage::Downloading,
                progress: Some(0.42),
            })
        );
        assert_eq!(
            b(U::Ready { version: v() }),
            Some(UpdateButton {
                label: "Restart to Update to 0.3.1".into(),
                enabled: true,
                stage: UpdateStage::Ready,
                progress: None,
            })
        );
        assert_eq!(
            b(U::Failed {
                message: "x".into()
            }),
            None
        );
    }

    #[test]
    fn the_icon_draws_the_progress_as_a_ring() {
        let of = |u: U| update_icon_svg(&update_button(&u).unwrap());
        let v = || "0.3.1".to_owned();
        // Half done: the arc is half the circle (2π·10.5 ≈ 65.97).
        let half = of(U::Downloading {
            version: v(),
            progress: Some(0.5),
        });
        assert!(half.contains("stroke-dasharray='32.99 65.97'"), "{half}");
        // Unknown size: dots; not yet started: a whole thin ring; ready: no ring (a disc is drawn around it).
        assert!(
            of(U::Downloading {
                version: v(),
                progress: None
            })
            .contains("stroke-dasharray='1 4.4'")
        );
        assert!(of(U::Available { version: v() }).contains("<circle"));
        assert!(!of(U::Ready { version: v() }).contains("<circle"));
        // A picture a data URI can carry: no characters that end it.
        assert!(!half.contains('#') && !half.contains('"'));
    }
}
