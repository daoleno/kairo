//! Profiles: the subscriptions and files Kairo can run. Add one from a URL or
//! a file, update them, switch between them, and edit the overrides Kairo
//! merges over the active one.

use kairo_core::profile::{Profile, Source};
use nsh_base::{alert_dialog, button, editor, theme};
use nsh_core::file_dialog::{FileFilter, OpenDialog};
use nsh_core::prelude::*;

use crate::pages::overview::expiry_text;
use crate::chrome::Glyph;
use crate::store::{Page, store};
use crate::ui::toolbar;
use crate::ui::{self, Icon, Variant, icon, size};

pub fn page() -> impl IntoElement {
    let editing = signal(None::<String>);
    view().flex_1().child(dynamic(move || match editing.get() {
        Some(id) => untracked(move || detail(id, editing)).into_element(),
        None => untracked(move || list(editing)).into_element(),
    }))
}

// --- The list ------------------------------------------------------------------------

fn list(editing: Signal<Option<String>>) -> impl IntoElement {
    let store = store();
    let adding = signal(false);
    let profiles = computed(move || store.state.with(|s| s.profiles.profiles.clone()));
    let any_remote = move || profiles.with(|p| p.iter().any(|p| matches!(p.source, Source::Remote { .. })));
    let updating_all = move || store.state.with(|s| !s.updating.is_empty());
    view()
        .flex_1()
        .child(toolbar::floating(
            Page::Profiles,
            move || {
                let n = store.state.with(|s| s.profiles.profiles.len());
                match n {
                    0 => String::new(),
                    1 => "1 profile".to_owned(),
                    n => format!("{n} profiles"),
                }
            },
            vec![
                toolbar::button("profiles.update-all", "Update All", Glyph::Refresh, Icon::Refresh, move || {
                    store.run(store.svc().update_all_profiles())
                })
                .enabled(move || any_remote() && !updating_all()),
                toolbar::button("profiles.add", "Add Profile", Glyph::Add, Icon::Plus, move || adding.set(true))
                    .enabled(move || !adding.get())
                    .prominent(),
            ],
        ))
        .child(
            scroll().flex_1().child(
                view()
                    .px_6()
                    .pt(toolbar::scroll_top(20.0))
                    .pb_6()
                    .gap_4()
                    .w_full()
                    .max_w(px(900.0))
                    .self_center()
                    .child(when(move || adding.get(), move || add_panel(adding)))
                    .child(
                        when(move || profiles.with(|p| !p.is_empty()), move || {
                            ui::card().child(each(
                                move || profiles.get(),
                                |p: &Profile| p.id.clone(),
                                move |p, i| {
                                    let first = i.get_untracked() == 0;
                                    view()
                                        .child((!first).then(|| ui::divider().ml(px(44.0))))
                                        .child(profile_row(p, editing))
                                },
                            ))
                        })
                        .otherwise(move || {
                            ui::empty_state(
                                Icon::Profiles,
                                "No profiles yet",
                                "A profile is the configuration the core runs: a subscription URL from your provider, or a YAML file.",
                                (!adding.get_untracked()).then(|| {
                                    ui::btn_icon(Icon::Plus, "Add Profile", Variant::Primary).on_press(move || adding.set(true)).into_element()
                                }),
                            )
                        }),
                    ),
            ),
        )
}

