#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod apps;
mod clipboard;
mod commands;
mod config;
mod design;
mod ext;
mod files;
mod keys;
mod net;
mod ntfs;
mod scripts;
mod search;
mod settings;
mod snippets;
mod win;

use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{
    StreamExt,
    channel::mpsc::{self, UnboundedSender},
};
use gpui::{
    App, Bounds, ClipboardItem, Context, Entity, FocusHandle, Focusable, FontWeight, KeyBinding, QuitMode, RenderImage,
    ScrollHandle, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window, WindowBounds, WindowKind, WindowOptions,
    actions, div, prelude::*, size, uniform_list,
};
use gpui_ce_elements::editable_text::{
    EditableTextState, StringStorage, TextChanged,
    actions::{DEFAULT_INPUT_CONTEXT, default_bindings},
    text_input,
};
use nucleo_matcher::Matcher;
use serde_json::{Value, json};
use tray_icon::{
    Icon as TrayImage, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
use windows::Win32::Foundation::HWND;

use commands::Cmd;
use config::Config;
use design::{Tokens, g, px};
use ext::FieldKind;
use scripts::{Output, Script};
use search::{Act, Frecency, Icon, Item};
use win::{BINDS, Bind, Msg, TrayCmd};

actions!(procast, [SelectPrev, SelectNext, Dismiss, Launch, ToggleActions, Submit, NextSection, PrevSection]);
// Left and Right in the emoji grid (the search bar keeps them elsewhere).
actions!(procast, [SelectLeft, SelectRight]);
// Navigation-key styles; each does something only when `[launcher] navigation` picks it.
actions!(procast, [EmacsNext, EmacsPrev, EmacsBack, VimNext, VimPrev, VimBack]);

use design::{BAR, FOOTER, LIST_PAD, ROW};

fn window_height(cfg: &Config) -> f32 {
    // The window's 1 px border top and bottom, the bar, whole rows (results plus a section
    // header), the list padding and the action bar.
    2. + BAR + ROW * (cfg.max_results.max(1) + 1) as f32 + 2. * LIST_PAD + FOOTER
}

/// Compact mode's favourites strip under the search bar.
const FAV_ROW: f32 = 48.;

/// The launcher's width before scaling. `width` ≤ 1 is a share of the screen: the window
/// keeps that share (and its proportions) on every monitor, and its content scales with it.
const BASE_WIDTH: f32 = 750.;

/// The launcher's backdrop: acrylic under the translucent glass body; with glass off the body
/// is solid and the acrylic goes (Transparent, not Opaque: Opaque clears to white, which would
/// show in the anti-aliased corners inside the rounded window region).
fn backdrop(glass: bool) -> gpui::WindowsWindowBackground {
    if glass { gpui::WindowsWindowBackground::Blurred } else { gpui::WindowsWindowBackground::Transparent }
}

fn base_width(cfg: &Config) -> f32 {
    if cfg.width <= 1. { BASE_WIDTH } else { cfg.width }
}

/// What kind of message the action bar is showing.
#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Info,
    Ok,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Root,
    Clipboard,
    Emoji,
    Files,
    /// Collecting script or snippet arguments (see `Launcher::pending`).
    Args,
    /// Showing a script's output (see `Launcher::output`).
    Output,
    /// An extension command's view (see `Launcher::extv`).
    Ext,
    /// Asking what to install.
    Install,
    /// Browsing the Raycast store index.
    Store,
    /// Running apps, to quit.
    Running,
    /// A Quick AI answer (see `Launcher::ai`).
    Ai,
}

/// A Quick AI chat: one Claude session, kept while the view is open.
#[derive(Default)]
struct AiChat {
    turns: Vec<AiTurn>,
    /// The last turn is still streaming.
    busy: bool,
    /// Keep the newest question at the top of the view (until Up or Down).
    follow: bool,
}

struct AiTurn {
    id: u64,
    prompt: String,
    text: String,
}

impl AiChat {
    /// The chat as markdown, and how many lines come before the newest question.
    fn markdown(&self) -> (String, usize) {
        let parts: Vec<String> = self
            .turns
            .iter()
            .map(|t| {
                let text = if t.text.is_empty() { "Thinking…" } else { t.text.trim_end() };
                // An answer cut off inside a code block must not swallow the next question.
                let open = text.lines().filter(|l| l.trim_start().starts_with("```")).count() % 2 == 1;
                format!("### {}\n\n{text}{}\n\n", t.prompt, if open { "\n```" } else { "" })
            })
            .collect();
        let before = parts.len().saturating_sub(1);
        (parts.concat(), ext::markdown_lines(&parts[..before].concat()).len())
    }
}

impl Mode {
    fn of(bind: Bind) -> Self {
        match bind {
            Bind::Launcher => Mode::Root,
            Bind::Clipboard => Mode::Clipboard,
            Bind::Emoji => Mode::Emoji,
            Bind::Files => Mode::Files,
        }
    }
}

/// The global hotkeys: the four in `BINDS`, then one per command with a hotkey.
fn hotkey_specs(cfg: &Config) -> Vec<String> {
    let builtin = [&cfg.hotkey, &cfg.clipboard.hotkey, &cfg.emoji.hotkey, &cfg.files.hotkey];
    builtin.into_iter().chain(cfg.launcher.command_hotkeys().map(|(_, h)| h)).cloned().collect()
}

fn start_files(cfg: &Config, tx: &UnboundedSender<Msg>) -> Option<files::Files> {
    cfg.files.enabled.then(|| files::Files::start(&cfg.files, tx.clone()))
}

/// Section marker for a row that stands in for an empty view (drawn as an empty state).
const EMPTY: &str = "\u{0}empty";

/// List slots for items: None = a section header, Some(i) = item i. Consecutive items
/// with the same section share one header.
fn display_rows(items: &[Item]) -> Vec<Option<usize>> {
    let mut out = Vec::with_capacity(items.len() + 4);
    let mut last: Option<&str> = None;
    // A header says something only when there is more than one section, and "Results"
    // right under the search field says nothing.
    let one = items.windows(2).all(|w| w[0].section == w[1].section);
    for (i, it) in items.iter().enumerate() {
        if is_answer(items, i) {
            continue;
        }
        let sec = it.section.as_deref().filter(|s| *s != EMPTY);
        if sec.is_some() && sec != last && !one && sec != Some("Results") {
            out.push(None);
        }
        last = sec;
        out.push(Some(i));
    }
    out
}

/// The calculator's answer leads root results and is drawn as a card above the list.
fn is_answer(items: &[Item], i: usize) -> bool {
    i == 0 && items.first().is_some_and(|it| it.accessory == "Calculator")
}

fn empty_row(glyph: &'static str, title: String, hint: String, actions: Vec<(String, Act)>) -> Item {
    let mut it = Item::new(title, hint, Icon::Vector(glyph), actions.first().map(|a| a.1.clone()).unwrap_or(Act::None)).sec(EMPTY);
    it.extra = actions;
    it
}

/// Something waiting for its arguments, typed one at a time in the search bar.
enum Pending {
    /// A script (index) and what it has so far.
    Script(usize, Vec<String>),
    /// A snippet with `{argument}`s; `erase` and `then` as in `snippets::Job` (typed keywords).
    Snippet { name: String, template: String, args: Vec<snippets::Arg>, got: Vec<String>, erase: usize, then: String },
}

/// Hook settings: Hyper, double tap, and the snippet keywords and emoji shortcodes typed anywhere.
fn configure_keys(cfg: &Config, tx: &UnboundedSender<Msg>) {
    let keywords = cfg.snippet.iter().enumerate().filter(|(_, s)| cfg.snippets.expand && !cfg.launcher.is_disabled(&format!("sn:{}", s.name))).map(|(i, s)| (i, s.keyword.clone())).collect();
    let x = snippets::Expander::new(keywords, snippets::Trigger::of(&cfg.snippets.trigger), cfg.snippets.within_words, cfg.emoji.shortcodes);
    keys::configure(&cfg.keyboard.hyper, &cfg.keyboard.double_tap, Some(x), tx.clone());
}

/// What `configure_keys` reads, to tell when a reload must redo it.
fn key_settings(cfg: &Config) -> (config::Keyboard, Vec<config::Snippet>, config::Snippets, bool, Vec<String>) {
    (cfg.keyboard.clone(), cfg.snippet.clone(), cfg.snippets.clone(), cfg.emoji.shortcodes, cfg.launcher.disabled.clone())
}

/// The extension command currently on screen.
struct ExtView {
    run: u64,
    ext: usize,
    view: ext::View,
    depth: u64,
    /// Form values by field id.
    values: HashMap<String, Value>,
    inputs: HashMap<String, Entity<EditableTextState>>,
    /// Selected form field.
    field: usize,
    /// Last query passed to the extension's onSearchTextChange.
    sent_query: Option<String>,
    /// Recent queries we sent: a controlled `searchText` equal to one of these is a late
    /// echo of our own typing and must not overwrite newer keystrokes.
    sent_recent: Vec<String>,
    _subs: Vec<Subscription>,
}

struct Confirm {
    run: u64,
    id: u64,
    title: String,
    message: String,
    primary: String,
    /// The primary action destroys something (the extension says so).
    danger: bool,
}

struct Launcher {
    input: Entity<EditableTextState>,
    cfg: Config,
    hotkeys: win::Hotkeys,
    tx: UnboundedSender<Msg>,
    apps: Vec<win::App>,
    scripts: Vec<Script>,
    exts: Vec<(ext::Extension, Icon)>,
    catalog: Vec<Item>,
    scanned: Option<Instant>,
    frec: Frecency,
    history: clipboard::History,
    files: Option<files::Files>,
    file_gen: u64,
    host: Option<ext::Host>,
    next_run: u64,
    extv: Option<ExtView>,
    /// Background runs by id -> "ext/cmd", and when each command last started.
    bg_runs: HashMap<u64, String>,
    bg_last: HashMap<String, Instant>,
    /// Menu-bar commands drawn as tray icons: "ext/cmd" -> (run, icon).
    trays: HashMap<String, (u64, TrayIcon)>,
    confirm: Option<Confirm>,
    images: HashMap<PathBuf, Option<Arc<RenderImage>>>,
    /// Store index (None until loaded) and whether a load is running.
    store: Option<Vec<ext::StoreEntry>>,
    /// Quit Applications: the running apps, once listed (None while loading).
    running: Option<Vec<(apps::Running, Option<Arc<RenderImage>>)>>,
    /// Program icons by path, so the running list fills in at once next time.
    exe_icons: HashMap<String, Option<Arc<RenderImage>>>,
    autoquit: apps::AutoQuit,
    store_loading: bool,
    settings_window: Option<gpui::WindowHandle<settings::Settings>>,
    matcher: Matcher,
    mode: Mode,
    items: Vec<Item>,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// Ctrl+K: the selected item's actions and which one is highlighted.
    panel: Option<(Item, usize)>,
    /// Destructive command waiting for a second Enter.
    armed: Option<usize>,
    pending: Option<Pending>,
    /// Clipboard image thumbnails (by image id, `t:`) and previews (`p:`).
    clip_images: HashMap<String, Option<Arc<RenderImage>>>,
    /// Emoji picker: the grid's scroll position, by grid row.
    /// The emoji grid's rows; only the ones on screen are laid out.
    grid_list: gpui::ListState,
    /// Script output view: (index, output, still running).
    output: Option<(usize, String, bool)>,
    ai: Option<AiChat>,
    ai_scroll: ScrollHandle,
    /// Short-lived message in the footer.
    /// The action bar message, with how it should look.
    status: Option<(Tone, String)>,
    /// Config or extension problem, shown above the results.
    notice: Option<String>,
    hotkey_errors: Vec<Option<String>>,
    hwnd: HWND,
    prev: HWND,
    visible: bool,
    /// The procast notification-area icon (Settings → General → Show in tray).
    tray_icon: Option<TrayIcon>,
    /// When the window last closed, for "pop to root" after a timeout.
    hidden_at: Option<Instant>,
    /// Earlier root searches (oldest first) and which one Up last restored.
    searches: Vec<String>,
    search_pos: Option<usize>,
    /// Compact window opened up with Down.
    peek: bool,
    /// Height the window was last sized to, before interface scaling.
    height: f32,
    _subs: Vec<Subscription>,
}

