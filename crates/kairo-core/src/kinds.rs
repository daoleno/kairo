//! Apps by kind. An app is a destination service a person uses (Netflix,
//! Claude, Steam, Hugging Face), known by its domains and, for games and
//! launchers, by its program; the programs on this computer that talk to it
//! are its "used by". Every app has a kind (AI, Streaming, Games, Downloads,
//! Dev tools, or Everything else), every kind has an exit, and an app follows
//! its kind unless the person gives it an exit of its own.
//!
//! This file is the model: the kinds, the catalog of apps Kairo knows, what
//! the settings keep, how a connection is recognised as an app, and the
//! rules the choices become. Pure functions, testable without a core.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::ai::AiExit;
use crate::routes::{RouteKind, clean_app, clean_site, clean_target, process_rules};

// --- Kinds ----------------------------------------------------------------------------

/// A kind of app. `Other` is "Everything else": what no kind claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Ai,
    Streaming,
    Games,
    Downloads,
    DevTools,
    #[default]
    Other,
}

impl Kind {
    /// In the order they are shown.
    pub const ALL: [Kind; 6] = [
        Kind::Ai,
        Kind::Streaming,
        Kind::Games,
        Kind::Downloads,
        Kind::DevTools,
        Kind::Other,
    ];

    /// The kinds with an exit of their own that the person can point
    /// somewhere (AI's is pinned its own way, Everything else is the Location).
    pub const ROUTED: [Kind; 4] = [Kind::Streaming, Kind::Games, Kind::Downloads, Kind::DevTools];

    /// The kind's name in the language shown.
    pub fn title(self) -> String {
        crate::tr!(match self {
            Kind::Ai => "model.kind.ai",
            Kind::Streaming => "model.kind.streaming",
            Kind::Games => "model.kind.games",
            Kind::Downloads => "model.kind.downloads",
            Kind::DevTools => "model.kind.dev_tools",
            Kind::Other => "model.kind.other",
        })
    }

    /// The kind's name inside a sentence. Chinese puts a space between
    /// Chinese and Latin letters, so there a Latin name ("AI") gets one on
    /// each side; trim the sentence after.
    pub fn in_sentence(self) -> String {
        let title = self.title();
        if title.is_ascii() && crate::i18n::language().starts_with("zh") {
            format!(" {title} ")
        } else {
            title
        }
    }

    /// The hidden group Kairo makes for the kind when it needs one.
    pub fn group(self) -> &'static str {
        match self {
            Kind::Ai => crate::ai::KAIRO_AI_GROUP,
            Kind::Streaming => "Kairo Streaming",
            Kind::Games => "Kairo Games",
            Kind::Downloads => crate::downloads::KAIRO_DOWNLOADS_GROUP,
            Kind::DevTools => "Kairo Dev tools",
            Kind::Other => "",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Kind::Ai => "ai",
            Kind::Streaming => "streaming",
            Kind::Games => "games",
            Kind::Downloads => "downloads",
            Kind::DevTools => "devTools",
            Kind::Other => "other",
        }
    }

    /// Words in a subscription's group names that say a group is for this
    /// kind (an adopted group, as the subscription's AI group is).
    fn group_words(self) -> &'static [&'static str] {
        match self {
            Kind::Streaming => &["streaming", "stream", "media", "流媒体", "影视"],
            Kind::Games => &["game", "games", "gaming", "游戏"],
            Kind::DevTools => &["dev", "developer", "开发"],
            _ => &[],
        }
    }
}

// --- The catalog ------------------------------------------------------------------------

/// An app Kairo knows by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogApp {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    /// Domain suffixes that are this app.
    pub suffixes: &'static [&'static str],
    /// Program names that are this app (games and their launchers).
    pub programs: &'static [&'static str],
    /// Rule set / geosite names a subscription uses for this app.
    pub categories: &'static [&'static str],
    /// The site its logo is fetched from.
    pub site: &'static str,
    /// Goes direct unless told otherwise (AI run in China).
    pub direct: bool,
}

const fn app(
    id: &'static str,
    name: &'static str,
    kind: Kind,
    suffixes: &'static [&'static str],
    categories: &'static [&'static str],
) -> CatalogApp {
    CatalogApp {
        id,
        name,
        kind,
        suffixes,
        programs: &[],
        categories,
        site: suffixes[0],
        direct: false,
    }
}

