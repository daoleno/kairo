//! The user's config, reduced to what the core may run as SYSTEM.
//!
//! The host never hands the user's file to the core. It parses it, keeps what an
//! allowlist names, and writes a new config of its own. What is dropped is the
//! core's way to touch files and sockets outside what TUN needs: external UI
//! download, unix/pipe/TLS controllers, extra listeners and tunnels, certificate
//! and key paths, provider files. (The pinned core also checks that paths it reads
//! or writes lie inside its home directory; that check is lexical, so it only holds
//! because the home directory is SYSTEM-owned and the user cannot plant a link in
//! it. This list does not rely on it.)
//!
//! The controller is the host's: the core listens on a loopback port and secret
//! only the host knows (`Internal`); the user's port and secret are returned so
//! the host can answer on them.

use serde_yaml::{Mapping, Value};

/// Where the host put the core's controller.
pub struct Internal<'a> {
    pub port: u16,
    pub secret: &'a str,
}

/// What the host runs the core with, and what it learned from the user's config.
#[derive(Debug)]
pub struct Sanitized {
    pub text: String,
    /// The port the user's Kairo talks to the controller on.
    pub controller_port: u16,
    /// The secret it uses.
    pub secret: String,
    /// What was dropped, one line each (the host logs these).
    pub dropped: Vec<String>,
}

const MODES: &[&str] = &["rule", "global", "direct"];
const LOG_LEVELS: &[&str] = &["silent", "error", "warning", "info", "debug"];
const FIND_PROCESS: &[&str] = &["always", "strict", "off"];
const LOADERS: &[&str] = &["standard", "memconservative"];

/// Proxy kinds that need no local file or state directory.
const PROXY_TYPES: &[&str] = &[
    "ss", "ssr", "vmess", "vless", "trojan", "hysteria", "hysteria2", "tuic", "anytls", "socks5",
    "http", "snell", "wireguard", "mieru", "direct", "dns", "reject", "reject-drop", "sudoku",
    "trusttunnel", "masque",
];

/// Keys inside a proxy that name a file.
const PROXY_FILE_KEYS: &[&str] = &[
    "ca",
    "certificate",
    "private-key",
    "client-certificate",
    "client-key",
    "state-dir",
    "planet",
    "key",
];

const GROUP_KEYS: &[&str] = &[
    "name", "type", "proxies", "use", "url", "interval", "lazy", "timeout", "max-failed-times",
    "disable-udp", "include-all", "include-all-proxies", "include-all-providers", "filter",
    "exclude-filter", "exclude-type", "expected-status", "hidden", "icon", "strategy", "tolerance",
    "interface-name", "dialer-proxy",
];

const PROVIDER_KEYS: &[&str] = &[
    "type", "url", "interval", "health-check", "filter", "exclude-filter", "exclude-type",
    "override", "header", "proxy", "size-limit", "payload", "behavior", "format",
];

const TUN_KEYS: &[&str] = &[
    "enable", "device", "stack", "dns-hijack", "auto-route", "auto-detect-interface", "strict-route",
    "mtu", "gso", "gso-max-size", "endpoint-independent-nat", "udp-timeout", "icmp-timeout",
    "route-address", "route-exclude-address", "inet6-route-address", "inet6-route-exclude-address",
    "inet4-route-address", "inet4-route-exclude-address", "inet6-address", "include-interface",
    "exclude-interface", "loopback-address",
];

const DNS_KEYS: &[&str] = &[
    "listen", "enable", "ipv6", "enhanced-mode", "fake-ip-range", "fake-ip-range6", "fake-ip-filter",
    "fake-ip-filter-mode", "default-nameserver", "nameserver", "fallback", "proxy-server-nameserver",
    "direct-nameserver", "direct-nameserver-follow-policy", "nameserver-policy", "fallback-filter",
    "use-hosts", "use-system-hosts", "respect-rules", "prefer-h3", "cache-algorithm", "fake-ip-ttl",
    "ipv6-timeout",
];

