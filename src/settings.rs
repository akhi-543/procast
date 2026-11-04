//! The settings window. Every row edits one key of config.toml in place (comments and
//! layout kept); the launcher picks the change up through its usual reload.

use std::collections::HashMap;

use gpui::{
    AnyElement, Context, Entity, FocusHandle, Focusable, FontWeight, KeyDownEvent, SharedString, Subscription, Window, div,
    prelude::*,
};
use gpui_ce_elements::editable_text::{EditableTextState, StringStorage, TextChanged, text_input};

use crate::{Dismiss, Launch, NextSection, PrevSection, SelectNext, SelectPrev, config, design::px, ext, search::Item};

#[derive(Clone, PartialEq)]
enum Kind {
    Text,
    Number,
    Bool,
    Hotkey,
    Choice(&'static [&'static str]),
    /// A list of strings: removable chips plus an Add field.
    List,
    /// A list of launcher item keys, shown by item name; entries can only be removed here.
    Keys,
    /// Opens a file.
    Open(&'static str),
    /// Shortcuts: which kind of item the table lists (not saved).
    Category,
    /// Shortcuts: narrows the table by title (not saved).
    Filter,
    /// Shortcuts: one item (index into `Settings::items`); its input edits the alias.
    Command(usize),
}

const CATEGORIES: &[&str] = &["All", "Commands", "Applications", "Extensions", "Quicklinks", "Snippets", "Scripts"];

/// The Shortcuts category an item belongs to, from its key prefix.
fn category(it: &Item) -> &'static str {
    match it.key.as_deref().and_then(|k| k.split_once(':')).map(|(p, _)| p) {
        Some("cmd") => "Commands",
        Some("ext") => "Extensions",
        Some("ql") => "Quicklinks",
        Some("sn") => "Snippets",
        Some("sc") => "Scripts",
        _ => "Applications",
    }
}

fn key_of(it: &Item) -> String {
    it.key.clone().unwrap_or_default()
}

struct Row {
    label: String,
    help: String,
    path: Vec<String>,
    kind: Kind,
    /// Heading of the card the row sits in; consecutive rows with the same group share one.
    group: &'static str,
}

const SECTIONS: [&str; 8] = ["General", "Launcher", "Shortcuts", "Clipboard", "Files", "Theme", "Extensions", "Advanced"];

fn row(label: &str, path: &str, kind: Kind, help: &str) -> Row {
    Row { label: label.into(), help: help.into(), path: path.split('.').map(String::from).collect(), kind, group: "" }
}

