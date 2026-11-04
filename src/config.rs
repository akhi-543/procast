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
    pub clipboard: Clipboard,
    pub emoji: Emoji,
    pub files: Files,
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
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Clipboard {
    pub enabled: bool,
    pub history: usize,
    /// Opens straight into clipboard history, e.g. "Ctrl+Shift+V". Empty = none.
    pub hotkey: String,
    /// Executable names whose copies are never recorded.
    pub ignore_apps: Vec<String>,
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

pub const POP_TO_ROOT: &[&str] = &["immediately", "15s", "30s", "60s", "120s", "180s", "never"];

/// Keys handled by procast's keyboard hook.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Keyboard {
    /// A key that acts as Ctrl+Alt+Shift+Win while held: off | capslock | rightalt | rightctrl | scrolllock.
    pub hyper: String,
    /// Double-tap this modifier to open procast: off | ctrl | shift | alt.
    pub double_tap: String,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self { hyper: "off".into(), double_tap: "off".into() }
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

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Emoji {
    pub hotkey: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Files {
    pub enabled: bool,
    /// Folders to index. Empty = your user folder.
    pub roots: Vec<String>,
    /// File or folder names skipped anywhere in the tree.
    pub exclude: Vec<String>,
    pub hotkey: String,
    /// Search every NTFS drive through the admin helper, when it's installed.
    pub ntfs: bool,
    /// Whole-drive search: drive letters to index (empty = every NTFS drive).
    pub drives: Vec<String>,
    /// Whole-drive search: full paths left out, with everything under them.
    pub skip: Vec<String>,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: vec![],
            exclude: ["AppData", "node_modules", ".git", "target", ".cargo", ".rustup", "$RECYCLE.BIN", "__pycache__", ".venv", ".gradle", ".npm"]
                .map(String::from)
                .to_vec(),
            hotkey: String::new(),
            ntfs: true,
            drives: vec![],
            skip: [r"C:\Windows", r"C:\Program Files", r"C:\Program Files (x86)", r"C:\ProgramData"].map(String::from).to_vec(),
        }
    }
}

impl Files {
    pub fn roots(&self) -> Vec<String> {
        if self.roots.is_empty() { std::env::var("USERPROFILE").into_iter().collect() } else { self.roots.clone() }
    }
}

