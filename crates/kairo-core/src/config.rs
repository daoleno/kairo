//! Builds the config the core runs: the active profile, the user's override merged
//! on top, then the settings Kairo owns (ports, controller, mode, TUN). The result
//! is checked by the core itself (`mihomo -t`) before it replaces the running one.

use std::path::Path;
use std::time::Duration;

use serde_yaml::{Mapping, Value};

use crate::settings::Settings;

/// Keys an override may use to add to lists instead of replacing them.
const LIST_OPS: [(&str, &str, bool); 6] = [
    ("prepend-rules", "rules", true),
    ("append-rules", "rules", false),
    ("prepend-proxies", "proxies", true),
    ("append-proxies", "proxies", false),
    ("prepend-proxy-groups", "proxy-groups", true),
    ("append-proxy-groups", "proxy-groups", false),
];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("The profile is not valid YAML: {0}")]
    Profile(String),
    #[error("The override is not valid YAML: {0}")]
    Override(String),
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

use crate::profile::check_yaml_size;

/// Parses YAML text into a mapping (an empty text is an empty mapping).
pub fn parse_mapping(text: &str) -> Result<Mapping, String> {
    if text.trim().is_empty() {
        return Ok(Mapping::new());
    }
    check_yaml_size(text)?;
    match serde_yaml::from_str::<Value>(text).map_err(|e| e.to_string())? {
        Value::Mapping(m) => Ok(m),
        Value::Null => Ok(Mapping::new()),
        _ => Err("the top level is not a mapping".into()),
    }
}

/// The links of a config's `proxy-providers` and `rule-providers` (they
/// often carry the account's token, as the subscription link does).
pub fn provider_urls(text: &str) -> Vec<String> {
    let Ok(config) = parse_mapping(text) else { return Vec::new() };
    let mut urls = Vec::new();
    for key in ["proxy-providers", "rule-providers"] {
        if let Some(Value::Mapping(providers)) = config.get(key) {
            for p in providers.values().filter_map(Value::as_mapping) {
                if let Some(url) = p.get("url").and_then(Value::as_str).filter(|u| !u.is_empty()) {
                    urls.push(url.to_owned());
                }
            }
        }
    }
    urls
}

/// The `rule-providers` of a config: each one's name and `behavior`
/// (`domain`, `ipcidr`, `classical`), lowercase.
pub fn rule_set_behaviors(text: &str) -> Vec<(String, String)> {
    let Ok(config) = parse_mapping(text) else { return Vec::new() };
    let Some(Value::Mapping(providers)) = config.get("rule-providers") else { return Vec::new() };
    let mut out: Vec<(String, String)> = providers
        .iter()
        .filter_map(|(name, p)| {
            let behavior = p.as_mapping()?.get("behavior")?.as_str()?.to_ascii_lowercase();
            Some((name.as_str()?.to_owned(), behavior))
        })
        .collect();
    out.sort();
    out
}

/// Merges `patch` into `base`: mappings merge key by key, anything else replaces.
/// The `prepend-*` / `append-*` keys add to the named list instead.
pub fn merge(base: &mut Mapping, patch: Mapping) {
    for (key, value) in patch {
        if let Some(name) = key.as_str()
            && let Some((_, target, front)) = LIST_OPS.iter().find(|(op, _, _)| *op == name)
        {
            let items = match value {
                Value::Sequence(s) => s,
                other => vec![other],
            };
            let entry = base
                .entry(Value::from(*target))
                .or_insert_with(|| Value::Sequence(vec![]));
            if !entry.is_sequence() {
                *entry = Value::Sequence(vec![]);
            }
            let list = entry.as_sequence_mut().expect("a sequence");
            if *front {
                let rest = std::mem::take(list);
                list.extend(items);
                list.extend(rest);
            } else {
                list.extend(items);
            }
            continue;
        }
        match (base.get_mut(&key), value) {
            (Some(Value::Mapping(b)), Value::Mapping(p)) => merge(b, p),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

fn set(map: &mut Mapping, key: &str, value: impl Into<Value>) {
    map.insert(Value::from(key), value.into());
}

fn sub<'a>(map: &'a mut Mapping, key: &str) -> &'a mut Mapping {
    let entry = map
        .entry(Value::from(key))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if !entry.is_mapping() {
        *entry = Value::Mapping(Mapping::new());
    }
    entry.as_mapping_mut().expect("a mapping")
}

/// The final config: profile + override + Kairo's settings.
pub fn build(
    profile: &str,
    override_text: &str,
    settings: &Settings,
) -> Result<String, ConfigError> {
    build_moving(profile, override_text, settings, &|_| false)
}

/// [`build`], with the profile's own local ports that `taken` says another
/// program holds moved to free ones (see [`move_taken_inbounds`]).
pub fn build_moving(
    profile: &str,
    override_text: &str,
    settings: &Settings,
    taken: &dyn Fn(u16) -> bool,
) -> Result<String, ConfigError> {
    build_applied(profile, override_text, settings, taken).map(|(text, _)| text)
}

/// [`build_moving`], and what the AI layer did (its group, whether the
/// subscription had one).
pub fn build_applied(
    profile: &str,
    override_text: &str,
    settings: &Settings,
    taken: &dyn Fn(u16) -> bool,
) -> Result<(String, Option<crate::ai::Applied>), ConfigError> {
    build_full(profile, override_text, settings, taken).map(|b| (b.text, b.applied))
}

/// A built config and what Kairo did to it.
#[derive(Debug, Clone)]
pub struct Built {
    pub text: String,
    /// What the AI layer did (`None`: it is off, or there was nothing to route through).
    pub applied: Option<crate::ai::Applied>,
    /// Routes (ids) left out because their exit is not in the config.
    pub skipped_routes: Vec<String>,
}

/// [`build_applied`], with the routes' outcome too.
pub fn build_full(
    profile: &str,
    override_text: &str,
    settings: &Settings,
    taken: &dyn Fn(u16) -> bool,
) -> Result<Built, ConfigError> {
    let mut config = parse_mapping(profile).map_err(ConfigError::Profile)?;
    let avoid = [settings.mixed_port, settings.controller_port];
    for (from, to) in move_taken_inbounds(&mut config, taken, &avoid) {
        log::info!("the profile's local port {from} is taken: the core uses {to}");
    }
    let patch = parse_mapping(override_text).map_err(ConfigError::Override)?;
    // AI on its own exit, local models off the proxy, the person's routes for
    // apps and sites. It goes in before the override, so the override's
    // `prepend-rules` still come first and a `rules:` of its own replaces
    // everything, as it always did. "All Traffic" is then rules with the
    // Location as the rest (see `core_mode`).
    let all_traffic = settings.mode == crate::settings::Mode::Global;
    let applied = crate::ai::apply_with(
        &mut config,
        &settings.ai,
        &settings.routes,
        &settings.tray_group,
        all_traffic,
    );
    let skipped_routes = match &applied {
        Some(a) => a.skipped_routes.clone(),
        // The layer is off: the routes still apply.
        None => crate::ai::apply_routes(&mut config, &settings.routes, &settings.tray_group),
    };
    merge(&mut config, patch);
    apply_settings(&mut config, settings);
    // No group of locations to send the rest through (a bare list of nodes):
    // the core's own global mode is the only "All Traffic" there is.
    if all_traffic && settings.ai.enabled && applied.as_ref().is_none_or(|a| a.main.is_none()) {
        set(&mut config, "mode", "global");
    }
    if settings.allow_lan {
        guard_loopback(&mut config);
    }
    Ok(Built {
        text: serde_yaml::to_string(&Value::Mapping(config)).expect("yaml serialize"),
        applied,
        skipped_routes,
    })
}

/// The mode the core runs for a setting. mihomo's own `global` ignores the
/// rules, which would let AI traffic follow the Location; with the AI layer
/// on, "All Traffic" is the `rule` mode over rules that send everything
/// through the Location.
pub fn core_mode(settings: &Settings) -> &'static str {
    use crate::settings::Mode;
    if settings.ai.enabled && settings.mode == Mode::Global {
        Mode::Rule.as_str()
    } else {
        settings.mode.as_str()
    }
}

