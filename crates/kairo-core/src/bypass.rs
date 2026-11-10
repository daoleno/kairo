//! Tools that go around the proxy.
//!
//! With the system proxy (not TUN) only programs that honour it reach the
//! core; a command-line tool that ignores it connects straight out and the
//! core never sees it. The operating system's socket table does: this module
//! reads the current user's established TCP connections (`ss` on Linux,
//! `lsof` on macOS; no root, only the person's own processes appear) and
//! reports the ones that belong to a known AI tool and go to the internet
//! rather than to the proxy or this network.
//!
//! The parsers are pure and tested on captured output. Reading the socket
//! table is skipped when no process looks like an AI tool, so on most
//! machines most of the time this costs one process listing.

use std::collections::HashMap;
use std::net::IpAddr;

use crate::ai::{self, AiService};

/// A tool whose connection goes out directly, not through Kairo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bypass {
    /// The program's name (`claude`, `Cursor`).
    pub tool: String,
    /// The AI service it belongs to (the id in [`ai::SERVICES`]).
    pub service: Option<&'static str>,
    pub pid: u32,
    /// Where it is connected: `ip:port`, or `[ip]:port` for IPv6.
    pub remote: String,
    /// Unix seconds when it was first seen.
    pub since: i64,
}

/// A running process of the current user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    /// The executable's path (and, for an interpreter, the script's).
    pub path: String,
}

/// An established TCP connection, from the socket table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conn {
    pub pid: u32,
    pub name: String,
    pub remote: IpAddr,
    pub port: u16,
}

/// `1.2.3.4:443`, `[2001:db8::1]:443`, `[fe80::1]%eth0:443` or
/// `fe80::1%eth0:443`: the address and port. An IPv4-mapped IPv6 address is
/// the IPv4 one.
fn parse_addr(s: &str) -> Option<(IpAddr, u16)> {
    let (host, port) = match s.strip_prefix('[') {
        Some(rest) => {
            let (h, after) = rest.split_once(']')?;
            (h, after.rsplit_once(':')?.1)
        }
        None => s.rsplit_once(':')?,
    };
    let host = host.split('%').next()?;
    let ip: IpAddr = host.parse().ok()?;
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    };
    Some((ip, port.parse().ok()?))
}

/// The connections in `ss -tnpH state established`. The state column is
/// absent with that filter, but tolerated; connections without a `users:`
/// part (another user's) are skipped. A connection held by several
/// processes (a forked tool) gives one entry for each.
pub fn parse_ss(text: &str) -> Vec<Conn> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(at) = line.find("users:(") else {
            continue;
        };
        // The addresses are the last two that read as one before `users:`.
        let addrs: Vec<(IpAddr, u16)> = line[..at]
            .split_whitespace()
            .filter_map(parse_addr)
            .collect();
        let Some(&(remote, port)) = addrs.last() else {
            continue;
        };
        if addrs.len() < 2 {
            continue;
        }
        // `users:(("name",pid=1,fd=3),("name",pid=2,fd=3))`. A name may hold
        // commas, spaces and parentheses: it ends at `",pid=`.
        let mut rest = &line[at + "users:(".len()..];
        while let Some(i) = rest.find("(\"") {
            rest = &rest[i + 2..];
            let Some(j) = rest.find("\",pid=") else { break };
            let name = &rest[..j];
            rest = &rest[j + "\",pid=".len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(pid) = digits.parse() {
                out.push(Conn {
                    pid,
                    name: name.to_owned(),
                    remote,
                    port,
                });
            }
        }
    }
    out
}

