//! Timers and stopwatches. They keep to the wall clock (so a sleeping PC still counts the time
//! that passed) and live in `%APPDATA%\procast\timers.json`, so a restart keeps them.

use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use windows::Win32::{System::Diagnostics::Debug::MessageBeep, UI::WindowsAndMessaging::MB_OK};

/// `90`, `90s`, `5m`, `1h30m` or `1:30` (minutes and seconds; `1:02:03` has hours). Never zero.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim().to_lowercase();
    let secs = if s.contains(':') {
        let parts = s.split(':').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>()?;
        match parts[..] {
            [m, s] => m.checked_mul(60)?.checked_add(s)?,
            [h, m, s] => h.checked_mul(3600)?.checked_add(m.checked_mul(60)?)?.checked_add(s)?,
            _ => return None,
        }
    } else if let Ok(n) = s.parse::<u64>() {
        n
    } else {
        // Number-unit pairs, spaces allowed: "1h30m", "1 hour 30 minutes"; a bare number last
        // takes the unit below the one before it ("1h30", "5m30").
        let (mut total, mut last, mut rest) = (0u64, 0u64, s.as_str());
        while !rest.trim_start().is_empty() {
            rest = rest.trim_start();
            let n = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            let num = rest[..n].parse::<u64>().ok()?;
            rest = rest[n..].trim_start();
            let u = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
            last = match &rest[..u] {
                "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
                "m" | "min" | "mins" | "minute" | "minutes" => 60,
                "s" | "sec" | "secs" | "second" | "seconds" => 1,
                "" if rest.is_empty() && last > 1 => last / 60,
                _ => return None,
            };
            total = total.checked_add(num.checked_mul(last)?)?;
            rest = &rest[u..];
        }
        total
    };
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// "4:32", or "1:04:32" from an hour up.
pub fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// "25 min", "1 h 30 min", "45 s": a length as a toast or subtitle says it.
pub fn fmt_spoken(d: Duration) -> String {
    let s = d.as_secs();
    let parts: Vec<String> = [(s / 3600, "h"), (s / 60 % 60, "min"), (s % 60, "s")].iter().filter(|p| p.0 > 0).map(|(n, unit)| format!("{n} {unit}")).collect();
    if parts.is_empty() { "0 s".into() } else { parts.join(" ") }
}

/// The toast when a timer runs out: "tea · 25 min done".
pub fn done_text(label: &str, length: Duration) -> String {
    format!("{} · {} done", if label.is_empty() { "Timer" } else { label }, fmt_spoken(length))
}

/// A finished timer's accessory, until it is dismissed.
pub const DONE: &str = "Done";

/// How long ago, from the seconds since: "just now", "3 min ago", "2 d ago".
pub fn ago(secs: u64) -> String {
    match secs {
        a @ 0..60 => if a < 10 { "just now".into() } else { format!("{a} s ago") },
        a @ 60..3600 => format!("{} min ago", a / 60),
        a @ 3600..86400 => format!("{} h ago", a / 3600),
        a => format!("{} d ago", a / 86400),
    }
}

/// The system "ding"; silent when there is no sound device.
pub fn beep() {
    unsafe { _ = MessageBeep(MB_OK) };
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Entry {
    id: u64,
    label: String,
    /// Wall-clock start, ms since the epoch; moved on by the time spent paused.
    start: u64,
    /// Length in ms; None for a stopwatch.
    length: Option<u64>,
    /// When it was paused, ms since the epoch: the clock stands still from then.
    #[serde(default)]
    paused_at: Option<u64>,
    /// A timer that ran out and was announced; it says "Done" until stopped.
    #[serde(default)]
    done: bool,
}

/// One running row: what root search shows for a timer or stopwatch.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: u64,
    /// The label, or "Timer" / "Stopwatch".
    pub title: String,
    pub stopwatch: bool,
    pub paused: bool,
    pub done: bool,
    /// Time left, or a stopwatch's elapsed time ("4:12").
    pub time: String,
}

