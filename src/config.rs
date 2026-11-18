//! `%APPDATA%\procast\config.toml`: the only settings store. Unknown keys are ignored,
//! so settings for later phases can already live in the file.

use std::{collections::BTreeMap, fs, path::PathBuf, time::SystemTime};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub hotkey: String,
    /// `cursor` (monitor under the mouse), `active` (monitor of the focused window) or `primary`.
    pub monitor: String,
    pub width: f32,
    pub max_results: usize,
    pub start_at_login: bool,
    /// The procast icon in the notification area.
    pub tray: bool,
    /// Interface size: small | default | large.
    pub size: String,
    /// expanded | compact (only the search bar until you type).
    pub window_mode: String,
    pub launcher: Launcher,
    pub search: Search,
    pub ai: Ai,
    pub translator: Translator,
    pub dictionary: Dictionary,
    pub screenshots: Screenshots,
    pub notes: Notes,
    /// `[[translate]]`: commands that translate the selected text into one language.
    pub translate: Vec<TranslateCmd>,
    /// `[[language_set]]`: the translator's language sets after the first (see `translate::sets`).
    pub language_set: Vec<LanguageSet>,
    pub clipboard: Clipboard,
    pub emoji: Emoji,
    pub ytmusic: YtMusic,
    pub files: Files,
    pub window: Window,
    pub scripts: Scripts,
    pub extensions: Extensions,
    pub theme: Theme,
    pub keyboard: Keyboard,
    pub apps: Apps,
    /// Per-extension preference values: `[extension.obsidian] notePath = "..."`.
    pub extension: std::collections::HashMap<String, toml::Table>,
    /// `[[alias]]` from Phase 1 configs still works.
    #[serde(alias = "alias")]
    pub quicklink: Vec<Quicklink>,
    pub snippet: Vec<Snippet>,
    pub snippets: Snippets,
    pub games: Games,
    pub focus: Focus,
    /// `[[focus_category]]`: named app and website lists a Focus session can use.
    pub focus_category: Vec<FocusCategory>,
    pub calendar: Calendar,
    /// `[[calendar_feed]]`: the calendars My Schedule shows.
    pub calendar_feed: Vec<CalendarFeed>,
}

/// My Schedule: what to do with the feeds' events.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Calendar {
    /// How often the ICS feeds are fetched: 5, 15, 30 or 60 minutes.
    pub refresh_minutes: u32,
    /// Minutes before a meeting with a link to remind you: 0 (off), 2, 5, 10, 15 or 30.
    pub remind_minutes: u32,
    /// Ask "Join <title>?" before opening a meeting link.
    pub confirm_join: bool,
    /// The next meeting within 15 minutes heads root search.
    pub next_in_search: bool,
    /// Events whose title contains one of these words are left out.
    pub ignore: Vec<String>,
}

impl Default for Calendar {
    fn default() -> Self {
        Self { refresh_minutes: 15, remind_minutes: 5, confirm_join: true, next_in_search: true, ignore: vec![] }
    }
}

/// One `[[calendar_feed]]`: a calendar's secret ICS address, its name and colour.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct CalendarFeed {
    /// `https://` or `webcal://`.
    pub url: String,
    pub name: String,
    /// A kind tint: system, window, paste, web or calc.
    pub colour: String,
}

impl Default for CalendarFeed {
    fn default() -> Self {
        Self { url: String::new(), name: String::new(), colour: "system".into() }
    }
}

/// Focus sessions: what they offer and do.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Focus {
    /// Length offered when a session starts, in minutes; 0 = no limit.
    pub default_minutes: u32,
    /// How long Snooze lets you use the app Focus just closed, or a blocked website: 1, 3, 5 or
    /// 10 minutes (anything else reads as 3, `focus::snooze_for`).
    pub snooze_minutes: u8,
    /// A sound when a session's time is up.
    pub sound: bool,
    /// Reopen the apps Focus closed when the session ends.
    pub restore_apps: bool,
    /// Allow mode with no websites listed: allow every site (false: block every site).
    pub allow_all_on_empty_sites: bool,
    /// The floating timer pill while a session runs.
    pub pill: bool,
}

impl Default for Focus {
    fn default() -> Self {
        Self { default_minutes: 25, snooze_minutes: 3, sound: true, restore_apps: false, allow_all_on_empty_sites: false, pill: true }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FocusCategory {
    pub name: String,
    /// Program file names, e.g. "discord.exe".
    pub apps: Vec<String>,
    /// Domains; subdomains match too.
    pub sites: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Clipboard {
    pub enabled: bool,
    /// Entries kept when `keep` is "count".
    pub history: usize,
    /// How long entries are kept: count (the `history` number) | 1h | 1d | 7d | 30d | 90d | 180d | 365d | unlimited.
    pub keep: String,
    /// Find the text in copied images (Windows' own OCR, offline) so search finds them.
    pub ocr: bool,
    /// Opens straight into clipboard history, e.g. "Ctrl+Shift+V". Empty = none.
    pub hotkey: String,
    /// Executable names whose copies are never recorded.
    pub ignore_apps: Vec<String>,
}

/// What counts as a game, and what happens while one has focus.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Games {
    /// Turn HDR on while a game has focus, and off again after (only if procast turned it on).
    pub auto_hdr: bool,
    /// Program file names that always count as games, e.g. ["eldenring.exe"].
    pub list: Vec<String>,
}

/// Snippets typed anywhere turn into their text.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Snippets {
    /// Typing a snippet's keyword in any app replaces it with the snippet.
    pub expand: bool,
    /// immediately | delimiter (after a space or punctuation, which is kept) | delimiter_discard (the delimiter goes).
    pub trigger: String,
    /// Also expand when the keyword ends a longer word ("my;sig").
    pub within_words: bool,
    /// Pause before the text goes in, for slow apps: instant | quick | default | delayed | extended.
    pub speed: String,
    /// off | default | asterisk | exclamation | question (Windows system sounds).
    pub sound: String,
    /// Program file names where typing never expands.
    pub ignore_apps: Vec<String>,
}

impl Default for Snippets {
    fn default() -> Self {
        Self { expand: true, trigger: "immediately".into(), within_words: false, speed: "default".into(), sound: "off".into(), ignore_apps: vec![] }
    }
}

/// Root search behaviour (Raycast's "Launcher" settings).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Launcher {
    /// How long the last search survives closing the window: immediately | 5s … 180s | never.
    pub pop_to_root: String,
    /// low | medium | high: how loose a fuzzy match may be.
    pub sensitivity: String,
    /// Show matching files under root results.
    pub files: bool,
    /// Up arrow in an empty search bar brings back earlier searches.
    pub history: bool,
    /// Items (by key) left out of root search.
    pub hidden: Vec<String>,
    /// Items (by key) pinned to the top of root search.
    pub favorites: Vec<String>,
    /// In compact mode, show favourites as icons under the search bar.
    pub compact_favorites: bool,
    /// Item key -> alias typed in root search to bring it to the top (`cl`).
    pub aliases: BTreeMap<String, String>,
    /// Item key -> global hotkey that runs it.
    pub hotkeys: BTreeMap<String, String>,
    /// Items (by key, or `ext:<name>` for a whole extension) that are switched off everywhere.
    pub disabled: Vec<String>,
    /// Escape inside a command: `back` one level, or `close` the window and pop to root.
    pub escape: String,
    /// Extra list keys: `default`, `emacs` (Alt+N/P move, Alt+B back) or `vim` (Alt+J/K, Alt+H).
    pub navigation: String,
    /// Global hotkeys do nothing while a full-screen app or game has focus.
    pub game_mode: bool,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            pop_to_root: "immediately".into(),
            sensitivity: "medium".into(),
            files: false,
            history: true,
            hidden: vec![],
            favorites: vec![],
            compact_favorites: true,
            aliases: BTreeMap::new(),
            hotkeys: BTreeMap::new(),
            disabled: vec![],
            escape: "back".into(),
            navigation: "default".into(),
            game_mode: false,
        }
    }
}

