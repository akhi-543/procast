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
    commands::{self, Cmd, Desk, FocusCmd},
    desktops,
    config::Config,
    design::{Tint, g},
    scripts::Script,
    translate::{self, After, Job},
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
    /// Clipboard history entries, by `Entry::key`: paste, copy, pin (true) or unpin, delete.
    Clip(String),
    CopyClip(String),
    PinClip(String, bool),
    DeleteClip(String),
    /// Snippet template, expanded at paste time.
    Snippet(String),
    /// Index into the launcher's script list.
    Script(usize),
    Cmd(Cmd),
    /// Run a window layout preset, by name.
    Layout(String),
    /// Ask Claude; the answer streams into its own view.
    Ask(String),
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
    /// Switch to a browser tab: (browser connection, tab id).
    Tab(u32, i64),
    /// Start a timer: seconds and label.
    StartTimer(u64, String),
    /// Cancel a timer, or stop a stopwatch and copy its time, by id.
    StopTimer(u64),
    /// A running timer or stopwatch, by id: pause, resume, start over, add seconds, rename.
    PauseTimer(u64),
    ResumeTimer(u64),
    RestartTimer(u64),
    AddTime(u64, u64),
    RenameTimer(u64),
    /// Start a Focus session with a category's lists (by name), blocking them or allowing only them.
    FocusStart(String, crate::focus::Mode),
    /// Open a meeting link: event title, address. Asks "Join <title>?" first when `[calendar] confirm_join`.
    Join(String, String),
    /// A screenshot (by path) put on the clipboard as an image: copy, or copy and paste.
    CopyShot(String),
    PasteShot(String),
    /// Look a word up in the offline dictionary; the answer opens in its own view.
    Define(String),
    /// Translate with Google Translate; the result opens in its own view, is copied, or pastes over the selection.
    Translate(Job),
    /// The translator's language sets, by index (0 is Settings → Translator's): open the bar's
    /// menu, make one current, add one (None) or edit one, delete one (asks first).
    LangMenu,
    UseLangSet(usize),
    EditLangSet(Option<usize>),
    DeleteLangSet(usize),
    /// A row's own act with its argument boxes' values (`with_args`), for the acts that take them as they are.
    With(Box<Act>, Vec<String>),
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
            Act::With(act, _) => act.label(),
            Act::Cmd(Cmd::StartTimer | Cmd::StartStopwatch | Cmd::Focus(FocusCmd::Start)) => "Start",
            Act::Cmd(Cmd::Translate) => "Translate",
            Act::Cmd(Cmd::Define) => "Define",
            Act::Cmd(Cmd::Desktop(Desk::Rename)) => "Rename",
            Act::Copy(_) | Act::CopyClip(_) => "Copy",
            Act::CopyShot(_) => "Copy Image",
            Act::PasteShot(_) => "Paste Image",
            Act::PinClip(_, true) => "Pin Entry",
            Act::PinClip(_, false) => "Unpin Entry",
            Act::Paste(_) | Act::Clip(_) | Act::Snippet(_) => "Paste",
            Act::DeleteClip(_) => "Delete Entry",
            Act::Cmd(Cmd::SearchFiles | Cmd::ClipboardHistory | Cmd::Emoji | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::QuitApps | Cmd::AskAi | Cmd::SearchScreenshots) => "Open",
            Act::Install(_) => "Install",
            Act::Cmd(_) | Act::ExtRun(..) | Act::Layout(_) => "Run Command",
            Act::ExtFn(_) => "Run Action",
            Act::ExtSet(..) | Act::FormSet(..) => "Choose",
            Act::Confirm(true) => "Confirm",
            Act::Confirm(false) => "Cancel",
            Act::UninstallExt(_) => "Uninstall Extension",
            Act::Favorite(_, true) => "Add to Favorites",
            Act::Favorite(_, false) => "Remove from Favorites",
            Act::Hide(_) => "Hide from Root Search",
            Act::QuitApp(_) => "Quit",
            Act::Tab(..) => "Switch to Tab",
            Act::ForceQuit(_) => "Force Quit",
            Act::StartTimer(..) => "Start Timer",
            Act::StopTimer(_) => "Stop",
            Act::PauseTimer(_) => "Pause",
            Act::ResumeTimer(_) => "Resume",
            Act::RestartTimer(_) => "Restart",
            Act::AddTime(..) => "Add Time",
            Act::RenameTimer(_) => "Rename",
            Act::Join(..) => "Join Meeting",
            Act::FocusStart(_, crate::focus::Mode::Block) => "Start Focus Session",
            Act::FocusStart(_, crate::focus::Mode::Allow) => "Start Allow-Only Focus Session",
            Act::Translate(_) => "Translate",
            Act::Define(_) => "Define",
            Act::LangMenu => "Change Target",
            Act::UseLangSet(_) => "Make Current",
            Act::EditLangSet(None) => "Add Language Set",
            Act::EditLangSet(Some(_)) => "Edit",
            Act::DeleteLangSet(_) => "Delete",
            Act::None => "",
        }
    }

    /// Removes or ends something; the action panel draws these in `danger`.
    pub fn destructive(&self) -> bool {
        matches!(self, Act::DeleteClip(_) | Act::UninstallExt(_) | Act::ForceQuit(_) | Act::DeleteLangSet(_)) || matches!(self, Act::Cmd(c) if c.destructive())
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

/// What an argument box takes.
#[derive(Clone, Debug, PartialEq)]
pub enum ArgKind {
    Text,
    /// Drawn as dots.
    Secret,
    /// A fixed list, (id, label), opened as a menu under the box. An empty id is a row
    /// that cannot be picked yet.
    Choice(Vec<(String, String)>),
}

/// One argument box in the search bar (a row has at most `MAX_ARGS`).
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub name: SharedString,
    pub placeholder: SharedString,
    pub kind: ArgKind,
    pub optional: bool,
    /// One word only: a space moves on to the next box.
    pub word: bool,
    /// The help mark's tooltip; empty = no mark.
    pub help: String,
    /// What the action bar says when Enter finds this box empty.
    pub missing: String,
}

impl Arg {
    /// A required text box whose placeholder is its name.
    pub fn new(name: impl Into<SharedString>) -> Self {
        let name = name.into();
        Arg { missing: format!("Type {}", name.to_lowercase()), placeholder: name.clone(), name, kind: ArgKind::Text, optional: false, word: false, help: String::new() }
    }

