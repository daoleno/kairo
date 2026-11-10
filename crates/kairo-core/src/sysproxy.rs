//! The system proxy: pointing the desktop's HTTP/HTTPS/SOCKS proxy at the core,
//! and putting back exactly what was there before — on quit, and on the next
//! start after a crash (the previous settings are kept in `state/sysproxy.json`
//! until they are restored).
//!
//! - macOS: `networksetup`, for every enabled network service.
//! - GNOME and GTK desktops: `gsettings` (`org.gnome.system.proxy`).
//! - KDE Plasma: `kioslaverc` through `kwriteconfig6`/`kwriteconfig5`, then KIO is
//!   told to reload. GTK apps on Plasma read gsettings, so both are set there.
//! - Windows: the current user's Internet Settings (`ProxyEnable`, `ProxyServer`,
//!   `ProxyOverride`, `AutoConfigURL`), then WinINet is told to reload.

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
    #[serde(default)]
    pub windows: Option<WinProxy>,
    /// The port Kairo pointed the system at (absent in 0.1 state files).
    #[serde(default)]
    pub applied: Option<u16>,
}

/// Whether something accepts connections on 127.0.0.1:`port`.
pub fn answers(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(400)).is_ok()
}

/// A proxy on this machine that nothing answers: another client's, left
/// behind when it quit or crashed. Putting it back would leave the computer
/// offline, so it is put back as "no proxy".
fn dead_local(host: &str, port: u16, ours: Option<u16>) -> bool {
    let host = host
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("socks://");
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
        && port != 0
        // Kairo's own port (saved after a crash left it set) goes with Kairo.
        && (Some(port) == ours || !answers(port))
}

/// KIO's `http://127.0.0.1 7897` as host and port.
fn kde_parts(address: &str) -> (String, u16) {
    let mut it = address.split_whitespace();
    let host = it.next().unwrap_or("").to_owned();
    let port = it.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (host, port)
}

impl Saved {
    /// These settings with any dead local proxy among them turned off.
    pub fn without_dead_proxies(mut self) -> Saved {
        let ours = self.applied;
        if let Some(g) = &mut self.gnome
            && g.mode == "manual"
            && dead_local(&g.http.0, g.http.1, ours)
        {
            log::warn!(
                "the proxy saved before Kairo ({}:{}) is gone: restoring none",
                g.http.0,
                g.http.1
            );
            g.mode = "none".into();
        }
        if let Some(k) = &mut self.kde
            && k.proxy_type == "1"
        {
            let (host, port) = kde_parts(&k.http);
            if dead_local(&host, port, ours) {
                log::warn!("the proxy saved before Kairo ({host}:{port}) is gone: restoring none");
                k.proxy_type = "0".into();
            }
        }
        if let Some(w) = &mut self.windows
            && w.enabled
        {
            let (host, port) = win_first_proxy(w.server.as_deref().unwrap_or(""));
            if dead_local(&host, port, ours) {
                log::warn!("the proxy saved before Kairo ({host}:{port}) is gone: restoring none");
                w.enabled = false;
            }
        }
        if let Some(services) = &mut self.mac {
            for svc in services.iter_mut() {
                for p in [&mut svc.web, &mut svc.secure, &mut svc.socks] {
                    if p.enabled && dead_local(&p.server, p.port, ours) {
                        log::warn!(
                            "the proxy saved before Kairo ({}:{}) is gone: restoring none",
                            p.server,
                            p.port
                        );
                        p.enabled = false;
                    }
                }
            }
        }
        self
    }
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

/// Windows' proxy settings of the current user as they were.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WinProxy {
    pub enabled: bool,
    /// `ProxyServer` (`127.0.0.1:7890` or `http=…;https=…`), absent when unset.
    pub server: Option<String>,
    /// `ProxyOverride`, absent when unset.
    pub bypass: Option<String>,
    /// `AutoConfigURL` (a PAC script), absent when unset.
    pub pac: Option<String>,
}