impl Launcher {
    /// `pop_to_root` as a duration; None = never pop.
    pub fn keep_for(&self) -> Option<std::time::Duration> {
        match self.pop_to_root.as_str() {
            "never" => None,
            s => Some(std::time::Duration::from_secs(s.trim_end_matches('s').parse().unwrap_or(0))),
        }
    }

    /// Switched off, by itself or through its extension.
    pub fn is_disabled(&self, key: &str) -> bool {
        self.disabled.iter().any(|d| d == key || key.strip_prefix(d.as_str()).is_some_and(|rest| d.starts_with("ext:") && rest.starts_with('/')))
    }

    /// Command hotkeys with a key bound, in a stable order.
    pub fn command_hotkeys(&self) -> impl Iterator<Item = (&String, &String)> {
        self.hotkeys.iter().filter(|(k, h)| !h.trim().is_empty() && !self.is_disabled(k))
    }
}

/// Keys handled by procast's keyboard hook.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Keyboard {
    /// A key that acts as Ctrl+Alt+Shift+Win while held: off | capslock | rightalt | rightctrl | scrolllock.
    pub hyper: String,
    /// Double-tap this modifier to open procast: off | ctrl | shift | alt.
    pub double_tap: String,
    /// Pressing the Windows key alone opens procast instead of Start.
    pub win_key: bool,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self { hyper: "off".into(), double_tap: "off".into(), win_key: false }
    }
}

/// Applications in root search.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Apps {
    /// An app's own hotkey: launch (always a new one) | focus (an open window first) | toggle (hide it if focused).
    pub hotkey_action: String,
    /// Extra folders whose programs and shortcuts are listed as apps (portable apps).
    pub folders: Vec<String>,
    /// Also look inside subfolders of `folders`.
    pub deep: bool,
    /// Program file names closed after this many idle minutes, e.g. ["teams.exe"].
    pub auto_quit: Vec<String>,
    pub auto_quit_minutes: u64,
}

