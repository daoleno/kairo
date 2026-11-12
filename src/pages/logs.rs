//! Diagnostics (Settings › Advanced › Inspect): the core's log, live, newest
//! first, with the core's state above it; search, level, pause, clear, and
//! Copy Diagnostics, a summary for a bug report with nothing private in it.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use kairo_core::model::LogEntry;
use kairo_core::{CoreStatus, State};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::chrome::Glyph;
use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, Variant, size};

fn rank(level: &str) -> u8 {
    match level {
        "debug" => 0,
        "info" => 1,
        "warning" => 2,
        "error" => 3,
        _ => 1,
    }
}

/// The levels the filter offers, lowest first.
const LEVELS: [&str; 4] = ["Debug", "Info", "Warning", "Error"];

pub fn page() -> impl IntoElement {
    let store = store();
    let query = signal(String::new());
    let paused = signal(None::<Arc<Vec<LogEntry>>>);
    // A view filter only: what the core writes is its own setting.
    let level = signal(1usize);
    let live = computed(move || {
        let _ = store.log_seq.get();
        let mut all = store.svc().logs();
        all.reverse();
        Arc::new(all)
    });
    let rows = computed(move || {
        let q = query.get().trim().to_lowercase();
        let min = level.get() as u8;
        let src = paused.get().unwrap_or_else(|| live.get());
        let list: Vec<LogEntry> = src
            .iter()
            .filter(|l| {
                rank(&l.level) >= min && (q.is_empty() || l.payload.to_lowercase().contains(&q))
            })
            .cloned()
            .collect();
        Arc::new(list)
    });
    let copy = move || {
        let svc = store.svc();
        let state = store.state.get_untracked();
        let mut lines = svc.logs();
        let skip = lines.len().saturating_sub(DIAGNOSTIC_LINES);
        lines.drain(..skip);
        let text = diagnostics_text(&Report {
            kairo_version: &svc.version(),
            core: core_text(&state),
            os: svc.os_text(),
            ports: (state.settings.mixed_port, state.settings.controller_port),
            lines: &lines,
            hidden: &hidden_strings(
                &state,
                svc.paths().home.to_str(),
                svc.user_home().as_deref(),
            ),
        });
        ui::copy(store, text, "Diagnostics copied");
    };

    view()
        .flex_1()
        .child(toolbar::toolbar(
            Page::Logs,
            move || {
                let n = rows.with(|r| r.len());
                let lines = if n == 1 {
                    "1 line".to_owned()
                } else {
                    format!("{n} lines")
                };
                if paused.with(Option::is_some) {
                    format!("{lines} · paused")
                } else {
                    lines
                }
            },
            vec![
                crate::pages::settings::back(),
                toolbar::search("logs.search", "Search the log", query),
                toolbar::choice(
                    "logs.level",
                    "Level",
                    Glyph::Filter,
                    LEVELS.to_vec(),
                    move || level.get(),
                    move |i| level.set(i.min(LEVELS.len() - 1)),
                ),
                toolbar::button_dyn(
                    "logs.pause",
                    move || {
                        if paused.with(Option::is_some) {
                            "Resume"
                        } else {
                            "Pause"
                        }
                    },
                    move || {
                        if paused.with(Option::is_some) {
                            (Glyph::Resume, Icon::Play)
                        } else {
                            (Glyph::Pause, Icon::Pause)
                        }
                    },
                    move || {
                        if paused.with_untracked(Option::is_some) {
                            paused.set(None);
                        } else {
                            paused.set(Some(live.get_untracked()));
                        }
                    },
                ),
                toolbar::button(
                    "logs.clear",
                    "Clear",
                    Glyph::Clear,
                    Icon::Trash,
                    move || {
                        store.svc().clear_logs();
                        paused.set(None);
                    },
                ),
                toolbar::button(
                    "logs.copy",
                    "Copy Diagnostics",
                    Glyph::Copy,
                    Icon::File,
                    copy,
                )
                .prominent(),
            ],
        ))
        .child(header())
        .child(
            when(
                move || rows.with(|r| !r.is_empty()),
                move || {
                    virtual_list(
                        move || rows.with(|r| r.len()),
                        move |at| {
                            let entry = computed(move || rows.with(|r| r.get(at.index()).cloned()));
                            let lvl = move || {
                                entry.with(|e| {
                                    e.as_ref().map(|e| e.level.clone()).unwrap_or_default()
                                })
                            };
                            view()
                                .flex_row()
                                .items_start()
                                .gap_3()
                                .px_5()
                                .py(px(5.0))
                                .hover(|s| s.bg(move || theme().accent))
                                .child(
                                    text(move || -> SharedString {
                                        entry
                                            .with(|e| {
                                                e.as_ref()
                                                    .map(|e| e.time.clone())
                                                    .unwrap_or_default()
                                            })
                                            .into()
                                    })
                                    .text_size(px(size::small()))
                                    .tabular_nums()
                                    .text_color(move || theme().muted_foreground)
                                    .w(px(60.0 * size::scale()))
                                    .pt(px(1.0)),
                                )
                                .child(
                                    text(move || -> SharedString {
                                        match lvl().as_str() {
                                            "warning" => "WARN".into(),
                                            other => other.to_uppercase().into(),
                                        }
                                    })
                                    .text_size(px(size::caption()))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .w(px(44.0))
                                    .pt(px(2.0))
                                    .text_color(move || {
                                        match lvl().as_str() {
                                            "error" => theme().destructive,
                                            "warning" => ui::warning(),
                                            "debug" => theme().muted_foreground,
                                            _ => theme().primary,
                                        }
                                    }),
                                )
                                .child(
                                    text(move || -> SharedString {
                                        entry
                                            .with(|e| {
                                                e.as_ref()
                                                    .map(|e| e.payload.clone())
                                                    .unwrap_or_default()
                                            })
                                            .into()
                                    })
                                    .text_size(px(size::body()))
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .number_of_lines(3)
                                    .selectable(true),
                                )
                        },
                    )
                    .keys(move || rows.with(|r| r.iter().map(|e| e.seq).collect::<Vec<_>>()))
                    .estimated_item_size(px(26.0))
                    .flex_1()
                },
            )
            .otherwise(move || {
                ui::empty_state(
                    Icon::Logs,
                    move || -> SharedString {
                        if !store.state.with(|s| s.core.is_running()) {
                            "The core is not running".into()
                        } else if query.with(|q| !q.is_empty()) {
                            "Nothing matches".into()
                        } else {
                            "No log lines".into()
                        }
                    },
                    None,
                )
            }),
        )
}