/// The port in "127.0.0.1:7874", ":1053" or "[::1]:53".
fn port_of(addr: &str) -> Option<u16> {
    addr.rsplit(':').next()?.trim().parse().ok()
}

/// Old single-purpose inbounds: each listens on every address whatever
/// `allow-lan` says (mihomo 1.19.33, tried in a network namespace), and none
/// takes a listen address of its own. `tcptun-config`/`udptun-config` are
/// read by no current core; they go too.
const LEGACY_INBOUNDS: [&str; 5] = ["ss-config", "vmess-config", "tuic-server", "tcptun-config", "udptun-config"];

/// "Allow other devices on the network" off holds for every inbound of the
/// config, the profile's or the override's: its DNS server (`dns.listen`,
/// often `0.0.0.0:1053`), extra `listeners` and port `tunnels` answer on this
/// machine only (each of them listens on every address otherwise, whatever
/// `allow-lan` says), and the old inbounds that take no address go.
fn keep_listeners_local(config: &mut Mapping) {
    if let Some(Value::Mapping(dns)) = config.get_mut("dns")
        && let Some(Value::String(listen)) = dns.get_mut("listen")
    {
        *listen = on_loopback(listen);
    }
    if let Some(Value::Sequence(ls)) = config.get_mut("listeners") {
        for l in ls.iter_mut().filter_map(Value::as_mapping_mut) {
            let listen = l.get("listen").and_then(Value::as_str).unwrap_or("0.0.0.0");
            if !is_loopback_host(listen) {
                l.insert("listen".into(), "127.0.0.1".into());
            }
        }
    }
    if let Some(Value::Sequence(ts)) = config.get_mut("tunnels") {
        for t in ts.iter_mut() {
            tunnel_on_loopback(t);
        }
    }
    for key in LEGACY_INBOUNDS {
        if config.remove(key).is_some() {
            log::warn!("the config's `{key}` listens on every address: left out (other devices are not allowed)");
        }
    }
}

/// A `tunnels` entry, "tcp/udp,0.0.0.0:6553,1.1.1.1:53,proxy" or
/// `{network, address, target, proxy}`, on this machine only.
fn tunnel_on_loopback(t: &mut Value) {
    match t {
        Value::String(line) => {
            let mut parts: Vec<String> = line.split(',').map(|p| p.trim().to_owned()).collect();
            if let Some(addr) = parts.get_mut(1) {
                *addr = on_loopback(addr);
            }
            *line = parts.join(",");
        }
        Value::Mapping(m) => {
            if let Some(Value::String(addr)) = m.get_mut("address") {
                *addr = on_loopback(addr);
            }
        }
        _ => {}
    }
}

