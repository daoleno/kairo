//! Apps & Sites: the choices the subscription makes per app or site
//! (Netflix, AI, Telegram...), in plain terms: follow the location, go
//! direct, or a place of their own.

use kairo_core::places;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, size};

/// What a member of an app's group means to a person.
fn option_title(name: &str, main: &str, direct_groups: &[String]) -> String {
    if name == main {
        "Same as Location".into()
    } else if name == "DIRECT" || direct_groups.iter().any(|d| d == name) {
        "Direct".into()
    } else if name == "REJECT" || name == "REJECT-DROP" {
        "Block".into()
    } else {
        let (flag, rest) = places::split_flag(name);
        match flag {
            Some(f) => format!("{f} {}", rest.trim()),
            None => name.to_owned(),
        }
    }
}

pub fn page() -> impl IntoElement {
    let store = store();
    let query = signal(String::new());
    let data = move || {
        let preferred = store.state.with(|s| s.settings.tray_group.clone());
        store.proxies.with(|p| {
            let groups = &p.0.groups;
            let Some(main) = places::main_group(groups, &preferred) else { return (String::new(), vec![], vec![]) };
            let direct: Vec<String> = groups.iter().filter(|g| g.selectable() && g.name != main.name && g.now.as_deref() == Some("DIRECT")).map(|g| g.name.clone()).collect();
            (main.name.clone(), places::app_choices(groups, &main.name), direct)
        })
    };
    view()
        .flex_1()
        .child(toolbar::floating(Page::Apps, String::new, vec![toolbar::search("apps.search", "Search apps and sites", query)]))
        .child(
            scroll().flex_1().child(
                view()
                    .px_8()
                    .pt(toolbar::scroll_top(24.0))
                    .pb_8()
                    .gap_4()
                    .w_full()
                    .max_w(px(640.0))
                    .self_center()
                    .child(ui::muted("Your subscription routes these apps and sites on their own. Most follow your location; change one when it needs a particular country, or should go direct.").px_1())
                    .child(dynamic(move || {
                        let (main, apps, direct) = data();
                        let q = query.get().to_lowercase();
                        untracked(move || {
                            let apps: Vec<_> = apps.into_iter().filter(|a| q.is_empty() || a.group.to_lowercase().contains(&q)).collect();
                            if main.is_empty() || apps.is_empty() {
                                return ui::empty_state(Icon::Layers, "Nothing to choose", "Your subscription routes every app and site the same way.", None).into_element();
                            }
                            ui::rows(
                                apps.into_iter()
                                    .map(|a| {
                                        let group = stored(a.group.clone());
                                        let now = a.now.clone().unwrap_or_default();
                                        let options: Vec<(String, String)> = a.options.iter().map(|o| (o.clone(), option_title(o, &main, &direct))).collect();
                                        let selected = stored(now);
                                        ui::row_view()
                                            .child(view().flex_1().min_w(px(0.0)).child(ui::body(a.group.clone())))
                                            .child(ui::popup_button(move || options.clone(), move || selected.with(|s| s.clone()), move |v: String| {
                                                store.run(store.svc().select(group.with(|g| g.clone()), v))
                                            }))
                                            .into_element()
                                    })
                                    .collect(),
                            )
                            .into_element()
                        })
                    }))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .px_1()
                            .child(ui::muted("Every group and node, as the subscription has them:").text_size(px(size::small())))
                            .child(ui::link_btn("Groups").on_press(move || store.page.set(Page::Proxies))),
                    ),
            ),
        )
        .text_color(move || theme().foreground)
}
