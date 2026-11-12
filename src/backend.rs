//! What the pages talk to: the service in this process (the desktop and
//! phone apps), or one on another machine through `kairo-server` (the web).
//! The methods are the service's own, by name, so a page does not know which
//! it has; [`Remote`] keeps the service's streams as local `watch` channels,
//! fed by the server's messages.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use kairo_core::activity::Tally;
use kairo_core::ai::AiSettings;
use kairo_core::model::{Connections, DnsResponse, LogEntry, Rule, RuleProvider};
use kairo_core::paths::Paths;
use kairo_core::protocol::{
    ClientMsg, Command, Hello, IconInfo, IconKind, ServerMsg, SessionInfo,
};
use kairo_core::routes::{RouteExit, RouteKind};
use kairo_core::stats::Store as Stats;
use kairo_core::tokio::sync::{oneshot, watch};
use kairo_core::{Live, Mode, Proxies, Settings, State};
use serde::de::DeserializeOwned;

/// A command's answer, awaited on the UI's thread.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>>>>;

#[derive(Clone)]
pub enum Backend {
    /// The service in this process.
    #[cfg(not(target_arch = "wasm32"))]
    Local(kairo_core::Service),
    /// A service on another machine.
    Remote(Remote),
}

#[cfg(not(target_arch = "wasm32"))]
fn boxed<T: 'static>(f: impl Future<Output = Result<T, String>> + 'static) -> Fut<T> {
    Box::pin(f)
}

/// Dispatches to the local service or the remote one: `$local` is a
/// call on the `Service` (bound to `s`), `$cmd` the [`Command`] for it.
macro_rules! call {
    ($self:ident, |$s:ident| $local:expr, $cmd:expr) => {
        match $self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local($s) => boxed($local),
            Backend::Remote(r) => r.call($cmd),
        }
    };
}

/// The same for commands with no answer worth waiting for.
macro_rules! tell {
    ($self:ident, |$s:ident| $local:expr, $cmd:expr) => {
        match $self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local($s) => $local,
            Backend::Remote(r) => r.tell($cmd),
        }
    };
}

