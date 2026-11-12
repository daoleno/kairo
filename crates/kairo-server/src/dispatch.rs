//! A page's command, done by the service. Every [`Command`] has its arm
//! here (the match is exhaustive): what the desktop can do, a page can.

use std::path::Path;

use kairo_core::protocol::{Command, HIDDEN, IconInfo, IconKind};
use kairo_core::Service;
use kairo_core::tun::Privilege;
use serde::Serialize;
use serde_json::Value;

use crate::Ctx;
use crate::auth::SessionKey;
use crate::redact::{keep_hidden_settings, keep_hidden_url};

fn json<T: Serialize>(r: Result<T, String>) -> Result<Value, String> {
    r.and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
}

fn done() -> Result<Value, String> {
    Ok(Value::Null)
}

/// Does `cmd` for the person signed in with `session`.
pub async fn run(ctx: &Ctx, session: &SessionKey, cmd: Command) -> Result<Value, String> {
    let svc = &ctx.svc;
    match cmd {
        Command::Connect { on } => json(svc.connect(on).await),
        Command::StartCore => json(svc.start_core().await),
        Command::StopCore => {
            svc.stop_core().await;
            done()
        }
        Command::RestartCore => json(svc.restart_core().await),
        Command::SetMode { mode } => json(svc.set_mode(mode).await),
        Command::SetLocation { name } => json(svc.set_location(name).await),
        Command::TestPlaces => json(svc.test_places().await),
        Command::UpdateSettings { settings } => {
            let next = keep_hidden_settings(*settings, &svc.settings());
            json(svc.update_settings(next).await)
        }
        Command::SetSystemProxy { on } => json(svc.set_system_proxy(on).await),
        Command::SetTun { on } => json(svc.set_tun(on).await),
        // Never pkexec from here: its dialog would open on this machine's
        // screen, if anywhere. The owner runs the command; this checks again.
        Command::GrantTun => match svc.recheck_tun() {
            Privilege::Granted => done(),
            Privilege::Unsupported => Err(format!("Every app (TUN) is not available on {}", crate::machine_name())),
            Privilege::Needed => Err(match crate::link::tun_grant(&svc.core_path()) {
                Some(cmd) => format!("Run this on {}, then try again: {cmd}", crate::machine_name()),
                None => "The core has no TUN permission".into(),
            }),
        },
        Command::AddRemote { url, name } => {
            ctx.remember_link(&url);
            json(svc.add_remote(url, name).await)
        }
        Command::AddYaml { name, text } => add_yaml(svc, &name, text).await,
        Command::UpdateProfile { id } => json(svc.update_profile(id).await),
        Command::UpdateAllProfiles => json(svc.update_all_profiles().await),
        Command::Activate { id } => json(svc.activate(id).await),
        Command::RenameProfile { id, name, interval_hours, url } => {
            let current = svc
                .snapshot()
                .profiles
                .profiles
                .iter()
                .find(|p| p.id == id)
                .and_then(|p| p.url().map(str::to_owned));
            let url = keep_hidden_url(url, current.as_deref());
            if let Some(u) = &url {
                ctx.remember_link(u);
            }
            svc.rename_profile(id, name, interval_hours, url);
            done()
        }
        Command::DeleteProfile { id } => json(svc.delete_profile(id).await),
        Command::DismissFailure { id } => {
            svc.dismiss_failure(&id);
            done()
        }
        Command::RefreshProxies => json(svc.refresh_proxies().await),
        Command::Select { group, name } => json(svc.select(group, name).await),
        Command::TestNode { name, url } => json(svc.test_node(name, url).await),
        Command::TestGroup { group } => json(svc.test_group(group).await),
        Command::UpdateProvider { name } => json(svc.update_provider(name).await),
        Command::HealthcheckProvider { name } => json(svc.healthcheck_provider(name).await),
        Command::CloseConnection { id } => json(svc.close_connection(id).await),
        Command::CloseAllConnections => json(svc.close_all_connections().await),
        Command::Rules => json(svc.rules().await),
        Command::RuleProviders => json(svc.rule_providers().await),
        Command::UpdateRuleProvider { name } => json(svc.update_rule_provider(name).await),
        Command::DnsQuery { name, kind } => json(svc.dns_query(name, kind).await),
        Command::FlushFakeip => json(svc.flush_fakeip().await),
        Command::UpdateGeo => json(svc.update_geo().await),
        Command::GeoUpdated => json(Ok(geo_updated(&svc.paths().runtime_dir()))),
        Command::ClearLogs => {
            svc.clear_logs();
            done()
        }
        Command::ClearStats => {
            svc.clear_stats();
            done()
        }
        Command::SetRoute { kind, exit } => json(svc.set_route(kind, exit).await),
        Command::RemoveRoute { id } => json(svc.remove_route(id).await),
        Command::SetAiExit { name } => json(svc.set_ai_exit(name).await),
        Command::AiFollowLocation => json(svc.ai_follow_location().await),
        Command::DismissAiAlert => {
            svc.dismiss_ai_alert();
            done()
        }
        Command::UpdateAi { ai } => json(svc.update_ai(*ai).await),
        Command::CheckAi => json(svc.check_ai().await),
        Command::SetDownloadsExit { name } => json(svc.set_downloads_exit(name).await),
        Command::DownloadsFollowFastest => json(svc.downloads_follow_fastest().await),
        Command::TestMirror => json(svc.test_mirror().await),
        Command::SetMirror { on } => json(svc.set_mirror(on).await),
        Command::Icon { kind, name } => json(Ok(icon(svc, kind, &name))),
        Command::CheckForUpdate => json(svc.check_for_update().await),
        Command::DownloadUpdate => json(svc.download_update().await),
        Command::RestartToUpdate => json(svc.restart_to_update().await),
        Command::DismissUpdateFailure => {
            svc.dismiss_update_failure();
            done()
        }
        Command::NewSecret => {
            // Made and kept here; the page learns only that it changed.
            let mut next = svc.settings();
            next.secret = kairo_core::settings::new_secret();
            svc.update_settings(next).await?;
            json(Ok(HIDDEN))
        }
        Command::FreePorts { mixed, controller } => {
            let m = kairo_core::settings::free_port(mixed, &[]);
            let c = kairo_core::settings::free_port(controller, &[m]);
            json(Ok((m, c)))
        }
        Command::Sessions => json(Ok(ctx.auth.sessions(session))),
        Command::RevokeSession { id } => json(ctx.auth.revoke(&id)),
        Command::RevokeOtherSessions => {
            ctx.auth.revoke_others(session);
            done()
        }
        Command::SignOut => {
            ctx.auth.sign_out(session);
            done()
        }
    }
}

