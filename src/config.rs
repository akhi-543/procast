//! `%APPDATA%\akscast\config.toml`: the only settings store. Unknown keys are ignored,
//! so settings for later phases can already live in the file.

use std::{fs, path::PathBuf, time::SystemTime};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub hotkey: String,
    /// `cursor` (monitor under the mouse) or `primary`.
    pub monitor: String,
    pub width: f32,
    pub max_results: usize,
    pub start_at_login: bool,
    pub search: Search,
    pub clipboard: Clipboard,
    pub emoji: Emoji,
    pub files: Files,
    pub scripts: Scripts,
    pub extensions: Extensions,
    pub theme: Theme,
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
    /// system | dark | light
    pub mode: String,
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
        Self { node: "node".into(), memory_mb: 256, background: true, menu_bar: true }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Scripts {
    /// Folders holding Raycast-format script commands. Empty = %APPDATA%\akscast\scripts.
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
    /// `{query}` is replaced with the URL-encoded query. Empty disables the fallback row.
    pub fallback: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Alt+Space".into(),
            monitor: "cursor".into(),
            width: 750.,
            max_results: 8,
            start_at_login: true,
            search: Search::default(),
            clipboard: Clipboard::default(),
            emoji: Emoji::default(),
            files: Files::default(),
            scripts: Scripts::default(),
            extensions: Extensions::default(),
            theme: Theme::default(),
            extension: Default::default(),
            quicklink: vec![],
            snippet: vec![],
        }
    }
}

impl Default for Search {
    fn default() -> Self {
        Self { calculator: true, fallback: "https://www.google.com/search?q={query}".into() }
    }
}

const TEMPLATE: &str = r##"# akscast settings. Saved changes apply within ~2 seconds.
hotkey = "Alt+Space"          # e.g. "Ctrl+Shift+Space", "Alt+F1"
monitor = "cursor"            # cursor | primary
width = 750
max_results = 8
start_at_login = true

[search]
calculator = true
fallback = "https://www.google.com/search?q={query}"

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

[scripts]
# Raycast-format script commands (.ps1 .py .js .bat .cmd). Empty = %APPDATA%\akscast\scripts
dirs = []

# Raycast extensions: run "Install Extension" and type a store name (e.g. obsidian) or a folder.
[extensions]
node = "node"                 # Node.js 22+ runs the extension host
memory_mb = 256               # heap limit per running command
background = true             # run commands that declare an interval (e.g. every 5m)
menu_bar = true               # show menu-bar commands as tray icons

[theme]
mode = "system"               # system | dark | light
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

pub fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into());
    base.join("akscast")
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
    toml::from_str(text).map_err(|e| format!("config.toml: {}", e.message()))
}

/// One setting's value as the settings window shows it ("" when unset).
pub fn read_value(text: &str, path: &[String]) -> String {
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return String::new() };
    let mut item = doc.as_item();
    for key in path {
        match item.get(key) {
            Some(i) => item = i,
            None => return String::new(),
        }
    }
    match item.as_value() {
        Some(toml_edit::Value::String(s)) => s.value().clone(),
        Some(toml_edit::Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; "),
        Some(v) => v.to_string().trim().to_string(),
        None => String::new(),
    }
}

/// Sets one setting in the config text, keeping every comment and the layout; missing
/// tables (e.g. `[extension.youtube]`) are created.
pub fn write_value(text: &str, path: &[String], value: toml_edit::Value) -> Result<String, String> {
    let mut doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| format!("config.toml: {e}"))?;
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
            "doubletap" => return Err("double-tap hotkeys are not supported yet (Phase 1b)".into()),
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
        assert_eq!(c.quicklink.len(), 2);
        assert_eq!(c.snippet.len(), 1);
        assert_eq!(c.clipboard.hotkey, "Ctrl+Shift+V");
        // Phase 1 `[[alias]]` configs keep working; sections for later phases are ignored.
        let old = parse("[[alias]]\nkeyword = \"gh\"\nurl = \"u\"\n[extensions]\nnode = \"auto\"").unwrap();
        assert_eq!(old.quicklink[0].title(), "gh");
        assert_eq!(parse_hotkey("Alt+Space"), Ok((0x1, 0x20)));
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
        assert!(edited.contains("# akscast settings."));
        let c = parse(&edited).unwrap();
        assert_eq!((c.hotkey.as_str(), c.clipboard.history), ("Ctrl+Alt+K", 100));
        assert_eq!(c.extension["youtube"]["apikey"].as_str(), Some("abc"));
        assert_eq!(read_value(&edited, &p("extension.youtube.apikey")), "abc");
        assert_eq!(read_value(&edited, &p("files.exclude")).split("; ").next(), Some("AppData"));
        assert_eq!(read_value(&edited, &p("nope.missing")), "");

        let t = parse("[theme]\nmode = 'dark'\nselected = '#ff0000'").unwrap().theme;
        assert_eq!((t.mode.as_str(), t.selected.as_deref()), ("dark", Some("#ff0000")));
    }
}
