//! The AI check: for each service, can it be reached, does its API answer, and
//! which country does it see. Everything goes through Kairo's own proxy port,
//! so the rules send the probe the way the service's real traffic goes.
//!
//! What it never does: sign in, send a key, or say that a country is "fine".
//! It reports what it read. A country comes from the service's own Cloudflare
//! trace when it has one, else it is an estimate and is marked so.

use std::time::{Duration, Instant};

use crate::ai::{AiService, SERVICES};

/// Where the country came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Country {
    /// Read from the service's own answer (a Cloudflare trace).
    Seen {
        code: String,
        warp: bool,
    },
    /// The country another AI host reported for the same exit: a guess for a
    /// service that has no trace of its own.
    Estimated {
        code: String,
    },
    Unknown,
}

impl Country {
    pub fn code(&self) -> Option<&str> {
        match self {
            Country::Seen { code, .. } | Country::Estimated { code } => Some(code),
            Country::Unknown => None,
        }
    }
}

/// The API's answer to a call without a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Api {
    /// An answer that means "reachable" (a 401 is the API asking for a key).
    Answers {
        ms: u32,
    },
    /// Some other status: reached, but not what that API says when it is well.
    Unexpected {
        status: u16,
    },
    Unreachable {
        why: String,
    },
    /// This service has no API Kairo can ask.
    None,
}

/// One service's check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub service: &'static str,
    /// The trace host (or the API) answered at all.
    pub reachable: bool,
    pub api: Api,
    pub country: Country,
    /// Unix seconds.
    pub at: i64,
}

impl Check {
    /// Whether something here needs a look: a service not reached, or an API
    /// that answered oddly.
    pub fn trouble(&self) -> bool {
        !self.reachable || matches!(self.api, Api::Unexpected { .. } | Api::Unreachable { .. })
    }
}

/// The fields of a Cloudflare trace.
pub fn parse_trace(body: &str) -> Option<(String, bool)> {
    let mut loc = None;
    let mut warp = false;
    for line in body.lines() {
        match line.split_once('=') {
            Some(("loc", v)) if v.len() == 2 => loc = Some(v.to_ascii_uppercase()),
            Some(("warp", v)) => warp = v == "on" || v == "plus",
            _ => {}
        }
    }
    loc.map(|l| (l, warp))
}

/// A country code as a name, for the codes services report.
pub fn country_name(code: &str) -> String {
    match code {
        "US" => "United States",
        "JP" => "Japan",
        "SG" => "Singapore",
        "HK" => "Hong Kong",
        "TW" => "Taiwan",
        "KR" => "South Korea",
        "DE" => "Germany",
        "GB" => "United Kingdom",
        "FR" => "France",
        "NL" => "Netherlands",
        "CA" => "Canada",
        "AU" => "Australia",
        "IN" => "India",
        "CN" => "China",
        "RU" => "Russia",
        other => other,
    }
    .to_owned()
}

fn client(proxy_port: u16) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{proxy_port}")).ok()?)
        .timeout(Duration::from_secs(12))
        .user_agent("Kairo-check")
        .build()
        .ok()
}

async fn trace(http: &reqwest::Client, host: &str) -> Option<(String, bool)> {
    let r = http
        .get(format!("https://{host}/cdn-cgi/trace"))
        .send()
        .await
        .ok()?;
    if !r.status().is_success() {
        return None;
    }
    parse_trace(&r.text().await.ok()?)
}

/// Checks one service. `estimate` is the country of another service's trace
/// over the same exit, for those that have none of their own.
pub async fn check_service(
    proxy_port: u16,
    service: &'static AiService,
    estimate: Option<&str>,
    now: i64,
) -> Check {
    let Some(http) = client(proxy_port) else {
        return Check {
            service: service.id,
            reachable: false,
            api: Api::Unreachable {
                why: "no proxy client".into(),
            },
            country: Country::Unknown,
            at: now,
        };
    };
    let traced = match service.trace_host {
        Some(h) => trace(&http, h).await,
        None => None,
    };
    let api = match service.api {
        Some((host, path, ok)) => {
            let started = Instant::now();
            match http.get(format!("https://{host}{path}")).send().await {
                Ok(r) => {
                    let status = r.status().as_u16();
                    // "Answers" is the status that API gives without a key.
                    if ok.contains(&status) {
                        Api::Answers {
                            ms: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
                        }
                    } else {
                        Api::Unexpected { status }
                    }
                }
                Err(e) => Api::Unreachable { why: short(&e) },
            }
        }
        None => Api::None,
    };
    let country = match (&traced, estimate) {
        (Some((code, warp)), _) => Country::Seen {
            code: code.clone(),
            warp: *warp,
        },
        (None, Some(e)) => Country::Estimated { code: e.to_owned() },
        (None, None) => Country::Unknown,
    };
    let reachable = traced.is_some() || matches!(api, Api::Answers { .. } | Api::Unexpected { .. });
    Check {
        service: service.id,
        reachable,
        api,
        country,
        at: now,
    }
}