fn add_panel(adding: Signal<bool>) -> impl IntoElement {
    let store = store();
    let url = signal(String::new());
    let name = signal(String::new());
    let busy = signal(false);
    let valid = move || url.with(|u| {
        let u = u.trim();
        u.starts_with("http://") || u.starts_with("https://")
    });
    let submit = move || {
        if !valid() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        let n = name.get_untracked().trim().to_owned();
        store.run_then(store.svc().add_remote(url.get_untracked().trim().to_owned(), (!n.is_empty()).then_some(n)), move |r| {
            busy.set(false);
            if r.is_ok() {
                store.info("Profile added");
                adding.set(false);
            }
        });
    };
    let import = move || {
        let svc = store.svc();
        spawn(async move {
            let picked = pick_paths(
                OpenDialog::files().filter(FileFilter::new("Clash / mihomo config", ["yaml", "yml", "txt"])).title("Import a Profile"),
            )
            .await;
            match picked {
                Ok(Some(paths)) => {
                    if let Some(path) = paths.into_iter().next() {
                        store.run_then(svc.add_file(path), move |r| {
                            if r.is_ok() {
                                store.info("Profile imported");
                                adding.set(false);
                            }
                        });
                    }
                }
                Ok(None) => {}
                Err(e) => store.error(format!("No file dialog: {e}")),
            }
        });
    };
    ui::card()
        .p_4()
        .gap_3()
        .child(ui::section_label("New Profile"))
        .child(
            view()
                .gap_1()
                .child(ui::muted("Subscription URL"))
                .child(ui::field(url, "https://example.com/subscribe?token=…").on_submit(move |_: &str| submit())),
        )
        .child(
            view()
                .gap_1()
                .child(ui::muted("Name (optional; the provider's name is used otherwise)"))
                .child(ui::field(name, "My Provider").w(px(320.0)).on_submit(move |_: &str| submit())),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .mt_1()
                .child(ui::btn_icon(Icon::File, "Import File…", Variant::Ghost).on_press(import))
                .child(view().flex_1())
                .child(ui::muted(move || -> SharedString { if busy.get() { "Downloading…".into() } else { "".into() } }))
                .child(ui::btn("Cancel", Variant::Secondary).on_press(move || adding.set(false)))
                .child(ui::btn("Add", Variant::Primary).disabled(move || !valid() || busy.get()).on_press(submit)),
        )
}

fn source_text(p: &Profile) -> String {
    match &p.source {
        Source::Remote { url, interval_hours } => {
            let host = url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| url.clone());
            match interval_hours {
                0 => format!("{host} · updated by hand"),
                h => format!("{host} · every {h} h"),
            }
        }
        Source::Local { .. } => "Local file".to_owned(),
    }
}

pub fn ago(at: Option<i64>) -> String {
    let Some(at) = at else { return "never updated".into() };
    let ago = kairo_core::profile::now() - at;
    match ago {
        i64::MIN..=59 => "updated just now".into(),
        60..=3599 => format!("updated {} min ago", ago / 60),
        3600..=86399 => format!("updated {} h ago", ago / 3600),
        _ => format!("updated {} d ago", ago / 86400),
    }
}

fn stats_text(p: &Profile) -> String {
    let s = &p.stats;
    let mut parts = vec![format!("{} proxies", s.proxies)];
    if s.providers > 0 {
        parts.push(format!("{} providers", s.providers));
    }
    parts.push(format!("{} groups", s.groups));
    parts.push(format!("{} rules", s.rules));
    parts.join(" · ")
}

