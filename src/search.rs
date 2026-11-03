//! Query -> rows. Root, clipboard and emoji search run synchronously: a few hundred
//! catalog items match in well under a millisecond. File search, the one slow source,
//! runs on its own worker with a generation counter (files.rs).

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use gpui::{RenderImage, SharedString};
use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

use crate::{
    clipboard::Entry,
    commands::{self, Cmd},
    config::Config,
    scripts::Script,
    win::App,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    App(String),
    RunAsAdmin(String),
    Open(String),
    /// Opens a file or folder; secondary actions reveal or copy its path.
    File(String),
    Reveal(String),
    /// Windows Run: a command line.
    Run(String),
    Copy(String),
    Paste(String),
    /// A clipboard history entry (paste, or delete from history).
    Clip(String),
    DeleteClip(String),
    /// Snippet template, expanded at paste time.
    Snippet(String),
    /// Index into the launcher's script list.
    Script(usize),
    Cmd(Cmd),
    /// Put text in the search box (quicklink waiting for its argument).
    Fill(String),
}

impl Act {
    /// What Enter does, as shown in the footer.
    pub fn label(&self) -> &'static str {
        match self {
            Act::App(_) | Act::Open(_) | Act::File(_) => "Open",
            Act::RunAsAdmin(_) => "Run as Administrator",
            Act::Reveal(_) => "Show in Explorer",
            Act::Run(_) | Act::Script(_) => "Run",
            Act::Copy(_) => "Copy",
            Act::Paste(_) | Act::Clip(_) | Act::Snippet(_) => "Paste",
            Act::DeleteClip(_) => "Delete Entry",
            Act::Cmd(Cmd::SearchFiles | Cmd::ClipboardHistory | Cmd::Emoji) => "Open",
            Act::Cmd(_) => "Run Command",
            Act::Fill(_) => "Type Query",
        }
    }
}

#[derive(Clone)]
pub enum Icon {
    Image(Arc<RenderImage>),
    Glyph(SharedString),
}

impl Icon {
    pub fn glyph(s: impl Into<SharedString>) -> Self {
        Icon::Glyph(s.into())
    }
}

#[derive(Clone)]
pub struct Item {
    pub title: String,
    pub subtitle: String,
    pub icon: Icon,
    pub act: Act,
    /// Frecency key; None for rows that aren't learned (calculator, web, clipboard).
    pub key: Option<String>,
    /// Text the fuzzy matcher sees.
    hay: String,
}

impl Item {
    pub fn new(title: impl Into<String>, subtitle: impl Into<String>, icon: Icon, act: Act) -> Self {
        let title = title.into();
        Self { hay: title.clone(), title, subtitle: subtitle.into(), icon, act, key: None }
    }

    /// The Ctrl+K action panel: primary action first, then secondary ones.
    pub fn actions(&self) -> Vec<Act> {
        let mut acts = vec![self.act.clone()];
        match &self.act {
            Act::App(id) => acts.extend([Act::RunAsAdmin(id.clone()), Act::Copy(self.title.clone())]),
            Act::File(path) => acts.extend([Act::Reveal(path.clone()), Act::Copy(path.clone())]),
            Act::Open(url) | Act::Run(url) => acts.push(Act::Copy(url.clone())),
            Act::Copy(text) => acts.push(Act::Paste(text.clone())),
            Act::Paste(text) => acts.push(Act::Copy(text.clone())),
            Act::Clip(text) => acts.extend([Act::Copy(text.clone()), Act::DeleteClip(text.clone())]),
            _ => {}
        }
        acts
    }

    fn keyed(mut self, key: String) -> Self {
        self.key = Some(key);
        self
    }

