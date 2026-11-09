//! The Linux window, built from libadwaita's parts around the NSH content: a
//! navigation split view with a sidebar of the pages (a `.navigation-sidebar`
//! list with the icon theme's symbolic icons), header bars whose title is the
//! page and whose subtitle is the core's state, and the NSH view as the
//! content. The NSH content takes its colors from the GTK theme (libadwaita's
//! named colors, which KDE's color schemes set too), so it matches the chrome
//! on GNOME and on KDE.

use std::rc::Rc;

use kairo_core::{CoreStatus, Service, Theme};
use libadwaita as adw;
use nsh_app_kairo::chrome;
use nsh_app_kairo::store::Page;
use nsh_platform_linux::AppView;
use nsh_platform_linux::gtk4 as gtk;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

const PAGES: [Page; 8] =
    [Page::Overview, Page::Proxies, Page::Profiles, Page::Connections, Page::Rules, Page::Logs, Page::Dns, Page::Settings];

/// The icon theme's names for a page's glyph, best first (Adwaita and Breeze
/// name some differently).
fn icons(page: Page) -> &'static [&'static str] {
    match page {
        Page::Overview => &["power-profile-performance-symbolic", "speedometer-symbolic", "utilities-system-monitor-symbolic"],
        Page::Proxies => &["globe-symbolic", "web-browser-symbolic", "network-workgroup-symbolic"],
        Page::Profiles => &["text-x-generic-symbolic", "emblem-documents-symbolic", "document-properties-symbolic"],
        Page::Connections => &["network-transmit-receive-symbolic", "network-connect-symbolic", "network-wired-symbolic"],
        Page::Rules => &["view-filter-symbolic", "filter-symbolic", "view-list-symbolic", "format-justify-left-symbolic"],
        Page::Logs => &["utilities-terminal-symbolic"],
        Page::Dns => &["network-server-symbolic"],
        Page::Settings => &["emblem-system-symbolic", "preferences-system-symbolic"],
    }
}

fn sidebar_row(page: Page) -> (gtk::ListBoxRow, gtk::Label) {
    let icon = gtk::Image::from_gicon(&gio::ThemedIcon::from_names(icons(page)));
    let title = gtk::Label::builder().label(page.title()).xalign(0.0).hexpand(true).build();
    let count = gtk::Label::builder().css_classes(["dim-label", "numeric"]).build();
    let row = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(12).build();
    row.append(&icon);
    row.append(&title);
    row.append(&count);
    let r = gtk::ListBoxRow::builder().child(&row).build();
    r.update_property(&[gtk::accessible::Property::Label(page.title())]);
    (r, count)
}