/// Rows under card headings.
fn groups(list: Vec<(&'static str, Vec<Row>)>) -> Vec<Row> {
    list.into_iter().flat_map(|(g, rows)| rows.into_iter().map(move |r| Row { group: g, ..r })).collect()
}

/// How a choice value reads on screen.
fn choice_label(v: &str) -> String {
    match v {
        "cursor" => "Screen with the mouse".into(),
        "active" => "Screen with the focused window".into(),
        "primary" => "Main screen".into(),
        "back" => "Go back one level".into(),
        "close" => "Close and return to root".into(),
        "default" => "Arrows only".into(),
        "emacs" => "Emacs  (Alt+N / P / B)".into(),
        "vim" => "Vim  (Alt+J / K / H)".into(),
        "system" => "Follow Windows".into(),
        "expanded" => "Full list".into(),
        "compact" => "Search bar only".into(),
        s if s.ends_with('s') && s[..s.len() - 1].parse::<u32>().is_ok() => format!("{} s", &s[..s.len() - 1]),
        s => {
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

fn rows(section: usize, exts: &[ext::Extension]) -> Vec<Row> {
    match SECTIONS[section] {
        "General" => groups(vec![
            (
                "Startup",
                vec![
                    row("Open at login", "start_at_login", Kind::Bool, "Start akscast in the background when you sign in to Windows."),
                    row("Show in system tray", "tray", Kind::Bool, "Keep the akscast icon in the notification area, with Open, Settings and Quit."),
                ],
            ),
            (
                "Hotkeys",
                vec![
                    row("Open akscast", "hotkey", Kind::Hotkey, "Press anywhere to show or hide the launcher. Click, then press the new keys."),
                    row("Emoji picker", "emoji.hotkey", Kind::Hotkey, "Opens emoji search straight away. Other commands get hotkeys under Shortcuts."),
                ],
            ),
            (
                "Appearance",
                vec![
                    row("Colour mode", "theme.mode", Kind::Choice(&["system", "dark", "light"]), "Follow Windows switches with your system's light/dark setting."),
                    row("Dark theme", "theme.dark", Kind::Choice(crate::design::DARK_THEMES), "Used in dark mode. Fine-tune colours under Theme."),
                    row("Light theme", "theme.light", Kind::Choice(crate::design::LIGHT_THEMES), "Used in light mode."),
                    row("Interface size", "size", Kind::Choice(&["small", "default", "large"]), "Scales text, rows and the window together."),
                ],
            ),
            (
                "Window",
                vec![
                    row("Window style", "window_mode", Kind::Choice(&["expanded", "compact"]), "Search bar only keeps the window small until you type or press Down."),
                    row("Favourites under the search bar", "launcher.compact_favorites", Kind::Bool, "With the search bar only style, show your favourites as a row of icons."),
                    row("Width", "width", Kind::Number, "A share of the screen, e.g. 0.5 for half: looks the same on every monitor. Numbers above 1 are pixels."),
                ],
            ),
        ]),
        "Launcher" => groups(vec![
            (
                "Opening",
                vec![
                    row("Show akscast on", "monitor", Kind::Choice(&["cursor", "active", "primary"]), "Which monitor the window opens on when you have more than one."),
                    row("Keep the last search", "launcher.pop_to_root", Kind::Choice(config::POP_TO_ROOT), "How long akscast remembers where you were after it closes. Immediately always starts fresh."),
                ],
            ),
            (
                "Results",
                vec![
                    row("Match strictness", "launcher.sensitivity", Kind::Choice(&["low", "medium", "high"]), "Low: every word must appear as typed. Medium: drops weak fuzzy matches. High: any fuzzy match."),
                    row("Rows shown", "max_results", Kind::Number, "How many results root search lists at most, e.g. 8."),
                    row("Include files", "launcher.files", Kind::Bool, "Also list matching files below apps and commands."),
                    row("Calculator", "search.calculator", Kind::Bool, "Answer maths, unit and currency conversions as you type (e.g. 5 km in miles)."),
                ],
            ),
            (
                "Keyboard",
                vec![
                    row("Search history", "launcher.history", Kind::Bool, "Up arrow in an empty search bar brings back earlier searches."),
                    row("Escape key", "launcher.escape", Kind::Choice(&["back", "close"]), "What Escape does inside a command such as Clipboard History."),
                    row("Extra navigation keys", "launcher.navigation", Kind::Choice(&["default", "emacs", "vim"]), "Move through the list without the arrow keys."),
                    row("Game mode", "launcher.game_mode", Kind::Bool, "Hotkeys do nothing while a full-screen app or game has focus."),
                ],
            ),
            (
                "Root search items",
                vec![
                    row("Favourites", "launcher.favorites", Kind::Keys, "Pinned to the top of root search. Add one in akscast with Ctrl+K → Add to Favorites."),
                    row("Hidden", "launcher.hidden", Kind::Keys, "Left out of results; aliases and hotkeys still work. Hide with Ctrl+K → Hide from Root Search."),
                ],
            ),
            (
                "When nothing matches",
                vec![
                    row("Fallback commands", "search.fallbacks", Kind::List, "Offered below every search, in this order. \"web\" or the name of a quicklink with {query}."),
                    row("Web search URL", "search.fallback", Kind::Text, "Used by the \"web\" fallback; {query} becomes what you typed."),
                ],
            ),
        ]),
        "Clipboard" => groups(vec![(
            "Clipboard history",
            vec![
                row("Keep clipboard history", "clipboard.enabled", Kind::Bool, "Remember text you copy. Stored encrypted, readable only by your Windows account."),
                row("Entries kept", "clipboard.history", Kind::Number, "The oldest entries are dropped beyond this many, e.g. 500."),
                row("Hotkey", "clipboard.hotkey", Kind::Hotkey, "Opens clipboard history straight away."),
                row("Never record from", "clipboard.ignore_apps", Kind::List, "Program file names, e.g. keepass.exe. Copies made in them are not saved."),
            ],
        )]),
        "Files" => groups(vec![
            (
                "File search",
                vec![
                    row("File search", "files.enabled", Kind::Bool, "Find files and folders by name with Search Files."),
                    row("Hotkey", "files.hotkey", Kind::Hotkey, "Opens file search straight away."),
                ],
            ),
            (
                "Folders",
                vec![
                    row("Folders to search", "files.roots", Kind::List, "Full paths, e.g. D:\\Projects. With none, your user folder is searched."),
                    row("Skip names", "files.exclude", Kind::List, "File or folder names skipped wherever they appear, e.g. node_modules."),
                ],
            ),
            (
                "Whole-drive search",
                vec![
                    row("Search whole drives", "files.ntfs", Kind::Bool, "Every NTFS drive, kept up to date live. Run \"Enable Fast File Search\" once first (asks for admin)."),
                    row("Drives", "files.drives", Kind::List, "Letters to index, e.g. C and D. With none, every NTFS drive."),
                    row("Skip folders", "files.skip", Kind::List, "Full paths left out with everything under them; also saves memory."),
                ],
            ),
        ]),
        "Theme" => {
            let mut colours = vec![];
            for (label, key) in [
                ("Background", "background"),
                ("Text", "text"),
                ("Muted text", "muted"),
                ("Selection", "selected"),
                ("Selected text", "selected_text"),
                ("Border", "border"),
                ("Separator", "separator"),
                ("Panels", "container"),
            ] {
                colours.push(row(label, &format!("theme.{key}"), Kind::Text, "A colour like #1e1e2e. Leave empty to use the theme's own."));
            }
            groups(vec![("Mode", vec![row("Colour mode", "theme.mode", Kind::Choice(&["system", "dark", "light"]), "Follow Windows switches with your system setting.")]), ("Colours", colours)])
        }
        "Extensions" => {
            let mut r = groups(vec![(
                "Extensions",
                vec![
                    row("Background commands", "extensions.background", Kind::Bool, "Let commands that refresh on a schedule (e.g. every 5 minutes) run while akscast is closed."),
                    row("Tray icons", "extensions.menu_bar", Kind::Bool, "Show menu-bar commands as icons in the notification area."),
                    row("Node.js", "extensions.node", Kind::Text, "The node program that runs extensions; \"node\" uses the one on your PATH. Needs version 22 or later."),
                    row("Memory per command (MB)", "extensions.memory_mb", Kind::Number, "A command using more than this is stopped, e.g. 256."),
                ],
            )]);
            for x in exts {
                let mut seen = vec![];
                for p in x.prefs.iter().chain(x.commands.iter().flat_map(|c| c.prefs.iter())) {
                    if seen.contains(&p.name) {
                        continue;
                    }
                    seen.push(p.name.clone());
                    let title = if p.title.is_empty() { &p.name } else { &p.title };
                    let kind = if p.kind == "checkbox" { Kind::Bool } else { Kind::Text };
                    let mut help = if p.description.is_empty() { String::new() } else { format!("{} ", p.description) };
                    if p.required {
                        help.push_str("Required.");
                    }
                    r.push(Row {
                        label: format!("{} › {title}", x.title),
                        help,
                        path: vec!["extension".into(), x.name.clone(), p.name.clone()],
                        kind,
                        group: "Extension preferences",
                    });
                }
            }
            r
        }
        "Shortcuts" => vec![],
        _ => vec![
            Row { label: "Open config.toml".into(), help: "Every setting in one text file, with comments. Saved changes apply within 2 seconds.".into(), path: vec![], kind: Kind::Open("config.toml"), group: "Files" },
            Row { label: "Open extensions log".into(), help: "Errors and console output from extensions, plus file index status.".into(), path: vec![], kind: Kind::Open("extensions.log"), group: "Files" },
        ],
    }
}

/// A GPUI keystroke -> the hotkey syntax config.toml uses ("Ctrl+Alt+K").
fn hotkey_from(ev: &KeyDownEvent) -> Option<String> {
    let k = &ev.keystroke;
    let key = match k.key.as_str() {
        "control" | "shift" | "alt" | "platform" | "function" => return None,
        "space" => "Space".to_string(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        "capslock" => "CapsLock".into(),
        s if s.len() == 1 => s.to_uppercase(),
        s => {
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    };
    let m = &k.modifiers;
    let parts: Vec<&str> = [(m.control, "Ctrl"), (m.alt, "Alt"), (m.shift, "Shift"), (m.platform, "Win")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect();
    Some(parts.into_iter().chain([key.as_str()]).collect::<Vec<_>>().join("+"))
}

pub struct Settings {
    text: String,
    exts: Vec<ext::Extension>,
    /// Every launcher item, for the Shortcuts table.
    items: Vec<Item>,
    category: usize,
    filter: String,
    /// `[launcher]` as last saved: the Shortcuts table reads hotkeys and on/off from here
    /// instead of re-parsing the file for every row.
    launcher: config::Launcher,
    section: usize,
    selected: usize,
    inputs: HashMap<String, Entity<EditableTextState>>,
    recording: bool,
    status: Option<String>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Settings {
    pub fn new(items: Vec<Item>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            text: std::fs::read_to_string(config::path()).unwrap_or_default(),
            launcher: config::load().map(|c| c.launcher).unwrap_or_default(),
            exts: ext::scan(),
            items,
            category: 0,
            filter: String::new(),
            // AKSCAST_OPEN_SETTINGS=<page> opens on that page (screenshots and checks).
            section: std::env::var("AKSCAST_OPEN_SETTINGS").ok().and_then(|s| SECTIONS.iter().position(|x| *x == s)).unwrap_or(0),
            selected: 0,
            inputs: HashMap::new(),
            recording: false,
            status: None,
            focus: cx.focus_handle(),
            _subs: vec![],
        };
        this.build_inputs(cx);
        this.focus_row(window, cx);
        this
    }

    /// Fresh launcher items when the window is brought back; rows keep their indices
    /// until the section is next entered.
    pub fn set_items(&mut self, items: Vec<Item>) {
        if SECTIONS[self.section] != "Shortcuts" {
            self.items = items;
        }
    }

    fn rows(&self) -> Vec<Row> {
        if SECTIONS[self.section] == "Shortcuts" {
            return self.shortcut_rows();
        }
        rows(self.section, &self.exts)
    }

    /// The Shortcuts table: category and filter, then every matching item.
    fn shortcut_rows(&self) -> Vec<Row> {
        let mut out = vec![
            Row { label: "Show".into(), help: "Click a kind, or press Enter for the next one.".into(), path: vec![], kind: Kind::Category, group: "" },
            Row { label: "Find".into(), help: "Type part of a name or alias.".into(), path: vec!["shortcuts-filter".into()], kind: Kind::Filter, group: "" },
        ];
        let want = CATEGORIES[self.category];
        let words: Vec<String> = self.filter.split_whitespace().map(str::to_lowercase).collect();
        for (i, it) in self.items.iter().enumerate() {
            let hay = format!("{} {} {}", it.title, it.subtitle, it.alias).to_lowercase();
            if (want == "All" || category(it) == want) && words.iter().all(|w| hay.contains(w.as_str())) {
                let help = [category(it).trim_end_matches('s'), &it.subtitle].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                out.push(Row { label: it.title.clone(), help, path: vec!["launcher".into(), "aliases".into(), key_of(it)], kind: Kind::Command(i), group: "Alias · Hotkey · On" });
            }
        }
        out
    }

    fn build_inputs(&mut self, cx: &mut Context<Self>) {
        self.inputs.clear();
        self._subs.clear();
        for r in self.rows() {
            if matches!(r.kind, Kind::Text | Kind::Number | Kind::List | Kind::Command(_)) {
                let value = match r.kind {
                    Kind::Command(i) => self.launcher.aliases.get(&key_of(&self.items[i])).cloned().unwrap_or_default(),
                    // A list's input is its Add field.
                    Kind::List => String::new(),
                    _ => config::shown_value(&self.text, &r.path),
                };
                let input = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
                input.update(cx, |s, cx| s.emplace(&value, cx));
                self.inputs.insert(r.path.join("."), input);
            }
            if r.kind == Kind::Filter {
                let input = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
                input.update(cx, |s, cx| s.emplace(&self.filter, cx));
                self._subs.push(cx.subscribe(&input, |this, input, _: &TextChanged, cx| {
                    this.filter = input.read(cx).as_str().to_string();
                    cx.notify();
                }));
                self.inputs.insert(r.path.join("."), input);
            }
        }
    }

    fn focus_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.rows();
        let handle = rows.get(self.selected).and_then(|r| self.inputs.get(&r.path.join("."))).map(|i| i.focus_handle(cx)).unwrap_or_else(|| self.focus.clone());
        window.focus(&handle, cx);
        cx.notify();
    }

    fn save(&mut self, path: &[String], value: toml_edit::Value, cx: &mut Context<Self>) {
        let current = std::fs::read_to_string(config::path()).unwrap_or_else(|_| self.text.clone());
        match config::write_value(&current, path, value).and_then(|t| config::parse(&t).map(|_| t)) {
            Ok(text) => match std::fs::write(config::path(), &text) {
                Ok(()) => {
                    self.launcher = config::parse(&text).map(|c| c.launcher).unwrap_or_default();
                    self.text = text;
                    self.status = Some(format!("Saved {}", path.join(".")));
                }
                Err(e) => self.status = Some(format!("Could not write config.toml: {e}")),
            },
            Err(e) => self.status = Some(e),
        }
        cx.notify();
    }

    /// Saves the selected text-like row if its input differs from the file.
    fn commit(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows();
        let Some(r) = rows.get(self.selected).filter(|r| r.kind != Kind::Filter) else { return };
        let Some(input) = self.inputs.get(&r.path.join(".")).cloned() else { return };
        let typed = input.read(cx).as_str().trim().to_string();
        if r.kind == Kind::List {
            // The Add field: append what was typed ("a; b" adds both).
            if typed.is_empty() {
                return;
            }
            let mut list = self.list(&r.path);
            for part in typed.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                if !list.iter().any(|l| l == part) {
                    list.push(part.to_string());
                }
            }
            input.update(cx, |s, cx| s.emplace("", cx));
            return self.save_list(&r.path, list, cx);
        }
        if typed == config::shown_value(&self.text, &r.path) {
            return;
        }
        let value: toml_edit::Value = match r.kind {
            Kind::Number => match typed.parse::<f64>() {
                // Whole numbers stay integers in the file; fractions (e.g. width 0.5) are floats.
                Ok(n) if n > 0. && n.fract() == 0. => (n as i64).into(),
                Ok(n) if n > 0. => n.into(),
                _ => {
                    self.status = Some(format!("{} must be a positive number", r.label));
                    return cx.notify();
                }
            },
            _ => typed.into(),
        };
        let path = r.path.clone();
        self.save(&path, value, cx);
    }

    /// A list setting's entries (the default when the file doesn't set it).
    fn list(&self, path: &[String]) -> Vec<String> {
        config::shown_value(&self.text, path).split("; ").filter(|s| !s.is_empty()).map(String::from).collect()
    }

    fn save_list(&mut self, path: &[String], list: Vec<String>, cx: &mut Context<Self>) {
        self.save(path, list.iter().map(String::as_str).collect::<toml_edit::Array>().into(), cx);
    }

    /// The × on a list chip.
    fn remove_entry(&mut self, path: Vec<String>, entry: String, cx: &mut Context<Self>) {
        let mut list = self.list(&path);
        list.retain(|e| *e != entry);
        self.save_list(&path, list, cx);
    }

    /// A list chip's ← arrow: one place earlier (fallback order matters).
    fn move_entry_up(&mut self, path: Vec<String>, entry: String, cx: &mut Context<Self>) {
        let mut list = self.list(&path);
        if let Some(i) = list.iter().position(|e| *e == entry).filter(|i| *i > 0) {
            list.swap(i, i - 1);
            self.save_list(&path, list, cx);
        }
    }

    /// The name a list entry shows: an item key becomes the item's title.
    fn entry_label(&self, kind: &Kind, entry: &str) -> String {
        match kind {
            Kind::Keys => self.items.iter().find(|it| it.key.as_deref() == Some(entry)).map_or_else(|| entry.to_string(), |it| it.title.clone()),
            _ => entry.to_string(),
        }
    }

    fn select(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        self.recording = false;
        self.selected = i.min(self.rows().len().saturating_sub(1));
        self.focus_row(window, cx);
    }

    fn set_section(&mut self, s: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        self.recording = false;
        self.section = s % SECTIONS.len();
        self.selected = 0;
        // Inputs are built for every row the table can show.
        self.filter.clear();
        self.category = 0;
        self.build_inputs(cx);
        self.focus_row(window, cx);
    }

    /// Enter / click on a row.
    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.rows();
        let Some(r) = rows.get(self.selected) else { return };
        let current = config::shown_value(&self.text, &r.path);
        match &r.kind {
            Kind::Bool => {
                let on = current == "true";
                self.save(&r.path, (!on).into(), cx);
            }
            Kind::Choice(opts) => {
                let i = opts.iter().position(|o| *o == current).map_or(0, |i| (i + 1) % opts.len());
                self.save(&r.path, opts[i].into(), cx);
            }
            Kind::Hotkey | Kind::Command(_) => {
                self.commit(cx);
                self.recording = true;
                self.status = Some("Press the new shortcut (Backspace removes it, Esc cancels)".into());
                window.focus(&self.focus, cx);
                cx.notify();
            }
            Kind::Open(file) => crate::win::open(&config::dir().join(file).to_string_lossy()),
            Kind::Text | Kind::Number | Kind::List => self.commit(cx),
            Kind::Category => {
                self.category = (self.category + 1) % CATEGORIES.len();
                cx.notify();
            }
            Kind::Filter | Kind::Keys => {}
        }
    }

    /// Where a row's hotkey lives: its own key, or the item's entry under `[launcher.hotkeys]`.
    fn hotkey_path(&self, r: &Row) -> Vec<String> {
        match r.kind {
            Kind::Command(i) => vec!["launcher".into(), "hotkeys".into(), key_of(&self.items[i])],
            _ => r.path.clone(),
        }
    }

    /// The other setting already using this hotkey, by name.
    fn hotkey_owner(&self, spec: &str, except: &[String]) -> Option<String> {
        let want = config::parse_hotkey(spec).ok()?;
        let cfg = config::parse(&self.text).ok()?;
        let builtin = [("hotkey", "Open akscast", &cfg.hotkey), ("clipboard.hotkey", "Clipboard History", &cfg.clipboard.hotkey), ("emoji.hotkey", "Emoji", &cfg.emoji.hotkey), ("files.hotkey", "Search Files", &cfg.files.hotkey)];
        let builtin = builtin.into_iter().map(|(p, name, h)| (p.split('.').map(String::from).collect::<Vec<_>>(), name.to_string(), h.clone()));
        let commands = cfg.launcher.hotkeys.iter().map(|(k, h)| {
            let name = self.items.iter().find(|it| it.key.as_deref() == Some(k)).map_or(k.clone(), |it| it.title.clone());
            (vec!["launcher".into(), "hotkeys".into(), k.clone()], name, h.clone())
        });
        builtin.chain(commands).find(|(p, _, h)| p != except && config::parse_hotkey(h).ok() == Some(want)).map(|(_, name, _)| name)
    }

    /// Ctrl+D on a Shortcuts row: switch the item off everywhere, or back on.
    fn toggle_enabled(&mut self, i: usize, cx: &mut Context<Self>) {
        let key = key_of(&self.items[i]);
        let mut list = self.launcher.disabled.clone();
        let off = !list.contains(&key);
        list.retain(|k| *k != key);
        if off {
            list.push(key);
        }
        self.save(&["launcher".into(), "disabled".into()], list.iter().map(String::as_str).collect::<toml_edit::Array>().into(), cx);
        if self.status.as_deref().is_some_and(|s| s.starts_with("Saved")) {
            self.status = Some(format!("Saved: {} {}", self.items[i].title, if off { "switched off" } else { "switched on" }));
        }
    }

    fn is_off(&self, i: usize) -> bool {
        self.launcher.is_disabled(&key_of(&self.items[i]))
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let rows = self.rows();
        let Some(r) = rows.get(self.selected) else { return };
        let k = &ev.keystroke;
        if !self.recording {
            if let Kind::Command(i) = r.kind
                && k.modifiers.control
                && k.key == "d"
            {
                self.toggle_enabled(i, cx);
                cx.stop_propagation();
            }
            return;
        }
        let path = self.hotkey_path(r);
        if matches!(k.key.as_str(), "backspace" | "delete") && !k.modifiers.modified() {
            self.recording = false;
            return self.save(&path, "".into(), cx);
        }
        let Some(spec) = hotkey_from(ev) else { return };
        self.recording = false;
        match config::parse_hotkey(&spec) {
            Ok(_) => match self.hotkey_owner(&spec, &path) {
                Some(owner) => {
                    self.status = Some(format!("{spec} is already used by {owner}"));
                    cx.notify();
                }
                None => self.save(&path, spec.into(), cx),
            },
            Err(e) => {
                self.status = Some(e);
                cx.notify();
            }
        }
    }

    fn control(&self, r: &Row, on: bool, t: &crate::design::Tokens, me: &gpui::WeakEntity<Self>) -> AnyElement {
        if let Some(color) = swatch(r, t) {
            return div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().size(px(22.)).flex_none().rounded(px(5.)).border_1().border_color(t.border).bg(color))
                .child(self.input_control(r, on, t, me))
                .into_any_element();
        }
        self.input_control(r, on, t, me)
    }

    fn text_box(&self, r: &Row, on: bool, t: &crate::design::Tokens) -> AnyElement {
        let Some(input) = self.inputs.get(&r.path.join(".")) else { return div().into_any_element() };
        let width = match r.kind {
            Kind::Number => 120.,
            Kind::List => 240.,
            Kind::Filter => 320.,
            Kind::Command(_) => 110.,
            _ => 440.,
        };
        div()
            .w(px(width))
            .max_w_full()
            .flex_none()
            .child(
                text_input(SharedString::from(format!("set-{}", r.path.join("."))))
                    .state(input.downgrade())
                    .placeholder(match r.kind {
                        Kind::List => "Add… (Enter)",
                        Kind::Filter => "Type to filter",
                        Kind::Command(_) => "Alias",
                        _ => "",
                    })
                    .placeholder_color(t.text_muted)
                    .caret_color(t.accent)
                    .w_full()
                    .whitespace_nowrap()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(if on { t.accent } else { t.border })
                    .bg(t.surface),
            )
            .into_any_element()
    }

    fn input_control(&self, r: &Row, on: bool, t: &crate::design::Tokens, me: &gpui::WeakEntity<Self>) -> AnyElement {
        let value = config::shown_value(&self.text, &r.path);
        let pill = |text: String, active: bool| {
            div()
                .px_3()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(if on || active { t.accent } else { t.border })
                .when(active, |d| d.bg(t.accent).text_color(t.accent_text))
                .child(text)
                .into_any_element()
        };
        match &r.kind {
            Kind::Text | Kind::Number | Kind::Filter | Kind::Command(_) => self.text_box(r, on, t),
            Kind::List | Kind::Keys => {
                let entries = self.list(&r.path);
                let kind = r.kind.clone();
                let ordered = r.path.last().is_some_and(|k| k == "fallbacks");
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .children(entries.iter().enumerate().map(|(n, e)| {
                        let (rm, up) = (me.clone(), me.clone());
                        let (path, path2, e1, e2) = (r.path.clone(), r.path.clone(), e.clone(), e.clone());
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .pl(px(8.))
                            .pr(px(4.))
                            .py(px(2.))
                            .rounded(px(5.))
                            .bg(t.surface_strong)
                            .text_size(px(13.))
                            .child(self.entry_label(&kind, e))
                            .when(ordered && n > 0, |d| {
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("up-{}-{e}", r.path.join("."))))
                                        .px(px(3.))
                                        .rounded(px(3.))
                                        .text_color(t.text_muted)
                                        .hover(|s| s.bg(t.surface).text_color(t.text))
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            drop(up.update(cx, |this, cx| this.move_entry_up(path2.clone(), e2.clone(), cx)))
                                        })
                                        .child("←"),
                                )
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("rm-{}-{e}", r.path.join("."))))
                                    .px(px(4.))
                                    .rounded(px(3.))
                                    .text_color(t.text_muted)
                                    .hover(|s| s.bg(t.surface).text_color(t.danger))
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        drop(rm.update(cx, |this, cx| this.remove_entry(path.clone(), e1.clone(), cx)))
                                    })
                                    .child("×"),
                            )
                    }))
                    .when(entries.is_empty() && r.kind == Kind::Keys, |d| d.child(div().text_size(px(13.)).text_color(t.text_muted).child("None yet")))
                    .when(r.kind == Kind::List, |d| d.child(self.text_box(r, on, t)))
                    .into_any_element()
            }
            Kind::Bool => crate::design::toggle(value == "true", t).into_any_element(),
            Kind::Choice(opts) => {
                let me = me.clone();
                let path = r.path.clone();
                choices(opts, &value, t, &r.path.join("."), move |v, cx| {
                    let path = path.clone();
                    drop(me.update(cx, |this, cx| this.save(&path, v.into(), cx)));
                })
            }
            Kind::Hotkey if self.recording && on => pill("Press keys… (Backspace removes, Esc cancels)".into(), true),
            Kind::Hotkey if value.is_empty() => div().text_size(px(13.)).child(pill("Record Hotkey".into(), false)).into_any_element(),
            Kind::Hotkey => div()
                .flex()
                .items_center()
                .gap(px(4.))
                .children(value.split('+').map(|k| crate::design::key_cap(k, t)))
                .child(div().pl(px(8.)).text_size(px(12.)).text_color(t.text_muted).child("Click to change"))
                .into_any_element(),
            Kind::Open(_) => pill("Open".into(), false),
            Kind::Category => {
                let me = me.clone();
                choices(CATEGORIES, CATEGORIES[self.category], t, "category", move |v, cx| {
                    drop(me.update(cx, |this, cx| {
                        this.category = CATEGORIES.iter().position(|c| *c == v).unwrap_or(0);
                        cx.notify();
                    }));
                })
            }
        }
    }

    /// A Shortcuts row: alias input, hotkey, on/off switch.
    fn command_control(&self, i: usize, r: &Row, on: bool, t: &crate::design::Tokens, me: gpui::WeakEntity<Self>) -> AnyElement {
        let hotkey = self.launcher.hotkeys.get(&key_of(&self.items[i])).cloned().unwrap_or_default();
        let keys = if self.recording && on {
            div().px_2().py_1().rounded_md().bg(t.accent).text_color(t.accent_text).text_size(px(12.)).child("Press keys…").into_any_element()
        } else if hotkey.is_empty() {
            div().px_2().py_1().rounded_md().border_1().border_color(t.border).text_size(px(12.)).text_color(t.text_muted).child("Record Hotkey").into_any_element()
        } else {
            div().flex().gap(px(4.)).children(hotkey.split('+').map(|k| crate::design::key_cap(k, t))).into_any_element()
        };
        let (me2, me3) = (me.clone(), me);
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(self.text_box(r, on, t))
            .child(
                div()
                    .id(("hotkey", i))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        drop(me2.update(cx, |this, cx| {
                            if let Some(row) = this.rows().iter().position(|r| r.kind == Kind::Command(i)) {
                                this.select(row, window, cx);
                                this.activate(window, cx);
                            }
                        }))
                    })
                    .child(keys),
            )
            .child(
                div()
                    .id(("enabled", i))
                    .flex_none()
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        drop(me3.update(cx, |this, cx| this.toggle_enabled(i, cx)))
                    })
                    .child(crate::design::toggle(!self.is_off(i), t)),
            )
            .into_any_element()
    }
}

