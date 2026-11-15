//! The service's kinds with an exit of their own besides AI: Downloads,
//! Streaming, Games and Dev tools. Each one goes as the subscription routes
//! it (no rule of Kairo's), or to a group of its own that Kairo keeps on a
//! pinned node, the fastest one, or the Location. As with AI, moving a group
//! never closes a connection: new connections go to the new exit, open ones
//! finish where they began. The Downloads mirror and its test are here too.

use std::future::Future;
use std::sync::atomic::Ordering;

use super::{Service, unix};
use crate::ai::{self, AiExit, ExitState};
use crate::downloads::{self, MirrorTest};
use crate::kinds::{Kind, KindExit};
use crate::places;
use serde_yaml::Mapping;

/// The quality statistics' index of a kind's exit, for those it records.
fn stats_index(kind: Kind) -> Option<usize> {
    match kind {
        Kind::Ai => Some(0),
        Kind::Other => Some(1),
        Kind::Downloads => Some(2),
        _ => None,
    }
}

impl Service {
    /// The kind's group, once the config has it.
    pub(super) fn kind_group(&self, kind: Kind) -> Option<String> {
        self.0.state.borrow().kinds.groups.get(&kind).cloned()
    }

    /// The node a kind's traffic goes through now (`None`: it goes as the
    /// subscription routes it, or the core is not running).
    pub fn kind_node(&self, kind: Kind) -> Option<String> {
        if kind == Kind::Ai {
            return self.ai_node();
        }
        let group = self.kind_group(kind)?;
        ai::resolve(&self.0.proxies.borrow().groups, &group)
    }

    /// Keeps `State.kinds.nodes` up to date; wakes the UI only when it changed.
    pub(super) fn kind_nodes_changed(&self) {
        let nodes: std::collections::BTreeMap<Kind, String> = Kind::ROUTED
            .into_iter()
            .filter_map(|k| Some((k, self.kind_node(k)?)))
            .collect();
        self.0.state.send_if_modified(|s| {
            if s.kinds.nodes == nodes {
                return false;
            }
            s.kinds.nodes = nodes;
            true
        });
    }

