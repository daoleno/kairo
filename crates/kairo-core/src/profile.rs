//! Profiles: mihomo configs from a subscription URL or a local file, their
//! metadata, and subscription downloads.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::paths::{Paths, write_atomic};

/// Traffic usage and expiry, from a subscription's `subscription-userinfo` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Usage {
    pub upload: u64,
    pub download: u64,
    /// 0 when the provider sets no limit.
    pub total: u64,
    /// Unix seconds; `None` when it never expires.
    pub expire: Option<i64>,
}

impl Usage {
    pub fn used(&self) -> u64 {
        self.upload.saturating_add(self.download)
    }

    /// Used share of the total, 0..=1, when there is a total.
    pub fn fraction(&self) -> Option<f64> {
        (self.total > 0).then(|| (self.used() as f64 / self.total as f64).clamp(0.0, 1.0))
    }

    /// Parses `upload=1; download=2; total=3; expire=4` (any order, spaces allowed).
    pub fn parse(header: &str) -> Option<Usage> {
        let mut usage = Usage::default();
        let mut any = false;
        for part in header.split(';') {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let value = value.trim();
            // Some providers send floats ("1.2e10") or empty values.
            let number = value.parse::<u64>().ok().or_else(|| {
                value
                    .parse::<f64>()
                    .ok()
                    .filter(|v| *v >= 0.0)
                    .map(|v| v as u64)
            });
            match (key.trim().to_ascii_lowercase().as_str(), number) {
                ("upload", Some(n)) => (usage.upload, any) = (n, true),
                ("download", Some(n)) => (usage.download, any) = (n, true),
                ("total", Some(n)) => (usage.total, any) = (n, true),
                ("expire", Some(n)) => {
                    usage.expire = (n > 0).then_some(n as i64);
                    any = true;
                }
                _ => {}
            }
        }
        any.then_some(usage)
    }
}

/// Where a profile comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Source {
    /// A subscription, downloaded again every `interval_hours` (0: only by hand).
    #[serde(rename_all = "camelCase")]
    Remote { url: String, interval_hours: u32 },
    /// A file the user imported (a copy is kept; editing it changes the copy).
    #[serde(rename_all = "camelCase")]
    Local { origin: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub source: Source,
    /// Unix seconds of the last successful download or import.
    pub updated_at: Option<i64>,
    pub usage: Option<Usage>,
    /// The provider's home page (`profile-web-page-url`).
    pub web_page: Option<String>,
    /// Why the last update failed, until one succeeds. The config stays the
    /// last good one meanwhile.
    pub last_error: Option<String>,
    /// Unix seconds of the last failed update (scheduled ones retry hourly).
    #[serde(default)]
    pub failed_at: Option<i64>,
    /// The person closed the card that says the last update failed. Kept until the next failed
    /// update, which says it again.
    #[serde(default)]
    pub failure_dismissed: bool,
    /// Counts of the config's parts, for the list.
    #[serde(default)]
    pub stats: ConfigStats,
}

impl Profile {
    pub fn is_remote(&self) -> bool {
        matches!(self.source, Source::Remote { .. })
    }

    pub fn url(&self) -> Option<&str> {
        match &self.source {
            Source::Remote { url, .. } => Some(url),
            Source::Local { .. } => None,
        }
    }

    /// An update failed at `now`: the config stays, the reason is kept, and the card is shown
    /// again even if the person closed it for an earlier failure.
    pub fn record_failure(&mut self, error: String, now: i64) {
        self.last_error = Some(error);
        self.failed_at = Some(now);
        self.failure_dismissed = false;
    }

    /// An update worked: nothing to say any more.
    pub fn record_success(&mut self) {
        self.last_error = None;
        self.failed_at = None;
        self.failure_dismissed = false;
    }

    /// Whether the page says the last update failed: it did, and the person has not closed the
    /// card since.
    pub fn failure_shown(&self) -> bool {
        self.last_error.is_some() && !self.failure_dismissed
    }

