//! The app's settings, kept in `settings.json`.

use serde::{Deserialize, Serialize};

use crate::paths::{Paths, write_atomic};

/// How the app looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// mihomo's routing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Rule,
    Global,
    Direct,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Rule, Mode::Global, Mode::Direct];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Rule => "rule",
            Mode::Global => "global",
            Mode::Direct => "direct",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Mode::Rule => "Rule",
            Mode::Global => "Global",
            Mode::Direct => "Direct",
        }
    }

    /// The route's name in Kairo's words.
    pub fn route(self) -> &'static str {
        match self {
            Mode::Rule => "Smart",
            Mode::Global => "All Traffic",
            Mode::Direct => "Direct",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.to_ascii_lowercase().as_str() {
            "rule" => Some(Mode::Rule),
            "global" => Some(Mode::Global),
            "direct" => Some(Mode::Direct),
            _ => None,
        }
    }
}

/// The core's log level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warning,
    Error,
    Silent,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warning => "warning",
            LogLevel::Error => "error",
            LogLevel::Silent => "silent",
        }
    }
}

/// Every setting. Missing keys in the file take their defaults, so old files keep
/// working as settings are added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// The HTTP + SOCKS port the system proxy points at.
    pub mixed_port: u16,
    /// The external controller's port (always on 127.0.0.1).
    pub controller_port: u16,
    /// The controller's secret; generated on first run.
    pub secret: String,
    /// Let other devices on the LAN use the proxy.
    pub allow_lan: bool,
    pub ipv6: bool,
    pub mode: Mode,
    pub log_level: LogLevel,
    pub system_proxy: bool,
    /// Hosts that bypass the system proxy.
    pub bypass: Vec<String>,
    pub tun: bool,
    /// The TUN stack: `mixed`, `gvisor` or `system`.
    pub tun_stack: String,
    pub theme: Theme,
    pub launch_at_login: bool,
    pub start_minimized: bool,
    /// Start the core when the app starts.
    pub auto_start_core: bool,
    /// The URL delay tests use.
    pub test_url: String,
    pub test_timeout_ms: u32,
    /// The proxy group the tray offers for quick switching (empty: the first selector).
    pub tray_group: String,
    /// Keep running in the tray when the window is closed.
    pub close_to_tray: bool,
    /// A core binary of the user's own; otherwise Kairo's downloaded one.
    pub core_path: Option<String>,
    /// Kairo keeps the location on the fastest place (tests on connect and
    /// every few minutes); picking a place by hand turns it off.
    pub auto_location: bool,
    /// The accent (`graphite`, `ember`, `iris`, `sage`...; empty: Kairo's).
    pub accent: String,
    /// AI services on one exit, local models off the proxy, domestic AI direct.
    pub ai: crate::ai::AiSettings,
    /// Downloads on an exit of their own: pinned, fastest, mirror.
    pub downloads: crate::downloads::DownloadsSettings,
    /// The person's choices for single apps and sites ("Apps & Sites").
    pub routes: Vec<crate::routes::Route>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mixed_port: 7897,
            controller_port: 9097,
            secret: String::new(),
            allow_lan: false,
            ipv6: false,
            mode: Mode::Rule,
            log_level: LogLevel::Info,
            // Connecting sets it: what a person means by "connected".
            system_proxy: true,
            bypass: default_bypass(),
            // Android has no other way to take apps in than the VPN.
            tun: cfg!(target_os = "android"),
            tun_stack: "mixed".into(),
            theme: Theme::System,
            launch_at_login: false,
            start_minimized: false,
            auto_start_core: true,
            test_url: "https://www.gstatic.com/generate_204".into(),
            test_timeout_ms: 5000,
            tray_group: String::new(),
            close_to_tray: true,
            core_path: None,
            auto_location: true,
            accent: String::new(),
            ai: crate::ai::AiSettings::default(),
            downloads: crate::downloads::DownloadsSettings::default(),
            routes: Vec::new(),
        }
    }
}

/// Whether something listens on a local port. A connect, not only a bind: on
/// macOS a bind to 127.0.0.1 can succeed beside a wildcard listener.
pub fn port_in_use(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
        || std::net::TcpListener::bind(addr).is_err()
}

/// `start` if it is free, else the next free port after it (not in `taken`).
pub fn free_port(start: u16, taken: &[u16]) -> u16 {
    (start..start.saturating_add(200))
        .find(|p| !taken.contains(p) && !port_in_use(*p))
        .unwrap_or(start)
}