fn tunnel_address(t: &Value) -> Option<String> {
    match t {
        Value::String(line) => line.split(',').nth(1).map(|a| a.trim().to_owned()),
        Value::Mapping(m) => m.get("address").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

fn tunnel_has_proxy(t: &Value) -> bool {
    match t {
        Value::String(line) => line.split(',').nth(3).is_some_and(|p| !p.trim().is_empty()),
        Value::Mapping(m) => m.get("proxy").and_then(Value::as_str).is_some_and(|p| !p.is_empty()),
        _ => false,
    }
}

fn host_of(addr: &str) -> &str {
    match addr.rfind(':') {
        Some(i) => &addr[..i],
        None => addr,
    }
}

/// A TUN inbound of the config's own: it would take this machine's routes
/// beside Kairo's TUN (or with no TUN on). Never kept.
fn drop_tun_listeners(config: &mut Mapping) {
    if let Some(Value::Sequence(ls)) = config.get_mut("listeners") {
        let before = ls.len();
        ls.retain(|l| !l.get("type").and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("tun")));
        if ls.len() != before {
            log::warn!("the config's `listeners` of type tun are left out: Kairo's TUN setting is the only one");
        }
    }
}

/// Connections from other devices to this machine's own loopback (127/8,
/// ::1, 0.0.0.0, `localhost` names, and names that resolve there) are refused:
/// "Allow other devices" opens the proxy, not this machine's local services.
/// The source is checked first, so a local program's connection is never
/// resolved for it.
pub const LOOPBACK_GUARD: &str = "AND,((NOT,((SRC-IP-CIDR,127.0.0.0/8))),(NOT,((SRC-IP-CIDR,::1/128))),(OR,((DOMAIN,localhost),(DOMAIN-SUFFIX,localhost),(IP-CIDR,127.0.0.0/8),(IP-CIDR,0.0.0.0/8),(IP-CIDR6,::1/128),(IP-CIDR6,::/128)))),REJECT";

/// [`LOOPBACK_GUARD`] while TUN is on: a TCP connection TUN captured from
/// this machine is local too. It comes in through TUN (`IN-TYPE,TUN`) from
/// TUN's own addresses, which only this machine has: with TUN up those
/// addresses route into the TUN device, so a device on the network that
/// forged one would never get a TCP handshake's answer. UDP has no handshake:
/// a datagram this machine forwards into TUN for a device routing through it
/// (IP forwarding on, as Docker sets it) could carry a forged source in TUN's
/// range, and nothing here tells it from a local program's. So UDP gets no
/// exemption (`NETWORK,TCP`): local UDP to a name that resolves to loopback
/// through TUN is refused too (SECURITY-NOTES item 10).
pub fn loopback_guard_with_tun(tun_v4: &str, tun_v6: &str) -> String {
    format!(
        "AND,((NOT,((SRC-IP-CIDR,127.0.0.0/8))),(NOT,((SRC-IP-CIDR,::1/128))),(NOT,((AND,((NETWORK,TCP),(IN-TYPE,TUN),(OR,((SRC-IP-CIDR,{tun_v4}),(SRC-IP-CIDR,{tun_v6}))))))),(OR,((DOMAIN,localhost),(DOMAIN-SUFFIX,localhost),(IP-CIDR,127.0.0.0/8),(IP-CIDR,0.0.0.0/8),(IP-CIDR6,::1/128),(IP-CIDR6,::/128)))),REJECT"
    )
}

/// TUN's own addresses in `config` as mihomo 1.19.33 sets them: IPv4 is the
/// fake-IP range's first address with a /30 (`tun.inet4-address` is not read),
/// IPv6 `tun.inet6-address` or its default.
fn tun_ranges(config: &Mapping) -> (String, String) {
    let dns = config.get("dns").and_then(Value::as_mapping);
    let v4 = dns
        .and_then(|d| d.get("fake-ip-range"))
        .and_then(Value::as_str)
        .and_then(|r| r.split('/').next()?.trim().parse::<std::net::Ipv4Addr>().ok())
        .unwrap_or(std::net::Ipv4Addr::new(198, 18, 0, 1));
    let v6 = config
        .get("tun")
        .and_then(Value::as_mapping)
        .and_then(|t| t.get("inet6-address"))
        .and_then(|a| match a {
            Value::Sequence(s) => s.first().and_then(Value::as_str).map(str::to_owned),
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .filter(|a| a.split('/').next().is_some_and(|h| h.trim().parse::<std::net::Ipv6Addr>().is_ok()))
        .unwrap_or_else(|| "fdfe:dcba:9876::1/126".to_owned());
    (format!("{v4}/30"), v6)
}

/// With other devices allowed, the core runs its `rule` mode always, so the
/// [`LOOPBACK_GUARD`] comes before anything: mihomo's `direct` and `global`
/// modes, and an inbound's own `proxy`, skip every rule. Direct is then
/// "everything DIRECT", All Traffic "everything through GLOBAL"; a listener
/// on the network with a `proxy` of its own becomes an `IN-NAME` rule (the
/// same route), a port tunnel on the network with one answers this machine
/// only, and every list of `sub-rules` (a listener's `rule:`) starts with the
/// guard too.
fn guard_loopback(config: &mut Mapping) {
    let tun_on = config.get("tun").and_then(Value::as_mapping).and_then(|t| t.get("enable")).and_then(Value::as_bool) == Some(true);
    let guard: String = if tun_on {
        let (v4, v6) = tun_ranges(config);
        loopback_guard_with_tun(&v4, &v6)
    } else {
        LOOPBACK_GUARD.to_owned()
    };
    let mut first: Vec<Value> = vec![guard.clone().into()];
    if let Some(Value::Sequence(ls)) = config.get_mut("listeners") {
        for l in ls.iter_mut().filter_map(Value::as_mapping_mut) {
            let listen = l.get("listen").and_then(Value::as_str).unwrap_or("0.0.0.0").to_owned();
            if is_loopback_host(&listen) {
                continue;
            }
            let Some(proxy) = l.get("proxy").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned) else {
                continue;
            };
            match l.get("name").and_then(Value::as_str).filter(|n| !n.is_empty() && !n.contains(',')) {
                Some(name) => {
                    first.push(format!("IN-NAME,{name},{proxy}").into());
                    l.remove("proxy");
                }
                None => {
                    l.insert("listen".into(), "127.0.0.1".into());
                }
            }
        }
    }
    if let Some(Value::Sequence(ts)) = config.get_mut("tunnels") {
        for t in ts.iter_mut() {
            let on_network = tunnel_address(t).is_some_and(|a| !is_loopback_host(host_of(&a)));
            if on_network && tunnel_has_proxy(t) {
                log::warn!("a port tunnel with a proxy of its own skips the rules: it answers this machine only");
                tunnel_on_loopback(t);
            }
        }
    }
    let mode = config.get("mode").and_then(Value::as_str).unwrap_or("rule").to_ascii_lowercase();
    let rest: Vec<Value> = match mode.as_str() {
        "direct" => vec!["MATCH,DIRECT".into()],
        "global" => vec!["MATCH,GLOBAL".into()],
        _ => match config.get("rules") {
            Some(Value::Sequence(r)) => r.clone(),
            _ => Vec::new(),
        },
    };
    first.extend(rest);
    set(config, "rules", Value::Sequence(first));
    set(config, "mode", "rule");
    // A listener's `rule: <name>` runs that list of `sub-rules` in place of
    // the rules: the guard heads each of them too.
    if let Some(Value::Mapping(subs)) = config.get_mut("sub-rules") {
        for list in subs.values_mut() {
            if let Value::Sequence(rules) = list {
                rules.insert(0, guard.clone().into());
            }
        }
    }
}

fn is_loopback_host(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    h == "localhost" || h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// "0.0.0.0:1053", ":1053" or "[::]:53" on this machine only; a loopback
/// address stays as it is.
fn on_loopback(addr: &str) -> String {
    let host = match addr.rfind(':') {
        Some(i) => &addr[..i],
        None => return addr.to_owned(),
    };
    if is_loopback_host(host) {
        addr.to_owned()
    } else {
        match port_of(addr) {
            Some(port) => format!("127.0.0.1:{port}"),
            None => addr.to_owned(),
        }
    }
}

fn with_port(addr: &str, port: u16) -> String {
    match addr.rfind(':') {
        Some(i) => format!("{}:{port}", &addr[..i]),
        None => format!(":{port}"),
    }
}

/// A provider's config may open local ports of its own: a DNS listener
/// (`dns.listen`, often 127.0.0.1:7874 or :1053), extra `listeners`, port
/// `tunnels`. Another client running the same subscription holds the same
/// ports, and the core would start without them ("address already in use").
/// They cannot simply go: the profile's own DNS settings may point at them
/// (`proxy-server-nameserver: [udp://127.0.0.1:7874]` resolves every node's
/// host through the listener). So each port `taken` says is held, or that is
/// one of Kairo's own (`avoid`), moves to the next free one, and every DNS
/// server address that names it follows. Runs on the profile only: an
/// override's ports are the user's. The moves, as (from, to).
pub fn move_taken_inbounds(
    config: &mut Mapping,
    taken: &dyn Fn(u16) -> bool,
    avoid: &[u16],
) -> Vec<(u16, u16)> {
    // Where each inbound's address lives, read once.
    let mut ports: Vec<u16> = Vec::new();
    if let Some(p) = config
        .get("dns")
        .and_then(|d| d.get("listen"))
        .and_then(Value::as_str)
        .and_then(port_of)
    {
        ports.push(p);
    }
    if let Some(Value::Sequence(ls)) = config.get("listeners") {
        for l in ls {
            if let Some(p) = l.get("port").and_then(|v| {
                v.as_u64()
                    .and_then(|n| u16::try_from(n).ok())
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            }) {
                ports.push(p);
            }
        }
    }
    if let Some(Value::Sequence(ts)) = config.get("tunnels") {
        for t in ts {
            let addr = match t {
                Value::String(s) => s.split(',').nth(1).map(str::to_owned),
                Value::Mapping(m) => m.get("address").and_then(Value::as_str).map(str::to_owned),
                _ => None,
            };
            if let Some(p) = addr.as_deref().and_then(port_of) {
                ports.push(p);
            }
        }
    }
    let mut moves: Vec<(u16, u16)> = Vec::new();
    for &p in &ports {
        if moves.iter().any(|m| m.0 == p) || !(avoid.contains(&p) || taken(p)) {
            continue;
        }
        let free = (p.saturating_add(1)..p.saturating_add(200)).find(|q| {
            !avoid.contains(q)
                && !ports.contains(q)
                && !moves.iter().any(|m| m.1 == *q)
                && !taken(*q)
        });
        if let Some(q) = free {
            moves.push((p, q));
        }
    }
    if moves.is_empty() {
        return moves;
    }
    let moved = |p: u16| moves.iter().find(|m| m.0 == p).map(|m| m.1);
    if let Some(Value::Mapping(dns)) = config.get_mut("dns") {
        if let Some(Value::String(l)) = dns.get_mut("listen")
            && let Some(q) = port_of(l).and_then(moved)
        {
            *l = with_port(l, q);
        }
        for (_, v) in dns.iter_mut() {
            repoint(v, &moves);
        }
    }
    if let Some(Value::Sequence(ls)) = config.get_mut("listeners") {
        for l in ls {
            if let Value::Mapping(m) = l
                && let Some(v) = m.get_mut("port")
            {
                let p = v
                    .as_u64()
                    .and_then(|n| u16::try_from(n).ok())
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()));
                if let Some(q) = p.and_then(moved) {
                    *v = Value::from(q);
                }
            }
        }
    }
    if let Some(Value::Sequence(ts)) = config.get_mut("tunnels") {
        for t in ts {
            match t {
                Value::String(s) => {
                    let mut f: Vec<String> = s.split(',').map(str::to_owned).collect();
                    if let Some(a) = f.get_mut(1)
                        && let Some(q) = port_of(a).and_then(moved)
                    {
                        *a = with_port(a, q);
                        *s = f.join(",");
                    }
                }
                Value::Mapping(m) => {
                    if let Some(Value::String(a)) = m.get_mut("address")
                        && let Some(q) = port_of(a).and_then(moved)
                    {
                        *a = with_port(a, q);
                    }
                }
                _ => {}
            }
        }
    }
    moves
}

/// Points the DNS server addresses in `v` ("udp://127.0.0.1:7874",
/// "127.0.0.1:7874#DIRECT", "tcp://localhost:7874") at the moved ports. Only
/// this machine's addresses: a remote server on the same port is another one.
fn repoint(v: &mut Value, moves: &[(u16, u16)]) {
    match v {
        Value::String(s) => {
            for host in ["127.0.0.1", "localhost", "[::1]", "0.0.0.0"] {
                for &(from, to) in moves {
                    let pat = format!("{host}:{from}");
                    let mut out = String::new();
                    let mut rest = s.as_str();
                    while let Some(i) = rest.find(&pat) {
                        let after = &rest[i + pat.len()..];
                        let before_ok = i == 0
                            || !rest[..i]
                                .chars()
                                .last()
                                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '.');
                        let after_ok = !after.starts_with(|c: char| c.is_ascii_digit());
                        out.push_str(&rest[..i]);
                        if before_ok && after_ok {
                            out.push_str(&format!("{host}:{to}"));
                        } else {
                            out.push_str(&pat);
                        }
                        rest = after;
                    }
                    out.push_str(rest);
                    *s = out;
                }
            }
        }
        Value::Sequence(seq) => seq.iter_mut().for_each(|x| repoint(x, moves)),
        Value::Mapping(m) => m.iter_mut().for_each(|(_, x)| repoint(x, moves)),
        _ => {}
    }
}

