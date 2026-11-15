//! The app's settings, kept in `settings.json`.

use serde::{Deserialize, Serialize};

#[cfg(feature = "engine")]
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

/// The language Kairo shows: the system's, or one picked in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Language {
    #[serde(rename = "en")]
    English,
    /// Simplified Chinese.
    #[serde(rename = "zh-Hans")]
    Chinese,
    /// The system's (the first of the person's languages Kairo has, else
    /// English); also what a value this Kairo does not know reads as.
    #[default]
    #[serde(rename = "system", other)]
    System,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::System, Language::English, Language::Chinese];

    /// The BCP 47 tag picked; none for the system's.
    pub fn tag(self) -> Option<&'static str> {
        match self {
            Language::System => None,
            Language::English => Some("en"),
            Language::Chinese => Some("zh-Hans"),
        }
    }
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

    pub fn title(self) -> String {
        match self {
            Mode::Rule => crate::tr!("model.settings.mode.rule"),
            Mode::Global => crate::tr!("model.settings.mode.global"),
            Mode::Direct => crate::tr!("model.settings.mode.direct"),
        }
    }

    /// The route's name in Kairo's words.
    pub fn route(self) -> String {
        match self {
            Mode::Rule => crate::tr!("model.settings.route.smart"),
            Mode::Global => crate::tr!("model.settings.route.all"),
            Mode::Direct => crate::tr!("model.settings.route.direct"),
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
    /// The language of Kairo's text (a value it does not know reads as the system's).
    pub language: Language,
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
    /// The settings file's form: 2 since apps by kind (1 had `ai`,
    /// `downloads` and `routes`, moved once by [`migrate_v1`]).
    pub schema: u32,
    /// Apps are not routed by kind: only the subscription's rules, and the apps the person gave an exit of their own.
    pub kinds_off: bool,
    /// Every kind's exit.
    pub kinds: crate::kinds::Kinds,
    /// The person's choices for single apps, and their own apps.
    pub apps: Vec<crate::kinds::AppChoice>,
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
            language: Language::System,
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
            schema: SCHEMA,
            kinds_off: false,
            kinds: crate::kinds::Kinds::default(),
            apps: Vec::new(),
        }
    }
}

/// Whether something listens on a local port. A connect, not only a bind: on
/// macOS a bind to 127.0.0.1 can succeed beside a wildcard listener.
#[cfg(feature = "engine")]
pub fn port_in_use(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
        || std::net::TcpListener::bind(addr).is_err()
}

