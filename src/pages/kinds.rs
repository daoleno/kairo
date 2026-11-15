//! Apps by kind, as Home and Apps show them: where each kind leaves, the apps
//! in use and what their kind's check measured, an app's exit menu, and the
//! app's sheet. The facts come from `kairo_core` (`kinds`, `kind_check`, the
//! tally and the statistics); this file decides how they read.

use kairo_core::activity::{AppUse, Tally};
use kairo_core::kind_check::{self, KindCheck, Measured, Ssh};
use kairo_core::kinds::{self, AppChoice, AppExit, Kind};
use kairo_core::places::{self, Place};
use kairo_core::routes::RouteKind;
use kairo_core::{State, ai_check, tr};

use crate::i18n::plural;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::store::{Page, Store, store};
use crate::ui::{self, Icon, Variant, size};

// --- Where a kind leaves ----------------------------------------------------------------

/// The place a node of the subscription stands for.
pub fn node_place(name: &str) -> Option<Place> {
    store().proxies.with(|p| {
        p.0.groups
            .iter()
            .flat_map(|g| g.members.iter())
            .find(|m| m.name == name && !m.is_group)
            .map(places::place)
    })
}

/// The kind goes as the subscription routes it: no rule of Kairo's.
pub fn on_subscription(state: &State, kind: Kind) -> bool {
    match kind {
        Kind::Other => false,
        _ if state.settings.kinds_off => true,
        Kind::Ai => state.ai.group.is_empty(),
        k => state.settings.kinds.exit(k).is_none_or(|e| e.subscription),
    }
}

/// Kairo keeps the kind on the fastest server by itself.
pub fn on_fastest(state: &State, kind: Kind) -> bool {
    match kind {
        Kind::Ai => false,
        Kind::Other => state.settings.auto_location,
        k => state
            .settings
            .kinds
            .exit(k)
            .is_some_and(|e| !e.subscription && e.exit.is_none() && e.fastest),
    }
}

/// The kind follows Everything else (the Location).
pub fn follows_location(state: &State, kind: Kind) -> bool {
    match kind {
        Kind::Other => false,
        Kind::Ai => state.settings.kinds.ai.exit.is_none(),
        k => state
            .settings
            .kinds
            .exit(k)
            .is_some_and(|e| !e.subscription && e.exit.is_none() && !e.fastest),
    }
}

/// When the kind's server was pinned, if it is.
fn pinned_since(state: &State, kind: Kind) -> Option<i64> {
    match kind {
        Kind::Ai => state.settings.kinds.ai.exit.as_ref().map(|e| e.since),
        Kind::Other => None,
        k => state
            .settings
            .kinds
            .exit(k)
            .filter(|e| !e.subscription)
            .and_then(|e| e.exit.as_ref().map(|x| x.since)),
    }
}

/// Where the subscription's own rules send a kind's apps now: the server of
/// the first of its apps the rules name.
fn subscription_place(kind: Kind) -> Option<Place> {
    let store = store();
    let targets = store.state.with(|s| s.kinds.sub_targets.clone());
    let target = kinds::catalog()
        .iter()
        .filter(|a| a.kind == kind)
        .find_map(|a| targets.get(a.id).cloned())?;
    if let Some(p) = node_place(&target) {
        return Some(p);
    }
    // A group (often one of groups): the server it goes through now.
    let node = store.proxies.with(|p| kairo_core::ai::resolve(&p.0.groups, &target))?;
    node_place(&node)
}

/// The place `kind` goes through now.
pub fn place_of(kind: Kind) -> Option<Place> {
    let store = store();
    store.state.with(|_| ());
    store.proxies.with(|_| ());
    let state = store.state.with(|s| s.clone());
    match kind {
        Kind::Other => {
            let preferred = state.settings.tray_group.clone();
            store.proxies.with(|p| {
                let g = places::main_group(&p.0.groups, &preferred)?;
                places::current(&p.0.groups, g)
            })
        }
        k if on_subscription(&state, k) => subscription_place(k),
        Kind::Ai => crate::pages::ai::exit_place(),
        k => state.kinds.nodes.get(&k).and_then(|n| node_place(n)),
    }
}

/// The line under a kind's country: which server, and why it is there.
pub fn subtitle(kind: Kind) -> String {
    let store = store();
    let state = store.state.with(|s| s.clone());
    let node = place_of(kind)
        .map(|p| p.title)
        .unwrap_or_else(|| "—".into());
    if on_subscription(&state, kind) {
        return tr!("kinds.as_subscription");
    }
    if let Some(since) = pinned_since(&state, kind) {
        if (kairo_core::unix() - since).max(0) < 86400 {
            return tr!("exits.subtitle.pinned_today", node = node);
        }
        return tr!("exits.subtitle.pinned", node = node, when = pinned_for(since));
    }
    if on_fastest(&state, kind) {
        return tr!("exits.subtitle.fastest", node = node);
    }
    match kind {
        Kind::Other => tr!("exits.subtitle.chosen", node = node),
        _ => tr!("exits.subtitle.follows", node = node),
    }
}

