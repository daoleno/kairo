//! The service's Downloads exit: keeping the hidden group on a pinned node,
//! the fastest one, or the Location, and the mirror test. As with AI, moving
//! the group never closes a connection: new downloads go to the new exit,
//! open ones finish where they began.

use std::future::Future;
use std::sync::atomic::Ordering;

use super::{Service, unix};
use crate::ai::{self, AiExit, ExitState};
use crate::downloads::{self, MirrorTest};
use crate::places;
use serde_yaml::Mapping;

impl Service {
    /// The group, once the config has it.
    fn downloads_group(&self) -> Option<String> {
        let g = self.0.state.borrow().downloads.group.clone();
        (!g.is_empty()).then_some(g)
    }

    /// The node downloads go through now.
    pub fn downloads_node(&self) -> Option<String> {
        let group = self.downloads_group()?;
        ai::resolve(&self.0.proxies.borrow().groups, &group)
    }

    /// Keeps `State.downloads.node` (and the group name) up to date; wakes
    /// the UI only when it changed.
    pub(super) fn downloads_node_changed(&self) {
        let node = self.downloads_node();
        self.0.state.send_if_modified(|s| {
            if s.downloads.node == node {
                return false;
            }
            s.downloads.node = node;
            true
        });
    }

    /// Copies the settings' switches into the view.
    pub(super) fn downloads_view_sync(&self) {
        self.0.state.send_if_modified(|s| {
            let (fastest, mirror) = (s.settings.downloads.fastest, s.settings.downloads.mirror);
            if s.downloads.fastest == fastest && s.downloads.mirror == mirror {
                return false;
            }
            s.downloads.fastest = fastest;
            s.downloads.mirror = mirror;
            true
        });
    }

    /// The config was written: note the group, and whether the pinned
    /// exit is still in the subscription (renamed: followed; gone: the
    /// group goes back to following the fastest or the Location, and the
    /// person is told).
    pub(super) fn downloads_config_loaded(&self, mapping: &Mapping, group: Option<String>) {
        let group = group.unwrap_or_default();
        self.modify(|s| s.downloads.group = group.clone());
        self.downloads_view_sync();
        if group.is_empty() {
            return;
        }
        let exit = self.settings().downloads.exit;
        match ai::exit_state(mapping, exit.as_ref()) {
            ExitState::Same | ExitState::Unpinned => {}
            ExitState::Renamed(name) => {
                self.modify(|s| {
                    if let Some(e) = &mut s.settings.downloads.exit {
                        e.name = name;
                    }
                });
                self.save_settings();
            }
            ExitState::Gone => {
                let was = exit.map(|e| e.name).unwrap_or_default();
                self.modify(|s| s.settings.downloads.exit = None);
                self.save_settings();
                self.notify(
                    format!(
                        "The server {was} you chose for downloads is no longer in the subscription. Downloads follow the {} now.",
                        if self.settings().downloads.fastest {
                            "fastest server"
                        } else {
                            "Location"
                        }
                    ),
                    false,
                );
            }
        }
    }

    /// Makes the Downloads group go where it should: the pinned node, else
    /// the fastest (measured by the same tests as the Location's), else the
    /// Location. `retest` measures the places first; without it they are
    /// measured only when nothing is known yet.
    pub(super) async fn downloads_sync(&self, retest: bool) {
        if !self.running() {
            return;
        }
        let Some(group) = self.downloads_group() else {
            return;
        };
        let Some(main) = self.main_group() else {
            return;
        };
        let d = self.settings().downloads;
        if d.exit.is_none() && d.fastest {
            let known = places::places(&self.0.proxies.borrow().groups, &main)
                .iter()
                .any(|p| p.delay.is_some_and(|x| x > 0));
            // Nothing goes to the group without its rules (and no route of
            // the person's names it): no tests on its account.
            let used = self.settings().ai.downloads
                || self
                    .settings()
                    .routes
                    .iter()
                    .any(|r| r.exit == crate::routes::RouteExit::Downloads);
            if used && (retest || !known) {
                let _ = self.test_places().await;
            }
        }
        let (want, now) = {
            let p = self.0.proxies.borrow();
            // The main group may have changed under the tests.
            let main = places::main_group(&p.groups, &self.settings().tray_group)
                .cloned()
                .unwrap_or(main);
            (
                downloads::want(
                    &p.groups,
                    &group,
                    &main,
                    d.exit.as_ref().map(|e| e.name.as_str()),
                    d.fastest,
                ),
                p.groups
                    .iter()
                    .find(|g| g.name == group)
                    .and_then(|g| g.now.clone()),
            )
        };
        if let Some(want) = want
            && now.as_deref() != Some(want.as_str())
        {
            // Straight to the core: no closing of what is open.
            if let Err(e) = self.controller().select(&group, &want).await {
                log::warn!("Downloads exit: {e}");
            }
            self.refresh_proxies_now().await;
        }
        self.downloads_node_changed();
    }

