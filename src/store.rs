//! The UI's view of the service: its `watch` channels turned into signals, and
//! the page the window shows. Made once in the root and provided as context.

use std::sync::Arc;

use kairo_core::activity::Tally;
use kairo_core::model::Connections;
use kairo_core::{Live, Proxies, Service, State};
use nsh_core::prelude::*;

/// The window's destinations: the five of the sidebar and Settings, and the
/// four views Settings › Advanced › Inspect opens (they have no row of their own).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Overview,
    Exits,
    Activity,
    Apps,
    Profiles,
    Settings,
    /// Inspect: the subscription's own groups.
    Proxies,
    /// Inspect: the running rules, read-only.
    Rules,
    /// Inspect: the core's log, as Diagnostics.
    Logs,
    /// Inspect: DNS lookups.
    Dns,
}

impl Page {
    pub const ALL: [Page; 10] = [
        Page::Overview,
        Page::Exits,
        Page::Activity,
        Page::Apps,
        Page::Profiles,
        Page::Settings,
        Page::Proxies,
        Page::Rules,
        Page::Logs,
        Page::Dns,
    ];

    /// The pages of the sidebars, in order (Settings last, apart).
    pub const SIDEBAR: [Page; 6] = [
        Page::Overview,
        Page::Exits,
        Page::Activity,
        Page::Apps,
        Page::Profiles,
        Page::Settings,
    ];

    /// The page named `name` (its title or an old one, any case).
    pub fn named(name: &str) -> Option<Page> {
        let old = match name.to_ascii_lowercase().as_str() {
            "overview" | "home" => Some(Page::Overview),
            "location" | "locations" => Some(Page::Exits),
            "profiles" | "subscriptions" => Some(Page::Profiles),
            "apps" => Some(Page::Apps),
            "connections" => Some(Page::Activity),
            "groups" => Some(Page::Proxies),
            "diagnostics" => Some(Page::Logs),
            _ => None,
        };
        old.or_else(|| {
            Page::ALL
                .into_iter()
                .find(|p| p.title().eq_ignore_ascii_case(name))
        })
    }

    /// An Inspect view: opened from Settings, shown under its row.
    pub fn inspect(self) -> bool {
        matches!(self, Page::Proxies | Page::Rules | Page::Logs | Page::Dns)
    }

    /// The sidebar row that stands for this page.
    pub fn sidebar(self) -> Page {
        if self.inspect() { Page::Settings } else { self }
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Overview => "Home",
            Page::Exits => "Exits",
            Page::Activity => "Activity",
            Page::Apps => "Apps & Sites",
            Page::Profiles => "Subscriptions",
            Page::Settings => "Settings",
            Page::Proxies => "Groups",
            Page::Rules => "Rules",
            Page::Logs => "Diagnostics",
            Page::Dns => "DNS",
        }
    }
}

/// Wrapper so large values notify on every new snapshot without comparing them.
#[derive(Clone)]
pub struct Snap<T>(pub Arc<T>);

impl<T> PartialEq for Snap<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Copy)]
pub struct Store {
    pub svc: Stored<Service>,
    pub state: Signal<State>,
    pub live: Signal<Live>,
    pub connections: Signal<Snap<Connections>>,
    pub proxies: Signal<Snap<Proxies>>,
    pub activity: Signal<Snap<Tally>>,
    pub log_seq: Signal<u64>,
    pub page: Signal<Page>,
    /// Activity shows the raw connection list (otherwise: by app).
    pub all_connections: Signal<bool>,
    /// Where the platform's chrome floats over the content.
    pub insets: Signal<crate::chrome::Insets>,
    /// The connections page is paused (the list stops following).
    pub toast: Signal<Option<(u64, SharedString, bool)>>,
}