/// `mode` picks the base palette; any colour can be overridden with "#rrggbb" or "#rrggbbaa".
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Theme {
    /// system (follow Windows) | dark | light
    pub mode: String,
    /// Named palettes used in dark and light mode (see design::DARK_THEMES / LIGHT_THEMES).
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
    /// May contain `{query}`.
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
    /// Placeholders: {clipboard}, {date}, {time}.
    pub text: String,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self {
            enabled: true,
            history: 500,
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
    /// Rows offered for any query, in order: "web" (the URL above), "chatgpt", "ai", or a quicklink's name.
    pub fallbacks: Vec<String>,
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
            clipboard: Clipboard::default(),
            emoji: Emoji::default(),
            files: Files::default(),
            scripts: Scripts::default(),
            extensions: Extensions::default(),
            theme: Theme::default(),
            keyboard: Keyboard::default(),
            apps: Apps::default(),
            extension: Default::default(),
            quicklink: vec![],
            snippet: vec![],
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

impl Default for Search {
    fn default() -> Self {
        Self { calculator: true, fallback: "https://www.google.com/search?q={query}".into(), fallbacks: ["web", "chatgpt", "ai"].map(String::from).to_vec() }
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

[apps]
hotkey_action = "focus"       # an app's hotkey: launch (new window) | focus (open window first) | toggle (hide if focused)
folders = []                  # extra folders listed as apps, e.g. ["D:\\Portable"]
deep = false                  # also look in their subfolders
auto_quit = []                # program names closed when unused, e.g. ["teams.exe"]
auto_quit_minutes = 60

[search]
calculator = true
fallback = "https://www.google.com/search?q={query}"
fallbacks = ["web", "chatgpt", "ai"]  # rows for any query, in order: "web", "chatgpt", "ai", or a quicklink name with {query}

[ai]
model = "haiku"               # Ask AI, on your Claude Code login: haiku | sonnet | opus

[clipboard]
enabled = true                # text history, encrypted at rest for your Windows account
history = 500
hotkey = "Ctrl+Shift+V"       # opens clipboard history directly; "" for none
ignore_apps = ["keepass.exe", "keepassxc.exe", "1password.exe", "bitwarden.exe"]

[emoji]
hotkey = ""                   # e.g. "Ctrl+Alt+E"

[files]
enabled = true
roots = []                    # empty = your user folder; e.g. ["C:\\Users\\you", "D:\\Projects"]
exclude = ["AppData", "node_modules", ".git", "target", ".cargo", ".rustup", "$RECYCLE.BIN", "__pycache__", ".venv", ".gradle", ".npm"]
hotkey = ""
ntfs = true                   # after "Enable Fast File Search" (admin, once): search every NTFS drive, live
drives = []                   # whole-drive search: e.g. ["C", "D"]; empty = every NTFS drive
skip = ['C:\Windows', 'C:\Program Files', 'C:\Program Files (x86)', 'C:\ProgramData']  # left out of whole-drive search (and memory)

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
dark = "procast Dark"         # procast Dark | Midnight | Graphite | Forest
light = "procast Light"       # procast Light | Paper | Mist
# Any colour can be overridden ("#rrggbb" or "#rrggbbaa"):
# background = "#1e1e2e"
# text = "#cdd6f4"
# muted = "#7f849c"
# selected = "#89b4fa"
# selected_text = "#11111b"
# border = "#313244"
# separator = "#313244"
# container = "#181825"

# Preference values per extension, by the names in its package.json:
# [extension.obsidian]
# notePath = "D:\\Notes"

# Quicklinks: search by name, or type the keyword, a space, then your query.
[[quicklink]]
name = "GitHub Search"
keyword = "gh"
url = "https://github.com/search?q={query}"

[[quicklink]]
name = "YouTube"
keyword = "yt"
url = "https://www.youtube.com/results?search_query={query}"

# Snippets: search by name, Enter pastes. Placeholders: {clipboard} {date} {time}
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

/// The value in effect: the file's, or the default when the key isn't there (older
/// config files lack settings added since).
pub fn shown_value(text: &str, path: &[String]) -> String {
    read_opt(text, path).or_else(|| read_opt(TEMPLATE, path)).unwrap_or_default()
}

fn read_opt(text: &str, path: &[String]) -> Option<String> {
    let doc = text.trim_start_matches('\u{feff}').parse::<toml_edit::DocumentMut>().ok()?;
    let mut item = doc.as_item();
    for key in path {
        item = item.get(key)?;
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

/// Sets one setting in the config text, keeping every comment and the layout; missing
/// tables (e.g. `[extension.youtube]`) are created.
pub fn write_value(text: &str, path: &[String], value: toml_edit::Value) -> Result<String, String> {
    let mut doc = text.trim_start_matches('\u{feff}').parse::<toml_edit::DocumentMut>().map_err(|e| format!("config.toml: {e}"))?;
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
    // Replace in place: the key keeps comments above it, the value its trailing comment.
    if let Some(old) = table.get_mut(last).and_then(|i| i.as_value_mut()) {
        let decor = old.decor().clone();
        *old = value;
        *old.decor_mut() = decor;
    } else {
        table.insert(last, toml_edit::Item::Value(value));
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
    fn template_parses_and_hotkeys() {
        let c = parse(TEMPLATE).unwrap();
        assert_eq!(c.hotkey, "Alt+Space");
        assert_eq!(parse(&format!("\u{feff}{TEMPLATE}")).unwrap(), c, "a byte-order mark is ignored");
        assert_eq!(c.files.skip.first().map(String::as_str), Some(r"C:\Windows"));
        assert_eq!(c.quicklink.len(), 2);
        assert_eq!(c.snippet.len(), 1);
        assert_eq!(c.clipboard.hotkey, "Ctrl+Shift+V");
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

        let t = parse("[theme]\nmode = 'dark'\nselected = '#ff0000'").unwrap().theme;
        assert_eq!((t.mode.as_str(), t.selected.as_deref()), ("dark", Some("#ff0000")));
    }
}
