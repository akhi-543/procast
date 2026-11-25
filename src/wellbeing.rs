// ponytail: removed once the sampler (Task 3) uses everything
#![allow(dead_code)]
//! Screen-time accounting: what a sample is worth, categories, and the weekly summary text.

use crate::calendar::{civil_from_days, days_from_civil};
use std::collections::HashMap;

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
}
