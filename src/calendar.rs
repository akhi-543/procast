//! My Schedule: events from ICS feeds (Google, Outlook and Apple all publish one). Feeds are
//! fetched every 10 minutes off the UI thread; the last good copy is kept in memory. Times are
//! Unix seconds (UTC) here and local time on screen.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::SYSTEMTIME,
    System::Time::{SystemTimeToTzSpecificLocalTime, TzSpecificLocalTimeToSystemTime},
};

use crate::{
    design::g,
    search::{Act, Icon, Item},
};

const REFRESH: Duration = Duration::from_secs(600);
const DAY: i64 = 86400;

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub start: i64,
    pub end: i64,
    pub title: String,
    /// The meeting link (Teams, Zoom, Meet, Webex) found in the event's text.
    pub join: Option<String>,
    pub all_day: bool,
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
                    "DESCRIPTION" | "LOCATION" | "URL" | "X-GOOGLE-CONFERENCE" | "X-MICROSOFT-SKYPETEAMSMEETINGURL" => {
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
                out.push(Event { start: s, end: s + len, title, join: join_link(&r.text), all_day: start.date });
            }
        }
    }
    out.sort_by(|a, b| (a.start, &a.title).cmp(&(b.start, &b.title)));
    out
}

/// The first Teams, Zoom, Meet or Webex meeting link in `text`.
pub fn join_link(text: &str) -> Option<String> {
    // (host, path prefix): the host is the domain itself or a subdomain of it.
    const MEETINGS: [(&str, &str); 7] = [("teams.microsoft.com", "/l/meetup-join"), ("teams.live.com", "/meet"), ("zoom.us", "/j/"), ("zoom.us", "/my/"), ("zoom.com", "/j/"), ("meet.google.com", "/"), ("webex.com", "/")];
    text.split(|c: char| c.is_whitespace() || "<>\"'()[]".contains(c))
        .map(|w| w.trim_end_matches(['.', ',', ';']))
        .find(|w| {
            let Some(rest) = w.strip_prefix("https://").or_else(|| w.strip_prefix("http://")) else { return false };
            let (host, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
            let host = host.to_ascii_lowercase();
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
    /// Feeds whose last fetch failed and was announced: quiet until one works again.
    failed: HashSet<String>,
    /// Events already reminded of: (start, title).
    fired: HashSet<(i64, String)>,
}

impl Calendar {
    /// Time to fetch: feeds are set up, none is being fetched, and 10 minutes passed. A failed
    /// fetch waits just as long, and keeps the last good events.
    pub fn wants_fetch(&self, feeds: usize, now: Instant) -> bool {
        feeds > 0 && !self.fetching && self.fetched.is_none_or(|t| now.duration_since(t) >= REFRESH)
    }

    pub fn fetching(&mut self, now: Instant) {
        self.fetching = true;
        self.fetched = Some(now);
    }

    /// The first fetch has not come back yet.
    pub fn pending(&self) -> bool {
        self.fetching || self.fetched.is_none()
    }

    /// The feed list changed: forget the old feeds and fetch again at once.
    pub fn reset(&mut self, urls: &[String]) {
        self.feeds.retain(|u, _| urls.contains(u));
        self.failed.retain(|u| urls.contains(u));
        self.fetched = None;
    }

    /// Takes a fetch's results; returns what to tell the user: each failing feed once.
    pub fn done(&mut self, results: Vec<(String, Result<Vec<Event>, String>)>) -> Vec<String> {
        self.fetching = false;
        let mut say = vec![];
        for (url, r) in results {
            match r {
                Ok(events) => {
                    self.failed.remove(&url);
                    self.feeds.insert(url, events);
                }
                Err(e) => {
                    let host = crate::focus::host_of(&url_of(&url)).unwrap_or_default();
                    if self.failed.insert(url) {
                        say.push(format!("Calendar {host}: {e}"));
                    }
                }
            }
        }
        say
    }

    /// Every event of the feeds now configured, without the ignored ones, by start time.
    pub fn events(&self, urls: &[String], ignore: &[String]) -> Vec<Event> {
        let mut v: Vec<Event> = urls.iter().filter_map(|u| self.feeds.get(u)).flatten().filter(|e| !ignored(&e.title, ignore)).cloned().collect();
        v.sort_by_key(|e| e.start);
        v
    }

    /// Reminder texts for events starting within `minutes`, each event once. 0 turns them off.
    pub fn due(&mut self, events: &[Event], now: i64, minutes: u32) -> Vec<String> {
        self.fired.retain(|(start, _)| *start > now - DAY);
        let window = i64::from(minutes) * 60;
        events
            .iter()
            .filter(|e| window > 0 && !e.all_day && e.start - window <= now && now < e.start)
            .filter(|e| self.fired.insert((e.start, e.title.clone())))
            .map(|e| format!("{} starts in {} min", e.title, (e.start - now + 59) / 60))
            .collect()
    }
}

pub fn ignored(title: &str, ignore: &[String]) -> bool {
    let title = title.to_lowercase();
    ignore.iter().any(|w| !w.trim().is_empty() && title.contains(&w.trim().to_lowercase()))
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

/// On now, or starting within `tolerance` seconds: what the empty root search shows.
pub fn near(e: &Event, now: i64, tolerance: i64) -> bool {
    !e.all_day && e.start - tolerance <= now && now < e.end.max(e.start + 1)
}

/// "Today" or "Tomorrow" by the event's local start day.
pub fn day_label(start: i64, now: i64) -> &'static str {
    if utc_to_local(start).div_euclid(DAY) <= utc_to_local(now).div_euclid(DAY) { "Today" } else { "Tomorrow" }
}

fn clock(naive: i64) -> String {
    let s = naive.rem_euclid(DAY);
    format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
}

pub fn span(e: &Event) -> String {
    if e.all_day { "All day".into() } else { format!("{} – {}", clock(utc_to_local(e.start)), clock(utc_to_local(e.end))) }
}

/// How far off an event is: "Now", "in 5 min", "in 3 h", nothing beyond a day.
pub fn when(e: &Event, now: i64) -> String {
    let d = e.start - now;
    match d {
        _ if e.all_day => String::new(),
        ..=0 => "Now".into(),
        1..=3599 => format!("in {} min", (d + 59) / 60),
        3600..=86399 => format!("in {} h", d / 3600),
        _ => String::new(),
    }
}

/// An event as a row; Enter joins its meeting, or does `otherwise` when it has no link.
pub fn row(e: &Event, now: i64, otherwise: Act) -> Item {
    let act = e.join.as_ref().map_or(otherwise, |url| Act::Join(e.title.clone(), url.clone()));
    Item::new(e.title.clone(), span(e), Icon::Vector(g::CALENDAR), act).acc(when(e, now))
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

/// Blocks on the network: call from a worker thread. Keeps the days around now.
pub fn fetch(url: &str) -> Result<Vec<Event>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build().into();
    let mut resp = agent.get(url_of(url)).call().map_err(|e| e.to_string())?;
    let body = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    if !body.contains("BEGIN:VCALENDAR") {
        return Err("that is not an ICS calendar".into());
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
        Event { start, end, title: title.into(), join: None, all_day: false }
    }

    #[test]
    fn a_utc_event_keeps_its_times() {
        let ics = cal("BEGIN:VEVENT\r\nUID:1\r\nDTSTART:20260115T093000Z\r\nDTEND:20260115T103000Z\r\nSUMMARY:Standup\\, daily\r\nEND:VEVENT\r\n");
        let e = parse_ics(&ics, 0, FAR);
        assert_eq!(e, vec![Event { start: utc(2026, 1, 15, 9, 30), end: utc(2026, 1, 15, 10, 30), title: "Standup, daily".into(), join: None, all_day: false }]);
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
        assert_eq!(join_link("https://meet.google.com/"), None);
        assert_eq!(join_link("no link"), None);
    }

    #[test]
    fn ignored_titles_are_skipped_whatever_their_case() {
        let mut c = Calendar::default();
        c.done(vec![("u".into(), Ok(vec![ev(10, 20, "Focus Time"), ev(30, 40, "Review")]))]);
        let urls = ["u".to_string()];
        assert_eq!(c.events(&urls, &[]).len(), 2);
        let left = c.events(&urls, &["focus".into(), " ".into()]);
        assert_eq!(left.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), vec!["Review"]);
        // Feeds dropped from the config do not show.
        assert!(c.events(&[], &[]).is_empty());
    }

    #[test]
    fn reminders_come_once_inside_the_window() {
        let mut c = Calendar::default();
        let events = [ev(1000, 2000, "Standup"), ev(5000, 6000, "Later")];
        assert!(c.due(&events, 600, 5).is_empty()); // 400 s away > 5 min? no: 400 > 300
        assert_eq!(c.due(&events, 710, 5), vec!["Standup starts in 5 min".to_string()]); // 290 s -> 5 min
        assert!(c.due(&events, 720, 5).is_empty()); // already told
        assert!(c.due(&events, 1000, 5).is_empty()); // started
        assert!(Calendar::default().due(&events, 900, 0).is_empty()); // off
        let all_day = [Event { all_day: true, ..ev(1000, 2000, "Holiday") }];
        assert!(Calendar::default().due(&all_day, 900, 5).is_empty());
    }

    #[test]
    fn the_empty_root_shows_what_is_on_now_or_about_to_be() {
        let e = ev(10_000, 12_000, "Call");
        assert!(!near(&e, 10_000 - 601, 600));
        assert!(near(&e, 10_000 - 600, 600));
        assert!(near(&e, 11_000, 600));
        assert!(!near(&e, 12_000, 600));
        assert!(!near(&Event { all_day: true, ..e.clone() }, 11_000, 600));
        assert_eq!((when(&e, 11_000), when(&e, 10_000 - 61), when(&e, 10_000 - 7200), when(&e, -100_000)), ("Now".into(), "in 2 min".into(), "in 2 h".into(), String::new()));
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
        let mut c = Calendar::default();
        let t = Instant::now();
        assert!(c.wants_fetch(1, t) && !c.wants_fetch(0, t));
        c.fetching(t);
        assert!(!c.wants_fetch(1, t + REFRESH * 2), "one fetch at a time");
        let url = "https://cal.example.com/private/secret.ics".to_string();
        assert!(c.done(vec![(url.clone(), Ok(vec![ev(1, 2, "A")]))]).is_empty());
        assert!(!c.wants_fetch(1, t + REFRESH - Duration::from_secs(1)) && c.wants_fetch(1, t + REFRESH));
        let say = c.done(vec![(url.clone(), Err("http status: 500".into()))]);
        assert_eq!(say, vec!["Calendar cal.example.com: http status: 500".to_string()]);
        assert!(!say[0].contains("secret"));
        assert!(c.done(vec![(url.clone(), Err("again".into()))]).is_empty());
        assert_eq!(c.events(std::slice::from_ref(&url), &[]).len(), 1);
        c.done(vec![(url.clone(), Ok(vec![]))]);
        assert_eq!(c.done(vec![(url, Err("down".into()))]).len(), 1, "told again after a good fetch");
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
