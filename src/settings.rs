//! The settings window. Every row edits one key of config.toml in place (comments and
//! layout kept); the launcher picks the change up through its usual reload.

use std::collections::HashMap;

use gpui::{
    AnyElement, Context, Entity, FocusHandle, Focusable, FontWeight, KeyDownEvent, SharedString, Subscription, Window, div,
    prelude::*,
};
use gpui_ce_elements::editable_text::{EditableTextState, StringStorage, TextChanged, text_input};

use crate::{
    Dismiss, Launch, NextSection, PrevSection, SelectNext, SelectPrev, config,
    design::{self, px},
    ext,
    search::{Icon, Item},
};

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

const SECTIONS: [&str; 9] = ["General", "Launcher", "Shortcuts", "Apps", "Clipboard", "Files", "Theme", "Extensions", "Advanced"];

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
        "cursor" => "Mouse".into(),
        "active" => "Focused window".into(),
        "primary" => "Main screen".into(),
        "back" => "Go back".into(),
        "close" => "Close".into(),
        "default" => "Arrows".into(),
        "immediately" => "Now".into(),
        "system" => "Follow Windows".into(),
        "expanded" => "Full list".into(),
        "compact" => "Search bar only".into(),
        "off" => "Off".into(),
        "capslock" => "Caps Lock".into(),
        "rightalt" => "Right Alt".into(),
        "rightctrl" => "Right Ctrl".into(),
        "scrolllock" => "Scroll Lock".into(),
        "launch" => "New window".into(),
        "focus" => "Switch to it".into(),
        "toggle" => "Switch or hide".into(),
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
                    row("Open at login", "start_at_login", Kind::Bool, "Start procast in the background when you sign in to Windows."),
                    row("Show in system tray", "tray", Kind::Bool, "Keep the procast icon in the notification area, with Open, Settings and Quit."),
                ],
            ),
            (
                "Hotkeys",
                vec![
                    row("Open procast", "hotkey", Kind::Hotkey, "Press anywhere to show or hide the launcher. Click, then press the new keys."),
                    row("Emoji picker", "emoji.hotkey", Kind::Hotkey, "Opens emoji search straight away. Other commands get hotkeys under Shortcuts."),
                    row("Double-tap to open", "keyboard.double_tap", Kind::Choice(crate::keys::TAP_KEYS), "Press this key twice quickly to open procast, besides the hotkey above."),
                    row("Hyper key", "keyboard.hyper", Kind::Choice(crate::keys::HYPER_KEYS), "Hold this key for Ctrl+Alt+Shift+Win, so Hyper+K style hotkeys never clash with apps. Tapped alone it does nothing."),
                ],
            ),
            (
                "Appearance",
                vec![
                    row("Colour mode", "theme.mode", Kind::Choice(&["system", "dark", "light"]), "Follow Windows switches with your system's light/dark setting."),
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
                    row("Show procast on", "monitor", Kind::Choice(&["cursor", "active", "primary"]), "Which monitor the window opens on: the one with the mouse, the one with the window you're using, or the main one."),
                    row("Keep the last search", "launcher.pop_to_root", Kind::Choice(config::POP_TO_ROOT), "How long procast remembers where you were after it closes. Now always starts fresh."),
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
                    row("Escape key", "launcher.escape", Kind::Choice(&["back", "close"]), "Inside a command such as Clipboard History: go back one level, or close the window and start fresh next time."),
                    row("Extra navigation keys", "launcher.navigation", Kind::Choice(&["default", "emacs", "vim"]), "Move without the arrow keys. Emacs: Alt+N/P move, Alt+B back. Vim: Alt+J/K move, Alt+H back."),
                    row("Game mode", "launcher.game_mode", Kind::Bool, "Hotkeys do nothing while a full-screen app or game has focus."),
                ],
            ),
            (
                "Root search items",
                vec![
                    row("Favourites", "launcher.favorites", Kind::Keys, "Pinned to the top of root search. Add one in procast with Ctrl+K → Add to Favorites."),
                    row("Hidden", "launcher.hidden", Kind::Keys, "Left out of results; aliases and hotkeys still work. Hide with Ctrl+K → Hide from Root Search."),
                ],
            ),
            (
                "When nothing matches",
                vec![
                    row("Fallback commands", "search.fallbacks", Kind::List, "Offered below every search, in this order. \"web\", \"chatgpt\", \"ai\", or the name of a quicklink with {query}."),
                    row("Web search URL", "search.fallback", Kind::Text, "Used by the \"web\" fallback; {query} becomes what you typed."),
                    row("Ask AI model", "ai.model", Kind::Choice(&["haiku", "sonnet", "opus"]), "Used by the \"ai\" fallback, on your Claude Code login. Haiku answers fastest."),
                ],
            ),
        ]),
        "Apps" => groups(vec![
            (
                "Hotkeys",
                vec![row(
                    "An app's hotkey",
                    "apps.hotkey_action",
                    Kind::Choice(&["launch", "focus", "toggle"]),
                    "When the app is already open, its hotkey (set under Shortcuts) can open a new window, switch to the open one, or switch to it and hide it again if it's already in front.",
                )],
            ),
            (
                "More apps",
                vec![
                    row("Extra folders", "apps.folders", Kind::List, "Programs and shortcuts in these folders are listed as apps, e.g. D:\\Portable."),
                    row("Include subfolders", "apps.deep", Kind::Bool, "Also look up to four folders deep."),
                ],
            ),
            (
                "Quit when unused",
                vec![
                    row("Apps to quit", "apps.auto_quit", Kind::List, "Program file names, e.g. teams.exe. They're asked to close once they haven't been in front for a while."),
                    row("After (minutes)", "apps.auto_quit_minutes", Kind::Number, "How long an app may stay unused, e.g. 60."),
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
            ] {
                colours.push(row(label, &format!("theme.{key}"), Kind::Text, "A colour like #1e1e2e. Leave empty to use the theme's own."));
            }
            groups(vec![("Mode", vec![row("Colour mode", "theme.mode", Kind::Choice(&["system", "dark", "light"]), "Follow Windows switches with your system setting.")]), ("Colours", colours)])
        }
        "Extensions" => {
            let mut r = groups(vec![(
                "Extensions",
                vec![
                    row("Background commands", "extensions.background", Kind::Bool, "Let commands that refresh on a schedule (e.g. every 5 minutes) run while procast is closed."),
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
            Row { label: "Open config.toml".into(), help: format!("Every setting in one text file, with comments: {}. Saved changes apply within 2 seconds.", config::path().display()), path: vec![], kind: Kind::Open("config.toml"), group: "Files" },
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
    if m.control && m.alt && m.shift && m.platform {
        return Some(format!("Hyper+{key}"));
    }
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
    /// The footer message and whether it is an error.
    status: Option<(bool, String)>,
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
            // PROCAST_OPEN_SETTINGS=<page> opens on that page (screenshots and checks).
            section: std::env::var("PROCAST_OPEN_SETTINGS").ok().and_then(|s| SECTIONS.iter().position(|x| *x == s)).unwrap_or(0),
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
                out.push(Row { label: it.title.clone(), help, path: vec!["launcher".into(), "aliases".into(), key_of(it)], kind: Kind::Command(i), group: if self.category == 0 { "Everything" } else { CATEGORIES[self.category] } });
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
                    self.status = Some((false, format!("Saved {}", path.join("."))));
                }
                Err(e) => self.status = Some((true, format!("Could not write config.toml: {e}"))),
            },
            Err(e) => self.status = Some((true, e)),
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
                    self.status = Some((true, format!("{} must be a positive number", r.label)));
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
                self.status = Some((false, "Press the new shortcut (Backspace removes it, Esc cancels)".into()));
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
        let builtin = [("hotkey", "Open procast", &cfg.hotkey), ("clipboard.hotkey", "Clipboard History", &cfg.clipboard.hotkey), ("emoji.hotkey", "Emoji", &cfg.emoji.hotkey), ("files.hotkey", "Search Files", &cfg.files.hotkey)];
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
        if self.status.as_ref().is_some_and(|(error, _)| !error) {
            self.status = Some((false, format!("{} switched {}", self.items[i].title, if off { "off" } else { "on" })));
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
                    self.status = Some((true, format!("{spec} is already used by {owner}")));
                    cx.notify();
                }
                None => self.save(&path, spec.into(), cx),
            },
            Err(e) => {
                self.status = Some((true, e));
                cx.notify();
            }
        }
    }

    fn control(&self, r: &Row, on: bool, t: &design::Tokens, me: &gpui::WeakEntity<Self>) -> AnyElement {
        if let Some(color) = swatch(r, t) {
            return div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().size(px(design::CAP)).flex_none().rounded(px(design::r::XS)).border_1().border_color(t.border).bg(color))
                .child(self.input_control(r, on, t, me))
                .into_any_element();
        }
        self.input_control(r, on, t, me)
    }

    fn text_box(&self, r: &Row, on: bool, t: &design::Tokens) -> AnyElement {
        let Some(input) = self.inputs.get(&r.path.join(".")) else { return div().into_any_element() };
        let width = match r.kind {
            Kind::Number => 120.,
            Kind::List => 240.,
            Kind::Filter => 320.,
            Kind::Command(_) => 110.,
            _ => 440.,
        };
        design::field(on, t)
            .w(px(width))
            .min_w(px(80.))
            .flex_shrink(1.)
            .child(
                text_input(SharedString::from(format!("set-{}", r.path.join("."))))
                    .state(input.downgrade())
                    .placeholder(match r.kind {
                        Kind::List => "Add, then Enter",
                        Kind::Filter => "Type to filter",
                        Kind::Command(_) => "Alias",
                        _ => "",
                    })
                    .placeholder_color(t.text_faint)
                    .caret_color(t.accent)
                    .w_full()
                    .whitespace_nowrap(),
            )
            .into_any_element()
    }

    fn input_control(&self, r: &Row, on: bool, t: &design::Tokens, me: &gpui::WeakEntity<Self>) -> AnyElement {
        let value = config::shown_value(&self.text, &r.path);
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
                    .gap(px(8.))
                    .children(entries.iter().enumerate().map(|(n, e)| {
                        let (rm, up) = (me.clone(), me.clone());
                        let (path, path2, e1, e2) = (r.path.clone(), r.path.clone(), e.clone(), e.clone());
                        div()
                            .flex()
                            .items_center()
                            .h(px(design::CONTROL - 4.))
                            .pl(px(8.))
                            .pr(px(4.))
                            .rounded(px(design::r::SM))
                            .bg(t.surface_strong)
                            .text_size(px(design::ts::BODY_SM))
                            .child(self.entry_label(&kind, e))
                            .when(ordered && n > 0, |d| {
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("up-{}-{e}", r.path.join("."))))
                                        .size(px(design::CAP + 4.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(design::r::XS))
                                        .hover(|s| s.bg(t.surface))
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            drop(up.update(cx, |this, cx| this.move_entry_up(path2.clone(), e2.clone(), cx)))
                                        })
                                        .child(design::icon(design::g::BACK, design::ic::SM, t.text_muted)),
                                )
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("rm-{}-{e}", r.path.join("."))))
                                    .size(px(design::CAP + 4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(design::r::XS))
                                    .hover(|s| s.bg(t.surface))
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        drop(rm.update(cx, |this, cx| this.remove_entry(path.clone(), e1.clone(), cx)))
                                    })
                                    .child(design::icon(design::g::CANCEL, design::ic::SM, t.text_muted)),
                            )
                    }))
                    .when(entries.is_empty() && r.kind == Kind::Keys, |d| d.child(div().text_size(px(design::ts::BODY_SM)).text_color(t.text_muted).child("None yet")))
                    .when(r.kind == Kind::List, |d| d.child(self.text_box(r, on, t)))
                    .into_any_element()
            }
            Kind::Bool => design::toggle(value == "true", t).into_any_element(),
            Kind::Choice(opts) => {
                let me = me.clone();
                let path = r.path.clone();
                choices(opts, &value, t, &r.path.join("."), move |v, cx| {
                    let path = path.clone();
                    drop(me.update(cx, |this, cx| this.save(&path, v.into(), cx)));
                })
            }
            Kind::Hotkey => hotkey_field(&value, self.recording && on, t),
            Kind::Open(_) => design::button("Open", design::Button::Quiet, on, t).into_any_element(),
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
    fn command_control(&self, i: usize, r: &Row, on: bool, t: &design::Tokens, me: gpui::WeakEntity<Self>) -> AnyElement {
        let hotkey = self.launcher.hotkeys.get(&key_of(&self.items[i])).cloned().unwrap_or_default();
        let keys = hotkey_field(&hotkey, self.recording && on, t);
        let (me2, me3) = (me.clone(), me);
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(8.))
            .child(self.text_box(r, on, t))
            .child(
                div()
                    .id(("hotkey", i))
                    .flex_none()
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
                    .child(design::toggle(!self.is_off(i), t)),
            )
            .into_any_element()
    }
}

