//! Groups (Settings › Advanced › Inspect): the subscription's own groups (and
//! providers) on the left; the selected one's nodes on the right, picked with a
//! click, tested one by one or all at once. Delays show here, in the expanded
//! list, and nowhere on the main pages.

use kairo_core::model::{Group, Node};
use nsh_base::{button, theme};
use nsh_core::prelude::*;

use crate::chrome::Glyph;
use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, Variant, icon, size};

#[derive(Debug, Clone, PartialEq)]
enum Selected {
    Group(String),
    Provider(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Config,
    Delay,
    Name,
}

pub fn page() -> impl IntoElement {
    let store = store();
    let selected = signal(None::<Selected>);
    let query = signal(String::new());
    let sort = signal(Sort::Config);

    // The first group until the user picks one.
    let current = computed(move || {
        let groups_empty = store.proxies.with(|p| p.0.groups.is_empty());
        match selected.get() {
            Some(s) => Some(s),
            None if groups_empty => None,
            None => store.proxies.with(|p| {
                p.0.groups
                    .iter()
                    .find(|g| g.name != "GLOBAL" && !g.hidden)
                    .or(p.0.groups.first())
                    .map(|g| Selected::Group(g.name.clone()))
            }),
        }
    });

    let running = move || store.state.with(|s| s.core.is_running());
    view()
        .flex_1()
        .child(ui::toolbar::toolbar(
            Page::Proxies,
            move || match current.get() {
                Some(Selected::Group(name) | Selected::Provider(name)) => name,
                None => String::new(),
            },
            vec![
                crate::pages::settings::back(),
                toolbar::search("proxies.filter", "Filter nodes", query),
                toolbar::choice(
                    "proxies.sort",
                    "Sort",
                    Glyph::Sort,
                    vec!["Config Order", "Delay", "Name"],
                    move || match sort.get() {
                        Sort::Config => 0,
                        Sort::Delay => 1,
                        Sort::Name => 2,
                    },
                    move |i| sort.set([Sort::Config, Sort::Delay, Sort::Name][i.min(2)]),
                ),
                toolbar::button(
                    "proxies.refresh",
                    "Refresh",
                    Glyph::Refresh,
                    Icon::Refresh,
                    move || store.run(store.svc().refresh_proxies()),
                ),
            ],
        ))
        .child(
            when(running, move || {
                view()
                    .flex_1()
                    .flex_row()
                    .min_h(px(0.0))
                    .child(group_list(current, selected))
                    .child(view().flex_1().min_w(px(0.0)).child(dynamic(move || {
                        let cur = current.get();
                        untracked(move || match cur {
                            Some(Selected::Group(name)) => {
                                group_detail(name, query, sort).into_element()
                            }
                            Some(Selected::Provider(name)) => {
                                provider_detail(name, query).into_element()
                            }
                            None => ui::empty_state(
                                Icon::Proxies,
                                "No groups",
                                "This profile has no proxy groups.",
                                None,
                            )
                            .into_element(),
                        })
                    })))
            })
            .otherwise(move || {
                ui::empty_state(
                    Icon::Proxies,
                    "The core is not running",
                    "Groups and nodes appear here once the core starts.",
                    Some(
                        ui::btn_icon(Icon::Play, "Start Core", Variant::Primary)
                            .on_press(move || store.run(store.svc().start_core()))
                            .into_element(),
                    ),
                )
            }),
        )
}

fn list_row(
    label: SharedString,
    detail: impl IntoReactive<SharedString>,
    is: impl Fn() -> bool + Copy + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    button()
        .selected(is)
        .on_press(on_press)
        .accessibility_label(label.clone())
        .gap_0p5()
        .px_2p5()
        .py_1p5()
        .rounded(ui::radius_control())
        .cursor_pointer()
        .hover(|s| s.bg(move || theme().accent))
        .style_when(is, |s| s.bg(move || ui::selection()))
        .child(
            text(label)
                .text_size(px(size::body()))
                .font_weight(FontWeight::MEDIUM)
                .number_of_lines(1),
        )
        .child(
            text(detail)
                .text_size(px(size::small()))
                .text_color(move || theme().muted_foreground)
                .number_of_lines(1),
        )
}

fn group_list(
    current: Memo<Option<Selected>>,
    selected: Signal<Option<Selected>>,
) -> impl IntoElement {
    let store = store();
    let groups = computed(move || store.proxies.with(|p| p.0.groups.clone()));
    let providers = computed(move || store.proxies.with(|p| p.0.providers.clone()));
    scroll()
        .w(px(240.0))
        .flex_shrink_0()
        .border_r_1()
        .border_color(move || theme().border)
        .child(
            view()
                .p_2()
                .gap_0p5()
                .child(ui::section_label("Groups").px_2p5().pt_1().pb_1())
                .child(each(
                    move || {
                        groups
                            .get()
                            .into_iter()
                            .filter(|g| !g.hidden)
                            .collect::<Vec<Group>>()
                    },
                    |g: &Group| g.name.clone(),
                    move |g, _| {
                        let g = g.get_untracked();
                        let name = g.name.clone();
                        let key = Selected::Group(name.clone());
                        let is = {
                            let key = key.clone();
                            let m = computed(move || current.get().as_ref() == Some(&key));
                            move || m.get()
                        };
                        let detail = {
                            let name = name.clone();
                            move || -> SharedString {
                                store.proxies.with(|p| {
                                    p.0.groups
                                        .iter()
                                        .find(|x| x.name == name)
                                        .map(|x| match &x.now {
                                            Some(now) => format!("{} · {now}", x.kind_title()),
                                            None => x.kind_title().to_owned(),
                                        })
                                        .unwrap_or_default()
                                        .into()
                                })
                            }
                        };
                        list_row(name.clone().into(), detail, is, move || {
                            selected.set(Some(key.clone()))
                        })
                    },
                ))
                .child(when(
                    move || !providers.with(Vec::is_empty),
                    move || {
                        view()
                            .gap_0p5()
                            .child(ui::section_label("Providers").px_2p5().pt_4().pb_1())
                            .child(each(
                                move || providers.get(),
                                |p: &kairo_core::model::ProxyProvider| p.name.clone(),
                                move |p, _| {
                                    let p = p.get_untracked();
                                    let key = Selected::Provider(p.name.clone());
                                    let is = {
                                        let key = key.clone();
                                        let m =
                                            computed(move || current.get().as_ref() == Some(&key));
                                        move || m.get()
                                    };
                                    list_row(
                                        p.name.clone().into(),
                                        SharedString::from(format!(
                                            "{} · {}/{} alive",
                                            p.vehicle, p.alive, p.count
                                        )),
                                        is,
                                        move || selected.set(Some(key.clone())),
                                    )
                                },
                            ))
                    },
                )),
        )
}

fn sorted(members: &[Node], query: &str, sort: Sort) -> Vec<Node> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<Node> = members
        .iter()
        .filter(|m| q.is_empty() || m.name.to_lowercase().contains(&q))
        .cloned()
        .collect();
    match sort {
        Sort::Config => {}
        Sort::Name => out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        Sort::Delay => out.sort_by_key(|m| match m.delay {
            Some(0) | None => u32::MAX,
            Some(d) => d,
        }),
    }
    out
}

