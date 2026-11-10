//! The service's AI operations: keeping AI on its exit, the checks every ten
//! minutes, the alert when the pinned exit goes away, the tally of tools.
//! None of it closes a connection: choosing an exit moves new connections, and
//! open ones finish where they began (mihomo keeps them on their node).

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::{AiAlert, Service, unix};
use crate::activity::Tally;
use crate::ai::{self, AiExit, AiSettings, Applied};
use crate::ai_check;
use crate::places;

/// The quick checks run at most this often, unless asked for.
const MIN_GAP: i64 = 60;
/// The 90 s hold runs at most this often, unless asked for.
const HOLD_EVERY: i64 = 3600;
const HOLD_SECS: u64 = 90;

impl Service {
    /// The tools and services seen on the wire, counted since Kairo opened.
    pub fn activity(&self) -> tokio::sync::watch::Receiver<Arc<Tally>> {
        self.0.activity.subscribe()
    }

    pub fn clear_activity(&self) {
        self.0.activity.send_modify(|a| Arc::make_mut(a).clear());
    }

    /// The config was written: note where AI goes in it, and whether the
    /// pinned exit is still in the subscription.
    pub(super) fn ai_config_loaded(&self, built: &str, applied: Option<Applied>) {
        let mapping = crate::config::parse_mapping(built).ok();
        let group = applied
            .as_ref()
            .map(|a| a.group.clone())
            .unwrap_or_default();
        let adopted = applied.as_ref().is_some_and(|a| a.adopted);
        let downloads = applied.as_ref().and_then(|a| a.downloads.clone());
        self.modify(|s| {
            s.ai.group = group;
            s.ai.adopted = adopted;
        });
        let Some(mapping) = mapping else { return };
        self.downloads_config_loaded(&mapping, downloads);
        let ai = self.settings().ai;
        if ai.enabled {
            match ai::exit_state(&mapping, ai.exit.as_ref()) {
                ai::ExitState::Same | ai::ExitState::Unpinned => {}
                ai::ExitState::Renamed(name) => {
                    // The same server under a new name: still the same exit.
                    self.modify(|s| {
                        if let Some(e) = &mut s.settings.ai.exit {
                            e.name = name;
                        }
                    });
                    self.save_settings();
                }
                ai::ExitState::Gone => {
                    // Never pick another for the person: AI follows the
                    // Location, and the alert says so.
                    let was = ai.exit.as_ref().map(|e| e.name.clone()).unwrap_or_default();
                    self.modify(|s| {
                        s.settings.ai.exit = None;
                        s.settings.ai.follow = true;
                        s.ai.alert = Some(AiAlert::ExitGone { was });
                    });
                    self.save_settings();
                }
            }
        }
        *self.0.ai_cfg.lock().expect("ai cfg") = Some(mapping);
    }

    fn ai_group(&self) -> Option<(String, String)> {
        let group = self.0.state.borrow().ai.group.clone();
        if group.is_empty() {
            return None;
        }
        Some((group, self.main_group()?.name))
    }

    /// Makes the AI group go where it should: the pinned exit, else the
    /// Location. With nothing pinned (and not told to follow) it pins the
    /// node the Location is on, once a real one is chosen.
    pub(super) async fn ai_sync(&self) {
        let ai = self.settings().ai;
        if !ai.enabled || !self.running() {
            return;
        }
        let Some((group, main)) = self.ai_group() else {
            return;
        };
        if ai.exit.is_none() && !ai.follow {
            if self.0.state.borrow().ai.adopted {
                // The subscription's own AI group already goes somewhere
                // (the person may have chosen it): that is the exit.
                // Never move it for them.
                self.ai_pin_adopted(&group);
            } else {
                self.ai_pin_current();
            }
        }
        let ai = self.settings().ai;
        if ai.exit.is_none() && !ai.follow {
            // Adopted, and it sits on something that is not one node: leave it.
            return;
        }
        let want = {
            let p = self.0.proxies.borrow();
            ai::want(
                &p.groups,
                &group,
                &main,
                ai.exit.as_ref().map(|e| e.name.as_str()),
            )
        };
        let now = self
            .0
            .proxies
            .borrow()
            .groups
            .iter()
            .find(|g| g.name == group)
            .and_then(|g| g.now.clone());
        if let Some(want) = want
            && now.as_deref() != Some(want.as_str())
        {
            // Straight to the core: no closing of what is open.
            if let Err(e) = self.controller().select(&group, &want).await {
                log::warn!("AI exit: {e}");
            }
            self.refresh_proxies_now().await;
        }
    }