impl Settings {
    /// Rows under their headings, each group in one rounded card.
    fn cards(&self, rows: &[Row], t: &design::Tokens, me: &gpui::WeakEntity<Self>) -> Vec<AnyElement> {
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
                    .gap(px(8.))
                    .pb(px(16.))
                    .when(!group.is_empty(), |d| d.child(div().px(px(4.)).font_family(design::FONT_SMALL).text_size(px(design::ts::SMALL)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_faint).child(group)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .rounded(px(design::r::LG))
                            .border_1()
                            .border_color(t.hairline)
                            .bg(t.surface)
                            .overflow_hidden()
                            .children((start..i).map(|j| self.row_view(j, &rows[j], j > start, t, me))),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    fn row_view(&self, i: usize, r: &Row, divider: bool, t: &design::Tokens, me: &gpui::WeakEntity<Self>) -> AnyElement {
        let on = i == self.selected;
        let click = me.clone();
        div()
            .id(("row", i))
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(16.))
            .py(px(12.))
            .relative()
            .when(divider, |d| d.border_t_1().border_color(t.hairline))
            .when(on, |d| d.rounded(px(design::r::LG)).bg(t.selected()).child(div().absolute().left(px(0.)).top(px(0.)).bottom(px(0.)).flex().items_center().child(div().w(px(3.)).h(px(16.)).rounded_full().bg(t.accent))))
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
                Kind::Command(c) => Some(design::icon_view(&self.items[c].icon, t, on)),
                _ => None,
            })
            .child(
                div()
                    .w(px(if matches!(r.kind, Kind::Command(_)) { 240. } else { 280. }))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().child(r.label.clone()))
                    .children((!r.help.is_empty()).then(|| div().text_size(px(design::ts::SMALL)).line_height(px(16.)).text_color(t.text_muted).child(r.help.clone()))),
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
        let t = design::Tokens::from_theme(&theme, window);
        design::scale_settings();
        window.set_rem_size(px(16.));
        let rows = self.rows();
        let me = cx.weak_entity();
        crate::dump_ui(
            "procast-settings.json",
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
            .font_family(design::FONT_UI)
            .text_size(px(design::ts::BODY))
            .bg(t.bg_solid)
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
                    this.status = Some((false, "Cancelled".into()));
                    cx.notify();
                } else {
                    this.commit(cx);
                    window.remove_window();
                }
            }))
            .child(
                div()
                    .w(px(192.))
                    .flex_none()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .bg(t.surface)
                    .border_r_1()
                    .border_color(t.hairline)
                    .child(div().h(px(design::ROW)).px(px(8.)).mb(px(4.)).flex().items_center().gap(px(8.)).child(design::icon(design::g::SETTING, design::ic::LG, t.accent)).child(div().font_weight(FontWeight::SEMIBOLD).child("procast")))
                    .children(SECTIONS.iter().enumerate().map(|(i, name)| {
                        let me = me.clone();
                        let on = i == self.section;
                        design::selectable(design::CONTROL, on, t.accent, &t)
                            .id(("section", i))
                            .px(px(12.))
                            .when(!on, |d| d.text_color(t.text_muted))
                            .on_click(move |_, window, cx| drop(me.update(cx, |this, cx| this.set_section(i, window, cx))))
                            .child(*name)
                    })),
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
                            .p(px(16.))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(div().h(px(design::ROW)).mb(px(8.)).flex().flex_none().items_center().font_family(design::FONT_DISPLAY).text_size(px(design::ts::DISPLAY)).font_weight(FontWeight::SEMIBOLD).child(SECTIONS[self.section]))
                            .children((SECTIONS[self.section] == "Theme").then(|| preview(&t)))
                            .children(self.cards(&rows, &t, &me))
                            .children(rows.is_empty().then(|| div().text_color(t.text_muted).child("Nothing to configure here yet."))),
                    )
                    .child(
                        // Key hints, or the last save's outcome.
                        div()
                            .h(px(design::FOOTER))
                            .flex_none()
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .border_t_1()
                            .border_color(t.hairline)
                            .text_size(px(design::ts::BODY_SM))
                            .text_color(t.text_muted)
                            .map(|d| match &self.status {
                                Some((error, m)) => d.child(div().size(px(8.)).flex_none().rounded_full().bg(if *error { t.danger } else { t.success })).child(div().min_w_0().truncate().text_color(t.text).child(m.clone())),
                                None if SECTIONS[self.section] == "Shortcuts" => d.child(design::shortcut("Record hotkey", &["↵"], &t)).child(design::vdivider(&t)).child(design::shortcut("Turn on or off", &["Ctrl", "D"], &t)),
                                None => d
                                    .child(design::shortcut("Rows", &["↑", "↓"], &t))
                                    .child(design::vdivider(&t))
                                    .child(design::shortcut("Pages", &["Ctrl", "↑", "↓"], &t))
                                    .child(design::vdivider(&t))
                                    .child(design::shortcut("Change", &["↵"], &t))
                                    .child(div().flex_1())
                                    .child(design::shortcut("Close", &["Esc"], &t)),
                            }),
                    ),
            )
    }
}

