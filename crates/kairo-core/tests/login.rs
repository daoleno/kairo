//! The login after a crash: Kairo and its core were killed, so the system proxy
//! still points at Kairo's port, and nothing answers there. At the next login
//! two things start at once: the network check (`kairo --restore-network`) and
//! Kairo itself (launch at login, connected last time). Whatever the order, the
//! check must not undo what Kairo sets for its new core: Kairo ends connected,
//! the system proxy points at it, traffic through that proxy goes out through
//! the subscription's node, and Disconnect still puts the proxy back.
//!
//! A check starts every 25 ms through Kairo's whole start, so that one starts
//! at each moment of it. What went wrong before: a check that decided while
//! Kairo was still cleaning up after the crash was still writing when Kairo
//! set the proxy, and at its end deleted Kairo's fresh record. Now the two take
//! turns, and a check that finds Kairo's core starting waits for it.
//!
//! The system proxy here is KDE's, through stand-ins for `kwriteconfig6` and
//! `kreadconfig6` that keep the settings in a file in a temporary folder and
//! take 0.2 s per write (as slow as macOS, where every network service takes
//! several `networksetup` runs). PATH holds only those two, so GNOME's
//! `gsettings` cannot be found: the machine's own proxy is never touched
//! (checked before anything runs). Linux; the core as in `e2e.rs`.

#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use kairo_core::Service;
use kairo_core::paths::Paths;
use kairo_core::settings::Settings;
use kairo_core::sysproxy::{self, Backends, ProxySpec};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Serves the subscription at `/sub`, "kairo-ok" elsewhere.
async fn origin(listener: TcpListener, subscription: Arc<String>) {
    while let Ok((mut s, _)) = listener.accept().await {
        let sub = subscription.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = if req
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .ends_with("/sub")
            {
                sub.to_string()
            } else {
                "kairo-ok".to_owned()
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes()).await;
            let _ = s.write_all(body.as_bytes()).await;
        });
    }
}