impl Settings {
    /// Rows under their headings, each group in one rounded card.
    fn cards(&self, rows: &[Row], t: &crate::design::Tokens, me: &gpui::WeakEntity<Self>) -> Vec<AnyElement> {
        let mut out = vec![];
        let mut i = 0;
        while i < rows.len() {
            let (group, start) = (rows[i].group, i);
            while i < rows.len() && rows[i].group == group {
                i += 1;
            }
            out.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .pb(px(14.))
                    .when(!group.is_empty(), |d| d.child(div().px(px(4.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_muted).child(group)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(t.hairline)
                            .bg(t.bg_elevated)
                            .overflow_hidden()
                            .children((start..i).map(|j| self.row_view(j, &rows[j], j > start, t, me))),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    fn row_view(&self, i: usize, r: &Row, divider: bool, t: &crate::design::Tokens, me: &gpui::WeakEntity<Self>) -> AnyElement {
        let on = i == self.selected;
        let click = me.clone();
        div()
            .id(("row", i))
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py(px(10.))
            .relative()
            .when(divider, |d| d.border_t_1().border_color(t.hairline))
            .when(on, |d| d.bg(t.surface).child(div().absolute().left(px(0.)).top(px(10.)).bottom(px(10.)).w(px(3.)).rounded_full().bg(t.accent)))
            .on_click(move |_, window, cx| {
                drop(click.update(cx, |this, cx| {
                    let again = this.selected == i;
                    this.select(i, window, cx);
                    let typed = matches!(this.rows()[i].kind, Kind::Text | Kind::Number | Kind::List | Kind::Keys | Kind::Filter | Kind::Command(_) | Kind::Choice(_) | Kind::Category);
                    if again && matches!(this.rows()[i].kind, Kind::Command(_)) || !typed {
                        this.activate(window, cx);
                    }
                }))
            })
            .when(matches!(r.kind, Kind::Command(c) if self.is_off(c)), |d| d.text_color(t.text_muted))
            .children(match r.kind {
                Kind::Command(c) => Some(crate::design::icon_view(&self.items[c].icon, t, on)),
                _ => None,
            })
            .child(
                div()
                    .w(px(300.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().child(r.label.clone()))
                    .children((!r.help.is_empty()).then(|| div().text_size(px(12.)).line_height(px(17.)).text_color(t.text_muted).child(r.help.clone()))),
            )
            .child(div().flex_1().min_w_0().flex().items_center().child(match r.kind {
                Kind::Command(c) => self.command_control(c, r, on, t, me.clone()),
                _ => self.control(r, on, t, me),
            }))
            .into_any_element()
    }
}

impl Focusable for Settings {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = config::parse(&self.text).map(|c| c.theme).unwrap_or_default();
        let t = crate::design::Tokens::from_theme(&theme, window);
        crate::design::scale_settings();
        window.set_rem_size(px(16.));
        let rows = self.rows();
        let me = cx.weak_entity();
        crate::dump_ui(
            "akscast-settings.json",
            serde_json::json!({
                "section": SECTIONS[self.section],
                "selected": self.selected,
                "rows": rows.iter().map(|r| &r.label).collect::<Vec<_>>(),
                "recording": self.recording,
                "status": self.status,
            }),
        );
        div()
            .size_full()
            .flex()
            .font_family(crate::design::FONT_UI)
            .text_size(px(14.))
            .bg(t.bg)
            .text_color(t.text)
            .key_context("Settings")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| this.select(this.selected + 1, window, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| this.select(this.selected.saturating_sub(1), window, cx)))
            .on_action(cx.listener(|this, _: &NextSection, window, cx| this.set_section(this.section + 1, window, cx)))
            .on_action(cx.listener(|this, _: &PrevSection, window, cx| this.set_section(this.section + SECTIONS.len() - 1, window, cx)))
            .on_action(cx.listener(|this, _: &Launch, window, cx| this.activate(window, cx)))
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                if this.recording {
                    this.recording = false;
                    this.status = Some("Cancelled".into());
                    cx.notify();
                } else {
                    this.commit(cx);
                    window.remove_window();
                }
            }))
            .child(
                div()
                    .w(px(190.))
                    .flex_none()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .bg(t.surface)
                    .border_r_1()
                    .border_color(t.hairline)
                    .child(div().px_2().pb_3().flex().items_center().gap(px(8.)).child(crate::design::fluent(crate::design::g::SETTING, 16., t.accent)).child(div().font_weight(FontWeight::SEMIBOLD).child("akscast")))
                    .children(SECTIONS.iter().enumerate().map(|(i, name)| {
                        let me = me.clone();
                        div()
                            .id(("section", i))
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .relative()
                            .when(i == self.section, |d| d.bg(t.surface_strong).child(div().absolute().left(px(0.)).top(px(8.)).w(px(3.)).h(px(16.)).rounded_full().bg(t.accent)))
                            .when(i != self.section, |d| d.text_color(t.text_muted).hover(|s| s.bg(t.surface)))
                            .on_click(move |_, window, cx| drop(me.update(cx, |this, cx| this.set_section(i, window, cx))))
                            .child(*name)
                    }))
                    .child(div().flex_1())
                    .child(div().px_2().text_xs().text_color(t.text_muted).child(if SECTIONS[self.section] == "Shortcuts" { "Type an alias · Enter record hotkey · Ctrl+D switch on/off" } else { "Up/Down rows · Ctrl+Up/Down pages · Enter change · Esc close" })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("rows")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().font_family(crate::design::FONT_DISPLAY).text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).pb_3().child(SECTIONS[self.section]))
                            .children((SECTIONS[self.section] == "Theme").then(|| preview(&t)))
                            .children(self.cards(&rows, &t, &me))
                            .children(rows.is_empty().then(|| div().text_color(t.text_muted).child("Nothing to configure here yet."))),
                    )
                    .child(
                        div()
                            .h(px(34.))
                            .flex_none()
                            .px_4()
                            .flex()
                            .items_center()
                            .border_t_1()
                            .border_color(t.hairline)
                            .text_xs()
                            .text_color(match self.status.as_deref() {
                                Some(s) if !(s.starts_with("Saved") || s.starts_with("Press") || s == "Cancelled") => t.danger,
                                _ => t.text_muted,
                            })
                            .child(self.status.clone().unwrap_or_else(|| format!("Changes save to {} and apply within 2 s", config::path().display()))),
                    ),
            )
    }
}