impl Store {
    pub fn new(svc: Service) -> Store {
        let state_rx = svc.state();
        let live_rx = svc.live();
        let conn_rx = svc.connections();
        let prox_rx = svc.proxies();
        let activity_rx = svc.activity();
        let log_rx = svc.log_seq();
        let store = Store {
            state: signal(state_rx.borrow().clone()),
            live: signal(live_rx.borrow().clone()),
            connections: signal(Snap(conn_rx.borrow().clone())),
            proxies: signal(Snap(prox_rx.borrow().clone())),
            activity: signal(Snap(activity_rx.borrow().clone())),
            log_seq: signal(*log_rx.borrow()),
            page: signal(*crate::chrome::bridge().page.borrow()),
            all_connections: signal(false),
            toast: signal(None),
            insets: signal(*crate::chrome::bridge().insets.borrow()),
            svc: stored(svc),
        };
        follow(state_rx, move |s: &State| {
            let notice = s.notice.clone();
            store.state.set(s.clone());
            if let Some(n) = notice {
                store.show_notice(n.id, n.text, n.error);
            }
        });
        follow(live_rx, move |l: &Live| store.live.set(l.clone()));
        follow(conn_rx, move |c: &Arc<Connections>| {
            store.connections.set(Snap(c.clone()))
        });
        follow(prox_rx, move |p: &Arc<Proxies>| {
            store.proxies.set(Snap(p.clone()))
        });
        follow(activity_rx, move |a: &Arc<Tally>| {
            store.activity.set(Snap(a.clone()))
        });
        follow(log_rx, move |n: &u64| store.log_seq.set(*n));
        if crate::chrome::native() {
            // The platform's sidebar and menus select pages too.
            follow(crate::chrome::bridge().page.subscribe(), move |p: &Page| {
                store.page.set(*p)
            });
            effect(move || crate::chrome::select(store.page.get()));
            follow(
                crate::chrome::bridge().insets.subscribe(),
                move |i: &crate::chrome::Insets| store.insets.set(*i),
            );
            if let Some(mut events) = crate::chrome::bridge().take_events() {
                spawn(async move {
                    while let Some(event) = events.recv().await {
                        crate::ui::toolbar::dispatch(event);
                    }
                });
            }
        }
        store
    }

    pub fn svc(&self) -> Service {
        self.svc.with(|s| s.clone())
    }

    /// Shows a message at the bottom of the window for a few seconds.
    pub fn show_notice(&self, id: u64, text: impl Into<SharedString>, error: bool) {
        let shown = self.toast.get_untracked().map(|t| t.0);
        if shown == Some(id) {
            return;
        }
        self.toast.set(Some((id, text.into(), error)));
        let toast = self.toast;
        set_timeout(
            std::time::Duration::from_secs(if error { 6 } else { 3 }),
            move || {
                if toast.get_untracked().is_some_and(|t| t.0 == id) {
                    toast.set(None);
                }
            },
        );
    }

    /// Runs a service command; a failure becomes a message.
    pub fn run<T: 'static>(
        &self,
        f: impl std::future::Future<Output = Result<T, String>> + 'static,
    ) {
        let store = *self;
        spawn(async move {
            if let Err(e) = f.await {
                store.error(e);
            }
        });
    }

    /// Runs a command and then `done` with its success.
    pub fn run_then<T: 'static>(
        &self,
        f: impl std::future::Future<Output = Result<T, String>> + 'static,
        done: impl FnOnce(Result<T, String>) + 'static,
    ) {
        let store = *self;
        spawn(async move {
            let r = f.await;
            if let Err(e) = &r {
                store.error(e.clone());
            }
            done(r);
        });
    }

    pub fn error(&self, text: impl Into<SharedString>) {
        let id = NOTICE.with(|n| {
            n.set(n.get() + 1);
            u64::MAX - n.get()
        });
        self.show_notice(id, text, true);
    }

    pub fn info(&self, text: impl Into<SharedString>) {
        let id = NOTICE.with(|n| {
            n.set(n.get() + 1);
            u64::MAX - n.get()
        });
        self.show_notice(id, text, false);
    }
}

thread_local! {
    static NOTICE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Calls `each` with every new value of `rx`, on the engine thread.
fn follow<T: Clone + Send + Sync + 'static>(
    mut rx: kairo_core::tokio::sync::watch::Receiver<T>,
    each: impl Fn(&T) + 'static,
) {
    spawn(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let value = rx.borrow_and_update().clone();
            each(&value);
        }
    });
}

pub fn store() -> Store {
    use_context::<Store>().expect("the root provides the store")
}
