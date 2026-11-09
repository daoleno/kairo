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
//! goes both ways; the page's title and status go to the chrome.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use kairo_core::tokio::sync::watch;

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

/// What the chrome shows of the app.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Status {
    /// "Running", "Stopped", …
    pub core: String,
    /// The core's tone: 0 neutral, 1 good, 2 busy, 3 failed.
    pub tone: u8,
    pub mode: String,
    pub up: u64,
    pub down: u64,
    pub connections: usize,
    pub profiles: usize,
}

pub struct Bridge {
    pub page: watch::Sender<Page>,
    pub status: watch::Sender<Status>,
}

pub fn bridge() -> &'static Bridge {
    static BRIDGE: OnceLock<Bridge> = OnceLock::new();
    BRIDGE.get_or_init(|| Bridge {
        page: watch::channel(Page::Overview).0,
        status: watch::channel(Status::default()).0,
    })
}

/// Selects `page` from the chrome (a sidebar click, a menu).
pub fn select(page: Page) {
    bridge().page.send_if_modified(|p| std::mem::replace(p, page) != page);
}
