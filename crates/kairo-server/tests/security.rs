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
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("kairo-server runs");
    let out = BufReader::new(child.stdout.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in out.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut setup_code = None;
    loop {
        let line = rx.recv_timeout(Duration::from_secs(30)).expect("the server says where it is");
        if let Some(code) = line.strip_prefix("Setup code: ") {
            setup_code = Some(code.trim().to_owned());
        }
        if let Some(rest) = line.strip_prefix("Kairo is at http://") {
            let port = rest.rsplit(':').next().unwrap().trim().parse().unwrap();
            return Server { child, port, home, setup_code };
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
    // Cross-site WebSocket hijacking: even with the cookie, another origin is refused.
    for origin in [Some("http://evil.example"), None] {
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
    let dir = folder();
    let server = start(dir.path(), &[]);
    let cookie = server.set_up().await;
    let other = server.sign_in().await;
    let mut ws = server.socket(Some(&cookie), Some(&server.origin())).await.unwrap();
    let mut watcher = server.socket(Some(&other), Some(&server.origin())).await.unwrap();
    // Nothing listens there: the add fails and its error quotes the link.
    const TOKEN: &str = "a8f3c2e7d91b04f6c5e2";
    let link = format!("http://127.0.0.1:9/api/v1/client/subscribe?token={TOKEN}");
    send(&mut ws, 1, "add_remote", json!({ "url": link, "name": null })).await;
    let (mine, _) = frames(&mut ws, Duration::from_secs(4)).await;
    let (theirs, _) = frames(&mut watcher, Duration::from_millis(500)).await;
    let secret = std::fs::read_to_string(server.home.join("settings.json")).unwrap();
    let secret: Value = serde_json::from_str(&secret).unwrap();
    let secret = secret["secret"].as_str().unwrap().to_owned();
    assert!(secret.len() >= 16);
    let all: Vec<&String> = mine.iter().chain(theirs.iter()).collect();
    let reply = all.iter().find(|t| t.contains("\"id\":1")).expect("the add was answered");
    assert!(reply.contains("\"err\""), "the add failed: {reply}");
    for t in all {
        for leak in [TOKEN, "/api/v1/client/subscribe", secret.as_str()] {
            assert!(!t.contains(leak), "{leak} went out in {t}");
        }
    }
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
    // Files outside the page's folder are not served.
    for path in ["/../settings.json", "/%2e%2e/settings.json", "/..%2fsettings.json", "/web/password"] {
        let r = reqwest::get(server.url(path)).await.unwrap();
        let body = r.text().await.unwrap();
        assert!(!body.contains("mixedPort") && !body.contains("$argon2"), "{path}");
    }
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
        let out = Command::new(env!("CARGO_BIN_EXE_kairo-server"))
            .args(["serve", "--listen", "127.0.0.1:0", "--web"])
            .arg(&web)
            .env("KAIRO_HOME", &home)
            .env("KAIRO_NO_SYSPROXY", "1")
            .env("KAIRO_NO_UPDATE", "1")
            .output()
            .unwrap();
        assert!(!out.status.success(), "started with --web {}", web.display());
        assert!(String::from_utf8_lossy(&out.stderr).contains("overlap"));
    }
    // A proper one: nothing of the folder is served by its name.
    let server = start(dir.path(), &[]);
    server.set_up().await;
    for path in ["/setup-code", "/password", "/sessions.json", "/web/password", "/settings.json", "/control.sock"] {
        let r = reqwest::get(server.url(path)).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
    }
}
