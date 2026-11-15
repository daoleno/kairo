//! Activity: who is using the connection. By app first (each app, what it talks
//! to, which exit it leaves through, how much), then the raw list of open
//! connections with close. A tool that goes around Kairo is said at the top,
//! with the way to bring it back. Only counts are kept: app, site or service,
//! exit and bytes. No addresses beyond the host, no content.

use kairo_core::activity::{AppFlow, ExitKind};
use kairo_core::ai;
use kairo_core::kinds::{self, Kind};
use kairo_core::bytes;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::chrome::Glyph;
use crate::i18n::plural;
use crate::store::{Page, Snap, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, Variant, size};

/// One line of the by-app table, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct AppRow {
    pub name: String,
    /// "2 open", or "idle".
    pub open: String,
    /// What it talks to: the apps it used (their kind colours the chip), by
    /// traffic, then the sites no app claims.
    pub talks: Vec<(String, Option<Kind>)>,
    pub more: usize,
    /// The exits it leaves through, in words.
    pub goes: String,
    pub total: u64,
    /// Whether it stays on this computer (nothing to count).
    pub local: bool,
}

/// An app's name for a chip, and its kind.
fn app_title(id: &str) -> (String, Option<Kind>) {
    if let Some(c) = kinds::catalog_app(id) {
        return (kairo_core::ai::domestic_title(c.name), Some(c.kind));
    }
    let name = id
        .strip_prefix("site:")
        .or_else(|| id.strip_prefix("app:"))
        .or_else(|| id.strip_prefix("game:"))
        .unwrap_or(id);
    (name.to_owned(), Some(if id.starts_with("game:") { Kind::Games } else { Kind::Other }))
}

/// How many names show in "Talks to" before "+N".
const TALKS: usize = 4;

/// How Activity names an exit (`ExitKind` is data; this is its text). Also
/// outside the window (the table is made in tests): `tr!`.
pub fn exit_title(kind: ExitKind) -> String {
    kairo_core::tr!(match kind {
        ExitKind::Ai => "activity.exit.ai",
        ExitKind::Rest => "activity.exit.rest",
        ExitKind::Downloads => "activity.exit.downloads",
        ExitKind::Streaming => "activity.exit.streaming",
        ExitKind::Games => "activity.exit.games",
        ExitKind::DevTools => "activity.exit.dev_tools",
        ExitKind::Direct => "activity.exit.direct",
        ExitKind::Blocked => "activity.exit.blocked",
        ExitKind::Local => {
            if cfg!(target_os = "android") {
                "activity.exit.local_phone"
            } else {
                "activity.exit.local_computer"
            }
        }
    })
}

