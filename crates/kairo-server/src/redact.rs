//! What never reaches a browser: the subscription links (they carry the
//! account's token) and the controller's secret. Every message the server
//! sends goes through [`encode`]: the state is redacted field by field, and
//! then every string of every message is scrubbed of the secrets it knows,
//! so a link quoted in an error, a notice or a log line is caught too.
//!
//! A redacted value the page sends back unchanged (the settings it edits,
//! a subscription it renames) must leave the real one as it is: see
//! [`keep_hidden_settings`] and [`keep_hidden_url`].

use kairo_core::profile::Source;
use kairo_core::protocol::{HIDDEN, ServerMsg};
use kairo_core::{Settings, State};
use serde_json::Value;

/// The secrets of the moment, from the state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Secrets {
    /// Each secret string and what stands in for it.
    pairs: Vec<(String, String)>,
}

impl Secrets {
    pub fn of(state: &State) -> Secrets {
        Secrets::with_links(state, &[])
    }

    /// The state's secrets and those of `links` (sent by pages, perhaps
    /// never a subscription).
    pub fn with_links(state: &State, links: &[String]) -> Secrets {
        let mut pairs = Vec::new();
        for url in links {
            add_url(&mut pairs, url);
        }
        for p in &state.profiles.profiles {
            if let Source::Remote { url, .. } = &p.source {
                add_url(&mut pairs, url);
            }
        }
        // A secret short enough to match ordinary words is no secret worth
        // scrubbing (Kairo makes 32 hex characters).
        if state.settings.secret.len() >= 8 {
            pairs.push((state.settings.secret.clone(), HIDDEN.to_owned()));
        }
        // Longest first: a link is replaced before a part of it.
        pairs.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        pairs.dedup();
        Secrets { pairs }
    }

    /// `text` with every secret replaced.
    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (secret, shown) in &self.pairs {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), shown);
            }
        }
        out
    }

    fn scrub_value(&self, v: &mut Value) {
        match v {
            Value::String(s) => {
                if self.pairs.iter().any(|(secret, _)| s.contains(secret.as_str())) {
                    *s = self.scrub(s);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|i| self.scrub_value(i)),
            Value::Object(map) => map.values_mut().for_each(|i| self.scrub_value(i)),
            _ => {}
        }
    }
}

/// A link and the parts of it that carry its token: the link itself, its
/// path and query, and the query alone; each stands as the link's host.
fn add_url(pairs: &mut Vec<(String, String)>, url: &str) {
    let shown = shown_url(url);
    if url.is_empty() {
        return;
    }
    pairs.push((url.to_owned(), shown.clone()));
    if let Ok(u) = url::Url::parse(url) {
        let path = u.path();
        let query = u.query().unwrap_or_default();
        // Long enough not to be an ordinary word of a message.
        if query.len() >= 8 {
            pairs.push((format!("{path}?{query}"), "/…".to_owned()));
            pairs.push((query.to_owned(), "…".to_owned()));
        }
        if path.len() >= 12 {
            pairs.push((path.to_owned(), "/…".to_owned()));
        }
        if let Some(pass) = u.password().filter(|p| p.len() >= 4) {
            pairs.push((pass.to_owned(), "…".to_owned()));
        }
        // As a log or an error may quote it, percent-encoded.
        let encoded: String = url::form_urlencoded::byte_serialize(url.as_bytes()).collect();
        if encoded != url {
            pairs.push((encoded, shown));
        }
    }
}

/// A link as the page sees it: where it goes, not what it carries
/// (`https://example.com/`).
pub fn shown_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let host = u.host_str().unwrap_or_default();
            match u.port() {
                Some(p) => format!("{}://{host}:{p}/", u.scheme()),
                None => format!("{}://{host}/", u.scheme()),
            }
        }
        Err(_) => "…".to_owned(),
    }
}

/// The state as a page may see it.
pub fn redact_state(state: &State) -> State {
    let mut s = state.clone();
    s.settings.secret = HIDDEN.to_owned();
    for p in &mut s.profiles.profiles {
        if let Source::Remote { url, .. } = &mut p.source {
            *url = shown_url(url);
        }
    }
    s
}

/// One message, ready to send: redacted and scrubbed.
pub fn encode(msg: &ServerMsg, secrets: &Secrets) -> String {
    let redacted;
    let msg = match msg {
        ServerMsg::State(s) => {
            redacted = ServerMsg::State(Box::new(redact_state(s)));
            &redacted
        }
        other => other,
    };
    let mut value = serde_json::to_value(msg).unwrap_or(Value::Null);
    secrets.scrub_value(&mut value);
    value.to_string()
}

/// Settings from the page: what it was not shown stays as it is here, and
/// what a page may not change too: the program the server runs (any binary
/// would do), and the desktop app's login item, minimizing and tray (the
/// server starts with the machine; a login item would start the desktop app).
pub fn keep_hidden_settings(mut next: Settings, current: &Settings) -> Settings {
    if next.secret == HIDDEN || next.secret.is_empty() {
        next.secret = current.secret.clone();
    }
    next.core_path = current.core_path.clone();
    next.launch_at_login = current.launch_at_login;
    next.start_minimized = current.start_minimized;
    next.close_to_tray = current.close_to_tray;
    next
}

