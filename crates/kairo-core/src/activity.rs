//! Which tools talk to which AI services, and how much: the connections the
//! core reports, sorted into (tool, service) pairs. Only counts are kept (host
//! class, process name, bytes), never a URL; HTTPS hides those anyway.

use std::collections::{BTreeMap, HashMap};

use crate::ai;
use crate::model::Connection;

/// Where a connection goes, in the terms of this view.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    /// One of the AI services Kairo knows (`ai::SERVICES` id).
    Service(&'static str),
    /// A domestic AI, direct.
    Domestic(&'static str),
    /// Stays on this machine or the LAN (local models).
    Local,
}

/// What the connection is, or `None` when it is not AI (or local) traffic.
pub fn target_of(c: &Connection) -> Option<Target> {
    if let Some(s) = ai::service_of(&c.host) {
        return Some(Target::Service(s.id));
    }
    if let Some(d) = ai::domestic_of(&c.host) {
        return Some(Target::Domestic(d));
    }
    // A local model server: loopback or a private address, on the ports the
    // usual ones listen on (Ollama 11434, LM Studio 1234, llama.cpp 8080,
    // vLLM 8000), or any port when the process is a known local-model app.
    let local_host = is_local_host(&c.host);
    let known_port = matches!(c.port.as_str(), "11434" | "1234" | "8080" | "8000");
    let local_app = {
        let p = c.process_path.to_ascii_lowercase();
        ["ollama", "lm studio", "lm-studio", "llama"]
            .iter()
            .any(|n| p.contains(n))
            || ["ollama", "llama-server"]
                .iter()
                .any(|n| c.process.eq_ignore_ascii_case(n))
    };
    (local_host && (known_port || local_app)).then_some(Target::Local)
}

fn is_local_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".local") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
            std::net::IpAddr::V6(v) => {
                v.is_loopback()
                    || (v.segments()[0] & 0xfe00) == 0xfc00
                    || (v.segments()[0] & 0xffc0) == 0xfe80
            }
        };
    }
    false
}

/// The tool a connection comes from, as a person names it: the app of a
/// `.app` bundle, else the executable. `node` stays `node` (the process does
/// not say which program it runs).
pub fn tool_name(process: &str, process_path: &str) -> String {
    for part in process_path.split(['/', '\\']) {
        if let Some(app) = part.strip_suffix(".app") {
            return app.to_owned();
        }
    }
    let base = process_path
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(process);
    let base = base.strip_suffix(".exe").unwrap_or(base);
    if base.is_empty() {
        return "Unknown".into();
    }
    // Electron helpers: "Cursor Helper (Renderer)" is Cursor.
    let base = base.split(" Helper").next().unwrap_or(base);
    let mut chars = base.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() && base != "node" && !base.contains('-') => {
            format!("{}{}", c.to_ascii_uppercase(), chars.as_str())
        }
        _ => base.to_owned(),
    }
}

/// Seconds since the epoch of an RFC 3339 time (`2026-11-09T23:35:46.7+08:00`).
pub fn parse_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    // Days since 1970-01-01 (civil-from-days, Howard Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut t = days * 86400 + h * 3600 + mi * 60 + se;
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(sign) = rest.chars().next().filter(|c| *c == '+' || *c == '-') {
        let oh = rest
            .get(1..3)
            .and_then(|x| x.parse::<i64>().ok())
            .unwrap_or(0);
        let om = rest
            .get(4..6)
            .and_then(|x| x.parse::<i64>().ok())
            .unwrap_or(0);
        let off = oh * 3600 + om * 60;
        t -= if sign == '+' { off } else { -off };
    }
    Some(t)
}

/// What one (tool, target) pair has done.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Flow {
    /// Bytes sent and received since Kairo started counting (today).
    pub up: u64,
    pub down: u64,
    /// Connections open right now.
    pub open: u32,
    /// Age in seconds of the oldest open one.
    pub longest: i64,
    /// Connections ever seen, and how many ended within 5 s having moved
    /// almost nothing (a dropped or refused connection looks like that).
    pub seen: u32,
    pub short: u32,
}

