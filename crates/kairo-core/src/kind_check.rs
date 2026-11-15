//! The check by kind: what an app of each kind needs from the exit, measured
//! the way that kind of app uses it. Streaming: is the page there and which
//! catalog does it serve. Games: how long a connection takes, and how much it
//! varies. Downloads: how fast a big file comes. Dev tools: how soon the site
//! answers, and whether SSH gets through.
//!
//! Every probe goes through Kairo's own mixed port, so the rules send it out
//! the exit the app's real traffic takes. What it never does: sign in, send a
//! key, or say that something is "fine" or "unlocked". It reports what it
//! measured.

use std::time::Duration;

#[cfg(feature = "engine")]
use std::time::Instant;

#[cfg(feature = "engine")]
use futures_util::StreamExt;
#[cfg(feature = "engine")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "engine")]
use tokio::net::TcpStream;

/// Less than this is no measurement of a speed.
const MIN_BYTES: u64 = 256 * 1024;

/// One app's check.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KindCheck {
    /// The catalog id of the app: `netflix`.
    pub app: String,
    /// Unix seconds.
    pub at: i64,
    pub measured: Measured,
}

/// What was measured, by the app's kind.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Measured {
    Streaming {
        /// The page gave any HTTP answer.
        reachable: bool,
        catalog: Catalog,
        /// An ISO code: the page's own when it says one, else an estimate.
        country: Option<String>,
        /// Megabits a second over the page, when it was big enough to tell.
        mbps: Option<u32>,
    },
    Games {
        /// The median connection time over the samples that worked.
        median_ms: Option<u32>,
        /// The spread of those samples: p90 minus p10.
        jitter_ms: Option<u32>,
        /// The share of samples that failed, in percent.
        loss_pct: u8,
    },
    Downloads {
        reachable: bool,
        /// Megabytes (10^6 bytes) a second, times ten: 42.3 MB/s is 423.
        mb_per_s_x10: Option<u32>,
    },
    DevTools {
        reachable: bool,
        /// The time to the response headers.
        ttfb_ms: Option<u32>,
        ssh: Ssh,
    },
}

/// Which catalog a streaming page serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Catalog {
    /// Titles it licenses from others are there too.
    Full,
    /// Only its own titles.
    OriginalsOnly,
    /// The service says it is not offered here.
    NotAvailable,
    Unknown,
}

/// Whether SSH to a code host gets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Ssh {
    /// Port 22 sent an SSH banner.
    Open,
    /// Port 22 did not, but the host's SSH on 443 did ([`ssh_snippet`]).
    ClosedUse443,
    /// Neither did.
    Closed,
    /// This app has no SSH to try.
    NotApplicable,
}

impl KindCheck {
    /// Whether something here needs a look.
    pub fn trouble(&self) -> bool {
        self.measured.trouble()
    }

    /// The short text for the app's row.
    pub fn line(&self, country_name: impl Fn(&str) -> String) -> String {
        self.measured.line(country_name)
    }
}

impl Measured {
    /// Whether something here needs a look: not reached, not offered, slow,
    /// or SSH that does not get through on 22.
    pub fn trouble(&self) -> bool {
        match self {
            Measured::Streaming {
                reachable, catalog, ..
            } => !reachable || *catalog == Catalog::NotAvailable,
            Measured::Games {
                median_ms,
                jitter_ms,
                loss_pct,
            } => {
                median_ms.is_some_and(|m| m > 150)
                    || jitter_ms.is_some_and(|j| j > 30)
                    || *loss_pct > 10
            }
            Measured::Downloads {
                reachable,
                mb_per_s_x10,
            } => !reachable || mb_per_s_x10.is_some_and(|x| x < 10),
            Measured::DevTools { reachable, ssh, .. } => {
                !reachable || matches!(ssh, Ssh::ClosedUse443 | Ssh::Closed)
            }
        }
    }

    /// The short text for the row: `US catalog`, `36 ms · jitter 3 ms`.
    /// `country_name` turns a code into a name (`JP` into `Japan`).
    pub fn line(&self, country_name: impl Fn(&str) -> String) -> String {
        match self {
            Measured::Streaming {
                reachable: false, ..
            } => crate::tr!("model.kind_check.unreachable"),
            Measured::Streaming {
                catalog, country, ..
            } => match (catalog, country) {
                (Catalog::Full, Some(c)) => crate::tr!("model.kind_check.catalog", code = c),
                (Catalog::Full, None) => crate::tr!("model.kind_check.full_catalog"),
                (Catalog::OriginalsOnly, _) => crate::tr!("model.kind_check.originals_only"),
                (Catalog::NotAvailable, _) => crate::tr!("model.kind_check.not_available"),
                (Catalog::Unknown, Some(c)) => crate::tr!("model.kind_check.sees", country = country_name(c)),
                (Catalog::Unknown, None) => crate::tr!("model.kind_check.reachable"),
            },
            Measured::Games {
                median_ms: None, ..
            } => crate::tr!("model.kind_check.unreachable"),
            Measured::Games {
                median_ms: Some(m),
                jitter_ms,
                loss_pct,
            } => {
                let jitter = jitter_ms.unwrap_or(0);
                if *loss_pct > 0 {
                    crate::tr!("model.kind_check.game_lost", ms = m, jitter = jitter, lost = loss_pct)
                } else {
                    crate::tr!("model.kind_check.game", ms = m, jitter = jitter)
                }
            }
            Measured::Downloads {
                reachable: false, ..
            } => crate::tr!("model.kind_check.unreachable"),
            Measured::Downloads {
                mb_per_s_x10: Some(x),
                ..
            } => {
                if *x < 100 {
                    format!("{}.{} MB/s", x / 10, x % 10)
                } else {
                    format!("{} MB/s", (x + 5) / 10)
                }
            }
            Measured::Downloads { .. } => crate::tr!("model.kind_check.reachable"),
            Measured::DevTools {
                reachable: false, ..
            } => crate::tr!("model.kind_check.unreachable"),
            Measured::DevTools { ssh, ttfb_ms, .. } => match (ssh, ttfb_ms) {
                (Ssh::ClosedUse443, _) => crate::tr!("model.kind_check.ssh_22_closed"),
                (Ssh::Closed, _) => crate::tr!("model.kind_check.ssh_closed"),
                (_, Some(ms)) => crate::tr!("model.kind_check.reachable_ms", ms = ms),
                (_, None) => crate::tr!("model.kind_check.reachable"),
            },
        }
    }
}

/// The `~/.ssh/config` lines that send SSH for a code host over its port
/// 443, for the hosts that offer one.
pub fn ssh_snippet(host: &str) -> Option<String> {
    let alt = match host {
        "github.com" => "ssh.github.com",
        "gitlab.com" => "altssh.gitlab.com",
        _ => return None,
    };
    const NL: &str = "\n";
    Some(format!("Host {host}{NL}  Hostname {alt}{NL}  Port 443{NL}  User git{NL}"))
}