/// The by-app table from the tally.
pub fn app_rows(apps: &[(&str, &AppFlow)], query: &str) -> Vec<AppRow> {
    let q = query.trim().to_lowercase();
    apps.iter()
        .filter(|(name, a)| {
            q.is_empty()
                || name.to_lowercase().contains(&q)
                || a.sites.keys().any(|s| s.contains(&q))
                || a.services.keys().any(|s| s.contains(&q))
        })
        .map(|(name, a)| {
            let mut talks: Vec<(String, Option<Kind>)> = Vec::new();
            let mut apps: Vec<(&String, &u64)> = a.kapps.iter().collect();
            apps.sort_by(|x, y| y.1.cmp(x.1).then(x.0.cmp(y.0)));
            for (id, _) in &apps {
                talks.push(app_title(id));
            }
            // AI services Kairo knew of before the tally learned apps.
            let mut services: Vec<(&&'static str, &u64)> = a.services.iter().collect();
            services.sort_by(|x, y| y.1.cmp(x.1));
            for (id, _) in services {
                if a.kapps.contains_key(*id) {
                    continue;
                }
                let title = ai::SERVICES
                    .iter()
                    .find(|s| s.id == *id)
                    .map(|s| s.name.to_owned())
                    .unwrap_or_else(|| (*id).to_owned());
                talks.push((title, Some(Kind::Ai)));
            }
            let mut sites: Vec<(&String, &u64)> = a.sites.iter().collect();
            sites.sort_by(|x, y| y.1.cmp(x.1).then(x.0.cmp(y.0)));
            for (site, _) in sites {
                let claimed = kinds::app_of_host(site).is_some_and(|c| a.kapps.contains_key(c.id))
                    || a.kapps.contains_key(&format!("site:{site}"))
                    || ai::SERVICES.iter().any(|s| a.services.contains_key(s.id) && s.suffixes.iter().any(|x| site == x || site.ends_with(&format!(".{x}"))));
                if !claimed {
                    talks.push((site.clone(), None));
                }
            }
            let more = talks.len().saturating_sub(TALKS);
            talks.truncate(TALKS);
            // The exits of what is open now; with nothing open, of what it did.
            let mut exits: Vec<(ExitKind, u64)> = a
                .exits
                .iter()
                .filter(|(k, _)| a.open == 0 || a.open_exits.contains(*k))
                .map(|(k, v)| (*k, *v))
                .collect();
            exits.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
            let local = !exits.is_empty() && exits.iter().all(|(k, _)| *k == ExitKind::Local);
            let goes = exits
                .iter()
                .map(|(k, _)| exit_title(*k))
                .collect::<Vec<_>>()
                .join(" · ");
            AppRow {
                name: (*name).to_owned(),
                open: if a.open == 0 {
                    kairo_core::tr!("activity.idle")
                } else {
                    kairo_core::tr!("activity.open", count = a.open)
                },
                talks,
                more,
                goes,
                total: a.total(),
                local,
            }
        })
        .collect()
}

pub fn page() -> impl IntoElement {
    let store = store();
    let query = signal(String::new());
    let paused = signal(None::<Snap<kairo_core::model::Connections>>);
    let all = store.all_connections;
    let apps = computed(move || {
        let q = query.get();
        store.activity.with(|a| app_rows(&a.0.apps(), &q))
    });
    let total = move || store.connections.with(|c| c.0.list.len());
    view()
        .flex_1()
        .child(toolbar::toolbar(
            Page::Activity,
            move || {
                let n = apps.with(|a| a.len());
                let c = total();
                kairo_core::tr!(
                    "activity.subtitle",
                    apps = kairo_core::tr!(&plural("activity.apps", n as u64), count = n),
                    connections = kairo_core::tr!(&plural("activity.connections", c as u64), count = c)
                )
            },
            vec![
                toolbar::choice(
                    "activity.view",
                    kairo_core::tr!("activity.view.title"),
                    Glyph::Sort,
                    vec![kairo_core::tr!("activity.view.by_app"), kairo_core::tr!("activity.view.all")],
                    move || usize::from(all.get()),
                    move |i| all.set(i == 1),
                ),
                toolbar::search("activity.search", kairo_core::tr!("activity.search"), query),
                toolbar::button_dyn(
                    "activity.pause",
                    move || {
                        if paused.with(Option::is_some) {
                            kairo_core::tr!("activity.resume")
                        } else {
                            kairo_core::tr!("activity.pause")
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
                            paused.set(Some(store.connections.get_untracked()));
                        }
                    },
                ),
                toolbar::button(
                    "activity.close-all",
                    kairo_core::tr!("activity.close_all"),
                    Glyph::CloseAll,
                    Icon::Close,
                    move || {
                        let svc = store.svc();
                        store.run_then(svc.close_all_connections(), move |_| paused.set(None));
                    },
                )
                .enabled(move || total() > 0),
            ],
        ))
        .child(bypass_banner())
        .child(dynamic(move || {
            if all.get() {
                untracked(move || crate::pages::connections::list(query, paused).into_element())
            } else {
                untracked(move || by_app(apps, query).into_element())
            }
        }))
}

// --- A tool that goes around Kairo ---------------------------------------------------

/// The tools the socket table shows leaving directly, de-duplicated.
pub fn bypassing_tools(list: &[kairo_core::bypass::Bypass]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in list {
        if !out.contains(&b.tool) {
            out.push(b.tool.clone());
        }
    }
    out
}

fn bypass_banner() -> impl IntoElement {
    let store = store();
    // Read from the store (it outlives every page), not from a memo of this page:
    // the banner comes and goes with the list, and a binding that runs while the
    // page is being left must not read a handle the page already disposed.
    let tools = move || store.state.with(|s| bypassing_tools(&s.bypass));
    when(
        move || !tools().is_empty(),
        move || {
            let headline = move || -> SharedString {
                let t = tools();
                match t.len() {
                    0 => "".into(),
                    1 => t!("activity.bypass.one", tool = t[0]).into(),
                    2 => t!("activity.bypass.two", first = t[0], second = t[1]).into(),
                    n => t!("activity.bypass.more", first = t[0], second = t[1], count = n - 2).into(),
                }
            };
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .mx_6()
                .mb_2()
                .px_4()
                .py_3()
                .rounded(px(18.0))
                .bg(move || ui::warning().opacity(0.14))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap_0p5()
                        .child(ui::body(headline).font_weight(FontWeight::SEMIBOLD)),
                )
                .child(when(
                    move || {
                        store.state.with(|s| {
                            s.tun_privilege != Some(kairo_core::tun::Privilege::Unsupported)
                        })
                    },
                    move || {
                        ui::capsule(t!("activity.bypass.use_tun"), true)
                            .h(px(30.0))
                            .px_4()
                            .on_press(move || {
                                crate::pages::home::set_via(crate::pages::home::Via::Tun)
                            })
                    },
                ))
                .child(
                    ui::capsule(t!("activity.bypass.copy_terminal"), false)
                        .h(px(30.0))
                        .px_4()
                        .on_press(move || ui::copy_terminal_env(store)),
                )
        },
    )
}