/// Builds the window's content around `nsh` (the NSH view's widget).
pub fn build(window: &adw::ApplicationWindow, view: Rc<AppView>, svc: Service) {
    let list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
    let mut counts = Vec::new();
    for page in PAGES {
        let (row, count) = sidebar_row(page);
        list.append(&row);
        counts.push(count);
    }
    // The everyday pages, what is flowing, and Settings, apart.
    list.set_header_func(|row, _| {
        let i = row.index();
        let separator = (i == 3 || i == 7).then(|| gtk::Separator::new(gtk::Orientation::Horizontal));
        row.set_header(separator.as_ref());
    });
    // The page the app opens on, before the list reports selections.
    let first = *chrome::bridge().page.borrow();
    let first_row = PAGES.iter().position(|p| *p == first).unwrap_or(0);
    list.select_row(list.row_at_index(first_row as i32).as_ref());
    list.connect_row_selected(|_, row| {
        if let Some(page) = row.and_then(|r| PAGES.get(r.index() as usize)) {
            chrome::select(*page);
        }
    });
    let side_scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&list).build();
    let side_bar = adw::HeaderBar::builder().title_widget(&adw::WindowTitle::new("Kairo", "")).build();
    let side_view = adw::ToolbarView::new();
    side_view.add_top_bar(&side_bar);
    side_view.set_content(Some(&side_scroll));
    let sidebar = adw::NavigationPage::builder().title("Kairo").tag("sidebar").child(&side_view).build();

    let title = adw::WindowTitle::new("Overview", "");
    let content_bar = adw::HeaderBar::builder().title_widget(&title).build();
    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&content_bar);
    content_view.set_content(Some(&view.widget()));
    let content = adw::NavigationPage::builder().title("Overview").tag("content").child(&content_view).build();

    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar)
        .content(&content)
        .min_sidebar_width(200.0)
        .max_sidebar_width(260.0)
        .build();
    window.set_content(Some(&split));

    // The NSH content in the GTK theme's colors, again when they change.
    let style = adw::StyleManager::default();
    let sync = {
        let view = view.clone();
        let probe = list.clone();
        move || {
            if let Err(e) = view.set_theme_overrides(Some(&theme_json(&probe))) {
                log::warn!("theme overrides: {e}");
            }
        }
    };
    sync();
    {
        let sync = sync.clone();
        style.connect_dark_notify(move |_| sync());
    }
    {
        let sync = sync.clone();
        // libadwaita 1.6's system accent color, where there is one.
        if style.find_property("accent-color-rgba").is_some() {
            style.connect_notify_local(Some("accent-color-rgba"), move |_, _| sync());
        }
    }
    {
        let sync = sync.clone();
        style.connect_high_contrast_notify(move |_| sync());
    }

    // The page: from the sidebar, the NSH content (keys, links) and menus.
    let mut pages = chrome::bridge().page.subscribe();
    let list_page = list.clone();
    let title_page = title.clone();
    let content_page = content.clone();
    glib::spawn_future_local(async move {
        loop {
            let page = *pages.borrow_and_update();
            let i = PAGES.iter().position(|p| *p == page).unwrap_or(0);
            if list_page.selected_row().map(|r| r.index() as usize) != Some(i) {
                list_page.select_row(list_page.row_at_index(i as i32).as_ref());
            }
            title_page.set_title(page.title());
            content_page.set_title(page.title());
            if pages.changed().await.is_err() {
                break;
            }
        }
    });

    // The core's state: the subtitle, the counts, the color scheme.
    let mut state = svc.state();
    let mut live = svc.live();
    glib::spawn_future_local(async move {
        loop {
            let s = state.borrow_and_update().clone();
            let l = live.borrow_and_update().clone();
            title.set_subtitle(&match &s.core {
                CoreStatus::Running { .. } => format!(
                    "{} · {} · ↓ {}  ↑ {}",
                    s.core.title(),
                    s.settings.mode.title(),
                    kairo_core::speed(l.down),
                    kairo_core::speed(l.up)
                ),
                other => other.title().to_owned(),
            });
            let n = |v: usize| if v > 0 { v.to_string() } else { String::new() };
            counts[2].set_label(&n(s.profiles.profiles.len()));
            counts[3].set_label(&n(l.connections));
            let scheme = match s.settings.theme {
                Theme::System => adw::ColorScheme::Default,
                Theme::Light => adw::ColorScheme::ForceLight,
                Theme::Dark => adw::ColorScheme::ForceDark,
            };
            if style.color_scheme() != scheme {
                style.set_color_scheme(scheme);
            }
            kairo_core::tokio::select! {
                r = state.changed() => if r.is_err() { break },
                r = live.changed() => if r.is_err() { break },
            }
        }
    });
}

fn hex(c: gdk::RGBA) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    if c.alpha() >= 0.999 {
        format!("#{:02x}{:02x}{:02x}", b(c.red()), b(c.green()), b(c.blue()))
    } else {
        format!("rgba({}, {}, {}, {:.3})", b(c.red()), b(c.green()), b(c.blue()), c.alpha())
    }
}