/// What to measure for an app. Plain data, so a test can point it anywhere.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Probe {
    /// GET the page; read catalog and region from it; measure throughput on
    /// the body when it is at least 256 KiB.
    Streaming { url: String, reader: Reader },
    /// Connect to host:port through the proxy 10 times in a row, and time
    /// each.
    Games { host: String, port: u16 },
    /// A ranged GET (the first 2 MiB) of a big file; MB/s over the bytes
    /// read. `url` None: only whether `reach` answers.
    Downloads { reach: String, url: Option<String> },
    /// GET `url` and time it to the response headers. `ssh`: (host, port,
    /// alt host, alt port): an SSH banner from host:port within 6 s, else
    /// from the alt.
    DevTools {
        url: String,
        ssh: Option<(String, u16, String, u16)>,
    },
}

/// How a streaming page is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Reader {
    /// A licensed title's page: there in the full catalog, a 404 in the
    /// originals-only one ([`parse_netflix`]).
    NetflixTitle,
    /// The Premium page and the country it names ([`parse_youtube`]).
    YoutubePremium,
    /// Whether it answers; the country is the caller's estimate.
    CountryOnly,
}

/// The real target for an app of the catalog. `None` for an app this check
/// does not know (AI services have their own).
pub fn probe_for(app: &str) -> Option<Probe> {
    let streaming = |url: &str, reader| Probe::Streaming {
        url: url.into(),
        reader,
    };
    let games = |host: &str| Probe::Games {
        host: host.into(),
        port: 443,
    };
    let downloads = |reach: &str, url: Option<&str>| Probe::Downloads {
        reach: reach.into(),
        url: url.map(Into::into),
    };
    let dev = |url: &str, ssh: Option<(&str, &str)>| Probe::DevTools {
        url: url.into(),
        ssh: ssh.map(|(h, alt)| (h.into(), 22, alt.into(), 443)),
    };
    Some(match app {
        // A title Netflix licenses (not one of its own), so its page tells
        // the full catalog from the originals-only one.
        "netflix" => streaming(
            "https://www.netflix.com/title/70143836",
            Reader::NetflixTitle,
        ),
        "youtube" => streaming("https://www.youtube.com/premium", Reader::YoutubePremium),
        "disneyplus" => streaming("https://www.disneyplus.com/", Reader::CountryOnly),
        "primevideo" => streaming("https://www.primevideo.com/", Reader::CountryOnly),
        "max" => streaming("https://www.max.com/", Reader::CountryOnly),
        "spotify" => streaming("https://open.spotify.com/", Reader::CountryOnly),
        "twitch" => streaming("https://www.twitch.tv/", Reader::CountryOnly),
        "hulu" => streaming("https://www.hulu.com/", Reader::CountryOnly),
        "steam" => games("steamcommunity.com"),
        "epic" => games("store.epicgames.com"),
        "battlenet" => games("us.battle.net"),
        "riot" => games("auth.riotgames.com"),
        "playstation" => games("my.playstation.com"),
        "xbox" => games("xboxlive.com"),
        "huggingface" => downloads(
            "https://huggingface.co/",
            Some("https://huggingface.co/openai-community/gpt2/resolve/main/model.safetensors"),
        ),
        "github-releases" => downloads(
            "https://objects.githubusercontent.com/",
            Some(
                "https://github.com/MetaCubeX/mihomo/releases/download/v1.19.15/mihomo-linux-amd64-v1.19.15.gz",
            ),
        ),
        "pypi" => downloads("https://pypi.org/simple/", None),
        "pytorch" => downloads("https://download.pytorch.org/whl/", None),
        "ollama" => downloads("https://registry.ollama.ai/v2/", None),
        // Registries answer 401 without a token: that is reached.
        "dockerhub" => downloads("https://registry-1.docker.io/v2/", None),
        "ghcr" => downloads("https://ghcr.io/v2/", None),
        "github" => dev(
            "https://github.com/",
            Some(("github.com", "ssh.github.com")),
        ),
        "gitlab" => dev(
            "https://gitlab.com/",
            Some(("gitlab.com", "altssh.gitlab.com")),
        ),
        "npm" => dev("https://registry.npmjs.org/", None),
        "crates" => dev("https://index.crates.io/config.json", None),
        "go" => dev("https://proxy.golang.org/", None),
        "jetbrains" => dev("https://plugins.jetbrains.com/", None),
        "vscode-marketplace" => dev("https://marketplace.visualstudio.com/", None),
        _ => return None,
    })
}

/// The two-letter code right after `marker` at one of its places in `body`,
/// upper-cased. Each place is tried, as such markers turn up elsewhere too.
fn code_after(body: &str, marker: &str) -> Option<String> {
    body.match_indices(marker).find_map(|(i, _)| {
        let rest = &body.as_bytes()[i + marker.len()..];
        (rest.len() >= 3
            && rest[0].is_ascii_alphabetic()
            && rest[1].is_ascii_alphabetic()
            && rest[2] == b'"')
            .then(|| String::from_utf8_lossy(&rest[..2]).to_ascii_uppercase())
    })
}

/// The service's own "not offered in your country" page.
fn says_not_available(body: &str) -> bool {
    body.to_ascii_lowercase()
        .contains("not available in your country")
}

/// The country of a Netflix URL's first path part: `/jp/` or `/jp-en/`.
fn country_in_path(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let first = url.path_segments()?.next()?;
    let b = first.as_bytes();
    let code = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1].is_ascii_alphabetic();
    let rest_ok = b.len() == 2
        || (b.len() == 5 && b[2] == b'-' && b[3..].iter().all(u8::is_ascii_alphabetic));
    (code && rest_ok).then(|| first[..2].to_ascii_uppercase())
}