    /// Whether a scheduled update is due at `now` (unix seconds): on its
    /// interval, or an hour after one failed.
    pub fn update_due(&self, now: i64) -> bool {
        if self.last_error.is_some() {
            return matches!(self.source, Source::Remote { interval_hours, .. } if interval_hours > 0)
                && self.failed_at.is_none_or(|at| now - at >= RETRY_SECS);
        }
        match &self.source {
            Source::Remote { interval_hours, .. } if *interval_hours > 0 => match self.updated_at {
                Some(at) => now - at >= i64::from(*interval_hours) * 3600,
                None => true,
            },
            _ => false,
        }
    }
}

/// How long a failed scheduled update waits before it tries again.
pub const RETRY_SECS: i64 = 3600;

/// How many proxies, groups and rules a config has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ConfigStats {
    pub proxies: usize,
    pub providers: usize,
    pub groups: usize,
    pub rules: usize,
}

/// The profile list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ProfileIndex {
    pub active: Option<String>,
    pub profiles: Vec<Profile>,
}

impl ProfileIndex {
    pub fn load(paths: &Paths) -> ProfileIndex {
        std::fs::read(paths.profiles_index())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        write_atomic(
            &paths.profiles_index(),
            &serde_json::to_vec_pretty(self).expect("profiles serialize"),
        )
    }

    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }

    pub fn active_profile(&self) -> Option<&Profile> {
        self.active.as_deref().and_then(|id| self.get(id))
    }
}

/// Why a profile cannot be used, in words for the user.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProfileError {
    #[error("The file is empty")]
    Empty,
    #[error("This looks like a base64 node list, not a Clash/mihomo config")]
    NodeList,
    #[error("Not valid YAML: {0}")]
    Yaml(String),
    /// The provider answered with a message instead of a config (a login or
    /// "enable fetching" notice, an expired plan): its own words.
    #[error("Your provider says: {0}")]
    Notice(String),
    #[error("The config has no proxies, proxy providers or proxy groups")]
    NoProxies,
    #[error("Download failed: {0}")]
    Download(String),
    #[error("The server answered HTTP {0}")]
    Http(u16),
}

/// Checks that `text` is a usable mihomo config and counts its parts. The core's
/// own check (`mihomo -t`) runs again on the final config before it is applied.
pub fn inspect(text: &str) -> Result<ConfigStats, ProfileError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ProfileError::Empty);
    }
    if looks_like_html(trimmed) {
        return Err(ProfileError::Notice(notice_of(&strip_tags(trimmed))));
    }
    let value: serde_yaml::Value = match serde_yaml::from_str(trimmed) {
        Ok(v) => v,
        Err(err) => {
            return Err(if looks_like_base64(trimmed) {
                ProfileError::NodeList
            } else if looks_like_prose(trimmed) {
                ProfileError::Notice(notice_of(trimmed))
            } else {
                ProfileError::Yaml(err.to_string())
            });
        }
    };
    let Some(map) = value.as_mapping() else {
        return Err(if looks_like_base64(trimmed) {
            ProfileError::NodeList
        } else if value.is_string() || looks_like_prose(trimmed) {
            ProfileError::Notice(notice_of(trimmed))
        } else {
            ProfileError::Yaml("the top level is not a mapping".into())
        });
    };
    let len = |key: &str| -> usize {
        match map.get(key) {
            Some(serde_yaml::Value::Sequence(s)) => s.len(),
            Some(serde_yaml::Value::Mapping(m)) => m.len(),
            _ => 0,
        }
    };
    let stats = ConfigStats {
        proxies: len("proxies"),
        providers: len("proxy-providers"),
        groups: len("proxy-groups"),
        rules: len("rules"),
    };
    if stats.proxies == 0 && stats.providers == 0 && stats.groups == 0 {
        return Err(ProfileError::NoProxies);
    }
    Ok(stats)
}