impl Launcher {
    fn new(cfg: Config, notice: Option<String>, tx: UnboundedSender<Msg>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
        let subs = vec![
            cx.subscribe(&input, |this, _, _: &TextChanged, cx| this.query_changed(cx)),
            cx.observe_window_activation(window, |this, window, cx| {
                if this.visible && !window.is_window_active() {
                    this.hide(false, cx);
                }
            }),
        ];
        clipboard::configure(&cfg.clipboard.ignore_apps, cfg.clipboard.ocr);
        clipboard::watch(tx.clone());
        for (dir, recursive) in win::app_dirs() {
            let tx = tx.clone();
            std::thread::spawn(move || win::watch_dir(&dir, recursive, |_| tx.unbounded_send(Msg::AppsChanged).is_ok()));
        }
        let mut this = Self {
            input,
            hotkeys: {
                configure_keys(&cfg, &tx);
                win::Hotkeys::spawn(hotkey_specs(&cfg), tx.clone())
            },
            history: clipboard::History::load(config::dir().join("clipboard.bin"), cfg.clipboard.history, &cfg.clipboard.keep),
            files: start_files(&cfg, &tx),
            scripts: scripts::scan(&cfg.scripts.dirs()),
            exts: vec![],
            catalog: vec![],
            cfg,
            tx,
            apps: vec![],
            scanned: None,
            running: None,
            exe_icons: HashMap::new(),
            autoquit: apps::AutoQuit::default(),
            frec: Frecency::load(config::dir().join("frecency.json")),
            file_gen: 0,
            host: None,
            next_run: 0,
            extv: None,
            bg_runs: HashMap::new(),
            bg_last: HashMap::new(),
            trays: HashMap::new(),
            confirm: None,
            images: HashMap::new(),
            store: None,
            store_loading: false,
            settings_window: None,
            matcher: Matcher::new(nucleo_matcher::Config::DEFAULT),
            mode: Mode::Root,
            items: vec![],
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            panel: None,
            armed: None,
            pending: None,
            clip_images: HashMap::new(),
            grid_list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(200.)),
            output: None,
            ai: None,
            ai_scroll: ScrollHandle::new(),
            status: None,
            notice,
            hotkey_errors: vec![],
            hwnd: HWND::default(),
            prev: HWND::default(),
            visible: false,
            tray_icon: None,
            hidden_at: None,
            searches: std::fs::read_to_string(config::dir().join("search-history.txt")).unwrap_or_default().lines().map(String::from).collect(),
            search_pos: None,
            peek: false,
            height: 0.,
            _subs: subs,
        };
        this.load_extensions();
        this
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).as_str().to_string()
    }

    /// Compact mode with nothing typed: only the search bar (and favourites) show.
    fn collapsed(&self, cx: &App) -> bool {
        self.cfg.window_mode == "compact"
            && self.mode == Mode::Root
            && !self.peek
            && self.panel.is_none()
            && self.notice.is_none()
            && self.query(cx).trim().is_empty()
    }

    fn fav_strip(&self) -> bool {
        self.cfg.launcher.compact_favorites && !self.cfg.launcher.favorites.is_empty()
    }

    /// Window height for the current state, before interface scaling.
    fn target_height(&self, cx: &App) -> f32 {
        if self.collapsed(cx) { BAR + if self.fav_strip() { FAV_ROW } else { 0. } + 2. } else { window_height(&self.cfg) }
    }

    /// Asks the message loop to resize the window when its content height changed.
    fn request_resize(&mut self, cx: &App) {
        let h = self.target_height(cx);
        if self.visible && h != self.height {
            self.height = h;
            let _ = self.tx.unbounded_send(Msg::Resize);
        }
    }

    /// The last search survives closing for `[launcher] pop_to_root`.
    fn keeps_state(&self) -> bool {
        self.hidden_at.is_some_and(|t| self.cfg.launcher.keep_for().is_none_or(|d| t.elapsed() < d))
    }

    /// Up in an empty (or just-restored) root search bar: the previous search.
    fn recall_search(&mut self, cx: &mut Context<Self>) -> bool {
        let q = self.query(cx);
        let restored = self.search_pos.is_some_and(|i| self.searches.get(i) == Some(&q));
        if self.mode != Mode::Root || !self.cfg.launcher.history || self.searches.is_empty() || !(q.is_empty() || restored) {
            return false;
        }
        let i = match self.search_pos.filter(|_| restored) {
            Some(i) => i.saturating_sub(1),
            None => self.searches.len() - 1,
        };
        self.search_pos = Some(i);
        let text = self.searches[i].clone();
        self.input.update(cx, |s, cx| s.emplace(&text, cx));
        true
    }

    fn remember_search(&mut self, q: String) {
        self.searches.retain(|s| *s != q);
        self.searches.push(q);
        let extra = self.searches.len().saturating_sub(50);
        self.searches.drain(..extra);
        let _ = std::fs::write(config::dir().join("search-history.txt"), self.searches.join("\n"));
    }

    /// Writes `[launcher] <name>` (favourites / hidden) and applies it at once.
    fn save_launcher_list(&mut self, name: &str, list: Vec<String>, cx: &mut Context<Self>) {
        let text = std::fs::read_to_string(config::path()).unwrap_or_default();
        let value = toml_edit::Value::Array(list.iter().map(String::as_str).collect());
        match config::write_value(&text, &["launcher".into(), name.into()], value).and_then(|t| std::fs::write(config::path(), t).map_err(|e| e.to_string())) {
            Ok(()) => {
                if name == "favorites" {
                    self.cfg.launcher.favorites = list;
                } else {
                    self.cfg.launcher.hidden = list;
                }
                self.rebuild_catalog(cx);
            }
            Err(e) => self.status = Some((Tone::Error, e)),
        }
        cx.notify();
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.panel = None;
        self.send_search(cx);
        self.selected = 0;
        self.refresh(cx);
    }

    /// Passes the search text to an extension that filters itself, once per change. Also
    /// called on render, since text typed before the first render has no handler yet.
    fn send_search(&mut self, cx: &mut Context<Self>) {
        if self.mode != Mode::Ext {
            return;
        }
        let q = self.query(cx);
        let Some(ev) = &mut self.extv else { return };
        let Some(f) = ev.view.on_search.clone() else { return };
        if ev.sent_query.as_ref() == Some(&q) || (ev.sent_query.is_none() && q.is_empty()) {
            return;
        }
        ev.sent_query = Some(q.clone());
        ev.sent_recent.push(q.clone());
        if ev.sent_recent.len() > 32 {
            ev.sent_recent.remove(0);
        }
        let run = ev.run;
        self.send(json!({"t": "event", "run": run, "fn": f, "args": [q]}));
    }

    fn image(&mut self, path: PathBuf) -> Option<Arc<RenderImage>> {
        // ponytail: dropped wholesale past 200 images; per-entry LRU if lists get that big.
        if self.images.len() > 200 && !self.images.contains_key(&path) {
            self.images.clear();
        }
        self.images.entry(path.clone()).or_insert_with(|| ext::load_image(&path)).clone()
    }

    /// An extension icon value: "icon:Name", an emoji, an asset file or a URL.
    fn ext_icon(&mut self, ext: usize, icon: Option<&str>) -> Icon {
        let Some(icon) = icon else { return Icon::Vector(g::BULLETS) };
        if let Some(name) = icon.strip_prefix("icon:") {
            return Icon::Vector(ext::icon_glyph(name));
        }
        if icon.starts_with("http") {
            // Web images: drawn from the cache once downloaded; a globe until then.
            let cached = net::image_path(icon);
            if cached.exists() {
                return self.image(cached).map(Icon::Image).unwrap_or(Icon::Vector(g::GLOBE));
            }
            net::fetch_image(icon, &self.tx);
            return Icon::Vector(g::GLOBE);
        }
        if icon.starts_with("data:") {
            return Icon::Vector(g::DOCUMENT);
        }
        if icon.chars().count() <= 4 && !icon.contains('.') {
            return Icon::glyph(icon.to_string());
        }
        let dir = self.exts[ext].0.dir.join("assets");
        let path = if std::path::Path::new(icon).is_absolute() { PathBuf::from(icon) } else { dir.join(icon) };
        match self.image(path) {
            Some(i) => Icon::Image(i),
            None => Icon::Vector(g::BULLETS),
        }
    }

    fn load_extensions(&mut self) {
        let exts = ext::scan();
        let mut out = Vec::with_capacity(exts.len());
        for e in exts {
            let icon = e.icon.clone().and_then(|p| self.image(p)).map(Icon::Image).unwrap_or(Icon::Vector(g::PUZZLE));
            out.push((e, icon));
        }
        self.exts = out;
        self.catalog = search::catalog(&self.apps, &self.scripts, &self.exts, &self.cfg);
        // Drop tray icons (and their runs) for menu-bar commands that are gone.
        let alive: Vec<String> = self.exts.iter().flat_map(|(x, _)| x.commands.iter().map(move |c| format!("{}/{}", x.name, c.name))).collect();
        let gone: Vec<(String, u64)> = self.trays.iter().filter(|(k, _)| !alive.contains(k)).map(|(k, (r, _))| (k.clone(), *r)).collect();
        for (key, run) in gone {
            self.trays.remove(&key);
            self.bg_runs.remove(&run);
            self.send(json!({"t": "stop", "run": run}));
        }
    }

    fn ext_rows(&mut self, query: &str) -> Vec<Item> {
        if let Some(c) = &self.confirm {
            // Drawn as a dialog; these two rows only drive selection and Enter.
            let mut ok = Item::new(c.primary.clone(), "", Icon::Vector(g::CHECK), Act::Confirm(true));
            ok.extra = vec![(c.primary.clone(), Act::Confirm(true))];
            return vec![ok, Item::new("Cancel", "", Icon::Vector(g::CANCEL), Act::Confirm(false))];
        }
        let Some(ev) = &self.extv else { return vec![] };
        let (ext, view) = (ev.ext, ev.view.clone());
        let view_actions: Vec<(String, Act)> =
            view.actions.iter().filter_map(|a| Some((a.title.clone(), Act::ExtFn(a.on_action.clone()?)))).collect();
        match view.kind.as_str() {
            "List" | "Grid" => {
                let mut rows: Vec<Item> = view
                    .items
                    .iter()
                    .map(|it| {
                        let acts: Vec<(String, Act)> =
                            it.actions.iter().filter_map(|a| Some((a.title.clone(), Act::ExtFn(a.on_action.clone()?)))).collect();
                        let primary = acts.first().map(|a| a.1.clone()).unwrap_or(Act::None);
                        let hay = [it.title.as_str(), it.subtitle.as_str(), &it.keywords.join(" "), it.section.as_deref().unwrap_or("")].join(" ");
                        let mut row = Item::new(&it.title, &it.subtitle, self.ext_icon(ext, it.icon.as_deref()), primary).acc(it.accessories.join("  ·  ")).with_hay(hay);
                        row.section = it.section.clone();
                        row.extra = acts;
                        row
                    })
                    .collect();
                if view.on_search.is_none() {
                    rows = search::filter(query, rows, &mut self.matcher);
                }
                // The search-bar dropdown's options are reachable from Ctrl+K on any row.
                if let Some((opts, cur, Some(f))) = &view.dropdown {
                    for row in &mut rows {
                        row.extra.extend(opts.iter().map(|(v, t)| (format!("{} {t}", if v == cur { "Showing:" } else { "Show:" }), Act::ExtSet(f.clone(), v.clone()))));
                    }
                }
                if rows.is_empty() {
                    let (glyph, title, sub) = match &view.empty {
                        _ if view.loading => (g::CLOCK, "Loading…".to_string(), String::new()),
                        Some((t, d)) if !t.is_empty() => (g::SEARCH, t.clone(), d.clone()),
                        _ => (g::SEARCH, "No results".to_string(), String::new()),
                    };
                    rows.push(empty_row(glyph, title, sub, view_actions));
                }
                rows
            }
            "" => vec![empty_row(g::CLOCK, "Loading…".into(), String::new(), vec![])],
            // Detail and Form draw their own body; Enter and Ctrl+K use the view's actions.
            _ => {
                let mut row = Item::new(view.title.clone().unwrap_or_default(), "", Icon::Vector(g::BULLETS), view_actions.first().map(|a| a.1.clone()).unwrap_or(Act::None));
                row.extra = view_actions;
                vec![row]
            }
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let max = self.cfg.max_results.max(1);
        let keep_selection = self.mode == Mode::Ext;
        self.armed = None;
        self.items = match self.mode {
            Mode::Root => {
                // Matching files arrive later (Msg::Files) and slot in above the fallbacks.
                if self.cfg.launcher.files
                    && query.trim().chars().count() >= 3
                    && let Some(f) = &self.files
                {
                    self.file_gen += 1;
                    f.search(self.file_gen, &query, 5);
                }
                search::root(&query, &self.catalog, &self.cfg, &self.frec, &mut self.matcher)
            }
            Mode::Clipboard => {
                if self.clip_images.len() > 300 {
                    self.clip_images.clear();
                }
                let (history, cache) = (&self.history, &mut self.clip_images);
                let mut thumb = |id: &str| cache.entry(format!("t:{id}")).or_insert_with(|| history.png(id).and_then(|p| clipboard::render_image(&p, 64))).clone();
                search::clipboard(&query, &history.entries, 200, &mut self.matcher, &mut thumb)
            }
            Mode::Emoji => search::emoji(&query, 400, &mut self.matcher, &self.cfg.emoji),
            Mode::Files => {
                // Results arrive later as Msg::Files; keep showing the previous ones meanwhile.
                self.file_gen += 1;
                if let Some(f) = &self.files {
                    f.search(self.file_gen, &query, 50);
                }
                if query.trim().is_empty() {
                    vec![empty_row(g::SEARCH, "Search your files".into(), "Type a name; include \\ to match a path".into(), vec![])]
                } else {
                    std::mem::take(&mut self.items)
                }
            }
            Mode::Args => match &self.pending {
                Some(Pending::Script(i, got)) => {
                    let s = &self.scripts[*i];
                    let n = s.args.len();
                    vec![Item::new(format!("Run {}", s.title), format!("argument {} of {n}", got.len() + 1), Icon::Vector(g::COMMAND), Act::Script(*i))]
                }
                Some(Pending::Snippet { name, template, args, got, .. }) => {
                    let sub = format!("{} · {} of {}", args[got.len()].name, got.len() + 1, args.len());
                    vec![Item::new(format!("Paste {name}"), sub, Icon::Vector(g::EDIT), Act::Snippet(template.clone()))]
                }
                None => vec![],
            },
            // Typed text is the next question; with an empty bar, Enter copies the answer.
            Mode::Ai if !query.trim().is_empty() => vec![Item::new("Ask", "", Icon::Vector(g::LIGHTBULB), Act::Ask(query.trim().to_string()))],
            Mode::Ai => self.ai.iter().filter_map(|a| a.turns.last()).filter(|t| !t.text.is_empty()).map(|t| Item::new("Copy answer", "", Icon::Vector(g::COPY), Act::Copy(t.text.clone()))).collect(),
            Mode::Output => match &self.output {
                Some((_, _, true)) => vec![empty_row(g::CLOCK, "Running…".into(), String::new(), vec![])],
                Some((_, text, false)) => {
                    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                    let shown = if lines.is_empty() { vec!["(no output)"] } else { lines };
                    shown.iter().map(|l| Item::new(*l, "", Icon::Vector(g::COMMAND), Act::Copy(text.clone())).sec("Output")).collect()
                }
                None => vec![],
            },
            Mode::Ext => self.ext_rows(&query),
            Mode::Store => match &self.store {
                None => vec![empty_row(g::CLOCK, "Loading the store index…".into(), "The first time fetches every extension's manifest (~1 min)".into(), vec![])],
                Some(entries) => {
                    let installed: Vec<&str> = self.exts.iter().map(|(x, _)| x.name.as_str()).collect();
                    let rows: Vec<Item> = entries
                        .iter()
                        .filter(|s| s.windows)
                        .map(|s| {
                            let done = installed.contains(&s.name.as_str());
                            let mut it = Item::new(&s.title, &s.description, Icon::Vector(if done { g::CHECK } else { g::PUZZLE }), Act::Install(s.name.clone()))
                                .acc(if done { "Installed".to_string() } else { s.author.clone() })
                                .with_hay(format!("{} {} {} {}", s.title, s.name, s.author, s.description));
                            it.extra = vec![
                                ((if done { "Reinstall / Update" } else { "Install" }).to_string(), Act::Install(s.name.clone())),
                                ("Open Store Page".into(), Act::Open(format!("https://www.raycast.com/{}/{}", s.author, s.name))),
                                ("Open Source on GitHub".into(), Act::Open(format!("https://github.com/raycast/extensions/tree/main/extensions/{}", s.name))),
                            ];
                            it
                        })
                        .collect();
                    // ponytail: alphabetical; the index has no download counts to rank by.
                    search::filter(&query, rows, &mut self.matcher)
                }
            },
            Mode::Running => match &self.running {
                None => vec![empty_row(g::CLOCK, "Looking for running apps…".into(), String::new(), vec![])],
                Some(list) => {
                    let rows: Vec<Item> = list
                        .iter()
                        .map(|(r, icon)| {
                            let file = std::path::Path::new(&r.exe).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                            let windows = if r.windows.len() > 1 { format!("{} windows", r.windows.len()) } else { String::new() };
                            let name = if r.name.is_empty() { file.clone() } else { r.name.clone() };
                            // "Reddit - Google Chrome" -> "Reddit"; nothing when it just repeats the name.
                            let mut title = r.title.as_str();
                            for sep in [" - ", " – ", " — "] {
                                if let Some((head, tail)) = title.rsplit_once(sep)
                                    && name.split_whitespace().any(|w| tail.contains(w))
                                {
                                    title = head;
                                }
                            }
                            let title = if title == name { "" } else { title };
                            Item::new(&name, title, icon.clone().map(Icon::Image).unwrap_or(Icon::Vector(g::APP)), Act::QuitApp(r.pid))
                                .acc(windows)
                                .with_hay(format!("{name} {} {file}", r.title))
                        })
                        .collect();
                    search::filter(&query, rows, &mut self.matcher)
                }
            },
            Mode::Install => {
                let what = query.trim();
                let title = if what.is_empty() { "Install an extension".to_string() } else { format!("Install {what}") };
                vec![empty_row(g::DOWNLOAD, title, "A store name (e.g. obsidian) or a folder path, then Enter · needs git, Node and npm".into(), vec![])]
            }
        };
        if self.items.is_empty() && self.mode == Mode::Clipboard {
            let hint = if query.trim().is_empty() { "Text, images and files you copy show up here" } else { "Nothing copied matches" };
            self.items = vec![empty_row(g::PASTE, if query.trim().is_empty() { "Nothing copied yet" } else { "No matches" }.into(), hint.into(), vec![])];
        }
        if self.items.is_empty() && self.mode == Mode::Emoji {
            self.items = vec![empty_row(g::EMOJI, "No emoji found".into(), "Try another word, e.g. heart or smile".into(), vec![])];
        }
        if self.mode == Mode::Emoji {
            self.grid_list.reset(grid_rows(&self.items, self.cfg.emoji.columns).len());
        }
        let _ = max;
        self.selected = if keep_selection { self.selected.min(self.items.len().saturating_sub(1)) } else { 0 };
        self.scroll_to_selected();
        self.request_resize(cx);
        cx.notify();
    }

    /// Scrolls the selected row into view (section headers take list slots too).
    fn scroll_to_selected(&self) {
        let at = display_rows(&self.items).iter().position(|r| *r == Some(self.selected)).unwrap_or(0);
        self.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
    }

    fn rebuild_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog = search::catalog(&self.apps, &self.scripts, &self.exts, &self.cfg);
        if self.mode == Mode::Root {
            self.refresh(cx);
        }
    }

    /// Off-thread. The cached list (with icons) is used at once; a fresh scan of names
    /// follows (~1 s), and only apps without a known icon get one extracted.
    /// `fresh_icons` re-extracts every icon (Reload Apps).
    fn scan_apps(&mut self, force: bool, fresh_icons: bool) {
        if !force && self.scanned.is_some_and(|t| t.elapsed() < Duration::from_secs(300)) {
            return;
        }
        self.scanned = Some(Instant::now());
        let tx = self.tx.clone();
        let cache = config::dir().join("apps.bin");
        let (folders, deep) = (self.cfg.apps.folders.clone(), self.cfg.apps.deep);
        let mut known = if fresh_icons { vec![] } else { self.apps.clone() };
        std::thread::spawn(move || {
            if known.is_empty() && !fresh_icons {
                known = win::load_apps(&cache);
                if !known.is_empty() {
                    let _ = tx.unbounded_send(Msg::Apps(known.clone()));
                }
            }
            let mut apps = win::scan_apps();
            apps.extend(apps::folder_apps(&folders, deep));
            for app in &mut apps {
                app.icon = known.iter().find(|k| k.id == app.id).and_then(|k| k.icon.clone());
            }
            let same = |a: &[win::App], b: &[win::App]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.id == y.id && x.name == y.name);
            if apps.iter().any(|a| a.icon.is_none()) {
                let _ = tx.unbounded_send(Msg::Apps(apps.clone()));
                win::load_icons(&mut apps);
            } else if same(&apps, &known) {
                return;
            }
            let _ = tx.unbounded_send(Msg::Apps(apps.clone()));
            let _ = win::save_apps(&cache, &apps);
        });
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if mode != Mode::Ext {
            self.stop_ext_view();
        }
        if mode != Mode::Ai && self.ai.take().is_some() {
            self.send(json!({"t": "aiStop"}));
        }
        self.mode = mode;
        self.panel = None;
        if mode == Mode::Files
            && let Some(f) = &self.files
        {
            f.refresh_if_stale(Duration::from_secs(3600));
        }
        if mode == Mode::Store {
            self.load_store();
        }
        if mode == Mode::Running {
            self.load_running();
        }
        self.input.update(cx, |s, cx| s.emplace("", cx));
        self.selected = 0;
        self.refresh(cx);
    }

    fn show(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        window.set_background_appearance(backdrop(design::refresh_glass()).into());
        let fg = win::foreground();
        if fg != self.hwnd {
            self.prev = fg;
        }
        // Script headers are a handful of small files; re-reading them keeps edits live.
        let scripts = scripts::scan(&self.cfg.scripts.dirs());
        if scripts != self.scripts {
            self.scripts = scripts;
            self.catalog = search::catalog(&self.apps, &self.scripts, &self.exts, &self.cfg);
        }
        self.status = None;
        // Within the pop-to-root timeout the launcher reopens where it was.
        if mode == Mode::Root && self.keeps_state() {
            self.refresh(cx);
        } else {
            self.peek = false;
            self.search_pos = None;
            self.set_mode(mode, cx);
        }
        if let Ok(view) = std::env::var("PROCAST_SHOW") {
            let mode = match view.as_str() {
                "clipboard" => Mode::Clipboard,
                "emoji" => Mode::Emoji,
                "files" => Mode::Files,
                "running" => Mode::Running,
                _ => Mode::Root,
            };
            self.set_mode(mode, cx);
            let q = std::env::var("PROCAST_QUERY").unwrap_or_default();
            self.input.update(cx, |s, cx| s.emplace(&q, cx));
        }
        win::cloak(self.hwnd, false);
        win::focus(self.hwnd);
        window.activate();
        window.focus(&self.input.focus_handle(cx), cx);
        self.visible = true;
        self.scan_apps(false, false);
        // The window was placed before this view existed (a notice, a kept search or a
        // query can each change its height), so size it to what it shows now. Without this,
        // `height` could claim a size the window doesn't have, and typing in compact mode
        // would never grow it. win::resize does nothing when the size is already right.
        self.height = 0.;
        self.request_resize(cx);
    }

    /// `restore` hands focus back to the window that was active before we opened;
    /// false only when focus already moved elsewhere (deactivation).
    fn hide(&mut self, restore: bool, cx: &mut Context<Self>) {
        win::cloak(self.hwnd, true);
        self.visible = false;
        self.hidden_at = Some(Instant::now());
        self.panel = None;
        // ponytail: an extension view ends when the window closes (Raycast keeps it ~90 s).
        self.stop_ext_view();
        if self.mode == Mode::Ext {
            self.mode = Mode::Root;
        }
        if restore && !self.prev.is_invalid() {
            win::focus(self.prev);
        }
        cx.notify();
    }

    fn paste(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.hide(true, cx);
        clipboard::paste_soon();
    }

    // ---- clipboard history and snippets --------------------------------------------

    /// Puts a history entry (text, image or files) back on the clipboard.
    fn put_clip(&mut self, key: &str) -> bool {
        let Some(e) = self.history.get(key) else { return false };
        match &e.kind {
            clipboard::Kind::Text => clipboard::set(self.hwnd, clipboard::Put::Text(&e.text), false),
            clipboard::Kind::Image { id, .. } => self.history.png(id).is_some_and(|png| clipboard::set(self.hwnd, clipboard::Put::Image(&png), false)),
            clipboard::Kind::Files(f) => clipboard::set(self.hwnd, clipboard::Put::Files(f), false),
        }
    }

    /// A clipboard image fitted for the detail pane, cached.
    fn clip_preview(&mut self, id: &str) -> Option<Arc<RenderImage>> {
        let history = &self.history;
        self.clip_images.entry(format!("p:{id}")).or_insert_with(|| history.png(id).and_then(|p| clipboard::render_image(&p, 640))).clone()
    }

    /// A snippet chosen in the launcher (`erase` 0) or typed in an app: asks for its
    /// arguments first if it has any, then goes into the app.
    fn start_snippet(&mut self, name: String, template: String, erase: usize, then: String, window: &mut Window, cx: &mut Context<Self>) {
        let args = snippets::arguments(&template);
        if !args.is_empty() {
            self.pending = Some(Pending::Snippet { name, template, args, got: vec![], erase, then });
            if !self.visible {
                self.show(Mode::Args, window, cx);
            }
            return self.set_mode(Mode::Args, cx);
        }
        self.insert_snippet(template, vec![], erase, then, cx);
    }

    fn insert_snippet(&mut self, template: String, args: Vec<String>, erase: usize, then: String, cx: &mut Context<Self>) {
        let opened = self.visible;
        if opened {
            self.hide(true, cx);
        }
        let s = &self.cfg.snippets;
        // From the launcher: the app needs a moment to get focus back first.
        let pause = if opened { snippets::pause(&s.speed).max(Duration::from_millis(80)) } else { snippets::pause(&s.speed) };
        let sound = if erase > 0 { snippets::sound(&s.sound) } else { None };
        snippets::insert(snippets::Job { owner: self.hwnd, template, args, erase, then, pause, sound });
    }

    /// Whether a typed keyword should expand where it was typed.
    fn expands_here(&self) -> bool {
        !snippets::procast_focused() && !self.cfg.snippets.ignore_apps.iter().any(|a| a.eq_ignore_ascii_case(&clipboard::exe_of(win::foreground())))
    }

    /// The snippet a typed keyword asks for, when it must open the launcher for its arguments.
    fn expand_opens(&self, hit: &snippets::Hit) -> bool {
        matches!(hit, snippets::Hit::Snippet { index, .. } if self.cfg.snippet.get(*index).is_some_and(|s| !snippets::arguments(&s.text).is_empty())) && self.expands_here()
    }

    fn on_expand(&mut self, hit: snippets::Hit, window: &mut Window, cx: &mut Context<Self>) {
        if !self.expands_here() {
            return;
        }
        match hit {
            snippets::Hit::Emoji { emoji, erase } => {
                let e = search::toned(emoji, &self.cfg.emoji.skin_tone).to_string();
                self.insert_snippet(e, vec![], erase, String::new(), cx);
            }
            snippets::Hit::Snippet { index, erase, then } => {
                let Some(s) = self.cfg.snippet.get(index).cloned() else { return };
                let fg = win::foreground();
                if fg != self.hwnd {
                    self.prev = fg;
                }
                self.start_snippet(s.name, s.text, erase, then, window, cx);
            }
        }
    }

    // ---- Quick AI -------------------------------------------------------------------

    /// Opens an empty chat and starts Claude, so it is ready by the time a question is typed.
    fn open_ai(&mut self, cx: &mut Context<Self>) {
        if !self.ensure_host() {
            return self.refresh(cx);
        }
        self.status = None;
        self.send(json!({"t": "aiOpen", "model": self.cfg.ai.model}));
        self.ai = Some(AiChat::default());
        self.set_mode(Mode::Ai, cx);
    }

    /// Asks in the open chat (or starts one); earlier questions stay as context.
    fn ask(&mut self, prompt: String, cx: &mut Context<Self>) {
        if !self.ensure_host() {
            return self.refresh(cx);
        }
        self.next_run += 1;
        let id = self.next_run;
        self.status = None;
        self.send(json!({"t": "ai", "id": id, "prompt": prompt, "model": self.cfg.ai.model}));
        let chat = self.ai.get_or_insert_default();
        chat.turns.push(AiTurn { id, prompt, text: String::new() });
        chat.busy = true;
        chat.follow = true;
        self.set_mode(Mode::Ai, cx);
    }

    fn on_ai(&mut self, v: &Value, cx: &mut Context<Self>) {
        if v["t"] == "aiClosed" {
            // The sidecar closed an idle chat. Out of sight it just ends; in view the
            // answers stay, but the next question starts without their context.
            if self.mode == Mode::Ai && !self.visible {
                self.set_mode(Mode::Root, cx);
            } else if self.ai.as_ref().is_some_and(|a| !a.turns.is_empty()) {
                self.status = Some((Tone::Info, "Idle chat closed: the next question starts a new one".into()));
                self.refresh(cx);
            }
            return;
        }
        // Only the newest question's answer is still wanted.
        let Some(chat) = self.ai.as_mut().filter(|a| a.turns.last().map(|t| t.id) == v["id"].as_u64()) else { return };
        if let (Some(text), Some(turn)) = (v["text"].as_str(), chat.turns.last_mut()) {
            turn.text.push_str(text);
        }
        if v["t"] == "aiDone" {
            chat.busy = false;
            if let Some(e) = v["error"].as_str() {
                self.status = Some((Tone::Error, e.to_string()));
            }
        }
        self.refresh(cx);
    }

    /// Up and Down read the chat.
    fn scroll_ai(&mut self, dy: f32, cx: &mut Context<Self>) {
        if let Some(chat) = &mut self.ai {
            chat.follow = false;
        }
        let (offset, max) = (self.ai_scroll.offset(), self.ai_scroll.max_offset());
        let y = (offset.y - px(dy)).min(px(0.)).max(-max.y);
        self.ai_scroll.set_offset(gpui::point(offset.x, y));
        cx.notify();
    }

    // ---- extensions -----------------------------------------------------------------

    fn send(&mut self, msg: Value) {
        if let Some(h) = &mut self.host
            && !h.send(msg)
        {
            self.host = None;
        }
    }

    fn ensure_host(&mut self) -> bool {
        if self.host.is_none() {
            match ext::Host::spawn(&self.cfg.extensions.node, self.tx.clone()) {
                Ok(h) => self.host = Some(h),
                Err(e) => {
                    self.notice = Some(e);
                    return false;
                }
            }
        }
        true
    }

    fn stop_ext_view(&mut self) {
        if let Some(ev) = self.extv.take() {
            self.send(json!({"t": "stop", "run": ev.run}));
        }
        self.confirm = None;
    }

    /// Starts an extension command. `background` runs have no UI.
    fn start_ext(&mut self, e: usize, c: usize, background: bool, args: Value, cx: &mut Context<Self>) {
        let (ext, cmd) = (self.exts[e].0.clone(), self.exts[e].0.commands[c].clone());
        let prefs = match ext::preference_values(&ext, &cmd, self.cfg.extension.get(&ext.name)) {
            Ok(p) => p,
            Err(missing) => {
                if !background {
                    self.notice = Some(format!("{} needs: {}. Add them under [extension.{}] in config.toml", ext.title, missing.join(", "), ext.name));
                    cx.notify();
                }
                return;
            }
        };
        if !self.ensure_host() {
            return cx.notify();
        }
        self.next_run += 1;
        let run = self.next_run;
        let view = (cmd.mode == "view" || cmd.mode == "menu-bar") && !background;
        self.send(ext::run_msg(run, &ext, &cmd, prefs, args, background, self.cfg.extensions.memory_mb));
        if view {
            self.stop_ext_view();
            self.extv = Some(ExtView { run, ext: e, view: ext::View::default(), depth: 1, values: HashMap::new(), inputs: HashMap::new(), field: 0, sent_query: None, sent_recent: vec![], _subs: vec![] });
            self.mode = Mode::Ext;
            self.panel = None;
            self.input.update(cx, |s, cx| s.emplace("", cx));
            self.selected = 0;
            self.refresh(cx);
        } else {
            self.bg_runs.insert(run, format!("{}/{}", ext.name, cmd.name));
            if !background {
                self.hide(true, cx);
            }
        }
    }

    /// Starts background work that is due: interval no-view commands, and menu-bar
    /// commands (started once, re-run on their interval to refresh the tray menu).
    fn tick(&mut self, cx: &mut Context<Self>) {
        let running = |this: &Self, key: &str| this.bg_runs.iter().find(|(_, k)| *k == key).map(|(r, _)| *r);
        let mut due = vec![];
        for (e, (x, _)) in self.exts.iter().enumerate() {
            for (c, cmd) in x.commands.iter().enumerate() {
                let key = format!("{}/{}", x.name, cmd.name);
                let elapsed = |i: Duration| self.bg_last.get(&key).is_none_or(|t| t.elapsed() >= i);
                let wanted = match cmd.mode.as_str() {
                    "no-view" => self.cfg.extensions.background && cmd.interval.is_some_and(elapsed) && running(self, &key).is_none(),
                    "menu-bar" => self.cfg.extensions.menu_bar && (running(self, &key).is_none() || cmd.interval.is_some_and(elapsed)),
                    _ => false,
                };
                if wanted {
                    due.push((e, c, key));
                }
            }
        }
        for (e, c, key) in due {
            // A menu-bar command is re-run fresh; its tray icon stays and gets the new menu.
            if let Some(old) = running(self, &key) {
                self.bg_runs.remove(&old);
                self.send(json!({"t": "stop", "run": old}));
            }
            self.bg_last.insert(key, Instant::now());
            self.start_ext(e, c, true, json!({}), cx);
        }
    }

    /// Draws a menu-bar command's render as its tray icon + menu.
    fn update_tray(&mut self, run: u64, key: String, tree: &Value) {
        use tray_icon::menu::{IsMenuItem, MenuId, PredefinedMenuItem, Submenu};
        let Some(bar) = ext::parse_menubar(tree) else { return };
        let Some(e) = self.exts.iter().position(|(x, _)| key.starts_with(&format!("{}/", x.name))) else { return };
        let ext = &self.exts[e].0;
        let cmd_title = ext.commands.iter().find(|c| key.ends_with(&format!("/{}", c.name))).map(|c| c.title.clone()).unwrap_or_default();

        fn add(nodes: &[ext::MenuNode], run: u64, push: &dyn Fn(&dyn IsMenuItem)) {
            for (i, n) in nodes.iter().enumerate() {
                match n {
                    ext::MenuNode::Item { title, on_action } => {
                        let id = MenuId::new(format!("ext|{run}|{}", on_action.clone().unwrap_or_default()));
                        push(&MenuItem::with_id(id, title, on_action.is_some(), None));
                    }
                    ext::MenuNode::Separator => push(&PredefinedMenuItem::separator()),
                    ext::MenuNode::Section { title, children } => {
                        if i > 0 {
                            push(&PredefinedMenuItem::separator());
                        }
                        if !title.is_empty() {
                            push(&MenuItem::new(title, false, None));
                        }
                        add(children, run, push);
                    }
                    ext::MenuNode::Submenu { title, children } => {
                        let sub = Submenu::new(title, true);
                        add(children, run, &|item| drop(sub.append(item)));
                        push(&sub);
                    }
                }
            }
        }
        let menu = Menu::new();
        if !bar.title.is_empty() {
            let _ = menu.append(&MenuItem::new(&bar.title, false, None));
            let _ = menu.append(&PredefinedMenuItem::separator());
        }
        add(&bar.items, run, &|item| drop(menu.append(item)));

        let icon_path = bar
            .icon
            .as_deref()
            .filter(|i| !i.starts_with("icon:") && !i.starts_with("http") && i.contains('.'))
            .map(|i| ext.dir.join("assets").join(i))
            .filter(|p| p.exists())
            .or_else(|| ext.icon.clone());
        let icon = icon_path.and_then(|p| ext::load_rgba(&p)).and_then(|(rgba, w, h)| TrayImage::from_rgba(rgba, w, h).ok());
        let tooltip = [bar.tooltip.as_str(), bar.title.as_str(), cmd_title.as_str()].into_iter().find(|t| !t.is_empty()).unwrap_or("procast").to_string();

        match self.trays.get_mut(&key) {
            Some((r, tray)) => {
                *r = run;
                tray.set_menu(Some(Box::new(menu)));
                let _ = tray.set_tooltip(Some(&tooltip));
                if let Some(i) = icon {
                    let _ = tray.set_icon(Some(i));
                }
            }
            None => {
                let built = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip(&tooltip).with_icon(icon.unwrap_or_else(tray_icon_image)).build();
                if let Ok(tray) = built {
                    self.trays.insert(key, (run, tray));
                }
            }
        }
    }

    /// "extension/command" for log lines.
    fn run_name(&self, run: u64) -> String {
        if let Some(k) = self.bg_runs.get(&run) {
            return k.clone();
        }
        match &self.extv {
            Some(e) if e.run == run => self.exts[e.ext].0.name.clone(),
            _ => format!("run {run}"),
        }
    }

    fn reply(&mut self, run: u64, id: u64, value: Result<Value, String>) {
        let msg = match value {
            Ok(v) => json!({"t": "reply", "run": run, "id": id, "value": v}),
            Err(e) => json!({"t": "reply", "run": run, "id": id, "error": e}),
        };
        self.send(msg);
    }

    fn on_ext(&mut self, v: Value, window: &mut Window, cx: &mut Context<Self>) {
        let run = v["run"].as_u64().unwrap_or(0);
        let ui = self.extv.as_ref().is_some_and(|e| e.run == run);
        match v["t"].as_str().unwrap_or_default() {
            "render" if ui => {
                let view = ext::parse_view(&v["tree"]);
                let first_form = view.kind == "Form" && self.extv.as_ref().is_some_and(|e| e.view.kind != "Form");
                if let Some(ev) = &mut self.extv {
                    ev.view = view;
                    ev.depth = v["depth"].as_u64().unwrap_or(1);
                }
                self.sync_form(first_form, window, cx);
                if let Some(text) = self.extv.as_ref().and_then(|e| e.view.search_text.clone())
                    && text != self.query(cx)
                    && !self.extv.as_ref().is_some_and(|e| e.sent_recent.contains(&text) || (text.is_empty() && e.sent_query.is_none()))
                {
                    // Controlled search text: the extension owns it, so don't echo it back.
                    if let Some(ev) = &mut self.extv {
                        ev.sent_query = Some(text.clone());
                    }
                    self.input.update(cx, |s, cx| s.emplace(&text, cx));
                }
                self.send_search(cx);
                self.refresh(cx);
            }
            "render" => {
                if let Some(key) = self.bg_runs.get(&run).cloned()
                    && self.exts.iter().any(|(x, _)| x.commands.iter().any(|c| c.mode == "menu-bar" && key == format!("{}/{}", x.name, c.name)))
                {
                    self.update_tray(run, key, &v["tree"]);
                    cx.notify();
                }
            }
            "call" => self.on_call(run, ui, v["method"].as_str().unwrap_or_default(), &v["params"], cx),
            "request" => {
                let id = v["id"].as_u64().unwrap_or(0);
                let p = &v["params"];
                match v["method"].as_str().unwrap_or_default() {
                    "confirm" if ui => {
                        self.confirm = Some(Confirm {
                            run,
                            id,
                            title: p["title"].as_str().unwrap_or("Are you sure?").into(),
                            message: p["message"].as_str().unwrap_or_default().into(),
                            primary: p["primary"].as_str().unwrap_or("Confirm").into(),
                            danger: p["destructive"].as_bool().unwrap_or(false),
                        });
                        self.selected = 0;
                        self.refresh(cx);
                    }
                    "readClipboard" => {
                        let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                        self.reply(run, id, Ok(json!(text)));
                    }
                    "getApplications" => {
                        let apps: Vec<Value> = self.apps.iter().map(|a| json!({"name": a.name, "path": format!("shell:AppsFolder\\{}", a.id), "bundleId": a.id})).collect();
                        self.reply(run, id, Ok(json!(apps)));
                    }
                    m => self.reply(run, id, Err(format!("{m} is not supported in procast yet"))),
                }
            }
            "error" if ui => {
                let msg = v["message"].as_str().unwrap_or_default();
                self.notice = Some(msg.lines().next().unwrap_or(msg).chars().take(200).collect());
                ext_log(&self.run_name(run), msg);
                cx.notify();
            }
            "error" | "log" => ext_log(&self.run_name(run), v["message"].as_str().or(v["text"].as_str()).unwrap_or_default()),
            "exit" => {
                if let Some(e) = v["error"].as_str() {
                    ext_log(&self.run_name(run), e);
                    if !self.bg_runs.contains_key(&run) || ui {
                        self.notice = Some(e.lines().next().unwrap_or(e).chars().take(200).collect());
                    }
                }
                self.bg_runs.remove(&run);
                if ui {
                    self.extv = None;
                    if self.mode == Mode::Ext {
                        self.set_mode(Mode::Root, cx);
                    }
                }
            }
            "aiDelta" | "aiDone" | "aiClosed" => self.on_ai(&v, cx),
            "hostExit" => {
                self.host = None;
                self.bg_runs.clear();
                if let Some(chat) = self.ai.as_mut().filter(|a| a.busy) {
                    chat.busy = false;
                    self.status = Some((Tone::Error, "The extension host stopped".into()));
                    self.refresh(cx);
                }
                if self.extv.take().is_some() {
                    self.notice = Some("The extension host stopped".into());
                    self.set_mode(Mode::Root, cx);
                }
            }
            _ => {}
        }
    }

    fn on_call(&mut self, run: u64, ui: bool, method: &str, p: &Value, cx: &mut Context<Self>) {
        let text = |k: &str| p[k].as_str().unwrap_or_default().to_string();
        match method {
            "copy" => {
                cx.write_to_clipboard(ClipboardItem::new_string(text("content")));
                if ui && p["close"].as_bool().unwrap_or(false) {
                    self.hide(true, cx);
                }
            }
            "paste" => self.paste(text("content"), cx),
            "open" => {
                win::open(&text("target"));
                if ui {
                    self.hide(true, cx);
                }
            }
            "reveal" => {
                win::reveal(&text("path"));
                self.hide(true, cx);
            }
            "toast" if ui => {
                self.status = (!p["hide"].as_bool().unwrap_or(false)).then(|| {
                    let (t, m) = (text("title"), text("message"));
                    let tone = match p["style"].as_str().unwrap_or_default().to_lowercase().as_str() {
                        "failure" => Tone::Error,
                        "success" => Tone::Ok,
                        _ => Tone::Info,
                    };
                    (tone, if m.is_empty() { t } else { format!("{t}: {m}") })
                });
                cx.notify();
            }
            "hud" if ui => self.hide(true, cx),
            "closeMainWindow" if ui => self.hide(true, cx),
            "popToRoot" if ui => self.set_mode(Mode::Root, cx),
            "clearSearchBar" if ui => self.input.update(cx, |s, cx| s.emplace("", cx)),
            "openPreferences" => win::open(&config::path().to_string_lossy()),
            "launchCommand" => {
                let ext = self.extv.as_ref().filter(|e| e.run == run).map(|e| e.ext).or_else(|| {
                    let key = self.bg_runs.get(&run)?;
                    self.exts.iter().position(|(x, _)| key.starts_with(&format!("{}/", x.name)))
                });
                if let Some(e) = ext
                    && let Some(c) = self.exts[e].0.commands.iter().position(|c| c.name == text("name"))
                {
                    self.start_ext(e, c, p["type"] == "background", p["arguments"].clone(), cx);
                }
            }
            "trash" if ui => {
                self.status = Some((Tone::Error, "Move to Recycle Bin isn't supported yet".into()));
                cx.notify();
            }
            _ => {}
        }
    }

    /// Keeps form inputs in step with the extension's fields.
    fn sync_form(&mut self, focus_first: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ev) = &mut self.extv else { return };
        if ev.view.kind != "Form" {
            return;
        }
        let run = ev.run;
        for f in ev.view.fields.clone() {
            if f.controlled || !ev.values.contains_key(&f.id) {
                ev.values.insert(f.id.clone(), f.value.clone());
            }
            if !matches!(f.kind, FieldKind::Text | FieldKind::Password | FieldKind::TextArea) {
                continue;
            }
            let want = f.value.as_str().unwrap_or_default().to_string();
            match ev.inputs.get(&f.id) {
                Some(input) => {
                    if f.controlled && input.read(cx).as_str() != want {
                        input.update(cx, |s, cx| s.emplace(&want, cx));
                    }
                }
                None => {
                    let input = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
                    input.update(cx, |s, cx| s.emplace(&want, cx));
                    let id = f.id.clone();
                    ev._subs.push(cx.subscribe(&input, move |this, input, _: &TextChanged, cx| {
                        let value = input.read(cx).as_str().to_string();
                        let Some(ev) = &mut this.extv else { return };
                        if ev.run != run {
                            return;
                        }
                        ev.values.insert(id.clone(), json!(value));
                        if let Some(f) = ev.view.fields.iter().find(|f| f.id == id).and_then(|f| f.on_change.clone()) {
                            this.send(json!({"t": "event", "run": run, "fn": f, "args": [value]}));
                        }
                    }));
                    ev.inputs.insert(f.id.clone(), input);
                }
            }
        }
        if focus_first {
            ev.field = ev.view.fields.iter().position(|f| !matches!(f.kind, FieldKind::Description | FieldKind::Separator)).unwrap_or(0);
            self.focus_field(window, cx);
        }
    }

    fn focus_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ev) = &self.extv else { return };
        let handle: FocusHandle = ev
            .view
            .fields
            .get(ev.field)
            .and_then(|f| ev.inputs.get(&f.id))
            .map(|i| i.focus_handle(cx))
            .unwrap_or_else(|| self.input.focus_handle(cx));
        window.focus(&handle, cx);
        cx.notify();
    }

    fn move_field(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ev) = &mut self.extv else { return };
        let n = ev.view.fields.len() as isize;
        let mut i = ev.field as isize;
        for _ in 0..n {
            i = (i + delta).clamp(0, n - 1);
            if !matches!(ev.view.fields[i as usize].kind, FieldKind::Description | FieldKind::Separator) {
                break;
            }
        }
        ev.field = i.max(0) as usize;
        self.focus_field(window, cx);
    }

    fn submit_form(&mut self, cx: &mut Context<Self>) {
        let Some(ev) = &self.extv else { return };
        if let Some(f) = ev.view.actions.first().and_then(|a| a.on_action.clone()) {
            let (run, values) = (ev.run, json!(ev.values));
            self.send(json!({"t": "event", "run": run, "fn": f, "args": [values]}));
        }
        cx.notify();
    }

    fn is_form(&self) -> bool {
        self.mode == Mode::Ext && self.confirm.is_none() && self.extv.as_ref().is_some_and(|e| e.view.kind == "Form")
    }

    // ---- actions --------------------------------------------------------------------

    /// Enter on the selected row (or on the highlighted Ctrl+K action).
    fn launch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((item, i)) = self.panel.take() {
            let act = item.actions().swap_remove(i).1;
            return self.run(act, item.key.clone(), window, cx);
        }
        if self.mode == Mode::Install {
            let what = self.query(cx).trim().to_string();
            return self.install(what, cx);
        }
        if self.is_form() {
            let ev = self.extv.as_mut().unwrap();
            let field = ev.view.fields.get(ev.field).cloned();
            match field.map(|f| (f.kind.clone(), f)) {
                Some((FieldKind::Checkbox, f)) => {
                    let on = !ev.values.get(&f.id).and_then(Value::as_bool).unwrap_or(false);
                    ev.values.insert(f.id.clone(), json!(on));
                    if let Some(h) = f.on_change {
                        let run = ev.run;
                        self.send(json!({"t": "event", "run": run, "fn": h, "args": [on]}));
                    }
                    return cx.notify();
                }
                Some((FieldKind::Dropdown(opts), f)) => {
                    let mut it = Item::new(&f.title, "", Icon::glyph("▾"), Act::None);
                    it.extra = opts.into_iter().map(|(v, t)| (t, Act::FormSet(f.id.clone(), v))).collect();
                    self.panel = Some((it, 0));
                    return cx.notify();
                }
                _ => {
                    let last = ev.view.fields.iter().rposition(|f| !matches!(f.kind, FieldKind::Description | FieldKind::Separator));
                    if Some(ev.field) == last { self.submit_form(cx) } else { self.move_field(1, window, cx) }
                    return;
                }
            }
        }
        let Some(item) = self.items.get(self.selected).cloned() else { return };
        if self.mode == Mode::Args {
            return self.next_argument(cx);
        }
        let needs_confirm = match item.act {
            Act::Cmd(cmd) => cmd.destructive(),
            Act::Script(i) => self.scripts[i].confirm,
            _ => false,
        };
        if needs_confirm && self.armed != Some(self.selected) {
            self.armed = Some(self.selected);
            return cx.notify();
        }
        self.run(item.act, item.key, window, cx);
    }

    fn run(&mut self, act: Act, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let housekeeping = matches!(act, Act::Favorite(..) | Act::Hide(_));
        if let Some(key) = key.as_ref().filter(|_| !housekeeping) {
            self.frec.record(key, search::now());
        }
        if self.visible && self.mode == Mode::Root && self.cfg.launcher.history && !housekeeping {
            let q = self.query(cx).trim().to_string();
            if !q.is_empty() {
                self.remember_search(q);
            }
        }
        match act {
            Act::Favorite(key, add) => {
                let mut list = self.cfg.launcher.favorites.clone();
                list.retain(|k| *k != key);
                if add {
                    list.push(key);
                }
                self.status = Some((Tone::Ok, if add { "Added to Favorites" } else { "Removed from Favorites" }.into()));
                return self.save_launcher_list("favorites", list, cx);
            }
            Act::Hide(key) => {
                let mut list = self.cfg.launcher.hidden.clone();
                if !list.contains(&key) {
                    list.push(key);
                }
                self.status = Some((Tone::Info, "Hidden from root search. Undo in Settings → Launcher".into()));
                return self.save_launcher_list("hidden", list, cx);
            }
            Act::Cmd(Cmd::ClearSearchHistory) => {
                self.searches.clear();
                let _ = std::fs::remove_file(config::dir().join("search-history.txt"));
            }
            Act::App(id) => win::launch_app(&id),
            Act::RunAsAdmin(id) => win::run_as_admin(&id),
            Act::Open(target) | Act::File(target) => win::open(&target),
            Act::Reveal(path) => win::reveal(&path),
            Act::Run(cmdline) => win::run(&cmdline),
            Act::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Act::Paste(text) => return self.paste(text, cx),
            Act::Clip(key) => {
                if self.put_clip(&key) {
                    self.hide(true, cx);
                    return clipboard::paste_soon();
                }
                self.status = Some((Tone::Error, "Could not put that on the clipboard".into()));
                return cx.notify();
            }
            Act::CopyClip(key) => {
                if !self.put_clip(&key) {
                    self.status = Some((Tone::Error, "Could not put that on the clipboard".into()));
                    return cx.notify();
                }
            }
            Act::PinClip(key, on) => {
                self.history.pin(&key, on);
                self.status = Some((Tone::Ok, if on { "Pinned" } else { "Unpinned" }.into()));
                return self.refresh(cx);
            }
            Act::DeleteClip(key) => {
                self.history.remove(&key);
                self.status = Some((Tone::Ok, "Deleted from clipboard history".into()));
                return self.refresh(cx);
            }
            Act::Snippet(template) => {
                let name = key.as_deref().and_then(|k| k.strip_prefix("sn:")).unwrap_or("snippet").to_string();
                return self.start_snippet(name, template, 0, String::new(), window, cx);
            }
            Act::Script(i) => {
                if self.scripts[i].args.is_empty() {
                    return self.start_script(i, vec![], cx);
                }
                self.pending = Some(Pending::Script(i, vec![]));
                return self.set_mode(Mode::Args, cx);
            }
            Act::Fill(text) => return self.input.update(cx, |s, cx| s.emplace(&text, cx)),
            Act::ExtRun(e, c) => return self.start_ext(e, c, false, json!({}), cx),
            Act::ExtFn(f) => {
                if let Some(ev) = &self.extv {
                    let (run, values) = (ev.run, json!(ev.values));
                    self.send(json!({"t": "event", "run": run, "fn": f, "args": [values]}));
                }
                return;
            }
            Act::ExtSet(f, v) => {
                if let Some(ev) = &self.extv {
                    let run = ev.run;
                    self.send(json!({"t": "event", "run": run, "fn": f, "args": [v]}));
                }
                return;
            }
            Act::FormSet(id, v) => {
                if let Some(ev) = &mut self.extv {
                    ev.values.insert(id.clone(), json!(v));
                    if let Some(h) = ev.view.fields.iter().find(|f| f.id == id).and_then(|f| f.on_change.clone()) {
                        let run = ev.run;
                        self.send(json!({"t": "event", "run": run, "fn": h, "args": [v]}));
                    }
                }
                self.focus_field(window, cx);
                return;
            }
            Act::Confirm(ok) => {
                if let Some(c) = self.confirm.take() {
                    self.reply(c.run, c.id, Ok(json!(ok)));
                }
                return self.refresh(cx);
            }
            Act::UninstallExt(name) => {
                let dir = ext::dir().join(&name);
                if dir.starts_with(ext::dir()) && dir.join("package.json").exists() {
                    let _ = std::fs::remove_dir_all(&dir);
                }
                self.load_extensions();
                self.status = Some((Tone::Ok, format!("Uninstalled {name}")));
                return self.refresh(cx);
            }
            Act::None => return,
            Act::Cmd(Cmd::SearchFiles) => return self.set_mode(Mode::Files, cx),
            Act::Cmd(Cmd::ClipboardHistory) => return self.set_mode(Mode::Clipboard, cx),
            Act::Cmd(Cmd::Emoji) => return self.set_mode(Mode::Emoji, cx),
            Act::Cmd(Cmd::InstallExtension) => return self.set_mode(Mode::Install, cx),
            Act::Cmd(Cmd::BrowseStore) => return self.set_mode(Mode::Store, cx),
            Act::Cmd(Cmd::QuitApps) => return self.set_mode(Mode::Running, cx),
            Act::Cmd(Cmd::AskAi) => return self.open_ai(cx),
            Act::Ask(prompt) => return self.ask(prompt, cx),
            Act::QuitApp(pid) => {
                apps::quit(pid);
                self.status = Some((Tone::Info, "Asked to quit; it may ask to save first".into()));
                return self.load_running_after(Duration::from_millis(700));
            }
            Act::ForceQuit(pid) => {
                self.status = Some(if apps::force_quit(pid) { (Tone::Ok, "Force quit".into()) } else { (Tone::Error, "Could not end it (it may run as administrator)".into()) });
                return self.load_running_after(Duration::from_millis(300));
            }
            Act::Install(name) => return self.install(name, cx),
            Act::Cmd(Cmd::ClearClipboardHistory) => self.history.clear(),
            Act::Cmd(Cmd::Settings) => {
                self.hide(false, cx);
                return self.open_settings(cx);
            }
            Act::Cmd(Cmd::EditConfig) => win::open(&config::path().to_string_lossy()),
            Act::Cmd(Cmd::ReloadApps) => self.scan_apps(true, true),
            Act::Cmd(Cmd::Quit) => return cx.quit(),
            Act::Cmd(cmd) => {
                // Hide first so window commands act on the window you came from.
                self.hide(true, cx);
                let target = self.prev.0 as isize;
                std::thread::spawn(move || commands::run(cmd, HWND(target as _)));
                return;
            }
        }
        // Hand focus back even when launching: a cloaked launcher that stays foreground
        // would swallow keys typed before the new app's window appears.
        self.hide(true, cx);
    }

    /// Lists running apps in the background; icons come from the cache or are read once.
    fn load_running(&mut self) {
        self.load_running_after(Duration::ZERO);
    }

    fn load_running_after(&mut self, delay: Duration) {
        let (tx, known) = (self.tx.clone(), self.exe_icons.clone());
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let list = apps::running().into_iter().map(|r| {
                let icon = match known.get(&r.exe) {
                    Some(i) => i.clone(),
                    None => win::file_icon(&r.exe),
                };
                (r, icon)
            });
            let _ = tx.unbounded_send(Msg::Running(list.collect()));
        });
    }

    /// The command a hotkey past `BINDS` runs (None if it's switched off or gone).
    fn hotkey_item(&self, i: usize) -> Option<&Item> {
        let key = self.cfg.launcher.command_hotkeys().nth(i.checked_sub(BINDS.len())?)?.0;
        self.catalog.iter().find(|it| it.key.as_ref() == Some(key))
    }

    /// Whether running this needs the launcher on screen (it opens a view or asks for input).
    fn needs_window(&self, act: &Act) -> bool {
        match act {
            Act::Cmd(Cmd::SearchFiles | Cmd::ClipboardHistory | Cmd::Emoji | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::QuitApps | Cmd::AskAi) | Act::Fill(_) => true,
            Act::Script(i) => self.scripts.get(*i).is_some_and(|s| !s.args.is_empty() || matches!(s.output, Output::Full)),
            Act::ExtRun(e, c) => self.exts.get(*e).and_then(|(x, _)| x.commands.get(*c)).is_some_and(|c| c.mode == "view"),
            _ => false,
        }
    }

    fn hotkey_opens(&self, i: usize) -> bool {
        i < BINDS.len() || self.hotkey_item(i).is_some_and(|it| self.needs_window(&it.act))
    }

    /// A command's own hotkey: runs it straight away, opening the launcher only when it must.
    fn run_hotkey(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(it) = self.hotkey_item(i).cloned() else { return };
        // An app's hotkey brings its open window forward (or hides it) instead of starting another.
        if let Act::App(id) = &it.act
            && self.cfg.apps.hotkey_action != "launch"
        {
            let open = apps::windows_of(id, win::app_target(id).as_deref());
            let fg = win::foreground();
            if self.cfg.apps.hotkey_action == "toggle" && open.contains(&fg) {
                return apps::minimize(fg);
            }
            if let Some(&w) = open.first() {
                if self.visible {
                    self.hide(false, cx);
                }
                return apps::bring_forward(w);
            }
        }
        if self.needs_window(&it.act) {
            self.show(Mode::Root, window, cx);
        } else if !self.visible {
            let fg = win::foreground();
            if fg != self.hwnd {
                self.prev = fg;
            }
        }
        self.run(it.act, it.key, window, cx);
    }

    /// Brings up the settings window, reusing it if it's already open.
    fn open_settings(&mut self, cx: &mut Context<Self>) {
        let items = search::all_items(&self.apps, &self.scripts, &self.exts, &self.cfg);
        if let Some(h) = self.settings_window
            && h.update(cx, |s, window, _| {
                s.set_items(items.clone());
                window.activate()
            })
            .is_ok()
        {
            return;
        }
        let bounds = Bounds::centered(None, size(px(880.), px(620.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // Hidden: Settings draws its own caption bar in the theme (`settings::title_bar`).
            titlebar: Some(gpui::TitlebarOptions { title: Some("procast Settings".into()), appears_transparent: true, ..Default::default() }),
            windows_window_background: gpui::WindowsWindowBackground::MicaBackdrop,
            ..Default::default()
        };
        design::refresh_glass();
        self.settings_window = cx.open_window(options, |window, cx| cx.new(|cx| settings::Settings::new(items, window, cx))).ok();
    }

    /// Reads the store index in the background, syncing the sparse clone first when it's
    /// missing or more than a day old.
    fn load_store(&mut self) {
        if self.store_loading || self.store.is_some() {
            return;
        }
        let Some(host_dir) = ext::host_dir() else { return };
        self.store_loading = true;
        let (node, tx, dir) = (self.cfg.extensions.node.clone(), self.tx.clone(), ext::dir());
        std::thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            let repo = ext::store_repo();
            let fresh = std::fs::metadata(repo.join(".git").join("FETCH_HEAD")).or_else(|_| std::fs::metadata(repo.join(".git").join("HEAD")))
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|e| e < Duration::from_secs(86400)));
            if !fresh {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new(node)
                    .arg(host_dir.join("install.mjs"))
                    .args(["--sync", &dir.to_string_lossy()])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .creation_flags(0x0800_0000)
                    .status();
            }
            let _ = tx.unbounded_send(Msg::Store(ext::read_store(&repo)));
        });
    }

    fn install(&mut self, what: String, cx: &mut Context<Self>) {
        if what.is_empty() {
            return;
        }
        let Some(host_dir) = ext::host_dir() else {
            self.notice = Some("exthost folder not found next to procast.exe".into());
            return cx.notify();
        };
        let (node, tx, dir) = (self.cfg.extensions.node.clone(), self.tx.clone(), ext::dir());
        self.status = Some((Tone::Info, format!("Installing {what}…")));
        cx.notify();
        std::thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            let _ = std::fs::create_dir_all(&dir);
            let child = std::process::Command::new(node)
                .arg(host_dir.join("install.mjs"))
                .args([&what, &dir.to_string_lossy().into_owned()])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .creation_flags(0x0800_0000)
                .spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => return drop(tx.unbounded_send(Msg::Install(format!("could not start Node.js: {e}"), Some(false)))),
            };
            for line in BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok) {
                let v: Value = serde_json::from_str(&line).unwrap_or_default();
                let msg = v["message"].as_str().unwrap_or(&line).to_string();
                let _ = tx.unbounded_send(Msg::Install(msg, None));
            }
            let ok = child.wait().is_ok_and(|s| s.success());
            let _ = tx.unbounded_send(Msg::Install(if ok { format!("Installed {what}") } else { format!("Installing {what} failed") }, Some(ok)));
        });
    }

    fn next_argument(&mut self, cx: &mut Context<Self>) {
        let value = self.query(cx).trim().to_string();
        match self.pending.take() {
            None => {}
            Some(Pending::Script(i, mut got)) => {
                let spec = &self.scripts[i].args[got.len()];
                if value.is_empty() && !spec.optional {
                    self.status = Some((Tone::Error, format!("{} is required", spec.placeholder)));
                    self.pending = Some(Pending::Script(i, got));
                    return cx.notify();
                }
                got.push(value);
                if got.len() < self.scripts[i].args.len() {
                    self.pending = Some(Pending::Script(i, got));
                    self.status = None;
                    self.input.update(cx, |s, cx| s.emplace("", cx));
                    return self.refresh(cx);
                }
                self.start_script(i, got, cx);
            }
            Some(Pending::Snippet { name, template, args, mut got, erase, then }) => {
                got.push(value);
                if got.len() < args.len() {
                    self.pending = Some(Pending::Snippet { name, template, args, got, erase, then });
                    self.input.update(cx, |s, cx| s.emplace("", cx));
                    return self.refresh(cx);
                }
                self.mode = Mode::Root;
                self.insert_snippet(template, got, erase, then, cx);
            }
        }
    }

    fn start_script(&mut self, i: usize, args: Vec<String>, cx: &mut Context<Self>) {
        let script = self.scripts[i].clone();
        match script.output {
            Output::Full => {
                self.output = Some((i, String::new(), true));
                self.set_mode(Mode::Output, cx);
            }
            Output::Compact => {
                self.status = Some((Tone::Info, format!("Running {}…", script.title)));
                self.set_mode(Mode::Root, cx);
            }
            Output::Silent => self.hide(true, cx),
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let (ok, out) = scripts::run(&script, &args);
            let _ = tx.unbounded_send(Msg::ScriptDone(i, ok, out));
        });
    }

    /// The caller refreshes the glass decision first (it holds the window, for the backdrop).
    fn reload(&mut self, cx: &mut Context<Self>) {
        match config::load() {
            Ok(cfg) => {
                if hotkey_specs(&cfg) != hotkey_specs(&self.cfg) {
                    self.hotkey_errors.clear();
                    self.hotkeys.set(hotkey_specs(&cfg));
                }
                if key_settings(&cfg) != key_settings(&self.cfg) {
                    configure_keys(&cfg, &self.tx);
                }
                if cfg.clipboard != self.cfg.clipboard {
                    clipboard::configure(&cfg.clipboard.ignore_apps, cfg.clipboard.ocr);
                    self.history.set_limits(cfg.clipboard.history, &cfg.clipboard.keep);
                }
                if cfg.apps.folders != self.cfg.apps.folders || cfg.apps.deep != self.cfg.apps.deep {
                    self.scanned = None;
                }
                if cfg.start_at_login != self.cfg.start_at_login {
                    win::set_start_at_login(cfg.start_at_login);
                }
                if cfg.tray != self.cfg.tray {
                    self.tray_icon = if cfg.tray { tray(self.tx.clone()) } else { None };
                }
                design::set_size(&cfg.size);
                let same_files = cfg.files.enabled == self.files.is_some()
                    && self.files.as_ref().is_none_or(|f| f.same_settings(&cfg.files));
                if !same_files {
                    self.files = start_files(&cfg, &self.tx);
                }
                self.scripts = scripts::scan(&cfg.scripts.dirs());
                self.cfg = cfg;
                self.notice = None;
            }
            Err(e) => self.notice = Some(e),
        }
        self.rebuild_catalog(cx);
    }

    fn on_msg(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<Self>) {
        match msg {
            Msg::Hotkey(_) if self.cfg.launcher.game_mode && !self.visible && win::full_screen_app() => {}
            Msg::Hotkey(i) if i >= BINDS.len() => self.run_hotkey(i, window, cx),
            Msg::Hotkey(i) if self.visible && self.mode == Mode::of(BINDS[i]) => self.hide(true, cx),
            Msg::Hotkey(i) => self.show(Mode::of(BINDS[i]), window, cx),
            Msg::Tray(TrayCmd::Open) => self.show(Mode::Root, window, cx),
            Msg::Tray(TrayCmd::Settings) => self.open_settings(cx),
            Msg::Tray(TrayCmd::EditConfig) => win::open(&config::path().to_string_lossy()),
            Msg::Tray(TrayCmd::Quit) => cx.quit(),
            Msg::Apps(apps) => {
                self.apps = apps;
                self.rebuild_catalog(cx);
            }
            Msg::Expand(hit) => self.on_expand(hit, window, cx),
            Msg::Clip(entry, png) => {
                if self.cfg.clipboard.enabled {
                    self.history.push(entry, png);
                    if self.visible && self.mode == Mode::Clipboard {
                        self.refresh(cx);
                    }
                }
            }
            Msg::Files(generation, items) if self.mode == Mode::Root => {
                if generation == self.file_gen && !items.is_empty() {
                    let at = self.items.iter().position(|it| it.section.as_deref().is_some_and(|s| s.starts_with("Use \""))).unwrap_or(self.items.len());
                    self.items.splice(at..at, items.into_iter().map(|it| it.sec("Files")));
                    cx.notify();
                }
            }
            // Handled by the message loop before this update.
            Msg::Resize => {}
            Msg::Files(generation, items) => {
                if self.mode == Mode::Files && generation == self.file_gen && !self.query(cx).trim().is_empty() {
                    self.items = if items.is_empty() { vec![empty_row(g::SEARCH, "No matching files".into(), "Try fewer letters, or add folders in Settings → Files".into(), vec![])] } else { items };
                    self.selected = 0;
                    self.scroll_to_selected();
                    cx.notify();
                }
            }
            Msg::FilesIndexed => {
                if self.visible && self.mode == Mode::Files {
                    self.refresh(cx);
                } else {
                    cx.notify();
                }
            }
            Msg::ScriptDone(i, ok, out) => {
                let Some(script) = self.scripts.get(i).cloned() else { return };
                let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                match script.output {
                    Output::Full => {
                        if self.mode == Mode::Output && self.output.as_ref().is_some_and(|o| o.0 == i) {
                            self.output = Some((i, out, false));
                            self.status = (!ok).then(|| (Tone::Error, format!("{} failed", script.title)));
                            self.refresh(cx);
                        }
                    }
                    Output::Compact => {
                        self.status = Some(if ok { (Tone::Ok, if last.is_empty() { format!("{} done", script.title) } else { last }) } else { (Tone::Error, format!("{} failed: {last}", script.title)) });
                        cx.notify();
                    }
                    Output::Silent if !ok => {
                        self.notice = Some(format!("{} failed: {last}", script.title));
                        self.show(Mode::Root, window, cx);
                    }
                    Output::Silent => {}
                }
            }
            Msg::Ext(v) => self.on_ext(v, window, cx),
            Msg::Install(message, done) => {
                self.status = Some((match done { Some(true) => Tone::Ok, Some(false) => Tone::Error, None => Tone::Info }, message));
                if let Some(ok) = done {
                    self.load_extensions();
                    if ok && self.mode == Mode::Install {
                        self.set_mode(Mode::Root, cx);
                    }
                    self.tick(cx);
                }
                cx.notify();
            }
            // Rescanned on the next tick, so a burst of changes from one install is one scan.
            Msg::AppsChanged => self.scanned = None,
            Msg::Tick => {
                if self.scanned.is_none() {
                    self.scan_apps(true, false);
                }
                // Entries kept for a time leave once it's up.
                self.history.prune(search::now());
                let idle = Duration::from_secs(self.cfg.apps.auto_quit_minutes.max(1) * 60);
                self.autoquit.tick(&self.cfg.apps.auto_quit, idle, win::foreground());
                self.tick(cx);
            }
            Msg::ImageReady(path) => {
                self.images.remove(&path);
                if self.visible && self.mode == Mode::Ext {
                    self.refresh(cx);
                }
            }
            Msg::Running(list) => {
                for (r, icon) in &list {
                    self.exe_icons.insert(r.exe.clone(), icon.clone());
                }
                self.running = Some(list);
                if self.mode == Mode::Running {
                    self.refresh(cx);
                }
            }
            Msg::Store(entries) => {
                self.store_loading = false;
                self.status = Some((Tone::Info, format!("{} Windows-capable extensions", entries.iter().filter(|s| s.windows).count())));
                self.store = Some(entries);
                if self.mode == Mode::Store {
                    self.refresh(cx);
                }
            }
            Msg::TrayExt(run, f) => {
                if !f.is_empty() {
                    self.send(json!({"t": "event", "run": run, "fn": f, "args": [{"type": "left-click"}]}));
                }
            }
            Msg::HotkeyStatus(i, status) => {
                if self.hotkey_errors.len() <= i {
                    self.hotkey_errors.resize(i + 1, None);
                }
                let title = self.hotkey_item(i).map(|it| it.title.clone());
                self.hotkey_errors[i] = status.err().map(|e| title.map_or(e.clone(), |t| format!("{t}: {e}")));
                // Without a working main hotkey the user can't reach us, so open and say why.
                if i == 0 && self.hotkey_errors[0].is_some() {
                    self.show(Mode::Root, window, cx);
                }
                cx.notify();
            }
        }
    }

    fn label(&self) -> Option<String> {
        match self.mode {
            _ if self.panel.is_some() => Some("Actions".into()),
            _ if self.confirm.is_some() => self.confirm.as_ref().map(|c| c.title.clone()),
            Mode::Root => None,
            Mode::Clipboard => Some("Clipboard History".into()),
            Mode::Emoji => Some("Emoji".into()),
            Mode::Files => Some("Search Files".into()),
            Mode::Install => Some("Install Extension".into()),
            Mode::Store => Some("Extension Store".into()),
            Mode::Running => Some("Quit Applications".into()),
            Mode::Args => self.pending.as_ref().map(|p| match p {
                Pending::Script(i, _) => self.scripts[*i].title.clone(),
                Pending::Snippet { name, .. } => name.clone(),
            }),
            Mode::Output => self.output.as_ref().map(|(i, _, _)| self.scripts[*i].title.clone()),
            Mode::Ai => Some(self.ai.as_ref().and_then(|a| a.turns.last()).map_or("Ask AI".into(), |t| t.prompt.clone())),
            Mode::Ext => self.extv.as_ref().map(|e| {
                let (x, _) = &self.exts[e.ext];
                e.view.title.clone().unwrap_or_else(|| x.title.clone())
            }),
        }
    }

    fn placeholder(&self) -> SharedString {
        match self.mode {
            Mode::Root => "Search apps and commands…".into(),
            Mode::Clipboard => "Search clipboard history…".into(),
            Mode::Emoji => "Search emoji…".into(),
            Mode::Files => "Search files…".into(),
            Mode::Install => "Extension name or folder…".into(),
            Mode::Store => "Search the extension store…".into(),
            Mode::Running => "Search running apps…".into(),
            Mode::Ext => self.extv.as_ref().and_then(|e| e.view.placeholder.clone()).unwrap_or_else(|| "Search…".into()).into(),
            Mode::Args => self
                .pending
                .as_ref()
                .map(|p| match p {
                    Pending::Script(i, got) => {
                        let a = &self.scripts[*i].args[got.len()];
                        if a.optional { format!("{} (optional)", a.placeholder) } else { a.placeholder.clone() }
                    }
                    Pending::Snippet { args, got, .. } => {
                        let a = &args[got.len()];
                        if a.default.is_empty() { format!("{}…", a.name) } else { format!("{} (empty: {})", a.name, a.default) }
                    }
                })
                .unwrap_or_default()
                .into(),
            Mode::Output => "Enter copies the output".into(),
            Mode::Ai if self.ai.as_ref().is_some_and(|a| !a.turns.is_empty()) => "Ask a follow-up, or Enter copies the answer".into(),
            Mode::Ai => "Ask a question…".into(),
        }
    }

    /// Text on the left of the action bar: a toast, index status, or where you are.
    fn footer_left(&self) -> String {
        if let Some((_, s)) = &self.status {
            return s.clone();
        }
        if self.mode == Mode::Files
            && let Some(f) = &self.files
        {
            let n = f.count.load(std::sync::atomic::Ordering::SeqCst);
            let scope = if f.whole_drives() { " on all drives" } else { "" };
            return if f.indexing.load(std::sync::atomic::Ordering::SeqCst) { format!("Indexing… {n} so far") } else { format!("{n} files indexed{scope}") };
        }
        if self.mode == Mode::Ext && self.extv.as_ref().is_some_and(|e| e.view.loading) {
            return "Loading…".into();
        }
        // The grid shows no names: the selected emoji's is here.
        if self.grid() && self.panel.is_none()
            && let Some(it) = self.items.get(self.selected)
        {
            return it.title.clone();
        }
        if let Some(label) = self.label() {
            return label;
        }
        match self.items.get(self.selected).filter(|it| it.section.as_deref() != Some(EMPTY)) {
            Some(it) if !it.accessory.is_empty() && !it.accessory.contains(" ago") => it.accessory.clone(),
            _ => "procast".into(),
        }
    }

    /// The icon on the left of the action bar.
    fn context_icon(&self) -> Icon {
        match self.mode {
            Mode::Ext => self.extv.as_ref().map(|e| self.exts[e.ext].1.clone()).unwrap_or(Icon::Vector(g::PUZZLE)),
            Mode::Root => Icon::Vector(g::SEARCH),
            Mode::Clipboard => Icon::Vector(g::PASTE),
            Mode::Emoji => Icon::Vector(g::EMOJI),
            Mode::Files => Icon::Vector(g::FOLDER),
            Mode::Store => Icon::Vector(g::SHOP),
            Mode::Install => Icon::Vector(g::DOWNLOAD),
            Mode::Running => Icon::Vector(g::CANCEL),
            Mode::Args | Mode::Output => Icon::Vector(g::COMMAND),
            Mode::Ai => Icon::Vector(g::LIGHTBULB),
        }
    }

    /// Esc: close a popover/dialog, pop a pushed view, leave a mode, or hide.
    /// An Emacs or Vim navigation key: acts as `action` when that style is chosen.
    fn nav(&mut self, style: &str, action: Box<dyn gpui::Action>, window: &mut Window, cx: &mut Context<Self>) {
        if self.cfg.launcher.navigation == style {
            window.dispatch_action(action, cx);
        }
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel.take().is_some() {
            cx.notify();
            if self.is_form() {
                self.focus_field(window, cx);
            }
        } else if let Some(c) = self.confirm.take() {
            self.reply(c.run, c.id, Ok(json!(false)));
            self.refresh(cx);
        } else if self.mode == Mode::Ext && self.extv.as_ref().is_some_and(|e| e.depth > 1) {
            let run = self.extv.as_ref().unwrap().run;
            self.send(json!({"t": "pop", "run": run}));
        } else if self.mode == Mode::Root {
            self.hide(true, cx)
        } else if self.cfg.launcher.escape == "close" {
            self.pending = None;
            self.set_mode(Mode::Root, cx);
            self.hide(true, cx);
            // Popped to root: the next open starts fresh whatever pop_to_root says.
            self.hidden_at = None;
        } else {
            self.pending = None;
            self.set_mode(Mode::Root, cx);
            window.focus(&self.input.focus_handle(cx), cx);
        }
    }

    fn render_form(&self, t: &Tokens) -> gpui::AnyElement {
        let Some(ev) = &self.extv else { return div().into_any_element() };
        let field_box = |on: bool| design::field(on, t).w_full();
        div()
            .id("form")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .pt(px(24.))
            .child(div().w(px(520.)).flex().flex_col().gap(px(12.)).children(ev.view.fields.iter().enumerate().map(|(i, f)| {
                let on = i == ev.field;
                let value = ev.values.get(&f.id).cloned().unwrap_or(Value::Null);
                let control = match &f.kind {
                    FieldKind::Text | FieldKind::Password | FieldKind::TextArea => match ev.inputs.get(&f.id) {
                        Some(input) => field_box(on)
                            .when(f.kind == FieldKind::TextArea, |d| d.h(px(2. * design::CONTROL)).items_start().py(px(8.)))
                            .child(
                                text_input(SharedString::from(format!("field-{}", f.id)))
                                    .state(input.downgrade())
                                    .placeholder(f.placeholder.clone())
                                    .placeholder_color(t.text_faint)
                                    .caret_color(t.accent)
                                    .w_full()
                                    .whitespace_nowrap(),
                            )
                            .into_any_element(),
                        None => div().into_any_element(),
                    },
                    FieldKind::Checkbox => {
                        let checked = value.as_bool().unwrap_or(false);
                        div()
                            .h(px(design::CONTROL))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(design::icon(if checked { g::CHECKBOX_ON } else { g::CHECKBOX }, design::ic::LG, if checked { t.accent } else { t.text_muted }))
                            .child(div().text_color(if on { t.text } else { t.text_muted }).child(f.title.clone()))
                            .into_any_element()
                    }
                    FieldKind::Dropdown(opts) => {
                        let cur = value.as_str().unwrap_or_default();
                        let title = opts.iter().find(|o| o.0 == cur).map(|o| o.1.clone()).unwrap_or_default();
                        field_box(on).justify_between().child(div().truncate().child(title)).child(design::icon(g::CHEVRON_DOWN, design::ic::SM, t.text_muted)).into_any_element()
                    }
                    FieldKind::Description => div().text_size(px(design::ts::BODY_SM)).text_color(t.text_muted).child(value.as_str().unwrap_or_default().to_string()).into_any_element(),
                    FieldKind::Separator => div().h(px(1.)).w_full().bg(t.hairline).into_any_element(),
                };
                let label = if matches!(f.kind, FieldKind::Checkbox | FieldKind::Separator) { String::new() } else { f.title.clone() };
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .child(div().w(px(120.)).flex_none().h(px(design::CONTROL)).flex().items_center().justify_end().text_size(px(design::ts::BODY_SM)).text_color(t.text_muted).child(label))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(control)
                            .children(f.error.clone().map(|e| div().text_size(px(design::ts::SMALL)).text_color(t.danger).child(e))),
                    )
            })))
            .into_any_element()
    }

    fn render_markdown(md: &str, metadata: &[(String, String)], t: &Tokens, id: &'static str, scroll: Option<&ScrollHandle>) -> gpui::AnyElement {
        div()
            .id(id)
            .when_some(scroll, |d, h| d.track_scroll(h))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(24.))
            .py(px(16.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .children(ext::markdown_lines(md).into_iter().map(|(level, text)| {
                let line = div().line_height(px(21.)).child(if text.is_empty() { " ".to_string() } else { text });
                match level {
                    1 => line.pt(px(4.)).line_height(px(28.)).font_family(design::FONT_DISPLAY).text_size(px(design::ts::DISPLAY)).font_weight(FontWeight::SEMIBOLD),
                    2 => line.pt(px(4.)).text_size(px(design::ts::HEADING)).font_weight(FontWeight::SEMIBOLD),
                    3 => line.pt(px(4.)).font_weight(FontWeight::SEMIBOLD),
                    9 => line.font_family(design::FONT_MONO).text_size(px(design::ts::BODY_SM)).px(px(12.)).bg(t.surface).rounded(px(design::r::XS)),
                    _ => line.text_color(t.text),
                }
            }))
            .when(!metadata.is_empty(), |d| d.children(info_rows(metadata, t)))
            .into_any_element()
    }

    /// Clipboard History's right-hand pane: the whole entry, then where and when it came from.
    fn clip_detail(&mut self, t: &Tokens) -> gpui::AnyElement {
        let key = match self.items.get(self.selected).map(|it| &it.act) {
            Some(Act::Clip(k)) => k.clone(),
            _ => return div().flex_1().into_any_element(),
        };
        let Some(e) = self.history.get(&key).cloned() else { return div().flex_1().into_any_element() };
        let lines = |text: &str, mono: bool| {
            div()
                .flex()
                .flex_col()
                .when(mono, |d| d.font_family(design::FONT_MONO))
                .text_size(px(design::ts::BODY_SM))
                .line_height(px(20.))
                .children(text.lines().take(300).map(|l| div().child(if l.is_empty() { " ".to_string() } else { l.chars().take(2000).collect() })))
        };
        let mut meta: Vec<(String, String)> = vec![];
        let top = match &e.kind {
            clipboard::Kind::Text => {
                let (chars, words) = (e.text.chars().count(), e.text.split_whitespace().count());
                meta.push(("Content".into(), format!("{chars} characters · {words} words")));
                lines(&e.text, e.text.contains(['{', ';']) && e.text.lines().count() > 1).into_any_element()
            }
            clipboard::Kind::Image { id, w, h, bytes } => {
                meta.push(("Dimensions".into(), format!("{w} × {h}")));
                meta.push(("Size".into(), format!("{:.1} KB", *bytes as f64 / 1024.)));
                let preview = self.clip_preview(id);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .h(px(200.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(design::r::MD))
                            .bg(t.surface)
                            .border_1()
                            .border_color(t.hairline)
                            .overflow_hidden()
                            .children(preview.map(|p| gpui::img(p).max_w_full().max_h(px(196.)).object_fit(gpui::ObjectFit::Contain))),
                    )
                    .when(!e.text.is_empty(), |d| d.child(div().text_color(t.text_muted).child(lines(&e.text, false))))
                    .into_any_element()
            }
            clipboard::Kind::Files(files) => {
                meta.push(("Files".into(), files.len().to_string()));
                lines(&e.text, false).into_any_element()
            }
        };
        meta.insert(0, ("Application".into(), if e.app.is_empty() { "Unknown".into() } else { e.app.clone() }));
        meta.push(("Copied".into(), format!("{} · {}", win::local_time_of(e.at), search::ago(e.at))));
        if e.pinned {
            meta.push(("Pinned".into(), "Yes".into()));
        }
        div()
            .id("clip-detail")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(16.))
            .py(px(12.))
            .flex()
            .flex_col()
            .child(top)
            .children(info_rows(&meta, t))
            .into_any_element()
    }

    /// Emoji picker: moves the selection by `dx` cells or `dy` grid rows.
    fn grid_move(&mut self, dx: isize, dy: isize, cx: &mut Context<Self>) {
        let rows = grid_rows(&self.items, self.cfg.emoji.columns);
        let cells: Vec<&Vec<usize>> = rows.iter().filter_map(|r| if let GridRow::Cells(c) = r { Some(c) } else { None }).collect();
        let Some((r, c)) = cells.iter().enumerate().find_map(|(r, row)| row.iter().position(|&i| i == self.selected).map(|c| (r, c))) else { return };
        self.selected = if dy != 0 {
            let r = (r as isize + dy).clamp(0, cells.len() as isize - 1) as usize;
            cells[r][c.min(cells[r].len() - 1)]
        } else {
            (self.selected as isize + dx).clamp(0, self.items.len() as isize - 1) as usize
        };
        if let Some(at) = rows.iter().position(|row| matches!(row, GridRow::Cells(c) if c.contains(&self.selected))) {
            self.grid_list.scroll_to_reveal_item(at);
        }
        cx.notify();
    }

    /// The emoji picker shows a grid (it falls back to the list for its empty state).
    fn grid(&self) -> bool {
        self.mode == Mode::Emoji && self.items.first().is_some_and(|it| it.section.as_deref() != Some(EMPTY))
    }

    fn grid_view(&self, t: &Tokens, cx: &mut Context<Self>) -> gpui::AnyElement {
        let cols = self.cfg.emoji.columns.clamp(4, 16);
        let me = cx.weak_entity();
        let (selected, t) = (self.selected, t.clone());
        let glyphs: Rc<Vec<SharedString>> = Rc::new(self.items.iter().map(|it| if let Icon::Glyph(s) = &it.icon { s.clone() } else { SharedString::default() }).collect());
        let rows = Rc::new(grid_rows(&self.items, cols));
        // ponytail: rows are laid out only while on screen; 400 emoji cells per keystroke made typing lag.
        let list = gpui::list(self.grid_list.clone(), move |k, _, _| match &rows[k] {
                GridRow::Header(s) => design::section_header(s, None, &t).into_any_element(),
                GridRow::Cells(cells) => {
                    let pad = cols - cells.len();
                    div()
                        .flex()
                        .w_full()
                        .gap(px(4.))
                        .children(cells.iter().map(|&i| {
                            let on = i == selected;
                            let me = me.clone();
                            let glyph = glyphs[i].clone();
                            let t = &t;
                            let hover = t.hover;
                            div()
                                .id(("emoji", i))
                                .flex_1()
                                .h(px(EMOJI_CELL))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(design::r::MD))
                                .map(|d| if on { design::lozenge(d, t) } else { d.hover(move |s| s.bg(hover)) })
                                .font_family(design::FONT_EMOJI)
                                .text_size(px(30.))
                                .child(glyph)
                                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                                    me.update(cx, |this, cx| {
                                        this.selected = i;
                                        this.launch(window, cx);
                                    })
                                    .ok();
                                })
                                .into_any_element()
                        }))
                        .children((0..pad).map(|_| div().flex_1().into_any_element()))
                        .into_any_element()
                }
            });
        div().flex_1().min_h_0().flex().flex_col().px(px(8.)).py(px(LIST_PAD)).child(list.flex_1().min_h_0()).into_any_element()
    }
}