/// The core and Kairo above the log: versions, state, memory, ports, and
/// the buttons that go with them.
fn header() -> impl IntoElement {
    let store = store();
    let home = store.svc().paths().home.clone();
    let shown_home = tilde(
        &home.display().to_string(),
        store.svc().user_home().as_deref(),
    );
    let core_line = move || -> SharedString {
        let (text, running) = store.state.with(|s| (core_text(s), s.core.is_running()));
        let memory = store.live.with(|l| l.memory);
        let ports = store.state.with(|s| {
            format!(
                "ports {} / {}",
                s.settings.mixed_port, s.settings.controller_port
            )
        });
        let mut line = text;
        if running && memory > 0 {
            line.push_str(&format!(" · memory {}", kairo_core::bytes(memory)));
        }
        format!("{line} · {ports}").into()
    };
    let running = move || store.state.with(|s| s.core.is_running());
    ui::card()
        .mx_5()
        .mt_3()
        .mb_2()
        .child(ui::setting_row_dyn(
            "Core",
            core_line,
            ui::btn(
                move || -> SharedString {
                    if running() {
                        "Restart Core".into()
                    } else {
                        "Start Core".into()
                    }
                },
                Variant::Secondary,
            )
            .on_press(move || {
                if running() {
                    store.run(store.svc().restart_core());
                } else {
                    store.run(store.svc().start_core());
                }
            }),
        ))
        .child(ui::divider().ml_4())
        .child(ui::setting_row_dyn(
            concat!("Kairo ", env!("CARGO_PKG_VERSION")),
            move || -> SharedString { format!("data folder {shown_home}").into() },
            ui::btn("Open Folder", Variant::Secondary).on_press(move || {
                let home = home.clone();
                spawn(async move {
                    let _ = open_path(home).await;
                });
            }),
        ))
}

