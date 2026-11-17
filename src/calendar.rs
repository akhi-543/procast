//! My Schedule: events from ICS feeds (Google, Outlook and Apple all publish one). Feeds are
//! fetched every `[calendar] refresh_minutes` off the UI thread; the last good copy is kept in
//! memory. Times are Unix seconds (UTC) here and local time on screen.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::SYSTEMTIME,
    System::Time::{SystemTimeToTzSpecificLocalTime, TzSpecificLocalTimeToSystemTime},
};

use crate::{
    design::{Tint, g},
    search::{Act, Icon, Item},
};

const DAY: i64 = 86400;
/// How soon an event must start to lead root search as Next (and for Join Meeting).
const NEXT_WITHIN: i64 = 15 * 60;

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub start: i64,
    pub end: i64,
    pub title: String,
    /// The meeting link (Teams, Zoom, Meet, Webex) found in the event's text.
    pub join: Option<String>,
    pub all_day: bool,
    /// Which `[[calendar_feed]]` it came from (set by `Calendar::events`).
    pub feed: usize,
    pub location: String,
    /// The ICS UID: one per event, shared by a repeating event's occurrences.
    pub uid: String,
}

// ---- time -----------------------------------------------------------------------------

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Weekday of a day number, Monday = 0 (1970-01-01 was a Thursday).
fn weekday(day: i64) -> i64 {
    (day + 3).rem_euclid(7)
}

fn systime(naive: i64) -> Option<SYSTEMTIME> {
    let (y, m, d) = civil_from_days(naive.div_euclid(DAY));
    let s = naive.rem_euclid(DAY);
    Some(SYSTEMTIME { wYear: u16::try_from(y).ok()?, wMonth: m as u16, wDay: d as u16, wDayOfWeek: 0, wHour: (s / 3600) as u16, wMinute: (s % 3600 / 60) as u16, wSecond: (s % 60) as u16, wMilliseconds: 0 })
}

fn naive_of(t: &SYSTEMTIME) -> i64 {
    days_from_civil(t.wYear.into(), t.wMonth.into(), t.wDay.into()) * DAY + i64::from(t.wHour) * 3600 + i64::from(t.wMinute) * 60 + i64::from(t.wSecond)
}

/// A wall-clock time (seconds as if UTC) in this PC's time zone, as the Unix time it is.
pub fn local_to_utc(naive: i64) -> i64 {
    let Some(local) = systime(naive) else { return naive };
    let mut utc = SYSTEMTIME::default();
    if unsafe { TzSpecificLocalTimeToSystemTime(None, &local, &mut utc) }.is_ok() { naive_of(&utc) } else { naive }
}

/// A Unix time as this PC's wall clock (seconds as if UTC).
pub fn utc_to_local(utc: i64) -> i64 {
    let Some(t) = systime(utc) else { return utc };
    let mut local = SYSTEMTIME::default();
    if unsafe { SystemTimeToTzSpecificLocalTime(None, &t, &mut local) }.is_ok() { naive_of(&local) } else { utc }
}

// ---- ICS ------------------------------------------------------------------------------

/// An ICS date or date-time: `20260115`, `20260115T093000` (local) or `20260115T093000Z`.
#[derive(Clone, Copy)]
struct When {
    /// Seconds as if UTC: the wall-clock reading, or the real UTC time when `utc`.
    naive: i64,
    utc: bool,
    date: bool,
}

impl When {
    fn parse(v: &str, params: &str) -> Option<Self> {
        let v = v.trim();
        let num = |a: usize, b: usize| v.get(a..b)?.parse::<i64>().ok();
        if !v.is_ascii() || !(v.len() == 8 || v.len() >= 15) {
            return None;
        }
        let (y, m, d) = (num(0, 4)?, num(4, 6)?, num(6, 8)?);
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        let date = v.len() == 8;
        let secs = if date { 0 } else { num(9, 11)? * 3600 + num(11, 13)? * 60 + num(13, 15)? };
        // ponytail: a TZID other than UTC is read as this PC's time zone (no zone database here;
        // add VTIMEZONE offsets or an IANA table when an event shows at the wrong hour).
        let tzid = params.to_ascii_uppercase();
        let utc = v.ends_with(['Z', 'z']) || tzid.contains("TZID=UTC") || tzid.contains("TZID=ETC/UTC");
        Some(Self { naive: days_from_civil(y, m, d) * DAY + secs, utc, date })
    }

    fn at(self, naive: i64) -> i64 {
        if self.utc { naive } else { local_to_utc(naive) }
    }

    fn instant(self) -> i64 {
        self.at(self.naive)
    }
}

/// `NAME;PARAM=x:value` -> (NAME, ";PARAM=x", value). Colons inside quotes belong to a parameter.
fn property(line: &str) -> Option<(String, &str, &str)> {
    let mut quoted = false;
    let colon = line.char_indices().find(|&(_, c)| {
        quoted ^= c == '"';
        c == ':' && !quoted
    })?;
    let (head, value) = (&line[..colon.0], &line[colon.0 + 1..]);
    let (name, params) = head.split_once(';').map_or((head, ""), |(n, _)| (n, &head[n.len()..]));
    Some((name.to_ascii_uppercase(), params, value))
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
        } else if let Some(o) = it.next() {
            out.push(if matches!(o, 'n' | 'N') { '\n' } else { o });
        }
    }
    out
}

/// FREQ=DAILY or WEEKLY, with INTERVAL, COUNT, UNTIL and BYDAY.
struct Rule {
    weekly: bool,
    interval: i64,
    count: Option<usize>,
    until: Option<i64>,
    /// Monday = 0.
    byday: Vec<i64>,
}

impl Rule {
    /// None for any other frequency: the event then shows once.
    // ponytail: only DAILY and WEEKLY repeat; MONTHLY and YEARLY need their own stepping, add one when an event shows missing.
    fn parse(s: &str) -> Option<Self> {
        let mut r = Self { weekly: false, interval: 1, count: None, until: None, byday: vec![] };
        let mut freq = false;
        for part in s.split(';') {
            let (k, v) = part.split_once('=')?;
            match k.to_ascii_uppercase().as_str() {
                "FREQ" => match v.to_ascii_uppercase().as_str() {
                    "DAILY" => freq = true,
                    "WEEKLY" => (freq, r.weekly) = (true, true),
                    _ => return None,
                },
                "INTERVAL" => r.interval = v.parse().ok().filter(|n| *n > 0)?,
                "COUNT" => r.count = v.parse().ok(),
                "UNTIL" => r.until = When::parse(v, "").map(|w| w.instant() + if w.date { DAY - 1 } else { 0 }),
                "BYDAY" => {
                    r.byday = v
                        .split(',')
                        .filter_map(|d| ["MO", "TU", "WE", "TH", "FR", "SA", "SU"].iter().position(|n| n.eq_ignore_ascii_case(d.trim_start_matches(|c: char| c.is_ascii_digit() || c == '+' || c == '-'))))
                        .map(|i| i as i64)
                        .collect()
                }
                _ => {}
            }
        }
        freq.then_some(r)
    }

