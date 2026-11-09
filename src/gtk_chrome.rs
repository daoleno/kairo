//! The Linux window, built from libadwaita's parts around the NSH content: a
//! navigation split view with a sidebar of the pages (a `.navigation-sidebar`
//! list with the icon theme's symbolic icons), header bars whose title is the
//! page and whose subtitle is the core's state, and the NSH view as the
//! content. The NSH content takes its colors from the GTK theme (libadwaita's
//! named colors, which KDE's color schemes set too), so it matches the chrome
//! on GNOME and on KDE.

use std::rc::Rc;

use kairo_core::{CoreStatus, Live, Service, State, Theme};
use nsh_app_kairo::chrome::{Tool, ToolEvent};
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
    let footer = Footer::new();
    side_view.add_bottom_bar(&footer.root);
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
    let apply = {
        let view = view.clone();
        let probe = list.clone();
        move || {
            if let Err(e) = view.set_theme_overrides(Some(&theme_json(&probe))) {
                log::warn!("theme overrides: {e}");
            }
        }
    };
    apply();
    // Read once libadwaita has swapped its stylesheet (after the notify).
    let sync = move || {
        let apply = apply.clone();
        glib::idle_add_local_once(move || apply());
    };
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

    // The content's text at the desktop's UI font size (Kairo's body is 13 px
    // at 1.0; Cantarell 11 is 14.7 px), with the desktop's text scaling.
    if let Some(settings) = gtk::Settings::default() {
        let apply = {
            let view = view.clone();
            move |s: &gtk::Settings| view.set_font_scale(Some(font_scale(s)))
        };
        apply(&settings);
        let a = apply.clone();
        settings.connect_gtk_font_name_notify(move |s| a(s));
        settings.connect_gtk_xft_dpi_notify(move |s| apply(s));
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

    // The page's tools, at the end of the content's header bar.
    let tools = Rc::new(std::cell::RefCell::new(HeaderTools::default()));
    let actions = gio::SimpleActionGroup::new();
    window.insert_action_group("tool", Some(&actions));
    let mut toolbar = chrome::bridge().toolbar.subscribe();
    let title_tools = title.clone();
    glib::spawn_future_local(async move {
        loop {
            let t = toolbar.borrow_and_update().clone();
            tools.borrow_mut().show(&content_bar, &actions, &t.tools);
            title_tools.set_subtitle(&t.subtitle);
            if toolbar.changed().await.is_err() {
                break;
            }
        }
    });

    // The core's state: the sidebar's footer, the counts, the color scheme.
    let mut state = svc.state();
    let mut live = svc.live();
    glib::spawn_future_local(async move {
        loop {
            let s = state.borrow_and_update().clone();
            let l = live.borrow_and_update().clone();
            footer.update(&s, &l);
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

/// The sidebar's foot: the core's state, the mode and the speeds.
struct Footer {
    root: gtk::Box,
    dot: gtk::Label,
    status: gtk::Label,
    mode: gtk::Label,
    up: gtk::Label,
    down: gtk::Label,
}

impl Footer {
    fn new() -> Footer {
        let label = |classes: &[&str]| gtk::Label::builder().xalign(0.0).css_classes(classes.to_vec()).build();
        let dot = gtk::Label::new(Some("●"));
        let status = label(&["caption-heading"]);
        status.set_hexpand(true);
        let mode = label(&["caption", "dim-label"]);
        let up = label(&["caption", "numeric", "dim-label"]);
        up.set_hexpand(true);
        let down = label(&["caption", "numeric", "dim-label"]);
        down.set_hexpand(true);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        top.append(&dot);
        top.append(&status);
        top.append(&mode);
        let speeds = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        speeds.append(&up);
        speeds.append(&down);
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_start(18)
            .margin_end(18)
            .margin_top(10)
            .margin_bottom(12)
            .build();
        root.append(&top);
        root.append(&speeds);
        Footer { root, dot, status, mode, up, down }
    }

    fn update(&self, s: &State, l: &Live) {
        self.status.set_label(s.core.title());
        self.mode.set_label(s.settings.mode.title());
        let tone = match &s.core {
            CoreStatus::Running { .. } => "success",
            CoreStatus::Starting | CoreStatus::Stopping => "warning",
            CoreStatus::Failed { .. } => "error",
            _ => "dim-label",
        };
        self.dot.set_css_classes(&[tone]);
        let running = s.core.is_running();
        self.up.set_visible(running);
        self.down.set_visible(running);
        self.up.set_label(&format!("↑ {}", kairo_core::speed(l.up)));
        self.down.set_label(&format!("↓ {}", kairo_core::speed(l.down)));
        self.root.update_property(&[gtk::accessible::Property::Label(&format!("{} · {}", s.core.title(), s.settings.mode.title()))]);
    }
}

/// The widgets of the page's tools in the header bar.
#[derive(Default)]
struct HeaderTools {
    shape: Vec<(&'static str, u8)>,
    widgets: Vec<gtk::Widget>,
}

impl HeaderTools {
    fn show(&mut self, bar: &adw::HeaderBar, actions: &gio::SimpleActionGroup, tools: &[Tool]) {
        let shape: Vec<(&'static str, u8)> = tools
            .iter()
            .map(|t| {
                (t.id(), match t {
                    Tool::Search { .. } => 0,
                    Tool::Button { .. } => 1,
                    Tool::Choice { .. } => 2,
                })
            })
            .collect();
        if shape != self.shape {
            for w in self.widgets.drain(..) {
                bar.remove(&w);
            }
            for name in actions.list_actions() {
                actions.remove_action(&name);
            }
            // pack_end puts each one left of the last: the last tool first.
            for tool in tools.iter().rev() {
                let w = make_tool(actions, tool);
                bar.pack_end(&w);
                self.widgets.push(w);
            }
            self.widgets.reverse();
            self.shape = shape;
        }
        for (tool, w) in tools.iter().zip(&self.widgets) {
            update_tool(actions, tool, w);
        }
    }
}

fn make_tool(actions: &gio::SimpleActionGroup, tool: &Tool) -> gtk::Widget {
    match tool {
        Tool::Search { id, placeholder, .. } => {
            let entry = gtk::SearchEntry::builder().placeholder_text(*placeholder).width_chars(22).build();
            entry.update_property(&[gtk::accessible::Property::Label(placeholder)]);
            let id = *id;
            entry.connect_search_changed(move |e| chrome::tool_event(ToolEvent::Search(id, e.text().to_string())));
            entry.upcast()
        }
        Tool::Button { id, glyph, .. } => {
            let b = gtk::Button::builder().child(&gtk::Image::from_gicon(&gio::ThemedIcon::from_names(glyph.icon()))).build();
            let id = *id;
            b.connect_clicked(move |_| chrome::tool_event(ToolEvent::Pressed(id)));
            b.upcast()
        }
        Tool::Choice { id, label, glyph, options, .. } => {
            let action = gio::SimpleAction::new_stateful(id, Some(glib::VariantTy::INT32), &0i32.to_variant());
            let tid = *id;
            action.connect_activate(move |a, v| {
                if let Some(i) = v.and_then(|v| v.get::<i32>()) {
                    a.set_state(&i.to_variant());
                    chrome::tool_event(ToolEvent::Chose(tid, i as usize));
                }
            });
            actions.add_action(&action);
            let menu = gio::Menu::new();
            for (i, o) in options.iter().enumerate() {
                let item = gio::MenuItem::new(Some(o), None);
                item.set_action_and_target_value(Some(&format!("tool.{id}")), Some(&(i as i32).to_variant()));
                menu.append_item(&item);
            }
            let b = gtk::MenuButton::builder()
                .menu_model(&menu)
                .tooltip_text(*label)
                .child(&gtk::Image::from_gicon(&gio::ThemedIcon::from_names(glyph.icon())))
                .build();
            b.update_property(&[gtk::accessible::Property::Label(label)]);
            b.upcast()
        }
    }
}

fn update_tool(actions: &gio::SimpleActionGroup, tool: &Tool, w: &gtk::Widget) {
    match tool {
        Tool::Search { value, .. } => {
            if let Some(e) = w.downcast_ref::<gtk::SearchEntry>()
                && e.text() != value.as_str()
            {
                e.set_text(value);
            }
        }
        Tool::Button { label, glyph, enabled, .. } => {
            if let Some(b) = w.downcast_ref::<gtk::Button>() {
                b.set_sensitive(*enabled);
                b.set_tooltip_text(Some(label));
                b.update_property(&[gtk::accessible::Property::Label(label)]);
                b.set_child(Some(&gtk::Image::from_gicon(&gio::ThemedIcon::from_names(glyph.icon()))));
            }
        }
        Tool::Choice { id, selected, .. } => {
            if let Some(a) = actions.lookup_action(id).and_then(|a| a.downcast::<gio::SimpleAction>().ok()) {
                a.set_state(&(*selected as i32).to_variant());
            }
        }
    }
}

/// The NSH text scale that makes Kairo's 13 px body the desktop's UI font.
fn font_scale(settings: &gtk::Settings) -> f32 {
    let points = settings
        .gtk_font_name()
        .map(|n| gtk::pango::FontDescription::from_string(&n))
        .map(|d| d.size() as f32 / gtk::pango::SCALE as f32)
        .filter(|p| *p > 0.0)
        .unwrap_or(11.0);
    let dpi = settings.gtk_xft_dpi();
    let system = if dpi > 0 { dpi as f32 / 1024.0 / 96.0 } else { 1.0 };
    (points * 96.0 / 72.0 / 13.0 * system).clamp(0.8, 3.0)
}