/// Apps by kind that are not AI (AI comes from `ai::SERVICES` and `ai::DOMESTIC`).
const OTHERS: &[CatalogApp] = &[
    // Streaming.
    app("netflix", "Netflix", Kind::Streaming, &["netflix.com", "netflix.net", "nflxvideo.net", "nflximg.net", "nflximg.com", "nflxso.net", "nflxext.com"], &["netflix"]),
    app("youtube", "YouTube", Kind::Streaming, &["youtube.com", "youtu.be", "googlevideo.com", "ytimg.com", "youtube-nocookie.com", "youtubei.googleapis.com", "yt3.ggpht.com"], &["youtube"]),
    app("disneyplus", "Disney+", Kind::Streaming, &["disneyplus.com", "disney-plus.net", "dssott.com", "bamgrid.com"], &["disney", "disneyplus"]),
    app("primevideo", "Prime Video", Kind::Streaming, &["primevideo.com", "aiv-cdn.net", "aiv-delivery.net", "amazonvideo.com"], &["primevideo", "amazonprimevideo"]),
    app("max", "Max", Kind::Streaming, &["max.com", "hbomax.com", "hbo.com", "hbomaxcdn.com"], &["hbo", "hbomax"]),
    app("hulu", "Hulu", Kind::Streaming, &["hulu.com", "hulustream.com", "huluim.com"], &["hulu"]),
    app("spotify", "Spotify", Kind::Streaming, &["spotify.com", "scdn.co", "spotifycdn.com", "spotifycdn.net"], &["spotify"]),
    app("twitch", "Twitch", Kind::Streaming, &["twitch.tv", "ttvnw.net", "jtvnw.net", "twitchcdn.net"], &["twitch"]),
    // Games: the platforms by their domains, their clients by program.
    CatalogApp {
        programs: &["steam", "steamwebhelper", "steam.exe", "steamwebhelper.exe"],
        ..app("steam", "Steam", Kind::Games, &["steampowered.com", "steamcommunity.com", "steamstatic.com", "steamserver.net", "steamcontent.com", "steamgames.com"], &["steam"])
    },
    CatalogApp {
        programs: &["EpicGamesLauncher", "EpicGamesLauncher.exe"],
        ..app("epic", "Epic Games", Kind::Games, &["epicgames.com", "epicgames.dev", "unrealengine.com", "epicgames.net"], &["epicgames", "epic"])
    },
    CatalogApp {
        programs: &["Battle.net", "Battle.net.exe"],
        ..app("battlenet", "Battle.net", Kind::Games, &["battle.net", "blizzard.com", "blizzard.net"], &["blizzard"])
    },
    CatalogApp {
        programs: &["RiotClientServices", "RiotClientServices.exe"],
        ..app("riot", "Riot Games", Kind::Games, &["riotgames.com", "leagueoflegends.com", "playvalorant.com", "riotcdn.net"], &["riot"])
    },
    app("playstation", "PlayStation Network", Kind::Games, &["playstation.com", "playstation.net", "sonyentertainmentnetwork.com"], &["playstation"]),
    app("xbox", "Xbox network", Kind::Games, &["xboxlive.com", "xbox.com"], &["xbox"]),
    // Downloads: the big files (more specific hosts than the dev tools').
    app("huggingface", "Hugging Face", Kind::Downloads, &["huggingface.co", "hf.co", "hf-mirror.com"], &["huggingface"]),
    app("pypi", "PyPI", Kind::Downloads, &["pypi.org", "files.pythonhosted.org"], &["pypi"]),
    app("pytorch", "PyTorch", Kind::Downloads, &["download.pytorch.org"], &[]),
    app("ollama", "Ollama models", Kind::Downloads, &["registry.ollama.ai", "ollama.com"], &[]),
    app("dockerhub", "Docker Hub", Kind::Downloads, &["registry-1.docker.io", "production.cloudflare.docker.com", "auth.docker.io"], &["docker"]),
    app("ghcr", "GitHub Packages", Kind::Downloads, &["ghcr.io", "pkg-containers.githubusercontent.com"], &[]),
    app("github-releases", "GitHub Releases", Kind::Downloads, &["objects.githubusercontent.com", "release-assets.githubusercontent.com"], &[]),
    // Dev tools: code hosting and package APIs.
    app("github", "GitHub", Kind::DevTools, &["github.com", "githubusercontent.com", "githubassets.com", "github.io"], &["github"]),
    app("gitlab", "GitLab", Kind::DevTools, &["gitlab.com", "gitlab-static.net"], &["gitlab"]),
    app("npm", "npm", Kind::DevTools, &["npmjs.com", "npmjs.org"], &["npmjs", "npm"]),
    app("crates", "crates.io", Kind::DevTools, &["crates.io"], &[]),
    app("go", "Go modules", Kind::DevTools, &["proxy.golang.org", "sum.golang.org", "pkg.go.dev", "go.dev"], &[]),
    app("jetbrains", "JetBrains", Kind::DevTools, &["jetbrains.com"], &["jetbrains"]),
    app("vscode-marketplace", "VS Code Marketplace", Kind::DevTools, &["marketplace.visualstudio.com", "vscode-cdn.net", "gallerycdn.vsassets.io", "vsassets.io"], &[]),
];

/// The rule-set names of the AI services (for those a subscription names
/// differently from Kairo's ids).
fn ai_categories(id: &str) -> &'static [&'static str] {
    match id {
        "claude" => &["anthropic", "claude"],
        "chatgpt" => &["openai", "chatgpt"],
        "gemini" => &["google-gemini", "gemini", "bard"],
        "cursor" => &["cursor"],
        "copilot" => &["github-copilot", "copilot"],
        "perplexity" => &["perplexity"],
        "grok" => &["xai", "grok"],
        _ => &[],
    }
}

