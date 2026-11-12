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
    design::{Tint, g},
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
    /// Ask Claude; the answer streams into its own view.
    Ask(String),
    /// Put text in the search box (quicklink waiting for its argument).
    Fill(String),
    /// Start extension command (extension index, command index).
    ExtRun(usize, usize),
    /// Call a handler in the running extension view.
    ExtFn(String),
    /// Call a handler with one value (search-bar dropdown `onChange`).
    ExtSet(String, String),
    /// Set a form field (field id, value).
    FormSet(String, String),
    /// Answer an extension's confirmAlert.
    Confirm(bool),
    UninstallExt(String),
    /// Install a store extension by name.
    Install(String),
    /// Pin (true) or unpin a root item, by key.
    Favorite(String, bool),
    /// Leave a root item out of root search, by key.
    Hide(String),
    /// Ask a running app (by process id) to close its windows.
    QuitApp(u32),
    /// End a running app's process.
    ForceQuit(u32),
    /// A row with nothing to do (loading, empty views).
    None,
}

impl Act {
    /// What Enter does, as shown in the footer.
    pub fn label(&self) -> &'static str {
        match self {
            Act::App(_) | Act::Open(_) | Act::File(_) => "Open",
            Act::RunAsAdmin(_) => "Run as Administrator",
            Act::Reveal(_) => "Show in Explorer",
            Act::Run(_) | Act::Script(_) => "Run",
            Act::Ask(_) => "Ask",
            Act::Copy(_) => "Copy",
            Act::Paste(_) | Act::Clip(_) | Act::Snippet(_) => "Paste",
            Act::DeleteClip(_) => "Delete Entry",
            Act::Cmd(Cmd::SearchFiles | Cmd::ClipboardHistory | Cmd::Emoji | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::QuitApps | Cmd::AskAi) => "Open",
            Act::Install(_) => "Install",
            Act::Cmd(_) | Act::ExtRun(..) => "Run Command",
            Act::ExtFn(_) => "Run Action",
            Act::ExtSet(..) | Act::FormSet(..) => "Choose",
            Act::Confirm(true) => "Confirm",
            Act::Confirm(false) => "Cancel",
            Act::UninstallExt(_) => "Uninstall Extension",
            Act::Fill(_) => "Type Query",
            Act::Favorite(_, true) => "Add to Favorites",
            Act::Favorite(_, false) => "Remove from Favorites",
            Act::Hide(_) => "Hide from Root Search",
            Act::QuitApp(_) => "Quit",
            Act::ForceQuit(_) => "Force Quit",
            Act::None => "",
        }
    }

    /// Removes or ends something; the action panel draws these in `danger`.
    pub fn destructive(&self) -> bool {
        matches!(self, Act::DeleteClip(_) | Act::UninstallExt(_) | Act::ForceQuit(_)) || matches!(self, Act::Cmd(c) if c.destructive())
    }
}

#[derive(Clone)]
pub enum Icon {
    Image(Arc<RenderImage>),
    /// A vector icon (a `design::g` name), drawn in a tile.
    Vector(&'static str),
    /// Emoji or other content text.
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
    /// Muted text right after the title.
    pub subtitle: String,
    /// Right-aligned muted text ("Application", "Command", "3 min ago").
    pub accessory: String,
    /// Section header this row sits under, if any.
    pub section: Option<String>,
    pub icon: Icon,
    pub act: Act,
    /// Frecency key; None for rows that aren't learned (calculator, web, clipboard).
    pub key: Option<String>,
    /// Text the fuzzy matcher sees.
    hay: String,
    /// Explicit Ctrl+K actions (extension rows); empty = derived from `act`.
    pub extra: Vec<(String, Act)>,
    /// Root-search item: whether it's a favourite (Ctrl+K offers pin/unpin and hide).
    pub fav: Option<bool>,
    /// Typed alone in root search, puts this item first.
    pub alias: String,
    /// Left out of root search; its alias and hotkey still work.
    pub hidden: bool,
    /// The hue of its icon tile (what kind of thing it is).
    pub tint: Option<Tint>,
}

impl Item {
    pub fn new(title: impl Into<String>, subtitle: impl Into<String>, icon: Icon, act: Act) -> Self {
        let title = title.into();
        Self { hay: title.clone(), title, subtitle: subtitle.into(), accessory: String::new(), section: None, icon, act, key: None, extra: vec![], fav: None, alias: String::new(), hidden: false, tint: None }
    }

