//! The system proxy: pointing the desktop's HTTP/HTTPS/SOCKS proxy at the core,
//! and putting back exactly what was there before — on quit, and on the next
//! start after a crash (the previous settings are kept in `state/sysproxy.json`
//! until they are restored).
//!
//! - macOS: `networksetup`, for every enabled network service.
//! - GNOME and GTK desktops: `gsettings` (`org.gnome.system.proxy`).
//! - KDE Plasma: `kioslaverc` through `kwriteconfig6`/`kwriteconfig5`, then KIO is
//!   told to reload. GTK apps on Plasma read gsettings, so both are set there.

use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::paths::{Paths, write_atomic};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySpec {
    pub host: String,
    pub port: u16,
    pub bypass: Vec<String>,
}

/// The desktop's proxy settings as they were before Kairo changed them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub gnome: Option<GnomeProxy>,
    pub kde: Option<KdeProxy>,
    pub mac: Option<Vec<MacService>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GnomeProxy {
    pub mode: String,
    pub http: (String, u16),
    pub https: (String, u16),
    pub socks: (String, u16),
    pub ignore_hosts: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KdeProxy {
    pub proxy_type: String,
    pub http: String,
    pub https: String,
    pub socks: String,
    pub no_proxy_for: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MacProxy {
    pub enabled: bool,
    pub server: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MacService {
    pub name: String,
    pub web: MacProxy,
    pub secure: MacProxy,
    pub socks: MacProxy,
    pub bypass: Vec<String>,
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{program} {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn have(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
    })
}

/// Which mechanisms apply on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backends {
    pub gnome: bool,
    pub kde: Option<&'static str>,
    pub mac: bool,
}

pub fn backends() -> Backends {
    // Tests and sandboxed runs: never touch the machine's proxy.
    if std::env::var_os("KAIRO_NO_SYSPROXY").is_some() {
        return Backends { gnome: false, kde: None, mac: false };
    }
    if cfg!(target_os = "macos") {
        return Backends { gnome: false, kde: None, mac: true };
    }
    if cfg!(target_os = "windows") {
        return Backends { gnome: false, kde: None, mac: false };
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_uppercase();
    let kde = desktop.contains("KDE").then(|| {
        if have("kwriteconfig6") { Some("6") } else if have("kwriteconfig5") { Some("5") } else { None }
    }).flatten();
    Backends { gnome: have("gsettings"), kde, mac: false }
}

/// Whether there is any way to set the system proxy here.
pub fn supported() -> bool {
    let b = backends();
    b.gnome || b.kde.is_some() || b.mac
}

// --- GNOME -------------------------------------------------------------------------

fn gs_get(schema: &str, key: &str) -> Result<String, String> {
    run("gsettings", &["get", schema, key]).map(|v| v.trim_matches('\'').to_owned())
}

fn gs_set(schema: &str, key: &str, value: &str) -> Result<(), String> {
    run("gsettings", &["set", schema, key, value]).map(|_| ())
}

fn gnome_read() -> Result<GnomeProxy, String> {
    let pair = |s: &str| -> Result<(String, u16), String> {
        let schema = format!("org.gnome.system.proxy.{s}");
        Ok((gs_get(&schema, "host")?, gs_get(&schema, "port")?.parse().unwrap_or(0)))
    };
    Ok(GnomeProxy {
        mode: gs_get("org.gnome.system.proxy", "mode")?,
        http: pair("http")?,
        https: pair("https")?,
        socks: pair("socks")?,
        ignore_hosts: run("gsettings", &["get", "org.gnome.system.proxy", "ignore-hosts"])?,
    })
}

/// A GVariant string list: `['a', 'b']`.
pub fn gvariant_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("'{}'", i.replace('\'', ""))).collect();
    format!("[{}]", quoted.join(", "))
}

fn gnome_apply(spec: &ProxySpec) -> Result<(), String> {
    for s in ["http", "https", "socks"] {
        let schema = format!("org.gnome.system.proxy.{s}");
        gs_set(&schema, "host", &spec.host)?;
        gs_set(&schema, "port", &spec.port.to_string())?;
    }
    gs_set("org.gnome.system.proxy", "ignore-hosts", &gvariant_list(&spec.bypass))?;
    gs_set("org.gnome.system.proxy", "mode", "manual")
}

fn gnome_restore(saved: &GnomeProxy) -> Result<(), String> {
    for (s, (host, port)) in [("http", &saved.http), ("https", &saved.https), ("socks", &saved.socks)] {
        let schema = format!("org.gnome.system.proxy.{s}");
        gs_set(&schema, "host", host)?;
        gs_set(&schema, "port", &port.to_string())?;
    }
    gs_set("org.gnome.system.proxy", "ignore-hosts", &saved.ignore_hosts)?;
    gs_set("org.gnome.system.proxy", "mode", if saved.mode.is_empty() { "none" } else { &saved.mode })
}

fn gnome_points_to(spec: &ProxySpec) -> bool {
    gs_get("org.gnome.system.proxy", "mode").is_ok_and(|m| m == "manual")
        && gs_get("org.gnome.system.proxy.http", "host").is_ok_and(|h| h == spec.host)
        && gs_get("org.gnome.system.proxy.http", "port").is_ok_and(|p| p == spec.port.to_string())
}

// --- KDE ---------------------------------------------------------------------------

fn kde_get(v: &str, key: &str) -> String {
    run(&format!("kreadconfig{v}"), &["--file", "kioslaverc", "--group", "Proxy Settings", "--key", key])
        .unwrap_or_default()
}

fn kde_set(v: &str, key: &str, value: &str) -> Result<(), String> {
    run(
        &format!("kwriteconfig{v}"),
        &["--file", "kioslaverc", "--group", "Proxy Settings", "--key", key, value],
    )
    .map(|_| ())
}

fn kde_read(v: &str) -> KdeProxy {
    KdeProxy {
        proxy_type: kde_get(v, "ProxyType"),
        http: kde_get(v, "httpProxy"),
        https: kde_get(v, "httpsProxy"),
        socks: kde_get(v, "socksProxy"),
        no_proxy_for: kde_get(v, "NoProxyFor"),
    }
}

/// KIO's form of a proxy address: `http://127.0.0.1 7897`.
pub fn kde_address(scheme: &str, spec: &ProxySpec) -> String {
    format!("{scheme}://{} {}", spec.host, spec.port)
}

fn kde_reload() {
    let _ = Command::new("dbus-send")
        .args(["--type=signal", "/KIO/Scheduler", "org.kde.KIO.Scheduler.reparseSlaveConfiguration", "string:"])
        .status();
}

fn kde_apply(v: &str, spec: &ProxySpec) -> Result<(), String> {
    kde_set(v, "httpProxy", &kde_address("http", spec))?;
    kde_set(v, "httpsProxy", &kde_address("http", spec))?;
    kde_set(v, "socksProxy", &kde_address("socks", spec))?;
    kde_set(v, "NoProxyFor", &spec.bypass.join(","))?;
    kde_set(v, "ProxyType", "1")?;
    kde_reload();
    Ok(())
}

fn kde_restore(v: &str, saved: &KdeProxy) -> Result<(), String> {
    kde_set(v, "httpProxy", &saved.http)?;
    kde_set(v, "httpsProxy", &saved.https)?;
    kde_set(v, "socksProxy", &saved.socks)?;
    kde_set(v, "NoProxyFor", &saved.no_proxy_for)?;
    kde_set(v, "ProxyType", if saved.proxy_type.is_empty() { "0" } else { &saved.proxy_type })?;
    kde_reload();
    Ok(())
}

fn kde_points_to(v: &str, spec: &ProxySpec) -> bool {
    kde_get(v, "ProxyType") == "1" && kde_get(v, "httpProxy") == kde_address("http", spec)
}

// --- macOS -------------------------------------------------------------------------

/// The enabled services of `networksetup -listallnetworkservices`.
pub fn mac_services_of(text: &str) -> Vec<String> {
    text.lines()
        .skip(1) // "An asterisk (*) denotes that a network service is disabled."
        .filter(|l| !l.trim().is_empty() && !l.starts_with('*'))
        .map(|l| l.trim().to_owned())
        .collect()
}

/// `networksetup -getwebproxy <service>`'s answer.
pub fn mac_proxy_of(text: &str) -> MacProxy {
    let mut p = MacProxy::default();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim();
            match k.trim() {
                "Enabled" => p.enabled = v.eq_ignore_ascii_case("yes"),
                "Server" => p.server = v.to_owned(),
                "Port" => p.port = v.parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    p
}

/// `networksetup -getproxybypassdomains <service>`'s answer.
pub fn mac_bypass_of(text: &str) -> Vec<String> {
    if text.contains("There aren't any") {
        return vec![];
    }
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()
}

fn ns(args: &[&str]) -> Result<String, String> {
    run("/usr/sbin/networksetup", args)
}

fn mac_read() -> Result<Vec<MacService>, String> {
    let services = mac_services_of(&ns(&["-listallnetworkservices"])?);
    services
        .into_iter()
        .map(|name| {
            Ok(MacService {
                web: mac_proxy_of(&ns(&["-getwebproxy", &name])?),
                secure: mac_proxy_of(&ns(&["-getsecurewebproxy", &name])?),
                socks: mac_proxy_of(&ns(&["-getsocksfirewallproxy", &name])?),
                bypass: mac_bypass_of(&ns(&["-getproxybypassdomains", &name]).unwrap_or_default()),
                name,
            })
        })
        .collect()
}

fn mac_apply(services: &[MacService], spec: &ProxySpec) -> Result<(), String> {
    let port = spec.port.to_string();
    for s in services {
        let n = s.name.as_str();
        ns(&["-setwebproxy", n, &spec.host, &port])?;
        ns(&["-setsecurewebproxy", n, &spec.host, &port])?;
        ns(&["-setsocksfirewallproxy", n, &spec.host, &port])?;
        let mut args = vec!["-setproxybypassdomains", n];
        args.extend(spec.bypass.iter().map(String::as_str));
        ns(&args)?;
    }
    Ok(())
}

fn mac_restore(services: &[MacService]) -> Result<(), String> {
    let set = |kind: &str, n: &str, p: &MacProxy| -> Result<(), String> {
        if !p.server.is_empty() {
            ns(&[&format!("-set{kind}"), n, &p.server, &p.port.to_string()])?;
        }
        ns(&[&format!("-set{kind}state"), n, if p.enabled { "on" } else { "off" }])?;
        Ok(())
    };
    for s in services {
        let n = s.name.as_str();
        set("webproxy", n, &s.web)?;
        set("securewebproxy", n, &s.secure)?;
        set("socksfirewallproxy", n, &s.socks)?;
        let mut args = vec!["-setproxybypassdomains", n];
        if s.bypass.is_empty() {
            args.push("Empty");
        } else {
            args.extend(s.bypass.iter().map(String::as_str));
        }
        ns(&args)?;
    }
    Ok(())
}

fn mac_points_to(spec: &ProxySpec) -> bool {
    mac_read().is_ok_and(|services| {
        services.iter().any(|s| s.web.enabled && s.web.server == spec.host && s.web.port == spec.port)
    })
}

// --- Public ------------------------------------------------------------------------

fn load_saved(paths: &Paths) -> Option<Saved> {
    std::fs::read(paths.sysproxy_state()).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// Points the system proxy at `spec`. The settings it replaces are saved first
/// (once: enabling again keeps the original ones).
pub fn enable(paths: &Paths, spec: &ProxySpec) -> Result<(), String> {
    let b = backends();
    if !(b.gnome || b.kde.is_some() || b.mac) {
        return Err("This desktop has no system proxy Kairo can set".into());
    }
    if load_saved(paths).is_none() {
        let saved = Saved {
            gnome: if b.gnome { gnome_read().ok() } else { None },
            kde: b.kde.map(kde_read),
            mac: if b.mac { Some(mac_read()?) } else { None },
        };
        write_atomic(&paths.sysproxy_state(), &serde_json::to_vec_pretty(&saved).expect("saved"))
            .map_err(|e| e.to_string())?;
    }
    let mut errors = Vec::new();
    if b.gnome && let Err(e) = gnome_apply(spec) {
        errors.push(e);
    }
    if let Some(v) = b.kde && let Err(e) = kde_apply(v, spec) {
        errors.push(e);
    }
    if b.mac {
        let services = load_saved(paths).and_then(|s| s.mac).unwrap_or_default();
        if let Err(e) = mac_apply(&services, spec) {
            errors.push(e);
        }
    }
    // One working mechanism is enough (GNOME's tools may exist on Plasma without a dconf).
    let total = usize::from(b.gnome) + usize::from(b.kde.is_some()) + usize::from(b.mac);
    if errors.len() == total { Err(errors.join("; ")) } else { Ok(()) }
}

/// Puts back the settings saved by [`enable`]; with none saved, turns the proxy off.
pub fn disable(paths: &Paths) -> Result<(), String> {
    let b = backends();
    let saved = load_saved(paths);
    let mut errors = Vec::new();
    match &saved {
        Some(saved) => {
            if let Some(g) = &saved.gnome && let Err(e) = gnome_restore(g) {
                errors.push(e);
            }
            if let (Some(v), Some(k)) = (b.kde, &saved.kde) && let Err(e) = kde_restore(v, k) {
                errors.push(e);
            }
            if let Some(m) = &saved.mac && let Err(e) = mac_restore(m) {
                errors.push(e);
            }
        }
        None => {
            if b.gnome {
                let _ = gs_set("org.gnome.system.proxy", "mode", "none");
            }
            if let Some(v) = b.kde {
                let _ = kde_set(v, "ProxyType", "0");
                kde_reload();
            }
            if b.mac && let Ok(services) = mac_read() {
                for s in services {
                    for kind in ["webproxystate", "securewebproxystate", "socksfirewallproxystate"] {
                        let _ = ns(&[&format!("-set{kind}"), &s.name, "off"]);
                    }
                }
            }
        }
    }
    if errors.is_empty() {
        let _ = std::fs::remove_file(paths.sysproxy_state());
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// After a crash: restores the settings a previous session saved and never put back.
pub fn recover(paths: &Paths) -> bool {
    if paths.sysproxy_state().exists() {
        log::warn!("restoring the system proxy a previous session left on");
        if let Err(e) = disable(paths) {
            log::error!("could not restore the system proxy: {e}");
        }
        return true;
    }
    false
}

/// Whether the system proxy currently points at `spec`.
pub fn points_to(spec: &ProxySpec) -> bool {
    let b = backends();
    (b.gnome && gnome_points_to(spec)) || b.kde.is_some_and(|v| kde_points_to(v, spec)) || (b.mac && mac_points_to(spec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_networksetup() {
        let list = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Thunderbolt Bridge\nUSB 10/100/1000 LAN\n";
        assert_eq!(mac_services_of(list), vec!["Wi-Fi", "USB 10/100/1000 LAN"]);
        let p = mac_proxy_of("Enabled: Yes\nServer: 127.0.0.1\nPort: 7897\nAuthenticated Proxy Enabled: 0\n");
        assert_eq!(p, MacProxy { enabled: true, server: "127.0.0.1".into(), port: 7897 });
        let p = mac_proxy_of("Enabled: No\nServer: \nPort: 0\n");
        assert!(!p.enabled);
        assert!(mac_bypass_of("There aren't any bypass domains set on Wi-Fi.").is_empty());
        assert_eq!(mac_bypass_of("*.local\n169.254/16\n"), vec!["*.local", "169.254/16"]);
    }

    #[test]
    fn formats_for_desktops() {
        let spec = ProxySpec { host: "127.0.0.1".into(), port: 7897, bypass: vec!["localhost".into(), "*.local".into()] };
        assert_eq!(kde_address("http", &spec), "http://127.0.0.1 7897");
        assert_eq!(gvariant_list(&spec.bypass), "['localhost', '*.local']");
    }
}