/// A short message for a person rather than a config: few lines, no YAML keys
/// at the start of a line.
fn looks_like_prose(text: &str) -> bool {
    text.len() <= 2000
        && text.lines().count() <= 12
        && !text.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with('-')
                || l.split_once(':').is_some_and(|(k, _)| {
                    !k.is_empty() && !k.contains(char::is_whitespace) && k.is_ascii()
                })
        })
}

fn looks_like_html(text: &str) -> bool {
    let head = text
        .get(..text.len().min(200))
        .unwrap_or(text)
        .to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html") || head.contains("<body")
}

fn strip_tags(html: &str) -> String {
    let body = html.find("<body").map_or(html, |i| &html[i..]);
    let mut out = String::new();
    let mut tag = false;
    for c in body.chars() {
        match c {
            '<' => tag = true,
            '>' => {
                tag = false;
                out.push(' ');
            }
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// The notice, on one line and not too long to show.
fn notice_of(text: &str) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 240 {
        format!("{}…", one.chars().take(240).collect::<String>())
    } else {
        one
    }
}

fn looks_like_base64(text: &str) -> bool {
    !text.contains(':')
        && text.len() > 16
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=\r\n-_".contains(c))
}

/// A downloaded subscription.
#[derive(Debug, Clone)]
pub struct Download {
    pub body: String,
    pub usage: Option<Usage>,
    pub name: Option<String>,
    pub interval_hours: Option<u32>,
    pub web_page: Option<String>,
}

/// The User-Agent subscriptions see: providers serve mihomo configs to it.
pub fn user_agent() -> String {
    format!("Kairo/{} clash.meta mihomo", env!("CARGO_PKG_VERSION"))
}

/// Downloads a subscription, through `proxy` (an `http://host:port` URL) when given.
pub async fn download(url: &str, proxy: Option<&str>) -> Result<Download, ProfileError> {
    let mut builder = reqwest::Client::builder()
        .user_agent(user_agent())
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10));
    builder = match proxy {
        Some(p) => builder
            .proxy(reqwest::Proxy::all(p).map_err(|e| ProfileError::Download(e.to_string()))?),
        None => builder.no_proxy(),
    };
    let client = builder
        .build()
        .map_err(|e| ProfileError::Download(e.to_string()))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| ProfileError::Download(short_error(&e)))?;
    if !response.status().is_success() {
        return Err(ProfileError::Http(response.status().as_u16()));
    }
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let usage = header("subscription-userinfo").and_then(|h| Usage::parse(&h));
    let name = header("content-disposition").and_then(|h| filename_of(&h));
    let interval_hours = header("profile-update-interval")
        .and_then(|h| h.trim().parse::<f64>().ok())
        .map(|h| h.max(0.0).round() as u32);
    let web_page = header("profile-web-page-url");
    let body = response
        .text()
        .await
        .map_err(|e| ProfileError::Download(short_error(&e)))?;
    Ok(Download {
        body,
        usage,
        name,
        interval_hours,
        web_page,
    })
}

fn short_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "could not connect".into()
    } else {
        let mut text = e.to_string();
        let mut source = std::error::Error::source(e);
        while let Some(s) = source {
            text = s.to_string();
            source = s.source();
        }
        text
    }
}

/// The file name of a `Content-Disposition` header, without its extension.
pub fn filename_of(header: &str) -> Option<String> {
    let mut plain = None;
    for part in header.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("filename*=") {
            // RFC 5987: UTF-8''percent-encoded
            let v = v.trim_matches('"');
            let encoded = v.split_once("''").map(|(_, e)| e).unwrap_or(v);
            if let Ok(decoded) = percent_decode(encoded) {
                return Some(strip_ext(&decoded));
            }
        } else if let Some(v) = part.strip_prefix("filename=") {
            plain = Some(strip_ext(v.trim_matches('"')));
        }
    }
    plain.filter(|s| !s.is_empty())
}

fn strip_ext(name: &str) -> String {
    let name = name.trim();
    for ext in [".yaml", ".yml", ".txt"] {
        if let Some(stem) = name.strip_suffix(ext) {
            return stem.to_owned();
        }
    }
    name.to_owned()
}