impl Row {
    /// "4:12 left", "12:48", "Paused · 4:12" or "Done".
    pub fn accessory(&self) -> String {
        match (self.done, self.paused, self.stopwatch) {
            (true, ..) => DONE.into(),
            (_, true, _) => format!("Paused · {}", self.time),
            (_, _, true) => self.time.clone(),
            _ => format!("{} left", self.time),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Timers {
    next: u64,
    items: Vec<Entry>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Timers {
    pub fn load(path: PathBuf) -> Self {
        let mut t: Self = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        t.path = Some(path);
        t
    }

    fn save(&self) {
        if let Some(path) = &self.path {
            let _ = crate::focus::write_atomic(path, &serde_json::to_string(self).unwrap_or_default());
        }
    }

    fn push(&mut self, label: String, length: Option<u64>) -> u64 {
        self.next += 1;
        self.items.push(Entry { id: self.next, label, start: millis(SystemTime::now()), length, ..Default::default() });
        self.save();
        self.next
    }

    pub fn start(&mut self, label: String, d: Duration) -> u64 {
        self.push(label, Some(d.as_millis() as u64))
    }

    pub fn stopwatch(&mut self, label: String) -> u64 {
        self.push(label, None)
    }

    /// Changes entry `id` and saves; nothing for an unknown id.
    fn edit(&mut self, id: u64, f: impl FnOnce(&mut Entry)) {
        if let Some(e) = self.items.iter_mut().find(|e| e.id == id) {
            f(e);
            self.save();
        }
    }

    pub fn pause(&mut self, id: u64, now: SystemTime) {
        self.edit(id, |e| {
            if !e.done {
                e.paused_at.get_or_insert(millis(now));
            }
        });
    }

    pub fn resume(&mut self, id: u64, now: SystemTime) {
        self.edit(id, |e| {
            if let Some(at) = e.paused_at.take() {
                e.start += millis(now).saturating_sub(at);
            }
        });
    }

    /// From the start again, running.
    pub fn restart(&mut self, id: u64, now: SystemTime) {
        self.edit(id, |e| (e.start, e.paused_at, e.done) = (millis(now), None, false));
    }

    /// More time on a timer; a stopwatch counts it as already elapsed.
    pub fn add(&mut self, id: u64, d: Duration) {
        let ms = d.as_millis() as u64;
        self.edit(id, |e| match &mut e.length {
            Some(l) => *l += ms,
            None => e.start = e.start.saturating_sub(ms),
        });
    }

    pub fn rename(&mut self, id: u64, name: String) {
        self.edit(id, |e| e.label = name);
    }

    pub fn cancel(&mut self, id: u64) {
        self.items.retain(|e| e.id != id);
        self.save();
    }

    /// Whether anything still counts (not paused, not done): the 1 s tick runs only then.
    pub fn ticking(&self) -> bool {
        self.items.iter().any(|e| !e.done && e.paused_at.is_none())
    }

    /// How long a stopwatch has run (None for a timer or an unknown id).
    pub fn elapsed(&self, id: u64, now: SystemTime) -> Option<Duration> {
        let e = self.items.iter().find(|e| e.id == id && e.length.is_none())?;
        Some(Duration::from_millis(e.paused_at.unwrap_or(millis(now)).saturating_sub(e.start)))
    }

    /// Timers that ran out, (id, label, length): each is handed back once, then shows Done.
    pub fn due(&mut self, now: SystemTime) -> Vec<(u64, String, Duration)> {
        let now = millis(now);
        let mut out = vec![];
        for e in &mut self.items {
            if let Some(l) = e.length
                && !e.done
                && e.paused_at.is_none()
                && e.start + l <= now
            {
                e.done = true;
                out.push((e.id, e.label.clone(), Duration::from_millis(l)));
            }
        }
        if !out.is_empty() {
            self.save();
        }
        out
    }

    /// The `rows` root search shows: all of them while the query is empty or names timers
    /// ("tim", "tmr", "timer 5m", "t 90"), else those whose title contains it.
    pub fn matching(&self, now: SystemTime, query: &str) -> Vec<Row> {
        let q = query.trim().to_lowercase();
        let all = q.is_empty() || names_timers(&q);
        self.rows(now).into_iter().filter(|r| all || r.title.to_lowercase().contains(&q)).collect()
    }

    /// Everything running, paused or done, oldest first.
    pub fn rows(&self, now: SystemTime) -> Vec<Row> {
        let now = millis(now);
        self.items
            .iter()
            .map(|e| {
                let at = e.paused_at.unwrap_or(now);
                // A timer counts down in whole seconds, rounded up: 0:00 means done.
                let ms = match e.length {
                    Some(l) => (e.start + l).saturating_sub(at).div_ceil(1000) * 1000,
                    None => at.saturating_sub(e.start),
                };
                let kind = if e.length.is_some() { "Timer" } else { "Stopwatch" };
                let title = if e.label.is_empty() { kind.to_string() } else { e.label.clone() };
                Row { id: e.id, title, stopwatch: e.length.is_none(), paused: e.paused_at.is_some(), done: e.done, time: fmt_duration(Duration::from_millis(ms)) }
            })
            .collect()
    }
}

/// `q` (lower case) is "timer" being typed ("ti" on, never just "t": "te" is Terminal, not
/// timers) or starts with a timer keyword ("timer 5m", "t 90").
fn names_timers(q: &str) -> bool {
    q.starts_with("timer ") || q.starts_with("t ") || (q.len() >= 2 && "timer".starts_with(q))
}

/// Whether the timer rows on screen (their ids, in order) are no longer the `rows` that should
/// show: the list needs rebuilding, not just the times.
pub fn rows_changed(shown: &[u64], rows: &[Row]) -> bool {
    shown.len() != rows.len() || shown.iter().zip(rows).any(|(id, r)| *id != r.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Option<Duration> {
        Some(Duration::from_secs(n))
    }

    #[test]
    fn durations_parse_in_every_spelling_and_refuse_nonsense() {
        assert_eq!(parse_duration("90"), secs(90));
        assert_eq!(parse_duration("90s"), secs(90));
        assert_eq!(parse_duration("5m"), secs(300));
        assert_eq!(parse_duration("1h30m"), secs(5400));
        assert_eq!(parse_duration("1:30"), secs(90));
        assert_eq!(parse_duration("1:02:03"), secs(3723));
        assert_eq!(parse_duration(" 5M "), secs(300));
        assert_eq!(parse_duration("1h30"), secs(5400), "a bare number after hours is minutes");
        assert_eq!(parse_duration("5m30"), secs(330), "and after minutes, seconds");
        assert_eq!(parse_duration("1 hour 30 minutes"), secs(5400));
        assert_eq!(parse_duration("50 min"), secs(3000));
        assert_eq!(parse_duration("2 hrs 10 secs"), secs(7210));
        for bad in ["0", "0m", "abc", "", "5x", "m", "1:2:3:4", "1:", "99999999999999999999", "30 1h", "1 hour minutes", "5 m é"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
    }

    #[test]
    fn durations_show_as_clock_time() {
        assert_eq!(fmt_duration(Duration::from_secs(272)), "4:32");
        assert_eq!(fmt_duration(Duration::from_secs(3904)), "1:05:04");
    }

    #[test]
    fn a_timer_is_due_once_and_a_stopwatch_never() {
        let mut t = Timers::default();
        let tea = t.start("tea".into(), Duration::from_secs(60));
        let watch = t.stopwatch(String::new());
        let now = SystemTime::now();
        assert!(t.due(now).is_empty());
        let later = now + Duration::from_secs(61);
        assert_eq!(t.due(later), vec![(tea, "tea".to_string(), Duration::from_secs(60))]);
        assert!(t.due(later).is_empty());
        assert!(t.elapsed(watch, later).is_some_and(|e| e >= Duration::from_secs(60)));
        assert_eq!(t.rows(later).len(), 2, "the done timer stays until stopped");
    }

    #[test]
    fn a_timer_already_past_when_checked_fires_once() {
        let mut t = Timers::default();
        t.items.push(Entry { id: 7, label: "old".into(), start: 1_000, length: Some(5_000), ..Default::default() });
        assert_eq!(t.due(SystemTime::now()), vec![(7, "old".to_string(), Duration::from_secs(5))]);
        assert!(t.due(SystemTime::now()).is_empty());
    }

    #[test]
    fn rows_count_down_and_up() {
        let items = vec![Entry { id: 1, label: "tea".into(), start: 0, length: Some(300_000), ..Default::default() }, Entry { id: 2, start: 0, ..Default::default() }];
        let mut t = Timers { items, ..Default::default() };
        let at = |s| UNIX_EPOCH + Duration::from_millis(s);
        let shown = |t: &Timers| t.rows(at(28_500)).iter().map(|r| (r.id, r.title.clone(), r.accessory())).collect::<Vec<_>>();
        assert_eq!(shown(&t), vec![(1, "tea".into(), "4:32 left".into()), (2, "Stopwatch".into(), "0:28".into())]);
        t.cancel(1);
        assert_eq!(t.rows(at(0)).len(), 1);
    }

    #[test]
    fn a_query_filters_rows_and_only_a_changed_set_needs_a_rebuild() {
        let items = vec![Entry { id: 1, label: "Tea".into(), start: 0, length: Some(300_000), ..Default::default() }, Entry { id: 2, start: 0, ..Default::default() }];
        let mut t = Timers { items, ..Default::default() };
        let at = |s| UNIX_EPOCH + Duration::from_millis(s);
        let ids = |q: &str| t.matching(at(1_000), q).iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!((ids(""), ids("tea"), ids(" STOP "), ids("x")), (vec![1, 2], vec![1], vec![2], vec![]));
        assert_eq!((ids("tim"), ids("timer"), ids("timer 5m"), ids("t 90"), ids("e")), (vec![1, 2], vec![1, 2], vec![1, 2], vec![1, 2], vec![1]), "a query for timers shows them all");
        // Query "tea" with both running: one row shown, one expected, so a tick only updates times.
        let rows = t.matching(at(2_000), "tea");
        assert!(!rows_changed(&[1], &rows));
        assert!(!rows_changed(&[], &t.matching(at(2_000), "x")));
        // A row that finished, appeared or swapped does need it.
        t.cancel(1);
        assert!(rows_changed(&[1], &t.matching(at(3_000), "tea")));
        assert!(rows_changed(&[], &t.matching(at(3_000), "")));
        assert!(rows_changed(&[1, 2], &t.matching(at(3_000), "")));
        assert!(rows_changed(&[1], &t.matching(at(3_000), "")));
    }

    #[test]
    fn only_timer_being_typed_or_a_timer_keyword_names_timers() {
        for q in ["ti", "tim", "time", "timer", "timer 5m", "t 90"] {
            assert!(names_timers(q), "{q}");
        }
        for q in ["t", "te", "ter", "tm", "tmr", "timers", "tea", "stopwatch"] {
            assert!(!names_timers(q), "{q}: an ordinary word");
        }
    }

    #[test]
    fn durations_are_spoken_in_units() {
        assert_eq!(fmt_spoken(Duration::from_secs(25 * 60)), "25 min");
        assert_eq!(fmt_spoken(Duration::from_secs(90 * 60)), "1 h 30 min");
        assert_eq!(fmt_spoken(Duration::from_secs(45)), "45 s");
    }

    #[test]
    fn the_done_toast_names_the_timer_and_its_length() {
        assert_eq!(done_text("tea", Duration::from_secs(25 * 60)), "tea · 25 min done");
        assert_eq!(done_text("", Duration::from_secs(25 * 60)), "Timer · 25 min done");
    }

    #[test]
    fn pause_then_resume_keeps_the_time_left_and_add_adds_it() {
        let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
        let mut t = Timers { items: vec![Entry { id: 1, label: "tea".into(), start: 0, length: Some(300_000), ..Default::default() }], ..Default::default() };
        t.pause(1, at(28));
        assert_eq!(t.rows(at(100))[0].accessory(), "Paused · 4:32", "a paused timer does not count down");
        assert!(!t.ticking() && t.due(at(1000)).is_empty(), "nor run out");
        t.resume(1, at(100));
        assert_eq!(t.rows(at(100))[0].accessory(), "4:32 left");
        t.add(1, Duration::from_secs(300));
        assert_eq!(t.rows(at(100))[0].accessory(), "9:32 left");
        t.restart(1, at(200));
        assert_eq!(t.rows(at(200))[0].accessory(), "10:00 left");
        t.rename(1, "pasta".into());
        assert_eq!(t.rows(at(200))[0].title, "pasta");
    }

    #[test]
    fn a_stopwatch_pauses_and_shows_its_time() {
        let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
        let mut t = Timers { items: vec![Entry { id: 2, start: 0, ..Default::default() }], ..Default::default() };
        assert_eq!(t.rows(at(768))[0].accessory(), "12:48");
        t.pause(2, at(768));
        assert_eq!(t.elapsed(2, at(900)), Some(Duration::from_secs(768)));
        t.resume(2, at(900));
        assert_eq!(t.rows(at(901))[0].accessory(), "12:49");
    }

    #[test]
    fn a_finished_timer_stays_done_until_stopped() {
        let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
        let mut t = Timers { items: vec![Entry { id: 1, label: "tea".into(), start: 0, length: Some(60_000), ..Default::default() }], ..Default::default() };
        assert_eq!(t.due(at(61)), vec![(1, "tea".to_string(), Duration::from_secs(60))]);
        let row = &t.rows(at(62))[0];
        assert!(row.done && row.accessory() == DONE, "the row says Done");
        assert!(t.due(at(62)).is_empty() && !t.ticking(), "announced once, nothing left to count");
        t.cancel(1);
        assert!(t.rows(at(62)).is_empty());
    }

    #[test]
    fn the_save_replaces_the_file_through_a_temp_file() {
        let path = std::env::temp_dir().join(format!("procast-timers-atomic-{}.json", std::process::id()));
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        fs::write(&tmp, "half a file").unwrap();
        let mut t = Timers::load(path.clone());
        t.start("tea".into(), Duration::from_secs(60));
        assert!(!tmp.exists(), "the temp file was renamed over the real one");
        assert_eq!(Timers::load(path.clone()).items, t.items);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn timers_survive_a_restart() {
        let path = std::env::temp_dir().join(format!("procast-timers-test-{}.json", std::process::id()));
        let mut t = Timers::load(path.clone());
        let (a, b) = (t.start("tea".into(), Duration::from_secs(300)), t.stopwatch(String::new()));
        let mut back = Timers::load(path.clone());
        assert_eq!(back.items, t.items);
        // Ids go on from where they were.
        assert!(back.stopwatch(String::new()) > b.max(a));
        let _ = fs::remove_file(path);
    }
}
