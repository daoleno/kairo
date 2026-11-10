//! AI services as first-class things: which hosts belong to which service,
//! the rules that keep their traffic on one exit, local models off the proxy,
//! domestic AI direct and big downloads on an exit of their own. Pure functions over
//! the config's YAML, so they are testable without a core.
//!
//! The layer sits between the profile and the user's override: the override's
//! `prepend-rules` still win over it, and the profile's own rules come after.
//! The person's per-app and per-site routes ([`crate::routes`]) come right
//! after the never-proxied local block, before everything else of the layer.

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

use crate::places;
use crate::routes::{self, Route};

pub use crate::downloads::KAIRO_DOWNLOADS_GROUP;

/// The name of the group Kairo makes when the subscription has none for AI.
pub const KAIRO_AI_GROUP: &str = "Kairo AI";

/// An AI service Kairo knows by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiService {
    pub id: &'static str,
    pub name: &'static str,
    /// One or two letters for its tile.
    pub mono: &'static str,
    /// Domain suffixes that are this service.
    pub suffixes: &'static [&'static str],
    /// The API's host and a path to ask, with the statuses that mean
    /// "reachable" when asked without a key (a 401 is the API saying it wants
    /// one). Measured, not guessed: see the check's tests and the plan.
    pub api: Option<(&'static str, &'static str, &'static [u16])>,
    /// A Cloudflare-fronted host whose `/cdn-cgi/trace` says where the
    /// service sees the connection from. `None`: only an estimate is possible.
    pub trace_host: Option<&'static str>,
    /// Parts of an executable's path that say a tool belongs to the service
    /// (lower case); tools are told apart by their process, not their host.
    pub tools: &'static [&'static str],
}

/// The services, in the order they are shown.
pub const SERVICES: &[AiService] = &[
    AiService {
        id: "claude",
        name: "Claude",
        mono: "C",
        suffixes: &[
            "claude.ai",
            "claude.com",
            "anthropic.com",
            "claudeusercontent.com",
        ],
        api: Some(("api.anthropic.com", "/v1/models", &[401])),
        trace_host: Some("claude.ai"),
        tools: &["claude"],
    },
    AiService {
        id: "chatgpt",
        name: "ChatGPT",
        mono: "O",
        suffixes: &[
            "chatgpt.com",
            "openai.com",
            "oaistatic.com",
            "oaiusercontent.com",
        ],
        api: Some(("api.openai.com", "/v1/models", &[401])),
        trace_host: Some("chatgpt.com"),
        tools: &["codex", "chatgpt"],
    },
    AiService {
        id: "gemini",
        name: "Gemini",
        mono: "G",
        suffixes: &[
            "gemini.google.com",
            "generativelanguage.googleapis.com",
            "aistudio.google.com",
            "ai.google.dev",
            "makersuite.google.com",
            "notebooklm.google.com",
            "bard.google.com",
            "deepmind.google",
            "deepmind.com",
        ],
        api: Some((
            "generativelanguage.googleapis.com",
            "/v1beta/models",
            &[403],
        )),
        trace_host: None,
        tools: &["gemini"],
    },
    AiService {
        id: "cursor",
        name: "Cursor",
        mono: "Cu",
        suffixes: &["cursor.sh", "cursor.com", "cursorapi.com", "cursor-cdn.com"],
        api: Some(("api2.cursor.sh", "/", &[200])),
        trace_host: None,
        tools: &["cursor"],
    },
    AiService {
        id: "copilot",
        name: "Copilot",
        mono: "Co",
        suffixes: &[
            "githubcopilot.com",
            "copilot-proxy.githubusercontent.com",
            "copilot.microsoft.com",
        ],
        api: Some(("api.githubcopilot.com", "/", &[404])),
        trace_host: None,
        tools: &["copilot"],
    },
    AiService {
        id: "perplexity",
        name: "Perplexity",
        mono: "P",
        suffixes: &["perplexity.ai", "pplx.ai"],
        api: None,
        trace_host: Some("www.perplexity.ai"),
        tools: &["perplexity"],
    },
    AiService {
        id: "grok",
        name: "Grok",
        mono: "X",
        suffixes: &["grok.com", "x.ai"],
        api: Some(("api.x.ai", "/v1/models", &[401])),
        trace_host: Some("grok.com"),
        tools: &["grok"],
    },
];

/// AI services that run in China: reached directly, never through the exit.
pub const DOMESTIC: &[(&str, &[&str])] = &[
    (
        "Kimi",
        &["kimi.com", "kimi.ai", "moonshot.cn", "moonshot.ai"],
    ),
    ("DeepSeek", &["deepseek.com"]),
    ("Doubao", &["doubao.com", "volces.com", "volcengine.com"]),
    (
        "Tongyi",
        &["tongyi.aliyun.com", "qianwen.com", "dashscope.aliyuncs.com"],
    ),
    (
        "Zhipu",
        &["bigmodel.cn", "zhipuai.cn", "chatglm.cn", "z.ai"],
    ),
    ("Yuanbao", &["yuanbao.tencent.com", "hunyuan.tencent.com"]),
    ("MiniMax", &["minimaxi.com", "minimax.chat"]),
    ("Ernie", &["yiyan.baidu.com", "wenxin.baidu.com"]),
];

/// Big downloads, on their own exit (not the AI one): models, wheels, images
/// and release files. `hf-mirror.com` is here so that the mirror, when the
/// person uses it, goes the way Hugging Face does (and the mirror test
/// measures that way).
pub const DOWNLOADS: &[&str] = &[
    "huggingface.co",
    "hf.co",
    "hf-mirror.com",
    "files.pythonhosted.org",
    "pypi.org",
    "download.pytorch.org",
    "registry.ollama.ai",
    "ollama.com",
    "ghcr.io",
    "registry-1.docker.io",
    "production.cloudflare.docker.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// What stays on this machine (local models, the LAN): never proxied.
const LOCAL: &[&str] = &[
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
];
const LOCAL6: &[&str] = &["::1/128", "fc00::/7", "fe80::/10"];

/// The service a host belongs to.
pub fn service_of(host: &str) -> Option<&'static AiService> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    SERVICES.iter().find(|s| {
        s.suffixes
            .iter()
            .any(|x| host == *x || host.ends_with(&format!(".{x}")))
    })
}

