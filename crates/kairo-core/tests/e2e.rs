//! End to end, against a real mihomo core: the test serves a subscription and
//! runs two local HTTP proxies and an origin server, then drives the service as
//! the app does — add the profile, start the core, pick a node, test delays,
//! send traffic through the proxy and see it in the connections and traffic
//! streams, switch modes, survive a core crash, reject a bad override.
//!
//! The core: `KAIRO_TEST_CORE=/path/to/mihomo`, else `target/mihomo/mihomo`
//! (`scripts/fetch-mihomo.sh`). It is given to the service as the bundled core,
//! as a packaged Kairo has it, and runs from the copy Kairo makes. Nothing here
//! touches the machine's system proxy (`KAIRO_NO_SYSPROXY`), and every port is
//! a free one on 127.0.0.1.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use kairo_core::paths::Paths;
use kairo_core::settings::{Mode, Settings};
use kairo_core::{CoreStatus, Service};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An origin: `/hello` answers "kairo-ok", `/slow` streams for three seconds,
/// `/long` for six (a stand-in for a model's answer streaming in),
/// `/generate_204` answers 204, `/sub` is the subscription.
async fn origin(listener: TcpListener, subscription: Arc<String>) {
    loop {
        let Ok((mut s, _)) = listener.accept().await else {
            return;
        };
        let sub = subscription.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_owned();
            if path.ends_with("/generate_204") {
                let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            } else if path.ends_with("/sub") {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/yaml\r\nContent-Length: {}\r\nsubscription-userinfo: upload=1048576; download=5242880; total=107374182400; expire=1893456000\r\nprofile-update-interval: 12\r\ncontent-disposition: attachment; filename*=UTF-8''Test%20Sub.yaml\r\nConnection: close\r\n\r\n",
                    sub.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(sub.as_bytes()).await;
            } else if path.ends_with("/long") {
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 60000\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                for _ in 0..60 {
                    if s.write_all(&[b'y'; 1000]).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            } else if path.ends_with("/slow") {
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 300000\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                for _ in 0..30 {
                    if s.write_all(&[b'x'; 10000]).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            } else {
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nkairo-ok").await;
            }
        });
    }
}

/// A minimal HTTP CONNECT proxy that counts the tunnels it opened.
async fn connect_proxy(listener: TcpListener, count: Arc<AtomicUsize>) {
    loop {
        let Ok((mut client, _)) = listener.accept().await else {
            return;
        };
        let count = count.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let mut read = 0;
            loop {
                let n = client.read(&mut buf[read..]).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                read += n;
                if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buf[..read]).to_string();
            let mut parts = head.split_whitespace();
            if parts.next() != Some("CONNECT") {
                let _ = client
                    .write_all(b"HTTP/1.1 405 Method Not Allowed\r\n\r\n")
                    .await;
                return;
            }
            let target = loopback_for_test_names(parts.next().unwrap_or(""));
            let Ok(mut upstream) = TcpStream::connect(&target).await else {
                let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                return;
            };
            count.fetch_add(1, Ordering::SeqCst);
            let _ = client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await;
            let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        });
    }
}

/// The nodes reach the documentation range 203.0.113.0/24 and `*.test` on
/// loopback, so traffic captured by TUN has somewhere to go.
fn loopback_for_test_names(target: &str) -> String {
    match target.rsplit_once(':') {
        Some((host, port))
            if host.starts_with("203.0.113.")
                || host.ends_with(".test")
                || host.ends_with("claude.ai") =>
        {
            format!("127.0.0.1:{port}")
        }
        _ => target.to_owned(),
    }
}

fn core_binary(target: &std::path::Path) -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("KAIRO_TEST_CORE") {
        return p.into();
    }
    let p = target.join("mihomo/mihomo");
    assert!(
        p.is_file(),
        "no core: run scripts/fetch-mihomo.sh or set KAIRO_TEST_CORE"
    );
    p
}