/// "today", "2 days", "3 weeks": how long ago a pin was made.
pub fn pinned_for(since: i64) -> String {
    let days = (kairo_core::unix() - since).max(0) / 86400;
    match days {
        0 => tr!("exits.pinned_for.today"),
        1..=13 => tr!(&plural("exits.pinned_for.days", days as u64), count = days),
        _ => tr!("exits.pinned_for.weeks", count = days / 7),
    }
}

/// The country a place is in, in words ("Hong Kong"), from its region.
pub fn place_country(p: &Place) -> String {
    if p.region == "Other" {
        p.title.clone()
    } else {
        places::region_title(p.region)
    }
}

/// Picks `name` for `kind`.
pub fn pick(kind: Kind, name: String) {
    let store = store();
    match kind {
        Kind::Ai => store.run(store.svc().set_ai_exit(name)),
        Kind::Other => store.run(store.svc().set_location(Some(name))),
        k => store.run(store.svc().set_kind_exit(k, name)),
    }
}

/// "Fastest" for the kinds that have it.
pub fn use_fastest(kind: Kind) {
    let store = store();
    match kind {
        Kind::Ai => {}
        Kind::Other => store.run(store.svc().set_location(None)),
        k => store.run(store.svc().kind_follow_fastest(k)),
    }
}

/// "Follow Everything else".
pub fn follow_location(kind: Kind) {
    let store = store();
    match kind {
        Kind::Other => {}
        Kind::Ai => store.run(store.svc().ai_follow_location()),
        k => store.run(store.svc().kind_follow_location(k)),
    }
}

/// "As the subscription routes it": Kairo's rules for the kind go.
pub fn to_subscription(kind: Kind) {
    let store = store();
    if kinds::Kind::ROUTED.contains(&kind) {
        store.run(store.svc().kind_to_subscription(kind));
    }
}

// --- Apps -------------------------------------------------------------------------------

/// How a row reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Look,
    Bad,
    Idle,
}

pub fn tone_color(t: Tone) -> AdaptiveColor {
    match t {
        Tone::Ok => ui::success(),
        Tone::Look => ui::warning(),
        Tone::Bad => theme().destructive,
        Tone::Idle => theme().muted_foreground,
    }
}

/// The one button a row offers when its check found something.
#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    UseEveryApp,
    CopyForSsh(String),
}

impl Fix {
    pub fn title(&self) -> String {
        match self {
            Fix::UseEveryApp => tr!("kinds.fix.every_app"),
            Fix::CopyForSsh(_) => tr!("kinds.fix.copy_ssh"),
        }
    }

    pub fn apply(&self, store: Store) {
        match self {
            Fix::UseEveryApp => crate::pages::home::set_via(crate::pages::home::Via::Tun),
            Fix::CopyForSsh(text) => ui::copy(store, text.clone(), tr!("kinds.fix.ssh_copied")),
        }
    }
}

