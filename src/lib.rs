//! Kairo: a native mihomo client. This crate is the UI, written with Native
//! Shell (NSH) in Rust; `kairo-core` is the engine and `kairo-tray` the tray.

pub mod accent;
pub mod chrome;
pub mod palette;
mod pages;
mod shell;
pub mod store;
pub mod ui;

use kairo_core::{Service, Theme};
use nsh_base::theme;
use nsh_core::element::desktop::{MenuItem, set_application_menu};
use nsh_core::platform::{Appearance, Os};
use nsh_core::prelude::*;

use crate::store::{Page, Store};

/// The app's theme (`theme.json`, also named by `nsh.json`).
pub const THEME: &str = include_str!("../theme.json");

/// The design tokens of [`THEME`], for runners that mount the root themselves.
pub fn tokens() -> nsh_core::element::Tokens {
    nsh_base::tokens_of(Some(&accent::apply(THEME))).expect("theme.json is a valid theme")
}

/// Kairo's UI set (`nsh.json` `"rust"`).
#[native_module("kairo/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// The application's root.
#[component]
pub fn App() -> impl IntoElement {
    let store = Store::new(Service::global());
    provide_context(store);

    // The app's appearance follows the setting (System follows the desktop).
    let chosen = computed(move || store.state.with(|s| s.settings.theme));
    effect(move || {
        set_color_scheme(match chosen.get() {
            Theme::System => Appearance::System,
            Theme::Light => Appearance::Light,
            Theme::Dark => Appearance::Dark,
        })
    });

    if os() == Os::MacOs && !chrome::native() {
        nsh_core::element::desktop::set_title_bar(nsh_core::desktop::TitleBarStyle::Hidden, Some(px(ui::HEADER_HEIGHT)));
    }
    // macOS has a menu bar; GTK apps on Linux keep their window free of one.
    if os() == Os::MacOs {
        menus(store);
    }
    for (i, page) in GO.into_iter().enumerate() {
        on_key(&format!("mod+{}", i + 1), move |_| store.page.set(page));
    }

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(shell::shell())
}

/// The pages ⌘1… open, in the sidebar's order.
pub const GO: [Page; 8] = [Page::Overview, Page::Apps, Page::Proxies, Page::Profiles, Page::Connections, Page::Rules, Page::Logs, Page::Dns];

/// Fastest, then the quickest places (the rest are a search away on Home).
fn location_items(store: Store) -> Vec<MenuItem> {
    use kairo_core::places;
    let auto = store.state.with(|s| s.settings.auto_location);
    let preferred = store.state.with(|s| s.settings.tray_group.clone());
    let (now, mut list) = store.proxies.with(|p| match places::main_group(&p.0.groups, &preferred) {
        Some(g) => (places::current(&p.0.groups, g).map(|c| c.name), places::places(&p.0.groups, g)),
        None => (None, vec![]),
    });
    list.retain(|p| !p.is_group);
    list.sort_by_key(|p| match p.delay {
        Some(d) if d > 0 => d,
        _ => u32::MAX,
    });
    let mut items = vec![MenuItem::new("Fastest").checked(auto).on_select(move || store.run(store.svc().set_location(None))), MenuItem::separator()];
    for p in list.into_iter().take(12) {
        let name = p.name.clone();
        let title = format!("{}{}{}", p.flag.map(|f| f + "  ").unwrap_or_default(), p.title, p.delay.filter(|d| *d > 0).map(|d| format!("   {d} ms")).unwrap_or_default());
        items.push(MenuItem::new(title).checked(!auto && now.as_deref() == Some(name.as_str())).on_select(move || store.run(store.svc().set_location(Some(name.clone())))));
    }
    items.push(MenuItem::separator());
    items.push(MenuItem::new("All Locations…").on_select(move || store.page.set(Page::Overview)));
    items
}