// --- By app ----------------------------------------------------------------------------

const COLS: [f32; 5] = [0.24, 0.34, 0.20, 0.10, 0.12];

fn cell(i: usize) -> View {
    view()
        .w(percentage(COLS[i]))
        .flex_shrink_0()
        .min_w(px(0.0))
        .flex_row()
        .items_center()
        .gap_2()
        .pr_2()
}

fn by_app(rows: Memo<Vec<AppRow>>, query: Signal<String>) -> impl IntoElement {
    let store = store();
    let phone = ui::phone();
    scroll().flex_1().child(
        view()
            .px(px(if phone { 16.0 } else { 24.0 }))
            .pt(px(if phone { 12.0 } else { 0.0 }))
            .pb_6()
            .child(view().pb_3().child(crate::pages::stats::overview()))
            .child(ui::section_label(t!("activity.by_app")).px_1().pb_1())
            .child(when(move || phone, move || phone_list(rows, query)))
            .child(when(move || !phone, move || {
                ui::card_soft()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .px_5()
                            .h(px(30.0))
                            .child(head(0, t!("activity.column.app")))
                            .child(head(1, t!("activity.column.talks_to")))
                            .child(head(2, t!("activity.column.goes_through")))
                            .child(head(3, t!("activity.column.total")))
                            .child(cell(4)),
                    )
                    .child(dynamic(move || {
                        let list = rows.get();
                        untracked(move || {
                            if list.is_empty() {
                                return ui::empty_state(
                                    Icon::Connections,
                                                                        move || -> SharedString {
                                        if !store.state.with(|s| s.core.is_running()) {
                                            t!("activity.empty.not_connected").into()
                                        } else if query.with(|q| !q.is_empty()) {
                                            t!("activity.empty.no_match").into()
                                        } else {
                                            t!("activity.empty.nothing_yet").into()
                                        }
                                    },
                                    None,
                                )
                                .into_element();
                            }
                            view()
                                .children(list.into_iter().map(app_row))
                                .into_element()
                        })
                    }))
            }))
            ,
    )
}

/// The by-app list on a phone: one card per app, not a table. The app and its
/// menu on top, what it talks to as chips that wrap, then where it goes and
/// how much, each on a line that can wrap too.
fn phone_list(rows: Memo<Vec<AppRow>>, query: Signal<String>) -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let list = rows.get();
        untracked(move || {
            if list.is_empty() {
                return ui::card_soft()
                    .child(ui::empty_state(
                        Icon::Connections,
                                                move || -> SharedString {
                            if !store.state.with(|s| s.core.is_running()) {
                                t!("activity.empty.not_connected").into()
                            } else if query.with(|q| !q.is_empty()) {
                                t!("activity.empty.no_match").into()
                            } else {
                                t!("activity.empty.nothing_yet").into()
                            }
                        },
                        None,
                    ))
                    .into_element();
            }
            view()
                .gap_2()
                .children(list.into_iter().map(phone_app))
                .into_element()
        })
    })
}

