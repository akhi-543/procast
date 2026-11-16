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
        let (mut total, mut num) = (0u64, String::new());
        for c in s.chars() {
            let unit = match c {
                '0'..='9' => {
                    num.push(c);
                    continue;
                }
                'h' => 3600,
                'm' => 60,
                's' => 1,
                _ => return None,
            };
            total = total.checked_add(num.parse::<u64>().ok()?.checked_mul(unit)?)?;
            num.clear();
        }
        if !num.is_empty() {
            return None;
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

/// The system "ding"; silent when there is no sound device.
pub fn beep() {
    unsafe { _ = MessageBeep(MB_OK) };
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Entry {
    id: u64,
    label: String,
    /// Wall-clock start, ms since the epoch.
    start: u64,
    /// Length in ms; None for a stopwatch.
    length: Option<u64>,
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
            let _ = fs::write(path, serde_json::to_string(self).unwrap_or_default());
        }
    }

    fn add(&mut self, label: String, length: Option<u64>) -> u64 {
        self.next += 1;
        self.items.push(Entry { id: self.next, label, start: millis(SystemTime::now()), length });
        self.save();
        self.next
    }

    pub fn start(&mut self, label: String, d: Duration) -> u64 {
        self.add(label, Some(d.as_millis() as u64))
    }

    pub fn stopwatch(&mut self, label: String) -> u64 {
        self.add(label, None)
    }

    pub fn cancel(&mut self, id: u64) {
        self.items.retain(|e| e.id != id);
        self.save();
    }

    pub fn any(&self) -> bool {
        !self.items.is_empty()
    }

    /// How long a stopwatch has run (None for a timer or an unknown id).
    pub fn elapsed(&self, id: u64, now: SystemTime) -> Option<Duration> {
        let e = self.items.iter().find(|e| e.id == id && e.length.is_none())?;
        Some(Duration::from_millis(millis(now).saturating_sub(e.start)))
    }

    /// Timers that have run out: each one is handed back once, then forgotten.
    pub fn due(&mut self, now: SystemTime) -> Vec<(u64, String)> {
        let now = millis(now);
        let (done, left): (Vec<_>, Vec<_>) = std::mem::take(&mut self.items).into_iter().partition(|e| e.length.is_some_and(|l| e.start + l <= now));
        self.items = left;
        if !done.is_empty() {
            self.save();
        }
        done.into_iter().map(|e| (e.id, e.label)).collect()
    }

    /// The `rows` whose title contains `query` (case aside): what root search shows.
    pub fn matching(&self, now: SystemTime, query: &str) -> Vec<(u64, String, String)> {
        let q = query.trim().to_lowercase();
        self.rows(now).into_iter().filter(|r| r.1.to_lowercase().contains(&q)).collect()
    }

    /// (id, title, time) of everything running: the time left, or a stopwatch's elapsed time.
    pub fn rows(&self, now: SystemTime) -> Vec<(u64, String, String)> {
        let now = millis(now);
        self.items
            .iter()
            .map(|e| {
                let (name, ms) = match e.length {
                    Some(l) => ("Timer", (e.start + l).saturating_sub(now)),
                    None => ("Stopwatch", now.saturating_sub(e.start)),
                };
                // A timer counts down in whole seconds, rounded up: 0:00 means done.
                let ms = if e.length.is_some() { ms.div_ceil(1000) * 1000 } else { ms };
                let title = if e.label.is_empty() { name.to_string() } else { e.label.clone() };
                (e.id, title, fmt_duration(Duration::from_millis(ms)))
            })
            .collect()
    }
}

/// Whether the timer rows on screen (their ids, in order) are no longer the `rows` that should
/// show: the list needs rebuilding, not just the times.
pub fn rows_changed(shown: &[u64], rows: &[(u64, String, String)]) -> bool {
    shown.len() != rows.len() || shown.iter().zip(rows).any(|(id, r)| *id != r.0)
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
        for bad in ["0", "0m", "abc", "", "5x", "m", "1:2:3:4", "1:", "99999999999999999999"] {
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
        assert_eq!(t.due(later), vec![(tea, "tea".to_string())]);
        assert!(t.due(later).is_empty());
        assert!(t.elapsed(watch, later).is_some_and(|e| e >= Duration::from_secs(60)));
        assert_eq!(t.rows(later).len(), 1);
    }

    #[test]
    fn a_timer_already_past_when_checked_fires_once() {
        let mut t = Timers::default();
        t.items.push(Entry { id: 7, label: "old".into(), start: 1_000, length: Some(5_000) });
        assert_eq!(t.due(SystemTime::now()), vec![(7, "old".to_string())]);
        assert!(t.due(SystemTime::now()).is_empty());
    }

    #[test]
    fn rows_count_down_and_up() {
        let items = vec![Entry { id: 1, label: "tea".into(), start: 0, length: Some(300_000) }, Entry { id: 2, label: String::new(), start: 0, length: None }];
        let mut t = Timers { items, ..Default::default() };
        let at = |s| UNIX_EPOCH + Duration::from_millis(s);
        assert_eq!(t.rows(at(28_500)), vec![(1, "tea".into(), "4:32".into()), (2, "Stopwatch".into(), "0:28".into())]);
        t.cancel(1);
        assert_eq!(t.rows(at(0)).len(), 1);
    }

    #[test]
    fn a_query_filters_rows_and_only_a_changed_set_needs_a_rebuild() {
        let items = vec![Entry { id: 1, label: "Tea".into(), start: 0, length: Some(300_000) }, Entry { id: 2, label: String::new(), start: 0, length: None }];
        let mut t = Timers { items, ..Default::default() };
        let at = |s| UNIX_EPOCH + Duration::from_millis(s);
        let ids = |q: &str| t.matching(at(1_000), q).iter().map(|r| r.0).collect::<Vec<_>>();
        assert_eq!((ids(""), ids("tea"), ids(" STOP "), ids("x")), (vec![1, 2], vec![1], vec![2], vec![]));
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