async fn wait_for<T>(what: &str, limit: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let start = std::time::Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(start.elapsed() < limit, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_core_end_to_end() {
    // SAFETY: set before any thread of this test reads the environment.
    unsafe {
        std::env::set_var("KAIRO_NO_SYSPROXY", "1");
        std::env::set_var("KAIRO_NO_UPDATE", "1");
    }
    let _ = env_logger_init();
    let home = tempfile::tempdir().unwrap();
    let target = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let core = core_binary(&target);

    // Local servers.
    let origin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr: SocketAddr = origin_listener.local_addr().unwrap();
    let (pa, pb) = (
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
    );
    let (pa_port, pb_port) = (
        pa.local_addr().unwrap().port(),
        pb.local_addr().unwrap().port(),
    );
    let (count_a, count_b) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    tokio::spawn(connect_proxy(pa, count_a.clone()));
    tokio::spawn(connect_proxy(pb, count_b.clone()));
    let subscription = format!(
        r#"
mixed-port: 7890
proxies:
  - {{name: node-a, type: http, server: 127.0.0.1, port: {pa_port}}}
  - {{name: node-b, type: http, server: 127.0.0.1, port: {pb_port}}}
proxy-groups:
  - {{name: Proxy, type: select, proxies: [node-a, node-b, DIRECT]}}
  - {{name: Auto, type: url-test, proxies: [node-a, node-b], url: "http://{origin_addr}/generate_204", interval: 300}}
  - {{name: Blocked, type: select, proxies: [REJECT, DIRECT]}}
rules:
  - DOMAIN,blocked.test,Blocked
  - DOMAIN-SUFFIX,direct.test,DIRECT
  - MATCH,Proxy
"#
    );
    tokio::spawn(origin(origin_listener, Arc::new(subscription)));

    // Settings: free ports, a local delay-test URL. The core is the bundled one.
    let mut paths = Paths::new(home.path());
    paths.bundled_core = Some(core.clone());
    paths.ensure().unwrap();
    let _core = CoreGuard(paths.core_pid());
    let mut settings = Settings::default();
    settings.mixed_port = free_port();
    settings.controller_port = free_port();
    settings.secret = "e2e-secret".into();
    settings.test_url = format!("http://{origin_addr}/generate_204");
    settings.test_timeout_ms = 3000;
    // This test is the core and the service without the AI layer (which sends
    // loopback traffic direct); the AI layer has a test of its own below.
    settings.ai.enabled = false;
    settings.save(&paths).unwrap();
    let mixed = settings.mixed_port;

    let svc = Service::start(paths.clone());
    let state = svc.state();
    wait_for("the core version", Duration::from_secs(15), || {
        state.borrow().core_version.clone()
    })
    .await;

    // 0. A first run: the bundled core is copied in and runs at once, with no
    // profile yet (everything direct). No install step.
    wait_for(
        "the core running with no profile",
        Duration::from_secs(20),
        || state.borrow().core.is_running().then_some(()),
    )
    .await;
    assert_eq!(svc.core_path(), paths.core_bin(), "it runs Kairo's copy");
    assert_eq!(
        std::fs::read(paths.core_bin()).unwrap(),
        std::fs::read(&core).unwrap(),
        "a copy of the bundled core"
    );
    assert!(
        std::fs::read_to_string(paths.runtime_config())
            .unwrap()
            .contains("MATCH,DIRECT")
    );

    // 1. Add the subscription; as the first profile it becomes active and the running core loads it.
    let id = svc
        .add_remote(format!("http://{origin_addr}/sub"), None)
        .await
        .expect("add the subscription");
    let s = svc.snapshot();
    let p = s.profiles.get(&id).expect("the profile");
    assert_eq!(p.name, "Test Sub");
    assert_eq!(s.profiles.active.as_deref(), Some(id.as_str()));
    let usage = p.usage.expect("usage from the header");
    assert_eq!(
        (usage.upload, usage.download, usage.total),
        (1048576, 5242880, 107374182400)
    );
    assert_eq!(p.stats.proxies, 2);
    assert!(matches!(
        p.source,
        kairo_core::profile::Source::Remote {
            interval_hours: 12,
            ..
        }
    ));
    wait_for("the core running", Duration::from_secs(20), || {
        state.borrow().core.is_running().then_some(())
    })
    .await;
    let config = std::fs::read_to_string(paths.runtime_config()).unwrap();
    assert!(
        config.contains(&format!("mixed-port: {mixed}")),
        "Kairo's port wins over the profile's"
    );
    assert!(!config.contains("mixed-port: 7890"));

    // 2. Groups, in config order, with their selection.
    let proxies = svc.proxies();
    wait_for("groups", Duration::from_secs(10), || {
        (!proxies.borrow().groups.is_empty()).then_some(())
    })
    .await;
    let names: Vec<String> = proxies
        .borrow()
        .groups
        .iter()
        .map(|g| g.name.clone())
        .collect();
    assert_eq!(names, ["Proxy", "Auto", "Blocked", "GLOBAL"]);
    assert_eq!(proxies.borrow().groups[0].now.as_deref(), Some("node-a"));

    // 3. Pick a node.
    svc.select("Proxy".into(), "node-b".into())
        .await
        .expect("select");
    assert_eq!(proxies.borrow().groups[0].now.as_deref(), Some("node-b"));

    // 4. Delay tests: one node, then the group.
    let delay = svc
        .test_node("node-a".into(), None)
        .await
        .expect("node delay");
    assert!(delay > 0);
    svc.test_group("Proxy".into()).await.expect("group delay");
    let group = proxies.borrow().groups[0].clone();
    assert!(
        group
            .members
            .iter()
            .filter(|m| m.name.starts_with("node-"))
            .all(|m| m.delay.unwrap_or(0) > 0),
        "{group:?}"
    );

    // 5. Traffic through the proxy, seen by the node, the connections and traffic streams.
    let before_b = count_b.load(Ordering::SeqCst);
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{mixed}")).unwrap())
        .build()
        .unwrap();
    let body = client
        .get(format!("http://{origin_addr}/hello"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "kairo-ok");
    assert!(
        count_b.load(Ordering::SeqCst) > before_b,
        "the request went through node-b"
    );
    let slow = {
        let client = client.clone();
        let url = format!("http://{origin_addr}/slow");
        tokio::spawn(async move {
            client
                .get(url)
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .len()
        })
    };
    let conns = svc.connections();
    let seen = wait_for(
        "the connection in the stream",
        Duration::from_secs(5),
        || {
            conns
                .borrow()
                .list
                .iter()
                .find(|c| c.port == origin_addr.port().to_string())
                .cloned()
        },
    )
    .await;
    assert_eq!(seen.outbound(), "node-b");
    assert!(seen.chains.contains(&"Proxy".to_string()));
    assert_eq!(seen.rule, "Match");
    let live = svc.live();
    wait_for("download speed", Duration::from_secs(5), || {
        (live.borrow().down > 0).then_some(())
    })
    .await;
    assert_eq!(slow.await.unwrap(), 300000);
    assert!(live.borrow().history.len() >= 2);

    // 5b. A long stream (a model's answer) goes on while the location
    // changes and the route mode changes back and forth: neither closes it.
    let long = {
        let client = client.clone();
        let url = format!("http://{origin_addr}/long");
        tokio::spawn(async move { client.get(url).send().await?.bytes().await.map(|b| b.len()) })
    };
    wait_for("the long stream", Duration::from_secs(5), || {
        conns
            .borrow()
            .list
            .iter()
            .any(|c| c.port == origin_addr.port().to_string() && c.outbound() == "node-b")
            .then_some(())
    })
    .await;
    svc.select("Proxy".into(), "node-a".into())
        .await
        .expect("select");
    svc.set_mode(Mode::Direct).await.expect("mode");
    svc.set_mode(Mode::Rule).await.expect("mode");
    svc.select("Proxy".into(), "node-b".into())
        .await
        .expect("select");
    assert!(
        !long.is_finished(),
        "the stream still runs during the switches"
    );
    assert_eq!(
        long.await.unwrap().expect("the stream was not cut"),
        60000,
        "the whole answer arrived"
    );

    // 6. Rules and logs.
    let rules = svc.rules().await.expect("rules");
    assert_eq!(rules.len(), 3);
    assert_eq!(rules[0].kind, "Domain");
    wait_for("log lines", Duration::from_secs(5), || {
        (!svc.logs().is_empty()).then_some(())
    })
    .await;

    // 6b. The DNS tool asks the core's resolver (the system's, as the profile has no `dns`).
    let answer = svc
        .dns_query("localhost".into(), "A".into())
        .await
        .expect("the DNS API is on");
    assert!((0..=5).contains(&answer.status), "a DNS status: {answer:?}");

    // 7. Modes: Direct skips the nodes.
    svc.set_mode(Mode::Direct).await.expect("mode");
    assert_eq!(svc.settings().mode, Mode::Direct);
    let before = count_b.load(Ordering::SeqCst) + count_a.load(Ordering::SeqCst);
    let body = client
        .get(format!("http://{origin_addr}/hello"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "kairo-ok");
    assert_eq!(
        count_b.load(Ordering::SeqCst) + count_a.load(Ordering::SeqCst),
        before,
        "direct mode used no node"
    );
    svc.set_mode(Mode::Rule).await.expect("mode");

    // 8. A bad override is refused, the running config stays.
    let err = svc
        .save_override(
            id.clone(),
            "proxy-groups:\n  - {name: X, type: select, proxies: [nope]}\n".into(),
        )
        .await;
    assert!(err.is_err(), "the core rejects an unknown proxy");
    assert!(svc.override_text(&id).is_empty());
    svc.save_override(
        id.clone(),
        "prepend-rules:\n  - DOMAIN,kairo.test,DIRECT\n".into(),
    )
    .await
    .expect("good override");
    assert_eq!(svc.rules().await.unwrap().len(), 4);

    // 9. Close connections.
    svc.close_all_connections().await.expect("close all");

    // 10. The core crashes: it comes back by itself.
    let pid: i32 = std::fs::read_to_string(paths.core_pid())
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    unsafe { libc_kill(pid) };
    wait_for("the restart", Duration::from_secs(5), || {
        (!state.borrow().core.is_running()).then_some(())
    })
    .await;
    wait_for("running again", Duration::from_secs(20), || {
        state.borrow().core.is_running().then_some(())
    })
    .await;
    let new_pid: i32 = std::fs::read_to_string(paths.core_pid())
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_ne!(pid, new_pid);
    // The selection survives (store-selected).
    svc.refresh_proxies().await.unwrap();
    wait_for("the selection kept", Duration::from_secs(5), || {
        (proxies
            .borrow()
            .groups
            .first()
            .and_then(|g| g.now.clone())
            .as_deref()
            == Some("node-b"))
        .then_some(())
    })
    .await;

    // 11. Off and on again at once (the tray's toggle, clicked twice): the
    // second asks before the first has stopped the core, and still connects.
    let off = svc.connect(false);
    let on = svc.connect(true);
    off.await.unwrap();
    on.await.unwrap();
    wait_for(
        "connected after off-and-on",
        Duration::from_secs(20),
        || state.borrow().core.is_running().then_some(()),
    )
    .await;
    assert!(svc.settings().auto_start_core);
    let mixed = svc.settings().mixed_port;

    // 12. Another program takes the proxy port while Kairo is off (another
    // client whose core outlived its window): Kairo moves past it.
    svc.connect(false).await.unwrap();
    let squatter = std::net::TcpListener::bind(("127.0.0.1", mixed)).unwrap();
    svc.connect(true).await.unwrap();
    assert!(state.borrow().core.is_running());
    let moved = svc.settings().mixed_port;
    assert_ne!(moved, mixed);
    let note = state.borrow().note.clone().unwrap_or_default();
    assert!(
        note.contains(&mixed.to_string()) && note.contains(&moved.to_string()),
        "{note}"
    );
    drop(squatter);

    // 13. Stop.
    svc.stop_core().await;
    assert_eq!(state.borrow().core, CoreStatus::Stopped);
    assert!(!paths.core_pid().exists());
}

/// Stops the test's core when the test ends early (a failed assertion): the
/// core outlives the process that started it, as it does a crashed Kairo.
struct CoreGuard(std::path::PathBuf);

impl Drop for CoreGuard {
    fn drop(&mut self) {
        if let Some(pid) = std::fs::read_to_string(&self.0)
            .ok()
            .and_then(|t| t.trim().parse::<i32>().ok())
        {
            unsafe { libc_kill(pid) };
        }
    }
}

unsafe fn libc_kill(pid: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe {
        kill(pid, 9);
    }
}

fn env_logger_init() -> Option<()> {
    None
}

/// TUN, for real, without touching the machine: run the test binary in a user
/// and network namespace of its own, where it is uid 0 and only `lo` exists.
///
///   cargo test -p kairo-core --test e2e --no-run
///   KAIRO_TEST_TUN=1 unshare -rn target/debug/deps/e2e-<hash> tun_in_a_network_namespace
///
/// Before TUN nothing but loopback is reachable; with TUN on, a request to
/// 203.0.113.10 (and to a name, through the DNS hijack and fake IPs) goes
/// through the core and a node; with TUN off it fails again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tun_in_a_network_namespace() {
    if std::env::var_os("KAIRO_TEST_TUN").is_none() {
        eprintln!("skipped: set KAIRO_TEST_TUN=1 and run under `unshare -rn`");
        return;
    }
    unsafe {
        std::env::set_var("KAIRO_NO_SYSPROXY", "1");
        std::env::set_var("KAIRO_NO_UPDATE", "1");
    }
    let _ = env_logger_init();
    let up = std::process::Command::new("ip")
        .args(["link", "set", "lo", "up"])
        .status()
        .expect("ip");
    assert!(up.success(), "bring lo up (run under `unshare -rn`)");
    let home = tempfile::tempdir().unwrap();
    let core = std::env::var_os("KAIRO_TEST_CORE")
        .map(std::path::PathBuf::from)
        .expect("KAIRO_TEST_CORE (no network in here)");
    assert_eq!(
        kairo_core::tun::status(&core),
        kairo_core::tun::Privilege::Granted,
        "uid 0 in the namespace"
    );

    let origin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_port = origin_listener.local_addr().unwrap().port();
    let node = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let node_port = node.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    tokio::spawn(connect_proxy(node, count.clone()));
    let subscription = format!(
        "proxies:\n  - {{name: node, type: http, server: 127.0.0.1, port: {node_port}}}\nproxy-groups:\n  - {{name: Proxy, type: select, proxies: [node]}}\nrules:\n  - MATCH,Proxy\n"
    );
    tokio::spawn(origin(origin_listener, Arc::new(subscription)));

    let mut paths = Paths::new(home.path());
    paths.bundled_core = Some(core.clone());
    paths.ensure().unwrap();
    let _core = CoreGuard(paths.core_pid());
    let mut settings = Settings::default();
    settings.mixed_port = free_port();
    settings.controller_port = free_port();
    settings.save(&paths).unwrap();
    let svc = Service::start(paths.clone());
    let state = svc.state();
    wait_for(
        "the bundled core copied in",
        Duration::from_secs(15),
        || state.borrow().core_version.clone(),
    )
    .await;
    // TUN's privilege is about the copy Kairo runs, not the bundled file.
    assert_eq!(svc.core_path(), paths.core_bin());
    assert_eq!(
        state.borrow().tun_privilege,
        Some(kairo_core::tun::Privilege::Granted),
        "uid 0 in the namespace"
    );
    svc.add_remote(format!("http://127.0.0.1:{origin_port}/sub"), None)
        .await
        .expect("add the profile");
    wait_for("the core running", Duration::from_secs(20), || {
        state.borrow().core.is_running().then_some(())
    })
    .await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let ip_url = format!("http://203.0.113.10:{origin_port}/hello");
    assert!(
        client.get(&ip_url).send().await.is_err(),
        "no route before TUN"
    );

    svc.set_tun(true).await.expect("TUN on");
    wait_for("the TUN device", Duration::from_secs(10), || {
        let out = std::process::Command::new("ip")
            .args(["-br", "addr"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .contains("198.18.0.1")
            .then_some(())
    })
    .await;
    let before = count.load(Ordering::SeqCst);
    let body = client
        .get(&ip_url)
        .send()
        .await
        .expect("through TUN")
        .text()
        .await
        .unwrap();
    assert_eq!(body, "kairo-ok");
    assert!(count.load(Ordering::SeqCst) > before, "the node carried it");
    let conns = svc.connections();
    let body = {
        let client = client.clone();
        let url = format!("http://tun-e2e.kairo.test:{origin_port}/slow");
        tokio::spawn(async move {
            client
                .get(url)
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .ok()?
                .bytes()
                .await
                .ok()
                .map(|b| b.len())
        })
    };
    let seen = wait_for("a TUN connection", Duration::from_secs(5), || {
        conns
            .borrow()
            .list
            .iter()
            .find(|c| c.host.starts_with("tun-e2e.kairo.test"))
            .cloned()
    })
    .await;
    assert!(seen.inbound.starts_with("Tun"), "inbound {}", seen.inbound);
    assert_eq!(
        body.await.unwrap(),
        Some(300000),
        "a name, through the DNS hijack"
    );

    svc.set_tun(false).await.expect("TUN off");
    wait_for("the TUN device gone", Duration::from_secs(10), || {
        let out = std::process::Command::new("ip")
            .args(["-br", "addr"])
            .output()
            .ok()?;
        (!String::from_utf8_lossy(&out.stdout).contains("198.18.0.1")).then_some(())
    })
    .await;
    assert!(
        client.get(&ip_url).send().await.is_err(),
        "no route after TUN"
    );
    svc.stop_core().await;
}

/// Upgrading from a Kairo without the AI layer: a subscription with its own
/// AI group, set by the person to another node than the Location, keeps going
/// where it was. The layer takes it over as the pinned exit; it never moves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upgrade_keeps_the_subscriptions_own_ai_group_where_it_was() {
    // SAFETY: set before any thread of this test reads the environment.
    unsafe {
        std::env::set_var("KAIRO_NO_SYSPROXY", "1");
        std::env::set_var("KAIRO_NO_UPDATE", "1");
    }
    let home = tempfile::tempdir().unwrap();
    let target = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let core = core_binary(&target);
    let origin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr: SocketAddr = origin_listener.local_addr().unwrap();
    let (pa, pb) = (
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
    );
    let (pa_port, pb_port) = (
        pa.local_addr().unwrap().port(),
        pb.local_addr().unwrap().port(),
    );
    tokio::spawn(connect_proxy(pa, Arc::new(AtomicUsize::new(0))));
    tokio::spawn(connect_proxy(pb, Arc::new(AtomicUsize::new(0))));
    // The Location (Proxy) starts on node-a; the AI group, listing node-b
    // first, starts on node-b: the person's own choice.
    let subscription = format!(
        r#"
proxies:
  - {{name: node-a, type: http, server: 127.0.0.1, port: {pa_port}}}
  - {{name: node-b, type: http, server: 127.0.0.1, port: {pb_port}}}
proxy-groups:
  - {{name: Proxy, type: select, proxies: [node-a, node-b, DIRECT]}}
  - {{name: AI, type: select, proxies: [node-b, node-a, Proxy]}}
rules:
  - DOMAIN-SUFFIX,openai.com,AI
  - MATCH,Proxy
"#
    );
    tokio::spawn(origin(origin_listener, Arc::new(subscription)));
    let mut paths = Paths::new(home.path());
    paths.bundled_core = Some(core);
    paths.ensure().unwrap();
    let _core = CoreGuard(paths.core_pid());
    // The settings file of a Kairo that knew nothing of AI: no `ai` key.
    let mut settings = Settings::default();
    settings.mixed_port = free_port();
    settings.controller_port = free_port();
    settings.secret = "e2e-secret".into();
    settings.test_url = format!("http://{origin_addr}/generate_204");
    settings.auto_location = false;
    settings.test_timeout_ms = 3000;
    settings.save(&paths).unwrap();
    let text = std::fs::read_to_string(paths.settings()).unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json.as_object_mut().unwrap().remove("ai");
    std::fs::write(paths.settings(), serde_json::to_string(&json).unwrap()).unwrap();

    let svc = Service::start(paths.clone());
    let state = svc.state();
    wait_for("the core", Duration::from_secs(20), || {
        state.borrow().core.is_running().then_some(())
    })
    .await;
    assert!(
        state.borrow().settings.ai.enabled,
        "the layer is on by default"
    );
    svc.add_remote(format!("http://{origin_addr}/sub"), None)
        .await
        .expect("add the subscription");
    let pinned = wait_for("an exit pinned", Duration::from_secs(30), || {
        state.borrow().settings.ai.exit.clone()
    })
    .await;
    assert_eq!(
        pinned.name, "node-b",
        "the AI group's own choice is the exit"
    );
    assert_eq!(state.borrow().ai.group, "AI");
    assert!(state.borrow().ai.adopted);
    // Let the sync and the location checks settle; AI must still be on node-b.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(svc.ai_node().as_deref(), Some("node-b"));
    svc.stop_core().await;
}

/// The AI layer against a real core: AI hosts go to their own group on one
/// pinned exit, a change of location or of exit never cuts an open stream,
/// loopback stays direct, and "All Traffic" keeps AI where it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ai_layer_end_to_end() {
    use kairo_core::activity::Target;
    // SAFETY: set before any thread of this test reads the environment.
    unsafe {
        std::env::set_var("KAIRO_NO_SYSPROXY", "1");
        std::env::set_var("KAIRO_NO_UPDATE", "1");
    }
    let home = tempfile::tempdir().unwrap();
    let target = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let core = core_binary(&target);

    let origin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr: SocketAddr = origin_listener.local_addr().unwrap();
    let (pa, pb) = (
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
    );
    let (pa_port, pb_port) = (
        pa.local_addr().unwrap().port(),
        pb.local_addr().unwrap().port(),
    );
    let (count_a, count_b) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    tokio::spawn(connect_proxy(pa, count_a.clone()));
    tokio::spawn(connect_proxy(pb, count_b.clone()));
    let subscription = format!(
        r#"
proxies:
  - {{name: node-a, type: http, server: 127.0.0.1, port: {pa_port}}}
  - {{name: node-b, type: http, server: 127.0.0.1, port: {pb_port}}}
proxy-groups:
  - {{name: Proxy, type: select, proxies: [node-a, node-b, DIRECT]}}
rules:
  - MATCH,Proxy
"#
    );
    tokio::spawn(origin(origin_listener, Arc::new(subscription)));

    let mut paths = Paths::new(home.path());
    paths.bundled_core = Some(core);
    paths.ensure().unwrap();
    let _core = CoreGuard(paths.core_pid());
    let mut settings = Settings::default();
    settings.mixed_port = free_port();
    settings.controller_port = free_port();
    settings.secret = "e2e-secret".into();
    settings.test_url = format!("http://{origin_addr}/generate_204");
    settings.test_timeout_ms = 3000;
    settings.save(&paths).unwrap();
    let mixed = settings.mixed_port;

    let svc = Service::start(paths.clone());
    let state = svc.state();
    wait_for("the core", Duration::from_secs(20), || {
        state.borrow().core.is_running().then_some(())
    })
    .await;
    svc.add_remote(format!("http://{origin_addr}/sub"), None)
        .await
        .expect("add the subscription");

    // The layer's group is in, hidden, and an exit gets pinned by itself.
    let proxies = svc.proxies();
    wait_for("the AI group", Duration::from_secs(20), || {
        proxies
            .borrow()
            .groups
            .iter()
            .any(|g| g.name == "Kairo AI" && g.hidden)
            .then_some(())
    })
    .await;
    let pinned = wait_for("an exit pinned", Duration::from_secs(30), || {
        state.borrow().settings.ai.exit.clone()
    })
    .await;
    assert!(pinned.name.starts_with("node-"), "{pinned:?}");
    assert_eq!(state.borrow().ai.group, "Kairo AI");
    let other = if pinned.name == "node-a" {
        "node-b"
    } else {
        "node-a"
    };
    wait_for(
        "the AI group on the pinned exit",
        Duration::from_secs(10),
        || (svc.ai_node().as_deref() == Some(pinned.name.as_str())).then_some(()),
    )
    .await;

    // Its rules are in the core, ahead of the subscription's.
    let rules = svc.rules().await.expect("rules");
    let at = |kind: &str, payload: &str| {
        rules
            .iter()
            .position(|r| r.kind == kind && r.payload == payload)
    };
    assert_eq!(at("IPCIDR", "127.0.0.0/8"), Some(0), "{rules:?}");
    let claude = at("DomainSuffix", "claude.ai").expect("claude.ai has a rule");
    assert_eq!(rules[claude].proxy, "Kairo AI");
    assert_eq!(rules.last().unwrap().kind, "Match");

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{mixed}")).unwrap())
        .build()
        .unwrap();
    let node_count = |n: &str| {
        if n == "node-a" {
            count_a.load(Ordering::SeqCst)
        } else {
            count_b.load(Ordering::SeqCst)
        }
    };
    let conns = svc.connections();

    // A long answer from an AI host goes through the exit, and is tallied.
    let before = node_count(&pinned.name);
    let long = {
        let client = client.clone();
        let url = format!("http://claude.ai:{}/long", origin_addr.port());
        tokio::spawn(async move { client.get(url).send().await?.bytes().await.map(|b| b.len()) })
    };
    let seen = wait_for("the AI connection", Duration::from_secs(5), || {
        conns
            .borrow()
            .list
            .iter()
            .find(|c| c.host == "claude.ai")
            .cloned()
    })
    .await;
    assert_eq!(seen.outbound(), pinned.name);
    assert!(
        seen.chains.contains(&"Kairo AI".to_string()),
        "{:?}",
        seen.chains
    );
    assert!(node_count(&pinned.name) > before);
    let activity = svc.activity();
    wait_for("the tally", Duration::from_secs(5), || {
        activity
            .borrow()
            .flows()
            .iter()
            .any(|(_, t, f)| **t == Target::Service("claude") && f.open == 1)
            .then_some(())
    })
    .await;

    // Moving the Location does not move AI; moving AI's exit does not cut the stream.
    svc.select("Proxy".into(), other.into())
        .await
        .expect("select");
    assert_eq!(
        svc.ai_node().as_deref(),
        Some(pinned.name.as_str()),
        "AI stays put"
    );
    svc.set_ai_exit(other.into())
        .await
        .expect("pin another exit");
    assert_eq!(svc.ai_node().as_deref(), Some(other));
    assert_eq!(
        state.borrow().settings.ai.exit.as_ref().unwrap().name,
        other
    );
    assert!(
        !long.is_finished(),
        "the stream still runs after the exit moved"
    );
    assert_eq!(long.await.unwrap().expect("not cut"), 60000);
    // New AI connections take the new exit.
    let before = node_count(other);
    client
        .get(format!("http://claude.ai:{}/hello", origin_addr.port()))
        .send()
        .await
        .unwrap();
    assert!(node_count(other) > before);

    // Loopback never goes through a node (a local model).
    client
        .get(format!("http://{origin_addr}/slow"))
        .send()
        .await
        .unwrap();
    let local = conns
        .borrow()
        .list
        .iter()
        .find(|c| c.port == origin_addr.port().to_string() && c.host == "127.0.0.1")
        .cloned();
    if let Some(c) = local {
        assert_eq!(c.outbound(), "DIRECT");
    }

    // "All Traffic" is rules with the Location as the rest: AI keeps its exit.
    svc.set_mode(Mode::Global).await.expect("all traffic");
    let rules = svc.rules().await.expect("rules");
    let last = rules.last().unwrap();
    assert_eq!(
        (last.kind.as_str(), last.proxy.as_str()),
        ("Match", "Proxy")
    );
    assert!(
        rules
            .iter()
            .any(|r| r.payload == "claude.ai" && r.proxy == "Kairo AI")
    );
    assert_eq!(svc.ai_node().as_deref(), Some(other));
    svc.set_mode(Mode::Rule).await.expect("smart");

    // Following the Location is a choice too.
    svc.ai_follow_location().await.expect("follow");
    assert!(state.borrow().settings.ai.exit.is_none());
    let groups = proxies.borrow().groups.clone();
    let ai = groups.iter().find(|g| g.name == "Kairo AI").unwrap();
    assert_eq!(ai.now.as_deref(), Some("Proxy"));

    // Turning the layer off puts the subscription's own rules back.
    let mut next = svc.settings().ai;
    next.enabled = false;
    svc.update_ai(next).await.expect("off");
    let rules = svc.rules().await.expect("rules");
    assert_eq!(rules.len(), 1, "{rules:?}");
}