/// One app as the pages show it.
#[derive(Debug, Clone, PartialEq)]
pub struct AppRow {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Its exit (its kind's unless the person chose).
    pub exit: AppExit,
    /// What makes a connection this app, for an app of the person's.
    pub matcher: Option<RouteKind>,
    /// Bytes over the last 7 days (or since Kairo opened, if more).
    pub bytes: u64,
    pub open: u32,
    /// The programs that used it, most traffic first.
    pub programs: Vec<String>,
    pub tone: Tone,
    pub status: String,
    pub fix: Option<Fix>,
}

impl AppRow {
    /// The app's picture.
    pub fn logo(&self, pts: f32) -> AnyElement {
        logo_of(&self.id, &self.name, self.matcher.as_ref(), pts)
    }
}

/// An app's picture: the bundled logo of its first site, else its letters.
pub fn logo_of(id: &str, name: &str, matcher: Option<&RouteKind>, pts: f32) -> AnyElement {
    use ui::logo::Subject;
    let subject = match (matcher, kinds::catalog_app(id)) {
        (Some(RouteKind::App { name }), _) => Subject::App(name.clone()),
        (Some(RouteKind::Site { domain }), _) => Subject::Site(domain.clone()),
        (None, Some(a)) if a.kind == Kind::Ai => Subject::Service(a.id, letters(a.id, a.name)),
        (None, Some(a)) if !a.suffixes.is_empty() => Subject::Site(a.suffixes[0].to_owned()),
        (None, Some(a)) if !a.programs.is_empty() => Subject::App(a.programs[0].to_owned()),
        _ => Subject::App(name.to_owned()),
    };
    ui::logo::logo(subject, pts)
}

/// The letters shown for an AI service with no picture.
fn letters(id: &str, name: &'static str) -> &'static str {
    kairo_core::ai::SERVICES.iter().find(|s| s.id == id).map(|s| s.mono).unwrap_or(name)
}

/// The ISO code's country, in words.
pub fn country(code: &str) -> String {
    ai_check::country_name(code)
}

/// The host whose SSH a code host's check tries.
fn ssh_host(id: &str) -> Option<&'static str> {
    match id {
        "github" => Some("github.com"),
        "gitlab" => Some("gitlab.com"),
        _ => None,
    }
}

/// How an app's check reads on its row, and the fix when there is one.
pub fn status_of(state: &State, id: &str, kind: Kind, exit: &AppExit, bytes: u64) -> (Tone, String, Option<Fix>) {
    let bypassing = state.bypass.iter().any(|b| b.game.as_deref() == Some(id));
    if bypassing {
        return (Tone::Bad, tr!("kinds.status.not_through_kairo"), Some(Fix::UseEveryApp));
    }
    match exit {
        AppExit::Direct => return (Tone::Idle, tr!("kinds.exit.direct"), None),
        AppExit::Block => return (Tone::Idle, tr!("kinds.status.blocked"), None),
        _ => {}
    }
    if kind == Kind::Ai {
        let check = state.ai.checks.iter().find(|c| c.service == id);
        return match check {
            Some(c) if !c.reachable => (Tone::Bad, tr!("kinds.status.unreachable"), None),
            Some(c) => match c.country.code() {
                Some(code) => {
                    let name = country(code);
                    let name = match c.country {
                        ai_check::Country::Estimated { .. } => format!("≈ {name}"),
                        _ => name,
                    };
                    (Tone::Ok, tr!("kinds.status.sees", country = name), None)
                }
                None => (Tone::Ok, tr!("kinds.status.reachable"), None),
            },
            None => (Tone::Idle, tr!("kinds.not_checked"), None),
        };
    }
    match state.kinds.check(id) {
        Some(c) => {
            let fix = match &c.measured {
                Measured::DevTools { ssh: Ssh::ClosedUse443, .. } => ssh_host(id)
                    .and_then(kind_check::ssh_snippet)
                    .map(Fix::CopyForSsh),
                _ => None,
            };
            let tone = match &c.measured {
                Measured::Streaming { reachable: false, .. }
                | Measured::Downloads { reachable: false, .. }
                | Measured::DevTools { reachable: false, .. }
                | Measured::Games { median_ms: None, .. } => Tone::Bad,
                _ if c.trouble() => Tone::Look,
                _ => Tone::Ok,
            };
            (tone, c.line(country), fix)
        }
        None if matches!(kind, Kind::Other) => (Tone::Idle, tr!("kinds.status.this_week", bytes = kairo_core::bytes(bytes)), None),
        None if kind_check::probe_for(id).is_some() => (
            Tone::Idle,
            if state.kinds.checking { tr!("kinds.checking") } else { tr!("kinds.not_checked") },
            None,
        ),
        None => (Tone::Idle, tr!("kinds.status.this_week", bytes = kairo_core::bytes(bytes)), None),
    }
}

/// The apps to show: those used in the last 7 days (any traffic, or a
/// connection open now) and those the person made a choice for. With
/// `min`, only those with at least that much (Home's [`kinds::USED_BYTES`]).
pub fn rows(state: &State, tally: &Tally, week: &[(String, u64)], min: u64) -> Vec<AppRow> {
    let mut ids: Vec<(String, u64)> = Vec::new();
    let empty = AppUse::default();
    let seen = tally.kapps();
    for (id, u) in &seen {
        let past = week.iter().find(|(w, _)| w == id).map_or(0, |(_, b)| *b);
        ids.push((id.to_string(), u.total().max(past)));
    }
    for (id, b) in week {
        if id != kairo_core::stats::OTHER_KAPP && !ids.iter().any(|(i, _)| i == id) {
            ids.push((id.clone(), *b));
        }
    }
    for a in &state.settings.apps {
        if !ids.iter().any(|(i, _)| *i == a.id) {
            ids.push((a.id.clone(), 0));
        }
    }
    let mut out = Vec::new();
    for (id, bytes) in ids {
        let choice = state.settings.apps.iter().find(|a| a.id == id);
        let catalog = kinds::catalog_app(&id);
        let u = tally.kapp(&id).unwrap_or(&empty);
        let is_choice = choice.is_some();
        if (bytes < min.max(1) && u.open == 0) && !(min == 0 && is_choice) {
            continue;
        }
        let kind = choice
            .map(|c| c.kind)
            .or(catalog.map(|c| c.kind))
            .unwrap_or(u.kind);
        let exit = choice
            .map(|c| c.exit.clone())
            .or(catalog.map(kinds::default_exit))
            .unwrap_or(AppExit::Kind);
        let name = choice
            .map(AppChoice::name)
            .or(catalog.map(|c| kairo_core::ai::domestic_title(c.name)))
            .or((!u.name.is_empty()).then(|| u.name.clone()))
            .unwrap_or_else(|| id.clone());
        let mut programs: Vec<(String, u64)> = u.programs.iter().map(|(p, b)| (p.clone(), *b)).collect();
        programs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let (tone, status, fix) = status_of(state, &id, kind, &exit, bytes);
        out.push(AppRow {
            matcher: choice.and_then(|c| c.matcher.clone()),
            id,
            name,
            kind,
            exit,
            bytes,
            open: u.open,
            programs: programs.into_iter().map(|(p, _)| p).collect(),
            tone,
            status,
            fix,
        });
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name)));
    out
}

/// The 7 days' bytes per app, from the statistics.
pub fn week(store: Store) -> Vec<(String, u64)> {
    let now = kairo_core::unix();
    store.stats.with(|s| {
        s.0.top_kapps(kairo_core::stats::Range::Week, now, kairo_core::stats::local_offset(now))
    })
}