/// "mihomo 1.19.18 (running)".
fn core_text(s: &State) -> String {
    let version = match &s.core_version {
        Some(v) => format!("mihomo {v}"),
        None => "mihomo".to_owned(),
    };
    let state = match &s.core {
        CoreStatus::Running { .. } => "running",
        CoreStatus::Starting => "starting",
        CoreStatus::Stopping => "stopping",
        CoreStatus::Stopped => "stopped",
        CoreStatus::Failed { .. } => "stopped with an error",
    };
    format!("{version} · {state}")
}

// --- Copy Diagnostics ----------------------------------------------------------------

/// How many of the newest log lines a report carries.
const DIAGNOSTIC_LINES: usize = 200;

/// What a report says.
struct Report<'a> {
    kairo_version: &'a str,
    /// "mihomo 1.19.18 · running".
    core: String,
    os: String,
    /// The proxy port and the controller port.
    ports: (u16, u16),
    /// The log, oldest first.
    lines: &'a [LogEntry],
    /// Text that must not appear (see [`hidden_strings`]).
    hidden: &'a [String],
}

/// What a report must never contain, as literal text: the controller's secret,
/// every subscription's link (and its host, and the provider's page), and the
/// folders that carry the user's name.
fn hidden_strings(state: &State, kairo_home: Option<&str>, user_home: Option<&str>) -> Vec<String> {
    let mut out = vec![state.settings.secret.clone()];
    for p in &state.profiles.profiles {
        for url in p.url().into_iter().chain(p.web_page.as_deref()) {
            out.push(url.to_owned());
            if let Ok(u) = url::Url::parse(url)
                && let Some(host) = u.host_str()
            {
                out.push(host.to_owned());
            }
        }
    }
    out.extend(kairo_home.map(str::to_owned));
    out.extend(user_home.map(str::to_owned));
    out
}

/// A report for a bug: Kairo's and the core's versions, the system, the ports,
/// and the newest log lines, cleaned by [`redact`].
fn diagnostics_text(r: &Report) -> String {
    let skip = r.lines.len().saturating_sub(DIAGNOSTIC_LINES);
    let lines = &r.lines[skip..];
    let mut out = format!(
        "Kairo {}\n{}\nSystem: {}\nPorts: proxy {}, controller {}\n\nThe last {} log line{} (oldest first; addresses, links and keys removed):\n",
        r.kairo_version,
        r.core,
        r.os,
        r.ports.0,
        r.ports.1,
        lines.len(),
        if lines.len() == 1 { "" } else { "s" },
    );
    for l in lines {
        out.push_str(&format!(
            "{} {} {}\n",
            l.time,
            l.level,
            redact(&l.payload, r.hidden)
        ));
    }
    // The header and the lines went through the same filter: nothing of `hidden`
    // survives in the core's or the system's text either.
    scrub_literals(&out, r.hidden)
}