    /// Pins the node the adopted AI group goes through now (its own choice,
    /// or the Location it follows), when that node is one of its members:
    /// the exit stays where traffic already goes, and from then on stays
    /// there when the Location changes.
    fn ai_pin_adopted(&self, group: &str) {
        let name = {
            let p = self.0.proxies.borrow();
            ai::resolve(&p.groups, group).filter(|n| {
                p.groups
                    .iter()
                    .find(|g| g.name == group)
                    .is_some_and(|g| g.members.iter().any(|m| m.name == *n && !m.is_group))
            })
        };
        let Some(name) = name else { return };
        if places::is_note(&name) || places::is_builtin(&name) {
            return;
        }
        let fingerprint = self
            .0
            .ai_cfg
            .lock()
            .expect("ai cfg")
            .as_ref()
            .and_then(|c| ai::fingerprint(c, &name));
        let Some(fingerprint) = fingerprint else {
            return;
        };
        self.modify(|s| {
            s.settings.ai.exit = Some(AiExit {
                name,
                fingerprint,
                since: unix(),
            });
            s.settings.ai.follow = false;
        });
        self.save_settings();
    }

    /// Pins the node the Location is on now, if it is a real place.
    fn ai_pin_current(&self) {
        let place = {
            let p = self.0.proxies.borrow();
            self.main_group()
                .and_then(|m| places::current(&p.groups, &m))
        };
        let Some(place) = place else { return };
        if places::is_note(&place.name) {
            return;
        }
        let fingerprint = self
            .0
            .ai_cfg
            .lock()
            .expect("ai cfg")
            .as_ref()
            .and_then(|c| ai::fingerprint(c, &place.name));
        let Some(fingerprint) = fingerprint else {
            return;
        };
        self.modify(|s| {
            s.settings.ai.exit = Some(AiExit {
                name: place.name.clone(),
                fingerprint,
                since: unix(),
            });
            s.settings.ai.follow = false;
        });
        self.save_settings();
    }

    /// The node AI traffic goes through now.
    pub fn ai_node(&self) -> Option<String> {
        let (group, _) = self.ai_group()?;
        ai::resolve(&self.0.proxies.borrow().groups, &group)
    }