/// The domestic AIs' ids, as the catalog names them.
pub fn domestic_id(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Every app Kairo knows: AI first (the services, then the domestic ones),
/// then the others.
pub fn catalog() -> &'static [CatalogApp] {
    static CATALOG: OnceLock<Vec<CatalogApp>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut all: Vec<CatalogApp> = crate::ai::SERVICES
            .iter()
            .map(|s| CatalogApp {
                id: s.id,
                name: s.name,
                kind: Kind::Ai,
                suffixes: s.suffixes,
                programs: &[],
                categories: ai_categories(s.id),
                site: s.suffixes[0],
                direct: false,
            })
            .collect();
        for (name, sfx) in crate::ai::DOMESTIC {
            let id: &'static str = Box::leak(domestic_id(name).into_boxed_str());
            all.push(CatalogApp {
                id,
                name,
                kind: Kind::Ai,
                suffixes: sfx,
                programs: &[],
                categories: &[],
                site: sfx[0],
                direct: true,
            });
        }
        all.extend_from_slice(OTHERS);
        all
    })
}

pub fn catalog_app(id: &str) -> Option<&'static CatalogApp> {
    catalog().iter().find(|a| a.id == id)
}

fn host_in(host: &str, suffix: &str) -> bool {
    host == suffix || host.ends_with(&format!(".{suffix}"))
}

/// The catalog app a host belongs to: the longest matching suffix wins, so
/// `objects.githubusercontent.com` is GitHub Releases, not GitHub.
pub fn app_of_host(host: &str) -> Option<&'static CatalogApp> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    catalog()
        .iter()
        .flat_map(|a| a.suffixes.iter().map(move |s| (a, *s)))
        .filter(|(_, s)| host_in(&host, s))
        .max_by_key(|(_, s)| s.len())
        .map(|(a, _)| a)
}

/// Browsers are never an app: they are what apps are used by.
pub fn is_browser(process: &str) -> bool {
    let p = process.to_ascii_lowercase();
    let p = p.trim_end_matches(".exe");
    [
        "chrome", "google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "firefox",
        "firefox-esr", "msedge", "microsoft-edge", "safari", "brave", "brave-browser", "opera", "vivaldi",
        "arc", "zen", "librewolf", "com.apple.webkit.networking",
    ]
    .contains(&p)
}

/// The game folder in a Steam library path (`…/steamapps/common/Counter-Strike Global Offensive/…`).
pub fn steam_game(process_path: &str) -> Option<String> {
    let low = process_path.replace('\\', "/");
    let i = low.to_ascii_lowercase().find("/steamapps/common/")?;
    let rest = &low[i + "/steamapps/common/".len()..];
    let folder = rest.split('/').next()?.trim();
    (!folder.is_empty() && rest.contains('/')).then(|| folder.to_owned())
}

/// The kind a rule set's or geosite's name stands for, by its words.
pub fn kind_of_category(name: &str) -> Option<Kind> {
    let low = name.to_ascii_lowercase();
    let words: Vec<&str> = low.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &[&str]| words.iter().any(|x| w.contains(x));
    if has(&["game", "games", "gaming"]) {
        Some(Kind::Games)
    } else if has(&["streaming", "stream", "media", "video", "tv"]) {
        Some(Kind::Streaming)
    } else if has(&["ai", "openai", "llm", "anthropic"]) {
        Some(Kind::Ai)
    } else if has(&["dev", "developer", "git"]) {
        Some(Kind::DevTools)
    } else if has(&["download", "downloads"]) {
        Some(Kind::Downloads)
    } else {
        None
    }
}

/// What makes an app "in use" for Home and the checks: this many bytes in
/// the last 7 days (or a connection open now).
pub const USED_BYTES: u64 = 1_000_000;

// --- What the settings keep -------------------------------------------------------------

/// The AI kind's exit: a pinned node, or following the Location.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiKind {
    pub exit: Option<AiExit>,
    /// Follows the Location (no exit pinned): the person's choice, or the
    /// state after a pinned exit went away.
    pub follow: bool,
}

/// The exit of a kind other than AI: as the subscription routes it (no rule
/// of Kairo's), or Kairo's: a pinned node, the fastest, or the Location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KindExit {
    /// No rule of Kairo's: the subscription decides, as it did before.
    pub subscription: bool,
    pub exit: Option<AiExit>,
    /// Kairo keeps it on the fastest node (unless one is pinned); else the Location.
    pub fastest: bool,
}

impl Default for KindExit {
    fn default() -> Self {
        Self::SUBSCRIPTION
    }
}

impl KindExit {
    pub const SUBSCRIPTION: KindExit = KindExit {
        subscription: true,
        exit: None,
        fastest: true,
    };
    pub const FASTEST: KindExit = KindExit {
        subscription: false,
        exit: None,
        fastest: true,
    };
    pub const LOCATION: KindExit = KindExit {
        subscription: false,
        exit: None,
        fastest: false,
    };
}

/// Every kind's exit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Kinds {
    pub ai: AiKind,
    pub streaming: KindExit,
    pub games: KindExit,
    pub downloads: KindExit,
    pub dev_tools: KindExit,
    /// Mirrors for model hubs and package indexes (only after the test passed).
    pub mirror: bool,
}

impl Default for Kinds {
    fn default() -> Self {
        Self {
            ai: AiKind::default(),
            // New kinds start where the subscription sends them: nothing
            // changes until the person picks a place.
            streaming: KindExit::SUBSCRIPTION,
            games: KindExit::SUBSCRIPTION,
            downloads: KindExit::FASTEST,
            dev_tools: KindExit::SUBSCRIPTION,
            mirror: false,
        }
    }
}