/// `start` if it is free, else the next free port after it (not in `taken`).
#[cfg(feature = "engine")]
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
    #[cfg(feature = "engine")]
    pub fn load(paths: &Paths) -> Settings {
        let first_run = !paths.settings().exists();
        let bytes = std::fs::read(paths.settings()).ok();
        let mut migrated = false;
        let mut settings = bytes
            .as_deref()
            .and_then(|bytes| match serde_json::from_slice::<serde_json::Value>(bytes) {
                Ok(v) => match Settings::from_value(v) {
                    Ok((s, m)) => {
                        migrated = m;
                        Some(s)
                    }
                    Err(err) => {
                        log::warn!("settings.json is unreadable ({err}); using defaults");
                        None
                    }
                },
                Err(err) => {
                    log::warn!("settings.json is unreadable ({err}); using defaults");
                    None
                }
            })
            .unwrap_or_default();
        if migrated {
            log::info!("settings moved to apps by kind (schema {SCHEMA})");
            let _ = settings.save(paths);
        }
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
        // Choices edited by hand are checked again; ones that cannot be rules go.
        let stored = std::mem::take(&mut settings.apps);
        settings.apps = crate::kinds::sanitize(stored);
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

    /// The settings in a file's JSON, moved to the current schema when the
    /// file is older (`true`: it was, and should be written back).
    pub fn from_value(mut v: serde_json::Value) -> Result<(Settings, bool), serde_json::Error> {
        let old = v.is_object() && v.get("schema").and_then(|s| s.as_u64()).unwrap_or(1) < u64::from(SCHEMA);
        let moved = old.then(|| migrate_v1(&v));
        if let Some(o) = v.as_object_mut() {
            for k in ["ai", "downloads", "routes"] {
                o.remove(k);
            }
        }
        let mut s: Settings = serde_json::from_value(v)?;
        if let Some((rules_only, kinds, apps)) = moved {
            s.schema = SCHEMA;
            s.kinds_off = rules_only;
            s.kinds = kinds;
            s.apps = apps;
        }
        Ok((s, old))
    }

    #[cfg(feature = "engine")]
    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).expect("settings serialize");
        write_atomic(&paths.settings(), &bytes)
    }

    /// Problems that would stop the core from starting, in words for the user.
    pub fn validate(&self) -> Result<(), String> {
        if self.mixed_port == 0 || self.controller_port == 0 {
            return Err(crate::tr!("model.settings.error.ports"));
        }
        if self.mixed_port == self.controller_port {
            return Err(crate::tr!("model.settings.error.same_port"));
        }
        if self.test_timeout_ms < 100 {
            return Err(crate::tr!("model.settings.error.timeout"));
        }
        url::Url::parse(&self.test_url)
            .map_err(|_| crate::tr!("model.settings.error.test_url"))?;
        if !["mixed", "gvisor", "system"].contains(&self.tun_stack.as_str()) {
            return Err(crate::tr!("model.settings.error.tun_stack"));
        }
        Ok(())
    }
}

/// The settings file's current form.
pub const SCHEMA: u32 = 2;

/// Settings from before apps by kind (schema 1): the AI layer's switches,
/// the Downloads exit and the "Apps & Sites" routes, moved once to the kinds
/// and the apps. Nothing changes where traffic goes:
///
/// - the AI layer off is "the subscription's rules only";
/// - the AI exit (pinned, or following the Location) is the AI kind's;
/// - domestic AI not direct: those apps go as the subscription routes them;
/// - downloads off: the Downloads kind goes as the subscription routes it;
///   its pinned node, "fastest" and the mirror carry over;
/// - a route for a site or program the catalog knows (exactly one of its
///   domains, or one of its programs) is that app's exit; any other is an
///   app of the person's (kind Everything else) with that exit;
/// - a route's exit: Direct, Block, Everything else and a server stay; "AI
///   exit" and "Downloads" are that kind's exit ("Same as AI" for an app of
///   another kind).
pub fn migrate_v1(
    v: &serde_json::Value,
) -> (bool, crate::kinds::Kinds, Vec<crate::kinds::AppChoice>) {
    use crate::kinds::{AppChoice, AppExit, Kind, KindExit, Kinds, catalog, catalog_app, upsert};
    use crate::routes::{RouteExit, RouteKind};
    let ai: crate::ai::AiSettings = v
        .get("ai")
        .and_then(|a| serde_json::from_value(a.clone()).ok())
        .unwrap_or_default();
    let dl: crate::downloads::DownloadsSettings = v
        .get("downloads")
        .and_then(|a| serde_json::from_value(a.clone()).ok())
        .unwrap_or_default();
    let routes: Vec<crate::routes::Route> = v
        .get("routes")
        .and_then(|a| serde_json::from_value(a.clone()).ok())
        .map(crate::routes::sanitize)
        .unwrap_or_default();
    let kinds = Kinds {
        ai: crate::kinds::AiKind {
            exit: ai.exit.clone(),
            follow: ai.follow,
        },
        downloads: KindExit {
            subscription: !ai.downloads,
            exit: dl.exit.clone(),
            fastest: dl.fastest,
        },
        mirror: dl.mirror,
        ..Kinds::default()
    };
    let mut apps: Vec<AppChoice> = Vec::new();
    if !ai.domestic_direct {
        for a in catalog().iter().filter(|a| a.direct) {
            upsert(
                &mut apps,
                AppChoice { id: a.id.into(), kind: a.kind, matcher: None, exit: AppExit::Subscription },
            );
        }
    }
    for r in routes {
        // The catalog app the route names exactly, if any.
        let known = match &r.kind {
            RouteKind::Site { domain } => catalog().iter().find(|a| a.suffixes.contains(&domain.as_str())),
            RouteKind::App { name } => catalog()
                .iter()
                .find(|a| a.programs.iter().any(|p| p.eq_ignore_ascii_case(name))),
        };
        let kind = known.map(|a| a.kind).unwrap_or(Kind::Other);
        let exit = match r.exit {
            RouteExit::Usual => continue,
            RouteExit::Direct => AppExit::Direct,
            RouteExit::Block => AppExit::Block,
            RouteExit::Everything => AppExit::Everything,
            RouteExit::Node(n) => AppExit::Node(n),
            RouteExit::Ai if kind == Kind::Ai => AppExit::Kind,
            RouteExit::Ai => AppExit::Of(Kind::Ai),
            RouteExit::Downloads if kind == Kind::Downloads => AppExit::Kind,
            RouteExit::Downloads => AppExit::Of(Kind::Downloads),
        };
        let choice = match known {
            Some(a) => AppChoice { id: a.id.into(), kind, matcher: None, exit },
            None => match AppChoice::custom(r.kind.clone(), kind, exit) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("route {} is not moved: {e}", r.id);
                    continue;
                }
            },
        };
        // A catalog app back on its defaults (an AI site on the AI exit) needs no choice.
        if choice.matcher.is_none() && catalog_app(&choice.id).is_some() && choice.is_default() {
            apps.retain(|a| a.id != choice.id);
            continue;
        }
        upsert(&mut apps, choice);
    }
    (!ai.enabled, kinds, apps)
}

