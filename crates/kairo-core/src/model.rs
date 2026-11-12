//! The data the mihomo controller API returns, shaped for the UI.

use std::collections::HashMap;

use serde::Deserialize;

/// One proxy or group as `/proxies` lists it.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RawProxy {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub all: Option<Vec<String>>,
    #[serde(default)]
    pub now: Option<String>,
    #[serde(default)]
    pub history: Vec<DelayRecord>,
    #[serde(default)]
    pub udp: bool,
    #[serde(default)]
    pub alive: Option<bool>,
    #[serde(default)]
    pub hidden: Option<bool>,
    #[serde(default, rename = "testUrl")]
    pub test_url: Option<String>,
    #[serde(default, rename = "provider-name")]
    pub provider_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DelayRecord {
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub delay: u32,
}

#[derive(Debug, Deserialize)]
pub struct ProxiesResponse {
    pub proxies: HashMap<String, RawProxy>,
}

/// A proxy node, with its last measured delay.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Node {
    pub name: String,
    pub kind: String,
    pub udp: bool,
    /// The last delay in ms: `Some(0)` means the last test failed; `None` never tested.
    pub delay: Option<u32>,
    pub alive: bool,
    /// A group nested as a member of another group.
    pub is_group: bool,
}

/// A proxy group and its members.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Group {
    pub name: String,
    /// `Selector`, `URLTest`, `Fallback`, `LoadBalance`, `Relay`.
    pub kind: String,
    pub now: Option<String>,
    pub members: Vec<Node>,
    pub hidden: bool,
    pub test_url: Option<String>,
}

impl Group {
    /// Whether the user picks the member (`select` groups; the others pick
    /// themselves, though mihomo lets fallback/url-test be pinned too).
    pub fn selectable(&self) -> bool {
        self.kind == "Selector"
    }

    pub fn kind_title(&self) -> &str {
        match self.kind.as_str() {
            "Selector" => "Select",
            "URLTest" => "URL Test",
            "Fallback" => "Fallback",
            "LoadBalance" => "Load Balance",
            "Relay" => "Relay",
            other => other,
        }
    }
}

/// The groups of `/proxies`, in the order the config lists them (mihomo's
/// `GLOBAL` group keeps that order), `GLOBAL` last.
pub fn groups_of(resp: &ProxiesResponse) -> Vec<Group> {
    let proxies = &resp.proxies;
    let is_group = |name: &str| proxies.get(name).is_some_and(|p| p.all.is_some());
    let node = |name: &str| -> Node {
        match proxies.get(name) {
            Some(p) => Node {
                name: p.name.clone(),
                kind: p.kind.clone(),
                udp: p.udp,
                delay: p.history.last().map(|h| h.delay),
                alive: p.alive.unwrap_or(true),
                is_group: p.all.is_some(),
            },
            None => Node {
                name: name.to_owned(),
                kind: String::new(),
                udp: false,
                delay: None,
                alive: true,
                is_group: false,
            },
        }
    };
    let group = |p: &RawProxy| Group {
        name: p.name.clone(),
        kind: p.kind.clone(),
        now: p.now.clone().filter(|n| !n.is_empty()),
        members: p
            .all
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|n| node(n))
            .collect(),
        hidden: p.hidden.unwrap_or(false),
        test_url: p.test_url.clone(),
    };
    let mut ordered: Vec<Group> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(global) = proxies.get("GLOBAL") {
        for name in global.all.as_deref().unwrap_or(&[]) {
            if is_group(name)
                && let Some(p) = proxies.get(name)
                && seen.insert(name.clone())
            {
                ordered.push(group(p));
            }
        }
    }
    // Groups GLOBAL does not list (hidden ones), by name.
    let mut rest: Vec<&RawProxy> = proxies
        .values()
        .filter(|p| p.all.is_some() && p.name != "GLOBAL" && !seen.contains(&p.name))
        .collect();
    rest.sort_by(|a, b| a.name.cmp(&b.name));
    ordered.extend(rest.into_iter().map(group));
    if let Some(global) = proxies.get("GLOBAL") {
        ordered.push(group(global));
    }
    ordered
}

