//! Statistics: what Kairo records of its own use, on this computer only.
//!
//! Traffic (per exit, per app, per AI service), the AI checks (reachable or
//! not, how long the API took, the country seen), the exits' response times
//! and switches, and a subscription's usage. Everything lands in hourly
//! buckets (the per-minute counts of the connection snapshots are folded into
//! the current hour as they come), kept 30 days in one small file that
//! nothing reads but Kairo. The queries here are pure: the caller gives the
//! time and the offset of the local time zone, so the days are the person's
//! days.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const HOUR: i64 = 3600;
pub const DAY: i64 = 86_400;
/// How long anything is kept.
pub const KEEP_SECS: i64 = 30 * DAY;
/// Apps and sites kept per hour (the biggest ones; the rest is summed as "other").
const TOP_PER_HOUR: usize = 40;
/// Response samples kept per hour and thing.
const SAMPLES: usize = 12;
/// Switches kept.
const MAX_SWITCHES: usize = 400;
/// Usage samples kept.
const MAX_USAGE: usize = 2000;
/// The file's format.
const VERSION: u32 = 1;

/// The exits whose servers are tested, by the name the records use.
pub const EXITS: [&str; 3] = ["ai", "rest", "downloads"];

/// The exits traffic is counted under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    Ai = 0,
    Rest = 1,
    Downloads = 2,
    Direct = 3,
}

impl Slot {
    pub const ALL: [Slot; 4] = [Slot::Ai, Slot::Rest, Slot::Downloads, Slot::Direct];

    pub fn title(self) -> &'static str {
        match self {
            Slot::Ai => "AI",
            Slot::Rest => "Everything else",
            Slot::Downloads => "Downloads",
            Slot::Direct => "Direct",
        }
    }
}

/// One snapshot's worth of bytes of one connection, from the activity tally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub app: String,
    /// `None`: stays on this computer, or is blocked (not traffic).
    pub slot: Option<Slot>,
    pub service: Option<&'static str>,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hour {
    /// Bytes per exit, in the order of [`Slot`].
    pub x: [u64; 4],
    pub apps: BTreeMap<String, u64>,
    pub services: BTreeMap<String, u64>,
}

impl Hour {
    pub fn total(&self) -> u64 {
        self.x.iter().sum()
    }
}

/// What the AI checks of one service found in an hour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiHour {
    pub checks: u32,
    pub passed: u32,
    /// API answer times in ms (the ones that answered).
    pub api_ms: Vec<u32>,
    /// The country the service saw, as a two-letter code, if it said.
    pub country: Option<String>,
    /// Connections held open for 90 s, and how many were still open at the end.
    pub holds: u32,
    pub held: u32,
}

