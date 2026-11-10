//! The Downloads exit: models, wheels, images and release files go through a
//! hidden group of their own (`Kairo Downloads`), apart from AI (which stays
//! put on its pinned exit) and from everything else (the Location).
//!
//! The group is a selector listing the Location first, then every node, as
//! the AI group does. Which one it is on is decided here (pure functions,
//! tested without a core): a node pinned by the person, else the fastest
//! node Kairo measured (when `fastest` is on), else the Location.
//!
//! A mirror (hf-mirror.com for Hugging Face) is only ever *measured* and
//! offered: Kairo does not edit dotfiles or the environment. When the mirror
//! passes its test the person can copy the shell lines ([`mirror_env_lines`]).

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::ai::AiExit;
use crate::model::Group;
use crate::places;

/// The hidden group downloads go to.
pub const KAIRO_DOWNLOADS_GROUP: &str = "Kairo Downloads";

/// The mirror and the original it stands in for, as one small public file
/// asked for with a `Range` (the same bytes from both).
pub const MIRROR_URL: &str = "https://hf-mirror.com/gpt2/resolve/main/tokenizer.json";
pub const ORIGIN_URL: &str = "https://huggingface.co/gpt2/resolve/main/tokenizer.json";
/// Said when the mirror is asked for before it passed.
pub const MIRROR_REFUSED: &str =
    "Test the mirror first: it can be turned on once it has answered through the Downloads exit";
/// How much of it a test reads.
const SAMPLE_BYTES: u64 = 512 * 1024;
/// Less than this is no measurement of a download.
const MIN_BYTES: u64 = 64 * 1024;

/// The settings of the Downloads exit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DownloadsSettings {
    /// A node pinned for downloads (by what it is, like the AI exit).
    pub exit: Option<AiExit>,
    /// Kairo keeps the group on the fastest node (unless one is pinned).
    pub fastest: bool,
    /// The person turned the mirror on (only possible after it passed).
    pub mirror: bool,
}

impl Default for DownloadsSettings {
    fn default() -> Self {
        Self {
            exit: None,
            fastest: true,
            mirror: false,
        }
    }
}

/// How the mirror test stands.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MirrorTest {
    #[default]
    Untested,
    Running,
    /// The mirror answered through the Downloads exit and is not slower.
    /// `direct_ms`: the original's time for the same bytes (`None`: the
    /// original did not answer, which is what a mirror is for).
    Passed {
        mirror_ms: u64,
        direct_ms: Option<u64>,
    },
    Failed {
        why: String,
    },
}

impl MirrorTest {
    pub fn passed(&self) -> bool {
        matches!(self, MirrorTest::Passed { .. })
    }
}

/// What the UI reads of the Downloads exit.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DownloadsInfo {
    /// The group, once the config has it (empty while the AI layer is off).
    pub group: String,
    /// The node downloads go through now.
    pub node: Option<String>,
    pub fastest: bool,
    pub mirror: bool,
    pub mirror_test: MirrorTest,
}

/// The shell lines to copy when the mirror is on. Kairo does not run or
/// write them.
pub fn mirror_env_lines() -> Vec<String> {
    vec![
        "export HF_ENDPOINT=https://hf-mirror.com".to_owned(),
        "export PIP_INDEX_URL=https://pypi.tuna.tsinghua.edu.cn/simple".to_owned(),
    ]
}

/// What a test download measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub bytes: u64,
    pub ms: u64,
}

impl Sample {
    /// Bytes per second.
    fn rate(&self) -> u64 {
        self.bytes.saturating_mul(1000) / self.ms.max(1)
    }
}

/// Whether the mirror is usable and not slower than the original, from what
/// was measured of each through the Downloads exit. A mirror within a tenth
/// of the original's speed counts as not slower (a test is noisy).
pub fn judge(mirror: Result<Sample, String>, origin: Result<Sample, String>) -> MirrorTest {
    let mirror = match mirror {
        Ok(m) if m.bytes >= MIN_BYTES => m,
        Ok(_) => {
            return MirrorTest::Failed {
                why: "The mirror answered with too little to measure".into(),
            };
        }
        Err(_) => {
            return MirrorTest::Failed {
                why: "The mirror did not answer through the Downloads exit".into(),
            };
        }
    };
    let direct = match origin {
        Ok(o) if o.bytes >= MIN_BYTES => Some(o),
        // The original did not answer (or not enough): the mirror is the one that works.
        _ => None,
    };
    if let Some(o) = direct
        && mirror.rate().saturating_mul(10) < o.rate().saturating_mul(9)
    {
        return MirrorTest::Failed {
            why: format!(
                "The mirror is slower than Hugging Face through this exit ({} ms against {} ms)",
                mirror.ms, o.ms
            ),
        };
    }
    MirrorTest::Passed {
        mirror_ms: mirror.ms,
        direct_ms: direct.map(|o| o.ms),
    }
}