impl Default for Apps {
    fn default() -> Self {
        Self { hotkey_action: "focus".into(), folders: vec![], deep: false, auto_quit: vec![], auto_quit_minutes: 60 }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Emoji {
    pub hotkey: String,
    /// Emoji per row in the picker.
    pub columns: usize,
    /// What Enter does: paste (into the app you came from) | copy.
    pub action: String,
    /// none | light | medium_light | medium | medium_dark | dark: used for emoji that come in skin tones.
    pub skin_tone: String,
    /// Typing ":name:" in any app (e.g. ":thumbsup:") turns into the emoji.
    pub shortcodes: bool,
}

impl Default for Emoji {
    fn default() -> Self {
        Self { hotkey: String::new(), columns: 8, action: "paste".into(), skin_tone: "none".into(), shortcodes: true }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct YtMusic {
    pub enabled: bool,
    /// A tray icon with the current track and playback controls.
    pub tray: bool,
    /// Albums per row in the grid.
    pub columns: usize,
    /// Percent that Volume Up and Volume Down change the volume by.
    pub volume_step: u32,
    /// YouTube Music Desktop App companion (ytmdesktop, port 9863).
    pub desktop_url: String,
}

impl Default for YtMusic {
    fn default() -> Self {
        Self {
            enabled: true,
            tray: false,
            columns: 5,
            volume_step: 10,
            desktop_url: "http://127.0.0.1:9863".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Files {
    pub enabled: bool,
    /// Folders to index. Empty = your user folder.
    pub roots: Vec<String>,
    /// File or folder names skipped anywhere in the tree.
    pub exclude: Vec<String>,
    /// Folder search skips what `.gitignore` and `.ignore` files list.
    pub gitignore: bool,
    /// Include files and folders marked hidden.
    pub hidden: bool,
    pub hotkey: String,
    /// Search every NTFS drive through the admin helper, when it's installed.
    pub ntfs: bool,
    /// Whole-drive search: drive letters to index (empty = every NTFS drive).
    pub drives: Vec<String>,
    /// Whole-drive search: full paths left out, with everything under them.
    pub skip: Vec<String>,
    /// Whole-drive search: include removable (USB) NTFS drives.
    pub removable: bool,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: vec![],
            exclude: ["AppData", "node_modules", ".git", "target", ".cargo", ".rustup", "$RECYCLE.BIN", "__pycache__", ".venv", ".gradle", ".npm"]
                .map(String::from)
                .to_vec(),
            gitignore: true,
            hidden: true,
            hotkey: String::new(),
            ntfs: true,
            drives: vec![],
            skip: [r"C:\Windows", r"C:\Program Files", r"C:\Program Files (x86)", r"C:\ProgramData"].map(String::from).to_vec(),
            removable: false,
        }
    }
}

impl Files {
    pub fn roots(&self) -> Vec<String> {
        if self.roots.is_empty() { std::env::var("USERPROFILE").into_iter().collect() } else { self.roots.clone() }
    }
}

/// Window management (the Snap commands and desktop switching).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Window {
    /// Pixels between snapped windows.
    pub gap: u32,
    /// Pixels between snapped windows and the screen edges.
    pub screen_gap: u32,
    /// Repeating Left, Right, Top or Bottom Half cycles through sizes (1/2, 2/3, 1/3).
    pub cycle: bool,
    /// Remove a virtual desktop with no windows when you switch away from it.
    pub auto_close_empty_desktops: bool,
    /// Named window arrangements, each a root-search command "Layout: <name>".
    pub layouts: Vec<Layout>,
}

impl Default for Window {
    fn default() -> Self {
        Self { gap: 0, screen_gap: 0, cycle: true, auto_close_empty_desktops: false, layouts: vec![] }
    }
}

/// A window layout preset: every slot finds (or starts) a program and snaps its window.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Layout {
    pub name: String,
    pub slots: Vec<Slot>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Slot {
    /// Program file name, e.g. "Code.exe".
    pub app: String,
    /// A `Snap` variant name, e.g. "FirstTwoThirds".
    pub snap: String,
    /// 1-based monitor; None keeps the window's own.
    pub display: Option<usize>,
}

/// `mode` picks the base palette; any colour can be overridden with "#rrggbb" or "#rrggbbaa".
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Theme {
    /// system (follow Windows) | dark | light
    pub mode: String,
    /// Old palette names: still parsed so existing files load, no longer used.
    pub dark: String,
    pub light: String,
    pub background: Option<String>,
    pub text: Option<String>,
    pub muted: Option<String>,
    pub selected: Option<String>,
    pub selected_text: Option<String>,
    pub border: Option<String>,
    pub separator: Option<String>,
    pub container: Option<String>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            mode: "system".into(),
            dark: "procast Dark".into(),
            light: "procast Light".into(),
            background: None,
            text: None,
            muted: None,
            selected: None,
            selected_text: None,
            border: None,
            separator: None,
            container: None,
        }
    }
}

/// "#rrggbb" / "#rrggbbaa" -> 0xRRGGBBAA.
pub fn parse_hex(s: &str) -> Option<u32> {
    let h = s.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        6 => Some(v << 8 | 0xff),
        8 => Some(v),
        _ => None,
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Extensions {
    /// Node.js executable for the extension host.
    pub node: String,
    /// Heap limit per running command.
    pub memory_mb: u32,
    /// Run commands that declare an `interval` (e.g. every 5m) in the background.
    pub background: bool,
    /// Show menu-bar commands as tray icons.
    pub menu_bar: bool,
}

impl Default for Extensions {
    fn default() -> Self {
        Self { node: "node".into(), memory_mb: 256, background: true, menu_bar: false }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Scripts {
    /// Folders holding Raycast-format script commands. Empty = %APPDATA%\procast\scripts.
    pub dirs: Vec<String>,
}

impl Scripts {
    pub fn dirs(&self) -> Vec<String> {
        if self.dirs.is_empty() {
            let d = dir().join("scripts");
            let _ = std::fs::create_dir_all(&d);
            vec![d.to_string_lossy().into_owned()]
        } else {
            self.dirs.clone()
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Quicklink {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub keyword: String,
    /// May contain `{query}` (typed after the keyword), `{selection}` (selected in the app you
    /// came from) and `{clipboard}`.
    pub url: String,
}

impl Quicklink {
    pub fn title(&self) -> &str {
        if self.name.is_empty() { &self.keyword } else { &self.name }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Snippet {
    pub name: String,
    #[serde(default)]
    pub keyword: String,
    /// Placeholders: see `snippets::expand`.
    pub text: String,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self {
            enabled: true,
            history: 500,
            keep: "count".into(),
            ocr: true,
            hotkey: String::new(),
            ignore_apps: ["keepass.exe", "keepassxc.exe", "1password.exe", "bitwarden.exe"].map(String::from).to_vec(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Search {
    pub calculator: bool,
    /// `{query}` is replaced with the URL-encoded query. Empty disables the web row.
    pub fallback: String,
    /// Rows offered for any query, in order: "web" (the URL above), "chatgpt", "ai", "define" (one-word queries), or a quicklink's name.
    pub fallbacks: Vec<String>,
    /// What runs a `>` command line: run (Windows Run) | cmd | powershell | pwsh.
    pub shell: String,
    /// Open that shell in Windows Terminal instead of its own console window.
    pub terminal: bool,
    /// A quicklink switches to a browser tab already showing its page (needs the procast extension).
    pub existing_tab: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Alt+Space".into(),
            monitor: "cursor".into(),
            width: 0.5,
            max_results: 8,
            start_at_login: true,
            tray: true,
            size: "default".into(),
            window_mode: "expanded".into(),
            launcher: Launcher::default(),
            search: Search::default(),
            ai: Ai::default(),
            translator: Translator::default(),
            dictionary: Dictionary::default(),
            screenshots: Screenshots::default(),
            notes: Notes::default(),
            translate: vec![],
            language_set: vec![],
            clipboard: Clipboard::default(),
            emoji: Emoji::default(),
            ytmusic: YtMusic::default(),
            files: Files::default(),
            window: Window::default(),
            scripts: Scripts::default(),
            extensions: Extensions::default(),
            theme: Theme::default(),
            keyboard: Keyboard::default(),
            apps: Apps::default(),
            extension: Default::default(),
            quicklink: vec![],
            snippet: vec![],
            snippets: Snippets::default(),
            games: Games::default(),
            focus: Focus::default(),
            focus_category: vec![],
            calendar: Calendar::default(),
            calendar_feed: vec![],
        }
    }
}

/// Quick AI, offered by the "ai" fallback row.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Ai {
    pub model: String,
}

impl Default for Ai {
    fn default() -> Self {
        Self { model: "haiku".into() }
    }
}

/// Screenshot search: the pictures in your screenshot folders, found by the text in them.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Screenshots {
    /// Folders searched. Empty = Pictures\Screenshots.
    pub folders: Vec<String>,
    /// How long a screenshot stays listed: 1h | 1d | 7d | 30d | 90d | 180d | 365d | unlimited.
    pub keep: String,
    /// Find the text in screenshots (Windows' own OCR, offline) so search finds them.
    pub ocr: bool,
    /// Also list videos (their text is not read).
    pub all_media: bool,
    /// Enter: copy | paste (into the app you came from) | open.
    pub primary: String,
    /// Thumbnails per row in Search Screenshots, 3 to 6.
    pub columns: usize,
    /// Also list (and read) OneDrive files kept only in the cloud, which downloads them.
    pub cloud: bool,
}

impl Default for Screenshots {
    fn default() -> Self {
        Self { folders: vec![], keep: "unlimited".into(), ocr: true, all_media: false, primary: "copy".into(), columns: 3, cloud: false }
    }
}

/// Notes: one markdown file per note, shown in the Notes window.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Notes {
    /// The notes folder. Empty = %APPDATA%\procast\notes.
    pub folder: String,
    /// Recently deleted notes are removed after this many days.
    pub keep_deleted_days: u32,
}

impl Default for Notes {
    fn default() -> Self {
        Self { folder: String::new(), keep_deleted_days: 30 }
    }
}

impl Notes {
    /// The notes folder. A relative one would follow the working directory, so the default
    /// stands in for it (`folder_error` says why).
    pub fn dir(&self) -> PathBuf {
        let folder = std::path::Path::new(self.folder.trim());
        if folder.is_absolute() { folder.to_path_buf() } else { dir().join("notes") }
    }

    /// Why the folder set is not used.
    pub fn folder_error(&self) -> Option<&'static str> {
        (!self.folder.trim().is_empty() && !std::path::Path::new(self.folder.trim()).is_absolute()).then_some("Notes folder needs a full path; using the default")
    }

    /// Days a deleted note is kept: at least one, so 0 can't purge every deleted note at startup.
    pub fn keep_days(&self) -> u32 {
        self.keep_deleted_days.max(1)
    }
}

/// The offline dictionary (WordNet): `define dog` in root search.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Dictionary {
    pub enabled: bool,
}

impl Default for Dictionary {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// The translator (Google Translate, no key): `translate hola` in root search, and `[[translate]]` commands.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(from = "TranslatorFile")]
pub struct Translator {
    /// Source language code, or "auto".
    pub from: String,
    /// The first target (see `translate::LANGUAGES`), and the second; empty for none.
    pub primary: String,
    pub secondary: String,
    /// What Enter does with a translation: copy | paste; Ctrl+Enter does the other.
    pub action: String,
    /// "Translate" opens with the text selected in the app you came from.
    pub fill_selection: bool,
    /// A target equal to the source language goes last.
    pub cross_first: bool,
    /// HTTP proxy for Google Translate; empty = none.
    pub proxy: String,
    /// The language set in use: 0 is the one these settings make, i the i-th `[[language_set]]`.
    pub current: usize,
}

impl Default for Translator {
    fn default() -> Self {
        Self { from: "auto".into(), primary: "en".into(), secondary: String::new(), action: "copy".into(), fill_selection: true, cross_first: true, proxy: String::new(), current: 0 }
    }
}

impl Translator {
    /// The language set these settings make: Translate from, then Primary and Secondary.
    pub fn set(&self) -> crate::translate::LangSet {
        crate::translate::LangSet::new(&self.from, &[self.primary.clone(), self.secondary.clone()])
    }
}

/// `[translator]` as written, with the keys configs used before: `to` held the targets,
/// `primary` was Enter's action and `prefill_selection` was `fill_selection`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct TranslatorFile {
    from: String,
    primary: String,
    secondary: String,
    action: String,
    fill_selection: Option<bool>,
    cross_first: Option<bool>,
    proxy: String,
    current: usize,
    #[serde(deserialize_with = "one_or_many")]
    to: Vec<String>,
    prefill_selection: Option<bool>,
}

impl From<TranslatorFile> for Translator {
    fn from(f: TranslatorFile) -> Self {
        let d = Self::default();
        let or = |v: String, d: String| if v.is_empty() { d } else { v };
        let (action, primary) = if matches!(f.primary.as_str(), "copy" | "paste") { (f.primary, String::new()) } else { (f.action, f.primary) };
        let mut to = f.to.into_iter();
        let primary = or(primary, to.next().unwrap_or(d.primary));
        Self {
            from: or(f.from, d.from),
            primary,
            secondary: or(f.secondary, to.next().unwrap_or_default()),
            action: or(action, d.action),
            fill_selection: f.fill_selection.or(f.prefill_selection).unwrap_or(d.fill_selection),
            cross_first: f.cross_first.unwrap_or(d.cross_first),
            proxy: f.proxy,
            current: f.current,
        }
    }
}

/// A `[[language_set]]`: a source language and one or two targets.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LanguageSet {
    #[serde(default = "auto")]
    pub from: String,
    #[serde(default = "english")]
    pub to1: String,
    /// Empty for none.
    #[serde(default)]
    pub to2: String,
}

/// `to = "de"` (how older configs wrote it) or `to = ["en", "de"]`.
fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

/// A `[[translate]]` command: translates the selected text into `to`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TranslateCmd {
    /// Shown instead of "Translate to <language>".
    #[serde(default)]
    pub name: String,
    #[serde(default = "auto")]
    pub from: String,
    #[serde(default = "english")]
    pub to: String,
    /// open (show the translation) | replace (paste it over the selection).
    #[serde(default = "open")]
    pub output: String,
}

fn auto() -> String {
    "auto".into()
}

fn english() -> String {
    "en".into()
}

fn open() -> String {
    "open".into()
}

impl Default for Search {
    fn default() -> Self {
        Self { calculator: true, fallback: "https://www.google.com/search?q={query}".into(), fallbacks: ["web", "chatgpt", "ai"].map(String::from).to_vec(), shell: "run".into(), terminal: false, existing_tab: true }
    }
}

const TEMPLATE: &str = r##"# procast settings. Saved changes apply within ~2 seconds.
hotkey = "Alt+Space"          # e.g. "Ctrl+Shift+Space", "Alt+F1"
monitor = "cursor"            # cursor | active (screen of the focused window) | primary
width = 0.5                   # share of the screen's width (same look on every monitor); above 1 = pixels
max_results = 8
start_at_login = true
tray = true                   # icon in the notification area
size = "default"              # small | default | large
window_mode = "expanded"      # expanded | compact (only the search bar until you type)

[launcher]
pop_to_root = "immediately"   # keep the last search after closing: immediately | 5s | 15s | 30s | 60s | 90s | 120s | 180s | never
sensitivity = "medium"        # low (close matches only) | medium | high (any fuzzy match)
files = false                 # also show matching files in root search
history = true                # Up arrow in an empty search bar restores earlier searches
hidden = []                   # items left out of root search (use Ctrl+K → Hide from Root Search)
favorites = []                # items pinned to the top (Ctrl+K → Add to Favorites)
compact_favorites = true      # compact mode: favourites as icons under the search bar
disabled = []                 # items switched off everywhere; "ext:<name>" for a whole extension
escape = "back"               # Escape in a command: back (one level) | close (hide and pop to root)
navigation = "default"        # default | emacs (Alt+N/P move, Alt+B back) | vim (Alt+J/K move, Alt+H back)
game_mode = false             # hotkeys do nothing while a full-screen app or game is focused
# Aliases and hotkeys for any item, by key (Settings → Shortcuts edits these):
# aliases = { "cmd:ClipboardHistory" = "cl" }
# hotkeys = { "cmd:Snap(LeftHalf)" = "Ctrl+Alt+Left" }

[keyboard]
hyper = "off"                 # a key that acts as Ctrl+Alt+Shift+Win while held: off | capslock | rightalt | rightctrl | scrolllock
double_tap = "off"            # double-tap a modifier to open procast: off | ctrl | shift | alt
win_key = false               # the Windows key pressed alone opens procast instead of Start (Win+E and the like still work)

[apps]
hotkey_action = "focus"       # an app's hotkey: launch (new window) | focus (open window first) | toggle (hide if focused)
folders = []                  # extra folders listed as apps, e.g. ["D:\\Portable"]
deep = false                  # also look in their subfolders
auto_quit = []                # program names closed when unused, e.g. ["teams.exe"]
auto_quit_minutes = 60

[search]
calculator = true
fallback = "https://www.google.com/search?q={query}"
fallbacks = ["web", "chatgpt", "ai"]  # rows for any query, in order: "web", "chatgpt", "ai", "define" (one-word queries), or a quicklink name with {query}
shell = "run"                 # what runs "> command": run (like Win+R) | cmd | powershell | pwsh
terminal = false              # open that shell in Windows Terminal
existing_tab = true           # quicklinks switch to a tab already showing the page (with the procast browser extension)

[ai]
model = "haiku"               # Ask AI, on your Claude Code login: haiku | sonnet | opus

[translator]
from = "auto"            # source language code, or "auto"
primary = "en"           # first target
secondary = ""           # second target, empty for none
action = "copy"          # Enter: "copy" or "paste"; Ctrl+Enter does the other
fill_selection = true    # open Translate with the selected text
cross_first = true       # a target equal to the source goes last
proxy = ""               # HTTP proxy for Google Translate, e.g. http://127.0.0.1:8080
current = 0              # language set in use: 0 is the one above, 1 the first [[language_set]]

# More language sets, picked from the translator's dropdown (Ctrl+P). to2 = "" for none.
# [[language_set]]
# from = "auto"
# to1 = "ja"
# to2 = ""

# A command per language: translates the selected text. output: open (show it) | replace (paste over the selection).
# [[translate]]
# to = "DE"
# output = "replace"

[dictionary]
enabled = true                # define dog: offline WordNet, downloaded once (16 MB) into %APPDATA%\procast\wordnet

[screenshots]
folders = []                  # searched for pictures; empty = Pictures\Screenshots, e.g. ["D:\\Shots"]
keep = "unlimited"            # how long a screenshot stays listed: 1h | 1d | 7d | 30d | 90d | 180d | 365d | unlimited
ocr = true                    # find text in screenshots (Windows' own OCR, offline); off lists them by name only
all_media = false             # also list videos (their text is not read)
primary = "copy"              # Enter: copy | paste (into the app you came from) | open
columns = 3                   # thumbnails per row in Search Screenshots: 3 to 6
cloud = false                 # also list OneDrive files kept only online; reading them downloads them

[notes]
folder = ""                   # one markdown file per note; empty = %APPDATA%\procast\notes
keep_deleted_days = 30        # recently deleted notes are removed after this many days

[clipboard]
enabled = true                # text, images and copied files, encrypted at rest for your Windows account
history = 500                 # entries kept when keep = "count"
keep = "count"                # count | 1h | 1d | 7d | 30d | 90d | 180d | 365d | unlimited (pinned entries always stay)
ocr = true                    # find text inside copied images (Windows' own OCR, offline)
hotkey = "Ctrl+Shift+V"       # opens clipboard history directly; "" for none
ignore_apps = ["keepass.exe", "keepassxc.exe", "1password.exe", "bitwarden.exe"]

[snippets]
expand = true                 # typing a snippet's keyword in any app replaces it with the snippet
trigger = "immediately"       # immediately | delimiter (after a space or punctuation) | delimiter_discard (and drop it)
within_words = false          # also expand when the keyword ends a longer word
speed = "default"             # pause before the text goes in: instant | quick | default | delayed | extended
sound = "off"                 # off | default | asterisk | exclamation | question
ignore_apps = []              # program names where typing never expands, e.g. ["code.exe"]

[games]
auto_hdr = false              # turn HDR on while a game has focus, off again after
list = []                     # program names that always count as games, e.g. ["eldenring.exe"]; full-screen D3D apps and borderless Steam/Epic/Xbox/Riot games are found by themselves

[focus]
default_minutes = 25          # length offered when you start a Focus session; 0 = no limit
snooze_minutes = 3            # how long Snooze lets you use the app Focus just closed, or a blocked website: 1, 3, 5 or 10
sound = true                  # a sound when a session's time is up
restore_apps = false          # reopen the apps Focus closed when the session ends
allow_all_on_empty_sites = false  # allow mode with no websites listed: allow every site (false: block every site)
pill = true                   # false hides the floating Focus timer
# Categories: lists to block (or to allow only) in a session. Apps are program file names; sites match their subdomains too.
# [[focus_category]]
# name = "Social"
# apps = ["discord.exe", "slack.exe"]
# sites = ["reddit.com", "x.com"]

[calendar]
refresh_minutes = 15          # how often the ICS feeds are fetched: 5, 15, 30 or 60
remind_minutes = 5            # notice before meetings with a link: 0 (off), 2, 5, 10, 15 or 30
confirm_join = true           # ask before opening the meeting link
next_in_search = true         # show the next meeting within 15 minutes at the top of search
ignore = []                   # events whose title contains one of these words are hidden, e.g. ["Focus time"]
# Calendars (Google: "Secret address in iCal format"; Outlook: Publish -> ICS; webcal:// works too):
# [[calendar_feed]]
# url = ""                    # the calendar's secret ICS address
# name = "Work"
# colour = "system"           # system, window, paste, web or calc

[emoji]
hotkey = ""                   # e.g. "Ctrl+Alt+E"
columns = 8                   # emoji per row in the picker
action = "paste"              # Enter: paste (into the app you came from) | copy
skin_tone = "none"            # none | light | medium_light | medium | medium_dark | dark
shortcodes = true             # typing :name: in any app (e.g. :thumbsup:) turns into the emoji

[ytmusic]
enabled = true                # search, library and playback through the YouTube Music Desktop App
tray = false                  # a tray icon with the current track and playback controls
columns = 5                   # covers per row in the albums and playlists grid, 3 to 6
volume_step = 10              # percent that Volume Up and Volume Down change the volume by
desktop_url = "http://127.0.0.1:9863"  # YouTube Music Desktop App companion server

[files]
enabled = true
roots = []                    # empty = your user folder; e.g. ["C:\\Users\\you", "D:\\Projects"]
exclude = ["AppData", "node_modules", ".git", "target", ".cargo", ".rustup", "$RECYCLE.BIN", "__pycache__", ".venv", ".gradle", ".npm"]
gitignore = true              # folder search skips what .gitignore and .ignore files list
hidden = true                 # include files and folders marked hidden
hotkey = ""
ntfs = true                   # after "Enable Fast File Search" (admin, once): search every NTFS drive, live
drives = []                   # whole-drive search: e.g. ["C", "D"]; empty = every NTFS drive
skip = ['C:\Windows', 'C:\Program Files', 'C:\Program Files (x86)', 'C:\ProgramData']  # left out of whole-drive search (and memory)
removable = false             # whole-drive search: include removable (USB) NTFS drives (the Fast File Search helper reads this from the config of the account it runs as)

[window]
gap = 0                       # pixels between snapped windows
screen_gap = 0                # pixels between snapped windows and the screen edges
cycle = true                  # repeating Left, Right, Top or Bottom Half cycles through sizes (1/2, 2/3, 1/3)
auto_close_empty_desktops = false  # remove a desktop with no windows when you switch away from it
# Layout presets: each becomes a command "Layout: <name>" that finds (or starts) each app and snaps it.
# app = the program's exe name (or a full path); a Start-menu app is launched from there, else Windows searches PATH.
# Packaged Store apps (other than Windows Terminal, which is built in) can be snapped when already running, but a layout can't start them.
# snap = a Snap name (LeftHalf, RightHalf, FirstTwoThirds, LastThird ...); display = 2 puts it on that monitor.
# [[window.layouts]]
# name = "Coding"
# slots = [
#   { app = "Code.exe", snap = "FirstTwoThirds" },
#   { app = "WindowsTerminal.exe", snap = "LastThird" },
# ]

[scripts]
# Raycast-format script commands (.ps1 .py .js .bat .cmd). Empty = %APPDATA%\procast\scripts
dirs = []

# Raycast extensions: run "Install Extension" and type a store name (e.g. obsidian) or a folder.
[extensions]
node = "node"                 # Node.js 22+ runs the extension host
memory_mb = 256               # heap limit per running command
background = true             # run commands that declare an interval (e.g. every 5m)
menu_bar = false              # also show menu-bar commands as tray icons

[theme]
mode = "system"               # system (follow Windows) | dark | light
# These colours can be overridden ("#rrggbb" or "#rrggbbaa"):
# background = "#1e1e2e"
# text = "#cdd6f4"
# muted = "#7f849c"
# selected = "#89b4fa"

# Preference values per extension, by the names in its package.json:
# [extension.obsidian]
# notePath = "D:\\Notes"

# Quicklinks: search by name, or type the keyword, a space, then your query.
# {query} is what you type; {selection} the text selected in the app you came from; {clipboard} your last copy.
[[quicklink]]
name = "GitHub Search"
keyword = "gh"
url = "https://github.com/search?q={query}"

[[quicklink]]
name = "YouTube"
keyword = "yt"
url = "https://www.youtube.com/results?search_query={query}"

# Snippets: search by name, Enter pastes; or type the keyword in any app. Settings → Snippets edits them.
# Placeholders: {clipboard} {selection} {date} {time} {cursor} {argument name="To" default="you"}
[[snippet]]
name = "Today's date"
keyword = ";date"
text = "{date}"
"##;

fn appdata() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into())
}

pub fn dir() -> PathBuf {
    appdata().join("procast")
}

/// The product's name before it became procast. Only migration code uses it.
pub const OLD_NAME: &str = "akscast";

/// First start after the rename: the old data folder (settings, clipboard history,
/// extensions, caches) becomes procast's, and settings that named the old folder or the old
/// theme names are updated. Nothing happens once the new folder exists.
pub fn migrate_old_data() {
    let (old, new) = (appdata().join(OLD_NAME), dir());
    if new.exists() || !old.exists() {
        return;
    }
    // Renaming fails while the old app still has files open; copy instead then.
    if fs::rename(&old, &new).is_err() && copy_dir(&old, &new).is_err() {
        return;
    }
    let Ok(text) = fs::read_to_string(path()) else { return };
    let (old_s, new_s) = (old.to_string_lossy().into_owned(), new.to_string_lossy().into_owned());
    let fixed = text
        .replace(&format!("\"{OLD_NAME} Dark\""), "\"procast Dark\"")
        .replace(&format!("\"{OLD_NAME} Light\""), "\"procast Light\"")
        .replace(&format!("# {OLD_NAME} settings."), "# procast settings.")
        // Paths are written either escaped ("C:\\Users\\...") or literal ('C:\Users\...').
        .replace(&old_s.replace('\\', "\\\\"), &new_s.replace('\\', "\\\\"))
        .replace(&old_s, &new_s);
    if fixed != text {
        let _ = fs::write(path(), fixed);
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)?.flatten() {
        let target = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            // A locked file (e.g. an open log) is skipped rather than stopping the move.
            let _ = fs::copy(e.path(), target);
        }
    }
    Ok(())
}

pub fn path() -> PathBuf {
    dir().join("config.toml")
}

pub fn modified() -> Option<SystemTime> {
    fs::metadata(path()).and_then(|m| m.modified()).ok()
}

/// Loads the config, writing the commented template on first run.
pub fn load() -> Result<Config, String> {
    let path = path();
    if !path.exists() {
        let _ = fs::create_dir_all(dir());
        let _ = fs::write(&path, TEMPLATE);
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text)
}

pub fn parse(text: &str) -> Result<Config, String> {
    // Notepad and PowerShell can save with a byte-order mark; TOML doesn't allow one.
    toml::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("config.toml: {}", e.message()))
}

