//! The provider's groups in plain terms: where traffic goes (the main
//! group's members are *locations*, by country), which notes in the node
//! list are the provider's (traffic left, expiry), and the other choices
//! the config offers per app or site (Netflix, AI, Telegram...).

use crate::model::{Group, Node};

/// The group whose members are the locations: the one the settings name
/// (`trayGroup`), else the selector the most other groups follow (the
/// usual "Proxies" every app group lists), else the first selector.
pub fn main_group<'a>(groups: &'a [Group], preferred: &str) -> Option<&'a Group> {
    let selectors = || {
        groups
            .iter()
            .filter(|g| g.selectable() && !g.hidden && g.name != "GLOBAL")
    };
    if !preferred.is_empty()
        && let Some(g) = selectors().find(|g| g.name == preferred)
    {
        return Some(g);
    }
    let followers = |name: &str| {
        groups
            .iter()
            .filter(|g| {
                g.name != name && g.name != "GLOBAL" && g.members.iter().any(|m| m.name == name)
            })
            .count()
    };
    let reachable = |g: &Group| !places(groups, g).is_empty();
    // Most followed (the group every app group lists); the first in config
    // order on a tie (configs list the main choice first).
    let mut best: Option<(&Group, usize)> = None;
    for g in selectors() {
        if !reachable(g) {
            continue;
        }
        let f = followers(&g.name);
        if best.is_none_or(|(_, b)| f > b) {
            best = Some((g, f));
        }
    }
    best.map(|(g, _)| g).or_else(|| selectors().next())
}

/// mihomo's own outbounds.
pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "DIRECT" | "REJECT" | "REJECT-DROP" | "PASS" | "COMPATIBLE"
    )
}

/// A node that is the provider's note rather than a place: "Traffic Reset:
/// 15 Days Left", "Expire Date: 2027/01/26", "282.21 G | 500.00 G",
/// "剩余流量：…", "官网 …".
pub fn is_note(name: &str) -> bool {
    const WORDS: [&str; 22] = [
        "剩余",
        "到期",
        "流量",
        "官网",
        "套餐",
        "过期",
        "重置",
        "网址",
        "客服",
        "订阅",
        "更新",
        "expire",
        "traffic",
        "reset",
        "days left",
        "remaining",
        "website",
        "official",
        "renew",
        "plan:",
        "使用",
        "距离",
    ];
    let low = name.to_lowercase();
    if WORDS.iter().any(|w| low.contains(w)) {
        return true;
    }
    // "282.21 G | 500.00 G", "12.3GB": an amount of data.
    let amount = regex_lite_amount(&low);
    amount || (name.contains('|') && name.chars().filter(|c| c.is_ascii_digit()).count() >= 4)
}

fn regex_lite_amount(s: &str) -> bool {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if !b[i].is_ascii_digit() {
            continue;
        }
        let mut j = i;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
            j += 1;
        }
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        if j < b.len() && (b[j] == b'g' || b[j] == b't' || b[j] == b'm') {
            let next = b.get(j + 1).copied();
            if next.is_none_or(|c| c == b'b' || c == b' ' || c == b'|' || c == b'/') {
                return true;
            }
        }
    }
    false
}

/// A location: a node of the main group.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub name: String,
    /// The name without its flag.
    pub title: String,
    pub flag: Option<String>,
    /// The country or region, in English ("Other" when the name says none).
    pub region: &'static str,
    /// Last delay in ms, `Some(0)` failed, `None` untested.
    pub delay: Option<u32>,
    pub is_group: bool,
}

/// The places the main group reaches: its nodes, and the nodes of the
/// selector groups among its members ("Asia", "United States"), each once.
/// Notes and mihomo's own outbounds are left out (not somewhere to go).
pub fn places(groups: &[Group], main: &Group) -> Vec<Place> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut push = |m: &Node, out: &mut Vec<Place>| {
        if !m.is_group && !is_note(&m.name) && !is_builtin(&m.name) && seen.insert(m.name.clone()) {
            out.push(place(m));
        }
    };
    for m in &main.members {
        push(m, &mut out);
    }
    for m in main.members.iter().filter(|m| m.is_group) {
        if let Some(g) = groups.iter().find(|g| g.name == m.name && g.selectable()) {
            for n in &g.members {
                push(n, &mut out);
            }
        }
    }
    out
}

