//! The service: one per process, on its own Tokio runtime. It owns the core
//! process, the settings and profiles, the live streams and the system proxy.
//! The UI and the tray both talk to it: commands in (async methods), state out
//! (`watch` channels the UI turns into signals).

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

mod ai_ops;
mod bypass_ops;
mod downloads_ops;
mod route_ops;
mod stats_ops;

use crate::activity::Tally;
use crate::ai_check;
use crate::config;
use crate::controller::{ApiError, Controller};
use crate::core_bin;
use crate::model::*;
use crate::paths::Paths;
use crate::profile::{self, Profile, ProfileIndex, Source};
use crate::settings::{Mode, Settings};
use crate::supervisor::CoreProcess;
use crate::sysproxy::{self, ProxySpec};
use crate::tun::{self, Privilege};

/// The core's state.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum CoreStatus {
    #[default]
    Stopped,
    Starting,
    Running {
        version: String,
    },
    Stopping,
    /// It stopped by itself, or could not start: why, in the core's words.
    Failed {
        message: String,
    },
}

impl CoreStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, CoreStatus::Running { .. })
    }

    pub fn title(&self) -> &'static str {
        match self {
            CoreStatus::Stopped => "Stopped",
            CoreStatus::Starting => "Starting",
            CoreStatus::Running { .. } => "Running",
            CoreStatus::Stopping => "Stopping",
            CoreStatus::Failed { .. } => "Stopped with an error",
        }
    }

    /// The state as a person says it (the core runs: connected).
    pub fn plain(&self) -> &'static str {
        match self {
            CoreStatus::Stopped => "Not Connected",
            CoreStatus::Starting => "Connecting",
            CoreStatus::Running { .. } => "Connected",
            CoreStatus::Stopping => "Disconnecting",
            CoreStatus::Failed { .. } => "Couldn't Connect",
        }
    }
}

/// A message for the user, shown once.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub id: u64,
    pub text: String,
    pub error: bool,
}

/// Work in progress that the UI shows (a download).
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub label: String,
    /// 0..=1 when known.
    pub progress: Option<f32>,
}

/// What the AI layer knows: where AI goes and how the checks stand.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AiView {
    /// The group AI traffic goes to, once the config has it.
    pub group: String,
    /// The subscription's own AI group, not one of Kairo's.
    pub adopted: bool,
    /// The last check of each service, in the order they are shown.
    pub checks: Vec<ai_check::Check>,
    /// The last 90 s hold of each service (by id).
    pub holds: Vec<(String, ai_check::Hold)>,
    /// A check is running.
    pub checking: bool,
    /// Something the person should know about the exit.
    pub alert: Option<AiAlert>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AiAlert {
    /// The pinned exit is no longer in the subscription; AI follows the
    /// Location until one is chosen.
    ExitGone { was: String },
}

/// Everything that changes rarely.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub core: CoreStatus,
    /// The core's version.
    pub core_version: Option<String>,
    /// Kairo's own update, when there is one.
    pub update: crate::update::UpdateStatus,
    pub settings: Settings,
    pub profiles: ProfileIndex,
    /// Profiles being downloaded.
    pub updating: HashSet<String>,
    /// Whether the system proxy currently points at Kairo.
    pub sysproxy_active: bool,
    pub sysproxy_supported: bool,
    pub tun_privilege: Option<Privilege>,
    pub task: Option<Task>,
    pub notice: Option<Notice>,
    /// A problem the core reported at start that it survives (a port in use).
    pub warning: Option<String>,
    /// Something done for the user that they may want to know (Kairo moved
    /// to a free port); shown quietly while connected.
    pub note: Option<String>,
    /// Kairo is finding the first place of a new subscription (or replacing
    /// a provider's note in use); later checks happen without saying so.
    pub locating: bool,
    pub ai: AiView,
    /// The Downloads exit: where it goes, the mirror and its test.
    pub downloads: crate::downloads::DownloadsInfo,
    /// Routes (ids) the running config leaves out because their exit is not
    /// in it (a node the subscription no longer has).
    pub routes_skipped: Vec<String>,
    /// AI tools whose connections go around the proxy (system-proxy mode
    /// only; empty with TUN, or when the core is not running).
    pub bypass: Vec<crate::bypass::Bypass>,
}

impl State {
    /// The connection as a person says it: nothing to connect through yet
    /// is "Not Set Up", whatever the core does meanwhile.
    pub fn plain(&self) -> &'static str {
        if self.profiles.active.is_none() {
            "Not Set Up"
        } else {
            self.core.plain()
        }
    }
}

/// The live numbers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Live {
    pub up: u64,
    pub down: u64,
    pub upload_total: u64,
    pub download_total: u64,
    pub memory: u64,
    pub connections: usize,
    /// The last 90 seconds, oldest first.
    pub history: VecDeque<Traffic>,
}

pub const HISTORY: usize = 90;

/// Groups and providers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Proxies {
    pub groups: Vec<Group>,
    pub providers: Vec<ProxyProvider>,
    /// Nodes and groups being tested.
    pub testing: HashSet<String>,
}

const LOG_CAPACITY: usize = 3000;

/// The config before the first profile: no nodes, everything direct.
const NO_PROFILE: &str = "proxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n";

enum CoreCmd {
    Start(oneshot::Sender<Result<(), String>>),
    Stop(oneshot::Sender<()>),
    Restart(oneshot::Sender<Result<(), String>>),
}

struct Inner {
    paths: Paths,
    rt: tokio::runtime::Handle,
    state: watch::Sender<State>,
    live: watch::Sender<Live>,
    connections: watch::Sender<Arc<Connections>>,
    proxies: watch::Sender<Arc<Proxies>>,
    logs: Mutex<VecDeque<LogEntry>>,
    log_seq: watch::Sender<u64>,
    core_tx: mpsc::UnboundedSender<CoreCmd>,
    notice_seq: Mutex<u64>,
    /// Serializes config applies.
    apply_lock: tokio::sync::Mutex<()>,
    /// One geo data download at a time (the boot prefetch, a connect).
    geo_lock: tokio::sync::Mutex<()>,
    /// One update check or install at a time.
    update_lock: tokio::sync::Mutex<()>,
    /// A subscription was just added: its first place is still to be found.
    fresh: std::sync::atomic::AtomicBool,
    /// Which tools talk to which AI services (counts since Kairo opened).
    activity: watch::Sender<Arc<Tally>>,
    /// What is recorded for the statistics pages, and the copy they read.
    stats: Mutex<stats_ops::Recorder>,
    stats_tx: watch::Sender<Arc<crate::stats::Store>>,
    /// The running config as a mapping, for what the AI layer reads of it.
    ai_cfg: Mutex<Option<serde_yaml::Mapping>>,
    /// One AI check at a time, and when the last 90 s hold ran (unix s).
    ai_busy: std::sync::atomic::AtomicBool,
    ai_hold_at: std::sync::atomic::AtomicI64,
    ai_checked_at: std::sync::atomic::AtomicI64,
    /// Process names of local connections the core came back without a name
    /// for (macOS), and whether a look for them is running.
    proc_names: Mutex<crate::procname::Names>,
    proc_busy: std::sync::atomic::AtomicBool,
    /// A mirror test is running.
    mirror_busy: std::sync::atomic::AtomicBool,
    /// Ends the app (the launcher's Quit), for "Restart to Update".
    quit: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// A handle to the service (cheap to clone).
#[derive(Clone)]
pub struct Service(Arc<Inner>);

static GLOBAL: OnceLock<Service> = OnceLock::new();

/// Awaits futures together.
async fn futures_join<T>(fs: Vec<impl Future<Output = T>>) -> Vec<T> {
    let handles: Vec<_> = fs.into_iter().map(|f| Box::pin(f)).collect();
    let mut out = Vec::new();
    // Each is already running on the runtime (`run`); awaiting in order is fine.
    for h in handles {
        out.push(h.await);
    }
    out
}

/// Turns a task's failure into the user's words.
fn joined<T>(r: Result<Result<T, String>, tokio::task::JoinError>) -> Result<T, String> {
    r.unwrap_or_else(|e| Err(format!("internal error: {e}")))
}

impl Service {
    /// The process' service, started on first use at the default location.
    pub fn global() -> Service {
        GLOBAL
            .get_or_init(|| Service::start(Paths::new(Paths::default_home())))
            .clone()
    }

    /// The process' service if it was started.
    pub fn try_global() -> Option<Service> {
        GLOBAL.get().cloned()
    }