/// The connections in `lsof -nP +c 0 -iTCP -sTCP:ESTABLISHED -Fpcn`: `p`
/// (process id), `c` (command), `n` (`local->remote`) lines.
pub fn parse_lsof(text: &str) -> Vec<Conn> {
    let mut out = Vec::new();
    let mut pid = 0u32;
    let mut command: Option<&str> = None;
    for line in text.lines() {
        let Some(rest) = line.get(1..) else { continue };
        match line.as_bytes()[0] {
            b'p' => {
                command = None;
                pid = rest.parse().unwrap_or(0);
            }
            b'c' => command = Some(rest),
            b'n' => {
                let Some((_, remote)) = rest.split_once("->") else {
                    continue;
                };
                if let (Some(c), Some((ip, port))) = (command, parse_addr(remote.trim())) {
                    out.push(Conn {
                        pid,
                        name: c.to_owned(),
                        remote: ip,
                        port,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// The processes in `ps -x -o pid=,comm=` (macOS: `comm` is the path).
pub fn parse_ps(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let (pid, path) = l.split_once(char::is_whitespace)?;
            let path = path.trim();
            let name = path.rsplit('/').next().unwrap_or(path);
            let pid = pid.parse().ok()?;
            (!name.is_empty()).then(|| Proc {
                pid,
                name: name.to_owned(),
                path: path.to_owned(),
            })
        })
        .collect()
}

/// Whether an address is this machine or this network (a local model, the
/// router, a Tailscale peer, the TUN's fake range): never a bypass.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                // Carrier-grade NAT (Tailscale) and the benchmark range the
                // core's fake IPs live in.
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 198 && (18..20).contains(&o[1]))
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_local(IpAddr::V4(v4)))
        }
    }
}

/// The AI service a program belongs to, by its name and where it lives. The
/// part of the path that is the person's home folder does not count (a user
/// called `claude` does not make every program theirs).
pub fn tool_of(name: &str, path: &str, home: Option<&str>) -> Option<&'static AiService> {
    let rel = match home {
        Some(h) if !h.is_empty() => path.strip_prefix(h).unwrap_or(path),
        _ => path,
    };
    ai::tool_service(name).or_else(|| ai::tool_service(rel))
}

/// What the person can paste in a terminal so command-line tools use the
/// proxy: one line, upper and lower case (tools read either).
pub fn terminal_env(mixed_port: u16) -> String {
    let proxy = format!("http://127.0.0.1:{mixed_port}");
    let skip = "localhost,127.0.0.1,::1,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,.local";
    format!(
        "export HTTP_PROXY={proxy} HTTPS_PROXY={proxy} ALL_PROXY={proxy} NO_PROXY={skip} \
         http_proxy={proxy} https_proxy={proxy} all_proxy={proxy} no_proxy={skip}"
    )
}