impl Backend {
    /// The service of the desktop and phone apps.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn local() -> Backend {
        Backend::Local(kairo_core::Service::global())
    }

    /// Whether this is the page of a service on another machine.
    pub fn is_remote(&self) -> bool {
        matches!(self, Backend::Remote(_))
    }

    /// The machine the switches change, as a person says it: "this Mac",
    /// or the server's name.
    pub fn machine(&self) -> Option<String> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => None,
            Backend::Remote(r) => Some(r.hello().machine),
        }
    }

    /// Kairo's version where the service runs.
    pub fn version(&self) -> String {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => env!("CARGO_PKG_VERSION").to_owned(),
            Backend::Remote(r) => r.hello().version,
        }
    }

    /// The system the service runs on (`linux x86_64`).
    pub fn os_text(&self) -> String {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            Backend::Remote(r) => r.hello().os,
        }
    }

    /// The home folder of the account the service runs as, when known (for
    /// writing paths under it as `~`).
    pub fn user_home(&self) -> Option<String> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => std::env::var("HOME").ok(),
            Backend::Remote(_) => None,
        }
    }

    // --- Streams -------------------------------------------------------------------

    pub fn state(&self) -> watch::Receiver<State> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.state(),
            Backend::Remote(r) => r.0.state.subscribe(),
        }
    }

    pub fn live(&self) -> watch::Receiver<Live> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.live(),
            Backend::Remote(r) => r.0.live.subscribe(),
        }
    }

    pub fn connections(&self) -> watch::Receiver<Arc<Connections>> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.connections(),
            Backend::Remote(r) => r.0.connections.subscribe(),
        }
    }

    pub fn proxies(&self) -> watch::Receiver<Arc<Proxies>> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.proxies(),
            Backend::Remote(r) => r.0.proxies.subscribe(),
        }
    }

    pub fn activity(&self) -> watch::Receiver<Arc<Tally>> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.activity(),
            Backend::Remote(r) => r.0.activity.subscribe(),
        }
    }

    pub fn stats(&self) -> watch::Receiver<Arc<Stats>> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.stats(),
            Backend::Remote(r) => r.0.stats.subscribe(),
        }
    }

    pub fn log_seq(&self) -> watch::Receiver<u64> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.log_seq(),
            Backend::Remote(r) => r.0.log_seq.subscribe(),
        }
    }

    pub fn icons_rev(&self) -> watch::Receiver<u64> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.icons_rev(),
            Backend::Remote(r) => r.0.icons_rev.subscribe(),
        }
    }

    // --- What is known now -------------------------------------------------------------

    pub fn snapshot(&self) -> State {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.snapshot(),
            Backend::Remote(r) => r.0.state.borrow().clone(),
        }
    }

    pub fn settings(&self) -> Settings {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.settings(),
            Backend::Remote(r) => r.0.state.borrow().settings.clone(),
        }
    }

    pub fn paths(&self) -> Paths {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.paths().clone(),
            Backend::Remote(r) => Paths {
                home: PathBuf::from(r.hello().home),
                bundled_core: None,
            },
        }
    }

    pub fn logs(&self) -> Vec<LogEntry> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.logs(),
            Backend::Remote(r) => r.0.logs.borrow().iter().cloned().collect(),
        }
    }

    /// The command that grants TUN where the service runs, when a page
    /// can not ask for it (a server's).
    pub fn tun_grant(&self) -> Option<String> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => None,
            Backend::Remote(r) => r.hello().tun_grant,
        }
    }

    pub fn terminal_env(&self) -> String {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.terminal_env(),
            Backend::Remote(r) => r.hello().terminal_env,
        }
    }

    /// The node AI goes through now.
    pub fn ai_node(&self) -> Option<String> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.ai_node(),
            Backend::Remote(r) => {
                let state = r.0.state.borrow();
                let group = state.ai.group.clone();
                if group.is_empty() {
                    return None;
                }
                let proxies = r.0.proxies.borrow();
                kairo_core::places::main_group(&proxies.groups, &state.settings.tray_group)?;
                kairo_core::ai::resolve(&proxies.groups, &group)
            }
        }
    }

    /// A site's picture: a file here, or where the page loads it from.
    pub fn site_icon(&self, domain: &str) -> Option<PathBuf> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.site_icon(domain),
            Backend::Remote(r) => r.icon(IconKind::Site, domain).url.map(PathBuf::from),
        }
    }

    pub fn app_icon(&self, name: &str) -> Option<PathBuf> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.app_icon(name),
            Backend::Remote(r) => r.icon(IconKind::App, name).url.map(PathBuf::from),
        }
    }

    pub fn app_label(&self, name: &str) -> Option<String> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.app_label(name),
            Backend::Remote(r) => r.icon(IconKind::App, name).label,
        }
    }

    // --- Commands --------------------------------------------------------------------

    pub fn connect(&self, on: bool) -> Fut<()> {
        call!(self, |s| s.connect(on), Command::Connect { on })
    }

    pub fn start_core(&self) -> Fut<()> {
        call!(self, |s| s.start_core(), Command::StartCore)
    }

    pub fn stop_core(&self) -> Pin<Box<dyn Future<Output = ()>>> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => Box::pin(s.stop_core()),
            Backend::Remote(r) => {
                let f = r.call::<()>(Command::StopCore);
                Box::pin(async move {
                    let _ = f.await;
                })
            }
        }
    }

    pub fn restart_core(&self) -> Fut<()> {
        call!(self, |s| s.restart_core(), Command::RestartCore)
    }

    pub fn set_mode(&self, mode: Mode) -> Fut<()> {
        call!(self, |s| s.set_mode(mode), Command::SetMode { mode })
    }

    pub fn set_location(&self, name: Option<String>) -> Fut<()> {
        call!(self, |s| s.set_location(name.clone()), Command::SetLocation { name })
    }

    pub fn test_places(&self) -> Fut<()> {
        call!(self, |s| s.test_places(), Command::TestPlaces)
    }

    pub fn update_settings(&self, next: Settings) -> Fut<()> {
        call!(
            self,
            |s| s.update_settings(next.clone()),
            Command::UpdateSettings { settings: Box::new(next) }
        )
    }

    pub fn set_system_proxy(&self, on: bool) -> Fut<()> {
        call!(self, |s| s.set_system_proxy(on), Command::SetSystemProxy { on })
    }

    pub fn set_tun(&self, on: bool) -> Fut<()> {
        call!(self, |s| s.set_tun(on), Command::SetTun { on })
    }

    pub fn grant_tun(&self) -> Fut<()> {
        call!(self, |s| s.grant_tun(), Command::GrantTun)
    }

    pub fn add_remote(&self, url: String, name: Option<String>) -> Fut<String> {
        call!(
            self,
            |s| s.add_remote(url.clone(), name.clone()),
            Command::AddRemote { url, name }
        )
    }

    /// A config file on this machine (the desktops' open dialog).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn add_file(&self, path: PathBuf) -> Fut<String> {
        match self {
            Backend::Local(s) => boxed(s.add_file(path)),
            Backend::Remote(r) => match std::fs::read_to_string(&path) {
                Ok(text) => r.call(Command::AddYaml {
                    name: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    text,
                }),
                Err(e) => Box::pin(std::future::ready(Err(e.to_string()))),
            },
        }
    }

    /// A config the page read from the person's device (the web's file picker).
    pub fn add_yaml(&self, name: String, text: String) -> Fut<String> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => {
                // Through a file, as the open dialog gives it.
                let dir = s.paths().home.join("imports");
                let path = dir.join(sanitize_file_name(&name));
                let svc = s.clone();
                boxed(async move {
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    std::fs::write(&path, text).map_err(|e| e.to_string())?;
                    let r = svc.add_file(path.clone()).await;
                    let _ = std::fs::remove_file(&path);
                    r
                })
            }
            Backend::Remote(r) => r.call(Command::AddYaml { name, text }),
        }
    }

    pub fn update_profile(&self, id: String) -> Fut<()> {
        call!(self, |s| s.update_profile(id.clone()), Command::UpdateProfile { id })
    }

    pub fn update_all_profiles(&self) -> Fut<()> {
        call!(self, |s| s.update_all_profiles(), Command::UpdateAllProfiles)
    }

    pub fn activate(&self, id: String) -> Fut<()> {
        call!(self, |s| s.activate(id.clone()), Command::Activate { id })
    }

    pub fn rename_profile(
        &self,
        id: String,
        name: String,
        interval_hours: Option<u32>,
        url: Option<String>,
    ) {
        tell!(
            self,
            |s| s.rename_profile(id, name, interval_hours, url),
            Command::RenameProfile { id, name, interval_hours, url }
        )
    }

    pub fn delete_profile(&self, id: String) -> Fut<()> {
        call!(self, |s| s.delete_profile(id.clone()), Command::DeleteProfile { id })
    }

    pub fn dismiss_failure(&self, id: &str) {
        tell!(self, |s| s.dismiss_failure(id), Command::DismissFailure { id: id.to_owned() })
    }

    pub fn refresh_proxies(&self) -> Fut<()> {
        call!(self, |s| s.refresh_proxies(), Command::RefreshProxies)
    }

    pub fn select(&self, group: String, name: String) -> Fut<()> {
        call!(
            self,
            |s| s.select(group.clone(), name.clone()),
            Command::Select { group, name }
        )
    }

    pub fn test_node(&self, name: String, url: Option<String>) -> Fut<u32> {
        call!(
            self,
            |s| s.test_node(name.clone(), url.clone()),
            Command::TestNode { name, url }
        )
    }

    pub fn test_group(&self, group: String) -> Fut<()> {
        call!(self, |s| s.test_group(group.clone()), Command::TestGroup { group })
    }

    pub fn update_provider(&self, name: String) -> Fut<()> {
        call!(self, |s| s.update_provider(name.clone()), Command::UpdateProvider { name })
    }

    pub fn healthcheck_provider(&self, name: String) -> Fut<()> {
        call!(
            self,
            |s| s.healthcheck_provider(name.clone()),
            Command::HealthcheckProvider { name }
        )
    }

    pub fn close_connection(&self, id: String) -> Fut<()> {
        call!(self, |s| s.close_connection(id.clone()), Command::CloseConnection { id })
    }

    pub fn close_all_connections(&self) -> Fut<()> {
        call!(self, |s| s.close_all_connections(), Command::CloseAllConnections)
    }

    pub fn rules(&self) -> Fut<Vec<Rule>> {
        call!(self, |s| s.rules(), Command::Rules)
    }

    pub fn rule_providers(&self) -> Fut<Vec<RuleProvider>> {
        call!(self, |s| s.rule_providers(), Command::RuleProviders)
    }

    pub fn update_rule_provider(&self, name: String) -> Fut<()> {
        call!(
            self,
            |s| s.update_rule_provider(name.clone()),
            Command::UpdateRuleProvider { name }
        )
    }

    pub fn dns_query(&self, name: String, kind: String) -> Fut<DnsResponse> {
        call!(
            self,
            |s| s.dns_query(name.clone(), kind.clone()),
            Command::DnsQuery { name, kind }
        )
    }

    pub fn flush_fakeip(&self) -> Fut<()> {
        call!(self, |s| s.flush_fakeip(), Command::FlushFakeip)
    }

    pub fn update_geo(&self) -> Fut<()> {
        call!(self, |s| s.update_geo(), Command::UpdateGeo)
    }

    /// When the geo databases were last written (unix seconds).
    pub fn geo_updated(&self) -> Fut<Option<i64>> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => {
                let at = geo_mtime(s.paths());
                Box::pin(std::future::ready(Ok(at)))
            }
            Backend::Remote(r) => r.call(Command::GeoUpdated),
        }
    }

    pub fn clear_logs(&self) {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(s) => s.clear_logs(),
            Backend::Remote(r) => r.tell(Command::ClearLogs),
        }
    }

    pub fn clear_stats(&self) {
        tell!(self, |s| s.clear_stats(), Command::ClearStats)
    }

    pub fn set_route(&self, kind: RouteKind, exit: RouteExit) -> Fut<()> {
        call!(
            self,
            |s| s.set_route(kind.clone(), exit.clone()),
            Command::SetRoute { kind, exit }
        )
    }

    pub fn remove_route(&self, id: String) -> Fut<()> {
        call!(self, |s| s.remove_route(id.clone()), Command::RemoveRoute { id })
    }

    pub fn set_ai_exit(&self, name: String) -> Fut<()> {
        call!(self, |s| s.set_ai_exit(name.clone()), Command::SetAiExit { name })
    }

    pub fn ai_follow_location(&self) -> Fut<()> {
        call!(self, |s| s.ai_follow_location(), Command::AiFollowLocation)
    }

    pub fn dismiss_ai_alert(&self) {
        tell!(self, |s| s.dismiss_ai_alert(), Command::DismissAiAlert)
    }

    pub fn update_ai(&self, next: AiSettings) -> Fut<()> {
        call!(
            self,
            |s| s.update_ai(next.clone()),
            Command::UpdateAi { ai: Box::new(next) }
        )
    }

    pub fn check_ai(&self) -> Fut<()> {
        call!(self, |s| s.check_ai(), Command::CheckAi)
    }

    pub fn set_downloads_exit(&self, name: String) -> Fut<()> {
        call!(
            self,
            |s| s.set_downloads_exit(name.clone()),
            Command::SetDownloadsExit { name }
        )
    }

    pub fn downloads_follow_fastest(&self) -> Fut<()> {
        call!(self, |s| s.downloads_follow_fastest(), Command::DownloadsFollowFastest)
    }

    pub fn test_mirror(&self) -> Fut<()> {
        call!(self, |s| s.test_mirror(), Command::TestMirror)
    }

    pub fn set_mirror(&self, on: bool) -> Fut<()> {
        call!(self, |s| s.set_mirror(on), Command::SetMirror { on })
    }

    pub fn check_for_update(&self) -> Fut<Option<String>> {
        call!(self, |s| s.check_for_update(), Command::CheckForUpdate)
    }

    pub fn download_update(&self) -> Fut<()> {
        call!(self, |s| s.download_update(), Command::DownloadUpdate)
    }

    pub fn restart_to_update(&self) -> Fut<()> {
        call!(self, |s| s.restart_to_update(), Command::RestartToUpdate)
    }

    pub fn dismiss_update_failure(&self) {
        tell!(self, |s| s.dismiss_update_failure(), Command::DismissUpdateFailure)
    }

    /// A new controller secret. Here it is made here; a server makes its own
    /// and the page only learns that it changed.
    pub fn new_secret(&self) -> Fut<String> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => Box::pin(std::future::ready(Ok(kairo_core::settings::new_secret()))),
            Backend::Remote(r) => r.call(Command::NewSecret),
        }
    }

    /// Free ports for the core, from these on (both differ).
    pub fn free_ports(&self, mixed: u16, controller: u16) -> Fut<(u16, u16)> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => {
                let m = kairo_core::settings::free_port(mixed, &[]);
                let c = kairo_core::settings::free_port(controller, &[m]);
                Box::pin(std::future::ready(Ok((m, c))))
            }
            Backend::Remote(r) => r.call(Command::FreePorts { mixed, controller }),
        }
    }

    // --- The web's own -------------------------------------------------------------------

    pub fn sessions(&self) -> Fut<Vec<SessionInfo>> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => Box::pin(std::future::ready(Ok(Vec::new()))),
            Backend::Remote(r) => r.call(Command::Sessions),
        }
    }

    pub fn revoke_session(&self, id: String) -> Fut<()> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => Box::pin(std::future::ready(Ok(()))),
            Backend::Remote(r) => r.call(Command::RevokeSession { id }),
        }
    }

    pub fn revoke_other_sessions(&self) -> Fut<()> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => Box::pin(std::future::ready(Ok(()))),
            Backend::Remote(r) => r.call(Command::RevokeOtherSessions),
        }
    }

    /// Signs this browser out (and forgets its cookie).
    pub fn sign_out(&self) -> Fut<()> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Local(_) => Box::pin(std::future::ready(Ok(()))),
            #[cfg(target_arch = "wasm32")]
            Backend::Remote(_) => Box::pin(crate::web::sign_out()),
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Remote(r) => r.call(Command::SignOut),
        }
    }

    /// The local service, for what only the desktop does (its menus, the update's restart).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn local_service(&self) -> Option<&kairo_core::Service> {
        match self {
            Backend::Local(s) => Some(s),
            Backend::Remote(_) => None,
        }
    }
}