/// The host and port of the first proxy in a `ProxyServer` value.
pub fn win_first_proxy(server: &str) -> (String, u16) {
    let first = server.split(';').next().unwrap_or("").trim();
    // `http=host:port` names the scheme; a bare `host:port` serves all.
    let addr = first.rsplit_once('=').map_or(first, |(_, a)| a);
    let addr = addr.trim_start_matches("http://").trim_start_matches("socks=");
    match addr.rsplit_once(':') {
        Some((host, port)) => (host.to_owned(), port.parse().unwrap_or(0)),
        None => (addr.to_owned(), 0),
    }
}

/// `ProxyOverride` for a bypass list. Windows takes host names, `*` wildcards
/// and `<local>`, not CIDR ranges: a range on a byte boundary (and the
/// private `172.16.0.0/12`) becomes the wildcards that cover it.
pub fn win_override(bypass: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    for entry in bypass {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((addr, bits)) = entry.split_once('/') else {
            add(entry.to_owned());
            continue;
        };
        let (Ok(ip), Ok(bits)) = (addr.parse::<std::net::Ipv4Addr>(), bits.parse::<u32>()) else {
            // IPv6 ranges have no wildcard form here.
            continue;
        };
        let o = ip.octets();
        match bits {
            8 => add(format!("{}.*", o[0])),
            16 => add(format!("{}.{}.*", o[0], o[1])),
            24 => add(format!("{}.{}.{}.*", o[0], o[1], o[2])),
            12 if o[0] == 172 && o[1] == 16 => {
                for second in 16..32 {
                    add(format!("172.{second}.*"));
                }
            }
            _ => {}
        }
    }
    add("<local>".to_owned());
    out.join(";")
}

#[cfg(windows)]
fn win_read() -> WinProxy {
    use crate::winapi::{INTERNET_SETTINGS as K, reg_get_dword, reg_get_string};
    WinProxy {
        enabled: reg_get_dword(K, "ProxyEnable").unwrap_or(0) != 0,
        server: reg_get_string(K, "ProxyServer"),
        bypass: reg_get_string(K, "ProxyOverride"),
        pac: reg_get_string(K, "AutoConfigURL"),
    }
}

#[cfg(windows)]
fn win_apply(spec: &ProxySpec) -> Result<(), String> {
    use crate::winapi::{INTERNET_SETTINGS as K, reg_delete, reg_set_dword, reg_set_string};
    reg_set_string(K, "ProxyServer", &format!("{}:{}", spec.host, spec.port))?;
    reg_set_string(K, "ProxyOverride", &win_override(&spec.bypass))?;
    // A PAC script would win over the fixed proxy.
    reg_delete(K, "AutoConfigURL")?;
    reg_set_dword(K, "ProxyEnable", 1)?;
    crate::winapi::notify_proxy_changed();
    Ok(())
}

#[cfg(windows)]
fn win_restore(w: &WinProxy) -> Result<(), String> {
    use crate::winapi::{INTERNET_SETTINGS as K, reg_delete, reg_set_dword, reg_set_string};
    let put = |name: &str, value: &Option<String>| match value {
        Some(v) => reg_set_string(K, name, v),
        None => reg_delete(K, name),
    };
    put("ProxyServer", &w.server)?;
    put("ProxyOverride", &w.bypass)?;
    put("AutoConfigURL", &w.pac)?;
    reg_set_dword(K, "ProxyEnable", u32::from(w.enabled))?;
    crate::winapi::notify_proxy_changed();
    Ok(())
}