/// Whether this config text asks for removable drives in whole-drive search. The elevated
/// helper reads the user's file with this; anything unreadable counts as no.
pub fn removable_wanted(text: &str) -> bool {
    parse(text).is_ok_and(|c| c.files.removable)
}

/// The value in effect: the file's, or the default when the key isn't there (older
/// config files lack settings added since).
pub fn shown_value(text: &str, path: &[String]) -> String {
    // A snippet or quicklink field has no default: the template's own examples are not yours.
    let default = || if table_field(path).is_some() { None } else { read_opt(TEMPLATE, path) };
    let text = if is_translator(path) { migrate_translator(text) } else { text.to_string() };
    read_opt(&text, path).or_else(default).unwrap_or_default()
}

fn is_translator(path: &[String]) -> bool {
    path.first().is_some_and(|k| k == "translator")
}

/// `[translator]` in today's keys: older configs held the targets in `to`, Enter's action in
/// `primary` ("copy" or "paste") and `fill_selection` in `prefill_selection`. Those are
/// rewritten as `Translator` reads them, so a row shows and saves what is in effect; other
/// text, or text that doesn't parse, comes back as it was.
fn migrate_translator(text: &str) -> String {
    let Ok(mut doc) = text.trim_start_matches('\u{feff}').parse::<toml_edit::DocumentMut>() else { return text.into() };
    let Some(table) = doc.get_mut("translator").and_then(|t| t.as_table_mut()) else { return text.into() };
    let old = table.contains_key("to") || table.contains_key("prefill_selection") || table.get("primary").and_then(|p| p.as_str()).is_some_and(|p| matches!(p, "copy" | "paste"));
    let Some(t) = old.then(|| parse(text).ok()).flatten().map(|c| c.translator) else { return text.into() };
    table.remove("to");
    table.remove("prefill_selection");
    for (key, value) in [("from", t.from.into()), ("primary", t.primary.into()), ("secondary", t.secondary.into()), ("action", t.action.into()), ("fill_selection", t.fill_selection.into())] {
        set_keeping_decor(table, key, value);
    }
    doc.to_string()
}

