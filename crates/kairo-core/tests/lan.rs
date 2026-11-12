//! "Allow other devices", against a real core in a network namespace: the
//! namespace's loopback carries two more addresses, 10.77.0.1 (this machine
//! as another device on the network reaches it) and 10.77.0.2 (a server out
//! on the network). A client bound to 10.77.0.1 is "another device".
//!
//! - On: another device proxies to the network as before, but never to this
//!   machine's loopback (127/8, 0.0.0.0, `localhost`, a name that resolves
//!   there), in every mode, through an inbound with a proxy of its own too;
//!   a program on this machine still reaches its loopback.
//! - Off: no inbound of the config answers another device (`tunnels`, the
//!   old `ss-config`/`vmess-config`, `listeners`, `dns.listen`), and a
//!   `listeners` entry of type tun never reaches the core.
//!
//! Run by `scripts/tun-e2e.sh`:
//!   KAIRO_TEST_NETNS=1 unshare -rn sh -c 'ip link set lo up; ip addr add 10.77.0.1/32 dev lo; ip addr add 10.77.0.2/32 dev lo; exec target/debug/deps/lan-<hash>'

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use kairo_core::settings::{Mode, Settings};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const LAN: &str = "10.77.0.1";
const OUT_THERE: &str = "10.77.0.2";

fn free_port(ip: &str) -> u16 {
    std::net::TcpListener::bind((ip, 0)).unwrap().local_addr().unwrap().port()
}

/// Answers every request with `body`.
async fn serve(listener: TcpListener, body: &'static str) {
    loop {
        let Ok((mut s, _)) = listener.accept().await else { return };
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let _ = s.read(&mut buf).await;
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(head.as_bytes()).await;
            let _ = s.write_all(body.as_bytes()).await;
        });
    }
}

struct Core(Child);

impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The core on `config` (Kairo's built text), once its mixed port answers.
async fn run_core(dir: &Path, config: &str, mixed: u16) -> Core {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    let core = std::env::var_os("KAIRO_TEST_CORE").expect("KAIRO_TEST_CORE (no network in here)");
    let log = std::fs::File::create(dir.join("core.log")).unwrap();
    let child = Command::new(core)
        .arg("-d")
        .arg(dir)
        .arg("-f")
        .arg(dir.join("config.yaml"))
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let core = Core(child);
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", mixed)).await.is_ok() {
            return core;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the core did not start: {}", std::fs::read_to_string(dir.join("core.log")).unwrap_or_default());
}

/// GET `url` through the HTTP proxy at `proxy`, from `from`: the body if 200.
async fn through(from: &str, proxy: SocketAddr, url: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://{proxy}")).unwrap())
        .local_address(from.parse::<IpAddr>().unwrap())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let r = client.get(url).send().await.ok()?;
    if r.status() != reqwest::StatusCode::OK {
        return None;
    }
    r.text().await.ok()
}

fn settings(allow_lan: bool, mode: Mode, mixed: u16, controller: u16) -> Settings {
    Settings {
        allow_lan,
        mode,
        mixed_port: mixed,
        controller_port: controller,
        ..Settings::default()
    }
}

const PROFILE: &str = "proxies:\n  - {name: a, type: direct}\nproxy-groups:\n  - {name: G, type: select, proxies: [a, DIRECT]}\nhosts:\n  loop.test: 127.0.0.1\nrules:\n  - MATCH,G\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_devices_never_reach_this_machines_loopback() {
    if std::env::var_os("KAIRO_TEST_NETNS").is_none() {
        eprintln!("skipped: set KAIRO_TEST_NETNS=1 and run under `unshare -rn` (scripts/tun-e2e.sh)");
        return;
    }
    let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let secret_port = local.local_addr().unwrap().port();
    tokio::spawn(serve(local, "LOOPBACK-SECRET"));
    let public = TcpListener::bind((OUT_THERE, 0)).await.expect("10.77.0.2 on lo (see the header)");
    let public_url = format!("http://{}/", public.local_addr().unwrap());
    tokio::spawn(serve(public, "PUBLIC-OK"));
    let dir = tempfile::tempdir().unwrap();

    let refused_targets = [
        format!("http://127.0.0.1:{secret_port}/"),
        format!("http://127.0.0.2:{secret_port}/"),
        format!("http://0.0.0.0:{secret_port}/"),
        format!("http://localhost:{secret_port}/"),
        format!("http://loop.test:{secret_port}/"),
    ];
    let listener_port = free_port(LAN);
    let sub_rules_port = free_port(LAN);
    let tunnel_port = free_port(LAN);
    // Inbounds that skip the main rules: a listener with a proxy of its own,
    // one with rules of its own (`rule:` names a list of `sub-rules`), a port
    // tunnel with an exit of its own.
    let over = format!(
        "listeners:\n  - {{name: lan-direct, type: http, port: {listener_port}, listen: 0.0.0.0, proxy: DIRECT}}\n  - {{name: lan-own, type: http, port: {sub_rules_port}, listen: 0.0.0.0, rule: own}}\nsub-rules:\n  own:\n    - MATCH,DIRECT\ntunnels:\n  - tcp,0.0.0.0:{tunnel_port},127.0.0.1:{secret_port},DIRECT\n"
    );
    for (i, mode) in [Mode::Rule, Mode::Direct, Mode::Global].into_iter().enumerate() {
        let (mixed, controller) = (free_port(LAN), free_port("127.0.0.1"));
        let s = settings(true, mode, mixed, controller);
        let text = kairo_core::config::build(PROFILE, &over, &s).unwrap();
        let _core = run_core(&dir.path().join(format!("on-{i}")), &text, mixed).await;
        let from_lan: SocketAddr = format!("{LAN}:{mixed}").parse().unwrap();
        let via_listener: SocketAddr = format!("{LAN}:{listener_port}").parse().unwrap();
        let via_sub_rules: SocketAddr = format!("{LAN}:{sub_rules_port}").parse().unwrap();
        for target in &refused_targets {
            let got = through(LAN, from_lan, target).await;
            assert!(got.is_none(), "{mode:?}: another device reached {target}: {got:?}");
            let got = through(LAN, via_listener, target).await;
            assert!(got.is_none(), "{mode:?}: another device reached {target} through a listener with its own proxy: {got:?}");
            let got = through(LAN, via_sub_rules, target).await;
            assert!(got.is_none(), "{mode:?}: another device reached {target} through a listener with its own rules: {got:?}");
        }
        assert_eq!(through(LAN, via_sub_rules, &public_url).await.as_deref(), Some("PUBLIC-OK"));
        assert_eq!(
            through(LAN, from_lan, &public_url).await.as_deref(),
            Some("PUBLIC-OK"),
            "{mode:?}: another device proxies to the network as before"
        );
        assert_eq!(through(LAN, via_listener, &public_url).await.as_deref(), Some("PUBLIC-OK"));
        // A program on this machine: its own loopback, as always.
        let mine: SocketAddr = format!("127.0.0.1:{mixed}").parse().unwrap();
        assert_eq!(
            through("127.0.0.1", mine, &format!("http://127.0.0.1:{secret_port}/")).await.as_deref(),
            Some("LOOPBACK-SECRET"),
            "{mode:?}: a local program reaches its loopback"
        );
        assert_eq!(through("127.0.0.1", mine, &public_url).await.as_deref(), Some("PUBLIC-OK"));
        // A tunnel straight to the loopback service answers this machine only.
        assert!(
            tokio::net::TcpStream::connect((LAN, tunnel_port)).await.is_err(),
            "{mode:?}: the port tunnel answered another device"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_other_devices_off_no_inbound_answers_them() {
    if std::env::var_os("KAIRO_TEST_NETNS").is_none() {
        eprintln!("skipped: set KAIRO_TEST_NETNS=1 and run under `unshare -rn` (scripts/tun-e2e.sh)");
        return;
    }
    let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let secret_port = local.local_addr().unwrap().port();
    tokio::spawn(serve(local, "LOOPBACK-SECRET"));
    let ports: Vec<u16> = (0..6).map(|_| free_port(LAN)).collect();
    let [ss, vmess, t1, t2, l1, dns] = ports[..] else { unreachable!() };
    // The provider's and the override's inbounds, every kind that listens.
    let profile = format!(
        "{PROFILE}ss-config: ss://aes-128-gcm:pass@:{ss}\nvmess-config: vmess://b831381d-6324-4d53-ad4f-8cda48b30811:0@:{vmess}\ndns:\n  enable: true\n  listen: 0.0.0.0:{dns}\n  nameserver: [system]\nlisteners:\n  - {{name: l1, type: mixed, port: {l1}}}\n  - {{name: t, type: tun, stack: gvisor, inet4-address: [198.19.0.1/30]}}\n"
    );
    let over = format!(
        "tunnels:\n  - tcp,0.0.0.0:{t1},127.0.0.1:{secret_port},DIRECT\n  - {{network: [tcp], address: '0.0.0.0:{t2}', target: '127.0.0.1:{secret_port}', proxy: DIRECT}}\n"
    );
    let (mixed, controller) = (free_port("127.0.0.1"), free_port("127.0.0.1"));
    let s = settings(false, Mode::Rule, mixed, controller);
    let text = kairo_core::config::build(&profile, &over, &s).unwrap();
    assert!(!text.contains("type: tun"), "a tun listener never reaches the core:\n{text}");
    let dir = tempfile::tempdir().unwrap();
    let _core = run_core(dir.path(), &text, mixed).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    for (what, port) in [("ss-config", ss), ("vmess-config", vmess), ("tunnel", t1), ("tunnel", t2), ("listener", l1), ("dns.listen", dns)] {
        assert!(
            tokio::net::TcpStream::connect((LAN, port)).await.is_err(),
            "the {what} on {port} answered another device"
        );
    }
    // Still there for this machine.
    for port in [t1, t2, l1] {
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok(), "{port} on this machine");
    }
    // This namespace's links (/sys is the machine's: not remounted here).
    let links: Vec<String> = std::fs::read_to_string("/proc/self/net/dev")
        .unwrap()
        .lines()
        .skip(2)
        .filter_map(|l| l.split(':').next().map(|n| n.trim().to_owned()))
        .collect();
    assert_eq!(links, ["lo"], "no tun device came up");
}

/// Answers by path: a redirect, a body of 17 MB (with and without its
/// length said), the page of a local service.
async fn serve_paths(listener: TcpListener, local_page: String) {
    loop {
        let Ok((mut s, _)) = listener.accept().await else { return };
        let local_page = local_page.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_owned();
            const BIG: usize = 17 * 1024 * 1024;
            match path.as_str() {
                "/go" => {
                    let head = format!("HTTP/1.1 302 Found\r\nLocation: {local_page}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    let _ = s.write_all(head.as_bytes()).await;
                }
                "/big" | "/big-said" => {
                    let head = if path == "/big-said" {
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {BIG}\r\nConnection: close\r\n\r\n")
                    } else {
                        "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_owned()
                    };
                    let _ = s.write_all(head.as_bytes()).await;
                    let line = b"# filler line of a very large subscription\n";
                    let mut sent = 0;
                    while sent < BIG {
                        if s.write_all(line).await.is_err() {
                            return;
                        }
                        sent += line.len();
                    }
                }
                _ => {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                }
            }
        });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscription_that_leads_to_this_machine_tells_nothing_of_it() {
    use kairo_core::profile::{ProfileError, download};
    if std::env::var_os("KAIRO_TEST_NETNS").is_none() {
        eprintln!("skipped: set KAIRO_TEST_NETNS=1 and run under `unshare -rn` (scripts/tun-e2e.sh)");
        return;
    }
    let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_page = format!("http://127.0.0.1:{}/page.html", local.local_addr().unwrap().port());
    tokio::spawn(serve(local, "<html><body>Internal dashboard: LOOPBACK-HTML-SECRET-777</body></html>"));
    // An address out on the internet, as Kairo sees it (203.0.113.5, a
    // documentation address, on this namespace's lo).
    let public = TcpListener::bind("203.0.113.5:0").await.expect("203.0.113.5 on lo (see tun-e2e.sh)");
    let base = format!("http://{}", public.local_addr().unwrap());
    tokio::spawn(serve_paths(public, local_page.clone()));
    // Straight there, or by a public address's redirect: one answer, nothing of the page.
    for url in [local_page.clone(), format!("{base}/go")] {
        let got = download(&url, None, None).await.and_then(|d| d.inspect().map(|_| d.body));
        assert_eq!(got, Err(ProfileError::Private), "{url}");
    }
    // At most 16 MB is read, said or not.
    for path in ["/big", "/big-said"] {
        match download(&format!("{base}{path}"), None, None).await {
            Err(ProfileError::Download(e)) => assert!(e.contains("16 MB"), "{path}: {e}"),
            Err(e) => panic!("{path}: {e}"),
            Ok(d) => panic!("{path}: {} bytes read", d.body.len()),
        }
    }
}