/// Every app shown, reactive.
pub fn all_rows(min: u64) -> Memo<Vec<AppRow>> {
    let store = store();
    computed(move || {
        let week = week(store);
        store.state.with(|s| store.activity.with(|a| rows(s, &a.0, &week, min)))
    })
}

// --- An app's exit ----------------------------------------------------------------------

/// The words for an app's exit.
pub fn exit_title(exit: &AppExit, kind: Kind) -> String {
    match exit {
        AppExit::Kind => tr!("kinds.exit.same_as", kind = kind.in_sentence()).trim().to_owned(),
        AppExit::Subscription => tr!("kinds.as_subscription"),
        AppExit::Direct => tr!("kinds.exit.direct"),
        AppExit::Block => tr!("kinds.exit.block"),
        AppExit::Everything => Kind::Other.title(),
        AppExit::Of(k) => tr!("kinds.exit.same_as", kind = k.in_sentence()).trim().to_owned(),
        AppExit::Node(name) => node_title(name),
    }
}

/// A node as a country: its flag and the country it is in.
pub fn node_title(name: &str) -> String {
    let p = places::split_flag(name);
    let country = places::place(&kairo_core::model::Node {
        name: name.to_owned(),
        kind: String::new(),
        udp: false,
        delay: None,
        alive: true,
        is_group: false,
    })
    .region;
    let flag = p.0.or_else(|| places::region_flag(country)).unwrap_or_default();
    if country == "Other" {
        format!("{flag} {}", p.1.trim()).trim().to_owned()
    } else {
        format!("{flag} {}", places::region_title(country)).trim().to_owned()
    }
}

/// The exit menu's choices for an app of `kind`: its kind, Everything else,
/// the subscription's way, Direct, a country (its fastest server), Block.
pub fn exit_choices(kind: Kind, current: &AppExit) -> Vec<(AppExit, String)> {
    let store = store();
    let mut out = vec![(AppExit::Kind, exit_title(&AppExit::Kind, kind))];
    if kind != Kind::Other {
        out.push((AppExit::Everything, Kind::Other.title()));
    }
    out.push((AppExit::Subscription, exit_title(&AppExit::Subscription, kind)));
    out.push((AppExit::Direct, tr!("kinds.exit.direct")));
    let preferred = store.state.with_untracked(|s| s.settings.tray_group.clone());
    store.proxies.with_untracked(|p| {
        if let Some(g) = places::main_group(&p.0.groups, &preferred) {
            let all = places::places(&p.0.groups, g);
            for (region, list) in places::by_region(&all) {
                let best = list
                    .iter()
                    .filter(|p| p.delay.is_some_and(|d| d > 0))
                    .min_by_key(|p| p.delay)
                    .or(list.first());
                if let Some(best) = best {
                    // The country the chosen node is in stands for it.
                    let exit = match current {
                        AppExit::Node(n) if list.iter().any(|p| p.name == *n) => AppExit::Node(n.clone()),
                        _ => AppExit::Node(best.name.clone()),
                    };
                    let flag = best
                        .flag
                        .clone()
                        .or_else(|| places::region_flag(region))
                        .unwrap_or_default();
                    out.push((exit, format!("{flag} {}", places::region_title(region)).trim().to_owned()));
                }
            }
        }
    });
    out.push((AppExit::Block, tr!("kinds.exit.block")));
    if !out.iter().any(|(e, _)| e == current) {
        out.push((current.clone(), exit_title(current, kind)));
    }
    out
}

/// Sets an app's kind and exit.
pub fn set_app(id: String, matcher: Option<RouteKind>, kind: Kind, exit: AppExit) {
    let store = store();
    store.run(store.svc().set_app(id, matcher, kind, exit));
}

/// The app can be given an exit of its own (a game Kairo found by its folder
/// cannot: its kind's exit is where it goes).
pub fn choosable(id: &str, matcher: Option<&RouteKind>) -> bool {
    matcher.is_some() || kinds::catalog_app(id).is_some()
}

/// An app's exit as a pop-up.
pub fn exit_button(r: &AppRow) -> AnyElement {
    if !choosable(&r.id, r.matcher.as_ref()) {
        return ui::muted(exit_title(&r.exit, r.kind)).into_element();
    }
    let kind = r.kind;
    let current = r.exit.clone();
    let shown = stored(r.exit.clone());
    let id = stored(r.id.clone());
    let matcher = stored(r.matcher.clone());
    ui::popup_button(
        move || exit_choices(kind, &current),
        move || shown.with(|e| e.clone()),
        move |e: AppExit| set_app(id.with(|i| i.clone()), matcher.with(|m| m.clone()), kind, e),
    )
    .into_element()
}