/// What the tests of one exit found in an hour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitHour {
    pub tests: u32,
    pub fails: u32,
    pub ms: Vec<u32>,
    /// The server it was on.
    pub node: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Switch {
    pub at: i64,
    /// `"ai"`, `"rest"` or `"downloads"`.
    pub exit: String,
    pub from: String,
    pub to: String,
    /// Done by Kairo (the fastest, a fallback), not by the person.
    pub auto: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSample {
    pub at: i64,
    pub profile: String,
    pub used: u64,
    pub total: u64,
    pub expire: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub version: u32,
    /// When recording began (unix s); 0 before the first record.
    pub first_at: i64,
    /// Keyed by the start of the (UTC) hour, in unix seconds.
    pub hours: BTreeMap<i64, Hour>,
    pub ai: BTreeMap<i64, BTreeMap<String, AiHour>>,
    pub exits: BTreeMap<i64, BTreeMap<String, ExitHour>>,
    pub switches: Vec<Switch>,
    pub usage: Vec<UsageSample>,
}

fn hour_of(t: i64) -> i64 {
    t - t.rem_euclid(HOUR)
}

fn push_sample(v: &mut Vec<u32>, x: u32) {
    if v.len() == SAMPLES {
        v.remove(0);
    }
    v.push(x);
}

impl Store {
    fn touch(&mut self, now: i64) {
        self.version = VERSION;
        if self.first_at == 0 {
            self.first_at = now;
        }
    }

    /// Adds the bytes of one snapshot.
    pub fn add_traffic(&mut self, now: i64, deltas: &[Delta]) {
        if deltas.iter().all(|d| d.bytes == 0) {
            return;
        }
        self.touch(now);
        let h = self.hours.entry(hour_of(now)).or_default();
        for d in deltas.iter().filter(|d| d.bytes > 0) {
            if let Some(slot) = d.slot {
                h.x[slot as usize] += d.bytes;
                *h.apps.entry(d.app.clone()).or_default() += d.bytes;
                if let Some(s) = d.service {
                    *h.services.entry(s.to_owned()).or_default() += d.bytes;
                }
            }
        }
        if h.apps.len() > TOP_PER_HOUR {
            // Fold the smallest into "Other".
            let mut by: Vec<(String, u64)> = std::mem::take(&mut h.apps).into_iter().collect();
            by.sort_by(|a, b| b.1.cmp(&a.1));
            let rest: u64 = by.split_off(TOP_PER_HOUR).iter().map(|x| x.1).sum();
            h.apps = by.into_iter().collect();
            *h.apps.entry("Other".into()).or_default() += rest;
        }
    }

    /// One AI check of one service.
    pub fn record_ai(
        &mut self,
        now: i64,
        service: &str,
        reachable: bool,
        api_ms: Option<u32>,
        country: Option<&str>,
    ) {
        self.touch(now);
        let e = self
            .ai
            .entry(hour_of(now))
            .or_default()
            .entry(service.to_owned())
            .or_default();
        e.checks += 1;
        if reachable {
            e.passed += 1;
        }
        if let Some(ms) = api_ms {
            push_sample(&mut e.api_ms, ms);
        }
        if let Some(c) = country {
            e.country = Some(c.to_owned());
        }
    }

    /// A 90 s hold finished: whether the connection was still open.
    pub fn record_hold(&mut self, now: i64, service: &str, held: bool) {
        self.touch(now);
        let e = self
            .ai
            .entry(hour_of(now))
            .or_default()
            .entry(service.to_owned())
            .or_default();
        e.holds += 1;
        if held {
            e.held += 1;
        }
    }

    /// One test of an exit's server: its delay, or `None` for a failure.
    pub fn record_exit(&mut self, now: i64, exit: &str, node: &str, ms: Option<u32>) {
        self.touch(now);
        let e = self
            .exits
            .entry(hour_of(now))
            .or_default()
            .entry(exit.to_owned())
            .or_default();
        e.tests += 1;
        match ms {
            Some(ms) if ms > 0 => push_sample(&mut e.ms, ms),
            _ => e.fails += 1,
        }
        e.node = node.to_owned();
    }

    pub fn record_switch(&mut self, now: i64, exit: &str, from: &str, to: &str, auto: bool) {
        self.touch(now);
        if from == to {
            return;
        }
        self.switches.push(Switch {
            at: now,
            exit: exit.to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
            auto,
        });
        if self.switches.len() > MAX_SWITCHES {
            self.switches.remove(0);
        }
    }

    /// A subscription's usage as its provider reports it. Kept when it changed
    /// or when an hour has passed since the last one of that subscription.
    pub fn record_usage(
        &mut self,
        now: i64,
        profile: &str,
        used: u64,
        total: u64,
        expire: Option<i64>,
    ) {
        self.touch(now);
        let last = self.usage.iter().rev().find(|u| u.profile == profile);
        if let Some(l) = last
            && l.used == used
            && l.total == total
            && now - l.at < HOUR
        {
            return;
        }
        self.usage.push(UsageSample {
            at: now,
            profile: profile.to_owned(),
            used,
            total,
            expire,
        });
        if self.usage.len() > MAX_USAGE {
            self.usage.remove(0);
        }
    }

    /// Drops what is older than 30 days.
    pub fn prune(&mut self, now: i64) {
        let from = hour_of(now - KEEP_SECS);
        self.hours = self.hours.split_off(&from);
        self.ai = self.ai.split_off(&from);
        self.exits = self.exits.split_off(&from);
        self.switches.retain(|s| s.at >= from);
        self.usage.retain(|u| u.at >= from);
    }

    pub fn is_empty(&self) -> bool {
        self.hours.is_empty()
            && self.ai.is_empty()
            && self.exits.is_empty()
            && self.usage.is_empty()
    }

    /// The file's text.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads a file's text; a damaged or newer file gives an empty store.
    pub fn from_json(text: &str) -> Store {
        match serde_json::from_str::<Store>(text) {
            Ok(s) if s.version <= VERSION => s,
            _ => Store::default(),
        }
    }

    // --- queries --------------------------------------------------------

    /// Whether there is not yet an hour to draw.
    pub fn collecting(&self, now: i64) -> bool {
        self.first_at == 0 || now - self.first_at < HOUR
    }

    /// Traffic in the buckets of `range`, oldest first.
    pub fn traffic(&self, range: Range, now: i64, tz: i64) -> Series {
        let (starts, per) = range.buckets(now, tz);
        let mut buckets: Vec<Bucket> = starts
            .iter()
            .map(|&s| Bucket {
                start: s,
                x: [0; 4],
                open: s > now,
            })
            .collect();
        let step = per.secs();
        for (&h, hour) in self.hours.range(starts[0] - HOUR..) {
            if let Some(i) = starts.iter().position(|&s| s <= h && h < s + step) {
                for k in 0..4 {
                    buckets[i].x[k] += hour.x[k];
                }
            }
        }
        let total = buckets.iter().map(|b| b.x.iter().sum::<u64>()).sum();
        Series {
            per,
            buckets,
            total,
        }
    }

    fn hours_in(&self, range: Range, now: i64, tz: i64) -> impl Iterator<Item = &Hour> {
        let from = range.from(now, tz);
        self.hours.range(from..).map(|(_, h)| h)
    }

    /// Apps by bytes in `range`, biggest first.
    pub fn top_apps(&self, range: Range, now: i64, tz: i64) -> Vec<(String, u64)> {
        let mut m: BTreeMap<String, u64> = BTreeMap::new();
        for h in self.hours_in(range, now, tz) {
            for (k, v) in &h.apps {
                *m.entry(k.clone()).or_default() += v;
            }
        }
        sorted(m)
    }

    /// AI services by bytes in `range`, biggest first.
    pub fn top_services(&self, range: Range, now: i64, tz: i64) -> Vec<(String, u64)> {
        let mut m: BTreeMap<String, u64> = BTreeMap::new();
        for h in self.hours_in(range, now, tz) {
            for (k, v) in &h.services {
                *m.entry(k.clone()).or_default() += v;
            }
        }
        sorted(m)
    }

    /// One service's checks hour by hour over the last `days` days, ending
    /// at the hour of `now`: (hour start, its checks).
    pub fn ai_hours(&self, service: &str, now: i64, days: i64) -> Vec<(i64, Option<AiHour>)> {
        let end = hour_of(now);
        let n = days * 24;
        (0..n)
            .map(|k| {
                let h = end - (n - 1 - k) * HOUR;
                (h, self.ai.get(&h).and_then(|m| m.get(service)).cloned())
            })
            .collect()
    }

    /// One exit's tests hour by hour over the last `hours` hours.
    pub fn exit_hours(&self, exit: &str, now: i64, hours: i64) -> Vec<(i64, Option<ExitHour>)> {
        let end = hour_of(now);
        (0..hours)
            .map(|k| {
                let h = end - (hours - 1 - k) * HOUR;
                (h, self.exits.get(&h).and_then(|m| m.get(exit)).cloned())
            })
            .collect()
    }

    /// The switches of one exit since `since`, newest first.
    pub fn switches_of(&self, exit: &str, since: i64) -> Vec<&Switch> {
        let mut v: Vec<&Switch> = self
            .switches
            .iter()
            .filter(|s| s.exit == exit && s.at >= since)
            .collect();
        v.reverse();
        v
    }

    /// A subscription's samples, oldest first.
    pub fn usage_of(&self, profile: &str) -> Vec<&UsageSample> {
        self.usage.iter().filter(|u| u.profile == profile).collect()
    }
}

fn sorted(m: BTreeMap<String, u64>) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = m.into_iter().filter(|x| x.1 > 0).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// The span of a chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Today,
    Week,
    Month,
}

