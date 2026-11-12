//! The server's controls, each tried against the real program: a
//! `kairo-server` on a scratch folder, reached over HTTP and WebSocket the
//! way a browser (or an attacker's page) would. See SECURITY-NOTES.md.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::StatusCode;
use reqwest::header::{COOKIE, HOST, ORIGIN, SET_COOKIE};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{self, Message, client::IntoClientRequest};

const PASSWORD: &str = "correct horse battery";

/// A running `kairo-server` on its own folder.
struct Server {
    child: Child,
    port: u16,
    home: PathBuf,
    setup_code: Option<String>,
    /// Everything it wrote, stdout and stderr.
    output: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A scratch place: Kairo's folder in `home` (no subscription, no core, geo
/// data in place so the engine fetches nothing) and the page beside it in
/// `page`, as a package lays them out (never one inside the other).
fn folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("home/runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    for name in ["geoip.metadb", "GeoSite.dat", "Country.mmdb", "GeoIP.dat"] {
        let f = std::fs::File::create(runtime.join(name)).unwrap();
        f.set_len(1024 * 1024).unwrap();
    }
    let web = dir.path().join("page");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html><title>Kairo</title><script>1</script>").unwrap();
    dir
}

/// Starts a server on the scratch place `dir` (see [`folder`]).
fn start(dir: &Path, extra: &[&str]) -> Server {
    start_with(dir, extra, &[])
}

fn start_with(dir: &Path, extra: &[&str], envs: &[(&str, &str)]) -> Server {
    let home = dir.join("home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_kairo-server"))
        .args(["serve", "--listen", "127.0.0.1:0", "--web"])
        .arg(dir.join("page"))
        .args(extra)
        .env("KAIRO_HOME", &home)
        .env("KAIRO_NO_SYSPROXY", "1")
        .env("KAIRO_NO_UPDATE", "1")
        .env("KAIRO_NO_ICON_FETCH", "1")
        .env("RUST_LOG", "error")
        .envs(envs.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("kairo-server runs");
    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let (out, keep) = (BufReader::new(child.stdout.take().unwrap()), output.clone());
        std::thread::spawn(move || {
            for line in out.lines().map_while(Result::ok) {
                keep.lock().unwrap().push(line.clone());
                let _ = tx.send(line);
            }
        });
        let (err, keep) = (BufReader::new(child.stderr.take().unwrap()), output.clone());
        std::thread::spawn(move || {
            for line in err.lines().map_while(Result::ok) {
                keep.lock().unwrap().push(line);
            }
        });
    }
    loop {
        let line = rx.recv_timeout(Duration::from_secs(30)).expect("the server says where it is");
        // The harness's own address (others may come first, or never).
        if let Some(rest) = line.strip_prefix("Kairo is at http://").filter(|r| r.starts_with("localhost:")) {
            let port = rest.rsplit(':').next().unwrap().trim().parse().unwrap();
            // As its owner reads it (`kairo-server status`): never printed
            // where a journal keeps it.
            let setup_code = std::fs::read_to_string(home.join("web/setup-code")).ok().map(|c| c.trim().to_owned());
            return Server { child, port, home, setup_code, output };
        }
    }
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    async fn post(&self, path: &str, body: Value, cookie: Option<&str>) -> reqwest::Response {
        let mut r = reqwest::Client::new()
            .post(self.url(path))
            .header(ORIGIN, self.origin())
            .json(&body);
        if let Some(c) = cookie {
            r = r.header(COOKIE, c);
        }
        r.send().await.unwrap()
    }

    async fn session_state(&self, cookie: Option<&str>) -> String {
        let mut r = reqwest::Client::new().get(self.url("/api/session"));
        if let Some(c) = cookie {
            r = r.header(COOKIE, c);
        }
        let v: Value = r.send().await.unwrap().json().await.unwrap();
        v["state"].as_str().unwrap().to_owned()
    }

    /// Sets the password with the setup code: the session's cookie.
    async fn set_up(&self) -> String {
        let code = self.setup_code.clone().expect("a fresh folder has a setup code");
        let r = self.post("/api/setup", json!({ "code": code, "password": PASSWORD }), None).await;
        assert_eq!(r.status(), StatusCode::OK);
        cookie_of(&r)
    }

    async fn sign_in(&self) -> String {
        let r = self.post("/api/login", json!({ "password": PASSWORD }), None).await;
        assert_eq!(r.status(), StatusCode::OK);
        cookie_of(&r)
    }

    /// Opens the page's socket with these headers.
    async fn socket(
        &self,
        cookie: Option<&str>,
        origin: Option<&str>,
    ) -> Result<Ws, tungstenite::Error> {
        let mut request = format!("ws://127.0.0.1:{}/api/ws", self.port).into_client_request().unwrap();
        if let Some(c) = cookie {
            request.headers_mut().insert(COOKIE, c.parse().unwrap());
        }
        if let Some(o) = origin {
            request.headers_mut().insert(ORIGIN, o.parse().unwrap());
        }
        tokio_tungstenite::connect_async(request).await.map(|(ws, _)| ws)
    }
}