    fn optional(name: impl Into<SharedString>) -> Self {
        Arg { optional: true, ..Arg::new(name) }
    }
}

/// Raycast's limit, and what fits in the bar.
pub const MAX_ARGS: usize = 3;

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
    /// Argument boxes shown in the search bar while this row is selected.
    pub args: Vec<Arg>,
    /// The boxes' values as the query filled them (`prefill`).
    pub values: Vec<String>,
    /// Words that, typed with a space after them, name this row like its title does ("timer").
    pub keywords: Vec<String>,
    /// The accessory ticks (a timer's time left): tabular figures at a fixed width, shown in root too.
    pub live: bool,
}

impl Item {
    pub fn new(title: impl Into<String>, subtitle: impl Into<String>, icon: Icon, act: Act) -> Self {
        let title = title.into();
        Self {
            hay: title.clone(),
            title,
            subtitle: subtitle.into(),
            accessory: String::new(),
            section: None,
            icon,
            act,
            key: None,
            extra: vec![],
            fav: None,
            alias: String::new(),
            hidden: false,
            tint: None,
            args: vec![],
            values: vec![],
            keywords: vec![],
            live: false,
        }
    }

    /// Argument boxes (the first `MAX_ARGS`) and the keywords that jump into them.
    pub fn boxes(mut self, mut args: Vec<Arg>, keywords: &[&str]) -> Self {
        args.truncate(MAX_ARGS);
        self.args = args;
        self.keywords = keywords.iter().filter(|k| !k.is_empty()).map(|k| k.to_string()).collect();
        self
    }

    /// "3 arguments": the row's accessory while it is not selected.
    pub fn arg_count(&self) -> Option<String> {
        match self.args.len() {
            0 => None,
            1 => Some("1 argument".into()),
            n => Some(format!("{n} arguments")),
        }
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
            Act::Open(url) | Act::Run(url) | Act::Join(_, url) => acts.push(Act::Copy(url.clone())),
            Act::Copy(text) => acts.push(Act::Paste(text.clone())),
            Act::Paste(text) => acts.push(Act::Copy(text.clone())),
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

/// What follows the row's title, alias or a keyword and a space at the start of `q` ("timer 5m"
/// gives "5m", "timer " gives ""); None when `q` does not start by naming the row.
pub fn named<'a>(item: &Item, q: &'a str) -> Option<&'a str> {
    let mut names: Vec<&str> = [item.title.as_str(), item.alias.as_str()].into_iter().chain(item.keywords.iter().map(String::as_str)).filter(|n| !n.is_empty()).collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    names.into_iter().find_map(|n| q.get(..n.len()).filter(|head| head.eq_ignore_ascii_case(n)).and_then(|_| q[n.len()..].strip_prefix(' ')))
}

/// `named`, except that "focus " (also Search Focus Categories' prefix) does not name Start
/// Focus Session while a focus category's name contains the rest: the category rows win.
pub fn named_in<'a>(item: &Item, q: &'a str, cfg: &Config) -> Option<&'a str> {
    let rest = named(item, q)?;
    let by_focus = q.get(..6).is_some_and(|h| h.eq_ignore_ascii_case("focus ")) && item.act == Act::Cmd(Cmd::Focus(FocusCmd::Start));
    let rest_lc = rest.trim().to_lowercase();
    (!(by_focus && cfg.focus_category.iter().any(|c| c.name.to_lowercase().contains(&rest_lc)))).then_some(rest)
}

/// Whether `q` names this row so that it leads root search with its boxes: its title or a
/// keyword of two letters or more alone ("timer"), or either followed by a space and values
/// ("timer 5m", "t 90").
pub fn leads(item: &Item, q: &str, cfg: &Config) -> bool {
    !item.args.is_empty() && (named_in(item, q, cfg).is_some() || exact(item, q))
}

/// Whether this row heads root search above every section: an alias typed exactly, else a
/// row that `leads`. An alias always wins over a keyword ("t" for Terminal beats "t 90").
pub fn heads(item: &Item, q: &str, cfg: &Config) -> bool {
    alias_is(item, q) || leads(item, q, cfg)
}

/// `q` is the row's alias, case aside.
fn alias_is(item: &Item, q: &str) -> bool {
    !item.alias.is_empty() && item.alias.eq_ignore_ascii_case(q.trim())
}

/// `q` is the row's title or one of its keywords of two letters or more, case aside. A
/// one-letter keyword alone ("t") is just the start of a word.
fn exact(item: &Item, q: &str) -> bool {
    let q = q.trim();
    !q.is_empty() && (item.title.eq_ignore_ascii_case(q) || (q.chars().count() > 1 && item.keywords.iter().any(|k| k.eq_ignore_ascii_case(q))))
}

/// The boxes' values from the rest of a query that names the row: a one-word box takes a
/// word, a text box the rest. A list's own word leading the rest goes to that list's box
/// when more text follows ("tr de hola"). Trailing empty boxes are left out.
pub fn prefill(item: &Item, query: &str) -> Vec<String> {
    let Some(rest) = named(item, query) else { return vec![] };
    let mut rest = rest.trim();
    let mut out = vec![String::new(); item.args.len()];
    for (i, a) in item.args.iter().enumerate() {
        if let ArgKind::Choice(opts) = &a.kind
            && let Some((w, more)) = rest.split_once(' ')
            && let Some((_, label)) = opts.iter().find(|(id, l)| !id.is_empty() && (id.eq_ignore_ascii_case(w) || l.eq_ignore_ascii_case(w)))
        {
            out[i] = label.clone();
            rest = more.trim_start();
        }
    }
    for (i, a) in item.args.iter().enumerate() {
        if rest.is_empty() {
            break;
        }
        match a.kind {
            ArgKind::Choice(_) => {}
            _ if a.word => {
                let (w, more) = rest.split_once(' ').unwrap_or((rest, ""));
                out[i] = w.to_string();
                rest = more.trim_start();
            }
            _ => out[i] = std::mem::take(&mut rest).to_string(),
        }
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// What Enter does with the boxes' values: the act to run, or (box, message) for the first
/// required box that is empty or does not parse. A list's label becomes its id.
pub fn with_args(item: &Item, values: &[String]) -> Result<Act, (usize, String)> {
    let v: Vec<String> = item
        .args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let v = values.get(i).map_or("", |s| s.trim());
            match &a.kind {
                ArgKind::Choice(opts) => opts.iter().find(|(id, l)| !id.is_empty() && (l.eq_ignore_ascii_case(v) || id.eq_ignore_ascii_case(v))).map_or(v, |o| o.0.as_str()),
                _ => v,
            }
            .to_string()
        })
        .collect();
    let missing = |i: usize| (i, item.args[i].missing.clone());
    if let Some(i) = item.args.iter().zip(&v).position(|(a, v)| !a.optional && v.is_empty()) {
        return Err(missing(i));
    }
    Ok(match &item.act {
        Act::Cmd(Cmd::StartTimer) => Act::StartTimer(crate::timers::parse_duration(&v[0]).ok_or_else(|| missing(0))?.as_secs(), v[1].clone()),
        // Empty from and to: the configured languages.
        Act::Cmd(Cmd::Translate) => Act::Translate(Job { text: v[0].clone(), from: String::new(), to: v[1..].iter().filter(|l| !l.is_empty()).map(|l| translate::language(l)).collect(), all: true, after: After::Open }),
        Act::Cmd(Cmd::Define) => Act::Define(v[0].clone()),
        Act::Cmd(Cmd::Focus(FocusCmd::Start | FocusCmd::Edit)) => {
            crate::focus::parse_start(&[String::new(), v[1].clone()], &[], 0).map_err(|e| (1, e))?;
            Act::With(Box::new(item.act.clone()), v)
        }
        Act::Open(url) => Act::Open(url.replace("{query}", &url_encode(&v[0]))),
        act => Act::With(Box::new(act.clone()), v),
    })
}

