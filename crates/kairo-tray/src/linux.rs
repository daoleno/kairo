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
            .map(|size| ksni::Icon { width: size, height: size, data: crate::icon_argb(size as u32, !self.model.running) })
            .collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "Kairo".into(), description: self.model.status.clone(), ..Default::default() }
    }

    /// A click on the icon shows the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        (self.hooks.show)();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let m = &self.model;
        let mut items: Vec<MenuItem<Self>> = vec![
            StandardItem { label: m.status.clone(), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            RadioGroup {
                selected: Mode::ALL.iter().position(|x| *x == m.mode).unwrap_or(0),
                select: Box::new(|t: &mut Self, i| act::mode(&t.svc, Mode::ALL[i])),
                options: Mode::ALL.iter().map(|x| RadioItem { label: format!("{} Mode", x.title()), ..Default::default() }).collect(),
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "System Proxy".into(),
                checked: m.sysproxy,
                enabled: m.sysproxy_supported,
                activate: Box::new(|t: &mut Self| act::sysproxy(&t.svc, !t.model.sysproxy)),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "TUN Mode".into(),
                checked: m.tun,
                enabled: m.tun_allowed || m.tun,
                activate: Box::new(|t: &mut Self| act::tun(&t.svc, !t.model.tun)),
                ..Default::default()
            }
            .into(),
        ];
        if let Some((group, choices, now)) = &m.group {
            items.push(MenuItem::Separator);
            let selected = now.as_ref().and_then(|n| choices.iter().position(|c| &c.name == n)).unwrap_or(usize::MAX);
            let g = group.clone();
            items.push(
                SubMenu {
                    label: format!("{group}: {}", now.clone().unwrap_or_default()),
                    enabled: m.running,
                    submenu: vec![
                        RadioGroup {
                            selected,
                            select: Box::new(move |t: &mut Self, i| {
                                if let Some((_, choices, _)) = &t.model.group
                                    && let Some(c) = choices.get(i)
                                {
                                    act::select(&t.svc, g.clone(), c.name.clone());
                                }
                            }),
                            options: choices.iter().map(|c| RadioItem { label: Model::choice_label(c).replace('_', "__"), ..Default::default() }).collect(),
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
            StandardItem {
                label: if m.running { "Stop Core".into() } else { "Start Core".into() },
                enabled: true,
                activate: Box::new(|t: &mut Self| act::toggle_core(&t.svc, t.model.running)),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Show Kairo".into(),
                activate: Box::new(|t: &mut Self| (t.hooks.show)()),
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Quit Kairo".into(),
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
    let tray = KairoTray { model: Model::of(&svc), svc: svc.clone(), hooks };
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
            if handle.update(|t| {
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
    rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "the tray did not start".to_string())?
}