/// The selections that make the main group go through `name`: itself, or
/// the member selector that has it, then that selector in the main group.
pub fn route_to(groups: &[Group], main: &Group, name: &str) -> Vec<(String, String)> {
    if main.members.iter().any(|m| m.name == name) {
        return vec![(main.name.clone(), name.to_owned())];
    }
    // The member group in use first, then the others in order.
    let mut members: Vec<&Node> = main.members.iter().filter(|m| m.is_group).collect();
    members.sort_by_key(|m| main.now.as_deref() != Some(m.name.as_str()));
    for m in members {
        if let Some(g) = groups.iter().find(|g| g.name == m.name && g.selectable())
            && g.members.iter().any(|n| n.name == name)
        {
            return vec![
                (g.name.clone(), name.to_owned()),
                (main.name.clone(), g.name.clone()),
            ];
        }
    }
    vec![]
}

/// The node the main group goes through now (through a member selector).
pub fn current(groups: &[Group], main: &Group) -> Option<Place> {
    let now = main.now.as_deref()?;
    match main.members.iter().find(|m| m.name == now)? {
        m if !m.is_group => Some(place(m)),
        m => {
            let g = groups.iter().find(|g| g.name == m.name)?;
            let inner = g.now.as_deref()?;
            g.members
                .iter()
                .find(|n| n.name == inner && !n.is_group)
                .map(place)
        }
    }
}

/// The groups whose members are places: the main one and its member selectors.
pub fn place_groups(groups: &[Group], main: &Group) -> Vec<String> {
    let mut out = vec![main.name.clone()];
    out.extend(
        main.members
            .iter()
            .filter(|m| m.is_group && groups.iter().any(|g| g.name == m.name && g.selectable()))
            .map(|m| m.name.clone()),
    );
    out
}

pub fn place(node: &Node) -> Place {
    let (flag, rest) = split_flag(&node.name);
    let region = flag
        .as_deref()
        .and_then(region_of_flag)
        .or_else(|| region_of_words(&node.name))
        .unwrap_or("Other");
    Place {
        name: node.name.clone(),
        title: rest.trim().to_owned(),
        flag,
        region,
        delay: node.delay,
        is_group: node.is_group,
    }
}

/// The provider's notes in the main group ("Expire Date: 2027/01/26").
pub fn notes(group: &Group) -> Vec<String> {
    group
        .members
        .iter()
        .filter(|m| is_note(&m.name))
        .map(|m| m.name.clone())
        .collect()
}

/// The fastest place that answered.
pub fn fastest(groups: &[Group], main: &Group) -> Option<Place> {
    places(groups, main)
        .into_iter()
        .filter(|p| p.delay.is_some_and(|d| d > 0))
        .min_by_key(|p| p.delay)
}

/// A flag at the start of the name (two regional indicators), and the rest.
pub fn split_flag(name: &str) -> (Option<String>, &str) {
    let t = name.trim_start();
    let mut it = t.char_indices();
    let ri = |c: char| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c);
    match (it.next(), it.next()) {
        (Some((_, a)), Some((j, b))) if ri(a) && ri(b) => {
            let end = j + b.len_utf8();
            (Some(t[..end].to_owned()), &t[end..])
        }
        _ => (None, t),
    }
}

/// The first flag anywhere in `text` (two regional indicators): the text before it, the
/// flag, the text after it.
pub fn find_flag(text: &str) -> (&str, Option<String>, &str) {
    let ri = |c: char| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c);
    let mut it = text.char_indices().peekable();
    while let Some((i, a)) = it.next() {
        if let (true, Some(&(j, b))) = (ri(a), it.peek())
            && ri(b)
        {
            let end = j + b.len_utf8();
            return (&text[..i], Some(text[i..end].to_owned()), &text[end..]);
        }
    }
    (text, None, "")
}

/// The ISO code of a flag.
pub fn flag_code(flag: &str) -> Option<String> {
    let code: String = flag
        .chars()
        .map(|c| {
            ('\u{1F1E6}'..='\u{1F1FF}')
                .contains(&c)
                .then(|| char::from_u32(c as u32 - 0x1F1E6 + 'A' as u32))
                .flatten()
        })
        .collect::<Option<String>>()?;
    (code.len() == 2).then_some(code)
}

/// A flag of an ISO code.
pub fn flag_of(code: &str) -> String {
    code.chars()
        .filter_map(|c| char::from_u32(c.to_ascii_uppercase() as u32 - 'A' as u32 + 0x1F1E6))
        .collect()
}