/// The domestic AI a host belongs to.
pub fn domestic_of(host: &str) -> Option<&'static str> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    DOMESTIC.iter().find_map(|(name, sfx)| {
        sfx.iter()
            .any(|x| host == *x || host.ends_with(&format!(".{x}")))
            .then_some(*name)
    })
}

/// The service an executable belongs to, by its path (an editor or CLI).
pub fn tool_service(process_path: &str) -> Option<&'static AiService> {
    let low = process_path.to_ascii_lowercase();
    SERVICES
        .iter()
        .find(|s| s.tools.iter().any(|t| low.contains(t)))
}

/// The exit AI traffic is kept on: a node, remembered by what it is (its
/// server and port), so a renamed node is still found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiExit {
    pub name: String,
    pub fingerprint: String,
    /// Unix seconds when it was pinned.
    pub since: i64,
}

/// The settings of the AI layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiSettings {
    /// Master switch: off, the config is the subscription's own.
    pub enabled: bool,
    pub exit: Option<AiExit>,
    /// AI follows the Location (no exit pinned, and none pinned for it): the
    /// person's choice, or the state after a pinned exit went away.
    pub follow: bool,
    /// Kimi, DeepSeek, Doubao... go direct.
    pub domestic_direct: bool,
    /// Model, package and image downloads go to the Downloads exit (see
    /// [`crate::downloads`]), which follows the Location until told otherwise.
    pub downloads: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            exit: None,
            follow: false,
            domestic_direct: true,
            downloads: true,
        }
    }
}

/// What the layer did to a config.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Applied {
    /// The group AI traffic goes to (the subscription's own, or Kairo's).
    pub group: String,
    /// The subscription already had it.
    pub adopted: bool,
    /// The group of locations (where "All Traffic" goes).
    pub main: Option<String>,
    /// The hidden group downloads go to.
    pub downloads: Option<String>,
    /// Routes left out because their exit is not in the config (ids).
    pub skipped_routes: Vec<String>,
}

fn get<'a>(map: &'a Mapping, key: &str) -> Option<&'a Value> {
    map.get(key)
}

fn name_of(v: &Value) -> Option<&str> {
    v.as_mapping()?.get("name")?.as_str()
}

fn groups(config: &Mapping) -> Vec<&Mapping> {
    get(config, "proxy-groups")
        .and_then(Value::as_sequence)
        .map(|s| s.iter().filter_map(Value::as_mapping).collect())
        .unwrap_or_default()
}