impl Kinds {
    /// The exit of a routed kind (Streaming, Games, Downloads, Dev tools).
    pub fn exit(&self, kind: Kind) -> Option<&KindExit> {
        match kind {
            Kind::Streaming => Some(&self.streaming),
            Kind::Games => Some(&self.games),
            Kind::Downloads => Some(&self.downloads),
            Kind::DevTools => Some(&self.dev_tools),
            Kind::Ai | Kind::Other => None,
        }
    }

    pub fn exit_mut(&mut self, kind: Kind) -> Option<&mut KindExit> {
        match kind {
            Kind::Streaming => Some(&mut self.streaming),
            Kind::Games => Some(&mut self.games),
            Kind::Downloads => Some(&mut self.downloads),
            Kind::DevTools => Some(&mut self.dev_tools),
            Kind::Ai | Kind::Other => None,
        }
    }
}

/// Where one app's traffic goes, when it is not where its kind's goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppExit {
    /// Its kind's exit. Never stored on its own.
    Kind,
    /// No rule of Kairo's: the subscription decides.
    Subscription,
    Direct,
    Block,
    /// The Location.
    Everything,
    /// Another kind's exit.
    Of(Kind),
    /// One node of the subscription, by name.
    Node(String),
}

/// The person's choice for one app: its own exit, another kind, or an app of
/// their own (a site or a program Kairo did not know).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppChoice {
    /// A catalog id, or `site:<domain>` / `app:<program>` for an app of the person's.
    pub id: String,
    pub kind: Kind,
    /// What makes a connection this app, for an app of the person's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<RouteKind>,
    pub exit: AppExit,
}

impl AppChoice {
    /// A choice for an app of the person's (`kind` checked).
    pub fn custom(matcher: RouteKind, kind: Kind, exit: AppExit) -> Result<AppChoice, String> {
        let matcher = matcher.cleaned()?;
        if let AppExit::Node(n) = &exit {
            clean_target(n)?;
        }
        Ok(AppChoice {
            id: matcher.id(),
            kind,
            matcher: Some(matcher),
            exit,
        })
    }

    /// The app's name: the catalog's, or the site or program.
    pub fn name(&self) -> String {
        if let Some(a) = catalog_app(&self.id) {
            return a.name.to_owned();
        }
        match &self.matcher {
            Some(RouteKind::Site { domain }) => domain.clone(),
            Some(RouteKind::App { name }) => name.clone(),
            None => self.id.clone(),
        }
    }

    /// Nothing differs from what the catalog says: the choice need not be kept.
    pub fn is_default(&self) -> bool {
        match catalog_app(&self.id) {
            Some(a) => self.kind == a.kind && self.exit == default_exit(a),
            None => false,
        }
    }
}

/// A catalog app's exit when the person has not chosen one.
pub fn default_exit(a: &CatalogApp) -> AppExit {
    if a.direct { AppExit::Direct } else { AppExit::Kind }
}

/// Puts a choice in the list: one per app, and a catalog app back to its
/// defaults leaves the list.
pub fn upsert(apps: &mut Vec<AppChoice>, choice: AppChoice) {
    apps.retain(|a| a.id != choice.id);
    if !choice.is_default() {
        apps.push(choice);
    }
}

/// A stored list made safe to use: values checked again (a hand-edited
/// file), unknown catalog ids and duplicates dropped.
pub fn sanitize(apps: Vec<AppChoice>) -> Vec<AppChoice> {
    let mut out = Vec::new();
    for a in apps {
        let ok = match &a.matcher {
            Some(m) => AppChoice::custom(m.clone(), a.kind, a.exit.clone()).map(|c| c.id == a.id).unwrap_or(false),
            None => catalog_app(&a.id).is_some(),
        };
        let node_ok = match &a.exit {
            AppExit::Node(n) => clean_target(n).is_ok(),
            _ => true,
        };
        if ok && node_ok {
            upsert(&mut out, a);
        } else {
            log::warn!("a stored app choice is dropped: {}", a.id);
        }
    }
    out
}

// --- Recognising a connection -----------------------------------------------------------

/// How an app was told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum How {
    /// An app of the person's.
    Own,
    /// The catalog, by the host.
    Domain,
    /// The catalog or a game folder, by the program.
    Program,
    /// The rule set or geosite the core matched.
    Rule,
}

/// What a connection is, as an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recognised {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub how: How,
}

/// What the core says of a connection, as recognition reads it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Seen<'a> {
    pub host: &'a str,
    pub process: &'a str,
    pub process_path: &'a str,
    pub rule: &'a str,
    pub rule_payload: &'a str,
}