/// A random controller secret.
#[cfg(feature = "engine")]
pub fn new_secret() -> String {
    use rand::Rng;
    let bytes: [u8; 16] = rand::rng().random();
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_tags_and_an_unknown_one_is_the_systems() {
        let s: Settings = serde_json::from_str(r#"{"language": "zh-Hans"}"#).unwrap();
        assert_eq!(s.language, Language::Chinese);
        let s: Settings = serde_json::from_str(r#"{"language": "ja"}"#).unwrap();
        assert_eq!(s.language, Language::System);
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.language, Language::System);
        assert_eq!(serde_json::to_value(Language::English).unwrap(), "en");
    }

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

    /// A schema 1 file with everything the AI layer, the Downloads exit and
    /// "Apps & Sites" kept, moved once.
    const V1: &str = r#"{
      "mixedPort": 7897, "secret": "s", "autoLocation": true,
      "ai": {"enabled": true, "exit": {"name": "🇯🇵 Tokyo 02", "fingerprint": "ab", "since": 5},
             "follow": false, "domesticDirect": false, "downloads": true},
      "downloads": {"exit": {"name": "🇸🇬 Singapore 01", "fingerprint": "cd", "since": 6}, "fastest": false, "mirror": true},
      "routes": [
        {"id": "site:netflix.com", "kind": {"site": {"domain": "netflix.com"}}, "exit": {"node": "🇺🇸 Los Angeles 05"}},
        {"id": "app:steam", "kind": {"app": {"name": "steam"}}, "exit": "direct"},
        {"id": "app:yt-dlp", "kind": {"app": {"name": "yt-dlp"}}, "exit": "ai"},
        {"id": "site:example.com", "kind": {"site": {"domain": "example.com"}}, "exit": "downloads"},
        {"id": "site:doubleclick.net", "kind": {"site": {"domain": "doubleclick.net"}}, "exit": "block"},
        {"id": "site:claude.ai", "kind": {"site": {"domain": "claude.ai"}}, "exit": "ai"},
        {"id": "site:github.com", "kind": {"site": {"domain": "github.com"}}, "exit": "everything"}
      ]
    }"#;

    #[test]
    fn schema_1_moves_once_to_kinds_and_apps() {
        use crate::kinds::{AppExit, Kind};
        use crate::routes::RouteKind;
        let (s, moved) = Settings::from_value(serde_json::from_str(V1).unwrap()).unwrap();
        assert!(moved);
        assert_eq!(s.schema, SCHEMA);
        assert!(!s.kinds_off);
        assert_eq!(s.kinds.ai.exit.as_ref().unwrap().name, "🇯🇵 Tokyo 02");
        assert!(!s.kinds.ai.follow);
        assert_eq!(s.kinds.downloads.exit.as_ref().unwrap().fingerprint, "cd");
        assert!(!s.kinds.downloads.subscription && !s.kinds.downloads.fastest && s.kinds.mirror);
        // New kinds start as the subscription routes them: nothing moves.
        assert!(s.kinds.streaming.subscription && s.kinds.games.subscription && s.kinds.dev_tools.subscription);
        let exit = |id: &str| s.apps.iter().find(|a| a.id == id).map(|a| (a.kind, a.exit.clone()));
        // Domestic AI that was not direct goes as the subscription routes it.
        assert_eq!(exit("kimi"), Some((Kind::Ai, AppExit::Subscription)));
        assert_eq!(exit("deepseek"), Some((Kind::Ai, AppExit::Subscription)));
        // Routes the catalog knows are those apps' exits.
        assert_eq!(exit("netflix"), Some((Kind::Streaming, AppExit::Node("🇺🇸 Los Angeles 05".into()))));
        assert_eq!(exit("steam"), Some((Kind::Games, AppExit::Direct)));
        assert_eq!(exit("github"), Some((Kind::DevTools, AppExit::Everything)));
        // An AI site on the AI exit is the default: no choice is kept.
        assert_eq!(exit("claude"), None);
        // Others are the person's own apps, with the same exit.
        assert_eq!(exit("app:yt-dlp"), Some((Kind::Other, AppExit::Of(Kind::Ai))));
        assert_eq!(exit("site:example.com"), Some((Kind::Other, AppExit::Of(Kind::Downloads))));
        assert_eq!(exit("site:doubleclick.net"), Some((Kind::Other, AppExit::Block)));
        let own = s.apps.iter().find(|a| a.id == "app:yt-dlp").unwrap();
        assert_eq!(own.matcher, Some(RouteKind::app("yt-dlp")));
        // Written back, the old keys are gone and reading it again moves nothing.
        let text = serde_json::to_value(&s).unwrap();
        for k in ["ai", "downloads", "routes"] {
            assert!(text.get(k).is_none(), "{k} is still written");
        }
        let (again, moved) = Settings::from_value(text).unwrap();
        assert!(!moved);
        assert_eq!(again, s);
    }

    #[test]
    fn schema_1_layer_off_and_downloads_off() {
        let v = serde_json::json!({"ai": {"enabled": false, "follow": true, "downloads": false}, "routes": []});
        let (s, _) = Settings::from_value(v).unwrap();
        assert!(s.kinds_off);
        assert!(s.kinds.ai.follow && s.kinds.ai.exit.is_none());
        assert!(s.kinds.downloads.subscription);
        // Domestic AI direct was the default: nothing to keep.
        assert!(s.apps.is_empty());
    }

    #[test]
    fn a_v1_file_on_disk_is_rewritten_once() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        std::fs::write(paths.settings(), V1).unwrap();
        let s = Settings::load(&paths);
        assert_eq!(s.schema, SCHEMA);
        let on_disk: serde_json::Value = serde_json::from_slice(&std::fs::read(paths.settings()).unwrap()).unwrap();
        assert_eq!(on_disk.get("schema").and_then(|v| v.as_u64()), Some(u64::from(SCHEMA)));
        assert!(on_disk.get("routes").is_none());
        assert_eq!(Settings::load(&paths), s);
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