/// An emoji cell's height.
const EMOJI_CELL: f32 = 56.;

enum GridRow {
    Header(String),
    /// Item indices, at most `columns`.
    Cells(Vec<usize>),
}

/// The emoji picker's layout: each section's header, then its emoji in rows of `cols`.
fn grid_rows(items: &[Item], cols: usize) -> Vec<GridRow> {
    let cols = cols.clamp(4, 16);
    let mut out = vec![];
    let mut last: Option<&str> = None;
    for (i, it) in items.iter().enumerate() {
        let sec = it.section.as_deref();
        let new_sec = i == 0 || sec != last;
        if new_sec && let Some(s) = sec {
            out.push(GridRow::Header(s.to_string()));
        }
        match out.last_mut() {
            Some(GridRow::Cells(c)) if !new_sec && c.len() < cols => c.push(i),
            _ => out.push(GridRow::Cells(vec![i])),
        }
        last = sec;
    }
    out
}

/// "Information" and key / value rows under a detail view.
fn info_rows(metadata: &[(String, String)], t: &Tokens) -> Vec<gpui::AnyElement> {
    let head = div().mt(px(12.)).pt(px(12.)).border_t_1().border_color(t.hairline).font_family(design::FONT_SMALL).text_size(px(design::ts::SMALL)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_faint).child("Information");
    std::iter::once(head.into_any_element())
        .chain(metadata.iter().map(|(k, v)| {
            div()
                .flex()
                .justify_between()
                .gap(px(16.))
                .py(px(8.))
                .border_b_1()
                .border_color(t.hairline)
                .text_size(px(design::ts::BODY_SM))
                .child(div().flex_none().text_color(t.text_muted).child(k.clone()))
                .child(div().min_w_0().truncate().child(v.clone()))
                .into_any_element()
        }))
        .collect()
}