pub fn default_bypass() -> Vec<String> {
    [
        "localhost",
        "127.0.0.1",
        "::1",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "*.local",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl Settings {
    /// Reads the settings, or the defaults (with a new secret) when there are none.
    /// On the first run the ports are the defaults or, when another program
    /// (another Clash client) holds those, the next free ones.
    pub fn load(paths: &Paths) -> Settings {
        let first_run = !paths.settings().exists();
        let bytes = std::fs::read(paths.settings()).ok();
        let mut settings = bytes
            .as_deref()
            .and_then(|bytes| match serde_json::from_slice::<Settings>(bytes) {
                Ok(s) => Some(s),
                Err(err) => {
                    log::warn!("settings.json is unreadable ({err}); using defaults");
                    None
                }
            })
            .unwrap_or_default();
        // Settings from before the automatic location (0.1): the node picked
        // there stays, until the person asks for the fastest.
        let upgraded = bytes
            .as_deref()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
            .is_some_and(|v| v.is_object() && v.get("autoLocation").is_none());
        if upgraded {
            settings.auto_location = false;
            let _ = settings.save(paths);
        }
        // Routes edited by hand are checked again; ones that cannot be rules go.
        let stored = std::mem::take(&mut settings.routes);
        settings.routes = crate::routes::sanitize(stored);
        if first_run {
            settings.mixed_port = free_port(settings.mixed_port, &[]);
            settings.controller_port = free_port(settings.controller_port, &[settings.mixed_port]);
        }
        if settings.secret.is_empty() || first_run {
            if settings.secret.is_empty() {
                settings.secret = new_secret();
            }
            let _ = settings.save(paths);
        }
        settings
    }

    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).expect("settings serialize");
        write_atomic(&paths.settings(), &bytes)
    }

    /// Problems that would stop the core from starting, in words for the user.
    pub fn validate(&self) -> Result<(), String> {
        if self.mixed_port == 0 || self.controller_port == 0 {
            return Err("Ports must be between 1 and 65535".into());
        }
        if self.mixed_port == self.controller_port {
            return Err("The proxy port and the controller port must differ".into());
        }
        if self.test_timeout_ms < 100 {
            return Err("The delay test timeout is too short".into());
        }
        url::Url::parse(&self.test_url)
            .map_err(|_| "The delay test URL is not a URL".to_string())?;
        if !["mixed", "gvisor", "system"].contains(&self.tun_stack.as_str()) {
            return Err("Unknown TUN stack".into());
        }
        Ok(())
    }
}

/// A random controller secret.
pub fn new_secret() -> String {
    use rand::Rng;
    let bytes: [u8; 16] = rand::rng().random();
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_missing_keys() {
        let s: Settings = serde_json::from_str(r#"{"mixedPort": 1234}"#).unwrap();
        assert_eq!(s.mixed_port, 1234);
        assert_eq!(s.controller_port, 9097);
        assert_eq!(s.mode, Mode::Rule);
        assert!(s.close_to_tray);
    }

    #[test]
    fn an_upgrade_keeps_the_picked_node() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        std::fs::write(
            paths.settings(),
            br#"{"mixedPort": 7897, "systemProxy": false, "secret": "s"}"#,
        )
        .unwrap();
        let s = Settings::load(&paths);
        assert!(!s.auto_location && !s.system_proxy);
        let again = Settings::load(&paths);
        assert!(!again.auto_location);
        let fresh = tempfile::tempdir().unwrap();
        assert!(Settings::load(&Paths::new(fresh.path().to_path_buf())).auto_location);
    }

    #[test]
    fn load_creates_a_secret_and_persists_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        paths.ensure().unwrap();
        let a = Settings::load(&paths);
        assert_eq!(a.secret.len(), 32);
        let b = Settings::load(&paths);
        assert_eq!(a.secret, b.secret);
    }

    #[test]
    fn validation_catches_port_clash() {
        let mut s = Settings::default();
        assert!(s.validate().is_ok());
        s.controller_port = s.mixed_port;
        assert!(s.validate().is_err());
    }

    #[test]
    fn mode_round_trip() {
        for m in Mode::ALL {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
        assert_eq!(Mode::parse("GLOBAL"), Some(Mode::Global));
        assert_eq!(Mode::parse("x"), None);
    }
}