    pub fn acc(mut self, accessory: impl Into<String>) -> Self {
        self.accessory = accessory.into();
        self
    }

    pub fn tinted(mut self, tint: Tint) -> Self {
        self.tint = Some(tint);
        self
    }

    pub fn sec(mut self, section: impl Into<String>) -> Self {
        self.section = Some(section.into());
        self
    }

    /// Searchable text beyond the title (subtitle, keywords).
    pub fn with_hay(mut self, hay: String) -> Self {
        self.hay = hay;
        self
    }

    /// The Ctrl+K action panel, labelled: primary action first, then secondary ones.
    pub fn actions(&self) -> Vec<(String, Act)> {
        let mut out = self.own_actions();
        if let (Some(fav), Some(key)) = (self.fav, &self.key) {
            out.extend([Act::Favorite(key.clone(), !fav), Act::Hide(key.clone())].map(|a| (a.label().to_string(), a)));
        }
        out
    }

    fn own_actions(&self) -> Vec<(String, Act)> {
        if !self.extra.is_empty() {
            return self.extra.clone();
        }
        let mut acts = vec![self.act.clone()];
        match &self.act {
            Act::App(id) => acts.extend([Act::RunAsAdmin(id.clone()), Act::Copy(self.title.clone())]),
            Act::File(path) => acts.extend([Act::Reveal(path.clone()), Act::Copy(path.clone())]),
            Act::Open(url) | Act::Run(url) => acts.push(Act::Copy(url.clone())),
            Act::Copy(text) => acts.push(Act::Paste(text.clone())),
            Act::Paste(text) => acts.push(Act::Copy(text.clone())),
            Act::Clip(text) => acts.extend([Act::Copy(text.clone()), Act::DeleteClip(text.clone())]),
            Act::QuitApp(pid) => acts.push(Act::ForceQuit(*pid)),
            _ => {}
        }
        acts.into_iter().map(|a| (a.label().to_string(), a)).collect()
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

/// Everything root search can find, minus what's switched off. Rebuilt when apps or config change.
pub fn catalog(apps: &[App], scripts: &[Script], exts: &[(crate::ext::Extension, Icon)], cfg: &Config) -> Vec<Item> {
    let mut items = all_items(apps, scripts, exts, cfg);
    items.retain(|it| it.key.as_ref().is_none_or(|k| !cfg.launcher.is_disabled(k)));
    items
}

/// Every item, switched off or not, with its alias, hidden and favourite state
/// (the Shortcuts table in Settings).
pub fn all_items(apps: &[App], scripts: &[Script], exts: &[(crate::ext::Extension, Icon)], cfg: &Config) -> Vec<Item> {
    let mut items = items(apps, scripts, exts, cfg);
    for it in &mut items {
        let Some(k) = &it.key else { continue };
        it.fav = Some(cfg.launcher.favorites.contains(k));
        it.hidden = cfg.launcher.hidden.contains(k);
        if let Some(a) = cfg.launcher.aliases.get(k).map(|a| a.trim()).filter(|a| !a.is_empty()) {
            it.alias = a.to_string();
            it.hay = format!("{} {a}", it.hay);
        }
    }
    items
}

fn items(apps: &[App], scripts: &[Script], exts: &[(crate::ext::Extension, Icon)], cfg: &Config) -> Vec<Item> {
    let mut items: Vec<Item> = apps
        .iter()
        .map(|a| {
            let icon = a.icon.clone().map(Icon::Image).unwrap_or(Icon::Vector(g::APP));
            Item::new(&a.name, "", icon, Act::App(a.id.clone())).acc("Application").keyed(a.id.clone())
        })
        .collect();
    for &(cmd, title, words, glyph) in commands::ALL {
        if matches!(cmd, Cmd::ClipboardHistory | Cmd::ClearClipboardHistory) && !cfg.clipboard.enabled
            || cmd == Cmd::SearchFiles && !cfg.files.enabled
        {
            continue;
        }
        items.push(Item::new(title, cmd.subtitle(), Icon::Vector(glyph), Act::Cmd(cmd)).acc("Command").keyed(cmd.key()).words(words).tinted(cmd.tint()));
    }
    for q in &cfg.quicklink {
        let act = if q.url.contains("{query}") {
            Act::Fill(format!("{} ", if q.keyword.is_empty() { q.title() } else { &q.keyword }))
        } else {
            Act::Open(q.url.clone())
        };
        items.push(Item::new(q.title(), &q.keyword, Icon::Vector(g::LINK), act).acc("Quicklink").keyed(format!("ql:{}", q.title())).words(&q.keyword).tinted(Tint::Web));
    }
    for s in &cfg.snippet {
        items.push(Item::new(&s.name, &s.keyword, Icon::Vector(g::EDIT), Act::Snippet(s.text.clone())).acc("Snippet").keyed(format!("sn:{}", s.name)).words(&s.keyword).tinted(Tint::Paste));
    }
    for (i, s) in scripts.iter().enumerate() {
        let icon = s.icon.clone().map(Icon::glyph).unwrap_or(Icon::Vector(g::COMMAND));
        items.push(Item::new(&s.title, &s.package, icon, Act::Script(i)).acc("Script Command").keyed(format!("sc:{}", s.path)));
    }
    for (e, (ext, icon)) in exts.iter().enumerate() {
        for (c, cmd) in ext.commands.iter().enumerate() {
            let mut it = Item::new(&cmd.title, &ext.title, icon.clone(), Act::ExtRun(e, c)).acc("Command").keyed(format!("ext:{}/{}", ext.name, cmd.name)).words(&ext.title);
            it.extra = vec![("Run Command".into(), Act::ExtRun(e, c)), (format!("Uninstall {}", ext.title), Act::UninstallExt(ext.name.clone()))];
            items.push(it);
        }
    }
    items
}

/// Fuzzy-filters rows by their searchable text, best first (extension lists without
/// their own search handler).
pub fn filter(q: &str, rows: Vec<Item>, matcher: &mut Matcher) -> Vec<Item> {
    if q.trim().is_empty() {
        return rows;
    }
    fuzzy(q.trim(), rows.iter(), &Frecency::default(), matcher).into_iter().map(|(_, it)| it.clone()).collect()
}

fn fuzzy<'a>(q: &str, items: impl Iterator<Item = &'a Item>, frec: &Frecency, matcher: &mut Matcher) -> Vec<(f64, &'a Item)> {
    let now = now();
    let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = Vec::new();
    let mut hits: Vec<(f64, &Item)> = items
        .filter_map(|it| {
            let s = pattern.score(Utf32Str::new(&it.hay, &mut buf), matcher)?;
            // Extra search words only help find an item; a match on the visible title ranks higher.
            let in_title = pattern.score(Utf32Str::new(&it.title, &mut buf), matcher).map_or(0., |t| t as f64 / 2.) + tier(&it.title, q);
            let boost = it.key.as_deref().map_or(0., |k| frec.score(k, now).min(10.));
            // ponytail: fixed weight; tune once there is real launch history.
            Some((s as f64 + in_title + 25. * boost, it))
        })
        .collect();
    hits.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.title.len().cmp(&b.1.title.len())));
    hits
}