/// The child's exit and stderr if it exits within `limit`; else it is killed: `None`.
fn exit_within(mut child: Child, limit: Duration) -> Option<(std::process::ExitStatus, String)> {
    use std::io::Read;
    let deadline = std::time::Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            return Some((status, err));
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// `name=value` of the response's cookie.
fn cookie_of(r: &reqwest::Response) -> String {
    let set = r.headers().get(SET_COOKIE).expect("a cookie").to_str().unwrap();
    set.split(';').next().unwrap().to_owned()
}

/// Every text frame for `wait`, and how the socket closed (if it did).
async fn frames(ws: &mut Ws, wait: Duration) -> (Vec<String>, Option<u16>) {
    let mut texts = Vec::new();
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return (texts, None),
            Ok(None) | Ok(Some(Err(_))) => return (texts, Some(0)),
            Ok(Some(Ok(Message::Text(t)))) => texts.push(t.to_string()),
            Ok(Some(Ok(Message::Close(frame)))) => {
                return (texts, Some(frame.map(|f| u16::from(f.code)).unwrap_or(0)));
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

async fn send(ws: &mut Ws, id: u64, cmd: &str, args: Value) {
    let msg = if args.is_null() { json!({ "id": id, "cmd": cmd }) } else { json!({ "id": id, "cmd": cmd, "args": args }) };
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();
}

#[tokio::test]
async fn first_run_needs_the_setup_code_and_no_password_is_set_by_default() {
    let dir = folder();
    let server = start(dir.path(), &[]);
    assert_eq!(server.session_state(None).await, "setup");
    // No default password works.
    for guess in ["", "admin", "kairo", "password"] {
        let r = server.post("/api/login", json!({ "password": guess }), None).await;
        assert_ne!(r.status(), StatusCode::OK, "{guess:?}");
    }
    // A wrong code sets nothing.
    let r = server.post("/api/setup", json!({ "code": "000000", "password": PASSWORD }), None).await;
    assert_ne!(r.status(), StatusCode::OK);
    assert_eq!(server.session_state(None).await, "setup");
    // The code is for its owner (`kairo-server status`, the file): never in
    // what the server writes out, which a journal keeps.
    let code = server.setup_code.clone().unwrap();
    let said = server.output.lock().unwrap().join("\n");
    assert!(!said.contains(&code), "the setup code went out: {said}");
    // The right one does, once.
    server.set_up().await;
    let again = server.setup_code.clone().unwrap();
    let r = server.post("/api/setup", json!({ "code": again, "password": "another password" }), None).await;
    assert_eq!(r.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_session_cookie_is_http_only_strict_and_this_hosts_alone() {
    let dir = folder();
    let server = start(dir.path(), &[]);
    let r = server
        .post("/api/setup", json!({ "code": server.setup_code.clone().unwrap(), "password": PASSWORD }), None)
        .await;
    let set = r.headers().get(SET_COOKIE).unwrap().to_str().unwrap().to_owned();
    assert!(set.starts_with("kairo_session="), "{set}");
    for part in ["HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(set.contains(part), "{part} in {set}");
    }
    // No Domain: the browser sends it to this host only (not to its subdomains).
    assert!(!set.to_ascii_lowercase().contains("domain="), "{set}");
    // The token is not in the body either (only the HttpOnly cookie carries it).
    let body = r.text().await.unwrap();
    let token = set.split(';').next().unwrap().split_once('=').unwrap().1;
    assert!(!body.contains(token));
}

#[tokio::test]
async fn behind_https_the_cookie_is_secure_and_host_prefixed() {
    let dir = folder();
    let server = start(dir.path(), &["--public-url", "https://kairo.example.ts.net"]);
    let r = reqwest::Client::new()
        .post(server.url("/api/setup"))
        .header(HOST, "kairo.example.ts.net")
        .header(ORIGIN, "https://kairo.example.ts.net")
        .json(&json!({ "code": server.setup_code.clone().unwrap(), "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let set = r.headers().get(SET_COOKIE).unwrap().to_str().unwrap();
    assert!(set.starts_with("__Host-kairo_session="), "{set}");
    assert!(set.contains("; Secure"), "{set}");
    assert!(r.headers().get("strict-transport-security").is_some());
}

#[tokio::test]
async fn another_name_for_this_address_is_refused() {
    // DNS rebinding: a page of evil.example whose name now points here.
    let dir = folder();
    let server = start(dir.path(), &[]);
    let r = reqwest::Client::new()
        .get(server.url("/api/session"))
        .header(HOST, format!("evil.example:{}", server.port))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::MISDIRECTED_REQUEST);
    // A name it was given is answered.
    let dir2 = folder();
    let named = start(dir2.path(), &["--host", "kairo.home.arpa"]);
    let r = reqwest::Client::new()
        .get(named.url("/api/session"))
        .header(HOST, format!("kairo.home.arpa:{}", named.port))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn another_sites_page_can_not_sign_in_or_open_the_socket() {
    let dir = folder();
    let server = start(dir.path(), &[]);
    let cookie = server.set_up().await;
    // A form or fetch from another origin.
    for origin in [Some("http://evil.example"), Some("null"), None, Some(&format!("http://127.0.0.1:{}", server.port + 1)[..])] {
        let mut r = reqwest::Client::new().post(server.url("/api/login")).json(&json!({ "password": PASSWORD }));
        if let Some(o) = origin {
            r = r.header(ORIGIN, o);
        }
        assert_eq!(r.send().await.unwrap().status(), StatusCode::FORBIDDEN, "{origin:?}");
    }
    // Cross-site WebSocket hijacking: even with the cookie, another origin is
    // refused; so is this host under another scheme (a page served over https
    // by someone else on this name).
    let other_scheme = format!("https://127.0.0.1:{}", server.port);
    for origin in [Some("http://evil.example"), None, Some(other_scheme.as_str())] {
        match server.socket(Some(&cookie), origin).await {
            Err(tungstenite::Error::Http(r)) => assert_eq!(r.status(), 403, "{origin:?}"),
            other => panic!("{origin:?} opened the socket: {:?}", other.map(|_| ())),
        }
    }
    // The page itself opens it.
    assert!(server.socket(Some(&cookie), Some(&server.origin())).await.is_ok());
}

#[tokio::test]
async fn nothing_opens_without_a_session_and_no_secret_travels_in_a_url() {
    let dir = folder();
    let server = start(dir.path(), &[]);
    let cookie = server.set_up().await;
    let token = cookie.split_once('=').unwrap().1.to_owned();
    // No cookie: no socket, no pictures.
    match server.socket(None, Some(&server.origin())).await {
        Err(tungstenite::Error::Http(r)) => assert_eq!(r.status(), 401),
        other => panic!("opened without a session: {:?}", other.map(|_| ())),
    }
    let r = reqwest::get(server.url("/icons/apps/x.png")).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // The token in a URL is no key.
    let mut request = format!("ws://127.0.0.1:{}/api/ws?token={token}&session={token}", server.port)
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(ORIGIN, server.origin().parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_err());
    // Nor is a password: signing in is a POST with a body; a GET is refused.
    let r = reqwest::get(server.url(&format!("/api/login?password={}", PASSWORD.replace(' ', "+")))).await.unwrap();
    assert_ne!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn wrong_passwords_are_slowed_down_even_before_the_right_one() {
    let dir = folder();
    let server = start(dir.path(), &[]);
    server.set_up().await;
    let mut limited = false;
    for _ in 0..8 {
        let r = server.post("/api/login", json!({ "password": "wrong guess" }), None).await;
        if r.status() == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
    assert!(limited, "the sixth try waits");
    // While it waits, the right password waits too (a guess is not tested).
    let r = server.post("/api/login", json!({ "password": PASSWORD }), None).await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_subscription_link_and_the_controller_secret_never_reach_a_page() {
    // A provider: the subscription for Kairo; for the core, which fetches the
    // config's providers itself, the connection just closes, and its log
    // line quotes the link it tried ("Get \"<link>\": EOF").
    const SUB_TOKEN: &str = "a8f3c2e7d91b04f6c5e2";
    const PROVIDER_TOKEN: &str = "f00dfeed5eed1234abcd";
    let provider = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let (port, _) = http_server({
        let provider = provider.clone();
        move |path, request| {
            let to_kairo = request.to_ascii_lowercase().contains("user-agent: kairo/");
            if path.starts_with("/api/v1/client/subscribe") && to_kairo {
                let config = provider.lock().unwrap().clone();
                Some(ok("text/yaml", config.into_bytes()))
            } else {
                None
            }
        }
    });
    let link = format!("http://127.0.0.1:{port}/api/v1/client/subscribe?token={SUB_TOKEN}");
    let provider_link = format!("http://127.0.0.1:{port}/providers/extra?token={PROVIDER_TOKEN}");
    *provider.lock().unwrap() = format!(
        "proxies:\n  - {{name: a, type: direct}}\nproxy-providers:\n  same:\n    type: http\n    url: '{link}'\n    path: ./p/same.yaml\n    interval: 3600\n  extra:\n    type: http\n    url: '{provider_link}'\n    path: ./p/extra.yaml\n    interval: 3600\nproxy-groups:\n  - {{name: G, type: select, proxies: [a], use: [same, extra]}}\nrules:\n  - MATCH,G\n"
    );
    let dir = folder_with_core();
    let server = start(dir.path(), &[]);
    let cookie = server.set_up().await;
    let other = server.sign_in().await;
    let mut ws = server.socket(Some(&cookie), Some(&server.origin())).await.unwrap();
    let mut watcher = server.socket(Some(&other), Some(&server.origin())).await.unwrap();
    send(&mut ws, 1, "add_remote", json!({ "url": link, "name": null })).await;
    send(&mut ws, 2, "connect", json!({ "on": true })).await;
    let (mut mine, _) = frames(&mut ws, Duration::from_secs(8)).await;
    // A link a page sent that never became a subscription.
    let never = format!("http://127.0.0.1:9/api/v1/client/subscribe?token={SUB_TOKEN}9");
    send(&mut ws, 3, "add_remote", json!({ "url": never, "name": null })).await;
    send(&mut ws, 4, "update_provider", json!({ "name": "extra" })).await;
    let (more, _) = frames(&mut ws, Duration::from_secs(4)).await;
    mine.extend(more);
    let (theirs, _) = frames(&mut watcher, Duration::from_millis(500)).await;
    let secret: Value = serde_json::from_str(&std::fs::read_to_string(server.home.join("settings.json")).unwrap()).unwrap();
    let secret = secret["secret"].as_str().unwrap().to_owned();
    assert!(secret.len() >= 16);
    let all: Vec<&String> = mine.iter().chain(theirs.iter()).collect();
    let reply = |id: u64| {
        all.iter()
            .filter_map(|t| serde_json::from_str::<Value>(t).ok())
            .find(|v| v["t"] == "reply" && v["v"]["id"] == id)
            .map(|v| v["v"].to_string())
    };
    assert!(reply(1).is_some_and(|r| r.contains("\"ok\"")), "the subscription was added: {:?}", reply(1));
    // The frames really carried what is redacted: the core's lines about
    // both providers, and the provider update's error.
    let logs: Vec<&&String> = all.iter().filter(|t| t.starts_with("{\"t\":\"logs\"")).collect();
    assert!(logs.iter().any(|t| t.contains("same")), "no log line about the provider that is the subscription");
    assert!(logs.iter().any(|t| t.contains("extra")), "no log line about the other provider");
    assert!(reply(4).is_some_and(|r| r.contains("\"err\"")), "the provider update failed: {:?}", reply(4));
    for t in all {
        for leak in [SUB_TOKEN, PROVIDER_TOKEN, "/api/v1/client/subscribe", "/providers/extra", secret.as_str()] {
            assert!(!t.contains(leak), "{leak} went out in {t}");
        }
    }
}

// --- Helpers for the tests below ------------------------------------------------------------

/// The core the tests run (`KAIRO_TEST_CORE`, as the core's own e2e tests).
fn core() -> String {
    std::env::var("KAIRO_TEST_CORE").expect("KAIRO_TEST_CORE: the mihomo core (scripts/fetch-mihomo.sh)")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// [`folder`], with Kairo's settings on the test core and free ports.
fn folder_with_core() -> tempfile::TempDir {
    let dir = folder();
    let settings = json!({
        "corePath": core(),
        "mixedPort": free_port(),
        "controllerPort": free_port(),
        "testTimeoutMs": 2000,
    });
    std::fs::write(dir.path().join("home/settings.json"), settings.to_string()).unwrap();
    dir
}

/// An HTTP answer.
fn ok(kind: &str, body: Vec<u8>) -> Vec<u8> {
    let mut r = format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
        .into_bytes();
    r.extend(body);
    r
}

/// A server on 127.0.0.1: `answer(path, request)` is the whole answer, or
/// `None` to close without one. Its port, and how many requests it had.
fn http_server(
    answer: impl Fn(&str, &str) -> Option<Vec<u8>> + Send + Sync + 'static,
) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (count, answer) = (hits.clone(), std::sync::Arc::new(answer));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let answer = answer.clone();
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                if let Some(bytes) = answer(&path, &request) {
                    let _ = stream.write_all(&bytes);
                }
            });
        }
    });
    (port, hits)
}

/// Sends a command and waits for its reply (other frames are skipped).
async fn call(ws: &mut Ws, id: u64, cmd: &str, args: Value) -> Value {
    send(ws, id, cmd, args).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["t"] == "reply" && v["v"]["id"] == id {
                    return v["v"].clone();
                }
            }
            Ok(Some(Ok(_))) => {}
            other => panic!("no reply to {cmd}: {other:?}"),
        }
    }
}

/// The first state a socket gets.
async fn first_state(ws: &mut Ws) -> Value {
    let (first, _) = frames(ws, Duration::from_secs(2)).await;
    first
        .iter()
        .filter_map(|t| serde_json::from_str::<Value>(t).ok())
        .find(|v| v["t"] == "state")
        .expect("the state")["v"]
        .clone()
}

/// Every `name=value` the response sets.
fn cookies_of(r: &reqwest::Response) -> Vec<String> {
    r.headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_page_names_only_subscriptions_that_exist() {
    let dir = folder();
    let server = start(dir.path(), &[]);
    let cookie = server.set_up().await;
    let mut ws = server.socket(Some(&cookie), Some(&server.origin())).await.unwrap();
    // A name of a file elsewhere, and an id of Kairo's form that is no subscription.
    let outside = dir.path().join("outside");
    let escape = format!("../../{}", outside.display());
    let mut id = 10;
    for target in [escape.as_str(), "../settings", "0123456789ab"] {
        for (cmd, args) in [
            ("activate", json!({ "id": target })),
            ("delete_profile", json!({ "id": target })),
            ("update_profile", json!({ "id": target })),
            ("rename_profile", json!({ "id": target, "name": "x", "interval_hours": null, "url": null })),
            ("dismiss_failure", json!({ "id": target })),
        ] {
            id += 1;
            let r = call(&mut ws, id, cmd, args).await;
            assert_eq!(r["err"], json!("There is no such subscription"), "{cmd} {target}: {r}");
        }
    }
    assert!(!outside.exists() && !dir.path().join("outside.yaml").exists());
    assert!(server.home.join("settings.json").is_file(), "nothing was deleted");
}

#[tokio::test]
async fn tries_at_once_count_and_the_owners_browser_is_never_kept_out() {
    let dir = folder();
    let server = start(dir.path(), &[]);
    // The owner's browser signed in before: it holds its device cookie.
    let code = server.setup_code.clone().unwrap();
    let r = server.post("/api/setup", json!({ "code": code, "password": PASSWORD }), None).await;
    let device = cookies_of(&r).into_iter().find(|c| c.starts_with("kairo_device=")).expect("a device cookie");
    // Twenty wrong tries at once from one address: no more than five are tried.
    let tries = (0..20).map(|_| async {
        reqwest::Client::new()
            .post(server.url("/api/login"))
            .header(ORIGIN, server.origin())
            .json(&json!({ "password": "wrong guess" }))
            .send()
            .await
            .unwrap()
            .status()
    });
    let statuses = futures_util::future::join_all(tries).await;
    let tried = statuses.iter().filter(|s| **s == StatusCode::UNAUTHORIZED).count();
    let held = statuses.iter().filter(|s| **s == StatusCode::TOO_MANY_REQUESTS).count();
    assert_eq!((tried, held), (5, 15), "{statuses:?}");
    // That address waits, the right password too...
    let r = server.post("/api/login", json!({ "password": PASSWORD }), None).await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    // ...but not the owner's browser, from the same address.
    let r = server.post("/api/login", json!({ "password": PASSWORD }), Some(&device)).await;
    assert_eq!(r.status(), StatusCode::OK, "the owner was kept out");
}

#[tokio::test]
async fn a_page_can_not_read_or_map_this_machines_services() {
    let (port, _) = http_server(|path, _| match path {
        "/page.html" => Some(ok("text/html", b"<html><body>Internal dashboard: LOOPBACK-HTML-SECRET-777</body></html>".to_vec())),
        "/notes.txt" => Some(ok("text/plain", b"LOOPBACK-TEXT-SECRET-888 is the key".to_vec())),
        "/redirect" => Some(b"HTTP/1.1 302 Found\r\nLocation: /page.html\r\nContent-Length: 0\r\n\r\n".to_vec()),
        "/away" => Some(b"HTTP/1.1 302 Found\r\nLocation: file:///etc/passwd\r\nContent-Length: 0\r\n\r\n".to_vec()),
        "/nothing" => Some(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nnot found".to_vec()),
        "/teapot" => Some(b"HTTP/1.1 418 I'm a teapot\r\nContent-Length: 0\r\n\r\n".to_vec()),
        _ => None,
    });
    let closed = free_port();
    let dir = folder();
    let server = start(dir.path(), &[]);
    let cookie = server.set_up().await;
    let mut ws = server.socket(Some(&cookie), Some(&server.origin())).await.unwrap();
    let mut errors = Vec::new();
    for (i, url) in [
        format!("http://127.0.0.1:{port}/page.html"),
        format!("http://127.0.0.1:{port}/notes.txt"),
        format!("http://localhost:{port}/redirect"),
        format!("http://127.0.0.1:{port}/away"),
        format!("http://127.0.0.1:{port}/nothing"),
        format!("http://127.0.0.1:{port}/teapot"),
        format!("http://127.0.0.1:{port}/eof"),
        format!("http://127.0.0.1:{closed}/"),
        format!("http://[::ffff:127.0.0.1]:{port}/page.html"),
    ]
    .into_iter()
    .enumerate()
    {
        let r = call(&mut ws, 100 + i as u64, "add_remote", json!({ "url": url, "name": null })).await;
        let err = r["err"].as_str().unwrap_or_else(|| panic!("{url} was added: {r}")).to_owned();
        for leak in ["SECRET", "Internal dashboard", "404", "418", "teapot", "not found", "connect"] {
            assert!(!err.contains(leak), "{url}: {err}");
        }
        errors.push(err);
    }
    errors.dedup();
    assert_eq!(errors.len(), 1, "one answer, whatever was there: {errors:?}");
}

#[tokio::test]
async fn a_page_can_not_point_the_delay_test_at_this_machine() {
    let (port, hits) = http_server(|_, _| Some(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec()));
    let dir = folder_with_core();
    let server = start(dir.path(), &[]);
    let cookie = server.set_up().await;
    let mut ws = server.socket(Some(&cookie), Some(&server.origin())).await.unwrap();
    let state = first_state(&mut ws).await;
    let config = "proxies:\n  - {name: a, type: direct}\nproxy-groups:\n  - {name: G, type: select, proxies: [a]}\nrules:\n  - MATCH,G\n";
    let r = call(&mut ws, 1, "add_yaml", json!({ "name": "t", "text": config })).await;
    assert!(r.get("ok").is_some(), "{r}");
    let r = call(&mut ws, 2, "connect", json!({ "on": true })).await;
    assert!(r.get("ok").is_some(), "{r}");
    // A page's own address for the test is not used...
    let local = format!("http://127.0.0.1:{port}/generate_204");
    call(&mut ws, 3, "test_node", json!({ "name": "a", "url": local })).await;
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "the core probed the page's address");
    // ...nor can it be saved as the test address.
    for url in [local.as_str(), "http://localhost/x", "http://10.0.0.1/", "http://[::1]/", "file:///etc/passwd", "http://169.254.169.254/latest/meta-data/"] {
        let mut settings = state["settings"].clone();
        settings["testUrl"] = json!(url);
        let r = call(&mut ws, 4, "update_settings", json!({ "settings": settings })).await;
        assert!(r.get("err").is_some(), "{url} was taken: {r}");
    }
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(server.home.join("settings.json")).unwrap()).unwrap();
    assert_ne!(saved["testUrl"], json!(local));
    call(&mut ws, 5, "test_node", json!({ "name": "a", "url": null })).await;
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_page_can_not_choose_the_controller_secret() {
    let dir = folder();
    let server = start(dir.path(), &[]);
    let cookie = server.set_up().await;
    let mut ws = server.socket(Some(&cookie), Some(&server.origin())).await.unwrap();
    let state = first_state(&mut ws).await;
    let before: Value = serde_json::from_str(&std::fs::read_to_string(server.home.join("settings.json")).unwrap()).unwrap();
    let mut settings = state["settings"].clone();
    settings["secret"] = json!("PAGE-CHOSEN-SECRET-1");
    settings["mixedPort"] = json!(free_port());
    let r = call(&mut ws, 1, "update_settings", json!({ "settings": settings })).await;
    assert!(r.get("ok").is_some(), "{r}");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(server.home.join("settings.json")).unwrap()).unwrap();
    assert_ne!(saved["secret"], json!("PAGE-CHOSEN-SECRET-1"));
    assert_eq!(saved["secret"], before["secret"]);
    // A new one is the server's to make.
    let r = call(&mut ws, 2, "new_secret", Value::Null).await;
    assert!(r.get("ok").is_some(), "{r}");
    let after: Value = serde_json::from_str(&std::fs::read_to_string(server.home.join("settings.json")).unwrap()).unwrap();
    assert_ne!(after["secret"], before["secret"]);
}

#[tokio::test]
async fn an_address_that_can_not_be_had_yet_stops_nothing() {
    // One address this machine does not have (bound anyway on Linux) and one
    // port another program holds: the server, its other address and the core
    // carry on, and it takes the held port once it is free.
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let held_port = held.local_addr().unwrap().port();
    let not_here = format!("192.0.2.77:{}", free_port());
    let dir = folder();
    let server = start(dir.path(), &["--listen", &format!("127.0.0.1:{held_port}"), "--listen", &not_here]);
    assert_eq!(server.session_state(None).await, "setup");
    drop(held);
    let mut reached = false;
    for _ in 0..30 {
        if let Ok(r) = reqwest::get(format!("http://127.0.0.1:{held_port}/api/session")).await
            && r.status() == StatusCode::OK
        {
            reached = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(reached, "the held port was taken once free");
    assert!(server_alive(&server), "the server runs");
    // On Linux the address it does not have is bound already (IP_FREEBIND).
    let said = server.output.lock().unwrap().join("\n");
    assert!(said.contains(&format!("Kairo is at http://{not_here}")), "{said}");
}

fn server_alive(server: &Server) -> bool {
    std::os::unix::net::UnixStream::connect(server.home.join("web/control.sock")).is_ok()
}

#[tokio::test]
async fn status_json_says_what_runs_and_whether_tun_is_really_up() {
    let dir = folder();
    let home = dir.path().join("home");
    let mut settings: Value = json!({});
    settings["mixedPort"] = json!(free_port());
    settings["controllerPort"] = json!(free_port());
    std::fs::write(home.join("settings.json"), settings.to_string()).unwrap();
    let status = || {
        let out = Command::new(env!("CARGO_BIN_EXE_kairo-server"))
            .args(["status", "--json"])
            .env("KAIRO_HOME", &home)
            .output()
            .unwrap();
        serde_json::from_slice::<Value>(&out.stdout).expect("JSON")
    };
    let off = status();
    assert_eq!(off["running"], json!(false));
    let server = start(dir.path(), &[]);
    let on = status();
    assert_eq!(on["running"], json!(true));
    assert_eq!(on["setup_code"].as_str(), server.setup_code.as_deref());
    // No core runs: TUN is not up, whatever the setting says (the TUN e2e
    // test sees it up, and the setting on with the core stopped).
    assert!(on["every_app_setting"].is_boolean());
    assert_eq!(on["every_app"], json!(false));
}

#[tokio::test]
async fn a_page_can_not_pick_the_program_the_server_runs() {
    let dir = folder();
    let server = start(dir.path(), &[]);
    let cookie = server.set_up().await;
    let mut ws = server.socket(Some(&cookie), Some(&server.origin())).await.unwrap();
    let (first, _) = frames(&mut ws, Duration::from_secs(2)).await;
    let state: Value = first
        .iter()
        .filter_map(|t| serde_json::from_str::<Value>(t).ok())
        .find(|v| v["t"] == "state")
        .expect("the state")["v"]
        .clone();
    let mut settings = state["settings"].clone();
    settings["corePath"] = json!("/bin/sh");
    settings["launchAtLogin"] = json!(true);
    send(&mut ws, 2, "update_settings", json!({ "settings": settings })).await;
    frames(&mut ws, Duration::from_secs(2)).await;
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(server.home.join("settings.json")).unwrap()).unwrap();
    assert_ne!(saved["corePath"], json!("/bin/sh"));
    assert_eq!(saved["launchAtLogin"], json!(false));
}

#[tokio::test]
async fn a_revoked_browser_is_signed_out_at_once_and_a_restart_keeps_the_rest() {
    let dir = folder();
    let mut server = start(dir.path(), &[]);
    let mine = server.set_up().await;
    let theirs = server.sign_in().await;
    let mut ws = server.socket(Some(&mine), Some(&server.origin())).await.unwrap();
    let mut other = server.socket(Some(&theirs), Some(&server.origin())).await.unwrap();
    frames(&mut other, Duration::from_millis(500)).await;
    send(&mut ws, 1, "revoke_other_sessions", Value::Null).await;
    let (texts, closed) = frames(&mut other, Duration::from_secs(5)).await;
    assert_eq!(closed, Some(4401), "closed as signed out");
    assert!(texts.iter().any(|t| t.contains("signed_out")));
    assert_eq!(server.session_state(Some(&theirs)).await, "sign_in");
    assert_eq!(server.session_state(Some(&mine)).await, "signed_in");
    drop(ws);

    // A restart: the session stays, the revoked one stays revoked.
    unsafe { libc::kill(server.child.id() as i32, libc::SIGTERM) };
    let _ = server.child.wait();
    std::mem::forget(server);
    let server = start(dir.path(), &[]);
    assert_eq!(server.session_state(Some(&mine)).await, "signed_in");
    assert_eq!(server.session_state(Some(&theirs)).await, "sign_in");
    assert!(server.socket(Some(&mine), Some(&server.origin())).await.is_ok());
}

#[tokio::test]
async fn the_password_and_sessions_are_its_owners_alone() {
    use std::os::unix::fs::PermissionsExt;
    let dir = folder();
    let server = start(dir.path(), &[]);
    server.set_up().await;
    let web = server.home.join("web");
    assert_eq!(std::fs::metadata(&web).unwrap().permissions().mode() & 0o777, 0o700);
    for f in ["password", "sessions.json"] {
        let mode = std::fs::metadata(web.join(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{f}");
    }
    // Sessions are kept as hashes: the cookie's token is not in the file.
    let cookie = server.sign_in().await;
    let token = cookie.split_once('=').unwrap().1;
    let kept = std::fs::read_to_string(web.join("sessions.json")).unwrap();
    assert!(!kept.contains(token));
    let sock = std::fs::metadata(web.join("control.sock")).unwrap().permissions().mode() & 0o777;
    assert_eq!(sock, 0o600);
}

#[tokio::test]
async fn a_second_server_on_the_same_folder_will_not_start() {
    let dir = folder();
    let _server = start(dir.path(), &[]);
    let out = Command::new(env!("CARGO_BIN_EXE_kairo-server"))
        .args(["serve", "--listen", "127.0.0.1:0", "--web"])
        .arg(dir.path().join("page"))
        .env("KAIRO_HOME", dir.path().join("home"))
        .env("KAIRO_NO_SYSPROXY", "1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("another Kairo uses"));
}

#[tokio::test]
async fn the_page_is_sent_with_its_protections() {
    let dir = folder();
    let server = start(dir.path(), &[]);
    let r = reqwest::get(server.url("/")).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let h = r.headers();
    let csp = h.get("content-security-policy").unwrap().to_str().unwrap();
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    // Scripts: the page's own files and its inline scripts by hash, nothing else.
    let scripts = csp.split(';').map(str::trim).find(|d| d.starts_with("script-src")).unwrap();
    assert!(!scripts.contains("unsafe-inline") && !scripts.contains("'unsafe-eval'"), "{csp}");
    assert_eq!(h.get("x-frame-options").unwrap(), "DENY");
    assert_eq!(h.get("referrer-policy").unwrap(), "no-referrer");
    // Files outside the page's folder are not served: Kairo's settings sit
    // at ../home/settings.json from it. Sent as raw request lines (a client
    // library would tidy the dots away before they reach the server).
    assert!(server.home.join("settings.json").is_file());
    for path in [
        "/../home/settings.json",
        "/./../home/settings.json",
        "/%2e%2e/home/settings.json",
        "/..%2fhome%2fsettings.json",
        "//../home/settings.json",
    ] {
        let answer = raw_get(server.port, path).await;
        assert!(!answer.contains("mixedPort"), "{path} served Kairo's settings: {answer}");
        assert!(answer.starts_with("HTTP/1.1 404") || answer.starts_with("HTTP/1.1 400"), "{path}: {answer}");
    }
}

/// `GET <path>` exactly as written, over a plain socket: the whole answer.
async fn raw_get(port: u16, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    s.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut answer)).await;
    String::from_utf8_lossy(&answer).into_owned()
}

#[tokio::test]
async fn the_page_is_never_served_from_kairos_own_folder() {
    // A page folder inside Kairo's (or Kairo's inside it) would serve the
    // setup code, the password's hash and the sessions as files.
    let dir = folder();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join("web")).unwrap();
    std::fs::write(home.join("web/index.html"), "<!doctype html>").unwrap();
    for web in [home.join("web"), home.clone(), dir.path().to_path_buf()] {
        let child = Command::new(env!("CARGO_BIN_EXE_kairo-server"))
            .args(["serve", "--listen", "127.0.0.1:0", "--web"])
            .arg(&web)
            .env("KAIRO_HOME", &home)
            .env("KAIRO_NO_SYSPROXY", "1")
            .env("KAIRO_NO_UPDATE", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // A server that starts runs until stopped: that is a failure here, not a hang.
        let (status, stderr) = exit_within(child, Duration::from_secs(15))
            .unwrap_or_else(|| panic!("it started and served --web {}", web.display()));
        assert!(!status.success(), "started with --web {}", web.display());
        assert!(stderr.contains("overlap"), "{stderr}");
    }
    // A proper one: nothing of the folder is served by its name.
    let server = start(dir.path(), &[]);
    server.set_up().await;
    for path in ["/setup-code", "/password", "/sessions.json", "/web/password", "/settings.json", "/control.sock"] {
        let r = reqwest::get(server.url(path)).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
    }
}