/// A row of an app: its logo and name, the check's line, and its exit.
pub fn app_row(r: AppRow) -> AnyElement {
    let store = store();
    let id = r.id.clone();
    let tone = r.tone;
    let phone = ui::phone();
    nsh_base::button()
        .accessibility_label(r.name.clone())
        .on_press(move || store.app_sheet.set(Some(id.clone())))
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .min_h(px(ui::touch_h(44.0)))
        .py_1()
        .cursor_pointer()
        .hover(|s| s.bg(move || theme().accent))
        .when(phone, |v| v.flex_wrap().gap_y_1())
        .child(r.logo(24.0))
        .child(ui::body(r.name.clone()).number_of_lines(1).w(px(if phone { 120.0 } else { 160.0 })).flex_shrink(1.0).min_w(px(72.0)))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .min_w(px(0.0))
                .child(ui::dot(move || tone_color(tone)))
                .child(ui::muted(r.status.clone()).number_of_lines(1).flex_1()),
        )
        .child(r.fix.clone().map(|f| {
            let fix = f.clone();
            ui::capsule(f.title(), true).h(px(26.0)).px_3().on_press(move || fix.apply(store))
        }))
        .child(exit_button(&r))
        .into_element()
}

/// A program's menu (Activity's rows): its own exit, or an app of a kind.
/// A browser is never an app: its menu only shows its connections.
pub fn program_menu(name: String, show: impl Fn() + 'static) {
    use crate::ui::menu::{self, Choice};
    let store = store();
    let mut items = Vec::new();
    // "Unknown" is the tally's name for traffic of no known program: no program to choose for.
    if !kinds::is_browser(&name) && name != "Unknown" {
        let n = stored(name.clone());
        let set = move |kind: Kind, exit: AppExit| {
            move || {
                store.run_then(
                    store.svc().set_app(String::new(), Some(RouteKind::App { name: n.with(|n| n.clone()) }), kind, exit.clone()),
                    move |r| {
                        if r.is_ok() {
                            store.info(tr!("activity.route.saved"));
                        }
                    },
                )
            }
        };
        items.push(Choice::submenu(
            tr!("kinds.menu.exit_for_program"),
            exit_choices(Kind::Other, &AppExit::Kind)
                .into_iter()
                .filter(|(e, _)| *e != AppExit::Kind)
                .map(|(exit, title)| Choice::new(title).on_select(set(Kind::Other, exit)))
                .collect::<Vec<_>>(),
        ));
        items.push(Choice::submenu(
            tr!("kinds.menu.add_as_app"),
            Kind::ALL
                .into_iter()
                .map(|kind| Choice::new(kind.title()).on_select(set(kind, AppExit::Kind)))
                .collect::<Vec<_>>(),
        ));
        items.push(Choice::separator());
    }
    items.push(Choice::new(tr!("activity.show_connections")).on_select(show));
    menu::open(items);
}

// --- The sheet --------------------------------------------------------------------------

/// The app's sheet, over the page, while one is open.
pub fn sheet_host() -> impl IntoElement {
    let store = store();
    dynamic(move || {
        let open = store.app_sheet.get();
        untracked(move || match open {
            Some(id) => sheet(id).into_element(),
            None => view().into_element(),
        })
    })
}

fn sheet(id: String) -> impl IntoElement {
    let store = store();
    let close = move || store.app_sheet.set(None);
    view()
        .absolute()
        .inset_0()
        .child(
            overlay()
                .absolute()
                .inset_0()
                .bg(move || theme().foreground.opacity(0.28))
                .on_press(close),
        )
        .child(
            overlay()
                .absolute()
                .inset_0()
                .items_center()
                .justify_center()
                .px_4()
                .pointer_events(nsh_core::PointerEvents::BoxNone)
                .on_cancel(close)
                .child(sheet_card(id)),
        )
}

/// What Kairo goes by to know a connection is this app.
fn recognised_by(id: &str, matcher: Option<&RouteKind>) -> String {
    if let Some(m) = matcher {
        return match m {
            RouteKind::Site { domain } => tr!("kinds.by.site", domain = domain),
            RouteKind::App { name } => tr!("kinds.by.program", name = name),
        };
    }
    if let Some(folder) = id.strip_prefix("game:") {
        return tr!("kinds.by.game", folder = folder);
    }
    let Some(a) = kinds::catalog_app(id) else {
        return id.to_owned();
    };
    let mut parts: Vec<String> = Vec::new();
    if !a.suffixes.is_empty() {
        let more = a.suffixes.len().saturating_sub(3);
        let mut line = a.suffixes.iter().take(3).copied().collect::<Vec<_>>().join(" · ");
        if more > 0 {
            line = tr!("kinds.by.more", sites = line, count = more);
        }
        parts.push(line);
    }
    if !a.programs.is_empty() {
        parts.push(tr!("kinds.by.catalog_program", name = a.programs[0]));
    }
    if !a.categories.is_empty() {
        parts.push(tr!("kinds.by.rule_sets", names = a.categories.iter().take(2).copied().collect::<Vec<_>>().join(", ")));
    }
    parts.join(" · ")
}

/// One line of what was measured.
fn measured_row(ok: Option<bool>, title: String, value: String) -> AnyElement {
    let (mark, tone) = match ok {
        Some(true) => ("✓", ui::success()),
        Some(false) => ("!", ui::warning()),
        None => ("–", theme().muted_foreground),
    };
    ui::row_view()
        .child(text(mark).w(px(18.0)).text_color(move || tone).font_weight(FontWeight::SEMIBOLD))
        .child(ui::body(title).w(px(150.0)))
        .child(ui::muted(value).flex_1().number_of_lines(2))
        .into_element()
}