/// Scalars at the top of the file: (key, what it may be).
enum Scalar {
    Bool,
    Int,
    Enum(&'static [&'static str]),
    Text,
}

const SCALARS: &[(&str, Scalar)] = &[
    ("mode", Scalar::Enum(MODES)),
    ("log-level", Scalar::Enum(LOG_LEVELS)),
    ("find-process-mode", Scalar::Enum(FIND_PROCESS)),
    ("ipv6", Scalar::Bool),
    ("unified-delay", Scalar::Bool),
    ("tcp-concurrent", Scalar::Bool),
    ("allow-lan", Scalar::Bool),
    ("mixed-port", Scalar::Int),
    ("bind-address", Scalar::Text),
    ("interface-name", Scalar::Text),
    ("global-client-fingerprint", Scalar::Text),
    ("global-ua", Scalar::Text),
    ("geodata-mode", Scalar::Bool),
    ("geodata-loader", Scalar::Enum(LOADERS)),
    ("geo-auto-update", Scalar::Bool),
    ("geo-update-interval", Scalar::Int),
    ("keep-alive-interval", Scalar::Int),
    ("keep-alive-idle", Scalar::Int),
    ("disable-keep-alive", Scalar::Bool),
];

fn text_ok(s: &str) -> bool {
    s.len() <= 4096 && !s.chars().any(|c| c.is_control())
}

fn scalar_ok(v: &Value, kind: &Scalar) -> bool {
    match (kind, v) {
        (Scalar::Bool, Value::Bool(_)) => true,
        (Scalar::Int, Value::Number(n)) => n.as_u64().is_some_and(|n| n <= u32::MAX as u64),
        (Scalar::Enum(list), Value::String(s)) => list.contains(&s.as_str()),
        (Scalar::Text, Value::String(s)) => text_ok(s),
        _ => false,
    }
}

/// A value that is plain data: scalars, lists and mappings of them, no control
/// characters, not deeper than `depth`.
fn plain(v: &Value, depth: usize) -> bool {
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(s) => text_ok(s),
        Value::Sequence(s) => depth > 0 && s.iter().all(|x| plain(x, depth - 1)),
        Value::Mapping(m) => {
            depth > 0
                && m.iter()
                    .all(|(k, x)| k.as_str().is_some_and(text_ok) && plain(x, depth - 1))
        }
        Value::Tagged(_) => false,
    }
}

fn key_of(k: &Value) -> Option<&str> {
    k.as_str()
}

/// `port` out of `host:port`, `:port` or `[::]:port`.
fn port_of(addr: &str) -> Option<u16> {
    let p = addr.rsplit(':').next()?;
    let port: u16 = p.parse().ok()?;
    (port >= 1024).then_some(port)
}

fn https(s: &str) -> bool {
    s.starts_with("https://") && text_ok(s)
}

fn keep_subset(
    name: &str,
    v: &Value,
    allowed: &[&str],
    dropped: &mut Vec<String>,
) -> Option<Value> {
    let Value::Mapping(m) = v else {
        dropped.push(format!("{name}: not a mapping"));
        return None;
    };
    let mut out = Mapping::new();
    for (k, x) in m {
        let Some(key) = key_of(k) else { continue };
        if allowed.contains(&key) && plain(x, 12) {
            out.insert(k.clone(), x.clone());
        } else {
            dropped.push(format!("{name}.{key}"));
        }
    }
    Some(Value::Mapping(out))
}