/// Where a connection leaves, in Kairo's three exits and the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExitKind {
    Ai,
    Rest,
    Downloads,
    Direct,
    Blocked,
    /// Stays on this computer or the LAN.
    Local,
}

impl ExitKind {
    /// How the Activity page names it.
    pub fn title(self) -> &'static str {
        match self {
            ExitKind::Ai => "AI exit",
            ExitKind::Rest => "Everything else",
            ExitKind::Downloads => "Downloads",
            ExitKind::Direct => "Direct",
            ExitKind::Blocked => "Blocked",
            ExitKind::Local => "On this computer",
        }
    }
}

/// The names of the groups that stand for the exits in the running config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExitNames {
    pub ai: String,
    pub downloads: String,
}

/// The exit a connection leaves through, from its chain of groups.
pub fn exit_of(c: &Connection, names: &ExitNames) -> ExitKind {
    let has = |n: &str| !n.is_empty() && c.chains.iter().any(|x| x == n);
    if has(&names.ai) {
        ExitKind::Ai
    } else if has(&names.downloads) {
        ExitKind::Downloads
    } else if c.chains.iter().any(|x| x.starts_with("REJECT")) {
        ExitKind::Blocked
    } else if c.chains.iter().any(|x| x == "DIRECT") {
        if is_local_host(&c.host) {
            ExitKind::Local
        } else {
            ExitKind::Direct
        }
    } else {
        ExitKind::Rest
    }
}

/// A site's name as a person says it: `www.youtube.com` is `youtube.com`,
/// `a.b.example.co.uk` is `example.co.uk`; an address stays as it is.
pub fn site_of(host: &str) -> String {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return host.to_owned();
    }
    let labels: Vec<&str> = host.trim_end_matches('.').split('.').collect();
    if labels.len() <= 2 {
        return host.to_owned();
    }
    let n = labels.len();
    let second_level = labels[n - 1].len() == 2
        && matches!(labels[n - 2], "co" | "com" | "org" | "net" | "gov" | "edu" | "ac");
    labels[n - if second_level { 3 } else { 2 }..].join(".")
}

/// What one app has done, over every connection (not only AI's).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppFlow {
    pub up: u64,
    pub down: u64,
    pub open: u32,
    /// Bytes per site.
    pub sites: BTreeMap<String, u64>,
    /// Bytes per AI service.
    pub services: BTreeMap<&'static str, u64>,
    /// Bytes per exit.
    pub exits: BTreeMap<ExitKind, u64>,
    /// The exits of the connections open now.
    pub open_exits: std::collections::BTreeSet<ExitKind>,
}

impl AppFlow {
    pub fn total(&self) -> u64 {
        self.up + self.down
    }
}

/// Counts the stream of snapshots. `ingest` is called once per snapshot.
#[derive(Debug, Clone, Default)]
pub struct Tally {
    flows: BTreeMap<(String, Target), Flow>,
    apps: BTreeMap<String, AppFlow>,
    /// The groups that are the exits, for telling where a connection left.
    pub names: ExitNames,
    /// Last bytes seen per connection id, and its key and start.
    last: HashMap<String, Seen>,
    /// Connections that ended through the core's own close (user or Kairo).
    pub cut: u32,
}

#[derive(Debug, Clone)]
struct Seen {
    key: Option<(String, Target)>,
    up: u64,
    down: u64,
    start: i64,
    last_seen: i64,
}