/// The row's subtitle while its boxes hold something: the values as typed (a timer's
/// duration as parsed, "25 min"), " · " between them (a secret never shows). Empty when
/// nothing is filled.
pub fn mirror(item: &Item, values: &[String]) -> String {
    let shown = |i: usize, v: &str| match crate::timers::parse_duration(v) {
        Some(d) if i == 0 && item.act == Act::Cmd(Cmd::StartTimer) => crate::timers::fmt_spoken(d),
        _ => v.to_string(),
    };
    item.args.iter().zip(values).enumerate().filter(|(_, (a, v))| a.kind != ArgKind::Secret && !v.trim().is_empty()).map(|(i, (_, v))| shown(i, v.trim())).collect::<Vec<_>>().join(" · ")
}

/// A built-in command's boxes and the keywords that jump into them.
fn command_args(cmd: Cmd, cfg: &Config) -> (Vec<Arg>, &'static [&'static str]) {
    let named = || Arg { placeholder: "Type name".into(), ..Arg::new("Name") };
    match cmd {
        Cmd::StartTimer => (vec![Arg { word: true, missing: "Type a time: 5m, 1h30m, 1:30".into(), ..Arg::new("Duration") }, Arg::optional("Name")], &["timer", "t"]),
        Cmd::StartStopwatch => (vec![Arg::optional("Name")], &[]),
        Cmd::Translate => {
            let to = ArgKind::Choice(translate::LANGUAGES.iter().map(|(code, name)| (code.to_string(), name.to_string())).collect());
            // With the selection to fall back on, the text may stay empty.
            let text = Arg { optional: cfg.translator.fill_selection, missing: "Type the text to translate".into(), ..Arg::new("Text") };
            (vec![text, Arg { kind: to, ..Arg::optional("To") }], &["translate", "tr"])
        }
        Cmd::Define => (vec![Arg { word: true, missing: "Type a word to define".into(), ..Arg::new("Word") }], &["define", "def"]),
        Cmd::Focus(f @ (FocusCmd::Start | FocusCmd::Edit)) => {
            let mut cats: Vec<(String, String)> = cfg.focus_category.iter().map(|c| (c.name.clone(), c.name.clone())).collect();
            // ponytail: not pickable until the app picker (feature screens Task 12).
            cats.push((String::new(), "Applications…".into()));
            let args = vec![
                Arg { help: "What are you focusing on?".into(), ..Arg::optional("Goal") },
                Arg { word: true, help: format!("0 for no limit; empty: {} min", cfg.focus.default_minutes), ..Arg::optional("Minutes") },
                Arg { kind: ArgKind::Choice(cats), help: "A category to block; add \"allow\" to allow only it".into(), ..Arg::optional("Block") },
            ];
            (args, if f == FocusCmd::Start { &["focus"] } else { &[] })
        }
        Cmd::Focus(FocusCmd::CreateCategory) => (vec![Arg { missing: "Type a name for the category".into(), ..named() }, Arg { help: "Comma separated, e.g. discord.exe, slack.exe".into(), ..Arg::optional("Apps") }, Arg { help: "Comma separated, e.g. reddit.com, x.com".into(), ..Arg::optional("Sites") }], &[]),
        Cmd::Focus(FocusCmd::ImportCategories) => (vec![Arg { help: "A Raycast focus categories .json file".into(), missing: "Type the path to a .json file".into(), ..Arg::new("File") }], &[]),
        Cmd::Desktop(Desk::Rename) => (vec![Arg { missing: "Type a name for the desktop".into(), ..named() }], &[]),
        _ => (vec![], &[]),
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
            || cmd == Cmd::Define && !cfg.dictionary.enabled
            || cmd == Cmd::SearchFiles && !cfg.files.enabled
            || matches!(cmd, Cmd::Desktop(_)) && !desktops::supported()
        {
            continue;
        }
        let (args, keywords) = command_args(cmd, cfg);
        items.push(Item::new(title, cmd.subtitle(), Icon::Vector(glyph), Act::Cmd(cmd)).acc("Command").keyed(cmd.key()).words(words).tinted(cmd.tint()).boxes(args, keywords));
    }
    for &(title, page, words) in crate::system::SETTINGS_PAGES {
        let it = Item::new(title, "Windows Settings", Icon::Vector(g::SETTING), Act::Open(format!("ms-settings:{page}")));
        items.push(it.acc("Windows Settings").keyed(format!("ms:{page}")).words(&format!("settings {words}")).tinted(Tint::System));
    }
    for l in &cfg.window.layouts {
        items.push(Item::new(format!("Layout: {}", l.name), "Window", Icon::Vector(g::TILES), Act::Layout(l.name.clone())).acc("Command").keyed(format!("wl:{}", l.name)).words("window arrange preset").tinted(Tint::Window));
    }
    for q in &cfg.quicklink {
        let mut it = if q.url.contains("{query}") {
            let query = Arg { missing: "Type a search query".into(), ..Arg::new("Query") };
            let mut it = Item::new(q.title(), &q.keyword, Icon::Vector(g::LINK), Act::Open(q.url.clone())).boxes(vec![query], &[q.keyword.as_str()]);
            // The text selected in the app you came from, as the argument.
            let with_selection = Act::Open(q.url.replace("{query}", "{selection}"));
            it.extra = [(it.act.clone(), it.act.label()), (with_selection, "Open with Selected Text"), (Act::Copy(q.url.clone()), "Copy")].map(|(a, l)| (l.to_string(), a)).to_vec();
            it
        } else {
            Item::new(q.title(), &q.keyword, Icon::Vector(g::LINK), Act::Open(q.url.clone()))
        };
        it = it.acc("Quicklink").keyed(format!("ql:{}", q.title())).words(&q.keyword).tinted(Tint::Web);
        items.push(it);
    }
    for t in &cfg.translate {
        let title = crate::translate::command_title(&t.name, &t.to);
        let after = if t.output == "replace" { After::Replace } else { After::Open };
        let job = Job { text: String::new(), from: translate::language(&t.from), to: vec![translate::language(&t.to)], all: false, after };
        items.push(Item::new(&title, "Translator", Icon::Vector(g::LANGUAGES), Act::Translate(job)).acc("Command").keyed(format!("tr:{title}")).words("translate language selection").tinted(Tint::Web));
    }
    for s in &cfg.snippet {
        let args = crate::snippets::arguments(&s.text).iter().map(crate::snippets::Arg::to_arg).collect();
        items.push(Item::new(&s.name, &s.keyword, Icon::Vector(g::EDIT), Act::Snippet(s.text.clone())).acc("Snippet").keyed(format!("sn:{}", s.name)).words(&s.keyword).tinted(Tint::Paste).boxes(args, &[s.keyword.as_str()]));
    }
    for (i, s) in scripts.iter().enumerate() {
        let icon = s.icon.clone().map(Icon::glyph).unwrap_or(Icon::Vector(g::TERMINAL));
        let args = s.args.iter().map(crate::scripts::Arg::to_arg).collect();
        items.push(Item::new(&s.title, &s.package, icon, Act::Script(i)).acc("Script Command").keyed(format!("sc:{}", s.path)).boxes(args, &[]));
    }
    for (e, (ext, icon)) in exts.iter().enumerate() {
        for (c, cmd) in ext.commands.iter().enumerate() {
            let mut it = Item::new(&cmd.title, &ext.title, icon.clone(), Act::ExtRun(e, c)).acc("Command").keyed(format!("ext:{}/{}", ext.name, cmd.name)).words(&ext.title).boxes(cmd.args.clone(), &[]);
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

/// How a query meets a title, strongest first: the whole title, a whole word ("ai" in
/// "Ask AI"), the title's start, the start of a word, the initials. Scattered letters
/// ("sear*c*h fi*l*es" for "cl") get nothing extra.
fn tier(title: &str, q: &str) -> f64 {
    let (t, q) = (title.to_lowercase(), q.to_lowercase());
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    if t == q {
        1000.
    } else if words.iter().any(|w| *w == q) {
        700.
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

/// A quicklink's icon as the catalog has it: its favicon once downloaded, else the link glyph.
fn quicklink_icon(catalog: &[Item], title: &str) -> Icon {
    let key = format!("ql:{title}");
    catalog.iter().find(|it| it.key.as_deref() == Some(key.as_str())).map_or(Icon::Vector(g::LINK), |it| it.icon.clone())
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
        top.push(Item::new(cmdline, "", Icon::Vector(g::TERMINAL), Act::Run(cmdline.to_string())).acc("Windows Run"));
    }
    // A row named alone or with a space after it ("timer 5m") leads, its boxes filled from
    // the rest; named alone, its subtitle says what the first box wants.
    let typed = catalog.iter().find(|it| leads(it, query.trim_start(), cfg));
    if let Some(it) = typed {
        let values = prefill(it, query.trim_start());
        let subtitle = match it.args.first() {
            Some(a) if values.is_empty() && !a.optional => a.missing.clone(),
            _ => it.subtitle.clone(),
        };
        top.push(Item { values, subtitle, ..it.clone() });
    }
    if cfg.search.calculator
        && let Some(result) = calculate(q)
    {
        let full = result.trim_start_matches("approx. ").to_string();
        top.push(Item::new(tidy_number(&result), q, Icon::Vector(g::CALCULATOR), Act::Copy(full)).acc("Calculator").tinted(Tint::Calc));
    }

    // Fallback commands: the web search and quicklinks that take a query, in the configured
    // order. None for a command's exact name or keyword ("timer"): nothing competes with it.
    let command = catalog.iter().any(|it| matches!(it.act, Act::Cmd(_)) && exact(it, q));
    let fallbacks: Vec<Item> = cfg
        .search
        .fallbacks
        .iter()
        .filter(|_| !command)
        .filter_map(|name| {
            if name.eq_ignore_ascii_case("ai") {
                return Some(Item::new("Ask AI", "", Icon::Vector(g::SPARKLES), Act::Ask(q.to_string())).acc("AI").tinted(Tint::Web).sec(format!("Use \"{q}\" with")));
            }
            if name.eq_ignore_ascii_case("define") {
                return (cfg.dictionary.enabled && crate::dict::looks_like_word(q)).then(|| Item::new("Define", "", Icon::Vector(g::BOOK_OPEN), Act::Define(q.to_string())).acc("Dictionary").tinted(Tint::Web).sec(format!("Use \"{q}\" with")));
            }
            let (title, url, icon, kind) = if name.eq_ignore_ascii_case("web") {
                (!cfg.search.fallback.is_empty()).then(|| ("Search the web".to_string(), cfg.search.fallback.clone(), Icon::Vector(g::GLOBE), "Web"))?
            } else if name.eq_ignore_ascii_case("chatgpt") {
                ("Search with ChatGPT".to_string(), "https://chatgpt.com/?q={query}".to_string(), Icon::Vector(g::SEARCH), "Web")
            } else {
                let l = cfg.quicklink.iter().find(|l| l.title().eq_ignore_ascii_case(name) && l.url.contains("{query}"))?;
                (l.title().to_string(), l.url.clone(), quicklink_icon(catalog, l.title()), "Quicklink")
            };
            Some(Item::new(title, "", icon, Act::Open(url.replace("{query}", &url_encode(q)))).acc(kind).tinted(Tint::Web).sec(format!("Use \"{q}\" with")))
        })
        .collect();
    // An alias typed exactly puts its item first, hidden or not.
    let aliased: Vec<&Item> = catalog.iter().filter(|it| alias_is(it, q)).collect();
    top.splice(0..0, aliased.iter().map(|it| (*it).clone()));
    let room = max.saturating_sub(top.len() + fallbacks.len());
    let shown = |it: &Item| aliased.iter().chain(&typed).any(|a| std::ptr::eq(*a, it));
    let hits = fuzzy(q, catalog.iter().filter(|it| !it.hidden && !shown(it)), frec, matcher);
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

/// "3 min ago".
pub fn ago(at: u64) -> String {
    match now().saturating_sub(at) {
        a @ 0..60 => if a < 10 { "just now".into() } else { format!("{a} s ago") },
        a @ 60..3600 => format!("{} min ago", a / 60),
        a @ 3600..86400 => format!("{} h ago", a / 3600),
        a => format!("{} d ago", a / 86400),
    }
}

fn file_name(path: &str) -> &str {
    path.trim_end_matches('\\').rsplit('\\').next().unwrap_or(path)
}

/// Clipboard history rows, pinned entries first. `thumb` gives an image entry's thumbnail by id.
pub fn clipboard(query: &str, entries: &[Entry], max: usize, matcher: &mut Matcher, thumb: &mut dyn FnMut(&str) -> Option<Arc<RenderImage>>) -> Vec<Item> {
    use crate::clipboard::Kind;
    let now = now();
    let row = |e: &Entry| {
        let first = |s: &str| s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().chars().take(120).collect::<String>();
        let (title, sub, icon, hay) = match &e.kind {
            Kind::Text => {
                let (glyph, kind) = clip_kind(&e.text);
                (first(&e.text), kind.to_string(), Icon::Vector(glyph), e.text.chars().take(500).collect())
            }
            Kind::Image { id, w, h, .. } => {
                let icon = thumb(id).map(Icon::Image).unwrap_or(Icon::Vector(g::PICTURE));
                (format!("Image {w} × {h}"), first(&e.text), icon, format!("image picture {}", e.text.chars().take(2000).collect::<String>()))
            }
            Kind::Files(files) => {
                let name = file_name(&files[0]).to_string();
                let title = if files.len() > 1 { format!("{name} and {} more", files.len() - 1) } else { name };
                let folder = files.len() == 1 && std::path::Path::new(&files[0]).is_dir();
                let sub = if files.len() > 1 { format!("{} files", files.len()) } else if folder { "Folder".into() } else { "File".into() };
                (title, sub, Icon::Vector(if folder { g::FOLDER } else { g::DOCUMENT }), format!("file {}", e.text))
            }
        };
        let key = e.key();
        let section = if e.pinned { "Pinned" } else if now.saturating_sub(e.at) < 86400 { "Today" } else { "Earlier" };
        let mut it = Item::new(title, sub, icon, Act::Clip(key.clone())).acc(ago(e.at)).sec(section).tinted(Tint::Paste).with_hay(hay);
        it.extra = [Act::Clip(key.clone()), Act::CopyClip(key.clone()), Act::PinClip(key.clone(), !e.pinned)].map(|a| (a.label().to_string(), a)).to_vec();
        if let Kind::Files(files) = &e.kind {
            it.extra.push(("Show in Explorer".into(), Act::Reveal(files[0].clone())));
        }
        it.extra.push(("Delete Entry".into(), Act::DeleteClip(key)));
        it
    };
    let ordered = entries.iter().filter(|e| e.pinned).chain(entries.iter().filter(|e| !e.pinned));
    let q = query.trim();
    if q.is_empty() {
        return ordered.take(max).map(row).collect();
    }
    let rows: Vec<Item> = ordered.map(row).collect();
    fuzzy(q, rows.iter(), &Frecency::default(), matcher).into_iter().take(max).map(|(_, it)| it.clone()).collect()
}

/// `[emoji] skin_tone` for the emojis crate.
fn skin_tone(name: &str) -> Option<emojis::SkinTone> {
    use emojis::SkinTone as S;
    Some(match name {
        "light" => S::Light,
        "medium_light" => S::MediumLight,
        "medium" => S::Medium,
        "medium_dark" => S::MediumDark,
        "dark" => S::Dark,
        _ => return None,
    })
}

/// An emoji in the chosen skin tone, when it comes in tones.
pub fn toned(e: &'static str, tone: &str) -> &'static str {
    match (emojis::get(e), skin_tone(tone)) {
        (Some(e), Some(t)) => e.with_skin_tone(t).map_or(e.as_str(), |x| x.as_str()),
        _ => e,
    }
}

pub fn emoji(query: &str, max: usize, matcher: &mut Matcher, cfg: &crate::config::Emoji) -> Vec<Item> {
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
        let shown = toned(e.as_str(), &cfg.skin_tone);
        let act = if cfg.action == "copy" { Act::Copy(shown.into()) } else { Act::Paste(shown.into()) };
        let mut it = Item::new(e.name(), "", Icon::glyph(shown), act).acc(group);
        it.hay = std::iter::once(e.name()).chain(e.shortcodes()).collect::<Vec<_>>().join(" ");
        it
    };
    // The tone variants have their own entries; the chosen tone is applied to the base ones.
    // ponytail: Segoe UI Emoji draws up to Emoji 15.1; newer ones would be empty boxes. Raise when Windows ships them.
    let drawn = |e: &emojis::Emoji| e.unicode_version() <= emojis::UnicodeVersion::new(15, 1);
    let all = || emojis::iter().filter(move |e| drawn(e) && e.skin_tone().is_none_or(|t| t == emojis::SkinTone::Default));
    let q = query.trim();
    if q.is_empty() {
        return all().take(max).map(|e| { let it = row(e); let s = it.accessory.clone(); it.sec(s) }).collect();
    }
    let rows: Vec<Item> = all().map(row).collect();
    fuzzy(q, rows.iter(), &Frecency::default(), matcher).into_iter().take(max).map(|(_, it)| it.clone()).collect()
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

pub fn url_encode(s: &str) -> String {
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
    #[ignore = "cargo test bench_emoji_search -- --ignored --nocapture"]
    fn bench_emoji_search() {
        let (mut m, e) = (Matcher::new(nucleo_matcher::Config::DEFAULT), crate::config::Emoji::default());
        for q in ["", "s", "sm", "smi", "smile", "thumbs up"] {
            let t = Instant::now();
            let n = (0..20).map(|_| emoji(q, 400, &mut m, &e).len()).last().unwrap();
            println!("{q:>10}: {n} rows, {:.2?} each", t.elapsed() / 20);
        }
    }

    #[test]
    fn timer_query_offers_to_start_a_timer_first() {
        let cfg = crate::config::parse("").unwrap();
        let cat = catalog(&[], &[], &[], &cfg);
        let r = root("timer 5m tea", &cat, &cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT));
        assert_eq!(with_args(&r[0], &r[0].values), Ok(Act::StartTimer(300, "tea".into())));
        let r = root("timer soon", &cat, &cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT));
        assert_eq!(with_args(&r[0], &r[0].values).map_err(|e| e.0), Err(0), "not a time: Duration is missing");
    }

    #[test]
    fn translate_query_offers_a_translation_first() {
        let cfg = crate::config::parse("[translator]
to = \"FR\"").unwrap();
        let cat = catalog(&[], &[], &[], &cfg);
        let ask = |q: &str| root(q, &cat, &cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT));
        let job = |text: &str, to: &[&str]| Job { text: text.into(), from: String::new(), to: to.iter().map(|s| s.to_string()).collect(), all: true, after: After::Open };
        let r = ask("tr de hola mundo");
        assert_eq!((r[0].act.clone(), r[0].values.clone()), (Act::Cmd(Cmd::Translate), vec!["hola mundo".to_string(), "German".into()]));
        assert_eq!(with_args(&r[0], &r[0].values), Ok(Act::Translate(job("hola mundo", &["de"]))));
        let r = ask("translate hola");
        assert_eq!(with_args(&r[0], &r[0].values), Ok(Act::Translate(job("hola", &[]))), "the configured target");
        assert!(ask("tr").iter().all(|it| it.values.is_empty()));
    }

    #[test]
    fn instant_translate_commands_replace_the_old_rows() {
        let titles: Vec<String> = catalog(&[], &[], &[], &Config::default()).into_iter().map(|it| it.title).collect();
        for gone in ["Instant Translate", "Quick Translate"] {
            assert!(!titles.iter().any(|t| t == gone), "{gone}");
        }
        for new in ["Instant Translate Copy", "Instant Translate Paste"] {
            assert!(titles.iter().any(|t| t == new), "{new}");
        }
    }

    #[test]
    fn translate_commands_become_rows_with_stable_keys() {
        let cfg = crate::config::parse("[[translate]]
to = \"de\"
output = \"replace\"

[[translate]]
name = \"Spanish, please\"
from = \"EN\"
to = \"ES\"").unwrap();
        let items = items(&[], &[], &[], &cfg);
        let german = items.iter().find(|it| it.title == "Translate to German").unwrap();
        assert_eq!(german.key.as_deref(), Some("tr:Translate to German"));
        assert_eq!(german.act, Act::Translate(Job { text: String::new(), from: "auto".into(), to: vec!["de".into()], all: false, after: After::Replace }));
        let spanish = items.iter().find(|it| it.title == "Spanish, please").unwrap();
        assert_eq!(spanish.act, Act::Translate(Job { text: String::new(), from: "en".into(), to: vec!["es".into()], all: false, after: After::Open }));
    }

    #[test]
    fn define_offers_a_row_for_the_query_and_as_a_fallback() {
        let ask = |cfg: &Config, q: &str| root(q, &catalog(&[], &[], &[], cfg), cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT));
        let cfg = crate::config::parse("[search]
fallbacks = [\"define\", \"web\"]").unwrap();
        let r = ask(&cfg, "define dog");
        assert_eq!(with_args(&r[0], &r[0].values), Ok(Act::Define("dog".into())));
        let tail: Vec<_> = ask(&cfg, "dog").into_iter().filter(|it| it.section.as_deref() == Some("Use \"dog\" with")).map(|it| it.act).collect();
        assert_eq!(tail[0], Act::Define("dog".into()), "first, in the configured order");
        assert!(ask(&cfg, "dog food").iter().all(|it| !matches!(it.act, Act::Define(_))), "one word only");
        assert!(ask(&Config::default(), "dog").iter().any(|it| it.act == Act::Define("dog".into())), "on by default");
        let unlisted = crate::config::parse("[search]\nfallbacks = [\"web\"]").unwrap();
        assert!(ask(&unlisted, "dog").iter().all(|it| !matches!(it.act, Act::Define(_))), "off when not listed");
        assert!(catalog(&[], &[], &[], &cfg).iter().any(|it| it.act == Act::Cmd(Cmd::Define)));

        let off = crate::config::parse("[dictionary]
enabled = false
[search]
fallbacks = [\"define\"]").unwrap();
        assert!(ask(&off, "define dog").iter().chain(&ask(&off, "dog")).all(|it| !matches!(it.act, Act::Define(_))));
        assert!(catalog(&[], &[], &[], &off).iter().all(|it| it.act != Act::Cmd(Cmd::Define)));
    }

    #[test]
    fn destructive_actions_are_the_ones_that_remove_or_end_something() {
        assert!(Act::DeleteClip("x".into()).destructive() && Act::ForceQuit(1).destructive() && Act::Cmd(Cmd::ShutDown).destructive());
        assert!(Act::Cmd(Cmd::ClearClipboardHistory).destructive() && Act::Cmd(Cmd::ClearSearchHistory).destructive());
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
        assert_eq!(r.last().unwrap().accessory, "Dictionary", "the last fallback");
        assert!(r.iter().any(|h| h.accessory == "AI"));
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

        let gh = &root("gh rust gpui", &cat, &cfg, &frec, &mut m)[0];
        assert_eq!(with_args(gh, &gh.values), Ok(Act::Open("https://github.com/search?q=rust%20gpui".into())));
        // A quicklink found by name asks for its argument.
        let gh = &root("github", &cat, &cfg, &frec, &mut m)[0];
        assert_eq!((gh.args.len(), with_args(gh, &[]).map_err(|e| e.0)), (1, Err(0)));
        assert_eq!(root("> cmd /k dir", &cat, &cfg, &frec, &mut m)[0].act, Act::Run("cmd /k dir".into()));
        assert_eq!(root(";sig", &cat, &cfg, &frec, &mut m)[0].title, "Signature");

        use crate::clipboard::Kind;
        let mut clips = vec![Entry::text("fn main() {}", now()), Entry::text("hello world", now() - 7200)];
        clips.push(Entry { kind: Kind::Image { id: "ab".into(), w: 640, h: 480, bytes: 9 }, ..Entry::text("Invoice 42", now() - 90_000) });
        clips.push(Entry { kind: Kind::Files(vec![r"C:\a\report.pdf".into(), r"C:\a\b.txt".into()]), pinned: true, ..Entry::text("C:\\a\\report.pdf\nC:\\a\\b.txt", now()) });
        let mut none = |_: &str| None;
        let c = clipboard("hel", &clips, 5, &mut m, &mut none);
        assert_eq!(c[0].act, Act::Clip("text:hello world".into()));
        assert_eq!(c[0].actions().last().map(|a| &a.1), Some(&Act::DeleteClip("text:hello world".into())));
        let all = clipboard("", &clips, 5, &mut m, &mut none);
        assert_eq!((all[0].title.as_str(), all[0].section.as_deref()), ("report.pdf and 1 more", Some("Pinned")), "pinned first");
        assert!(all[0].actions().iter().any(|a| a.1 == Act::PinClip(clips[3].key(), false)), "pinned entries offer Unpin");
        assert_eq!(all[2].accessory, "2 h ago");
        assert_eq!(clipboard("invoice", &clips, 5, &mut m, &mut none)[0].title, "Image 640 × 480", "text found in an image is searchable");

        let e = crate::config::Emoji::default();
        assert_eq!(emoji("thumbs up", 5, &mut m, &e)[0].act, Act::Paste("👍".into()));
        let e = crate::config::Emoji { action: "copy".into(), skin_tone: "dark".into(), ..e };
        assert_eq!(emoji("thumbs up", 5, &mut m, &e)[0].act, Act::Copy("👍🏿".into()), "copy, in the chosen skin tone");
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
    fn more_layouts_tile_the_work_area() {
        use windows::Win32::Foundation::RECT;
        use commands::{Snap, layout};
        let rect = |l, t, r, b| RECT { left: l, top: t, right: r, bottom: b };
        let wa = rect(0, 0, 1200, 900);
        let cur = rect(100, 100, 900, 700);
        let at = |s| layout(s, wa, cur);
        assert_eq!((at(Snap::TopLeftQuarter), at(Snap::BottomRightQuarter)), (rect(0, 0, 600, 450), rect(600, 450, 1200, 900)));
        assert_eq!((at(Snap::TopRightQuarter), at(Snap::BottomLeftQuarter)), (rect(600, 0, 1200, 450), rect(0, 450, 600, 900)));
        assert_eq!((at(Snap::TopCenterSixth), at(Snap::BottomRightSixth)), (rect(400, 0, 800, 450), rect(800, 450, 1200, 900)));
        assert_eq!((at(Snap::TopLeftSixth), at(Snap::TopRightSixth)), (rect(0, 0, 400, 450), rect(800, 0, 1200, 450)));
        assert_eq!((at(Snap::BottomLeftSixth), at(Snap::BottomCenterSixth)), (rect(0, 450, 400, 900), rect(400, 450, 800, 900)));
        assert_eq!((at(Snap::TopThird), at(Snap::MiddleThird), at(Snap::BottomThird)), (rect(0, 0, 1200, 300), rect(0, 300, 1200, 600), rect(0, 600, 1200, 900)));
        assert_eq!(at(Snap::ReasonableSize), rect(240, 135, 960, 765));

        let odd = rect(0, 0, 1201, 901);
        let (q, s, t) = (layout(Snap::BottomRightQuarter, odd, cur), layout(Snap::BottomRightSixth, odd, cur), layout(Snap::BottomThird, odd, cur));
        assert_eq!((q.right, q.bottom, s.right, s.bottom, t.right, t.bottom), (1201, 901, 1201, 901, 1201, 901), "right and bottom pieces end on the work area");
    }

    #[test]
    fn layouts_become_root_commands() {
        let cfg = crate::config::parse("[[window.layouts]]\nname = \"Coding\"\nslots = [{ app = \"Code.exe\", snap = \"LeftHalf\" }]").unwrap();
        let cat = catalog(&[], &[], &[], &cfg);
        let it = cat.iter().find(|i| i.title == "Layout: Coding").unwrap();
        assert_eq!((it.act.clone(), it.key.as_deref(), it.subtitle.as_str()), (Act::Layout("Coding".into()), Some("wl:Coding"), "Window"));
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
        // Room for the results next to the four fallback rows.
        let cfg = crate::config::parse("max_results = 12").unwrap();
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

    fn strings<const N: usize>(v: [&str; N]) -> Vec<String> {
        v.map(String::from).to_vec()
    }

    #[test]
    fn argument_boxes_fill_from_the_query() {
        let cat = catalog(&[], &[], &[], &Config::default());
        let cmd = |c: Cmd| cat.iter().find(|it| it.act == Act::Cmd(c)).unwrap();
        let timer = cmd(Cmd::StartTimer);
        assert_eq!(prefill(timer, "timer 25m tea"), ["25m", "tea"]);
        assert_eq!(prefill(timer, "t 90"), ["90"]);
        assert_eq!(prefill(timer, "timer 1:30"), ["1:30"]);
        assert_eq!(prefill(timer, "Start Timer 5m"), ["5m"], "the command's name works too");
        assert!(prefill(timer, "timer").is_empty() && prefill(timer, "timers 5m").is_empty());
        let tr = cmd(Cmd::Translate);
        assert_eq!(prefill(tr, "translate guten Morgen"), ["guten Morgen"]);
        assert_eq!(prefill(tr, "tr de hola mundo"), ["hola mundo", "German"], "a language code leading the text picks To");
        assert_eq!(prefill(tr, "tr de"), ["de"], "a lone code is the text");
        assert_eq!(prefill(cmd(Cmd::Define), "def dog"), ["dog"]);
    }

    #[test]
    fn with_args_runs_only_when_the_required_boxes_are_filled() {
        let cat = catalog(&[], &[], &[], &Config::default());
        let cmd = |c: Cmd| cat.iter().find(|it| it.act == Act::Cmd(c)).unwrap();
        let timer = cmd(Cmd::StartTimer);
        assert_eq!(with_args(timer, &[]), Err((0, "Type a time: 5m, 1h30m, 1:30".to_string())));
        assert_eq!(with_args(timer, &strings(["soon"])), Err((0, "Type a time: 5m, 1h30m, 1:30".to_string())), "not a time");
        assert_eq!(with_args(timer, &strings(["25m", "tea"])), Ok(Act::StartTimer(1500, "tea".into())));
        let typed = crate::config::parse("[translator]\nfill_selection = false").unwrap();
        let typed = catalog(&[], &[], &[], &typed);
        let tr = typed.iter().find(|it| it.act == Act::Cmd(Cmd::Translate)).unwrap();
        assert_eq!(with_args(tr, &strings(["", "German"])), Err((0, "Type the text to translate".to_string())));
        let tr = cmd(Cmd::Translate);
        assert_eq!(with_args(tr, &strings(["", "German"])).map(|_| ()), Ok(()), "the selection fills it");
        let Ok(Act::Translate(job)) = with_args(tr, &strings(["hallo", "German"])) else { panic!("a translation") };
        assert_eq!((job.text.as_str(), job.to.as_slice()), ("hallo", &["de".to_string()][..]));
        let Ok(Act::Translate(job)) = with_args(tr, &strings(["hallo"])) else { panic!("a translation") };
        assert!(job.to.is_empty() && job.from.is_empty(), "empty: the configured languages");
        assert_eq!(with_args(cmd(Cmd::Define), &strings([" dog "])), Ok(Act::Define("dog".into())));
        let focus = cmd(Cmd::Focus(crate::commands::FocusCmd::Start));
        assert_eq!(with_args(focus, &[]), Ok(Act::With(Box::new(focus.act.clone()), strings(["", "", ""]))), "all optional");
        assert_eq!(with_args(focus, &strings(["Read", "soon"])).map_err(|e| e.0), Err(1), "Minutes is checked");
    }

    #[test]
    fn argument_rows_mirror_their_values_and_count_their_boxes() {
        let cat = catalog(&[], &[], &[], &Config::default());
        let cmd = |c: Cmd| cat.iter().find(|it| it.act == Act::Cmd(c)).unwrap();
        let focus = cmd(Cmd::Focus(crate::commands::FocusCmd::Start));
        assert_eq!(mirror(cmd(Cmd::StartTimer), &strings(["25m", "tea"])), "25 min · tea", "the time as parsed");
        assert_eq!(mirror(cmd(Cmd::StartTimer), &strings(["soon"])), "soon", "not a time: as typed");
        assert_eq!(mirror(focus, &strings(["Deep work", "50", ""])), "Deep work · 50");
        assert_eq!(mirror(focus, &[]), "");
        assert_eq!(focus.arg_count().as_deref(), Some("3 arguments"));
        assert_eq!(cmd(Cmd::StartStopwatch).arg_count().as_deref(), Some("1 argument"));
        assert_eq!(cmd(Cmd::Settings).arg_count(), None);
        let secret = Item { args: vec![Arg { kind: ArgKind::Secret, ..Arg::new("Token") }], ..Item::new("Log In", "", Icon::glyph("•"), Act::None) };
        assert_eq!(mirror(&secret, &strings(["hunter2"])), "", "a secret is never shown");
    }

    #[test]
    fn a_command_named_with_a_space_leads_root_search_with_its_boxes_filled() {
        let cfg = Config::default();
        let cat = catalog(&[], &[], &[], &cfg);
        let ask = |q: &str| root(q, &cat, &cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT));
        let r = ask("timer 25m tea");
        assert_eq!((r[0].title.as_str(), r[0].values.clone()), ("Start Timer", strings(["25m", "tea"])));
        assert_eq!(r.iter().filter(|it| it.title == "Start Timer").count(), 1);
        assert_eq!(ask("timer ")[0].title, "Start Timer", "the space typed after the keyword");
        assert_eq!(ask("define dog")[0].values, strings(["dog"]));
        for it in &cat {
            assert!(it.args.len() <= 3, "{}", it.title);
        }
    }

    #[test]
    fn timer_keywords_lead_root_search_and_hold_back_the_fallbacks() {
        let cfg = crate::config::parse("[search]
fallbacks = [\"define\", \"web\"]").unwrap();
        let cat = catalog(&[], &[], &[], &cfg);
        let ask = |q: &str| root(q, &cat, &cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT));
        assert_eq!(with_args(&ask("t 90")[0], &ask("t 90")[0].values), Ok(Act::StartTimer(90, String::new())));
        assert_eq!(with_args(&ask("timer 1:30")[0], &ask("timer 1:30")[0].values), Ok(Act::StartTimer(90, String::new())));
        let r = ask("timer");
        assert_eq!((r[0].title.as_str(), r[0].subtitle.as_str()), ("Start Timer", "Type a time: 5m, 1h30m, 1:30"));
        assert!(r.iter().all(|it| it.section.as_deref().is_none_or(|s| !s.starts_with("Use "))), "no Define or web rows for a command's keyword");
        assert!(ask("Start Timer").iter().all(|it| !matches!(it.act, Act::Define(_) | Act::Open(_))), "nor for its name");
        assert!(ask("timber").iter().any(|it| matches!(it.act, Act::Define(_))), "a plain word still gets them");
    }

    #[test]
    fn focus_names_start_focus_session_only_when_no_category_matches() {
        let cfg = crate::config::parse("[[focus_category]]\nname = \"Work\"\napps = []\nsites = []").unwrap();
        let cat = catalog(&[], &[], &[], &cfg);
        let start = cat.iter().find(|it| it.act == Act::Cmd(Cmd::Focus(FocusCmd::Start))).unwrap();
        assert_eq!(named_in(start, "focus ", &cfg), None, "Search Focus Categories' prefix lists the categories");
        assert_eq!(named_in(start, "focus wor", &cfg), None);
        assert_eq!(named_in(start, "focus deep work", &cfg), Some("deep work"));
        let ask = |q: &str| root(q, &cat, &cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT));
        assert!(ask("focus wor").iter().all(|it| it.values.is_empty()), "no Goal pre-filled from a category search");
        assert_eq!(ask("focus deep work")[0].values, strings(["deep work"]));
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