/// Downloads the start of `url` through the proxy on `port` (so the rules,
/// and with them the Downloads exit, decide the way) and times it.
pub async fn measure(port: u16, url: &str) -> Result<Sample, String> {
    let proxy =
        reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).map_err(|e| e.to_string())?;
    let http = reqwest::Client::builder()
        .user_agent(concat!("Kairo/", env!("CARGO_PKG_VERSION")))
        .proxy(proxy)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    let resp = http
        .get(url)
        .header("Range", format!("bytes=0-{}", SAMPLE_BYTES - 1))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let mut bytes = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        bytes += chunk.map_err(|e| e.to_string())?.len() as u64;
        if bytes >= SAMPLE_BYTES {
            break;
        }
    }
    Ok(Sample {
        bytes,
        ms: start.elapsed().as_millis() as u64,
    })
}

/// The delay of a node, as the groups last measured it.
fn delay_of(groups: &[Group], node: &str) -> Option<u32> {
    groups
        .iter()
        .flat_map(|g| g.members.iter())
        .find(|m| m.name == node && !m.is_group)
        .and_then(|m| m.delay)
        .filter(|d| *d > 0)
}

/// Where the Downloads group should go: the pinned node when it is one of
/// its members; with `fastest`, the fastest place measured (the current one
/// stays unless it is much slower, so a switch is not made for a few ms);
/// else the Location. `None`: nothing to select.
pub fn want(
    groups: &[Group],
    group: &str,
    main: &Group,
    pinned: Option<&str>,
    fastest: bool,
) -> Option<String> {
    let g = groups.iter().find(|g| g.name == group)?;
    let member = |n: &str| g.members.iter().any(|m| m.name == n && !m.is_group);
    if let Some(n) = pinned
        && member(n)
    {
        return Some(n.to_owned());
    }
    if fastest
        && let Some(best) = places::fastest(groups, main)
        && member(&best.name)
    {
        let best_delay = best.delay.unwrap_or(0);
        let current = crate::ai::resolve(groups, group).filter(|c| member(c));
        if let Some(c) = current
            && let Some(d) = delay_of(groups, &c)
            && d <= best_delay.saturating_mul(3) / 2 + 40
        {
            return Some(c);
        }
        return Some(best.name);
    }
    crate::ai::follow_target(groups, group, &main.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Node;

    fn node(name: &str, delay: Option<u32>) -> Node {
        Node {
            name: name.into(),
            kind: String::new(),
            udp: false,
            delay,
            alive: delay.is_some_and(|d| d > 0),
            is_group: false,
        }
    }

    fn group(name: &str, now: &str, members: Vec<Node>) -> Group {
        Group {
            name: name.into(),
            kind: "Selector".into(),
            now: Some(now.into()),
            members,
            hidden: false,
            test_url: None,
        }
    }

    fn world(now: &str) -> Vec<Group> {
        let a = node("A", Some(300));
        let b = node("B", Some(100));
        let c = node("C", Some(0));
        let mut loc = Node {
            is_group: true,
            ..node("Proxies", None)
        };
        loc.alive = true;
        vec![
            group("Proxies", "A", vec![a.clone(), b.clone(), c.clone()]),
            group("Kairo Downloads", now, vec![loc, a, b, c]),
        ]
    }

    #[test]
    fn defaults_and_old_files() {
        let s: DownloadsSettings = serde_json::from_str("{}").unwrap();
        assert!(s.fastest && !s.mirror && s.exit.is_none());
        let d = DownloadsSettings::default();
        assert!(d.fastest && !d.mirror);
        // A settings file from before Downloads has no such key (and the old
        // `ai.downloads` bool is still read).
        let all: crate::settings::Settings =
            serde_json::from_str(r#"{"mixedPort": 1234, "ai": {"downloads": false}}"#).unwrap();
        assert_eq!(all.downloads, DownloadsSettings::default());
        assert!(!all.ai.downloads);
        assert!(all.routes.is_empty());
        let pinned = DownloadsSettings {
            exit: Some(AiExit {
                name: "x".into(),
                fingerprint: "f".into(),
                since: 1,
            }),
            fastest: false,
            mirror: true,
        };
        let back: DownloadsSettings =
            serde_json::from_str(&serde_json::to_string(&pinned).unwrap()).unwrap();
        assert_eq!(back, pinned);
    }

    #[test]
    fn the_group_goes_where_the_person_and_the_measurements_say() {
        let groups = world("Proxies");
        let main = groups[0].clone();
        // Pinned wins, even over the fastest.
        assert_eq!(
            want(&groups, "Kairo Downloads", &main, Some("A"), true).as_deref(),
            Some("A")
        );
        // Fastest: B (100 ms) beats the Location's A (300 ms).
        assert_eq!(
            want(&groups, "Kairo Downloads", &main, None, true).as_deref(),
            Some("B")
        );
        // Not fastest, nothing pinned: follows the Location (the group itself).
        assert_eq!(
            want(&groups, "Kairo Downloads", &main, None, false).as_deref(),
            Some("Proxies")
        );
        // A pinned node that is no member falls back to the fastest.
        assert_eq!(
            want(&groups, "Kairo Downloads", &main, Some("Gone"), true).as_deref(),
            Some("B")
        );
        assert_eq!(want(&groups, "Nope", &main, None, true), None);
    }

    #[test]
    fn the_fastest_does_not_flap_for_a_few_milliseconds() {
        // On B (100 ms) while A is 90 ms: stays. On A (300 ms): moves to B.
        let mut groups = world("B");
        for g in &mut groups {
            for m in &mut g.members {
                if m.name == "A" {
                    m.delay = Some(90);
                }
                if m.name == "B" {
                    m.delay = Some(100);
                }
            }
        }
        let main = groups[0].clone();
        assert_eq!(
            want(&groups, "Kairo Downloads", &main, None, true).as_deref(),
            Some("B")
        );
        // Without any measurement, fastest has nothing: the Location.
        let mut cold = world("Proxies");
        for g in &mut cold {
            for m in &mut g.members {
                m.delay = None;
            }
        }
        let main = cold[0].clone();
        assert_eq!(
            want(&cold, "Kairo Downloads", &main, None, true).as_deref(),
            Some("Proxies")
        );
    }

    fn s(bytes: u64, ms: u64) -> Result<Sample, String> {
        Ok(Sample { bytes, ms })
    }

    #[test]
    fn the_mirror_passes_when_it_answers_and_is_not_slower() {
        let k = 512 * 1024;
        // Faster, equal, and within a tenth of the original's speed.
        assert_eq!(
            judge(s(k, 400), s(k, 900)),
            MirrorTest::Passed {
                mirror_ms: 400,
                direct_ms: Some(900)
            }
        );
        assert!(judge(s(k, 900), s(k, 900)).passed());
        assert!(judge(s(k, 980), s(k, 900)).passed());
        // Clearly slower: refused, with the numbers.
        match judge(s(k, 2000), s(k, 900)) {
            MirrorTest::Failed { why } => assert!(why.contains("2000") && why.contains("900")),
            other => panic!("{other:?}"),
        }
        // The original does not answer: the mirror passes alone.
        assert_eq!(
            judge(s(k, 700), Err("timeout".into())),
            MirrorTest::Passed {
                mirror_ms: 700,
                direct_ms: None
            }
        );
        // The mirror does not answer, or answers with a stub: failed.
        assert!(matches!(
            judge(Err("HTTP 502".into()), s(k, 900)),
            MirrorTest::Failed { .. }
        ));
        assert!(matches!(
            judge(s(120, 50), s(k, 900)),
            MirrorTest::Failed { .. }
        ));
        assert!(matches!(
            judge(Err("x".into()), Err("y".into())),
            MirrorTest::Failed { .. }
        ));
    }

    #[test]
    fn a_mirror_that_answers_fewer_bytes_is_judged_by_speed() {
        // Half the bytes in half the time is the same speed.
        assert!(judge(s(256 * 1024, 450), s(512 * 1024, 900)).passed());
        assert!(!judge(s(256 * 1024, 900), s(512 * 1024, 900)).passed());
    }

    #[test]
    fn the_shell_lines_are_plain_exports() {
        let lines = mirror_env_lines();
        assert!(lines.contains(&"export HF_ENDPOINT=https://hf-mirror.com".to_owned()));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("export PIP_INDEX_URL=https://"))
        );
        assert!(
            lines
                .iter()
                .all(|l| l.starts_with("export ") && !l.contains('\n'))
        );
    }
}