/// Reads a Netflix title page: 200 is the full catalog, 404 the
/// originals-only one, 403 (or the "not available" page) not offered here.
/// The region is the one the page names, else the one in its final URL.
pub fn parse_netflix(status: u16, final_url: &str, body: &str) -> (Catalog, Option<String>) {
    let catalog = match status {
        403 => Catalog::NotAvailable,
        _ if says_not_available(body) => Catalog::NotAvailable,
        200 => Catalog::Full,
        404 => Catalog::OriginalsOnly,
        _ => Catalog::Unknown,
    };
    let region = code_after(body, r#""requestCountry":{"id":""#)
        .or_else(|| code_after(body, r#""country":""#))
        .or_else(|| country_in_path(final_url));
    (catalog, region)
}

/// Reads the YouTube Premium page: the country it names, or that it is not
/// offered there. A page that names a country is the full catalog.
pub fn parse_youtube(status: u16, body: &str) -> (Catalog, Option<String>) {
    let region = code_after(body, r#""countryCode":""#).or_else(|| code_after(body, r#""GL":""#));
    let catalog = if says_not_available(body) {
        Catalog::NotAvailable
    } else if !(200..400).contains(&status) {
        Catalog::Unknown
    } else if region.is_some() {
        Catalog::Full
    } else {
        Catalog::Unknown
    };
    (catalog, region)
}

/// The median and the spread (p90 minus p10) of some times in ms. Sorts
/// them. `(None, None)` when there are none.
pub fn jitter(samples: &mut [u32]) -> (Option<u32>, Option<u32>) {
    if samples.is_empty() {
        return (None, None);
    }
    samples.sort_unstable();
    let n = samples.len();
    let median = if n % 2 == 1 {
        samples[n / 2]
    } else {
        (samples[n / 2 - 1] + samples[n / 2]) / 2
    };
    let at = |q: f64| samples[((n - 1) as f64 * q).round() as usize];
    (Some(median), Some(at(0.9) - at(0.1)))
}

/// Whether bytes read from a port are an SSH server's greeting: a line that
/// starts with `SSH-` (a server may send other lines before it).
pub fn is_ssh_banner(bytes: &[u8]) -> bool {
    bytes.starts_with(b"SSH-") || bytes.windows(5).any(|w| w == b"\nSSH-")
}

/// Megabytes (10^6 bytes) a second, times ten, rounded. `None` under 256
/// KiB, which is too little to tell.
pub fn mb_per_s_x10(bytes: u64, elapsed: Duration) -> Option<u32> {
    if bytes < MIN_BYTES {
        return None;
    }
    let secs = elapsed.as_secs_f64().max(0.001);
    Some(
        (bytes as f64 * 10.0 / 1e6 / secs)
            .round()
            .min(u32::MAX as f64) as u32,
    )
}

/// Megabits a second, rounded. `None` under 256 KiB.
#[cfg(feature = "engine")]
fn mbps(bytes: u64, elapsed: Duration) -> Option<u32> {
    if bytes < MIN_BYTES {
        return None;
    }
    let secs = elapsed.as_secs_f64().max(0.001);
    Some(
        (bytes as f64 * 8.0 / 1e6 / secs)
            .round()
            .min(u32::MAX as f64) as u32,
    )
}

/// How many connections a games check makes, and how long each may take.
#[cfg(feature = "engine")]
const GAME_SAMPLES: u8 = 10;
#[cfg(feature = "engine")]
const GAME_WAIT: Duration = Duration::from_secs(3);
/// How long an SSH port has to greet.
#[cfg(feature = "engine")]
const SSH_WAIT: Duration = Duration::from_secs(6);
/// How long a download or a page may take.
#[cfg(feature = "engine")]
const BUDGET: Duration = Duration::from_secs(12);
/// How much of a big file a downloads check asks for.
#[cfg(feature = "engine")]
const RANGE_BYTES: u64 = 2 * 1024 * 1024;
/// How much of a page is kept to read; the rest is only counted.
#[cfg(feature = "engine")]
const PAGE_KEEP: usize = 4 * 1024 * 1024;
#[cfg(feature = "engine")]
const PAGE_MAX: u64 = 8 * 1024 * 1024;

/// A streaming page is asked for as a browser would, so it serves the page a
/// person sees (in English, for the "not available" text).
#[cfg(feature = "engine")]
const BROWSER: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// The HTTP client: through the mixed port, or direct (`None`, for tests).
/// No gzip, so a speed counts the bytes that came.
#[cfg(feature = "engine")]
fn client(proxy: Option<u16>, user_agent: &str) -> Option<reqwest::Client> {
    let b = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .user_agent(user_agent)
        .no_gzip();
    let b = match proxy {
        Some(p) => b.proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{p}")).ok()?),
        None => b.no_proxy(),
    };
    b.build().ok()
}

#[cfg(feature = "engine")]
fn ms(d: Duration) -> u32 {
    d.as_millis().min(u32::MAX as u128) as u32
}

/// Reads a body until `max` bytes or the deadline. The bytes counted, the
/// first `keep` of them, and the time it took.
#[cfg(feature = "engine")]
async fn read_body(
    resp: reqwest::Response,
    max: u64,
    keep: usize,
    deadline: tokio::time::Instant,
) -> (u64, Vec<u8>, Duration) {
    let started = Instant::now();
    let mut count = 0u64;
    let mut kept = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Ok(Some(Ok(chunk))) = tokio::time::timeout_at(deadline, stream.next()).await {
        count += chunk.len() as u64;
        if kept.len() < keep {
            let room = keep - kept.len();
            kept.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
        if count >= max {
            break;
        }
    }
    (count, kept, started.elapsed())
}

/// Opens a tunnel to host:port through the proxy with `CONNECT`. The stream,
/// and any bytes that came after the proxy's answer.
#[cfg(feature = "engine")]
async fn connect_via(proxy: u16, host: &str, port: u16) -> std::io::Result<(TcpStream, Vec<u8>)> {
    use std::io::{Error, ErrorKind};
    let mut s = TcpStream::connect(("127.0.0.1", proxy)).await?;
    let target = format!("{host}:{port}");
    const CRLF: &str = "\r\n";
    s.write_all(format!("CONNECT {target} HTTP/1.1{CRLF}Host: {target}{CRLF}{CRLF}").as_bytes())
        .await?;
    let mut buf = Vec::new();
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 8192 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "no end to the proxy's answer",
            ));
        }
        let mut chunk = [0u8; 1024];
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]);
    let mut status = head.lines().next().unwrap_or("").split_whitespace();
    let ok =
        status.next().is_some_and(|v| v.starts_with("HTTP/1.")) && status.next() == Some("200");
    if !ok {
        return Err(Error::new(
            ErrorKind::ConnectionRefused,
            "the proxy refused",
        ));
    }
    Ok((s, buf.split_off(end)))
}