/// A log line without what is private: the literal `hidden` texts, links down
/// to their host, IP addresses, the address a dial failed on (a node's), keys
/// and tokens, long token-like words.
fn redact(line: &str, hidden: &[String]) -> String {
    let line = scrub_literals(line, hidden);
    let mut out = String::with_capacity(line.len());
    let mut word = String::new();
    let mut after_key = false;
    // Words after "dial": the protocol, then the address.
    let mut dial = 0u8;
    let flush = |word: &mut String,
                 out: &mut String,
                 after_key: &mut bool,
                 dial: &mut u8,
                 next: Option<char>| {
        if word.is_empty() {
            return;
        }
        let w = std::mem::take(word);
        let lower = w.to_ascii_lowercase();
        if std::mem::take(after_key) {
            out.push_str("<redacted>");
            return;
        }
        // "key=value": what follows a secret-looking key is the secret.
        if next == Some('=') && is_secret_key(&lower) {
            *after_key = true;
        }
        match *dial {
            1 if ["tcp", "tcp4", "tcp6", "udp", "udp4", "udp6"].contains(&lower.as_str()) => {
                *dial = 2;
                out.push_str(&w);
                return;
            }
            2 => {
                *dial = 0;
                let (_, tail) = split_tail(&w);
                out.push_str("<server>");
                out.push_str(tail);
                return;
            }
            _ => *dial = 0,
        }
        if lower == "dial" {
            *dial = 1;
        }
        out.push_str(&scrub_word(&w));
    };
    for c in line.chars() {
        if is_delimiter(c) {
            flush(&mut word, &mut out, &mut after_key, &mut dial, Some(c));
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out, &mut after_key, &mut dial, None);
    out
}

fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || "\"'()<>,;{}=|`".contains(c)
}

fn is_secret_key(key: &str) -> bool {
    [
        "token", "secret", "password", "passwd", "pwd", "auth", "uuid", "key", "sign",
    ]
    .iter()
    .any(|k| key.contains(k))
}

/// `text` with every non-empty `hidden` string replaced (the longest first, so
/// a link goes whole before its host does).
fn scrub_literals(text: &str, hidden: &[String]) -> String {
    let mut hidden: Vec<&str> = hidden
        .iter()
        .map(|h| h.trim())
        .filter(|h| h.len() >= 3)
        .collect();
    hidden.sort_by_key(|h| std::cmp::Reverse(h.len()));
    hidden.dedup();
    let mut out = text.to_owned();
    for h in hidden {
        out = out.replace(h, "<hidden>");
    }
    out
}

/// A word split from the punctuation that ends it ("1.2.3.4:443:" and ":").
fn split_tail(w: &str) -> (&str, &str) {
    let core = w.trim_end_matches([':', '.', '!', '?', '/']);
    (core, &w[core.len()..])
}

fn scrub_word(w: &str) -> String {
    let (core, tail) = split_tail(w);
    let swap = |s: String| format!("{s}{tail}");
    // An address, with or without a port.
    if let Ok(addr) = core.parse::<SocketAddr>() {
        return swap(mask_ip(addr.ip(), Some(addr.port())));
    }
    if let Ok(ip) = core.trim_matches(['[', ']']).parse::<IpAddr>() {
        return swap(mask_ip(ip, None));
    }
    // A link: its scheme and host, no path, no keys.
    if core.contains("://") {
        return swap(match url::Url::parse(core) {
            Ok(u) => match u.host() {
                Some(url::Host::Domain(d)) => format!("{}://{d}", u.scheme()),
                Some(url::Host::Ipv4(a)) => {
                    format!("{}://{}", u.scheme(), mask_ip(IpAddr::V4(a), None))
                }
                Some(url::Host::Ipv6(a)) => {
                    format!("{}://{}", u.scheme(), mask_ip(IpAddr::V6(a), None))
                }
                None => format!("{}://<link>", u.scheme()),
            },
            Err(_) => "<link>".into(),
        });
    }
    // host:port with an address in front.
    if let Some((host, port)) = core.rsplit_once(':')
        && port.chars().all(|c| c.is_ascii_digit())
        && !port.is_empty()
        && let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>()
    {
        return swap(mask_ip(ip, port.parse().ok()));
    }
    if looks_like_a_token(core) {
        return swap("<redacted>".into());
    }
    w.to_owned()
}

