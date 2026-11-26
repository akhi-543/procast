//! Screen-time accounting: what a sample is worth, categories, and the weekly summary text.

use crate::calendar::{civil_from_days, days_from_civil, weekday};
use crate::config::{FocusCategory, ScreenTime};
use crate::{apps, browser, focus, win};
use futures::channel::mpsc::UnboundedSender;
use rusqlite::{Connection, MAIN_DB, OptionalExtension};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::{
    System::SystemInformation::GetTickCount,
    UI::{
        Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
        WindowsAndMessaging::GetWindowThreadProcessId,
    },
};

/// A sample later than this means the machine slept; discard it.
const GAP_S: f64 = 10.0;
/// No input for this long = away.
const IDLE_S: f64 = 120.0;
const BROWSERS: [&str; 5] = ["chrome.exe", "msedge.exe", "brave.exe", "opera.exe", "vivaldi.exe"];
const IGNORED: [&str; 2] = ["", "lockapp.exe"];
const POLL: Duration = Duration::from_secs(2);
const FLUSH: Duration = Duration::from_secs(30);
/// Daily database copies kept.
const BACKUPS: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    App,
    Site,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::App => "app",
            Kind::Site => "site",
        }
    }
}

/// Credit one sample: the app, plus the site when a browser is in front.
pub fn account(exe: &str, idle_s: f64, site: &str, audible: bool, dt: f64) -> Vec<(Kind, String, f64)> {
    if !(dt > 0.0 && dt <= GAP_S) || IGNORED.contains(&exe) {
        return vec![];
    }
    // ponytail: one shared tab report; with the extension in two browsers the last to report wins.
    let browsing = BROWSERS.contains(&exe) && !site.is_empty();
    // ponytail: only browser tabs get the audible exemption; video in a native player counts as idle.
    if idle_s > IDLE_S && !(browsing && audible) {
        return vec![];
    }
    let mut out = vec![(Kind::App, exe.to_string(), dt)];
    if browsing {
        out.push((Kind::Site, site.to_string(), dt));
    }
    out
}

/// The unbroken-use streak: the app in front and for how long.
#[derive(Default)]
pub struct Streak {
    pub exe: String,
    pub seconds: f64,
}

impl Streak {
    /// `exe` is "" for a sample that wasn't credited. Returns 1 if this sample moved to a different app.
    pub fn advance(&mut self, exe: &str, dt: f64) -> u32 {
        if exe.is_empty() {
            *self = Streak::default();
            return 0;
        }
        let switched = !self.exe.is_empty() && self.exe != exe;
        self.exe = exe.to_string();
        self.seconds += dt;
        u32::from(switched)
    }
}

/// Lowercase app or site name -> category.
pub type Rules = HashMap<String, String>;

pub fn classify(kind: Kind, name: &str, rules: &Rules) -> String {
    let name = name.to_lowercase();
    let other = || "Other".to_string();
    if kind == Kind::App {
        return rules.get(&name).cloned().unwrap_or_else(other);
    }
    let parts: Vec<&str> = name.split('.').collect();
    // most specific suffix first, never the bare TLD
    (0..1.max(parts.len() - 1)).find_map(|i| rules.get(&parts[i..].join("."))).cloned().unwrap_or_else(other)
}

pub fn duration(seconds: f64) -> String {
    let m = (seconds / 60.0).round() as u64;
    if m >= 60 { format!("{}h {}m", m / 60, m % 60) } else { format!("{m}m") }
}

/// The weekly toast body, short enough for a toast.
pub fn summary_text(total: u64, previous: u64, top: &str) -> String {
    let mut text = format!("Last week {}", duration(total as f64));
    if previous > 0 {
        let change = ((total as f64 - previous as f64) / previous as f64 * 100.0).round() as i64;
        match change {
            0 => text += ", same",
            c => text += &format!(", {}% {}", c.abs(), if c > 0 { "more" } else { "less" }),
        }
    }
    format!("{text}. Most: {top}")
}