    /// Start times (in `first`'s own reading) up to `limit`, the first one included.
    fn starts(&self, first: When, limit: i64) -> Vec<i64> {
        let (day0, tod) = (first.naive.div_euclid(DAY), first.naive.rem_euclid(DAY));
        let mut out = vec![];
        for k in 0..20_000 {
            let days: Vec<i64> = if self.weekly {
                let monday = day0 - weekday(day0) + k * 7 * self.interval;
                let mut by = if self.byday.is_empty() { vec![weekday(day0)] } else { self.byday.clone() };
                by.sort_unstable();
                by.into_iter().map(|d| monday + d).collect()
            } else {
                let d = day0 + k * self.interval;
                if self.byday.is_empty() || self.byday.contains(&weekday(d)) { vec![d] } else { vec![] }
            };
            for d in days {
                let n = d * DAY + tod;
                if n < first.naive {
                    continue;
                }
                if n > limit || out.len() >= self.count.unwrap_or(usize::MAX) || self.until.is_some_and(|u| first.at(n) > u) {
                    return out;
                }
                out.push(n);
            }
        }
        out
    }
}

#[derive(Default)]
struct Raw {
    uid: String,
    /// The occurrence this event replaces (a moved or cancelled one of a repeating event).
    replaces: Option<i64>,
    start: Option<When>,
    end: Option<When>,
    title: String,
    location: String,
    /// Description, location and conference properties: where a meeting link hides.
    text: String,
    rrule: Option<String>,
    exdates: Vec<i64>,
    cancelled: bool,
}

/// The events overlapping `from..to` (Unix seconds), repeating ones expanded, by start time.
pub fn parse_ics(s: &str, from: i64, to: i64) -> Vec<Event> {
    // Unfold: a line starting with a space or tab continues the one before.
    let mut lines: Vec<String> = vec![];
    for l in s.lines() {
        match (l.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(l.to_string()),
        }
    }
    let (mut raws, mut cur, mut alarm) = (vec![], None::<Raw>, false);
    for l in &lines {
        let Some((name, params, value)) = property(l) else { continue };
        match (name.as_str(), value.trim()) {
            ("BEGIN", "VEVENT") => cur = Some(Raw::default()),
            ("END", "VEVENT") => raws.extend(cur.take()),
            ("BEGIN", "VALARM") => alarm = true,
            ("END", "VALARM") => alarm = false,
            _ if alarm => {}
            _ => {
                let Some(r) = cur.as_mut() else { continue };
                match name.as_str() {
                    "UID" => r.uid = value.trim().to_string(),
                    "RECURRENCE-ID" => r.replaces = When::parse(value, params).map(When::instant),
                    "DTSTART" => r.start = When::parse(value, params),
                    "DTEND" => r.end = When::parse(value, params),
                    "SUMMARY" => r.title = unescape(value).trim().to_string(),
                    "LOCATION" => {
                        r.location = unescape(value).trim().to_string();
                        r.text.push_str(&r.location);
                        r.text.push('\n');
                    }
                    "DESCRIPTION" | "URL" | "X-GOOGLE-CONFERENCE" | "X-MICROSOFT-SKYPETEAMSMEETINGURL" => {
                        r.text.push_str(&unescape(value));
                        r.text.push('\n');
                    }
                    "RRULE" => r.rrule = Some(value.trim().to_string()),
                    "EXDATE" => r.exdates.extend(value.split(',').filter_map(|v| When::parse(v, params)).map(When::instant)),
                    "STATUS" => r.cancelled = value.trim().eq_ignore_ascii_case("CANCELLED"),
                    _ => {}
                }
            }
        }
    }
    // A replaced occurrence is dropped from its repeating event, cancelled or moved.
    let replaced: HashSet<(String, i64)> = raws.iter().filter_map(|r| Some((r.uid.clone(), r.replaces?))).collect();
    let mut out = vec![];
    for r in raws.iter().filter(|r| !r.cancelled) {
        let Some(start) = r.start else { continue };
        let s0 = start.instant();
        let len = match (r.end, start.date) {
            (Some(e), _) => e.instant() - s0,
            (None, true) => DAY,
            (None, false) => 0,
        }
        .max(0);
        let rule = r.rrule.as_deref().and_then(Rule::parse);
        let starts = match &rule {
            Some(rule) => rule.starts(start, to + 2 * DAY),
            None => vec![start.naive],
        };
        for n in starts {
            let s = start.at(n);
            if r.exdates.contains(&s) || (r.replaces.is_none() && replaced.contains(&(r.uid.clone(), s))) {
                continue;
            }
            if s < to && s + len.max(1) > from {
                let title = if r.title.is_empty() { "(No title)".to_string() } else { r.title.clone() };
                out.push(Event { start: s, end: s + len, title, join: join_link(&r.text), all_day: start.date, feed: 0, location: r.location.clone(), uid: r.uid.clone() });
            }
        }
    }
    out.sort_by(|a, b| (a.start, &a.title).cmp(&(b.start, &b.title)));
    out
}

/// An `https://` link's host (lower case) and the rest from its path on. The address ends at
/// the first `/`, `?`, `#` or `\`; a link with a user name, or a port other than 443 or 80, is refused.
fn split_link(link: &str) -> Option<(String, &str)> {
    let rest = link.get(8..).filter(|_| link[..8].eq_ignore_ascii_case("https://"))?;
    let (authority, path) = rest.split_at(rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len()));
    let host = match authority.split_once(':') {
        Some((h, "443" | "80")) => h,
        Some(_) => return None,
        None => authority,
    };
    (!host.is_empty() && !authority.contains('@')).then(|| (host.to_ascii_lowercase(), path))
}

/// Where a meeting link goes, for the "Join <title> on <host>?" question.
pub fn link_host(link: &str) -> Option<String> {
    split_link(link).map(|(host, _)| host)
}

/// The first Teams, Zoom, Meet or Webex meeting link in `text`.
pub fn join_link(text: &str) -> Option<String> {
    // (host, path prefix): the host is the domain itself or a subdomain of it.
    const MEETINGS: [(&str, &str); 7] = [("teams.microsoft.com", "/l/meetup-join"), ("teams.live.com", "/meet"), ("zoom.us", "/j/"), ("zoom.us", "/my/"), ("zoom.com", "/j/"), ("meet.google.com", "/"), ("webex.com", "/")];
    text.split(|c: char| c.is_whitespace() || "<>\"'()[]".contains(c))
        .map(|w| w.trim_end_matches(['.', ',', ';']))
        .find(|w| {
            let Some((host, path)) = split_link(w) else { return false };
            MEETINGS.iter().any(|(h, p)| (host == *h || host.ends_with(&format!(".{h}"))) && path.starts_with(p) && path.len() > 1)
        })
        .map(str::to_string)
}