/// What a bucket of a chart spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Per {
    Hour,
    Day,
}

impl Per {
    pub fn secs(self) -> i64 {
        match self {
            Per::Hour => HOUR,
            Per::Day => DAY,
        }
    }
}

impl Range {
    pub fn title(self) -> &'static str {
        match self {
            Range::Today => "Today",
            Range::Week => "7 days",
            Range::Month => "30 days",
        }
    }

    /// The first instant (UTC unix) the range covers.
    pub fn from(self, now: i64, tz: i64) -> i64 {
        let midnight = local_midnight(now, tz);
        match self {
            Range::Today => midnight,
            Range::Week => midnight - 6 * DAY,
            Range::Month => midnight - 29 * DAY,
        }
    }

    /// The starts (UTC unix) of the buckets, oldest first, and what each spans.
    pub fn buckets(self, now: i64, tz: i64) -> (Vec<i64>, Per) {
        let first = self.from(now, tz);
        match self {
            Range::Today => ((0..24).map(|h| first + h * HOUR).collect(), Per::Hour),
            Range::Week => ((0..7).map(|d| first + d * DAY).collect(), Per::Day),
            Range::Month => ((0..30).map(|d| first + d * DAY).collect(), Per::Day),
        }
    }
}

/// The start (UTC unix) of the local day `now` is in.
pub fn local_midnight(now: i64, tz: i64) -> i64 {
    let local = now + tz;
    local - local.rem_euclid(DAY) - tz
}