/// Sets `key`; a value already there keeps its trailing comment, the key the comments above it.
fn set_keeping_decor(table: &mut toml_edit::Table, key: &str, value: toml_edit::Value) {
    if let Some(old) = table.get_mut(key).and_then(|i| i.as_value_mut()) {
        let decor = old.decor().clone();
        *old = value;
        *old.decor_mut() = decor;
    } else {
        table.insert(key, toml_edit::Item::Value(value));
    }
}

/// `snippet.2.name`: field `name` of the third `[[snippet]]`.
fn table_field(path: &[String]) -> Option<(&str, usize, &str)> {
    match path {
        [list, i, field] => Some((list.as_str(), i.parse().ok()?, field.as_str())),
        _ => None,
    }
}

fn read_opt(text: &str, path: &[String]) -> Option<String> {
    let doc = text.trim_start_matches('\u{feff}').parse::<toml_edit::DocumentMut>().ok()?;
    let mut item = doc.as_item();
    if let Some((list, i, field)) = table_field(path)
        && let Some(tables) = item.get(list).and_then(|t| t.as_array_of_tables())
    {
        item = tables.get(i)?.get(field)?;
    } else {
        for key in path {
            item = item.get(key)?;
        }
    }
    Some(match item.as_value()? {
        toml_edit::Value::String(s) => s.value().clone(),
        toml_edit::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; "),
        // Without its decor: a trailing comment is not part of the value.
        v => {
            let mut v = v.clone();
            v.decor_mut().clear();
            v.to_string()
        }
    })
}