// ---- state ----------------------------------------------------------------------------

/// The fetched feeds, and what has been announced.
#[derive(Default)]
pub struct Calendar {
    feeds: HashMap<String, Vec<Event>>,
    fetched: Option<Instant>,
    fetching: bool,
    /// When a fetch last brought events (Unix seconds): "Updated 5 min ago" while the next one runs.
    updated: Option<u64>,
    /// Feeds whose last fetch failed and was announced: quiet until one works again.
    failed: HashSet<String>,
    /// Events already reminded of: (start, title).
    fired: HashSet<(i64, String)>,
}

impl Calendar {
    /// Time to fetch: feeds are set up, none is being fetched, and `every` passed. A failed
    /// fetch waits just as long, and keeps the last good events.
    pub fn wants_fetch(&self, feeds: usize, now: Instant, every: Duration) -> bool {
        feeds > 0 && !self.fetching && self.fetched.is_none_or(|t| now.duration_since(t) >= every)
    }

    pub fn fetching(&mut self, now: Instant) {
        self.fetching = true;
        self.fetched = Some(now);
    }

    /// The first fetch has not come back yet.
    pub fn pending(&self) -> bool {
        self.fetching || self.fetched.is_none()
    }

    /// While a fetch runs over events already shown: "Updated 5 min ago", from the last good one.
    pub fn updated_line(&self, now: u64) -> Option<String> {
        self.updated.filter(|_| self.fetching).map(|at| format!("Updated {}", crate::timers::ago(now.saturating_sub(at))))
    }

    /// The feed list changed: forget the old feeds and fetch again at once.
    pub fn reset(&mut self, urls: &[String]) {
        self.feeds.retain(|u, _| urls.contains(u));
        self.failed.retain(|u| urls.contains(u));
        self.fetched = None;
    }

    /// Takes a fetch's results (finished at `now`); returns what to tell the user: each failing feed once.
    pub fn done(&mut self, results: Vec<(String, Result<Vec<Event>, String>)>, now: u64) -> Vec<String> {
        self.fetching = false;
        let mut say = vec![];
        for (url, r) in results {
            match r {
                Ok(events) => {
                    self.failed.remove(&url);
                    self.feeds.insert(url, events);
                    self.updated = Some(now);
                }
                Err(e) => {
                    if self.failed.insert(url) {
                        say.push(format!("Calendar: {e}"));
                    }
                }
            }
        }
        say
    }

    /// Every event of the feeds now configured (`urls`, in `[[calendar_feed]]` order), without
    /// the ignored and the hidden ones, by start time.
    pub fn events(&self, urls: &[String], ignore: &[String], hidden: &HashSet<String>) -> Vec<Event> {
        let mut v: Vec<Event> = urls
            .iter()
            .enumerate()
            .filter_map(|(i, u)| Some((i, self.feeds.get(u)?)))
            .flat_map(|(i, evs)| evs.iter().map(move |e| Event { feed: i, ..e.clone() }))
            .filter(|e| !ignored(&e.title, ignore) && !hidden.contains(&hidden_key(e)))
            .collect();
        v.sort_by_key(|e| e.start);
        v
    }

    /// Reminder texts, "Standup · in 5 min", for events with a meeting link starting within
    /// `minutes`, each event once. 0 turns them off.
    pub fn due(&mut self, events: &[Event], now: i64, minutes: u32) -> Vec<String> {
        self.fired.retain(|(start, _)| *start > now - DAY);
        let window = i64::from(minutes) * 60;
        events
            .iter()
            .filter(|e| window > 0 && !e.all_day && e.join.is_some() && e.start - window <= now && now < e.start)
            .filter(|e| self.fired.insert((e.start, e.title.clone())))
            .map(|e| format!("{} · {}", e.title, in_minutes(e, now)))
            .collect()
    }

    /// The event root search's Next section shows: the meeting a reminder announced, until it
    /// starts (so Enter on Next joins it); else the first one starting within 15 minutes (when
    /// `in_search`).
    pub fn next<'a>(&self, events: &'a [Event], now: i64, in_search: bool) -> Option<&'a Event> {
        let upcoming = || events.iter().filter(|e| !e.all_day && e.start > now);
        upcoming().find(|e| self.fired.contains(&(e.start, e.title.clone()))).or_else(|| upcoming().find(|e| in_search && e.start - now <= NEXT_WITHIN))
    }
}

/// What Join Meeting joins: an event with a link that is on now, else the next one starting
/// within 15 minutes.
pub fn joinable(events: &[Event], now: i64) -> Option<&Event> {
    let linked = || events.iter().filter(|e| !e.all_day && e.join.is_some());
    linked().find(|e| e.start <= now && now < e.end).or_else(|| linked().find(|e| e.start > now && e.start - now <= NEXT_WITHIN))
}

pub fn ignored(title: &str, ignore: &[String]) -> bool {
    let title = title.to_lowercase();
    ignore.iter().any(|w| !w.trim().is_empty() && title.contains(&w.trim().to_lowercase()))
}

/// What Hide This Event stores: the occurrence, by UID and start ("{uid}|{start}"; a repeating
/// event's occurrences share the UID), or by start and title when it has no UID.
pub fn hidden_key(e: &Event) -> String {
    if e.uid.is_empty() { format!("{}|{}", e.start, e.title) } else { format!("{}|{}", e.uid, e.start) }
}

/// The hidden events' keys in `calendar-hidden.json` (a JSON list). A file that cannot be
/// read or parsed hides nothing; only `hide` writes it.
pub fn load_hidden(path: &std::path::Path) -> HashSet<String> {
    read_hidden(path).unwrap_or_default().into_iter().collect()
}

fn read_hidden(path: &std::path::Path) -> Result<Vec<String>, String> {
    let text = crate::focus::text_or_empty(std::fs::read_to_string(path)).map_err(|_| "Could not read calendar-hidden.json".to_string())?;
    if text.trim().is_empty() {
        return Ok(vec![]);
    }
    serde_json::from_str(&text).map_err(|_| "calendar-hidden.json is not a list of events".to_string())
}

/// Adds `key` to the hidden events and writes the file; a file it cannot read is left as it is.
pub fn hide(path: &std::path::Path, key: String) -> Result<HashSet<String>, String> {
    let mut list = read_hidden(path)?;
    if !list.contains(&key) {
        list.push(key);
    }
    let text = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    crate::focus::write_atomic(path, &text).map_err(|e| format!("Could not write calendar-hidden.json: {e}"))?;
    Ok(list.into_iter().collect())
}

/// Everything in `events` overlapping `from..to`.
pub fn within(events: &[Event], from: i64, to: i64) -> Vec<Event> {
    events.iter().filter(|e| e.start < to && e.end.max(e.start + 1) > from).cloned().collect()
}

/// Starts of today and of the day after tomorrow (local), as Unix times: My Schedule's range.
pub fn today_and_tomorrow(now: i64) -> (i64, i64) {
    let midnight = utc_to_local(now).div_euclid(DAY) * DAY;
    (local_to_utc(midnight), local_to_utc(midnight + 2 * DAY))
}

