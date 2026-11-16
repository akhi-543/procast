//! The settings window. Every row edits one key of config.toml in place (comments and
//! layout kept); the launcher picks the change up through its usual reload.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use gpui::{
    AnyElement, Context, Entity, FocusHandle, Focusable, FontWeight, KeyDownEvent, ScrollHandle, SharedString, Subscription, Window, div,
    prelude::*,
};
use futures::channel::mpsc::UnboundedSender;
use gpui_ce_elements::editable_text::{EditableTextState, StringStorage, TextChanged, text_input};

use crate::{
    Dismiss, Launch, NextSection, PrevSection, SelectNext, SelectPrev, config,
    design::{self, g, px},
    ext, menu,
    search::{Icon, Item},
    win::Msg,
};

#[derive(Clone, PartialEq)]
enum Kind {
    Text,
    Number,
    Bool,
    Hotkey,
    /// One of a few labelled values: a segmented control.
    Choice(&'static [Opt]),
    /// A language code from `translate::LANGUAGES`, picked in a dropdown with search; these
    /// options ("Auto-Detect", "None") lead the list.
    Language(&'static [Opt]),
    /// A list of strings: removable chips plus an Add field.
    List,
    /// A list of launcher item keys, shown by item name; entries can only be removed here.
    Keys,
    /// Opens a file.
    Open(&'static str),
    /// A line of live status; nothing to edit.
    Status(Live),
    /// A button that tells the launcher to do something (by name).
    Action(&'static str),
    /// Shortcuts: which kind of item the table lists (not saved).
    Category,
    /// Shortcuts: narrows the table by title (not saved).
    Filter,
    /// Shortcuts: one item (index into `Settings::items`); its input edits the alias.
    Command(usize),
    /// Adds a `[[snippet]]` / `[[quicklink]]` table.
    TableAdd(&'static str),
    /// Copy and Delete for one table of that list.
    TableItem(&'static str, usize),
}

/// What a `Status` row shows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Live {
    /// The file index line, in the control column.
    Index,
    /// Whether the foreground hook runs, in the row's label.
    Foreground,
}

/// What a `Choice` option writes: a string, or a whole number (minutes, days).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Val {
    S(&'static str),
    N(i64),
}

impl Val {
    fn value(self) -> toml_edit::Value {
        match self {
            Val::S(s) => s.into(),
            Val::N(n) => n.into(),
        }
    }

    /// As `config::shown_value` reads it back.
    fn text(self) -> String {
        match self {
            Val::S(s) => s.into(),
            Val::N(n) => n.to_string(),
        }
    }
}

/// An option: its label, then the value it writes.
type Opt = (&'static str, Val);
use Val::{N, S};

const COLOUR_MODES: &[Opt] = &[("System", S("system")), ("Dark", S("dark")), ("Light", S("light"))];
/// Values are what config.toml stores.
const TAP_KEYS: &[Opt] = &[("Off", S("off")), ("Ctrl", S("ctrl")), ("Shift", S("shift")), ("Alt", S("alt"))];
const HYPER_KEYS: &[Opt] = &[("Off", S("off")), ("Caps Lock", S("capslock")), ("Right Alt", S("rightalt")), ("Right Ctrl", S("rightctrl")), ("Scroll Lock", S("scrolllock"))];
const POP_TO_ROOT: &[Opt] = &[("Now", S("immediately")), ("15 s", S("15s")), ("30 s", S("30s")), ("60 s", S("60s")), ("120 s", S("120s")), ("180 s", S("180s")), ("Never", S("never"))];
const CLIPBOARD_KEEP: &[Opt] = &[
    ("By count", S("count")),
    ("1 hour", S("1h")),
    ("1 day", S("1d")),
    ("1 week", S("7d")),
    ("1 month", S("30d")),
    ("3 months", S("90d")),
    ("6 months", S("180d")),
    ("1 year", S("365d")),
    ("Forever", S("unlimited")),
];
const SKIN_TONES: &[Opt] = &[("None", S("none")), ("Light", S("light")), ("Medium light", S("medium_light")), ("Medium", S("medium")), ("Medium dark", S("medium_dark")), ("Dark", S("dark"))];

/// The fields a `[[list]]` table is edited by, with their labels and help.
fn table_fields(list: &str) -> &'static [(&'static str, &'static str, &'static str)] {
    match list {
        "snippet" => &[
            ("name", "Name", "What you search for in procast."),
            ("keyword", "Keyword", "Typed in any app, it turns into the text, e.g. ;sig. Empty: search only."),
            ("text", "Text", "\\n for a new line. {clipboard} {selection} {date} {time} {cursor} {argument name=\"To\"}"),
        ],
        _ => &[
            ("name", "Name", "What you search for in procast."),
            ("keyword", "Keyword", "Type it, a space, then your search, e.g. gh rust."),
            ("url", "Link", "{query} becomes what you type, e.g. https://github.com/search?q={query}. {selection} takes the text selected in the app you came from; {clipboard} what you copied last."),
        ],
    }
}

/// A snippet's text edits on one line, with "\n" for its line breaks.
fn multiline(path: &[String]) -> bool {
    matches!(path, [list, _, field] if list == "snippet" && field == "text")
}

/// The Snippets / Quicklinks page: add, then each table's fields, Copy and Delete.
fn table_rows(list: &'static str, text: &str) -> Vec<Row> {
    let doc = text.trim_start_matches('\u{feff}').parse::<toml_edit::DocumentMut>().ok();
    let count = doc.as_ref().and_then(|d| d.get(list)).and_then(|t| t.as_array_of_tables()).map_or(0, |t| t.len());
    let what = if list == "snippet" { "snippet" } else { "quicklink" };
    let mut out = vec![Row { label: format!("Add a {what}"), help: String::new(), path: vec![], kind: Kind::TableAdd(list), group: format!("{}s", capital(what)), icon: "" }];
    for i in 0..count {
        let name = config::shown_value(text, &[list.into(), i.to_string(), "name".into()]);
        let group = format!("{} {} · {}", capital(what), i + 1, if name.is_empty() { "unnamed" } else { &name });
        for (field, label, help) in table_fields(list) {
            out.push(Row { label: (*label).into(), help: (*help).into(), path: vec![list.into(), i.to_string(), (*field).into()], kind: Kind::Text, group: group.clone(), icon: "" });
        }
        out.push(Row { label: "Copy or delete".into(), help: "Ctrl+D copies it, Delete removes it.".into(), path: vec![], kind: Kind::TableItem(list, i), group, icon: "" });
    }
    out
}

const CATEGORIES: &[&str] = &["All", "Commands", "Applications", "Extensions", "Quicklinks", "Snippets", "Scripts", "Layouts", "Windows Settings"];

/// The Shortcuts category an item belongs to, from its key prefix.
fn category(it: &Item) -> &'static str {
    match it.key.as_deref().and_then(|k| k.split_once(':')).map(|(p, _)| p) {
        Some("cmd") => "Commands",
        Some("ext") => "Extensions",
        Some("ql") => "Quicklinks",
        Some("sn") => "Snippets",
        Some("sc") => "Scripts",
        Some("wl") => "Layouts",
        Some("ms") => "Windows Settings",
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
    group: String,
    /// A `design::g` name; a row with one takes the `.pc-setrow` recipe. Empty: the older look.
    icon: &'static str,
}

impl Row {
    fn with_icon(self, icon: &'static str) -> Row {
        Row { icon, ..self }
    }
}

const SECTIONS: [&str; 19] = [
    "General",
    "Launcher",
    "Shortcuts",
    "Apps",
    "Clipboard",
    "Snippets",
    "Quicklinks",
    "Emoji",
    "Files",
    "Window",
    "Theme",
    "Extensions",
    "Translator",
    "Screenshots",
    "Focus",
    "Calendar",
    "Notes",
    "Games",
    "Advanced",
];
/// The sidebar icon of each section, in `SECTIONS` order. A feature's section takes its
/// command's icon.
const SECTION_ICONS: [&str; 19] = [
    design::g::SLIDERS_HORIZONTAL,
    design::g::SEARCH,
    design::g::KEYBOARD,
    design::g::APP,
    design::g::PASTE,
    design::g::TEXT_CURSOR_INPUT,
    design::g::LINK,
    design::g::EMOJI,
    design::g::FOLDER,
    design::g::WIN_LEFT_HALF,
    design::g::PERSONALIZE,
    design::g::PUZZLE,
    design::g::LANGUAGES,
    design::g::SCAN_SEARCH,
    design::g::SHIELD_CHECK,
    design::g::CALENDAR_CLOCK,
    design::g::NOTEBOOK_PEN,
    design::g::GAMEPAD_2,
    design::g::WRENCH,
];

/// A feature section's page header (`.pc-sett__head`): its command's tint for the sticker
/// tile, and one line on what it does.
fn head(section: &str) -> Option<(design::Tint, &'static str)> {
    use design::Tint;
    Some(match section {
        "Translator" => (Tint::Web, ""),
        "Screenshots" => (Tint::File, ""),
        "Calendar" => (Tint::System, "Today's meetings in search, with a one-key join."),
        "Focus" | "Notes" | "Games" => (Tint::System, ""),
        _ => return None,
    })
}

fn row(label: &str, path: &str, kind: Kind, help: &str) -> Row {
    Row { label: label.into(), help: help.into(), path: path.split('.').map(String::from).collect(), kind, group: String::new(), icon: "" }
}

/// Rows under card headings.
fn groups(list: Vec<(&'static str, Vec<Row>)>) -> Vec<Row> {
    list.into_iter().flat_map(|(g, rows)| rows.into_iter().map(move |r| Row { group: g.into(), ..r })).collect()
}

/// "snippet" -> "Snippet".
fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// The label a language row shows for `code`.
fn language_label(lead: &[Opt], code: &str) -> String {
    lead.iter().find(|o| o.1.text() == code).map_or_else(|| crate::translate::language_name(code), |o| o.0.to_string())
}

/// A language row's dropdown: its leading options, a rule, every language; the current one selected.
fn language_menu(lead: &[Opt], current: &str) -> menu::Menu {
    let mut entries: Vec<menu::Entry> = lead.iter().map(|o| menu::Entry { id: o.1.text(), label: o.0.into(), ..Default::default() }).collect();
    if let Some(last) = entries.last_mut() {
        last.rule_after = true;
    }
    entries.extend(crate::translate::LANGUAGES.iter().map(|(code, name)| menu::Entry { id: (*code).into(), label: (*name).into(), ..Default::default() }));
    // ponytail: opens under the window's top-right corner, not under the row; place it at
    // the row once rows report their bounds.
    let mut m = menu::Menu::new(entries, Some("Search…".into()), menu::Place::Right);
    if let Some(i) = m.entries.iter().position(|e| e.id == current) {
        m.selected = i;
    }
    m
}

fn rows(section: usize, exts: &[ext::Extension], text: &str) -> Vec<Row> {
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
                    row("Double-tap to open", "keyboard.double_tap", Kind::Choice(TAP_KEYS), "Press this key twice quickly to open procast, besides the hotkey above."),
                    row("Hyper key", "keyboard.hyper", Kind::Choice(HYPER_KEYS), "Hold this key for Ctrl+Alt+Shift+Win, so Hyper+K style hotkeys never clash with apps. Tapped alone it does nothing."),
                ],
            ),
            (
                "Appearance",
                vec![
                    row("Colour mode", "theme.mode", Kind::Choice(COLOUR_MODES), "System follows the light or dark setting of Windows."),
                    row("Interface size", "size", Kind::Choice(&[("Small", S("small")), ("Default", S("default")), ("Large", S("large"))]), "Scales text, rows and the window together."),
                ],
            ),
            (
                "Window",
                vec![
                    row("Window style", "window_mode", Kind::Choice(&[("Full list", S("expanded")), ("Search bar only", S("compact"))]), "Search bar only keeps the window small until you type or press Down."),
                    row("Favourites under the search bar", "launcher.compact_favorites", Kind::Bool, "With the search bar only style, show your favourites as a row of icons."),
                    row("Width", "width", Kind::Number, "A share of the screen, e.g. 0.5 for half: looks the same on every monitor. Numbers above 1 are pixels."),
                ],
            ),
        ]),
        "Launcher" => groups(vec![
            (
                "Opening",
                vec![
                    row("Show procast on", "monitor", Kind::Choice(&[("Mouse", S("cursor")), ("Focused window", S("active")), ("Main screen", S("primary"))]), "Which monitor the window opens on: the one with the mouse, the one with the window you're using, or the main one."),
                    row("Keep the last search", "launcher.pop_to_root", Kind::Choice(POP_TO_ROOT), "How long procast remembers where you were after it closes. Now always starts fresh."),
                ],
            ),
            (
                "Results",
                vec![
                    row("Match strictness", "launcher.sensitivity", Kind::Choice(&[("Low", S("low")), ("Medium", S("medium")), ("High", S("high"))]), "Low: every word must appear as typed. Medium: drops weak fuzzy matches. High: any fuzzy match."),
                    row("Rows shown", "max_results", Kind::Number, "How many results root search lists at most, e.g. 8."),
                    row("Include files", "launcher.files", Kind::Bool, "Also list matching files below apps and commands."),
                    row("Calculator", "search.calculator", Kind::Bool, "Answer maths, unit and currency conversions as you type (e.g. 5 km in miles)."),
                ],
            ),
            (
                "Keyboard",
                vec![
                    row("Search history", "launcher.history", Kind::Bool, "Up arrow in an empty search bar brings back earlier searches."),
                    row("Escape key", "launcher.escape", Kind::Choice(&[("Go back", S("back")), ("Close", S("close"))]), "Inside a command such as Clipboard History: go back one level, or close the window and start fresh next time."),
                    row("Extra navigation keys", "launcher.navigation", Kind::Choice(&[("Default", S("default")), ("Emacs", S("emacs")), ("Vim", S("vim"))]), "Move without the arrow keys. Emacs: Alt+N/P move, Alt+B back. Vim: Alt+J/K move, Alt+H back."),
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
                "Windows Run",
                vec![
                    row("Run with", "search.shell", Kind::Choice(&[("Run", S("run")), ("Cmd", S("cmd")), ("Powershell", S("powershell")), ("Pwsh", S("pwsh"))]), "What runs a line typed after >. Run works like Win+R; a shell stays open on what the command printed."),
                    row("In Windows Terminal", "search.terminal", Kind::Bool, "Open that shell as a Windows Terminal tab instead of its own window."),
                ],
            ),
            (
                "When nothing matches",
                vec![
                    row("Fallback commands", "search.fallbacks", Kind::List, "Offered below every search, in this order. \"web\", \"chatgpt\", \"ai\", or the name of a quicklink with {query}."),
                    row("Web search URL", "search.fallback", Kind::Text, "Used by the \"web\" fallback; {query} becomes what you typed."),
                    row("Ask AI model", "ai.model", Kind::Choice(&[("Haiku", S("haiku")), ("Sonnet", S("sonnet")), ("Opus", S("opus"))]), "Used by the \"ai\" fallback, on your Claude Code login. Haiku answers fastest."),
                ],
            ),
        ]),
        "Apps" => groups(vec![
            (
                "Hotkeys",
                vec![row(
                    "An app's hotkey",
                    "apps.hotkey_action",
                    Kind::Choice(&[("New window", S("launch")), ("Switch to it", S("focus")), ("Switch or hide", S("toggle"))]),
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
        "Clipboard" => groups(vec![
            (
                "Clipboard history",
                vec![
                    row("Keep clipboard history", "clipboard.enabled", Kind::Bool, "Remember text, images and files you copy. Stored encrypted, readable only by your Windows account."),
                    row("Hotkey", "clipboard.hotkey", Kind::Hotkey, "Opens clipboard history straight away."),
                    row("Never record from", "clipboard.ignore_apps", Kind::List, "Program file names, e.g. keepass.exe. Copies made in them are not saved."),
                ],
            ),
            (
                "Keeping",
                vec![
                    row("Keep entries for", "clipboard.keep", Kind::Choice(CLIPBOARD_KEEP), "How long copies stay. By count keeps the newest ones below. Pinned entries (Ctrl+K → Pin Entry) always stay; Forever grows on disk."),
                    row("Entries kept", "clipboard.history", Kind::Number, "With By count: the oldest entries are dropped beyond this many, e.g. 500."),
                ],
            ),
            (
                "Images",
                vec![row("Find text in images", "clipboard.ocr", Kind::Bool, "Read the text in copied pictures with Windows' own text recognition (offline), so search finds them.")],
            ),
        ]),
        "Snippets" => {
            let mut r = groups(vec![(
                "Typing anywhere",
                vec![
                    row("Expand keywords", "snippets.expand", Kind::Bool, "Typing a snippet's keyword in any app replaces it with the snippet."),
                    row("Expand", "snippets.trigger", Kind::Choice(&[("Now", S("immediately")), ("After a space", S("delimiter")), ("Space dropped", S("delimiter_discard"))]), "Now: as the keyword's last letter goes in. After a space: once you type a space or punctuation after it, which stays or is dropped."),
                    row("Inside words", "snippets.within_words", Kind::Bool, "Also expand when the keyword ends a longer word (my;sig)."),
                    row("Speed", "snippets.speed", Kind::Choice(&[("Instant", S("instant")), ("Quick", S("quick")), ("Default", S("default")), ("Delayed", S("delayed")), ("Extended", S("extended"))]), "A slower setting gives slow apps time before the text goes in."),
                    row("Sound", "snippets.sound", Kind::Choice(&[("Off", S("off")), ("Default", S("default")), ("Asterisk", S("asterisk")), ("Exclamation", S("exclamation")), ("Question", S("question"))]), "A Windows sound when a snippet goes in."),
                    row("Not in these apps", "snippets.ignore_apps", Kind::List, "Program file names, e.g. code.exe. Keywords typed there stay as they are."),
                ],
            )]);
            r.extend(table_rows("snippet", text));
            r
        }
        "Quicklinks" => {
            let mut r = groups(vec![(
                "Browser",
                vec![row(
                    "Switch to open tab",
                    "search.existing_tab",
                    Kind::Bool,
                    "A quicklink whose page is already open goes to that tab instead of opening another. Needs the procast browser extension (Set Up Browser Extension).",
                )],
            )]);
            r.extend(table_rows("quicklink", text));
            r
        }
        "Emoji" => groups(vec![(
            "Emoji picker",
            vec![
                row("Hotkey", "emoji.hotkey", Kind::Hotkey, "Opens the emoji picker straight away."),
                row("Columns", "emoji.columns", Kind::Number, "Emoji per row in the picker, 4 to 16."),
                row("Enter", "emoji.action", Kind::Choice(&[("Paste", S("paste")), ("Copy", S("copy"))]), "Paste into the app you came from, or only copy."),
                row("Skin tone", "emoji.skin_tone", Kind::Choice(SKIN_TONES), "Used for every emoji that comes in skin tones."),
                row("Shortcodes anywhere", "emoji.shortcodes", Kind::Bool, "Typing :name: in any app (e.g. :thumbsup:) turns into the emoji."),
            ],
        )]),
        "Window" => groups(vec![
            (
                "Snapping",
                vec![
                    row("Cycle halves", "window.cycle", Kind::Bool, "Pressing Left Half again makes the window ⅔, then ⅓."),
                    row("Window gap", "window.gap", Kind::Number, "Space between snapped windows, in pixels."),
                    row("Screen gap", "window.screen_gap", Kind::Number, "Space between windows and the screen edges, in pixels."),
                ],
            ),
            (
                "Desktops",
                vec![row("Close empty desktops", "window.auto_close_empty_desktops", Kind::Bool, "Remove a desktop with no windows when you switch away from it.")],
            ),
        ]),
        "Files" => groups(vec![
            (
                "File search",
                vec![
                    row("File search", "files.enabled", Kind::Bool, "Find files and folders by name with Search Files."),
                    row("Hotkey", "files.hotkey", Kind::Hotkey, "Opens file search straight away."),
                ],
            ),
            (
                "Index",
                vec![
                    Row { path: vec![], ..row("Status", "", Kind::Status(Live::Index), "") },
                    Row { path: vec![], ..row("Rebuild index", "", Kind::Action("rebuild-index"), "Crawl the folders again and rescan the whole drives.") },
                ],
            ),
            (
                "Folders",
                vec![
                    row("Folders to search", "files.roots", Kind::List, "Full paths, e.g. D:\\Projects. With none, your user folder is searched."),
                    row("Skip names", "files.exclude", Kind::List, "File or folder names skipped wherever they appear, e.g. node_modules or *.log."),
                    row("Respect .gitignore", "files.gitignore", Kind::Bool, "Skip what .gitignore and .ignore files list. Folders only, not whole-drive search."),
                    row("Include hidden files", "files.hidden", Kind::Bool, "Also find files and folders marked hidden, in folder and whole-drive search."),
                ],
            ),
            (
                "Whole-drive search",
                vec![
                    row("Search whole drives", "files.ntfs", Kind::Bool, "Every NTFS drive, kept up to date live. Run \"Enable Fast File Search\" once first (asks for admin)."),
                    row("Drives", "files.drives", Kind::List, "Letters to index, e.g. C and D. With none, every NTFS drive."),
                    row("Skip folders", "files.skip", Kind::List, "Full paths left out with everything under them; also saves memory."),
                    row("Include removable drives", "files.removable", Kind::Bool, "NTFS drives only; network drives aren't searched."),
                ],
            ),
        ]),
        "Theme" => {
            let mut colours = vec![];
            for (label, key) in [
                ("Background", "background"),
                ("Text", "text"),
                ("Muted text", "muted"),
                ("Accent", "selected"),
            ] {
                colours.push(row(label, &format!("theme.{key}"), Kind::Text, "A colour like #1e1e2e. Leave empty to use the theme's own."));
            }
            groups(vec![("Mode", vec![row("Colour mode", "theme.mode", Kind::Choice(COLOUR_MODES), "System follows the light or dark setting of Windows.")]), ("Colours", colours)])
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
                        group: "Extension preferences".into(),
                        icon: "",
                    });
                }
            }
            r
        }
        "Translator" => groups(vec![(
            "Languages",
            vec![
                row("Translate from", "translator.from", Kind::Language(&[("Auto-Detect", S("auto"))]), "The language you type in; Auto-Detect lets Google tell.").with_icon(g::LANGUAGES),
                row("Primary language", "translator.primary", Kind::Language(&[]), "What Translate turns your text into.").with_icon(g::LANGUAGES),
                row("Secondary language", "translator.secondary", Kind::Language(&[("None", S(""))]), "A second translation under the first.").with_icon(g::LANGUAGES),
            ],
        )]),
        "Games" => groups(vec![(
            "Games",
            vec![
                row("Auto HDR", "games.auto_hdr", Kind::Bool, "Turn HDR on while a game has focus and off again after. HDR you turned on yourself stays on.").with_icon(g::SUN_MEDIUM),
                row("Games", "games.list", Kind::List, "Program file names that always count as games, e.g. eldenring.exe. Full-screen D3D apps and borderless Steam, Epic, Xbox and Riot games are found by themselves.").with_icon(g::GAMEPAD_2),
            ],
        )]),
        "Focus" => groups(vec![(
            "Sessions",
            vec![
                row("Default length", "focus.default_minutes", Kind::Number, "Minutes offered when you start a Focus session. 0 means no limit.").with_icon(g::TIMER),
                row(
                    "Snooze duration",
                    "focus.snooze_minutes",
                    Kind::Choice(&[("1 min", N(1)), ("3 min", N(3)), ("5 min", N(5)), ("10 min", N(10))]),
                    "How long Snooze Focus Session lets you use the app Focus just closed.",
                )
                .with_icon(g::COFFEE),
                row("Allow all websites", "focus.allow_all_on_empty_sites", Kind::Bool, "Allow mode with no websites listed: let every site through. Off blocks every site.").with_icon(g::GLOBE),
                row("Reopen closed apps", "focus.restore_apps", Kind::Bool, "When a session ends, start again the apps Focus closed.").with_icon(g::ROTATE_CCW),
                row("Play sound", "focus.sound", Kind::Bool, "Play a sound when a session ends.").with_icon(g::VOLUME_2),
            ],
        )]),
        "Calendar" => groups(vec![(
            "Meetings",
            vec![
                row("Calendar feeds", "calendar.ics", Kind::List, "ICS addresses (https:// or webcal://) of the calendars My Schedule shows, checked every 10 minutes.").with_icon(g::CALENDAR_CLOCK),
                row("Show events within", "calendar.tolerance_secs", Kind::Number, "Seconds around now within which an event shows in an empty search.").with_icon(g::CALENDAR_CLOCK),
                row("Confirm before joining", "calendar.confirm_join", Kind::Bool, "Ask \"Join <title>?\" before opening a meeting link.").with_icon(g::VIDEO),
                row("Remind before", "calendar.remind_minutes", Kind::Number, "Minutes before an event to show a reminder. 0 turns reminders off.").with_icon(g::BELL),
                row("Ignore events", "calendar.ignore", Kind::List, "Events whose title contains one of these words are left out.").with_icon(g::BAN),
            ],
        )]),
        "Shortcuts" | "Screenshots" | "Notes" => vec![],
        _ => {
            let watcher = if crate::foreground::RUNNING.load(std::sync::atomic::Ordering::Relaxed) { "Foreground watcher: running" } else { "Foreground watcher: stopped" };
            vec![
                Row { label: "Open config.toml".into(), help: format!("Every setting in one text file, with comments: {}. Saved changes apply within 2 seconds.", config::path().display()), path: vec![], kind: Kind::Open("config.toml"), group: "Files".into(), icon: "" },
                Row { label: "Open extensions log".into(), help: "Errors and console output from extensions, plus file index status.".into(), path: vec![], kind: Kind::Open("extensions.log"), group: "Files".into(), icon: "" },
                Row { group: "Debugging".into(), path: vec![], ..row(watcher, "", Kind::Status(Live::Foreground), "Tells Auto HDR and Focus which window is in front.").with_icon(g::EYE) },
            ]
        }
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
    /// To tell the launcher what a button asked for.
    tx: UnboundedSender<Msg>,
    /// The file index line, kept current by the launcher.
    index_status: Arc<RwLock<String>>,
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
    /// The caption bar's colours as last set, with Windows' appearance then, so DWM is called only when either changes.
    dark_caption: Option<(u32, u32, gpui::WindowAppearance)>,
    /// Kept apart from `_subs`, which `build_inputs` clears on every page change.
    _appearance: Subscription,
    /// An open language dropdown; it takes the keys first. Its search text is `menu_input`.
    menu: Option<OpenMenu>,
    menu_input: Entity<EditableTextState>,
    _menu_filter: Subscription,
}

struct OpenMenu {
    menu: menu::Menu,
    /// The setting a pick writes.
    path: Vec<String>,
    scroll: ScrollHandle,
}

impl Settings {
    pub fn new(items: Vec<Item>, tx: UnboundedSender<Msg>, index_status: Arc<RwLock<String>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let menu_input = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
        let _menu_filter = cx.subscribe(&menu_input, |this, input, _: &TextChanged, cx| {
            if let Some(open) = &mut this.menu {
                open.menu.set_filter(input.read(cx).as_str());
                cx.notify();
            }
        });
        let mut this = Self {
            menu: None,
            menu_input,
            _menu_filter,
            text: std::fs::read_to_string(config::path()).unwrap_or_default(),
            tx,
            index_status,
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
            dark_caption: None,
            // gpui doesn't repaint on a Windows theme change: without this the content and the
            // caption wait for an unrelated repaint.
            _appearance: cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
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
        rows(self.section, &self.exts, &self.text)
    }

    /// The Shortcuts table: category and filter, then every matching item.
    fn shortcut_rows(&self) -> Vec<Row> {
        let mut out = vec![
            Row { label: "Show".into(), help: "Pick a kind, or press Enter for the next one.".into(), path: vec![], kind: Kind::Category, group: String::new(), icon: "" },
            Row { label: "Find".into(), help: "Type part of a name or alias.".into(), path: vec!["shortcuts-filter".into()], kind: Kind::Filter, group: String::new(), icon: "" },
        ];
        let want = CATEGORIES[self.category];
        let words: Vec<String> = self.filter.split_whitespace().map(str::to_lowercase).collect();
        for (i, it) in self.items.iter().enumerate() {
            let hay = format!("{} {} {}", it.title, it.subtitle, it.alias).to_lowercase();
            if (want == "All" || category(it) == want) && words.iter().all(|w| hay.contains(w.as_str())) {
                let help = [category(it).trim_end_matches('s'), &it.subtitle].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                out.push(Row { label: it.title.clone(), help, path: vec!["launcher".into(), "aliases".into(), key_of(it)], kind: Kind::Command(i), group: if self.category == 0 { "Everything" } else { CATEGORIES[self.category] }.into(), icon: "" });
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
                    _ if multiline(&r.path) => config::shown_value(&self.text, &r.path).replace('\n', "\\n"),
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
                    // Named by its row; a hotkey saved from a Shortcuts row has no row of its own path.
                    let rows = self.rows();
                    let name = rows.iter().find(|r| r.path == path).or(rows.get(self.selected)).map_or_else(|| path.join("."), |r| r.label.clone());
                    self.status = Some((false, format!("{name} saved")));
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
        let mut typed = input.read(cx).as_str().trim().to_string();
        if multiline(&r.path) {
            typed = typed.replace("\\n", "\n");
        }
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

    /// Adds, copies or deletes a snippet or quicklink, then selects what changed.
    fn edit_table(&mut self, list: &'static str, edit: config::TableEdit, window: &mut Window, cx: &mut Context<Self>) {
        let current = std::fs::read_to_string(config::path()).unwrap_or_else(|_| self.text.clone());
        let (verb, removed) = match edit {
            config::TableEdit::Add(_) => ("added", None),
            config::TableEdit::Duplicate(_) => ("copied", None),
            config::TableEdit::Remove(i) => ("deleted", Some(i)),
        };
        let text = match config::edit_table(&current, list, edit).and_then(|t| config::parse(&t).map(|_| t)) {
            Ok(t) => t,
            Err(e) => {
                self.status = Some((true, e));
                return cx.notify();
            }
        };
        if let Err(e) = std::fs::write(config::path(), &text) {
            self.status = Some((true, format!("Could not write config.toml: {e}")));
            return cx.notify();
        }
        self.text = text;
        self.build_inputs(cx);
        let rows = self.rows();
        let name_rows: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| r.path.len() == 3 && r.path[2] == "name").map(|(i, _)| i).collect();
        // A new one is last; after a delete, the one that took its place (or the new last).
        let at = match removed {
            None => name_rows.last(),
            Some(i) => name_rows.get(i).or(name_rows.last()),
        };
        self.selected = at.copied().unwrap_or(0);
        self.status = Some((false, format!("{} {verb}", capital(list))));
        self.focus_row(window, cx);
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
        self.menu = None;
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
                let i = opts.iter().position(|o| o.1.text() == current).map_or(0, |i| (i + 1) % opts.len());
                self.save(&r.path, opts[i].1.value(), cx);
            }
            Kind::Language(lead) => {
                self.menu_input.update(cx, |s, cx| s.emplace("", cx));
                window.focus(&self.menu_input.focus_handle(cx), cx);
                self.menu = Some(OpenMenu { menu: language_menu(lead, &current), path: r.path.clone(), scroll: ScrollHandle::new() });
                cx.notify();
            }
            Kind::Hotkey | Kind::Command(_) => {
                self.commit(cx);
                self.recording = true;
                self.status = Some((false, "Recording a hotkey".into()));
                window.focus(&self.focus, cx);
                cx.notify();
            }
            Kind::Open(file) => crate::win::open(&config::dir().join(file).to_string_lossy()),
            Kind::Text | Kind::Number | Kind::List => self.commit(cx),
            Kind::Category => {
                self.category = (self.category + 1) % CATEGORIES.len();
                cx.notify();
            }
            Kind::TableAdd(list) => {
                let fields: &[(&str, &str)] = if *list == "snippet" { &[("name", "New snippet"), ("keyword", ""), ("text", "")] } else { &[("name", "New quicklink"), ("keyword", ""), ("url", "https://www.google.com/search?q={query}")] };
                self.edit_table(list, config::TableEdit::Add(fields), window, cx);
            }
            Kind::Action("rebuild-index") => drop(self.tx.unbounded_send(Msg::RebuildIndex)),
            Kind::Action(_) | Kind::Status(_) | Kind::Filter | Kind::Keys | Kind::TableItem(..) => {}
        }
    }

    /// Hands a key to the open dropdown; false when none is open.
    fn menu_key(&mut self, key: menu::Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(open) = &mut self.menu else { return false };
        match open.menu.key(key) {
            menu::Event::Picked(i) => self.menu_pick(i, window, cx),
            menu::Event::Closed => self.close_menu(window, cx),
            menu::Event::None => {
                if let Some(k) = open.menu.selected_row() {
                    open.scroll.scroll_to_item(k);
                }
                cx.notify()
            }
        }
        true
    }

    /// Closes the dropdown and saves entry `i` to its setting.
    fn menu_pick(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu.take() else { return };
        self.save(&open.path, open.menu.entries[i].id.as_str().into(), cx);
        self.focus_row(window, cx);
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.focus_row(window, cx);
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

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            return;
        }
        let rows = self.rows();
        let Some(r) = rows.get(self.selected) else { return };
        let k = &ev.keystroke;
        if !self.recording {
            match r.kind {
                Kind::Command(i) if k.modifiers.control && k.key == "d" => self.toggle_enabled(i, cx),
                Kind::TableItem(list, i) if k.modifiers.control && k.key == "d" => self.edit_table(list, config::TableEdit::Duplicate(i), window, cx),
                Kind::TableItem(list, i) if k.key == "delete" && !k.modifiers.modified() => self.edit_table(list, config::TableEdit::Remove(i), window, cx),
                _ => return,
            }
            return cx.stop_propagation();
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
                        Kind::List => "Add an entry…",
                        Kind::Filter => "Filter by name or alias…",
                        Kind::Command(_) => "Alias…",
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
                        // An entry is a Field holding its name and its buttons.
                        design::field(false, t)
                            .pr(px(4.))
                            .gap(px(4.))
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
                                        .hover(|s| s.bg(t.surface_strong))
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
                                    .hover(|s| s.bg(t.surface_strong))
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
            &Kind::Choice(opts) => {
                let me = me.clone();
                let path = r.path.clone();
                let on = opts.iter().position(|o| o.1.text() == value);
                choices(opts.iter().map(|o| o.0), on, stacked(&r.kind), t, &r.path.join("."), move |k, cx| {
                    drop(me.update(cx, |this, cx| this.save(&path, opts[k].1.value(), cx)));
                })
            }
            Kind::Language(lead) => design::dropdown(&language_label(lead, &value), t).into_any_element(),
            Kind::Hotkey => design::hotkey_control(&value, self.recording && on, t).into_any_element(),
            Kind::Open(_) => design::button("Open", design::Button::Quiet, on, t).into_any_element(),
            Kind::Action(_) => design::button("Rebuild", design::Button::Quiet, on, t).into_any_element(),
            Kind::Status(Live::Index) => div().text_size(px(design::ts::BODY_SM)).text_color(t.text_muted).child(self.index_status.read().unwrap().clone()).into_any_element(),
            Kind::Status(Live::Foreground) => div().into_any_element(),
            Kind::TableAdd(_) => design::button("Add", design::Button::Quiet, on, t).into_any_element(),
            &Kind::TableItem(list, i) => {
                let (copy, delete) = (me.clone(), me.clone());
                div()
                    .flex()
                    .gap(px(8.))
                    .child(design::button("Copy", design::Button::Quiet, false, t).id(SharedString::from(format!("copy-{list}-{i}"))).on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        drop(copy.update(cx, |this, cx| this.edit_table(list, config::TableEdit::Duplicate(i), window, cx)))
                    }))
                    .child(design::button("Delete", design::Button::Danger, false, t).id(SharedString::from(format!("delete-{list}-{i}"))).on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        drop(delete.update(cx, |this, cx| this.edit_table(list, config::TableEdit::Remove(i), window, cx)))
                    }))
                    .into_any_element()
            }
            Kind::Category => {
                let me = me.clone();
                choices(CATEGORIES.iter().copied(), Some(self.category), stacked(&r.kind), t, "category", move |k, cx| {
                    drop(me.update(cx, |this, cx| {
                        this.category = k;
                        cx.notify();
                    }));
                })
            }
        }
    }

    /// A Shortcuts row: alias input, hotkey, on/off switch.
    fn command_control(&self, i: usize, r: &Row, on: bool, t: &design::Tokens, me: gpui::WeakEntity<Self>) -> AnyElement {
        let hotkey = self.launcher.hotkeys.get(&key_of(&self.items[i])).cloned().unwrap_or_default();
        let keys = design::hotkey_control(&hotkey, self.recording && on, t).into_any_element();
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
            let (group, start) = (rows[i].group.clone(), i);
            while i < rows.len() && rows[i].group == group {
                i += 1;
            }
            if !rows[start].icon.is_empty() {
                // `.pc-sett__group`: the label inside the surface group, a hairline above every row.
                let label = |d: gpui::Div| {
                    d.child(
                        div()
                            .pt(px(design::sp::S8))
                            .px(px(design::sp::S16))
                            .pb(px(design::sp::S4))
                            .font_family(design::FONT_SMALL)
                            .text_size(px(design::ts::SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text_faint)
                            .child(group.clone()),
                    )
                };
                let labelled = !group.is_empty();
                out.push(
                    div()
                        .flex()
                        .flex_col()
                        .rounded(px(design::r::MD))
                        .bg(t.surface)
                        .overflow_hidden()
                        .when(labelled, label)
                        .children((start..i).map(|j| self.row_view(j, &rows[j], labelled || j > start, t, me)))
                        .into_any_element(),
                );
                continue;
            }
            out.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .when(!group.is_empty(), |d| d.child(div().px(px(4.)).font_family(design::FONT_SMALL).text_size(px(design::ts::SMALL)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_faint).child(group.clone())))
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
        let stack = stacked(&r.kind);
        let recipe = !r.icon.is_empty();
        let base = div()
            .id(("row", i))
            .relative()
            .when(divider, |d| d.border_t_1().border_color(t.hairline))
            .when(on, |d| design::lozenge(d.rounded(px(if recipe { design::r::MD } else { design::r::LG })), t).child(design::pill(t)))
            .on_click(move |_, window, cx| {
                drop(click.update(cx, |this, cx| {
                    let again = this.selected == i;
                    this.select(i, window, cx);
                    let typed = matches!(this.rows()[i].kind, Kind::Text | Kind::Number | Kind::List | Kind::Keys | Kind::Filter | Kind::Command(_) | Kind::Choice(_) | Kind::Category);
                    if again && matches!(this.rows()[i].kind, Kind::Command(_)) || !typed {
                        this.activate(window, cx);
                    }
                }))
            });
        if recipe {
            // A control too wide for the right edge (lists, long segments) wraps under the title.
            let wide = stack || matches!(r.kind, Kind::List | Kind::Keys);
            return base
                .child(design::set_row(r.icon, &r.label, &r.help, t).when(wide, |d| d.flex_wrap()).child(div().when(wide, |d| d.w_full()).flex().items_center().child(self.control(r, on, t, me))))
                .into_any_element();
        }
        base.flex()
            .map(|d| if stack { d.flex_col().gap(px(8.)) } else { d.items_center().gap(px(16.)) })
            .px(px(16.))
            .py(px(12.))
            .when(matches!(r.kind, Kind::Command(c) if self.is_off(c)), |d| d.text_color(t.text_muted))
            .children(match r.kind {
                Kind::Command(c) => Some(design::icon_view(&self.items[c].icon, t, on)),
                _ => None,
            })
            .child(
                div()
                    .map(|d| if stack { d.w_full() } else { d.w(px(if matches!(r.kind, Kind::Command(_)) { 240. } else { 280. })) })
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().child(r.label.clone()))
                    .children((!r.help.is_empty()).then(|| div().text_size(px(design::ts::SMALL)).line_height(px(16.)).text_color(t.text_muted).child(r.help.clone()))),
            )
            .child(div().map(|d| if stack { d.w_full() } else { d.flex_1() }).min_w_0().flex().items_center().child(match r.kind {
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
        // Every mode change re-renders (a save, or the Windows theme through `_appearance`), so this covers them all.
        // Keyed on Windows' appearance too: gpui resets the caption to it when that changes.
        let colorref = |c: gpui::Hsla| {
            let c = gpui::hsla_to_rgba(c);
            let b = |v: f32| (v * 255.).round() as u32;
            b(c.red) | b(c.green) << 8 | b(c.blue) << 16
        };
        let caption = (colorref(t.bg_solid), colorref(t.text), window.appearance());
        if self.dark_caption != Some(caption) {
            crate::win::title_bar(crate::win::hwnd(window), t.dark, caption.0, caption.1);
            self.dark_caption = Some(caption);
        }
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
                "menu": self.menu.as_ref().map(|o| o.menu.visible().into_iter().map(|i| o.menu.entries[i].label.clone()).collect::<Vec<_>>()),
            }),
        );
        let maximized = window.is_maximized();
        let pop = self.menu.as_ref().map(|open| {
            let me = cx.weak_entity();
            let pick = move |i: usize, window: &mut Window, cx: &mut gpui::App| drop(me.update(cx, |this, cx| this.menu_pick(i, window, cx)));
            let me = cx.weak_entity();
            let outside = move |_: &gpui::MouseDownEvent, window: &mut Window, cx: &mut gpui::App| drop(me.update(cx, |this, cx| this.close_menu(window, cx)));
            let field = open.menu.search.as_ref().map(|placeholder| {
                text_input("settings-menu-search")
                    .state(self.menu_input.downgrade())
                    .placeholder(placeholder.clone())
                    .placeholder_color(t.text_faint)
                    .caret_color(t.accent)
                    .flex_1()
                    .whitespace_nowrap()
                    .into_any_element()
            });
            design::pop_menu(&open.menu, field, &open.scroll, pick, outside, &t)
        });
        let heading = match head(SECTIONS[self.section]) {
            // `.pc-sett__head`: the sticker tile, the name, the line on what it does.
            Some((tint, line)) => div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(design::sp::S12))
                .child(design::icon_tile(&Icon::Vector(SECTION_ICONS[self.section]), &t, false, Some(tint)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(design::ts::HEADING)).font_weight(FontWeight::SEMIBOLD).child(SECTIONS[self.section]))
                        .when(!line.is_empty(), |d| d.child(div().text_size(px(design::ts::BODY_SM)).text_color(t.text_muted).child(line))),
                ),
            None => div().h(px(design::ROW)).flex().flex_none().items_center().font_family(design::FONT_DISPLAY).text_size(px(design::ts::DISPLAY)).font_weight(FontWeight::SEMIBOLD).child(SECTIONS[self.section]),
        };
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .font_family(design::FONT_UI)
            .text_size(px(design::ts::BODY))
            .bg(t.bg_solid)
            .text_color(t.text)
            .key_context("Settings")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                if !this.menu_key(menu::Key::Down, window, cx) {
                    this.select(this.selected + 1, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| {
                if !this.menu_key(menu::Key::Up, window, cx) {
                    this.select(this.selected.saturating_sub(1), window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &NextSection, window, cx| this.set_section(this.section + 1, window, cx)))
            .on_action(cx.listener(|this, _: &PrevSection, window, cx| this.set_section(this.section + SECTIONS.len() - 1, window, cx)))
            .on_action(cx.listener(|this, _: &Launch, window, cx| {
                if !this.menu_key(menu::Key::Enter, window, cx) {
                    this.activate(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                if this.menu_key(menu::Key::Esc, window, cx) {
                    return;
                }
                if this.recording {
                    this.recording = false;
                    this.status = Some((false, "Cancelled".into()));
                    cx.notify();
                } else {
                    this.commit(cx);
                    window.remove_window();
                }
            }))
            .child(title_bar(&t, maximized))
            .child(div().flex_1().min_h_0().flex().child(
                div()
                    .id("sections")
                    .w(px(design::SIDEBAR))
                    .flex_none()
                    .overflow_y_scroll()
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
                        design::selectable(design::CONTROL, on, &t)
                            .id(("section", i))
                            .gap(px(8.))
                            .text_color(if on { t.text } else { t.text_muted })
                            .on_click(move |_, window, cx| drop(me.update(cx, |this, cx| this.set_section(i, window, cx))))
                            .child(design::icon(SECTION_ICONS[i], design::ic::MD, if on { t.text } else { t.text_muted }))
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
                            .gap(px(24.))
                            .child(heading)
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
                                Some((error, m)) => d.gap(px(8.)).child(design::status_dot(if *error { t.danger } else { t.success })).child(div().min_w_0().truncate().text_color(t.text).child(m.clone())),
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
            ))
            .children(pop)
    }
}

/// The window's caption bar, drawn in the theme. gpui hides the system one
/// (`appears_transparent`); Windows still drags, snaps, minimises, maximises and closes the
/// window through these control areas.
fn title_bar(t: &design::Tokens, maximized: bool) -> impl IntoElement {
    use gpui::WindowControlArea::{Close, Drag, Max, Min};
    let (hover, ink, muted, danger) = (t.hover, t.text, t.text_muted, t.danger);
    let button = move |id: &'static str, area, glyph: &'static str| {
        // Close turns red under the mouse, as on every Windows caption.
        let (bg, on) = if id == "close" { (danger, gpui::white()) } else { (hover, ink) };
        div()
            .id(id)
            .group(id)
            .w(px(46.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .window_control_area(area)
            .hover(move |s| s.bg(bg))
            .child(design::icon(glyph, design::ic::MD, muted).group_hover(id, move |s| s.text_color(on)))
    };
    // Drag goes on the title only: gpui takes the first control area under the mouse, so a
    // Drag on the whole bar would swallow the buttons.
    div()
        .h(px(design::CONTROL))
        .flex_none()
        .flex()
        .items_center()
        .border_b_1()
        .border_color(t.hairline)
        .child(div().id("drag").window_control_area(Drag).flex_1().min_w_0().h_full().flex().items_center().px(px(12.)).child(div().truncate().text_size(px(design::ts::SMALL)).text_color(muted).child("procast Settings")))
        .child(button("minimise", Min, design::g::MINUS))
        // Two overlapping squares: the restore glyph.
        .child(button("maximise", Max, if maximized { design::g::COPY } else { design::g::SQUARE }))
        .child(button("close", Close, design::g::CANCEL))
}

/// A segmented row (`Choice`, or the Shortcuts `Category`) whose options outrun the control
/// column (more than the spec's five, or long labels) stacks: label and help on top, the track
/// the full row width underneath. Weight is label characters plus 4 per option (its padding);
/// 44 is about the column at the default interface size.
/// ponytail: an estimate, not a measurement; at the large size a row weighing near 44 can still
/// wrap. Measure the column from the viewport if that matters.
fn stacked(kind: &Kind) -> bool {
    let labels: Vec<&str> = match kind {
        Kind::Choice(o) => o.iter().map(|o| o.0).collect(),
        Kind::Category => CATEGORIES.to_vec(),
        _ => return false,
    };
    labels.len() > 5 || labels.iter().map(|l| l.chars().count() + 4).sum::<usize>() > 44
}

/// A segmented control over `labels`, segment `on` lit; clicking one gives `pick` its index.
/// `stretch` spreads the segments over the full width (a stacked row).
fn choices<'a>(labels: impl Iterator<Item = &'a str>, on: Option<usize>, stretch: bool, t: &design::Tokens, id: &str, pick: impl Fn(usize, &mut gpui::App) + Clone + 'static) -> AnyElement {
    design::segment_track(
        labels.enumerate().map(|(k, label)| {
            let pick = pick.clone();
            design::segment(label, on == Some(k), t).when(stretch, |d| d.flex_1().justify_center()).id(SharedString::from(format!("{id}={k}"))).on_click(move |_, _, cx| {
                cx.stop_propagation();
                pick(k, cx)
            })
        }),
        t,
    )
    .when(stretch, |d| d.w_full())
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
    use design::{Tint, g, ic, icon, list_row, section_header};
    // Settings is opaque, so the preview shows the launcher's solid body.
    let t = &design::Tokens { glass: false, ..*t };
    let row = |glyph: &'static str, title: &str, sub: &str, tint: Tint, on: bool| list_row(design::icon_tile(&Icon::Vector(glyph), t, on, Some(tint)), title, sub, None, design::ACCESSORY_MAX, on, t);
    let query = div().flex_1().font_family(design::FONT_DISPLAY).text_size(px(design::ts::QUERY)).text_color(t.text_faint).child("Search apps and commands…");
    let keys = |label: &str, keys: &[&str]| design::shortcut(label, keys, t).into_any_element();
    design::glass_window(t)
        .flex_none()
        .child(design::search_bar(design::search_mark(t), query, [], t))
        .child(div().py(px(design::LIST_PAD)).child(row(g::FOLDER, "Search Files", "Files", Tint::File, false)).child(row(g::PASTE, "Clipboard History", "Clipboard", Tint::Paste, true)).child(row(g::WIN_LEFT_HALF, "Left Half", "Window", Tint::Window, false)).child(section_header("Use \"42\" with", None, t)).child(row(g::GLOBE, "Search the web", "", Tint::Web, false)))
        .child(design::action_bar(icon(g::PASTE, ic::MD, Tint::Paste.hue(t)).into_any_element(), "Command", false, vec![keys("Open", &["↵"]), design::vdivider(t).into_any_element(), keys("Actions", &["Ctrl", "K"])], t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_choice_rows_stack() {
        let labels: Vec<String> = (0..SECTIONS.len()).flat_map(|s| rows(s, &[], "")).filter(|r| stacked(&r.kind)).map(|r| r.label).collect();
        assert_eq!(labels, ["Hyper key", "Keep the last search", "An app's hotkey", "Keep entries for", "Speed", "Sound", "Skin tone"]);
        assert!(stacked(&Kind::Category));
    }

    const FEATURES: [&str; 6] = ["Translator", "Screenshots", "Focus", "Calendar", "Notes", "Games"];

    #[test]
    fn feature_sections_follow_extensions_in_order() {
        let at = SECTIONS.iter().position(|s| *s == "Extensions").unwrap();
        assert_eq!(SECTIONS[at + 1..at + 7], FEATURES);
        assert_eq!(SECTIONS.last(), Some(&"Advanced"));
    }

    #[test]
    fn every_feature_row_has_an_icon() {
        for name in FEATURES {
            let s = SECTIONS.iter().position(|x| *x == name).unwrap();
            for r in rows(s, &[], "") {
                assert!(!r.icon.is_empty(), "{name} › {}", r.label);
            }
        }
        // Not vacuous: the moved groups landed in their sections.
        for (name, label) in [("Games", "Auto HDR"), ("Focus", "Reopen closed apps"), ("Calendar", "Confirm before joining"), ("Translator", "Primary language")] {
            let s = SECTIONS.iter().position(|x| *x == name).unwrap();
            assert!(rows(s, &[], "").iter().any(|r| r.label == label), "{label} in {name}");
        }
        let launcher = rows(SECTIONS.iter().position(|x| *x == "Launcher").unwrap(), &[], "");
        assert!(!launcher.iter().any(|r| ["Games", "Focus", "Calendar"].contains(&r.group.as_str())));
    }

    #[test]
    fn a_minutes_choice_writes_an_integer() {
        let focus = rows(SECTIONS.iter().position(|x| *x == "Focus").unwrap(), &[], "");
        let snooze = focus.iter().find(|r| r.path == ["focus", "snooze_minutes"]).unwrap();
        let Kind::Choice(opts) = snooze.kind else { panic!("Snooze is a Choice") };
        assert_eq!(opts.iter().map(|o| o.0).collect::<Vec<_>>(), ["1 min", "3 min", "5 min", "10 min"]);
        let text = config::write_value("[focus]\n", &snooze.path, opts[1].1.value()).unwrap();
        assert!(text.contains("snooze_minutes = 3\n"), "{text}");
        assert_eq!(config::shown_value(&text, &snooze.path), opts[1].1.text(), "the segment reads back as selected");
        // A string choice stays a string.
        let text = config::write_value("", &["emoji".into(), "action".into()], Val::S("copy").value()).unwrap();
        assert!(text.contains("action = \"copy\""), "{text}");
    }

    #[test]
    fn language_menu_leads_with_its_options_and_selects_the_current_one() {
        let m = language_menu(&[("Auto-Detect", Val::S("auto"))], "de");
        assert_eq!((m.entries[0].label.as_str(), m.entries[0].id.as_str()), ("Auto-Detect", "auto"));
        assert!(m.entries[0].rule_after);
        assert_eq!(m.entries.len(), 1 + crate::translate::LANGUAGES.len());
        assert_eq!(m.picked().map(|e| e.id.as_str()), Some("de"));
        assert!(m.search.is_some());
        assert_eq!(language_label(&[("None", Val::S(""))], ""), "None");
        assert_eq!(language_label(&[], "de"), "German");
    }

    #[test]
    fn foreground_row_reads_the_watcher_state() {
        let advanced = SECTIONS.iter().position(|x| *x == "Advanced").unwrap();
        let label = |on| {
            crate::foreground::RUNNING.store(on, std::sync::atomic::Ordering::Relaxed);
            rows(advanced, &[], "").into_iter().find(|r| r.kind == Kind::Status(Live::Foreground)).map(|r| (r.label, r.icon)).unwrap()
        };
        assert_eq!(label(false), ("Foreground watcher: stopped".into(), design::g::EYE));
        assert_eq!(label(true).0, "Foreground watcher: running");
    }
}