/// A segmented control with the current option highlighted; clicking an option picks it.
fn choices(opts: &[&'static str], current: &str, t: &design::Tokens, id: &str, pick: impl Fn(&'static str, &mut gpui::App) + Clone + 'static) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(design::r::SM))
        .bg(t.surface)
        .children(opts.iter().map(|o| {
            let cur = *o == current;
            let pick = pick.clone();
            let o: &'static str = o;
            div()
                .id(SharedString::from(format!("{id}={o}")))
                .h(px(design::CONTROL - 4.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(px(design::r::XS))
                .text_size(px(design::ts::BODY_SM))
                .when(cur, |d| d.bg(t.surface_strong).text_color(t.text).font_weight(FontWeight::SEMIBOLD).child(div().mr(px(8.)).size(px(6.)).flex_none().rounded_full().bg(t.accent)))
                .when(!cur, |d| d.text_color(t.text_muted).hover(|s| s.bg(t.hover).text_color(t.text)))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    pick(o, cx)
                })
                .child(choice_label(o))
        }))
        .into_any_element()
}

/// Theme rows show the colour in use: the override, or the palette's own.
fn swatch(r: &Row, t: &design::Tokens) -> Option<gpui::Hsla> {
    let [section, key] = r.path.as_slice() else { return None };
    if section != "theme" {
        return None;
    }
    Some(match key.as_str() {
        "background" => t.bg,
        "text" => t.text,
        "muted" => t.text_muted,
        "selected" => t.accent,
        _ => return None,
    })
}