    /// Starts a service at `paths` on its own runtime thread.
    pub fn start(paths: Paths) -> Service {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("kairo-service".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("kairo-rt")
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                let handle = rt.handle().clone();
                tx.send(handle).expect("service start");
                // Keep the runtime alive for the process' life.
                rt.block_on(std::future::pending::<()>());
            })
            .expect("service thread");
        let rt = rx.recv().expect("service runtime");
        let _guard = rt.enter();
        if let Err(e) = paths.ensure() {
            log::error!("cannot create {}: {e}", paths.home.display());
        }
        sysproxy::recover(&paths);
        let settings = Settings::load(&paths);
        let profiles = ProfileIndex::load(&paths);
        let (core_tx, core_rx) = mpsc::unbounded_channel();
        let state = State {
            downloads: crate::downloads::DownloadsInfo {
                fastest: settings.downloads.fastest,
                mirror: settings.downloads.mirror,
                ..Default::default()
            },
            settings,
            profiles,
            sysproxy_supported: sysproxy::supported(),
            ..State::default()
        };
        let svc = Service(Arc::new(Inner {
            paths,
            rt: rt.clone(),
            state: watch::Sender::new(state),
            live: watch::Sender::new(Live::default()),
            connections: watch::Sender::new(Arc::new(Connections::default())),
            proxies: watch::Sender::new(Arc::new(Proxies::default())),
            logs: Mutex::new(VecDeque::with_capacity(LOG_CAPACITY)),
            log_seq: watch::Sender::new(0),
            core_tx,
            notice_seq: Mutex::new(0),
            apply_lock: tokio::sync::Mutex::new(()),
            geo_lock: tokio::sync::Mutex::new(()),
            update_lock: tokio::sync::Mutex::new(()),
            quit: Mutex::new(None),
            fresh: std::sync::atomic::AtomicBool::new(false),
            activity: watch::Sender::new(Arc::new(Tally::default())),
            stats: Mutex::new(stats_ops::Recorder::new(crate::stats::Store::default())),
            stats_tx: watch::Sender::new(Arc::new(crate::stats::Store::default())),
            ai_cfg: Mutex::new(None),
            ai_busy: std::sync::atomic::AtomicBool::new(false),
            ai_hold_at: std::sync::atomic::AtomicI64::new(0),
            ai_checked_at: std::sync::atomic::AtomicI64::new(0),
            proc_names: Mutex::new(crate::procname::Names::default()),
            proc_busy: std::sync::atomic::AtomicBool::new(false),
            mirror_busy: std::sync::atomic::AtomicBool::new(false),
        }));
        let actor = svc.clone();
        rt.spawn(async move { actor.core_actor(core_rx).await });
        let boot = svc.clone();
        rt.spawn(async move { boot.boot().await });
        let sched = svc.clone();
        rt.spawn(async move { sched.scheduler().await });
        svc.stats_load();
        let rec = svc.clone();
        rt.spawn(async move { rec.stats_task().await });
        let places = svc.clone();
        rt.spawn(async move { places.location_loop().await });
        let checks = svc.clone();
        rt.spawn(async move { checks.ai_loop().await });
        let bypass = svc.clone();
        rt.spawn(async move { bypass.bypass_loop().await });
        svc
    }

    pub fn paths(&self) -> &Paths {
        &self.0.paths
    }

