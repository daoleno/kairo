//! The service's choices for single apps: an app's own exit, its kind, and
//! the person's own apps (a site or a program Kairo did not know). Kept in
//! the settings, turned into rules when the config is built (see
//! `kinds::rules`), and reloaded the way a change of a kind is: the config
//! is rebuilt and checked by the core, and open connections stay on their
//! route through the reload.

use std::future::Future;

use super::Service;
use crate::kinds::{self, AppChoice, AppExit, Kind};
use crate::routes::RouteKind;

impl Service {
    /// The person's choices for apps, in the order they were made.
    pub fn apps(&self) -> Vec<AppChoice> {
        self.0.state.borrow().settings.apps.clone()
    }

    /// Sets an app's kind and exit. `id` is a catalog id, or for an app of
    /// the person's `matcher` says what it is (a site or a program; checked,
    /// and a site made lower case: an error says what is wrong). A catalog
    /// app back on its kind's exit leaves the list.
    pub fn set_app(
        &self,
        id: String,
        matcher: Option<RouteKind>,
        kind: Kind,
        exit: AppExit,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let choice = match matcher {
                Some(m) => {
                    if let RouteKind::App { name } = &m
                        && kinds::is_browser(name)
                    {
                        return Err(crate::tr!("engine.route_ops.browser", name = name));
                    }
                    AppChoice::custom(m, kind, exit)?
                }
                None => {
                    if kinds::catalog_app(&id).is_none() {
                        return Err(crate::tr!("engine.route_ops.unknown_app", id = id));
                    }
                    AppChoice { id, kind, matcher: None, exit }
                }
            };
            if let AppExit::Node(n) = &choice.exit
                && let Some(cfg) = svc.0.ai_cfg.lock().expect("ai cfg").as_ref()
                && !crate::ai::has_node(cfg, n)
            {
                return Err(crate::tr!("engine.service.not_a_server", name = n));
            }
            svc.change_apps(move |list| kinds::upsert(list, choice)).await
        })
    }

    /// Forgets an app of the person's, or a catalog app's choices (it goes
    /// back to its kind).
    pub fn remove_app(&self, id: String) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move { svc.change_apps(move |list| list.retain(|a| a.id != id)).await })
    }

    /// Changes the list, saves it, and has a running core load the new
    /// rules; if the core refuses them, the old list comes back.
    async fn change_apps(&self, edit: impl FnOnce(&mut Vec<AppChoice>)) -> Result<(), String> {
        let prev = self.settings().apps;
        let mut next = prev.clone();
        edit(&mut next);
        if next == prev {
            return Ok(());
        }
        self.modify(|s| s.settings.apps = next);
        self.save_settings();
        if self.running()
            && let Err(e) = self.write_config(true).await
        {
            self.modify(|s| s.settings.apps = prev);
            self.save_settings();
            return Err(e);
        }
        if !self.running() {
            // Nothing to reload: the next start builds with them. Which
            // ones will not apply is known from the config then.
            self.modify(|s| s.routes_skipped.clear());
        }
        // The activity's names follow the new list.
        let apps = self.settings().apps;
        self.0.activity.send_modify(|a| std::sync::Arc::make_mut(a).set_apps(apps));
        Ok(())
    }
}
