#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod commands;
mod config;
mod design;
mod ext;
mod files;
mod net;
mod ntfs;
mod scripts;
mod search;
mod settings;
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
    ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window, WindowBounds, WindowKind, WindowOptions,
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

actions!(akscast, [SelectPrev, SelectNext, Dismiss, Launch, ToggleActions, Submit, NextSection, PrevSection]);
// Navigation-key styles; each does something only when `[launcher] navigation` picks it.
actions!(akscast, [EmacsNext, EmacsPrev, EmacsBack, VimNext, VimPrev, VimBack]);

const BAR: f32 = 56.;
const ROW: f32 = 44.;
const FOOTER: f32 = 34.;

fn window_height(cfg: &Config) -> f32 {
    BAR + ROW * cfg.max_results.max(1) as f32 + 12. + FOOTER
}

/// Compact mode's favourites strip under the search bar.
const FAV_ROW: f32 = 48.;

/// The launcher's width before scaling. `width` ≤ 1 is a share of the screen: the window
/// keeps that share (and its proportions) on every monitor, and its content scales with it.
const BASE_WIDTH: f32 = 750.;

fn base_width(cfg: &Config) -> f32 {
    if cfg.width <= 1. { BASE_WIDTH } else { cfg.width }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Root,
    Clipboard,
    Emoji,
    Files,
    /// Collecting script arguments (see `Launcher::pending`).
    Args,
    /// Showing a script's output (see `Launcher::output`).
    Output,
    /// An extension command's view (see `Launcher::extv`).
    Ext,
    /// Asking what to install.
    Install,
    /// Browsing the Raycast store index.
    Store,
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
    for (i, it) in items.iter().enumerate() {
        let sec = it.section.as_deref().filter(|s| *s != EMPTY);
        if sec.is_some() && sec != last {
            out.push(None);
        }
        last = sec;
        out.push(Some(i));
    }
    out
}