/// A proxy provider (`/providers/proxies`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RawProvider {
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(rename = "vehicleType", default)]
    pub vehicle: String,
    #[serde(default)]
    pub proxies: Vec<RawProxy>,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: Option<String>,
    #[serde(rename = "subscriptionInfo", default)]
    pub subscription_info: Option<SubscriptionInfo>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Default, serde::Serialize)]
pub struct SubscriptionInfo {
    #[serde(rename = "Upload", default)]
    pub upload: u64,
    #[serde(rename = "Download", default)]
    pub download: u64,
    #[serde(rename = "Total", default)]
    pub total: u64,
    #[serde(rename = "Expire", default)]
    pub expire: i64,
}

#[derive(Debug, Deserialize)]
pub struct ProvidersResponse<T> {
    pub providers: HashMap<String, T>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProxyProvider {
    pub name: String,
    pub vehicle: String,
    pub count: usize,
    pub alive: usize,
    pub updated_at: Option<String>,
    pub info: Option<SubscriptionInfo>,
}

/// The providers the user configured (mihomo's built-in `default` is left out).
pub fn proxy_providers_of(resp: ProvidersResponse<RawProvider>) -> Vec<ProxyProvider> {
    let mut out: Vec<ProxyProvider> = resp
        .providers
        .into_values()
        .filter(|p| p.vehicle != "Compatible" && p.name != "default")
        .map(|p| ProxyProvider {
            count: p.proxies.len(),
            alive: p.proxies.iter().filter(|x| x.alive.unwrap_or(true)).count(),
            name: p.name,
            vehicle: p.vehicle,
            updated_at: p.updated_at,
            info: p.subscription_info,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A rule provider (`/providers/rules`).
#[derive(Debug, Clone, Deserialize, PartialEq, serde::Serialize)]
pub struct RuleProvider {
    pub name: String,
    #[serde(default)]
    pub behavior: String,
    #[serde(default)]
    pub format: String,
    #[serde(rename = "ruleCount", default)]
    pub rule_count: usize,
    #[serde(rename = "vehicleType", default)]
    pub vehicle: String,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: Option<String>,
}

/// A rule (`/rules`).
#[derive(Debug, Clone, Deserialize, PartialEq, serde::Serialize)]
pub struct Rule {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub payload: String,
    #[serde(default)]
    pub proxy: String,
    #[serde(default)]
    pub size: i64,
}

#[derive(Debug, Deserialize)]
pub struct RulesResponse {
    pub rules: Vec<Rule>,
}

/// One second of traffic (`/traffic`), bytes per second.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Default, serde::Serialize)]
pub struct Traffic {
    pub up: u64,
    pub down: u64,
}

/// The core's memory (`/memory`).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Default)]
pub struct Memory {
    #[serde(default)]
    pub inuse: u64,
    #[serde(default)]
    pub oslimit: u64,
}

/// A log line (`/logs`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LogLine {
    #[serde(rename = "type")]
    pub level: String,
    pub payload: String,
}

/// A log line with when it arrived and a sequence number.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub seq: u64,
    /// Local time, `HH:MM:SS`.
    pub time: String,
    pub level: String,
    pub payload: String,
}

/// A connection's metadata.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ConnMeta {
    pub network: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "sourceIP")]
    pub source_ip: String,
    #[serde(rename = "destinationIP")]
    pub destination_ip: String,
    #[serde(rename = "sourcePort")]
    pub source_port: String,
    #[serde(rename = "destinationPort")]
    pub destination_port: String,
    pub host: String,
    #[serde(rename = "sniffHost")]
    pub sniff_host: String,
    pub process: String,
    #[serde(rename = "processPath")]
    pub process_path: String,
    #[serde(rename = "inboundName")]
    pub inbound_name: String,
    #[serde(rename = "dnsMode")]
    pub dns_mode: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RawConnection {
    pub id: String,
    #[serde(default)]
    pub metadata: ConnMeta,
    #[serde(default)]
    pub upload: u64,
    #[serde(default)]
    pub download: u64,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub chains: Vec<String>,
    #[serde(default)]
    pub rule: String,
    #[serde(default, rename = "rulePayload")]
    pub rule_payload: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct ConnectionsResponse {
    #[serde(rename = "downloadTotal", default)]
    pub download_total: u64,
    #[serde(rename = "uploadTotal", default)]
    pub upload_total: u64,
    #[serde(default)]
    pub connections: Option<Vec<RawConnection>>,
    #[serde(default)]
    pub memory: Option<u64>,
}

/// A connection for the UI, with its current speed.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    pub id: String,
    /// The host, or the destination IP when there is no host.
    pub host: String,
    pub port: String,
    pub network: String,
    pub inbound: String,
    pub process: String,
    pub process_path: String,
    pub source: String,
    pub destination_ip: String,
    pub rule: String,
    pub rule_payload: String,
    /// The proxy chain, outermost (the node) last as mihomo lists it.
    pub chains: Vec<String>,
    pub upload: u64,
    pub download: u64,
    pub up_speed: u64,
    pub down_speed: u64,
    pub start: String,
}

