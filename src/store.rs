//! The UI's view of the service: its `watch` channels turned into signals, and
//! the page the window shows. Made once in the root and provided as context.

use std::sync::Arc;

use kairo_core::activity::Tally;
use kairo_core::model::Connections;
use kairo_core::stats::Store as Stats;
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
    /// One AI service: reachability and API time over the week. Opened from
    /// Home or Activity; shown under Activity.
    AiService,
}

impl Page {
    pub const ALL: [Page; 11] = [
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
        Page::AiService,
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
            "aiservice" | "ai-service" => Some(Page::AiService),
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

    /// Where an Inspect view was opened from after `now` is shown with `prev` before it:
    /// the page it came from (Subscriptions, Apps ...), kept while one Inspect view leads to
    /// another, and forgotten on any other page.
    pub fn origin_after(prev: Page, origin: Option<Page>, now: Page) -> Option<Page> {
        if !now.inspect() {
            None
        } else if prev.inspect() {
            origin
        } else {
            Some(prev)
        }
    }

    /// The sidebar row that stands for `self` when it was opened from `origin`: an Inspect view
    /// stays on the row of the page it came from, else it stands under Settings.
    pub fn row_from(self, origin: Option<Page>) -> Page {
        match origin.map(Page::sidebar) {
            Some(row) if self.inspect() && !row.inspect() => row,
            _ => self.sidebar(),
        }
    }

    /// The sidebar row that stands for this page.
    pub fn sidebar(self) -> Page {
        if self.inspect() {
            Page::Settings
        } else if self == Page::AiService {
            Page::Activity
        } else {
            self
        }
    }

    /// The short name for a phone's tab bar.
    pub fn tab_title(self) -> &'static str {
        match self {
            Page::Apps => "Apps",
            Page::Profiles => "Subs",
            other => other.title(),
        }
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
            Page::AiService => "AI Service",
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
    /// What is recorded for the charts.
    pub stats: Signal<Snap<Stats>>,
    /// The AI service the detail page shows (an `ai::SERVICES` id).
    pub ai_service: Signal<&'static str>,
    pub log_seq: Signal<u64>,
    /// Goes up when a picture of an app or a site has been kept (see `ui::logo`).
    pub icons_rev: Signal<u64>,
    /// The page an Inspect view (Groups, Rules ...) was opened from: its sidebar row stays on
    /// that page and Back returns to it (see `Page::origin_after`).
    pub origin: Signal<Option<Page>>,
    pub page: Signal<Page>,
    /// Activity shows the raw connection list (otherwise: by app).
    pub all_connections: Signal<bool>,
    /// Where the platform's chrome floats over the content.
    pub insets: Signal<crate::chrome::Insets>,
    /// The connections page is paused (the list stops following).
    pub toast: Signal<Option<(u64, SharedString, bool)>>,
}

impl Store {
    /// The sidebar row that stands for the page shown.
    pub fn row(&self) -> Page {
        self.page.get().row_from(self.origin.get())
    }