/// Theme studio: a small launcher drawn with the current theme, updated as you edit. Built
/// from the launcher's own components, so what you see here is what you get.
fn preview(t: &design::Tokens) -> impl IntoElement {
    use design::{Tint, icon, g, ic, key_cap, list_row, section_header};
    let row = |glyph: &'static str, title: &str, sub: &str, tint: Tint, on: bool| list_row(design::icon_tile(&Icon::Vector(glyph), t, on, Some(tint)), title, sub, None, on, t.accent, t);
    div()
        .mb(px(16.))
        .flex_none()
        .rounded(px(design::r::LG))
        .border_1()
        .border_color(t.border)
        .bg(t.body())
        .overflow_hidden()
        .child(
            div()
                .h(px(design::BAR))
                .px(px(16.))
                .flex()
                .items_center()
                .gap(px(12.))
                .border_b_1()
                .border_color(t.hairline)
                .child(div().size(px(design::TILE)).flex().items_center().justify_center().child(icon(g::SEARCH, ic::LG, t.accent)))
                .child(div().font_family(design::FONT_DISPLAY).text_size(px(design::ts::QUERY)).text_color(t.text_faint).child("Search apps and commands…")),
        )
        .child(row(g::FOLDER, "Search Files", "Files", Tint::File, false))
        .child(row(g::PASTE, "Clipboard History", "Clipboard", Tint::Paste, true))
        .child(row(g::DOCK_LEFT, "Left Half", "Window", Tint::Window, false))
        .child(section_header("Use \"42\" with", None, t))
        .child(row(g::GLOBE, "Search the web", "", Tint::Web, false))
        .child(
            div()
                .h(px(design::FOOTER))
                .px(px(16.))
                .flex()
                .items_center()
                .gap(px(8.))
                .border_t_1()
                .border_color(t.hairline)
                .text_size(px(design::ts::BODY_SM))
                .child(icon(g::PASTE, ic::MD, Tint::Paste.hue(t)))
                .child(div().flex_1().text_color(t.text_muted).child("Command"))
                .child(div().text_color(t.text).child("Open"))
                .child(key_cap("↵", t)),
        )
}

/// The one hotkey control: a field holding the keys, or an invitation to record them.
fn hotkey_field(value: &str, recording: bool, t: &design::Tokens) -> AnyElement {
    let field = design::field(recording, t).flex_none().gap(px(8.)).text_size(px(design::ts::BODY_SM));
    if recording {
        field.text_color(t.text).child("Press keys. Backspace removes, Esc cancels").into_any_element()
    } else if value.is_empty() {
        field.text_color(t.text_muted).child(design::icon(design::g::KEYBOARD, design::ic::MD, t.text_faint)).child("Record").into_any_element()
    } else {
        field.child(design::key_caps(value, t)).into_any_element()
    }
}