fn short(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "no answer in 12 s".into()
    } else if e.is_connect() {
        "could not connect".into()
    } else {
        "the connection failed".into()
    }
}

/// Checks every service. The estimate for those without a trace is the first
/// country a service with one reported.
pub async fn check_all(proxy_port: u16, now: i64) -> Vec<Check> {
    let mut done = Vec::new();
    let mut estimate: Option<String> = None;
    // Those with a trace first, so the others have something to estimate from.
    let mut order: Vec<&'static AiService> =
        SERVICES.iter().filter(|s| s.trace_host.is_some()).collect();
    order.extend(SERVICES.iter().filter(|s| s.trace_host.is_none()));
    let first = order.iter().take_while(|s| s.trace_host.is_some()).count();
    let (traced, rest) = order.split_at(first);
    let futs = traced
        .iter()
        .map(|s| check_service(proxy_port, s, None, now));
    for c in futures_util::future::join_all(futs).await {
        if estimate.is_none() {
            estimate = c.country.code().map(str::to_owned);
        }
        done.push(c);
    }
    let futs = rest
        .iter()
        .map(|s| check_service(proxy_port, s, estimate.as_deref(), now));
    done.extend(futures_util::future::join_all(futs).await);
    // In the order the services are shown.
    done.sort_by_key(|c| {
        SERVICES
            .iter()
            .position(|s| s.id == c.service)
            .unwrap_or(usize::MAX)
    });
    done
}

/// What differs between services: the countries, when they do not all agree.
/// `None` when they agree (or fewer than two reported).
pub fn odd_ones(checks: &[Check]) -> Option<(String, Vec<&'static str>)> {
    let seen: Vec<(&'static str, &str)> = checks
        .iter()
        .filter_map(|c| match &c.country {
            Country::Seen { code, .. } => Some((c.service, code.as_str())),
            _ => None,
        })
        .collect();
    if seen.len() < 2 {
        return None;
    }
    // The country most services report is "where the rest are".
    let mut best: (&str, usize) = ("", 0);
    for (_, code) in &seen {
        let n = seen.iter().filter(|(_, c)| c == code).count();
        if n > best.1 {
            best = (code, n);
        }
    }
    let odd: Vec<&'static str> = seen
        .iter()
        .filter(|(_, c)| *c != best.0)
        .map(|(s, _)| *s)
        .collect();
    (!odd.is_empty()).then(|| (best.0.to_owned(), odd))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chk(service: &'static str, country: Country) -> Check {
        Check {
            service,
            reachable: true,
            api: Api::None,
            country,
            at: 0,
        }
    }

    #[test]
    fn reads_a_trace() {
        let body = "fl=1008f76\nh=chatgpt.com\nip=203.0.113.9\nloc=jp\nwarp=off\n";
        assert_eq!(parse_trace(body), Some(("JP".into(), false)));
        assert_eq!(parse_trace("loc=TW\nwarp=on\n"), Some(("TW".into(), true)));
        assert_eq!(parse_trace("<html>Just a moment</html>"), None);
        assert_eq!(parse_trace("loc=XXX\n"), None);
    }

    #[test]
    fn finds_the_service_that_sees_another_country() {
        let jp = |s| {
            chk(
                s,
                Country::Seen {
                    code: "JP".into(),
                    warp: false,
                },
            )
        };
        let all = [
            jp("claude"),
            jp("chatgpt"),
            chk(
                "gemini",
                Country::Seen {
                    code: "HK".into(),
                    warp: false,
                },
            ),
        ];
        assert_eq!(odd_ones(&all), Some(("JP".into(), vec!["gemini"])));
        assert_eq!(odd_ones(&[jp("claude"), jp("chatgpt")]), None);
        // An estimate is not evidence of disagreement.
        let est = [
            jp("claude"),
            jp("chatgpt"),
            chk("cursor", Country::Estimated { code: "HK".into() }),
        ];
        assert_eq!(odd_ones(&est), None);
        assert_eq!(odd_ones(&[jp("claude")]), None);
    }

    #[test]
    fn trouble_is_unreached_or_odd_api() {
        let mut c = chk("claude", Country::Unknown);
        assert!(!c.trouble());
        c.api = Api::Answers { ms: 300 };
        assert!(!c.trouble());
        c.api = Api::Unexpected { status: 500 };
        assert!(c.trouble());
        c.api = Api::Unreachable { why: "x".into() };
        assert!(c.trouble());
        c.api = Api::None;
        c.reachable = false;
        assert!(c.trouble());
    }

    #[test]
    fn names_countries() {
        assert_eq!(country_name("JP"), "Japan");
        assert_eq!(country_name("ZZ"), "ZZ");
    }

    #[tokio::test]
    async fn a_dead_proxy_is_unreachable_not_a_crash() {
        // Nothing listens on port 9: every probe fails at once.
        let s = SERVICES.iter().find(|s| s.id == "claude").unwrap();
        let c = check_service(9, s, None, 0).await;
        assert!(!c.reachable);
        assert!(matches!(c.api, Api::Unreachable { .. }));
        assert_eq!(c.country, Country::Unknown);
        assert!(c.trouble());
    }
}
