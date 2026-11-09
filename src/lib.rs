//! Kairo: a native mihomo client. This crate is the UI, written with Native
//! Shell (NSH) in Rust; `kairo-core` is the engine and `kairo-tray` the tray.

pub mod chrome;
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
    nsh_base::tokens_of(Some(THEME)).expect("theme.json is a valid theme")
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
    for (i, page) in [Page::Overview, Page::Proxies, Page::Profiles, Page::Connections, Page::Rules, Page::Logs, Page::Dns]
        .into_iter()
        .enumerate()
    {
        on_key(&format!("mod+{}", i + 1), move |_| store.page.set(page));
    }

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(shell::shell())
}

fn menus(store: Store) {
    use nsh_core::desktop::MenuRole;
    let mac = os() == Os::MacOs;
    effect(move || {
        let mode = store.state.with(|s| s.settings.mode);
        let running = store.state.with(|s| s.core.is_running());
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
        let mut core = vec![
            MenuItem::new(if running { "Stop Core" } else { "Start Core" }).on_select({
                let svc = svc.clone();
                move || {
                    if running {
                        let f = svc.stop_core();
                        spawn(f);
                    } else {
                        store.run(svc.start_core());
                    }
                }
            }),
            MenuItem::new("Restart Core").disabled(!running).on_select({
                let svc = svc.clone();
                move || store.run(svc.restart_core())
            }),
            MenuItem::separator(),
        ];
        for m in kairo_core::Mode::ALL {
            let svc = svc.clone();
            core.push(MenuItem::new(format!("{} Mode", m.title())).checked(mode == m).on_select(move || store.run(svc.set_mode(m))));
        }
        if !mac {
            core.push(MenuItem::separator());
            core.push(MenuItem::new("Settings…").shortcut("mod+,").on_select(move || store.page.set(Page::Settings)));
            core.push(MenuItem::role(MenuRole::Quit));
        }
        menus.push(MenuItem::submenu("Core", core));
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