/// How a query meets a title, strongest first: the whole title, its start, the start of a
/// word, the initials. Scattered letters ("sear*c*h fi*l*es" for "cl") get nothing extra.
fn tier(title: &str, q: &str) -> f64 {
    let (t, q) = (title.to_lowercase(), q.to_lowercase());
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    if t == q {
        1000.
    } else if t.starts_with(&q) {
        600.
    } else if words.iter().any(|w| w.starts_with(&q)) {
        400.
    } else if initials.starts_with(&q.replace(' ', "")) && q.len() > 1 {
        300.
    } else {
        0.
    }
}

/// The calculator's answer as shown: no "approx.", at most four decimals.
pub fn tidy_number(answer: &str) -> String {
    let a = answer.trim_start_matches("approx. ").trim();
    let (num, rest) = a.split_once(' ').unwrap_or((a, ""));
    let shown = match num.parse::<f64>() {
        Ok(n) if num.split_once('.').is_some_and(|(_, d)| d.len() > 4) => {
            let s = format!("{n:.4}");
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        }
        _ => num.to_string(),
    };
    if rest.is_empty() { shown } else { format!("{shown} {rest}") }
}

pub fn root(query: &str, catalog: &[Item], cfg: &Config, frec: &Frecency, matcher: &mut Matcher) -> Vec<Item> {
    let q = query.trim();
    let max = cfg.max_results.max(1);

    if q.is_empty() {
        let now = now();
        let favorites = favorites(catalog, cfg);
        let mut recent: Vec<_> = catalog
            .iter()
            .filter(|it| it.fav != Some(true) && !it.hidden)
            .filter_map(|it| Some((frec.score(it.key.as_deref()?, now), it)))
            .filter(|(s, _)| *s > 0.01)
            .collect();
        recent.sort_by(|a, b| b.0.total_cmp(&a.0));
        if recent.is_empty() && favorites.is_empty() {
            // First run: point at a few things worth knowing about.
            let starters = [Cmd::ClipboardHistory, Cmd::SearchFiles, Cmd::Emoji, Cmd::BrowseStore, Cmd::Settings];
            return catalog.iter().filter(|it| matches!(it.act, Act::Cmd(c) if starters.contains(&c))).map(|it| it.clone().sec("Suggestions")).collect();
        }
        let favorites = favorites.into_iter().map(|it| it.sec("Favorites"));
        return favorites.chain(recent.into_iter().take(max).map(|(_, it)| it.clone().sec("Recent"))).collect();
    }

    let mut top = Vec::new();
    if let Some(cmdline) = q.strip_prefix('>').map(str::trim).filter(|c| !c.is_empty()) {
        top.push(Item::new(cmdline, "", Icon::Vector(g::COMMAND), Act::Run(cmdline.to_string())).acc("Windows Run"));
    }
    if let Some((kw, rest)) = q.split_once(' ')
        && let Some(link) = cfg.quicklink.iter().find(|l| !l.keyword.is_empty() && l.keyword.eq_ignore_ascii_case(kw))
        && !rest.trim().is_empty()
    {
        let url = link.url.replace("{query}", &url_encode(rest.trim()));
        top.push(Item::new(rest.trim(), link.title(), Icon::Vector(g::LINK), Act::Open(url)).acc("Quicklink").keyed(format!("ql:{}", link.title())));
    }
    if cfg.search.calculator
        && let Some(result) = calculate(q)
    {
        let full = result.trim_start_matches("approx. ").to_string();
        top.push(Item::new(tidy_number(&result), q, Icon::Vector(g::CALCULATOR), Act::Copy(full)).acc("Calculator").tinted(Tint::Calc));
    }

    // Fallback commands: the web search and quicklinks that take a query, in the configured order.
    let fallbacks: Vec<Item> = cfg
        .search
        .fallbacks
        .iter()
        .filter_map(|name| {
            if name.eq_ignore_ascii_case("ai") {
                return Some(Item::new("Ask AI", "", Icon::Vector(g::LIGHTBULB), Act::Ask(q.to_string())).acc("AI").tinted(Tint::Web).sec(format!("Use \"{q}\" with")));
            }
            let (title, url, glyph, kind) = if name.eq_ignore_ascii_case("web") {
                (!cfg.search.fallback.is_empty()).then(|| ("Search the web".to_string(), cfg.search.fallback.clone(), g::GLOBE, "Web"))?
            } else if name.eq_ignore_ascii_case("chatgpt") {
                ("Search with ChatGPT".to_string(), "https://chatgpt.com/?q={query}".to_string(), g::SEARCH, "Web")
            } else {
                let l = cfg.quicklink.iter().find(|l| l.title().eq_ignore_ascii_case(name) && l.url.contains("{query}"))?;
                (l.title().to_string(), l.url.clone(), g::LINK, "Quicklink")
            };
            Some(Item::new(title, "", Icon::Vector(glyph), Act::Open(url.replace("{query}", &url_encode(q)))).acc(kind).tinted(Tint::Web).sec(format!("Use \"{q}\" with")))
        })
        .collect();
    // An alias typed exactly puts its item first, hidden or not.
    let aliased: Vec<&Item> = catalog.iter().filter(|it| !it.alias.is_empty() && it.alias.eq_ignore_ascii_case(q)).collect();
    top.splice(0..0, aliased.iter().map(|it| (*it).clone()));
    let room = max.saturating_sub(top.len() + fallbacks.len());
    let hits = fuzzy(q, catalog.iter().filter(|it| !it.hidden && !aliased.iter().any(|a| std::ptr::eq(*a, *it))), frec, matcher);
    let best = hits.first().map_or(0., |h| h.0);
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    let sensitive = |(score, it): &(f64, &Item)| match cfg.launcher.sensitivity.as_str() {
        // Every word appears as typed (case aside).
        "low" => {
            let hay = it.hay.to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        }
        "high" => true,
        // Drop the weakest fuzzy matches.
        _ => *score >= best * 0.3,
    };
    top.extend(hits.into_iter().filter(sensitive).take(room).map(|(_, it)| it.clone()));
    for it in &mut top {
        it.section = Some("Results".into());
    }
    top.extend(fallbacks);
    top.truncate(max);
    top
}

