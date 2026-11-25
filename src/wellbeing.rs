// ponytail: removed once the sampler (Task 3) uses everything
#![allow(dead_code)]
//! Screen-time accounting: what a sample is worth, categories, and the weekly summary text.

use crate::calendar::{civil_from_days, days_from_civil};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// A sample later than this means the machine slept; discard it.
const GAP_S: f64 = 10.0;
/// No input for this long = away.
const IDLE_S: f64 = 120.0;
const BROWSERS: [&str; 5] = ["chrome.exe", "msedge.exe", "brave.exe", "opera.exe", "vivaldi.exe"];
const IGNORED: [&str; 2] = ["", "lockapp.exe"];

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

/// App seconds credited on `day`; 0 if the database can't be read.
pub fn seconds_on(con: &Connection, day: i64) -> f64 {
    con.query_row("SELECT COALESCE(SUM(seconds), 0) FROM usage WHERE kind = 'app' AND day = ?", [iso(day)], |r| r.get(0)).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