fn proxy(p: &Value, dropped: &mut Vec<String>, label: &str) -> Option<Value> {
    let Value::Mapping(m) = p else {
        dropped.push(format!("{label}: not a mapping"));
        return None;
    };
    let ty = m.get("type").and_then(Value::as_str).unwrap_or("");
    let name = m.get("name").and_then(Value::as_str).unwrap_or("?");
    if !PROXY_TYPES.contains(&ty) {
        dropped.push(format!("{label} {name}: type {ty}"));
        return None;
    }
    let mut out = Mapping::new();
    for (k, x) in m {
        let Some(key) = key_of(k) else { continue };
        if PROXY_FILE_KEYS.contains(&key) || key.starts_with("<<") {
            dropped.push(format!("{label} {name}: {key}"));
        } else if plain(x, 12) {
            out.insert(k.clone(), x.clone());
        } else {
            dropped.push(format!("{label} {name}: {key} (not plain data)"));
        }
    }
    Some(Value::Mapping(out))
}

fn providers(kind: &str, v: &Value, dropped: &mut Vec<String>) -> Option<Value> {
    let Value::Mapping(all) = v else {
        dropped.push(format!("{kind}: not a mapping"));
        return None;
    };
    let mut out = Mapping::new();
    for (k, p) in all {
        let (Some(name), Value::Mapping(m)) = (key_of(k), p) else { continue };
        let ty = m.get("type").and_then(Value::as_str).unwrap_or("");
        if ty != "http" && ty != "inline" {
            dropped.push(format!("{kind} {name}: type {ty}"));
            continue;
        }
        let mut one = Mapping::new();
        for (pk, pv) in m {
            let Some(key) = key_of(pk) else { continue };
            if !PROVIDER_KEYS.contains(&key) {
                // Includes `path`: the core names the file itself, inside its home.
                dropped.push(format!("{kind} {name}: {key}"));
            } else if key == "payload" && kind == "proxy-providers" {
                if let Value::Sequence(list) = pv {
                    let kept: Vec<Value> = list
                        .iter()
                        .filter_map(|x| proxy(x, dropped, &format!("{kind} {name}")))
                        .collect();
                    one.insert(pk.clone(), Value::Sequence(kept));
                }
            } else if plain(pv, 12) {
                one.insert(pk.clone(), pv.clone());
            }
        }
        if ty == "http" && !one.get("url").and_then(Value::as_str).is_some_and(|u| u.starts_with("http")) {
            dropped.push(format!("{kind} {name}: no url"));
            continue;
        }
        out.insert(k.clone(), Value::Mapping(one));
    }
    Some(Value::Mapping(out))
}