#[cfg(windows)]
fn win_points_to(spec: &ProxySpec) -> bool {
    let w = win_read();
    w.enabled && win_first_proxy(w.server.as_deref().unwrap_or("")) == (spec.host.clone(), spec.port)
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
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Which mechanisms apply on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backends {
    pub gnome: bool,
    pub kde: Option<&'static str>,
    pub mac: bool,
    pub windows: bool,
}

pub fn backends() -> Backends {
    // Tests and sandboxed runs: never touch the machine's proxy.
    if std::env::var_os("KAIRO_NO_SYSPROXY").is_some() {
        return Backends {
            gnome: false,
            kde: None,
            mac: false,
            windows: false,
        };
    }
    if cfg!(target_os = "macos") {
        return Backends {
            gnome: false,
            kde: None,
            mac: true,
            windows: false,
        };
    }
    if cfg!(target_os = "windows") {
        return Backends {
            gnome: false,
            kde: None,
            mac: false,
            windows: true,
        };
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_uppercase();
    let kde = desktop
        .contains("KDE")
        .then(|| {
            if have("kwriteconfig6") {
                Some("6")
            } else if have("kwriteconfig5") {
                Some("5")
            } else {
                None
            }
        })
        .flatten();
    Backends {
        gnome: have("gsettings"),
        kde,
        mac: false,
        windows: false,
    }
}

/// Whether there is any way to set the system proxy here.
pub fn supported() -> bool {
    let b = backends();
    b.gnome || b.kde.is_some() || b.mac || b.windows
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
        Ok((
            gs_get(&schema, "host")?,
            gs_get(&schema, "port")?.parse().unwrap_or(0),
        ))
    };
    Ok(GnomeProxy {
        mode: gs_get("org.gnome.system.proxy", "mode")?,
        http: pair("http")?,
        https: pair("https")?,
        socks: pair("socks")?,
        ignore_hosts: run(
            "gsettings",
            &["get", "org.gnome.system.proxy", "ignore-hosts"],
        )?,
    })
}

/// A GVariant string list: `['a', 'b']`.
pub fn gvariant_list(items: &[String]) -> String {
    let quoted: Vec<String> = items
        .iter()
        .map(|i| format!("'{}'", i.replace('\'', "")))
        .collect();
    format!("[{}]", quoted.join(", "))
}

fn gnome_apply(spec: &ProxySpec) -> Result<(), String> {
    for s in ["http", "https", "socks"] {
        let schema = format!("org.gnome.system.proxy.{s}");
        gs_set(&schema, "host", &spec.host)?;
        gs_set(&schema, "port", &spec.port.to_string())?;
    }
    gs_set(
        "org.gnome.system.proxy",
        "ignore-hosts",
        &gvariant_list(&spec.bypass),
    )?;
    gs_set("org.gnome.system.proxy", "mode", "manual")
}

fn gnome_restore(saved: &GnomeProxy) -> Result<(), String> {
    for (s, (host, port)) in [
        ("http", &saved.http),
        ("https", &saved.https),
        ("socks", &saved.socks),
    ] {
        let schema = format!("org.gnome.system.proxy.{s}");
        gs_set(&schema, "host", host)?;
        gs_set(&schema, "port", &port.to_string())?;
    }
    gs_set(
        "org.gnome.system.proxy",
        "ignore-hosts",
        &saved.ignore_hosts,
    )?;
    gs_set(
        "org.gnome.system.proxy",
        "mode",
        if saved.mode.is_empty() {
            "none"
        } else {
            &saved.mode
        },
    )
}

fn gnome_points_to(spec: &ProxySpec) -> bool {
    gs_get("org.gnome.system.proxy", "mode").is_ok_and(|m| m == "manual")
        && gs_get("org.gnome.system.proxy.http", "host").is_ok_and(|h| h == spec.host)
        && gs_get("org.gnome.system.proxy.http", "port").is_ok_and(|p| p == spec.port.to_string())
}

// --- KDE ---------------------------------------------------------------------------

fn kde_get(v: &str, key: &str) -> String {
    run(
        &format!("kreadconfig{v}"),
        &[
            "--file",
            "kioslaverc",
            "--group",
            "Proxy Settings",
            "--key",
            key,
        ],
    )
    .unwrap_or_default()
}

fn kde_set(v: &str, key: &str, value: &str) -> Result<(), String> {
    run(
        &format!("kwriteconfig{v}"),
        &[
            "--file",
            "kioslaverc",
            "--group",
            "Proxy Settings",
            "--key",
            key,
            value,
        ],
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
        .args([
            "--type=signal",
            "/KIO/Scheduler",
            "org.kde.KIO.Scheduler.reparseSlaveConfiguration",
            "string:",
        ])
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
    kde_set(
        v,
        "ProxyType",
        if saved.proxy_type.is_empty() {
            "0"
        } else {
            &saved.proxy_type
        },
    )?;
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
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
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
        ns(&[
            &format!("-set{kind}state"),
            n,
            if p.enabled { "on" } else { "off" },
        ])?;
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
        services
            .iter()
            .any(|s| s.web.enabled && s.web.server == spec.host && s.web.port == spec.port)
    })
}