fn percent_decode(s: &str) -> Result<String, ()> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|_| ())?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| ())?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

/// A name for a subscription URL when the server gives none: its host.
pub fn name_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "Subscription".into())
}

/// A new profile id.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Saves a profile's config text.
pub fn write_config(paths: &Paths, id: &str, text: &str) -> std::io::Result<()> {
    write_atomic(&paths.profile(id), text.as_bytes())
}

pub fn read_config(paths: &Paths, id: &str) -> std::io::Result<String> {
    std::fs::read_to_string(paths.profile(id))
}

/// The user's override YAML for a profile (empty when there is none).
pub fn read_override(paths: &Paths, id: &str) -> String {
    std::fs::read_to_string(paths.override_file(id)).unwrap_or_default()
}

pub fn write_override(paths: &Paths, id: &str, text: &str) -> std::io::Result<()> {
    write_atomic(&paths.override_file(id), text.as_bytes())
}

/// Reads a local file to import.
pub fn read_import(path: &Path) -> Result<String, ProfileError> {
    let bytes = std::fs::read(path).map_err(|e| ProfileError::Download(e.to_string()))?;
    String::from_utf8(bytes).map_err(|_| ProfileError::Yaml("the file is not UTF-8 text".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_userinfo() {
        let u = Usage::parse("upload=1024; download=2048; total=10240; expire=1893456000").unwrap();
        assert_eq!(u.upload, 1024);
        assert_eq!(u.download, 2048);
        assert_eq!(u.total, 10240);
        assert_eq!(u.expire, Some(1893456000));
        assert_eq!(u.used(), 3072);
        assert!((u.fraction().unwrap() - 0.3).abs() < 1e-9);
    }

    #[test]
    fn userinfo_tolerates_odd_values() {
        let u = Usage::parse("upload=0;download=1.5e3;total=0;expire=").unwrap();
        assert_eq!(u.download, 1500);
        assert_eq!(u.total, 0);
        assert_eq!(u.fraction(), None);
        assert_eq!(u.expire, None);
        assert_eq!(Usage::parse("garbage"), None);
        assert_eq!(Usage::parse("expire=0").unwrap().expire, None);
    }

    #[test]
    fn filename_from_disposition() {
        assert_eq!(
            filename_of("attachment; filename=\"My Sub.yaml\""),
            Some("My Sub".into())
        );
        assert_eq!(
            filename_of("attachment; filename*=UTF-8''%E6%9C%BA%E5%9C%BA.yaml; filename=x.yaml"),
            Some("机场".into())
        );
        assert_eq!(filename_of("inline"), None);
    }

    #[test]
    fn provider_notices_in_their_words() {
        let zh = "请登录系统网站后台，或在系统网站后台的订阅(SS/Trojan)页面开启订阅获取。每次开启后，订阅可获取有效时间为：10 分钟";
        assert_eq!(inspect(zh), Err(ProfileError::Notice(zh.into())));
        assert_eq!(
            inspect("Subscription expired.\nRenew at the dashboard"),
            Err(ProfileError::Notice(
                "Subscription expired. Renew at the dashboard".into()
            ))
        );
        assert_eq!(
            inspect("<!DOCTYPE html><html><body><h1>Login required</h1></body></html>"),
            Err(ProfileError::Notice("Login required".into()))
        );
        assert_eq!(
            ProfileError::Notice("x".into()).to_string(),
            "Your provider says: x"
        );
        // Configs and broken YAML stay what they were.
        assert!(matches!(
            inspect("port: 7890\nproxies: [\n"),
            Err(ProfileError::Yaml(_))
        ));
        assert_eq!(inspect("port: 7890"), Err(ProfileError::NoProxies));
    }

    #[test]
    fn inspect_counts_and_rejects() {
        let text = "proxies:\n  - {name: a, type: direct}\nproxy-groups:\n  - {name: G, type: select, proxies: [a]}\nrules:\n  - MATCH,G\n";
        let stats = inspect(text).unwrap();
        assert_eq!(
            stats,
            ConfigStats {
                proxies: 1,
                providers: 0,
                groups: 1,
                rules: 1
            }
        );
        assert_eq!(inspect("   "), Err(ProfileError::Empty));
        assert_eq!(inspect("port: 7890\n"), Err(ProfileError::NoProxies));
        assert_eq!(
            inspect("dm1lc3M6Ly9leUoySWpvaU1pSXNJbkJ6SWpvaWRHVnpkQ0o5"),
            Err(ProfileError::NodeList)
        );
        assert!(matches!(
            inspect("proxies: [unclosed"),
            Err(ProfileError::Yaml(_))
        ));
    }

    #[test]
    fn update_schedule() {
        let mut p = Profile {
            id: "x".into(),
            name: "x".into(),
            source: Source::Remote {
                url: "https://e.com/s".into(),
                interval_hours: 12,
            },
            updated_at: Some(1000),
            usage: None,
            web_page: None,
            last_error: None,
            failed_at: None,
            failure_dismissed: false,
            stats: ConfigStats::default(),
        };
        assert!(!p.update_due(1000 + 11 * 3600));
        assert!(p.update_due(1000 + 12 * 3600));
        p.updated_at = None;
        assert!(p.update_due(0));
        p.source = Source::Remote {
            url: "u".into(),
            interval_hours: 0,
        };
        assert!(!p.update_due(i64::MAX / 2));
        p.source = Source::Local { origin: None };
        assert!(!p.update_due(i64::MAX / 2));
        // A failed one keeps its config and tries again an hour later.
        p.source = Source::Remote {
            url: "u".into(),
            interval_hours: 12,
        };
        p.updated_at = Some(1000);
        p.last_error = Some("Your provider says: log in".into());
        p.failed_at = Some(5000);
        assert!(!p.update_due(5000 + RETRY_SECS - 1));
        assert!(p.update_due(5000 + RETRY_SECS));
    }

    #[test]
    fn a_closed_failure_card_stays_closed_until_the_next_failure() {
        let mut p = Profile {
            id: "x".into(),
            name: "x".into(),
            source: Source::Remote {
                url: "https://e.com/s".into(),
                interval_hours: 12,
            },
            updated_at: Some(1000),
            usage: None,
            web_page: None,
            last_error: None,
            failed_at: None,
            failure_dismissed: false,
            stats: ConfigStats::default(),
        };
        assert!(!p.failure_shown());
        p.record_failure("Your provider says: 503".into(), 5000);
        assert!(p.failure_shown());
        p.failure_dismissed = true;
        assert!(!p.failure_shown());
        // The same words an hour later are a new failure: the card is back.
        p.record_failure("Your provider says: 503".into(), 5000 + RETRY_SECS);
        assert!(p.failure_shown());
        // A success clears both, so a later failure shows too.
        p.failure_dismissed = true;
        p.record_success();
        assert!(!p.failure_shown() && !p.failure_dismissed);
        p.record_failure("x".into(), 9000);
        assert!(p.failure_shown());
    }

    #[test]
    fn index_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        paths.ensure().unwrap();
        let mut index = ProfileIndex::default();
        index.profiles.push(Profile {
            id: new_id(),
            name: "Local".into(),
            source: Source::Local {
                origin: Some("/tmp/a.yaml".into()),
            },
            updated_at: Some(now()),
            usage: None,
            web_page: None,
            last_error: None,
            failed_at: None,
            failure_dismissed: false,
            stats: ConfigStats::default(),
        });
        index.active = Some(index.profiles[0].id.clone());
        index.save(&paths).unwrap();
        let back = ProfileIndex::load(&paths);
        assert_eq!(back, index);
        assert_eq!(back.active_profile().unwrap().name, "Local");
    }

    #[test]
    fn name_from_host() {
        assert_eq!(
            name_from_url("https://sub.example.com/api?token=1"),
            "sub.example.com"
        );
        assert_eq!(name_from_url("nonsense"), "Subscription");
    }
}