/// A subscription's new link from the page: `None` (unchanged) when the
/// page only sent back what it was shown, or nothing.
pub fn keep_hidden_url(url: Option<String>, current: Option<&str>) -> Option<String> {
    let url = url?;
    let url = url.trim();
    if url.is_empty() || url == "…" {
        return None;
    }
    if let Some(current) = current {
        if url == shown_url(current) || url == current {
            return None;
        }
    }
    Some(url.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kairo_core::profile::{Profile, ProfileIndex};

    const LINK: &str = "https://sub.example.com/api/v1/client/subscribe?token=8f3c2a7e91d04b6f";
    const SECRET: &str = "5d41402abc4b2a76b9719d911017c592";

    fn state() -> State {
        let mut s = State::default();
        s.settings.secret = SECRET.into();
        s.profiles = ProfileIndex {
            active: Some("a".into()),
            profiles: vec![Profile {
                id: "a".into(),
                name: "sub.example.com".into(),
                source: Source::Remote { url: LINK.into(), interval_hours: 24 },
                updated_at: None,
                usage: None,
                web_page: None,
                last_error: Some(format!("Couldn't download {LINK}: 502")),
                failed_at: None,
                failure_dismissed: false,
                stats: Default::default(),
            }],
        };
        s.notice = Some(kairo_core::Notice {
            id: 1,
            text: format!("Updated from {LINK}"),
            error: false,
        });
        s
    }

    fn leaks(text: &str) -> Vec<&'static str> {
        let encoded: String = url::form_urlencoded::byte_serialize(LINK.as_bytes()).collect();
        let encoded: &'static str = Box::leak(encoded.into_boxed_str());
        [LINK, SECRET, "8f3c2a7e91d04b6f", "/api/v1/client/subscribe", encoded]
            .into_iter()
            .filter(|s| text.contains(s))
            .collect()
    }

    #[test]
    fn the_state_carries_no_link_and_no_secret() {
        let s = state();
        let out = encode(&ServerMsg::State(Box::new(s.clone())), &Secrets::of(&s));
        assert_eq!(leaks(&out), Vec::<&str>::new(), "{out}");
        assert!(out.contains("https://sub.example.com/"), "the host stays: {out}");
        // The page still reads it.
        let back: ServerMsg = serde_json::from_str(&out).unwrap();
        let ServerMsg::State(back) = back else { panic!() };
        assert_eq!(back.settings.secret, HIDDEN);
    }

    #[test]
    fn logs_and_replies_are_scrubbed_too() {
        let s = state();
        let secrets = Secrets::of(&s);
        let encoded: String = url::form_urlencoded::byte_serialize(LINK.as_bytes()).collect();
        let msgs = [
            ServerMsg::Logs {
                reset: false,
                seq: 3,
                lines: vec![kairo_core::model::LogEntry {
                    seq: 3,
                    time: "12:00:00".into(),
                    level: "error".into(),
                    payload: format!("fetch {LINK} failed; secret {SECRET}; as a parameter url={encoded}"),
                }],
            },
            ServerMsg::reply(7, Err(format!("Couldn't download {LINK}"))),
            ServerMsg::reply(8, Ok(serde_json::json!({ "deep": [format!("token=8f3c2a7e91d04b6f")] }))),
        ];
        for m in msgs {
            let out = encode(&m, &secrets);
            assert_eq!(leaks(&out), Vec::<&str>::new(), "{out}");
        }
    }

    #[test]
    fn echoed_redactions_keep_the_real_values() {
        let s = state();
        let page = redact_state(&s);
        // The settings the page edits come back with the hidden secret.
        let mut back = page.settings.clone();
        back.mixed_port = 1234;
        back.core_path = Some("/bin/sh".into());
        back.launch_at_login = !s.settings.launch_at_login;
        back.allow_lan = !s.settings.allow_lan;
        let kept = keep_hidden_settings(back, &s.settings);
        assert_eq!(kept.secret, SECRET);
        assert_eq!(kept.mixed_port, 1234);
        assert_eq!(kept.core_path, s.settings.core_path, "a page cannot pick the binary");
        assert_eq!(kept.launch_at_login, s.settings.launch_at_login, "nor add a login item");
        assert_ne!(kept.allow_lan, s.settings.allow_lan, "the network switch is the person's");
        // The link it was shown, sent back as is, changes nothing.
        let shown = page.profiles.profiles[0].url().unwrap().to_owned();
        assert_eq!(keep_hidden_url(Some(shown), Some(LINK)), None);
        assert_eq!(keep_hidden_url(Some(String::new()), Some(LINK)), None);
        assert_eq!(keep_hidden_url(None, Some(LINK)), None);
        let new = "https://other.example.com/sub?token=x".to_owned();
        assert_eq!(keep_hidden_url(Some(new.clone()), Some(LINK)), Some(new));
    }
}