/// The bypasses in `conns`: connections of `procs` that are AI tools, to a
/// non-local address, not to the proxy's port, from a process other than
/// `exclude` (Kairo and its core); one for each tool and remote address.
pub fn detect(
    conns: &[Conn],
    procs: &[Proc],
    mixed_port: u16,
    exclude: &[u32],
    home: Option<&str>,
    now: i64,
) -> Vec<Bypass> {
    let tools: HashMap<u32, (&Proc, &'static AiService)> = procs
        .iter()
        .filter(|p| !exclude.contains(&p.pid))
        .filter_map(|p| Some((p.pid, (p, tool_of(&p.name, &p.path, home)?))))
        .collect();
    let mut seen: Vec<(String, IpAddr)> = Vec::new();
    let mut out = Vec::new();
    for c in conns {
        let Some((p, service)) = tools.get(&c.pid) else {
            continue;
        };
        if is_local(c.remote) || c.port == mixed_port {
            continue;
        }
        let key = (p.name.clone(), c.remote);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(Bypass {
            tool: p.name.clone(),
            service: Some(service.id),
            pid: c.pid,
            remote: match c.remote {
                IpAddr::V4(v4) => format!("{v4}:{}", c.port),
                IpAddr::V6(v6) => format!("[{v6}]:{}", c.port),
            },
            since: now,
        });
    }
    out
}

/// The address part of a `Bypass::remote`.
fn remote_ip(remote: &str) -> &str {
    remote
        .rsplit_once(':')
        .map(|(h, _)| h.trim_matches(['[', ']']))
        .unwrap_or(remote)
}

/// `found`, with the time each (tool, address) was first seen carried over
/// from `previous`; oldest first.
pub fn merge_since(previous: &[Bypass], found: Vec<Bypass>) -> Vec<Bypass> {
    let mut out: Vec<Bypass> = found
        .into_iter()
        .map(|mut b| {
            if let Some(p) = previous
                .iter()
                .find(|p| p.tool == b.tool && remote_ip(&p.remote) == remote_ip(&b.remote))
            {
                b.since = p.since;
            }
            b
        })
        .collect();
    out.sort_by(|a, b| (a.since, &a.tool, &a.remote).cmp(&(b.since, &b.tool, &b.remote)));
    out
}

/// The current user's processes. Blocking.
#[cfg(unix)]
pub fn list_processes() -> Vec<Proc> {
    if cfg!(target_os = "macos") {
        return std::process::Command::new("/bin/ps")
            .args(["-x", "-o", "pid=,comm="])
            .output()
            .map(|o| parse_ps(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default();
    }
    use std::os::unix::fs::MetadataExt;
    let me = unsafe { libc::geteuid() };
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in dir.filter_map(Result::ok) {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let base = e.path();
        if std::fs::metadata(&base).map(|m| m.uid()).ok() != Some(me) {
            continue;
        }
        let Ok(comm) = std::fs::read_to_string(base.join("comm")) else {
            continue;
        };
        let name = comm.trim().to_owned();
        let mut path = std::fs::read_link(base.join("exe"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        // A script run by node: the tool is the script.
        if name == "node"
            && let Ok(cmd) = std::fs::read(base.join("cmdline"))
        {
            for arg in cmd.split(|b| *b == 0).skip(1).take(2) {
                path.push(' ');
                path.push_str(&String::from_utf8_lossy(arg));
            }
        }
        out.push(Proc { pid, name, path });
    }
    out
}

#[cfg(not(unix))]
pub fn list_processes() -> Vec<Proc> {
    Vec::new()
}

/// The current user's established TCP connections. Blocking.
#[cfg(unix)]
pub fn list_connections() -> Vec<Conn> {
    if cfg!(target_os = "macos") {
        let Some(lsof) = crate::procname::lsof_binary() else {
            return Vec::new();
        };
        return std::process::Command::new(lsof)
            .args(["-nP", "+c", "0", "-iTCP", "-sTCP:ESTABLISHED", "-Fpcn"])
            .output()
            .map(|o| parse_lsof(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default();
    }
    let ss = ["/usr/bin/ss", "/usr/sbin/ss", "/bin/ss", "/sbin/ss"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("ss");
    std::process::Command::new(ss)
        .args(["-tnpH", "state", "established"])
        .output()
        .map(|o| parse_ss(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

#[cfg(not(unix))]
pub fn list_connections() -> Vec<Conn> {
    Vec::new()
}

/// Looks for bypasses now. Blocking (two short commands at most); the
/// socket table is read only when some process looks like an AI tool.
/// `KAIRO_NO_BYPASS=1` looks for nothing (screenshots, tests on a busy machine).
pub fn scan(mixed_port: u16, exclude: &[u32], now: i64) -> Vec<Bypass> {
    if std::env::var_os("KAIRO_NO_BYPASS").is_some_and(|v| !v.is_empty()) {
        return Vec::new();
    }
    let home = std::env::var("HOME").ok();
    let procs = list_processes();
    let candidates: Vec<Proc> = procs
        .into_iter()
        .filter(|p| !exclude.contains(&p.pid))
        .filter(|p| tool_of(&p.name, &p.path, home.as_deref()).is_some())
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    detect(
        &list_connections(),
        &candidates,
        mixed_port,
        exclude,
        home.as_deref(),
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ss -tnpH state established` as iproute2 prints it (the State column
    /// is left out by the filter), with an IPv6 peer, a mapped address, a
    /// name with spaces and parentheses, a forked tool and another user's
    /// connection (no `users:`).
    const SS: &str = "\
0      0          192.168.1.20:51234   160.79.104.10:443   users:((\"claude\",pid=4242,fd=19))
0      0          192.168.1.20:51236    104.18.32.47:443   users:((\"node\",pid=4300,fd=22),(\"node\",pid=4301,fd=22))
0      0             127.0.0.1:41234       127.0.0.1:7897  users:((\"claude\",pid=4242,fd=20))
0      0        [2001:db8:1::5]:50422 [2606:4700:4400::6812:2a2f]:443 users:((\"Cursor Helper (Plugin)\",pid=5150,fd=33))
0      0  [::ffff:192.168.1.20]:46000 [::ffff:140.82.112.21]:443 users:((\"copilot\",pid=77,fd=9))
0      0          192.168.1.20:40000    192.168.1.50:11434 users:((\"claude\",pid=4242,fd=25))
0      0          192.168.1.20:22        192.168.1.9:50511
ESTAB  0      0   [fe80::1]%eth0:50000  [fe80::2]%eth0:443 users:((\"odd,name\",pid=9,fd=3))
0      0          192.168.1.20:41000     34.120.0.1:443    users:((\"a\",pid=abc,fd=3))
";

    #[test]
    fn reads_ss_output() {
        let c = parse_ss(SS);
        let by = |pid: u32| c.iter().filter(|x| x.pid == pid).collect::<Vec<_>>();
        assert_eq!(by(4242).len(), 3);
        assert_eq!(
            by(4242)[0].remote,
            "160.79.104.10".parse::<IpAddr>().unwrap()
        );
        assert_eq!(by(4242)[0].port, 443);
        assert_eq!(by(4242)[0].name, "claude");
        // A forked tool: one entry for each process.
        assert_eq!(by(4300).len() + by(4301).len(), 2);
        // Names with spaces and parentheses; IPv6 peers.
        let cursor = by(5150);
        assert_eq!(cursor[0].name, "Cursor Helper (Plugin)");
        assert_eq!(
            cursor[0].remote,
            "2606:4700:4400::6812:2a2f".parse::<IpAddr>().unwrap()
        );
        // An IPv4-mapped address is the IPv4 one.
        assert_eq!(by(77)[0].remote, "140.82.112.21".parse::<IpAddr>().unwrap());
        // A zone id is dropped; a comma in a name is kept; the State column is tolerated.
        assert_eq!(by(9)[0].name, "odd,name");
        assert_eq!(by(9)[0].remote, "fe80::2".parse::<IpAddr>().unwrap());
        // Another user's connection, and an unreadable pid, give nothing.
        assert!(
            c.iter()
                .all(|x| x.remote != "192.168.1.9".parse::<IpAddr>().unwrap())
        );
        assert_eq!(c.len(), 3 + 2 + 1 + 1 + 1);
    }

    const LSOF: &str = "\
p615
cCursor Helper (Renderer)
n127.0.0.1:52235->127.0.0.1:7897
n192.168.1.20:52300->104.18.32.47:443
n[2001:db8:1::5]:52301->[2606:4700::6812:2a2f]:443
n*:3000
p700
cclaude
n192.168.1.20:52400->160.79.104.10:443
p701
n192.168.1.20:52401->1.1.1.1:443
";

    #[test]
    fn reads_lsof_output() {
        let c = parse_lsof(LSOF);
        assert_eq!(c.len(), 4);
        assert_eq!(c[0].pid, 615);
        assert_eq!(c[0].name, "Cursor Helper (Renderer)");
        assert_eq!(
            (c[0].remote, c[0].port),
            ("127.0.0.1".parse().unwrap(), 7897)
        );
        assert_eq!(
            c[2].remote,
            "2606:4700::6812:2a2f".parse::<IpAddr>().unwrap()
        );
        assert_eq!((c[3].pid, c[3].name.as_str()), (700, "claude"));
        // A process with no `c` line (p701) gives no connection, and no name carries over.
        assert!(c.iter().all(|x| x.pid != 701));
    }

    #[test]
    fn reads_ps_output() {
        let p = parse_ps(
            "  615 /Applications/Cursor.app/Contents/MacOS/Cursor\n 9001 /usr/bin/curl\n  oops\n 12 /Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Renderer).app/Contents/MacOS/Cursor Helper (Renderer)\n",
        );
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].name, "Cursor");
        assert_eq!(p[0].path, "/Applications/Cursor.app/Contents/MacOS/Cursor");
        assert_eq!(p[2].name, "Cursor Helper (Renderer)");
    }

    #[test]
    fn local_traffic_is_never_a_bypass() {
        for local in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "172.31.255.1",
            "192.168.1.50",
            "169.254.1.1",
            "100.100.1.1",
            "198.18.0.7",
            "::1",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:192.168.1.5",
            "0.0.0.0",
        ] {
            assert!(is_local(local.parse().unwrap()), "{local}");
        }
        for public in [
            "160.79.104.10",
            "172.32.0.1",
            "100.63.0.1",
            "100.128.0.1",
            "198.20.0.1",
            "2606:4700::1",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_local(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn a_tool_is_known_by_its_name_and_where_it_lives_not_by_the_users_name() {
        let id = |n: &str, p: &str, h: Option<&str>| tool_of(n, p, h).map(|s| s.id);
        assert_eq!(id("claude", "", None), Some("claude"));
        assert_eq!(
            id(
                "node",
                "/usr/bin/node /home/me/.npm/_npx/claude-code/cli.js",
                None
            ),
            Some("claude")
        );
        assert_eq!(
            id(
                "Cursor",
                "/Applications/Cursor.app/Contents/MacOS/Cursor",
                Some("/Users/me")
            ),
            Some("cursor")
        );
        assert_eq!(
            id("foo", "/Users/me/.local/bin/codex", Some("/Users/me")),
            Some("chatgpt")
        );
        // A user called "claude": their home folder is not a tool.
        assert_eq!(
            id("curl", "/home/claude/bin/curl", Some("/home/claude")),
            None
        );
        assert_eq!(id("curl", "/usr/bin/curl", Some("/home/me")), None);
        assert_eq!(id("firefox", "/usr/lib/firefox/firefox", None), None);
    }

    fn proc(pid: u32, name: &str, path: &str) -> Proc {
        Proc {
            pid,
            name: name.into(),
            path: path.into(),
        }
    }

    #[test]
    fn finds_direct_connections_of_ai_tools_only() {
        let procs = vec![
            proc(4242, "claude", "/home/me/.local/bin/claude"),
            proc(4300, "node", "/usr/bin/node /opt/claude-code/cli.js"),
            proc(5150, "Cursor Helper (Plugin)", "/opt/Cursor/cursor"),
            proc(9000, "curl", "/usr/bin/curl"),
            proc(1, "kairo", "/usr/bin/kairo"),
            proc(2, "gemini", "/usr/bin/gemini"),
        ];
        let c = parse_ss(SS);
        let mut all = c.clone();
        // A tool that is not one; Kairo itself (even if its name were a tool's);
        // a tool to a remote proxy's port; a second port to the same address.
        all.push(Conn {
            pid: 9000,
            name: "curl".into(),
            remote: "1.2.3.4".parse().unwrap(),
            port: 443,
        });
        all.push(Conn {
            pid: 2,
            name: "gemini".into(),
            remote: "1.2.3.4".parse().unwrap(),
            port: 443,
        });
        all.push(Conn {
            pid: 4242,
            name: "claude".into(),
            remote: "9.9.9.9".parse().unwrap(),
            port: 7897,
        });
        all.push(Conn {
            pid: 4242,
            name: "claude".into(),
            remote: "160.79.104.10".parse().unwrap(),
            port: 8443,
        });
        let found = detect(&all, &procs, 7897, &[2], Some("/home/me"), 1000);
        let view: Vec<(&str, Option<&str>, u32, &str)> = found
            .iter()
            .map(|b| (b.tool.as_str(), b.service, b.pid, b.remote.as_str()))
            .collect();
        assert_eq!(
            view,
            [
                // claude to Anthropic: once, though it has two sockets there.
                ("claude", Some("claude"), 4242, "160.79.104.10:443"),
                // node running claude-code: its two forks to the same address are one.
                ("node", Some("claude"), 4300, "104.18.32.47:443"),
                // The IPv6 peer, in brackets.
                (
                    "Cursor Helper (Plugin)",
                    Some("cursor"),
                    5150,
                    "[2606:4700:4400::6812:2a2f]:443"
                ),
            ]
        );
        assert!(found.iter().all(|b| b.since == 1000));
        // Not the proxy (127.0.0.1:7897), not the LAN model (192.168.1.50:11434),
        // not curl, not the excluded pid, not a remote on the proxy's port.
        assert!(
            found
                .iter()
                .all(|b| !b.remote.contains("11434") && !b.remote.contains("9.9.9.9"))
        );
        assert!(found.iter().all(|b| b.tool != "curl" && b.tool != "gemini"));
    }

    #[test]
    fn a_bypass_keeps_the_time_it_was_first_seen() {
        let b = |tool: &str, remote: &str, since: i64| Bypass {
            tool: tool.into(),
            service: Some("claude"),
            pid: 1,
            remote: remote.into(),
            since,
        };
        let before = vec![b("claude", "160.79.104.10:443", 100)];
        let merged = merge_since(
            &before,
            vec![
                b("claude", "160.79.104.10:8443", 500),
                b("codex", "104.18.1.1:443", 500),
                b("claude", "[2606:4700::1]:443", 500),
            ],
        );
        // Same tool and address (another port): the old time. New ones: now.
        assert_eq!(merged[0].since, 100);
        assert_eq!(merged[0].remote, "160.79.104.10:8443");
        assert!(merged[1..].iter().all(|x| x.since == 500));
        assert_eq!(remote_ip("[2606:4700::1]:443"), "2606:4700::1");
        assert!(merge_since(&before, vec![]).is_empty());
    }

    #[test]
    fn the_terminal_block_is_one_line_with_both_cases() {
        let t = terminal_env(7897);
        assert!(!t.contains('\n'));
        for var in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            assert!(t.contains(&format!("{var}=http://127.0.0.1:7897")), "{var}");
        }
        for var in ["NO_PROXY", "no_proxy"] {
            assert!(
                t.contains(&format!(
                    "{var}=localhost,127.0.0.1,::1,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,.local"
                )),
                "{var}"
            );
        }
        assert!(t.starts_with("export "));
        assert!(terminal_env(1234).contains("127.0.0.1:1234"));
    }

    /// The real `ss` prints what the parser reads (skipped without `ss`).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_ss_shows_this_process_connection() {
        if !["/usr/bin/ss", "/usr/sbin/ss", "/bin/ss", "/sbin/ss"]
            .iter()
            .any(|p| std::path::Path::new(p).exists())
        {
            return;
        }
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let _c = std::net::TcpStream::connect(addr).unwrap();
        let _s = l.accept().unwrap();
        let me = std::process::id();
        let found = list_connections()
            .into_iter()
            .any(|c| c.pid == me && c.port == addr.port() && c.remote.is_loopback());
        assert!(found, "ss lists this process's loopback connection");
        // ... and a scan with it never calls it a bypass (it is local).
        assert!(scan(7897, &[], 0).iter().all(|b| b.pid != me));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lists_this_process_among_the_users_own() {
        let me = std::process::id();
        let procs = list_processes();
        let mine = procs
            .iter()
            .find(|p| p.pid == me)
            .expect("this process is listed");
        assert!(!mine.name.is_empty());
        // Another user's processes (init) are not.
        assert!(procs.iter().all(|p| p.pid != 1) || unsafe { libc::geteuid() } == 0);
    }
}