/// Icon for an action in the Ctrl+K panel.
fn act_glyph(act: &Act) -> &'static str {
    match act {
        Act::Favorite(..) => g::STAR,
        Act::Hide(_) => g::CANCEL,
        Act::Copy(_) | Act::CopyClip(_) => g::COPY,
        Act::PinClip(..) => g::PIN,
        Act::Paste(_) | Act::Clip(_) | Act::Snippet(_) => g::PASTE,
        Act::Open(_) => g::GLOBE,
        Act::File(_) => g::DOCUMENT,
        Act::Reveal(_) => g::FOLDER,
        Act::RunAsAdmin(_) => g::SHIELD,
        Act::DeleteClip(_) | Act::UninstallExt(_) => g::DELETE,
        Act::Install(_) => g::DOWNLOAD,
        Act::Run(_) | Act::Script(_) | Act::Cmd(_) | Act::ExtRun(..) => g::PLAY,
        Act::ExtSet(..) | Act::FormSet(..) => g::FILTER,
        Act::Confirm(true) => g::CHECK,
        Act::Confirm(false) => g::CANCEL,
        _ => g::PLAY,
    }
}

/// Test seam: with PROCAST_E2E=1, each render writes what's on screen to
/// %TEMP%\<file> so spikes/e2e.py can assert on it.
fn dump_ui(file: &str, state: Value) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("PROCAST_E2E").is_some()) {
        let _ = std::fs::write(std::env::temp_dir().join(file), state.to_string());
    }
}