    fn words(mut self, words: &str) -> Self {
        if !words.is_empty() {
            self.hay = format!("{} {words}", self.hay);
        }
        self
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

/// Everything root search can find. Rebuilt when apps or config change.
pub fn catalog(apps: &[App], scripts: &[Script], cfg: &Config) -> Vec<Item> {
    let mut items: Vec<Item> = apps
        .iter()
        .map(|a| {
            let icon = a.icon.clone().map(Icon::Image).unwrap_or(Icon::glyph("▫"));
            Item::new(&a.name, "Application", icon, Act::App(a.id.clone())).keyed(a.id.clone())
        })
        .collect();
    for &(cmd, title, words, glyph) in commands::ALL {
        if matches!(cmd, Cmd::ClipboardHistory | Cmd::ClearClipboardHistory) && !cfg.clipboard.enabled
            || cmd == Cmd::SearchFiles && !cfg.files.enabled
        {
            continue;
        }
        items.push(Item::new(title, cmd.subtitle(), Icon::glyph(glyph), Act::Cmd(cmd)).keyed(cmd.key()).words(words));
    }
    for q in &cfg.quicklink {
        let act = if q.url.contains("{query}") {
            Act::Fill(format!("{} ", if q.keyword.is_empty() { q.title() } else { &q.keyword }))
        } else {
            Act::Open(q.url.clone())
        };
        items.push(Item::new(q.title(), "Quicklink", Icon::glyph("↗"), act).keyed(format!("ql:{}", q.title())).words(&q.keyword));
    }
    for s in &cfg.snippet {
        items.push(Item::new(&s.name, "Snippet", Icon::glyph("✎"), Act::Snippet(s.text.clone())).keyed(format!("sn:{}", s.name)).words(&s.keyword));
    }
    for (i, s) in scripts.iter().enumerate() {
        let sub = if s.package.is_empty() { "Script Command".to_string() } else { s.package.clone() };
        let icon = Icon::glyph(s.icon.clone().unwrap_or_else(|| "⌨".into()));
        items.push(Item::new(&s.title, sub, icon, Act::Script(i)).keyed(format!("sc:{}", s.path)));
    }
    items
}

fn fuzzy<'a>(q: &str, items: impl Iterator<Item = &'a Item>, frec: &Frecency, matcher: &mut Matcher) -> Vec<(f64, &'a Item)> {
    let now = now();
    let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = Vec::new();
    let mut hits: Vec<(f64, &Item)> = items
        .filter_map(|it| {
            let s = pattern.score(Utf32Str::new(&it.hay, &mut buf), matcher)?;
            let boost = it.key.as_deref().map_or(0., |k| frec.score(k, now).min(10.));
            // ponytail: fixed weight; tune once there is real launch history.
            Some((s as f64 + 25. * boost, it))
        })
        .collect();
    hits.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.title.len().cmp(&b.1.title.len())));
    hits
}

pub fn root(query: &str, catalog: &[Item], cfg: &Config, frec: &Frecency, matcher: &mut Matcher) -> Vec<Item> {
    let q = query.trim();
    let max = cfg.max_results.max(1);

    if q.is_empty() {
        let now = now();
        let mut recent: Vec<_> =
            catalog.iter().filter_map(|it| Some((frec.score(it.key.as_deref()?, now), it))).filter(|(s, _)| *s > 0.01).collect();
        recent.sort_by(|a, b| b.0.total_cmp(&a.0));
        if recent.is_empty() {
            // First run: point at a few things worth knowing about.
            let starters = [Cmd::ClipboardHistory, Cmd::Emoji, Cmd::EditConfig];
            return catalog.iter().filter(|it| matches!(it.act, Act::Cmd(c) if starters.contains(&c))).cloned().collect();
        }
        return recent.into_iter().take(max).map(|(_, it)| it.clone()).collect();
    }

    let mut top = Vec::new();
    if let Some(cmdline) = q.strip_prefix('>').map(str::trim).filter(|c| !c.is_empty()) {
        top.push(Item::new(format!("Run {cmdline}"), "Windows Run", Icon::glyph("›"), Act::Run(cmdline.to_string())));
    }
    if let Some((kw, rest)) = q.split_once(' ')
        && let Some(link) = cfg.quicklink.iter().find(|l| !l.keyword.is_empty() && l.keyword.eq_ignore_ascii_case(kw))
        && !rest.trim().is_empty()
    {
        let url = link.url.replace("{query}", &url_encode(rest.trim()));
        top.push(Item::new(format!("{}: {}", link.title(), rest.trim()), "Quicklink", Icon::glyph("↗"), Act::Open(url)).keyed(format!("ql:{}", link.title())));
    }
    if cfg.search.calculator
        && let Some(result) = calculate(q)
    {
        top.push(Item::new(format!("= {result}"), "Calculator · Enter copies", Icon::glyph("="), Act::Copy(result)));
    }

    let fallback = (!cfg.search.fallback.is_empty()).then(|| {
        Item::new(format!("Search the web for \"{q}\""), "Web", Icon::glyph("🔍"), Act::Open(cfg.search.fallback.replace("{query}", &url_encode(q))))
    });
    let room = max.saturating_sub(top.len() + fallback.is_some() as usize);
    top.extend(fuzzy(q, catalog.iter(), frec, matcher).into_iter().take(room).map(|(_, it)| it.clone()));
    top.extend(fallback);
    top.truncate(max);
    top
}