/// The app a connection belongs to, in this order: the person's own apps,
/// the catalog by host, the catalog by program (and Steam's game folders),
/// then the rule set or geosite the core matched it with. `None`: not
/// recognised (it is shown, if it moves enough, under "Seen, not sorted").
/// A person's choice of kind for a catalog app is applied too.
pub fn recognise(seen: &Seen, apps: &[AppChoice]) -> Option<Recognised> {
    let host = seen.host.trim_end_matches('.').to_ascii_lowercase();
    let kind_for = |id: &str, default: Kind| {
        apps.iter().find(|a| a.id == id).map(|a| a.kind).unwrap_or(default)
    };
    // 1. The person's own apps: a program by its name, a site and what is under it.
    for a in apps {
        let hit = match &a.matcher {
            Some(RouteKind::App { name }) => {
                !seen.process.is_empty()
                    && (seen.process.eq_ignore_ascii_case(name)
                        || seen.process.eq_ignore_ascii_case(&format!("{name}.exe")))
            }
            Some(RouteKind::Site { domain }) => !host.is_empty() && host_in(&host, domain),
            None => false,
        };
        if hit {
            return Some(Recognised { id: a.id.clone(), name: a.name(), kind: a.kind, how: How::Own });
        }
    }
    // 2. The catalog, by the host.
    if let Some(c) = app_of_host(&host) {
        return Some(Recognised { id: c.id.into(), name: c.name.into(), kind: kind_for(c.id, c.kind), how: How::Domain });
    }
    // 3. The catalog, by the program; a game in a Steam library.
    if !seen.process.is_empty() && !is_browser(seen.process)
        && let Some(c) = catalog().iter().find(|c| c.programs.iter().any(|p| p.eq_ignore_ascii_case(seen.process)))
    {
        return Some(Recognised { id: c.id.into(), name: c.name.into(), kind: kind_for(c.id, c.kind), how: How::Program });
    }
    if let Some(game) = steam_game(seen.process_path) {
        return Some(Recognised {
            id: format!("game:{}", game.to_ascii_lowercase()),
            name: game,
            kind: Kind::Games,
            how: How::Program,
        });
    }
    // 4. The rule set or geosite the subscription's rule matched it with.
    if matches!(seen.rule.to_ascii_uppercase().as_str(), "GEOSITE" | "RULE-SET" | "RULESET") && !seen.rule_payload.is_empty() {
        let payload = seen.rule_payload.to_ascii_lowercase();
        let payload = payload.trim_start_matches("geosite:").trim_start_matches("category-");
        if let Some(c) = catalog().iter().find(|c| c.categories.contains(&payload)) {
            return Some(Recognised { id: c.id.into(), name: c.name.into(), kind: kind_for(c.id, c.kind), how: How::Rule });
        }
        if let Some(kind) = kind_of_category(seen.rule_payload)
            && !host.is_empty()
        {
            let site = crate::activity::site_of(&host);
            let id = format!("site:{site}");
            return Some(Recognised { kind: kind_for(&id, kind), id, name: site, how: How::Rule });
        }
    }
    None
}

// --- The rules -----------------------------------------------------------------------------

/// The groups each kind's traffic goes to in the running config. `None`: no
/// rule of Kairo's for that kind (it goes as the subscription routes it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Targets {
    pub kinds: BTreeMap<Kind, String>,
    /// The Location.
    pub everything: Option<String>,
}

impl Targets {
    fn of(&self, kind: Kind) -> Option<&str> {
        self.kinds.get(&kind).map(String::as_str)
    }
}

/// The rules for every app, and the ids of the apps left out (their exit is
/// not in the config).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Generated {
    pub rules: Vec<String>,
    pub skipped: Vec<String>,
}

/// The target of an app, in the config's names. `None`: no rule.
fn target_of<'a>(
    exit: &'a AppExit,
    kind: Kind,
    targets: &'a Targets,
    node_exists: &dyn Fn(&str) -> bool,
) -> Result<Option<&'a str>, ()> {
    Ok(match exit {
        AppExit::Kind => targets.of(kind),
        AppExit::Subscription => None,
        AppExit::Direct => Some("DIRECT"),
        AppExit::Block => Some("REJECT"),
        AppExit::Everything => Some(targets.everything.as_deref().ok_or(())?),
        AppExit::Of(k) => Some(targets.of(*k).or(targets.everything.as_deref()).ok_or(())?),
        AppExit::Node(n) => {
            if node_exists(n) {
                Some(n.as_str())
            } else {
                return Err(());
            }
        }
    })
}