/// %APPDATA%\procast\extensions.log: extension errors and console output.
fn ext_log(who: &str, text: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(config::dir().join("extensions.log")) {
        let _ = writeln!(f, "[{who}] {}", text.trim_end());
    }
}

/// One slot in the results list.
#[derive(Clone)]
enum Slot {
    Header(String, Option<usize>),
    Row { idx: usize, title: String, subtitle: String, accessory: String, hotkey: String, icon: Icon, tint: Option<design::Tint> },
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        design::scale_launcher();
        // Rem-based spacing follows the scale too.
        window.set_rem_size(px(16.));
        let t = Tokens::from_theme(&self.cfg.theme, window);
        let notices: Vec<String> = self.notice.iter().chain(self.hotkey_errors.iter().flatten()).cloned().collect();
        let empty = (self.items.len() == 1 && self.items[0].section.as_deref() == Some(EMPTY)).then(|| self.items[0].clone());
        // Views with their own body: the emoji grid, and clipboard history's detail pane.
        let grid = self.grid().then(|| self.grid_view(&t, cx));
        let side = (self.mode == Mode::Clipboard && empty.is_none()).then(|| self.clip_detail(&t));
        let selected_item = self.items.get(self.selected);
        let actions = selected_item.map(|it| it.actions()).unwrap_or_default();
        let primary = actions.first().map(|a| a.0.clone()).filter(|l| !l.is_empty());