fn phone_app(r: AppRow) -> impl IntoElement {
    let menu_name = stored(r.name.clone());
    ui::card_soft()
        .px_4()
        .py_3()
        .gap_2()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .child(ui::logo::logo(ui::logo::Subject::App(r.name.clone()), 28.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(ui::body(ui::logo::app_name(r.name.clone())).font_weight(FontWeight::MEDIUM))
                        .child(ui::muted(r.open.clone()).text_size(px(size::small()))),
                )
                .child(
                    ui::icon_btn(Icon::More, t!("activity.app_menu"))
                        .mr(px(-8.0))
                        .on_press(move || row_menu(menu_name.with(|n| n.clone()))),
                ),
        )
        .child((!r.talks.is_empty()).then(|| {
            view().flex_row().flex_wrap().gap_1p5().children(
                r.talks
                    .iter()
                    .map(|(t, k)| pill(t.clone(), *k).into_element())
                    .chain((r.more > 0).then(|| ui::muted(format!("+{}", r.more)).into_element())),
            )
        }))
        .child(
            view()
                .flex_row()
                .items_baseline()
                .gap_3()
                .child(
                    ui::muted(if r.goes.is_empty() {
                        "—".to_owned()
                    } else {
                        r.goes.clone()
                    })
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(size::small())),
                )
                .child(
                    ui::body(if r.local || r.total == 0 {
                        "—".to_owned()
                    } else {
                        bytes(r.total)
                    })
                    .tabular_nums(),
                ),
        )
}

/// The choices for one program: an exit of its own, an app of a kind, or
/// its connections (the menu on a row).
fn row_menu(n: String) {
    let store = store();
    let show = {
        let n = n.clone();
        move || {
            store.all_connections.set(true);
            // The search field is the page's: it follows the chrome's own.
            crate::chrome::tool_event(crate::chrome::ToolEvent::Search("activity.search", n.clone()));
        }
    };
    crate::pages::kinds::program_menu(n, show);
}

fn head(i: usize, t: String) -> impl IntoElement {
    cell(i).child(
        ui::muted(t)
            .text_size(px(size::small()))
            .font_weight(FontWeight::MEDIUM),
    )
}

/// A chip of "Talks to", in its kind's colour (a site no app claims is grey).
fn pill(text_: String, kind: Option<Kind>) -> impl IntoElement {
    let tone = move || match kind {
        Some(Kind::Ai) => Some(theme().primary),
        Some(Kind::Streaming) => Some(ui::warning()),
        Some(Kind::Games) => Some(ui::success()),
        Some(Kind::Downloads) => Some(ui::chart_downloads()),
        Some(Kind::DevTools) => Some(ui::chart_rest()),
        Some(Kind::Other) | None => None,
    };
    view()
        .px_2()
        .h(px(20.0))
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(move || match tone() {
            Some(c) => c.opacity(0.14),
            None => theme().foreground.opacity(0.07),
        })
        .child(
            text(text_)
                .text_size(px(size::small()))
                .text_color(move || tone().unwrap_or(theme().muted_foreground))
                .number_of_lines(1),
        )
}

fn app_row(r: AppRow) -> impl IntoElement {
    let name = r.name.clone();
    let menu_name = stored(r.name.clone());
    view()
        .flex_row()
        .items_center()
        .px_5()
        .py_2()
        .min_h(px(50.0))
        .border_t_1()
        .border_color(move || theme().border.opacity(0.7))
        .child(
            cell(0).child(ui::logo::logo(ui::logo::Subject::App(name.clone()), 28.0)).child(
                view()
                    .min_w(px(0.0))
                    .child(ui::body(ui::logo::app_name(r.name.clone())).number_of_lines(1))
                    .child(ui::muted(r.open.clone()).text_size(px(size::small()))),
            ),
        )
        .child(
            cell(1).flex_wrap().gap_1p5().children(
                r.talks
                    .iter()
                    .map(|(t, k)| pill(t.clone(), *k).into_element())
                    .chain((r.more > 0).then(|| ui::muted(format!("+{}", r.more)).into_element())),
            ),
        )
        .child(cell(2).child(ui::muted(r.goes.clone()).text_size(px(size::body()))))
        .child(
            cell(3).child(
                ui::body(if r.local || r.total == 0 {
                    "—".to_owned()
                } else {
                    bytes(r.total)
                })
                .tabular_nums(),
            ),
        )
        .child(
            cell(4).justify_end().child(
                ui::btn("⋯", Variant::Secondary)
                    .h(px(26.0))
                    .rounded_full()
                    .on_press(move || {
                        row_menu(menu_name.with(|n| n.clone()));
                    }),
            ),
        )
}

