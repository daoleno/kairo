//! Kairo: a native mihomo client. This crate is the UI, written with Native
//! Shell (NSH) in Rust; `kairo-core` is the engine and `kairo-tray` the tray.

pub mod accent;
pub mod backend;
pub mod chrome;
mod flags;
pub mod i18n;
pub mod logos;
mod logos_table;
mod pages;
pub mod palette;
mod shell;
#[cfg(target_arch = "wasm32")]
pub mod web;
pub mod store;
pub mod ui;

use kairo_core::Theme;
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
    #[cfg(target_arch = "wasm32")]
    return web::app().into_element();
    #[cfg(not(target_arch = "wasm32"))]
    main_view(backend::Backend::local()).into_element()
}

/// The window's content over `backend`: its theme, keys, menus and pages.
pub fn main_view(backend: backend::Backend) -> impl IntoElement {
    ui::set_machine(backend.machine());
    let store = Store::new(backend);
    provide_context(store);

    // The language follows the setting (System: the person's languages).
    let language = computed(move || store.state.with(|s| s.settings.language));
    effect(move || {
        nsh_core::i18n::set_locale(language.get().tag().and_then(nsh_core::platform::Locale::parse))
    });
    let shown = i18n::shown();

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
        let mut bar = nsh_core::desktop::TitleBarOptions::default();
        bar.style = nsh_core::desktop::TitleBarStyle::Hidden;
        bar.height = Some(px(ui::HEADER_HEIGHT));
        let mut attributes = nsh_core::desktop::WindowAttributes::default();
        attributes.title_bar = Some(bar);
        nsh_core::element::desktop::current_window().set(attributes);
    }
    // macOS has a menu bar; GTK apps on Linux keep their window free of one.
    if os() == Os::MacOs {
        menus(store, shown);
    }
    for (i, page) in GO.into_iter().enumerate() {
        on_key(&format!("mod+{}", i + 1), move |_| store.page.set(page));
    }

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        // A new language builds the window again: every label is read anew.
        .child(dynamic(move || {
            let _ = shown.get();
            untracked(|| shell::shell().into_element())
        }))
}

/// The pages ⌘1… open, in the sidebar's order.
pub const GO: [Page; 4] = [
    Page::Overview,
    Page::Apps,
    Page::Activity,
    Page::Profiles,
];

/// Fastest, then the quickest places (the rest are a search away on Home).
fn location_items(store: Store) -> Vec<MenuItem> {
    use kairo_core::places;
    let auto = store.state.with(|s| s.settings.auto_location);
    let preferred = store.state.with(|s| s.settings.tray_group.clone());
    let (now, mut list) =
        store
            .proxies
            .with(|p| match places::main_group(&p.0.groups, &preferred) {
                Some(g) => (
                    places::current(&p.0.groups, g).map(|c| c.name),
                    places::places(&p.0.groups, g),
                ),
                None => (None, vec![]),
            });
    list.retain(|p| !p.is_group);
    list.sort_by_key(|p| match p.delay {
        Some(d) if d > 0 => d,
        _ => u32::MAX,
    });
    let mut items = vec![
        MenuItem::new(t!("menu.fastest"))
            .checked(auto)
            .on_select(move || store.run(store.svc().set_location(None))),
        MenuItem::separator(),
    ];
    for p in list.into_iter().take(12) {
        let name = p.name.clone();
        let title = ui::menu_text(format!(
            "{}{}{}",
            p.flag.map(|f| f + "  ").unwrap_or_default(),
            p.title,
            p.delay
                .filter(|d| *d > 0)
                .map(|d| format!("   {d} ms"))
                .unwrap_or_default()
        ));
        items.push(
            MenuItem::new(title)
                .checked(!auto && now.as_deref() == Some(name.as_str()))
                .on_select(move || store.run(store.svc().set_location(Some(name.clone())))),
        );
    }
    items.push(MenuItem::separator());
    items.push(MenuItem::new(t!("menu.all_locations")).on_select(move || store.page.set(Page::Apps)));
    items
}

