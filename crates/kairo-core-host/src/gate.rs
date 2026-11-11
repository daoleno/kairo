//! Which controller calls the host forwards to the core.
//!
//! The core's controller can load any config (`PUT /configs` takes a `path` or a
//! whole `payload`), restart and upgrade the core, and through `PATCH /configs`
//! open listeners and name key files. Whoever can reach it can therefore make a
//! SYSTEM process do almost anything, whatever was checked at start. So the core's
//! own controller is the host's secret, and the user's Kairo talks to the host's
//! front of it, which forwards only what Kairo asks for, below, and nothing else.

use serde_json::Value;

/// What the host does with one request.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Pass it to the core as it is.
    Forward,
    /// `PUT /configs`, whatever it carries: re-read the inbox, sanitize it and
    /// load it from the host's own path.
    Reload,
    Deny(&'static str),
}

/// The streams Kairo reads over a WebSocket; an upgrade on any other path is
/// not passed on (it would keep the connection open for a second request).
pub fn is_stream(method: &str, target: &str) -> bool {
    let path = target.split_once('?').map_or(target, |(p, _)| p);
    method == "GET" && matches!(path, "/traffic" | "/logs" | "/connections" | "/memory")
}

/// Biggest body the host reads from the user.
pub const MAX_BODY: usize = 64 * 1024;

const PATCH_KEYS: &[&str] = &["mode", "log-level"];
const MODES: &[&str] = &["rule", "global", "direct"];
const LOG_LEVELS: &[&str] = &["silent", "error", "warning", "info", "debug"];

/// One path segment of a name Kairo sends: percent-encoded text, never a path.
fn segment_ok(s: &str) -> bool {
    // Read an encoded slash as the slash it stands for: no part may be a dot or
    // two (a core that decoded before routing would then walk up a path).
    let decoded = s.replace("%2F", "/").replace("%2f", "/");
    !s.is_empty()
        && s.len() <= 1024
        && !s.contains(['\\', '\0'])
        && decoded.split('/').all(|part| part != "." && part != "..")
}

/// Decides on a request. `target` is the request target as sent (`/path?query`).
pub fn decide(method: &str, target: &str, body: &[u8]) -> Verdict {
    let (path, _query) = target.split_once('?').unwrap_or((target, ""));
    let lower = path.to_ascii_lowercase();
    // Encoded dots, backslashes and NULs are the ways to say a different path.
    // An encoded slash stays: a node's or group's name can hold one (Kairo encodes
    // it), but only as part of a name, never as a way to reach another route.
    if !path.starts_with('/')
        || lower.contains("%2e")
        || lower.contains("%5c")
        || lower.contains("%00")
        || path.contains("//")
        || path.chars().any(|c| c.is_control() || !c.is_ascii())
    {
        return Verdict::Deny("path");
    }
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if segs.iter().skip(1).any(|s| !segment_ok(s)) {
        return Verdict::Deny("path");
    }
    let s: Vec<&str> = if segs == [""] { vec![] } else { segs };
    let deny = Verdict::Deny("not a call Kairo makes");
    match (method, s.as_slice()) {
        ("GET", []) => Verdict::Forward,
        ("GET", ["version" | "configs" | "proxies" | "rules" | "connections" | "traffic" | "memory" | "logs"]) => {
            Verdict::Forward
        }
        ("GET", ["proxies", _] | ["proxies", _, "delay"] | ["group", _, "delay"]) => Verdict::Forward,
        ("GET", ["providers", "proxies" | "rules"]) => Verdict::Forward,
        ("GET", ["providers", "proxies", _, "healthcheck"]) => Verdict::Forward,
        ("PUT", ["providers", "proxies" | "rules", _]) => Verdict::Forward,
        ("GET", ["dns", "query"]) => Verdict::Forward,
        ("DELETE", ["connections"] | ["connections", _]) => Verdict::Forward,
        ("POST", ["cache", "fakeip", "flush"]) => Verdict::Forward,
        ("POST", ["configs", "geo"]) => Verdict::Forward,
        ("PUT", ["proxies", _]) => select_body(body),
        ("PATCH", ["configs"]) => patch_body(body),
        // The one way to change the config: the host's, from the inbox.
        ("PUT", ["configs"]) => Verdict::Reload,
        _ => deny,
    }
}