        // Footer right side, also dumped for tests.
        let footer_action = match (&self.panel, selected_item) {
            (Some(_), _) => "Run  ↵    Close  Esc".to_string(),
            _ if self.is_form() => "Submit  Ctrl+↵    Actions  Ctrl+K".to_string(),
            (None, Some(it)) if self.armed == Some(self.selected) => format!("Press ↵ again to {}", it.title),
            (None, Some(_)) => match &primary {
                Some(p) if actions.len() > 1 => format!("{p}  ↵    Actions  Ctrl+K"),
                Some(p) => format!("{p}  ↵"),
                None => String::new(),
            },
            (None, None) => String::new(),
        };
        let view = self.extv.as_ref().filter(|_| self.mode == Mode::Ext && self.panel.is_none() && self.confirm.is_none()).map(|e| e.view.clone());
        dump_ui(
            "procast-ui.json",
            json!({
                "visible": self.visible,
                "bg": format!("{:?}", t.bg),
                "query": self.query(cx),
                "label": self.label(),
                "rows": match &self.panel {
                    Some((item, _)) => item.actions().into_iter().map(|a| a.0).collect::<Vec<_>>(),
                    None => self.items.iter().filter(|it| it.section.as_deref() != Some(EMPTY)).map(|it| it.title.clone()).collect(),
                },
                "selected": self.panel.as_ref().map_or(self.selected, |p| p.1),
                "panel": self.panel.is_some(),
                "footer": [self.footer_left(), footer_action.clone()],
                "ai": self.ai.as_ref().map(|a| json!({"text": a.turns.last().map_or("", |t| t.text.as_str()), "busy": a.busy, "turns": a.turns.len()})),
                "notice": self.notice,
                "view": view.as_ref().map(|v| &v.kind),
                "fields": self.extv.as_ref().map(|e| e.view.fields.iter().map(|f| &f.title).collect::<Vec<_>>()),
                "field": self.extv.as_ref().map(|e| e.field),
                "trays": self.trays.keys().collect::<Vec<_>>(),
            }),
        );