impl Connection {
    pub fn from_raw(raw: RawConnection, previous: Option<&Connection>) -> Connection {
        let m = raw.metadata;
        let host = if !m.host.is_empty() {
            m.host
        } else if !m.sniff_host.is_empty() {
            m.sniff_host
        } else {
            m.destination_ip.clone()
        };
        let (up_speed, down_speed) = match previous {
            Some(p) => (
                raw.upload.saturating_sub(p.upload),
                raw.download.saturating_sub(p.download),
            ),
            None => (0, 0),
        };
        Connection {
            id: raw.id,
            host,
            port: m.destination_port,
            network: m.network,
            inbound: if m.inbound_name.is_empty() {
                m.kind
            } else {
                format!("{} ({})", m.kind, m.inbound_name)
            },
            process: m.process,
            process_path: m.process_path,
            source: if m.source_ip.is_empty() {
                String::new()
            } else {
                format!("{}:{}", m.source_ip, m.source_port)
            },
            destination_ip: m.destination_ip,
            rule: raw.rule,
            rule_payload: raw.rule_payload,
            chains: raw.chains,
            upload: raw.upload,
            download: raw.download,
            up_speed,
            down_speed,
            start: raw.start,
        }
    }

    /// The node the connection leaves through (the first of mihomo's chain list).
    pub fn outbound(&self) -> &str {
        self.chains.first().map(String::as_str).unwrap_or("")
    }

    /// Whether `query` (lowercase) matches the host, process, rule or chain.
    pub fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || [
                &self.host,
                &self.process,
                &self.rule,
                &self.rule_payload,
                &self.destination_ip,
                &self.network,
            ]
            .iter()
            .any(|f| f.to_ascii_lowercase().contains(query))
            || self
                .chains
                .iter()
                .any(|c| c.to_ascii_lowercase().contains(query))
    }
}

/// A snapshot of the open connections.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Connections {
    pub upload_total: u64,
    pub download_total: u64,
    pub list: Vec<Connection>,
}

/// The answer of `/dns/query`.
#[derive(Debug, Clone, Deserialize, PartialEq, Default, serde::Serialize)]
pub struct DnsResponse {
    #[serde(rename = "Status", default)]
    pub status: i32,
    #[serde(rename = "Answer", default)]
    pub answer: Option<Vec<DnsAnswer>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, serde::Serialize)]
pub struct DnsAnswer {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: u16,
    #[serde(rename = "TTL", default)]
    pub ttl: u32,
    #[serde(default)]
    pub data: String,
}

/// A DNS record type's name.
pub fn dns_type_name(kind: u16) -> String {
    match kind {
        1 => "A".into(),
        2 => "NS".into(),
        5 => "CNAME".into(),
        6 => "SOA".into(),
        12 => "PTR".into(),
        15 => "MX".into(),
        16 => "TXT".into(),
        28 => "AAAA".into(),
        33 => "SRV".into(),
        65 => "HTTPS".into(),
        other => other.to_string(),
    }
}