const REGIONS: [(&str, &str, &[&str]); 30] = [
    ("HK", "Hong Kong", &["香港", "hong kong", "hongkong", "hk"]),
    ("TW", "Taiwan", &["台湾", "臺灣", "taiwan", "tw"]),
    (
        "JP",
        "Japan",
        &["日本", "japan", "tokyo", "osaka", "东京", "大阪", "jp"],
    ),
    ("SG", "Singapore", &["新加坡", "狮城", "singapore", "sg"]),
    (
        "US",
        "United States",
        &[
            "美国",
            "美國",
            "united states",
            "usa",
            "los angeles",
            "san jose",
            "seattle",
            "new york",
            "us",
        ],
    ),
    (
        "KR",
        "South Korea",
        &["韩国", "韓國", "korea", "seoul", "首尔", "kr"],
    ),
    (
        "GB",
        "United Kingdom",
        &["英国", "united kingdom", "london", "uk"],
    ),
    ("DE", "Germany", &["德国", "germany", "frankfurt", "de"]),
    ("FR", "France", &["法国", "france", "paris"]),
    ("NL", "Netherlands", &["荷兰", "netherlands", "amsterdam"]),
    ("CA", "Canada", &["加拿大", "canada"]),
    (
        "AU",
        "Australia",
        &["澳大利亚", "澳洲", "australia", "sydney"],
    ),
    ("IN", "India", &["印度", "india"]),
    ("RU", "Russia", &["俄罗斯", "russia"]),
    ("TR", "Türkiye", &["土耳其", "turkey", "türkiye"]),
    ("MY", "Malaysia", &["马来西亚", "malaysia"]),
    ("TH", "Thailand", &["泰国", "thailand"]),
    ("VN", "Vietnam", &["越南", "vietnam"]),
    ("PH", "Philippines", &["菲律宾", "philippines"]),
    ("ID", "Indonesia", &["印尼", "印度尼西亚", "indonesia"]),
    ("AR", "Argentina", &["阿根廷", "argentina"]),
    ("BR", "Brazil", &["巴西", "brazil"]),
    ("MO", "Macao", &["澳门", "macao", "macau"]),
    (
        "AE",
        "United Arab Emirates",
        &["阿联酋", "迪拜", "dubai", "emirates"],
    ),
    ("IT", "Italy", &["意大利", "italy"]),
    ("ES", "Spain", &["西班牙", "spain"]),
    ("CH", "Switzerland", &["瑞士", "switzerland"]),
    ("SE", "Sweden", &["瑞典", "sweden"]),
    ("MX", "Mexico", &["墨西哥", "mexico"]),
    ("CN", "China", &["中国", "china", "回国"]),
];

fn region_of_flag(flag: &str) -> Option<&'static str> {
    let code = flag_code(flag)?;
    REGIONS
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, n, _)| *n)
        .or(Some(match code.as_str() {
            "UK" => "United Kingdom",
            _ => "Other",
        }))
}

fn region_of_words(name: &str) -> Option<&'static str> {
    let low = name.to_lowercase();
    // Whole words for the short codes; anywhere for the rest.
    let words: Vec<&str> = low
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    REGIONS
        .iter()
        .find(|(_, _, keys)| {
            keys.iter().any(|k| {
                if k.len() <= 3 && k.is_ascii() {
                    words.contains(k)
                } else {
                    low.contains(k)
                }
            })
        })
        .map(|(_, n, _)| *n)
}

/// The flag of a region (for places whose names carry none).
pub fn region_flag(region: &str) -> Option<String> {
    REGIONS
        .iter()
        .find(|(_, n, _)| *n == region)
        .map(|(c, _, _)| flag_of(c))
}

