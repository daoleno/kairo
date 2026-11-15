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
pub mod i18n;
pub mod icons;
pub mod kind_check;
pub mod kinds;
pub mod model;
pub mod paths;
pub mod places;
pub mod profile;
pub mod protocol;
pub mod routes;
pub mod settings;
pub mod setup_code;
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

// The catalog: every language of `locales/`, English where a key has no other.
rust_i18n::i18n!("../../locales", fallback = "en");

#[cfg(feature = "engine")]
pub use service::Service;
pub use settings::{Language, LogLevel, Mode, Settings, Theme};
pub use tokio;
pub use view::{CoreStatus, Live, Notice, Proxies, State, Task, UpdateStatus};

/// What the engine's failure messages ask of the person, read from their text
/// in any of the catalog's languages: a message keeps the language it was made
/// in (a server's page gets it as text, the window may show another language
/// since), and the window's way out (a free port, TUN's permission) still shows.
pub mod failure {
    /// Whether `message` (any case, maybe inside a longer one) is the text of
    /// `key` in one of the catalog's languages: the words around its `%{…}`s,
    /// in order.
    pub fn says(message: &str, key: &str) -> bool {
        let message = message.to_lowercase();
        crate::_rust_i18n_available_locales().into_iter().any(|locale| {
            let text = crate::i18n::text_in(&locale, key, &[]);
            if text == format!("{locale}.{key}") {
                return false;
            }
            let text = text.to_lowercase();
            let mut rest = message.as_str();
            let mut found = false;
            for part in fixed_parts(&text) {
                match rest.find(part) {
                    Some(i) => rest = &rest[i + part.len()..],
                    None => return false,
                }
                found = true;
            }
            found
        })
    }

    /// The text between a catalog text's `%{name}`s (none empty).
    fn fixed_parts(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(i) = rest.find("%{") {
            out.push(&rest[..i]);
            rest = rest[i..].find('}').map_or("", |j| &rest[i + j + 1..]);
        }
        out.push(rest);
        out.retain(|p| !p.trim().is_empty());
        out
    }

    /// Kairo's port is taken and none is free near it (Settings' port, or a
    /// free one Kairo picks).
    pub fn port_taken(message: &str) -> bool {
        says(message, "engine.service.port_taken")
    }

    /// TUN cannot start until the core has its permission.
    pub fn tun_needs_permission(message: &str) -> bool {
        says(message, "engine.supervisor.tun_needs_permission")
            || says(message, "engine.service.tun_needs_permission")
    }
}

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

    #[test]
    fn a_failure_is_read_in_the_language_it_was_made_in() {
        use super::failure;
        use super::i18n::text_in;
        for language in ["en", "zh"] {
            let port = text_in(language, "engine.service.port_taken", &[("port", &7890), ("who", &"verge-mihomo")]);
            assert!(failure::port_taken(&port), "{port}");
            // As the window reads it: lower case.
            assert!(failure::port_taken(&port.to_lowercase()), "{port}");
            let tun = text_in(language, "engine.supervisor.tun_needs_permission", &[]);
            let start = text_in(language, "engine.service.start_failed", &[("error", &tun)]);
            assert!(failure::tun_needs_permission(&start), "{start}");
            assert!(!failure::port_taken(&tun) && !failure::tun_needs_permission(&port));
            // A config that is not YAML is read as one by the profile's reading of failures.
            for key in ["engine.config.profile_not_yaml", "engine.config.override_not_yaml"] {
                let yaml = text_in(language, key, &[("error", &"line 3")]);
                assert_eq!(super::profile::failure_of(&yaml), Some(super::profile::Failure::BadConfig), "{yaml}");
            }
        }
        assert!(!failure::port_taken("The core stopped") && !failure::port_taken(""));
    }

    #[test]
    fn no_other_engine_text_reads_as_a_failure_with_a_way_out() {
        use super::failure;
        use super::profile::{Failure, failure_of};
        let catalog = super::i18n::catalog();
        for language in ["en", "zh"] {
            for (key, text) in catalog[language].iter().filter(|(k, _)| k.starts_with("engine.")) {
                // Its arguments filled with what says nothing.
                let mut text = text.clone();
                while let Some(i) = text.find("%{") {
                    let j = text[i..].find('}').map_or(text.len(), |j| i + j + 1);
                    text.replace_range(i..j, "7890");
                }
                let key = key.as_str();
                assert_eq!(failure::port_taken(&text), key == "engine.service.port_taken", "{language} {key}: {text}");
                let tun = ["engine.supervisor.tun_needs_permission", "engine.service.tun_needs_permission"];
                assert_eq!(failure::tun_needs_permission(&text), tun.contains(&key), "{language} {key}: {text}");
                let yaml = ["engine.config.profile_not_yaml", "engine.config.override_not_yaml"];
                // A time out reads as one in both; an update's error (that its
                // server was not reached) never meets the subscription's reading.
                let expected = if yaml.contains(&key) {
                    Some(Failure::BadConfig)
                } else if key == "engine.controller.timeout" {
                    Some(Failure::Unreachable)
                } else if key == "engine.update.unreachable" {
                    failure_of(&text).filter(|f| *f == Failure::Unreachable)
                } else {
                    None
                };
                assert_eq!(failure_of(&text), expected, "{language} {key}: {text}");
            }
        }
    }
}
