//! A core that cannot name the process of a connection (as on a macOS where
//! its read of the socket tables fails) still gets its tools named in the
//! Activity tally: Kairo asks `lsof` for the owner of the socket. Here the
//! core is told not to look (`find-process-mode: off`, which a profile may
//! set for itself and Kairo keeps), and `KAIRO_LSOF_PROCESS=1` turns the
//! lookup on as it is on macOS. Its own test file: the variable is the
//! process's, and the other end-to-end tests must not see it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use kairo_core::Service;
use kairo_core::activity::Target;
use kairo_core::paths::Paths;
use kairo_core::settings::Settings;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

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
            if path.ends_with("/sub") {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/yaml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    sub.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(sub.as_bytes()).await;
            } else if path.ends_with("/generate_204") {
                let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            } else {
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
            }
        });
    }
}

async fn connect_proxy(listener: TcpListener) {
    loop {
        let Ok((mut client, _)) = listener.accept().await else {
            return;
        };
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
                return;
            }
            let target = parts.next().unwrap_or("");
            let target = match target.rsplit_once(':') {
                Some((host, port)) if host.ends_with("claude.ai") => format!("127.0.0.1:{port}"),
                _ => target.to_owned(),
            };
            let Ok(mut upstream) = TcpStream::connect(&target).await else {
                return;
            };
            let _ = client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await;
            let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        });
    }
}

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
            unsafe {
                kill(pid, 9);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_core_that_names_no_process_is_helped_by_lsof() {
    // SAFETY: set before any thread of this test reads the environment.
    unsafe {
        std::env::set_var("KAIRO_NO_SYSPROXY", "1");
        std::env::set_var("KAIRO_NO_UPDATE", "1");
        std::env::set_var("KAIRO_LSOF_PROCESS", "1");
    }
    if !["/usr/bin/lsof", "/usr/sbin/lsof"]
        .iter()
        .any(|p| std::path::Path::new(p).exists())
    {
        eprintln!("no lsof here: skipped");
        return;
    }
    let target = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let core = match std::env::var_os("KAIRO_TEST_CORE") {
        Some(p) => std::path::PathBuf::from(p),
        None => target.join("mihomo/mihomo"),
    };
    assert!(core.is_file(), "no core: set KAIRO_TEST_CORE");
    let home = tempfile::tempdir().unwrap();
    let origin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr: SocketAddr = origin_listener.local_addr().unwrap();
    let pa = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pa_port = pa.local_addr().unwrap().port();
    tokio::spawn(connect_proxy(pa));
    let subscription = format!(
        "find-process-mode: off\nproxies:\n  - {{name: node-a, type: http, server: 127.0.0.1, port: {pa_port}}}\nproxy-groups:\n  - {{name: Proxy, type: select, proxies: [node-a, DIRECT]}}\nrules:\n  - MATCH,Proxy\n"
    );
    tokio::spawn(origin(origin_listener, Arc::new(subscription)));

    let mut paths = Paths::new(home.path());
    paths.bundled_core = Some(core);
    paths.ensure().unwrap();
    let _core = CoreGuard(paths.core_pid());
    let mut settings = Settings::default();
    settings.mixed_port = free_port();
    settings.controller_port = free_port();
    settings.secret = "lsof-secret".into();
    settings.test_url = format!("http://{origin_addr}/generate_204");
    settings.test_timeout_ms = 3000;
    settings.save(&paths).unwrap();
    let mixed = settings.mixed_port;

    let svc = Service::start(paths.clone());
    let state = svc.state();
    let started = std::time::Instant::now();
    while !state.borrow().core.is_running() {
        assert!(started.elapsed() < Duration::from_secs(20), "the core");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    svc.add_remote(format!("http://{origin_addr}/sub"), None)
        .await
        .expect("add the subscription");

    // The core is told not to name processes: its own list has none.
    let mut tool = tokio::process::Command::new("curl")
        .args(["-s", "-m", "30", "-o", "/dev/null", "-x"])
        .arg(format!("http://127.0.0.1:{mixed}"))
        .arg(format!("http://claude.ai:{}/long", origin_addr.port()))
        .kill_on_drop(true)
        .spawn()
        .expect("curl");
    let conns = svc.connections();
    let activity = svc.activity();
    let started = std::time::Instant::now();
    loop {
        let named = activity.borrow().flows().iter().any(|(tool, t, f)| {
            **t == Target::Service("claude") && f.open >= 1 && tool.eq_ignore_ascii_case("curl")
        });
        if named {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "no tool named curl in the tally: {:?}; core says {:?}",
            activity.borrow().flows(),
            conns
                .borrow()
                .list
                .iter()
                .map(|c| (c.host.clone(), c.process.clone()))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Nothing was counted as an unknown tool first.
    assert!(
        !activity
            .borrow()
            .flows()
            .iter()
            .any(|(tool, t, _)| **t == Target::Service("claude") && *tool == "Unknown"),
        "{:?}",
        activity.borrow().flows()
    );
    tool.kill().await.ok();
    svc.stop_core().await;
}