/// Places by region: regions by their fastest place, each region's places by
/// delay (untested and failed last), "Other" at the end.
pub fn by_region(places: &[Place]) -> Vec<(&'static str, Vec<Place>)> {
    let rank = |p: &Place| match p.delay {
        Some(d) if d > 0 => d,
        Some(_) => u32::MAX,
        None => u32::MAX - 1,
    };
    let mut regions: Vec<(&'static str, Vec<Place>)> = Vec::new();
    for p in places {
        match regions.iter_mut().find(|(r, _)| *r == p.region) {
            Some((_, list)) => list.push(p.clone()),
            None => regions.push((p.region, vec![p.clone()])),
        }
    }
    for (_, list) in &mut regions {
        list.sort_by_key(|p| rank(p));
    }
    regions.sort_by_key(|(r, list)| (*r == "Other", list.first().map(rank).unwrap_or(u32::MAX)));
    regions
}

/// One of the config's choices per app or site: a selector other than the
/// main group, with what it routes to now in plain terms.
#[derive(Debug, Clone, PartialEq)]
pub struct AppChoice {
    pub group: String,
    pub now: Option<String>,
    /// It goes where the main group goes.
    pub follows_main: bool,
    /// It goes straight out (DIRECT, or a selector that does).
    pub direct: bool,
    pub options: Vec<String>,
}

pub fn app_choices(groups: &[Group], main: &str) -> Vec<AppChoice> {
    let direct_groups: Vec<&str> = groups
        .iter()
        .filter(|g| g.selectable() && g.name != main && g.now.as_deref() == Some("DIRECT"))
        .map(|g| g.name.as_str())
        .collect();
    groups
        .iter()
        .filter(|g| {
            g.selectable()
                && !g.hidden
                && g.name != main
                && g.name != "GLOBAL"
                && !direct_groups.contains(&g.name.as_str())
        })
        .map(|g| AppChoice {
            group: g.name.clone(),
            now: g.now.clone(),
            follows_main: g.now.as_deref() == Some(main),
            direct: g
                .now
                .as_deref()
                .is_some_and(|n| n == "DIRECT" || direct_groups.contains(&n)),
            options: g.members.iter().map(|m| m.name.clone()).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_is_found_anywhere_in_a_line() {
        assert_eq!(
            find_flag("Select · 🇺🇸 Los Angeles 01"),
            ("Select · ", Some("🇺🇸".to_owned()), " Los Angeles 01")
        );
        assert_eq!(find_flag("🇯🇵 Tokyo"), ("", Some("🇯🇵".to_owned()), " Tokyo"));
        assert_eq!(find_flag("Tokyo 01"), ("Tokyo 01", None, ""));
        // One regional indicator alone is not a flag.
        assert_eq!(find_flag("a 🇯 b"), ("a 🇯 b", None, ""));
    }

    fn node(name: &str, delay: Option<u32>) -> Node {
        Node {
            name: name.into(),
            kind: "Trojan".into(),
            udp: true,
            delay,
            alive: true,
            is_group: false,
        }
    }

    fn group(name: &str, members: &[&str]) -> Group {
        Group {
            name: name.into(),
            kind: "Selector".into(),
            now: members.first().map(|m| m.to_string()),
            members: members
                .iter()
                .map(|m| Node {
                    is_group: false,
                    ..node(m, None)
                })
                .collect(),
            hidden: false,
            test_url: None,
        }
    }

    #[test]
    fn notes_are_not_places() {
        for n in [
            "282.21 G | 500.00 G",
            "Traffic Reset：15 Days Left",
            "Expire Date：2027/01/26",
            "剩余流量：120.5 GB",
            "官网 example.com",
            "12.3GB",
        ] {
            assert!(is_note(n), "{n}");
        }
        for n in [
            "🇭🇰 Hong Kong 01",
            "🇯🇵 Tokyo 03",
            "US Seattle 1x",
            "香港 IEPL 02",
            "Singapore G1",
        ] {
            assert!(!is_note(n), "{n}");
        }
    }

    #[test]
    fn flags_and_regions() {
        let p = place(&node("🇭🇰 Hong Kong 01", Some(40)));
        assert_eq!(
            (p.flag.as_deref(), p.title.as_str(), p.region),
            (Some("🇭🇰"), "Hong Kong 01", "Hong Kong")
        );
        assert_eq!(place(&node("日本 大阪 02", None)).region, "Japan");
        assert_eq!(place(&node("US Seattle", None)).region, "United States");
        assert_eq!(place(&node("Plus 03", None)).region, "Other");
        assert_eq!(place(&node("🇺🇸 Los Angeles", None)).region, "United States");
        assert_eq!(region_flag("Japan").as_deref(), Some("🇯🇵"));
    }

    #[test]
    fn the_main_group_is_the_one_apps_follow() {
        let groups = vec![
            group("Netflix", &["Proxies", "DIRECT"]),
            group("Proxies", &["🇭🇰 HK 1", "🇯🇵 JP 1"]),
            group("AI", &["Proxies", "🇯🇵 JP 1"]),
            group("Final", &["Proxies", "DIRECT"]),
        ];
        let mut gs = groups.clone();
        for g in &mut gs {
            for m in &mut g.members {
                m.is_group = ["Proxies", "Netflix", "AI", "Final"].contains(&m.name.as_str());
            }
        }
        assert_eq!(main_group(&gs, "").unwrap().name, "Proxies");
        assert_eq!(main_group(&gs, "AI").unwrap().name, "AI");
        let apps = app_choices(&gs, "Proxies");
        assert_eq!(
            apps.iter().map(|a| a.group.as_str()).collect::<Vec<_>>(),
            ["Netflix", "AI", "Final"]
        );
        assert!(apps[0].follows_main);
    }

    #[test]
    fn a_main_group_of_groups() {
        // The fixture's shape: Proxy picks a region group, the region its node.
        let mut proxy = group("Proxy", &["Auto", "Asia", "US", "DIRECT"]);
        for m in &mut proxy.members {
            m.is_group = m.name != "DIRECT";
        }
        proxy.now = Some("Asia".into());
        let mut auto = group("Auto", &["🇯🇵 Tokyo 01", "🇺🇸 Seattle 01"]);
        auto.kind = "URLTest".into();
        let mut asia = group("Asia", &["🇯🇵 Tokyo 01", "🇭🇰 HK 02"]);
        asia.now = Some("🇭🇰 HK 02".into());
        let us = group("US", &["🇺🇸 Seattle 01"]);
        let mut fin = group("Final", &["Proxy", "DIRECT"]);
        fin.members[0].is_group = true;
        let mut ai = group("AI", &["US", "Asia", "Proxy"]);
        for m in &mut ai.members {
            m.is_group = true;
        }
        let gs = vec![proxy.clone(), auto, asia, us, ai, fin];
        assert_eq!(main_group(&gs, "").unwrap().name, "Proxy");
        let names: Vec<String> = places(&gs, &proxy).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["🇯🇵 Tokyo 01", "🇭🇰 HK 02", "🇺🇸 Seattle 01"]);
        assert_eq!(current(&gs, &proxy).unwrap().name, "🇭🇰 HK 02");
        assert_eq!(
            route_to(&gs, &proxy, "🇺🇸 Seattle 01"),
            [
                ("US".to_string(), "🇺🇸 Seattle 01".to_string()),
                ("Proxy".to_string(), "US".to_string())
            ]
        );
        assert_eq!(route_to(&gs, &proxy, "🇯🇵 Tokyo 01")[0].0, "Asia");
    }

    #[test]
    fn fastest_skips_notes_and_failures() {
        let mut g = group("Proxies", &[]);
        g.members = vec![
            node("282.21 G | 500.00 G", Some(20)),
            node("🇭🇰 HK 1", Some(0)),
            node("🇯🇵 JP 1", Some(48)),
            node("🇸🇬 SG 1", Some(39)),
        ];
        let gs = vec![g.clone()];
        assert_eq!(fastest(&gs, &g).unwrap().name, "🇸🇬 SG 1");
        let regions = by_region(&places(&gs, &g));
        assert_eq!(
            regions.iter().map(|r| r.0).collect::<Vec<_>>(),
            ["Singapore", "Japan", "Hong Kong"]
        );
    }
}

#[cfg(test)]
mod real {
    /// `KAIRO_PROXIES_JSON=/path/to/proxies.json cargo test -p kairo-core real_config -- --nocapture --ignored`
    #[test]
    #[ignore]
    fn real_config() {
        let text = std::fs::read_to_string(std::env::var("KAIRO_PROXIES_JSON").unwrap()).unwrap();
        let resp: crate::model::ProxiesResponse = serde_json::from_str(&text).unwrap();
        let groups = crate::model::groups_of(&resp);
        let main = super::main_group(&groups, "").unwrap();
        println!("main: {} ({} members)", main.name, main.members.len());
        println!("notes: {:?}", super::notes(main));
        for (r, ps) in super::by_region(&super::places(&groups, main)) {
            println!("{r}: {}", ps.len());
        }
        for a in super::app_choices(&groups, &main.name) {
            println!(
                "app {} now={:?} follows={} direct={}",
                a.group, a.now, a.follows_main, a.direct
            );
        }
    }
}