/// When the newest geo file was written (unix seconds).
#[cfg(not(target_arch = "wasm32"))]
fn geo_mtime(paths: &Paths) -> Option<i64> {
    [kairo_core::geo::MMDB, kairo_core::geo::GEOSITE]
        .into_iter()
        .filter_map(|f| {
            std::fs::metadata(paths.runtime_dir().join(f.name))
                .ok()?
                .modified()
                .ok()
        })
        .max()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// A file name from a browser, made one plain name (no folders, no dots first).
pub fn sanitize_file_name(name: &str) -> String {
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

// --- Remote ------------------------------------------------------------------------------

/// A service on another machine: the server's messages in ([`Remote::receive`]),
/// commands out through `send`.
#[derive(Clone)]
pub struct Remote(Rc<Inner>);

pub struct Inner {
    send: RefCell<Option<Box<dyn Fn(String)>>>,
    next_id: Cell<u64>,
    pending: RefCell<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>,
    hello: RefCell<Option<Hello>>,
    state: watch::Sender<State>,
    live: watch::Sender<Live>,
    connections: watch::Sender<Arc<Connections>>,
    proxies: watch::Sender<Arc<Proxies>>,
    activity: watch::Sender<Arc<Tally>>,
    stats: watch::Sender<Arc<Stats>>,
    log_seq: watch::Sender<u64>,
    logs: RefCell<VecDeque<LogEntry>>,
    icons_rev: watch::Sender<u64>,
    icons: RefCell<HashMap<(IconKind, String), Option<IconInfo>>>,
    /// The link to the server is up.
    online: watch::Sender<bool>,
    /// The server ended this session.
    signed_out: watch::Sender<bool>,
    /// The server has sent everything once (after each connect).
    synced: watch::Sender<bool>,
}

/// The most lines of the core's log a page keeps.
const LOG_LINES: usize = 3000;

impl Default for Remote {
    fn default() -> Self {
        Remote::new()
    }
}

impl Remote {
    pub fn new() -> Remote {
        Remote(Rc::new(Inner {
            send: RefCell::new(None),
            next_id: Cell::new(1),
            pending: RefCell::new(HashMap::new()),
            hello: RefCell::new(None),
            state: watch::Sender::new(State::default()),
            live: watch::Sender::new(Live::default()),
            connections: watch::Sender::new(Arc::default()),
            proxies: watch::Sender::new(Arc::default()),
            activity: watch::Sender::new(Arc::default()),
            stats: watch::Sender::new(Arc::default()),
            log_seq: watch::Sender::new(0),
            logs: RefCell::new(VecDeque::new()),
            icons_rev: watch::Sender::new(0),
            icons: RefCell::new(HashMap::new()),
            online: watch::Sender::new(false),
            signed_out: watch::Sender::new(false),
            synced: watch::Sender::new(false),
        }))
    }

    /// Where commands go from now on (a socket that is open); `None`: nowhere
    /// (the socket closed), and what waits for an answer fails.
    pub fn set_link(&self, send: Option<Box<dyn Fn(String)>>) {
        let up = send.is_some();
        *self.0.send.borrow_mut() = send;
        if !up {
            for (_, tx) in self.0.pending.borrow_mut().drain() {
                let _ = tx.send(Err("Lost the connection to Kairo".into()));
            }
        }
        self.0.online.send_replace(up);
    }

    pub fn hello(&self) -> Hello {
        self.0.hello.borrow().clone().unwrap_or_else(|| Hello {
            protocol: kairo_core::protocol::VERSION,
            version: String::new(),
            machine: String::new(),
            os: String::new(),
            home: String::new(),
            offset: 0,
            terminal_env: String::new(),
            tun_grant: None,
        })
    }

    /// Whether the link to the server is up, and its changes.
    pub fn online(&self) -> watch::Receiver<bool> {
        self.0.online.subscribe()
    }

    /// Whether the server ended this session.
    pub fn signed_out(&self) -> watch::Receiver<bool> {
        self.0.signed_out.subscribe()
    }

    pub fn mark_signed_out(&self) {
        self.0.signed_out.send_replace(true);
    }

    pub fn has_hello(&self) -> bool {
        self.0.hello.borrow().is_some()
    }

    /// Whether the server has sent everything once, and its changes.
    pub fn synced(&self) -> watch::Receiver<bool> {
        self.0.synced.subscribe()
    }

    fn send(&self, cmd: Command) -> Option<u64> {
        let id = self.0.next_id.get();
        self.0.next_id.set(id + 1);
        let text = serde_json::to_string(&ClientMsg { id, cmd }).ok()?;
        match &*self.0.send.borrow() {
            Some(send) => {
                send(text);
                Some(id)
            }
            None => None,
        }
    }

    /// Sends `cmd` and gives its answer.
    pub fn call<T: DeserializeOwned + 'static>(&self, cmd: Command) -> Fut<T> {
        let (tx, rx) = oneshot::channel();
        match self.send(cmd) {
            Some(id) => {
                self.0.pending.borrow_mut().insert(id, tx);
            }
            None => {
                let _ = tx.send(Err("Not connected to Kairo".into()));
            }
        }
        Box::pin(async move {
            let value = rx.await.map_err(|_| "Lost the connection to Kairo".to_owned())??;
            serde_json::from_value(value).map_err(|e| format!("Unexpected answer: {e}"))
        })
    }

    /// Sends `cmd`; a failure shows as the state's notice, as it does here.
    pub fn tell(&self, cmd: Command) {
        let _ = self.send(cmd);
    }

    /// What is known of a picture; the first time, the server is asked (its
    /// answer bumps `icons_rev`, so the page looks again).
    fn icon(&self, kind: IconKind, name: &str) -> IconInfo {
        let key = (kind, name.to_owned());
        if let Some(known) = self.0.icons.borrow().get(&key) {
            return known.clone().unwrap_or_default();
        }
        self.0.icons.borrow_mut().insert(key.clone(), None);
        let answer = self.call::<IconInfo>(Command::Icon {
            kind,
            name: name.to_owned(),
        });
        let me = self.clone();
        // Under no owner: the row that asked may be made again before the
        // answer comes, and the answer must still be kept.
        nsh_core::prelude::Owner::detached().run(|| nsh_core::prelude::spawn(async move {
            if let Ok(info) = answer.await {
                let some = info.url.is_some() || info.label.is_some();
                me.0.icons.borrow_mut().insert(key, Some(info));
                if some {
                    me.0.icons_rev.send_modify(|n| *n += 1);
                }
            } else {
                me.0.icons.borrow_mut().remove(&key);
            }
        }));
        IconInfo::default()
    }

    /// One message of the server.
    pub fn receive(&self, text: &str) -> Result<(), String> {
        let msg: ServerMsg = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let me = &self.0;
        match msg {
            ServerMsg::Hello(hello) => {
                #[cfg(target_arch = "wasm32")]
                kairo_core::time::set_remote_offset(hello.offset);
                *me.hello.borrow_mut() = Some(*hello);
            }
            ServerMsg::Reply { id, ok, err } => {
                if let Some(tx) = me.pending.borrow_mut().remove(&id) {
                    let _ = tx.send(match err {
                        Some(e) => Err(e),
                        None => Ok(ok.unwrap_or(serde_json::Value::Null)),
                    });
                }
            }
            ServerMsg::State(s) => {
                me.state.send_replace(*s);
            }
            ServerMsg::Live(l) => {
                me.live.send_replace(*l);
            }
            ServerMsg::Connections(c) => {
                me.connections.send_replace(c);
            }
            ServerMsg::Proxies(p) => {
                me.proxies.send_replace(p);
            }
            ServerMsg::Activity(a) => {
                me.activity.send_replace(a);
            }
            ServerMsg::Stats(s) => {
                me.stats.send_replace(s);
            }
            ServerMsg::Logs { reset, lines, seq } => {
                let mut logs = me.logs.borrow_mut();
                if reset {
                    logs.clear();
                }
                logs.extend(lines);
                while logs.len() > LOG_LINES {
                    logs.pop_front();
                }
                drop(logs);
                me.log_seq.send_replace(seq);
            }
            ServerMsg::IconsRev(n) => {
                // Pictures the server had not got may be there now.
                me.icons
                    .borrow_mut()
                    .retain(|_, v| v.as_ref().is_some_and(|i| i.url.is_some()));
                // Read, then written: never both at once (the lock is not re-entrant).
                let next = n.max(*me.icons_rev.borrow() + 1);
                me.icons_rev.send_replace(next);
            }
            ServerMsg::Synced => {
                me.synced.send_replace(true);
            }
            ServerMsg::SignedOut => {
                me.signed_out.send_replace(true);
            }
        }
        Ok(())
    }
}