/// `{"name": "<a node>"}`, nothing else.
fn select_body(body: &[u8]) -> Verdict {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(m)) if m.len() == 1 && m.get("name").is_some_and(Value::is_string) => Verdict::Forward,
        _ => Verdict::Deny("body"),
    }
}

/// `{"mode": …}` and `{"log-level": …}` with a known value; no listener, no path.
fn patch_body(body: &[u8]) -> Verdict {
    let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(body) else {
        return Verdict::Deny("body");
    };
    if m.is_empty() {
        return Verdict::Deny("body");
    }
    for (k, v) in &m {
        let ok = PATCH_KEYS.contains(&k.as_str())
            && v.as_str().is_some_and(|v| match k.as_str() {
                "mode" => MODES.contains(&v),
                _ => LOG_LEVELS.contains(&v),
            });
        if !ok {
            return Verdict::Deny("body");
        }
    }
    Verdict::Forward
}

/// Whether this request asks for a WebSocket the host will pass on.
pub fn wants_stream(head: &Head) -> bool {
    is_stream(&head.method, &head.target)
        && head.header("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
}

/// A parsed request head.
#[derive(Debug)]
pub struct Head {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    /// Bytes the head took in the buffer.
    pub len: usize,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Reads a request head from `buf`; `Ok(None)` while it is incomplete.
pub fn parse_head(buf: &[u8]) -> Result<Option<Head>, &'static str> {
    let mut storage = [httparse::EMPTY_HEADER; 48];
    let mut req = httparse::Request::new(&mut storage);
    match req.parse(buf) {
        Ok(httparse::Status::Complete(len)) => Ok(Some(Head {
            method: req.method.unwrap_or("").to_owned(),
            target: req.path.unwrap_or("").to_owned(),
            headers: req
                .headers
                .iter()
                .map(|h| (h.name.to_owned(), String::from_utf8_lossy(h.value).into_owned()))
                .collect(),
            len,
        })),
        Ok(httparse::Status::Partial) => Ok(None),
        Err(_) => Err("not an HTTP request"),
    }
}

/// Whether the request authenticates with `secret` (`Authorization: Bearer`).
/// Compared in constant time.
pub fn authorized(head: &Head, secret: &str) -> bool {
    let Some(v) = head.header("authorization") else { return false };
    let Some(given) = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")) else {
        return false;
    };
    let (a, b) = (given.as_bytes(), secret.as_bytes());
    let mut diff = (a.len() != b.len()) as u8;
    for i in 0..a.len().min(b.len()) {
        diff |= a[i] ^ b[i];
    }
    diff == 0 && !secret.is_empty()
}