    /// The service's runtime, for work that belongs with it (the tray).
    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.0.rt.clone()
    }

    /// Runs a command's future on the service runtime and reports a failure to
    /// the user (for callers with no place to show it: the tray).
    pub fn fire<T: Send + 'static>(
        &self,
        f: impl Future<Output = Result<T, String>> + Send + 'static,
    ) {
        let svc = self.clone();
        self.0.rt.spawn(async move {
            if let Err(e) = f.await {
                svc.notify(e, true);
            }
        });
    }

    /// Runs `f` on the service's runtime; the future resolves on any executor.
    fn run<T: Send + 'static>(
        &self,
        f: impl Future<Output = Result<T, String>> + Send + 'static,
    ) -> impl Future<Output = Result<T, String>> + 'static {
        let h = self.0.rt.spawn(f);
        async move { joined(h.await) }
    }

    // --- State --------------------------------------------------------------------

    pub fn state(&self) -> watch::Receiver<State> {
        self.0.state.subscribe()
    }
    pub fn live(&self) -> watch::Receiver<Live> {
        self.0.live.subscribe()
    }
    pub fn connections(&self) -> watch::Receiver<Arc<Connections>> {
        self.0.connections.subscribe()
    }
    pub fn proxies(&self) -> watch::Receiver<Arc<Proxies>> {
        self.0.proxies.subscribe()
    }
    /// The sequence number of the newest log line: changes with each new line.
    pub fn log_seq(&self) -> watch::Receiver<u64> {
        self.0.log_seq.subscribe()
    }
    /// A copy of the kept log lines, oldest first.
    pub fn logs(&self) -> Vec<LogEntry> {
        self.0
            .logs
            .lock()
            .map(|l| l.iter().cloned().collect())
            .unwrap_or_default()
    }
    pub fn clear_logs(&self) {
        if let Ok(mut l) = self.0.logs.lock() {
            l.clear();
        }
        self.0.log_seq.send_modify(|s| *s += 1);
    }

    pub fn snapshot(&self) -> State {
        self.0.state.borrow().clone()
    }

    pub fn settings(&self) -> Settings {
        self.0.state.borrow().settings.clone()
    }

    fn modify<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut out = None;
        self.0.state.send_modify(|s| out = Some(f(s)));
        out.expect("modified")
    }

    fn set_core(&self, status: CoreStatus) {
        let running = status.is_running();
        self.modify(|s| {
            s.core = status;
            if !running {
                s.bypass.clear();
            }
        });
    }

    /// Shows `text` to the user once.
    pub fn notify(&self, text: impl Into<String>, error: bool) {
        let id = {
            let mut seq = self.0.notice_seq.lock().expect("notice seq");
            *seq += 1;
            *seq
        };
        let text = text.into();
        if error {
            log::warn!("{text}");
        }
        self.modify(|s| s.notice = Some(Notice { id, text, error }));
    }

    fn controller(&self) -> Controller {
        let s = self.0.state.borrow();
        Controller::new(s.settings.controller_port, &s.settings.secret)
    }

    fn running(&self) -> bool {
        self.0.state.borrow().core.is_running()
    }

    /// The core binary in use.
    pub fn core_path(&self) -> std::path::PathBuf {
        match &self.0.state.borrow().settings.core_path {
            Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
            _ => self.0.paths.core_bin(),
        }
    }

    /// Makes the core's ports free for it. A core of Kairo's own that is
    /// still letting go gets a moment; one a crashed Kairo left behind (its
    /// command line names this home) is stopped. Another program (Clash Verge
    /// Rev uses 7897 and 9097 too, and its core can outlive its window) is left
    /// alone: Kairo moves to the next free port, which it keeps, and the system
    /// proxy follows. A sentence for the user when the proxy port moved.
    async fn claim_ports(&self) -> Result<Option<String>, String> {
        let mut next = self.settings();
        let mut note = None;
        for controller in [true, false] {
            let port = if controller {
                next.controller_port
            } else {
                next.mixed_port
            };
            let free_now = tokio::task::spawn_blocking(move || !crate::settings::port_in_use(port))
                .await
                .unwrap_or(false);
            if free_now {
                continue;
            }
            let mut holder = tokio::task::spawn_blocking(move || crate::ports::holder(port))
                .await
                .ok()
                .flatten();
            // A program that can be seen and is not this Kairo's core will not
            // let go: move at once. Otherwise (this Kairo's core on its way
            // out, or a holder that cannot be seen) give it a moment. A core
            // on its way out has no command line any more (its memory goes
            // before its sockets do): that is a holder that cannot be seen.
            let other_program = holder.as_ref().is_some_and(|h| {
                h.pid.is_some()
                    && h.command.as_deref().is_some_and(|c| !c.is_empty())
                    && !crate::ports::is_ours(h, &self.0.paths.runtime_dir())
            });
            if !other_program {
                if crate::ports::wait_free(port, Duration::from_millis(2500)).await {
                    continue;
                }
                holder = tokio::task::spawn_blocking(move || crate::ports::holder(port))
                    .await
                    .ok()
                    .flatten();
            }
            if let Some(h) = &holder
                && let Some(pid) = h.pid
                && crate::ports::is_ours(h, &self.0.paths.runtime_dir())
            {
                log::warn!(
                    "port {port}: a core of this Kairo's (pid {pid}) is still there; stopping it"
                );
                crate::supervisor::stop_pid(pid as i32).await;
                if crate::ports::wait_free(port, Duration::from_secs(3)).await {
                    continue;
                }
            }
            let other = if controller {
                next.mixed_port
            } else {
                next.controller_port
            };
            let free = tokio::task::spawn_blocking(move || {
                crate::settings::free_port(port.saturating_add(1), &[other])
            })
            .await
            .unwrap_or(port);
            let who = holder
                .map(|h| h.describe())
                .unwrap_or_else(|| "another program".into());
            let taken = tokio::task::spawn_blocking(move || crate::settings::port_in_use(free))
                .await
                .unwrap_or(true);
            if taken || free == other {
                return Err(format!(
                    "Port {port} is used by {who}, and Kairo found no free port near it. Quit {who} or choose another port in Settings."
                ));
            }
            log::warn!("port {port} is used by {who}: Kairo moves to {free}");
            if controller {
                next.controller_port = free;
            } else {
                next.mixed_port = free;
                note = Some(format!(
                    "Port {port} is used by {who}, so Kairo is on {free}."
                ));
            }
        }
        if next != self.settings() {
            self.modify(|s| s.settings = next.clone());
            self.save_settings();
        }
        Ok(note)
    }

    fn save_settings(&self) {
        let s = self.settings();
        if let Err(e) = s.save(&self.0.paths) {
            self.notify(format!("Could not save settings: {e}"), true);
        }
    }

    fn save_profiles(&self) {
        let p = self.0.state.borrow().profiles.clone();
        if let Err(e) = p.save(&self.0.paths) {
            self.notify(format!("Could not save profiles: {e}"), true);
        }
    }

    // --- Boot ---------------------------------------------------------------------

    async fn boot(self) {
        let geo = self.clone();
        tokio::spawn(async move { geo.prefetch_geo().await });
        // The bundled core, copied in place (first run, or after an app update).
        let seeded = core_bin::seed(&self.0.paths).await;
        if let Err(e) = &seeded {
            log::warn!("core: {e}");
        }
        let core = self.core_path();
        let version = core_bin::binary_version(&core).await;
        let privilege = tun::status(&core);
        self.modify(|s| {
            s.core_version = version.clone();
            s.tun_privilege = Some(privilege);
        });
        // A new core (an app update) does not have TUN's grant yet: say so
        // rather than start a TUN that cannot come up.
        if self.settings().tun && privilege == Privilege::Needed {
            self.modify(|s| s.settings.tun = false);
            self.save_settings();
            self.notify(
                "TUN is off: the updated core needs permission again. Turn TUN on to grant it.",
                true,
            );
        }
        match (version, seeded) {
            (Some(_), _) => {
                if self.settings().auto_start_core
                    && let Err(e) = self.start_core().await
                {
                    log::warn!("start: {e}");
                }
            }
            (None, Err(e)) => self.set_core(CoreStatus::Failed {
                message: e.to_string(),
            }),
            (None, Ok(_)) => self.set_core(CoreStatus::Failed {
                message: format!("The core at {} does not run", core.display()),
            }),
        }
        crate::update::spawn_checks(self.clone());
    }

    /// Updates subscriptions when due.
    async fn scheduler(self) {
        let mut tick = tokio::time::interval(Duration::from_secs(300));
        tick.tick().await;
        loop {
            tick.tick().await;
            let now = profile::now();
            let due: Vec<String> = self
                .0
                .state
                .borrow()
                .profiles
                .profiles
                .iter()
                .filter(|p| p.update_due(now))
                .map(|p| p.id.clone())
                .collect();
            for id in due {
                if let Err(e) = self.update_profile(id).await {
                    log::warn!("scheduled update: {e}");
                }
            }
        }
    }

    // --- The core -----------------------------------------------------------------

    fn send_core<T: Send + 'static>(
        &self,
        make: fn(oneshot::Sender<T>) -> CoreCmd,
    ) -> impl Future<Output = Option<T>> + use<T> {
        let (tx, rx) = oneshot::channel();
        let sent = self.0.core_tx.send(make(tx)).is_ok();
        async move { if sent { rx.await.ok() } else { None } }
    }

    pub fn start_core(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let f = self.send_core(CoreCmd::Start);
        async move { f.await.unwrap_or_else(|| Err("The service stopped".into())) }
    }

    pub fn stop_core(&self) -> impl Future<Output = ()> + 'static {
        let f = self.send_core(CoreCmd::Stop);
        async move {
            f.await;
        }
    }

    pub fn restart_core(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let f = self.send_core(CoreCmd::Restart);
        async move { f.await.unwrap_or_else(|| Err("The service stopped".into())) }
    }

    async fn core_actor(self, mut rx: mpsc::UnboundedReceiver<CoreCmd>) {
        let mut process: Option<CoreProcess> = None;
        let mut streams: Vec<JoinHandle<()>> = Vec::new();
        let mut want = false;
        let mut crashes: VecDeque<Instant> = VecDeque::new();
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    match cmd {
                        CoreCmd::Start(reply) => {
                            want = true;
                            let r = if process.is_some() { Ok(()) } else { self.launch(&mut process, &mut streams).await };
                            let _ = reply.send(r);
                        }
                        CoreCmd::Stop(reply) => {
                            want = false;
                            self.halt(&mut process, &mut streams).await;
                            let _ = reply.send(());
                        }
                        CoreCmd::Restart(reply) => {
                            want = true;
                            self.halt(&mut process, &mut streams).await;
                            let r = self.launch(&mut process, &mut streams).await;
                            let _ = reply.send(r);
                        }
                    }
                }
                exit = async { process.as_mut().expect("guarded").wait().await }, if process.is_some() => {
                    process = None;
                    for s in streams.drain(..) { s.abort(); }
                    self.sysproxy_off_temporarily();
                    let reason = exit.reason();
                    log::warn!("the core exited: {reason}");
                    if !want {
                        self.set_core(CoreStatus::Stopped);
                        continue;
                    }
                    let now = Instant::now();
                    crashes.push_back(now);
                    while crashes.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
                        crashes.pop_front();
                    }
                    if crashes.len() <= 3 {
                        self.notify(format!("The core stopped unexpectedly and is restarting: {reason}"), true);
                        self.set_core(CoreStatus::Starting);
                        tokio::time::sleep(Duration::from_millis(500 * crashes.len() as u64)).await;
                        if let Err(e) = self.launch(&mut process, &mut streams).await {
                            log::warn!("restart failed: {e}");
                        }
                    } else {
                        want = false;
                        self.set_core(CoreStatus::Failed { message: reason });
                    }
                }
            }
        }
    }

    async fn launch(
        &self,
        process: &mut Option<CoreProcess>,
        streams: &mut Vec<JoinHandle<()>>,
    ) -> Result<(), String> {
        let core = self.core_path();
        if !core.exists() {
            let message = format!("The core is missing ({})", core.display());
            self.set_core(CoreStatus::Failed {
                message: message.clone(),
            });
            return Err(message);
        }
        // A core left behind by a Kairo that crashed would hold the ports.
        crate::supervisor::stop_stale(&self.0.paths).await;
        self.set_core(CoreStatus::Starting);
        let note = match self.claim_ports().await {
            Ok(note) => note,
            Err(message) => {
                self.set_core(CoreStatus::Failed {
                    message: message.clone(),
                });
                return Err(message);
            }
        };
        self.modify(|s| {
            s.warning = None;
            s.note = note;
        });
        // A fresh config from the active profile and the current settings.
        if let Err(e) = self.write_config(false).await {
            self.set_core(CoreStatus::Failed { message: e.clone() });
            return Err(e);
        }
        let mut child = match CoreProcess::spawn(&core, &self.0.paths).await {
            Ok(c) => c,
            Err(e) => {
                let message = format!("Could not start the core: {e}");
                self.set_core(CoreStatus::Failed {
                    message: message.clone(),
                });
                return Err(message);
            }
        };
        let ctl = self.controller();
        let ready = tokio::select! {
            r = ctl.wait_ready(Duration::from_secs(15)) => r.map_err(|e| e.to_string()),
            exit = child.wait() => Err(exit.reason()),
        };
        let version = match ready {
            Ok(v) => v.version,
            Err(e) => {
                child.stop(&self.0.paths).await;
                let message = if e == ApiError::Unreachable.to_string()
                    || e == ApiError::Timeout.to_string()
                {
                    let tail = child.tail();
                    let core_error = tail
                        .iter()
                        .rev()
                        .find(|l| l.contains("level=error") || l.contains("level=fatal"));
                    core_error.map(|l| config::explain(l)).unwrap_or_else(|| {
                        "The core started but its controller never answered".into()
                    })
                } else {
                    e
                };
                self.set_core(CoreStatus::Failed {
                    message: message.clone(),
                });
                return Err(message);
            }
        };
        // Problems it survives: say so.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let in_use = child
            .tail()
            .into_iter()
            .find(|l| l.contains("address already in use"))
            .map(|l| config::explain(&l));
        if let Some(w) = in_use {
            self.modify(|s| s.warning = Some(w));
        }
        *process = Some(child);
        self.set_core(CoreStatus::Running { version });
        streams.extend(self.spawn_streams(ctl));
        let svc = self.clone();
        tokio::spawn(async move {
            svc.refresh_proxies_now().await;
            svc.keep_fastest().await;
            svc.ai_sync().await;
            svc.downloads_sync(false).await;
            svc.run_ai_check(false).await;
        });
        if self.settings().system_proxy && self.0.state.borrow().sysproxy_supported {
            self.apply_sysproxy(true);
        }
        Ok(())
    }

    async fn halt(&self, process: &mut Option<CoreProcess>, streams: &mut Vec<JoinHandle<()>>) {
        for s in streams.drain(..) {
            s.abort();
        }
        if let Some(mut p) = process.take() {
            self.set_core(CoreStatus::Stopping);
            self.sysproxy_off_temporarily();
            p.stop(&self.0.paths).await;
        }
        self.set_core(CoreStatus::Stopped);
        self.0.live.send_modify(|l| {
            l.up = 0;
            l.down = 0;
            l.connections = 0;
        });
        self.0
            .connections
            .send_replace(Arc::new(Connections::default()));
    }

    fn spawn_streams(&self, ctl: Controller) -> Vec<JoinHandle<()>> {
        let mut out = Vec::new();
        // Traffic.
        let (svc, c) = (self.clone(), ctl.clone());
        out.push(tokio::spawn(async move {
            loop {
                let _ = c
                    .stream::<Traffic>("/traffic", |t| {
                        svc.0.live.send_modify(|l| {
                            l.up = t.up;
                            l.down = t.down;
                            if l.history.len() == HISTORY {
                                l.history.pop_front();
                            }
                            l.history.push_back(t);
                        });
                        true
                    })
                    .await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }));
        // Memory.
        let (svc, c) = (self.clone(), ctl.clone());
        out.push(tokio::spawn(async move {
            loop {
                let _ = c
                    .stream::<Memory>("/memory", |m| {
                        svc.0.live.send_if_modified(|l| {
                            let changed = l.memory != m.inuse;
                            l.memory = m.inuse;
                            changed
                        });
                        true
                    })
                    .await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }));
        // Connections.
        let (svc, c) = (self.clone(), ctl.clone());
        out.push(tokio::spawn(async move {
            loop {
                let _ = c
                    .stream::<ConnectionsResponse>("/connections?interval=1000", |resp| {
                        svc.take_connections(resp);
                        true
                    })
                    .await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }));
        // Logs.
        // mihomo hands every event to `/logs` subscribers, filtered by the
        // query's level alone, so the stream follows the setting live.
        let (svc, c) = (self.clone(), ctl);
        out.push(tokio::spawn(async move {
            loop {
                let level = svc.settings().log_level;
                let mut state = svc.0.state.subscribe();
                let path = format!("/logs?level={}", level.as_str());
                let stream = c.stream::<LogLine>(&path, |line| {
                    svc.push_log(line);
                    true
                });
                tokio::select! {
                    _ = stream => tokio::time::sleep(Duration::from_secs(1)).await,
                    _ = async move { state.wait_for(|s| s.settings.log_level != level).await.is_ok() } => {}
                }
            }
        }));
        out
    }

    /// Gives the connections the core could not name the process of (macOS)
    /// the name `lsof` knows them by, and looks again, off the async threads,
    /// when one it has not seen appears.
    fn name_unnamed(&self, list: &mut [Connection], now: i64) {
        if !crate::procname::enabled() {
            return;
        }
        let port = |c: &Connection| {
            c.source
                .rsplit_once(':')
                .and_then(|(_, p)| p.parse::<u16>().ok())
        };
        // Gives each unnamed connection the owner `lsof` last found for its
        // port; returns the ports still unknown.
        let apply = |list: &mut [Connection]| -> Vec<u16> {
            let names = self.0.proc_names.lock().expect("proc names");
            let mut unnamed = Vec::new();
            for c in list.iter_mut().filter(|c| c.process.is_empty()) {
                let Some(p) = port(c) else { continue };
                match names.get(p) {
                    Some((pid, n)) => {
                        c.process = n.to_owned();
                        // Kairo's own checks: with no path from the core, say
                        // whose they are so the tally can leave them out.
                        if pid == std::process::id()
                            && let Ok(exe) = std::env::current_exe()
                        {
                            c.process_path = exe.to_string_lossy().into_owned();
                        }
                    }
                    None => unnamed.push(p),
                }
            }
            unnamed
        };
        let unnamed = apply(list);
        let due = {
            let names = self.0.proc_names.lock().expect("proc names");
            names.due(now, unnamed.iter().copied())
        };
        if !due
            || self
                .0
                .proc_busy
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let look = {
            let svc = self.clone();
            move || {
                let found = crate::procname::lookup();
                svc.0
                    .proc_names
                    .lock()
                    .expect("proc names")
                    .replace(found, unix());
                svc.0
                    .proc_busy
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
        };
        // A request that lasts a second is over before the next snapshot: ask
        // now, so this snapshot's connections are named while they still
        // exist. Where the runtime cannot spare the thread, ask in the
        // background (the next snapshot has it).
        match tokio::runtime::Handle::try_current() {
            Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(look);
                apply(list);
            }
            Ok(h) => {
                h.spawn_blocking(look);
            }
            Err(_) => look(),
        }
    }

    fn take_connections(&self, resp: ConnectionsResponse) {
        let previous = self.0.connections.borrow().clone();
        let by_id: HashMap<&str, &Connection> =
            previous.list.iter().map(|c| (c.id.as_str(), c)).collect();
        let mut list: Vec<Connection> = resp
            .connections
            .unwrap_or_default()
            .into_iter()
            .map(|raw| {
                let prev = by_id.get(raw.id.as_str()).copied();
                Connection::from_raw(raw, prev)
            })
            .collect();
        // Newest first.
        list.sort_by(|a, b| b.start.cmp(&a.start));
        let count = list.len();
        drop(by_id);
        let now = unix();
        self.name_unnamed(&mut list, now);
        // Kairo's own checks go through the proxy too: they are not a tool's.
        let own = std::env::current_exe().ok();
        let young = |c: &Connection| {
            c.process.is_empty()
                && crate::procname::enabled()
                && crate::activity::parse_time(&c.start).is_none_or(|t| now - t < 3)
        };
        let tools: Vec<Connection> = list
            .iter()
            // A connection still without a name is counted once it has one
            // (or has lived three seconds without), not under "Unknown" first.
            .filter(|c| !young(c))
            .filter(|c| {
                own.as_deref().is_none_or(|p| {
                    c.process_path.is_empty() || std::path::Path::new(&c.process_path) != p
                })
            })
            .cloned()
            .collect();
        let names = {
            let st = self.0.state.borrow();
            crate::activity::ExitNames {
                ai: st.ai.group.clone(),
                downloads: st.downloads.group.clone(),
            }
        };
        let mut deltas = Vec::new();
        self.0.activity.send_modify(|a| {
            let a = Arc::make_mut(a);
            a.names = names;
            deltas = a.ingest(&tools, now);
        });
        self.stats_traffic(now, &deltas);
        self.0.connections.send_replace(Arc::new(Connections {
            upload_total: resp.upload_total,
            download_total: resp.download_total,
            list,
        }));
        self.0.live.send_modify(|l| {
            l.connections = count;
            l.upload_total = resp.upload_total;
            l.download_total = resp.download_total;
        });
    }

    fn push_log(&self, line: LogLine) {
        let seq = {
            let mut logs = self.0.logs.lock().expect("logs");
            let seq = logs.back().map(|l| l.seq + 1).unwrap_or(1);
            if logs.len() == LOG_CAPACITY {
                logs.pop_front();
            }
            logs.push_back(LogEntry {
                seq,
                time: clock(),
                level: line.level,
                payload: line.payload,
            });
            seq
        };
        self.0.log_seq.send_replace(seq);
    }

    /// Stops the core and puts the system proxy back: call before the app exits.
    pub fn shutdown(&self) {
        let svc = self.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.0.rt.spawn(async move {
            svc.stop_core().await;
            let _ = sysproxy::disable(&svc.0.paths)
                .map_err(|e| log::error!("restoring the system proxy: {e}"));
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(Duration::from_secs(6));
        self.stats_close();
        // Whatever happened above, never leave the proxy pointing at a dead core.
        if self.0.paths.sysproxy_state().exists() {
            let _ = sysproxy::disable(&self.0.paths);
        }
    }

    /// The geo data `config` needs, in place (fetched now if missing). A failed
    /// download is left to the core, which tries the same mirrors.
    async fn ensure_geo(&self, config: &str) {
        let _lock = self.0.geo_lock.lock().await;
        for file in crate::geo::needed(config) {
            if crate::geo::present(&self.0.paths, file) {
                continue;
            }
            let start = Instant::now();
            match crate::geo::fetch(&self.0.paths, file).await {
                Ok(_) => log::info!("{} in {:.1} s", file.name, start.elapsed().as_secs_f32()),
                Err(e) => log::warn!("geo data: {e}"),
            }
        }
    }

    /// From the first launch, the geo data the active subscription needs (or,
    /// before there is one, the GeoIP data nearly every subscription needs),
    /// while the user is still pasting the link.
    async fn prefetch_geo(&self) {
        let active = self.0.state.borrow().profiles.active.clone();
        let text = match active.and_then(|id| profile::read_config(&self.0.paths, &id).ok()) {
            Some(t) => t,
            None => "rules:\n  - GEOIP,CN,DIRECT\n".to_owned(),
        };
        self.ensure_geo(&text).await;
    }

    /// The emergency path (signal handlers, panics): restore the proxy, synchronously.
    pub fn restore_system_proxy_now(paths: &Paths) {
        if paths.sysproxy_state().exists() {
            let _ = sysproxy::disable(paths);
        }
    }

    // --- Config -------------------------------------------------------------------

    /// Builds the config from the active profile, checks it with the core, and
    /// makes it the runtime config. With `reload`, a running core loads it.
    async fn write_config(&self, reload: bool) -> Result<(), String> {
        let _lock = self.0.apply_lock.lock().await;
        let (active, settings) = {
            let s = self.0.state.borrow();
            (s.profiles.active.clone(), s.settings.clone())
        };
        let paths = &self.0.paths;
        // With no profile yet the core still runs, sending everything direct.
        let (text, over) = match &active {
            Some(id) => (
                profile::read_config(paths, id)
                    .map_err(|e| format!("The active profile's file is missing: {e}"))?,
                profile::read_override(paths, id),
            ),
            None => (NO_PROFILE.to_owned(), String::new()),
        };
        // The profile's own local ports another client holds move aside (the
        // ones this Kairo's running core holds stay).
        let runtime = paths.runtime_dir();
        let config::Built {
            text: built,
            applied,
            skipped_routes,
        } = config::build_full(&text, &over, &settings, &|p| {
            crate::ports::held_by_other(p, &runtime)
        })
        .map_err(|e| e.to_string())?;
        // The core would stop to download missing geo data while it loads.
        self.ensure_geo(&built).await;
        crate::paths::write_atomic(&paths.candidate_config(), built.as_bytes())
            .map_err(|e| e.to_string())?;
        let core = self.core_path();
        if core.exists() {
            config::validate(&core, &paths.runtime_dir(), &paths.candidate_config()).await?;
        }
        std::fs::rename(paths.candidate_config(), paths.runtime_config())
            .map_err(|e| e.to_string())?;
        self.ai_config_loaded(&built, applied);
        self.modify(|s| s.routes_skipped = skipped_routes);
        if reload && self.running() {
            self.controller()
                .reload(&paths.runtime_config().to_string_lossy())
                .await
                .map_err(|e| format!("The core rejected the config: {e}"))?;
            self.refresh_proxies_now().await;
            let svc = self.clone();
            tokio::spawn(async move {
                svc.keep_fastest().await;
                svc.ai_sync().await;
                svc.downloads_sync(false).await;
                svc.run_ai_check(false).await;
            });
        }
        Ok(())
    }

    /// Checks profile + override text with the core without applying anything.
    pub fn check_config(
        &self,
        profile_text: String,
        override_text: String,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let built = config::build(&profile_text, &override_text, &svc.settings())
                .map_err(|e| e.to_string())?;
            svc.ensure_geo(&built).await;
            let path = svc.0.paths.runtime_dir().join("check.yaml");
            crate::paths::write_atomic(&path, built.as_bytes()).map_err(|e| e.to_string())?;
            let core = svc.core_path();
            if !core.exists() {
                return Ok(());
            }
            let r = config::validate(&core, &svc.0.paths.runtime_dir(), &path).await;
            let _ = std::fs::remove_file(&path);
            r
        })
    }

    // --- Settings -----------------------------------------------------------------

    pub fn set_mode(&self, mode: Mode) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let before = svc.settings();
            svc.modify(|s| s.settings.mode = mode);
            svc.save_settings();
            if svc.running() {
                // "All Traffic" with the AI layer on is other rules, not the
                // core's global mode: the config is rebuilt and reloaded
                // (open connections stay on their route through a reload).
                if before.ai.enabled && (before.mode == Mode::Global || mode == Mode::Global) {
                    svc.write_config(true).await?;
                } else {
                    svc.controller()
                        .patch_configs(serde_json::json!({ "mode": mode.as_str() }))
                        .await
                        .map_err(|e| e.to_string())?;
                }
                // New connections take the new route; open ones (a model's
                // answer streaming in, a download) finish on theirs. Only the
                // user's own Close ends them.
                svc.refresh_proxies_now().await;
                // Leaving or entering Direct changes what a check measures.
                let moved = (before.mode == Mode::Direct) != (mode == Mode::Direct);
                if moved {
                    let me = svc.clone();
                    tokio::spawn(async move { me.run_ai_check(true).await });
                }
            }
            Ok(())
        })
    }

    /// Replaces the settings; applies what changed (live where mihomo allows it).
    pub fn update_settings(
        &self,
        next: Settings,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let mut next = next;
            next.validate()?;
            // Routes are checked again whoever hands them over.
            next.routes = crate::routes::sanitize(std::mem::take(&mut next.routes));
            let prev = svc.settings();
            if prev == next {
                return Ok(());
            }
            // The mirror is only ever turned on after it has passed its test.
            if next.downloads.mirror
                && !prev.downloads.mirror
                && !svc.0.state.borrow().downloads.mirror_test.passed()
            {
                return Err(crate::downloads::MIRROR_REFUSED.into());
            }
            svc.modify(|s| s.settings = next.clone());
            svc.save_settings();
            svc.downloads_view_sync();
            if prev.launch_at_login != next.launch_at_login
                || prev.start_minimized != next.start_minimized
            {
                crate::autostart::set(next.launch_at_login, next.start_minimized)
                    .map_err(|e| format!("Launch at login: {e}"))?;
            }
            let restart = prev.controller_port != next.controller_port
                || prev.secret != next.secret
                || prev.core_path != next.core_path;
            let reload = prev.mixed_port != next.mixed_port
                || prev.allow_lan != next.allow_lan
                || prev.ipv6 != next.ipv6
                || prev.log_level != next.log_level
                || prev.tun != next.tun
                || prev.tun_stack != next.tun_stack
                || prev.routes != next.routes;
            if svc.running() {
                if restart {
                    svc.restart_core().await?;
                } else if reload {
                    if let Err(e) = svc.write_config(true).await {
                        // Put the old settings back: the running core still has them.
                        svc.modify(|s| s.settings = prev.clone());
                        svc.save_settings();
                        svc.downloads_view_sync();
                        return Err(e);
                    }
                    if prev.log_level != next.log_level {
                        svc.restart_streams_hint();
                    }
                }
                if (prev.mixed_port != next.mixed_port || prev.bypass != next.bypass)
                    && next.system_proxy
                {
                    svc.apply_sysproxy(true);
                }
            }
            if prev.system_proxy != next.system_proxy {
                svc.apply_sysproxy(next.system_proxy && svc.running());
            }
            if next.tun || !next.system_proxy {
                svc.clear_bypass();
            }
            if prev.downloads.exit != next.downloads.exit
                || prev.downloads.fastest != next.downloads.fastest
            {
                svc.downloads_sync(false).await;
            }
            Ok(())
        })
    }

    fn restart_streams_hint(&self) {
        // The log stream's level is fixed per connection; a restart of the core is
        // not worth it for this, so new lines simply follow the next start.
    }

    pub fn set_system_proxy(&self, on: bool) -> impl Future<Output = Result<(), String>> + 'static {
        let mut next = self.settings();
        next.system_proxy = on;
        let svc = self.clone();
        let f = self.update_settings(next);
        async move {
            f.await?;
            let s = svc.snapshot();
            if on && s.core.is_running() && !s.sysproxy_active {
                return Err("The system proxy could not be set".into());
            }
            if on && !s.core.is_running() {
                return Err("The system proxy turns on when the core is running".into());
            }
            Ok(())
        }
    }

    pub fn set_tun(&self, on: bool) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        let mut next = self.settings();
        next.tun = on;
        async move {
            if on && tun::status(&svc.core_path()) != Privilege::Granted {
                return Err("TUN needs permission first".into());
            }
            svc.update_settings(next).await
        }
    }

    /// Asks the OS to let the core run TUN (a password prompt).
    pub fn grant_tun(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let core = svc.core_path();
            let result = tokio::task::spawn_blocking(move || tun::grant(&core))
                .await
                .map_err(|e| e.to_string())?;
            let status = tun::status(&svc.core_path());
            svc.modify(|s| s.tun_privilege = Some(status));
            result?;
            // A running core keeps its old privileges until it starts again.
            if svc.running() {
                svc.restart_core().await?;
            }
            Ok(())
        })
    }

    /// Sets (or restores) the system proxy now, recording the outcome.
    fn apply_sysproxy(&self, on: bool) {
        let paths = self.0.paths.clone();
        let settings = self.settings();
        let result = if on {
            sysproxy::enable(
                &paths,
                &ProxySpec {
                    host: "127.0.0.1".into(),
                    port: settings.mixed_port,
                    bypass: settings.bypass.clone(),
                },
            )
        } else {
            sysproxy::disable(&paths)
        };
        if on
            && result.is_ok()
            && let Err(e) = crate::autostart::install_restore()
        {
            log::warn!("network check at login: {e}");
        }
        match result {
            Ok(()) => self.modify(|s| s.sysproxy_active = on),
            Err(e) => {
                self.modify(|s| s.sysproxy_active = false);
                self.notify(format!("System proxy: {e}"), true);
            }
        }
    }

    /// The core stopped: put the desktop's proxy back, keeping the user's wish
    /// so it returns with the core.
    fn sysproxy_off_temporarily(&self) {
        if self.0.state.borrow().sysproxy_active || self.0.paths.sysproxy_state().exists() {
            self.apply_sysproxy(false);
        }
    }

    // --- Connect and location -----------------------------------------------------

    /// Connected: the core runs (and the system proxy points at it, when that
    /// is on); it stays so across launches. Off: the core stops.
    pub fn connect(&self, on: bool) -> impl Future<Output = Result<(), String>> + 'static {
        // Both done now, in the caller's order: an off and an on in quick
        // succession (the tray's toggle, clicked twice) each run as a task of
        // their own, and the last click must win.
        if self.settings().auto_start_core != on {
            self.modify(|s| s.settings.auto_start_core = on);
            self.save_settings();
        }
        let core = if on {
            futures_util::future::Either::Left(self.start_core())
        } else {
            let stop = self.stop_core();
            futures_util::future::Either::Right(async move {
                stop.await;
                Ok(())
            })
        };
        self.run(core)
    }

    /// The group whose members are the locations.
    pub fn main_group(&self) -> Option<crate::model::Group> {
        let preferred = self.settings().tray_group;
        crate::places::main_group(&self.0.proxies.borrow().groups, &preferred).cloned()
    }

    /// Goes through `name` (a place the main group reaches); `None`: the
    /// fastest, kept so (automatic).
    pub fn set_location(
        &self,
        name: Option<String>,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            if name.is_some() {
                svc.stats_manual(1);
            }
            let mut next = svc.settings();
            if next.auto_location != name.is_none() {
                next.auto_location = name.is_none();
                svc.update_settings(next).await?;
            }
            match name {
                Some(n) => svc.go_through(&n).await,
                None => svc.pick_fastest(true).await.map(|_| ()),
            }
        })
    }

    /// Selects the way to `name`: in its selector, then that selector in the main group.
    async fn go_through(&self, name: &str) -> Result<(), String> {
        let group = self
            .main_group()
            .ok_or("This subscription has no locations to choose from")?;
        let steps = crate::places::route_to(&self.0.proxies.borrow().groups, &group, name);
        if steps.is_empty() {
            return Err(format!(
                "{name} is not one of this subscription's locations"
            ));
        }
        for (g, member) in steps {
            self.select_inner(g, member).await?;
        }
        Ok(())
    }

    /// Tests every place (the main group and its member selectors) at once.
    pub fn test_places(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let Some(group) = svc.main_group() else {
                return Ok(());
            };
            let names = crate::places::place_groups(&svc.0.proxies.borrow().groups, &group);
            let tests: Vec<_> = names.into_iter().map(|g| svc.test_group(g)).collect();
            let mut last = Ok(());
            for r in futures_join(tests).await {
                if r.is_err() {
                    last = r;
                }
            }
            last
        })
    }

    /// Tests the places and goes through the fastest (when `force`, or when
    /// the current one is a note, failed, or much slower). The place it chose.
    async fn pick_fastest(&self, force: bool) -> Result<Option<String>, String> {
        if self.main_group().is_none() {
            return Ok(None);
        }
        self.test_places().await?;
        let Some(group) = self.main_group() else {
            return Ok(None);
        };
        let (best, current) = {
            let groups = &self.0.proxies.borrow().groups;
            (
                crate::places::fastest(groups, &group),
                crate::places::current(groups, &group),
            )
        };
        let Some(best) = best else {
            return Err(
                "No location answered. Check the network, or update the subscription".into(),
            );
        };
        let keep = !force
            && current.as_ref().is_some_and(|c| {
                !crate::places::is_note(&c.name)
                    && c.delay.is_some_and(|d| {
                        d > 0 && d <= best.delay.unwrap_or(0).saturating_mul(3) / 2 + 40
                    })
            });
        if keep || current.as_ref().is_some_and(|c| c.name == best.name) {
            return Ok(current.map(|c| c.name));
        }
        // A place picked by hand meanwhile wins (a provider's note is no place).
        if !self.settings().auto_location
            && current
                .as_ref()
                .is_some_and(|c| !crate::places::is_note(&c.name))
        {
            return Ok(current.map(|c| c.name));
        }
        self.go_through(&best.name).await?;
        Ok(Some(best.name))
    }

    /// Automatic location: on connect and on a new config, then every ten
    /// minutes while it stays automatic.
    async fn keep_fastest(&self) {
        static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        // Say so only when there is no real place yet: a new subscription, or
        // a provider's note in use. Otherwise it checks quietly. A note is
        // never somewhere to be, so Kairo moves off one even when the place
        // is not automatic (a 0.1 install whose node was never picked).
        let on_note = self.on_note();
        if !self.settings().auto_location && !on_note
            || RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let fresh = self
            .0
            .fresh
            .swap(false, std::sync::atomic::Ordering::SeqCst);
        let mut force = false;
        if fresh || on_note {
            self.modify(|s| s.locating = true);
            // The first place that answers is one of the quickest and enough to
            // browse with: connected now, the full comparison follows quietly.
            match self.first_answer().await {
                // A place picked by hand meanwhile wins.
                Some(_) if !self.settings().auto_location && !self.on_note() => {}
                Some(name) => {
                    if let Err(e) = self.go_through(&name).await {
                        log::warn!("first place: {e}");
                        force = true;
                    }
                }
                None => force = true,
            }
            if !force {
                self.modify(|s| s.locating = false);
            }
        }
        if let Err(e) = self.pick_fastest(force).await {
            log::warn!("automatic location: {e}");
        }
        self.modify(|s| s.locating = false);
        RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether the main group goes through a provider's note (or nowhere).
    fn on_note(&self) -> bool {
        self.main_group().is_some_and(|g| {
            crate::places::current(&self.0.proxies.borrow().groups, &g)
                .is_none_or(|p| crate::places::is_note(&p.name))
        })
    }

    /// Tests every place at once and returns the first that answers.
    async fn first_answer(&self) -> Option<String> {
        use futures_util::StreamExt;
        let group = self.main_group()?;
        let names: Vec<String> = crate::places::places(&self.0.proxies.borrow().groups, &group)
            .into_iter()
            .filter(|p| !p.is_group && !crate::places::is_note(&p.name))
            .map(|p| p.name)
            .collect();
        let s = self.settings();
        let ctl = self.controller();
        let mut tests: futures_util::stream::FuturesUnordered<_> = names
            .into_iter()
            .map(|name| {
                let (ctl, url) = (ctl.clone(), s.test_url.clone());
                async move { (ctl.delay(&name, &url, s.test_timeout_ms).await, name) }
            })
            .collect();
        while let Some((result, name)) = tests.next().await {
            if result.is_ok_and(|d| d > 0) {
                return Some(name);
            }
        }
        None
    }

    /// The periodic part of [`Service::keep_fastest`].
    async fn location_loop(&self) {
        let mut tick = tokio::time::interval(Duration::from_secs(600));
        tick.tick().await;
        loop {
            tick.tick().await;
            if self.running() {
                self.keep_fastest().await;
                // The fastest place for downloads is measured by the same
                // tests; when the Location is not automatic nothing ran them.
                self.downloads_sync(!self.settings().auto_location).await;
            }
        }
    }

    // --- Profiles -----------------------------------------------------------------

    /// Adds a subscription; it becomes active when it is the first profile.
    pub fn add_remote(
        &self,
        url: String,
        name: Option<String>,
    ) -> impl Future<Output = Result<String, String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let url = url.trim().to_owned();
            let parsed = url::Url::parse(&url).map_err(|_| "Enter a valid URL".to_string())?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err("Subscriptions use http or https".into());
            }
            let download = svc.download(&url).await?;
            let stats = profile::inspect(&download.body).map_err(|e| e.to_string())?;
            let id = profile::new_id();
            profile::write_config(&svc.0.paths, &id, &download.body).map_err(|e| e.to_string())?;
            let profile = Profile {
                id: id.clone(),
                name: name
                    .filter(|n| !n.trim().is_empty())
                    .or(download.name)
                    .unwrap_or_else(|| profile::name_from_url(&url)),
                source: Source::Remote {
                    url,
                    interval_hours: download.interval_hours.unwrap_or(24),
                },
                updated_at: Some(profile::now()),
                usage: download.usage,
                web_page: download.web_page,
                last_error: None,
                failed_at: None,
                stats,
            };
            svc.insert_profile(profile).await;
            svc.stats_usage(&id);
            Ok(id)
        })
    }

    /// Imports a local config file.
    pub fn add_file(
        &self,
        path: std::path::PathBuf,
    ) -> impl Future<Output = Result<String, String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let text = profile::read_import(&path).map_err(|e| e.to_string())?;
            let stats = profile::inspect(&text).map_err(|e| e.to_string())?;
            let id = profile::new_id();
            profile::write_config(&svc.0.paths, &id, &text).map_err(|e| e.to_string())?;
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Local".into());
            svc.insert_profile(Profile {
                id: id.clone(),
                name,
                source: Source::Local {
                    origin: Some(path.to_string_lossy().into_owned()),
                },
                updated_at: Some(profile::now()),
                usage: None,
                web_page: None,
                last_error: None,
                failed_at: None,
                stats,
            })
            .await;
            Ok(id)
        })
    }

    async fn insert_profile(&self, profile: Profile) {
        let first = self.modify(|s| {
            s.profiles.profiles.push(profile.clone());
            s.profiles.active.is_none()
        });
        self.save_profiles();
        if first {
            if self.settings().auto_location {
                // From the first frame of Home until the first place is chosen.
                self.0
                    .fresh
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                self.modify(|s| s.locating = true);
            }
            let _ = self
                .activate_inner(profile.id)
                .await
                .map_err(|e| self.notify(e, true));
        }
    }

    async fn download(&self, url: &str) -> Result<profile::Download, String> {
        match profile::download(url, None).await {
            Ok(d) => Ok(d),
            Err(direct) => {
                // Blocked directly? Try through the core when it runs.
                if self.running() {
                    let proxy = format!("http://127.0.0.1:{}", self.settings().mixed_port);
                    profile::download(url, Some(&proxy))
                        .await
                        .map_err(|e| e.to_string())
                } else {
                    Err(direct.to_string())
                }
            }
        }
    }

    /// Downloads a subscription again; reloads the core when it is the active one.
    pub fn update_profile(&self, id: String) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let Some(p) = svc.0.state.borrow().profiles.get(&id).cloned() else {
                return Err("No such profile".into());
            };
            let Some(url) = p.url().map(str::to_owned) else {
                return Err("Local profiles have nothing to update from".into());
            };
            svc.modify(|s| s.updating.insert(id.clone()));
            let result = async {
                let d = svc.download(&url).await?;
                let stats = profile::inspect(&d.body).map_err(|e| e.to_string())?;
                profile::write_config(&svc.0.paths, &id, &d.body).map_err(|e| e.to_string())?;
                Ok::<_, String>((d, stats))
            }
            .await;
            svc.modify(|s| {
                s.updating.remove(&id);
                if let Some(p) = s.profiles.get_mut(&id) {
                    match &result {
                        Ok((d, stats)) => {
                            p.updated_at = Some(profile::now());
                            p.last_error = None;
                            p.failed_at = None;
                            p.stats = *stats;
                            if d.usage.is_some() {
                                p.usage = d.usage;
                            }
                            if d.web_page.is_some() {
                                p.web_page = d.web_page.clone();
                            }
                        }
                        // The saved config stays: the connection keeps working.
                        Err(e) => {
                            p.last_error = Some(e.clone());
                            p.failed_at = Some(profile::now());
                        }
                    }
                }
            });
            svc.save_profiles();
            result?;
            svc.stats_usage(&id);
            let active = svc.0.state.borrow().profiles.active.as_deref() == Some(id.as_str());
            if active && svc.running() {
                svc.write_config(true).await?;
            }
            Ok(())
        })
    }

    pub fn update_all_profiles(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let ids: Vec<String> = svc
                .0
                .state
                .borrow()
                .profiles
                .profiles
                .iter()
                .filter(|p| p.is_remote())
                .map(|p| p.id.clone())
                .collect();
            let mut errors = Vec::new();
            for id in ids {
                if let Err(e) = svc.update_profile(id).await {
                    errors.push(e);
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("\n"))
            }
        })
    }

    /// Makes a profile the one the core runs.
    pub fn activate(&self, id: String) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move { svc.activate_inner(id).await })
    }

    async fn activate_inner(&self, id: String) -> Result<(), String> {
        let previous = self.0.state.borrow().profiles.active.clone();
        self.modify(|s| s.profiles.active = Some(id.clone()));
        let result = if self.running() {
            self.write_config(true).await
        } else if self.core_path().exists() {
            // A core already on its way up (the launch at startup) read its
            // config before this profile was the active one: once it is up,
            // load the profile.
            match self.start_core().await {
                Ok(()) if self.running() => self.write_config(true).await,
                other => other,
            }
        } else {
            Ok(())
        };
        if let Err(e) = &result {
            // Keep the core on the profile that works.
            if self.running() {
                self.modify(|s| s.profiles.active = previous);
            }
            return Err(e.clone());
        }
        self.save_profiles();
        Ok(())
    }

    pub fn rename_profile(
        &self,
        id: String,
        name: String,
        interval_hours: Option<u32>,
        url: Option<String>,
    ) {
        self.modify(|s| {
            if let Some(p) = s.profiles.get_mut(&id) {
                if !name.trim().is_empty() {
                    p.name = name.trim().to_owned();
                }
                if let Source::Remote {
                    url: u,
                    interval_hours: h,
                } = &mut p.source
                {
                    if let Some(i) = interval_hours {
                        *h = i;
                    }
                    if let Some(url) = url.filter(|u| url::Url::parse(u).is_ok()) {
                        *u = url;
                    }
                }
            }
        });
        self.save_profiles();
    }

    pub fn delete_profile(&self, id: String) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let was_active = svc.0.state.borrow().profiles.active.as_deref() == Some(id.as_str());
            svc.modify(|s| {
                s.profiles.profiles.retain(|p| p.id != id);
                if was_active {
                    s.profiles.active = s.profiles.profiles.first().map(|p| p.id.clone());
                }
            });
            svc.save_profiles();
            let _ = std::fs::remove_file(svc.0.paths.profile(&id));
            let _ = std::fs::remove_file(svc.0.paths.override_file(&id));
            if was_active {
                let next = svc.0.state.borrow().profiles.active.clone();
                match next {
                    Some(_) if svc.running() => svc.write_config(true).await?,
                    None => svc.stop_core().await,
                    _ => {}
                }
            }
            Ok(())
        })
    }

    pub fn profile_text(&self, id: &str) -> String {
        profile::read_config(&self.0.paths, id).unwrap_or_default()
    }

    pub fn override_text(&self, id: &str) -> String {
        profile::read_override(&self.0.paths, id)
    }

    /// Saves an override after the core accepts it; reloads when active.
    pub fn save_override(
        &self,
        id: String,
        text: String,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        let check = self.check_config(self.profile_text(&id), text.clone());
        self.run(async move {
            check.await?;
            profile::write_override(&svc.0.paths, &id, &text).map_err(|e| e.to_string())?;
            let active = svc.0.state.borrow().profiles.active.as_deref() == Some(id.as_str());
            if active && svc.running() {
                svc.write_config(true).await?;
            }
            Ok(())
        })
    }

    // --- Proxies ------------------------------------------------------------------

    async fn refresh_proxies_now(&self) {
        let ctl = self.controller();
        let (groups, providers) = tokio::join!(ctl.proxies(), ctl.proxy_providers());
        let testing = self.0.proxies.borrow().testing.clone();
        if let Ok(groups) = groups {
            self.0.proxies.send_replace(Arc::new(Proxies {
                groups: groups_of(&groups),
                providers: providers.unwrap_or_default(),
                testing,
            }));
            self.downloads_node_changed();
        }
    }

    pub fn refresh_proxies(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.refresh_proxies_now().await;
            Ok(())
        })
    }

    /// Picks `name` in `group` by hand: in a group of places (the main one or
    /// its member selectors) that ends the automatic location.
    pub fn select(
        &self,
        group: String,
        name: String,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let place_group = svc.main_group().is_some_and(|m| {
                crate::places::place_groups(&svc.0.proxies.borrow().groups, &m).contains(&group)
            });
            if place_group && svc.settings().auto_location {
                let mut next = svc.settings();
                next.auto_location = false;
                svc.update_settings(next).await?;
            }
            svc.select_inner(group, name).await
        })
    }

    fn select_inner(
        &self,
        group: String,
        name: String,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            // Show it at once; the refresh confirms.
            svc.0.proxies.send_modify(|p| {
                let mut next = (**p).clone();
                if let Some(g) = next.groups.iter_mut().find(|g| g.name == group) {
                    g.now = Some(name.clone());
                }
                *p = Arc::new(next);
            });
            let r = svc
                .controller()
                .select(&group, &name)
                .await
                .map_err(|e| e.to_string());
            // Open connections stay on the node they started on (mihomo keeps
            // them through a switch) and finish there: an automatic switch to a
            // faster place, or a pick by hand, never cuts a long stream. Only
            // the user's own Close ends them.
            svc.refresh_proxies_now().await;
            r
        })
    }

    fn set_testing(&self, names: &[String], on: bool) {
        self.0.proxies.send_modify(|p| {
            let mut next = (**p).clone();
            for n in names {
                if on {
                    next.testing.insert(n.clone());
                } else {
                    next.testing.remove(n);
                }
            }
            *p = Arc::new(next);
        });
    }

    fn set_delay(&self, name: &str, delay: u32) {
        self.0.proxies.send_modify(|p| {
            let mut next = (**p).clone();
            for g in &mut next.groups {
                for m in &mut g.members {
                    if m.name == name {
                        m.delay = Some(delay);
                        m.alive = delay > 0;
                    }
                }
            }
            *p = Arc::new(next);
        });
    }

    /// Tests one node; its delay in ms (0: failed).
    pub fn test_node(
        &self,
        name: String,
        url: Option<String>,
    ) -> impl Future<Output = Result<u32, String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let s = svc.settings();
            let url = url.unwrap_or(s.test_url.clone());
            svc.set_testing(std::slice::from_ref(&name), true);
            let r = svc.controller().delay(&name, &url, s.test_timeout_ms).await;
            svc.set_testing(std::slice::from_ref(&name), false);
            let delay = r.as_ref().copied().unwrap_or(0);
            svc.set_delay(&name, delay);
            r.map_err(|e| e.to_string())
        })
    }

    /// Tests every member of a group at once.
    pub fn test_group(&self, group: String) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let s = svc.settings();
            let (members, url) = {
                let p = svc.0.proxies.borrow();
                let g = p
                    .groups
                    .iter()
                    .find(|g| g.name == group)
                    .ok_or("No such group")?;
                (
                    g.members
                        .iter()
                        .filter(|m| !m.is_group)
                        .map(|m| m.name.clone())
                        .collect::<Vec<_>>(),
                    // Groups that test themselves keep their own URL; mihomo reports a
                    // default one for select groups too, which the user's setting overrides.
                    match g.kind.as_str() {
                        "URLTest" | "Fallback" | "LoadBalance" => g
                            .test_url
                            .clone()
                            .filter(|u| !u.is_empty())
                            .unwrap_or(s.test_url.clone()),
                        _ => s.test_url.clone(),
                    },
                )
            };
            let mut marks = members.clone();
            marks.push(group.clone());
            svc.set_testing(&marks, true);
            let r = svc
                .controller()
                .group_delay(&group, &url, s.test_timeout_ms)
                .await;
            svc.set_testing(&marks, false);
            match r {
                Ok(delays) => {
                    for m in &members {
                        svc.set_delay(m, delays.get(m).copied().unwrap_or(0));
                    }
                    svc.refresh_proxies_now().await;
                    Ok(())
                }
                Err(e) => Err(e.to_string()),
            }
        })
    }

    pub fn update_provider(
        &self,
        name: String,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.controller()
                .update_proxy_provider(&name)
                .await
                .map_err(|e| e.to_string())?;
            svc.refresh_proxies_now().await;
            Ok(())
        })
    }

    pub fn healthcheck_provider(
        &self,
        name: String,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.set_testing(std::slice::from_ref(&name), true);
            let r = svc
                .controller()
                .healthcheck_provider(&name)
                .await
                .map_err(|e| e.to_string());
            svc.set_testing(std::slice::from_ref(&name), false);
            svc.refresh_proxies_now().await;
            r
        })
    }

    // --- Connections, rules, DNS ---------------------------------------------------

    pub fn close_connection(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.controller()
                .close_connection(&id)
                .await
                .map_err(|e| e.to_string())
        })
    }

    pub fn close_all_connections(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.controller()
                .close_all_connections()
                .await
                .map_err(|e| e.to_string())
        })
    }

    pub fn rules(&self) -> impl Future<Output = Result<Vec<Rule>, String>> + 'static {
        let svc = self.clone();
        self.run(async move { svc.controller().rules().await.map_err(|e| e.to_string()) })
    }

    pub fn rule_providers(
        &self,
    ) -> impl Future<Output = Result<Vec<RuleProvider>, String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.controller()
                .rule_providers()
                .await
                .map_err(|e| e.to_string())
        })
    }

    pub fn update_rule_provider(
        &self,
        name: String,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.controller()
                .update_rule_provider(&name)
                .await
                .map_err(|e| e.to_string())
        })
    }

    pub fn dns_query(
        &self,
        name: String,
        kind: String,
    ) -> impl Future<Output = Result<DnsResponse, String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.controller()
                .dns_query(&name, &kind)
                .await
                .map_err(|e| e.to_string())
        })
    }

    pub fn flush_fakeip(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.controller()
                .flush_fakeip()
                .await
                .map_err(|e| e.to_string())
        })
    }

    pub fn update_geo(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.modify(|s| {
                s.task = Some(Task {
                    label: "Updating geo databases".into(),
                    progress: None,
                })
            });
            let r = svc
                .controller()
                .update_geo()
                .await
                .map_err(|e| e.to_string());
            svc.modify(|s| s.task = None);
            r
        })
    }

    // --- Kairo's own updates ------------------------------------------------------

    pub(crate) fn set_update(&self, status: crate::update::UpdateStatus) {
        self.modify(|s| s.update = status);
    }

    pub(crate) fn update_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.0.update_lock
    }

    /// The ways to reach GitHub, best first: through the core when it runs with
    /// a profile (many networks need it), then directly.
    pub(crate) fn download_routes(&self) -> Vec<Option<String>> {
        let s = self.0.state.borrow();
        let via_core = (s.core.is_running() && s.profiles.active.is_some())
            .then(|| format!("http://127.0.0.1:{}", s.settings.mixed_port));
        match via_core {
            Some(p) => vec![Some(p), None],
            None => vec![None],
        }
    }

    /// What the launcher calls to quit the app (after a clean shutdown).
    pub fn set_quit_hook(&self, quit: Arc<dyn Fn() + Send + Sync>) {
        *self.0.quit.lock().expect("quit hook") = Some(quit);
    }

    /// Looks for a Kairo update now.
    pub fn check_for_update(
        &self,
    ) -> impl Future<Output = Result<Option<String>, String>> + 'static {
        let svc = self.clone();
        self.run(async move { crate::update::check(&svc).await })
    }

    /// Downloads the update on offer ([`crate::UpdateStatus::Available`]):
    /// `Available` → `Downloading` → `Ready`. A failure puts it back to
    /// `Available` and tells the person in a notice; the future still
    /// resolves `Ok` then (read the state). Asked again meanwhile, it does
    /// nothing.
    pub fn download_update(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move { crate::update::download(&svc).await })
    }

    /// Installs the downloaded update and restarts into it.
    pub fn restart_to_update(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let _busy = svc.0.update_lock.lock().await;
            let previous = svc.snapshot().update;
            let crate::update::UpdateStatus::Ready { version } = previous.clone() else {
                return Err("No update is ready".into());
            };
            svc.set_update(crate::update::UpdateStatus::Installing { version });
            let exe = match crate::update::install(&svc).await {
                Ok(exe) => exe,
                Err(e) => {
                    svc.set_update(previous);
                    return Err(e);
                }
            };
            crate::update::relaunch(&exe)?;
            let quit = svc.0.quit.lock().expect("quit hook").clone();
            match quit {
                Some(quit) => quit(),
                None => {
                    svc.shutdown();
                    std::process::exit(0);
                }
            }
            Ok(())
        })
    }
}

/// Unix seconds now.
pub fn unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The local wall clock as `HH:MM:SS`.
pub fn clock() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let offset = local_offset_seconds();
    let t = (now + offset).rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", t / 3600, t % 3600 / 60, t % 60)
}

/// The local time zone's offset from UTC, in seconds.
pub fn local_offset_seconds() -> i64 {
    #[cfg(unix)]
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm.tm_gmtoff
    }
    #[cfg(not(unix))]
    {
        0
    }
}