/// The offset of the local time zone from UTC at `t`, in seconds.
#[cfg(unix)]
pub fn local_offset(t: i64) -> i64 {
    // SAFETY: localtime_r writes into the struct we give it.
    unsafe {
        let tt: libc::time_t = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&tt, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

#[cfg(not(unix))]
pub fn local_offset(_t: i64) -> i64 {
    0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    pub start: i64,
    pub x: [u64; 4],
    /// Not yet (after now).
    pub open: bool,
}

impl Bucket {
    pub fn total(&self) -> u64 {
        self.x.iter().sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    pub per: Per,
    pub buckets: Vec<Bucket>,
    pub total: u64,
}

// --- derived readings ----------------------------------------------------

/// How an hour of checks reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    None,
    Ok,
    Part,
    Down,
}

impl Cell {
    pub fn of(h: Option<&AiHour>) -> Cell {
        match h {
            None => Cell::None,
            Some(h) if h.checks == 0 => Cell::None,
            Some(h) if h.passed == h.checks => Cell::Ok,
            Some(h) if h.passed * 2 >= h.checks => Cell::Part,
            Some(_) => Cell::Down,
        }
    }

    /// The worse of two.
    pub fn worst(a: Cell, b: Cell) -> Cell {
        fn rank(c: Cell) -> u8 {
            match c {
                Cell::None => 0,
                Cell::Ok => 1,
                Cell::Part => 2,
                Cell::Down => 3,
            }
        }
        if rank(b) > rank(a) { b } else { a }
    }
}

/// The share of checks that passed, in percent (one decimal), over the hours.
pub fn uptime(hours: &[(i64, Option<AiHour>)]) -> Option<f32> {
    let (mut c, mut p) = (0u32, 0u32);
    for (_, h) in hours {
        if let Some(h) = h {
            c += h.checks;
            p += h.passed;
        }
    }
    (c > 0).then(|| (p as f32 / c as f32 * 1000.0).round() / 10.0)
}

/// The median of the numbers (the upper one of two), if any.
pub fn median(mut v: Vec<u32>) -> Option<u32> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

/// The value `p` (0..1) of the way up the sorted numbers.
pub fn percentile(mut v: Vec<u32>, p: f32) -> Option<u32> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let i = ((v.len() as f32 - 1.0) * p).round() as usize;
    Some(v[i.min(v.len() - 1)])
}

/// The API answer times of an hour's checks, median per hour.
pub fn api_median(h: &Option<AiHour>) -> Option<u32> {
    h.as_ref().and_then(|h| median(h.api_ms.clone()))
}

/// The median delay of an exit's hour.
pub fn exit_median(h: &Option<ExitHour>) -> Option<u32> {
    h.as_ref().and_then(|h| median(h.ms.clone()))
}

/// Where a subscription stands, from its samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    pub used: u64,
    pub total: u64,
    pub expire: Option<i64>,
    /// Bytes a day over the recent days (at least a day of samples).
    pub pace: Option<f64>,
    /// When the plan runs out at that pace (unix s), if before it expires or
    /// there is no expiry.
    pub runs_out: Option<i64>,
    /// The most a day that lasts until the expiry.
    pub budget: Option<f64>,
    pub from: i64,
}