    /// Pins `name` (a node the AI group lists) as the AI exit. New AI
    /// connections go there; open ones finish where they are.
    pub fn set_ai_exit(&self, name: String) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.stats_manual(0);
        self.run(async move {
            let Some((group, _)) = svc.ai_group() else {
                return Err("This subscription has no AI group to pin".into());
            };
            let listed = svc
                .0
                .proxies
                .borrow()
                .groups
                .iter()
                .find(|g| g.name == group)
                .is_some_and(|g| g.members.iter().any(|m| m.name == name));
            if !listed {
                return Err(format!("{name} is not one of the AI group's choices"));
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
                s.settings.ai.exit = Some(AiExit {
                    name,
                    fingerprint,
                    since: unix(),
                });
                s.settings.ai.follow = false;
                s.ai.alert = None;
            });
            svc.save_settings();
            svc.ai_sync().await;
            svc.run_ai_check(false).await;
            Ok(())
        })
    }

    /// AI follows the Location from now on (nothing pinned).
    pub fn ai_follow_location(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.modify(|s| {
                s.settings.ai.exit = None;
                s.settings.ai.follow = true;
                s.ai.alert = None;
            });
            svc.save_settings();
            svc.ai_sync().await;
            Ok(())
        })
    }

    pub fn dismiss_ai_alert(&self) {
        self.modify(|s| s.ai.alert = None);
    }

    /// Changes the AI layer's switches; the config is rebuilt and reloaded
    /// (open connections stay on their route through a reload).
    pub fn update_ai(
        &self,
        next: AiSettings,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let prev = svc.settings().ai;
            if prev == next {
                return Ok(());
            }
            svc.modify(|s| s.settings.ai = next.clone());
            svc.save_settings();
            if svc.running()
                && let Err(e) = svc.write_config(true).await
            {
                svc.modify(|s| s.settings.ai = prev);
                svc.save_settings();
                return Err(e);
            }
            Ok(())
        })
    }

    /// The check of every service, again, with the 90 s hold.
    pub fn check_ai(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            if !svc.running() {
                return Err("Connect first".into());
            }
            svc.run_ai_check(true).await;
            Ok(())
        })
    }

    /// Every ten minutes while connected.
    pub(super) async fn ai_loop(&self) {
        let mut tick = tokio::time::interval(Duration::from_secs(600));
        tick.tick().await;
        loop {
            tick.tick().await;
            self.run_ai_check(false).await;
        }
    }

    /// Checks the services: reachable, API answers, the country each sees.
    /// The 90 s hold runs when `hold` or when it has not run for an hour.
    /// `forced` ignores the minimum gap between checks.
    pub(super) async fn run_ai_check(&self, forced: bool) {
        let st = self.snapshot();
        if !st.settings.ai.enabled || !self.running() || st.profiles.active.is_none() {
            return;
        }
        // With the Route on Direct the core sends everything out directly,
        // so a check would measure this computer's own way out, not the exit:
        // show nothing rather than something that reads as the exit's.
        if st.settings.mode == crate::settings::Mode::Direct {
            self.modify(|s| {
                s.ai.checks.clear();
                s.ai.holds.clear();
            });
            return;
        }
        let now = unix();
        if !forced && now - self.0.ai_checked_at.load(Ordering::SeqCst) < MIN_GAP {
            return;
        }
        if self.0.ai_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.0.ai_checked_at.store(now, Ordering::SeqCst);
        self.modify(|s| s.ai.checking = true);
        let port = st.settings.mixed_port;
        let checks = ai_check::check_all(port, now).await;
        self.modify(|s| s.ai.checks = checks.clone());
        self.stats_ai(&checks);
        let due = forced || now - self.0.ai_hold_at.load(Ordering::SeqCst) >= HOLD_EVERY;
        if due {
            self.0.ai_hold_at.store(now, Ordering::SeqCst);
            let tasks = checks.iter().filter(|c| c.reachable).filter_map(|c| {
                let svc = ai::SERVICES.iter().find(|s| s.id == c.service)?;
                let (host, path) = match (svc.trace_host, svc.api) {
                    (Some(h), _) => (h, "/cdn-cgi/trace"),
                    (None, Some((h, p, _))) => (h, p),
                    _ => return None,
                };
                Some((svc.id, host, path))
            });
            let runs = tasks.map(|(id, host, path)| {
                let me = self.clone();
                async move {
                    let (a, b) = (me.clone(), me.clone());
                    let hold = ai_check::hold(
                        port,
                        host,
                        path,
                        HOLD_SECS,
                        move |h, since| a.open_connection_to(h, since),
                        move |id| b.connection_open(id),
                        unix,
                    )
                    .await;
                    me.stats_hold(id, &hold);
                    me.modify(|s| {
                        s.ai.holds.retain(|(k, _)| k != id);
                        s.ai.holds.push((id.to_owned(), hold));
                    });
                }
            });
            futures_util::future::join_all(runs).await;
        }
        self.modify(|s| s.ai.checking = false);
        self.0.ai_busy.store(false, Ordering::SeqCst);
    }

    /// The newest open connection to `host` that began at or after `since`.
    fn open_connection_to(&self, host: &str, since: i64) -> Option<String> {
        self.0
            .connections
            .borrow()
            .list
            .iter()
            .filter(|c| c.host == host)
            .filter(|c| crate::activity::parse_time(&c.start).is_some_and(|t| t >= since))
            .max_by_key(|c| crate::activity::parse_time(&c.start))
            .map(|c| c.id.clone())
    }

    fn connection_open(&self, id: &str) -> bool {
        self.0.connections.borrow().list.iter().any(|c| c.id == id)
    }
}