/// Builds the config the core runs from the user's text.
pub fn sanitize(input: &str, internal: &Internal<'_>) -> Result<Sanitized, String> {
    let mut root: Value = serde_yaml::from_str(input).map_err(|e| format!("not a YAML config: {e}"))?;
    // The core's YAML library expands `<<` merges; expand them here so what is
    // checked below is what the core would have read.
    root.apply_merge().map_err(|e| format!("YAML merge: {e}"))?;
    let Value::Mapping(top) = root else {
        return Err("the config is not a mapping".into());
    };

    let mut dropped = Vec::new();
    let mut out = Mapping::new();
    let mut user_port = None;
    let mut user_secret = String::new();

    for (k, v) in &top {
        let Some(key) = key_of(k) else {
            dropped.push("(a key that is not text)".into());
            continue;
        };
        if let Some((_, kind)) = SCALARS.iter().find(|(n, _)| *n == key) {
            if scalar_ok(v, kind) {
                out.insert(k.clone(), v.clone());
            } else {
                dropped.push(format!("{key}: unusable value"));
            }
            continue;
        }
        match key {
            "external-controller" => {
                user_port = v.as_str().and_then(port_of);
            }
            "secret" => {
                user_secret = v.as_str().unwrap_or("").to_owned();
            }
            "tun" => {
                if let Some(t) = keep_subset("tun", v, TUN_KEYS, &mut dropped) {
                    out.insert(k.clone(), t);
                }
            }
            "dns" => {
                if let Some(mut t) = keep_subset("dns", v, DNS_KEYS, &mut dropped) {
                    // The profile's own resolver on this machine (its nodes'
                    // names resolve through it): kept, on loopback, unprivileged port.
                    let listen = v.get("listen").and_then(Value::as_str).and_then(port_of);
                    if let Some(m) = t.as_mapping_mut()
                        && m.contains_key("listen")
                    {
                        match listen {
                            Some(port) => {
                                m.insert("listen".into(), format!("127.0.0.1:{port}").into());
                            }
                            None => {
                                m.remove("listen");
                                dropped.push("dns.listen".into());
                            }
                        }
                    }
                    out.insert(k.clone(), t);
                }
            }
            "profile" => {
                if let Some(t) = keep_subset("profile", v, &["store-selected", "store-fake-ip"], &mut dropped) {
                    out.insert(k.clone(), t);
                }
            }
            "sniffer" | "hosts" | "sub-rules" | "authentication" | "skip-auth-prefixes"
            | "lan-allowed-ips" | "lan-disallowed-ips" => {
                if plain(v, 12) {
                    out.insert(k.clone(), v.clone());
                } else {
                    dropped.push(format!("{key}: not plain data"));
                }
            }
            "geox-url" => {
                if let Value::Mapping(m) = v {
                    let mut g = Mapping::new();
                    for (gk, gv) in m {
                        match (key_of(gk), gv.as_str()) {
                            (Some(_), Some(u)) if https(u) => {
                                g.insert(gk.clone(), Value::String(u.to_owned()));
                            }
                            (n, _) => dropped.push(format!("geox-url.{}", n.unwrap_or("?"))),
                        }
                    }
                    out.insert(k.clone(), Value::Mapping(g));
                }
            }
            "proxies" => {
                if let Value::Sequence(list) = v {
                    let kept: Vec<Value> =
                        list.iter().filter_map(|p| proxy(p, &mut dropped, "proxies")).collect();
                    out.insert(k.clone(), Value::Sequence(kept));
                }
            }
            "proxy-groups" => {
                if let Value::Sequence(list) = v {
                    let kept: Vec<Value> = list
                        .iter()
                        .filter_map(|g| keep_subset("proxy-groups", g, GROUP_KEYS, &mut dropped))
                        .collect();
                    out.insert(k.clone(), Value::Sequence(kept));
                }
            }
            "proxy-providers" | "rule-providers" => {
                if let Some(p) = providers(key, v, &mut dropped) {
                    out.insert(k.clone(), p);
                }
            }
            "rules" => {
                if let Value::Sequence(list) = v {
                    let kept: Vec<Value> = list
                        .iter()
                        .filter(|r| r.as_str().is_some_and(text_ok))
                        .cloned()
                        .collect();
                    if kept.len() != list.len() {
                        dropped.push("rules: unusable entries".into());
                    }
                    out.insert(k.clone(), Value::Sequence(kept));
                }
            }
            other => dropped.push(other.to_owned()),
        }
    }

    let controller_port = user_port.ok_or("the config names no controller port (external-controller)")?;
    if user_secret.len() < 16 || !text_ok(&user_secret) {
        return Err("the config has no controller secret of at least 16 characters".into());
    }
    out.insert("external-controller".into(), format!("127.0.0.1:{}", internal.port).into());
    out.insert("secret".into(), internal.secret.into());
    // The core must not pick up a UI or any other listener of its own.
    let text = serde_yaml::to_string(&Value::Mapping(out)).map_err(|e| e.to_string())?;
    Ok(Sanitized { text, controller_port, secret: user_secret, dropped })
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERNAL: Internal<'static> = Internal { port: 40000, secret: "host-only-secret" };

    fn run(input: &str) -> Sanitized {
        sanitize(input, &INTERNAL).unwrap()
    }

    fn parsed(s: &Sanitized) -> Mapping {
        match serde_yaml::from_str::<Value>(&s.text).unwrap() {
            Value::Mapping(m) => m,
            _ => panic!(),
        }
    }

    const BASE: &str = "external-controller: 127.0.0.1:9097\nsecret: 0123456789abcdef0123\n";

    #[test]
    fn the_controller_is_the_hosts_not_the_users() {
        let s = run(&format!("{BASE}mixed-port: 7897\n"));
        assert_eq!(s.controller_port, 9097);
        assert_eq!(s.secret, "0123456789abcdef0123");
        let m = parsed(&s);
        assert_eq!(m["external-controller"].as_str(), Some("127.0.0.1:40000"));
        assert_eq!(m["secret"].as_str(), Some("host-only-secret"));
        assert_eq!(m["mixed-port"].as_u64(), Some(7897));
    }

    #[test]
    fn a_config_without_a_strong_secret_or_a_port_is_refused() {
        assert!(sanitize("external-controller: 127.0.0.1:9097\nsecret: short\n", &INTERNAL).is_err());
        assert!(sanitize("secret: 0123456789abcdef0123\n", &INTERNAL).is_err());
        assert!(sanitize("external-controller: 127.0.0.1:80\nsecret: 0123456789abcdef0123\n", &INTERNAL).is_err());
        assert!(sanitize("- a\n- b\n", &INTERNAL).is_err());
        assert!(sanitize("{ not yaml", &INTERNAL).is_err());
    }

    fn with_base(body: &str) -> String {
        format!("{BASE}{body}")
    }

    #[test]
    fn file_and_socket_options_do_not_survive() {
        let s = run(&with_base(
            r#"
external-ui: C:/Windows/System32
external-ui-url: https://example.com/ui.zip
external-controller-unix: C:/Windows/evil.sock
external-controller-pipe: \\.\pipe\x
external-controller-tls: 0.0.0.0:9443
tls:
  certificate: C:/x.pem
  private-key: C:/x.key
listeners:
  - name: t
    type: tunnel
    port: 8080
    target: 127.0.0.1:1
tunnels:
  - tcp/udp,127.0.0.1:6553,114.114.114.114:53,proxy
script:
  code: x
experimental:
  quic-go-disable-gso: true
mode: rule
"#,
        ));
        let m = parsed(&s);
        for gone in [
            "external-ui", "external-ui-url", "external-controller-unix", "external-controller-pipe",
            "external-controller-tls", "tls", "listeners", "tunnels", "script", "experimental",
        ] {
            assert!(!m.contains_key(gone), "{gone} survived");
        }
        assert!(!s.text.contains("evil.sock") && !s.text.contains("x.pem"));
        assert_eq!(m["mode"].as_str(), Some("rule"));
        assert!(s.dropped.iter().any(|d| d == "external-ui"));
    }

    #[test]
    fn provider_files_and_paths_are_dropped_and_urls_kept() {
        let s = run(&with_base(
            r#"
proxy-providers:
  web:
    type: http
    url: https://example.com/sub
    path: C:/Windows/Temp/x.yaml
    interval: 3600
  disk:
    type: file
    path: C:/Users/victim/secret.txt
rule-providers:
  r:
    type: http
    behavior: domain
    url: https://example.com/r.yaml
    path: ../../../evil
  local:
    type: file
    behavior: domain
    path: C:/x
"#,
        ));
        let m = parsed(&s);
        let pp = m["proxy-providers"].as_mapping().unwrap();
        assert!(pp.contains_key("web") && !pp.contains_key("disk"));
        assert!(pp["web"].as_mapping().unwrap().get("path").is_none());
        assert_eq!(pp["web"]["interval"].as_u64(), Some(3600));
        let rp = m["rule-providers"].as_mapping().unwrap();
        assert!(rp.contains_key("r") && !rp.contains_key("local"));
        assert!(!s.text.contains("evil") && !s.text.contains("victim"));
    }

    #[test]
    fn proxies_lose_file_fields_and_unknown_kinds_but_keep_what_a_node_needs() {
        let s = run(&with_base(
            r#"
proxies:
  - {name: a, type: vless, server: h, port: 443, uuid: u, network: ws, ws-opts: {path: /ws, headers: {Host: h}}, ca: C:/x.pem, private-key: C:/k}
  - {name: b, type: ssh, server: h, port: 22, private-key: C:/id}
  - {name: c, type: ss, server: h, port: 1, cipher: aes-128-gcm, password: p, certificate: C:/c}
"#,
        ));
        let m = parsed(&s);
        let list = m["proxies"].as_sequence().unwrap();
        assert_eq!(list.len(), 2, "the ssh node (a key file on disk) is dropped");
        let a = list[0].as_mapping().unwrap();
        assert_eq!(a["ws-opts"]["path"].as_str(), Some("/ws"), "a URL path is not a file path");
        assert!(a.get("ca").is_none() && a.get("private-key").is_none());
        assert!(list[1].as_mapping().unwrap().get("certificate").is_none());
    }

    #[test]
    fn a_merge_key_cannot_carry_what_was_dropped() {
        let s = run(&with_base(
            r#"
x: &evil
  ca: C:/x.pem
  private-key: C:/k
proxies:
  - <<: *evil
    name: a
    type: trojan
    server: h
    port: 1
    password: p
"#,
        ));
        let m = parsed(&s);
        let a = m["proxies"].as_sequence().unwrap()[0].as_mapping().unwrap();
        assert!(a.get("ca").is_none() && a.get("private-key").is_none());
        assert!(!s.text.contains("<<") && !s.text.contains("x.pem"));
    }

    #[test]
    fn geo_urls_must_be_https_and_tun_keeps_what_tun_needs() {
        let s = run(&with_base(
            r#"
geox-url:
  mmdb: https://example.com/a.metadb
  geoip: file:///C:/Windows/win.ini
  geosite: http://example.com/s.dat
tun:
  enable: true
  stack: mixed
  auto-route: true
  route-address-set: [x]
  file-descriptor: 5
  device: Kairo
dns:
  enable: true
  listen: 0.0.0.0:1053
  enhanced-mode: fake-ip
"#,
        ));
        let m = parsed(&s);
        let g = m["geox-url"].as_mapping().unwrap();
        assert_eq!(g.len(), 1);
        let tun = m["tun"].as_mapping().unwrap();
        assert!(tun.contains_key("auto-route") && tun.contains_key("stack"));
        assert!(!tun.contains_key("route-address-set") && !tun.contains_key("file-descriptor"));
        let dns = m["dns"].as_mapping().unwrap();
        // A resolver of the profile's own stays, but only on loopback.
        assert!(dns.contains_key("enhanced-mode"));
        assert_eq!(dns["listen"].as_str(), Some("127.0.0.1:1053"));
    }

    #[test]
    fn a_resolver_on_a_privileged_port_is_dropped() {
        let s = run(&with_base("dns:\n  enable: true\n  listen: 0.0.0.0:53\n"));
        assert!(parsed(&s)["dns"].as_mapping().unwrap().get("listen").is_none());
    }

    #[test]
    fn unknown_top_level_keys_and_odd_values_are_dropped_not_passed() {
        let s = run(&with_base("mode: evil\nsomething-new: {path: C:/x}\nlog-level: info\n"));
        let m = parsed(&s);
        assert!(!m.contains_key("something-new") && !m.contains_key("mode"));
        assert_eq!(m["log-level"].as_str(), Some("info"));
    }

    #[test]
    fn rules_and_groups_pass_through() {
        let s = run(&with_base(
            r#"
proxy-groups:
  - {name: Auto, type: url-test, proxies: [a], url: 'https://x/', interval: 300, script: evil}
rules:
  - DOMAIN-SUFFIX,example.com,Auto
  - MATCH,DIRECT
"#,
        ));
        let m = parsed(&s);
        assert_eq!(m["rules"].as_sequence().unwrap().len(), 2);
        let g = m["proxy-groups"].as_sequence().unwrap()[0].as_mapping().unwrap();
        assert!(g.contains_key("interval") && !g.contains_key("script"));
    }
}
