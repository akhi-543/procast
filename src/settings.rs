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
    /// A list of strings, edited as "a; b; c".
    List,
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
}

const SECTIONS: [&str; 8] = ["General", "Launcher", "Shortcuts", "Clipboard", "Files", "Theme", "Extensions", "Advanced"];

fn row(label: &str, path: &str, kind: Kind, help: &str) -> Row {
    Row { label: label.into(), help: help.into(), path: path.split('.').map(String::from).collect(), kind }
}

fn rows(section: usize, exts: &[ext::Extension]) -> Vec<Row> {
    match SECTIONS[section] {
        "General" => vec![
            row("Open at login", "start_at_login", Kind::Bool, ""),
            row("Show in tray", "tray", Kind::Bool, "The akscast icon in the notification area"),
            row("Launcher hotkey", "hotkey", Kind::Hotkey, "Opens akscast"),
            row("Emoji hotkey", "emoji.hotkey", Kind::Hotkey, "Opens emoji search directly"),
            row("Appearance", "theme.mode", Kind::Choice(&["system", "dark", "light"]), "system follows Windows"),
            row("Dark theme", "theme.dark", Kind::Choice(crate::design::DARK_THEMES), ""),
            row("Light theme", "theme.light", Kind::Choice(crate::design::LIGHT_THEMES), ""),
            row("Interface size", "size", Kind::Choice(&["small", "default", "large"]), "Text, rows and window"),
            row("Window mode", "window_mode", Kind::Choice(&["expanded", "compact"]), "Compact: only the search bar until you type"),
            row("Favourites in compact mode", "launcher.compact_favorites", Kind::Bool, "Icons under the search bar"),
            row("Width", "width", Kind::Number, "Share of the screen, e.g. 0.5 (same look on every monitor); above 1 = pixels"),
        ],
        "Launcher" => vec![
            row("Show akscast on", "monitor", Kind::Choice(&["cursor", "active", "primary"]), "Screen with the mouse, with the focused window, or the main one"),
            row("Pop to root search", "launcher.pop_to_root", Kind::Choice(config::POP_TO_ROOT), "How long the last search survives closing"),
            row("Search sensitivity", "launcher.sensitivity", Kind::Choice(&["low", "medium", "high"]), "Low shows only close matches"),
            row("Rows", "max_results", Kind::Number, "Results shown in root search"),
            row("Include files", "launcher.files", Kind::Bool, "Matching files under root results"),
            row("Search history", "launcher.history", Kind::Bool, "Up arrow brings back earlier searches"),
            row("Escape key", "launcher.escape", Kind::Choice(&["back", "close"]), "In a command: go back one level, or close and pop to root"),
            row("Navigation keys", "launcher.navigation", Kind::Choice(&["default", "emacs", "vim"]), "Emacs: Alt+N/P move, Alt+B back · Vim: Alt+J/K move, Alt+H back"),
            row("Game mode", "launcher.game_mode", Kind::Bool, "Hotkeys do nothing while a full-screen app or game is focused"),
            row("Favourites", "launcher.favorites", Kind::List, "Pinned to the top · Ctrl+K → Add to Favorites"),
            row("Hidden from root search", "launcher.hidden", Kind::List, "Ctrl+K → Hide from Root Search; remove one to show it again"),
            row("Calculator", "search.calculator", Kind::Bool, "Math, units and currencies"),
            row("Fallback commands", "search.fallbacks", Kind::List, "Offered for any query, in order: web, or quicklink names"),
            row("Web search URL", "search.fallback", Kind::Text, "{query} is replaced"),
        ],
        "Clipboard" => vec![
            row("Clipboard history", "clipboard.enabled", Kind::Bool, "Text only, encrypted for your Windows account"),
            row("Entries kept", "clipboard.history", Kind::Number, ""),
            row("Hotkey", "clipboard.hotkey", Kind::Hotkey, "Opens clipboard history directly"),
            row("Ignored apps", "clipboard.ignore_apps", Kind::List, "Executable names; their copies are never saved"),
        ],
        "Files" => vec![
            row("File search", "files.enabled", Kind::Bool, ""),
            row("Hotkey", "files.hotkey", Kind::Hotkey, "Opens file search directly"),
            row("Folders", "files.roots", Kind::List, "Empty = your user folder"),
            row("Skip names", "files.exclude", Kind::List, "Folder or file names skipped anywhere"),
            row("Whole-drive search", "files.ntfs", Kind::Bool, "Every NTFS drive, live. Needs \"Enable Fast File Search\" once (admin)"),
            row("Drives", "files.drives", Kind::List, "Whole-drive search: letters, e.g. C; D. Empty = every NTFS drive"),
            row("Skip folders", "files.skip", Kind::List, "Whole-drive search: full paths left out (saves memory)"),
        ],
        "Theme" => {
            let mut r = vec![row("Mode", "theme.mode", Kind::Choice(&["system", "dark", "light"]), "")];
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
                r.push(row(label, &format!("theme.{key}"), Kind::Text, "#rrggbb, empty = from mode"));
            }
            r
        }
        "Extensions" => {
            let mut r = vec![
                row("Background commands", "extensions.background", Kind::Bool, "Run commands that declare an interval"),
                row("Tray icons", "extensions.menu_bar", Kind::Bool, "Show menu-bar commands in the tray"),
                row("Node.js", "extensions.node", Kind::Text, "Runs the extension host"),
                row("Memory per command (MB)", "extensions.memory_mb", Kind::Number, ""),
            ];
            for x in exts {
                let mut seen = vec![];
                for p in x.prefs.iter().chain(x.commands.iter().flat_map(|c| c.prefs.iter())) {
                    if seen.contains(&p.name) {
                        continue;
                    }
                    seen.push(p.name.clone());
                    let title = if p.title.is_empty() { &p.name } else { &p.title };
                    let kind = if p.kind == "checkbox" { Kind::Bool } else { Kind::Text };
                    let help = match (p.required, p.kind.as_str()) {
                        (true, k) => format!("required · {k}"),
                        (false, k) => k.to_string(),
                    };
                    r.push(Row {
                        label: format!("{} › {title}", x.title),
                        help,
                        path: vec!["extension".into(), x.name.clone(), p.name.clone()],
                        kind,
                    });
                }
            }
            r
        }
        "Shortcuts" => vec![],
        _ => vec![
            Row { label: "Open config.toml".into(), help: "Every setting, with comments".into(), path: vec![], kind: Kind::Open("config.toml") },
            Row { label: "Open extensions log".into(), help: "Extension errors and console output".into(), path: vec![], kind: Kind::Open("extensions.log") },
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
            section: 0,
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
            Row { label: "Show".into(), help: "↵ next category".into(), path: vec![], kind: Kind::Category },
            Row { label: "Filter".into(), help: "By name or alias".into(), path: vec!["shortcuts-filter".into()], kind: Kind::Filter },
        ];
        let want = CATEGORIES[self.category];
        let words: Vec<String> = self.filter.split_whitespace().map(str::to_lowercase).collect();
        for (i, it) in self.items.iter().enumerate() {
            let hay = format!("{} {} {}", it.title, it.subtitle, it.alias).to_lowercase();
            if (want == "All" || category(it) == want) && words.iter().all(|w| hay.contains(w.as_str())) {
                let help = [category(it).trim_end_matches('s'), &it.subtitle].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                out.push(Row { label: it.title.clone(), help, path: vec!["launcher".into(), "aliases".into(), key_of(it)], kind: Kind::Command(i) });
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
                    _ => config::read_value(&self.text, &r.path),
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
        let Some(input) = self.inputs.get(&r.path.join(".")) else { return };
        let typed = input.read(cx).as_str().trim().to_string();
        if typed == config::read_value(&self.text, &r.path) {
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
            Kind::List => {
                let mut a = toml_edit::Array::new();
                for part in typed.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                    a.push(part);
                }
                a.into()
            }
            _ => typed.into(),
        };
        let path = r.path.clone();
        self.save(&path, value, cx);
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
        let current = config::read_value(&self.text, &r.path);
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
            Kind::Filter => {}
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

    fn control(&self, r: &Row, on: bool, t: &crate::design::Tokens) -> AnyElement {
        if let Some(color) = swatch(r, t) {
            return div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().size(px(22.)).flex_none().rounded(px(5.)).border_1().border_color(t.border).bg(color))
                .child(div().flex_1().min_w_0().child(self.input_control(r, on, t)))
                .into_any_element();
        }
        self.input_control(r, on, t)
    }

    fn input_control(&self, r: &Row, on: bool, t: &crate::design::Tokens) -> AnyElement {
        let value = config::read_value(&self.text, &r.path);
        let pill = |text: String, active: bool| {
            div()
                .px_3()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(if on { t.accent } else { t.border })
                .when(active, |d| d.bg(t.accent).text_color(t.accent_text))
                .child(text)
                .into_any_element()
        };
        match &r.kind {
            Kind::Text | Kind::Number | Kind::List | Kind::Filter | Kind::Command(_) => match self.inputs.get(&r.path.join(".")) {
                Some(input) => text_input(SharedString::from(format!("set-{}", r.path.join("."))))
                    .state(input.downgrade())
                    .placeholder(match r.kind {
                        Kind::List => "a; b; c",
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
                    .bg(t.surface)
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Kind::Bool => crate::design::toggle(value == "true", t).into_any_element(),
            Kind::Choice(opts) => choices(opts, &value, t),
            Kind::Hotkey if self.recording && on => pill("Press keys…".into(), true),
            Kind::Hotkey if value.is_empty() => pill("Record".into(), false),
            Kind::Hotkey => div().flex().gap(px(4.)).children(value.split('+').map(|k| crate::design::key_cap(k, t))).into_any_element(),
            Kind::Open(_) => pill("Open".into(), false),
            Kind::Category => choices(CATEGORIES, CATEGORIES[self.category], t),
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
            .child(div().w(px(110.)).flex_none().child(self.input_control(r, on, t)))
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
                    .child(div().px_2().text_xs().text_color(t.text_muted).child(if SECTIONS[self.section] == "Shortcuts" { "Alias: type · ↵ hotkey · Ctrl+D on/off" } else { "↑↓ rows · Ctrl+↑↓ sections · ↵ change · Esc close" })),
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
                            .children(rows.iter().enumerate().map(|(i, r)| {
                                let on = i == self.selected;
                                let me = me.clone();
                                div()
                                    .id(("row", i))
                                    .flex()
                                    .items_center()
                                    .gap_4()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .when(on, |d| d.bg(t.surface))
                                    .on_click({
                                        let me = me.clone();
                                        move |_, window, cx| {
                                            drop(me.update(cx, |this, cx| {
                                                let again = this.selected == i;
                                                this.select(i, window, cx);
                                                if again || !matches!(this.rows()[i].kind, Kind::Text | Kind::Number | Kind::List | Kind::Filter | Kind::Command(_)) {
                                                    this.activate(window, cx);
                                                }
                                            }))
                                        }
                                    })
                                    .when(matches!(r.kind, Kind::Command(c) if self.is_off(c)), |d| d.text_color(t.text_muted))
                                    .children(match r.kind {
                                        Kind::Command(c) => Some(crate::design::icon_view(&self.items[c].icon, &t, on)),
                                        _ => None,
                                    })
                                    .child(
                                        div()
                                            .w(px(240.))
                                            .flex_none()
                                            .flex()
                                            .flex_col()
                                            .child(div().truncate().child(r.label.clone()))
                                            .children((!r.help.is_empty()).then(|| div().text_xs().text_color(t.text_muted).truncate().child(r.help.clone()))),
                                    )
                                    .child(div().flex_1().min_w_0().child(match r.kind {
                                        Kind::Command(c) => self.command_control(c, r, on, &t, me.clone()),
                                        _ => self.control(r, on, &t),
                                    }))
                            }))
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

/// A segmented control with the current option highlighted.
fn choices(opts: &[&str], current: &str, t: &crate::design::Tokens) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(6.))
        .bg(t.surface)
        .children(opts.iter().map(|o| {
            let cur = *o == current;
            div().px(px(10.)).py(px(3.)).rounded(px(4.)).text_size(px(13.)).when(cur, |d| d.bg(t.accent).text_color(t.accent_text)).when(!cur, |d| d.text_color(t.text_muted)).child(o.to_string())
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
