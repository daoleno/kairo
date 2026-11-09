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

use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};

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
            Glyph::Bolt => &["power-profile-performance-symbolic", "speedometer-symbolic", "system-run-symbolic"],
            Glyph::Sort => &["view-sort-ascending-symbolic", "view-sort-descending-symbolic", "view-list-symbolic"],
            Glyph::Filter => &["view-filter-symbolic", "filter-symbolic", "view-list-symbolic"],
            Glyph::Restart => &["view-refresh-symbolic"],
            Glyph::Stop => &["media-playback-stop-symbolic", "process-stop-symbolic"],
        }
    }
}

/// One item of a page's toolbar, as the chrome draws it.
#[derive(Debug, Clone, PartialEq)]
pub enum Tool {
    /// A search field. `value` is what the page holds (the chrome sets it
    /// only when they differ, so typing is never interrupted).
    Search { id: &'static str, placeholder: &'static str, value: String },
    /// A button: a glyph with a label for VoiceOver, Orca and tooltips.
    Button { id: &'static str, label: String, glyph: Glyph, enabled: bool },
    /// A pull-down of choices, the selected one checked.
    Choice { id: &'static str, label: &'static str, glyph: Glyph, options: Vec<&'static str>, selected: usize },
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

pub struct Bridge {
    pub page: watch::Sender<Page>,
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
            toolbar: watch::channel(Toolbar::default()).0,
            events: tx,
            events_rx: Mutex::new(Some(rx)),
        }
    })
}

/// Selects `page` from the chrome (a sidebar click, a menu).
pub fn select(page: Page) {
    bridge().page.send_if_modified(|p| std::mem::replace(p, page) != page);
}

/// Reports a tool's event from the chrome.
pub fn tool_event(event: ToolEvent) {
    let _ = bridge().events.send(event);
}