/// A segmented control with the current option highlighted; clicking an option picks it.
fn choices(opts: &[&'static str], current: &str, t: &crate::design::Tokens, id: &str, pick: impl Fn(&'static str, &mut gpui::App) + Clone + 'static) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(6.))
        .bg(t.surface)
        .children(opts.iter().map(|o| {
            let cur = *o == current;
            let pick = pick.clone();
            let o: &'static str = o;
            div()
                .id(SharedString::from(format!("{id}={o}")))
                .px(px(10.))
                .py(px(3.))
                .rounded(px(4.))
                .text_size(px(13.))
                .when(cur, |d| d.bg(t.accent).text_color(t.accent_text))
                .when(!cur, |d| d.text_color(t.text_muted).hover(|s| s.bg(t.surface_strong).text_color(t.text)))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    pick(o, cx)
                })
                .child(choice_label(o))
        }))
        .into_any_element()
}

/// Theme rows show the colour in use: the override, or the palette's own.
fn swatch(r: &Row, t: &crate::design::Tokens) -> Option<gpui::Hsla> {
    let [section, key] = r.path.as_slice() else { return None };
    if section != "theme" {
        return None;
    }
    Some(match key.as_str() {
        "background" => t.bg,
        "text" => t.text,
        "muted" => t.text_muted,
        "selected" => t.accent,
        "selected_text" => t.accent_text,
        "border" => t.border,
        "separator" => t.hairline,
        "container" => t.bg_elevated,
        _ => return None,
    })
}

