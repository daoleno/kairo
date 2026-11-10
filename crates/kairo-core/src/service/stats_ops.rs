//! The service's statistics: what the snapshots, the AI checks, the exit
//! tests and the subscriptions' usage add to [`crate::stats::Store`], the file
//! it is kept in, and the copy the pages read. It records with the window
//! closed: nothing here depends on a page being open.

use std::sync::Arc;
use std::time::Duration;

use super::{Service, unix};
use crate::ai_check::{Api, Check, Country, Hold};
use crate::stats::{Delta, Store};

/// The exit tests run this often.
const PROBE_EVERY: i64 = 600;
/// The file is written at most this often while things change.
const SAVE_EVERY: i64 = 120;

/// The store and when it was last written, probed and told to the pages.
pub(super) struct Recorder {
    pub store: Store,
    dirty: bool,
    saved_at: i64,
    probed_at: i64,
    /// The server each exit was on at the last look.
    nodes: [Option<String>; 3],
    /// Hand changes the person just made, by exit index (unix s).
    manual: [i64; 3],
}

impl Recorder {
    pub(super) fn new(store: Store) -> Recorder {
        Recorder {
            store,
            dirty: false,
            saved_at: 0,
            probed_at: 0,
            nodes: [None, None, None],
            manual: [0; 3],
        }
    }
}

use crate::stats::EXITS;

impl Service {
    /// What is recorded; a new copy comes about a minute after things change.
    pub fn stats(&self) -> tokio::sync::watch::Receiver<Arc<Store>> {
        self.0.stats_tx.subscribe()
    }

    /// Forgets everything recorded (Activity › Clear History).
    pub fn clear_stats(&self) {
        {
            let mut r = self.0.stats.lock().expect("stats");
            r.store = Store::default();
            r.dirty = true;
        }
        self.stats_publish();
        self.stats_save(true);
    }

    pub(super) fn stats_load(&self) {
        let text = std::fs::read_to_string(self.0.paths.stats_file()).unwrap_or_default();
        let mut store = Store::from_json(&text);
        if !text.is_empty() && store.is_empty() {
            log::warn!("the statistics file could not be read; starting a new one");
        }
        store.prune(unix());
        let mut r = self.0.stats.lock().expect("stats");
        r.store = store;
        drop(r);
        self.stats_publish();
    }

    fn stats_publish(&self) {
        let copy = Arc::new(self.0.stats.lock().expect("stats").store.clone());
        self.0.stats_tx.send_replace(copy);
    }

    /// Writes the file when something changed (always with `force`).
    pub fn stats_save(&self, force: bool) {
        let now = unix();
        let text = {
            let mut r = self.0.stats.lock().expect("stats");
            if !r.dirty && !force {
                return;
            }
            if !force && now - r.saved_at < SAVE_EVERY {
                return;
            }
            r.dirty = false;
            r.saved_at = now;
            r.store.prune(now);
            r.store.to_json()
        };
        if let Err(e) = crate::paths::write_atomic(&self.0.paths.stats_file(), text.as_bytes()) {
            log::warn!("the statistics could not be written: {e}");
        }
    }

    pub(super) fn stats_traffic(&self, now: i64, deltas: &[Delta]) {
        let mut r = self.0.stats.lock().expect("stats");
        if deltas.iter().any(|d| d.bytes > 0) {
            r.store.add_traffic(now, deltas);
            r.dirty = true;
        }
    }

    pub(super) fn stats_ai(&self, checks: &[Check]) {
        let mut r = self.0.stats.lock().expect("stats");
        for c in checks {
            let ms = match &c.api {
                Api::Answers { ms } => Some(*ms),
                _ => None,
            };
            let country = match &c.country {
                Country::Seen { code, .. } => Some(code.as_str()),
                _ => None,
            };
            r.store.record_ai(c.at, c.service, c.reachable, ms, country);
        }
        r.dirty = true;
    }

    pub(super) fn stats_hold(&self, service: &str, hold: &Hold) {
        let held = match hold {
            Hold::Held { .. } => true,
            Hold::Dropped { .. } => false,
            // The test did not run: neither a hold nor a drop.
            Hold::Failed { .. } => return,
        };
        let mut r = self.0.stats.lock().expect("stats");
        r.store.record_hold(unix(), service, held);
        r.dirty = true;
    }

    /// The person just chose a server for an exit (0 AI, 1 everything else,
    /// 2 downloads): a change in the next look is theirs, not Kairo's.
    pub(super) fn stats_manual(&self, exit: usize) {
        if let Some(m) = self.0.stats.lock().expect("stats").manual.get_mut(exit) {
            *m = unix();
        }
    }

    pub(super) fn stats_usage(&self, id: &str) {
        let Some(p) = self.snapshot().profiles.get(id).cloned() else {
            return;
        };
        let Some(u) = p.usage else { return };
        let mut r = self.0.stats.lock().expect("stats");
        r.store
            .record_usage(unix(), id, u.upload + u.download, u.total, u.expire);
        r.dirty = true;
    }

    /// The server each exit goes through now.
    fn exit_nodes(&self) -> [Option<String>; 3] {
        let rest = self.main_group().and_then(|m| {
            let p = self.0.proxies.borrow();
            crate::places::current(&p.groups, &m).map(|c| c.name)
        });
        [self.ai_node(), rest, self.downloads_node()]
    }

    /// Looks at where the exits are (a change is a switch) and, every ten
    /// minutes, tests the server each is on.
    async fn stats_look(&self) {
        let now = unix();
        if !self.running() {
            return;
        }
        let nodes = self.exit_nodes();
        let probe = {
            let mut r = self.0.stats.lock().expect("stats");
            for (i, node) in nodes.iter().enumerate() {
                let (Some(to), Some(from)) = (node, r.nodes[i].clone()) else {
                    continue;
                };
                if *to != from && !crate::places::is_note(to) && !crate::places::is_note(&from) {
                    let auto = now - r.manual[i] > 60;
                    r.store.record_switch(now, EXITS[i], &from, to, auto);
                    r.dirty = true;
                }
            }
            for (i, node) in nodes.iter().enumerate() {
                if node.is_some() {
                    r.nodes[i] = node.clone();
                }
            }
            let due = now - r.probed_at >= PROBE_EVERY;
            if due {
                r.probed_at = now;
            }
            due
        };
        if !probe {
            return;
        }
        for (i, node) in nodes.into_iter().enumerate() {
            let Some(node) = node else { continue };
            if crate::places::is_note(&node) {
                continue;
            }
            let ms = self
                .test_node(node.clone(), None)
                .await
                .ok()
                .filter(|m| *m > 0);
            let mut r = self.0.stats.lock().expect("stats");
            r.store.record_exit(unix(), EXITS[i], &node, ms);
            r.dirty = true;
        }
    }

    /// The background part: each minute look at the exits, tell the pages and
    /// write the file when it is time.
    pub(super) async fn stats_task(self) {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.tick().await;
        loop {
            tick.tick().await;
            self.stats_look().await;
            let changed = self.0.stats.lock().expect("stats").dirty;
            if changed {
                self.stats_publish();
            }
            self.stats_save(false);
        }
    }

    pub(super) fn stats_close(&self) {
        self.stats_save(true);
    }
}
