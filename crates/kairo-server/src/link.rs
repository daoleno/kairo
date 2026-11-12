//! One page's WebSocket: what the service shows, pushed as it changes; the
//! page's commands, done and answered. Everything out goes through
//! [`crate::redact::encode`].

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use futures_util::{SinkExt, StreamExt};
use kairo_core::protocol::{ClientMsg, Hello, ServerMsg, VERSION};
use tokio::sync::{Semaphore, mpsc};

use crate::Ctx;
use crate::auth::SessionKey;
use crate::redact::encode;

/// The close code a page reads as "signed out".
pub const SIGNED_OUT: u16 = 4401;
/// Commands one page may have running at once.
const IN_FLIGHT: usize = 16;

pub fn hello(ctx: &Ctx) -> Hello {
    Hello {
        protocol: VERSION,
        version: kairo_core::update::VERSION.to_owned(),
        machine: crate::machine_name(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        home: ctx.svc.paths().home.to_string_lossy().into_owned(),
        offset: kairo_core::time::local_offset_seconds(),
        terminal_env: ctx.svc.terminal_env(),
        tun_grant: tun_grant(&ctx.svc.core_path()),
    }
}

/// What the owner runs on this machine to let the core start TUN: the
/// desktop app asks for it in a password dialog; a server has no one there.
pub fn tun_grant(core: &std::path::Path) -> Option<String> {
    let path = shell_quote(&core.to_string_lossy());
    if cfg!(target_os = "linux") {
        Some(format!("sudo setcap cap_net_admin,cap_net_bind_service=+ep {path}"))
    } else if cfg!(target_os = "macos") {
        Some(format!("sudo chown root:admin {path} && sudo chmod u+s,a+x {path}"))
    } else {
        None
    }
}

/// `s` as one word for a shell.
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_grant_command_quotes_the_path() {
        assert_eq!(super::shell_quote("/home/a/.local/share/kairo/core/mihomo"), "/home/a/.local/share/kairo/core/mihomo");
        assert_eq!(super::shell_quote("/srv/my core/it's"), "'/srv/my core/it'\\''s'");
    }
}

pub async fn serve(ctx: Arc<Ctx>, key: SessionKey, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
    let svc = ctx.svc.clone();
    let mut state = svc.state();
    let mut live = svc.live();
    let mut connections = svc.connections();
    let mut proxies = svc.proxies();
    let mut activity = svc.activity();
    let mut stats = svc.stats();
    let mut log_seq = svc.log_seq();
    let mut icons = svc.icons_rev();
    let mut ended = ctx.auth.ended();
    let running = Arc::new(Semaphore::new(IN_FLIGHT));
    let mut check = tokio::time::interval(Duration::from_secs(20));
    check.tick().await;

    let mut secrets = ctx.secrets(&state.borrow_and_update());
    // What the page needs before its first page.
    let mut last_log = *log_seq.borrow_and_update();
    let first = vec![
        ServerMsg::Hello(Box::new(hello(&ctx))),
        ServerMsg::State(Box::new(state.borrow().clone())),
        ServerMsg::Live(Box::new(live.borrow_and_update().clone())),
        ServerMsg::Proxies(proxies.borrow_and_update().clone()),
        ServerMsg::Connections(connections.borrow_and_update().clone()),
        ServerMsg::Activity(activity.borrow_and_update().clone()),
        ServerMsg::Stats(stats.borrow_and_update().clone()),
        ServerMsg::Logs { reset: true, lines: svc.logs(), seq: last_log },
        ServerMsg::IconsRev(*icons.borrow_and_update()),
        ServerMsg::Synced,
    ];
    for m in first {
        if sink.send(Message::Text(encode(&m, &secrets).into())).await.is_err() {
            return;
        }
    }
    let mut last_line = svc.logs().last().map(|l| l.seq).unwrap_or(0);

    let close = |code: u16| {
        Message::Close(Some(CloseFrame { code, reason: Utf8Bytes::from_static("") }))
    };
    loop {
        let msg = tokio::select! {
            r = state.changed() => {
                if r.is_err() { break }
                ServerMsg::State(Box::new(state.borrow_and_update().clone()))
            }
            r = live.changed() => {
                if r.is_err() { break }
                ServerMsg::Live(Box::new(live.borrow_and_update().clone()))
            }
            r = connections.changed() => {
                if r.is_err() { break }
                ServerMsg::Connections(connections.borrow_and_update().clone())
            }
            r = proxies.changed() => {
                if r.is_err() { break }
                ServerMsg::Proxies(proxies.borrow_and_update().clone())
            }
            r = activity.changed() => {
                if r.is_err() { break }
                ServerMsg::Activity(activity.borrow_and_update().clone())
            }
            r = stats.changed() => {
                if r.is_err() { break }
                ServerMsg::Stats(stats.borrow_and_update().clone())
            }
            r = icons.changed() => {
                if r.is_err() { break }
                ServerMsg::IconsRev(*icons.borrow_and_update())
            }
            r = log_seq.changed() => {
                if r.is_err() { break }
                let seq = *log_seq.borrow_and_update();
                let all = svc.logs();
                // The log was cleared (or turned over past what was sent).
                let reset = seq < last_log || all.first().is_some_and(|f| f.seq > last_line + 1) || (all.is_empty() && last_line > 0);
                let lines: Vec<_> = if reset {
                    all.clone()
                } else {
                    all.iter().filter(|l| l.seq > last_line).cloned().collect()
                };
                last_log = seq;
                last_line = all.last().map(|l| l.seq).unwrap_or(0);
                ServerMsg::Logs { reset, lines, seq }
            }
            out = out_rx.recv() => {
                let Some(text) = out else { break };
                if sink.send(Message::Text(text.into())).await.is_err() { break }
                continue;
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(msg) = serde_json::from_str::<ClientMsg>(&text) else {
                            log::warn!("a page sent a message Kairo does not know");
                            continue;
                        };
                        let Ok(permit) = running.clone().try_acquire_owned() else {
                            let m = ServerMsg::reply(msg.id, Err("Too many commands at once".into()));
                            let _ = out_tx.send(encode(&m, &secrets)).await;
                            continue;
                        };
                        let ctx = ctx.clone();
                        let key = key.clone();
                        let tx = out_tx.clone();
                        tokio::spawn(async move {
                            let result = crate::dispatch::run(&ctx, &key, msg.cmd).await;
                            // Scrubbed with the secrets of after the command (a link just added).
                            let secrets = ctx.secrets(&ctx.svc.snapshot());
                            let _ = tx.send(encode(&ServerMsg::reply(msg.id, result), &secrets)).await;
                            drop(permit);
                        });
                        continue;
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => continue,
                }
            }
            gone = ended.recv() => {
                match gone {
                    Ok(k) if k == key => {
                        let _ = sink.send(Message::Text(encode(&ServerMsg::SignedOut, &secrets).into())).await;
                        let _ = sink.send(close(SIGNED_OUT)).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => continue,
                }
            }
            _ = check.tick() => {
                // Ended elsewhere (the CLI), or expired.
                if !ctx.auth.alive(&key) {
                    let _ = sink.send(close(SIGNED_OUT)).await;
                    break;
                }
                let _ = sink.send(Message::Ping(Default::default())).await;
                continue;
            }
        };
        // Fresh each time: a link another page just sent is a secret already.
        secrets = ctx.secrets(&state.borrow());
        if sink.send(Message::Text(encode(&msg, &secrets).into())).await.is_err() {
            break;
        }
    }
}