impl Tally {
    /// Takes one snapshot of the open connections at `now` (unix seconds).
    pub fn ingest(&mut self, conns: &[Connection], now: i64) {
        for f in self.flows.values_mut() {
            f.open = 0;
            f.longest = 0;
        }
        for a in self.apps.values_mut() {
            a.open = 0;
            a.open_exits.clear();
        }
        let mut alive = std::collections::HashSet::new();
        for c in conns {
            let tool = tool_name(&c.process, &c.process_path);
            let start = parse_time(&c.start).unwrap_or(now);
            alive.insert(c.id.clone());
            let previous = self.last.get(&c.id).map(|p| (p.up, p.down));
            let (du, dd) = match previous {
                Some((pu, pd)) => (c.upload.saturating_sub(pu), c.download.saturating_sub(pd)),
                None => (c.upload, c.download),
            };
            // Every app, every connection.
            let kind = exit_of(c, &self.names);
            let app = self.apps.entry(tool.clone()).or_default();
            app.up += du;
            app.down += dd;
            app.open += 1;
            app.open_exits.insert(kind);
            *app.exits.entry(kind).or_default() += du + dd;
            let target = target_of(c);
            match &target {
                Some(Target::Service(id)) => *app.services.entry(id).or_default() += du + dd,
                Some(_) => {}
                None => *app.sites.entry(site_of(&c.host)).or_default() += du + dd,
            }
            // AI and local-model pairs, for the flow of tools to services.
            let key = target.map(|t| (tool, t));
            if let Some(key) = &key {
                let flow = self.flows.entry(key.clone()).or_default();
                if previous.is_none() {
                    flow.seen += 1;
                }
                flow.up += du;
                flow.down += dd;
                flow.open += 1;
                flow.longest = flow.longest.max((now - start).max(0));
            }
            self.last.insert(
                c.id.clone(),
                Seen {
                    key,
                    up: c.upload,
                    down: c.download,
                    start,
                    last_seen: now,
                },
            );
        }
        // Ended: not in this snapshot.
        let gone: Vec<String> = self
            .last
            .keys()
            .filter(|id| !alive.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            if let Some(s) = self.last.remove(&id)
                && s.last_seen - s.start <= 5
                && s.up + s.down < 2048
                && let Some(key) = &s.key
                && let Some(f) = self.flows.get_mut(key)
            {
                f.short += 1;
            }
        }
    }

    /// Every pair seen, most traffic first.
    pub fn flows(&self) -> Vec<(&str, &Target, &Flow)> {
        let mut v: Vec<_> = self
            .flows
            .iter()
            .map(|((t, g), f)| (t.as_str(), g, f))
            .collect();
        v.sort_by(|a, b| {
            (b.2.up + b.2.down)
                .cmp(&(a.2.up + a.2.down))
                .then(a.0.cmp(b.0))
        });
        v
    }