    pub fn new(svc: Service) -> Store {
        let state_rx = svc.state();
        let live_rx = svc.live();
        let conn_rx = svc.connections();
        let prox_rx = svc.proxies();
        let activity_rx = svc.activity();
        let stats_rx = svc.stats();
        let log_rx = svc.log_seq();
        let icons_rx = svc.icons_rev();
        let store = Store {
            state: signal(state_rx.borrow().clone()),
            live: signal(live_rx.borrow().clone()),
            connections: signal(Snap(conn_rx.borrow().clone())),
            proxies: signal(Snap(prox_rx.borrow().clone())),
            activity: signal(Snap(activity_rx.borrow().clone())),
            stats: signal(Snap(stats_rx.borrow().clone())),
            ai_service: signal("claude"),
            log_seq: signal(*log_rx.borrow()),
            icons_rev: signal(*icons_rx.borrow()),
            origin: signal(None),
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
        follow(stats_rx, move |s: &Arc<Stats>| {
            store.stats.set(Snap(s.clone()))
        });
        follow(log_rx, move |n: &u64| store.log_seq.set(*n));
        follow(icons_rx, move |n: &u64| store.icons_rev.set(*n));
        {
            let last = std::rc::Rc::new(std::cell::Cell::new(store.page.get_untracked()));
            effect(move || {
                let now = store.page.get();
                let prev = last.replace(now);
                let origin = Page::origin_after(prev, store.origin.get_untracked(), now);
                store.origin.set(origin);
            });
        }
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
        // Under no owner: a notice raised by a page must still go when that page is left.
        Owner::detached().run(|| {
            set_timeout(
                std::time::Duration::from_secs(if error { 6 } else { 3 }),
                move || {
                    if toast.get_untracked().is_some_and(|t| t.0 == id) {
                        toast.set(None);
                    }
                },
            )
        });
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

    /// Runs an update of a subscription (or of all). When it fails and the card on the
    /// Subscriptions page is showing that failure, the card is the message: no toast says it again.
    pub fn run_update<T: 'static>(
        &self,
        f: impl std::future::Future<Output = Result<T, String>> + 'static,
    ) {
        self.run_update_then(f, |_| {});
    }

    /// `run_update`, and then `done` with the result.
    pub fn run_update_then<T: 'static>(
        &self,
        f: impl std::future::Future<Output = Result<T, String>> + 'static,
        done: impl FnOnce(Result<T, String>) + 'static,
    ) {
        let store = *self;
        spawn(async move {
            let r = f.await;
            if let Err(e) = &r {
                let carded = {
                    let state = store.svc().state();
                    let state = state.borrow();
                    failure_on_card(store.page.get_untracked(), &state.profiles)
                };
                if !carded {
                    store.error(e.clone());
                }
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

/// Whether the Subscriptions page is open and its card shows a failed update.
pub fn failure_on_card(page: Page, profiles: &kairo_core::profile::ProfileIndex) -> bool {
    page == Page::Profiles && profiles.profiles.iter().any(|p| p.failure_shown())
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

#[cfg(test)]
mod page_tests {
    use super::{Page, failure_on_card};
    use kairo_core::profile::{ConfigStats, Profile, ProfileIndex, Source};

    fn index(failed: bool, dismissed: bool) -> ProfileIndex {
        let mut p = Profile {
            id: "a".into(),
            name: "Demo".into(),
            source: Source::Remote { url: "https://e.com/s".into(), interval_hours: 12 },
            updated_at: Some(1000),
            usage: None,
            web_page: None,
            last_error: None,
            failed_at: None,
            failure_dismissed: false,
            stats: ConfigStats::default(),
        };
        if failed {
            p.record_failure("The server answered HTTP 502".into(), 1);
        }
        p.failure_dismissed = dismissed;
        ProfileIndex { active: None, profiles: vec![p] }
    }

    #[test]
    fn a_failure_the_card_shows_is_not_also_a_toast() {
        // The card is open on Subscriptions and shows it: no toast.
        assert!(failure_on_card(Page::Profiles, &index(true, false)));
        // On another page the card is not in view: the toast stays.
        assert!(!failure_on_card(Page::Overview, &index(true, false)));
        assert!(!failure_on_card(Page::Apps, &index(true, false)));
        // A closed card, or no failure: nothing is showing, the toast says it.
        assert!(!failure_on_card(Page::Profiles, &index(true, true)));
        assert!(!failure_on_card(Page::Profiles, &index(false, false)));
    }

    #[test]
    fn an_inspect_view_stays_on_the_row_of_the_page_it_came_from() {
        // Opened from Subscriptions: the row stays there, and the origin is Subscriptions.
        let origin = Page::origin_after(Page::Profiles, None, Page::Proxies);
        assert_eq!(origin, Some(Page::Profiles));
        assert_eq!(Page::Proxies.row_from(origin), Page::Profiles);
        // One Inspect view leading to another keeps the origin.
        let origin = Page::origin_after(Page::Proxies, origin, Page::Rules);
        assert_eq!(origin, Some(Page::Profiles));
        assert_eq!(Page::Rules.row_from(origin), Page::Profiles);
        // Any other page forgets it; the pages of the sidebar are their own row.
        assert_eq!(Page::origin_after(Page::Rules, origin, Page::Apps), None);
        assert_eq!(Page::Apps.row_from(origin), Page::Apps);
        // From Settings, or with no origin, an Inspect view stands under Settings.
        assert_eq!(Page::Logs.row_from(Some(Page::Settings)), Page::Settings);
        assert_eq!(Page::Dns.row_from(None), Page::Settings);
        // An AI service stays under Activity.
        assert_eq!(Page::AiService.row_from(None), Page::Activity);
    }
}
