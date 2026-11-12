//! Kairo's model and engine, with no UI. Without the `engine` feature it is the
//! model alone (what the UI shows, and the protocol of `kairo serve`). The engine: it runs and supervises the mihomo core, keeps
//! profiles and settings, talks to the core's controller, and owns the system
//! proxy and TUN. The NSH app and the tray are views over [`service::Service`].

// The model: plain data and pure logic, on every target (the browser too).
pub mod activity;
pub mod ai;
pub mod ai_check;
pub mod bypass;
pub mod downloads;
pub mod geo;
pub mod icons;
pub mod model;
pub mod paths;
pub mod places;
pub mod profile;
pub mod protocol;
pub mod routes;
pub mod settings;
pub mod stats;
pub mod time;
pub mod tun;
pub mod view;
pub mod wire;

// The engine: processes, sockets, files, the system proxy and TUN.
#[cfg(all(feature = "engine", target_os = "android"))]
pub mod android;
#[cfg(feature = "engine")]
pub mod autostart;
#[cfg(feature = "engine")]
pub mod config;
#[cfg(feature = "engine")]
pub mod controller;
#[cfg(feature = "engine")]
pub mod core_bin;
#[cfg(feature = "engine")]
pub mod elevated;
#[cfg(feature = "engine")]
pub mod ports;
#[cfg(feature = "engine")]
pub mod procname;
#[cfg(feature = "engine")]
pub mod service;
#[cfg(feature = "engine")]
pub mod supervisor;
#[cfg(feature = "engine")]
pub mod sysproxy;
#[cfg(feature = "engine")]
pub mod update;
#[cfg(all(feature = "engine", windows))]
pub mod winapi;

#[cfg(feature = "engine")]
pub use service::Service;
pub use settings::{LogLevel, Mode, Settings, Theme};
pub use tokio;
pub use view::{CoreStatus, Live, Notice, Proxies, State, Task, UpdateStatus};

/// Now, in seconds since the epoch: the system clock, or the page's in a
/// browser (where `std`'s clock panics).
pub fn unix() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0) as i64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// Formats a byte count: `1.2 MB`.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

/// Formats a speed: `1.2 MB/s`.
pub fn speed(n: u64) -> String {
    format!("{}/s", bytes(n))
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_bytes() {
        assert_eq!(super::bytes(0), "0 B");
        assert_eq!(super::bytes(1023), "1023 B");
        assert_eq!(super::bytes(1536), "1.50 KB");
        assert_eq!(super::bytes(15 * 1024 * 1024), "15.0 MB");
        assert_eq!(super::speed(300 * 1024), "300 KB/s");
    }
}