/// What the kind's check measured, a line each.
fn measured_rows(kind: Kind, c: &KindCheck) -> Vec<AnyElement> {
    let through = tr!("kinds.m.through", kind = kind.in_sentence());
    match &c.measured {
        Measured::Streaming { reachable, catalog, country: cc, mbps } => {
            use kind_check::Catalog;
            let mut v = vec![measured_row(
                Some(*reachable),
                tr!("kinds.m.reachable"),
                if *reachable { through } else { tr!("kinds.m.no_answer") },
            )];
            let region = match (catalog, cc) {
                (Catalog::Full, Some(code)) => (Some(true), tr!("kinds.m.catalog", code = code)),
                (Catalog::Full, None) => (Some(true), tr!("kinds.m.full_catalog")),
                (Catalog::OriginalsOnly, _) => (Some(false), tr!("kinds.m.originals_only")),
                (Catalog::NotAvailable, _) => (Some(false), tr!("kinds.m.not_available")),
                (Catalog::Unknown, Some(code)) => (None, tr!("kinds.status.sees", country = country(code))),
                (Catalog::Unknown, None) => (None, tr!("kinds.m.did_not_say")),
            };
            v.push(measured_row(region.0, tr!("kinds.m.region"), region.1));
            v.push(measured_row(
                mbps.map(|_| true),
                tr!("kinds.m.speed"),
                mbps.map(|m| format!("{m} Mbit/s")).unwrap_or_else(|| tr!("kinds.m.too_little")),
            ));
            v
        }
        Measured::Games { median_ms, jitter_ms, loss_pct } => vec![
            measured_row(
                median_ms.map(|m| m <= 150),
                tr!("kinds.m.latency"),
                median_ms.map(|m| tr!("kinds.m.latency_ms", ms = m)).unwrap_or_else(|| tr!("kinds.m.no_answer")),
            ),
            measured_row(
                jitter_ms.map(|j| j <= 30),
                tr!("kinds.m.jitter"),
                jitter_ms.map(|j| format!("{j} ms")).unwrap_or_else(|| "—".into()),
            ),
            measured_row(Some(*loss_pct <= 10), tr!("kinds.m.lost"), tr!("kinds.m.lost_pct", pct = loss_pct)),
        ],
        Measured::Downloads { reachable, mb_per_s_x10 } => vec![
            measured_row(Some(*reachable), tr!("kinds.m.reachable"), if *reachable { through } else { tr!("kinds.m.no_answer") }),
            measured_row(
                mb_per_s_x10.map(|x| x >= 10),
                tr!("kinds.m.speed"),
                if mb_per_s_x10.is_some() && *reachable { c.line(country) } else { tr!("kinds.m.too_little") },
            ),
        ],
        Measured::DevTools { reachable, ttfb_ms, ssh } => vec![
            measured_row(Some(*reachable), tr!("kinds.m.reachable"), if *reachable { through } else { tr!("kinds.m.no_answer") }),
            measured_row(
                ttfb_ms.map(|_| true),
                tr!("kinds.m.first_byte"),
                ttfb_ms.map(|m| format!("{m} ms")).unwrap_or_else(|| "—".into()),
            ),
            match ssh {
                Ssh::Open => measured_row(Some(true), "SSH".into(), tr!("kinds.m.ssh_open")),
                Ssh::ClosedUse443 => measured_row(Some(false), "SSH".into(), tr!("kinds.m.ssh_443")),
                Ssh::Closed => measured_row(Some(false), "SSH".into(), tr!("kinds.m.ssh_closed")),
                Ssh::NotApplicable => measured_row(None, "SSH".into(), tr!("kinds.m.ssh_none")),
            },
        ],
    }
}

