//! The StatusNotifierItem.

use std::time::Duration;

use kairo_core::{Mode, Service};
use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, TrayMethods};

use crate::{ExitId, Hooks, Model, act};

struct KairoTray {
    svc: Service,
    hooks: Hooks,
    model: Model,
}

impl ksni::Tray for KairoTray {
    fn id(&self) -> String {
        "dev.kairo.app".into()
    }

    fn title(&self) -> String {
        "Kairo".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [22, 32, 48]
            .into_iter()
            .map(|size| ksni::Icon {
                width: size,
                height: size,
                data: crate::icon_argb(size as u32, !self.model.running),
            })
            .collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Kairo".into(),
            description: self.model.status.clone(),
            ..Default::default()
        }
    }

    /// A click on the icon shows the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        (self.hooks.show)();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let m = &self.model;
        let mut items: Vec<MenuItem<Self>> = vec![
            CheckmarkItem {
                label: if m.running {
                    "Connected".into()
                } else {
                    m.status.clone()
                },
                checked: m.running,
                enabled: m.has_profile,
                activate: Box::new(|t: &mut Self| act::connect(&t.svc, !t.model.running)),
                ..Default::default()
            }
            .into(),
        ];
        if !m.ai.is_empty() {
            items.push(
                StandardItem {
                    label: m.ai.clone().replace('_', "__"),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(MenuItem::Separator);
        for e in &m.exits {
            let id = e.id;
            let mut sub: Vec<MenuItem<Self>> = Vec::new();
            if let Some(on) = e.fastest {
                sub.push(
                    CheckmarkItem {
                        label: "Fastest".into(),
                        checked: on,
                        activate: Box::new(|t: &mut Self| act::locate(&t.svc, String::new())),
                        ..Default::default()
                    }
                    .into(),
                );
                sub.push(MenuItem::Separator);
            }
            let selected = e
                .choices
                .iter()
                .position(|c| c.checked)
                .unwrap_or(usize::MAX);
            sub.push(
                RadioGroup {
                    selected,
                    select: Box::new(move |t: &mut Self, i| {
                        let name = t
                            .model
                            .exits
                            .iter()
                            .find(|e| e.id == id)
                            .and_then(|e| e.choices.get(i))
                            .map(|c| c.name.clone());
                        if let Some(name) = name {
                            match id {
                                ExitId::Rest => act::locate(&t.svc, name),
                                ExitId::Ai => act::ai_exit(&t.svc, name),
                            }
                        }
                    }),
                    options: e
                        .choices
                        .iter()
                        .map(|c| RadioItem {
                            label: Model::choice_label(c).replace('_', "__"),
                            ..Default::default()
                        })
                        .collect(),
                }
                .into(),
            );
            sub.push(MenuItem::Separator);
            sub.push(
                StandardItem {
                    label: "All Servers…".into(),
                    activate: Box::new(|t: &mut Self| (t.hooks.page)("exits")),
                    ..Default::default()
                }
                .into(),
            );
            sub.push(
                StandardItem {
                    label: "Test Again".into(),
                    activate: Box::new(|t: &mut Self| act::test_again(&t.svc)),
                    ..Default::default()
                }
                .into(),
            );
            items.push(
                SubMenu {
                    label: format!("{}: {}", e.title, e.place).replace('_', "__"),
                    submenu: sub,
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(MenuItem::Separator);
        items.push(
            RadioGroup {
                selected: Mode::ALL.iter().position(|x| *x == m.mode).unwrap_or(0),
                select: Box::new(|t: &mut Self, i| act::mode(&t.svc, Mode::ALL[i])),
                options: Mode::ALL
                    .iter()
                    .map(|x| RadioItem {
                        label: format!("Mode: {}", Model::route_title(*x)),
                        ..Default::default()
                    })
                    .collect(),
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Open Kairo".into(),
                icon_name: "window-new-symbolic".into(),
                activate: Box::new(|t: &mut Self| (t.hooks.show)()),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Settings…".into(),
                icon_name: "emblem-system-symbolic".into(),
                activate: Box::new(|t: &mut Self| (t.hooks.page)("settings")),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Quit Kairo".into(),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(|t: &mut Self| (t.hooks.quit)()),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

pub fn start(svc: Service, hooks: Hooks) -> Result<(), String> {
    let rt = svc.runtime();
    let (tx, rx) = std::sync::mpsc::channel();
    let tray = KairoTray {
        model: Model::of(&svc),
        svc: svc.clone(),
        hooks,
    };
    rt.spawn(async move {
        let handle = match tray.spawn().await {
            Ok(h) => {
                let _ = tx.send(Ok(()));
                h
            }
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
                return;
            }
        };
        let (mut state, mut live, mut proxies) = (svc.state(), svc.live(), svc.proxies());
        loop {
            tokio::select! {
                r = state.changed() => if r.is_err() { break },
                r = live.changed() => if r.is_err() { break },
                r = proxies.changed() => if r.is_err() { break },
            }
            // Coalesce bursts (a delay test updates many nodes).
            tokio::time::sleep(Duration::from_millis(150)).await;
            let next = Model::of(&svc);
            if handle
                .update(|t| {
                    if t.model != next {
                        t.model = next;
                    }
                })
                .await
                .is_none()
            {
                break;
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the tray did not start".to_string())?
}