fn empty_row(glyph: char, title: String, hint: String, actions: Vec<(String, Act)>) -> Item {
    let mut it = Item::new(title, hint, Icon::Fluent(glyph), actions.first().map(|a| a.1.clone()).unwrap_or(Act::None)).sec(EMPTY);
    it.extra = actions;
    it
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
    /// Script waiting for arguments: (index, collected so far).
    pending: Option<(usize, Vec<String>)>,
    /// Script output view: (index, output, still running).
    output: Option<(usize, String, bool)>,
    /// Short-lived message in the footer.
    status: Option<String>,
    /// Config or extension problem, shown above the results.
    notice: Option<String>,
    hotkey_errors: Vec<Option<String>>,
    hwnd: HWND,
    prev: HWND,
    visible: bool,
    /// The akscast notification-area icon (Settings → General → Show in tray).
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
        // ponytail: ignore_apps is read once at start; changing it needs a restart.
        clipboard::watch(cfg.clipboard.ignore_apps.clone(), tx.clone());
        for (dir, recursive) in win::app_dirs() {
            let tx = tx.clone();
            std::thread::spawn(move || win::watch_dir(&dir, recursive, |_| tx.unbounded_send(Msg::AppsChanged).is_ok()));
        }
        let mut this = Self {
            input,
            hotkeys: win::Hotkeys::spawn(hotkey_specs(&cfg), tx.clone()),
            history: clipboard::History::load(config::dir().join("clipboard.bin"), cfg.clipboard.history),
            files: start_files(&cfg, &tx),
            scripts: scripts::scan(&cfg.scripts.dirs()),
            exts: vec![],
            catalog: vec![],
            cfg,
            tx,
            apps: vec![],
            scanned: None,
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
            output: None,
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
            Err(e) => self.status = Some(e),
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
        self.images.entry(path.clone()).or_insert_with(|| ext::load_image(&path)).clone()
    }

    /// An extension icon value: "icon:Name", an emoji, an asset file or a URL.
    fn ext_icon(&mut self, ext: usize, icon: Option<&str>) -> Icon {
        let Some(icon) = icon else { return Icon::Fluent(g::BULLETS) };
        if let Some(name) = icon.strip_prefix("icon:") {
            return Icon::Fluent(ext::icon_glyph(name));
        }
        if icon.starts_with("http") {
            // Web images: drawn from the cache once downloaded; a globe until then.
            let cached = net::image_path(icon);
            if cached.exists() {
                return self.image(cached).map(Icon::Image).unwrap_or(Icon::Fluent(g::GLOBE));
            }
            net::fetch_image(icon, &self.tx);
            return Icon::Fluent(g::GLOBE);
        }
        if icon.starts_with("data:") {
            return Icon::Fluent(g::DOCUMENT);
        }
        if icon.chars().count() <= 4 && !icon.contains('.') {
            return Icon::glyph(icon.to_string());
        }
        let dir = self.exts[ext].0.dir.join("assets");
        let path = if std::path::Path::new(icon).is_absolute() { PathBuf::from(icon) } else { dir.join(icon) };
        match self.image(path) {
            Some(i) => Icon::Image(i),
            None => Icon::Fluent(g::BULLETS),
        }
    }

    fn load_extensions(&mut self) {
        let exts = ext::scan();
        let mut out = Vec::with_capacity(exts.len());
        for e in exts {
            let icon = e.icon.clone().and_then(|p| self.image(p)).map(Icon::Image).unwrap_or(Icon::Fluent(g::PUZZLE));
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
            let mut ok = Item::new(c.primary.clone(), "", Icon::Fluent(g::CHECK), Act::Confirm(true));
            ok.extra = vec![(c.primary.clone(), Act::Confirm(true))];
            return vec![ok, Item::new("Cancel", "", Icon::Fluent(g::CANCEL), Act::Confirm(false))];
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
                let mut row = Item::new(view.title.clone().unwrap_or_default(), "", Icon::Fluent(g::BULLETS), view_actions.first().map(|a| a.1.clone()).unwrap_or(Act::None));
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
            Mode::Clipboard => search::clipboard(&query, &self.history.entries, 200, &mut self.matcher),
            Mode::Emoji => search::emoji(&query, 200, &mut self.matcher),
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
                Some((i, got)) => {
                    let s = &self.scripts[*i];
                    let n = s.args.len();
                    vec![Item::new(format!("Run {}", s.title), format!("argument {} of {n}", got.len() + 1), Icon::Fluent(g::COMMAND), Act::Script(*i))]
                }
                None => vec![],
            },
            Mode::Output => match &self.output {
                Some((_, _, true)) => vec![empty_row(g::CLOCK, "Running…".into(), String::new(), vec![])],
                Some((_, text, false)) => {
                    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                    let shown = if lines.is_empty() { vec!["(no output)"] } else { lines };
                    shown.iter().map(|l| Item::new(*l, "", Icon::Fluent(g::COMMAND), Act::Copy(text.clone())).sec("Output")).collect()
                }
                None => vec![],
            },
            Mode::Ext => self.ext_rows(&query),
            Mode::Store => match &self.store {
                None => vec![empty_row(g::DOWNLOAD, "Loading the store index…".into(), "The first time fetches every extension's manifest (~1 min)".into(), vec![])],
                Some(entries) => {
                    let installed: Vec<&str> = self.exts.iter().map(|(x, _)| x.name.as_str()).collect();
                    let rows: Vec<Item> = entries
                        .iter()
                        .filter(|s| s.windows)
                        .map(|s| {
                            let done = installed.contains(&s.name.as_str());
                            let mut it = Item::new(&s.title, &s.description, Icon::Fluent(if done { g::CHECK } else { g::PUZZLE }), Act::Install(s.name.clone()))
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
            Mode::Install => {
                let what = query.trim();
                let title = if what.is_empty() { "Install an extension".to_string() } else { format!("Install {what}") };
                vec![empty_row(g::DOWNLOAD, title, "A store name (e.g. obsidian) or a folder path, then Enter · needs git, Node and npm".into(), vec![])]
            }
        };
        if self.items.is_empty() && self.mode == Mode::Clipboard {
            self.items = vec![empty_row(g::PASTE, "Nothing copied yet".into(), "Text you copy shows up here".into(), vec![])];
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
        let mut known = if fresh_icons { vec![] } else { self.apps.clone() };
        std::thread::spawn(move || {
            if known.is_empty() && !fresh_icons {
                known = win::load_apps(&cache);
                if !known.is_empty() {
                    let _ = tx.unbounded_send(Msg::Apps(known.clone()));
                }
            }
            let mut apps = win::scan_apps();
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
        self.input.update(cx, |s, cx| s.emplace("", cx));
        self.selected = 0;
        self.refresh(cx);
    }

    fn show(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
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
        self.height = self.target_height(cx);
        win::cloak(self.hwnd, false);
        win::focus(self.hwnd);
        window.activate();
        window.focus(&self.input.focus_handle(cx), cx);
        self.visible = true;
        self.scan_apps(false, false);
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
        let tooltip = [bar.tooltip.as_str(), bar.title.as_str(), cmd_title.as_str()].into_iter().find(|t| !t.is_empty()).unwrap_or("akscast").to_string();

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
                    m => self.reply(run, id, Err(format!("{m} is not supported in akscast yet"))),
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
            "hostExit" => {
                self.host = None;
                self.bg_runs.clear();
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
                    if m.is_empty() { t } else { format!("{t} — {m}") }
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
                self.status = Some("Move to Recycle Bin isn't supported yet".into());
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
                self.status = Some(if add { "Added to Favorites" } else { "Removed from Favorites" }.into());
                return self.save_launcher_list("favorites", list, cx);
            }
            Act::Hide(key) => {
                let mut list = self.cfg.launcher.hidden.clone();
                if !list.contains(&key) {
                    list.push(key);
                }
                self.status = Some("Hidden from root search. Undo in Settings → Launcher".into());
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
            Act::Paste(text) | Act::Clip(text) => return self.paste(text, cx),
            Act::DeleteClip(text) => {
                self.history.remove(&text);
                self.status = Some("Deleted from clipboard history".into());
                return self.refresh(cx);
            }
            Act::Snippet(template) => {
                let clip = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                return self.paste(search::expand(&template, &clip, win::local_date_time()), cx);
            }
            Act::Script(i) => {
                if self.scripts[i].args.is_empty() {
                    return self.start_script(i, vec![], cx);
                }
                self.pending = Some((i, vec![]));
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
                self.status = Some(format!("Uninstalled {name}"));
                return self.refresh(cx);
            }
            Act::None => return,
            Act::Cmd(Cmd::SearchFiles) => return self.set_mode(Mode::Files, cx),
            Act::Cmd(Cmd::ClipboardHistory) => return self.set_mode(Mode::Clipboard, cx),
            Act::Cmd(Cmd::Emoji) => return self.set_mode(Mode::Emoji, cx),
            Act::Cmd(Cmd::InstallExtension) => return self.set_mode(Mode::Install, cx),
            Act::Cmd(Cmd::BrowseStore) => return self.set_mode(Mode::Store, cx),
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

    /// The command a hotkey past `BINDS` runs (None if it's switched off or gone).
    fn hotkey_item(&self, i: usize) -> Option<&Item> {
        let key = self.cfg.launcher.command_hotkeys().nth(i.checked_sub(BINDS.len())?)?.0;
        self.catalog.iter().find(|it| it.key.as_ref() == Some(key))
    }

    /// Whether running this needs the launcher on screen (it opens a view or asks for input).
    fn needs_window(&self, act: &Act) -> bool {
        match act {
            Act::Cmd(Cmd::SearchFiles | Cmd::ClipboardHistory | Cmd::Emoji | Cmd::InstallExtension | Cmd::BrowseStore) | Act::Fill(_) => true,
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
            titlebar: Some(gpui::TitlebarOptions { title: Some("akscast Settings".into()), ..Default::default() }),
            windows_window_background: gpui::WindowsWindowBackground::MicaBackdrop,
            ..Default::default()
        };
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
            self.notice = Some("exthost folder not found next to akscast.exe".into());
            return cx.notify();
        };
        let (node, tx, dir) = (self.cfg.extensions.node.clone(), self.tx.clone(), ext::dir());
        self.status = Some(format!("Installing {what}…"));
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
        let Some((i, mut got)) = self.pending.take() else { return };
        let value = self.query(cx).trim().to_string();
        let spec = &self.scripts[i].args[got.len()];
        if value.is_empty() && !spec.optional {
            self.pending = Some((i, got));
            self.status = Some(format!("{} is required", spec.placeholder));
            return cx.notify();
        }
        got.push(value);
        if got.len() < self.scripts[i].args.len() {
            self.pending = Some((i, got));
            self.status = None;
            self.input.update(cx, |s, cx| s.emplace("", cx));
            return self.refresh(cx);
        }
        self.start_script(i, got, cx);
    }

    fn start_script(&mut self, i: usize, args: Vec<String>, cx: &mut Context<Self>) {
        let script = self.scripts[i].clone();
        match script.output {
            Output::Full => {
                self.output = Some((i, String::new(), true));
                self.set_mode(Mode::Output, cx);
            }
            Output::Compact => {
                self.status = Some(format!("Running {}…", script.title));
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

    fn reload(&mut self, cx: &mut Context<Self>) {
        match config::load() {
            Ok(cfg) => {
                if hotkey_specs(&cfg) != hotkey_specs(&self.cfg) {
                    self.hotkey_errors.clear();
                    self.hotkeys.set(hotkey_specs(&cfg));
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
            Msg::Clip(text) => {
                if self.cfg.clipboard.enabled {
                    self.history.push(text, search::now());
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
                            self.status = (!ok).then(|| format!("{} failed", script.title));
                            self.refresh(cx);
                        }
                    }
                    Output::Compact => {
                        self.status = Some(if ok { if last.is_empty() { format!("{} done", script.title) } else { last } } else { format!("{} failed: {last}", script.title) });
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
                self.status = Some(message);
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
                self.tick(cx);
            }
            Msg::ImageReady(path) => {
                self.images.remove(&path);
                if self.visible && self.mode == Mode::Ext {
                    self.refresh(cx);
                }
            }
            Msg::Store(entries) => {
                self.store_loading = false;
                self.status = Some(format!("{} Windows-capable extensions", entries.iter().filter(|s| s.windows).count()));
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
            Mode::Args => self.pending.as_ref().map(|(i, _)| self.scripts[*i].title.clone()),
            Mode::Output => self.output.as_ref().map(|(i, _, _)| self.scripts[*i].title.clone()),
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
            Mode::Ext => self.extv.as_ref().and_then(|e| e.view.placeholder.clone()).unwrap_or_else(|| "Search…".into()).into(),
            Mode::Args => self
                .pending
                .as_ref()
                .map(|(i, got)| {
                    let a = &self.scripts[*i].args[got.len()];
                    if a.optional { format!("{} (optional)", a.placeholder) } else { a.placeholder.clone() }
                })
                .unwrap_or_default()
                .into(),
            Mode::Output => "Enter copies the output".into(),
        }
    }

    /// Text on the left of the action bar: a toast, index status, or where you are.
    fn footer_left(&self) -> String {
        if let Some(s) = &self.status {
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
        self.label().unwrap_or_else(|| "akscast".into())
    }

    /// The icon on the left of the action bar.
    fn context_icon(&self) -> Icon {
        match self.mode {
            Mode::Ext => self.extv.as_ref().map(|e| self.exts[e.ext].1.clone()).unwrap_or(Icon::Fluent(g::PUZZLE)),
            Mode::Root => Icon::Fluent(g::SEARCH),
            Mode::Clipboard => Icon::Fluent(g::PASTE),
            Mode::Emoji => Icon::Fluent(g::EMOJI),
            Mode::Files => Icon::Fluent(g::FOLDER),
            Mode::Store => Icon::Fluent(g::SHOP),
            Mode::Install => Icon::Fluent(g::DOWNLOAD),
            Mode::Args | Mode::Output => Icon::Fluent(g::COMMAND),
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
        let field_box = |on: bool| div().w_full().h(px(34.)).px(px(10.)).flex().items_center().rounded(px(6.)).bg(t.surface).border_1().border_color(if on { t.accent } else { t.border });
        div()
            .id("form")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .pt(px(28.))
            .child(div().w(px(540.)).flex().flex_col().gap(px(14.)).children(ev.view.fields.iter().enumerate().map(|(i, f)| {
                let on = i == ev.field;
                let value = ev.values.get(&f.id).cloned().unwrap_or(Value::Null);
                let control = match &f.kind {
                    FieldKind::Text | FieldKind::Password | FieldKind::TextArea => match ev.inputs.get(&f.id) {
                        Some(input) => field_box(on)
                            .when(f.kind == FieldKind::TextArea, |d| d.h(px(64.)).items_start().py(px(8.)))
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
                            .h(px(34.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(design::fluent(if checked { g::CHECKBOX_ON } else { g::CHECKBOX }, 16., if checked { t.accent } else { t.text_muted }))
                            .child(div().text_color(if on { t.text } else { t.text_muted }).child(f.title.clone()))
                            .into_any_element()
                    }
                    FieldKind::Dropdown(opts) => {
                        let cur = value.as_str().unwrap_or_default();
                        let title = opts.iter().find(|o| o.0 == cur).map(|o| o.1.clone()).unwrap_or_default();
                        field_box(on).justify_between().child(div().truncate().child(title)).child(design::fluent(g::CHEVRON_DOWN, 11., t.text_muted)).into_any_element()
                    }
                    FieldKind::Description => div().text_size(px(13.)).text_color(t.text_muted).child(value.as_str().unwrap_or_default().to_string()).into_any_element(),
                    FieldKind::Separator => div().h(px(1.)).w_full().bg(t.hairline).into_any_element(),
                };
                let label = if matches!(f.kind, FieldKind::Checkbox | FieldKind::Separator) { String::new() } else { f.title.clone() };
                div()
                    .flex()
                    .items_start()
                    .gap(px(14.))
                    .child(div().w(px(130.)).flex_none().h(px(34.)).flex().items_center().justify_end().text_size(px(13.)).text_color(t.text_muted).child(label))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(control)
                            .children(f.error.clone().map(|e| div().text_size(px(12.)).text_color(t.danger).child(e))),
                    )
            })))
            .into_any_element()
    }

    fn render_markdown(md: &str, metadata: &[(String, String)], t: &Tokens, id: &'static str) -> gpui::AnyElement {
        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(16.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(ext::markdown_lines(md).into_iter().map(|(level, text)| {
                let line = div().line_height(px(21.)).child(if text.is_empty() { " ".to_string() } else { text });
                match level {
                    1 => line.pt(px(4.)).font_family(design::FONT_DISPLAY).text_size(px(20.)).font_weight(FontWeight::SEMIBOLD),
                    2 => line.pt(px(4.)).text_size(px(16.)).font_weight(FontWeight::SEMIBOLD),
                    3 => line.font_weight(FontWeight::SEMIBOLD),
                    9 => line.font_family(design::FONT_MONO).text_size(px(13.)).px(px(10.)).bg(t.surface).rounded(px(4.)),
                    _ => line.text_color(t.text),
                }
            }))
            .when(!metadata.is_empty(), |d| {
                d.child(div().mt(px(14.)).pt(px(12.)).border_t_1().border_color(t.hairline).text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_faint).child("Information"))
                    .children(metadata.iter().map(|(k, v)| {
                        div()
                            .flex()
                            .justify_between()
                            .gap(px(16.))
                            .py(px(8.))
                            .border_b_1()
                            .border_color(t.hairline)
                            .text_size(px(13.))
                            .child(div().flex_none().text_color(t.text_muted).child(k.clone()))
                            .child(div().min_w_0().truncate().child(v.clone()))
                    }))
            })
            .into_any_element()
    }
}

/// Fluent glyph for an action in the Ctrl+K panel.
fn act_glyph(act: &Act) -> char {
    match act {
        Act::Favorite(..) => g::STAR,
        Act::Hide(_) => g::CANCEL,
        Act::Copy(_) => g::COPY,
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

/// Test seam: with AKSCAST_E2E=1, each render writes what's on screen to
/// %TEMP%\<file> so spikes/e2e.py can assert on it.
fn dump_ui(file: &str, state: Value) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("AKSCAST_E2E").is_some()) {
        let _ = std::fs::write(std::env::temp_dir().join(file), state.to_string());
    }
}

/// %APPDATA%\akscast\extensions.log: extension errors and console output.
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
    Row { idx: usize, title: String, subtitle: String, accessory: String, icon: Icon },
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        design::scale_launcher();
        // Rem-based spacing follows the scale too.
        window.set_rem_size(px(16.));
        let t = Tokens::from_theme(&self.cfg.theme, window);
        let notices: Vec<String> = self.notice.iter().chain(self.hotkey_errors.iter().flatten()).cloned().collect();
        let empty = (self.items.len() == 1 && self.items[0].section.as_deref() == Some(EMPTY)).then(|| self.items[0].clone());
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
            "akscast-ui.json",
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
                "notice": self.notice,
                "view": view.as_ref().map(|v| &v.kind),
                "fields": self.extv.as_ref().map(|e| e.view.fields.iter().map(|f| &f.title).collect::<Vec<_>>()),
                "field": self.extv.as_ref().map(|e| e.field),
                "trays": self.trays.keys().collect::<Vec<_>>(),
            }),
        );

        // ---- results list ----
        let slots: Vec<Slot> = {
            let mut out = vec![];
            let rows = display_rows(&self.items);
            let count = |sec: &Option<String>| self.items.iter().filter(|it| &it.section == sec).count();
            for (k, r) in rows.iter().enumerate() {
                match r {
                    None => {
                        let it = &self.items[rows[k + 1].unwrap_or(0)];
                        let sec = it.section.clone();
                        out.push(Slot::Header(sec.clone().unwrap_or_default(), (self.mode != Mode::Root).then(|| count(&sec))));
                    }
                    Some(i) => {
                        let it = &self.items[*i];
                        out.push(Slot::Row { idx: *i, title: it.title.clone(), subtitle: it.subtitle.clone(), accessory: it.accessory.clone(), icon: it.icon.clone() });
                    }
                }
            }
            out
        };
        let selected = self.selected;
        let me = cx.weak_entity();
        let slots = Rc::new(slots);
        // Narrow split lists (detail on the right) get a tighter accessory column.
        let acc_max = if view.as_ref().is_some_and(|v| v.show_detail) { 110. } else { 220. };
        let list = {
            let slots = slots.clone();
            uniform_list("rows", slots.len(), move |range, _, _| {
                range
                    .map(|k| match &slots[k] {
                        Slot::Header(title, count) => design::section_header(title, *count, &t).into_any_element(),
                        Slot::Row { idx, title, subtitle, accessory, icon } => {
                            let (i, on) = (*idx, *idx == selected);
                            let me = me.clone();
                            div()
                                .id(k)
                                .h(px(design::ROW))
                                .w_full()
                                .px(px(6.))
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .relative()
                                        .h(px(36.))
                                        .w_full()
                                        .px(px(10.))
                                        .flex()
                                        .items_center()
                                        .gap(px(12.))
                                        .rounded(px(8.))
                                        .when(on, |d| d.bg(t.surface))
                                        .when(!on, |d| d.hover(|s| s.bg(t.surface)))
                                        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                                            me.update(cx, |this, cx| {
                                                this.selected = i;
                                                this.launch(window, cx);
                                            })
                                            .ok();
                                        })
                                        // Fluent selection pill.
                                        .child(div().absolute().left(px(0.)).top(px(10.)).w(px(3.)).h(px(16.)).rounded_full().when(on, |d| d.bg(t.accent)))
                                        .child(design::icon_view(icon, &t, on))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .gap(px(10.))
                                                // Title keeps priority; the subtitle gives way first.
                                                .child(div().min_w(px(48.)).flex_shrink_0().max_w(gpui::relative(0.7)).truncate().text_color(t.text).child(title.clone()))
                                                .when(!subtitle.is_empty(), |d| d.child(div().min_w_0().flex_1().truncate().text_size(px(13.)).text_color(t.text_muted).child(subtitle.clone()))),
                                        )
                                        .when(!accessory.is_empty(), |d| d.child(div().flex_none().max_w(px(acc_max)).truncate().text_size(px(13.)).text_color(t.text_muted).child(accessory.clone()))),
                                )
                                .into_any_element()
                        }
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1()
            .min_h_0()
            .py(px(4.))
        };

        // ---- body ----
        let body = match (&empty, &view) {
            (Some(e), _) => {
                let glyph = if let Icon::Fluent(c) = e.icon { c } else { g::SEARCH };
                design::empty_state(glyph, &e.title, &e.subtitle, &t).into_any_element()
            }
            (_, Some(v)) if v.kind == "Form" => self.render_form(&t),
            (_, Some(v)) if v.kind == "Detail" => Self::render_markdown(&v.markdown, &v.metadata, &t, "detail"),
            (_, Some(v)) if v.show_detail && !v.items.is_empty() => {
                let it = self.items.get(self.selected).and_then(|row| v.items.iter().find(|it| it.title == row.title));
                let md = it.and_then(|it| it.detail.clone()).unwrap_or_default();
                let meta = it.map(|it| it.detail_meta.clone()).unwrap_or_default();
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().w(px(base_width(&self.cfg) * 0.4)).flex_none().flex().flex_col().min_h_0().child(list))
                    .child(div().flex_1().min_w_0().flex().flex_col().border_l_1().border_color(t.hairline).child(Self::render_markdown(&md, &meta, &t, "item-detail")))
                    .into_any_element()
            }
            _ if self.confirm.is_some() => div().flex_1().into_any_element(),
            _ => list.into_any_element(),
        };

        // ---- compact mode: favourites as icons ----
        let collapsed = self.collapsed(cx);
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
                    .size(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .hover(|s| s.bg(t.surface_strong))
                    .on_click(cx.listener(move |this, _, window, cx| this.run(act.clone(), key.clone(), window, cx)))
                    .child(design::icon_view(&it.icon, &t, false))
            }));

        // ---- search bar ----
        let at_root = self.mode == Mode::Root;
        let leading = if at_root {
            div().size(px(28.)).flex_none().flex().items_center().justify_center().child(design::fluent(g::SEARCH, 17., t.accent)).into_any_element()
        } else {
            div()
                .id("back")
                .size(px(28.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .bg(t.surface)
                .hover(|s| s.bg(t.surface_strong))
                .on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx)))
                .child(design::fluent(g::BACK, 13., t.text))
                .into_any_element()
        };
        let dropdown = self.extv.as_ref().filter(|_| self.mode == Mode::Ext).and_then(|e| e.view.dropdown.as_ref()).map(|(opts, cur, _)| {
            let title = opts.iter().find(|o| &o.0 == cur).map(|o| o.1.clone()).unwrap_or_default();
            div()
                .flex_none()
                .h(px(32.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(t.border)
                .text_size(px(13.))
                .child(design::fluent(g::FILTER, 12., t.text_muted))
                .child(div().max_w(px(180.)).truncate().child(title))
                .child(design::fluent(g::CHEVRON_DOWN, 10., t.text_muted))
        });
        let search_bar = div()
            .h(px(design::BAR))
            .flex_none()
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(12.))
            .border_b_1()
            .border_color(t.hairline)
            .child(leading)
            .child(
                text_input("query")
                    .state(self.input.downgrade())
                    .placeholder(self.placeholder())
                    .placeholder_color(t.text_faint)
                    .caret_color(t.accent)
                    .caret_blink_interval_500ms()
                    .flex_1()
                    .font_family(design::FONT_DISPLAY)
                    .text_size(px(17.))
                    .whitespace_nowrap(),
            )
            .children(dropdown);

        // ---- action bar ----
        let tone = match self.status.as_deref() {
            Some(s) if s.contains("failed") || s.contains("error") || s.contains("not supported") => Some(t.danger),
            Some(_) => Some(t.success),
            None => None,
        };
        let footer = div()
            .h(px(design::FOOTER))
            .flex_none()
            .px(px(14.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .border_t_1()
            .border_color(t.hairline)
            .text_size(px(13.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(match tone {
                        Some(c) => div().size(px(8.)).mx(px(4.)).rounded_full().bg(c).into_any_element(),
                        None => match self.context_icon() {
                            Icon::Fluent(ch) => design::fluent(ch, 14., if at_root { t.accent } else { t.text_muted }).into_any_element(),
                            other => div().size(px(16.)).flex().items_center().justify_center().child(design::icon_view(&other, &t, false)).into_any_element(),
                        },
                    })
                    .child(div().min_w_0().truncate().text_color(if tone.is_some() { t.text } else { t.text_muted }).child(self.footer_left())),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .map(|d| {
                        if self.panel.is_some() {
                            d.child(design::shortcut("Run", &["↵"], &t)).child(design::vdivider(&t)).child(design::shortcut("Close", &["Esc"], &t))
                        } else if self.is_form() {
                            d.child(design::shortcut("Submit", &["Ctrl", "↵"], &t)).child(design::vdivider(&t)).child(design::shortcut("Actions", &["Ctrl", "K"], &t))
                        } else if self.armed == Some(self.selected) {
                            d.child(div().text_color(t.warning).child("Press ↵ again to confirm"))
                        } else if let Some(p) = &primary {
                            let d = d.child(design::shortcut(p, &["↵"], &t));
                            if actions.len() > 1 { d.child(design::vdivider(&t)).child(design::shortcut("Actions", &["Ctrl", "K"], &t)) } else { d }
                        } else {
                            d
                        }
                    }),
            );

        // ---- overlays ----
        let popover = self.panel.as_ref().map(|(item, sel)| {
            let acts = item.actions();
            let me = cx.weak_entity();
            div()
                .absolute()
                .right(px(10.))
                .bottom(px(design::FOOTER + 6.))
                .w(px(330.))
                .max_h(px(320.))
                .p(px(6.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.bg_elevated)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .child(div().px(px(10.)).pt(px(4.)).pb(px(6.)).text_size(px(12.)).text_color(t.text_faint).truncate().child(item.title.clone()))
                .child(div().id("panel").flex().flex_col().overflow_y_scroll().children(acts.into_iter().enumerate().map(|(i, (label, act))| {
                    let on = i == *sel;
                    let me = me.clone();
                    let keys: &[&str] = match i {
                        0 => &["↵"],
                        1 => &["Ctrl", "↵"],
                        _ => &[],
                    };
                    div()
                        .id(("act", i))
                        .relative()
                        .h(px(34.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .rounded(px(6.))
                        .when(on, |d| d.bg(t.surface))
                        .hover(|s| s.bg(t.surface))
                        .on_click(move |_, window, cx| {
                            me.update(cx, |this, cx| {
                                if let Some((_, p)) = &mut this.panel {
                                    *p = i;
                                }
                                this.launch(window, cx);
                            })
                            .ok();
                        })
                        .child(div().absolute().left(px(0.)).top(px(9.)).w(px(3.)).h(px(16.)).rounded_full().when(on, |d| d.bg(t.accent)))
                        .child(design::fluent(act_glyph(&act), 14., if on { t.text } else { t.text_muted }))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .child(div().flex().gap(px(4.)).children(keys.iter().map(|k| design::key_cap(k, &t))))
                })))
        });
        let dialog = self.confirm.as_ref().map(|c| {
            let me = cx.weak_entity();
            let button = |i: usize, label: String, danger: bool| {
                let on = i == selected;
                let me = me.clone();
                div()
                    .id(("dialog", i))
                    .h(px(34.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if on { if danger { t.danger } else { t.accent } } else { t.border })
                    .bg(if on && i == 0 { if danger { t.danger } else { t.accent } } else { t.surface })
                    .text_color(if on && i == 0 { t.accent_text } else { t.text })
                    .on_click(move |_, window, cx| {
                        me.update(cx, |this, cx| {
                            this.selected = i;
                            this.launch(window, cx);
                        })
                        .ok();
                    })
                    .child(label)
            };
            let danger = c.primary.to_lowercase().contains("delete") || c.primary.to_lowercase().contains("remove");
            div().absolute().inset_0().flex().items_center().justify_center().bg(gpui::hsla(0., 0., 0., 0.35)).child(
                div()
                    .w(px(340.))
                    .p(px(18.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .rounded(px(12.))
                    .bg(t.bg_elevated)
                    .border_1()
                    .border_color(t.border)
                    .shadow_lg()
                    .child(design::fluent(if danger { g::WARNING } else { g::INFO }, 26., if danger { t.danger } else { t.accent }))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_center().child(c.title.clone()))
                    .children((!c.message.is_empty()).then(|| div().text_size(px(13.)).text_color(t.text_muted).text_center().child(c.message.clone())))
                    .child(div().w_full().pt(px(6.)).flex().flex_col().gap(px(8.)).child(button(0, c.primary.clone(), danger)).child(button(1, "Cancel".into(), false))),
            )
        });

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .font_family(design::FONT_UI)
            .text_size(px(14.))
            .bg(t.bg)
            .text_color(t.text)
            .border_1()
            .border_color(t.border)
            .rounded(px(8.))
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                if this.panel.is_none() && this.is_form() {
                    return this.move_field(1, window, cx);
                }
                if this.collapsed(cx) {
                    this.peek = true;
                    return this.refresh(cx);
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
                if this.panel.is_none() && this.is_form() {
                    return this.move_field(-1, window, cx);
                }
                if this.panel.is_none() && this.selected == 0 && this.recall_search(cx) {
                    return;
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
                    .mx(px(14.))
                    .mt(px(8.))
                    .px(px(10.))
                    .py(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(6.))
                    .bg(t.surface)
                    .text_size(px(13.))
                    .child(design::fluent(g::WARNING, 13., t.warning))
                    .child(div().min_w_0().child(n))
            }))
            .when(collapsed && self.fav_strip(), |d| d.child(fav_strip))
            .when(!collapsed, |d| d.child(body).child(footer))
            .children(popover)
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
    let open = MenuItem::new("Open akscast", true, None);
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
        // Only akscast's own icon opens the launcher; extension icons just show their menu.
        if let TrayIconEvent::Click { id, button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e
            && id.0 == "akscast"
        {
            let _ = tx.unbounded_send(Msg::Tray(TrayCmd::Open));
        }
    }));
    TrayIconBuilder::new().with_id("akscast").with_menu(Box::new(menu)).with_tooltip("akscast").with_icon(tray_icon_image()).build().ok()
}

fn main() {
    // The whole-drive file index helper (see ntfs.rs).
    match std::env::args().nth(1).as_deref() {
        Some("--indexer") => return ntfs::serve(),
        Some(flag @ ("--install-indexer" | "--uninstall-indexer")) => {
            let done = if flag == "--install-indexer" { ntfs::install() } else { ntfs::uninstall() };
            if let Err(e) = done {
                let _ = std::fs::write(std::env::temp_dir().join("akscast-indexer-error.txt"), e);
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
    gpui_ce_platform::application().with_quit_mode(QuitMode::Explicit).run(|cx: &mut App| {
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
            Ok(cfg) => (cfg, None),
            Err(e) => (Config::default(), Some(e)),
        };
        win::set_start_at_login(cfg.start_at_login);
        net::load_rates();
        design::set_size(&cfg.size);
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
            // Acrylic: the translucent g token lets the blurred desktop show through.
            windows_window_background: gpui::WindowsWindowBackground::Blurred,
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
            if std::env::var_os("AKSCAST_OPEN_SETTINGS").is_some() {
                let _ = this.tx.unbounded_send(Msg::Tray(TrayCmd::Settings));
            }
        });

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
                    let _ = handle.update(cx, |this, _, cx| this.reload(cx));
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