// Windows' backend calls the registry; elsewhere these never run.
#[cfg(windows)]
fn win_saved(b: Backends) -> Option<WinProxy> {
    b.windows.then(win_read)
}
#[cfg(not(windows))]
fn win_saved(_: Backends) -> Option<WinProxy> {
    None
}
#[cfg(windows)]
fn win_apply_spec(spec: &ProxySpec) -> Result<(), String> {
    win_apply(spec)
}
#[cfg(not(windows))]
fn win_apply_spec(_: &ProxySpec) -> Result<(), String> {
    Err("not Windows".into())
}
#[cfg(windows)]
fn win_restore_saved(w: &WinProxy) -> Result<(), String> {
    win_restore(w)
}
#[cfg(not(windows))]
fn win_restore_saved(_: &WinProxy) -> Result<(), String> {
    Ok(())
}
#[cfg(windows)]
fn win_points_to_spec(spec: &ProxySpec) -> bool {
    win_points_to(spec)
}
#[cfg(not(windows))]
fn win_points_to_spec(_: &ProxySpec) -> bool {
    false
}

// --- Public ------------------------------------------------------------------------

fn load_saved(paths: &Paths) -> Option<Saved> {
    std::fs::read(paths.sysproxy_state())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

/// Points the system proxy at `spec`. The settings it replaces are saved first
/// (once: enabling again keeps the original ones).
pub fn enable(paths: &Paths, spec: &ProxySpec) -> Result<(), String> {
    let _lock = lock(paths);
    let b = backends();
    if !(b.gnome || b.kde.is_some() || b.mac || b.windows) {
        return Err("This desktop has no system proxy Kairo can set".into());
    }
    let mut saved = match load_saved(paths) {
        Some(s) => s,
        None => Saved {
            gnome: if b.gnome { gnome_read().ok() } else { None },
            kde: b.kde.map(kde_read),
            mac: if b.mac { Some(mac_read()?) } else { None },
            windows: win_saved(b),
            applied: None,
        },
    };
    // Written before the change: whatever happens next, the way back is known.
    saved.applied = Some(spec.port);
    write_atomic(
        &paths.sysproxy_state(),
        &serde_json::to_vec_pretty(&saved).expect("saved"),
    )
    .map_err(|e| e.to_string())?;
    let mut errors = Vec::new();
    if b.gnome
        && let Err(e) = gnome_apply(spec)
    {
        errors.push(e);
    }
    if let Some(v) = b.kde
        && let Err(e) = kde_apply(v, spec)
    {
        errors.push(e);
    }
    if b.mac {
        let services = load_saved(paths).and_then(|s| s.mac).unwrap_or_default();
        if let Err(e) = mac_apply(&services, spec) {
            errors.push(e);
        }
    }
    if b.windows
        && let Err(e) = win_apply_spec(spec)
    {
        errors.push(e);
    }
    // One working mechanism is enough (GNOME's tools may exist on Plasma without a dconf).
    let total = usize::from(b.gnome)
        + usize::from(b.kde.is_some())
        + usize::from(b.mac)
        + usize::from(b.windows);
    if errors.len() == total {
        Err(errors.join("; "))
    } else {
        Ok(())
    }
}

/// Puts back the settings saved by [`enable`]. With none saved it does nothing:
/// [`enable`] saves before it changes anything, so Kairo has nothing of its own to
/// undo, and the proxy as it is (perhaps another app's) is the user's.
pub fn disable(paths: &Paths) -> Result<(), String> {
    let _lock = lock(paths);
    disable_locked(paths)
}

fn disable_locked(paths: &Paths) -> Result<(), String> {
    let Some(saved) = load_saved(paths) else {
        // An unreadable state file would otherwise stick around forever.
        let _ = std::fs::remove_file(paths.sysproxy_state());
        return Ok(());
    };
    let saved = saved.without_dead_proxies();
    let b = backends();
    let mut errors = Vec::new();
    if let Some(g) = &saved.gnome
        && let Err(e) = gnome_restore(g)
    {
        errors.push(e);
    }
    if let (Some(v), Some(k)) = (b.kde, &saved.kde)
        && let Err(e) = kde_restore(v, k)
    {
        errors.push(e);
    }
    if let Some(m) = &saved.mac
        && let Err(e) = mac_restore(m)
    {
        errors.push(e);
    }
    if let Some(w) = &saved.windows
        && let Err(e) = win_restore_saved(w)
    {
        errors.push(e);
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

/// One change of the system proxy at a time, across processes: Kairo setting
/// it and the login check putting it back must not interleave (the check
/// deciding on the old state, then undoing what Kairo just set). Held for the
/// length of one `enable`, `disable` or decision of the check. Waits for it
/// 15 s at most; `None` then, or when there is no lock file to be had, and the
/// change goes ahead as before (never stuck, as a panic's restore in the very
/// thread that holds it would be).
fn lock(paths: &Paths) -> Option<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let _ = std::fs::create_dir_all(paths.state_dir());
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(paths.state_dir().join("sysproxy.lock"))
            .ok()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            // SAFETY: a valid descriptor, kept open for as long as the lock is held.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Some(file);
            }
            if std::time::Instant::now() >= deadline {
                log::warn!("the system proxy's lock is held too long: going ahead");
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let _ = std::fs::create_dir_all(paths.state_dir());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            // Share mode 0: a second open fails while this one is held.
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .share_mode(0)
                .open(paths.state_dir().join("sysproxy.lock"))
            {
                return Some(file);
            }
            if std::time::Instant::now() >= deadline {
                log::warn!("the system proxy's lock is held too long: going ahead");
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = paths;
        None
    }
}

/// How long the login check waits for a Kairo core that is starting.
const LOGIN_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// For the login check: puts the system proxy back when Kairo left it pointing
/// at a port nothing answers any more (Kairo and its core gone, the computer
/// restarted). A live core is left alone, and so is one that is starting: a
/// Kairo opened at the same login sets the proxy once its core answers the
/// controller, a moment before the proxy port does, so the check waits for it.
pub fn restore_if_dead(paths: &Paths) -> bool {
    let deadline = std::time::Instant::now() + LOGIN_WAIT;
    loop {
        {
            let _lock = lock(paths);
            let Some(saved) = load_saved(paths) else {
                return false;
            };
            if saved.applied.is_some_and(answers) {
                return false;
            }
            if !crate::supervisor::core_alive(paths) || std::time::Instant::now() >= deadline {
                log::warn!("the system proxy points at a Kairo that is gone: restoring it");
                if let Err(e) = disable_locked(paths) {
                    log::error!("could not restore the system proxy: {e}");
                }
                return true;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Whether the system proxy currently points at `spec`.
pub fn points_to(spec: &ProxySpec) -> bool {
    let b = backends();
    (b.gnome && gnome_points_to(spec))
        || b.kde.is_some_and(|v| kde_points_to(v, spec))
        || (b.mac && mac_points_to(spec))
        || (b.windows && win_points_to_spec(spec))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With nothing saved, Kairo never changed the proxy, so quitting must not
    /// touch it (0.1.0 switched another app's proxy off here). Returning before
    /// any backend is consulted is what keeps this test from touching the machine.
    #[test]
    fn a_dead_local_proxy_is_not_put_back() {
        // Bound but not listening: it refuses connections, and no other test
        // can take the port meanwhile.
        let dead_socket = tokio::net::TcpSocket::new_v4().unwrap();
        dead_socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let dead = dead_socket.local_addr().unwrap().port();
        let live = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let live_port = live.local_addr().unwrap().port();
        let saved = Saved {
            gnome: Some(GnomeProxy {
                mode: "manual".into(),
                http: ("127.0.0.1".into(), dead),
                ..Default::default()
            }),
            kde: Some(KdeProxy {
                proxy_type: "1".into(),
                http: format!("http://127.0.0.1 {live_port}"),
                ..Default::default()
            }),
            mac: Some(vec![MacService {
                name: "Wi-Fi".into(),
                web: MacProxy {
                    enabled: true,
                    server: "127.0.0.1".into(),
                    port: dead,
                },
                secure: MacProxy {
                    enabled: true,
                    server: "proxy.example.com".into(),
                    port: dead,
                },
                ..Default::default()
            }]),
            windows: None,
            applied: Some(7897),
        }
        .without_dead_proxies();
        assert_eq!(saved.gnome.unwrap().mode, "none");
        // Alive: kept.
        assert_eq!(saved.kde.unwrap().proxy_type, "1");
        let mac = &saved.mac.unwrap()[0];
        assert!(!mac.web.enabled);
        // Not on this machine: not Kairo's to judge.
        assert!(mac.secure.enabled);
        // Kairo's own port, saved after a crash: never put back, even while it answers.
        let saved = Saved {
            gnome: Some(GnomeProxy {
                mode: "manual".into(),
                http: ("127.0.0.1".into(), live_port),
                ..Default::default()
            }),
            applied: Some(live_port),
            ..Default::default()
        }
        .without_dead_proxies();
        assert_eq!(saved.gnome.unwrap().mode, "none");
    }

    #[test]
    fn disable_without_saved_state_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path());
        paths.ensure().unwrap();
        assert!(disable(&paths).is_ok());
        assert!(!paths.sysproxy_state().exists());
        std::fs::write(paths.sysproxy_state(), b"not json").unwrap();
        assert!(disable(&paths).is_ok());
        assert!(
            !paths.sysproxy_state().exists(),
            "an unreadable state file is dropped, nothing restored"
        );
    }

    #[test]
    fn parses_networksetup() {
        let list = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Thunderbolt Bridge\nUSB 10/100/1000 LAN\n";
        assert_eq!(mac_services_of(list), vec!["Wi-Fi", "USB 10/100/1000 LAN"]);
        let p = mac_proxy_of(
            "Enabled: Yes\nServer: 127.0.0.1\nPort: 7897\nAuthenticated Proxy Enabled: 0\n",
        );
        assert_eq!(
            p,
            MacProxy {
                enabled: true,
                server: "127.0.0.1".into(),
                port: 7897
            }
        );
        let p = mac_proxy_of("Enabled: No\nServer: \nPort: 0\n");
        assert!(!p.enabled);
        assert!(mac_bypass_of("There aren't any bypass domains set on Wi-Fi.").is_empty());
        assert_eq!(
            mac_bypass_of("*.local\n169.254/16\n"),
            vec!["*.local", "169.254/16"]
        );
    }

    #[test]
    fn formats_for_desktops() {
        let spec = ProxySpec {
            host: "127.0.0.1".into(),
            port: 7897,
            bypass: vec!["localhost".into(), "*.local".into()],
        };
        assert_eq!(kde_address("http", &spec), "http://127.0.0.1 7897");
        assert_eq!(gvariant_list(&spec.bypass), "['localhost', '*.local']");
    }

    #[test]
    fn windows_override_has_no_ranges() {
        let list: Vec<String> = [
            "localhost",
            "127.0.0.0/8",
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "fc00::/7",
            "*.local",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let o = win_override(&list);
        let parts: Vec<&str> = o.split(';').collect();
        assert!(parts.contains(&"localhost"));
        assert!(parts.contains(&"127.*"));
        assert!(parts.contains(&"10.*"));
        assert!(parts.contains(&"172.16.*"));
        assert!(parts.contains(&"172.31.*"));
        assert!(parts.contains(&"192.168.*"));
        assert!(parts.contains(&"*.local"));
        assert_eq!(parts.last(), Some(&"<local>"));
        assert!(!o.contains('/'));
        assert!(!o.contains("fc00"));
    }

    #[test]
    fn windows_proxy_server_is_read() {
        assert_eq!(win_first_proxy("127.0.0.1:7890"), ("127.0.0.1".into(), 7890));
        assert_eq!(
            win_first_proxy("http=127.0.0.1:7890;https=127.0.0.1:7890"),
            ("127.0.0.1".into(), 7890)
        );
        assert_eq!(win_first_proxy(""), ("".into(), 0));
    }

    #[test]
    fn a_dead_windows_proxy_is_restored_as_none() {
        let saved = Saved {
            windows: Some(WinProxy {
                enabled: true,
                server: Some("127.0.0.1:1".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(!saved.without_dead_proxies().windows.unwrap().enabled);
    }
}