/// A small TLS ClientHello for `host`: whatever the server says back to it
/// (a ServerHello or an alert) is one round trip to the server itself.
#[cfg(feature = "engine")]
fn client_hello(host: &str) -> Vec<u8> {
    fn u16be(v: usize) -> [u8; 2] {
        (v as u16).to_be_bytes()
    }
    let name = host.as_bytes();
    let mut ext = Vec::new();
    // server_name
    ext.extend_from_slice(&[0, 0]);
    ext.extend_from_slice(&u16be(name.len() + 5));
    ext.extend_from_slice(&u16be(name.len() + 3));
    ext.push(0);
    ext.extend_from_slice(&u16be(name.len()));
    ext.extend_from_slice(name);
    // supported_groups: x25519, P-256, P-384
    ext.extend_from_slice(&[
        0x00, 0x0a, 0x00, 0x08, 0x00, 0x06, 0x00, 0x1d, 0x00, 0x17, 0x00, 0x18,
    ]);
    // ec_point_formats: uncompressed
    ext.extend_from_slice(&[0x00, 0x0b, 0x00, 0x02, 0x01, 0x00]);
    // signature_algorithms
    let sigs: [u8; 16] = [
        0x04, 0x03, 0x08, 0x04, 0x04, 0x01, 0x05, 0x03, 0x08, 0x05, 0x05, 0x01, 0x08, 0x06, 0x06,
        0x01,
    ];
    ext.extend_from_slice(&[0x00, 0x0d]);
    ext.extend_from_slice(&u16be(sigs.len() + 2));
    ext.extend_from_slice(&u16be(sigs.len()));
    ext.extend_from_slice(&sigs);

    let suites: [u8; 12] = [
        0xc0, 0x2b, 0xc0, 0x2f, 0xc0, 0x2c, 0xc0, 0x30, 0xcc, 0xa9, 0xcc, 0xa8,
    ];
    let mut hello = vec![0x03, 0x03];
    hello.extend((0..32u8).map(|i| i.wrapping_mul(37).wrapping_add(11)));
    hello.push(0); // no session id
    hello.extend_from_slice(&u16be(suites.len()));
    hello.extend_from_slice(&suites);
    hello.extend_from_slice(&[1, 0]); // no compression
    hello.extend_from_slice(&u16be(ext.len()));
    hello.extend_from_slice(&ext);

    let mut hs = vec![1];
    hs.extend_from_slice(&(hello.len() as u32).to_be_bytes()[1..]);
    hs.extend_from_slice(&hello);
    let mut record = vec![0x16, 0x03, 0x01];
    record.extend_from_slice(&u16be(hs.len()));
    record.extend_from_slice(&hs);
    record
}