fn profile_row(p: ReadSignal<Profile>, editing: Signal<Option<String>>) -> impl IntoElement {
    let store = store();
    let id = p.get_untracked().id.clone();
    let active = {
        let id = id.clone();
        let m = computed(move || store.state.with(|s| s.profiles.active.as_deref() == Some(id.as_str())));
        move || m.get()
    };
    let updating = {
        let id = id.clone();
        let m = computed(move || store.state.with(|s| s.updating.contains(&id)));
        move || m.get()
    };
    let remote = move || p.with(|p| matches!(p.source, Source::Remote { .. }));
    let id_activate = id.clone();
    let id_update = id.clone();
    let id_edit = id.clone();
    let id_delete = id.clone();
    button()
        .selected(active)
        .accessibility_label(move || -> SharedString { p.with(|p| p.name.clone()).into() })
        .on_press(move || {
            if !active() {
                store.run(store.svc().activate(id_activate.clone()));
            }
        })
        .flex_row()
        .items_start()
        .gap_3()
        .px_4()
        .py_3()
        .cursor_pointer()
        .hover(|s| s.bg(move || theme().accent))
        .child(
            view().w(px(16.0)).pt(px(2.0)).items_center().child(
                view()
                    .size(px(14.0))
                    .rounded_full()
                    .border(px(1.5))
                    .border_color(move || if active() { theme().primary } else { theme().input })
                    .items_center()
                    .justify_center()
                    .child(view().size(px(6.0)).rounded_full().bg(move || theme().primary).style_when(move || !active(), |s| s.invisible())),
            ),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap_1()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(text(move || -> SharedString { p.with(|p| p.name.clone()).into() }).text_size(px(size::body())).font_weight(FontWeight::SEMIBOLD).number_of_lines(1))
                        .child(when(active, || ui::badge("Active"))),
                )
                .child(ui::muted(move || -> SharedString {
                    p.with(|p| format!("{} · {}", source_text(p), if updating() { "updating…".to_owned() } else { ago(p.updated_at) })).into()
                }))
                .child(when(
                    move || p.with(|p| p.usage.is_some()),
                    move || {
                        view()
                            .gap_1()
                            .pt_1()
                            .max_w(px(420.0))
                            .child(ui::meter(move || p.with(|p| p.usage.as_ref().and_then(|u| u.fraction()).unwrap_or(0.0) as f32)))
                            .child(ui::muted(move || -> SharedString {
                                p.with(|p| {
                                    let Some(u) = &p.usage else { return String::new() };
                                    let used = kairo_core::bytes(u.used());
                                    let amount = if u.total > 0 { format!("{used} of {}", kairo_core::bytes(u.total)) } else { format!("{used} used") };
                                    format!("{amount} · {}", expiry_text(u.expire))
                                })
                                .into()
                            }))
                    },
                ))
                .child(ui::muted(move || -> SharedString { p.with(stats_text).into() }))
                .child(when(
                    move || p.with(|p| p.last_error.is_some()),
                    move || {
                        text(move || -> SharedString { p.with(|p| p.last_error.clone().unwrap_or_default()).into() })
                            .text_size(px(size::small()))
                            .text_color(move || theme().destructive)
                            .number_of_lines(2)
                    },
                )),
        )
        .child(
            view()
                .flex_row()
                .gap_0p5()
                .child(when(remote, move || {
                    let id = id_update.clone();
                    ui::icon_btn(Icon::Refresh, "Update").disabled(updating).on_press(move || store.run(store.svc().update_profile(id.clone())))
                }))
                .child(ui::icon_btn(Icon::Edit, "Edit").on_press(move || editing.set(Some(id_edit.clone()))))
                .child(ui::icon_btn(Icon::Trash, "Delete").on_press(move || {
                    let id = id_delete.clone();
                    let name = p.with_untracked(|p| p.name.clone());
                    spawn(async move {
                        let answer = alert_dialog()
                            .title(format!("Delete “{name}”?"))
                            .description("Its downloaded configuration and overrides are removed.")
                            .destructive("delete", "Delete")
                            .cancel("cancel", "Cancel")
                            .show()
                            .await;
                        if answer.is_ok_and(|a| a.action.as_deref() == Some("delete")) {
                            store.run(store.svc().delete_profile(id));
                        }
                    });
                })),
        )
}

// --- One profile: details and overrides ----------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overrides,
    Profile,
}

const OVERRIDE_HELP: &str = "YAML merged over the profile each time it loads: keys here replace the profile's, maps merge, \
and prepend-rules / append-rules, prepend-proxies / append-proxies and prepend-proxy-groups / append-proxy-groups add to those lists. \
Ports, the controller and TUN always come from Settings.";

const OVERRIDE_EXAMPLE: &str = "# Example\n# prepend-rules:\n#   - DOMAIN-SUFFIX,example.com,DIRECT\n# dns:\n#   enable: true\n";