/// The request the core receives: the headers Kairo's calls need, the host's
/// secret in place of the user's.
pub fn forward_head(head: &Head, internal_port: u16, internal_secret: &str, body_len: usize) -> String {
    let websocket = is_stream(&head.method, &head.target)
        && head
            .header("upgrade")
            .is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    let mut out = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{internal_port}\r\nAuthorization: Bearer {internal_secret}\r\nConnection: {}\r\n",
        head.method,
        head.target,
        if websocket { "Upgrade" } else { "close" }
    );
    if let Some(ct) = head.header("content-type").filter(|v| !v.contains(['\r', '\n'])) {
        out.push_str(&format!("Content-Type: {ct}\r\n"));
    }
    if body_len > 0 || matches!(head.method.as_str(), "PUT" | "POST" | "PATCH") {
        out.push_str(&format!("Content-Length: {body_len}\r\n"));
    }
    if websocket {
        out.push_str("Upgrade: websocket\r\n");
        for h in ["sec-websocket-key", "sec-websocket-version", "sec-websocket-protocol", "sec-websocket-extensions"] {
            if let Some(v) = head.header(h).filter(|v| !v.contains(['\r', '\n'])) {
                out.push_str(&format!("{h}: {v}\r\n"));
            }
        }
    }
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(m: &str, t: &str) -> Verdict {
        decide(m, t, b"")
    }

    #[test]
    fn the_calls_kairo_makes_are_forwarded() {
        for (m, t) in [
            ("GET", "/version"),
            ("GET", "/configs"),
            ("GET", "/proxies"),
            ("GET", "/proxies/Tokyo%2003/delay?timeout=5000&url=https%3A%2F%2Fx"),
            ("GET", "/group/Auto/delay?timeout=5000&url=x"),
            ("GET", "/providers/proxies"),
            ("PUT", "/providers/proxies/web"),
            ("GET", "/providers/proxies/web/healthcheck"),
            ("GET", "/providers/rules"),
            ("PUT", "/providers/rules/r"),
            ("GET", "/rules"),
            ("GET", "/connections"),
            ("DELETE", "/connections"),
            ("DELETE", "/connections/abc-123"),
            ("GET", "/dns/query?name=a.com&type=A"),
            ("POST", "/cache/fakeip/flush"),
            ("POST", "/configs/geo"),
            ("GET", "/traffic"),
            ("GET", "/memory"),
            ("GET", "/logs?level=info"),
        ] {
            assert_eq!(d(m, t), Verdict::Forward, "{m} {t}");
        }
        assert_eq!(decide("PUT", "/proxies/Proxies", br#"{"name":"Tokyo 03"}"#), Verdict::Forward);
        assert_eq!(decide("PATCH", "/configs", br#"{"mode":"global"}"#), Verdict::Forward);
    }

    #[test]
    fn restart_upgrade_ui_storage_and_the_rest_are_refused() {
        for (m, t) in [
            ("POST", "/restart"),
            ("POST", "/upgrade"),
            ("POST", "/upgrade/ui"),
            ("POST", "/upgrade/geo"),
            ("GET", "/storage/x"),
            ("PUT", "/storage/x"),
            ("GET", "/ui/"),
            ("PUT", "/debug/gc"),
            ("GET", "/debug/pprof/"),
            ("GET", "/doh"),
            ("POST", "/connections"),
            ("PUT", "/providers/proxies"),
            ("DELETE", "/configs"),
            ("GET", "/"),
        ] {
            let v = d(m, t);
            if (m, t) == ("GET", "/") {
                assert_eq!(v, Verdict::Forward);
            } else {
                assert!(matches!(v, Verdict::Deny(_)), "{m} {t} -> {v:?}");
            }
        }
    }

    #[test]
    fn a_name_with_a_slash_is_one_name() {
        // Kairo encodes a slash in a node's name (`controller::segment`).
        assert_eq!(d("GET", "/proxies/Tokyo%2F03/delay?timeout=5000&url=x"), Verdict::Forward);
        assert_eq!(decide("PUT", "/proxies/Auto%2FFast", br#"{"name":"JP%2F1"}"#), Verdict::Forward);
        assert_eq!(d("PUT", "/providers/proxies/a%2Fb"), Verdict::Forward);
    }

    #[test]
    fn only_the_streams_may_be_upgraded_to_a_websocket() {
        let ws = |target: &str| {
            let raw = format!("GET {target} HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: k\r\n\r\n");
            parse_head(raw.as_bytes()).unwrap().unwrap()
        };
        for t in ["/traffic", "/logs?level=info", "/connections", "/memory"] {
            let h = ws(t);
            assert!(wants_stream(&h), "{t}");
            assert!(forward_head(&h, 1, "s", 0).contains("Upgrade: websocket"));
        }
        // An upgrade on a plain call is a way to keep the pipe open for a second
        // request: it is passed on as a plain request, closing after the answer.
        for t in ["/version", "/proxies", "/configs"] {
            let h = ws(t);
            assert!(!wants_stream(&h), "{t}");
            let out = forward_head(&h, 1, "s", 0);
            assert!(out.contains("Connection: close\r\n") && !out.to_lowercase().contains("upgrade"), "{t}: {out}");
        }
    }

    #[test]
    fn put_configs_is_always_a_reload_from_the_inbox() {
        for body in [
            br#"{"path":"C:\\Users\\x\\evil.yaml","payload":""}"#.as_slice(),
            br#"{"payload":"external-ui: C:/Windows"}"#,
            b"",
        ] {
            assert_eq!(decide("PUT", "/configs?force=true", body), Verdict::Reload);
        }
    }

    #[test]
    fn patch_configs_cannot_open_listeners_or_name_files() {
        for body in [
            br#"{"tcptun-config":"127.0.0.1:1,127.0.0.1:2"}"#.as_slice(),
            br#"{"tun":{"enable":true,"route-address-set":["x"]}}"#,
            br#"{"tuic-server":{"enable":true,"certificate":"C:/x"}}"#,
            br#"{"mixed-port":80}"#,
            br#"{"allow-lan":true}"#,
            br#"{"mode":"global","interface-name":"x"}"#,
            br#"{"mode":"evil"}"#,
            br#"{}"#,
            b"not json",
        ] {
            assert!(matches!(decide("PATCH", "/configs", body), Verdict::Deny(_)), "{}", String::from_utf8_lossy(body));
        }
    }

    #[test]
    fn a_node_selection_carries_a_name_and_nothing_else() {
        assert!(matches!(decide("PUT", "/proxies/G", br#"{"name":"a","x":1}"#), Verdict::Deny(_)));
        assert!(matches!(decide("PUT", "/proxies/G", br#"{"name":5}"#), Verdict::Deny(_)));
        assert!(matches!(decide("PUT", "/proxies/G", b""), Verdict::Deny(_)));
    }

    #[test]
    fn another_way_to_write_a_path_is_refused() {
        for t in [
            "/proxies/%2e%2e/restart",
            "/proxies/..%2frestart",
            "/proxies/..%2Frestart",
            "/proxies/a%2F..%2Frestart",
            "/proxies/a%5cb",
            "/proxies/../restart",
            "/proxies/./x",
            "//restart",
            "/proxies//x",
            "restart",
            "/proxies/a\u{e9}",
            "/proxies/a%00",
            "/proxies/a\\b",
        ] {
            assert!(matches!(d("GET", t), Verdict::Deny(_)), "{t}");
        }
    }

    #[test]
    fn heads_are_parsed_and_the_secret_checked() {
        let raw = b"GET /version HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer s3cret-value-here\r\n\r\n";
        let h = parse_head(raw).unwrap().unwrap();
        assert_eq!(h.method, "GET");
        assert_eq!(h.len, raw.len());
        assert!(authorized(&h, "s3cret-value-here"));
        assert!(!authorized(&h, "s3cret-value-herf"));
        assert!(!authorized(&h, ""));
        let none = parse_head(b"GET /version HTTP/1.1\r\nHost: x\r\n\r\n").unwrap().unwrap();
        assert!(!authorized(&none, "anything"));
        assert!(parse_head(b"GET /ver").unwrap().is_none());
        assert!(parse_head(b"\x00\x01garbage").is_err());
    }

    #[test]
    fn the_core_sees_the_hosts_secret_never_the_users() {
        let raw = b"PUT /proxies/G HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer users-secret-0000000\r\nContent-Type: application/json\r\nX-Evil: 1\r\n\r\n";
        let h = parse_head(raw).unwrap().unwrap();
        let out = forward_head(&h, 40000, "host-only", 17);
        assert!(out.contains("Authorization: Bearer host-only\r\n"));
        assert!(!out.contains("users-secret") && !out.contains("X-Evil"));
        assert!(out.contains("Content-Length: 17\r\n") && out.contains("Connection: close\r\n"));
        assert!(out.ends_with("\r\n\r\n"));
        // A WebSocket keeps its upgrade, with one Connection header.
        let ws = b"GET /traffic HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer users-secret-0000000\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: abc==\r\nSec-WebSocket-Version: 13\r\n\r\n";
        let h = parse_head(ws).unwrap().unwrap();
        let out = forward_head(&h, 40000, "host-only", 0);
        assert_eq!(out.matches("Connection:").count(), 1);
        assert!(out.contains("Connection: Upgrade\r\n") && out.contains("Upgrade: websocket\r\n"));
        assert!(out.contains("sec-websocket-key: abc==\r\n"));
    }
}