/// A day number as `YYYY-MM-DD`.
pub fn iso(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DD` as a day number; None for anything that isn't a real date.
pub fn parse_iso(s: &str) -> Option<i64> {
    let mut it = s.split('-');
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() || [(y, 4), (m, 2), (d, 2)].iter().any(|(p, n)| p.len() != *n || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let (y, m, d) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    let day = days_from_civil(y, m, d);
    (civil_from_days(day) == (y, m, d)).then_some(day)
}

// --- storage ----------------------------------------------------------------

/// Open (creating if needed) the database. Same schema as micro bot's `data.db`.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let con = Connection::open(path)?;
    con.execute_batch(
        "CREATE TABLE IF NOT EXISTS usage(
            day TEXT, hour INTEGER, kind TEXT, name TEXT, seconds REAL,
            PRIMARY KEY(day, hour, kind, name));
        CREATE TABLE IF NOT EXISTS daily(
            day TEXT PRIMARY KEY, switches INTEGER, longest REAL, first TEXT, last TEXT);
        CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);",
    )?;
    Ok(con)
}

/// One day's focus numbers not yet written.
pub struct Daily {
    pub switches: u32,
    pub longest: f64,
    pub first: String,
    pub last: String,
}

/// Counters not yet written to disk.
#[derive(Default)]
pub struct Pending {
    pub usage: HashMap<(i64, u8, Kind, String), f64>,
    pub daily: HashMap<i64, Daily>,
}

impl Pending {
    /// Add one sample. An empty `credits` (idle, asleep) leaves the daily focus numbers alone.
    pub fn credit(&mut self, day: i64, hour: u8, minute: &str, credits: &[(Kind, String, f64)], switched: u32, streak: f64) {
        for (kind, name, seconds) in credits {
            *self.usage.entry((day, hour, *kind, name.clone())).or_default() += seconds;
        }
        if !credits.is_empty() {
            let d = self.daily.entry(day).or_insert_with(|| Daily { switches: 0, longest: 0.0, first: minute.into(), last: minute.into() });
            d.switches += switched;
            d.longest = d.longest.max(streak);
            d.last = minute.into();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.usage.is_empty() && self.daily.is_empty()
    }
}

/// Write `p` in one transaction; `p` is cleared only if it all landed.
pub fn flush(con: &mut Connection, p: &mut Pending) -> rusqlite::Result<()> {
    if p.is_empty() {
        return Ok(());
    }
    let tx = con.transaction()?;
    {
        let mut usage = tx.prepare(
            "INSERT INTO usage VALUES(?,?,?,?,?) \
             ON CONFLICT(day, hour, kind, name) DO UPDATE SET seconds = seconds + excluded.seconds",
        )?;
        for ((day, hour, kind, name), seconds) in &p.usage {
            usage.execute(rusqlite::params![iso(*day), hour, kind.as_str(), name, seconds])?;
        }
        let mut daily = tx.prepare(
            "INSERT INTO daily VALUES(?,?,?,?,?) ON CONFLICT(day) DO UPDATE SET \
             switches = switches + excluded.switches, longest = MAX(longest, excluded.longest), \
             first = MIN(first, excluded.first), last = MAX(last, excluded.last)",
        )?;
        for (day, d) in &p.daily {
            daily.execute(rusqlite::params![iso(*day), d.switches, d.longest, d.first, d.last])?;
        }
    }
    tx.commit()?;
    *p = Pending::default();
    Ok(())
}

/// The span of the same length just before `from..=to`.
pub fn prior(from: i64, to: i64) -> (i64, i64) {
    let len = to - from + 1;
    (from - len, from - 1)
}

type Seconds = BTreeMap<String, f64>;

fn add(m: &mut Seconds, key: String, seconds: f64) {
    *m.entry(key).or_default() += seconds;
}

/// Drop what rounds to nothing; the rest rounded to whole seconds.
fn clean(m: &Seconds) -> BTreeMap<&str, i64> {
    m.iter().map(|(k, v)| (k.as_str(), v.round() as i64)).filter(|(_, v)| *v > 0).collect()
}

/// Usage in `from..=to`: (one category map per bucket, apps, sites). Buckets are the 24 hours
/// of the day when hourly, otherwise one per day.
fn aggregate(con: &Connection, from: i64, to: i64, rules: &Rules, hourly: bool) -> rusqlite::Result<(Vec<Seconds>, Seconds, Seconds)> {
    let mut buckets = vec![Seconds::new(); if hourly { 24 } else { (to - from + 1).max(0) as usize }];
    let (mut apps, mut sites) = (Seconds::new(), Seconds::new());
    // ponytail: site rows don't record which browser, so all browsers are assumed to share
    // Chrome's category (Browsing by default). Upgrade path: store the browser per site row.
    let browser = classify(Kind::App, "chrome.exe", rules);
    let mut q = con.prepare("SELECT day, hour, kind, name, seconds FROM usage WHERE day BETWEEN ? AND ?")?;
    let rows = q.query_map([iso(from), iso(to)], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, f64>(4)?)))?;
    for row in rows {
        let (day, hour, kind, name, seconds) = row?;
        let idx = if hourly { Some(hour) } else { parse_iso(&day).map(|d| d - from) };
        let Some(bucket) = idx.and_then(|i| buckets.get_mut(usize::try_from(i).ok()?)) else { continue };
        let kind = if kind == "app" { Kind::App } else { Kind::Site };
        add(bucket, classify(kind, &name, rules), seconds);
        if kind == Kind::App {
            add(&mut apps, name, seconds);
        } else {
            add(&mut sites, name, seconds);
            add(bucket, browser.clone(), -seconds); // site time is already inside the browser's app time
        }
    }
    Ok((buckets, apps, sites))
}

fn by_category(buckets: &[Seconds]) -> Seconds {
    let mut totals = Seconds::new();
    for (cat, s) in buckets.iter().flatten() {
        add(&mut totals, cat.clone(), *s);
    }
    totals
}

/// Stats for `from..=to`, compared with the `before` span.
pub fn stats(con: &Connection, from: i64, to: i64, before: (i64, i64), rules: &Rules) -> rusqlite::Result<Value> {
    let hourly = from == to;
    let (buckets, apps, sites) = aggregate(con, from, to, rules, hourly)?;
    let (prev, prev_apps, _) = aggregate(con, before.0, before.1, rules, false)?;

    let top = |m: &Seconds, kind| {
        let mut ranked: Vec<_> = clean(m).into_iter().collect();
        ranked.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
        ranked.truncate(12);
        ranked.into_iter().map(|(n, s)| json!({"name": n, "seconds": s, "category": classify(kind, n, rules)})).collect::<Vec<_>>()
    };
    let totals = by_category(&buckets);
    let mut categories: Vec<_> = clean(&totals).into_iter().collect();
    categories.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
    let labels = (0..buckets.len() as i64).map(|i| if hourly { json!(i) } else { json!(iso(from + i)) });
    let span = [iso(from), iso(to)];
    let (tracked, switches, longest, first, last): (i64, i64, f64, Option<String>, Option<String>) = con.query_row(
        "SELECT COUNT(*), COALESCE(SUM(switches), 0), COALESCE(MAX(longest), 0), MIN(first), MAX(last) \
         FROM daily WHERE day BETWEEN ? AND ?",
        span.clone(),
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let longest_day: Option<String> =
        con.query_row("SELECT day FROM daily WHERE day BETWEEN ? AND ? ORDER BY longest DESC LIMIT 1", span.clone(), |r| r.get(0)).optional()?;

    Ok(json!({
        "from": span[0],
        "to": span[1],
        "total": apps.values().sum::<f64>().round() as i64,
        "previous": prev_apps.values().sum::<f64>().round() as i64,
        "categories": categories.into_iter().map(|(n, s)| json!({"name": n, "seconds": s})).collect::<Vec<_>>(),
        "previous_categories": clean(&by_category(&prev)),
        "buckets": labels.zip(&buckets).map(|(label, b)| json!({"label": label, "categories": clean(b)})).collect::<Vec<_>>(),
        "apps": top(&apps, Kind::App),
        "sites": top(&sites, Kind::Site),
        "focus": {
            "days": tracked,
            "switches": switches,
            "longest": longest.round() as i64,
            "longest_day": longest_day,
            "first": first,
            "last": last,
        },
    }))
}

/// `{"days": {iso day: {category: seconds}}}`
pub fn days(con: &Connection, from: i64, to: i64, rules: &Rules) -> rusqlite::Result<Value> {
    let (buckets, _, _) = aggregate(con, from, to, rules, false)?;
    let days: serde_json::Map<String, Value> = buckets.iter().enumerate().map(|(i, b)| (iso(from + i as i64), json!(clean(b)))).collect();
    Ok(json!({ "days": days }))
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// One CSV row per counter, UTF-8 with a BOM so Excel reads it right.
pub fn export_csv(con: &Connection, from: i64, to: i64, rules: &Rules) -> rusqlite::Result<Vec<u8>> {
    let mut out = String::from("\u{feff}day,hour,kind,name,category,seconds\r\n");
    let mut q = con.prepare("SELECT day, hour, kind, name, seconds FROM usage WHERE day BETWEEN ? AND ? ORDER BY day, hour, kind, name")?;
    let rows = q.query_map([iso(from), iso(to)], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, f64>(4)?)))?;
    for row in rows {
        let (day, hour, kind, name, seconds) = row?;
        // A leading = + - @ would make a spreadsheet run the name as a formula.
        let shown = if name.starts_with(['=', '+', '-', '@']) { format!("'{name}") } else { name.clone() };
        let category = classify(if kind == "app" { Kind::App } else { Kind::Site }, &name, rules);
        out += &format!("{day},{hour},{kind},{},{},{}\r\n", csv_field(&shown), csv_field(&category), seconds.round() as i64);
    }
    Ok(out.into_bytes())
}

// --- the launcher's Screen Time view ------------------------------------------

/// How long a period the launcher's Screen Time view shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Range {
    Day,
    Week,
    Month,
}

/// A range and a day inside it: the day, the week or the month holding `anchor`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Period {
    pub range: Range,
    pub anchor: i64,
}

/// First and last day of the month holding `day`.
fn month_of(day: i64) -> (i64, i64) {
    let (y, m, _) = civil_from_days(day);
    (days_from_civil(y, m, 1), days_from_civil(y + m / 12, m % 12 + 1, 1) - 1)
}

/// The period's days, `(from, to)`: the day, Monday to Sunday, or first to last of the month.
fn bounds(p: Period) -> (i64, i64) {
    match p.range {
        Range::Day => (p.anchor, p.anchor),
        Range::Week => {
            let monday = p.anchor - weekday(p.anchor);
            (monday, monday + 6)
        }
        Range::Month => month_of(p.anchor),
    }
}

/// The period shown and the one it is compared with, as `(from, to)` day numbers. The shown
/// one is whole; the one before is the same stretch of it that has happened (a partial week
/// or month against the same part of the last one), clamped to the last month's length.
pub fn span(p: Period, today: i64) -> ((i64, i64), (i64, i64)) {
    let shown = bounds(p);
    let done = (shown.0, shown.1.min(today));
    let before = match p.range {
        Range::Day => (done.0 - 1, done.1 - 1),
        Range::Week => (done.0 - 7, done.1 - 7),
        Range::Month => {
            let (first, last) = month_of(shown.0 - 1);
            (first, (first + done.1 - done.0).min(last))
        }
    };
    (shown, before)
}

/// The period `dir` (+1 or -1) periods away; a month lands on its 1st. Never one that starts
/// after today: that returns `p`.
pub fn step(p: Period, dir: i64, today: i64) -> Period {
    let anchor = match p.range {
        Range::Day => p.anchor + dir,
        Range::Week => p.anchor + 7 * dir,
        Range::Month => {
            let (first, last) = month_of(p.anchor);
            if dir > 0 { last + 1 } else { month_of(first - 1).0 }
        }
    };
    let next = Period { anchor, ..p };
    if bounds(next).0 > today { p } else { next }
}

/// The same anchor under another range.
pub fn with_range(p: Period, r: Range) -> Period {
    Period { range: r, ..p }
}

fn is_current(p: Period, today: i64) -> bool {
    let (from, to) = bounds(p);
    (from..=to).contains(&today)
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// "Thu 19 Nov"
fn day_label(day: i64) -> String {
    let (_, m, d) = civil_from_days(day);
    format!("{} {d} {}", WEEKDAYS[weekday(day) as usize], &MONTHS[m as usize - 1][..3])
}

/// "17 – 23 Nov" or "29 Dec – 4 Jan"
fn week_label(p: Period) -> String {
    let ((_, m1, d1), (_, m2, d2)) = (civil_from_days(bounds(p).0), civil_from_days(bounds(p).1));
    let month = |m: i64| &MONTHS[m as usize - 1][..3];
    if m1 == m2 { format!("{d1} – {d2} {}", month(m2)) } else { format!("{d1} {} – {d2} {}", month(m1), month(m2)) }
}

/// "Today", "Yesterday", "Thu 19 Nov"; "This week", "17 – 23 Nov"; "This month", "October 2026".
pub fn title(p: Period, today: i64) -> String {
    let current = is_current(p, today);
    match p.range {
        Range::Day if current => "Today".into(),
        Range::Day if p.anchor == today - 1 => "Yesterday".into(),
        Range::Day => day_label(p.anchor),
        Range::Week if current => "This week".into(),
        Range::Week => week_label(p),
        Range::Month if current => "This month".into(),
        Range::Month => {
            let (y, m, _) = civil_from_days(p.anchor);
            format!("{} {y}", MONTHS[m as usize - 1])
        }
    }
}

/// What follows a total: "so far today", "this week", "this month"; else "on Thu 19 Nov",
/// "17 – 23 Nov", "in October 2026".
pub fn caption(p: Period, today: i64) -> String {
    match (p.range, is_current(p, today)) {
        (Range::Day, true) => "so far today".into(),
        (Range::Week, true) => "this week".into(),
        (Range::Month, true) => "this month".into(),
        (Range::Day, false) => format!("on {}", day_label(p.anchor)),
        (Range::Week, false) => week_label(p),
        (Range::Month, false) => format!("in {}", title(p, today)),
    }
}

/// The categories that own a colour slot, in slot order; a category outside them gets the spare
/// colour (the largest one) or grey.
pub const SLOTS: [&str; 7] = ["Development", "Productivity", "Messaging", "Social", "Streaming", "Gaming", "Browsing"];

fn day_total(day: Option<&Value>) -> u64 {
    day.and_then(Value::as_object).map_or(0, |m| m.values().filter_map(Value::as_u64).sum())
}

/// Productive seconds over total seconds for each of `weeks` weeks from `first_monday`; None for
/// a week under a minute.
pub fn week_shares(days: &Value, productive: &[String], first_monday: i64, weeks: usize) -> Vec<Option<f64>> {
    (0..weeks as i64)
        .map(|w| {
            let (mut total, mut good) = (0, 0);
            for day in first_monday + w * 7..first_monday + w * 7 + 7 {
                let d = &days["days"][iso(day)];
                total += day_total(Some(d));
                good += productive.iter().map(|c| d[c.as_str()].as_u64().unwrap_or(0)).sum::<u64>();
            }
            (total >= 60).then(|| good as f64 / total as f64)
        })
        .collect()
}

/// A heatmap shade 0..=4 for each day of `from..=to`: 0 under a minute, else by share of the busiest day.
pub fn heat_levels(days: &Value, from: i64, to: i64) -> Vec<u8> {
    let totals: Vec<u64> = (from..=to).map(|d| day_total(days["days"].get(iso(d)))).collect();
    let max = totals.iter().copied().max().unwrap_or(0);
    totals.iter().map(|&t| if t < 60 { 0 } else { (1 + t * 4 / max).min(4) as u8 }).collect()
}

/// `(category, now, before)`: the slot categories always, then any other with time in either period.
pub fn compare_rows(stats: &Value) -> Vec<(String, u64, u64)> {
    let now: Vec<(&str, u64)> = stats["categories"].as_array().into_iter().flatten().filter_map(|c| Some((c["name"].as_str()?, c["seconds"].as_u64()?))).collect();
    let before = |n: &str| stats["previous_categories"][n].as_u64().unwrap_or(0);
    let seconds = |n: &str| now.iter().find(|(c, _)| *c == n).map_or(0, |(_, s)| *s);
    let mut rows: Vec<_> = SLOTS.iter().map(|&n| (n.to_string(), seconds(n), before(n))).collect();
    let mut rest: Vec<(String, u64, u64)> = now.iter().filter(|(n, _)| !SLOTS.contains(n)).map(|&(n, s)| (n.to_string(), s, before(n))).collect();
    rest.extend(stats["previous_categories"].as_object().into_iter().flatten().filter(|(n, _)| !SLOTS.contains(&n.as_str()) && seconds(n) == 0).filter_map(|(n, s)| Some((n.clone(), 0, s.as_u64()?))));
    rest.retain(|r| r.1 + r.2 > 0);
    rest.sort_by_key(|r| std::cmp::Reverse(r.1));
    rows.extend(rest);
    rows
}

/// The colour slot of a category: 0..7 its own, 7 the spare for the largest other category, None grey.
pub fn color_slot(category: &str, stats: &Value) -> Option<usize> {
    if let Some(i) = SLOTS.iter().position(|&s| s == category) {
        return Some(i);
    }
    let first = stats["categories"].as_array()?.iter().filter_map(|c| c["name"].as_str()).find(|n| *n != "Other" && !SLOTS.contains(n));
    (first == Some(category)).then_some(7)
}

/// The bucket with the most time: "11 AM" for an hour, "Wed 19" for a day; None when all are empty.
pub fn busiest(stats: &Value) -> Option<(String, u64)> {
    let (label, seconds) = stats["buckets"].as_array()?.iter().map(|b| (&b["label"], day_total(Some(&b["categories"])))).fold((None, 0), |best, (l, s)| if s > best.1 { (Some(l), s) } else { best });
    let name = match label?.as_str().and_then(parse_iso) {
        Some(day) => format!("{} {}", WEEKDAYS[weekday(day) as usize], civil_from_days(day).2),
        None => {
            let h = label?.as_u64()?;
            format!("{} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
        }
    };
    Some((name, seconds))
}

/// Whether an answer read for `asked` under `asked_gen` still fits what is on screen.
pub fn fresh(current: Period, asked: Period, current_gen: u64, asked_gen: u64) -> bool {
    current == asked && current_gen == asked_gen
}

/// The view's first line: "15m today · 5m more than yesterday".
pub fn header(total: u64, previous: u64, p: Period, today: i64) -> String {
    let current = is_current(p, today);
    let (now, then) = match (p.range, current) {
        (Range::Day, true) => ("today".to_string(), "yesterday"),
        (Range::Week, true) => ("this week".to_string(), "last week"),
        (Range::Month, true) => ("this month".to_string(), "last month"),
        (Range::Day, false) => (caption(p, today), "the day before"),
        (Range::Week, false) => (caption(p, today), "the week before"),
        (Range::Month, false) => (caption(p, today), "the month before"),
    };
    let mut text = format!("{} {now}", duration(total as f64));
    if previous > 0 {
        let gap = duration(total.abs_diff(previous) as f64);
        text += &if gap == "0m" { format!(" · same as {then}") } else { format!(" · {gap} {} than {then}", if total > previous { "more" } else { "less" }) };
    }
    text
}

/// One line of the view: a category, app or site, with its time and category.
pub struct Row {
    pub section: &'static str,
    pub name: String,
    pub seconds: u64,
    pub category: String,
}

/// The view's lines from `stats()`: categories, then apps, then sites, biggest first.
pub fn rows(stats: &Value) -> Vec<Row> {
    ["Categories", "Apps", "Sites"]
        .into_iter()
        .flat_map(|section| {
            let mut part: Vec<Row> = stats[section.to_lowercase()]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| {
                    let (name, seconds) = (r["name"].as_str()?, r["seconds"].as_u64()?);
                    Some(Row { section, name: name.into(), seconds, category: r["category"].as_str().unwrap_or(name).into() })
                })
                .collect();
            part.sort_by_key(|r| std::cmp::Reverse(r.seconds));
            part
        })
        .collect()
}

/// Seconds per hour of one day for each app and site (what `stats()` keeps only per category).
pub type Hours = HashMap<(Kind, String), [f64; 24]>;

pub fn hours_by_name(con: &Connection, day: i64) -> rusqlite::Result<Hours> {
    let mut out = Hours::new();
    let mut q = con.prepare("SELECT kind, name, hour, SUM(seconds) FROM usage WHERE day = ? GROUP BY kind, name, hour")?;
    for row in q.query_map([iso(day)], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, f64>(3)?)))? {
        let (kind, name, hour, seconds) = row?;
        if let Some(h) = out.entry((if kind == "app" { Kind::App } else { Kind::Site }, name)).or_insert([0.0; 24]).get_mut(usize::try_from(hour).unwrap_or(usize::MAX)) {
            *h += seconds;
        }
    }
    Ok(out)
}

/// The detail pane's key / value lines for `row` (None: the whole span): its time and share,
/// its time per hour (per day over a week, for categories), an app's category, then the focus
/// numbers. `hours` has the per-hour numbers of apps and sites when the span is one day.
pub fn detail(stats: &Value, row: Option<&Row>, hours: &Hours) -> Vec<(String, String)> {
    let secs = |key: &str| stats[key].as_u64().unwrap_or(0);
    let line = |k: &str, v: String| (k.to_string(), v);
    let mut out = match row {
        None => vec![line("Total", duration(secs("total") as f64)), line("Before", duration(secs("previous") as f64))],
        Some(r) => {
            let mut out = vec![line("Time", duration(r.seconds as f64))];
            if secs("total") > 0 {
                out.push(line("Share", format!("{}% of total", (r.seconds as f64 * 100.0 / secs("total") as f64).round())));
            }
            if r.section == "Categories" {
                for b in stats["buckets"].as_array().into_iter().flatten() {
                    let label = b["label"].as_i64().map_or_else(|| b["label"].as_str().unwrap_or_default().to_string(), |h| format!("{h:02}:00"));
                    out.extend(b["categories"][&r.name].as_u64().map(|s| line(&label, duration(s as f64))));
                }
            } else {
                out.push(line("Category", r.category.clone()));
                let kind = if r.section == "Apps" { Kind::App } else { Kind::Site };
                for (h, s) in hours.get(&(kind, r.name.clone())).into_iter().flatten().enumerate() {
                    if s.round() > 0.0 {
                        out.push(line(&format!("{h:02}:00"), duration(*s)));
                    }
                }
            }
            out
        }
    };
    out.extend(focus_lines(stats));
    out
}

/// The focus numbers of `stats` (none for a span with no tracked day).
fn focus_lines(stats: &Value) -> Vec<(String, String)> {
    let f = &stats["focus"];
    if f["days"].as_u64().unwrap_or(0) == 0 {
        return vec![];
    }
    let day = stats["from"] == stats["to"];
    let mut out = vec![("Switches".to_string(), f["switches"].as_u64().unwrap_or(0).to_string()), ("Longest focus".into(), duration(f["longest"].as_u64().unwrap_or(0) as f64))];
    for (key, label) in [("first", if day { "First active" } else { "Earliest start" }), ("last", if day { "Last active" } else { "Latest end" })] {
        out.extend(f[key].as_str().map(|t| (label.to_string(), t.to_string())));
    }
    out
}

// --- categories, settings, files --------------------------------------------

/// The classification rules: every app and site of the categories, lower case; a name in two
/// categories belongs to the first. `Other`'s entries override a broader suffix rule.
pub fn rules(cats: &[FocusCategory]) -> Rules {
    let mut rules = Rules::new();
    for c in cats {
        let name = if c.name.eq_ignore_ascii_case("other") { "Other" } else { c.name.as_str() };
        let apps = c.apps.iter().map(|a| focus::norm_app(a));
        let sites = c.sites.iter().map(|s| focus::norm_site(s));
        for key in apps.chain(sites).filter(|k| !k.is_empty()) {
            rules.entry(key).or_insert_with(|| name.to_string());
        }
    }
    rules
}

/// What an app (`.exe`) or site is stored as: (is_app, key). None for junk: empty, too long,
/// control characters, or a "site" that is not a plain host (no URL, user@ or :port, which
/// `norm_site` would cut; "www." is left as "www" by it).
fn name_key(name: &str) -> Option<(bool, String)> {
    let name = name.trim().to_lowercase();
    if name.is_empty() || name.chars().count() > 260 || name.chars().any(char::is_control) {
        return None;
    }
    let is_app = name.ends_with(".exe");
    let key = if is_app { focus::norm_app(&name) } else { focus::norm_site(&name) };
    let host = |s: &str| s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    let ok = if is_app { key.len() > ".exe".len() } else { (1..=253).contains(&key.len()) && key != "www" && host(&name) };
    ok.then_some((is_app, key))
}

/// Writes `changed` categories into config.toml's text as `[[focus_category]]` overrides (a
/// built-in becomes one; comments elsewhere stay).
fn write_categories(text: &str, changed: &[FocusCategory]) -> Result<String, String> {
    let text = focus::add_categories(text, changed)?.0; // creates the overrides that are missing
    let own = crate::config::parse(&text)?.focus_category;
    let array = |l: &[String]| toml_edit::Value::from(toml_edit::Array::from_iter(l.iter().map(String::as_str)));
    changed.iter().try_fold(text, |text, c| {
        let i = own.iter().position(|o| o.name.eq_ignore_ascii_case(&c.name)).ok_or("category override missing")?;
        let path = |field: &str| ["focus_category".to_string(), i.to_string(), field.to_string()];
        let text = crate::config::write_value(&text, &path("apps"), array(&c.apps))?;
        crate::config::write_value(&text, &path("sites"), array(&c.sites))
    })
}

/// Moves an app (`.exe`) or site out of every category into `category`, returning the new
/// config.toml text. Each category that changes is written as a `[[focus_category]]` override
/// (a built-in becomes one), so comments elsewhere stay. `Other` holds an explicit entry that
/// beats a broader suffix rule (m.example.com vs example.com).
pub fn move_name(text: &str, cats: &[FocusCategory], name: &str, category: &str) -> Result<String, String> {
    let (is_app, key) = name_key(name).ok_or("bad name")?;
    let target = focus::category(cats, category).ok_or("unknown category")?.name.clone();
    let norm: fn(&str) -> String = if is_app { focus::norm_app } else { focus::norm_site };
    let mut changed = Vec::new();
    for old in cats {
        let mut c = old.clone();
        let list = if is_app { &mut c.apps } else { &mut c.sites };
        list.retain(|n| norm(n) != key);
        if c.name == target {
            if is_app { c.apps.push(key.clone()) } else { c.sites.push(key.clone()) }
        }
        if c != *old {
            changed.push(c);
        }
    }
    write_categories(text, &changed)
}

/// `{category: "productive" | "distracting"}`; anything unlisted is neutral.
pub fn kinds(st: &ScreenTime) -> Value {
    let mut out = serde_json::Map::new();
    for (list, kind) in [(&st.productive, "productive"), (&st.distracting, "distracting")] {
        for c in list {
            out.insert(c.clone(), json!(kind));
        }
    }
    Value::Object(out)
}

pub fn db_path() -> PathBuf {
    crate::config::dir().join("screentime.db")
}

fn backups_dir() -> PathBuf {
    crate::config::dir().join("screentime-backups")
}

/// One copy of the database per day in `folder` (the SQLite backup API, so it is consistent
/// while the sampler writes); the newest `BACKUPS` are kept.
pub fn backup(con: &Connection, folder: &Path, day: i64) -> rusqlite::Result<()> {
    let io = |e: std::io::Error| rusqlite::Error::ToSqlConversionFailure(Box::new(e));
    std::fs::create_dir_all(folder).map_err(io)?;
    let target = folder.join(format!("screentime-{}.db", iso(day)));
    if !target.exists() {
        // Copied under another name, so a failed copy is never taken for the day's backup.
        let partial = target.with_extension("tmp");
        con.backup(MAIN_DB, &partial, None)?;
        std::fs::rename(&partial, &target).map_err(io)?;
    }
    let mut copies: Vec<PathBuf> = std::fs::read_dir(folder)
        .map_err(io)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("screentime-") && n.ends_with(".db")))
        .collect();
    copies.sort();
    for old in &copies[..copies.len().saturating_sub(BACKUPS)] {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

// --- weekly summary ---------------------------------------------------------------

/// The toast about the week that just ended, once per week (the first call on or after its
/// Monday): `on` is `[screen_time] weekly_summary`. The week is recorded in `meta` whether or not
/// there is anything to say; None too when the database can't be read (tried again next time).
pub fn weekly_summary(con: &Connection, today: i64, rules: &Rules, on: bool) -> Option<String> {
    let monday = today - crate::calendar::weekday(today);
    let week = iso(monday);
    let seen: Option<String> = con.query_row("SELECT value FROM meta WHERE key = 'summary_week'", [], |r| r.get(0)).optional().ok()?;
    if seen.as_deref() == Some(week.as_str()) {
        return None;
    }
    let last = (monday - 7, monday - 1);
    let s = stats(con, last.0, last.1, prior(last.0, last.1), rules).ok()?;
    con.execute("INSERT OR REPLACE INTO meta VALUES('summary_week', ?)", [&week]).ok()?;
    let total = s["total"].as_u64()?;
    if !on || total < 60 {
        return None;
    }
    Some(summary_text(total, s["previous"].as_u64().unwrap_or(0), s["categories"][0]["name"].as_str().unwrap_or("Other")))
}

// --- import from micro bot ----------------------------------------------------------

/// micro bot's category names that Focus has under another name: Focus's is kept.
pub const RENAME: &[(&str, &str)] = &[("Communication", "Messaging"), ("Entertainment", "Streaming"), ("Games", "Gaming")];

fn renamed(name: &str) -> &str {
    let name = name.trim();
    RENAME.iter().find(|(from, _)| from.eq_ignore_ascii_case(name)).map_or(name, |(_, to)| to)
}

/// What an import brought in.
pub struct Imported {
    pub rows: usize,
    pub days: usize,
    pub names_added: usize,
}

const NOT_MICRO_BOT: &str = "not a micro bot database";

/// Adds micro bot's `data.db` (`from`) to `con`: seconds are added, daily focus numbers merged as
/// `flush` does. All or nothing; returns (usage rows, days). A second import adds the hours
/// again, so it is refused with a warning unless `force`. `stop` runs once, after every check
/// has passed and just before anything is written.
pub fn import_db(con: &mut Connection, from: &Path, force: bool, stop: impl FnOnce()) -> Result<(usize, usize), String> {
    // ATTACH would create a missing file.
    if !from.is_file() {
        return Err("No data.db in that folder".into());
    }
    let attach = con.execute("ATTACH ?1 AS mb", [from.to_string_lossy()]);
    attach.map_err(|e| if e.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase) { NOT_MICRO_BOT.to_string() } else { e.to_string() })?;
    let done = merge_attached(con, from, force, stop);
    let _ = con.execute("DETACH mb", []);
    done
}

fn merge_attached(con: &mut Connection, from: &Path, force: bool, stop: impl FnOnce()) -> Result<(usize, usize), String> {
    let tables: Vec<String> = con
        .prepare("SELECT name FROM mb.sqlite_master WHERE type = 'table'")
        .and_then(|mut q| q.query_map([], |r| r.get(0))?.collect())
        .map_err(|_| NOT_MICRO_BOT.to_string())?;
    if !tables.iter().any(|t| t == "usage") {
        return Err(NOT_MICRO_BOT.into());
    }
    let meta = |key: &str| con.query_row("SELECT value FROM meta WHERE key = ?", [key], |r| r.get::<_, String>(0)).optional().map_err(|e| e.to_string());
    if !force && meta("imported_from")?.is_some() {
        let at = meta("imported_at")?.unwrap_or_default();
        return Err(format!("Already imported {at}"));
    }
    stop();
    let db = |e: rusqlite::Error| e.to_string();
    let tx = con.transaction().map_err(db)?;
    let rows = tx
        .execute(
            "INSERT INTO usage SELECT day, hour, kind, name, seconds FROM mb.usage WHERE true \
             ON CONFLICT(day, hour, kind, name) DO UPDATE SET seconds = seconds + excluded.seconds",
            [],
        )
        .map_err(db)?;
    let days = tx.query_row("SELECT COUNT(DISTINCT day) FROM mb.usage", [], |r| r.get::<_, usize>(0)).map_err(db)?;
    if tables.iter().any(|t| t == "daily") {
        tx.execute(
            "INSERT INTO daily SELECT day, switches, longest, first, last FROM mb.daily WHERE true ON CONFLICT(day) DO UPDATE SET \
             switches = switches + excluded.switches, longest = MAX(longest, excluded.longest), \
             first = MIN(first, excluded.first), last = MAX(last, excluded.last)",
            [],
        )
        .map_err(db)?;
    }
    // micro bot's weekly toast is not due again at once.
    if tables.iter().any(|t| t == "meta") {
        tx.execute("INSERT OR IGNORE INTO meta(key, value) SELECT key, value FROM mb.meta WHERE key = 'summary_week'", []).map_err(db)?;
    }
    tx.execute("INSERT OR REPLACE INTO meta VALUES('imported_from', ?)", [from.to_string_lossy()]).map_err(db)?;
    tx.execute("INSERT OR REPLACE INTO meta VALUES('imported_at', 'on ' || datetime('now', 'localtime'))", []).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok((rows, days))
}

fn names_of(c: &mut FocusCategory, is_app: bool) -> &mut Vec<String> {
    if is_app { &mut c.apps } else { &mut c.sites }
}

/// micro bot's `categories.json` (`{category: [app or site, ...]}`) merged into config.toml's
/// text, with the count of names added. Focus wins: a name some category already holds stays
/// there; the rest go to the category of the same name (see `RENAME`), created if Focus lacks
/// it. Names that are not a program or a plain host are skipped.
pub fn import_categories(text: &str, cats: &[FocusCategory], json: &str) -> Result<(String, usize), String> {
    let v: Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| format!("Bad categories.json, line {}", e.line()))?;
    let map = v.as_object().ok_or("categories.json isn't {category: [names]}")?;
    let mut now = cats.to_vec();
    let mut added = 0;
    for (name, list) in map {
        let target = renamed(name);
        for item in list.as_array().into_iter().flatten().filter_map(Value::as_str) {
            let Some((is_app, key)) = name_key(item) else { continue };
            let norm: fn(&str) -> String = if is_app { focus::norm_app } else { focus::norm_site };
            if now.iter_mut().any(|c| names_of(c, is_app).iter().any(|n| norm(n) == key)) {
                continue;
            }
            let i = now.iter().position(|c| c.name.eq_ignore_ascii_case(target)).unwrap_or_else(|| {
                now.push(FocusCategory { name: target.to_string(), apps: vec![], sites: vec![] });
                now.len() - 1
            });
            names_of(&mut now[i], is_app).push(key);
            added += 1;
        }
    }
    let changed: Vec<FocusCategory> = now.into_iter().filter(|c| !cats.contains(c)).collect();
    if changed.is_empty() {
        return Ok((text.to_string(), 0));
    }
    Ok((write_categories(text, &changed)?, added))
}

/// micro bot's `settings.json` as `[screen_time]` in config.toml's text, category names mapped
/// (see `RENAME`), and tracking switched on. What the file leaves out stays as it was.
pub fn import_settings(text: &str, json: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| format!("Bad settings.json, line {}", e.line()))?;
    if !v.is_object() {
        return Err("settings.json isn't an object".into());
    }
    let set = |text: String, field: &str, value: toml_edit::Value| crate::config::write_value(&text, &["screen_time".to_string(), field.to_string()], value);
    let mut text = text.to_string();
    for field in ["productive", "distracting"] {
        let Some(list) = v[field].as_array() else { continue };
        let mut names: Vec<&str> = vec![];
        for name in list.iter().filter_map(Value::as_str).map(renamed) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        text = set(text, field, toml_edit::Array::from_iter(names).into())?;
    }
    if let Some(on) = v["weekly_summary"].as_bool() {
        text = set(text, "weekly_summary", on.into())?;
    }
    set(text, "enabled", true.into())
}

/// Everything in micro bot's `folder` into procast: the database into `db`, categories and
/// settings into the config file. Files that don't read or parse fail before anything changes,
/// and `stop` (see `import_db`) runs only after that. The config is written after the database:
/// if that write fails, a retry needs "Add the hours again", which counts them twice.
pub fn import_folder(folder: &Path, config: &Path, db: &Path, force: bool, stop: impl FnOnce()) -> Result<Imported, String> {
    let read = |name: &str| match std::fs::read_to_string(folder.join(name)) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(format!("Could not read {name}")),
    };
    let (categories, settings) = (read("categories.json")?, read("settings.json")?.unwrap_or_else(|| "{}".into()));
    let merge = |text: &str| -> Result<(String, usize), String> {
        let cats = focus::categories(&crate::config::parse(text)?.focus_category);
        let (text, names) = match &categories {
            Some(json) => import_categories(text, &cats, json)?,
            None => (text.to_string(), 0),
        };
        Ok((import_settings(&text, &settings)?, names))
    };
    merge(&focus::text_or_empty(std::fs::read_to_string(config))?)?; // a dry run: bad files fail here
    if let Some(dir) = db.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut con = open(db).map_err(|e| e.to_string())?;
    let (rows, days) = import_db(&mut con, &folder.join("data.db"), force, stop)?;
    drop(con);
    let mut names_added = 0;
    crate::config::edit_file(config, |text| {
        let (new, names) = merge(text)?;
        names_added = names;
        Ok(new)
    })?;
    Ok(Imported { rows, days, names_added })
}

/// The Import Screen Time command: `import_folder` on procast's own files, which stops micro
/// bot's scheduled tracker (its last 30 s of counters are lost, accepted) once the import can go
/// ahead. Returns the toast text; a task that won't disable is a toast of its own on `tx`.
/// Blocks on SQLite and `schtasks`: never call it from the UI thread.
pub fn import_from(folder: &Path, force: bool, tx: &UnboundedSender<win::Msg>) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const TASK: &str = "WellbeingTracker";
    static BUSY: Mutex<()> = Mutex::new(());
    let Ok(_one) = BUSY.try_lock() else { return Err("An import is already running".into()) };
    // Before it does anything.
    if !folder.join("data.db").is_file() {
        return Err("No data.db in that folder".into());
    }
    let stop = || {
        // The dev instance must not disable the micro bot that is live.
        if win::dev() {
            return;
        }
        let schtasks = |args: &[&str]| std::process::Command::new("schtasks").args(args).creation_flags(CREATE_NO_WINDOW).status().is_ok_and(|s| s.success());
        // No such task is fine: micro bot may never have been scheduled.
        if !schtasks(&["/Query", "/TN", TASK]) {
            return;
        }
        schtasks(&["/End", "/TN", TASK]);
        if !schtasks(&["/Change", "/TN", TASK, "/Disable"]) {
            let _ = tx.unbounded_send(win::Msg::error("micro bot's task wasn't disabled"));
        }
    };
    let got = import_folder(folder, &crate::config::path(), &db_path(), force, stop)?;
    Ok(format!("Imported {} days of screen time ({} rows, {} names)", got.days, got.rows, got.names_added))
}

// --- sampler ------------------------------------------------------------------

/// The active tab of the browser in front, as the browser extension last reported it.
pub struct Tab {
    pub host: String,
    pub audible: bool,
}

pub static TAB: RwLock<Option<Tab>> = RwLock::new(None);

/// The tab answer that came in since the last call, if any.
fn take_tab() -> (String, bool) {
    TAB.write().unwrap_or_else(PoisonError::into_inner).take().map(|t| (t.host, t.audible)).unwrap_or_default()
}

/// Counters not yet on disk: the sampler adds, `flush_pending` writes.
static PENDING: LazyLock<Mutex<Pending>> = LazyLock::new(Mutex::default);

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Write the pending counters to the database; they are kept for the next try if that fails.
/// Blocks on SQLite: never call it from the UI thread.
pub fn flush_pending() -> Result<(), String> {
    let mut pending = locked(&PENDING);
    if pending.is_empty() {
        return Ok(());
    }
    let path = db_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut con = open(&path).map_err(|e| e.to_string())?;
    flush(&mut con, &mut pending).map_err(|e| e.to_string())
}

/// Lower-case file name of the program in front: "" without a window, "system" when its path
/// can't be read (a protected process is still screen time).
fn foreground_exe() -> String {
    let hwnd = win::foreground();
    if hwnd.is_invalid() {
        return String::new();
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    apps::exe_path(pid).map_or_else(|| "system".into(), |p| p.rsplit('\\').next().unwrap_or_default().to_lowercase())
}

/// Seconds since the last keyboard or mouse input; 0 if Windows won't say.
fn idle_seconds() -> f64 {
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // The tick count wraps every 49 days, so subtract wrapping.
    unsafe {
        if GetLastInputInfo(&mut info).as_bool() { GetTickCount().wrapping_sub(info.dwTime) as f64 / 1000.0 } else { 0.0 }
    }
}

/// The sampling loop (port of `tracker.py` `run()`): every `POLL` credit what is in front,
/// every `FLUSH` write it down, once a day back the database up. Ends when `stop` fires or is dropped.
fn run(stop: mpsc::Receiver<()>, browsers: browser::Browsers, tx: UnboundedSender<win::Msg>, rules: Arc<RwLock<Rules>>, st: Arc<RwLock<ScreenTime>>) {
    let mut streak = Streak::default();
    let mut tidied = i64::MIN;
    let mut last = Instant::now();
    let mut last_flush = last;
    while stop.recv_timeout(POLL) == Err(RecvTimeoutError::Timeout) {
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f64();
        last = now;
        let (day, hour, minute) = win::local_now();
        let exe = foreground_exe();
        // Only an answer that came in since the last sample counts: a browser that stops answering
        // (no extension, a PWA window) must not keep the last site alive.
        let tab = take_tab();
        let (site, audible) = if BROWSERS.contains(&exe.as_str()) {
            // The answer lands in TAB for the next sample (a 2 s lag, accepted).
            browsers.ask_all(json!({"t": "active"}));
            tab
        } else {
            Default::default()
        };
        let credits = account(&exe, idle_seconds(), &site, audible, dt);
        let switched = streak.advance(credits.first().map_or("", |c| c.1.as_str()), dt);
        locked(&PENDING).credit(day, hour, &minute, &credits, switched, streak.seconds);
        if now.duration_since(last_flush) >= FLUSH {
            last_flush = now;
            // A locked database must not stop the tracking: the counters stay and this runs again.
            if flush_pending().is_ok()
                && tidied != day
                && let Ok(con) = open(&db_path())
                && backup(&con, &backups_dir(), day).is_ok()
            {
                tidied = day;
                let on = st.read().unwrap_or_else(PoisonError::into_inner).weekly_summary;
                let text = weekly_summary(&con, day, &rules.read().unwrap_or_else(PoisonError::into_inner), on);
                // Sticky: the first flush comes ~30 s after sign-in, when a toast that closes itself is missed.
                if let Some(text) = text {
                    let _ = tx.unbounded_send(win::Msg::sticky(crate::toast::Kind::Info, text));
                }
            }
        }
    }
}

/// The running sampler thread.
pub struct Tracker {
    stop: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

impl Tracker {
    /// `tx` carries the weekly toast; `rules` and `st` are read at each use, so a config reload applies.
    pub fn start(tx: UnboundedSender<win::Msg>, browsers: browser::Browsers, rules: Arc<RwLock<Rules>>, st: Arc<RwLock<ScreenTime>>) -> Tracker {
        let (stop, rx) = mpsc::channel();
        Tracker { stop, thread: std::thread::spawn(move || run(rx, browsers, tx, rules, st)) }
    }

    /// Stop sampling and write what is pending.
    pub fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.thread.join();
        let _ = flush_pending();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn account_rules() {
        assert_eq!(account("code.exe", 0.0, "", false, 2.0), vec![(Kind::App, "code.exe".into(), 2.0)]);
        assert_eq!(account("chrome.exe", 0.0, "youtube.com", false, 2.0).len(), 2);
        assert_eq!(account("code.exe", 0.0, "youtube.com", false, 2.0).len(), 1); // site only in a browser
        assert!(account("code.exe", 500.0, "", false, 2.0).is_empty()); // idle
        assert!(!account("chrome.exe", 500.0, "youtube.com", true, 2.0).is_empty()); // audible beats idle
        assert!(account("chrome.exe", 500.0, "youtube.com", false, 2.0).is_empty());
        assert!(account("code.exe", 500.0, "youtube.com", true, 2.0).is_empty());
        assert!(!account("brave.exe", 500.0, "youtube.com", true, 2.0).is_empty());
        assert!(account("code.exe", 0.0, "", false, 3600.0).is_empty()); // slept
        assert!(account("code.exe", 0.0, "", false, 0.0).is_empty());
        assert!(account("lockapp.exe", 0.0, "", false, 2.0).is_empty());
        assert!(account("", 0.0, "", false, 2.0).is_empty());
    }

    #[test]
    fn classify_rules() {
        let rules: Rules = [
            ("code.exe", "Development"),
            ("youtube.com", "Streaming"),
            ("docs.google.com", "Productivity"),
            ("google.com", "Browsing"),
            ("mail.google.com", "Browsing"),
            ("localhost", "Development"),
            ("com", "Social"), // a bare TLD must never match
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let site = |n| classify(Kind::Site, n, &rules);
        assert_eq!(classify(Kind::App, "Code.exe", &rules), "Development");
        assert_eq!(classify(Kind::App, "mystery.exe", &rules), "Other");
        assert_eq!(site("m.youtube.com"), "Streaming");
        assert_eq!(site("docs.google.com"), "Productivity");
        assert_eq!(site("mail.google.com"), "Browsing");
        assert_eq!(site("localhost"), "Development");
        assert_eq!(site("notyoutube.com"), "Other");
        assert_eq!(site("example.com"), "Other");
    }

    #[test]
    fn streak_counts_switches() {
        let mut s = Streak::default();
        let got: Vec<u32> = ["code", "code", "chrome"].iter().map(|e| s.advance(e, 2.0)).collect();
        assert_eq!(got, [0, 0, 1]);
        assert_eq!((s.exe.as_str(), s.seconds), ("chrome", 6.0));
        assert_eq!(s.advance("", 2.0), 0);
        assert_eq!((s.exe.as_str(), s.seconds), ("", 0.0));
        assert_eq!(s.advance("code", 2.0), 0);
        assert_eq!((s.exe.as_str(), s.seconds), ("code", 2.0));
    }

    #[test]
    fn duration_formats() {
        assert_eq!(duration(0.0), "0m");
        assert_eq!(duration(90.0 * 60.0), "1h 30m");
        assert_eq!(duration(59.0 * 60.0), "59m");
    }

    #[test]
    fn summary_fits_a_toast() {
        assert_eq!(summary_text(129_960, 144_400, "Development"), "Last week 36h 6m, 10% less. Most: Development");
        assert_eq!(summary_text(129_960, 0, "Development"), "Last week 36h 6m. Most: Development");
        assert_eq!(summary_text(3600, 3600, "Gaming"), "Last week 1h 0m, same. Most: Gaming");
        assert!(summary_text(359_940, 1, "Productivity").chars().count() <= 60);
    }

    #[test]
    fn iso_round_trip() {
        assert_eq!(parse_iso("2026-09-30").map(iso).as_deref(), Some("2026-09-30"));
        assert_eq!(parse_iso("2026-13-01"), None);
        assert_eq!(parse_iso("2026-02-30"), None);
        assert_eq!(parse_iso("x"), None);
    }

    fn rules() -> Rules {
        [("code.exe", "Development"), ("chrome.exe", "Browsing"), ("youtube.com", "Streaming")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// App seconds credited on `day`.
    fn seconds_on(con: &Connection, day: i64) -> f64 {
        con.query_row("SELECT COALESCE(SUM(seconds), 0) FROM usage WHERE kind = 'app' AND day = ?", [iso(day)], |r| r.get(0)).unwrap()
    }

    fn mem() -> Connection {
        open(Path::new(":memory:")).unwrap()
    }

    fn insert(con: &Connection, rows: &[(&str, i64, &str, &str, f64)]) {
        for r in rows {
            con.execute("INSERT INTO usage VALUES(?,?,?,?,?)", rusqlite::params![r.0, r.1, r.2, r.3, r.4]).unwrap();
        }
    }

    fn fixture() -> Connection {
        let con = mem();
        insert(
            &con,
            &[
                ("2026-09-30", 9, "app", "code.exe", 600.0),
                ("2026-09-30", 9, "app", "chrome.exe", 300.0),
                ("2026-09-30", 9, "site", "youtube.com", 200.0),
                ("2026-09-29", 9, "app", "code.exe", 100.0),
            ],
        );
        con
    }

    #[test]
    fn stats_carve_sites_out_of_browser() {
        let (con, rules) = (fixture(), rules());
        let d = parse_iso("2026-09-30").unwrap();
        let day = stats(&con, d, d, prior(d, d), &rules).unwrap();
        assert_eq!(day["total"], 900);
        assert_eq!(day["previous"], 100);
        assert_eq!(day["buckets"].as_array().unwrap().len(), 24);
        assert_eq!(day["buckets"][9]["categories"], json!({"Development": 600, "Streaming": 200, "Browsing": 100}));
        assert_eq!(day["apps"][0], json!({"name": "code.exe", "seconds": 600, "category": "Development"}));
        assert_eq!(day["sites"], json!([{"name": "youtube.com", "seconds": 200, "category": "Streaming"}]));
        assert_eq!(day["previous_categories"], json!({"Development": 100}));
        assert_eq!(day["focus"]["days"], 0);
        let mon = parse_iso("2026-09-28").unwrap();
        let week = stats(&con, mon, mon + 6, prior(mon, mon + 6), &rules).unwrap();
        assert_eq!(week["buckets"][0]["label"], "2026-09-28");
        assert_eq!(week["buckets"].as_array().unwrap().len(), 7);
        assert_eq!(week["total"], 1000);
        assert_eq!(week["previous"], 0);
    }

    #[test]
    fn stats_uses_given_previous_span() {
        let con = mem();
        insert(
            &con,
            &[
                ("2026-09-21", 9, "app", "code.exe", 10.0),
                ("2026-09-23", 9, "app", "code.exe", 20.0),
                ("2026-09-24", 9, "app", "code.exe", 400.0), // past the given span
                ("2026-09-28", 9, "app", "code.exe", 100.0),
            ],
        );
        let mon = parse_iso("2026-09-28").unwrap();
        let s = stats(&con, mon, mon + 2, (mon - 7, mon - 5), &rules()).unwrap();
        assert_eq!(s["previous"], 30);
        assert_eq!(s["previous_categories"], json!({"Development": 30}));
        assert_eq!(prior(mon, mon + 2), (mon - 3, mon - 1));
    }

    #[test]
    fn days_and_focus_stats() {
        let con = fixture();
        con.execute("INSERT INTO daily VALUES('2026-09-30', 3, 120.0, '09:00', '17:30')", []).unwrap();
        con.execute("INSERT INTO daily VALUES('2026-09-29', 1, 300.0, '10:00', '11:00')", []).unwrap();
        let (a, b) = (parse_iso("2026-09-29").unwrap(), parse_iso("2026-09-30").unwrap());
        let d = days(&con, a, b, &rules()).unwrap();
        assert_eq!(d, json!({"days": {"2026-09-29": {"Development": 100}, "2026-09-30": {"Development": 600, "Streaming": 200, "Browsing": 100}}}));
        let s = stats(&con, a, b, prior(a, b), &rules()).unwrap();
        assert_eq!(s["focus"], json!({"days": 2, "switches": 4, "longest": 300, "longest_day": "2026-09-29", "first": "09:00", "last": "17:30"}));
    }

    #[test]
    fn csv_escapes_formulas() {
        let con = fixture();
        insert(&con, &[("2026-09-30", 10, "app", "=evil.exe", 5.0), ("2026-09-30", 11, "app", "a,\"b\".exe", 1.0)]);
        let d = parse_iso("2026-09-30").unwrap();
        let out = export_csv(&con, d, d, &rules()).unwrap();
        assert_eq!(out[..3], [0xEF, 0xBB, 0xBF]);
        let text = String::from_utf8(out[3..].to_vec()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "day,hour,kind,name,category,seconds");
        assert!(lines.contains(&"2026-09-30,9,site,youtube.com,Streaming,200"));
        assert!(lines.contains(&"2026-09-30,10,app,'=evil.exe,Other,5"));
        assert!(lines.contains(&"2026-09-30,11,app,\"a,\"\"b\"\".exe\",Other,1"));
    }

    fn usage_seconds(con: &Connection) -> f64 {
        con.query_row("SELECT seconds FROM usage", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn flush_upserts_and_clears() {
        let mut con = mem();
        let credit = [(Kind::App, "code.exe".to_string(), 2.0)];
        let mut p = Pending::default();
        p.credit(20_000, 9, "10:00", &credit, 1, 5.0);
        flush(&mut con, &mut p).unwrap();
        assert!(p.is_empty());
        p.credit(20_000, 9, "09:00", &credit, 1, 1.0);
        flush(&mut con, &mut p).unwrap();
        assert_eq!(usage_seconds(&con), 4.0);
        let got: (u32, f64, String, String) = con
            .query_row("SELECT switches, longest, first, last FROM daily WHERE day = ?", [iso(20_000)], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap();
        assert_eq!(got, (2, 5.0, "09:00".into(), "10:00".into()));
    }

    #[test]
    fn failed_flush_keeps_pending() {
        let mut con = mem();
        let mut p = Pending::default();
        p.credit(20_000, 9, "10:00", &[(Kind::App, "code.exe".into(), 2.0)], 0, 2.0);
        con.pragma_update(None, "query_only", true).unwrap();
        assert!(flush(&mut con, &mut p).is_err());
        assert!(!p.is_empty());
        con.pragma_update(None, "query_only", false).unwrap();
        flush(&mut con, &mut p).unwrap();
        assert_eq!(usage_seconds(&con), 2.0);
        assert!(p.is_empty());
        assert_eq!(seconds_on(&con, 20_000), 2.0);
        assert_eq!(seconds_on(&con, 20_001), 0.0);
    }

    #[test]
    fn pending_splits_days() {
        let mut p = Pending::default();
        let c = [(Kind::App, "code.exe".to_string(), 2.0)];
        p.credit(100, 23, "23:59", &c, 0, 2.0);
        p.credit(101, 0, "00:00", &c, 0, 4.0);
        assert_eq!(p.daily.len(), 2);
        assert_eq!(p.daily[&101].first, "00:00");
        assert_eq!(p.daily[&100].last, "23:59");
        p.credit(101, 0, "00:01", &[], 0, 0.0); // nothing credited: no daily change
        assert_eq!(p.daily[&101].last, "00:00");
    }

    fn cat(name: &str, apps: &[&str], sites: &[&str]) -> crate::config::FocusCategory {
        let own = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
        crate::config::FocusCategory { name: name.into(), apps: own(apps), sites: own(sites) }
    }

    #[test]
    fn rules_first_category_wins() {
        let r = super::rules(&[cat("Social", &[], &["reddit.com"]), cat("Streaming", &[], &["reddit.com", "youtube.com"])]);
        assert_eq!(classify(Kind::Site, "reddit.com", &r), "Social");
        assert_eq!(classify(Kind::Site, "youtube.com", &r), "Streaming");
    }

    #[test]
    fn rules_other_overrides_suffix() {
        let r = super::rules(&[cat("Streaming", &[], &["youtube.com"]), cat("Other", &[], &["m.youtube.com"])]);
        assert_eq!(classify(Kind::Site, "m.youtube.com", &r), "Other");
        assert_eq!(classify(Kind::Site, "www2.youtube.com", &r), "Streaming");
    }

    #[test]
    fn rules_lowercase_names() {
        let r = super::rules(&[cat("Development", &["Code.EXE"], &[])]);
        assert_eq!(classify(Kind::App, "code.exe", &r), "Development");
    }

    #[test]
    fn kinds_by_setting() {
        let st = crate::config::ScreenTime::default();
        assert_eq!(kinds(&st), json!({"Development": "productive", "Productivity": "productive", "Social": "distracting", "Streaming": "distracting", "Gaming": "distracting"}));
    }

    #[test]
    fn backup_keeps_seven() {
        let dir = std::env::temp_dir().join(format!("procast-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let con = fixture();
        let first = parse_iso("2026-09-01").unwrap();
        for d in 0..9 {
            backup(&con, &dir, first + d).unwrap();
        }
        let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names.len(), 7);
        assert_eq!((names[0].as_str(), names[6].as_str()), ("screentime-2026-09-03.db", "screentime-2026-09-09.db"));
        // A second call for the same day leaves the first copy alone.
        let copy = dir.join("screentime-2026-09-09.db");
        let before = std::fs::metadata(&copy).unwrap().modified().unwrap();
        con.execute("INSERT INTO usage VALUES('2026-10-01', 1, 'app', 'x.exe', 1.0)", []).unwrap();
        backup(&con, &dir, first + 8).unwrap();
        assert_eq!(std::fs::metadata(&copy).unwrap().modified().unwrap(), before);
        let copied = open(&copy).unwrap();
        let rows = |c: &Connection| c.query_row("SELECT COUNT(*) FROM usage", [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!((rows(&copied), rows(&con)), (4, 5));
        drop(copied);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn cfg_of(text: &str) -> Vec<FocusCategory> {
        focus::categories(&crate::config::parse(text).unwrap().focus_category)
    }

    fn holders(cats: &[FocusCategory], name: &str) -> Vec<String> {
        cats.iter().filter(|c| c.apps.iter().chain(&c.sites).any(|n| n == name)).map(|c| c.name.clone()).collect()
    }

    #[test]
    fn move_name_rewrites_categories() {
        let text = "# keep me\n";
        let moved = move_name(text, &cfg_of(text), "YouTube.com", "Development").unwrap();
        let cats = cfg_of(&moved);
        assert_eq!(holders(&cats, "youtube.com"), ["Development"]);
        assert!(moved.contains("# keep me"), "comments survive");
        assert!(moved.contains("[[focus_category]]"), "built-ins touched become overrides");
        assert!(focus::category(&cats, "Streaming").unwrap().sites.contains(&"netflix.com".to_string()), "the rest of the category stays");
        // an app moves too, and Other holds explicit entries
        let moved = move_name(&moved, &cats, "spotify.exe", "Other").unwrap();
        let cats = cfg_of(&moved);
        assert_eq!(holders(&cats, "spotify.exe"), ["Other"]);
        assert_eq!(holders(&cats, "youtube.com"), ["Development"], "earlier moves stay");
        // moving again rewrites the existing override instead of adding another
        let moved = move_name(&moved, &cats, "youtube.com", "Streaming").unwrap();
        assert_eq!(holders(&cfg_of(&moved), "youtube.com"), ["Streaming"]);
        assert_eq!(moved.matches("name = \"Streaming\"").count(), 1);
        // without .exe it is a site
        let moved = move_name("", &cfg_of(""), "m.example.com", "Other").unwrap();
        assert!(focus::category(&cfg_of(&moved), "Other").unwrap().sites.contains(&"m.example.com".to_string()));
    }

    #[test]
    fn move_name_rejects_bad_input() {
        let cats = cfg_of("");
        assert_eq!(move_name("", &cats, "x", "Nope"), Err("unknown category".into()));
        for bad in ["", "   ", &"a".repeat(261), "a\nb", "a\u{7}b", "/", "?", "#", ".", "@x", "www.", "a b", "a\"b", "b\u{fc}cher.de", ".exe", " .exe ", &"a".repeat(254)] {
            assert_eq!(move_name("", &cats, bad, "Other"), Err("bad name".into()), "{bad:?}");
        }
    }

    // --- import and weekly summary ---

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("procast-st-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A micro bot `data.db` with two days of usage and one daily row.
    fn micro_bot_db(dir: &Path) -> PathBuf {
        let path = dir.join("data.db");
        let con = open(&path).unwrap();
        insert(&con, &[("2026-09-30", 9, "app", "code.exe", 100.0), ("2026-09-29", 9, "app", "code.exe", 50.0)]);
        con.execute("INSERT INTO daily VALUES('2026-09-30', 2, 90.0, '09:00', '17:00')", []).unwrap();
        path
    }

    fn meta(con: &Connection, key: &str) -> Option<String> {
        con.query_row("SELECT value FROM meta WHERE key = ?", [key], |r| r.get(0)).optional().unwrap()
    }

    #[test]
    fn import_db_adds_rows() {
        let dir = scratch("adds");
        let from = micro_bot_db(&dir);
        let mut con = mem();
        insert(&con, &[("2026-09-30", 9, "app", "code.exe", 10.0)]);
        con.execute("INSERT INTO daily VALUES('2026-09-30', 1, 120.0, '10:00', '18:00')", []).unwrap();
        let stops = Cell::new(0);
        assert_eq!(import_db(&mut con, &from, false, || stops.set(stops.get() + 1)), Ok((2, 2)));
        assert_eq!(stops.get(), 1, "stopped once, after the checks");
        let seconds = |day: &str| con.query_row("SELECT seconds FROM usage WHERE day = ?", [day], |r| r.get::<_, f64>(0)).unwrap();
        assert_eq!((seconds("2026-09-30"), seconds("2026-09-29")), (110.0, 50.0));
        let daily: (u32, f64, String, String) =
            con.query_row("SELECT switches, longest, first, last FROM daily", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!(daily, (3, 120.0, "09:00".into(), "18:00".into()));
        assert_eq!(meta(&con, "imported_from"), Some(from.to_string_lossy().into_owned()));
        // The source is detached again: attaching it a second time works.
        assert!(import_db(&mut con, &from, true, || {}).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_db_keeps_micro_bots_weekly_summary_mark() {
        let dir = scratch("weekmark");
        let from = micro_bot_db(&dir);
        rusqlite::Connection::open(&from).unwrap().execute("INSERT INTO meta VALUES('summary_week', '2026-09-28')", []).unwrap();
        let mut con = mem();
        import_db(&mut con, &from, false, || {}).unwrap();
        assert_eq!(meta(&con, "summary_week").as_deref(), Some("2026-09-28"), "no repeat of the toast micro bot gave");
        // procast's own mark wins
        con.execute("INSERT OR REPLACE INTO meta VALUES('summary_week', '2026-10-05')", []).unwrap();
        import_db(&mut con, &from, true, || {}).unwrap();
        assert_eq!(meta(&con, "summary_week").as_deref(), Some("2026-10-05"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_db_rejects_wrong_file() {
        let dir = scratch("wrong");
        let mut con = mem();
        insert(&con, &[("2026-09-30", 9, "app", "code.exe", 10.0)]);
        let missing = dir.join("data.db");
        let stops = Cell::new(0);
        let stop = || stops.set(stops.get() + 1);
        assert!(import_db(&mut con, &missing, false, stop).unwrap_err().contains("data.db"));
        assert!(!missing.exists(), "a missing file is not created by the attach");
        let other = dir.join("other.db");
        rusqlite::Connection::open(&other).unwrap().execute_batch("CREATE TABLE notes(a)").unwrap();
        assert_eq!(import_db(&mut con, &other, false, stop), Err("not a micro bot database".into()));
        let text = dir.join("text.db");
        std::fs::write(&text, "this is not sqlite at all, just some text to fill a header").unwrap();
        assert_eq!(import_db(&mut con, &text, false, stop), Err("not a micro bot database".into()));
        assert_eq!(stops.get(), 0, "nothing is stopped for a file that is no good");
        assert_eq!((seconds_on(&con, parse_iso("2026-09-30").unwrap()), meta(&con, "imported_from")), (10.0, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_db_second_run_warns() {
        let dir = scratch("twice");
        let from = micro_bot_db(&dir);
        let mut con = mem();
        import_db(&mut con, &from, false, || {}).unwrap();
        let err = import_db(&mut con, &from, false, || panic!("stopped for a refused import")).unwrap_err();
        assert!(err.starts_with("Already imported on 20") && err.len() < 50, "{err}");
        assert_eq!(seconds_on(&con, parse_iso("2026-09-30").unwrap()), 100.0, "the warning changes nothing");
        import_db(&mut con, &from, true, || {}).unwrap();
        assert_eq!(seconds_on(&con, parse_iso("2026-09-30").unwrap()), 200.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_categories_focus_wins() {
        let cats = focus::categories(&[]);
        let json = r#"{"Entertainment":["reddit.com","crunchyroll.com"],"Communication":["slack.com","zoom.us","not a host","/","x.exe"],"Mine":["thing.exe"]}"#;
        let (text, added) = import_categories("# keep me\n", &cats, json).unwrap();
        let got = cfg_of(&text);
        assert_eq!(holders(&got, "reddit.com"), ["Social"], "Focus already files it");
        assert_eq!(holders(&got, "crunchyroll.com"), ["Streaming"]);
        assert_eq!(holders(&got, "slack.com"), ["Messaging"]);
        assert_eq!(holders(&got, "zoom.us"), ["Messaging"]);
        assert_eq!(holders(&got, "x.exe"), ["Messaging"], "an app is told by its .exe");
        assert_eq!(holders(&got, "thing.exe"), ["Mine"], "a category Focus lacks is created");
        assert!(holders(&got, "not a host").is_empty() && holders(&got, "/").is_empty(), "junk is skipped");
        assert_eq!(added, 4);
        assert!(text.contains("# keep me"));
        // a second import finds nothing to add
        assert_eq!(import_categories(&text, &got, json).unwrap().1, 0);
        assert!(import_categories("", &cats, "[1]").is_err());
    }

    #[test]
    fn import_settings_maps_names() {
        let json = r#"{"productive":["Development"],"distracting":["Entertainment","Games"],"weekly_summary":false}"#;
        let text = import_settings("# keep me\n", json).unwrap();
        let st = crate::config::parse(&text).unwrap().screen_time;
        assert_eq!((st.productive, st.distracting, st.weekly_summary, st.enabled), (vec!["Development".to_string()], vec!["Streaming".to_string(), "Gaming".to_string()], false, true));
        assert!(text.contains("# keep me"));
        // what micro bot did not set stays as it was; tracking still turns on
        let st = crate::config::parse(&import_settings("", "{}").unwrap()).unwrap().screen_time;
        assert_eq!((st.enabled, st.weekly_summary, st.productive), (true, true, ScreenTime::default().productive));
        assert!(import_settings("", "nope").is_err());
    }

    #[test]
    fn import_folder_moves_everything_and_changes_nothing_on_failure() {
        let dir = scratch("folder");
        micro_bot_db(&dir);
        std::fs::write(dir.join("categories.json"), r#"{"Entertainment":["crunchyroll.com"]}"#).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"distracting":["Games"]}"#).unwrap();
        let (cfg, db) = (dir.join("config.toml"), dir.join("screentime.db"));
        std::fs::write(&cfg, "# mine\n").unwrap();
        let stops = Cell::new(0);
        let stop = || stops.set(stops.get() + 1);
        let got = import_folder(&dir, &cfg, &db, false, stop).unwrap();
        assert_eq!((got.rows, got.days, got.names_added, stops.get()), (2, 2, 1, 1));
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.contains("# mine") && text.contains("crunchyroll.com") && text.contains("enabled = true"));
        // a bad categories.json fails before the database is touched
        let fresh = dir.join("fresh.db");
        std::fs::write(dir.join("categories.json"), "oops").unwrap();
        assert!(import_folder(&dir, &cfg, &fresh, false, stop).is_err());
        assert!(!fresh.exists());
        // so do a bad settings.json, a typo in config.toml, a database that is no good and an import done before
        std::fs::write(dir.join("categories.json"), "{}").unwrap();
        std::fs::write(dir.join("settings.json"), "oops").unwrap();
        assert!(import_folder(&dir, &cfg, &fresh, false, stop).is_err());
        std::fs::write(dir.join("settings.json"), "{}").unwrap();
        let good = std::fs::read_to_string(&cfg).unwrap();
        std::fs::write(&cfg, "enabled = = oops").unwrap();
        assert!(import_folder(&dir, &cfg, &fresh, false, stop).is_err());
        std::fs::write(&cfg, &good).unwrap();
        assert!(!fresh.exists());
        let err = import_folder(&dir, &cfg, &db, false, stop).err().unwrap();
        assert!(err.starts_with("Already imported"), "{err}");
        std::fs::rename(dir.join("data.db"), dir.join("moved.db")).unwrap();
        std::fs::copy(&cfg, dir.join("data.db")).unwrap();
        assert!(import_folder(&dir, &cfg, &fresh, false, stop).is_err());
        assert_eq!(stops.get(), 1, "micro bot is stopped only once the import can go ahead");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn weekly_summary_once_per_week() {
        let monday = parse_iso("2026-10-05").unwrap();
        assert_eq!(crate::calendar::weekday(monday), 0);
        let con = mem();
        insert(&con, &[("2026-10-01", 9, "app", "code.exe", 3600.0)]);
        let rules = rules();
        assert_eq!(weekly_summary(&con, monday, &rules, true).as_deref(), Some("Last week 1h 0m. Most: Development"));
        assert_eq!(meta(&con, "summary_week").as_deref(), Some("2026-10-05"));
        assert_eq!(weekly_summary(&con, monday + 2, &rules, true), None, "once per week");
        // under a minute: nothing to say, but the week is done
        insert(&con, &[("2026-10-08", 9, "app", "code.exe", 59.0)]);
        assert_eq!(weekly_summary(&con, monday + 7, &rules, true), None);
        assert_eq!(meta(&con, "summary_week").as_deref(), Some("2026-10-12"));
        // switched off: recorded, not shown; and a week later it compares with the one before
        insert(&con, &[("2026-10-15", 9, "app", "code.exe", 7200.0)]);
        assert_eq!(weekly_summary(&con, monday + 14, &rules, false), None);
        assert_eq!(meta(&con, "summary_week").as_deref(), Some("2026-10-19"));
        insert(&con, &[("2026-10-22", 9, "app", "code.exe", 3600.0)]);
        assert_eq!(weekly_summary(&con, monday + 21, &rules, true).as_deref(), Some("Last week 1h 0m, 50% less. Most: Development"));
    }

    #[test]
    fn rows_from_stats() {
        let d = parse_iso("2026-09-30").unwrap();
        let s = stats(&fixture(), d, d, span(Period { range: Range::Day, anchor: d }, d).1, &rules()).unwrap();
        let got: Vec<_> = rows(&s).into_iter().map(|r| (r.section, r.name, r.seconds, r.category)).collect();
        let row = |s, n: &str, secs, c: &str| (s, n.to_string(), secs, c.to_string());
        assert_eq!(
            got,
            [
                row("Categories", "Development", 600, "Development"),
                row("Categories", "Streaming", 200, "Streaming"),
                row("Categories", "Browsing", 100, "Browsing"),
                row("Apps", "code.exe", 600, "Development"),
                row("Apps", "chrome.exe", 300, "Browsing"),
                row("Sites", "youtube.com", 200, "Streaming"),
            ]
        );
        let unsorted = rows(&json!({"categories": [{"name": "A", "seconds": 1}, {"name": "B", "seconds": 5}]}));
        assert_eq!(unsorted.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["B", "A"], "biggest first");
        assert!(rows(&json!({})).is_empty());
    }

    #[test]
    fn header_compares_with_the_span_before() {
        let t = parse_iso("2026-11-26").unwrap();
        let (day, week, month) = (Period { range: Range::Day, anchor: t }, Period { range: Range::Week, anchor: t }, Period { range: Range::Month, anchor: t });
        assert_eq!(header(900, 300, day, t), "15m today · 10m more than yesterday");
        assert_eq!(header(900, 600, day, t), "15m today · 5m more than yesterday");
        assert_eq!(header(600, 900, week, t), "10m this week · 5m less than last week");
        assert_eq!(header(600, 610, day, t), "10m today · same as yesterday");
        assert_eq!(header(600, 0, day, t), "10m today", "nothing before to compare with");
        assert_eq!(header(0, 0, week, t), "0m this week");
        assert_eq!(header(600, 900, month, t), "10m this month · 5m less than last month");
        let past = |range| Period { range, anchor: t - 40 };
        assert_eq!(header(900, 300, past(Range::Day), t), "15m on Sat 17 Oct · 10m more than the day before");
        assert_eq!(header(900, 600, past(Range::Week), t), "15m 12 – 18 Oct · 5m more than the week before");
        assert_eq!(header(600, 900, past(Range::Month), t), "10m in October 2026 · 5m less than the month before");
    }

    #[test]
    fn week_compares_same_weekdays() {
        let day = |s: &str| parse_iso(s).unwrap();
        // Wednesday: Monday to Wednesday against last Monday to Wednesday
        let wk = |today: &str| Period { range: Range::Week, anchor: day(today) };
        let ((from, to), before) = span(wk("2026-09-30"), day("2026-09-30"));
        assert_eq!((iso(from), iso(to)), ("2026-09-28".into(), "2026-10-04".into()), "the whole week is shown, the part so far is compared");
        assert_eq!((iso(before.0), iso(before.1)), ("2026-09-21".into(), "2026-09-23".into()));
        // Monday: one day so far, against last Monday
        let (week, before) = span(wk("2026-09-28"), day("2026-09-28"));
        assert_eq!((week.1 - week.0, before), (6, (week.0 - 7, week.0 - 7)));
        // Sunday: the whole week against the whole week before
        let (week, before) = span(wk("2026-10-04"), day("2026-10-04"));
        assert_eq!((iso(week.0), iso(before.0), iso(before.1)), ("2026-09-28".into(), "2026-09-21".into(), "2026-09-27".into()));
        assert_eq!(span(Period { range: Range::Day, anchor: day("2026-09-30") }, day("2026-09-30")), ((day("2026-09-30"), day("2026-09-30")), (day("2026-09-29"), day("2026-09-29"))));
    }

    #[test]
    fn spans_per_period() {
        let d = |s| parse_iso(s).unwrap();
        let today = d("2026-11-26"); // Thursday
        let p = |range, a| Period { range, anchor: d(a) };
        assert_eq!(span(p(Range::Day, "2026-11-26"), today), ((today, today), (today - 1, today - 1)));
        assert_eq!(span(p(Range::Week, "2026-11-26"), today), ((d("2026-11-23"), d("2026-11-29")), (d("2026-11-16"), d("2026-11-19"))));
        assert_eq!(span(p(Range::Week, "2026-11-12"), today), ((d("2026-11-09"), d("2026-11-15")), (d("2026-11-02"), d("2026-11-08"))));
        assert_eq!(span(p(Range::Month, "2026-11-26"), today), ((d("2026-11-01"), d("2026-11-30")), (d("2026-10-01"), d("2026-10-26"))));
        assert_eq!(span(p(Range::Month, "2026-03-31"), today), ((d("2026-03-01"), d("2026-03-31")), (d("2026-02-01"), d("2026-02-28"))));
    }

    #[test]
    fn steps_and_titles() {
        let d = |s| parse_iso(s).unwrap();
        let today = d("2026-11-26");
        let p = |range, a| Period { range, anchor: d(a) };
        assert_eq!(step(p(Range::Month, "2026-01-31"), 1, today), p(Range::Month, "2026-02-01"));
        assert_eq!(step(p(Range::Month, "2026-01-15"), -1, today), p(Range::Month, "2025-12-01"));
        assert_eq!(step(p(Range::Month, "2025-12-15"), 1, today), p(Range::Month, "2026-01-01"));
        assert_eq!(step(p(Range::Day, "2026-11-26"), 1, today), p(Range::Day, "2026-11-26"), "no future");
        assert_eq!(step(p(Range::Week, "2026-11-26"), 1, today), p(Range::Week, "2026-11-26"));
        assert_eq!(step(p(Range::Week, "2026-11-26"), -1, today), p(Range::Week, "2026-11-19"));
        assert_eq!(with_range(p(Range::Day, "2026-11-19"), Range::Month), p(Range::Month, "2026-11-19"));
        assert_eq!(title(p(Range::Day, "2026-11-26"), today), "Today");
        assert_eq!(title(p(Range::Day, "2026-11-25"), today), "Yesterday");
        assert_eq!(title(p(Range::Day, "2026-11-19"), today), "Thu 19 Nov");
        assert_eq!(title(p(Range::Week, "2026-11-26"), today), "This week");
        assert_eq!(title(p(Range::Week, "2026-11-19"), today), "16 – 22 Nov");
        assert_eq!(title(p(Range::Week, "2026-01-01"), today), "29 Dec – 4 Jan");
        assert_eq!(title(p(Range::Month, "2026-11-02"), today), "This month");
        assert_eq!(title(p(Range::Month, "2026-10-09"), today), "October 2026");
        assert_eq!(caption(p(Range::Day, "2026-11-26"), today), "so far today");
        assert_eq!(caption(p(Range::Week, "2026-11-26"), today), "this week");
        assert_eq!(caption(p(Range::Month, "2026-11-26"), today), "this month");
        assert_eq!(caption(p(Range::Day, "2026-11-19"), today), "on Thu 19 Nov");
        assert_eq!(caption(p(Range::Week, "2026-11-19"), today), "16 – 22 Nov");
        assert_eq!(caption(p(Range::Month, "2026-10-09"), today), "in October 2026");
        let a = p(Range::Day, "2026-11-26");
        assert!(fresh(a, a, 3, 3));
        assert!(!fresh(p(Range::Week, "2026-11-26"), a, 3, 3), "an answer for another period is dropped");
        assert!(!fresh(a, a, 4, 3));
    }

    #[test]
    fn hours_by_name_sums_a_day_per_hour() {
        let con = fixture();
        insert(&con, &[("2026-09-30", 14, "app", "code.exe", 30.5), ("2026-09-30", 14, "site", "code.exe", 7.0)]);
        let hours = hours_by_name(&con, parse_iso("2026-09-30").unwrap()).unwrap();
        let code = hours[&(Kind::App, "code.exe".to_string())];
        assert_eq!((code[9], code[14], code.iter().sum::<f64>()), (600.0, 30.5, 630.5), "other days and kinds stay out");
        assert_eq!(hours[&(Kind::Site, "youtube.com".to_string())][9], 200.0);
        assert_eq!(hours[&(Kind::Site, "code.exe".to_string())][14], 7.0);
        assert!(hours_by_name(&con, parse_iso("2026-09-01").unwrap()).unwrap().is_empty());
    }

    #[test]
    fn detail_for_a_row_and_the_whole_span() {
        let (con, rules) = (fixture(), rules());
        con.execute("INSERT INTO daily VALUES('2026-09-30', 12, 3000.0, '09:05', '17:40')", []).unwrap();
        let d = parse_iso("2026-09-30").unwrap();
        let s = stats(&con, d, d, span(Period { range: Range::Day, anchor: d }, d).1, &rules).unwrap();
        let none_hours = Hours::new();
        let by = |section, name: &str| rows(&s).into_iter().find(|r| r.section == section && r.name == name);
        let text = |v: Vec<(String, String)>| v.into_iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>();
        let focus = ["Switches: 12", "Longest focus: 50m", "First active: 09:05", "Last active: 17:40"];
        let dev = text(detail(&s, by("Categories", "Development").as_ref(), &none_hours));
        assert_eq!(dev[..3], ["Time: 10m", "Share: 67% of total", "09:00: 10m"]);
        assert_eq!(dev[3..], focus);
        let chrome = text(detail(&s, by("Apps", "chrome.exe").as_ref(), &none_hours));
        assert_eq!(chrome[..3], ["Time: 5m", "Share: 33% of total", "Category: Browsing"]);
        assert_eq!(chrome[3..], focus, "no hours read: only the focus numbers follow");
        let hours = hours_by_name(&con, d).unwrap();
        let chrome = text(detail(&s, by("Apps", "chrome.exe").as_ref(), &hours));
        assert_eq!(chrome[..4], ["Time: 5m", "Share: 33% of total", "Category: Browsing", "09:00: 5m"]);
        assert_eq!(chrome[4..], focus);
        let site = text(detail(&s, by("Sites", "youtube.com").as_ref(), &hours));
        assert_eq!(site[..4], ["Time: 3m", "Share: 22% of total", "Category: Streaming", "09:00: 3m"]);
        assert_eq!(text(detail(&s, None, &hours))[..2], ["Total: 15m", "Before: 2m"]);
        // no per-day focus rows: nothing about focus
        let none = stats(&con, d - 5, d - 5, span(Period { range: Range::Day, anchor: d - 5 }, d - 5).1, &rules).unwrap();
        assert_eq!(text(detail(&none, None, &none_hours)), ["Total: 0m", "Before: 0m"]);
        // a week labels its buckets by day and calls the focus times what they are
        let mon = d - 2;
        let w = stats(&con, mon, d, span(Period { range: Range::Week, anchor: d }, d).1, &rules).unwrap();
        let dev = text(detail(&w, rows(&w).iter().find(|r| r.name == "Development"), &none_hours));
        assert!(dev.contains(&"2026-09-30: 10m".to_string()) && dev.contains(&"Earliest start: 09:05".to_string()), "{dev:?}");
    }

    #[test]
    fn comparison_keeps_categories_that_went_quiet() {
        let stats = json!({"categories": [{"name": "Development", "seconds": 120}, {"name": "Reading", "seconds": 60}],
                           "previous_categories": {"Development": 2340, "Messaging": 120, "Reading": 30, "Cooking": 600}});
        let rows = compare_rows(&stats);
        assert_eq!(&rows[..3], &[("Development".to_string(), 120, 2340), ("Productivity".into(), 0, 0), ("Messaging".into(), 0, 120)]);
        assert_eq!(rows.len(), 9, "one row per category, none doubled");
        assert_eq!(rows[7], ("Reading".to_string(), 60, 30), "in both periods");
        assert_eq!(rows[8], ("Cooking".to_string(), 0, 600), "only in the period before");
    }

    #[test]
    fn shares_levels_colours_and_busiest() {
        let d = |s| parse_iso(s).unwrap();
        let days = json!({"days": {"2026-11-16": {"Development": 3000, "Other": 1000}, "2026-11-24": {"Other": 30}}});
        let shares = week_shares(&days, &["Development".into()], d("2026-11-16"), 2);
        assert_eq!(shares, vec![Some(0.75), None]);
        assert_eq!(heat_levels(&days, d("2026-11-16"), d("2026-11-17")), vec![4, 0]);
        let stats = json!({"categories": [{"name": "Development", "seconds": 9}, {"name": "Reading", "seconds": 8}, {"name": "Cooking", "seconds": 7}, {"name": "Other", "seconds": 6}],
                           "buckets": [{"label": 10, "categories": {"Other": 5}}, {"label": 11, "categories": {"Development": 9}}]});
        assert_eq!(color_slot("Development", &stats), Some(0));
        assert_eq!(color_slot("Reading", &stats), Some(7));
        assert_eq!(color_slot("Cooking", &stats), None);
        assert_eq!(color_slot("Other", &stats), None);
        assert_eq!(busiest(&stats), Some(("11 AM".into(), 9)));
    }

    #[test]
    fn busiest_names_days_and_skips_empty() {
        let daily = json!({"buckets": [{"label": "2026-11-18", "categories": {"Other": 5}}, {"label": "2026-11-19", "categories": {"Development": 9}}]});
        assert_eq!(busiest(&daily), Some(("Thu 19".into(), 9)));
        assert_eq!(busiest(&json!({"buckets": [{"label": 0, "categories": {}}]})), None);
    }

    #[test]
    fn a_tab_answer_counts_once() {
        // The only test that touches TAB.
        *TAB.write().unwrap() = Some(Tab { host: "a.com".into(), audible: true });
        assert_eq!(take_tab(), ("a.com".to_string(), true));
        assert_eq!(take_tab(), (String::new(), false), "no new answer: no site");
    }

    #[test]
    fn local_now_shape() {
        let (day, hour, hm) = win::local_now();
        assert!(day > 20_000 && hour < 24);
        assert_eq!((hm.len(), &hm[2..3]), (5, ":"));
        assert_eq!(hm[..2].parse::<u8>().unwrap(), hour);
    }
}