/// Theme studio: a small launcher drawn with the current theme, updated as you edit.
fn preview(t: &crate::design::Tokens) -> impl IntoElement {
    use crate::design::{FONT_UI, fluent, g, key_cap};
    let row = |glyph: char, title: &str, sub: &str, acc: &str, on: bool| {
        div()
            .h(px(36.))
            .mx(px(6.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(6.))
            .relative()
            .when(on, |d| d.bg(t.surface_strong).child(div().absolute().left(px(0.)).top(px(10.)).w(px(3.)).h(px(16.)).rounded_full().bg(t.accent)))
            .child(div().size(px(24.)).flex().items_center().justify_center().rounded(px(6.)).bg(t.surface).child(fluent(glyph, 13., t.text_muted)))
            .child(div().child(title.to_string()))
            .child(div().text_color(t.text_muted).child(sub.to_string()))
            .child(div().flex_1())
            .child(div().text_size(px(12.)).text_color(t.text_faint).child(acc.to_string()))
    };
    div()
        .mb(px(14.))
        .rounded(px(10.))
        .border_1()
        .border_color(t.border)
        .bg(t.bg)
        .font_family(FONT_UI)
        .text_size(px(13.))
        .text_color(t.text)
        .overflow_hidden()
        .child(
            div()
                .h(px(44.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(10.))
                .border_b_1()
                .border_color(t.hairline)
                .child(fluent(g::SEARCH, 15., t.accent))
                .child(div().text_color(t.text_muted).child("Search apps and commands…")),
        )
        .child(div().px(px(14.)).pt(px(8.)).pb(px(4.)).text_size(px(11.5)).text_color(t.text_faint).child("Results"))
        .child(row(g::FOLDER, "Search Files", "akscast", "Command", true))
        .child(row(g::PASTE, "Clipboard History", "akscast", "Command", false))
        .child(row(g::CALCULATOR, "42", "6 × 7 =", "Calculator", false))
        .child(
            div()
                .h(px(36.))
                .mt(px(6.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(6.))
                .border_t_1()
                .border_color(t.hairline)
                .bg(t.bg_elevated)
                .child(div().size(px(8.)).rounded_full().bg(t.success))
                .child(div().flex_1().text_color(t.text_muted).child("Preview"))
                .child(div().text_color(t.text).child("Open"))
                .child(key_cap("↵", t)),
        )
}