/// Applies `edit` to the config file at `path`: a missing file counts as empty, one that can't
/// be read (locked, not UTF-8) is left alone; the result must parse and is written atomically.
/// Returns the new settings and text.
pub fn edit_file(path: &std::path::Path, edit: impl FnOnce(&str) -> Result<String, String>) -> Result<(Config, String), String> {
    let text = crate::focus::text_or_empty(fs::read_to_string(path))?;
    let new = edit(&text)?;
    let cfg = parse(&new)?;
    crate::focus::write_atomic(path, &new).map_err(|e| format!("Could not write config.toml: {e}"))?;
    Ok((cfg, new))
}

/// Sets one setting in the config text, keeping every comment and the layout; missing
/// tables (e.g. `[extension.youtube]`) are created.
pub fn write_value(text: &str, path: &[String], value: toml_edit::Value) -> Result<String, String> {
    // A language picked must not overwrite a legacy `primary = "paste"`.
    let text = if is_translator(path) { migrate_translator(text) } else { text.to_string() };
    let mut doc = text.trim_start_matches('\u{feff}').parse::<toml_edit::DocumentMut>().map_err(|e| format!("config.toml: {e}"))?;
    if let Some((list, i, field)) = table_field(path)
        && let Some(tables) = doc.get_mut(list).and_then(|t| t.as_array_of_tables_mut())
    {
        let t = tables.get_mut(i).ok_or(format!("no {list} number {}", i + 1))?;
        set_keeping_decor(t, field, value);
        return Ok(doc.to_string());
    }
    let (last, tables) = path.split_last().ok_or("empty setting path")?;
    let mut table = doc.as_table_mut();
    for key in tables {
        if !table.contains_key(key) {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            table.insert(key, toml_edit::Item::Table(t));
        }
        table = table[key.as_str()].as_table_mut().ok_or(format!("{key} is not a table in config.toml"))?;
    }
    set_keeping_decor(table, last, value);
    Ok(doc.to_string())
}