/// `c` over `base` (both opaque results).
fn over(c: gdk::RGBA, base: gdk::RGBA) -> gdk::RGBA {
    let a = c.alpha();
    gdk::RGBA::new(
        c.red() * a + base.red() * (1.0 - a),
        c.green() * a + base.green() * (1.0 - a),
        c.blue() * a + base.blue() * (1.0 - a),
        1.0,
    )
}

/// The NSH theme of the GTK theme's named colors (libadwaita's; KDE's color
/// schemes write them to gtk-4.0/colors.css), for the current scheme.
#[allow(deprecated)]
fn theme_json(probe: &impl IsA<gtk::Widget>) -> String {
    let ctx = probe.style_context();
    let dark = adw::StyleManager::default().is_dark();
    let get = |name: &str, fallback: &str| ctx.lookup_color(name).unwrap_or_else(|| gdk::RGBA::parse(fallback).expect("color"));
    let window = get("window_bg_color", if dark { "#222226" } else { "#fafafb" });
    let view = get("view_bg_color", if dark { "#1d1d20" } else { "#ffffff" });
    let fg = get("window_fg_color", if dark { "#ffffff" } else { "rgba(0,0,6,0.8)" });
    let card = over(get("card_bg_color", if dark { "rgba(255,255,255,0.08)" } else { "#ffffff" }), window);
    let accent_bg = get("accent_bg_color", "#3584e4");
    let accent_fg = get("accent_fg_color", "#ffffff");
    let accent_text = get("accent_color", if dark { "#81d0ff" } else { "#0461be" });
    let destructive = get("destructive_color", if dark { "#ff7b63" } else { "#c01c28" });
    let success = get("success_color", if dark { "#78e9ab" } else { "#1b8553" });
    let warning = get("warning_color", if dark { "#ffc252" } else { "#9c6e03" });
    let headerbar = get("headerbar_bg_color", &hex(window));
    let sidebar = get("sidebar_bg_color", &hex(headerbar));
    let shade = get("shade_color", if dark { "rgba(0,0,6,0.36)" } else { "rgba(0,0,6,0.07)" });
    let mix = |a: f32| over(gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), a), window);
    let muted_fg = mix(0.55);
    let border = mix(if dark { 0.15 } else { 0.15 });
    let muted = mix(0.06);
    let hover = mix(0.08);
    let tokens = format!(
        r#""background": "{window}", "foreground": "{fg}", "card": "{card}", "card-foreground": "{fg}",
        "popover": "{card}", "popover-foreground": "{fg}", "primary": "{accent_bg}", "primary-foreground": "{accent_fg}",
        "secondary": "{muted}", "secondary-foreground": "{fg}", "muted": "{muted}", "muted-foreground": "{muted_fg}",
        "accent": "{hover}", "accent-foreground": "{fg}", "destructive": "{destructive}", "border": "{border}",
        "input": "{border}", "ring": "{accent_text}", "sidebar": "{sidebar}", "sidebar-foreground": "{fg}",
        "sidebar-accent": "{hover}", "sidebar-accent-foreground": "{fg}", "sidebar-border": "{shade}",
        "sidebar-primary": "{accent_text}", "success": "{success}", "warning": "{warning}", "track": "{border}",
        "selection": "{hover}", "graph-up": "{muted_fg}", "subtle": "{view}""#,
        window = hex(window),
        fg = hex(over(fg, window)),
        card = hex(card),
        accent_bg = hex(accent_bg),
        accent_fg = hex(accent_fg),
        accent_text = hex(accent_text),
        muted = hex(muted),
        muted_fg = hex(muted_fg),
        hover = hex(hover),
        destructive = hex(destructive),
        border = hex(border),
        sidebar = hex(sidebar),
        shade = hex(shade),
        success = hex(success),
        warning = hex(warning),
        view = hex(view),
    );
    format!(r#"{{"cssVars": {{"theme": {{"radius": "12px", "radius-control": "6px"}}, "light": {{{tokens}}}, "dark": {{{tokens}}}}}}}"#)
}