fn group_detail(name: String, query: Signal<String>, sort: Signal<Sort>) -> impl IntoElement {
    let store = store();
    let gname = name.clone();
    let group = computed(move || {
        store
            .proxies
            .with(|p| p.0.groups.iter().find(|g| g.name == gname).cloned())
    });
    let nodes = computed(move || {
        group.with(|g| {
            g.as_ref()
                .map(|g| sorted(&g.members, &query.get(), sort.get()))
                .unwrap_or_default()
        })
    });
    let tname = name.clone();
    let testing_group = computed(move || store.proxies.with(|p| p.0.testing.contains(&tname)));
    let testing_group = move || testing_group.get();
    let header_name = name.clone();
    let test_name = name.clone();
    view()
        .flex_1()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .px_5()
                .py_3()
                .border_b_1()
                .border_color(move || theme().border)
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap_0p5()
                        .child(
                            text(header_name)
                                .text_size(px(size::large()))
                                .font_weight(FontWeight::SEMIBOLD)
                                .number_of_lines(1),
                        )
                        .child(ui::muted(move || -> SharedString {
                            group.with(|g| match g {
                                Some(g) => {
                                    let how = if g.selectable() {
                                        "pick a node"
                                    } else {
                                        "picks automatically"
                                    };
                                    format!(
                                        "{} · {} members · {how}",
                                        g.kind_title(),
                                        g.members.len()
                                    )
                                    .into()
                                }
                                None => "".into(),
                            })
                        })),
                )
                .child(
                    ui::btn_icon(
                        Icon::Bolt,
                        move || -> SharedString {
                            if testing_group() {
                                "Testing…".into()
                            } else {
                                "Test All".into()
                            }
                        },
                        Variant::Secondary,
                    )
                    .disabled(testing_group)
                    .on_press(move || store.run(store.svc().test_group(test_name.clone()))),
                ),
        )
        .child(node_list(name, group, nodes))
}