/// The first letter of an app in a soft square.

#[cfg(test)]
mod tests {
    use super::*;
    use kairo_core::activity::{ExitNames, Tally, parse_time};
    use kairo_core::model::Connection;

    fn conn(id: &str, host: &str, process: &str, chain: &[&str], down: u64) -> Connection {
        Connection {
            id: id.into(),
            host: host.into(),
            port: "443".into(),
            network: "tcp".into(),
            inbound: String::new(),
            process: process.into(),
            process_path: format!("/Applications/{process}.app/x"),
            source: String::new(),
            destination_ip: String::new(),
            rule: String::new(),
            rule_payload: String::new(),
            chains: chain.iter().map(|s| (*s).to_owned()).collect(),
            upload: 0,
            download: down,
            up_speed: 0,
            down_speed: 0,
            start: "2026-11-09T15:00:00Z".into(),
        }
    }

    fn tally() -> Tally {
        let mut t = Tally::default();
        t.names = ExitNames {
            ai: "Kairo AI".into(),
            downloads: "Kairo Downloads".into(),
            ..Default::default()
        };
        let now = parse_time("2026-11-09T15:00:00Z").unwrap() + 5;
        t.ingest(
            &[
                conn("1", "chatgpt.com", "Safari", &["Tokyo 03", "Kairo AI"], 900),
                conn(
                    "2",
                    "www.youtube.com",
                    "Safari",
                    &["Hong Kong 01", "Proxy"],
                    4000,
                ),
                conn(
                    "3",
                    "github.com",
                    "git",
                    &["Hong Kong 02", "Kairo Downloads"],
                    700,
                ),
            ],
            now,
        );
        t
    }

    #[test]
    fn an_app_shows_the_apps_it_used_by_traffic_and_its_exits() {
        let t = tally();
        let rows = app_rows(&t.apps(), "");
        assert_eq!(rows.len(), 2);
        let safari = rows.iter().find(|r| r.name == "Safari").unwrap();
        assert_eq!(
            safari.talks,
            vec![
                ("YouTube".to_owned(), Some(Kind::Streaming)),
                ("ChatGPT".to_owned(), Some(Kind::Ai))
            ]
        );
        assert_eq!(safari.goes, "Everything else · AI exit");
        assert_eq!(safari.total, 4900);
        assert_eq!(safari.open, "2 open");
        let git = rows
            .iter()
            .find(|r| r.name == "Git")
            .unwrap_or_else(|| rows.iter().find(|r| r.name != "Safari").unwrap());
        assert_eq!(git.goes, "Downloads");
    }

    #[test]
    fn the_filter_matches_apps_sites_and_services() {
        let t = tally();
        assert_eq!(app_rows(&t.apps(), "safari").len(), 1);
        assert_eq!(app_rows(&t.apps(), "github").len(), 1);
        assert_eq!(app_rows(&t.apps(), "chatgpt").len(), 1);
        assert_eq!(app_rows(&t.apps(), "nothing").len(), 0);
    }

    #[test]
    fn long_lists_end_in_a_count() {
        let mut t = Tally::default();
        let now = parse_time("2026-11-09T15:00:00Z").unwrap() + 5;
        let conns: Vec<Connection> = (0..7)
            .map(|i| {
                conn(
                    &i.to_string(),
                    &format!("site{i}.com"),
                    "Safari",
                    &["n", "Proxy"],
                    10 + i,
                )
            })
            .collect();
        t.ingest(&conns, now);
        let rows = app_rows(&t.apps(), "");
        assert_eq!(rows[0].talks.len(), TALKS);
        assert_eq!(rows[0].more, 3);
    }

    #[test]
    fn bypassing_tools_are_named_once() {
        let b = |tool: &str, remote: &str| kairo_core::bypass::Bypass {
            tool: tool.into(),
            service: None,
            game: None,
            pid: 1,
            remote: remote.into(),
            since: 0,
        };
        assert_eq!(
            bypassing_tools(&[
                b("Cursor", "1.1.1.1:443"),
                b("Cursor", "2.2.2.2:443"),
                b("claude", "3.3.3.3:443")
            ]),
            vec!["Cursor".to_owned(), "claude".to_owned()]
        );
    }
}