fn menus(store: Store) {
    use nsh_core::desktop::MenuRole;
    let mac = os() == Os::MacOs;
    effect(move || {
        let mode = store.state.with(|s| s.settings.mode);
        let running = store.state.with(|s| s.core.is_running());
        let has_profile = store.state.with(|s| s.profiles.active.is_some());
        let svc = store.svc();
        let mut menus = Vec::new();
        if mac {
            menus.push(MenuItem::submenu(
                "Kairo",
                [
                    MenuItem::role(MenuRole::About),
                    match store.state.with(|s| s.update.clone()) {
                        kairo_core::UpdateStatus::Ready { version } => MenuItem::new(format!("Restart to Update to {version}")).on_select({
                            let svc = svc.clone();
                            move || store.run(svc.restart_to_update())
                        }),
                        _ => MenuItem::new("Check for Updates…").on_select({
                            let svc = svc.clone();
                            move || store.run_then(svc.check_for_update(), move |r| {
                                if let Ok(None) = r {
                                    store.info("Kairo is up to date");
                                }
                            })
                        }),
                    },
                    MenuItem::separator(),
                    MenuItem::new("Settings…").shortcut("mod+,").on_select(move || store.page.set(Page::Settings)),
                    MenuItem::separator(),
                    MenuItem::role(MenuRole::Hide),
                    MenuItem::role(MenuRole::HideOthers),
                    MenuItem::separator(),
                    MenuItem::role(MenuRole::Quit),
                ],
            ));
        }
        let mut conn = vec![
            MenuItem::new(if running { "Disconnect" } else { "Connect" }).disabled(!has_profile).on_select({
                let svc = svc.clone();
                move || store.run(svc.connect(!running))
            }),
            MenuItem::separator(),
            MenuItem::submenu("Location", location_items(store)),
        ];
        for (m, title) in [(kairo_core::Mode::Rule, "Smart Route"), (kairo_core::Mode::Global, "All Traffic"), (kairo_core::Mode::Direct, "Direct")] {
            let svc = svc.clone();
            conn.push(MenuItem::new(title).checked(mode == m).on_select(move || store.run(svc.set_mode(m))));
        }
        conn.push(MenuItem::separator());
        conn.push(MenuItem::submenu(
            "Core",
            [
                MenuItem::new(if running { "Stop Core" } else { "Start Core" }).on_select({
                    let svc = svc.clone();
                    move || {
                        if running {
                            spawn(svc.stop_core());
                        } else {
                            store.run(svc.start_core());
                        }
                    }
                }),
                MenuItem::new("Restart Core").disabled(!running).on_select({
                    let svc = svc.clone();
                    move || store.run(svc.restart_core())
                }),
            ],
        ));
        if !mac {
            conn.push(MenuItem::separator());
            conn.push(MenuItem::new("Settings…").shortcut("mod+,").on_select(move || store.page.set(Page::Settings)));
            conn.push(MenuItem::role(MenuRole::Quit));
        }
        menus.push(MenuItem::submenu("Connection", conn));
        let mut go = vec![MenuItem::new("Search Kairo…").shortcut("mod+k").on_select(crate::palette::open), MenuItem::separator()];
        go.extend(
            GO.into_iter()
                .enumerate()
                .map(|(i, page)| MenuItem::new(page.title()).shortcut(["mod+1", "mod+2", "mod+3", "mod+4", "mod+5", "mod+6", "mod+7", "mod+8"][i]).on_select(move || store.page.set(page))),
        );
        menus.push(MenuItem::submenu("Go", go));
        menus.push(MenuItem::submenu(
            "Edit",
            [
                MenuItem::role(MenuRole::Undo),
                MenuItem::role(MenuRole::Redo),
                MenuItem::separator(),
                MenuItem::role(MenuRole::Cut),
                MenuItem::role(MenuRole::Copy),
                MenuItem::role(MenuRole::Paste),
                MenuItem::role(MenuRole::SelectAll),
            ],
        ));
        menus.push(MenuItem::submenu(
            "Window",
            [MenuItem::role(MenuRole::Minimize), MenuItem::role(MenuRole::Zoom), MenuItem::role(MenuRole::Close)],
        ));
        set_application_menu(menus);
    });
}
