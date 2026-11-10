//! Activity: who is using the connection. By app first (each app, what it talks
//! to, which exit it leaves through, how much), then the raw list of open
//! connections with close. A tool that goes around Kairo is said at the top,
//! with the way to bring it back. Only counts are kept: app, site or service,
//! exit and bytes. No addresses beyond the host, no content.

use kairo_core::activity::{AppFlow, ExitKind};
use kairo_core::ai;
use kairo_core::bytes;
use kairo_core::routes::{RouteExit, RouteKind};
use nsh_base::theme;
use crate::ui::menu::{self, Choice};
use nsh_core::prelude::*;

use crate::chrome::Glyph;
use crate::store::{Page, Snap, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, Variant, size};

/// One line of the by-app table, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct AppRow {
    pub name: String,
    /// "2 open", or "idle".
    pub open: String,
    /// What it talks to: AI services first (marked), then sites by traffic.
    pub talks: Vec<(String, bool)>,
    pub more: usize,
    /// The exits it leaves through, in words.
    pub goes: String,
    pub total: u64,
    /// Whether it stays on this computer (nothing to count).
    pub local: bool,
}

/// How many names show in "Talks to" before "+N".
const TALKS: usize = 4;

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
            let mut talks: Vec<(String, bool)> = Vec::new();
            let mut services: Vec<(&&'static str, &u64)> = a.services.iter().collect();
            services.sort_by(|x, y| y.1.cmp(x.1));
            for (id, _) in services {
                let title = ai::SERVICES
                    .iter()
                    .find(|s| s.id == *id)
                    .map(|s| s.name.to_owned())
                    .unwrap_or_else(|| (*id).to_owned());
                talks.push((title, true));
            }
            let mut sites: Vec<(&String, &u64)> = a.sites.iter().collect();
            sites.sort_by(|x, y| y.1.cmp(x.1).then(x.0.cmp(y.0)));
            for (site, _) in sites {
                talks.push((site.clone(), false));
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
                .map(|(k, _)| k.title())
                .collect::<Vec<_>>()
                .join(" · ");
            AppRow {
                name: (*name).to_owned(),
                open: if a.open == 0 {
                    "idle".into()
                } else {
                    format!("{} open", a.open)
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
                format!(
                    "{n} {} · {} connections",
                    if n == 1 { "app" } else { "apps" },
                    total()
                )
            },
            vec![
                toolbar::choice(
                    "activity.view",
                    "View",
                    Glyph::Sort,
                    vec!["By App", "All Connections"],
                    move || usize::from(all.get()),
                    move |i| all.set(i == 1),
                ),
                toolbar::search("activity.search", "Search host or app", query),
                toolbar::button_dyn(
                    "activity.pause",
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
                            paused.set(Some(store.connections.get_untracked()));
                        }
                    },
                ),
                toolbar::button(
                    "activity.close-all",
                    "Close All",
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
                    1 => format!("{} goes around Kairo", t[0]).into(),
                    2 => format!("{} and {} go around Kairo", t[0], t[1]).into(),
                    n => format!("{}, {} and {} more go around Kairo", t[0], t[1], n - 2).into(),
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
                        .child(ui::body(headline).font_weight(FontWeight::SEMIBOLD))
                        .child(ui::muted(
                            if cfg!(windows) {
                                "It does not follow the system's proxy settings, so its traffic leaves directly. A proxy line in the terminal it starts from takes it in."
                            } else {
                                "It does not follow the system's proxy settings, so its traffic leaves directly. Every app takes it in; so does a proxy line in a terminal."
                            },
                        )),
                )
                .child(when(
                    move || {
                        store.state.with(|s| {
                            s.tun_privilege != Some(kairo_core::tun::Privilege::Unsupported)
                        })
                    },
                    move || {
                        ui::capsule("Use Every App", true)
                            .h(px(30.0))
                            .px_4()
                            .on_press(move || {
                                crate::pages::home::set_via(crate::pages::home::Via::Tun)
                            })
                    },
                ))
                .child(
                    ui::capsule("Copy for terminal", false)
                        .h(px(30.0))
                        .px_4()
                        .on_press(move || {
                            set_clipboard(store.svc().terminal_env());
                            store.info("Copied. Paste it in the terminal before you start the tool.");
                        }),
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
            .child(ui::section_label("By app").px_1().pb_1())
            .child(when(move || phone, move || phone_list(rows, query)))
            .child(when(move || !phone, move || {
                ui::card_soft()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .px_5()
                            .h(px(30.0))
                            .child(head(0, "App"))
                            .child(head(1, "Talks to"))
                            .child(head(2, "Goes through"))
                            .child(head(3, "Total"))
                            .child(cell(4)),
                    )
                    .child(dynamic(move || {
                        let list = rows.get();
                        untracked(move || {
                            if list.is_empty() {
                                return ui::empty_state(
                                    Icon::Connections,
                                    "Nothing yet",
                                    move || -> SharedString {
                                        if !store.state.with(|s| s.core.is_running()) {
                                            "Activity appears while Kairo is connected.".into()
                                        } else if query.with(|q| !q.is_empty()) {
                                            "Nothing matches the filter.".into()
                                        } else {
                                            "Apps that use Kairo show up here.".into()
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
            .child(
                ui::muted(if phone {
                    "Total counts since Kairo started. ⋯ on an app: Always Direct · Always AI Exit · Block · Show Connections"
                } else {
                    "Total counts since Kairo started. ⋯ on a row: Always Direct · Always AI Exit · Block · Show Connections"
                })
                    .px_1()
                    .mt_2()
                    .text_size(px(size::small())),
            ),
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
                        "Nothing yet",
                        move || -> SharedString {
                            if !store.state.with(|s| s.core.is_running()) {
                                "Activity appears while Kairo is connected.".into()
                            } else if query.with(|q| !q.is_empty()) {
                                "Nothing matches the filter.".into()
                            } else {
                                "Apps that use Kairo show up here.".into()
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
                .child(letter(&r.name))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(ui::body(r.name.clone()).font_weight(FontWeight::MEDIUM))
                        .child(ui::muted(r.open.clone()).text_size(px(size::small()))),
                )
                .child(
                    ui::icon_btn(Icon::More, "Choices for this app")
                        .mr(px(-8.0))
                        .on_press(move || row_menu(menu_name.with(|n| n.clone()))),
                ),
        )
        .child((!r.talks.is_empty()).then(|| {
            view().flex_row().flex_wrap().gap_1p5().children(
                r.talks
                    .iter()
                    .map(|(t, ai)| pill(t.clone(), *ai).into_element())
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

/// The choices for one app: always direct, always the AI exit, block, or show
/// its connections (the menu on a row).
fn row_menu(n: String) {
    let store = store();
    let app = |exit: RouteExit, n: String| {
        move || {
            store.run_then(
                store
                    .svc()
                    .set_route(RouteKind::App { name: n.clone() }, exit.clone()),
                move |r| {
                    if r.is_ok() {
                        store.info("Saved in Apps & Sites");
                    }
                },
            )
        }
    };
    let show = {
        let n = n.clone();
        move || {
            store.all_connections.set(true);
            // The search field is the page's: it follows the chrome's own.
            crate::chrome::tool_event(crate::chrome::ToolEvent::Search(
                "activity.search",
                n.clone(),
            ));
        }
    };
    let items = vec![
        Choice::new("Always Direct").on_select(app(RouteExit::Direct, n.clone())),
        Choice::new("Always AI Exit").on_select(app(RouteExit::Ai, n.clone())),
        Choice::new("Block").on_select(app(RouteExit::Block, n.clone())),
        Choice::separator(),
        Choice::new("Show Connections").on_select(show),
    ];
    menu::open(items);
}

fn head(i: usize, t: &'static str) -> impl IntoElement {
    cell(i).child(
        ui::muted(t)
            .text_size(px(size::small()))
            .font_weight(FontWeight::MEDIUM),
    )
}

fn pill(text_: String, ai: bool) -> impl IntoElement {
    view()
        .px_2()
        .h(px(20.0))
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(move || {
            if ai {
                theme().primary.opacity(0.14)
            } else {
                theme().foreground.opacity(0.07)
            }
        })
        .child(
            text(text_)
                .text_size(px(size::small()))
                .text_color(move || {
                    if ai {
                        theme().primary
                    } else {
                        theme().muted_foreground
                    }
                })
                .number_of_lines(1),
        )
}

fn app_row(r: AppRow) -> impl IntoElement {
    let store = store();
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
            cell(0).child(letter(&name)).child(
                view()
                    .min_w(px(0.0))
                    .child(ui::body(r.name.clone()).number_of_lines(1))
                    .child(ui::muted(r.open.clone()).text_size(px(size::small()))),
            ),
        )
        .child(
            cell(1).flex_wrap().gap_1p5().children(
                r.talks
                    .iter()
                    .map(|(t, ai)| pill(t.clone(), *ai).into_element())
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
                        let n = menu_name.with(|n| n.clone());
                        let app = |exit: RouteExit, n: String| {
                            move || {
                                store.run_then(
                                    store.svc().set_route(
                                        RouteKind::App { name: n.clone() },
                                        exit.clone(),
                                    ),
                                    move |r| {
                                        if r.is_ok() {
                                            store.info("Saved in Apps & Sites");
                                        }
                                    },
                                )
                            }
                        };
                        let show = {
                            let n = n.clone();
                            move || {
                                store.all_connections.set(true);
                                // The search field is the page's: it follows the chrome's own.
                                crate::chrome::tool_event(crate::chrome::ToolEvent::Search(
                                    "activity.search",
                                    n.clone(),
                                ));
                            }
                        };
                        let items = vec![
                            Choice::new("Always Direct")
                                .on_select(app(RouteExit::Direct, n.clone())),
                            Choice::new("Always AI Exit")
                                .on_select(app(RouteExit::Ai, n.clone())),
                            Choice::new("Block").on_select(app(RouteExit::Block, n.clone())),
                            Choice::separator(),
                            Choice::new("Show Connections").on_select(show),
                        ];
                        menu::open(items);
                    }),
            ),
        )
}

/// The first letter of an app in a soft square.
fn letter(name: &str) -> impl IntoElement {
    let l: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "·".into());
    view()
        .size(px(28.0))
        .flex_shrink_0()
        .rounded(px(8.0))
        .items_center()
        .justify_center()
        .bg(ui::subtle)
        .child(
            text(l)
                .text_size(px(size::small()))
                .font_weight(FontWeight::SEMIBOLD),
        )
}

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
    fn an_app_shows_its_services_first_then_sites_and_its_exits() {
        let t = tally();
        let rows = app_rows(&t.apps(), "");
        assert_eq!(rows.len(), 2);
        let safari = rows.iter().find(|r| r.name == "Safari").unwrap();
        assert_eq!(
            safari.talks,
            vec![
                ("ChatGPT".to_owned(), true),
                ("youtube.com".to_owned(), false)
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