fn menus(store: Store, shown: Memo<String>) {
    use nsh_core::desktop::MenuRole;
    let mac = os() == Os::MacOs;
    effect(move || {
        // In the language shown: `tr!` (the page titles) follows it once this has run.
        let language = shown.get();
        // A standard item: NSH's English (with the app's name), else ours.
        let role = |r: MenuRole, key: &str| {
            if language == "en" {
                MenuItem::role(r)
            } else {
                MenuItem::role(r).label(t!(key))
            }
        };
        let mode = store.state.with(|s| s.settings.mode);
        let running = store.state.with(|s| s.core.is_running());
        let has_profile = store.state.with(|s| s.profiles.active.is_some());
        let svc = store.svc();
        let mut menus = Vec::new();
        if mac {
            menus.push(MenuItem::submenu(
                "Kairo",
                [
                    role(MenuRole::About, "menu.role.about"),
                    match store.state.with(|s| chrome::update_button(&s.update)) {
                        Some(b) => MenuItem::new(b.label).disabled(!b.enabled).on_select({
                            let svc = svc.clone();
                            move || chrome::update_pressed(&svc)
                        }),
                        None => MenuItem::new(t!("menu.check_updates")).on_select({
                            let svc = svc.clone();
                            move || {
                                store.run_then(svc.check_for_update(), move |r| {
                                    if let Ok(None) = r {
                                        store.info(kairo_core::tr!("menu.up_to_date"));
                                    }
                                })
                            }
                        }),
                    },
                    MenuItem::separator(),
                    MenuItem::new(t!("menu.settings"))
                        .shortcut("mod+,")
                        .on_select(move || store.page.set(Page::Settings)),
                    MenuItem::separator(),
                    role(MenuRole::Hide, "menu.role.hide"),
                    role(MenuRole::HideOthers, "menu.role.hide_others"),
                    MenuItem::separator(),
                    role(MenuRole::Quit, "menu.role.quit"),
                ],
            ));
        }
        let mut conn = vec![
            MenuItem::new(if running { t!("menu.disconnect") } else { t!("menu.connect") })
                .disabled(!has_profile)
                .on_select({
                    let svc = svc.clone();
                    move || store.run(svc.connect(!running))
                }),
            MenuItem::separator(),
            MenuItem::submenu(t!("menu.location"), location_items(store)),
        ];
        for (m, title) in [
            (kairo_core::Mode::Rule, t!("menu.mode.rule")),
            (kairo_core::Mode::Global, t!("menu.mode.global")),
            (kairo_core::Mode::Direct, t!("menu.mode.direct")),
        ] {
            let svc = svc.clone();
            conn.push(
                MenuItem::new(title)
                    .checked(mode == m)
                    .on_select(move || store.run(svc.set_mode(m))),
            );
        }
        conn.push(MenuItem::separator());
        conn.push(MenuItem::submenu(
            t!("menu.core"),
            [
                MenuItem::new(if running { t!("menu.core.stop") } else { t!("menu.core.start") }).on_select({
                    let svc = svc.clone();
                    move || {
                        if running {
                            spawn(svc.stop_core());
                        } else {
                            store.run(svc.start_core());
                        }
                    }
                }),
                MenuItem::new(t!("menu.core.restart")).disabled(!running).on_select({
                    let svc = svc.clone();
                    move || store.run(svc.restart_core())
                }),
            ],
        ));
        if !mac {
            conn.push(MenuItem::separator());
            conn.push(
                MenuItem::new(t!("menu.settings"))
                    .shortcut("mod+,")
                    .on_select(move || store.page.set(Page::Settings)),
            );
            conn.push(role(MenuRole::Quit, "menu.role.quit"));
        }
        menus.push(MenuItem::submenu(t!("menu.connection"), conn));
        let mut go = vec![
            MenuItem::new(t!("menu.search"))
                .shortcut("mod+k")
                .on_select(crate::palette::open),
            MenuItem::separator(),
        ];
        go.extend(GO.into_iter().enumerate().map(|(i, page)| {
            MenuItem::new(page.title())
                .shortcut(["mod+1", "mod+2", "mod+3", "mod+4", "mod+5"][i])
                .on_select(move || store.page.set(page))
        }));
        menus.push(MenuItem::submenu(t!("menu.go"), go));
        menus.push(MenuItem::submenu(
            t!("menu.edit"),
            [
                role(MenuRole::Undo, "menu.role.undo"),
                role(MenuRole::Redo, "menu.role.redo"),
                MenuItem::separator(),
                role(MenuRole::Cut, "menu.role.cut"),
                role(MenuRole::Copy, "menu.role.copy"),
                role(MenuRole::Paste, "menu.role.paste"),
                role(MenuRole::SelectAll, "menu.role.select_all"),
            ],
        ));
        menus.push(MenuItem::submenu(
            t!("menu.window"),
            [
                role(MenuRole::Minimize, "menu.role.minimize"),
                role(MenuRole::Zoom, "menu.role.zoom"),
                role(MenuRole::Close, "menu.role.close"),
            ],
        ));
        set_application_menu(menus);
    });
}
