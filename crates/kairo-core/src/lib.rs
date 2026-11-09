//! Kairo's engine, with no UI: it runs and supervises the mihomo core, keeps
//! profiles and settings, talks to the core's controller, and owns the system
//! proxy and TUN. The NSH app and the tray are views over [`service::Service`].

pub mod autostart;
pub mod config;
pub mod controller;
pub mod core_bin;
pub mod geo;
pub mod model;
pub mod paths;
pub mod places;
pub mod ports;
pub mod profile;
pub mod service;
pub mod settings;
pub mod supervisor;
pub mod sysproxy;
pub mod tun;
pub mod update;

pub use service::{CoreStatus, Live, Notice, Proxies, Service, State, Task};
pub use settings::{LogLevel, Mode, Settings, Theme};
pub use tokio;
pub use update::UpdateStatus;

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