/// A minimal HTTP CONNECT proxy (the subscription's node) counting its tunnels.
async fn node(listener: TcpListener, count: Arc<AtomicUsize>) {
    while let Ok((mut client, _)) = listener.accept().await {
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
            let target = head.split_whitespace().nth(1).unwrap_or("").to_owned();
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

async fn wait_for<T>(what: &str, limit: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let start = std::time::Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(start.elapsed() < limit, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `kwriteconfig6 --file F --group G --key K V` / `kreadconfig6 ... --key K`,
/// the settings in `$XDG_CONFIG_HOME/F.json`; a write takes 0.2 s.
const KDE_STAND_IN: &str = r#"#!/usr/bin/python3 -I
import json, os, sys, time
a = sys.argv[1:]
path = os.path.join(os.environ["XDG_CONFIG_HOME"], a[a.index("--file") + 1] + ".json")
key = a[a.index("--key") + 1]
try:
    data = json.load(open(path))
except (OSError, ValueError):
    data = {}
if WRITE:
    time.sleep(0.2)
    try:
        data = json.load(open(path))
    except (OSError, ValueError):
        data = {}
    data[key] = a[-1]
    tmp = path + ".%d" % os.getpid()
    json.dump(data, open(tmp, "w"))
    os.replace(tmp, path)
else:
    print(data.get(key, ""))
"#;

/// Stops the test's cores when the test ends, as passed or failed.
struct CoreGuard(std::path::PathBuf);

impl Drop for CoreGuard {
    fn drop(&mut self) {
        if let Some(pid) = std::fs::read_to_string(&self.0)
            .ok()
            .and_then(|t| t.trim().parse::<i32>().ok())
        {
            unsafe extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            unsafe { kill(pid, 9) };
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_login_check_leaves_a_starting_kairo_connected() {
    let python = std::path::Path::new("/usr/bin/python3");
    if !python.is_file() {
        eprintln!("skipped: no /usr/bin/python3 for the KDE stand-ins");
        return;
    }
    let core = match std::env::var_os("KAIRO_TEST_CORE") {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/mihomo/mihomo")
        }
    };
    assert!(
        core.is_file(),
        "no core: run scripts/fetch-mihomo.sh or set KAIRO_TEST_CORE"
    );

    // A desktop of the test's own: KDE's proxy settings in a temporary folder.
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let config = dir.path().join("config");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    for (tool, write) in [("kwriteconfig6", true), ("kreadconfig6", false)] {
        let path = bin.join(tool);
        std::fs::write(
            &path,
            KDE_STAND_IN.replace("WRITE", if write { "True" } else { "False" }),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // SAFETY: set before any thread of this test reads the environment.
    unsafe {
        std::env::set_var("PATH", &bin);
        std::env::set_var("HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("XDG_CURRENT_DESKTOP", "KDE");
        std::env::set_var("GSETTINGS_BACKEND", "memory");
        std::env::remove_var("DBUS_SESSION_BUS_ADDRESS");
        std::env::remove_var("KAIRO_NO_SYSPROXY");
        std::env::set_var("KAIRO_NO_UPDATE", "1");
    }
    assert_eq!(
        sysproxy::backends(),
        Backends {
            gnome: false,
            kde: Some("6"),
            mac: false,
            windows: false
        },
        "only the test's own KDE settings may be written"
    );
    let kioslaverc = config.join("kioslaverc.json");

    // The subscription: everything through one local node.
    let origin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = origin_listener.local_addr().unwrap();
    let node_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let node_port = node_listener.local_addr().unwrap().port();
    let tunnels = Arc::new(AtomicUsize::new(0));
    tokio::spawn(node(node_listener, tunnels.clone()));
    let subscription = format!(
        "proxies:\n  - {{name: node, type: http, server: 127.0.0.1, port: {node_port}}}\nproxy-groups:\n  - {{name: Proxy, type: select, proxies: [node]}}\nrules:\n  - MATCH,Proxy\n"
    );
    tokio::spawn(origin(origin_listener, Arc::new(subscription)));

    let home = dir.path().join("kairo");
    let mut paths = Paths::new(&home);
    paths.bundled_core = Some(core.clone());
    paths.ensure().unwrap();
    let _core = CoreGuard(paths.core_pid());
    let settings = Settings {
        mixed_port: free_port(),
        controller_port: free_port(),
        test_url: format!("http://{origin_addr}/generate_204"),
        // The test's origin is on loopback, which the AI layer keeps off the
        // nodes (a local model is never proxied); this test is about the
        // login check and the system proxy, not about AI.
        ai: kairo_core::ai::AiSettings {
            enabled: false,
            ..Default::default()
        },
        ..Settings::default()
    };
    settings.save(&paths).unwrap();
    let mixed = settings.mixed_port;
    let spec = ProxySpec {
        host: "127.0.0.1".into(),
        port: mixed,
        bypass: settings.bypass.clone(),
    };

    // The last session: the subscription added, connected, the system proxy on.
    {
        let svc = Service::start(paths.clone());
        let state = svc.state();
        wait_for("the core in place", Duration::from_secs(15), || {
            state.borrow().core_version.clone()
        })
        .await;
        svc.add_remote(format!("http://{origin_addr}/sub"), None)
            .await
            .expect("add the subscription");
        svc.connect(true).await.expect("connect");
        wait_for(
            "the first session connected",
            Duration::from_secs(20),
            || (state.borrow().core.is_running() && state.borrow().sysproxy_active).then_some(()),
        )
        .await;
        assert!(sysproxy::points_to(&spec));
        svc.shutdown();
    }
    // Then it crashed: Kairo, its guardian and its core killed, the proxy left
    // pointing at a port nothing answers.
    sysproxy::enable(&paths, &spec).unwrap();
    assert!(sysproxy::points_to(&spec) && !sysproxy::answers(mixed));
    assert!(Settings::load(&paths).auto_start_core && Settings::load(&paths).system_proxy);

    // The login: Kairo starts, and the network check starts at some moment
    // of it. Every 25 ms a check of its own, so that one starts at each moment.
    let stop = Arc::new(AtomicBool::new(false));
    let restored = Arc::new(AtomicUsize::new(0));
    let checks = {
        let (paths, stop, restored) = (paths.clone(), stop.clone(), restored.clone());
        std::thread::spawn(move || {
            let mut started = Vec::new();
            while !stop.load(Ordering::SeqCst) {
                let (paths, restored) = (paths.clone(), restored.clone());
                started.push(std::thread::spawn(move || {
                    if sysproxy::restore_if_dead(&paths) {
                        restored.fetch_add(1, Ordering::SeqCst);
                    }
                }));
                std::thread::sleep(Duration::from_millis(25));
            }
            let runs = started.len();
            for t in started {
                t.join().unwrap();
            }
            runs
        })
    };
    let svc = Service::start(paths.clone());
    let state = svc.state();
    wait_for("Kairo connected at login", Duration::from_secs(30), || {
        (state.borrow().core.is_running() && state.borrow().sysproxy_active).then_some(())
    })
    .await;
    // Checks keep coming after Kairo is up.
    tokio::time::sleep(Duration::from_secs(2)).await;
    stop.store(true, Ordering::SeqCst);
    let runs = checks.join().unwrap();
    let restored = restored.load(Ordering::SeqCst);
    eprintln!("{runs} checks during the login, {restored} restored the dead proxy");
    assert!(runs > 1);

    // Home: Connected. The system proxy: Kairo's port, which answers.
    assert!(state.borrow().core.is_running());
    assert!(state.borrow().sysproxy_active);
    let rc = std::fs::read_to_string(&kioslaverc).unwrap_or_default();
    assert!(
        sysproxy::points_to(&spec),
        "the system proxy was switched off under a connected Kairo:\n{rc}"
    );
    assert!(sysproxy::answers(mixed));
    assert!(
        paths.sysproxy_state().exists(),
        "Kairo still knows how to put the proxy back when it quits"
    );

    // Traffic through the system proxy goes out through the node.
    let before = tunnels.load(Ordering::SeqCst);
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
    assert!(tunnels.load(Ordering::SeqCst) > before, "through the node");

    // Disconnect puts the proxy back as it was before Kairo.
    svc.connect(false).await.unwrap();
    assert!(
        !sysproxy::points_to(&spec),
        "Disconnect left the system proxy on a stopped Kairo"
    );

    // And a real dead proxy is still put back: Kairo and its core gone.
    svc.shutdown();
    sysproxy::enable(&paths, &spec).unwrap();
    assert!(sysproxy::restore_if_dead(&paths));
    assert!(!sysproxy::points_to(&spec));
}
