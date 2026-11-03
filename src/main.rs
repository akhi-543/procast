#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod commands;
mod config;
mod ext;
mod files;
mod scripts;
mod search;
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
    actions, colors::Colors, div, img, prelude::*, px, rgb, rgb_to_hsla, size, uniform_list,
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
use ext::FieldKind;
use scripts::{Output, Script};
use search::{Act, Frecency, Icon, Item};
use win::{Bind, Msg, TrayCmd};

actions!(akscast, [SelectPrev, SelectNext, Dismiss, Launch, ToggleActions, Submit]);

const BAR: f32 = 56.;
const ROW: f32 = 44.;
const FOOTER: f32 = 34.;

fn window_height(cfg: &Config) -> f32 {
    BAR + ROW * cfg.max_results.max(1) as f32 + 12. + FOOTER
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

fn hotkey_specs(cfg: &Config) -> [String; 4] {
    [cfg.hotkey.clone(), cfg.clipboard.hotkey.clone(), cfg.emoji.hotkey.clone(), cfg.files.hotkey.clone()]
}

fn start_files(cfg: &Config, tx: &UnboundedSender<Msg>) -> Option<files::Files> {
    cfg.files.enabled.then(|| files::Files::start(cfg.files.roots(), cfg.files.exclude.clone(), tx.clone()))
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
    confirm: Option<Confirm>,
    images: HashMap<PathBuf, Option<Arc<RenderImage>>>,
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
    hotkey_errors: [Option<String>; 4],
    hwnd: HWND,
    prev: HWND,
    visible: bool,
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
            confirm: None,
            images: HashMap::new(),
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
            hotkey_errors: Default::default(),
            hwnd: HWND::default(),
            prev: HWND::default(),
            visible: false,
            _subs: subs,
        };
        this.load_extensions();
        this
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).as_str().to_string()
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
        let Some(icon) = icon else { return Icon::glyph("•") };
        if let Some(name) = icon.strip_prefix("icon:") {
            return Icon::glyph(ext::icon_glyph(name));
        }
        if icon.starts_with("http") || icon.starts_with("data:") {
            return Icon::glyph("🌐");
        }
        if icon.chars().count() <= 4 && !icon.contains('.') {
            return Icon::glyph(icon.to_string());
        }
        let dir = self.exts[ext].0.dir.join("assets");
        let path = if std::path::Path::new(icon).is_absolute() { PathBuf::from(icon) } else { dir.join(icon) };
        match self.image(path) {
            Some(i) => Icon::Image(i),
            None => Icon::glyph("•"),
        }
    }

    fn load_extensions(&mut self) {
        let exts = ext::scan();
        let mut out = Vec::with_capacity(exts.len());
        for e in exts {
            let icon = e.icon.clone().and_then(|p| self.image(p)).map(Icon::Image).unwrap_or(Icon::glyph("🧩"));
            out.push((e, icon));
        }
        self.exts = out;
        self.catalog = search::catalog(&self.apps, &self.scripts, &self.exts, &self.cfg);
    }

    fn ext_rows(&mut self, query: &str) -> Vec<Item> {
        if let Some(c) = &self.confirm {
            let mut ok = Item::new(c.primary.clone(), c.message.clone(), Icon::glyph("?"), Act::Confirm(true));
            ok.extra = vec![(c.primary.clone(), Act::Confirm(true))];
            return vec![ok, Item::new("Cancel", "", Icon::glyph("✕"), Act::Confirm(false))];
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
                        let sub: Vec<&str> = std::iter::once(it.subtitle.as_str()).chain(it.accessories.iter().map(String::as_str)).filter(|s| !s.is_empty()).collect();
                        let hay = [it.title.as_str(), it.subtitle.as_str(), &it.keywords.join(" "), it.section.as_deref().unwrap_or("")].join(" ");
                        let mut row = Item::new(&it.title, sub.join("  ·  "), self.ext_icon(ext, it.icon.as_deref()), primary).with_hay(hay);
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
                        row.extra.extend(opts.iter().map(|(v, t)| (format!("{} {t}", if v == cur { "●" } else { "Filter:" }), Act::ExtSet(f.clone(), v.clone()))));
                    }
                }
                if rows.is_empty() {
                    let (title, sub) = match &view.empty {
                        _ if view.loading => ("Loading…".to_string(), String::new()),
                        Some((t, d)) if !t.is_empty() => (t.clone(), d.clone()),
                        _ => ("No results".to_string(), String::new()),
                    };
                    let mut row = Item::new(title, sub, Icon::glyph(if view.loading { "⏳" } else { "∅" }), view_actions.first().map(|a| a.1.clone()).unwrap_or(Act::None));
                    row.extra = view_actions;
                    rows.push(row);
                }
                rows
            }
            "" => vec![Item::new("Loading…", "", Icon::glyph("⏳"), Act::None)],
            // Detail and Form draw their own body; Enter and Ctrl+K use the view's actions.
            _ => {
                let mut row = Item::new(view.title.clone().unwrap_or_default(), "", Icon::glyph("•"), view_actions.first().map(|a| a.1.clone()).unwrap_or(Act::None));
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
            Mode::Root => search::root(&query, &self.catalog, &self.cfg, &self.frec, &mut self.matcher),
            Mode::Clipboard => search::clipboard(&query, &self.history.entries, 200, &mut self.matcher),
            Mode::Emoji => search::emoji(&query, 200, &mut self.matcher),
            Mode::Files => {
                // Results arrive later as Msg::Files; keep showing the previous ones meanwhile.
                self.file_gen += 1;
                if let Some(f) = &self.files {
                    f.search(self.file_gen, &query, 50);
                }
                if query.trim().is_empty() { vec![] } else { std::mem::take(&mut self.items) }
            }
            Mode::Args => match &self.pending {
                Some((i, got)) => {
                    let s = &self.scripts[*i];
                    let n = s.args.len();
                    vec![Item::new(format!("Run {}", s.title), format!("argument {} of {n}", got.len() + 1), Icon::glyph("⌨"), Act::Script(*i))]
                }
                None => vec![],
            },
            Mode::Output => match &self.output {
                Some((_, _, true)) => vec![Item::new("Running…", "", Icon::glyph("⏳"), Act::Copy(String::new()))],
                Some((_, text, false)) => {
                    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                    let shown = if lines.is_empty() { vec!["(no output)"] } else { lines };
                    shown.iter().map(|l| Item::new(*l, "", Icon::glyph("›"), Act::Copy(text.clone()))).collect()
                }
                None => vec![],
            },
            Mode::Ext => self.ext_rows(&query),
            Mode::Install => {
                let what = query.trim();
                let title = if what.is_empty() { "Type a store name (e.g. obsidian) or a folder path".to_string() } else { format!("Install {what}") };
                vec![Item::new(title, "builds with ray build; needs git, Node and npm", Icon::glyph("🧩"), Act::None)]
            }
        };
        let _ = max;
        self.selected = if keep_selection { self.selected.min(self.items.len().saturating_sub(1)) } else { 0 };
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn rebuild_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog = search::catalog(&self.apps, &self.scripts, &self.exts, &self.cfg);
        if self.mode == Mode::Root {
            self.refresh(cx);
        }
    }

    /// Off-thread: names arrive first (~1 s), icons follow.
    // ponytail: no disk cache, so the app list is empty for the first second after start.
    fn scan_apps(&mut self, force: bool) {
        if !force && self.scanned.is_some_and(|t| t.elapsed() < Duration::from_secs(300)) {
            return;
        }
        self.scanned = Some(Instant::now());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut apps = win::scan_apps();
            let _ = tx.unbounded_send(Msg::Apps(apps.clone()));
            win::load_icons(&mut apps);
            let _ = tx.unbounded_send(Msg::Apps(apps));
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
            f.refresh_if_stale(Duration::from_secs(120));
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
        self.set_mode(mode, cx);
        win::cloak(self.hwnd, false);
        win::focus(self.hwnd);
        window.activate();
        window.focus(&self.input.focus_handle(cx), cx);
        self.visible = true;
        self.scan_apps(false);
    }

    /// `restore` hands focus back to the window that was active before we opened;
    /// false only when focus already moved elsewhere (deactivation).
    fn hide(&mut self, restore: bool, cx: &mut Context<Self>) {
        win::cloak(self.hwnd, true);
        self.visible = false;
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
        if cmd.mode == "menu-bar" || !self.ensure_host() {
            return cx.notify();
        }
        self.next_run += 1;
        let run = self.next_run;
        let view = cmd.mode == "view" && !background;
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

    /// Starts interval commands that are due (and not already running).
    fn tick(&mut self, cx: &mut Context<Self>) {
        if !self.cfg.extensions.background {
            return;
        }
        let due: Vec<(usize, usize)> = self
            .exts
            .iter()
            .enumerate()
            .flat_map(|(e, (x, _))| x.commands.iter().enumerate().map(move |(c, cmd)| (e, c, x, cmd)))
            .filter(|(_, _, x, cmd)| {
                let key = format!("{}/{}", x.name, cmd.name);
                cmd.mode == "no-view"
                    && cmd.interval.is_some_and(|i| self.bg_last.get(&key).is_none_or(|t| t.elapsed() >= i))
                    && !self.bg_runs.values().any(|k| *k == key)
            })
            .map(|(e, c, _, _)| (e, c))
            .collect();
        for (e, c) in due {
            let key = format!("{}/{}", self.exts[e].0.name, self.exts[e].0.commands[c].name);
            self.bg_last.insert(key, Instant::now());
            self.start_ext(e, c, true, json!({}), cx);
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
            return self.install(cx);
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
        if let Some(key) = &key {
            self.frec.record(key, search::now());
        }
        match act {
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
            Act::Cmd(Cmd::ClearClipboardHistory) => self.history.clear(),
            Act::Cmd(Cmd::EditConfig) => win::open(&config::path().to_string_lossy()),
            Act::Cmd(Cmd::ReloadApps) => self.scan_apps(true),
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

    fn install(&mut self, cx: &mut Context<Self>) {
        let what = self.query(cx).trim().to_string();
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
                    self.hotkeys.set(hotkey_specs(&cfg));
                }
                if cfg.start_at_login != self.cfg.start_at_login {
                    win::set_start_at_login(cfg.start_at_login);
                }
                let same_files = cfg.files.enabled == self.files.is_some()
                    && self.files.as_ref().is_none_or(|f| f.same_roots(&cfg.files.roots(), &cfg.files.exclude));
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
            Msg::Hotkey(bind) if self.visible && self.mode == Mode::of(bind) => self.hide(true, cx),
            Msg::Hotkey(bind) => self.show(Mode::of(bind), window, cx),
            Msg::Tray(TrayCmd::Open) => self.show(Mode::Root, window, cx),
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
            Msg::Files(generation, items) => {
                if self.mode == Mode::Files && generation == self.file_gen && !self.query(cx).trim().is_empty() {
                    self.items = items;
                    self.selected = 0;
                    self.scroll.scroll_to_item(0, ScrollStrategy::Top);
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
            Msg::Tick => self.tick(cx),
            Msg::HotkeyStatus(bind, status) => {
                self.hotkey_errors[bind as usize - 1] = status.err();
                // Without a working main hotkey the user can't reach us, so open and say why.
                if bind == Bind::Launcher && self.hotkey_errors[0].is_some() {
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
            Mode::Root => "Search apps and commands, calculate, > to run".into(),
            Mode::Clipboard => "Search clipboard history".into(),
            Mode::Emoji => "Search emoji".into(),
            Mode::Files => "Search files by name, or a path with \\".into(),
            Mode::Install => "Extension name (e.g. obsidian) or folder path".into(),
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

    fn footer_left(&self) -> String {
        if let Some(s) = &self.status {
            return s.clone();
        }
        if self.mode == Mode::Files
            && let Some(f) = &self.files
        {
            let n = f.count.load(std::sync::atomic::Ordering::SeqCst);
            return if f.indexing.load(std::sync::atomic::Ordering::SeqCst) { format!("Indexing… ({n} so far)") } else { format!("{n} files indexed") };
        }
        if self.mode == Mode::Ext && self.extv.as_ref().is_some_and(|e| e.view.loading) {
            return "Loading…".into();
        }
        self.label().unwrap_or_else(|| "akscast".into())
    }

    fn render_form(&self, colors: &Colors) -> gpui::AnyElement {
        let Some(ev) = &self.extv else { return div().into_any_element() };
        div()
            .id("form")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .children(ev.view.fields.iter().enumerate().map(|(i, f)| {
                let on = i == ev.field;
                let value = ev.values.get(&f.id).cloned().unwrap_or(Value::Null);
                let control = match &f.kind {
                    FieldKind::Text | FieldKind::Password | FieldKind::TextArea => match ev.inputs.get(&f.id) {
                        Some(input) => text_input(SharedString::from(format!("field-{}", f.id)))
                            .state(input.downgrade())
                            .placeholder(f.placeholder.clone())
                            .placeholder_color(colors.disabled)
                            .caret_color(rgb_to_hsla(colors.text))
                            .w_full()
                            .whitespace_nowrap()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(if on { colors.selected } else { colors.border })
                            .bg(colors.container)
                            .into_any_element(),
                        None => div().into_any_element(),
                    },
                    FieldKind::Checkbox => div().child(format!("{}  {}", if value.as_bool().unwrap_or(false) { "☑" } else { "☐" }, f.title)).into_any_element(),
                    FieldKind::Dropdown(opts) => {
                        let cur = value.as_str().unwrap_or_default();
                        let title = opts.iter().find(|o| o.0 == cur).map(|o| o.1.clone()).unwrap_or_default();
                        div().px_2().py_1().rounded_md().border_1().border_color(if on { colors.selected } else { colors.border }).child(format!("{title}  ▾")).into_any_element()
                    }
                    FieldKind::Description => div().text_sm().text_color(colors.disabled).child(value.as_str().unwrap_or_default().to_string()).into_any_element(),
                    FieldKind::Separator => div().h(px(1.)).w_full().bg(colors.separator).into_any_element(),
                };
                let label = if matches!(f.kind, FieldKind::Checkbox | FieldKind::Separator) { String::new() } else { f.title.clone() };
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .when(on && matches!(f.kind, FieldKind::Checkbox | FieldKind::Dropdown(_)), |d| d.bg(colors.container))
                    .child(div().w(px(150.)).flex_none().text_sm().text_color(colors.disabled).truncate().child(label))
                    .child(div().flex_1().min_w_0().child(control))
                    .children(f.error.clone().map(|e| div().flex_none().text_xs().text_color(rgb(0xff6b6b)).child(e)))
            }))
            .into_any_element()
    }

    fn render_markdown(md: &str, metadata: &[(String, String)], colors: &Colors, id: &'static str) -> gpui::AnyElement {
        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .children(ext::markdown_lines(md).into_iter().map(|(level, text)| {
                let line = div().child(if text.is_empty() { " ".to_string() } else { text });
                match level {
                    1 => line.text_xl().font_weight(FontWeight::SEMIBOLD),
                    2 => line.text_lg().font_weight(FontWeight::SEMIBOLD),
                    3 => line.font_weight(FontWeight::SEMIBOLD),
                    9 => line.font_family("Consolas").text_sm(),
                    _ => line,
                }
            }))
            .children(metadata.iter().map(|(t, v)| div().flex().gap_3().text_sm().child(div().w(px(140.)).text_color(colors.disabled).child(t.clone())).child(v.clone())))
            .into_any_element()
    }
}

/// Test seam: with AKSCAST_E2E=1, each render writes what's on screen to
/// %TEMP%\akscast-ui.json so spikes/e2e.py can assert on it.
fn dump_ui(state: Value) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("AKSCAST_E2E").is_some()) {
        let _ = std::fs::write(std::env::temp_dir().join("akscast-ui.json"), state.to_string());
    }
}

/// %APPDATA%\akscast\extensions.log: extension errors and console output.
fn ext_log(who: &str, text: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(config::dir().join("extensions.log")) {
        let _ = writeln!(f, "[{who}] {}", text.trim_end());
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Colors::for_appearance(window);
        let notices: Vec<String> = self.notice.iter().chain(self.hotkey_errors.iter().flatten()).cloned().collect();
        // Rows to draw: the Ctrl+K panel replaces the results while it's open.
        let (rows, selected): (Vec<(String, String, Icon)>, usize) = match &self.panel {
            Some((item, i)) => (item.actions().into_iter().map(|(label, _)| (label, String::new(), Icon::glyph("▸"))).collect(), *i),
            None => (self.items.iter().map(|it| (it.title.clone(), it.subtitle.clone(), it.icon.clone())).collect(), self.selected),
        };
        let footer_action = match (&self.panel, self.items.get(self.selected)) {
            (Some(_), _) => "Run  ↵    Close  Esc".to_string(),
            _ if self.is_form() => "Next  ↵    Submit  Ctrl+↵    Actions  Ctrl+K".to_string(),
            (None, Some(it)) if self.armed == Some(self.selected) => format!("Press ↵ again to {}", it.title),
            (None, Some(it)) => {
                let acts = it.actions();
                let label = acts.first().map(|a| a.0.clone()).unwrap_or_default();
                if label.is_empty() { String::new() } else if acts.len() > 1 { format!("{label}  ↵    Actions  Ctrl+K") } else { format!("{label}  ↵") }
            }
            (None, None) => String::new(),
        };
        let view = self.extv.as_ref().filter(|_| self.mode == Mode::Ext && self.panel.is_none() && self.confirm.is_none()).map(|e| e.view.clone());
        dump_ui(json!({
            "visible": self.visible,
            "query": self.query(cx),
            "label": self.label(),
            "rows": rows.iter().map(|r| &r.0).collect::<Vec<_>>(),
            "selected": selected,
            "panel": self.panel.is_some(),
            "footer": [self.footer_left(), footer_action.clone()],
            "notice": self.notice,
            "view": view.as_ref().map(|v| &v.kind),
            "fields": self.extv.as_ref().map(|e| e.view.fields.iter().map(|f| &f.title).collect::<Vec<_>>()),
            "field": self.extv.as_ref().map(|e| e.field),
        }));
        let me = cx.weak_entity();
        let rows = Rc::new(rows);
        let list = {
            let rows = rows.clone();
            uniform_list("rows", rows.len(), move |range, _, _| {
                range
                    .map(|i| {
                        let (title, subtitle, icon) = rows[i].clone();
                        let on = i == selected;
                        let icon = match icon {
                            Icon::Image(image) => img(image).size(px(24.)).into_any_element(),
                            Icon::Glyph(g) => div().size(px(24.)).flex().items_center().justify_center().child(g).into_any_element(),
                        };
                        let me = me.clone();
                        div()
                            .id(i)
                            .h(px(ROW))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_3()
                            .rounded_md()
                            .when(on, |d| d.bg(colors.selected).text_color(colors.selected_text))
                            .on_click(move |_, window, cx| {
                                me.update(cx, |this, cx| {
                                    match &mut this.panel {
                                        Some((_, p)) => *p = i,
                                        None => this.selected = i,
                                    }
                                    this.launch(window, cx);
                                })
                                .ok();
                            })
                            .child(div().flex_none().child(icon))
                            .child(div().flex_1().min_w_0().truncate().child(title))
                            .child(div().flex_none().max_w(px(300.)).truncate().text_sm().text_color(if on { colors.selected_text } else { colors.disabled }).child(subtitle))
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1()
            .min_h_0()
            .p_1p5()
        };
        let body = match &view {
            Some(v) if v.kind == "Form" => self.render_form(&colors),
            Some(v) if v.kind == "Detail" => Self::render_markdown(&v.markdown, &v.metadata, &colors, "detail"),
            Some(v) if v.show_detail && !v.items.is_empty() => {
                let md = self
                    .items
                    .get(self.selected)
                    .and_then(|row| v.items.iter().find(|it| it.title == row.title))
                    .and_then(|it| it.detail.clone())
                    .unwrap_or_default();
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().w(px(self.cfg.width * 0.42)).flex_none().flex().flex_col().min_h_0().child(list))
                    .child(div().flex_1().min_w_0().flex().flex_col().border_l_1().border_color(colors.separator).child(Self::render_markdown(&md, &[], &colors, "item-detail")))
                    .into_any_element()
            }
            _ => list.into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(colors.background)
            .text_color(colors.text)
            .border_1()
            .border_color(colors.border)
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                if this.panel.is_none() && this.is_form() {
                    return this.move_field(1, window, cx);
                }
                match &mut this.panel {
                    Some((item, i)) => *i = (*i + 1).min(item.actions().len().saturating_sub(1)),
                    None => this.selected = (this.selected + 1).min(this.items.len().saturating_sub(1)),
                }
                this.armed = None;
                let at = this.panel.as_ref().map_or(this.selected, |p| p.1);
                this.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| {
                if this.panel.is_none() && this.is_form() {
                    return this.move_field(-1, window, cx);
                }
                match &mut this.panel {
                    Some((_, i)) => *i = i.saturating_sub(1),
                    None => this.selected = this.selected.saturating_sub(1),
                }
                this.armed = None;
                let at = this.panel.as_ref().map_or(this.selected, |p| p.1);
                this.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleActions, _, cx| {
                this.panel = match this.panel.take() {
                    Some(_) => None,
                    None => this.items.get(this.selected).cloned().filter(|it| !it.actions().is_empty() && it.act != Act::None || !it.extra.is_empty()).map(|it| (it, 0)),
                };
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                if this.panel.take().is_some() {
                    cx.notify();
                    if this.is_form() {
                        this.focus_field(window, cx);
                    }
                } else if let Some(c) = this.confirm.take() {
                    this.reply(c.run, c.id, Ok(json!(false)));
                    this.refresh(cx);
                } else if this.mode == Mode::Ext && this.extv.as_ref().is_some_and(|e| e.depth > 1) {
                    let run = this.extv.as_ref().unwrap().run;
                    this.send(json!({"t": "pop", "run": run}));
                } else if this.mode == Mode::Root {
                    this.hide(true, cx)
                } else {
                    this.pending = None;
                    this.set_mode(Mode::Root, cx);
                    window.focus(&this.input.focus_handle(cx), cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Launch, window, cx| this.launch(window, cx)))
            .on_action(cx.listener(|this, _: &Submit, window, cx| if this.is_form() && this.panel.is_none() { this.submit_form(cx) } else { this.launch(window, cx) }))
            .child(
                div()
                    .h(px(BAR))
                    .flex_none()
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(colors.separator)
                    .child(
                        text_input("query")
                            .state(self.input.downgrade())
                            .placeholder(self.placeholder())
                            .placeholder_color(colors.disabled)
                            .caret_color(rgb_to_hsla(colors.text))
                            .caret_blink_interval_500ms()
                            .flex_1()
                            .text_lg()
                            .whitespace_nowrap(),
                    )
                    .children(self.extv.as_ref().filter(|_| self.mode == Mode::Ext).and_then(|e| e.view.dropdown.as_ref()).map(|(opts, cur, _)| {
                        let t = opts.iter().find(|o| &o.0 == cur).map(|o| o.1.clone()).unwrap_or_default();
                        div().flex_none().px_2().py_0p5().rounded_md().text_xs().border_1().border_color(colors.border).child(format!("{t} ▾"))
                    }))
                    .children(self.label().map(|l| {
                        div().flex_none().max_w(px(220.)).truncate().px_2().py_0p5().rounded_md().text_xs().bg(colors.container).text_color(colors.disabled).child(l)
                    })),
            )
            .children(notices.into_iter().map(|n| div().px_4().py_1().text_sm().text_color(rgb(0xff6b6b)).child(n)))
            .child(body)
            .child(
                div()
                    .h(px(FOOTER))
                    .flex_none()
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .border_t_1()
                    .border_color(colors.separator)
                    .text_xs()
                    .text_color(colors.disabled)
                    .child(div().flex_1().min_w_0().truncate().child(self.footer_left()))
                    .child(div().flex_none().child(footer_action)),
            )
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
    let edit = MenuItem::new("Edit config.toml", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&open, &edit, &quit]).ok()?;
    let ids = (open.id().clone(), edit.id().clone(), quit.id().clone());
    let menu_tx = tx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let cmd = match &e.id {
            id if *id == ids.0 => TrayCmd::Open,
            id if *id == ids.1 => TrayCmd::EditConfig,
            id if *id == ids.2 => TrayCmd::Quit,
            _ => return,
        };
        let _ = menu_tx.unbounded_send(Msg::Tray(cmd));
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            let _ = tx.unbounded_send(Msg::Tray(TrayCmd::Open));
        }
    }));
    TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("akscast").with_icon(tray_icon_image()).build().ok()
}

fn main() {
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
        ]);

        let (cfg, notice) = match config::load() {
            Ok(cfg) => (cfg, None),
            Err(e) => (Config::default(), Some(e)),
        };
        win::set_start_at_login(cfg.start_at_login);
        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        let tray = tray(tx.clone());
        let tick_tx = tx.clone();

        let bounds = Bounds::centered(None, size(px(cfg.width), px(window_height(&cfg))), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            show: false,
            focus: false,
            ..Default::default()
        };
        let handle = cx.open_window(options, |window, cx| cx.new(|cx| Launcher::new(cfg, notice, tx, window, cx))).unwrap();

        // Shown once, cloaked: from now on hotkey -> un-cloak -> next frame.
        let _ = handle.update(cx, |this, window, _| {
            this.hwnd = win::hwnd(window);
            win::make_tool_window(this.hwnd);
            win::cloak(this.hwnd, true);
            window.set_visible(true);
            this.scan_apps(true);
        });

        cx.spawn(async move |cx| {
            let _tray = tray;
            while let Some(msg) = rx.next().await {
                // SetWindowPos sends WM_SIZE/WM_DPICHANGED synchronously and GPUI's window
                // proc re-borrows the app, so the window is moved here, between updates.
                let opens = matches!(msg, Msg::Hotkey(_) | Msg::Tray(TrayCmd::Open) | Msg::HotkeyStatus(Bind::Launcher, Err(_)) | Msg::ScriptDone(..));
                let geometry = handle.update(cx, |this, _, _| {
                    (!this.visible).then(|| (this.hwnd, this.cfg.monitor.clone(), this.cfg.width, window_height(&this.cfg)))
                });
                if opens && let Ok(Some((hwnd, monitor, width, height))) = geometry {
                    win::place(hwnd, &monitor, width, height);
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
