//! The service's pictures of apps and sites (see `icons`): what is on disk is answered at once,
//! what is not is looked for in the background (one fetch at a time for a name, a few at once in
//! all), and the pages are told by a counter when something new has been kept.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::SystemTime;

use tokio::sync::{Semaphore, watch};

use super::Service;
use crate::icons::{self, Cached, Policy};

/// How many fetches run at once.
fn gate() -> &'static Semaphore {
    static GATE: OnceLock<Semaphore> = OnceLock::new();
    GATE.get_or_init(|| Semaphore::new(3))
}

/// `KAIRO_NO_ICON_FETCH=1`: nothing is fetched from sites or looked up (tests, isolated runs).
fn off() -> bool {
    std::env::var_os("KAIRO_NO_ICON_FETCH").is_some_and(|v| !v.is_empty())
}

impl Service {
    /// A counter that goes up whenever a picture has been kept, for the pages to look again.
    pub fn icons_rev(&self) -> watch::Receiver<u64> {
        self.0.icons_rev.subscribe()
    }

    /// The picture of a site (`example.com`): its file when there is one; the first time (and
    /// rarely after) it is fetched in the background from the site itself.
    pub fn site_icon(&self, domain: &str) -> Option<PathBuf> {
        let domain = crate::routes::clean_site(domain).ok()?;
        self.icon_for("sites", &domain)
    }

    /// The picture of an installed app by the program's name: its file when there is one; the
    /// first time it is looked for in the background.
    pub fn app_icon(&self, name: &str) -> Option<PathBuf> {
        let name = crate::icons::apps::program_name(name);
        self.icon_for("apps", &name)
    }

    /// The name an installed app shows people ("Firefox" for the program `firefox`), once the
    /// system has said it. Nothing for a program that is not an installed app.
    pub fn app_label(&self, name: &str) -> Option<String> {
        let name = crate::icons::apps::program_name(name);
        // Looked for with the icon, which every row asks for as well.
        let _ = self.icon_for("apps", &name);
        self.0.icons.label("apps", &name)
    }

    fn icon_for(&self, kind: &'static str, name: &str) -> Option<PathBuf> {
        let cached = self.0.icons.look(kind, name, SystemTime::now());
        let (path, wanted) = match cached {
            // A picture kept before names were: look once more for the name.
            Cached::Have { path, stale } => (
                Some(path),
                stale || (kind == "apps" && !self.0.icons.label_known(kind, name)),
            ),
            Cached::Missing => (None, false),
            Cached::Unknown => (None, true),
        };
        if wanted && !off() && self.0.icons.begin(kind, name) {
            let svc = self.clone();
            let name = name.to_owned();
            self.0.rt.spawn(async move {
                svc.find_icon(kind, &name).await;
                svc.0.icons.end(kind, &name);
            });
        }
        path
    }

    /// One look for a picture: kept when found, remembered as not found when not.
    async fn find_icon(&self, kind: &'static str, name: &str) {
        let Ok(_turn) = gate().acquire().await else {
            return;
        };
        // A site is asked for over its own route, which is the core's: without the core it goes
        // out directly, and a failure then may only be that the core is not up yet.
        let via_core = self.running();
        if kind == "apps" {
            let looked = name.to_owned();
            let found = tokio::task::spawn_blocking(move || icons::apps::lookup(&looked))
                .await
                .ok()
                .flatten();
            let had_icon = matches!(
                self.0.icons.look(kind, name, SystemTime::now()),
                Cached::Have { .. }
            );
            match found {
                Some(f) => {
                    self.0.icons.store_label(kind, name, f.label.as_deref());
                    match &f.icon {
                        Some(icon) => {
                            let _ = self.0.icons.store(kind, name, icon);
                        }
                        // A stale picture stays as it is.
                        None if !had_icon => self.0.icons.store_none(kind, name, false),
                        None => {}
                    }
                }
                None => {
                    self.0.icons.store_label(kind, name, None);
                    if !had_icon {
                        self.0.icons.store_none(kind, name, false);
                    }
                }
            }
            self.0.icons_rev.send_modify(|n| *n += 1);
            return;
        }
        // A site is a fetch; an app was dealt with above.
        let proxy = via_core.then(|| format!("http://127.0.0.1:{}", self.settings().mixed_port));
        let found = match icons::client(proxy.as_deref(), Policy::default()) {
            Ok(client) => {
                icons::fetch_site(&client, &icons::site_origins(name), Policy::default())
                    .await
                    .ok()
            }
            Err(_) => None,
        };
        match found {
            Some(icon) => {
                if self.0.icons.store(kind, name, &icon).is_ok() {
                    self.0.icons_rev.send_modify(|n| *n += 1);
                }
            }
            None => {
                // A stale picture stays as it is; only a name never found is marked.
                if !matches!(
                    self.0.icons.look(kind, name, SystemTime::now()),
                    Cached::Have { .. }
                ) {
                    self.0.icons.store_none(kind, name, !via_core);
                }
            }
        }
    }
}