fn sheet_card(id: String) -> impl IntoElement {
    let store = store();
    let all = all_rows(0);
    let key = id.clone();
    dynamic(move || {
        let row = all.with(|rows| rows.iter().find(|r| r.id == key).cloned());
        let (check, checking, ai_check) = store.state.with(|s| {
            (
                s.kinds.check(&key).cloned(),
                s.kinds.checking || s.ai.checking,
                s.ai.checks.iter().find(|c| c.service == key).cloned(),
            )
        });
        let days = {
            let now = kairo_core::unix();
            store.stats.with(|s| s.0.kapp_days(&key, now, kairo_core::stats::local_offset(now), 7))
        };
        let id = key.clone();
        untracked(move || {
            let Some(r) = row.or_else(|| {
                // An app with nothing this week: what the catalog says.
                let a = kinds::catalog_app(&id)?;
                Some(AppRow {
                    id: id.clone(),
                    name: kairo_core::ai::domestic_title(a.name),
                    kind: a.kind,
                    exit: kinds::default_exit(a),
                    matcher: None,
                    bytes: 0,
                    open: 0,
                    programs: vec![],
                    tone: Tone::Idle,
                    status: String::new(),
                    fix: None,
                })
            }) else {
                return view().into_element();
            };
            let kind = r.kind;
            let exit_line = {
                let place = match &r.exit {
                    AppExit::Kind => place_of(kind),
                    AppExit::Of(k) => place_of(*k),
                    AppExit::Everything => place_of(Kind::Other),
                    AppExit::Node(n) => node_place(n),
                    _ => None,
                };
                let mut t = exit_title(&r.exit, kind);
                if let (Some(p), false) = (place, matches!(r.exit, AppExit::Node(_))) {
                    t = format!("{t} · {} {}", p.flag.clone().unwrap_or_default(), place_country(&p));
                }
                t
            };
            let measured: Vec<AnyElement> = if kind == Kind::Ai {
                match &ai_check {
                    Some(c) => vec![
                        measured_row(Some(c.reachable), tr!("kinds.m.reachable"), if c.reachable { tr!("kinds.m.through", kind = Kind::Ai.in_sentence()) } else { tr!("kinds.m.no_answer") }),
                        measured_row(
                            c.country.code().map(|_| true),
                            tr!("kinds.m.sees"),
                            c.country.code().map(country).unwrap_or_else(|| "—".into()),
                        ),
                    ],
                    None => vec![],
                }
            } else {
                check.as_ref().map(|c| measured_rows(kind, c)).unwrap_or_default()
            };
            let checked_at = if kind == Kind::Ai { ai_check.as_ref().map(|c| c.at) } else { check.as_ref().map(|c| c.at) };
            let has_check = kind == Kind::Ai || kind_check::probe_for(&r.id).is_some();
            let total: u64 = days.iter().map(|(_, b)| *b).sum();
            let peak = days.iter().map(|(_, b)| *b).max().unwrap_or(0).max(1);
            let exit_id = stored(r.id.clone());
            let exit_matcher = stored(r.matcher.clone());
            let exit_now = stored(r.exit.clone());
            let kind_now = r.kind;
            let custom = r.matcher.is_some();
            let remove_id = r.id.clone();
            let check_id = r.id.clone();
            let fix = r.fix.clone();
            view()
                .w_full()
                .max_w(px(560.0))
                .max_h(percentage(0.9))
                .rounded(px(18.0))
                .bg(move || theme().background)
                .border_1()
                .border_color(move || theme().border)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .px_5()
                        .pt_5()
                        .pb_3()
                        .child(r.logo(36.0))
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    text(r.name.clone())
                                        .text_size(px(size::large() + 3.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .role(AccessibilityRole::Header),
                                )
                                .child(ui::muted(recognised_by(&r.id, r.matcher.as_ref())).number_of_lines(1)),
                        )
                        .child(ui::icon_btn(Icon::Close, tr!("kinds.sheet.close")).on_press(move || store.app_sheet.set(None))),
                )
                .child(
                    scroll().child(
                        view()
                            .px_5()
                            .pb_5()
                            .gap_4()
                            .child(ui::rows(vec![
                                ui::row_view()
                                    .child(ui::body(tr!("kinds.sheet.exit")).flex_1())
                                    .child(if choosable(&r.id, r.matcher.as_ref()) {
                                        ui::popup_button(
                                            move || {
                                                let now = exit_now.with(|e| e.clone());
                                                exit_choices(kind_now, &now)
                                                    .into_iter()
                                                    .map(|(e, t)| if e == now { (e, exit_line.clone()) } else { (e, t) })
                                                    .collect()
                                            },
                                            move || exit_now.with(|e| e.clone()),
                                            move |e: AppExit| set_app(exit_id.with(|i| i.clone()), exit_matcher.with(|m| m.clone()), kind_now, e),
                                        )
                                        .into_element()
                                    } else {
                                        ui::muted(exit_line.clone()).into_element()
                                    })
                                    .into_element(),
                                ui::row_view()
                                    .child(ui::body(tr!("kinds.sheet.kind")).flex_1())
                                    .child(if choosable(&r.id, r.matcher.as_ref()) {
                                        ui::popup_button(
                                            || Kind::ALL.into_iter().map(|k| (k, k.title())).collect(),
                                            move || kind_now,
                                            move |k: Kind| {
                                                // Another kind: the app goes with that kind.
                                                set_app(exit_id.with(|i| i.clone()), exit_matcher.with(|m| m.clone()), k, AppExit::Kind)
                                            },
                                        )
                                        .into_element()
                                    } else {
                                        ui::muted(kind.title()).into_element()
                                    })
                                    .into_element(),
                            ]))
                            .child(has_check.then(|| {
                                view()
                                    .gap_2()
                                    .child(ui::section_label(tr!("kinds.sheet.measured")).px_1())
                                    .child(if measured.is_empty() {
                                        ui::rows(vec![
                                            ui::row_view()
                                                .child(ui::muted(if checking { tr!("kinds.checking") } else { tr!("kinds.not_checked") }))
                                                .into_element(),
                                        ])
                                    } else {
                                        ui::rows(measured)
                                    })
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .px_1()
                                            .child(
                                                ui::muted(
                                                    checked_at
                                                        .map(|at| tr!("kinds.sheet.checked", ago = crate::pages::ai::ago(kairo_core::unix(), at)))
                                                        .unwrap_or_default(),
                                                )
                                                .text_size(px(size::small()))
                                                .flex_1(),
                                            )
                                            .child(fix.clone().map(|f| {
                                                let fx = f.clone();
                                                ui::capsule(f.title(), true).h(px(28.0)).px_4().on_press(move || fx.apply(store))
                                            }))
                                            .child(
                                                ui::capsule(if checking { tr!("kinds.checking") } else { tr!("kinds.check_now") }, false)
                                                    .h(px(28.0))
                                                    .px_4()
                                                    .disabled(move || checking)
                                                    .on_press(move || {
                                                        if kind == Kind::Ai {
                                                            store.run(store.svc().check_ai())
                                                        } else {
                                                            store.run(store.svc().check_app(check_id.clone()))
                                                        }
                                                    }),
                                            ),
                                    )
                            }))
                            .child({
                                let phone = ui::phone();
                                let row = view().gap_4();
                                let row = if phone { row } else { row.flex_row().items_start() };
                                row.child(
                                    view()
                                        .gap_2()
                                        .flex_1()
                                        .child(ui::section_label(tr!("kinds.sheet.used_by")).px_1())
                                        .child(if r.programs.is_empty() {
                                            ui::muted(tr!("kinds.sheet.nothing_this_week")).px_1().into_element()
                                        } else {
                                            view()
                                                .flex_row()
                                                .flex_wrap()
                                                .gap_3()
                                                .px_1()
                                                .children(r.programs.iter().take(4).map(|p| {
                                                    view()
                                                        .flex_row()
                                                        .items_center()
                                                        .gap_1p5()
                                                        .child(ui::logo::logo(ui::logo::Subject::App(p.clone()), 18.0))
                                                        .child(ui::body(ui::logo::app_name(p.clone())).text_size(px(size::small())))
                                                        .into_element()
                                                }))
                                                .into_element()
                                        }),
                                )
                                .child(
                                    view()
                                        .gap_2()
                                        .flex_1()
                                        .child(ui::section_label(tr!("kinds.sheet.this_week", bytes = kairo_core::bytes(total))).px_1())
                                        .child(
                                            view()
                                                .flex_row()
                                                .items_end()
                                                .gap_1p5()
                                                .h(px(44.0))
                                                .px_1()
                                                .children(days.iter().enumerate().map(|(i, (_, b))| {
                                                    let h = (*b as f32 / peak as f32 * 44.0).max(3.0);
                                                    let last = i + 1 == days.len();
                                                    view()
                                                        .flex_1()
                                                        .h(px(h))
                                                        .rounded(px(3.0))
                                                        .bg(move || if last { theme().primary } else { ui::track() })
                                                        .into_element()
                                                })),
                                        ),
                                )
                            })
                            .child(custom.then(|| {
                                view().flex_row().child(
                                    ui::btn(tr!("kinds.sheet.remove"), Variant::Secondary)
                                        .on_press(move || {
                                            store.run(store.svc().remove_app(remove_id.clone()));
                                            store.app_sheet.set(None);
                                        }),
                                )
                            }))
                            .child(
                                ui::muted(tr!("ai.detail.note"))
                                    .text_size(px(size::small())),
                            ),
                    ),
                )
                .into_element()
        })
    })
}