fn detail(id: String, editing: Signal<Option<String>>) -> impl IntoElement {
    let store = store();
    let profile = {
        let id = id.clone();
        computed(move || store.state.with(|s| s.profiles.profiles.iter().find(|p| p.id == id).cloned()))
    };
    let initial = untracked(|| profile.get()).unwrap_or_else(|| Profile {
        id: id.clone(),
        name: String::new(),
        source: Source::Local { origin: None },
        updated_at: None,
        usage: None,
        web_page: None,
        last_error: None,
        failed_at: None,
        stats: Default::default(),
    });
    let (url0, hours0) = match &initial.source {
        Source::Remote { url, interval_hours } => (Some(url.clone()), *interval_hours),
        Source::Local { .. } => (None, 0),
    };
    let remote = url0.is_some();
    let name = signal(initial.name.clone());
    let url = signal(url0.clone().unwrap_or_default());
    let hours = signal(hours0.to_string());
    let details_changed = move || {
        profile.with(|p| {
            let Some(p) = p else { return false };
            let (u0, h0) = match &p.source {
                Source::Remote { url, interval_hours } => (url.clone(), *interval_hours),
                Source::Local { .. } => (String::new(), 0),
            };
            name.with(|n| n.trim() != p.name)
                || (remote && (url.with(|u| u.trim() != u0) || hours.with(|h| h.trim() != h0.to_string())))
        })
    };
    let details_valid = move || {
        name.with(|n| !n.trim().is_empty())
            && (!remote || (url.with(|u| u.trim().starts_with("http")) && hours.with(|h| h.trim().parse::<u32>().is_ok())))
    };

    let svc = store.svc();
    let saved = signal(svc.override_text(&id));
    let draft = signal(saved.get_untracked());
    let profile_text = svc.profile_text(&id);
    let tab = signal(Tab::Overrides);
    let checking = signal(false);
    let status = signal(None::<(bool, String)>);
    let dirty = move || draft.with(|d| saved.with(|s| d != s));

    let check = {
        let profile_text = profile_text.clone();
        move || {
            checking.set(true);
            status.set(None);
            store.run_then(store.svc().check_config(profile_text.clone(), draft.get_untracked()), move |r| {
                checking.set(false);
                status.set(Some(match r {
                    Ok(()) => (true, "The core accepts this configuration.".to_owned()),
                    Err(e) => (false, e),
                }));
            });
        }
    };
    let save = {
        let id = id.clone();
        move || {
            checking.set(true);
            status.set(None);
            let text = draft.get_untracked();
            store.run_then(store.svc().save_override(id.clone(), text.clone()), move |r| {
                checking.set(false);
                match r {
                    Ok(()) => {
                        saved.set(text);
                        status.set(Some((true, "Saved and applied.".to_owned())));
                    }
                    Err(e) => status.set(Some((false, e))),
                }
            });
        }
    };
    let save_details = {
        let id = id.clone();
        move || {
            let svc = store.svc();
            svc.rename_profile(
                id.clone(),
                name.get_untracked().trim().to_owned(),
                remote.then(|| hours.get_untracked().trim().parse().unwrap_or(0)),
                remote.then(|| url.get_untracked().trim().to_owned()),
            );
            store.info("Profile details saved");
        }
    };

    view()
        .flex_1()
        .child(
            view()
                .h(px(ui::HEADER_HEIGHT))
                .flex_shrink_0()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .border_b_1()
                .border_color(move || theme().border)
                .window_drag(true)
                .child(
                    button()
                        .accessibility_label("Back to Profiles")
                        .on_press(move || editing.set(None))
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .h(px(28.0))
                        .px_2()
                        .rounded(ui::radius_control())
                        .cursor_pointer()
                        .text_color(move || theme().primary)
                        .hover(|s| s.bg(move || theme().accent))
                        .child(icon(Icon::ChevronLeft, 14.0))
                        .child(text("Profiles").text_size(px(size::body())).font_weight(FontWeight::MEDIUM)),
                )
                .child(
                    ui::title(move || -> SharedString { profile.with(|p| p.as_ref().map(|p| p.name.clone()).unwrap_or_default()).into() })
                        .text_size(px(size::large() + 2.0))
                        .flex_1()
                        .number_of_lines(1),
                ),
        )
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .p_5()
                .gap_4()
                .child(ui::group(
                    Some("Details"),
                    {
                        let mut rows = vec![ui::setting_row("Name", None, ui::field(name, "Name").w(px(320.0))).into_element()];
                        if remote {
                            rows.push(ui::setting_row("Subscription URL", None, ui::field(url, "https://…").w(px(420.0))).into_element());
                            rows.push(
                                ui::setting_row(
                                    "Update every",
                                    Some("Hours between automatic updates; 0 updates only by hand."),
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(ui::field(hours, "24").w(px(72.0)).text_right())
                                        .child(ui::muted("hours")),
                                )
                                .into_element(),
                            );
                        }
                        rows.push(
                            view()
                                .flex_row()
                                .justify_end()
                                .px_4()
                                .py_2()
                                .child(ui::btn("Save Details", Variant::Secondary).disabled(move || !details_changed() || !details_valid()).on_press(save_details))
                                .into_element(),
                        );
                        rows
                    },
                ))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(ui::segmented(vec![(Tab::Overrides, "Overrides"), (Tab::Profile, "Profile")], move || tab.get(), move |t| tab.set(t)))
                        .child(view().flex_1())
                        .child(when(
                            move || tab.get() == Tab::Overrides,
                            move || {
                                let check = check.clone();
                                let save = save.clone();
                                view()
                                    .flex_row()
                                    .gap_2()
                                    .child(ui::btn_icon(Icon::Check, "Check", Variant::Secondary).disabled(move || checking.get()).on_press(check))
                                    .child(ui::btn("Save", Variant::Primary).disabled(move || !dirty() || checking.get()).on_press(save))
                            },
                        )),
                )
                .child(ui::muted(move || -> SharedString {
                    match tab.get() {
                        Tab::Overrides => OVERRIDE_HELP.into(),
                        Tab::Profile => "The configuration as the provider sent it (read only). Kairo never edits it; updates replace it.".into(),
                    }
                }))
                .child(when(
                    move || status.with(Option::is_some) && tab.get() == Tab::Overrides,
                    move || {
                        let ok = move || status.with(|s| s.as_ref().is_some_and(|s| s.0));
                        view()
                            .flex_row()
                            .items_start()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded(ui::radius_control())
                            .bg(move || ui::subtle())
                            .border_1()
                            .border_color(move || theme().border)
                            .child(ui::dot(move || if ok() { ui::success() } else { theme().destructive }).mt(px(4.0)))
                            .child(
                                text(move || -> SharedString { status.with(|s| s.as_ref().map(|s| s.1.clone()).unwrap_or_default()).into() })
                                    .text_size(px(size::small()))
                                    .selectable(true)
                                    .flex_1(),
                            )
                    },
                ))
                .child(
                    ui::card().flex_1().min_h(px(200.0)).child(dynamic(move || {
                        let t = tab.get();
                        let profile_text = profile_text.clone();
                        untracked(move || match t {
                            Tab::Overrides => editor()
                                .default_value(if draft.get_untracked().trim().is_empty() { OVERRIDE_EXAMPLE.to_owned() } else { draft.get_untracked() })
                                .language("yaml")
                                .line_numbers(true)
                                .font_size(px(12.5))
                                .accessibility_label("Overrides")
                                .on_change(move |v: &SharedString| draft.set(v.to_string()))
                                .flex_1()
                                .into_element(),
                            Tab::Profile => editor()
                                .default_value(profile_text)
                                .language("yaml")
                                .line_numbers(true)
                                .read_only(true)
                                .font_size(px(12.5))
                                .accessibility_label("Profile")
                                .flex_1()
                                .into_element(),
                        })
                    })),
                ),
        )
}