/// Adds, copies, rewrites or removes one `[[list]]` table (a snippet, quicklink or language
/// set); comments elsewhere stay.
pub enum TableEdit<'a> {
    Add(&'a [(&'a str, &'a str)]),
    Duplicate(usize),
    /// Sets these fields of table i.
    Replace(usize, &'a [(&'a str, &'a str)]),
    Remove(usize),
}

pub fn edit_table(text: &str, list: &str, edit: TableEdit) -> Result<String, String> {
    let mut doc = text.trim_start_matches('\u{feff}').parse::<toml_edit::DocumentMut>().map_err(|e| format!("config.toml: {e}"))?;
    if !doc.contains_key(list) {
        doc.insert(list, toml_edit::Item::ArrayOfTables(Default::default()));
    }
    let tables = doc[list].as_array_of_tables_mut().ok_or(format!("{list} is not a list of [[{list}]] tables in config.toml"))?;
    match edit {
        TableEdit::Add(fields) => {
            let mut t = toml_edit::Table::new();
            for (k, v) in fields {
                t.insert(k, toml_edit::value(*v));
            }
            tables.push(t);
        }
        TableEdit::Duplicate(i) => {
            let mut t = tables.get(i).ok_or("nothing to copy")?.clone();
            t.decor_mut().clear();
            if let Some(name) = t.get("name").and_then(|n| n.as_str()).map(|n| format!("{n} copy")) {
                t.insert("name", toml_edit::value(name));
            }
            tables.push(t);
        }
        TableEdit::Replace(i, fields) => {
            let t = tables.get_mut(i).ok_or("already gone")?;
            for (k, v) in fields {
                set_keeping_decor(t, k, (*v).into());
            }
        }
        TableEdit::Remove(i) if i < tables.len() => _ = tables.remove(i),
        TableEdit::Remove(_) => return Err("already gone".into()),
    }
    Ok(doc.to_string())
}

/// "Ctrl+Alt+Space" -> (MOD_* bits, virtual-key code).
pub fn parse_hotkey(s: &str) -> Result<(u32, u32), String> {
    const ALT: u32 = 0x1;
    const CTRL: u32 = 0x2;
    const SHIFT: u32 = 0x4;
    const WIN: u32 = 0x8;
    let (mut mods, mut vk) = (0, None);
    for part in s.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => mods |= CTRL,
            "alt" => mods |= ALT,
            "shift" => mods |= SHIFT,
            "win" | "super" => mods |= WIN,
            "hyper" => mods |= CTRL | ALT | SHIFT | WIN,
            "doubletap" => return Err("for a double tap, use Settings → General → Double-tap to open".into()),
            key => {
                if vk.is_some() {
                    return Err(format!("hotkey \"{s}\" has more than one main key"));
                }
                vk = Some(key_code(key).ok_or_else(|| format!("unknown key \"{key}\" in hotkey \"{s}\""))?);
            }
        }
    }
    let vk = vk.ok_or_else(|| format!("hotkey \"{s}\" has no main key"))?;
    if mods == 0 && !(0x70..=0x87).contains(&vk) {
        return Err(format!("hotkey \"{s}\" needs a modifier (only F-keys work alone)"));
    }
    Ok((mods, vk))
}