/// Opens the Apps page on `kind`.
pub fn open_kind(kind: Kind) {
    crate::pages::exits::select(Some(kind));
    store().page.set(Page::Apps);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exits_are_named_for_people() {
        assert_eq!(exit_title(&AppExit::Kind, Kind::Streaming), "Same as Streaming");
        assert_eq!(exit_title(&AppExit::Of(Kind::Ai), Kind::Other), "Same as AI");
        assert_eq!(exit_title(&AppExit::Everything, Kind::Games), "Everything else");
        assert_eq!(exit_title(&AppExit::Node("🇺🇸 Seattle 02".into()), Kind::Ai), "🇺🇸 United States");
    }

    #[test]
    fn a_pin_is_dated_in_days() {
        let now = kairo_core::unix();
        assert_eq!(pinned_for(now), "today");
        assert_eq!(pinned_for(now - 5 * 86400), "5 days");
        assert_eq!(pinned_for(now - 21 * 86400), "3 weeks");
    }

    #[test]
    fn a_game_around_the_proxy_says_so_and_offers_every_app() {
        let mut state = State::default();
        state.bypass.push(kairo_core::bypass::Bypass {
            tool: "cs2".into(),
            service: None,
            game: Some("game:Counter-Strike Global Offensive".into()),
            pid: 1,
            remote: "1.2.3.4:27015".into(),
            since: 0,
        });
        let (tone, line, fix) = status_of(&state, "game:Counter-Strike Global Offensive", Kind::Games, &AppExit::Kind, 0);
        assert_eq!((tone, line.as_str(), fix), (Tone::Bad, "Not through Kairo", Some(Fix::UseEveryApp)));
    }

    #[test]
    fn ssh_closed_on_22_offers_the_snippet() {
        let mut state = State::default();
        state.kinds.checks.push(KindCheck {
            app: "github".into(),
            at: 0,
            measured: Measured::DevTools { reachable: true, ttfb_ms: Some(80), ssh: Ssh::ClosedUse443 },
        });
        let (tone, line, fix) = status_of(&state, "github", Kind::DevTools, &AppExit::Kind, 0);
        assert_eq!(tone, Tone::Look);
        assert_eq!(line, "SSH port 22 closed");
        assert!(matches!(fix, Some(Fix::CopyForSsh(s)) if s.contains("ssh.github.com")));
    }
}