/// Where the core fetches the GeoIP and GeoSite data it lacks: jsDelivr's
/// mirrors of MetaCubeX's releases, which answer where GitHub is slow (as
/// Clash Verge Rev does). A profile or override that names its own wins.
pub const GEOX_MIRROR: &str = "https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release";

/// Writes Kairo's settings over a config.
pub fn apply_settings(config: &mut Mapping, s: &Settings) {
    // One inbound, on the port the system proxy uses; profile ports would clash
    // with other clients.
    for key in [
        "port",
        "socks-port",
        "redir-port",
        "tproxy-port",
        "external-controller-unix",
        "external-controller-tls",
        "external-controller-pipe",
    ] {
        config.remove(key);
    }
    set(config, "mixed-port", s.mixed_port);
    // The Activity view names the tool behind each connection, which the core
    // looks up only when asked to. A profile that names its own mode wins.
    if s.ai.enabled && !config.contains_key("find-process-mode") {
        set(config, "find-process-mode", "always");
    }
    set(config, "allow-lan", s.allow_lan);
    drop_tun_listeners(config);
    if s.allow_lan {
        set(config, "bind-address", "*");
    } else {
        keep_listeners_local(config);
    }
    set(config, "ipv6", s.ipv6);
    set(config, "mode", core_mode(s));
    set(config, "log-level", s.log_level.as_str());
    set(
        config,
        "external-controller",
        format!("127.0.0.1:{}", s.controller_port),
    );
    set(config, "secret", s.secret.clone());
    if !config.contains_key("geox-url") {
        let geox = sub(config, "geox-url");
        for (key, file) in [
            ("geoip", "geoip.dat"),
            ("geosite", "geosite.dat"),
            ("mmdb", "geoip.metadb"),
            ("asn", "GeoLite2-ASN.mmdb"),
        ] {
            set(geox, key, format!("{GEOX_MIRROR}/{file}"));
        }
    }
    // Remember the nodes picked in select groups across restarts.
    let profile = sub(config, "profile");
    set(profile, "store-selected", true);

    let tun = sub(config, "tun");
    set(tun, "enable", s.tun);
    if s.tun {
        set(tun, "stack", s.tun_stack.clone());
        if !tun.contains_key("auto-route") {
            set(tun, "auto-route", true);
        }
        if !tun.contains_key("auto-detect-interface") {
            set(tun, "auto-detect-interface", true);
        }
        if !tun.contains_key("dns-hijack") {
            set(
                tun,
                "dns-hijack",
                Value::Sequence(vec!["any:53".into(), "tcp://any:53".into()]),
            );
        }
        // TUN needs the core's DNS to resolve what it captures.
        let dns = sub(config, "dns");
        let enabled = dns.get("enable").and_then(Value::as_bool).unwrap_or(false);
        if !enabled {
            set(dns, "enable", true);
            if !dns.contains_key("enhanced-mode") {
                set(dns, "enhanced-mode", "fake-ip");
                set(dns, "fake-ip-range", "198.18.0.1/16");
            }
            if !dns.contains_key("nameserver") {
                set(
                    dns,
                    "nameserver",
                    Value::Sequence(vec![
                        "223.5.5.5".into(),
                        "119.29.29.29".into(),
                        "8.8.8.8".into(),
                    ]),
                );
            }
        }
    } else if !config.contains_key("dns") {
        // Without a `dns` section the core resolves with the system's resolver
        // but its DNS API is off. The same resolver, as the core's DNS, keeps
        // lookups as they were and lets the DNS tool ask it.
        let dns = sub(config, "dns");
        set(dns, "enable", true);
        set(dns, "ipv6", s.ipv6);
        set(dns, "enhanced-mode", "redir-host");
        set(dns, "nameserver", Value::Sequence(vec!["system".into()]));
    }
    // Android: the tun device is the VPN service's, handed over as a
    // descriptor; the core must not touch routes or look for an interface.
    #[cfg(target_os = "android")]
    if s.tun
        && let Some(fd) = crate::android::tun_fd()
    {
        let tun = sub(config, "tun");
        set(tun, "file-descriptor", fd as i64);
        set(tun, "auto-route", false);
        set(tun, "auto-detect-interface", false);
        set(tun, "stack", "gvisor");
        set(tun, "inet4-address", Value::Sequence(vec!["172.19.0.1/30".into()]));
        set(tun, "dns-hijack", Value::Sequence(vec!["any:53".into(), "tcp://any:53".into()]));
        set(config, "find-process-mode", "off");
    }
}

