//! Routes the person chose for one app or one site ("Apps & Sites"): this
//! program direct, that site on the AI exit, this one blocked. They become
//! rules at the front of the config (after the user's own override, which
//! still wins, and after the never-proxied local block), so they win over the
//! AI layer's rules and the subscription's.
//!
//! A rule is one comma-separated line, so a value that carries a comma (or a
//! line break) could add fields of its own to it. Values are checked here
//! (never escaped), and a target that is not in the config is never written:
//! the core refuses a config with a rule that points nowhere, and then
//! nothing would connect.

use serde::{Deserialize, Serialize};

/// What a route is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteKind {
    /// A program, by its process name (`claude`, `Cursor`).
    App { name: String },
    /// A site and everything under it (`example.com`).
    Site { domain: String },
}

/// Where a route sends its traffic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteExit {
    /// No rule: the subscription decides. Never stored.
    Usual,
    Direct,
    /// The AI exit.
    Ai,
    /// The Location ("Everything else").
    Everything,
    /// The Downloads exit.
    Downloads,
    Block,
    /// One node of the subscription, by name.
    Node(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    /// `app:<name>` or `site:<domain>`: one route per app or site.
    pub id: String,
    pub kind: RouteKind,
    pub exit: RouteExit,
}

impl RouteKind {
    pub fn app(name: impl Into<String>) -> RouteKind {
        RouteKind::App { name: name.into() }
    }

    pub fn site(domain: impl Into<String>) -> RouteKind {
        RouteKind::Site {
            domain: domain.into(),
        }
    }

    /// The same kind with its value checked and in its usual form.
    pub fn cleaned(&self) -> Result<RouteKind, String> {
        match self {
            RouteKind::App { name } => clean_app(name).map(RouteKind::app),
            RouteKind::Site { domain } => clean_site(domain).map(RouteKind::site),
        }
    }

    /// The id of the route for this (cleaned) kind.
    pub fn id(&self) -> String {
        match self {
            RouteKind::App { name } => format!("app:{name}"),
            RouteKind::Site { domain } => format!("site:{domain}"),
        }
    }
}

impl Route {
    /// A route for `kind` (checked) to `exit`.
    pub fn new(kind: RouteKind, exit: RouteExit) -> Result<Route, String> {
        let kind = kind.cleaned()?;
        if let RouteExit::Node(n) = &exit {
            clean_target(n)?;
        }
        Ok(Route {
            id: kind.id(),
            kind,
            exit,
        })
    }
}

/// A program's name as a rule takes it: not empty, no comma, no line break
/// or other control character, no path (a rule matches the name only).
pub fn clean_app(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter the app's name".into());
    }
    if name.chars().count() > 128 {
        return Err("That app name is too long".into());
    }
    if name.contains(',') {
        return Err("An app name cannot contain a comma".into());
    }
    if name.chars().any(char::is_control) {
        return Err("An app name cannot contain line breaks or control characters".into());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("Use the app's name, not its path".into());
    }
    Ok(name.to_owned())
}

/// A site as a rule takes it: lower case, a name with dots, no scheme, path,
/// port, wildcard, space or comma. A leading `www.` stays (it is a different
/// name from the site under it); a leading dot or `*.` is dropped; a name in
/// another script is turned into its ASCII form.
pub fn clean_site(domain: &str) -> Result<String, String> {
    let mut d = domain.trim().to_lowercase();
    if d.contains("://") {
        return Err("Enter only the site's name, without http:// or https://".into());
    }
    if let Some(rest) = d.strip_prefix("*.") {
        d = rest.to_owned();
    }
    let d = d.trim_start_matches('.').trim_end_matches('.').to_owned();
    if d.is_empty() {
        return Err("Enter the site's name".into());
    }
    if d.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("A site name cannot contain spaces".into());
    }
    if d.contains('/') || d.contains('?') || d.contains('#') {
        return Err("Enter only the site's name, without a path".into());
    }
    if d.contains(':') || d.contains('@') {
        return Err("Enter only the site's name, without a port or a user".into());
    }
    if d.contains(',') || d.contains('*') {
        return Err("A site name cannot contain commas or wildcards".into());
    }
    let d = if d.is_ascii() {
        d
    } else {
        match url::Host::parse(&d) {
            Ok(url::Host::Domain(s)) => s,
            _ => return Err("That is not a site name".into()),
        }
    };
    if d.len() > 253 {
        return Err("That site name is too long".into());
    }
    let labels: Vec<&str> = d.split('.').collect();
    let ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    };
    if labels.len() < 2 || !labels.iter().all(|l| ok(l)) {
        return Err("That is not a site name (like example.com)".into());
    }
    // An address, not a name.
    if labels
        .last()
        .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("Use a site's name, not an address".into());
    }
    Ok(d)
}

