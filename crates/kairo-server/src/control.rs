//! The CLI's way into a running server: a Unix socket in `<home>/web` (a
//! folder only its owner can open), one line in, one line out. Being able
//! to open it is the authority: it is the shell of the account Kairo runs as.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::Ctx;

pub fn socket_path(home: &Path) -> PathBuf {
    crate::web_state_dir(home).join("control.sock")
}

pub fn spawn(ctx: Arc<Ctx>) {
    let path = socket_path(&ctx.opts.home);
    let _ = std::fs::remove_file(&path);
    let listener = match tokio::net::UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("no control socket at {}: {e}", path.display());
            return;
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                // One short line: no more is read.
                if BufReader::new(read.take(4096)).read_line(&mut line).await.is_err() {
                    return;
                }
                let answer = answer(&ctx, line.trim()).await;
                let _ = write.write_all(format!("{answer}\n").as_bytes()).await;
            });
        }
    });
}

async fn answer(ctx: &Ctx, line: &str) -> String {
    match line {
        "status" => status(ctx).await.to_string(),
        "every-app off" => match ctx.svc.set_tun(false).await {
            Ok(()) => json!({ "ok": true }).to_string(),
            Err(e) => json!({ "error": e }).to_string(),
        },
        _ => json!({ "error": "unknown command" }).to_string(),
    }
}

pub async fn status(ctx: &Ctx) -> serde_json::Value {
    let every_app = ctx.svc.tun_up().await;
    let s = ctx.svc.snapshot();
    json!({
        "running": true,
        "version": kairo_core::update::VERSION,
        "home": ctx.opts.home,
        "listen": ctx.opts.listen.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
        "public_url": ctx.opts.public_url.as_ref().map(|u| u.to_string()),
        "https": ctx.opts.tls.is_some(),
        // Listeners not opened (plain HTTP beyond loopback without --insecure-http).
        "refused": ctx.opts.listen.iter().filter(|a| ctx.opts.refused(a).is_some()).map(|a| a.to_string()).collect::<Vec<_>>(),
        "password_set": !ctx.auth.needs_setup(),
        "setup_code": ctx.auth.setup_code(),
        "core": s.core.title(),
        // TUN as it is (the core's own, and its device), not the setting.
        "every_app": every_app,
        "every_app_setting": s.settings.tun,
        "system_proxy": s.settings.system_proxy,
        "core_version": s.core_version,
        // Whether the core in this folder may run TUN: "granted", "needed"
        // (setcap again) or "unsupported".
        "tun_permission": s.tun_privilege.map(|p| match p {
            kairo_core::tun::Privilege::Granted => "granted",
            kairo_core::tun::Privilege::Needed => "needed",
            kairo_core::tun::Privilege::Unsupported => "unsupported",
        }),
        // The core in this folder was replaced at this start by the one
        // Kairo ships (another version): null when it was left as it was.
        "core_replaced": s.core_replaced.as_ref().map(|r| json!({
            "from": r.from,
            "to": r.to,
            "tun_permission_needed": s.tun_privilege == Some(kairo_core::tun::Privilege::Needed),
        })),
        "subscription": s.profiles.active.as_ref().and_then(|id| s.profiles.profiles.iter().find(|p| &p.id == id)).map(|p| p.name.clone()),
    })
}

/// Sends `line` to the server of `home`: its answer, or `None` when none runs.
pub fn ask(home: &Path, line: &str) -> Option<serde_json::Value> {
    use std::io::{BufRead, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(socket_path(home)).ok()?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(60))).ok()?;
    stream.write_all(format!("{line}\n").as_bytes()).ok()?;
    let mut answer = String::new();
    std::io::BufReader::new(stream).read_line(&mut answer).ok()?;
    serde_json::from_str(&answer).ok()
}