fn members(group: &Mapping) -> Vec<&str> {
    group
        .get("proxies")
        .and_then(Value::as_sequence)
        .map(|s| s.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn is_select(group: &Mapping) -> bool {
    group.get("type").and_then(Value::as_str) == Some("select")
}

fn hidden(group: &Mapping) -> bool {
    group.get("hidden").and_then(Value::as_bool) == Some(true)
}

/// The group of locations in the raw config: the one named in the settings,
/// else the selector the most other groups list, else the first selector.
pub fn main_group_name(config: &Mapping, preferred: &str) -> Option<String> {
    let all = groups(config);
    let selectors: Vec<&Mapping> = all
        .iter()
        .copied()
        .filter(|g| {
            is_select(g) && !hidden(g) && name_of(&Value::Mapping((*g).clone())) != Some("GLOBAL")
        })
        .collect();
    let name = |g: &Mapping| {
        g.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    if !preferred.is_empty()
        && let Some(g) = selectors.iter().find(|g| name(g) == preferred)
    {
        return Some(name(g));
    }
    let mut best: Option<(String, usize)> = None;
    for g in &selectors {
        let n = name(g);
        let followers = all
            .iter()
            .filter(|o| {
                let on = name(o);
                on != n && on != "GLOBAL" && members(o).contains(&n.as_str())
            })
            .count();
        if best.as_ref().is_none_or(|(_, b)| followers > *b) {
            best = Some((n, followers));
        }
    }
    best.map(|(n, _)| n)
        .or_else(|| selectors.first().map(|g| name(g)))
}

/// What a rule line points at: its third field (`DOMAIN-SUFFIX,x.com,Group`).
fn rule_target(rule: &str) -> Option<&str> {
    let mut it = rule.split(',').map(str::trim);
    let kind = it.next()?;
    let payload = it.next()?;
    let target = it.next()?;
    let _ = payload;
    matches!(kind, "DOMAIN" | "DOMAIN-SUFFIX" | "DOMAIN-KEYWORD").then_some(target)
}

/// The subscription's own group for AI, when it has one: where its first
/// rule for OpenAI, Anthropic or Claude points, if that is a selector of its
/// own (not the Location group, which would pin every site to one node).
fn adopted_group(config: &Mapping, main: Option<&str>) -> Option<String> {
    let rules = get(config, "rules")?.as_sequence()?;
    let probes = ["openai.com", "anthropic.com", "claude.ai", "chatgpt.com"];
    let target = rules.iter().filter_map(Value::as_str).find_map(|r| {
        let payload = r.split(',').nth(1)?.trim();
        probes
            .contains(&payload)
            .then(|| rule_target(r))
            .flatten()
            .map(str::to_owned)
    })?;
    if Some(target.as_str()) == main || target == "DIRECT" || target == "REJECT" {
        return None;
    }
    let g = groups(config)
        .into_iter()
        .find(|g| g.get("name").and_then(Value::as_str) == Some(target.as_str()))?;
    is_select(g).then_some(target)
}

fn proxy_names(config: &Mapping) -> Vec<String> {
    get(config, "proxies")
        .and_then(Value::as_sequence)
        .map(|s| {
            s.iter()
                .filter_map(name_of)
                .filter(|n| !places::is_note(n) && !places::is_builtin(n))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn suffix_rules(out: &mut Vec<Value>, suffixes: &[&str], target: &str) {
    for s in suffixes {
        out.push(Value::from(format!("DOMAIN-SUFFIX,{s},{target}")));
    }
}

/// A hidden selector of Kairo's own: the Location first (until something else
/// is chosen it follows it), then every node, so any can be picked. Left as
/// it is when the config already has a group of that name.
fn add_group(config: &mut Mapping, name: &str, main: Option<&str>) {
    if groups(config)
        .iter()
        .any(|g| g.get("name").and_then(Value::as_str) == Some(name))
    {
        return;
    }
    let mut proxies: Vec<Value> = vec![];
    if let Some(m) = main {
        proxies.push(Value::from(m));
    }
    proxies.extend(proxy_names(config).into_iter().map(Value::from));
    let mut g = Mapping::new();
    g.insert("name".into(), name.into());
    g.insert("type".into(), "select".into());
    g.insert("hidden".into(), true.into());
    g.insert("proxies".into(), Value::Sequence(proxies));
    if let Some(p) = get(config, "proxy-providers").and_then(Value::as_mapping) {
        let used: Vec<Value> = p.keys().cloned().collect();
        if !used.is_empty() {
            g.insert("use".into(), Value::Sequence(used));
        }
    }
    let list = config
        .entry("proxy-groups".into())
        .or_insert_with(|| Value::Sequence(vec![]));
    if let Some(seq) = list.as_sequence_mut() {
        seq.push(Value::Mapping(g));
    }
}

/// Whether `name` is a node (a proxy, not a group) of the config.
pub fn has_node(config: &Mapping, name: &str) -> bool {
    get(config, "proxies")
        .and_then(Value::as_sequence)
        .is_some_and(|s| s.iter().filter_map(name_of).any(|n| n == name))
}

/// Adds Kairo's AI rules (and, when the subscription has no AI group, one) to
/// the profile's config. `all_traffic` is the "All Traffic" route: not
/// mihomo's global mode (which ignores rules, and so would let AI traffic
/// follow the Location) but every site through the Location, with AI still on
/// its own exit. Returns what it did; `None` when there is nothing to route
/// through (no nodes) or the layer is off.
pub fn apply(
    config: &mut Mapping,
    ai: &AiSettings,
    preferred_main: &str,
    all_traffic: bool,
) -> Option<Applied> {
    apply_with(config, ai, &[], preferred_main, all_traffic)
}

/// [`apply`], with the person's routes: their rules go in after the local
/// block (what stays on this machine is never sent elsewhere), before the
/// rest of the layer's rules and the profile's own. When the layer does
/// nothing (`None`), the routes still apply: see [`apply_routes`].
pub fn apply_with(
    config: &mut Mapping,
    ai: &AiSettings,
    user_routes: &[Route],
    preferred_main: &str,
    all_traffic: bool,
) -> Option<Applied> {
    if !ai.enabled || proxy_names(config).is_empty() && get(config, "proxy-providers").is_none() {
        return None;
    }
    let main = main_group_name(config, preferred_main);
    let (group, adopted) = match adopted_group(config, main.as_deref()) {
        Some(g) => (g, true),
        None => {
            add_group(config, KAIRO_AI_GROUP, main.as_deref());
            (KAIRO_AI_GROUP.to_owned(), false)
        }
    };
    add_group(config, KAIRO_DOWNLOADS_GROUP, main.as_deref());
    let downloads = KAIRO_DOWNLOADS_GROUP.to_owned();

    let mut rules: Vec<Value> = vec![];
    // Local models and the LAN stay on this machine.
    for c in LOCAL.iter().chain(LOCAL6) {
        rules.push(Value::from(format!("IP-CIDR,{c},DIRECT,no-resolve")));
    }
    rules.push(Value::from("DOMAIN-SUFFIX,local,DIRECT"));
    rules.push(Value::from("DOMAIN,localhost,DIRECT"));
    // The person's own choices for apps and sites.
    let generated = routes::rules(
        user_routes,
        &routes::Targets {
            ai: Some(&group),
            everything: main.as_deref(),
            downloads: Some(&downloads),
        },
        &|n| has_node(config, n),
    );
    rules.extend(generated.rules.into_iter().map(Value::from));
    if ai.domestic_direct && !all_traffic {
        for (_, sfx) in DOMESTIC {
            suffix_rules(&mut rules, sfx, "DIRECT");
        }
    }
    if ai.downloads {
        suffix_rules(&mut rules, DOWNLOADS, &downloads);
    }
    for s in SERVICES {
        suffix_rules(&mut rules, s.suffixes, &group);
    }

    if all_traffic {
        // Everything else through the Location; the subscription's own
        // rules (which would send local sites direct) are set aside.
        if let Some(m) = &main {
            rules.push(Value::from(format!("MATCH,{m}")));
            config.insert("rules".into(), Value::Sequence(rules));
        } else {
            prepend(config, rules);
        }
    } else {
        prepend(config, rules);
    }
    Some(Applied {
        group,
        adopted,
        main,
        downloads: Some(downloads),
        skipped_routes: generated.skipped,
    })
}

/// The routes for a config the AI layer did not touch (it is off, or there
/// is nothing to route through): put in front of the profile's rules. `Ai`
/// and `Downloads` go where the Location does. The ids of the routes left
/// out.
pub fn apply_routes(
    config: &mut Mapping,
    user_routes: &[Route],
    preferred_main: &str,
) -> Vec<String> {
    if user_routes.is_empty() {
        return vec![];
    }
    let main = main_group_name(config, preferred_main);
    let generated = routes::rules(
        user_routes,
        &routes::Targets {
            ai: None,
            everything: main.as_deref(),
            downloads: None,
        },
        &|n| has_node(config, n),
    );
    if !generated.rules.is_empty() {
        prepend(
            config,
            generated.rules.into_iter().map(Value::from).collect(),
        );
    }
    generated.skipped
}

fn prepend(config: &mut Mapping, rules: Vec<Value>) {
    let entry = config
        .entry("rules".into())
        .or_insert_with(|| Value::Sequence(vec![]));
    if !entry.is_sequence() {
        *entry = Value::Sequence(vec![]);
    }
    let seq = entry.as_sequence_mut().expect("a sequence");
    let rest = std::mem::take(seq);
    seq.extend(rules);
    seq.extend(rest);
}

/// What identifies a node apart from its name.
fn fnv(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// A node's fingerprint: its type, server and port (not its name, which a
/// subscription may change, and nothing secret).
pub fn fingerprint(config: &Mapping, node: &str) -> Option<String> {
    let seq = get(config, "proxies")?.as_sequence()?;
    let p = seq
        .iter()
        .filter_map(Value::as_mapping)
        .find(|p| p.get("name").and_then(Value::as_str) == Some(node))?;
    let text = |k: &str| match p.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    let server = text("server");
    (!server.is_empty()).then(|| fnv(&format!("{}|{}|{}", text("type"), server, text("port"))))
}

/// How the pinned exit stands in a (new) config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitState {
    /// Still there under the same name.
    Same,
    /// The subscription renamed it: the same server, a new name.
    Renamed(String),
    /// Gone: the subscription no longer has that server.
    Gone,
    /// Nothing is pinned yet.
    Unpinned,
}

pub fn exit_state(config: &Mapping, exit: Option<&AiExit>) -> ExitState {
    let Some(exit) = exit else {
        return ExitState::Unpinned;
    };
    if fingerprint(config, &exit.name).as_deref() == Some(exit.fingerprint.as_str()) {
        return ExitState::Same;
    }
    // Same server under another name.
    let names = proxy_names(config);
    if let Some(n) = names
        .iter()
        .find(|n| fingerprint(config, n).as_deref() == Some(exit.fingerprint.as_str()))
    {
        return ExitState::Renamed(n.clone());
    }
    ExitState::Gone
}

/// The node a group goes through now, following selectors that point at other
/// selectors ("Proxies" → "Asia" → "Tokyo 03").
pub fn resolve(groups: &[crate::model::Group], group: &str) -> Option<String> {
    let mut name = group.to_owned();
    for _ in 0..8 {
        let g = groups.iter().find(|g| g.name == name)?;
        let now = g.now.clone()?;
        match g.members.iter().find(|m| m.name == now) {
            Some(m) if m.is_group => name = now,
            Some(_) => return Some(now),
            None => return None,
        }
    }
    None
}

/// What to select in the AI group so it follows the Location: the Location
/// group itself when the AI group lists it, else the node the Location is on.
pub fn follow_target(groups: &[crate::model::Group], group: &str, main: &str) -> Option<String> {
    let g = groups.iter().find(|g| g.name == group)?;
    if g.members.iter().any(|m| m.name == main) {
        return Some(main.to_owned());
    }
    let node = resolve(groups, main)?;
    g.members.iter().any(|m| m.name == node).then_some(node)
}

/// Where the AI traffic goes: the pinned exit when it is a node of the group
/// and still the server it was; else follow the Location.
pub fn want(
    groups: &[crate::model::Group],
    group: &str,
    main: &str,
    exit: Option<&str>,
) -> Option<String> {
    let g = groups.iter().find(|g| g.name == group)?;
    match exit {
        Some(n) if g.members.iter().any(|m| m.name == n) => Some(n.to_owned()),
        _ => follow_target(groups, group, main),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> Mapping {
        crate::config::parse_mapping(text).unwrap()
    }

    const PLAIN: &str = r#"
proxies:
  - {name: "🇯🇵 Tokyo 03", type: anytls, server: a.example.net, port: 443, password: x}
  - {name: "🇯🇵 Tokyo 07", type: anytls, server: b.example.net, port: 443, password: x}
  - {name: "282.21 G | 500.00 G", type: anytls, server: c.example.net, port: 443, password: x}
proxy-groups:
  - {name: Proxies, type: select, proxies: ["🇯🇵 Tokyo 03", "🇯🇵 Tokyo 07", DIRECT]}
  - {name: Netflix, type: select, proxies: [Proxies, DIRECT]}
rules:
  - DOMAIN-SUFFIX,example.org,Proxies
  - MATCH,Proxies
"#;

    const WITH_AI: &str = r#"
proxies:
  - {name: "🇯🇵 Tokyo 03", type: anytls, server: a.example.net, port: 443, password: x}
proxy-groups:
  - {name: Proxies, type: select, proxies: ["🇯🇵 Tokyo 03"]}
  - {name: AI, type: select, proxies: ["🇯🇵 Tokyo 03", Proxies]}
rules:
  - DOMAIN-SUFFIX,openai.com,AI
  - DOMAIN-SUFFIX,openai.com,Proxies
  - MATCH,Proxies
"#;

    fn rules(c: &Mapping) -> Vec<String> {
        get(c, "rules")
            .and_then(Value::as_sequence)
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn knows_services_by_host() {
        assert_eq!(service_of("api.anthropic.com").unwrap().id, "claude");
        assert_eq!(service_of("CHATGPT.com").unwrap().id, "chatgpt");
        assert_eq!(service_of("a.b.cursor.sh").unwrap().id, "cursor");
        assert!(service_of("notanthropic.com").is_none());
        assert!(service_of("example.com").is_none());
        assert_eq!(domestic_of("api.deepseek.com"), Some("DeepSeek"));
        assert_eq!(domestic_of("claude.ai"), None);
    }

    #[test]
    fn tools_by_path() {
        assert_eq!(
            tool_service("/Applications/Cursor.app/Contents/MacOS/Cursor")
                .unwrap()
                .id,
            "cursor"
        );
        assert_eq!(
            tool_service("/home/me/.local/bin/claude").unwrap().id,
            "claude"
        );
        assert!(tool_service("/usr/bin/curl").is_none());
    }

    #[test]
    fn makes_its_own_group_when_the_subscription_has_none() {
        let mut c = cfg(PLAIN);
        let done = apply(&mut c, &AiSettings::default(), "", false).unwrap();
        assert_eq!(done.group, KAIRO_AI_GROUP);
        assert!(!done.adopted);
        assert_eq!(done.main.as_deref(), Some("Proxies"));
        let g = groups(&c)
            .into_iter()
            .find(|g| {
                members(g).contains(&"Proxies")
                    && g.get("name").and_then(Value::as_str) == Some(KAIRO_AI_GROUP)
            })
            .unwrap();
        // The Location first, then the nodes; the provider's note is no place.
        assert_eq!(members(g), ["Proxies", "🇯🇵 Tokyo 03", "🇯🇵 Tokyo 07"]);
        assert_eq!(g.get("hidden").and_then(Value::as_bool), Some(true));
    }

    #[test]
    fn adopts_the_subscriptions_ai_group_and_leaves_its_members() {
        let mut c = cfg(WITH_AI);
        let before = groups(&c).len();
        let done = apply(&mut c, &AiSettings::default(), "", false).unwrap();
        assert_eq!(done.group, "AI");
        assert!(done.adopted);
        // Only the Downloads group is added; the subscription's AI group is its own.
        assert_eq!(groups(&c).len(), before + 1);
        assert!(group_named(&c, KAIRO_AI_GROUP).is_none());
        assert_eq!(done.downloads.as_deref(), Some(KAIRO_DOWNLOADS_GROUP));
        assert!(rules(&c).contains(&"DOMAIN-SUFFIX,cursor.sh,AI".to_owned()));
        assert!(rules(&c).contains(&"DOMAIN-SUFFIX,huggingface.co,Kairo Downloads".to_owned()));
    }

    #[test]
    fn does_not_adopt_the_location_group() {
        // openai.com pointing at the Location group is no AI group of its own.
        let text = WITH_AI.replace("openai.com,AI", "openai.com,Proxies");
        let mut c = cfg(&text);
        let done = apply(&mut c, &AiSettings::default(), "", false).unwrap();
        assert_eq!(done.group, KAIRO_AI_GROUP);
    }

    #[test]
    fn rules_come_first_in_this_order() {
        let mut c = cfg(PLAIN);
        apply(&mut c, &AiSettings::default(), "", false).unwrap();
        let r = rules(&c);
        let pos = |needle: &str| r.iter().position(|x| x == needle).unwrap();
        assert_eq!(pos("IP-CIDR,127.0.0.0/8,DIRECT,no-resolve"), 0);
        assert!(pos("DOMAIN-SUFFIX,deepseek.com,DIRECT") > pos("DOMAIN,localhost,DIRECT"));
        assert!(
            pos("DOMAIN-SUFFIX,huggingface.co,Kairo Downloads")
                > pos("DOMAIN-SUFFIX,deepseek.com,DIRECT")
        );
        assert!(
            pos("DOMAIN-SUFFIX,claude.ai,Kairo AI")
                > pos("DOMAIN-SUFFIX,huggingface.co,Kairo Downloads")
        );
        // The mirror goes the way Hugging Face does.
        assert!(r.contains(&"DOMAIN-SUFFIX,hf-mirror.com,Kairo Downloads".to_owned()));
        // The subscription's own rules are untouched, after ours.
        assert!(pos("DOMAIN-SUFFIX,example.org,Proxies") > pos("DOMAIN-SUFFIX,claude.ai,Kairo AI"));
        assert_eq!(r.last().unwrap(), "MATCH,Proxies");
    }

    #[test]
    fn switches_turn_parts_off() {
        let mut c = cfg(PLAIN);
        let ai = AiSettings {
            domestic_direct: false,
            downloads: false,
            ..Default::default()
        };
        apply(&mut c, &ai, "", false).unwrap();
        let r = rules(&c);
        assert!(!r.iter().any(|x| x.contains("deepseek.com")));
        assert!(!r.iter().any(|x| x.contains("huggingface.co")));
        assert!(r.iter().any(|x| x.contains("claude.ai")));
        let mut off = cfg(PLAIN);
        let ai = AiSettings {
            enabled: false,
            ..Default::default()
        };
        assert!(apply(&mut off, &ai, "", false).is_none());
        assert_eq!(off, cfg(PLAIN));
    }

    #[test]
    fn all_traffic_is_rules_with_the_location_as_the_rest() {
        let mut c = cfg(PLAIN);
        apply(&mut c, &AiSettings::default(), "", true).unwrap();
        let r = rules(&c);
        assert_eq!(r.last().unwrap(), "MATCH,Proxies");
        // Local still direct, AI still on its own exit, the profile's rules set aside.
        assert!(r.contains(&"IP-CIDR,192.168.0.0/16,DIRECT,no-resolve".to_owned()));
        assert!(r.contains(&"DOMAIN-SUFFIX,claude.ai,Kairo AI".to_owned()));
        assert!(!r.contains(&"DOMAIN-SUFFIX,example.org,Proxies".to_owned()));
        // Domestic AI follows "All Traffic" too.
        assert!(!r.iter().any(|x| x.contains("deepseek.com")));
    }

    #[test]
    fn nothing_to_route_through_leaves_the_config_alone() {
        let mut c = cfg("proxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n");
        assert!(apply(&mut c, &AiSettings::default(), "", false).is_none());
        assert_eq!(rules(&c), ["MATCH,DIRECT"]);
    }

    #[test]
    fn main_group_prefers_the_setting_then_the_most_followed() {
        let c = cfg(PLAIN);
        assert_eq!(main_group_name(&c, "").as_deref(), Some("Proxies"));
        assert_eq!(main_group_name(&c, "Netflix").as_deref(), Some("Netflix"));
        assert_eq!(main_group_name(&c, "Nope").as_deref(), Some("Proxies"));
    }

    #[test]
    fn the_exit_is_found_again_after_a_rename_and_missed_when_gone() {
        let c = cfg(PLAIN);
        let fp = fingerprint(&c, "🇯🇵 Tokyo 03").unwrap();
        assert_ne!(fp, fingerprint(&c, "🇯🇵 Tokyo 07").unwrap());
        let exit = AiExit {
            name: "🇯🇵 Tokyo 03".into(),
            fingerprint: fp,
            since: 0,
        };
        assert_eq!(exit_state(&c, Some(&exit)), ExitState::Same);
        assert_eq!(exit_state(&c, None), ExitState::Unpinned);

        let renamed = cfg(&PLAIN.replace("Tokyo 03", "Tokyo A"));
        assert_eq!(
            exit_state(&renamed, Some(&exit)),
            ExitState::Renamed("🇯🇵 Tokyo A".into())
        );
        let moved = cfg(&PLAIN.replace("a.example.net", "z.example.net"));
        assert_eq!(exit_state(&moved, Some(&exit)), ExitState::Gone);
    }

    #[test]
    fn settings_read_old_files_and_round_trip() {
        let s: AiSettings = serde_json::from_str("{}").unwrap();
        assert!(s.enabled && s.domestic_direct && s.downloads && s.exit.is_none());
        let s = AiSettings {
            exit: Some(AiExit {
                name: "x".into(),
                fingerprint: "f".into(),
                since: 5,
            }),
            ..Default::default()
        };
        let back: AiSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn the_catalog_has_no_duplicate_hosts() {
        let mut seen = std::collections::HashSet::new();
        for s in SERVICES {
            for x in s.suffixes {
                assert!(seen.insert(*x), "{x} is in two services");
            }
        }
        for (_, sfx) in DOMESTIC {
            for x in *sfx {
                assert!(seen.insert(*x), "{x} is both foreign and domestic");
            }
        }
    }

    fn group(name: &str, now: &str, members: &[(&str, bool)]) -> crate::model::Group {
        crate::model::Group {
            name: name.into(),
            kind: "Selector".into(),
            now: Some(now.into()),
            members: members
                .iter()
                .map(|(n, is_group)| crate::model::Node {
                    name: (*n).into(),
                    kind: String::new(),
                    udp: false,
                    delay: None,
                    alive: true,
                    is_group: *is_group,
                })
                .collect(),
            hidden: false,
            test_url: None,
        }
    }

    #[test]
    fn resolves_through_nested_selectors_and_follows_or_pins() {
        let groups = vec![
            group("Proxies", "Asia", &[("Asia", true), ("DIRECT", false)]),
            group(
                "Asia",
                "Tokyo 03",
                &[("Tokyo 03", false), ("Tokyo 07", false)],
            ),
            group(
                "AI",
                "Proxies",
                &[("Proxies", true), ("Tokyo 03", false), ("Tokyo 07", false)],
            ),
            group("Narrow", "Tokyo 03", &[("Tokyo 03", false)]),
        ];
        assert_eq!(resolve(&groups, "Proxies").as_deref(), Some("Tokyo 03"));
        assert_eq!(resolve(&groups, "AI").as_deref(), Some("Tokyo 03"));
        assert_eq!(resolve(&groups, "Nope"), None);
        // Follow: the Location group when listed, else the node it is on.
        assert_eq!(
            follow_target(&groups, "AI", "Proxies").as_deref(),
            Some("Proxies")
        );
        assert_eq!(
            follow_target(&groups, "Narrow", "Proxies").as_deref(),
            Some("Tokyo 03")
        );
        // Pinned: a member wins, a missing one falls back to following.
        assert_eq!(
            want(&groups, "AI", "Proxies", Some("Tokyo 07")).as_deref(),
            Some("Tokyo 07")
        );
        assert_eq!(
            want(&groups, "Narrow", "Proxies", Some("Tokyo 07")).as_deref(),
            Some("Tokyo 03")
        );
        assert_eq!(
            want(&groups, "AI", "Proxies", None).as_deref(),
            Some("Proxies")
        );
        // A loop of selectors does not hang.
        let looped = vec![
            group("A", "B", &[("B", true)]),
            group("B", "A", &[("A", true)]),
        ];
        assert_eq!(resolve(&looped, "A"), None);
    }

    #[test]
    fn all_traffic_without_locations_keeps_the_cores_global() {
        let text = "proxies:\n  - {name: a, type: anytls, server: a.example.net, port: 443, password: x}\nrules:\n  - MATCH,DIRECT\n";
        let mut s = crate::settings::Settings::default();
        s.mode = crate::settings::Mode::Global;
        let out = crate::config::build(text, "", &s).unwrap();
        let m = cfg(&out);
        assert_eq!(m.get("mode").and_then(Value::as_str), Some("global"));
        // With locations, All Traffic is rules.
        let out = crate::config::build(PLAIN, "", &s).unwrap();
        assert_eq!(cfg(&out).get("mode").and_then(Value::as_str), Some("rule"));
        s.ai.enabled = false;
        let out = crate::config::build(PLAIN, "", &s).unwrap();
        assert_eq!(
            cfg(&out).get("mode").and_then(Value::as_str),
            Some("global")
        );
    }

    fn group_named<'a>(c: &'a Mapping, name: &str) -> Option<&'a Mapping> {
        groups(c)
            .into_iter()
            .find(|g| g.get("name").and_then(Value::as_str) == Some(name))
    }

    #[test]
    fn downloads_get_a_group_of_their_own_built_like_the_ai_group() {
        let mut c = cfg(PLAIN);
        let done = apply(&mut c, &AiSettings::default(), "", false).unwrap();
        assert_eq!(done.downloads.as_deref(), Some("Kairo Downloads"));
        let g = group_named(&c, KAIRO_DOWNLOADS_GROUP).unwrap();
        assert_eq!(members(g), ["Proxies", "🇯🇵 Tokyo 03", "🇯🇵 Tokyo 07"]);
        assert_eq!(g.get("type").and_then(Value::as_str), Some("select"));
        assert_eq!(g.get("hidden").and_then(Value::as_bool), Some(true));
        // Providers' nodes can be picked too.
        let mut p = cfg(
            "proxy-providers:\n  sub: {type: http, url: 'http://x', path: ./x.yaml}\nproxy-groups:\n  - {name: Proxies, type: select, use: [sub]}\nrules:\n  - MATCH,Proxies\n",
        );
        apply(&mut p, &AiSettings::default(), "", false).unwrap();
        for name in [KAIRO_AI_GROUP, KAIRO_DOWNLOADS_GROUP] {
            let g = group_named(&p, name).unwrap();
            assert_eq!(
                g.get("use").and_then(Value::as_sequence).map(Vec::len),
                Some(1),
                "{name}"
            );
        }
        // Downloads rules point at it; with the switch off, they are not written.
        let r = rules(&c);
        assert!(r.contains(&"DOMAIN-SUFFIX,pypi.org,Kairo Downloads".to_owned()));
        assert!(
            !r.iter()
                .any(|x| x.contains("huggingface") && x.ends_with(",Proxies"))
        );
        let mut off = cfg(PLAIN);
        let ai = AiSettings {
            downloads: false,
            ..Default::default()
        };
        apply(&mut off, &ai, "", false).unwrap();
        assert!(!rules(&off).iter().any(|x| x.contains("huggingface")));
        // A group of that name the subscription already has is not made twice.
        let mut twice = cfg(&PLAIN.replace(
            "  - {name: Netflix",
            "  - {name: Kairo Downloads, type: select, proxies: [Proxies]}\n  - {name: Netflix",
        ));
        apply(&mut twice, &AiSettings::default(), "", false).unwrap();
        assert_eq!(
            groups(&twice)
                .iter()
                .filter(|g| g.get("name").and_then(Value::as_str) == Some(KAIRO_DOWNLOADS_GROUP))
                .count(),
            1
        );
    }

    fn site(domain: &str, exit: crate::routes::RouteExit) -> Route {
        Route::new(crate::routes::RouteKind::site(domain), exit).unwrap()
    }

    fn app(name: &str, exit: crate::routes::RouteExit) -> Route {
        Route::new(crate::routes::RouteKind::app(name), exit).unwrap()
    }

    #[test]
    fn routes_sit_after_the_local_block_and_before_the_rest_of_the_layer() {
        use crate::routes::RouteExit::*;
        let mut c = cfg(PLAIN);
        let routes = vec![
            app("claude", Direct),
            site("example.org", Block),
            site("internal.example.net", Node("🇯🇵 Tokyo 03".into())),
            site("models.example.com", Downloads),
            site("chat.example.com", Ai),
            site("shop.example.com", Everything),
        ];
        let done = apply_with(&mut c, &AiSettings::default(), &routes, "", false).unwrap();
        assert!(done.skipped_routes.is_empty());
        let r = rules(&c);
        let pos = |needle: &str| r.iter().position(|x| x == needle).unwrap();
        // The never-proxied local block stays first, the routes follow it.
        let local_end = pos("DOMAIN,localhost,DIRECT");
        let first_layer = pos("DOMAIN-SUFFIX,deepseek.com,DIRECT");
        for rule in [
            "PROCESS-NAME,claude,DIRECT",
            "DOMAIN-SUFFIX,example.org,REJECT",
            "DOMAIN-SUFFIX,internal.example.net,🇯🇵 Tokyo 03",
            "DOMAIN-SUFFIX,models.example.com,Kairo Downloads",
            "DOMAIN-SUFFIX,chat.example.com,Kairo AI",
            "DOMAIN-SUFFIX,shop.example.com,Proxies",
        ] {
            assert!(pos(rule) > local_end && pos(rule) < first_layer, "{rule}");
        }
        // Before the AI rules and the subscription's own.
        assert!(
            pos("DOMAIN-SUFFIX,shop.example.com,Proxies") < pos("DOMAIN-SUFFIX,claude.ai,Kairo AI")
        );
        assert!(
            pos("DOMAIN-SUFFIX,shop.example.com,Proxies")
                < pos("DOMAIN-SUFFIX,example.org,Proxies")
        );
        assert_eq!(r.last().unwrap(), "MATCH,Proxies");
        // "All Traffic": still before the MATCH that replaces the profile's rules.
        let mut all = cfg(PLAIN);
        apply_with(&mut all, &AiSettings::default(), &routes, "", true).unwrap();
        let r = rules(&all);
        assert!(
            r.iter()
                .position(|x| x == "PROCESS-NAME,claude,DIRECT")
                .unwrap()
                < r.len() - 1
        );
        assert_eq!(r.last().unwrap(), "MATCH,Proxies");
    }

    #[test]
    fn a_route_to_a_node_the_subscription_lost_is_left_out_and_the_config_stands() {
        use crate::routes::RouteExit::*;
        let routes = vec![
            site("gone.example.com", Node("🇯🇵 Tokyo 99".into())),
            site("kept.example.com", Node("🇯🇵 Tokyo 07".into())),
            // A name is a node only if it is a proxy: a group is not one.
            site("group.example.com", Node("Netflix".into())),
        ];
        let mut c = cfg(PLAIN);
        let done = apply_with(&mut c, &AiSettings::default(), &routes, "", false).unwrap();
        assert_eq!(
            done.skipped_routes,
            ["site:gone.example.com", "site:group.example.com"]
        );
        let r = rules(&c);
        assert!(r.contains(&"DOMAIN-SUFFIX,kept.example.com,🇯🇵 Tokyo 07".to_owned()));
        assert!(
            !r.iter()
                .any(|x| x.contains("gone.example.com") || x.contains("group.example.com"))
        );
        // Every rule's target is something the config has.
        let mut names: Vec<String> = proxy_names(&c);
        names.extend(
            groups(&c)
                .iter()
                .filter_map(|g| g.get("name")?.as_str().map(str::to_owned)),
        );
        names.extend(["DIRECT".into(), "REJECT".into()]);
        for rule in r.iter().filter(|x| !x.starts_with("MATCH,")) {
            let target = rule.split(',').nth(2).unwrap();
            assert!(names.iter().any(|n| n == target), "{rule}");
        }
    }

    #[test]
    fn routes_apply_when_the_layer_is_off_or_has_nothing_to_route_through() {
        use crate::routes::RouteExit::*;
        let routes = vec![
            site("a.example.com", Ai),
            site("b.example.com", Downloads),
            site("c.example.com", Block),
            site("d.example.com", Everything),
        ];
        let off = AiSettings {
            enabled: false,
            ..Default::default()
        };
        let mut c = cfg(PLAIN);
        assert!(apply_with(&mut c, &off, &routes, "", false).is_none());
        // The layer did nothing; the routes are the caller's to apply.
        assert_eq!(rules(&c), rules(&cfg(PLAIN)));
        let skipped = apply_routes(&mut c, &routes, "");
        assert!(skipped.is_empty());
        let r = rules(&c);
        assert_eq!(
            r[..4],
            [
                "DOMAIN-SUFFIX,a.example.com,Proxies",
                "DOMAIN-SUFFIX,b.example.com,Proxies",
                "DOMAIN-SUFFIX,c.example.com,REJECT",
                "DOMAIN-SUFFIX,d.example.com,Proxies",
            ]
        );
        assert_eq!(r.last().unwrap(), "MATCH,Proxies");
        assert!(group_named(&c, KAIRO_AI_GROUP).is_none());
        assert!(group_named(&c, KAIRO_DOWNLOADS_GROUP).is_none());
        // No profile yet: direct and block still apply, a location does not exist.
        let mut bare = cfg("proxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n");
        let skipped = apply_routes(&mut bare, &routes, "");
        assert_eq!(
            skipped,
            [
                "site:a.example.com",
                "site:b.example.com",
                "site:d.example.com"
            ]
        );
        assert_eq!(
            rules(&bare),
            ["DOMAIN-SUFFIX,c.example.com,REJECT", "MATCH,DIRECT"]
        );
        assert!(apply_routes(&mut bare, &[], "").is_empty());
    }

    #[test]
    fn the_users_override_still_wins_over_routes_and_a_stale_route_still_builds() {
        use crate::routes::RouteExit::*;
        let s = crate::settings::Settings {
            routes: vec![
                site("example.org", Block),
                site("old.example.com", Node("🇯🇵 Tokyo 99".into())),
                app("curl", Direct),
            ],
            ..Default::default()
        };
        let over = "prepend-rules:\n  - DOMAIN-SUFFIX,example.org,DIRECT\n";
        let (text, applied) = crate::config::build_applied(PLAIN, over, &s, &|_| false).unwrap();
        assert!(applied.is_some());
        let r = rules(&cfg(&text));
        let pos = |needle: &str| r.iter().position(|x| x == needle).unwrap();
        assert_eq!(pos("DOMAIN-SUFFIX,example.org,DIRECT"), 0);
        assert!(pos("DOMAIN-SUFFIX,example.org,DIRECT") < pos("DOMAIN-SUFFIX,example.org,REJECT"));
        assert!(pos("PROCESS-NAME,curl,DIRECT") < pos("DOMAIN-SUFFIX,claude.ai,Kairo AI"));
        assert!(!text.contains("old.example.com"));
        let built = crate::config::build_full(PLAIN, "", &s, &|_| false).unwrap();
        assert_eq!(built.skipped_routes, ["site:old.example.com"]);
        // With the layer off, the same routes still build.
        let mut off = s.clone();
        off.ai.enabled = false;
        let built = crate::config::build_full(PLAIN, "", &off, &|_| false).unwrap();
        assert_eq!(built.skipped_routes, ["site:old.example.com"]);
        assert!(rules(&cfg(&built.text)).contains(&"PROCESS-NAME,curl,DIRECT".to_owned()));
    }

    /// The core itself accepts what the layer writes (skipped without a core:
    /// set KAIRO_TEST_CORE to a mihomo binary).
    #[tokio::test]
    async fn the_core_accepts_the_layers_config() {
        let Some(core) = std::env::var_os("KAIRO_TEST_CORE").map(std::path::PathBuf::from) else {
            return;
        };
        use crate::routes::RouteExit::*;
        let routes = vec![
            app("claude", Direct),
            app("Google Chrome", Ai),
            site("example.org", Block),
            site("a.example.net", Downloads),
            site("b.example.net", Everything),
        ];
        let with_provider = "proxy-providers:\n  sub: {type: http, url: 'http://127.0.0.1:1/x', path: ./sub.yaml, interval: 3600}\nproxy-groups:\n  - {name: Proxies, type: select, use: [sub]}\nrules:\n  - MATCH,Proxies\n";
        for text in [PLAIN, WITH_AI, with_provider] {
            for all in [false, true] {
                let mut c = cfg(text);
                apply_with(&mut c, &AiSettings::default(), &routes, "", all).unwrap();
                let dir = tempfile::tempdir().unwrap();
                let file = dir.path().join("config.yaml");
                std::fs::write(&file, serde_yaml::to_string(&Value::Mapping(c)).unwrap()).unwrap();
                crate::config::validate(&core, dir.path(), &file)
                    .await
                    .unwrap();
            }
        }
    }
}