    /// Every app seen, most traffic first.
    pub fn apps(&self) -> Vec<(&str, &AppFlow)> {
        let mut v: Vec<_> = self.apps.iter().map(|(n, a)| (n.as_str(), a)).collect();
        v.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then(a.0.cmp(b.0)));
        v
    }

    /// Open connections and longest age of a service, over every tool.
    pub fn of_service(&self, id: &str) -> Flow {
        let mut total = Flow::default();
        for ((_, g), f) in &self.flows {
            if matches!(g, Target::Service(s) if *s == id) {
                total.up += f.up;
                total.down += f.down;
                total.open += f.open;
                total.longest = total.longest.max(f.longest);
                total.seen += f.seen;
                total.short += f.short;
            }
        }
        total
    }

    pub fn clear(&mut self) {
        *self = Tally::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(
        id: &str,
        host: &str,
        process: &str,
        path: &str,
        up: u64,
        down: u64,
        start: &str,
    ) -> Connection {
        Connection {
            id: id.into(),
            host: host.into(),
            port: "443".into(),
            network: "tcp".into(),
            inbound: String::new(),
            process: process.into(),
            process_path: path.into(),
            source: String::new(),
            destination_ip: String::new(),
            rule: String::new(),
            rule_payload: String::new(),
            chains: vec![],
            upload: up,
            download: down,
            up_speed: 0,
            down_speed: 0,
            start: start.into(),
        }
    }

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-11-09T15:35:46Z"), Some(1794238546));
        assert_eq!(
            parse_time("2026-11-09T23:35:46.7+08:00"),
            parse_time("2026-11-09T15:35:46Z")
        );
        assert_eq!(
            parse_time("2026-11-09T10:00:00-05:00"),
            parse_time("2026-11-09T15:00:00Z")
        );
        assert_eq!(parse_time("nope"), None);
        // A day is a day.
        assert_eq!(
            parse_time("2024-03-01T00:00:00Z").unwrap()
                - parse_time("2024-02-28T00:00:00Z").unwrap(),
            2 * 86400
        );
    }

    #[test]
    fn names_tools() {
        assert_eq!(
            tool_name("Cursor", "/Applications/Cursor.app/Contents/MacOS/Cursor"),
            "Cursor"
        );
        assert_eq!(
            tool_name(
                "Cursor Helper (Renderer)",
                "/Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Renderer).app/Contents/MacOS/Cursor Helper (Renderer)"
            ),
            "Cursor"
        );
        assert_eq!(tool_name("claude", "/home/me/.local/bin/claude"), "Claude");
        assert_eq!(tool_name("node", "/usr/bin/node"), "node");
        assert_eq!(tool_name("curl", ""), "Curl");
        assert_eq!(tool_name("", ""), "Unknown");
    }

    #[test]
    fn sorts_targets() {
        let c = conn("1", "api.anthropic.com", "claude", "/bin/claude", 0, 0, "");
        assert_eq!(target_of(&c), Some(Target::Service("claude")));
        assert_eq!(
            target_of(&conn("2", "api.deepseek.com", "x", "", 0, 0, "")),
            Some(Target::Domestic("DeepSeek"))
        );
        assert_eq!(
            target_of(&conn("3", "example.com", "x", "", 0, 0, "")),
            None
        );
        let mut local = conn("4", "127.0.0.1", "ollama", "/usr/bin/ollama", 0, 0, "");
        local.port = "11434".into();
        assert_eq!(target_of(&local), Some(Target::Local));
        // Loopback on an unrelated port is not a model.
        let mut other = conn("5", "127.0.0.1", "curl", "/usr/bin/curl", 0, 0, "");
        other.port = "9999".into();
        assert_eq!(target_of(&other), None);
    }

    #[test]
    fn counts_deltas_not_totals_and_tracks_open_and_longest() {
        let mut t = Tally::default();
        let start = "2026-11-09T15:00:00Z";
        let now = parse_time(start).unwrap();
        t.ingest(
            &[conn(
                "a",
                "api.anthropic.com",
                "claude",
                "/bin/claude",
                100,
                1000,
                start,
            )],
            now + 10,
        );
        t.ingest(
            &[conn(
                "a",
                "api.anthropic.com",
                "claude",
                "/bin/claude",
                150,
                4000,
                start,
            )],
            now + 70,
        );
        let c = t.of_service("claude");
        assert_eq!(
            (c.up, c.down, c.open, c.longest, c.seen),
            (150, 4000, 1, 70, 1)
        );
        // It ends: bytes stay counted, nothing is open.
        t.ingest(&[], now + 130);
        let c = t.of_service("claude");
        assert_eq!(
            (c.up, c.down, c.open, c.longest, c.short),
            (150, 4000, 0, 0, 0)
        );
    }

    #[test]
    fn a_connection_that_dies_at_once_counts_as_short() {
        let mut t = Tally::default();
        let start = "2026-11-09T15:00:00Z";
        let now = parse_time(start).unwrap();
        t.ingest(
            &[conn(
                "a",
                "chatgpt.com",
                "node",
                "/usr/bin/node",
                10,
                20,
                start,
            )],
            now + 1,
        );
        t.ingest(&[], now + 3);
        assert_eq!(t.of_service("chatgpt").short, 1);
        // A long one that ends is not.
        t.ingest(
            &[conn(
                "b",
                "chatgpt.com",
                "node",
                "/usr/bin/node",
                10,
                20,
                start,
            )],
            now + 100,
        );
        t.ingest(&[], now + 200);
        assert_eq!(t.of_service("chatgpt").short, 1);
    }

    #[test]
    fn groups_by_tool_and_orders_by_traffic() {
        let mut t = Tally::default();
        let s = "2026-11-09T15:00:00Z";
        let now = parse_time(s).unwrap();
        t.ingest(
            &[
                conn(
                    "1",
                    "api.openai.com",
                    "Cursor",
                    "/Applications/Cursor.app/x",
                    1,
                    5000,
                    s,
                ),
                conn("2", "claude.ai", "claude", "/bin/claude", 1, 9000, s),
            ],
            now + 10,
        );
        let f = t.flows();
        assert_eq!(f[0].0, "Claude");
        assert_eq!(f[1].0, "Cursor");
        assert_eq!(*f[1].1, Target::Service("chatgpt"));
    }

    #[test]
    fn every_app_is_counted_with_its_sites_and_exits() {
        let mut t = Tally::default();
        t.names = ExitNames { ai: "Kairo AI".into(), downloads: "Kairo Downloads".into() };
        let s = "2026-11-09T15:00:00Z";
        let now = parse_time(s).unwrap();
        let mut ai = conn("1", "chatgpt.com", "Safari", "/Applications/Safari.app/x", 10, 900, s);
        ai.chains = vec!["Tokyo 03".into(), "Kairo AI".into()];
        let mut video = conn("2", "www.youtube.com", "Safari", "/Applications/Safari.app/x", 5, 4000, s);
        video.chains = vec!["Hong Kong 01".into(), "Proxy".into()];
        let mut direct = conn("3", "www.bilibili.com", "Safari", "/Applications/Safari.app/x", 1, 100, s);
        direct.chains = vec!["DIRECT".into()];
        t.ingest(&[ai, video, direct], now + 5);
        let apps = t.apps();
        assert_eq!(apps.len(), 1);
        let (name, a) = apps[0];
        assert_eq!(name, "Safari");
        assert_eq!((a.up, a.down, a.open), (16, 5000, 3));
        assert_eq!(a.services.get("chatgpt"), Some(&910));
        assert_eq!(a.sites.get("youtube.com"), Some(&4005));
        assert_eq!(a.sites.get("bilibili.com"), Some(&101));
        assert_eq!(a.exits.get(&ExitKind::Ai), Some(&910));
        assert_eq!(a.exits.get(&ExitKind::Rest), Some(&4005));
        assert_eq!(a.exits.get(&ExitKind::Direct), Some(&101));
        // Closed: the bytes stay, nothing is open.
        t.ingest(&[], now + 60);
        let a = t.apps()[0].1;
        assert_eq!((a.total(), a.open), (5016, 0));
    }

    #[test]
    fn names_the_exit_by_the_group_in_the_chain() {
        let names = ExitNames { ai: "Kairo AI".into(), downloads: "Kairo Downloads".into() };
        let mut c = conn("1", "x.com", "a", "", 0, 0, "");
        c.chains = vec!["n".into(), "Proxy".into()];
        assert_eq!(exit_of(&c, &names), ExitKind::Rest);
        c.chains = vec!["n".into(), "Kairo Downloads".into()];
        assert_eq!(exit_of(&c, &names), ExitKind::Downloads);
        c.chains = vec!["REJECT".into()];
        assert_eq!(exit_of(&c, &names), ExitKind::Blocked);
        c.chains = vec!["DIRECT".into()];
        assert_eq!(exit_of(&c, &names), ExitKind::Direct);
        c.host = "localhost".into();
        assert_eq!(exit_of(&c, &names), ExitKind::Local);
        // An unset name never matches an empty chain entry.
        let none = ExitNames::default();
        c.chains = vec!["".into()];
        c.host = "x.com".into();
        assert_eq!(exit_of(&c, &none), ExitKind::Rest);
    }

    #[test]
    fn a_site_is_its_registrable_name() {
        assert_eq!(site_of("www.youtube.com"), "youtube.com");
        assert_eq!(site_of("a.b.example.co.uk"), "example.co.uk");
        assert_eq!(site_of("docker.io"), "docker.io");
        assert_eq!(site_of("10.0.0.5"), "10.0.0.5");
        assert_eq!(site_of("localhost"), "localhost");
    }
}