/// Asks the core whether `config` (a file in `dir`) is valid. The error is the
/// core's own words, trimmed to the lines that explain it.
pub async fn validate(core: &Path, dir: &Path, config: &Path) -> Result<(), String> {
    let mut command = tokio::process::Command::new(core);
    command
        .arg("-t")
        .arg("-d")
        .arg(dir)
        .arg("-f")
        .arg(config)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    // No console window flashing up (Kairo is a windowed program).
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let output = match tokio::time::timeout(Duration::from_secs(90), command.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("Could not run the core: {e}")),
        Err(_) => return Err("The core took too long to check the config".into()),
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() && text.contains("test is successful") {
        return Ok(());
    }
    Err(explain(&text))
}

/// The useful part of the core's output: error lines, without timestamps.
pub fn explain(output: &str) -> String {
    let mut lines: Vec<String> = output
        .lines()
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("error")
                || l.contains("fatal")
                || l.contains("failed")
                || l.contains("invalid")
        })
        .map(|l| {
            // logrus: time="..." level=error msg="..."
            if let Some(i) = l.find("msg=") {
                l[i + 4..].trim_matches('"').replace("\\\"", "\"")
            } else {
                l.trim().to_owned()
            }
        })
        .filter(|l| !l.is_empty())
        .collect();
    lines.dedup();
    if lines.is_empty() {
        let tail: Vec<&str> = output.lines().rev().take(5).collect();
        return tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    }
    lines.truncate(6);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_yaml_whose_aliases_unfold_too_far_is_refused_without_unfolding_it() {
        // Three levels of a thousand: under 50 KB of text, a thousand million
        // nodes unfolded, 200 times the limit. Counting to the limit takes a
        // few seconds in a debug build on a slow machine (twice here); going
        // through all of it would take hundreds of times that.
        let items = |item: &str| vec![item; 1_000].join(",");
        let text = format!("a: &a [{}]\nb: &b [{}]\nc: [{}]\n", items("x"), items("*a"), items("*b"));
        assert!(text.len() < 50_000);
        let started = std::time::Instant::now();
        let e = parse_mapping(&text).unwrap_err();
        assert!(e.contains("unfolds too far"), "{e}");
        assert!(matches!(crate::profile::inspect(&text), Err(crate::profile::ProfileError::Yaml(e)) if e.contains("unfolds too far")));
        assert!(started.elapsed() < Duration::from_secs(120), "stopped at the limit: {:?}", started.elapsed());
        // Aliases as configs use them pass.
        let normal = "base: &b {type: ss, port: 1}\nproxies:\n  - {<<: *b, name: a}\n  - {<<: *b, name: c}\n";
        assert!(parse_mapping(normal).is_ok());
        assert!(check_yaml_size(PROFILE).is_ok());
    }

    #[test]
    fn provider_links_are_found_for_redaction() {
        let text = "proxy-providers:\n  a: {type: http, url: 'https://p.example/sub?token=AAA', path: a.yaml}\n  b: {type: file, path: b.yaml}\nrule-providers:\n  r: {type: http, url: 'https://r.example/x?k=BBB', behavior: domain, path: r.yaml}\n";
        let mut urls = provider_urls(text);
        urls.sort();
        assert_eq!(urls, ["https://p.example/sub?token=AAA", "https://r.example/x?k=BBB"]);
        assert!(provider_urls("not: [yaml").is_empty());
    }

    #[test]
    fn without_the_network_the_profiles_own_listeners_answer_this_machine_only() {
        let text = "dns:\n  enable: true\n  listen: 0.0.0.0:1053\nlisteners:\n  - {name: a, type: mixed, port: 7777}\n  - {name: b, type: socks, port: 7778, listen: '::1'}\n  - {name: c, type: http, port: 7779, listen: 192.168.1.2}\n";
        let mut config = parse_mapping(text).unwrap();
        apply_settings(&mut config, &Settings::default());
        let dns = config.get("dns").unwrap().as_mapping().unwrap();
        assert_eq!(dns.get("listen").unwrap().as_str(), Some("127.0.0.1:1053"));
        let ls = config.get("listeners").unwrap().as_sequence().unwrap();
        let listen: Vec<_> = ls.iter().map(|l| l.get("listen").unwrap().as_str().unwrap()).collect();
        assert_eq!(listen, ["127.0.0.1", "::1", "127.0.0.1"]);
        assert_eq!(on_loopback(":53"), "127.0.0.1:53");
        assert_eq!(on_loopback("[::]:53"), "127.0.0.1:53");
        assert_eq!(on_loopback("127.0.0.1:7874"), "127.0.0.1:7874");
        // On the network, as the profile says.
        let mut config = parse_mapping(text).unwrap();
        apply_settings(&mut config, &Settings { allow_lan: true, ..Settings::default() });
        let dns = config.get("dns").unwrap().as_mapping().unwrap();
        assert_eq!(dns.get("listen").unwrap().as_str(), Some("0.0.0.0:1053"));
    }

    const PROFILE: &str = r#"
