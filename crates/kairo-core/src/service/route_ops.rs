//! The service's routes for single apps and sites: kept in the settings,
//! turned into rules when the config is built (see `routes` and
//! `ai::apply_with`), and reloaded the way a change of the AI layer is: the
//! config is rebuilt and checked by the core, and open connections stay on
//! their route through the reload.

use std::future::Future;

use super::Service;
use crate::routes::{self, Route, RouteExit, RouteKind};

impl Service {
    /// The person's routes, in the order they were added.
    pub fn routes(&self) -> Vec<Route> {
        self.0.state.borrow().settings.routes.clone()
    }

    /// Sends one app or site to `exit` (replacing its route); `Usual`
    /// removes it, so the subscription decides again. The value is checked
    /// (and made lower case for a site): an error says what is wrong with it.
    pub fn set_route(
        &self,
        kind: RouteKind,
        exit: RouteExit,
    ) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            let route = Route::new(kind, exit)?;
            if let RouteExit::Node(n) = &route.exit
                && let Some(cfg) = svc.0.ai_cfg.lock().expect("ai cfg").as_ref()
                && !crate::ai::has_node(cfg, n)
            {
                return Err(format!("{n} is not a server of this subscription"));
            }
            svc.change_routes(move |list| routes::upsert(list, route))
                .await
        })
    }

    /// Removes the route with this id (`app:<name>` or `site:<domain>`).
    pub fn remove_route(&self, id: String) -> impl Future<Output = Result<(), String>> + 'static {
        let svc = self.clone();
        self.run(async move {
            svc.change_routes(move |list| list.retain(|r| r.id != id))
                .await
        })
    }

    /// Changes the list, saves it, and has a running core load the new
    /// rules; if the core refuses them, the old list comes back.
    async fn change_routes(&self, edit: impl FnOnce(&mut Vec<Route>)) -> Result<(), String> {
        let prev = self.settings().routes;
        let mut next = prev.clone();
        edit(&mut next);
        if next == prev {
            return Ok(());
        }
        self.modify(|s| s.settings.routes = next);
        self.save_settings();
        if self.running()
            && let Err(e) = self.write_config(true).await
        {
            self.modify(|s| s.settings.routes = prev);
            self.save_settings();
            return Err(e);
        }
        if !self.running() {
            // Nothing to reload: the next start builds with them. Which
            // ones will not apply is known from the config then.
            self.modify(|s| s.routes_skipped.clear());
        }
        Ok(())
    }
}