        let collapsed = self.collapsed(cx);

        // ---- results list ----
        let slots: Vec<Slot> = {
            let mut out = vec![];
            let rows = display_rows(&self.items);
            let count = |sec: &Option<String>| self.items.iter().filter(|it| &it.section == sec).count();
            // The right-hand column shows what tells rows apart. In root search that is a
            // row's own hotkey or alias (its kind is in the action bar); elsewhere it is the
            // row's accessory, said once per run of rows that share it.
            let root = self.mode == Mode::Root;
            let mut above = String::new();
            for (k, r) in rows.iter().enumerate() {
                match r {
                    None => {
                        let it = &self.items[rows[k + 1].unwrap_or(0)];
                        let sec = it.section.clone();
                        above.clear();
                        out.push(Slot::Header(sec.clone().unwrap_or_default(), (self.mode != Mode::Root).then(|| count(&sec))));
                    }
                    Some(i) => {
                        let it = &self.items[*i];
                        let hotkey = it.key.as_ref().filter(|_| root).and_then(|k| self.cfg.launcher.hotkeys.get(k)).cloned().unwrap_or_default();
                        let accessory = if root {
                            it.alias.clone()
                        } else if it.accessory == above {
                            String::new()
                        } else {
                            above = it.accessory.clone();
                            above.clone()
                        };
                        out.push(Slot::Row { idx: *i, title: it.title.clone(), subtitle: it.subtitle.clone(), accessory, hotkey, icon: it.icon.clone(), tint: it.tint });
                    }
                }
            }
            out
        };
        let selected = self.selected;
        let me = cx.weak_entity();
        let slots = Rc::new(slots);
        // Narrow split lists (detail on the right) get a tighter accessory column.
        let acc_max = if view.as_ref().is_some_and(|v| v.show_detail) || self.mode == Mode::Clipboard { 110. } else { design::ACCESSORY_MAX };
        let list = {
            let slots = slots.clone();
            uniform_list("rows", slots.len(), move |range, _, _| {
                range
                    .map(|k| match &slots[k] {
                        Slot::Header(title, count) => design::section_header(title, *count, &t).into_any_element(),
                        Slot::Row { idx, title, subtitle, accessory, hotkey, icon, tint } => {
                            let (i, on) = (*idx, *idx == selected);
                            let me = me.clone();
                            let accessory = if !hotkey.is_empty() {
                                Some(design::key_caps(hotkey, &t).into_any_element())
                            } else if !accessory.is_empty() {
                                Some(accessory.clone().into_any_element())
                            } else {
                                None
                            };
                            design::list_row(design::icon_tile(icon, &t, on, *tint), title, subtitle, accessory, acc_max, on, &t)
                                .id(k)
                                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                                    me.update(cx, |this, cx| {
                                        this.selected = i;
                                        this.launch(window, cx);
                                    })
                                    .ok();
                                })
                                .into_any_element()
                        }
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1()
            .min_h_0()
            .py(px(LIST_PAD))
        };

        // ---- calculator answer card (two rows tall, so the list below keeps whole rows) ----
        let answer = (self.mode == Mode::Root && is_answer(&self.items, 0)).then(|| {
            let it = &self.items[0];
            let full = match &it.act {
                Act::Copy(v) => v.as_str(),
                _ => "",
            };
            let me = cx.weak_entity();
            design::answer_slot(design::answer_card(&it.subtitle, &it.title, full, self.selected == 0, &t).id("answer").on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                me.update(cx, |this, cx| {
                    this.selected = 0;
                    this.launch(window, cx);
                })
                .ok();
            }))
        });

        // ---- body ----
        let split = |list: gpui::AnyElement, detail: gpui::AnyElement, cfg: &Config| {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().w(px(base_width(cfg) * 0.4)).flex_none().flex().flex_col().min_h_0().child(list))
                .child(div().flex_1().min_w_0().flex().flex_col().border_l_1().border_color(t.hairline).child(detail))
                .into_any_element()
        };
        let body = if let Some(g) = grid {
            g
        } else if let Some(d) = side {
            split(list.into_any_element(), d, &self.cfg)
        } else {
            match (&empty, &view) {
            _ if self.mode == Mode::Ai => {
                match self.ai.as_ref().filter(|a| !a.turns.is_empty()) {
                    None => Self::render_markdown("Type a question, then press Enter.", &[], &t, "ai", Some(&self.ai_scroll)),
                    Some(chat) => {
                        let (md, newest) = chat.markdown();
                        if chat.follow {
                            self.ai_scroll.scroll_to_top_of_item(newest);
                        }
                        Self::render_markdown(&md, &[], &t, "ai", Some(&self.ai_scroll))
                    }
                }
            }
            (Some(e), _) => {
                let glyph = if let Icon::Vector(c) = e.icon { c } else { g::SEARCH };
                design::empty_state(glyph, &e.title, &e.subtitle, &t).into_any_element()
            }
            (_, Some(v)) if v.kind == "Form" => self.render_form(&t),
            (_, Some(v)) if v.kind == "Detail" => Self::render_markdown(&v.markdown, &v.metadata, &t, "detail", None),
            (_, Some(v)) if v.show_detail && !v.items.is_empty() => {
                let it = self.items.get(self.selected).and_then(|row| v.items.iter().find(|it| it.title == row.title));
                let md = it.and_then(|it| it.detail.clone()).unwrap_or_default();
                let meta = it.map(|it| it.detail_meta.clone()).unwrap_or_default();
                split(list.into_any_element(), Self::render_markdown(&md, &meta, &t, "item-detail", None), &self.cfg)
            }
            _ if self.confirm.is_some() => div().flex_1().into_any_element(),
            _ => div().flex_1().min_h_0().flex().flex_col().children(answer).child(list).into_any_element(),
            }
        };

        // ---- compact mode: favourites as icons ----
        let fav_strip = div()
            .h(px(FAV_ROW))
            .flex_none()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(4.))
            .border_t_1()
            .border_color(t.hairline)
            .children(search::favorites(&self.catalog, &self.cfg).into_iter().take(12).enumerate().map(|(i, it)| {
                let (act, key) = (it.act.clone(), it.key.clone());
                div()
                    .id(("fav", i))
                    .size(px(ROW - 4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(design::r::MD))
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.run(act.clone(), key.clone(), window, cx)))
                    .child(design::icon_view(&it.icon, &t, false))
            }));

        // ---- search bar ----
        let at_root = self.mode == Mode::Root;
        let leading = if at_root { design::search_mark(&t).into_any_element() } else { design::back_tile(&t).id("back").on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx))).into_any_element() };
        let dropdown = self.extv.as_ref().filter(|_| self.mode == Mode::Ext).and_then(|e| e.view.dropdown.as_ref()).map(|(opts, cur, _)| {
            let title = opts.iter().find(|o| &o.0 == cur).map(|o| o.1.as_str()).unwrap_or_default();
            design::dropdown(title, &t).into_any_element()
        });
        // Compact root with nothing typed: say what Down does, so the bare bar is not a dead end.
        let hint = (collapsed && !self.fav_strip()).then(|| design::search_hint("Recent", "↓", &t).into_any_element());
        let search_bar = design::search_bar(
            leading,
            text_input("query")
                .state(self.input.downgrade())
                .placeholder(self.placeholder())
                .placeholder_color(t.text_faint)
                .caret_color(t.accent)
                .caret_blink_interval_500ms()
                .flex_1()
                .font_family(design::FONT_DISPLAY)
                .text_size(px(design::ts::QUERY))
                .whitespace_nowrap(),
            hint.into_iter().chain(dropdown),
            &t,
        );

        // ---- action bar ----
        let tone = self.status.as_ref().map(|(tone, _)| match tone {
            Tone::Error => t.danger,
            Tone::Ok => t.success,
            Tone::Info => t.accent,
        });
        // A status shows its tone dot; in root search the selected row's own icon in its hue; else the view's icon.
        let mark = match tone {
            Some(c) => design::status_dot(c).into_any_element(),
            None => match selected_item.filter(|it| at_root && it.section.as_deref() != Some(EMPTY)).map_or_else(|| (self.context_icon(), t.text_muted), |it| (it.icon.clone(), it.tint.map_or(t.text_muted, |k| k.hue(&t)))) {
                (Icon::Vector(ch), hue) => design::icon(ch, design::ic::MD, hue).into_any_element(),
                (Icon::Image(image), _) => gpui::img(image).size(px(design::ic::LG)).flex_none().into_any_element(),
                (Icon::Glyph(s), _) => div().font_family(design::FONT_EMOJI).text_size(px(design::ic::MD)).child(s).into_any_element(),
            },
        };
        let keys = |label: &str, keys: &[&str]| design::shortcut(label, keys, &t).into_any_element();
        let right = if self.panel.is_some() {
            vec![keys("Run", &["↵"]), design::vdivider(&t).into_any_element(), keys("Close", &["Esc"])]
        } else if self.is_form() {
            vec![keys("Submit", &["Ctrl", "↵"]), design::vdivider(&t).into_any_element(), keys("Actions", &["Ctrl", "K"])]
        } else if self.armed == Some(self.selected) {
            vec![div().text_color(t.warning).child("Press ↵ again to confirm").into_any_element()]
        } else if let Some(p) = &primary {
            let mut v = vec![keys(p, &["↵"])];
            if actions.len() > 1 {
                v.extend([design::vdivider(&t).into_any_element(), keys("Actions", &["Ctrl", "K"])]);
            }
            v
        } else {
            vec![]
        };
        let footer = design::action_bar(mark, &self.footer_left(), tone.is_some(), right, &t);

        // ---- overlays ----
        let popover = self.panel.as_ref().map(|(item, sel)| {
            let me = cx.weak_entity();
            let rows = item.actions().into_iter().enumerate().map(|(i, (label, act))| {
                let me = me.clone();
                let keys: &[&str] = match i {
                    0 => &["↵"],
                    1 => &["Ctrl", "↵"],
                    _ => &[],
                };
                design::panel_row(act_glyph(&act), &label, keys, i == *sel, act.destructive(), &t).id(("act", i)).on_click(move |_, window, cx| {
                    me.update(cx, |this, cx| {
                        if let Some((_, p)) = &mut this.panel {
                            *p = i;
                        }
                        this.launch(window, cx);
                    })
                    .ok();
                })
            });
            // The window steps back under `scrim_soft` so the panel is plainly the top layer.
            // A click on it closes the panel; the panel occludes it, so clicks inside stay with the panel.
            [
                design::scrim(t.scrim_soft).id("panel-scrim").occlude().on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx))).into_any_element(),
                design::action_panel(design::panel_head(design::icon_tile(&item.icon, &t, false, item.tint), &item.title, &t), rows, &t).occlude().into_any_element(),
            ]
        });
        let dialog = self.confirm.as_ref().map(|c| {
            let me = cx.weak_entity();
            let button = |i: usize, label: &str, kind: design::Button| {
                let me = me.clone();
                design::button(label, kind, i == selected, &t).id(("dialog", i)).w_full().on_click(move |_, window, cx| {
                    me.update(cx, |this, cx| {
                        this.selected = i;
                        this.launch(window, cx);
                    })
                    .ok();
                })
            };
            let primary = if c.danger { design::Button::Danger } else { design::Button::Primary };
            design::dialog(c.danger, &c.title, &c.message, [button(0, &c.primary, primary), button(1, "Cancel", design::Button::Quiet)], &t)
        });

        design::glass_window(&t)
            .size_full()
            .font_family(design::FONT_UI)
            .text_size(px(design::ts::BODY))
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                if this.panel.is_none() && this.mode == Mode::Ai {
                    return this.scroll_ai(ROW * 3., cx);
                }
                if this.panel.is_none() && this.is_form() {
                    return this.move_field(1, window, cx);
                }
                if this.collapsed(cx) {
                    this.peek = true;
                    return this.refresh(cx);
                }
                if this.panel.is_none() && this.grid() {
                    return this.grid_move(0, 1, cx);
                }
                match &mut this.panel {
                    Some((item, i)) => *i = (*i + 1).min(item.actions().len().saturating_sub(1)),
                    None => this.selected = (this.selected + 1).min(this.items.len().saturating_sub(1)),
                }
                this.armed = None;
                this.scroll_to_selected();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| {
                if this.panel.is_none() && this.mode == Mode::Ai {
                    return this.scroll_ai(-ROW * 3., cx);
                }
                if this.panel.is_none() && this.is_form() {
                    return this.move_field(-1, window, cx);
                }
                if this.panel.is_none() && this.selected == 0 && this.recall_search(cx) {
                    return;
                }
                if this.panel.is_none() && this.grid() {
                    return this.grid_move(0, -1, cx);
                }
                match &mut this.panel {
                    Some((_, i)) => *i = i.saturating_sub(1),
                    None => this.selected = this.selected.saturating_sub(1),
                }
                this.armed = None;
                this.scroll_to_selected();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleActions, _, cx| {
                this.panel = match this.panel.take() {
                    Some(_) => None,
                    None => this.items.get(this.selected).cloned().filter(|it| it.section.as_deref() != Some(EMPTY) || !it.extra.is_empty()).filter(|it| it.act != Act::None || !it.extra.is_empty()).map(|it| (it, 0)),
                };
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.grid_move(-1, 0, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.grid_move(1, 0, cx)))
            // Left and Right move through the emoji grid instead of the search text.
            .when(self.grid() && self.panel.is_none(), |d| d.key_context("EmojiGrid"))
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| this.dismiss(window, cx)))
            .on_action(cx.listener(|this, _: &EmacsNext, window, cx| this.nav("emacs", Box::new(SelectNext), window, cx)))
            .on_action(cx.listener(|this, _: &EmacsPrev, window, cx| this.nav("emacs", Box::new(SelectPrev), window, cx)))
            .on_action(cx.listener(|this, _: &EmacsBack, window, cx| this.nav("emacs", Box::new(Dismiss), window, cx)))
            .on_action(cx.listener(|this, _: &VimNext, window, cx| this.nav("vim", Box::new(SelectNext), window, cx)))
            .on_action(cx.listener(|this, _: &VimPrev, window, cx| this.nav("vim", Box::new(SelectPrev), window, cx)))
            .on_action(cx.listener(|this, _: &VimBack, window, cx| this.nav("vim", Box::new(Dismiss), window, cx)))
            .on_action(cx.listener(|this, _: &Launch, window, cx| this.launch(window, cx)))
            .on_action(cx.listener(|this, _: &Submit, window, cx| {
                if this.is_form() && this.panel.is_none() {
                    this.submit_form(cx)
                } else if this.panel.is_none()
                    && let Some(item) = this.items.get(this.selected).cloned()
                    && item.actions().len() > 1
                {
                    // Ctrl+Enter: the secondary action, as shown in the action panel.
                    this.panel = Some((item, 1));
                    this.launch(window, cx);
                } else {
                    this.launch(window, cx)
                }
            }))
            .child(search_bar)
            .children(notices.into_iter().map(|n| {
                div()
                    .mx(px(16.))
                    .mt(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(design::r::SM))
                    .bg(t.surface)
                    .text_size(px(design::ts::BODY_SM))
                    .child(design::icon(g::WARNING, design::ic::MD, t.warning))
                    .child(div().min_w_0().child(n))
            }))
            .when(collapsed && self.fav_strip(), |d| d.child(fav_strip))
            .when(!collapsed, |d| d.child(body).child(footer))
            .children(popover.into_iter().flatten())
            .children(dialog)
    }
}