port: 7890
socks-port: 7891
mixed-port: 1234
external-controller: 0.0.0.0:9090
secret: theirs
dns:
  enable: true
  nameserver: [1.1.1.1]
proxies:
  - {name: a, type: direct}
proxy-groups:
  - {name: G, type: select, proxies: [a, DIRECT]}
rules:
  - DOMAIN,example.com,G
  - MATCH,DIRECT
"#;

    fn get<'a>(m: &'a Mapping, key: &str) -> &'a Value {
        m.get(key).unwrap_or_else(|| panic!("missing {key}"))
    }

    #[test]
    fn a_profiles_taken_local_ports_move_and_its_dns_follows() {
        // The shape of a real subscription: the nodes' hosts resolve through
        // the profile's own DNS listener.
        let profile = "dns:\n  enable: true\n  listen: 127.0.0.1:7874\n  nameserver: [1.1.1.1, 'tls://9.9.9.9:7874']\n  proxy-server-nameserver: ['udp://127.0.0.1:7874']\n  nameserver-policy:\n    '+.lan': '127.0.0.1:7874#DIRECT'\nlisteners:\n  - {name: in, type: mixed, port: 7777}\ntunnels:\n  - tcp,127.0.0.1:6553,1.1.1.1:53,DIRECT\n  - {network: [udp], address: '127.0.0.1:6554', target: '1.1.1.1:53', proxy: DIRECT}\nrules:\n  - MATCH,DIRECT\n";
        // Nothing taken: all as the profile has it.
        let m = parse_mapping(&build(profile, "", &Settings::default()).unwrap()).unwrap();
        let dns = get(&m, "dns").as_mapping().unwrap();
        assert_eq!(get(dns, "listen").as_str(), Some("127.0.0.1:7874"));
        assert_eq!(get(&m, "listeners").as_sequence().unwrap().len(), 1);
        // Another client holds 7874, 7777, 6553 and 6554.
        let taken = |p: u16| [7874, 7875, 7777, 6553, 6554].contains(&p);
        let out = build_moving(profile, "", &Settings::default(), &taken).unwrap();
        let m = parse_mapping(&out).unwrap();
        let dns = get(&m, "dns").as_mapping().unwrap();
        assert_eq!(get(dns, "listen").as_str(), Some("127.0.0.1:7876"));
        assert_eq!(
            get(dns, "proxy-server-nameserver").as_sequence().unwrap()[0].as_str(),
            Some("udp://127.0.0.1:7876")
        );
        let policy = get(dns, "nameserver-policy").as_mapping().unwrap();
        assert_eq!(get(policy, "+.lan").as_str(), Some("127.0.0.1:7876#DIRECT"));
        // A server elsewhere on the same port is another server.
        assert_eq!(
            get(dns, "nameserver").as_sequence().unwrap()[1].as_str(),
            Some("tls://9.9.9.9:7874")
        );
        let l = &get(&m, "listeners").as_sequence().unwrap()[0];
        assert_eq!(l.get("port").and_then(Value::as_u64), Some(7778));
        let t = get(&m, "tunnels").as_sequence().unwrap();
        assert_eq!(t[0].as_str(), Some("tcp,127.0.0.1:6555,1.1.1.1:53,DIRECT"));
        assert_eq!(
            t[1].get("address").and_then(Value::as_str),
            Some("127.0.0.1:6556")
        );
        // A listener on Kairo's own proxy port moves too.
        let mut s = Settings::default();
        s.mixed_port = 7874;
        let m = parse_mapping(&build(profile, "", &s).unwrap()).unwrap();
        let dns = get(&m, "dns").as_mapping().unwrap();
        assert_eq!(get(dns, "listen").as_str(), Some("127.0.0.1:7875"));
        // An override's own listener is the user's: kept.
        let out = build_moving(
            profile,
            "dns:\n  listen: 127.0.0.1:5353\n",
            &Settings::default(),
            &|p| p == 5353,
        )
        .unwrap();
        let m = parse_mapping(&out).unwrap();
        assert_eq!(
            get(get(&m, "dns").as_mapping().unwrap(), "listen").as_str(),
            Some("127.0.0.1:5353")
        );
        // Geo data from the mirror, unless the profile names a source.
        let geox = get(&m, "geox-url").as_mapping().unwrap();
        assert!(get(geox, "mmdb").as_str().unwrap().starts_with(GEOX_MIRROR));
        let out = build(
            "geox-url:\n  mmdb: https://example.com/x.metadb\n",
            "",
            &Settings::default(),
        )
        .unwrap();
        let m = parse_mapping(&out).unwrap();
        let geox = get(&m, "geox-url").as_mapping().unwrap();
        assert_eq!(
            get(geox, "mmdb").as_str(),
            Some("https://example.com/x.metadb")
        );
    }

    #[test]
    fn the_guard_lets_tuns_own_connections_through_only_while_tun_is_on() {
        let guard_of = |profile: &str, s: &Settings| {
            let m = parse_mapping(&build(profile, "listeners:\n  - {name: own, type: http, port: 17999, rule: r}\nsub-rules:\n  r:\n    - MATCH,DIRECT\n", s).unwrap()).unwrap();
            let rules = get(&m, "rules").as_sequence().unwrap().clone();
            let sub = get(get(&m, "sub-rules").as_mapping().unwrap(), "r").as_sequence().unwrap()[0].as_str().unwrap().to_owned();
            (rules[0].as_str().unwrap().to_owned(), rules, sub)
        };
        let off = Settings { allow_lan: true, tun: false, ..Settings::default() };
        let (first, rules, sub) = guard_of(PROFILE, &off);
        assert_eq!(first, LOOPBACK_GUARD);
        assert_eq!(sub, LOOPBACK_GUARD);
        assert!(rules.iter().all(|r| !r.as_str().unwrap_or_default().contains("IN-TYPE,TUN")), "no exemption with TUN off");

        let on = Settings { allow_lan: true, tun: true, ..Settings::default() };
        let (first, _, sub) = guard_of(PROFILE, &on);
        assert_eq!(first, loopback_guard_with_tun("198.18.0.1/30", "fdfe:dcba:9876::1/126"));
        assert_eq!(sub, first);
        let balanced = |s: &str| {
            s.chars().try_fold(0i32, |d, c| {
                let d = d + (c == '(') as i32 - (c == ')') as i32;
                (d >= 0).then_some(d)
            }) == Some(0)
        };
        assert!(balanced(LOOPBACK_GUARD) && balanced(&first), "{first}");
        // TCP only: a datagram forwarded into TUN may carry a forged source in its range.
        assert!(first.contains("(NOT,((AND,((NETWORK,TCP),(IN-TYPE,TUN),"), "{first}");
        // TUN's address follows the fake-IP range, as the core's does.
        let own_range = PROFILE.replacen("dns:\n  enable: true\n", "dns:\n  enable: true\n  enhanced-mode: fake-ip\n  fake-ip-range: 10.200.0.1/16\n", 1);
        assert_ne!(own_range, PROFILE);
        let (first, _, _) = guard_of(&own_range, &on);
        assert!(first.contains("(SRC-IP-CIDR,10.200.0.1/30)"), "{first}");
        // Without other devices there is no guard at all.
        let alone = Settings { allow_lan: false, tun: true, ..Settings::default() };
        let m = parse_mapping(&build(PROFILE, "", &alone).unwrap()).unwrap();
        assert!(get(&m, "rules").as_sequence().unwrap().iter().all(|r| !r.as_str().unwrap_or_default().contains("REJECT") || !r.as_str().unwrap_or_default().contains("127.0.0.0/8")));
    }

    #[test]
    fn settings_win_over_the_profile() {
        let mut s = Settings::default();
        s.secret = "mine".into();
        s.mixed_port = 17890;
        s.controller_port = 19090;
        let out = build(PROFILE, "", &s).unwrap();
        let m = parse_mapping(&out).unwrap();
        assert!(m.get("port").is_none());
        assert!(m.get("socks-port").is_none());
        assert_eq!(get(&m, "mixed-port").as_u64(), Some(17890));
        assert_eq!(
            get(&m, "external-controller").as_str(),
            Some("127.0.0.1:19090")
        );
        assert_eq!(get(&m, "secret").as_str(), Some("mine"));
        assert_eq!(get(&m, "mode").as_str(), Some("rule"));
        assert_eq!(
            get(get(&m, "tun").as_mapping().unwrap(), "enable").as_bool(),
            Some(false)
        );
        // Untouched parts stay: the profile's own rules come last, after
        // the AI layer's.
        let rules = get(&m, "rules").as_sequence().unwrap();
        assert!(rules.len() > 2);
        let tail: Vec<_> = rules[rules.len() - 2..]
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let profile = parse_mapping(PROFILE).unwrap();
        let own: Vec<_> = get(&profile, "rules")
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(tail, own);
        // With the layer off the config is the profile's own.
        s.ai.enabled = false;
        let out = build(PROFILE, "", &s).unwrap();
        assert_eq!(
            get(&parse_mapping(&out).unwrap(), "rules")
                .as_sequence()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn overrides_merge_and_extend_lists() {
        let over = r#"
dns:
  nameserver: [9.9.9.9]
  ipv6: false
prepend-rules:
  - DOMAIN,kairo.dev,DIRECT
append-proxies:
  - {name: b, type: direct}
"#;
        let mut plain = Settings::default();
        plain.ai.enabled = false;
        let out = build(PROFILE, over, &plain).unwrap();
        let m = parse_mapping(&out).unwrap();
        let dns = get(&m, "dns").as_mapping().unwrap();
        assert_eq!(
            get(dns, "enable").as_bool(),
            Some(true),
            "kept from the profile"
        );
        assert_eq!(
            get(dns, "nameserver").as_sequence().unwrap()[0].as_str(),
            Some("9.9.9.9")
        );
        let rules = get(&m, "rules").as_sequence().unwrap();
        assert_eq!(rules[0].as_str(), Some("DOMAIN,kairo.dev,DIRECT"));
        assert_eq!(rules.len(), 3);
        assert_eq!(get(&m, "proxies").as_sequence().unwrap().len(), 2);
        assert!(m.get("prepend-rules").is_none());
        // With the AI layer on, the person's own rule still comes first.
        let out = build(PROFILE, over, &Settings::default()).unwrap();
        let rules = get(&parse_mapping(&out).unwrap(), "rules")
            .as_sequence()
            .unwrap()
            .clone();
        assert_eq!(rules[0].as_str(), Some("DOMAIN,kairo.dev,DIRECT"));
        assert!(
            rules
                .iter()
                .any(|r| r.as_str().is_some_and(|r| r.contains("claude.ai")))
        );
    }

    #[test]
    fn missing_dns_uses_the_system_resolver() {
        let m = parse_mapping(&build("proxies: []\n", "", &Settings::default()).unwrap()).unwrap();
        let dns = get(&m, "dns").as_mapping().unwrap();
        assert_eq!(get(dns, "enable").as_bool(), Some(true));
        assert_eq!(
            get(dns, "nameserver").as_sequence().unwrap()[0].as_str(),
            Some("system")
        );
        // A profile's own DNS, even off, is left alone.
        let m = parse_mapping(&build("dns: {enable: false}\n", "", &Settings::default()).unwrap())
            .unwrap();
        assert_eq!(
            get(get(&m, "dns").as_mapping().unwrap(), "enable").as_bool(),
            Some(false)
        );
    }

    #[test]
    fn tun_turns_on_dns() {
        let mut s = Settings::default();
        s.tun = true;
        let out = build("proxies: [{name: a, type: direct}]", "", &s).unwrap();
        let m = parse_mapping(&out).unwrap();
        let tun = get(&m, "tun").as_mapping().unwrap();
        assert_eq!(get(tun, "enable").as_bool(), Some(true));
        assert_eq!(get(tun, "auto-route").as_bool(), Some(true));
        assert_eq!(get(tun, "stack").as_str(), Some("mixed"));
        let dns = get(&m, "dns").as_mapping().unwrap();
        assert_eq!(get(dns, "enable").as_bool(), Some(true));
        assert_eq!(get(dns, "enhanced-mode").as_str(), Some("fake-ip"));
    }

    #[test]
    fn bad_yaml_is_reported_by_source() {
        assert!(matches!(
            build("a: [", "", &Settings::default()),
            Err(ConfigError::Profile(_))
        ));
        assert!(matches!(
            build(PROFILE, "b: [", &Settings::default()),
            Err(ConfigError::Override(_))
        ));
        assert!(matches!(
            build("- a\n- b", "", &Settings::default()),
            Err(ConfigError::Profile(_))
        ));
    }

    #[test]
    fn explains_core_errors() {
        let out = "time=\"2026-11-08T10:00:00Z\" level=info msg=\"Start initial configuration in progress\"\ntime=\"2026-11-08T10:00:00Z\" level=error msg=\"proxy group[0]: 'G' not found\"\nconfiguration file /x test failed\n";
        let e = explain(out);
        assert!(e.starts_with("proxy group[0]: 'G' not found"), "{e}");
        assert!(e.contains("test failed"));
    }
}
