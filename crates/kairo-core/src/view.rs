//! What the service shows: the state, the live numbers and the groups. Plain
//! data, with no engine: the UI reads it in-process (the desktop) or over the
//! wire (`kairo serve` and the browser).

use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::ai_check;
use crate::model::{Group, ProxyProvider, Traffic};
use crate::profile::ProfileIndex;
use crate::settings::Settings;
use crate::tun::Privilege;

/// The core's state.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    pub id: u64,
    pub text: String,
    pub error: bool,
}

/// Work in progress that the UI shows (a download).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub label: String,
    /// 0..=1 when known.
    pub progress: Option<f32>,
}

/// What the AI layer knows: where AI goes and how the checks stand.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AiAlert {
    /// The pinned exit is no longer in the subscription; AI follows the
    /// Location until one is chosen.
    ExitGone { was: String },
}

/// A core replaced by the one Kairo ships.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoreReplaced {
    /// The version the old one reported, when it ran.
    pub from: Option<String>,
    pub to: String,
}

/// Everything that changes rarely.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct State {
    pub core: CoreStatus,
    /// The core's version.
    pub core_version: Option<String>,
    /// Kairo's own update, when there is one.
    pub update: UpdateStatus,
    pub settings: Settings,
    pub profiles: ProfileIndex,
    /// Profiles being downloaded.
    pub updating: HashSet<String>,
    /// Whether the system proxy currently points at Kairo.
    pub sysproxy_active: bool,
    pub sysproxy_supported: bool,
    pub tun_privilege: Option<Privilege>,
    /// The core in Kairo's folder was replaced at this start (another core
    /// came with an update). A replaced core has lost TUN's permission.
    #[serde(default)]
    pub core_replaced: Option<CoreReplaced>,
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
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Proxies {
    pub groups: Vec<Group>,
    pub providers: Vec<ProxyProvider>,
    /// Nodes and groups being tested.
    pub testing: HashSet<String>,
}

/// What the updater is doing, for the UI.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    /// When the last check found nothing newer (`HH:MM`, local time).
    UpToDate {
        checked: String,
    },
    /// A newer Kairo exists (its manifest is signed and checked); nothing is
    /// downloaded until `Service::download_update` is asked.
    Available {
        version: String,
    },
    Downloading {
        version: String,
        progress: Option<f32>,
    },
    /// Downloaded and verified: one tap installs it.
    Ready {
        version: String,
    },
    /// This installation cannot update itself: the release page.
    Manual {
        version: String,
        url: String,
    },
    Installing {
        version: String,
    },
    Failed {
        message: String,
    },
}