/// The pace over the last `window` seconds (a week at most) and where it
/// leads; `None` without two samples a day or more apart.
pub fn project(samples: &[&UsageSample], now: i64) -> Option<Projection> {
    let last = samples.last()?;
    let window = 7 * DAY;
    let first = samples
        .iter()
        .find(|s| s.at >= last.at - window)
        .copied()
        .unwrap_or(last);
    let span = last.at - first.at;
    let pace = (span >= DAY && last.used >= first.used)
        .then(|| (last.used - first.used) as f64 / (span as f64 / DAY as f64));
    let left = last.total.saturating_sub(last.used) as f64;
    let runs_out = pace
        .filter(|p| *p > 0.0)
        .map(|p| last.at + (left / p * DAY as f64) as i64);
    let budget = last
        .expire
        .filter(|e| *e > now)
        .map(|e| left / ((e - now) as f64 / DAY as f64).max(1.0));
    Some(Projection {
        used: last.used,
        total: last.total,
        expire: last.expire,
        pace,
        runs_out,
        budget,
        from: first.at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tue 2026-11-10 14:36:00 UTC+8 is 06:36 UTC.
    const NOW: i64 = 1_794_292_560;
    const TZ: i64 = 8 * HOUR;

    fn d(app: &str, slot: Option<Slot>, service: Option<&'static str>, bytes: u64) -> Delta {
        Delta {
            app: app.into(),
            slot,
            service,
            bytes,
        }
    }

    #[test]
    fn the_local_day_starts_at_local_midnight() {
        // 14:36 local; the day began at 00:00 local = 16:00 UTC the day before.
        let m = local_midnight(NOW, TZ);
        assert_eq!((m + TZ).rem_euclid(DAY), 0);
        assert!(NOW - m < DAY && NOW - m >= 14 * HOUR);
        // Just before and just after local midnight are different days.
        let before = m - 1;
        let after = m + 1;
        assert_eq!(local_midnight(before, TZ), m - DAY);
        assert_eq!(local_midnight(after, TZ), m);
        // West of UTC too.
        let west = -5 * HOUR;
        assert_eq!((local_midnight(NOW, west) + west).rem_euclid(DAY), 0);
    }

    #[test]
    fn traffic_is_counted_by_exit_app_and_service() {
        let mut s = Store::default();
        s.add_traffic(
            NOW,
            &[
                d("claude", Some(Slot::Ai), Some("claude"), 100),
                d("firefox", Some(Slot::Rest), None, 40),
                d("claude", Some(Slot::Ai), Some("claude"), 50),
                d("curl", None, None, 999),
                d("idle", Some(Slot::Direct), None, 0),
            ],
        );
        let h = &s.hours[&hour_of(NOW)];
        assert_eq!(h.x, [150, 40, 0, 0]);
        assert_eq!(h.apps["claude"], 150);
        assert!(!h.apps.contains_key("curl"));
        assert_eq!(h.services["claude"], 150);
        assert_eq!(s.first_at, NOW);
        assert_eq!(
            s.top_apps(Range::Today, NOW, TZ),
            vec![("claude".to_string(), 150), ("firefox".to_string(), 40)]
        );
        assert_eq!(
            s.top_services(Range::Today, NOW, TZ),
            vec![("claude".to_string(), 150)]
        );
    }

    #[test]
    fn today_is_24_hours_of_the_local_day_and_the_days_cross_midnight() {
        let mut s = Store::default();
        let m = local_midnight(NOW, TZ);
        // One byte count just before local midnight (yesterday), one just after.
        s.add_traffic(m - 60, &[d("a", Some(Slot::Rest), None, 7)]);
        s.add_traffic(m + 60, &[d("a", Some(Slot::Ai), None, 5)]);
        let today = s.traffic(Range::Today, NOW, TZ);
        assert_eq!(today.per, Per::Hour);
        assert_eq!(today.buckets.len(), 24);
        assert_eq!(today.total, 5, "yesterday's bytes are not today's");
        assert_eq!(today.buckets[0].x, [5, 0, 0, 0]);
        assert!(!today.buckets[14].open && today.buckets[15].open);
        let week = s.traffic(Range::Week, NOW, TZ);
        assert_eq!(week.per, Per::Day);
        assert_eq!(week.buckets.len(), 7);
        assert_eq!(week.buckets[5].x, [0, 7, 0, 0], "yesterday");
        assert_eq!(week.buckets[6].x, [5, 0, 0, 0], "today");
        assert_eq!(week.total, 12);
        let month = s.traffic(Range::Month, NOW, TZ);
        assert_eq!(month.buckets.len(), 30);
        assert_eq!(month.total, 12);
    }

    #[test]
    fn many_apps_in_an_hour_fold_into_other() {
        let mut s = Store::default();
        let ds: Vec<Delta> = (0..60)
            .map(|i| d(&format!("app{i:02}"), Some(Slot::Rest), None, 100 + i))
            .collect();
        s.add_traffic(NOW, &ds);
        let h = &s.hours[&hour_of(NOW)];
        assert_eq!(h.apps.len(), TOP_PER_HOUR + 1);
        assert!(h.apps.contains_key("Other"));
        let all: u64 = h.apps.values().sum();
        assert_eq!(all, (0..60).map(|i| 100 + i).sum::<u64>());
    }

    #[test]
    fn ai_checks_make_hours_and_cells() {
        let mut s = Store::default();
        for _ in 0..6 {
            s.record_ai(NOW, "claude", true, Some(400), Some("JP"));
        }
        s.record_ai(NOW - HOUR, "claude", true, Some(500), None);
        s.record_ai(NOW - HOUR, "claude", false, None, None);
        s.record_ai(NOW - 2 * HOUR, "claude", false, None, None);
        let hs = s.ai_hours("claude", NOW, 1);
        assert_eq!(hs.len(), 24);
        let last = hs.last().unwrap();
        assert_eq!(Cell::of(last.1.as_ref()), Cell::Ok);
        assert_eq!(last.1.as_ref().unwrap().country.as_deref(), Some("JP"));
        assert_eq!(Cell::of(hs[22].1.as_ref()), Cell::Part);
        assert_eq!(Cell::of(hs[21].1.as_ref()), Cell::Down);
        assert_eq!(Cell::of(hs[0].1.as_ref()), Cell::None);
        assert_eq!(uptime(&hs), Some(((7.0f32 / 9.0) * 1000.0).round() / 10.0));
        assert_eq!(api_median(&last.1), Some(400));
        assert_eq!(Cell::worst(Cell::Ok, Cell::Down), Cell::Down);
        assert_eq!(Cell::worst(Cell::None, Cell::Ok), Cell::Ok);
    }

    #[test]
    fn samples_are_capped_and_the_median_is_the_upper_middle() {
        let mut s = Store::default();
        for i in 0..30 {
            s.record_ai(NOW, "x", true, Some(100 + i), None);
        }
        let h = &s.ai[&hour_of(NOW)]["x"];
        assert_eq!(h.api_ms.len(), SAMPLES);
        assert_eq!(h.checks, 30);
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![5, 1, 3]), Some(3));
        assert_eq!(median(vec![4, 1, 3, 2]), Some(3));
        assert_eq!(
            percentile(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 0.95),
            Some(10)
        );
    }

    #[test]
    fn exits_record_delays_failures_and_switches() {
        let mut s = Store::default();
        s.record_exit(NOW, "rest", "HK 01", Some(52));
        s.record_exit(NOW, "rest", "HK 01", None);
        s.record_exit(NOW, "rest", "HK 01", Some(0));
        let e = &s.exits[&hour_of(NOW)]["rest"];
        assert_eq!((e.tests, e.fails, e.ms.clone()), (3, 2, vec![52]));
        s.record_switch(NOW, "rest", "HK 01", "HK 02", true);
        s.record_switch(NOW + 5, "rest", "HK 02", "HK 02", true);
        s.record_switch(NOW + 10, "ai", "A", "B", false);
        assert_eq!(s.switches.len(), 2);
        assert_eq!(s.switches_of("rest", 0).len(), 1);
        assert!(s.switches_of("rest", 0)[0].auto);
        let hs = s.exit_hours("rest", NOW, 24);
        assert_eq!(exit_median(&hs.last().unwrap().1), Some(52));
    }

    #[test]
    fn old_things_are_pruned_and_a_bad_file_is_empty() {
        let mut s = Store::default();
        s.add_traffic(NOW - 31 * DAY, &[d("a", Some(Slot::Rest), None, 1)]);
        s.add_traffic(NOW - 2 * DAY, &[d("a", Some(Slot::Rest), None, 2)]);
        s.record_switch(NOW - 40 * DAY, "rest", "a", "b", true);
        s.record_usage(NOW - 40 * DAY, "p", 1, 10, None);
        s.prune(NOW);
        assert_eq!(s.hours.len(), 1);
        assert!(s.switches.is_empty() && s.usage.is_empty());
        let back = Store::from_json(&s.to_json());
        assert_eq!(back, s);
        assert!(Store::from_json("{ not json").is_empty());
        assert!(Store::from_json(r#"{"version":99}"#).is_empty());
        assert!(Store::from_json("").is_empty());
    }

    #[test]
    fn collecting_until_an_hour_has_passed() {
        let mut s = Store::default();
        assert!(s.collecting(NOW));
        s.add_traffic(NOW, &[d("a", Some(Slot::Rest), None, 1)]);
        assert!(s.collecting(NOW + 10 * 60));
        assert!(!s.collecting(NOW + HOUR));
    }

    #[test]
    fn usage_projects_only_from_a_day_of_samples() {
        let mut s = Store::default();
        let gb = 1u64 << 30;
        s.record_usage(NOW, "p", 10 * gb, 100 * gb, Some(NOW + 22 * DAY));
        // Same numbers within the hour: not a new sample.
        s.record_usage(NOW + 60, "p", 10 * gb, 100 * gb, Some(NOW + 22 * DAY));
        assert_eq!(s.usage.len(), 1);
        let one = s.usage_of("p");
        assert!(
            project(&one, NOW).unwrap().pace.is_none(),
            "one sample: no pace"
        );
        s.record_usage(NOW + 2 * DAY, "p", 18 * gb, 100 * gb, Some(NOW + 22 * DAY));
        s.record_usage(NOW + 2 * DAY, "other", 1, 2, None);
        let v = s.usage_of("p");
        let p = project(&v, NOW + 2 * DAY).unwrap();
        let pace = p.pace.unwrap();
        assert!((pace - 4.0 * gb as f64).abs() < 1.0, "4 GB a day");
        // 82 GB left at 4 GB a day: 20.5 days after the last sample.
        let out = p.runs_out.unwrap();
        assert!(((out - (NOW + 2 * DAY)) as f64 / DAY as f64 - 20.5).abs() < 0.01);
        // It expires in 20 days: the budget is what lasts until then.
        let budget = p.budget.unwrap() / gb as f64;
        assert!((budget - 82.0 / 20.0).abs() < 0.01);
    }
}