fn key_code(key: &str) -> Option<u32> {
    let b = key.as_bytes();
    if b.len() == 1 && b[0].is_ascii_alphanumeric() {
        return Some(b[0].to_ascii_uppercase() as u32);
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(0x6F + n);
    }
    Some(match key {
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "esc" | "escape" => 0x1B,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" | "ins" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "capslock" => 0x14,
        "`" | "backquote" => 0xC0,
        ";" => 0xBA,
        "," => 0xBC,
        "." => 0xBE,
        "/" => 0xBF,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_settings_that_would_lose_notes_are_refused() {
        let notes = |folder: &str, days| Notes { folder: folder.into(), keep_deleted_days: days };
        assert_eq!(notes("", 0).keep_days(), 1, "0 would purge every deleted note at startup");
        assert_eq!(notes("", 30).keep_days(), 30);
        for relative in ["Notes", "D:", "\\Notes", ".\\n"] {
            assert_eq!((notes(relative, 30).dir(), notes(relative, 30).folder_error().is_some()), (dir().join("notes"), true), "{relative}");
        }
        assert_eq!((notes("D:\\Notes", 30).dir(), notes("D:\\Notes", 30).folder_error()), (PathBuf::from("D:\\Notes"), None));
        assert_eq!(notes("  ", 30).folder_error(), None, "empty is the default, not a mistake");
    }

    #[test]
    fn removable_is_read_from_config_text() {
        assert!(removable_wanted("[files]\nremovable = true\n"));
        assert!(removable_wanted("\u{feff}[files]\nremovable = true # usb\n"), "a byte-order mark is fine");
        assert!(!removable_wanted("[files]\nremovable = false\n"));
        assert!(!removable_wanted(""), "unset is off, as is a missing file's empty text");
        assert!(!removable_wanted(TEMPLATE));
        assert!(!removable_wanted("[files]\nremovable = true\nroots = 3\n"), "an invalid file counts as off");
    }

    #[test]
    fn layouts_parse() {
        let c = parse(
            "[[window.layouts]]\nname = \"Coding\"\nslots = [\n  { app = \"Code.exe\", snap = \"FirstTwoThirds\" },\n  { app = \"WindowsTerminal.exe\", snap = \"LastThird\" },\n]\n",
        )
        .unwrap();
        assert_eq!(c.window.layouts.len(), 1);
        let l = &c.window.layouts[0];
        assert_eq!(l.name, "Coding");
        assert_eq!(l.slots.len(), 2);
        assert_eq!((l.slots[0].app.as_str(), l.slots[0].snap.as_str(), l.slots[0].display), ("Code.exe", "FirstTwoThirds", None));
        let d = parse("[[window.layouts]]\nname = \"X\"\nslots = [{ app = \"a.exe\", snap = \"LeftHalf\", display = 2 }]\n").unwrap();
        assert_eq!(d.window.layouts[0].slots[0].display, Some(2));
        assert!(parse(TEMPLATE).unwrap().window.layouts.is_empty(), "the template's example is commented out");
    }

    #[test]
    fn translator_reads_the_old_keys_too() {
        assert_eq!(parse("").unwrap().translator, Translator::default());
        assert_eq!(parse(TEMPLATE).unwrap().translator, Translator::default());
        let t = |text: &str| parse(&format!("[translator]\n{text}")).unwrap().translator;
        let old = t("to = [\"de\", \"fr\"]\nprimary = \"paste\"\nprefill_selection = false");
        assert_eq!((old.primary.as_str(), old.secondary.as_str(), old.action.as_str(), old.fill_selection), ("de", "fr", "paste", false));
        assert_eq!(t("to = \"DE\"").primary, "DE");
        let new = t("primary = \"ja\"\nsecondary = \"es\"\naction = \"paste\"\ncross_first = false\nproxy = \"http://p:8080\"");
        assert_eq!((new.primary.as_str(), new.secondary.as_str(), new.action.as_str(), new.cross_first, new.proxy.as_str()), ("ja", "es", "paste", false, "http://p:8080"));
        assert!(parse("[translator]
to = 3").is_err());
    }

    #[test]
    fn translator_rows_read_and_write_what_is_in_effect() {
        let p = |s: &str| s.split('.').map(String::from).collect::<Vec<_>>();
        let old = "[translator]\nto = [\"de\", \"fr\"]  # targets\nprimary = \"paste\"\nprefill_selection = false\n";
        assert_eq!(shown_value(old, &p("translator.primary")), "de", "not the default");
        assert_eq!((shown_value(old, &p("translator.secondary")), shown_value(old, &p("translator.action"))), ("fr".into(), "paste".into()));
        assert_eq!(shown_value(old, &p("translator.fill_selection")), "false");
        let new = write_value(old, &p("translator.primary"), "ja".into()).unwrap();
        let t = parse(&new).unwrap().translator;
        assert_eq!((t.primary.as_str(), t.secondary.as_str(), t.action.as_str(), t.fill_selection), ("ja", "fr", "paste", false), "{new}");
        assert!(!new.contains("to =") && !new.contains("prefill_selection"), "{new}");
        let today = "[translator]\nprimary = \"ja\"   # first target\n";
        assert_eq!(write_value(today, &p("translator.secondary"), "es".into()).unwrap(), "[translator]\nprimary = \"ja\"   # first target\nsecondary = \"es\"\n", "today's keys are left alone");
    }

    #[test]
    fn edit_file_never_writes_over_a_file_it_could_not_read() {
        let dir = std::env::temp_dir().join(format!("procast-edit-file-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let current = |t: &str| write_value(t, &["translator".into(), "current".into()], 1i64.into());
        // Not UTF-8 (or locked): an error, and the file stays byte for byte.
        let bytes = b"hotkey = \"Alt+Space\"\n# \xff\xfe\n";
        fs::write(&path, bytes).unwrap();
        assert!(edit_file(&path, current).unwrap_err().starts_with("Could not read config.toml"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        // Read, but an edit that fails (or a result that doesn't parse) writes nothing either.
        fs::write(&path, "hotkey = [\n").unwrap();
        assert!(edit_file(&path, current).is_err());
        assert!(edit_file(&path, |_| Ok("width = \"wide\"".into())).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "hotkey = [\n");
        // A good edit keeps the rest of the file; a missing file counts as empty.
        fs::write(&path, "hotkey = \"Ctrl+K\"  # mine\n").unwrap();
        let (cfg, text) = edit_file(&path, current).unwrap();
        assert_eq!((cfg.hotkey.as_str(), cfg.translator.current), ("Ctrl+K", 1));
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert!(text.contains("# mine"));
        fs::remove_file(&path).unwrap();
        assert_eq!(edit_file(&path, current).unwrap().0.translator.current, 1);
        assert!(!dir.join("config.toml.tmp").exists(), "the temp file is renamed into place");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_deepl_config_still_loads() {
        let c = parse("[translator]
deepl_key = \"abc:fx\"
from = \"auto\"
to = \"EN-US\"
primary = \"paste\"

[[translate]]
to = \"PT-BR\"").unwrap();
        assert_eq!((c.translator.primary.as_str(), c.translator.action.as_str()), ("EN-US", "paste"));
        assert_eq!(c.translate[0].to, "PT-BR");
        assert!(!TEMPLATE.to_lowercase().contains("deepl"));
    }

    #[test]
    fn template_parses_and_hotkeys() {
        let c = parse(TEMPLATE).unwrap();
        assert_eq!(c.hotkey, "Alt+Space");
        assert_eq!(parse(&format!("\u{feff}{TEMPLATE}")).unwrap(), c, "a byte-order mark is ignored");
        assert_eq!(c.files.skip.first().map(String::as_str), Some(r"C:\Windows"));
        assert_eq!(c.quicklink.len(), 2);
        assert_eq!(c.snippet.len(), 1);
        assert_eq!(c.clipboard.hotkey, "Ctrl+Shift+V");
        assert_eq!(c.search.fallbacks, Search::default().fallbacks, "Define is offered only when listed");
        // Phase 1 `[[alias]]` configs keep working; sections for later phases are ignored.
        let old = parse("[[alias]]\nkeyword = \"gh\"\nurl = \"u\"\n[extensions]\nnode = \"auto\"").unwrap();
        assert_eq!(old.quicklink[0].title(), "gh");
        assert_eq!(parse_hotkey("Alt+Space"), Ok((0x1, 0x20)));
        let key = |s: &str| s.split('.').map(String::from).collect::<Vec<_>>();
        assert_eq!(shown_value("width = 0.49   # share of the screen", &key("width")), "0.49", "no trailing comment");
        assert_eq!(shown_value("[launcher]\nfiles = true", &key("launcher.history")), "true", "missing keys show their default");
        assert_eq!(parse_hotkey("ctrl+shift+k"), Ok((0x2 | 0x4, b'K' as u32)));
        assert_eq!(parse_hotkey("F9"), Ok((0, 0x78)));
        assert!(parse_hotkey("Space").is_err());
        assert!(parse_hotkey("Ctrl+A+B").is_err());
        assert!(parse_hotkey("DoubleTap+Win").is_err());
        assert_eq!(parse_hex("#1e1e2e"), Some(0x1e1e2eff));
        assert_eq!(parse_hex("#11223344"), Some(0x11223344));
        assert_eq!(parse_hex("1e1e2e"), None);
        // Settings window writes keep comments and create missing tables.
        let p = |s: &str| s.split('.').map(String::from).collect::<Vec<_>>();
        let edited = write_value(TEMPLATE, &p("hotkey"), "Ctrl+Alt+K".into()).unwrap();
        let edited = write_value(&edited, &p("clipboard.history"), 100i64.into()).unwrap();
        let edited = write_value(&edited, &p("extension.youtube.apikey"), "abc".into()).unwrap();
        assert!(edited.contains("hotkey = \"Ctrl+Alt+K\"          # e.g."), "trailing comment kept:\n{edited}");
        assert!(edited.contains("# procast settings."));
        let c = parse(&edited).unwrap();
        assert_eq!((c.hotkey.as_str(), c.clipboard.history), ("Ctrl+Alt+K", 100));
        assert_eq!(c.extension["youtube"]["apikey"].as_str(), Some("abc"));
        assert_eq!(shown_value(&edited, &p("extension.youtube.apikey")), "abc");
        assert_eq!(shown_value(&edited, &p("files.exclude")).split("; ").next(), Some("AppData"));
        assert_eq!(read_opt(&edited, &p("nope.missing")), None);

        // Snippets and quicklinks are edited one field, one table at a time.
        assert_eq!(shown_value(&edited, &p("snippet.0.keyword")), ";date");
        let edited = write_value(&edited, &p("snippet.0.text"), "{time}".into()).unwrap();
        let edited = edit_table(&edited, "snippet", TableEdit::Duplicate(0)).unwrap();
        let edited = edit_table(&edited, "snippet", TableEdit::Add(&[("name", "Sig"), ("keyword", ";sig"), ("text", "Cheers")])).unwrap();
        let c = parse(&edited).unwrap();
        assert_eq!(c.snippet.iter().map(|s| (s.name.as_str(), s.text.as_str())).collect::<Vec<_>>(), [("Today's date", "{time}"), ("Today's date copy", "{time}"), ("Sig", "Cheers")]);
        let edited = edit_table(&edited, "snippet", TableEdit::Remove(0)).unwrap();
        assert_eq!(parse(&edited).unwrap().snippet.len(), 2);
        assert!(edited.contains("# Quicklinks: search by name"), "comments elsewhere survive:\n{edited}");
        let none = edit_table("", "quicklink", TableEdit::Add(&[("name", "X"), ("url", "https://x")])).unwrap();
        assert_eq!(parse(&none).unwrap().quicklink[0].url, "https://x");

        let t = parse("[theme]\nmode = 'dark'\nselected = '#ff0000'").unwrap().theme;
        assert_eq!((t.mode.as_str(), t.selected.as_deref()), ("dark", Some("#ff0000")));
    }
}