/// The pinned items, in the order they were pinned.
pub fn favorites(catalog: &[Item], cfg: &Config) -> Vec<Item> {
    cfg.launcher.favorites.iter().filter_map(|k| catalog.iter().find(|it| !it.hidden && it.key.as_ref() == Some(k)).cloned()).collect()
}

/// What a clipboard entry looks like it is, for its icon and subtitle.
fn clip_kind(text: &str) -> (&'static str, &'static str) {
    let t = text.trim();
    let one_line = !t.contains('\n');
    if one_line && (t.starts_with("http://") || t.starts_with("https://")) {
        (g::LINK, "Link")
    } else if one_line && t.contains('@') && !t.contains(' ') && t.contains('.') {
        (g::MAIL, "Email")
    } else if one_line && t.starts_with('#') && matches!(t.len(), 4 | 7 | 9) && t[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        (g::PERSONALIZE, "Colour")
    } else if one_line && t.len() > 3 && t.as_bytes()[1] == b':' && t.as_bytes()[2] == b'\\' {
        (g::FOLDER, "Path")
    } else if t.lines().count() > 1 && t.contains(['{', ';', '(']) {
        (g::CODE, "Code")
    } else if t.lines().count() > 1 {
        (g::DOCUMENT, "Text")
    } else {
        (g::DOCUMENT, "")
    }
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
        let (glyph, kind) = clip_kind(&e.text);
        let mut it = Item::new(title, kind, Icon::Vector(glyph), Act::Clip(e.text.clone())).acc(when).sec(if age < 86400 { "Today" } else { "Earlier" }).tinted(Tint::Paste);
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
        let mut it = Item::new(e.name(), "", Icon::glyph(e.as_str()), Act::Paste(e.as_str().into())).acc(group);
        it.hay = std::iter::once(e.name()).chain(e.shortcodes()).collect::<Vec<_>>().join(" ");
        it
    };
    let q = query.trim();
    if q.is_empty() {
        return emojis::iter().take(max).map(|e| { let it = row(e); let s = it.accessory.clone(); it.sec(s) }).collect();
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

struct Rates;

impl fend_core::ExchangeRateFnV2 for Rates {
    fn relative_to_base_currency(&self, currency: &str, _: &fend_core::ExchangeRateFnV2Options) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
        crate::net::rate_of(currency)
    }
}