    /// Pins `name` (a node the Downloads group lists) as the exit for
    /// downloads; new downloads go there, open ones finish where they are.
    pub fn set_downloads_exit(
        &self,
        name: String,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let Some(group) = svc.downloads_group() else {
                return Err("Downloads have no exit of their own while the AI layer is off".into());
            };
            let listed = svc
                .0
                .proxies
                .borrow()
                .groups
                .iter()
                .find(|g| g.name == group)
                .is_some_and(|g| g.members.iter().any(|m| m.name == name && !m.is_group));
            if !listed {
                return Err(format!(
                    "{name} is not one of the Downloads group's choices"
                ));
            }
            let fingerprint = svc
                .0
                .ai_cfg
                .lock()
                .expect("ai cfg")
                .as_ref()
                .and_then(|c| ai::fingerprint(c, &name))
                .ok_or_else(|| format!("{name} is not a server of this subscription"))?;
            svc.modify(|s| {
                s.settings.downloads.exit = Some(AiExit {
                    name,
                    fingerprint,
                    since: unix(),
                });
                s.settings.downloads.fastest = false;
                // The mirror was measured through another exit.
                s.downloads.mirror_test = MirrorTest::Untested;
            });
            svc.save_settings();
            svc.downloads_view_sync();
            svc.downloads_sync(false).await;
            Ok(())
        })
    }

    /// Downloads follow the fastest server from now on (nothing pinned).
    pub fn downloads_follow_fastest(&self) -> impl Future<Output = Result<(), String>> + 'static {
        self.downloads_follow(true)
    }

    /// Downloads follow the Location from now on (nothing pinned, no
    /// fastest).
    pub fn downloads_follow_location(&self) -> impl Future<Output = Result<(), String>> + 'static {
        self.downloads_follow(false)
    }

    fn downloads_follow(
        &self,
        fastest: bool,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.modify(|s| {
                s.settings.downloads.exit = None;
                s.settings.downloads.fastest = fastest;
                s.downloads.mirror_test = MirrorTest::Untested;
            });
            svc.save_settings();
            svc.downloads_view_sync();
            // Measured now: the person just asked for the fastest.
            svc.downloads_sync(fastest).await;
            Ok(())
        })
    }

    /// Measures the mirror against Hugging Face, both through the Downloads
    /// exit, and keeps the result in `State.downloads.mirror_test`. Nothing
    /// is changed on this computer.
    pub fn test_mirror(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            if !svc.running() {
                return Err("Connect first".into());
            }
            if svc.downloads_group().is_none() {
                return Err("Downloads have no exit of their own while the AI layer is off".into());
            }
            // Without its rules the test would measure whatever way the
            // subscription sends these sites, not the Downloads exit.
            if !svc.settings().ai.downloads {
                return Err(
                    "Downloads are not sent to their own exit while that switch is off".into(),
                );
            }
            if svc.0.mirror_busy.swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            svc.modify(|s| s.downloads.mirror_test = MirrorTest::Running);
            let port = svc.settings().mixed_port;
            // One after the other: two downloads at once would share the line.
            let mirror = downloads::measure(port, downloads::MIRROR_URL).await;
            let origin = downloads::measure(port, downloads::ORIGIN_URL).await;
            let result = downloads::judge(mirror, origin);
            svc.modify(|s| s.downloads.mirror_test = result);
            svc.0.mirror_busy.store(false, Ordering::SeqCst);
            Ok(())
        })
    }

    /// Turns the mirror on or off (a setting: the UI then offers
    /// [`downloads::mirror_env_lines`] to copy). Refused unless the last
    /// test passed.
    pub fn set_mirror(&self, on: bool) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            if on && !svc.0.state.borrow().downloads.mirror_test.passed() {
                return Err(downloads::MIRROR_REFUSED.into());
            }
            if svc.settings().downloads.mirror != on {
                svc.modify(|s| s.settings.downloads.mirror = on);
                svc.save_settings();
                svc.downloads_view_sync();
            }
            Ok(())
        })
    }
}