pub fn clipboard(query: &str, entries: &[Entry], max: usize, matcher: &mut Matcher) -> Vec<Item> {
    let now = now();
    let row = |e: &Entry| {
        let first = e.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        let title: String = first.chars().take(120).collect();
        let age = now.saturating_sub(e.at);
        let when = match age {
            0..60 => "just now".to_string(),
            60..3600 => format!("{} min ago", age / 60),
            3600..86400 => format!("{} h ago", age / 3600),
            _ => format!("{} d ago", age / 86400),
        };
        let mut it = Item::new(title, when, Icon::glyph("📋"), Act::Clip(e.text.clone()));
        it.hay = e.text.chars().take(500).collect();
        it
    };
    let q = query.trim();
    if q.is_empty() {
        return entries.iter().take(max).map(row).collect();
    }
    let rows: Vec<Item> = entries.iter().map(row).collect();
    fuzzy(q, rows.iter(), &Frecency::default(), matcher).into_iter().take(max).map(|(_, it)| it.clone()).collect()
}

pub fn emoji(query: &str, max: usize, matcher: &mut Matcher) -> Vec<Item> {
    let row = |e: &'static emojis::Emoji| {
        use emojis::Group as G;
        let group = match e.group() {
            G::SmileysAndEmotion => "Smileys",
            G::PeopleAndBody => "People",
            G::AnimalsAndNature => "Nature",
            G::FoodAndDrink => "Food",
            G::TravelAndPlaces => "Travel",
            G::Activities => "Activities",
            G::Objects => "Objects",
            G::Symbols => "Symbols",
            G::Flags => "Flags",
        };
        let mut it = Item::new(format!("{}  {}", e.as_str(), e.name()), group, Icon::glyph(e.as_str()), Act::Paste(e.as_str().into()));
        it.hay = std::iter::once(e.name()).chain(e.shortcodes()).collect::<Vec<_>>().join(" ");
        it
    };
    let q = query.trim();
    if q.is_empty() {
        return emojis::iter().take(max).map(row).collect();
    }
    let rows: Vec<Item> = emojis::iter().map(row).collect();
    fuzzy(q, rows.iter(), &Frecency::default(), matcher).into_iter().take(max).map(|(_, it)| it.clone()).collect()
}

/// Fills snippet placeholders.
pub fn expand(template: &str, clipboard: &str, (date, time): (String, String)) -> String {
    template.replace("{clipboard}", clipboard).replace("{date}", &date).replace("{time}", &time)
}

struct Deadline(Instant);

impl fend_core::Interrupt for Deadline {
    fn should_interrupt(&self) -> bool {
        Instant::now() > self.0
    }
}