/// A node or group name as the third field of a rule takes it.
pub fn clean_target(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Choose a server".into());
    }
    if name.contains(',') || name.chars().any(char::is_control) {
        return Err(
            "That server's name has a comma or a line break, which a route cannot use".into(),
        );
    }
    Ok(())
}

/// Puts `route` in the list: one per app or site (it replaces), and
/// `Usual` removes (the subscription decides again).
pub fn upsert(routes: &mut Vec<Route>, route: Route) {
    if route.exit == RouteExit::Usual {
        routes.retain(|r| r.id != route.id);
        return;
    }
    match routes.iter_mut().find(|r| r.id == route.id) {
        Some(slot) => *slot = route,
        None => routes.push(route),
    }
}

/// A stored list made safe to use: values checked again (a hand-edited
/// settings file), ids recomputed, `Usual` and duplicates dropped.
pub fn sanitize(routes: Vec<Route>) -> Vec<Route> {
    let mut out = Vec::new();
    for r in routes {
        match Route::new(r.kind.clone(), r.exit.clone()) {
            Ok(clean) => upsert(&mut out, clean),
            Err(e) => log::warn!("a stored route is dropped ({}): {e}", r.id),
        }
    }
    out
}

/// The groups routes can point at, as they are named in the config. `None`:
/// there is no such group (the AI layer is off, or there is no Location).
#[derive(Debug, Clone, Copy, Default)]
pub struct Targets<'a> {
    pub ai: Option<&'a str>,
    pub everything: Option<&'a str>,
    pub downloads: Option<&'a str>,
}

/// The rules for `routes`, and the ids of the routes left out.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Generated {
    pub rules: Vec<String>,
    pub skipped: Vec<String>,
}

fn usable(target: &str) -> Option<&str> {
    clean_target(target).is_ok().then_some(target)
}

