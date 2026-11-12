//! The wire between `kairo-server` and its page: JSON text frames on one
//! WebSocket. The page sends [`ClientMsg`]s (a [`Command`] and its id); the
//! server answers each with a [`ServerMsg::Reply`] and pushes what changes
//! (the state, the live numbers, the lists) as it changes, never polled.
//!
//! What crosses is the model, redacted by the server first: no subscription
//! link, no controller secret (see `kairo-server`'s `redact`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::activity::Tally;
use crate::ai::AiSettings;
use crate::model::{Connections, LogEntry};
use crate::routes::{RouteExit, RouteKind};
use crate::settings::{Mode, Settings};
use crate::stats::Store as Stats;
use crate::view::{Live, Proxies, State};

/// The protocol's version: a page and a server that differ reload the page.
pub const VERSION: u32 = 1;

/// What stands in for a secret the page is not sent (the controller's).
pub const HIDDEN: &str = "\u{2022}hidden\u{2022}";

/// One request of the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientMsg {
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

/// What the page asks the service to do: the service's own commands, one
/// for one, plus the few a browser needs in place of a local file or path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum Command {
    Connect { on: bool },
    StartCore,
    StopCore,
    RestartCore,
    SetMode { mode: Mode },
    SetLocation { name: Option<String> },
    TestPlaces,
    UpdateSettings { settings: Box<Settings> },
    SetSystemProxy { on: bool },
    SetTun { on: bool },
    GrantTun,
    // Subscriptions.
    AddRemote { url: String, name: Option<String> },
    /// A YAML file the page read from the person's device.
    AddYaml { name: String, text: String },
    UpdateProfile { id: String },
    UpdateAllProfiles,
    Activate { id: String },
    RenameProfile { id: String, name: String, interval_hours: Option<u32>, url: Option<String> },
    DeleteProfile { id: String },
    DismissFailure { id: String },
    // Groups, nodes, providers.
    RefreshProxies,
    Select { group: String, name: String },
    TestNode { name: String, url: Option<String> },
    TestGroup { group: String },
    UpdateProvider { name: String },
    HealthcheckProvider { name: String },
    // Connections.
    CloseConnection { id: String },
    CloseAllConnections,
    // Inspect.
    Rules,
    RuleProviders,
    UpdateRuleProvider { name: String },
    DnsQuery { name: String, kind: String },
    FlushFakeip,
    UpdateGeo,
    /// When the geo databases were last written (unix seconds), if at all.
    GeoUpdated,
    ClearLogs,
    ClearStats,
    // Routes.
    SetRoute { kind: RouteKind, exit: RouteExit },
    RemoveRoute { id: String },
    // AI and Downloads.
    SetAiExit { name: String },
    AiFollowLocation,
    DismissAiAlert,
    UpdateAi { ai: Box<AiSettings> },
    CheckAi,
    SetDownloadsExit { name: String },
    DownloadsFollowFastest,
    TestMirror,
    SetMirror { on: bool },
    // Pictures: what the service knows of an app's or a site's.
    Icon { kind: IconKind, name: String },
    // Kairo's own update (of the server).
    CheckForUpdate,
    DownloadUpdate,
    RestartToUpdate,
    DismissUpdateFailure,
    /// A new controller secret, made by the server (the page never sees it).
    NewSecret,
    /// Free ports for the core, starting at these (the "ports busy" fix).
    FreePorts { mixed: u16, controller: u16 },
    // The web's own: who is signed in where.
    Sessions,
    RevokeSession { id: String },
    RevokeOtherSessions,
    SignOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconKind {
    App,
    Site,
}

/// What the server knows of a picture: where the page loads it (a path on
/// the server, behind the session) and the app's own name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IconInfo {
    pub url: Option<String>,
    pub label: Option<String>,
}

/// A signed-in browser, for Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    /// The browser, as it named itself.
    pub agent: String,
    /// Where it signed in from.
    pub address: String,
    pub created: i64,
    pub last_seen: i64,
    /// This browser.
    pub current: bool,
}

/// What the server tells a page once, when it connects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// Kairo's version on the server.
    pub version: String,
    /// The machine's name, for saying which machine a switch changes.
    pub machine: String,
    /// Its system (`linux`, `macos`, `windows`).
    pub os: String,
    /// Kairo's folder there.
    pub home: String,
    /// Its time zone, as seconds east of UTC.
    pub offset: i64,
    /// The shell lines that send a terminal through Kairo there.
    pub terminal_env: String,
}

/// One message of the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum ServerMsg {
    Hello(Box<Hello>),
    Reply {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        err: Option<String>,
    },
    State(Box<State>),
    Live(Box<Live>),
    Connections(Arc<Connections>),
    Proxies(Arc<Proxies>),
    Activity(Arc<Tally>),
    Stats(Arc<Stats>),
    /// New lines of the core's log (`reset`: the log was cleared first).
    Logs { reset: bool, lines: Vec<LogEntry>, seq: u64 },
    IconsRev(u64),
    /// Everything the page needs has been sent once: the pages may show.
    Synced,
    /// This session ended (signed out, or revoked from another browser).
    SignedOut,
}

impl ServerMsg {
    pub fn reply(id: u64, result: Result<serde_json::Value, String>) -> ServerMsg {
        match result {
            Ok(v) => ServerMsg::Reply { id, ok: Some(v), err: None },
            Err(e) => ServerMsg::Reply { id, ok: None, err: Some(e) },
        }
    }
}