/// "Today" or "Tomorrow" by the event's local start day.
pub fn day_label(start: i64, now: i64) -> &'static str {
    if utc_to_local(start).div_euclid(DAY) <= utc_to_local(now).div_euclid(DAY) { "Today" } else { "Tomorrow" }
}

/// My Schedule's sections: Now (timed events under way), Today and Tomorrow, the empty ones
/// left out. Today's past events are dropped; all-day events come first in their day.
pub fn sections(events: &[Event], now: i64) -> Vec<(&'static str, Vec<Event>)> {
    let (from, to) = today_and_tomorrow(now);
    let mut v = within(events, from, to);
    v.retain(|e| e.end.max(e.start + 1) > now);
    v.sort_by_key(|e| (!e.all_day, e.start));
    let mut out = vec![("Now", vec![]), ("Today", vec![]), ("Tomorrow", vec![])];
    for e in v {
        let i = if !e.all_day && e.start <= now { 0 } else if day_label(e.start, now) == "Today" { 1 } else { 2 };
        out[i].1.push(e);
    }
    out.retain(|(_, v)| !v.is_empty());
    out
}

fn clock(naive: i64) -> String {
    let s = naive.rem_euclid(DAY);
    format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
}

pub fn span(e: &Event) -> String {
    if e.all_day { "All day".into() } else { format!("{} – {}", clock(utc_to_local(e.start)), clock(utc_to_local(e.end))) }
}

/// The accessory of the event on now: "until 11:00".
pub fn until(e: &Event) -> String {
    format!("until {}", clock(utc_to_local(e.end)))
}

/// The Next row's accessory and the reminder's: "in 8 min" (rounded up).
pub fn in_minutes(e: &Event, now: i64) -> String {
    format!("in {} min", (e.start - now + 59).max(60) / 60)
}

/// The meeting service a join link goes to.
pub fn service(join: &str) -> Option<&'static str> {
    const SERVICES: [(&str, &str); 6] = [("meet.google.com", "Google Meet"), ("teams.microsoft.com", "Teams"), ("teams.live.com", "Teams"), ("zoom.us", "Zoom"), ("zoom.com", "Zoom"), ("webex.com", "Webex")];
    let (host, _) = split_link(join)?;
    SERVICES.iter().find(|(h, _)| host == *h || host.ends_with(&format!(".{h}"))).map(|(_, name)| *name)
}