fn node_list(
    group_name: String,
    group: Memo<Option<Group>>,
    nodes: Memo<Vec<Node>>,
) -> impl IntoElement {
    let store = store();
    let selectable = move || group.with(|g| g.as_ref().is_some_and(|g| g.selectable()));
    let now = move || group.with(|g| g.as_ref().and_then(|g| g.now.clone()));
    virtual_list(
        move || nodes.with(Vec::len),
        move |at| {
            let node = computed(move || nodes.with(|n| n.get(at.index()).cloned()));
            let name =
                move || node.with(|n| n.as_ref().map(|n| n.name.clone()).unwrap_or_default());
            let is_now = move || now().is_some_and(|n| n == name());
            let delay = move || node.with(|n| n.as_ref().and_then(|n| n.delay));
            let testing = move || store.proxies.with(|p| p.0.testing.contains(&name()));
            let gname = group_name.clone();
            button()
                .on_press(move || {
                    if selectable() && !is_now() {
                        store.run(store.svc().select(gname.clone(), name()));
                    }
                })
                .accessibility_label(move || -> SharedString { name().into() })
                .selected(is_now)
                .flex_row()
                .items_center()
                .gap_3()
                .h(px(36.0))
                .px_5()
                .cursor_pointer()
                .hover(|s| s.bg(move || theme().accent))
                .child(
                    view().w(px(16.0)).items_center().child(
                        icon(Icon::Check, 14.0)
                            .text_color(move || theme().primary)
                            .style_when(move || !is_now(), |s| s.opacity(0.0)),
                    ),
                )
                .child(
                    text(move || -> SharedString { name().into() })
                        .text_size(px(size::body()))
                        .flex_1()
                        .min_w(px(0.0))
                        .number_of_lines(1)
                        .font_weight(FontWeight::NORMAL)
                        .style_when(is_now, |s| {
                            s.font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().primary)
                        }),
                )
                .child(
                    text(move || -> SharedString {
                        node.with(|n| n.as_ref().map(|n| type_label(n)).unwrap_or_default())
                            .into()
                    })
                    .text_size(px(size::small()))
                    .text_color(move || theme().muted_foreground)
                    .w(px(110.0))
                    .number_of_lines(1),
                )
                .child(
                    text(move || -> SharedString {
                        if testing() {
                            "…".into()
                        } else {
                            ui::delay_text(delay())
                        }
                    })
                    .text_size(px(size::body()))
                    .tabular_nums()
                    .text_right()
                    .w(px(72.0))
                    .text_color(move || ui::delay_tone(delay())),
                )
                .child(
                    ui::icon_btn(Icon::Bolt, "Test")
                        .size(px(24.0))
                        .disabled(testing)
                        .on_press(move || store.run(store.svc().test_node(name(), None))),
                )
        },
    )
    .keys(move || nodes.with(|n| n.iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
    .estimated_item_size(px(36.0))
    .content_padding_top(px(4.0))
    .content_padding_bottom(px(12.0))
    .flex_1()
}

/// mihomo's adapter type, spelled the way the protocols spell themselves.
fn kind_name(kind: &str) -> &str {
    match kind {
        "Http" => "HTTP",
        "Socks5" => "SOCKS5",
        "Ss" | "Shadowsocks" => "Shadowsocks",
        "ShadowsocksR" | "Ssr" => "ShadowsocksR",
        "Vmess" => "VMess",
        "Vless" => "VLESS",
        "Tuic" => "TUIC",
        "Ssh" => "SSH",
        "AnyTLS" | "Anytls" => "AnyTLS",
        "WireGuard" | "Wireguard" => "WireGuard",
        other => other,
    }
}

fn type_label(n: &Node) -> String {
    let mut s = if n.is_group {
        "Group".to_owned()
    } else {
        kind_name(&n.kind).to_owned()
    };
    if n.udp {
        s.push_str(" · UDP");
    }
    s
}

fn provider_detail(name: String, query: Signal<String>) -> impl IntoElement {
    let store = store();
    let pname = name.clone();
    let provider = computed(move || {
        store
            .proxies
            .with(|p| p.0.providers.iter().find(|x| x.name == pname).cloned())
    });
    let tname = name.clone();
    let testing = computed(move || store.proxies.with(|p| p.0.testing.contains(&tname)));
    let testing = move || testing.get();
    let (n1, n2) = (name.clone(), name.clone());
    // A provider's nodes: the members of groups that come from it are not listed
    // separately by the API, so show its summary and actions.
    let _ = query;
    view()
        .flex_1()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_5()
                .py_3()
                .border_b_1()
                .border_color(move || theme().border)
                .child(
                    view()
                        .flex_1()
                        .gap_0p5()
                        .child(
                            text(name.clone())
                                .text_size(px(size::large()))
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(ui::muted(move || -> SharedString {
                            provider.with(|p| match p {
                                Some(p) => format!(
                                    "{} · {} nodes · {} alive{}",
                                    p.vehicle,
                                    p.count,
                                    p.alive,
                                    p.updated_at
                                        .as_ref()
                                        .map(|u| format!(
                                            " · updated {}",
                                            u.split('T').next().unwrap_or(u)
                                        ))
                                        .unwrap_or_default()
                                )
                                .into(),
                                None => "".into(),
                            })
                        })),
                )
                .child(
                    ui::btn_icon(
                        Icon::Bolt,
                        move || -> SharedString {
                            if testing() {
                                "Checking…".into()
                            } else {
                                "Health Check".into()
                            }
                        },
                        Variant::Secondary,
                    )
                    .disabled(testing)
                    .on_press(move || store.run(store.svc().healthcheck_provider(n1.clone()))),
                )
                .child(
                    ui::btn_icon(Icon::Refresh, "Update", Variant::Secondary).on_press(move || {
                        store.run_then(store.svc().update_provider(n2.clone()), move |r| {
                            if r.is_ok() {
                                store.info("Provider updated");
                            }
                        })
                    }),
                ),
        )
        .child(dynamic(move || {
            let info = provider.with(|p| p.as_ref().and_then(|p| p.info));
            untracked(move || {
                view()
                    .p_5()
                    .gap_3()
                    .child(info.map(|i| {
                        let used = i.upload + i.download;
                        let fraction = if i.total > 0 {
                            used as f32 / i.total as f32
                        } else {
                            0.0
                        };
                        ui::card()
                            .p_4()
                            .gap_2()
                            .child(ui::section_label("Subscription"))
                            .child(ui::meter(move || fraction))
                            .child(ui::muted(format!(
                                "{} of {} · {}",
                                kairo_core::bytes(used),
                                kairo_core::bytes(i.total),
                                crate::pages::overview::expiry_text(
                                    (i.expire > 0).then_some(i.expire)
                                )
                            )))
                    }))
                    .child(ui::muted(
                        "Nodes from this provider appear in the groups that use it.",
                    ))
            })
        }))
}
