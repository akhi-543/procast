//! Query -> rows. Every source is in-process and synchronous; the whole search runs in
//! well under a millisecond for a few hundred apps, so there is no async coordinator.
// ponytail: sync search on the UI thread. Add a generation-counted async coordinator
// (open-raycast pattern) when slow sources arrive: files (Phase 2), extensions (Phase 3).

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

use crate::{config::Config, win::App};

#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    App(String),
    Open(String),
    Copy(String),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub title: String,
    pub subtitle: &'static str,
    pub act: Act,
}

impl Item {
    /// Key used for frecency; only apps are learned.
    pub fn key(&self) -> Option<&str> {
        match &self.act {
            Act::App(id) => Some(id),
            _ => None,
        }
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Launch history: score = Σ exp(-age_days / 14) over the last 20 uses.
#[derive(Default)]
pub struct Frecency {
    path: Option<PathBuf>,
    uses: HashMap<String, Vec<u64>>,
}

impl Frecency {
    pub fn load(path: PathBuf) -> Self {
        let uses = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { path: Some(path), uses }
    }

    pub fn score(&self, key: &str, now: u64) -> f64 {
        let Some(times) = self.uses.get(key) else { return 0. };
        times.iter().map(|&t| (-(now.saturating_sub(t) as f64) / (14. * 86400.)).exp()).sum()
    }

    pub fn record(&mut self, key: &str, now: u64) {
        let times = self.uses.entry(key.to_string()).or_default();
        times.push(now);
        if times.len() > 20 {
            times.remove(0);
        }
        if let Some(path) = &self.path {
            let _ = fs::write(path, serde_json::to_string(&self.uses).unwrap_or_default());
        }
    }
}

pub fn search(query: &str, apps: &[App], cfg: &Config, frec: &Frecency, matcher: &mut Matcher) -> Vec<Item> {
    let q = query.trim();
    let now = now();
    let max = cfg.max_results.max(1);
    let app_item = |a: &App| Item { title: a.name.clone(), subtitle: "Application", act: Act::App(a.id.clone()) };

    if q.is_empty() {
        let mut recent: Vec<_> = apps.iter().map(|a| (frec.score(&a.id, now), a)).filter(|(s, _)| *s > 0.01).collect();
        recent.sort_by(|a, b| b.0.total_cmp(&a.0));
        return recent.into_iter().take(max).map(|(_, a)| app_item(a)).collect();
    }

    let mut top = Vec::new();
    if let Some((kw, rest)) = q.split_once(' ')
        && let Some(alias) = cfg.alias.iter().find(|a| a.keyword.eq_ignore_ascii_case(kw))
        && !rest.trim().is_empty()
    {
        let url = alias.url.replace("{query}", &url_encode(rest.trim()));
        top.push(Item { title: format!("{}: {}", alias.keyword, rest.trim()), subtitle: "Alias", act: Act::Open(url) });
    }
    if cfg.search.calculator
        && let Some(result) = calculate(q)
    {
        top.push(Item { title: format!("= {result}"), subtitle: "Calculator · Enter copies", act: Act::Copy(result) });
    }

    let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = Vec::new();
    let mut hits: Vec<(f64, &App)> = apps
        .iter()
        .filter_map(|a| {
            let s = pattern.score(Utf32Str::new(&a.name, &mut buf), matcher)?;
            // ponytail: fixed weight; tune once there is real launch history.
            Some((s as f64 + 25. * frec.score(&a.id, now).min(10.), a))
        })
        .collect();
    hits.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.name.len().cmp(&b.1.name.len())));

    let fallback = (!cfg.search.fallback.is_empty()).then(|| Item {
        title: format!("Search the web for \"{q}\""),
        subtitle: "Web",
        act: Act::Open(cfg.search.fallback.replace("{query}", &url_encode(q))),
    });
    let room = max.saturating_sub(top.len() + fallback.is_some() as usize);
    top.extend(hits.into_iter().take(room).map(|(_, a)| app_item(a)));
    top.extend(fallback);
    top.truncate(max);
    top
}

struct Deadline(Instant);

impl fend_core::Interrupt for Deadline {
    fn should_interrupt(&self) -> bool {
        Instant::now() > self.0
    }
}

/// Only for input with a digit; time-boxed so `9^9^9` can't freeze the UI.
fn calculate(q: &str) -> Option<String> {
    if !q.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut ctx = fend_core::Context::new();
    let deadline = Deadline(Instant::now() + Duration::from_millis(50));
    let result = fend_core::evaluate_with_interrupt(q, &mut ctx, &deadline).ok()?;
    let main = result.get_main_result().trim().to_string();
    (!main.is_empty() && main != q).then_some(main)
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_calc_alias_fallback() {
        let apps: Vec<App> = ["Visual Studio Code", "Visual Studio 2022", "Calculator", "Google Chrome", "Code Writer"]
            .iter()
            .enumerate()
            .map(|(i, n)| App { name: n.to_string(), id: format!("id{i}") })
            .collect();
        let mut cfg = crate::config::parse(
            "[[alias]]\nkeyword = \"gh\"\nurl = \"https://github.com/search?q={query}\"",
        )
        .unwrap();
        cfg.max_results = 5;
        let mut m = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let mut frec = Frecency::default();

        let r = search("vsc", &apps, &cfg, &frec, &mut m);
        assert_eq!(r[0].title, "Visual Studio Code");
        assert_eq!(r.last().unwrap().subtitle, "Web");
        assert!(r.len() <= 5);

        // Launch history breaks near-ties: "code" matches two apps.
        frec.record("id4", now());
        frec.record("id4", now());
        assert_eq!(search("code", &apps, &cfg, &frec, &mut m)[0].title, "Code Writer");
        // Empty query lists recent apps only.
        let recent = search("", &apps, &cfg, &frec, &mut m);
        assert_eq!(recent.len(), 1);

        let r = search("2+2*3", &apps, &cfg, &frec, &mut m);
        assert_eq!(r[0].act, Act::Copy("8".into()));
        assert!(search("chrome", &apps, &cfg, &frec, &mut m).iter().all(|i| !matches!(i.act, Act::Copy(_))));
        let t = Instant::now();
        calculate("9^9^9^9");
        assert!(t.elapsed() < Duration::from_millis(500), "calculator must be time-boxed");

        let r = search("gh rust gpui", &apps, &cfg, &frec, &mut m);
        assert_eq!(r[0].act, Act::Open("https://github.com/search?q=rust%20gpui".into()));
    }
}