/// One games sample. Direct (tests): a TCP connect. Through the proxy: the
/// proxy says 200 before it has dialled anything, so the sample lasts until
/// the server's first answer to a ClientHello.
#[cfg(feature = "engine")]
async fn game_sample(proxy: Option<u16>, host: &str, port: u16) -> std::io::Result<()> {
    match proxy {
        None => TcpStream::connect((host, port)).await.map(drop),
        Some(p) => {
            let (mut s, _) = connect_via(p, host, port).await?;
            s.write_all(&client_hello(host)).await?;
            let mut b = [0u8; 1];
            match s.read(&mut b).await? {
                0 => Err(std::io::ErrorKind::UnexpectedEof.into()),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(feature = "engine")]
async fn games(proxy: Option<u16>, host: &str, port: u16) -> Measured {
    let mut ok = Vec::new();
    let mut lost = 0u8;
    for _ in 0..GAME_SAMPLES {
        let started = Instant::now();
        match tokio::time::timeout(GAME_WAIT, game_sample(proxy, host, port)).await {
            Ok(Ok(())) => ok.push(ms(started.elapsed())),
            _ => lost += 1,
        }
    }
    let (median_ms, jitter_ms) = jitter(&mut ok);
    Measured::Games {
        median_ms,
        jitter_ms,
        loss_pct: lost * (100 / GAME_SAMPLES),
    }
}

/// Whether host:port sends an SSH banner within 6 s.
#[cfg(feature = "engine")]
async fn ssh_banner(proxy: Option<u16>, host: &str, port: u16) -> bool {
    let greet = async {
        let (mut s, mut buf) = match proxy {
            None => (TcpStream::connect((host, port)).await.ok()?, Vec::new()),
            Some(p) => connect_via(p, host, port).await.ok()?,
        };
        loop {
            if is_ssh_banner(&buf) {
                return Some(());
            }
            if buf.len() > 8192 {
                return None;
            }
            let mut chunk = [0u8; 512];
            let n = s.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    };
    matches!(tokio::time::timeout(SSH_WAIT, greet).await, Ok(Some(())))
}

#[cfg(feature = "engine")]
async fn streaming(
    proxy: Option<u16>,
    url: &str,
    reader: Reader,
    estimate: Option<&str>,
) -> Measured {
    let estimate = estimate.map(str::to_owned);
    let unreached = || Measured::Streaming {
        reachable: false,
        catalog: Catalog::Unknown,
        country: estimate.clone(),
        mbps: None,
    };
    let Some(http) = client(proxy, BROWSER) else {
        return unreached();
    };
    let deadline = tokio::time::Instant::now() + BUDGET;
    let sent = http
        .get(url)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send();
    let Ok(Ok(resp)) = tokio::time::timeout_at(deadline, sent).await else {
        return unreached();
    };
    let status = resp.status().as_u16();
    let final_url = resp.url().to_string();
    let (count, kept, took) = read_body(resp, PAGE_MAX, PAGE_KEEP, deadline).await;
    let body = String::from_utf8_lossy(&kept);
    let (catalog, region) = match reader {
        Reader::NetflixTitle => parse_netflix(status, &final_url, &body),
        Reader::YoutubePremium => parse_youtube(status, &body),
        Reader::CountryOnly => (Catalog::Unknown, None),
    };
    Measured::Streaming {
        reachable: true,
        catalog,
        country: region.or(estimate),
        mbps: mbps(count, took),
    }
}

#[cfg(feature = "engine")]
async fn downloads(proxy: Option<u16>, reach: &str, url: Option<&str>) -> Measured {
    let Some(http) = client(proxy, "Kairo-check") else {
        return Measured::Downloads {
            reachable: false,
            mb_per_s_x10: None,
        };
    };
    // Any answer is reached: a registry's 401 is it asking for a token.
    let reached = http.get(reach).send().await.is_ok();
    let mut answered = false;
    let mut speed = None;
    if let Some(url) = url {
        let deadline = tokio::time::Instant::now() + BUDGET;
        let sent = http
            .get(url)
            .header("Range", format!("bytes=0-{}", RANGE_BYTES - 1))
            .send();
        if let Ok(Ok(resp)) = tokio::time::timeout_at(deadline, sent).await {
            answered = true;
            if resp.status().is_success() {
                let (count, _, took) = read_body(resp, RANGE_BYTES, 0, deadline).await;
                speed = mb_per_s_x10(count, took);
            }
        }
    }
    Measured::Downloads {
        reachable: reached || answered,
        mb_per_s_x10: speed,
    }
}

#[cfg(feature = "engine")]
async fn dev_tools(
    proxy: Option<u16>,
    url: &str,
    ssh: Option<&(String, u16, String, u16)>,
) -> Measured {
    let page = async {
        let http = client(proxy, "Kairo-check")?;
        let started = Instant::now();
        let sent = tokio::time::timeout(BUDGET, http.get(url).send()).await;
        matches!(sent, Ok(Ok(_))).then(|| ms(started.elapsed()))
    };
    let ssh = async {
        let Some((host, port, alt_host, alt_port)) = ssh else {
            return Ssh::NotApplicable;
        };
        if ssh_banner(proxy, host, *port).await {
            Ssh::Open
        } else if ssh_banner(proxy, alt_host, *alt_port).await {
            Ssh::ClosedUse443
        } else {
            Ssh::Closed
        }
    };
    let (ttfb_ms, ssh) = tokio::join!(page, ssh);
    Measured::DevTools {
        reachable: ttfb_ms.is_some(),
        ttfb_ms,
        ssh,
    }
}

/// Checks one app through the mixed port (`proxy`; `None` goes direct, for
/// tests). `estimate` is a country another check saw over the same exit,
/// for a page that names none.
#[cfg(feature = "engine")]
pub async fn run(
    proxy: Option<u16>,
    app: &str,
    probe: &Probe,
    estimate: Option<&str>,
    now: i64,
) -> KindCheck {
    let measured = match probe {
        Probe::Streaming { url, reader } => streaming(proxy, url, *reader, estimate).await,
        Probe::Games { host, port } => games(proxy, host, *port).await,
        Probe::Downloads { reach, url } => downloads(proxy, reach, url.as_deref()).await,
        Probe::DevTools { url, ssh } => dev_tools(proxy, url, ssh.as_ref()).await,
    };
    KindCheck {
        app: app.to_owned(),
        at: now,
        measured,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_check::country_name;

    #[test]
    fn reads_a_netflix_title_page() {
        let us = r#"<script>netflix.reactContext = {"models":{"geo":{"data":{"requestCountry":{"id":"US","countryName":"United States"}}}}};</script>"#;
        let url = "https://www.netflix.com/title/70143836";
        assert_eq!(
            parse_netflix(200, url, us),
            (Catalog::Full, Some("US".into()))
        );
        let jp = r#"{"geo":{"country":"jp","locale":"ja-JP"}}"#;
        assert_eq!(
            parse_netflix(404, url, jp),
            (Catalog::OriginalsOnly, Some("JP".into()))
        );
        assert_eq!(
            parse_netflix(403, url, "<html>Forbidden</html>"),
            (Catalog::NotAvailable, None)
        );
        let gone = "<h1>Netflix is not available in your country yet.</h1>";
        assert_eq!(parse_netflix(200, url, gone).0, Catalog::NotAvailable);
        // No region in the body: the one in the URL it was sent on to.
        assert_eq!(
            parse_netflix(
                200,
                "https://www.netflix.com/jp-en/title/70143836",
                "<html/>"
            ),
            (Catalog::Full, Some("JP".into()))
        );
        assert_eq!(
            parse_netflix(404, "https://www.netflix.com/tw/title/70143836", ""),
            (Catalog::OriginalsOnly, Some("TW".into()))
        );
        assert_eq!(parse_netflix(200, url, "<html/>").1, None);
        // A "country" that is no code is passed over for the next one.
        let odd = r#"{"country":"United States"},{"country":"SG"}"#;
        assert_eq!(parse_netflix(200, url, odd).1, Some("SG".into()));
        assert_eq!(parse_netflix(500, url, "").0, Catalog::Unknown);
    }

    #[test]
    fn reads_the_youtube_premium_page() {
        let jp = r#"ytcfg.set({"INNERTUBE_CONTEXT":{"client":{"hl":"en","gl":"JP"}},"countryCode":"JP"});"#;
        assert_eq!(parse_youtube(200, jp), (Catalog::Full, Some("JP".into())));
        let hk = r#"{"HL":"en","GL":"HK","INNERTUBE_CONTEXT_GL":"HK"}"#;
        assert_eq!(parse_youtube(200, hk), (Catalog::Full, Some("HK".into())));
        assert_eq!(
            parse_youtube(200, "<html><body>YouTube</body></html>"),
            (Catalog::Unknown, None)
        );
        let gone = r#"<div>YouTube Premium is not available in your country.</div>{"GL":"CN"}"#;
        assert_eq!(
            parse_youtube(200, gone),
            (Catalog::NotAvailable, Some("CN".into()))
        );
        assert_eq!(parse_youtube(429, jp).0, Catalog::Unknown);
    }

    #[test]
    fn median_and_jitter() {
        let mut s = [40, 31, 30, 35, 33, 32, 36, 34, 90, 30];
        assert_eq!(jitter(&mut s), (Some(33), Some(10)));
        assert_eq!(jitter(&mut [20]), (Some(20), Some(0)));
        assert_eq!(jitter(&mut [10, 30, 20]), (Some(20), Some(20)));
        assert_eq!(jitter(&mut []), (None, None));
    }

    #[test]
    fn knows_an_ssh_banner() {
        assert!(is_ssh_banner(b"SSH-2.0-babeld-6f6b8d8e\r\n"));
        assert!(is_ssh_banner(b"hello there\r\nSSH-2.0-OpenSSH_9.6\r\n"));
        assert!(!is_ssh_banner(b"HTTP/1.1 400 Bad Request\r\n"));
        assert!(!is_ssh_banner(b""));
        assert!(!is_ssh_banner(b"SS"));
    }

    #[test]
    fn megabytes_a_second() {
        assert_eq!(
            mb_per_s_x10(2_097_152, Duration::from_millis(500)),
            Some(42)
        );
        assert_eq!(mb_per_s_x10(42_300_000, Duration::from_secs(1)), Some(423));
        assert_eq!(mb_per_s_x10(100_000, Duration::from_secs(1)), None);
        // No time at all is counted as 1 ms, not a division by zero.
        assert_eq!(mb_per_s_x10(1_000_000, Duration::ZERO), Some(10_000));
    }

    fn row(m: Measured) -> String {
        m.line(country_name)
    }

    #[test]
    fn says_what_was_measured() {
        let s = |reachable, catalog, country: Option<&str>| Measured::Streaming {
            reachable,
            catalog,
            country: country.map(Into::into),
            mbps: Some(80),
        };
        assert_eq!(row(s(true, Catalog::Full, Some("US"))), "US catalog");
        assert_eq!(row(s(true, Catalog::Full, Some("JP"))), "JP catalog");
        assert_eq!(row(s(true, Catalog::Full, None)), "Full catalog");
        assert_eq!(
            row(s(true, Catalog::OriginalsOnly, Some("US"))),
            "Originals only"
        );
        assert_eq!(
            row(s(true, Catalog::NotAvailable, None)),
            "Not available here"
        );
        assert_eq!(row(s(true, Catalog::Unknown, Some("JP"))), "Sees Japan");
        assert_eq!(row(s(true, Catalog::Unknown, None)), "Reachable");
        assert_eq!(row(s(false, Catalog::Unknown, Some("JP"))), "Unreachable");

        let g = |median_ms, jitter_ms, loss_pct| Measured::Games {
            median_ms,
            jitter_ms,
            loss_pct,
        };
        assert_eq!(row(g(Some(36), Some(3), 0)), "36 ms · jitter 3 ms");
        assert_eq!(
            row(g(Some(36), Some(3), 20)),
            "36 ms · jitter 3 ms · 20% lost"
        );
        assert_eq!(row(g(None, None, 100)), "Unreachable");

        let d = |reachable, mb_per_s_x10| Measured::Downloads {
            reachable,
            mb_per_s_x10,
        };
        assert_eq!(row(d(true, Some(423))), "42 MB/s");
        assert_eq!(row(d(true, Some(11))), "1.1 MB/s");
        assert_eq!(row(d(true, Some(100))), "10 MB/s");
        assert_eq!(row(d(true, None)), "Reachable");
        assert_eq!(row(d(false, None)), "Unreachable");

        let v = |reachable, ttfb_ms, ssh| Measured::DevTools {
            reachable,
            ttfb_ms,
            ssh,
        };
        assert_eq!(row(v(true, Some(210), Ssh::Open)), "Reachable · 210 ms");
        assert_eq!(
            row(v(true, Some(210), Ssh::NotApplicable)),
            "Reachable · 210 ms"
        );
        assert_eq!(
            row(v(true, Some(210), Ssh::ClosedUse443)),
            "SSH port 22 closed"
        );
        assert_eq!(row(v(true, Some(210), Ssh::Closed)), "SSH closed");
        assert_eq!(row(v(false, None, Ssh::Closed)), "Unreachable");
    }

    #[test]
    fn trouble_is_what_gets_in_the_way() {
        let s = |reachable, catalog| Measured::Streaming {
            reachable,
            catalog,
            country: None,
            mbps: None,
        };
        assert!(!s(true, Catalog::Full).trouble());
        assert!(!s(true, Catalog::OriginalsOnly).trouble());
        assert!(s(true, Catalog::NotAvailable).trouble());
        assert!(s(false, Catalog::Unknown).trouble());
        let g = |m, j, l| Measured::Games {
            median_ms: m,
            jitter_ms: j,
            loss_pct: l,
        };
        assert!(!g(Some(40), Some(5), 0).trouble());
        assert!(!g(Some(150), Some(30), 10).trouble());
        assert!(g(Some(151), Some(5), 0).trouble());
        assert!(g(Some(40), Some(31), 0).trouble());
        assert!(g(Some(40), Some(5), 20).trouble());
        assert!(g(None, None, 100).trouble());
        let d = |r, x| Measured::Downloads {
            reachable: r,
            mb_per_s_x10: x,
        };
        assert!(!d(true, Some(10)).trouble());
        assert!(!d(true, None).trouble());
        assert!(d(true, Some(9)).trouble());
        assert!(d(false, None).trouble());
        let v = |r, ssh| Measured::DevTools {
            reachable: r,
            ttfb_ms: None,
            ssh,
        };
        assert!(!v(true, Ssh::Open).trouble());
        assert!(!v(true, Ssh::NotApplicable).trouble());
        assert!(v(true, Ssh::ClosedUse443).trouble());
        assert!(v(true, Ssh::Closed).trouble());
        assert!(v(false, Ssh::Open).trouble());
        let k = KindCheck {
            app: "steam".into(),
            at: 0,
            measured: g(None, None, 100),
        };
        assert!(k.trouble());
        assert_eq!(k.line(country_name), "Unreachable");
    }

    #[test]
    fn ssh_over_443_for_the_hosts_that_offer_it() {
        assert_eq!(
            ssh_snippet("github.com").as_deref(),
            Some("Host github.com\n  Hostname ssh.github.com\n  Port 443\n  User git\n")
        );
        assert_eq!(
            ssh_snippet("gitlab.com").as_deref(),
            Some("Host gitlab.com\n  Hostname altssh.gitlab.com\n  Port 443\n  User git\n")
        );
        assert_eq!(ssh_snippet("bitbucket.org"), None);
    }

    #[test]
    fn every_app_has_its_target() {
        let streaming = [
            "netflix",
            "youtube",
            "disneyplus",
            "primevideo",
            "max",
            "spotify",
            "twitch",
            "hulu",
        ];
        let games = ["steam", "epic", "battlenet", "riot", "playstation", "xbox"];
        let downloads = [
            "huggingface",
            "github-releases",
            "pypi",
            "pytorch",
            "ollama",
            "dockerhub",
            "ghcr",
        ];
        let dev = [
            "github",
            "gitlab",
            "npm",
            "crates",
            "go",
            "jetbrains",
            "vscode-marketplace",
        ];
        for id in streaming {
            assert!(
                matches!(probe_for(id), Some(Probe::Streaming { .. })),
                "{id}"
            );
        }
        for id in games {
            assert!(
                matches!(probe_for(id), Some(Probe::Games { port: 443, .. })),
                "{id}"
            );
        }
        for id in downloads {
            assert!(
                matches!(probe_for(id), Some(Probe::Downloads { .. })),
                "{id}"
            );
        }
        for id in dev {
            assert!(
                matches!(probe_for(id), Some(Probe::DevTools { .. })),
                "{id}"
            );
        }
        assert_eq!(
            probe_for("netflix"),
            Some(Probe::Streaming {
                url: "https://www.netflix.com/title/70143836".into(),
                reader: Reader::NetflixTitle,
            })
        );
        assert_eq!(
            probe_for("youtube"),
            Some(Probe::Streaming {
                url: "https://www.youtube.com/premium".into(),
                reader: Reader::YoutubePremium,
            })
        );
        assert_eq!(
            probe_for("github"),
            Some(Probe::DevTools {
                url: "https://github.com/".into(),
                ssh: Some(("github.com".into(), 22, "ssh.github.com".into(), 443)),
            })
        );
        assert_eq!(
            probe_for("gitlab"),
            Some(Probe::DevTools {
                url: "https://gitlab.com/".into(),
                ssh: Some(("gitlab.com".into(), 22, "altssh.gitlab.com".into(), 443)),
            })
        );
        assert!(matches!(
            probe_for("huggingface"),
            Some(Probe::Downloads { url: Some(_), .. })
        ));
        assert!(matches!(
            probe_for("dockerhub"),
            Some(Probe::Downloads { url: None, .. })
        ));
        assert_eq!(probe_for("claude"), None);
        assert_eq!(probe_for(""), None);
    }

    #[test]
    fn a_check_round_trips_as_json() {
        let k = KindCheck {
            app: "netflix".into(),
            at: 1_700_000_000,
            measured: Measured::Streaming {
                reachable: true,
                catalog: Catalog::Full,
                country: Some("US".into()),
                mbps: Some(120),
            },
        };
        let j = serde_json::to_string(&k).unwrap();
        assert_eq!(serde_json::from_str::<KindCheck>(&j).unwrap(), k);
    }

    #[cfg(feature = "engine")]
    mod fixtures {
        use super::super::*;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        /// A raw HTTP/1.1 response that closes the connection.
        fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
            let mut r = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            for (k, v) in headers {
                r.push_str(&format!("{k}: {v}\r\n"));
            }
            r.push_str("\r\n");
            let mut r = r.into_bytes();
            r.extend_from_slice(body);
            r
        }

        /// Reads a request head (to its blank line).
        async fn read_head(s: &mut TcpStream) -> String {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            String::from_utf8_lossy(&buf).into_owned()
        }

        /// A tiny HTTP server: `answer` turns each request head into the
        /// bytes sent back. Its port.
        async fn http_server(answer: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> u16 {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            let answer = std::sync::Arc::new(answer);
            tokio::spawn(async move {
                while let Ok((mut s, _)) = l.accept().await {
                    let answer = answer.clone();
                    tokio::spawn(async move {
                        let head = read_head(&mut s).await;
                        let _ = s.write_all(&answer(&head)).await;
                        let _ = s.shutdown().await;
                        // Until the client closes, so no reset cuts the answer.
                        let mut sink = [0u8; 1024];
                        while matches!(s.read(&mut sink).await, Ok(n) if n > 0) {}
                    });
                }
            });
            port
        }

        /// A port that greets with `greeting` (nothing: silent) and keeps
        /// every connection open.
        async fn tcp_server(
            greeting: &'static [u8],
            answer: &'static [u8],
        ) -> (u16, tokio::task::JoinHandle<()>) {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            let task = tokio::spawn(async move {
                while let Ok((mut s, _)) = l.accept().await {
                    tokio::spawn(async move {
                        let _ = s.write_all(greeting).await;
                        let mut buf = [0u8; 2048];
                        while let Ok(n) = s.read(&mut buf).await {
                            if n == 0 {
                                break;
                            }
                            let _ = s.write_all(answer).await;
                        }
                    });
                }
            });
            (port, task)
        }

        /// A port nothing listens on.
        async fn closed_port() -> u16 {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        }

        /// A CONNECT proxy like mihomo's: it says 200 first and dials after,
        /// closing the tunnel when the dial fails. Any other request gets a
        /// small 200 page.
        async fn connect_proxy() -> u16 {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            tokio::spawn(async move {
                while let Ok((mut s, _)) = l.accept().await {
                    tokio::spawn(async move {
                        let head = read_head(&mut s).await;
                        let mut first = head.lines().next().unwrap_or("").split_whitespace();
                        if first.next() != Some("CONNECT") {
                            let _ = s.write_all(&response("200 OK", &[], b"ok")).await;
                            return;
                        }
                        let target = first.next().unwrap_or("").to_owned();
                        if s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                            .await
                            .is_err()
                        {
                            return;
                        }
                        if let Ok(mut up) = TcpStream::connect(target).await {
                            let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
                        }
                    });
                }
            });
            port
        }

        fn netflix_like(country: &str) -> Vec<u8> {
            let mut body = format!(
                r#"<html><script>netflix.reactContext = {{"models":{{"geo":{{"data":{{"requestCountry":{{"id":"{country}","countryName":"x"}}}}}}}}}};</script>"#
            )
            .into_bytes();
            body.resize(body.len() + 320 * 1024, b'x');
            body
        }

        #[tokio::test]
        async fn a_big_title_page_is_the_full_catalog_with_a_speed() {
            let port = http_server(|_| {
                response(
                    "200 OK",
                    &[("Content-Type", "text/html")],
                    &netflix_like("US"),
                )
            })
            .await;
            let probe = Probe::Streaming {
                url: format!("http://127.0.0.1:{port}/title/70143836"),
                reader: Reader::NetflixTitle,
            };
            let k = run(None, "netflix", &probe, Some("JP"), 7).await;
            assert_eq!(k.app, "netflix");
            assert_eq!(k.at, 7);
            let Measured::Streaming {
                reachable,
                catalog,
                country,
                mbps,
            } = k.measured
            else {
                panic!("{k:?}")
            };
            assert!(reachable);
            assert_eq!(catalog, Catalog::Full);
            assert_eq!(country.as_deref(), Some("US"));
            assert!(mbps.is_some_and(|m| m > 0), "{mbps:?}");
        }

        #[tokio::test]
        async fn a_404_title_is_originals_only_and_keeps_the_estimate() {
            let port =
                http_server(|_| response("404 Not Found", &[], b"<html>Not found</html>")).await;
            let probe = Probe::Streaming {
                url: format!("http://127.0.0.1:{port}/title/70143836"),
                reader: Reader::NetflixTitle,
            };
            let k = run(None, "netflix", &probe, Some("JP"), 0).await;
            assert_eq!(
                k.measured,
                Measured::Streaming {
                    reachable: true,
                    catalog: Catalog::OriginalsOnly,
                    country: Some("JP".into()),
                    mbps: None,
                }
            );
            assert_eq!(k.line(crate::ai_check::country_name), "Originals only");
        }

        #[tokio::test]
        async fn a_page_nobody_answers_is_unreachable() {
            let port = closed_port().await;
            let probe = Probe::Streaming {
                url: format!("http://127.0.0.1:{port}/"),
                reader: Reader::CountryOnly,
            };
            let k = run(None, "hulu", &probe, None, 0).await;
            assert!(matches!(
                k.measured,
                Measured::Streaming {
                    reachable: false,
                    ..
                }
            ));
            assert!(k.trouble());
        }

        #[tokio::test]
        async fn a_listening_game_port_has_a_time_and_no_loss() {
            let (port, _keep) = tcp_server(b"", b"").await;
            let probe = Probe::Games {
                host: "127.0.0.1".into(),
                port,
            };
            let Measured::Games {
                median_ms,
                jitter_ms,
                loss_pct,
            } = run(None, "steam", &probe, None, 0).await.measured
            else {
                panic!()
            };
            assert_eq!(loss_pct, 0);
            assert!(median_ms.is_some());
            assert!(jitter_ms.is_some());
        }

        #[tokio::test]
        async fn a_closed_game_port_is_all_loss() {
            let probe = Probe::Games {
                host: "127.0.0.1".into(),
                port: closed_port().await,
            };
            let k = run(None, "steam", &probe, None, 0).await;
            assert_eq!(
                k.measured,
                Measured::Games {
                    median_ms: None,
                    jitter_ms: None,
                    loss_pct: 100,
                }
            );
        }

        #[tokio::test]
        async fn ssh_that_greets_on_22_is_open() {
            let web = http_server(|_| response("200 OK", &[], b"hi")).await;
            let (ssh, _keep) = tcp_server(b"SSH-2.0-test\r\n", b"").await;
            let probe = Probe::DevTools {
                url: format!("http://127.0.0.1:{web}/"),
                ssh: Some((
                    "127.0.0.1".into(),
                    ssh,
                    "127.0.0.1".into(),
                    closed_port().await,
                )),
            };
            let Measured::DevTools {
                reachable,
                ttfb_ms,
                ssh,
            } = run(None, "github", &probe, None, 0).await.measured
            else {
                panic!()
            };
            assert!(reachable);
            assert!(ttfb_ms.is_some());
            assert_eq!(ssh, Ssh::Open);
        }

        #[tokio::test]
        async fn a_silent_22_with_ssh_on_the_alt_is_closed_use_443() {
            let web = http_server(|_| response("200 OK", &[], b"hi")).await;
            let (silent, _keep1) = tcp_server(b"", b"").await;
            let (alt, _keep2) = tcp_server(b"SSH-2.0-test\r\n", b"").await;
            let probe = Probe::DevTools {
                url: format!("http://127.0.0.1:{web}/"),
                ssh: Some(("127.0.0.1".into(), silent, "127.0.0.1".into(), alt)),
            };
            let k = run(None, "github", &probe, None, 0).await;
            assert!(matches!(
                k.measured,
                Measured::DevTools {
                    reachable: true,
                    ssh: Ssh::ClosedUse443,
                    ..
                }
            ));
            assert_eq!(k.line(crate::ai_check::country_name), "SSH port 22 closed");
        }

        #[tokio::test]
        async fn no_ssh_entry_is_not_applicable() {
            let web = http_server(|_| response("200 OK", &[], b"{}")).await;
            let probe = Probe::DevTools {
                url: format!("http://127.0.0.1:{web}/config.json"),
                ssh: None,
            };
            let k = run(None, "crates", &probe, None, 0).await;
            assert!(matches!(
                k.measured,
                Measured::DevTools {
                    reachable: true,
                    ttfb_ms: Some(_),
                    ssh: Ssh::NotApplicable,
                }
            ));
        }

        #[tokio::test]
        async fn a_ranged_download_has_a_speed() {
            let port = http_server(|head| {
                let ranged = head
                    .lines()
                    .any(|l| l.eq_ignore_ascii_case("range: bytes=0-2097151"));
                if head.starts_with("GET /big ") && ranged {
                    let body = vec![7u8; 2 * 1024 * 1024];
                    response(
                        "206 Partial Content",
                        &[("Content-Range", "bytes 0-2097151/9999999")],
                        &body,
                    )
                } else if head.starts_with("GET /big ") {
                    response("416 Range Not Satisfiable", &[], b"")
                } else {
                    response("401 Unauthorized", &[], b"{}")
                }
            })
            .await;
            let probe = Probe::Downloads {
                reach: format!("http://127.0.0.1:{port}/v2/"),
                url: Some(format!("http://127.0.0.1:{port}/big")),
            };
            let Measured::Downloads {
                reachable,
                mb_per_s_x10,
            } = run(None, "huggingface", &probe, None, 0).await.measured
            else {
                panic!()
            };
            assert!(reachable);
            assert!(mb_per_s_x10.is_some_and(|x| x > 0), "{mb_per_s_x10:?}");

            // Only the reach: a 401 is reached, with no speed.
            let probe = Probe::Downloads {
                reach: format!("http://127.0.0.1:{port}/v2/"),
                url: None,
            };
            assert_eq!(
                run(None, "dockerhub", &probe, None, 0).await.measured,
                Measured::Downloads {
                    reachable: true,
                    mb_per_s_x10: None,
                }
            );
        }

        #[tokio::test]
        async fn through_a_connect_proxy_that_answers_before_it_dials() {
            let proxy = connect_proxy().await;
            // A game server answers the ClientHello (with an alert).
            let (game, _keep1) = tcp_server(b"", b"\x15\x03\x03\x00\x02\x02\x28").await;
            let probe = Probe::Games {
                host: "127.0.0.1".into(),
                port: game,
            };
            let Measured::Games {
                median_ms,
                loss_pct,
                ..
            } = run(Some(proxy), "steam", &probe, None, 0).await.measured
            else {
                panic!()
            };
            assert_eq!(loss_pct, 0);
            assert!(median_ms.is_some());

            // The proxy's early 200 is not taken for an answer: a closed port
            // is still all loss.
            let probe = Probe::Games {
                host: "127.0.0.1".into(),
                port: closed_port().await,
            };
            let k = run(Some(proxy), "steam", &probe, None, 0).await;
            assert!(matches!(
                k.measured,
                Measured::Games {
                    median_ms: None,
                    loss_pct: 100,
                    ..
                }
            ));

            // SSH through the tunnel: on 22, and on the alt when 22 is closed.
            let (ssh, _keep2) = tcp_server(b"SSH-2.0-test\r\n", b"").await;
            let web = format!("http://127.0.0.1:{}/", closed_port().await);
            let probe = Probe::DevTools {
                url: web.clone(),
                ssh: Some((
                    "127.0.0.1".into(),
                    ssh,
                    "127.0.0.1".into(),
                    closed_port().await,
                )),
            };
            let k = run(Some(proxy), "github", &probe, None, 0).await;
            // The proxy answers plain requests itself: reached.
            assert!(matches!(
                k.measured,
                Measured::DevTools {
                    reachable: true,
                    ssh: Ssh::Open,
                    ..
                }
            ));
            let probe = Probe::DevTools {
                url: web,
                ssh: Some((
                    "127.0.0.1".into(),
                    closed_port().await,
                    "127.0.0.1".into(),
                    ssh,
                )),
            };
            let k = run(Some(proxy), "github", &probe, None, 0).await;
            assert!(matches!(
                k.measured,
                Measured::DevTools {
                    ssh: Ssh::ClosedUse443,
                    ..
                }
            ));
            let probe = Probe::DevTools {
                url: format!("http://127.0.0.1:{ssh}/"),
                ssh: Some((
                    "127.0.0.1".into(),
                    closed_port().await,
                    "127.0.0.1".into(),
                    closed_port().await,
                )),
            };
            let k = run(Some(proxy), "gitlab", &probe, None, 0).await;
            assert!(matches!(
                k.measured,
                Measured::DevTools {
                    ssh: Ssh::Closed,
                    ..
                }
            ));
        }
    }
}
