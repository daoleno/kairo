//! The StatusNotifierItem.

use std::time::Duration;

use kairo_core::{Mode, Service};
use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, TrayMethods};

use crate::{Hooks, Model, act};

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
            StandardItem {
                label: m.status.clone(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Connected".into(),
                checked: m.running,
                enabled: m.has_profile,
                activate: Box::new(|t: &mut Self| act::connect(&t.svc, !t.model.running)),
                ..Default::default()
            }
            .into(),
        ];
        if let Some((_, choices, now)) = &m.group {
            let selected = now
                .as_ref()
                .and_then(|n| choices.iter().position(|c| &c.name == n))
                .unwrap_or(usize::MAX);
            items.push(
                SubMenu {
                    label: format!("Location: {}", m.place).replace('_', "__"),
                    icon_name: "find-location-symbolic".into(),
                    submenu: vec![
                        RadioGroup {
                            selected,
                            select: Box::new(move |t: &mut Self, i| {
                                if let Some((_, choices, _)) = &t.model.group
                                    && let Some(c) = choices.get(i)
                                {
                                    act::locate(&t.svc, c.name.clone());
                                }
                            }),
                            options: choices
                                .iter()
                                .map(|c| RadioItem {
                                    label: Model::choice_label(c).replace('_', "__"),
                                    ..Default::default()
                                })
                                .collect(),
                        }
                        .into(),
                    ],
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
                        label: Model::route_title(*x).into(),
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