/// `/configs`, the parts Kairo reads back.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct RunningConfig {
    pub mode: String,
    #[serde(rename = "mixed-port")]
    pub mixed_port: u16,
    #[serde(rename = "allow-lan")]
    pub allow_lan: bool,
    pub ipv6: bool,
    #[serde(rename = "log-level")]
    pub log_level: String,
    /// The core's TUN as it is (`enable` goes false when it could not start).
    #[serde(default)]
    pub tun: Option<RunningTun>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct RunningTun {
    #[serde(default)]
    pub enable: bool,
    #[serde(default)]
    pub device: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct Version {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub meta: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_follow_global_order() {
        let json = r#"{"proxies":{
          "GLOBAL":{"name":"GLOBAL","type":"Selector","all":["Auto","Proxy","a","b"],"now":"Proxy","history":[]},
          "Proxy":{"name":"Proxy","type":"Selector","all":["a","b","Auto"],"now":"a","history":[]},
          "Auto":{"name":"Auto","type":"URLTest","all":["a","b"],"now":"b","history":[],"testUrl":"http://t"},
          "Hidden":{"name":"Hidden","type":"Selector","all":["a"],"now":"a","hidden":true,"history":[]},
          "a":{"name":"a","type":"Http","udp":true,"history":[{"time":"t","delay":120}],"alive":true},
          "b":{"name":"b","type":"Socks5","history":[{"time":"t","delay":0}],"alive":false}
        }}"#;
        let resp: ProxiesResponse = serde_json::from_str(json).unwrap();
        let groups = groups_of(&resp);
        let names: Vec<_> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Auto", "Proxy", "Hidden", "GLOBAL"]);
        let proxy = &groups[1];
        assert!(proxy.selectable());
        assert_eq!(proxy.now.as_deref(), Some("a"));
        assert_eq!(proxy.members[0].delay, Some(120));
        assert_eq!(proxy.members[1].delay, Some(0));
        assert!(!proxy.members[1].alive);
        assert!(proxy.members[2].is_group);
        assert_eq!(groups[0].kind_title(), "URL Test");
        assert!(groups[2].hidden);
    }

    #[test]
    fn connection_speed_and_search() {
        let raw: RawConnection = serde_json::from_str(
            r#"{"id":"1","metadata":{"network":"tcp","type":"HTTP","host":"example.com","destinationPort":"443","process":"curl","destinationIP":"1.2.3.4"},"upload":100,"download":1000,"start":"x","chains":["node-a","Proxy"],"rule":"Match","rulePayload":""}"#,
        )
        .unwrap();
        let first = Connection::from_raw(raw.clone(), None);
        assert_eq!(first.host, "example.com");
        assert_eq!(first.outbound(), "node-a");
        let mut later = raw;
        later.upload = 150;
        later.download = 4000;
        let second = Connection::from_raw(later, Some(&first));
        assert_eq!((second.up_speed, second.down_speed), (50, 3000));
        assert!(second.matches("curl"));
        assert!(second.matches("node-a"));
        assert!(second.matches(""));
        assert!(!second.matches("nothing"));
    }

    #[test]
    fn providers_skip_the_default() {
        let json = r#"{"providers":{
          "default":{"name":"default","type":"Proxy","vehicleType":"Compatible","proxies":[]},
          "sub":{"name":"sub","type":"Proxy","vehicleType":"HTTP","proxies":[{"name":"x","type":"Ss","alive":true,"history":[]},{"name":"y","type":"Ss","alive":false,"history":[]}],"updatedAt":"2026-11-08T00:00:00Z"}
        }}"#;
        let resp: ProvidersResponse<RawProvider> = serde_json::from_str(json).unwrap();
        let list = proxy_providers_of(resp);
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].count, list[0].alive), (2, 1));
    }
}