/// Only for input with a digit; time-boxed so `9^9^9` can't freeze the UI.
fn calculate(q: &str) -> Option<String> {
    if !q.bytes().any(|b| b.is_ascii_digit()) || q.starts_with('>') {
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
    fn root_search_modes_and_helpers() {
        let apps: Vec<App> = ["Visual Studio Code", "Visual Studio 2022", "Calculator", "Google Chrome", "Code Writer"]
            .iter()
            .enumerate()
            .map(|(i, n)| App { name: n.to_string(), id: format!("id{i}"), icon: None })
            .collect();
        let mut cfg = crate::config::parse(
            "[[quicklink]]\nname = \"GitHub Search\"\nkeyword = \"gh\"\nurl = \"https://github.com/search?q={query}\"\n\
             [[snippet]]\nname = \"Signature\"\nkeyword = \";sig\"\ntext = \"Cheers, {clipboard}\"",
        )
        .unwrap();
        cfg.max_results = 5;
        let scripts = [crate::scripts::parse("C:\\s\\deploy.ps1", "# @raycast.title Deploy Site\n# @raycast.packageName Web").unwrap()];
        let cat = catalog(&apps, &scripts, &cfg);
        let deploy = &root("deploy", &cat, &cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT))[0];
        assert_eq!((deploy.act.clone(), deploy.subtitle.as_str()), (Act::Script(0), "Web"));
        let file = Item::new("a.txt", "~", Icon::glyph("📄"), Act::File("C:\\a.txt".into()));
        assert_eq!(file.actions(), [Act::File("C:\\a.txt".into()), Act::Reveal("C:\\a.txt".into()), Act::Copy("C:\\a.txt".into())]);
        let mut m = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let mut frec = Frecency::default();

        let r = root("vsc", &cat, &cfg, &frec, &mut m);
        assert_eq!(r[0].title, "Visual Studio Code");
        assert_eq!(r.last().unwrap().subtitle, "Web");
        assert!(r.len() <= 5);

        // Launch history breaks near-ties: "code" matches two apps.
        frec.record("id4", now());
        frec.record("id4", now());
        assert_eq!(root("code", &cat, &cfg, &frec, &mut m)[0].title, "Code Writer");
        assert_eq!(root("", &cat, &cfg, &frec, &mut m)[0].title, "Code Writer");

        // Commands match on their extra words too.
        assert!(root("reboot", &cat, &cfg, &frec, &mut m).iter().any(|i| i.act == Act::Cmd(Cmd::Restart)));
        assert_eq!(root("left half", &cat, &cfg, &frec, &mut m)[0].act, Act::Cmd(Cmd::Snap(commands::Snap::LeftHalf)));

        assert_eq!(root("2+2*3", &cat, &cfg, &frec, &mut m)[0].act, Act::Copy("8".into()));
        assert!(root("chrome", &cat, &cfg, &frec, &mut m).iter().all(|i| !matches!(i.act, Act::Copy(_))));
        let t = Instant::now();
        calculate("9^9^9^9");
        assert!(t.elapsed() < Duration::from_millis(500), "calculator must be time-boxed");

        assert_eq!(root("gh rust gpui", &cat, &cfg, &frec, &mut m)[0].act, Act::Open("https://github.com/search?q=rust%20gpui".into()));
        // A quicklink found by name asks for its argument.
        assert_eq!(root("github", &cat, &cfg, &frec, &mut m)[0].act, Act::Fill("gh ".into()));
        assert_eq!(root("> cmd /k dir", &cat, &cfg, &frec, &mut m)[0].act, Act::Run("cmd /k dir".into()));
        assert_eq!(root(";sig", &cat, &cfg, &frec, &mut m)[0].title, "Signature");
        assert_eq!(expand("Cheers, {clipboard} {date}", "Ann", ("2026-11-03".into(), "10:00".into())), "Cheers, Ann 2026-11-03");

        let clips = [Entry { text: "fn main() {}".into(), at: now() }, Entry { text: "hello world".into(), at: now() - 7200 }];
        let c = clipboard("hel", &clips, 5, &mut m);
        assert_eq!(c[0].act, Act::Clip("hello world".into()));
        assert_eq!(c[0].actions().last(), Some(&Act::DeleteClip("hello world".into())));
        assert_eq!(clipboard("", &clips, 5, &mut m)[1].subtitle, "2 h ago");

        assert!(emoji("thumbs up", 5, &mut m)[0].title.starts_with('👍'));
    }

    #[test]
    fn window_layouts_tile_the_work_area() {
        use windows::Win32::Foundation::RECT;
        use commands::{Snap, layout};
        let wa = RECT { left: 0, top: 40, right: 1920, bottom: 1080 };
        let cur = RECT { left: 100, top: 100, right: 900, bottom: 700 };
        let l = layout(Snap::LeftHalf, wa, cur);
        let r = layout(Snap::RightHalf, wa, cur);
        assert_eq!((l.left, l.right, r.left, r.right), (0, 960, 960, 1920));
        assert_eq!((l.top, l.bottom), (40, 1080));
        let thirds = [Snap::LeftThird, Snap::CenterThird, Snap::RightThird].map(|s| layout(s, wa, cur));
        assert_eq!((thirds[0].right, thirds[1].left, thirds[1].right, thirds[2].left, thirds[2].right), (640, 640, 1280, 1280, 1920));
        let c = layout(Snap::Center, wa, cur);
        assert_eq!((c.right - c.left, c.left), (800, 560));
    }
}