/// Writes the rules for the apps. In this order:
///
/// 1. apps with an exit of their own (the person's choices, and the domestic
///    AIs' Direct), programs first, then sites (the longer name first);
/// 2. every other app on its kind's exit, programs first, then sites, the
///    longer name first, so `objects.githubusercontent.com` (Downloads) is
///    read before `githubusercontent.com` (Dev tools);
/// 3. the games in Steam libraries, by their folder.
///
/// An app whose kind goes as the subscription routes it gets no rule. An app
/// whose exit is not in the config is left out (with a note in the log), so
/// the config always stands. `all_traffic` ("All Traffic") leaves out the
/// catalog's Direct for the domestic AIs, as it always did; so does
/// `only_own` (the kinds are off: only the person's own choices are rules).
pub fn rules(
    apps: &[AppChoice],
    targets: &Targets,
    all_traffic: bool,
    only_own: bool,
    node_exists: &dyn Fn(&str) -> bool,
) -> Generated {
    struct Line {
        own: bool,
        program: bool,
        dots: usize,
        text: String,
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut skipped = Vec::new();
    let mut push_app = |id: &str, own: bool, programs: &[String], sites: &[String], target: &str| {
        for p in programs {
            match clean_app(p) {
                Ok(n) if n == *p => {
                    for r in process_rules(&n, target, cfg!(windows)) {
                        lines.push(Line { own, program: true, dots: 0, text: r });
                    }
                }
                _ => log::warn!("app {id}: the program name is not valid"),
            }
        }
        for s in sites {
            match clean_site(s) {
                Ok(d) if d == *s => lines.push(Line {
                    own,
                    program: false,
                    dots: d.matches('.').count(),
                    text: format!("DOMAIN-SUFFIX,{d},{target}"),
                }),
                _ => log::warn!("app {id}: the site name is not valid"),
            }
        }
    };
    // The catalog's apps, with the person's choices for them.
    for c in catalog() {
        let choice = apps.iter().find(|a| a.id == c.id);
        let kind = choice.map(|a| a.kind).unwrap_or(c.kind);
        let exit = choice.map(|a| a.exit.clone()).unwrap_or_else(|| default_exit(c));
        if (all_traffic || only_own) && c.direct && choice.is_none() {
            continue;
        }
        let own = !matches!(exit, AppExit::Kind);
        match target_of(&exit, kind, targets, node_exists) {
            Ok(Some(t)) if clean_target(t).is_ok() => {
                let programs: Vec<String> = c.programs.iter().map(|p| p.to_string()).collect();
                let sites: Vec<String> = c.suffixes.iter().map(|s| s.to_string()).collect();
                push_app(c.id, own, &programs, &sites, t);
            }
            Ok(_) => {}
            Err(()) => {
                log::warn!("app {} is not routed: its exit is not in the config", c.id);
                skipped.push(c.id.to_owned());
            }
        }
    }
    // The person's own apps.
    for a in apps.iter().filter(|a| a.matcher.is_some()) {
        match target_of(&a.exit, a.kind, targets, node_exists) {
            Ok(Some(t)) if clean_target(t).is_ok() => {
                let own = !matches!(a.exit, AppExit::Kind);
                match a.matcher.as_ref().expect("filtered") {
                    RouteKind::App { name } => push_app(&a.id, own, std::slice::from_ref(name), &[], t),
                    RouteKind::Site { domain } => push_app(&a.id, own, &[], std::slice::from_ref(domain), t),
                }
            }
            Ok(_) => {}
            Err(()) => {
                log::warn!("app {} is not routed: its exit is not in the config", a.id);
                skipped.push(a.id.clone());
            }
        }
    }
    // Own exits first; programs before sites; the longer site first. Stable,
    // so the catalog's order holds between equals.
    lines.sort_by_key(|l| (!l.own, !l.program, std::cmp::Reverse(l.dots)));
    let mut out: Vec<String> = lines.into_iter().map(|l| l.text).collect();
    if let Some(t) = targets.of(Kind::Games).filter(|_| !only_own) {
        out.push(format!("PROCESS-PATH-REGEX,(?i)[/\\\\]steamapps[/\\\\]common[/\\\\],{t}"));
    }
    Generated { rules: out, skipped }
}

/// The subscription's own group for `kind`, when it has a selector whose
/// name says so (`Streaming`, `🎮 Games`): Kairo picks its place there, as
/// it does in an adopted AI group, rather than making one. Never the Location.
pub fn adoptable_group(groups: &[(String, bool)], kind: Kind, main: Option<&str>) -> Option<String> {
    let words = kind.group_words();
    groups
        .iter()
        .filter(|(name, select)| *select && Some(name.as_str()) != main)
        .find(|(name, _)| {
            let low = name.to_lowercase();
            low.split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .any(|w| words.contains(&w))
                || words.iter().any(|w| !w.is_ascii() && low.contains(w))
        })
        .map(|(n, _)| n.clone())
}

/// Where the subscription's own rules send a host: the target of its first
/// `DOMAIN`, `DOMAIN-SUFFIX` or `DOMAIN-KEYWORD` rule for it, else its
/// `MATCH`. Rule sets and geosites are not read (they would need the core's
/// files): a host they catch reads as going where `MATCH` goes.
pub fn subscription_target(rules: &[String], host: &str) -> Option<String> {
    let host = host.to_ascii_lowercase();
    for r in rules {
        let mut it = r.split(',').map(str::trim);
        let (Some(kind), Some(payload)) = (it.next(), it.next()) else { continue };
        let payload_l = payload.to_ascii_lowercase();
        let hit = match kind {
            "DOMAIN" => host == payload_l,
            "DOMAIN-SUFFIX" => host_in(&host, &payload_l),
            "DOMAIN-KEYWORD" => host.contains(&payload_l),
            "MATCH" => return Some(payload.to_owned()),
            _ => false,
        };
        if hit {
            return it.next().map(str::to_owned);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen<'a>(host: &'a str, process: &'a str) -> Seen<'a> {
        Seen { host, process, ..Default::default() }
    }

    #[test]
    fn the_catalog_has_unique_ids_and_ai_first() {
        let ids: Vec<&str> = catalog().iter().map(|a| a.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "duplicate ids");
        assert_eq!(catalog()[0].kind, Kind::Ai);
        assert!(catalog_app("deepseek").is_some_and(|a| a.direct && a.kind == Kind::Ai));
        // Every download host of the old list is an app of the Downloads kind.
        for h in crate::ai::DOWNLOADS {
            assert_eq!(app_of_host(h).map(|a| a.kind), Some(Kind::Downloads), "{h}");
        }
    }

    #[test]
    fn github_is_split_by_host() {
        assert_eq!(app_of_host("github.com").unwrap().id, "github");
        assert_eq!(app_of_host("raw.githubusercontent.com").unwrap().id, "github");
        assert_eq!(app_of_host("objects.githubusercontent.com").unwrap().id, "github-releases");
        assert_eq!(app_of_host("ghcr.io").unwrap().id, "ghcr");
        // Copilot's own host is AI, not GitHub's.
        assert_eq!(app_of_host("copilot-proxy.githubusercontent.com").unwrap().id, "copilot");
    }

    #[test]
    fn recognition_order_own_then_domain_then_program_then_rule() {
        let own = vec![AppChoice::custom(RouteKind::site("netflix.com"), Kind::Other, AppExit::Direct).unwrap()];
        // 1. An app of the person's wins over the catalog.
        let r = recognise(&seen("www.netflix.com", "chrome"), &own).unwrap();
        assert_eq!((r.id.as_str(), r.how), ("site:netflix.com", How::Own));
        // 2. The catalog by host (a browser is never the app).
        let r = recognise(&seen("www.netflix.com", "chrome"), &[]).unwrap();
        assert_eq!((r.id.as_str(), r.kind, r.how), ("netflix", Kind::Streaming, How::Domain));
        // 3. The catalog by program, when the host is unknown (a game server's address).
        let r = recognise(&seen("203.0.113.9", "steam"), &[]).unwrap();
        assert_eq!((r.id.as_str(), r.how), ("steam", How::Program));
        let r = recognise(
            &Seen {
                host: "203.0.113.9",
                process: "cs2",
                process_path: "/home/me/.steam/steam/steamapps/common/Counter-Strike Global Offensive/game/bin/linuxsteamrt64/cs2",
                ..Default::default()
            },
            &[],
        )
        .unwrap();
        assert_eq!((r.id.as_str(), r.name.as_str(), r.kind), ("game:counter-strike global offensive", "Counter-Strike Global Offensive", Kind::Games));
        // 4. The rule the core matched: a known name, then a kind by its words.
        let by_rule = |payload: &'static str| Seen { host: "cdn.example.net", rule: "GEOSITE", rule_payload: payload, ..Default::default() };
        let r = recognise(&by_rule("netflix"), &[]).unwrap();
        assert_eq!((r.id.as_str(), r.how), ("netflix", How::Rule));
        let r = recognise(&by_rule("category-games"), &[]).unwrap();
        assert_eq!((r.id.as_str(), r.kind), ("site:example.net", Kind::Games));
        // Nothing: not sorted.
        assert!(recognise(&seen("example.org", "curl"), &[]).is_none());
        assert!(recognise(&Seen { host: "example.org", rule: "DOMAIN-SUFFIX", rule_payload: "netflix", ..Default::default() }, &[]).is_none());
    }

    #[test]
    fn a_kind_chosen_for_a_catalog_app_is_recognised() {
        let apps = vec![AppChoice { id: "youtube".into(), kind: Kind::Other, matcher: None, exit: AppExit::Kind }];
        assert_eq!(recognise(&seen("www.youtube.com", "firefox"), &apps).unwrap().kind, Kind::Other);
    }

    #[test]
    fn categories_read_as_kinds() {
        assert_eq!(kind_of_category("category-games"), Some(Kind::Games));
        assert_eq!(kind_of_category("Streaming_Global"), Some(Kind::Streaming));
        assert_eq!(kind_of_category("category-ai-!cn"), Some(Kind::Ai));
        assert_eq!(kind_of_category("taiwan"), None);
        assert_eq!(kind_of_category("cn"), None);
    }

    fn targets(kinds: &[(Kind, &str)]) -> Targets {
        Targets {
            kinds: kinds.iter().map(|(k, g)| (*k, g.to_string())).collect(),
            everything: Some("Proxies".into()),
        }
    }

    fn position(rules: &[String], needle: &str) -> usize {
        rules.iter().position(|r| r == needle).unwrap_or_else(|| panic!("{needle} not in {rules:#?}"))
    }

    #[test]
    fn rules_order_own_exits_then_kinds_by_specificity() {
        let t = targets(&[
            (Kind::Ai, "Kairo AI"),
            (Kind::Downloads, "Kairo Downloads"),
            (Kind::DevTools, "Kairo Dev tools"),
            (Kind::Games, "Kairo Games"),
        ]);
        let apps = vec![
            AppChoice { id: "youtube".into(), kind: Kind::Streaming, matcher: None, exit: AppExit::Node("Tokyo 02".into()) },
            AppChoice::custom(RouteKind::app("yt-dlp"), Kind::Other, AppExit::Direct).unwrap(),
        ];
        let g = rules(&apps, &t, false, false, &|n| n == "Tokyo 02");
        let r = &g.rules;
        assert!(g.skipped.is_empty());
        // Own exits first, programs before sites.
        assert!(position(r, "PROCESS-NAME,yt-dlp,DIRECT") < position(r, "DOMAIN-SUFFIX,youtube.com,Tokyo 02"));
        // The domestic AIs' Direct is an own exit, before every kind's rule.
        assert!(position(r, "DOMAIN-SUFFIX,deepseek.com,DIRECT") < position(r, "DOMAIN-SUFFIX,claude.ai,Kairo AI"));
        // The longer host first: GitHub's release files before GitHub.
        assert!(
            position(r, "DOMAIN-SUFFIX,objects.githubusercontent.com,Kairo Downloads")
                < position(r, "DOMAIN-SUFFIX,githubusercontent.com,Kairo Dev tools")
        );
        // Copilot's host (AI) before GitHub's suffix.
        assert!(
            position(r, "DOMAIN-SUFFIX,copilot-proxy.githubusercontent.com,Kairo AI")
                < position(r, "DOMAIN-SUFFIX,githubusercontent.com,Kairo Dev tools")
        );
        // A kind that goes as the subscription routes it has no rule.
        assert!(!r.iter().any(|x| x.contains("netflix.com")));
        // Games: their programs and the Steam library, last.
        assert!(r.contains(&"PROCESS-NAME,steam,Kairo Games".to_owned()));
        assert!(r.last().unwrap().starts_with("PROCESS-PATH-REGEX,"));
    }

    #[test]
    fn a_missing_node_is_skipped_and_all_traffic_drops_the_domestic_direct() {
        let apps = vec![AppChoice { id: "netflix".into(), kind: Kind::Streaming, matcher: None, exit: AppExit::Node("Gone".into()) }];
        let g = rules(&apps, &targets(&[(Kind::Ai, "AI")]), true, false, &|_| false);
        assert_eq!(g.skipped, ["netflix"]);
        assert!(!g.rules.iter().any(|r| r.contains("deepseek")));
        assert!(!g.rules.iter().any(|r| r.contains("netflix")));
    }

    #[test]
    fn a_domestic_ai_on_the_subscriptions_choice_has_no_rule() {
        let apps = vec![AppChoice { id: "kimi".into(), kind: Kind::Ai, matcher: None, exit: AppExit::Subscription }];
        let g = rules(&apps, &targets(&[(Kind::Ai, "AI")]), false, false, &|_| true);
        assert!(!g.rules.iter().any(|r| r.contains("kimi.com")));
        assert!(g.rules.iter().any(|r| r == "DOMAIN-SUFFIX,deepseek.com,DIRECT"));
    }

    #[test]
    fn upsert_keeps_only_what_differs() {
        let mut apps = vec![];
        upsert(&mut apps, AppChoice { id: "netflix".into(), kind: Kind::Streaming, matcher: None, exit: AppExit::Direct });
        assert_eq!(apps.len(), 1);
        upsert(&mut apps, AppChoice { id: "netflix".into(), kind: Kind::Streaming, matcher: None, exit: AppExit::Kind });
        assert!(apps.is_empty());
        upsert(&mut apps, AppChoice { id: "deepseek".into(), kind: Kind::Ai, matcher: None, exit: AppExit::Direct });
        assert!(apps.is_empty());
        let s = sanitize(vec![
            AppChoice { id: "nope".into(), kind: Kind::Other, matcher: None, exit: AppExit::Direct },
            AppChoice { id: "site:x.com".into(), kind: Kind::Other, matcher: Some(RouteKind::site("x.com")), exit: AppExit::Block },
            AppChoice { id: "site:bad".into(), kind: Kind::Other, matcher: Some(RouteKind::site("not a site")), exit: AppExit::Block },
        ]);
        assert_eq!(s.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["site:x.com"]);
    }

    #[test]
    fn adopts_a_subscription_group_by_its_name() {
        let groups = vec![
            ("Proxies".to_owned(), true),
            ("🎬 Streaming".to_owned(), true),
            ("Netflix".to_owned(), true),
            ("🎮 游戏".to_owned(), true),
            ("Gaming auto".to_owned(), false),
        ];
        assert_eq!(adoptable_group(&groups, Kind::Streaming, Some("Proxies")).as_deref(), Some("🎬 Streaming"));
        assert_eq!(adoptable_group(&groups, Kind::Games, Some("Proxies")).as_deref(), Some("🎮 游戏"));
        assert_eq!(adoptable_group(&groups, Kind::DevTools, Some("Proxies")), None);
    }

    #[test]
    fn reads_where_the_subscription_sends_a_host() {
        let rules: Vec<String> = ["DOMAIN-SUFFIX,netflix.com,Netflix", "DOMAIN-KEYWORD,youtube,YouTube", "GEOIP,CN,DIRECT", "MATCH,Proxies"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(subscription_target(&rules, "www.netflix.com").as_deref(), Some("Netflix"));
        assert_eq!(subscription_target(&rules, "www.youtube.com").as_deref(), Some("YouTube"));
        assert_eq!(subscription_target(&rules, "example.org").as_deref(), Some("Proxies"));
    }

    #[test]
    fn steam_games_by_folder() {
        assert_eq!(steam_game(r"C:\Program Files (x86)\Steam\steamapps\common\Dota 2\game\bin\win64\dota2.exe").as_deref(), Some("Dota 2"));
        assert_eq!(steam_game("/usr/bin/steam"), None);
    }
}