    /// The config was written: note each kind's group and where the
    /// subscription sends the apps Kairo leaves to it, and whether a pinned
    /// exit is still in the subscription (renamed: followed; gone: the kind
    /// goes back to the fastest or the Location, and the person is told).
    pub(super) fn kinds_config_loaded(&self, mapping: &Mapping, applied: Option<&ai::Applied>) {
        let groups = applied
            .map(|a| {
                a.kind_groups
                    .iter()
                    .filter(|(k, _)| **k != Kind::Ai)
                    .map(|(k, g)| (*k, g.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let rules: Vec<String> = mapping
            .get("rules")
            .and_then(|r| r.as_sequence())
            .map(|s| s.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let settings = self.settings();
        let sub_targets = crate::kinds::catalog()
            .iter()
            .filter_map(|a| {
                let t = crate::kinds::subscription_target(&rules, a.site)?;
                Some((a.id.to_owned(), t))
            })
            .collect();
        self.modify(|s| {
            s.kinds.groups = groups;
            s.kinds.sub_targets = sub_targets;
        });
        for kind in Kind::ROUTED {
            let Some(exit) = settings.kinds.exit(kind).cloned() else { continue };
            if exit.subscription || self.kind_group(kind).is_none() {
                continue;
            }
            match ai::exit_state(mapping, exit.exit.as_ref()) {
                ExitState::Same | ExitState::Unpinned => {}
                ExitState::Renamed(name) => {
                    self.modify(|s| {
                        if let Some(e) = s.settings.kinds.exit_mut(kind).and_then(|e| e.exit.as_mut()) {
                            e.name = name;
                        }
                    });
                    self.save_settings();
                }
                ExitState::Gone => {
                    let was = exit.exit.map(|e| e.name).unwrap_or_default();
                    self.modify(|s| {
                        if let Some(e) = s.settings.kinds.exit_mut(kind) {
                            e.exit = None;
                        }
                    });
                    self.save_settings();
                    self.notify(
                        if exit.fastest {
                            crate::tr!("engine.downloads_ops.exit_gone_fastest", was = was, kind = kind.title())
                        } else {
                            crate::tr!("engine.downloads_ops.exit_gone_location", was = was, kind = kind.title())
                        },
                        false,
                    );
                }
            }
        }
    }

    /// Makes every routed kind's group go where it should.
    pub(super) async fn kinds_sync(&self, retest: bool) {
        for kind in Kind::ROUTED {
            self.kind_sync(kind, retest).await;
        }
        self.kind_nodes_changed();
    }

    /// Makes a kind's group go where it should: the pinned node, else the
    /// fastest (measured by the same tests as the Location's), else the
    /// Location. `retest` measures the places first; without it they are
    /// measured only when nothing is known yet.
    pub(super) async fn kind_sync(&self, kind: Kind, retest: bool) {
        if !self.running() {
            return;
        }
        let Some(group) = self.kind_group(kind) else {
            return;
        };
        let Some(main) = self.main_group() else {
            return;
        };
        let Some(d) = self.settings().kinds.exit(kind).cloned() else {
            return;
        };
        if d.exit.is_none() && d.fastest {
            let known = places::places(&self.0.proxies.borrow().groups, &main)
                .iter()
                .any(|p| p.delay.is_some_and(|x| x > 0));
            if retest || !known {
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
                log::warn!("{} exit: {e}", kind.title());
            }
            self.refresh_proxies_now().await;
        }
    }

    /// Sets a kind's exit and rebuilds the config when that changes which
    /// rules exist (to or from the subscription's routing).
    fn change_kind(
        &self,
        kind: Kind,
        next: KindExit,
        retest: bool,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let Some(prev) = svc.settings().kinds.exit(kind).cloned() else {
                return Err(crate::tr!("engine.downloads_ops.no_exit_of_its_own", kind = kind.title()));
            };
            svc.modify(|s| {
                if let Some(e) = s.settings.kinds.exit_mut(kind) {
                    *e = next.clone();
                }
                if kind == Kind::Downloads {
                    // The mirror was measured through another exit.
                    s.downloads.mirror_test = MirrorTest::Untested;
                }
            });
            svc.save_settings();
            let rebuild = prev.subscription != next.subscription || svc.kind_group(kind).is_none();
            if rebuild
                && svc.running()
                && let Err(e) = svc.write_config(true).await
            {
                svc.modify(|s| {
                    if let Some(e) = s.settings.kinds.exit_mut(kind) {
                        *e = prev;
                    }
                });
                svc.save_settings();
                return Err(e);
            }
            svc.kind_sync(kind, retest).await;
            svc.kind_nodes_changed();
            Ok(())
        })
    }

    /// Pins `name` (a node the kind's group lists) as the kind's exit; new
    /// connections go there, open ones finish where they are. A kind that
    /// went as the subscription routes it gets its rules and group first.
    pub fn set_kind_exit(
        &self,
        kind: Kind,
        name: String,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'static>> {
        if kind == Kind::Ai {
            return Box::pin(self.set_ai_exit(name));
        }
        if let Some(i) = stats_index(kind) {
            self.stats_manual(i);
        }
        let fingerprint = self
            .0
            .ai_cfg
            .lock()
            .expect("ai cfg")
            .as_ref()
            .and_then(|c| ai::fingerprint(c, &name));
        let Some(fingerprint) = fingerprint else {
            return Box::pin(std::future::ready(Err(crate::tr!("engine.service.not_a_server", name = name))));
        };
        Box::pin(self.change_kind(
            kind,
            KindExit {
                subscription: false,
                exit: Some(AiExit { name, fingerprint, since: unix() }),
                fastest: false,
            },
            false,
        ))
    }

    /// The kind follows the fastest server from now on (nothing pinned).
    pub fn kind_follow_fastest(&self, kind: Kind) -> impl Future<Output = Result<(), String>> + 'static {
        self.change_kind(kind, KindExit::FASTEST, true)
    }

    /// The kind follows the Location from now on ("Same as Everything else").
    pub fn kind_follow_location(&self, kind: Kind) -> impl Future<Output = Result<(), String>> + 'static {
        self.change_kind(kind, KindExit::LOCATION, false)
    }

    /// The kind goes as the subscription routes it: Kairo writes no rule for
    /// it. Its last pinned node is kept for when the person comes back.
    pub fn kind_to_subscription(&self, kind: Kind) -> impl Future<Output = Result<(), String>> + 'static {
        let mut next = self.settings().kinds.exit(kind).cloned().unwrap_or_default();
        next.subscription = true;
        self.change_kind(kind, next, false)
    }

    /// Measures the mirror against Hugging Face, both through the Downloads
    /// exit, and keeps the result in `State.downloads.mirror_test`. Nothing
    /// is changed on this computer.
    pub fn test_mirror(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            if !svc.running() {
                return Err(crate::tr!("engine.service.connect_first"));
            }
            // Without its rules the test would measure whatever way the
            // subscription sends these sites, not the Downloads exit.
            if svc.kind_group(Kind::Downloads).is_none() {
                return Err(crate::tr!("engine.downloads_ops.as_subscription"));
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
                return Err(downloads::mirror_refused());
            }
            if svc.settings().kinds.mirror != on {
                svc.modify(|s| s.settings.kinds.mirror = on);
                svc.save_settings();
            }
            Ok(())
        })
    }
}