/// A config file the page read, imported as the open dialog's would be.
async fn add_yaml(svc: &Service, name: &str, text: String) -> Result<Value, String> {
    if text.len() > protocol_limit() {
        return Err("That file is too large for a config".into());
    }
    let dir = svc.paths().home.join("web").join("imports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(file_name(name));
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    let r = svc.add_file(path.clone()).await;
    let _ = std::fs::remove_file(&path);
    json(r)
}

/// The largest config a page may send.
pub fn protocol_limit() -> usize {
    8 * 1024 * 1024
}

/// A file name from a browser, as one plain name in our folder.
pub fn file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() || "-_. ".contains(c) { c } else { '_' })
        .collect();
    let clean = clean.trim_start_matches('.').trim();
    if clean.is_empty() {
        "config.yaml".into()
    } else {
        clean.chars().take(80).collect()
    }
}

/// What the page loads an app's or a site's picture from: the file's name
/// under `/icons/<kind>/`, served behind the session.
fn icon(svc: &Service, kind: IconKind, name: &str) -> IconInfo {
    let (path, label, folder) = match kind {
        IconKind::App => (svc.app_icon(name), svc.app_label(name), "apps"),
        IconKind::Site => (svc.site_icon(name), None, "sites"),
    };
    IconInfo {
        url: path
            .as_deref()
            .and_then(Path::file_name)
            .map(|f| format!("/icons/{folder}/{}", f.to_string_lossy())),
        label,
    }
}

/// When the geo databases were last written.
fn geo_updated(dir: &Path) -> Option<i64> {
    [kairo_core::geo::MMDB, kairo_core::geo::GEOSITE]
        .into_iter()
        .filter_map(|f| std::fs::metadata(dir.join(f.name)).ok()?.modified().ok())
        .max()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn browser_file_names_stay_in_the_folder() {
        assert_eq!(super::file_name("../../etc/passwd"), "passwd");
        assert_eq!(super::file_name("C:\\x\\sub.yaml"), "sub.yaml");
        assert_eq!(super::file_name(".bashrc"), "bashrc");
        assert_eq!(super::file_name(""), "config.yaml");
        assert_eq!(super::file_name("a/b/..."), "config.yaml");
    }
}
