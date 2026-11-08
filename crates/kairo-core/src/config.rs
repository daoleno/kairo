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

/// Parses YAML text into a mapping (an empty text is an empty mapping).
pub fn parse_mapping(text: &str) -> Result<Mapping, String> {
    if text.trim().is_empty() {
        return Ok(Mapping::new());
    }
    match serde_yaml::from_str::<Value>(text).map_err(|e| e.to_string())? {
        Value::Mapping(m) => Ok(m),
        Value::Null => Ok(Mapping::new()),
        _ => Err("the top level is not a mapping".into()),
    }
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
pub fn build(profile: &str, override_text: &str, settings: &Settings) -> Result<String, ConfigError> {
    let mut config = parse_mapping(profile).map_err(ConfigError::Profile)?;
    let patch = parse_mapping(override_text).map_err(ConfigError::Override)?;
    merge(&mut config, patch);
    apply_settings(&mut config, settings);
    Ok(serde_yaml::to_string(&Value::Mapping(config)).expect("yaml serialize"))
}

/// Writes Kairo's settings over a config.
pub fn apply_settings(config: &mut Mapping, s: &Settings) {
    // One inbound, on the port the system proxy uses; profile ports would clash
    // with other clients.
    for key in ["port", "socks-port", "redir-port", "tproxy-port", "external-controller-unix", "external-controller-tls", "external-controller-pipe"] {
        config.remove(key);
    }
    set(config, "mixed-port", s.mixed_port);
    set(config, "allow-lan", s.allow_lan);
    if s.allow_lan {
        set(config, "bind-address", "*");
    }
    set(config, "ipv6", s.ipv6);
    set(config, "mode", s.mode.as_str());
    set(config, "log-level", s.log_level.as_str());
    set(config, "external-controller", format!("127.0.0.1:{}", s.controller_port));
    set(config, "secret", s.secret.clone());
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
            set(tun, "dns-hijack", Value::Sequence(vec!["any:53".into(), "tcp://any:53".into()]));
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
                    Value::Sequence(vec!["223.5.5.5".into(), "119.29.29.29".into(), "8.8.8.8".into()]),
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
            l.contains("error") || l.contains("fatal") || l.contains("failed") || l.contains("invalid")
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
        assert_eq!(get(&m, "external-controller").as_str(), Some("127.0.0.1:19090"));
        assert_eq!(get(&m, "secret").as_str(), Some("mine"));
        assert_eq!(get(&m, "mode").as_str(), Some("rule"));
        assert_eq!(get(get(&m, "tun").as_mapping().unwrap(), "enable").as_bool(), Some(false));
        // Untouched parts stay.
        assert_eq!(get(&m, "rules").as_sequence().unwrap().len(), 2);
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
        let out = build(PROFILE, over, &Settings::default()).unwrap();
        let m = parse_mapping(&out).unwrap();
        let dns = get(&m, "dns").as_mapping().unwrap();
        assert_eq!(get(dns, "enable").as_bool(), Some(true), "kept from the profile");
        assert_eq!(get(dns, "nameserver").as_sequence().unwrap()[0].as_str(), Some("9.9.9.9"));
        let rules = get(&m, "rules").as_sequence().unwrap();
        assert_eq!(rules[0].as_str(), Some("DOMAIN,kairo.dev,DIRECT"));
        assert_eq!(rules.len(), 3);
        assert_eq!(get(&m, "proxies").as_sequence().unwrap().len(), 2);
        assert!(m.get("prepend-rules").is_none());
    }

    #[test]
    fn missing_dns_uses_the_system_resolver() {
        let m = parse_mapping(&build("proxies: []\n", "", &Settings::default()).unwrap()).unwrap();
        let dns = get(&m, "dns").as_mapping().unwrap();
        assert_eq!(get(dns, "enable").as_bool(), Some(true));
        assert_eq!(get(dns, "nameserver").as_sequence().unwrap()[0].as_str(), Some("system"));
        // A profile's own DNS, even off, is left alone.
        let m = parse_mapping(&build("dns: {enable: false}\n", "", &Settings::default()).unwrap()).unwrap();
        assert_eq!(get(get(&m, "dns").as_mapping().unwrap(), "enable").as_bool(), Some(false));
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
        assert!(matches!(build("a: [", "", &Settings::default()), Err(ConfigError::Profile(_))));
        assert!(matches!(build(PROFILE, "b: [", &Settings::default()), Err(ConfigError::Override(_))));
        assert!(matches!(build("- a\n- b", "", &Settings::default()), Err(ConfigError::Profile(_))));
    }

    #[test]
    fn explains_core_errors() {
        let out = "time=\"2026-11-08T10:00:00Z\" level=info msg=\"Start initial configuration in progress\"\ntime=\"2026-11-08T10:00:00Z\" level=error msg=\"proxy group[0]: 'G' not found\"\nconfiguration file /x test failed\n";
        let e = explain(out);
        assert!(e.starts_with("proxy group[0]: 'G' not found"), "{e}");
        assert!(e.contains("test failed"));
    }
}