/// Writes the rules: apps first, then sites (the longer name first, so
/// `a.example.com` is read before `example.com`), each group in the order
/// they were added. With the AI layer off `Ai` and `Downloads` fall back to
/// the Location. A route whose target is not there (a node the subscription
/// no longer has, no Location) or whose value would not be one field is left
/// out, with a note in the log, so the config always stands. `node_exists`
/// says whether a node is in the config.
pub fn rules(routes: &[Route], targets: &Targets, node_exists: &dyn Fn(&str) -> bool) -> Generated {
    let mut apps: Vec<String> = Vec::new();
    let mut sites: Vec<(usize, String)> = Vec::new();
    let mut skipped = Vec::new();
    for r in routes {
        let target: Option<&str> = match &r.exit {
            RouteExit::Usual => continue,
            RouteExit::Direct => Some("DIRECT"),
            RouteExit::Block => Some("REJECT"),
            RouteExit::Ai => targets.ai.or(targets.everything),
            RouteExit::Downloads => targets.downloads.or(targets.everything),
            RouteExit::Everything => targets.everything,
            RouteExit::Node(n) => node_exists(n).then_some(n.as_str()),
        };
        let Some(target) = target.and_then(usable) else {
            log::warn!(
                "route {} is not applied: its exit is not in the config",
                r.id
            );
            skipped.push(r.id.clone());
            continue;
        };
        // Checked again here: a route built by hand is not trusted.
        match &r.kind {
            RouteKind::App { name } => match clean_app(name) {
                Ok(n) if n == *name => apps.push(format!("PROCESS-NAME,{n},{target}")),
                _ => {
                    log::warn!("route {} is not applied: the app name is not valid", r.id);
                    skipped.push(r.id.clone());
                }
            },
            RouteKind::Site { domain } => match clean_site(domain) {
                Ok(d) if d == *domain => sites.push((
                    d.matches('.').count(),
                    format!("DOMAIN-SUFFIX,{d},{target}"),
                )),
                _ => {
                    log::warn!("route {} is not applied: the site name is not valid", r.id);
                    skipped.push(r.id.clone());
                }
            },
        }
    }
    sites.sort_by_key(|(dots, _)| std::cmp::Reverse(*dots));
    let mut out = apps;
    out.extend(sites.into_iter().map(|(_, r)| r));
    Generated {
        rules: out,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(kind: RouteKind, exit: RouteExit) -> Route {
        Route::new(kind, exit).unwrap()
    }

    fn targets() -> Targets<'static> {
        Targets {
            ai: Some("Kairo AI"),
            everything: Some("Proxies"),
            downloads: Some("Kairo Downloads"),
        }
    }

    fn known(n: &str) -> bool {
        n == "Tokyo 03"
    }

    #[test]
    fn apps_and_sites_become_rules_in_this_order() {
        let routes = vec![
            route(RouteKind::site("example.com"), RouteExit::Block),
            route(RouteKind::app("claude"), RouteExit::Ai),
            route(RouteKind::site("a.example.com"), RouteExit::Direct),
            route(RouteKind::app("Cursor"), RouteExit::Node("Tokyo 03".into())),
            route(RouteKind::site("dl.example.org"), RouteExit::Downloads),
            route(RouteKind::site("example.net"), RouteExit::Everything),
        ];
        let g = rules(&routes, &targets(), &known);
        assert_eq!(
            g.rules,
            [
                "PROCESS-NAME,claude,Kairo AI",
                "PROCESS-NAME,Cursor,Tokyo 03",
                // Longer names first, so a.example.com is read before example.com.
                "DOMAIN-SUFFIX,a.example.com,DIRECT",
                "DOMAIN-SUFFIX,dl.example.org,Kairo Downloads",
                "DOMAIN-SUFFIX,example.com,REJECT",
                "DOMAIN-SUFFIX,example.net,Proxies",
            ]
        );
        assert!(g.skipped.is_empty());
    }

    #[test]
    fn usual_is_no_rule_and_removes() {
        let mut list = vec![];
        upsert(
            &mut list,
            route(RouteKind::site("example.com"), RouteExit::Block),
        );
        upsert(
            &mut list,
            route(RouteKind::site("example.com"), RouteExit::Direct),
        );
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].exit, RouteExit::Direct);
        upsert(
            &mut list,
            route(RouteKind::site("EXAMPLE.com"), RouteExit::Usual),
        );
        assert!(list.is_empty());
        // Even a stored Usual makes no rule, and loading drops it.
        let stored = vec![Route {
            id: "site:x.com".into(),
            kind: RouteKind::site("x.com"),
            exit: RouteExit::Usual,
        }];
        assert!(rules(&stored, &targets(), &known).rules.is_empty());
        assert!(sanitize(stored).is_empty());
    }

    #[test]
    fn a_value_cannot_add_fields_to_a_rule() {
        for bad in [
            "claude,DIRECT",
            "a,b,c",
            "x\nMATCH,DIRECT",
            "x\r\nMATCH,DIRECT",
            "x\u{0}",
            "/usr/bin/claude",
            "C:\\claude.exe",
            "",
            "   ",
        ] {
            assert!(clean_app(bad).is_err(), "{bad:?} must be refused");
            assert!(Route::new(RouteKind::app(bad), RouteExit::Direct).is_err());
        }
        for bad in [
            "example.com,DIRECT",
            "example.com\nMATCH,DIRECT",
            "https://example.com",
            "example.com/path",
            "example.com:8080",
            "user@example.com",
            "exa mple.com",
            "example.com?x=1",
            "*.*.com",
            "localhost",
            "com",
            "192.168.1.1",
            "-a.example.com",
            "a..example.com",
            "",
        ] {
            assert!(clean_site(bad).is_err(), "{bad:?} must be refused");
        }
        // A server whose name would split the rule is refused, as a node route.
        assert!(
            Route::new(
                RouteKind::app("claude"),
                RouteExit::Node("Tokyo, 03".into())
            )
            .is_err()
        );
        // A hand-built route with a bad value writes no rule (and is reported).
        let evil = Route {
            id: "app:x".into(),
            kind: RouteKind::App {
                name: "x,DIRECT".into(),
            },
            exit: RouteExit::Block,
        };
        let g = rules(std::slice::from_ref(&evil), &targets(), &known);
        assert!(g.rules.is_empty());
        assert_eq!(g.skipped, ["app:x"]);
        // ... and an unclean-but-valid value (not in its usual form) too.
        let upper = Route {
            id: "site:Example.com".into(),
            kind: RouteKind::Site {
                domain: "Example.com".into(),
            },
            exit: RouteExit::Block,
        };
        assert!(rules(&[upper], &targets(), &known).rules.is_empty());
        assert!(sanitize(vec![evil]).is_empty());
    }

    #[test]
    fn sites_are_made_lower_case_and_plain() {
        assert_eq!(clean_site("  Example.COM ").unwrap(), "example.com");
        assert_eq!(clean_site("*.example.com").unwrap(), "example.com");
        assert_eq!(clean_site(".example.com.").unwrap(), "example.com");
        assert_eq!(clean_site("münchen.de").unwrap(), "xn--mnchen-3ya.de");
        assert_eq!(
            clean_site("a-b_c.example.co.uk").unwrap(),
            "a-b_c.example.co.uk"
        );
        assert_eq!(clean_app("  Google Chrome ").unwrap(), "Google Chrome");
        assert_eq!(
            clean_app("Cursor Helper (Renderer)").unwrap(),
            "Cursor Helper (Renderer)"
        );
        let r = route(RouteKind::site("Example.COM"), RouteExit::Block);
        assert_eq!(r.id, "site:example.com");
        assert_eq!(r.kind, RouteKind::site("example.com"));
    }

    #[test]
    fn a_target_that_is_not_there_is_never_written() {
        let routes = vec![
            route(RouteKind::site("a.com"), RouteExit::Node("Gone".into())),
            route(RouteKind::site("b.com"), RouteExit::Everything),
            route(RouteKind::site("c.com"), RouteExit::Ai),
        ];
        // No Location at all (a bare list of nodes): Everything has no target,
        // and AI/Downloads (layer off) have nothing to fall back to either.
        let none = Targets::default();
        let g = rules(&routes, &none, &known);
        assert!(g.rules.is_empty());
        assert_eq!(g.skipped, ["site:a.com", "site:b.com", "site:c.com"]);
        // AI layer off: AI and Downloads fall back to the Location.
        let off = Targets {
            everything: Some("Proxies"),
            ..Default::default()
        };
        let routes = vec![
            route(RouteKind::site("c.com"), RouteExit::Ai),
            route(RouteKind::site("d.com"), RouteExit::Downloads),
            route(RouteKind::site("e.com"), RouteExit::Block),
        ];
        let g = rules(&routes, &off, &known);
        assert_eq!(
            g.rules,
            [
                "DOMAIN-SUFFIX,c.com,Proxies",
                "DOMAIN-SUFFIX,d.com,Proxies",
                "DOMAIN-SUFFIX,e.com,REJECT"
            ]
        );
        // A group whose own name has a comma cannot be a target.
        let odd = Targets {
            everything: Some("Proxy, Select"),
            ..Default::default()
        };
        let g = rules(
            &[route(RouteKind::site("b.com"), RouteExit::Everything)],
            &odd,
            &known,
        );
        assert!(g.rules.is_empty());
    }

    #[test]
    fn routes_round_trip_in_settings_and_are_cleaned_on_load() {
        let routes = vec![
            route(RouteKind::app("claude"), RouteExit::Ai),
            route(
                RouteKind::site("example.com"),
                RouteExit::Node("Tokyo 03".into()),
            ),
            route(RouteKind::site("x.org"), RouteExit::Block),
        ];
        let json = serde_json::to_string(&routes).unwrap();
        let back: Vec<Route> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, routes);
        // A file edited by hand: a duplicate (the later wins), a bad value, a Usual.
        let text = r#"[
            {"id":"x","kind":{"site":{"domain":"Example.com"}},"exit":"block"},
            {"id":"y","kind":{"site":{"domain":"example.com"}},"exit":"direct"},
            {"id":"z","kind":{"app":{"name":"a,b"}},"exit":"direct"},
            {"id":"w","kind":{"app":{"name":"git"}},"exit":"usual"}
        ]"#;
        let loaded = sanitize(serde_json::from_str(text).unwrap());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "site:example.com");
        assert_eq!(loaded[0].exit, RouteExit::Direct);
    }
}