fn mask_ip(ip: IpAddr, port: Option<u16>) -> String {
    if ip.is_loopback() || ip.is_unspecified() {
        return match port {
            Some(p) => match ip {
                IpAddr::V4(_) => format!("{ip}:{p}"),
                IpAddr::V6(_) => format!("[{ip}]:{p}"),
            },
            None => ip.to_string(),
        };
    }
    match port {
        Some(p) => format!("<ip>:{p}"),
        None => "<ip>".into(),
    }
}

/// A long run of letters and digits (a key, a token, a UUID).
fn looks_like_a_token(w: &str) -> bool {
    w.len() >= 24
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '+' || c == '=')
        && w.chars().any(|c| c.is_ascii_digit())
        && w.chars().any(|c| c.is_ascii_alphabetic())
}

/// `path` with the user's home folder written as "~".
fn tilde(path: &str, user_home: Option<&str>) -> String {
    match user_home.filter(|h| h.len() > 1) {
        Some(h) if path == h => "~".into(),
        Some(h) => match path.strip_prefix(h) {
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            _ => path.to_owned(),
        },
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "fake-controller-secret-9d8c7b";
    const SUB: &str =
        "https://sub.provider.test/api/v1/client/subscribe?token=FAKETOKEN0123456789abcdef";

    fn entry(seq: u64, level: &str, payload: &str) -> LogEntry {
        LogEntry {
            seq,
            time: "14:36:12".into(),
            level: level.into(),
            payload: payload.into(),
        }
    }

    fn hidden() -> Vec<String> {
        vec![
            SECRET.into(),
            SUB.into(),
            "sub.provider.test".into(),
            "/home/fakeuser/.config/Kairo".into(),
            "/home/fakeuser".into(),
        ]
    }

    #[test]
    fn literals_are_hidden_everywhere() {
        let out = redact(
            &format!("controller said {SECRET} and fetched {SUB} for /home/fakeuser/x"),
            &hidden(),
        );
        assert!(!out.contains(SECRET), "{out}");
        assert!(!out.contains("FAKETOKEN"), "{out}");
        assert!(!out.contains("sub.provider.test"), "{out}");
        assert!(!out.contains("fakeuser"), "{out}");
    }

    #[test]
    fn a_link_shrinks_to_its_host_when_not_known() {
        let out = redact(
            "GET https://user:pw@dl.example.test/a/b?token=abc123 failed",
            &[],
        );
        assert!(out.contains("https://dl.example.test"), "{out}");
        assert!(
            !out.contains("abc123") && !out.contains("user:pw") && !out.contains("/a/b"),
            "{out}"
        );
    }

    #[test]
    fn addresses_are_masked_but_loopback_stays() {
        let out = redact(
            "[TCP] 127.0.0.1:50000 --> 203.0.113.9:443 and 2001:db8::7 and [2001:db8::8]:443 via ::1",
            &[],
        );
        assert!(
            !out.contains("203.0.113.9") && !out.contains("2001:db8"),
            "{out}"
        );
        assert!(
            out.contains("127.0.0.1:50000") && out.contains("<ip>:443") && out.contains("::1"),
            "{out}"
        );
    }

    #[test]
    fn the_address_a_dial_failed_on_is_a_servers() {
        let out = redact("dial tcp hk01.nodes.example.test:443: i/o timeout", &[]);
        assert!(!out.contains("hk01"), "{out}");
        assert!(
            out.contains("dial tcp <server>:") && out.contains("i/o timeout"),
            "{out}"
        );
        let out = redact(
            "proxy Tokyo 03 dial tcp 198.51.100.4:8443: connection refused",
            &[],
        );
        assert!(!out.contains("198.51.100.4"), "{out}");
    }

    #[test]
    fn keys_and_long_tokens_go() {
        let out = redact(
            "request token=abcdef0123 uuid=123e4567-e89b-12d3-a456-426614174000 and Zm9vYmFyYmF6cXV4MTIzNDU2Nzg5MA end",
            &[],
        );
        assert!(
            !out.contains("abcdef0123") && !out.contains("123e4567") && !out.contains("Zm9vYmFy"),
            "{out}"
        );
        assert!(out.contains("and") && out.contains("end"));
    }

    #[test]
    fn ordinary_lines_survive() {
        let line =
            "claude.ai:443 match RuleSet(ai) using AI[Tokyo 03] ServiceUnavailable at 14:35:58";
        assert_eq!(redact(line, &hidden()), line);
    }

    #[test]
    fn the_report_has_the_facts_and_none_of_the_private() {
        let lines = vec![
            entry(1, "info", &format!("subscription {SUB} updated")),
            entry(
                2,
                "error",
                &format!("API call with secret {SECRET} refused"),
            ),
            entry(3, "warning", "dial tcp 192.0.2.55:443: i/o timeout"),
            entry(4, "info", "claude.ai:443 --> AI exit"),
        ];
        let hidden = hidden();
        let text = diagnostics_text(&Report {
            kairo_version: "0.3.0",
            core: "mihomo 1.19.18 · running".into(),
            os: "macos aarch64".into(),
            ports: (7890, 9097),
            lines: &lines,
            hidden: &hidden,
        });
        for fact in [
            "Kairo 0.3.0",
            "mihomo 1.19.18 · running",
            "macos aarch64",
            "proxy 7890, controller 9097",
            "claude.ai:443 --> AI exit",
            "The last 4 log lines",
        ] {
            assert!(text.contains(fact), "{fact}\n{text}");
        }
        for private in [
            SECRET,
            "FAKETOKEN",
            "sub.provider.test",
            "192.0.2.55",
            "fakeuser",
        ] {
            assert!(!text.contains(private), "{private}\n{text}");
        }
    }

    #[test]
    fn the_report_keeps_only_the_newest_lines() {
        let lines: Vec<LogEntry> = (0..300)
            .map(|i| entry(i, "info", &format!("line number {i}")))
            .collect();
        let text = diagnostics_text(&Report {
            kairo_version: "0.3.0",
            core: String::new(),
            os: String::new(),
            ports: (1, 2),
            lines: &lines,
            hidden: &[],
        });
        assert!(text.contains("line number 299") && text.contains("line number 100"));
        assert!(!text.contains("line number 99\n"));
        assert!(text.contains(&format!("The last {DIAGNOSTIC_LINES} log lines")));
    }

    #[test]
    fn hidden_strings_cover_the_secret_the_links_and_the_homes() {
        use kairo_core::profile::{Profile, Source};
        let mut state = State::default();
        state.settings.secret = SECRET.into();
        let p = Profile {
            id: "p1".into(),
            name: "Fake".into(),
            source: Source::Remote {
                url: SUB.into(),
                interval_hours: 24,
            },
            updated_at: None,
            usage: None,
            web_page: Some("https://portal.provider.test/home".into()),
            last_error: None,
            failed_at: None,
            failure_dismissed: false,
            stats: Default::default(),
        };
        state.profiles.profiles.push(p);
        let h = hidden_strings(
            &state,
            Some("/home/fakeuser/.config/Kairo"),
            Some("/home/fakeuser"),
        );
        for want in [
            SECRET,
            SUB,
            "sub.provider.test",
            "https://portal.provider.test/home",
            "portal.provider.test",
            "/home/fakeuser",
        ] {
            assert!(h.iter().any(|x| x == want), "{want}: {h:?}");
        }
    }

    #[test]
    fn the_home_folder_is_written_as_a_tilde() {
        assert_eq!(
            tilde("/home/u/.config/Kairo", Some("/home/u")),
            "~/.config/Kairo"
        );
        assert_eq!(tilde("/home/u", Some("/home/u")), "~");
        assert_eq!(tilde("/homer/x", Some("/home")), "/homer/x");
        assert_eq!(tilde("/opt/x", None), "/opt/x");
    }
}