/// Only for input with a digit; time-boxed so `9^9^9` can't freeze the UI.
fn calculate(q: &str) -> Option<String> {
    if !q.bytes().any(|b| b.is_ascii_digit()) || q.starts_with('>') {
        return None;
    }
    let mut ctx = fend_core::Context::new();
    // Currencies (e.g. "50 usd to inr") use the cached daily ECB rates.
    ctx.set_exchange_rate_handler_v2(Rates);
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
    fn destructive_actions_are_the_ones_that_remove_or_end_something() {
        assert!(Act::DeleteClip("x".into()).destructive() && Act::ForceQuit(1).destructive() && Act::Cmd(Cmd::ShutDown).destructive());
        assert!(!Act::Copy("x".into()).destructive() && !Act::Hide("k".into()).destructive() && !Act::Cmd(Cmd::Settings).destructive());
    }

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
        let ext = crate::ext::parse_manifest(
            std::path::Path::new("C:\\e"),
            &serde_json::json!({"name": "notes", "title": "Notes", "commands": [
                {"name": "search", "title": "Search Notes", "mode": "view"},
                {"name": "bar", "title": "Notes Bar", "mode": "menu-bar"}]}),
        )
        .unwrap();
        let cat = catalog(&apps, &scripts, &[(ext, Icon::glyph("•"))], &cfg);
        let notes: Vec<_> = cat.iter().filter(|i| i.subtitle == "Notes").collect();
        assert_eq!(notes.len(), 2, "menu-bar commands open in the launcher");
        assert_eq!((notes[0].title.as_str(), &notes[0].act), ("Search Notes", &Act::ExtRun(0, 0)));
        assert_eq!(notes[0].actions()[1], ("Uninstall Notes".to_string(), Act::UninstallExt("notes".into())));
        let deploy = &root("deploy", &cat, &cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT))[0];
        assert_eq!((deploy.act.clone(), deploy.subtitle.as_str()), (Act::Script(0), "Web"));
        let file = Item::new("a.txt", "~", Icon::glyph("📄"), Act::File("C:\\a.txt".into()));
        let acts: Vec<Act> = file.actions().into_iter().map(|a| a.1).collect();
        assert_eq!(acts, [Act::File("C:\\a.txt".into()), Act::Reveal("C:\\a.txt".into()), Act::Copy("C:\\a.txt".into())]);
        let mut m = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let mut frec = Frecency::default();

        let r = root("vsc", &cat, &cfg, &frec, &mut m);
        assert_eq!(r[0].title, "Visual Studio Code");
        assert_eq!(r.last().unwrap().accessory, "AI");
        assert_eq!(r[0].section.as_deref(), Some("Results"));
        assert_eq!(r.last().unwrap().section.as_deref(), Some("Use \"vsc\" with"));
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
        crate::net::set_rates([("EUR".to_string(), 1.0), ("USD".to_string(), 1.1216), ("INR".to_string(), 98.5)].into());
        let fx = calculate("10 eur to usd").unwrap_or_default();
        assert!(fx.starts_with("11.21") && fx.contains("USD"), "currency conversion: {fx}");
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
        assert_eq!(c[0].actions().last().map(|a| &a.1), Some(&Act::DeleteClip("hello world".into())));
        assert_eq!(clipboard("", &clips, 5, &mut m)[1].accessory, "2 h ago");

        assert_eq!(emoji("thumbs up", 5, &mut m)[0].act, Act::Paste("👍".into()));
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

    #[test]
    fn launcher_favorites_hidden_sensitivity_and_fallbacks() {
        let apps: Vec<App> = ["Notepad", "Paint", "Photos"].iter().map(|n| App { name: n.to_string(), id: format!("app:{n}"), icon: None }).collect();
        let mut cfg = crate::config::parse(
            "[launcher]\nfavorites = [\"app:Photos\"]\nhidden = [\"app:Paint\"]\ndisabled = [\"cmd:Lock\", \"ext:todo\"]\n\
             aliases = { \"app:Paint\" = \"pt\", \"cmd:Emoji\" = \"em\" }\n\
             [search]\nfallbacks = [\"GitHub\", \"chatgpt\", \"web\", \"ai\"]\n\
             [[quicklink]]\nname = \"GitHub\"\nurl = \"https://github.com/search?q={query}\"",
        )
        .unwrap();
        let cat = catalog(&apps, &[], &[], &cfg);
        let mut m = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let frec = Frecency::default();
        assert!(root("paint", &cat, &cfg, &frec, &mut m).iter().all(|it| it.title != "Paint"), "hidden items leave root search");
        assert_eq!(root("PT", &cat, &cfg, &frec, &mut m)[0].title, "Paint", "...but their alias still finds them");
        assert_eq!(root("em", &cat, &cfg, &frec, &mut m)[0].act, Act::Cmd(Cmd::Emoji), "alias goes first");
        assert!(cat.iter().all(|it| it.act != Act::Cmd(Cmd::Lock)), "disabled items leave the catalog");
        assert!(cfg.launcher.is_disabled("ext:todo/list") && !cfg.launcher.is_disabled("ext:todos/list"), "a whole extension is switched off by name");

        let empty = root("", &cat, &cfg, &frec, &mut m);
        assert_eq!((empty[0].title.as_str(), empty[0].section.as_deref()), ("Photos", Some("Favorites")));
        let acts: Vec<String> = empty[0].actions().into_iter().map(|a| a.0).collect();
        assert!(acts.contains(&"Remove from Favorites".to_string()) && acts.contains(&"Hide from Root Search".to_string()));

        let hits = root("ntpd", &cat, &cfg, &frec, &mut m);
        assert_eq!(hits[0].title, "Notepad");
        let tail: Vec<&str> = hits.iter().filter(|h| h.section.as_deref() == Some("Use \"ntpd\" with")).map(|h| h.title.as_str()).collect();
        assert_eq!(tail, ["GitHub", "Search with ChatGPT", "Search the web", "Ask AI"], "fallbacks in configured order");
        let gpt = root("a b", &cat, &cfg, &frec, &mut m).into_iter().find(|h| h.title == "Search with ChatGPT").unwrap();
        assert_eq!(gpt.act, Act::Open("https://chatgpt.com/?q=a%20b".into()), "the query is encoded into the URL");
        assert!(root("a b", &cat, &cfg, &frec, &mut m).iter().any(|h| h.act == Act::Ask("a b".into())), "Ask AI takes the query as typed");
        assert_eq!(root("ai", &cat, &cfg, &frec, &mut m)[0].act, Act::Cmd(Cmd::AskAi), "typing ai finds the command first");

        cfg.launcher.sensitivity = "low".into();
        assert!(root("ntpd", &cat, &cfg, &frec, &mut m).iter().all(|h| h.title != "Notepad"), "low sensitivity wants the words as typed");
        assert_eq!(root("note", &cat, &cfg, &frec, &mut m)[0].title, "Notepad");
    }

    #[test]
    fn word_starts_beat_scattered_letters() {
        let apps: Vec<App> = ["Clock", "Claude"].iter().map(|n| App { name: n.to_string(), id: format!("app:{n}"), icon: None }).collect();
        let cfg = crate::config::parse("").unwrap();
        let cat = catalog(&apps, &[], &[], &cfg);
        let mut m = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let titles: Vec<String> = root("cl", &cat, &cfg, &Frecency::default(), &mut m).into_iter().map(|h| h.title).collect();
        let at = |t: &str| titles.iter().position(|x| x == t).unwrap_or(usize::MAX);
        assert!(at("Clipboard History") < at("Search Files") && at("Clock") < at("Search Files"), "{titles:?}");
        assert_eq!(tier("Quit Applications", "qa"), 300., "initials");
        assert_eq!(tidy_number("approx. 3.1068559612 miles"), "3.1069 miles");
        assert_eq!(tidy_number("42"), "42");
        assert_eq!(clip_kind("https://example.com").1, "Link");
        assert_eq!(clip_kind("#7c6cff").1, "Colour");
        assert_eq!(clip_kind(r"C:\Users\me").1, "Path");
        assert_eq!(clip_kind("me@example.com").1, "Email");
    }

    #[test]
    fn every_builtin_command_is_found_by_its_title() {
        let cfg = crate::config::parse("").unwrap();
        let cat = catalog(&[], &[], &[], &cfg);
        let mut m = Matcher::new(nucleo_matcher::Config::DEFAULT);
        for &(cmd, title, ..) in commands::ALL {
            let hits = root(title, &cat, &cfg, &Frecency::default(), &mut m);
            assert_eq!(hits.first().map(|h| &h.act), Some(&Act::Cmd(cmd)), "{title}");
        }
    }
}