/// A plain accent-coloured disc; no third-party artwork.
fn tray_icon_image() -> TrayImage {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            if dx * dx + dy * dy <= 15.0 * 15.0 {
                rgba[(y * N + x) * 4..][..4].copy_from_slice(&[0x5b, 0x8c, 0xff, 0xff]);
            }
        }
    }
    TrayImage::from_rgba(rgba, N as u32, N as u32).expect("valid icon")
}

fn tray(tx: UnboundedSender<Msg>) -> Option<TrayIcon> {
    let open = MenuItem::new("Open procast", true, None);
    let settings = MenuItem::new("Settings…", true, None);
    let edit = MenuItem::new("Edit config.toml", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&open, &settings, &edit, &quit]).ok()?;
    let ids = (open.id().clone(), settings.id().clone(), edit.id().clone(), quit.id().clone());
    let menu_tx = tx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        // Items of extension tray menus carry "ext|<run>|<handler>" ids.
        if let Some((run, f)) = e.id.0.strip_prefix("ext|").and_then(|r| r.split_once('|')) {
            if let Ok(run) = run.parse() {
                let _ = menu_tx.unbounded_send(Msg::TrayExt(run, f.to_string()));
            }
            return;
        }
        let cmd = match &e.id {
            id if *id == ids.0 => TrayCmd::Open,
            id if *id == ids.1 => TrayCmd::Settings,
            id if *id == ids.2 => TrayCmd::EditConfig,
            id if *id == ids.3 => TrayCmd::Quit,
            _ => return,
        };
        let _ = menu_tx.unbounded_send(Msg::Tray(cmd));
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        // Only procast's own icon opens the launcher; extension icons just show their menu.
        if let TrayIconEvent::Click { id, button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e
            && id.0 == "procast"
        {
            let _ = tx.unbounded_send(Msg::Tray(TrayCmd::Open));
        }
    }));
    TrayIconBuilder::new().with_id("procast").with_menu(Box::new(menu)).with_tooltip("procast").with_icon(tray_icon_image()).build().ok()
}

fn main() {
    config::migrate_old_data();
    // The whole-drive file index helper (see ntfs.rs).
    match std::env::args().nth(1).as_deref() {
        Some("--indexer") => return ntfs::serve(),
        Some(flag @ ("--install-indexer" | "--uninstall-indexer")) => {
            let done = if flag == "--install-indexer" { ntfs::install() } else { ntfs::uninstall() };
            if let Err(e) = done {
                let _ = std::fs::write(std::env::temp_dir().join("procast-indexer-error.txt"), e);
                std::process::exit(1);
            }
            return;
        }
        _ => {}
    }
    // ponytail: a second launch just exits; forward "show" to the running instance later.
    if !win::single_instance() {
        return;
    }
    gpui_ce_platform::application().with_assets(design::Icons).with_quit_mode(QuitMode::Explicit).run(|cx: &mut App| {
        design::register_fonts(cx);
        // Defaults first; ours are added later at the same depth, so they win for these keys.
        cx.bind_keys(default_bindings().as_keybindings(Some(DEFAULT_INPUT_CONTEXT)));
        cx.bind_keys([
            KeyBinding::new("up", SelectPrev, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("down", SelectNext, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("tab", SelectNext, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("shift-tab", SelectPrev, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("escape", Dismiss, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("enter", Launch, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("ctrl-enter", Submit, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("ctrl-k", ToggleActions, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("ctrl-down", NextSection, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("ctrl-up", PrevSection, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("alt-n", EmacsNext, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("alt-p", EmacsPrev, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("alt-b", EmacsBack, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("alt-j", VimNext, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("alt-k", VimPrev, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("alt-h", VimBack, Some(DEFAULT_INPUT_CONTEXT)),
            KeyBinding::new("left", SelectLeft, Some(&format!("EmojiGrid > {DEFAULT_INPUT_CONTEXT}"))),
            KeyBinding::new("right", SelectRight, Some(&format!("EmojiGrid > {DEFAULT_INPUT_CONTEXT}"))),
            // The settings window when no text field has focus.
            KeyBinding::new("up", SelectPrev, Some("Settings")),
            KeyBinding::new("down", SelectNext, Some("Settings")),
            KeyBinding::new("tab", SelectNext, Some("Settings")),
            KeyBinding::new("shift-tab", SelectPrev, Some("Settings")),
            KeyBinding::new("enter", Launch, Some("Settings")),
            KeyBinding::new("escape", Dismiss, Some("Settings")),
            KeyBinding::new("ctrl-down", NextSection, Some("Settings")),
            KeyBinding::new("ctrl-up", PrevSection, Some("Settings")),
        ]);

        let (cfg, notice) = match config::load() {
            Ok(cfg) if cfg.files.ntfs && ntfs::old_helper_installed() => {
                (cfg, Some("Whole-drive file search is off since the rename to procast. Run \"Enable Fast File Search\" once to turn it back on.".into()))
            }
            Ok(cfg) => (cfg, None),
            Err(e) => (Config::default(), Some(e)),
        };
        win::set_start_at_login(cfg.start_at_login);
        net::load_rates();
        design::set_size(&cfg.size);
        // Before the first render, so the body is not drawn as glass on a machine with transparency off.
        let glass = design::refresh_glass();
        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        let tray = if cfg.tray { tray(tx.clone()) } else { None };
        let tick_tx = tx.clone();

        let bounds = Bounds::centered(None, size(px(base_width(&cfg)), px(window_height(&cfg))), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            show: false,
            focus: false,
            windows_window_background: backdrop(glass),
            ..Default::default()
        };
        let handle = cx.open_window(options, |window, cx| cx.new(|cx| Launcher::new(cfg, notice, tx, window, cx))).unwrap();

        // Shown once, cloaked: from now on hotkey -> un-cloak -> next frame.
        let _ = handle.update(cx, |this, window, _| {
            this.hwnd = win::hwnd(window);
            win::make_tool_window(this.hwnd);
            win::round_corners(this.hwnd);
            win::cloak(this.hwnd, true);
            window.set_visible(true);
            this.tray_icon = tray;
            this.scan_apps(true, false);
            // For screenshots and checks of the settings window without clicking through.
            if std::env::var_os("PROCAST_OPEN_SETTINGS").is_some() {
                let _ = this.tx.unbounded_send(Msg::Tray(TrayCmd::Settings));
            }
            // PROCAST_SHOW=root|clipboard|emoji|files|running with PROCAST_QUERY: opens there.
            if std::env::var_os("PROCAST_SHOW").is_some() {
                let _ = this.tx.unbounded_send(Msg::Tray(TrayCmd::Open));
            }
        });

        // PROCAST_TYPE: text put in the search bar a second after opening, through the same
        // change event as typing (checks of compact mode without sending keys).
        if let Ok(text) = std::env::var("PROCAST_TYPE") {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = handle.update(cx, |this, _, cx| this.input.update(cx, |s, cx| s.emplace(&text, cx)));
            })
            .detach();
        }

        cx.spawn(async move |cx| {
            while let Some(msg) = rx.next().await {
                if let Msg::Resize = msg {
                    if let Ok((hwnd, h)) = handle.update(cx, |this, _, _| (this.hwnd, this.height)) {
                        win::resize(hwnd, h * design::launcher_scale());
                    }
                    continue;
                }
                // SetWindowPos sends WM_SIZE/WM_DPICHANGED synchronously and GPUI's window
                // proc re-borrows the app, so the window is moved here, between updates.
                let opens = match msg {
                    Msg::Hotkey(i) => handle.update(cx, |this, _, _| this.hotkey_opens(i) && !(this.cfg.launcher.game_mode && !this.visible && win::full_screen_app())).unwrap_or(false),
                    Msg::Expand(ref hit) => handle.update(cx, |this, _, _| this.expand_opens(hit)).unwrap_or(false),
                    _ => matches!(msg, Msg::Tray(TrayCmd::Open) | Msg::HotkeyStatus(0, Err(_)) | Msg::ScriptDone(..)),
                };
                let opens_root = matches!(msg, Msg::Hotkey(0) | Msg::Tray(TrayCmd::Open));
                let geometry = handle.update(cx, |this, _, cx| {
                    // Opening compact root search: just the bar, unless the last search is kept.
                    let compact = opens_root && this.cfg.window_mode == "compact" && (!this.keeps_state() || this.collapsed(cx));
                    let height = if compact { BAR + if this.fav_strip() { FAV_ROW } else { 0. } + 2. } else { window_height(&this.cfg) };
                    (!this.visible).then(|| (this.hwnd, this.cfg.monitor.clone(), this.cfg.width, base_width(&this.cfg), height))
                });
                if opens && let Ok(Some((hwnd, monitor, width, base, height))) = geometry {
                    let screen = win::screen(&monitor);
                    design::set_screen(if width <= 1. { width * screen.width() / BASE_WIDTH } else { 1. });
                    let s = design::launcher_scale();
                    win::place(hwnd, &screen, base * s, height * s);
                }
                let _ = handle.update(cx, |this, window, cx| this.on_msg(msg, window, cx));
            }
        })
        .detach();

        // ponytail: 2 s mtime poll instead of a file watcher; good enough for a hand-edited file.
        // The same loop drives interval extension commands (checked every 10 s).
        cx.spawn(async move |cx| {
            let mut last = config::modified();
            let mut ticks = 0u32;
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let now = config::modified();
                if now != last {
                    last = now;
                    let _ = handle.update(cx, |this, window, cx| {
                        window.set_background_appearance(backdrop(design::refresh_glass()).into());
                        this.reload(cx);
                    });
                }
                if ticks % 5 == 0 {
                    let _ = tick_tx.unbounded_send(Msg::Tick);
                }
                ticks += 1;
            }
        })
        .detach();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_grid_rows_break_at_sections_and_columns() {
        let it = |sec: &str| Item::new("x", "", Icon::glyph("x"), Act::None).sec(sec);
        let items: Vec<Item> = (0..6).map(|_| it("Smileys")).chain((0..2).map(|_| it("People"))).collect();
        let shape: Vec<String> = grid_rows(&items, 4).iter().map(|r| match r { GridRow::Header(h) => h.clone(), GridRow::Cells(c) => format!("{c:?}") }).collect();
        assert_eq!(shape, ["Smileys", "[0, 1, 2, 3]", "[4, 5]", "People", "[6, 7]"]);
        let searched: Vec<Item> = (0..5).map(|_| Item::new("x", "", Icon::glyph("x"), Act::None)).collect();
        assert_eq!(grid_rows(&searched, 4).len(), 2, "no sections: just rows");
    }

    #[test]
    fn ai_chat_markdown_pins_the_newest_question() {
        let turn = |prompt: &str, text: &str| AiTurn { id: 0, prompt: prompt.into(), text: text.into() };
        let mut chat = AiChat::default();
        chat.turns.push(turn("first", "one\n```rust\nfn main() {"));
        chat.turns.push(turn("second", ""));
        let (md, newest) = chat.markdown();
        let lines = ext::markdown_lines(&md);
        assert_eq!(lines[newest], (3, "second".to_string()), "the pinned line is the newest question");
        assert_eq!(lines.last().unwrap(), &(0, String::new()));
        assert!(lines.contains(&(0, "Thinking…".to_string())), "an unanswered question shows a placeholder");
        assert!(lines.iter().any(|l| l == &(9, "fn main() {".to_string())), "code stays code");
        assert!(lines[newest..].iter().all(|l| l.0 != 9), "a cut-off code block does not swallow the next question");
    }
}