/// A row's subtitle: the calendar's name, then the meeting service: "Work · Google Meet".
pub fn subtitle(feed_name: &str, e: &Event) -> String {
    [feed_name.trim(), e.join.as_deref().and_then(service).unwrap_or("")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// The event's day in its calendar's web page, for a Google or Outlook feed (`feed` is its ICS address).
pub fn calendar_url(feed: &str, e: &Event) -> Option<String> {
    let (host, _) = split_link(&url_of(feed))?;
    let (y, m, d) = civil_from_days(utc_to_local(e.start).div_euclid(DAY));
    match host.as_str() {
        "calendar.google.com" => Some(format!("https://calendar.google.com/calendar/r/day/{y}/{m}/{d}")),
        "outlook.live.com" => Some(format!("https://outlook.live.com/calendar/0/view/day/{y}/{m}/{d}")),
        _ => None,
    }
}

/// Copy Details: title, time, location and link, one per line.
pub fn details(e: &Event) -> String {
    [e.title.as_str(), &span(e), &e.location, e.join.as_deref().unwrap_or("")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
}

/// An event as a row (`feed`: its calendar's name and ICS address). Enter joins its meeting,
/// else opens its day in the calendar's web page, else copies its details; Ctrl+K also copies
/// the link or the details, or hides the event.
pub fn row(e: &Event, feed: (&str, &str), accessory: String) -> Item {
    let mut acts: Vec<(String, Act)> = vec![];
    match (&e.join, calendar_url(feed.1, e)) {
        (Some(url), _) => acts.extend([("Join Meeting".into(), Act::Join(e.title.clone(), url.clone())), ("Copy Join Link".into(), Act::Copy(url.clone()))]),
        (None, Some(page)) => acts.push(("Open in Calendar".into(), Act::Open(page))),
        (None, None) => {}
    }
    acts.push(("Copy Details".into(), Act::Copy(details(e))));
    acts.push(("Hide This Event".into(), Act::HideEvent(hidden_key(e))));
    let sub = subtitle(feed.0, e);
    let glyph = if e.join.is_some() { g::VIDEO } else { g::CALENDAR_CLOCK };
    let mut it = Item::new(e.title.clone(), sub.clone(), Icon::Vector(glyph), acts[0].1.clone()).acc(accessory).tinted(Tint::System).with_hay(format!("{} {sub} {}", e.title, e.location));
    it.extra = acts;
    it.live = true;
    it
}

// ---- fetching -------------------------------------------------------------------------

/// `webcal://` is `https://` under another name.
fn url_of(url: &str) -> String {
    let u = url.trim();
    ["webcals://", "webcal://"]
        .iter()
        .find(|p| u.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p)))
        .map_or_else(|| u.to_string(), |p| format!("https://{}", &u[p.len()..]))
}

/// What a failed fetch says. Only the host goes in: ureq's own text can hold the whole address,
/// and a feed address is a secret (its path is the key).
fn fetch_error(e: &ureq::Error, host: &str) -> String {
    match e {
        ureq::Error::StatusCode(code) => format!("HTTP {code} from {host}"),
        ureq::Error::Timeout(_) => format!("{host} timed out"),
        ureq::Error::BadUri(_) => "that calendar address is not valid".into(),
        _ => format!("could not reach {host}"),
    }
}

/// An http(s) address's host, read right after its own scheme, so it is never a piece of
/// the path or query ("?u=https://token"): a feed address is a secret.
fn feed_host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#', '\\']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (["http", "https"].iter().any(|s| scheme.eq_ignore_ascii_case(s)) && !host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Blocks on the network: call from a worker thread. Keeps the days around now.
pub fn fetch(url: &str) -> Result<Vec<Event>, String> {
    let url = url_of(url);
    let Some(host) = feed_host(&url) else { return Err("that calendar address is not valid".into()) };
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build().into();
    let mut resp = agent.get(&url).call().map_err(|e| fetch_error(&e, &host))?;
    let body = resp.body_mut().read_to_string().map_err(|e| fetch_error(&e, &host))?;
    if !body.contains("BEGIN:VCALENDAR") {
        return Err(format!("{host} did not send an ICS calendar"));
    }
    let now = crate::search::now() as i64;
    Ok(parse_ics(&body, now - DAY, now + 3 * DAY))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAR: i64 = 4_000_000_000;

    fn cal(events: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
    }

    fn utc(y: i64, m: i64, d: i64, h: i64, min: i64) -> i64 {
        days_from_civil(y, m, d) * DAY + h * 3600 + min * 60
    }

    fn ev(start: i64, end: i64, title: &str) -> Event {
        Event { start, end, title: title.into(), join: None, all_day: false, feed: 0, location: String::new(), uid: String::new() }
    }

    /// An event with a Google Meet link.
    fn meet(start: i64, end: i64, title: &str) -> Event {
        Event { join: Some("https://meet.google.com/abc-defg-hij".into()), ..ev(start, end, title) }
    }

    #[test]
    fn a_utc_event_keeps_its_times() {
        let ics = cal("BEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T093000Z\r\nDTEND:20260115T103000Z\r\nSUMMARY:Standup\\, daily\r\nEND:VEVENT\r\n");
        let e = parse_ics(&ics, 0, FAR);
        assert_eq!(e, vec![Event { start: utc(2026, 1, 15, 9, 30), end: utc(2026, 1, 15, 10, 30), title: "Standup, daily".into(), join: None, all_day: false, feed: 0, location: String::new(), uid: "1".into() }]);
        // Outside the window: not listed.
        assert!(parse_ics(&ics, utc(2026, 1, 15, 10, 30), FAR).is_empty());
    }

    #[test]
    fn folded_lines_join_and_alarms_are_ignored() {
        let ics = cal("BEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T093000Z\r\nDTEND:20260115T100000Z\r\nSUMMARY:Quarterly pla\r\n nning review\r\nBEGIN:VALARM\r\nSUMMARY:alarm text\r\nEND:VALARM\r\nEND:VEVENT\r\n");
        assert_eq!(parse_ics(&ics, 0, FAR)[0].title, "Quarterly planning review");
    }

    #[test]
    fn a_weekly_rule_expands_by_day_and_skips_exdates() {
        // Mon 5 Jan 2026 09:00Z, Mondays and Wednesdays, six times, without Wed 7 Jan.
        let ics = cal("BEGIN:VEVENT\r\nUID:w\r\nDTSTART:20260105T090000Z\r\nDTEND:20260105T093000Z\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=6\r\nEXDATE:20260107T090000Z\r\nSUMMARY:Sync\r\nEND:VEVENT\r\n");
        let days: Vec<i64> = parse_ics(&ics, 0, FAR).iter().map(|e| (e.start - utc(2026, 1, 1, 0, 0)) / DAY).collect();
        // Jan 5, 12, 14, 19, 21 (the 7th is gone, COUNT still counted it).
        assert_eq!(days, vec![4, 11, 13, 18, 20]);
    }

    #[test]
    fn rules_stop_at_until_and_step_by_interval() {
        let ics = cal("BEGIN:VEVENT\r\nUID:d\r\nDTSTART:20260105T090000Z\r\nDTEND:20260105T091500Z\r\nRRULE:FREQ=DAILY;INTERVAL=2;UNTIL=20260110T090000Z\r\nSUMMARY:Pill\r\nEND:VEVENT\r\n");
        assert_eq!(parse_ics(&ics, 0, FAR).len(), 3); // 5, 7, 9
        // Window limits a rule with no end.
        let open = cal("BEGIN:VEVENT\r\nUID:o\r\nDTSTART:20260105T090000Z\r\nDTEND:20260105T091500Z\r\nRRULE:FREQ=DAILY\r\nSUMMARY:Open\r\nEND:VEVENT\r\n");
        assert_eq!(parse_ics(&open, utc(2026, 2, 1, 0, 0), utc(2026, 2, 4, 0, 0)).len(), 3);
        // A weekday rule skips the weekend, and an unknown frequency shows once.
        let wk = cal("BEGIN:VEVENT\r\nUID:k\r\nDTSTART:20260105T090000Z\r\nDTEND:20260105T091500Z\r\nRRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR;COUNT=7\r\nSUMMARY:Work\r\nEND:VEVENT\r\n");
        assert_eq!(parse_ics(&wk, 0, FAR).iter().map(|e| weekday(e.start.div_euclid(DAY))).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4, 0, 1]);
        let month = cal("BEGIN:VEVENT\r\nUID:m\r\nDTSTART:20260105T090000Z\r\nDTEND:20260105T091500Z\r\nRRULE:FREQ=MONTHLY;COUNT=5\r\nSUMMARY:Rent\r\nEND:VEVENT\r\n");
        assert_eq!(parse_ics(&month, 0, FAR).len(), 1);
    }

    #[test]
    fn a_moved_or_cancelled_occurrence_replaces_the_original() {
        let ics = cal(
            "BEGIN:VEVENT\r\nUID:r\r\nDTSTART:20260105T090000Z\r\nDTEND:20260105T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Daily\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:r\r\nRECURRENCE-ID:20260106T090000Z\r\nDTSTART:20260106T140000Z\r\nDTEND:20260106T150000Z\r\nSUMMARY:Daily (moved)\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:r\r\nRECURRENCE-ID:20260107T090000Z\r\nDTSTART:20260107T090000Z\r\nSTATUS:CANCELLED\r\nSUMMARY:Daily\r\nEND:VEVENT\r\n",
        );
        let e: Vec<(i64, String)> = parse_ics(&ics, 0, FAR).into_iter().map(|e| (e.start, e.title)).collect();
        assert_eq!(e, vec![(utc(2026, 1, 5, 9, 0), "Daily".into()), (utc(2026, 1, 6, 14, 0), "Daily (moved)".into())]);
    }

    #[test]
    fn an_all_day_event_runs_from_local_midnight() {
        let ics = cal("BEGIN:VEVENT\r\nUID:a\r\nDTSTART;VALUE=DATE:20260120\r\nDTEND;VALUE=DATE:20260121\r\nSUMMARY:Holiday\r\nEND:VEVENT\r\n");
        let e = &parse_ics(&ics, 0, FAR)[0];
        assert!(e.all_day);
        assert_eq!((e.start, e.end), (local_to_utc(days_from_civil(2026, 1, 20) * DAY), local_to_utc(days_from_civil(2026, 1, 21) * DAY)));
        assert_eq!(span(e), "All day");
        // No DTEND: a day long.
        let one = cal("BEGIN:VEVENT\r\nUID:b\r\nDTSTART;VALUE=DATE:20260120\r\nSUMMARY:Day\r\nEND:VEVENT\r\n");
        assert_eq!(parse_ics(&one, 0, FAR)[0].end, e.end);
    }

    #[test]
    fn a_floating_time_is_local_and_the_clock_text_follows() {
        let ics = cal("BEGIN:VEVENT\r\nUID:l\r\nDTSTART;TZID=Europe/Berlin:20260115T093000\r\nDTEND;TZID=Europe/Berlin:20260115T103000\r\nSUMMARY:Local\r\nEND:VEVENT\r\n");
        let e = &parse_ics(&ics, 0, FAR)[0];
        assert_eq!(span(e), "09:30 – 10:30");
        assert_eq!(utc_to_local(local_to_utc(utc(2026, 1, 15, 12, 0))), utc(2026, 1, 15, 12, 0));
    }

    #[test]
    fn the_meeting_link_comes_from_the_description() {
        let teams = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=%7b%22Tid%22%7d";
        let ics = cal(&format!("BEGIN:VEVENT\r\nUID:t\r\nDTSTART:20260115T093000Z\r\nDTEND:20260115T100000Z\r\nSUMMARY:Planning\r\nDESCRIPTION:Join now\\n<{teams}>\\nMeeting ID: 1\r\nEND:VEVENT\r\n"));
        assert_eq!(parse_ics(&ics, 0, FAR)[0].join.as_deref(), Some(teams));
        let none = cal("BEGIN:VEVENT\r\nUID:n\r\nDTSTART:20260115T093000Z\r\nDTEND:20260115T100000Z\r\nSUMMARY:Lunch\r\nEND:VEVENT\r\n");
        assert_eq!(parse_ics(&none, 0, FAR)[0].join, None);
    }

    #[test]
    fn join_links_cover_four_services_and_refuse_lookalikes() {
        for url in ["https://us02web.zoom.us/j/123456789?pwd=abc", "https://zoom.us/my/someone", "https://meet.google.com/abc-defg-hij", "https://acme.webex.com/meet/jo", "https://teams.live.com/meet/9876"] {
            assert_eq!(join_link(&format!("Dial in: {url}.")).as_deref(), Some(url.trim_end_matches('.')), "{url}");
        }
        assert_eq!(join_link("see https://example.com/?u=zoom.us/j/1"), None);
        assert_eq!(join_link("https://notzoom.us/j/1"), None);
        // The address ends at / ? # or \, whatever comes after it.
        for bad in [
            "https://evil.com?.zoom.us/j/1",
            "https://evil.com#.zoom.us/j/1",
            "https://evil.com\\.zoom.us/j/1",
            "https://evil.com?.teams.microsoft.com/l/meetup-join/x",
            "https://zoom.us@evil.com/j/1",
            "https://zoom.us:pw@evil.com/j/1",
            "https://zoom.us:8443/j/1",
            "https://zoom.us.evil.com/j/1",
            "http://zoom.us/j/1",
            "ftp://zoom.us/j/1",
            "https://@zoom.us/j/1",
        ] {
            assert_eq!(join_link(bad), None, "{bad}");
        }
        assert_eq!(join_link("https://ZOOM.us/j/1").as_deref(), Some("https://ZOOM.us/j/1"));
        assert_eq!(join_link("HTTPS://Acme.Zoom.US:443/j/1").as_deref(), Some("HTTPS://Acme.Zoom.US:443/j/1"));
        assert_eq!(link_host("HTTPS://Acme.Zoom.US:443/j/1").as_deref(), Some("acme.zoom.us"));
        assert_eq!(join_link("https://meet.google.com/"), None);
        assert_eq!(join_link("no link"), None);
    }

    #[test]
    fn ignored_and_hidden_events_are_skipped() {
        let mut c = Calendar::default();
        c.done(vec![("u".into(), Ok(vec![ev(10, 20, "Focus Time"), ev(30, 40, "Review")]))], 0);
        let urls = ["u".to_string()];
        let none = HashSet::new();
        assert_eq!(c.events(&urls, &[], &none).len(), 2);
        let left = c.events(&urls, &["focus".into(), " ".into()], &none);
        assert_eq!(left.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), vec!["Review"]);
        // Feeds dropped from the config do not show.
        assert!(c.events(&[], &[], &none).is_empty());
        // Hide This Event: by UID, or by start and title without one.
        let hidden: HashSet<String> = [hidden_key(&ev(30, 40, "Review"))].into();
        assert_eq!(hidden_key(&ev(30, 40, "Review")), "30|Review");
        assert_eq!(c.events(&urls, &[], &hidden).iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), vec!["Focus Time"]);
        assert_eq!(hidden_key(&Event { uid: "abc@google.com".into(), ..ev(1, 2, "X") }), "abc@google.com|1");
        // Hiding one week of a weekly 1:1 leaves the next week.
        let ics = cal("BEGIN:VEVENT\r\nUID:w@google.com\r\nDTSTART:20260105T090000Z\r\nDTEND:20260105T093000Z\r\nRRULE:FREQ=WEEKLY;COUNT=2\r\nSUMMARY:1:1\r\nEND:VEVENT\r\n");
        let weeks = parse_ics(&ics, 0, FAR);
        let mut w = Calendar::default();
        w.done(vec![("w".into(), Ok(weeks.clone()))], 0);
        let hidden: HashSet<String> = [hidden_key(&weeks[0])].into();
        let left: Vec<i64> = w.events(&["w".into()], &[], &hidden).iter().map(|e| e.start).collect();
        assert_eq!(left, vec![weeks[1].start]);
        // Each event knows its feed's place in the list.
        c.done(vec![("v".into(), Ok(vec![ev(50, 60, "Gym")]))], 0);
        let feeds: Vec<usize> = c.events(&["u".into(), "v".into()], &[], &none).iter().map(|e| e.feed).collect();
        assert_eq!(feeds, vec![0, 0, 1]);
    }

    #[test]
    fn the_hidden_store_adds_keys_and_never_overwrites_a_bad_file() {
        let dir = std::env::temp_dir().join(format!("procast-cal-hidden-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("calendar-hidden.json");
        let _ = std::fs::remove_file(&path);
        assert!(load_hidden(&path).is_empty());
        assert_eq!(hide(&path, "a".into()).unwrap().len(), 1);
        assert_eq!(hide(&path, "a".into()).unwrap().len(), 1, "once");
        hide(&path, "b".into()).unwrap();
        assert_eq!(load_hidden(&path), ["a".to_string(), "b".to_string()].into());
        std::fs::write(&path, "not json").unwrap();
        assert!(load_hidden(&path).is_empty());
        assert!(hide(&path, "c".into()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reminders_come_once_for_meetings_with_a_link() {
        let mut c = Calendar::default();
        let events = [meet(1000, 2000, "Standup"), meet(5000, 6000, "Later"), ev(1000, 2000, "No link")];
        assert!(c.due(&events, 600, 5).is_empty()); // 400 s away: more than 5 min
        assert_eq!(c.due(&events, 710, 5), vec!["Standup · in 5 min".to_string()]); // 290 s -> 5 min
        assert!(c.due(&events, 720, 5).is_empty()); // already told
        assert!(c.due(&events, 1000, 5).is_empty()); // started
        assert!(Calendar::default().due(&events, 900, 0).is_empty()); // off
        let all_day = [Event { all_day: true, ..meet(1000, 2000, "Holiday") }];
        assert!(Calendar::default().due(&all_day, 900, 5).is_empty());
    }

    #[test]
    fn next_is_the_meeting_within_15_minutes_or_one_reminded_of() {
        let events = [ev(0, 500, "On now"), ev(2000, 3000, "Soon"), ev(4000, 5000, "Later")];
        let c = Calendar::default();
        assert_eq!(c.next(&events, 2000 - 900, true).map(|e| e.title.as_str()), Some("Soon"));
        assert_eq!(c.next(&events, 2000 - 901, true), None);
        assert_eq!(c.next(&events, 2000 - 60, false), None, "next_in_search off");
        assert_eq!(c.next(&events, 2000, true), None, "started");
        // A reminder 30 minutes ahead puts it in Next at once, even with next_in_search off.
        let mut c = Calendar::default();
        let later = [meet(4000, 5000, "Planning")];
        assert_eq!(c.due(&later, 4000 - 1800, 30).len(), 1);
        assert_eq!(c.next(&later, 4000 - 1800, false).map(|e| e.title.as_str()), Some("Planning"));
        // The reminded meeting is Next even when an event without a link starts sooner.
        let mut c = Calendar::default();
        let both = [ev(1180, 2000, "No link"), meet(1300, 2000, "Standup")];
        assert_eq!(c.due(&both, 1000, 5).len(), 1);
        assert_eq!(c.next(&both, 1000, true).map(|e| e.title.as_str()), Some("Standup"));
        // Without a reminder, the sooner one.
        assert_eq!(Calendar::default().next(&both, 1000, true).map(|e| e.title.as_str()), Some("No link"));
    }

    #[test]
    fn join_meeting_takes_the_meeting_on_now_then_the_next_one() {
        let events = [ev(0, 1000, "No link"), meet(100, 1000, "On now"), meet(1500, 2000, "Next")];
        assert_eq!(joinable(&events, 500).map(|e| e.title.as_str()), Some("On now"));
        assert_eq!(joinable(&events, 1000).map(|e| e.title.as_str()), Some("Next"));
        assert_eq!(joinable(&events[2..], 1500 - 901), None, "more than 15 min away");
        assert_eq!(joinable(&[ev(0, 1000, "No link")], 500), None);
    }

    #[test]
    fn sections_drop_the_past_and_put_all_day_first() {
        let now = utc(2026, 1, 15, 12, 0);
        let (today, _) = today_and_tomorrow(now);
        let tomorrow = today + DAY;
        let all_day = |start: i64, title: &str| Event { all_day: true, ..ev(start, start + DAY, title) };
        let events = [
            ev(today + 3600, today + 7200, "Early"),
            ev(now - 600, now + 600, "Standup"),
            ev(now + 3600, now + 5400, "1:1"),
            all_day(today, "Holiday"),
            ev(tomorrow + 9 * 3600, tomorrow + 10 * 3600, "Review"),
            all_day(tomorrow, "Trip"),
        ];
        let secs = sections(&events, now);
        let got: Vec<(&str, Vec<&str>)> = secs.iter().map(|(name, v)| (*name, v.iter().map(|e| e.title.as_str()).collect())).collect();
        // "Early" ended before now; counts are each section's length.
        assert_eq!(got, vec![("Now", vec!["Standup"]), ("Today", vec!["Holiday", "1:1"]), ("Tomorrow", vec!["Trip", "Review"])]);
        assert!(sections(&[ev(today + 60, today + 120, "Gone")], now).is_empty());
    }

    #[test]
    fn until_and_in_minutes_read_as_the_card_does() {
        let now = utc(2026, 1, 15, 12, 0);
        let e = ev(now - 600, local_to_utc(utc(2026, 1, 15, 11, 0)), "Design review");
        assert_eq!(until(&e), "until 11:00");
        let soon = ev(now + 8 * 60, now + 3600, "Sync");
        assert_eq!(in_minutes(&soon, now), "in 8 min");
        assert_eq!(in_minutes(&soon, now + 7 * 60 + 30), "in 1 min");
        assert_eq!(in_minutes(&soon, now + 8 * 60 - 1), "in 1 min");
    }

    #[test]
    fn service_names_each_meeting_host() {
        for (link, name) in [
            ("https://meet.google.com/abc-defg-hij", "Google Meet"),
            ("https://teams.microsoft.com/l/meetup-join/x", "Teams"),
            ("https://teams.live.com/meet/9876", "Teams"),
            ("https://us02web.zoom.us/j/1", "Zoom"),
            ("https://acme.webex.com/meet/jo", "Webex"),
        ] {
            assert_eq!(service(link), Some(name), "{link}");
        }
        assert_eq!(service("https://zoom.us.evil.com/j/1"), None);
        assert_eq!(service("https://example.com/"), None);
    }

    #[test]
    fn the_subtitle_is_the_calendar_then_the_service() {
        assert_eq!(subtitle("Work", &meet(0, 1, "Design review")), "Work · Google Meet");
        assert_eq!(subtitle("Work", &ev(0, 1, "1:1")), "Work");
        assert_eq!(subtitle("", &meet(0, 1, "x")), "Google Meet");
    }

    #[test]
    fn calendar_url_opens_the_day_in_google_or_outlook() {
        let e = ev(local_to_utc(utc(2026, 11, 17, 14, 0)), local_to_utc(utc(2026, 11, 17, 15, 0)), "Gym");
        let google = "https://calendar.google.com/calendar/ical/me%40gmail.com/private-abc/basic.ics";
        assert_eq!(calendar_url(google, &e).as_deref(), Some("https://calendar.google.com/calendar/r/day/2026/11/17"));
        let outlook = "webcal://outlook.live.com/owa/calendar/00000/abc/calendar.ics";
        assert_eq!(calendar_url(outlook, &e).as_deref(), Some("https://outlook.live.com/calendar/0/view/day/2026/11/17"));
        assert_eq!(calendar_url("https://cal.example.com/a.ics", &e), None);
        assert_eq!(calendar_url("https://evil.com?.calendar.google.com/a.ics", &e), None);
    }

    #[test]
    fn a_row_joins_else_opens_the_calendar_else_copies() {
        let google = "https://calendar.google.com/calendar/ical/x/basic.ics";
        let labels = |it: &Item| it.actions().into_iter().map(|(l, _)| l).collect::<Vec<_>>();
        let m = Event { location: "Room 4".into(), ..meet(local_to_utc(utc(2026, 1, 15, 10, 0)), local_to_utc(utc(2026, 1, 15, 11, 0)), "Design review") };
        let it = row(&m, ("Work", google), "14:00 – 14:30".into());
        assert_eq!(labels(&it), ["Join Meeting", "Copy Join Link", "Copy Details", "Hide This Event"]);
        assert!(matches!(&it.act, Act::Join(t, u) if t == "Design review" && u == "https://meet.google.com/abc-defg-hij"));
        assert_eq!((it.subtitle.as_str(), it.accessory.as_str(), it.live), ("Work · Google Meet", "14:00 – 14:30", true));
        assert!(matches!(it.icon, Icon::Vector(g::VIDEO)));
        assert_eq!(details(&m), "Design review\n10:00 – 11:00\nRoom 4\nhttps://meet.google.com/abc-defg-hij");
        let plain = ev(m.start, m.end, "Gym");
        let it = row(&plain, ("Personal", google), String::new());
        assert_eq!(labels(&it), ["Open in Calendar", "Copy Details", "Hide This Event"]);
        assert!(matches!(it.icon, Icon::Vector(g::CALENDAR_CLOCK)));
        let it = row(&plain, ("Personal", "https://cal.example.com/a.ics"), String::new());
        assert_eq!(labels(&it), ["Copy Details", "Hide This Event"]);
        assert_eq!(it.act, Act::Copy("Gym\n10:00 – 11:00".into()));
    }

    #[test]
    fn the_location_is_kept_and_its_link_still_joins() {
        let ics = cal("BEGIN:VEVENT\r\nUID:x@google.com\r\nDTSTART:20260115T093000Z\r\nDTEND:20260115T100000Z\r\nSUMMARY:Sync\r\nLOCATION:https://zoom.us/j/123\r\nEND:VEVENT\r\n");
        let e = &parse_ics(&ics, 0, FAR)[0];
        assert_eq!((e.location.as_str(), e.join.as_deref(), e.uid.as_str()), ("https://zoom.us/j/123", Some("https://zoom.us/j/123"), "x@google.com"));
    }

    #[test]
    fn today_and_tomorrow_are_two_local_days() {
        let now = utc(2026, 1, 15, 12, 0);
        let (from, to) = today_and_tomorrow(now);
        assert!(from <= now && now < from + DAY + 3600 && to - from >= 2 * DAY - 3600 && to - from <= 2 * DAY + 3600);
        assert_eq!((day_label(now, now), day_label(from + DAY + 7200, now)), ("Today", "Tomorrow"));
        assert_eq!(within(&[ev(from - 10, from - 1, "Yesterday"), ev(from + 5, from + 10, "Today"), ev(to, to + 5, "Later")], from, to).len(), 1);
    }

    #[test]
    fn a_failing_feed_is_announced_once_and_keeps_its_events() {
        const EVERY: Duration = Duration::from_secs(900);
        let mut c = Calendar::default();
        let t = Instant::now();
        assert!(c.wants_fetch(1, t, EVERY) && !c.wants_fetch(0, t, EVERY));
        c.fetching(t);
        assert!(!c.wants_fetch(1, t + EVERY * 2, EVERY), "one fetch at a time");
        let url = "https://cal.example.com/private/secret.ics".to_string();
        assert!(c.done(vec![(url.clone(), Ok(vec![ev(1, 2, "A")]))], 1000).is_empty());
        assert!(!c.wants_fetch(1, t + EVERY - Duration::from_secs(1), EVERY) && c.wants_fetch(1, t + EVERY, EVERY));
        let say = c.done(vec![(url.clone(), Err("http status: 500".into()))], 2000);
        assert_eq!(say, vec!["Calendar: http status: 500".to_string()]);
        assert!(!say[0].contains("secret"));
        assert!(c.done(vec![(url.clone(), Err("again".into()))], 2000).is_empty());
        assert_eq!(c.events(std::slice::from_ref(&url), &[], &HashSet::new()).len(), 1);
        c.done(vec![(url.clone(), Ok(vec![]))], 2000);
        assert_eq!(c.done(vec![(url, Err("down".into()))], 2000).len(), 1, "told again after a good fetch");
    }

    #[test]
    fn while_a_fetch_runs_the_bar_says_when_the_last_one_came() {
        let mut c = Calendar::default();
        assert_eq!(c.updated_line(1000), None);
        c.fetching(Instant::now());
        assert_eq!(c.updated_line(1000), None, "nothing fetched yet");
        c.done(vec![("u".into(), Ok(vec![]))], 1000);
        assert_eq!(c.updated_line(1300), None, "not loading");
        c.fetching(Instant::now());
        assert_eq!(c.updated_line(1300).as_deref(), Some("Updated 5 min ago"));
        // A failed fetch does not count as an update.
        c.done(vec![("u".into(), Err("down".into()))], 1300);
        c.fetching(Instant::now());
        assert_eq!(c.updated_line(1600).as_deref(), Some("Updated 10 min ago"));
    }

    #[test]
    fn fetch_errors_never_repeat_the_address() {
        // ureq's own text for an address without a host holds the whole address.
        let e = ureq::Agent::config_builder().build().new_agent().get("/private/token.ics").call().unwrap_err();
        assert!(matches!(e, ureq::Error::BadUri(_)) && e.to_string().contains("token"), "{e}");
        assert_eq!(fetch_error(&e, "cal.example.com"), "that calendar address is not valid");
        assert_eq!(fetch_error(&ureq::Error::StatusCode(404), "cal.example.com"), "HTTP 404 from cal.example.com");
        assert_eq!(fetch_error(&ureq::Error::HostNotFound, "cal.example.com"), "could not reach cal.example.com");
        assert_eq!(fetch_error(&ureq::Error::Timeout(ureq::Timeout::Global), "cal.example.com"), "cal.example.com timed out");
        // And the entries that never reach ureq.
        for bad in ["/private/token.ics", "token.ics", "ftp://x.com/token.ics", ""] {
            assert_eq!(fetch(bad), Err("that calendar address is not valid".to_string()), "{bad}");
        }
    }

    #[test]
    fn the_host_in_an_error_never_comes_from_the_path() {
        assert_eq!(feed_host("https://cal.example.com/private/a.ics?u=https://token.example/x").as_deref(), Some("cal.example.com"));
        assert_eq!(feed_host("HTTP://me:pw@Localhost:8000/a.ics").as_deref(), Some("localhost"));
        assert_eq!(feed_host("https://x.com\\a@token/b").as_deref(), Some("x.com"));
        for bad in ["ftp://x.com/a.ics", "/private/token.ics", "https:///token.ics", ""] {
            assert_eq!(feed_host(bad), None, "{bad}");
        }
    }

    #[test]
    fn webcal_addresses_are_fetched_over_https() {
        assert_eq!(url_of("webcal://cal.example.com/a.ics"), "https://cal.example.com/a.ics");
        assert_eq!(url_of(" WEBCAL://x.com/a.ics "), "https://x.com/a.ics");
        assert_eq!(url_of("https://x.com/a.ics"), "https://x.com/a.ics");
    }

    #[test]
    fn odd_input_does_not_panic() {
        for s in ["", "BEGIN:VEVENT", "DTSTART:é", "BEGIN:VEVENT\r\nDTSTART:2026ééé\r\nEND:VEVENT", "BEGIN:VEVENT\r\nDTSTART:20261345T000000\r\nEND:VEVENT", "BEGIN:VEVENT\r\nSUMMARY:x\\", ":::", "BEGIN:VEVENT\r\nDTSTART:20260105T090000Z\r\nRRULE:FREQ=WEEKLY;INTERVAL=0\r\nEND:VEVENT"] {
            let _ = parse_ics(s, 0, FAR);
            let _ = join_link(s);
        }
    }
}
