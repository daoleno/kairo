//! The checks of the kinds besides AI: what Streaming, Games, Downloads and
//! Dev tools apps see through their exits. They run with the AI checks (on
//! connect, after a reload, every ten minutes) for the apps in use, and on
//! demand for one app. Like the AI checks they go through Kairo's own proxy
//! port, so the rules send each probe the way the app's traffic goes.

use std::future::Future;
use std::sync::atomic::Ordering;

use super::{Service, unix};
use crate::kind_check::{self, KindCheck};
use crate::kinds::{self, Kind};

/// The checks run at most this often, unless asked for.
const MIN_GAP: i64 = 60;

/// A flag's country code: 🇯🇵 is `JP`.
pub fn flag_code(flag: &str) -> Option<String> {
    let code: String = flag
        .chars()
        .filter_map(|c| {
            let u = c as u32;
            (0x1F1E6..=0x1F1FF).contains(&u).then(|| char::from(b'A' + (u - 0x1F1E6) as u8))
        })
        .collect();
    (code.len() == 2).then_some(code)
}

impl Service {
    /// The apps in use (see [`Tally::in_use`](crate::activity::Tally::in_use)).
    pub fn apps_in_use(&self) -> Vec<String> {
        let tz = crate::time::local_offset_seconds();
        let week = self
            .0
            .stats
            .lock()
            .expect("stats")
            .store
            .top_kapps(crate::stats::Range::Week, unix(), tz);
        self.0.activity.borrow().in_use(&week)
    }

    /// The country code a kind's exit is in, from its node's flag, for the
    /// apps whose country is only an estimate.
    fn kind_estimate(&self, app: &str, kind: Kind) -> Option<String> {
        let node = self.kind_node(kind).or_else(|| {
            let group = self.0.state.borrow().kinds.sub_targets.get(app).cloned()?;
            crate::ai::resolve(&self.0.proxies.borrow().groups, &group).or(Some(group))
        })?;
        let place = crate::places::place(&crate::model::Node {
            name: node,
            kind: String::new(),
            udp: false,
            delay: None,
            alive: true,
            is_group: false,
        });
        place.flag.as_deref().and_then(flag_code)
    }

    /// Checks `ids` (apps with a probe) and keeps the results.
    async fn check_these(&self, ids: Vec<String>) {
        let port = self.settings().mixed_port;
        let now = unix();
        let runs = ids.into_iter().filter_map(|id| {
            let probe = kind_check::probe_for(&id)?;
            let kind = self
                .settings()
                .apps
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.kind)
                .or_else(|| kinds::catalog_app(&id).map(|a| a.kind))?;
            let estimate = self.kind_estimate(&id, kind);
            Some(async move { kind_check::run(Some(port), &id, &probe, estimate.as_deref(), now).await })
        });
        let done: Vec<KindCheck> = futures_util::future::join_all(runs).await;
        self.modify(|s| {
            for c in done {
                s.kinds.checks.retain(|x| x.app != c.app);
                s.kinds.checks.push(c);
            }
        });
    }

    /// Checks the apps in use. `forced` ignores the minimum gap.
    pub(super) async fn run_kind_checks(&self, forced: bool) {
        let st = self.snapshot();
        if st.settings.kinds_off || !self.running() || st.profiles.active.is_none() {
            return;
        }
        // On Direct a check would measure this computer's own way out.
        if st.settings.mode == crate::settings::Mode::Direct {
            self.modify(|s| s.kinds.checks.clear());
            return;
        }
        let now = unix();
        if !forced && now - self.0.kind_checked_at.load(Ordering::SeqCst) < MIN_GAP {
            return;
        }
        if self.0.kind_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.0.kind_checked_at.store(now, Ordering::SeqCst);
        self.modify(|s| s.kinds.checking = true);
        let ids = self.apps_in_use();
        self.check_these(ids).await;
        self.modify(|s| s.kinds.checking = false);
        self.0.kind_busy.store(false, Ordering::SeqCst);
    }

    /// Every check again: the AI services' (with the 90 s hold) and the
    /// other kinds' apps in use.
    pub fn check_apps(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            if !svc.running() {
                return Err(crate::tr!("engine.service.connect_first"));
            }
            let (a, b) = (svc.clone(), svc.clone());
            tokio::join!(a.run_ai_check(true), b.run_kind_checks(true));
            Ok(())
        })
    }

    /// One app's check, now (its detail's Check Now). An AI service runs
    /// the AI checks.
    pub fn check_app(&self, id: String) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            if !svc.running() {
                return Err(crate::tr!("engine.service.connect_first"));
            }
            if crate::ai::SERVICES.iter().any(|s| s.id == id) {
                svc.run_ai_check(true).await;
                return Ok(());
            }
            if kind_check::probe_for(&id).is_none() {
                return Err(crate::tr!("engine.kind_check_ops.no_check"));
            }
            svc.check_these(vec![id]).await;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_is_a_country_code() {
        assert_eq!(flag_code("🇯🇵").as_deref(), Some("JP"));
        assert_eq!(flag_code("🇺🇸").as_deref(), Some("US"));
        assert_eq!(flag_code("◎"), None);
    }
}
