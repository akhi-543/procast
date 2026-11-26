//! Raycast extensions: installed manifests, the Node extension host (exthost/), and the
//! JSON tree it renders, parsed into a small view model the launcher draws.

use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    time::Duration,
};

use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};

use crate::{
    search::{self, ArgKind},
    win::Msg,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Pref {
    pub name: String,
    pub title: String,
    pub kind: String,
    pub required: bool,
    pub default: Value,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExtCommand {
    pub name: String,
    pub title: String,
    pub mode: String,
    pub interval: Option<Duration>,
    pub prefs: Vec<Pref>,
    /// The manifest's `arguments`, as boxes in the search bar.
    pub args: Vec<search::Arg>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Extension {
    pub name: String,
    pub title: String,
    pub dir: PathBuf,
    pub icon: Option<PathBuf>,
    pub prefs: Vec<Pref>,
    pub commands: Vec<ExtCommand>,
}

pub fn dir() -> PathBuf {
    crate::config::dir().join("extensions")
}

pub fn support_dir(name: &str) -> PathBuf {
    crate::config::dir().join("extensions-data").join(name)
}

fn prefs(v: &Value) -> Vec<Pref> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|p| Pref {
            name: p["name"].as_str().unwrap_or_default().into(),
            title: p["title"].as_str().or(p["label"].as_str()).unwrap_or_default().into(),
            kind: p["type"].as_str().unwrap_or("textfield").into(),
            required: p["required"].as_bool().unwrap_or(false),
            default: p["default"].clone(),
            description: p["description"].as_str().unwrap_or_default().into(),
        })
        .collect()
}

/// Raycast's command `arguments`: `{name, placeholder, type: text | password | dropdown,
/// required, data: [{title, value}]}`.
fn arguments(v: &Value) -> Vec<search::Arg> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let name = a["name"].as_str()?.to_string();
            let kind = match a["type"].as_str() {
                Some("password") => ArgKind::Secret,
                Some("dropdown") => ArgKind::Choice(a["data"].as_array().into_iter().flatten().filter_map(|d| Some((d["value"].as_str()?.to_string(), d["title"].as_str()?.to_string()))).collect()),
                _ => ArgKind::Text,
            };
            let placeholder = a["placeholder"].as_str().unwrap_or(&name).to_string();
            Some(search::Arg { missing: format!("Type {}", placeholder.to_lowercase()), placeholder: placeholder.into(), kind, optional: !a["required"].as_bool().unwrap_or(false), ..search::Arg::new(name) })
        })
        .collect()
}

/// "5m", "30s", "1h", "1d" -> Duration. Raycast's minimum is 10 s.
pub fn parse_interval(s: &str) -> Option<Duration> {
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = n.parse().ok()?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        _ => return None,
    };
    Some(Duration::from_secs(secs.max(10)))
}

pub fn parse_manifest(dir: &Path, v: &Value) -> Option<Extension> {
    let name = v["name"].as_str()?.to_string();
    let commands = v["commands"]
        .as_array()?
        .iter()
        .filter_map(|c| {
            Some(ExtCommand {
                name: c["name"].as_str()?.into(),
                title: c["title"].as_str()?.into(),
                mode: c["mode"].as_str().unwrap_or("view").into(),
                interval: c["interval"].as_str().and_then(parse_interval),
                prefs: prefs(&c["preferences"]),
                args: arguments(&c["arguments"]),
            })
        })
        .collect();
    let icon = v["icon"].as_str().map(|i| dir.join("assets").join(i)).filter(|p| p.exists());
    Some(Extension { title: v["title"].as_str().unwrap_or(&name).into(), name, dir: dir.into(), icon, prefs: prefs(&v["preferences"]), commands })
}

/// Installed extensions: every built folder under %APPDATA%\procast\extensions.
pub fn scan() -> Vec<Extension> {
    let mut out: Vec<Extension> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path().join("package.json")).ok()?;
            parse_manifest(&e.path(), &serde_json::from_str(&text).ok()?)
        })
        .collect();
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
}

/// Preference values for one command: config.toml `[extension.<name>]` over manifest defaults.
/// Errs with the names of required preferences that have no value.
pub fn preference_values(ext: &Extension, cmd: &ExtCommand, configured: Option<&toml::Table>) -> Result<Value, Vec<String>> {
    let mut out = serde_json::Map::new();
    let mut missing = vec![];
    for p in ext.prefs.iter().chain(&cmd.prefs) {
        let v = configured
            .and_then(|t| t.get(&p.name))
            .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
            .filter(|v| !v.is_null())
            .unwrap_or_else(|| match (&p.default, p.kind.as_str()) {
                (Value::Null, "checkbox") => Value::Bool(false),
                (Value::Null, _) => Value::String(String::new()),
                (d, _) => d.clone(),
            });
        if p.required && v.as_str() == Some("") {
            missing.push(p.name.clone());
        }
        out.insert(p.name.clone(), v);
    }
    if missing.is_empty() { Ok(Value::Object(out)) } else { Err(missing) }
}

// ---- store --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct StoreEntry {
    pub name: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub windows: bool,
    pub commands: usize,
}

/// The shared sparse clone of raycast/extensions that installs and the store use.
pub fn store_repo() -> PathBuf {
    dir().join(".src").join("raycast-extensions")
}

/// Every extension manifest in the store index, sorted by title.
pub fn read_store(repo: &Path) -> Vec<StoreEntry> {
    let mut out: Vec<StoreEntry> = std::fs::read_dir(repo.join("extensions"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(e.path().join("package.json")).ok()?).ok()?;
            let platforms: Vec<String> = v["platforms"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_lowercase)).collect();
            Some(StoreEntry {
                name: v["name"].as_str()?.into(),
                title: v["title"].as_str().unwrap_or_default().into(),
                description: v["description"].as_str().unwrap_or_default().into(),
                author: v["author"].as_str().unwrap_or_default().into(),
                // A missing `platforms` field means macOS only.
                windows: platforms.iter().any(|p| p == "windows"),
                commands: v["commands"].as_array().map_or(0, Vec::len),
            })
        })
        .collect();
    out.sort_by_key(|a| a.title.to_lowercase());
    out
}

// ---- host process -------------------------------------------------------------------

/// Where exthost/ lives: next to the exe (installed), or the repo root (dev builds).
pub fn host_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().skip(1).take(4).map(|d| d.join("exthost")).find(|d| d.join("host.mjs").exists())
}

pub struct Host {
    _child: Child,
    stdin: ChildStdin,
}

impl Host {
    pub fn spawn(node: &str, tx: UnboundedSender<Msg>) -> Result<Self, String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let dir = host_dir().ok_or("exthost folder not found next to procast.exe")?;
        let mut child = Command::new(node)
            .arg(dir.join("host.mjs"))
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("could not start Node.js ({node}): {e}. Install Node 22+ or set [extensions] node in config.toml"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && tx.unbounded_send(Msg::Ext(v)).is_err()
                {
                    return;
                }
            }
            let _ = tx.unbounded_send(Msg::Ext(json!({"t": "hostExit"})));
        });
        Ok(Self { _child: child, stdin })
    }

    pub fn send(&mut self, msg: Value) -> bool {
        writeln!(self.stdin, "{msg}").and_then(|_| self.stdin.flush()).is_ok()
    }
}

/// The `run` message that starts a command in a worker.
pub fn run_msg(run: u64, ext: &Extension, cmd: &ExtCommand, prefs: Value, args: Value, background: bool, memory_mb: u32) -> Value {
    json!({
        "t": "run", "run": run, "mode": cmd.mode, "memoryMb": memory_mb,
        "entry": ext.dir.join(format!("{}.js", cmd.name)),
        "arguments": args, "preferences": prefs,
        "launchType": if background { "background" } else { "userInitiated" },
        "environment": {
            "extensionName": ext.name, "commandName": cmd.name, "commandMode": cmd.mode,
            "assetsPath": ext.dir.join("assets"), "supportPath": support_dir(&ext.name),
            "isDevelopment": false, "raycastVersion": "2.10.1", "appearance": "dark", "theme": "dark", "textSize": "medium",
        },
    })
}

// ---- view model ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ExtAction {
    pub title: String,
    pub on_action: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ExtItem {
    pub title: String,
    pub subtitle: String,
    pub accessories: Vec<String>,
    pub keywords: Vec<String>,
    pub icon: Option<String>,
    pub section: Option<String>,
    pub detail: Option<String>,
    /// List.Item.Detail metadata rows.
    pub detail_meta: Vec<Meta>,
    pub actions: Vec<ExtAction>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Text,
    Password,
    TextArea,
    Checkbox,
    Dropdown(Vec<(String, String)>),
    Description,
    Separator,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub id: String,
    pub kind: FieldKind,
    pub title: String,
    pub placeholder: String,
    /// Current value from the extension (controlled `value`, else `defaultValue`).
    pub value: Value,
    pub controlled: bool,
    pub on_change: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct View {
    /// "List", "Grid", "Detail", "Form" (or "" before the first render).
    pub kind: String,
    pub title: Option<String>,
    pub loading: bool,
    pub placeholder: Option<String>,
    /// The extension filters itself (onSearchTextChange without filtering=true).
    pub on_search: Option<String>,
    /// `pagination.onLoadMore` while `hasMore`: called as the list nears its end.
    pub load_more: Option<String>,
    pub search_text: Option<String>,
    pub show_detail: bool,
    pub items: Vec<ExtItem>,
    pub empty: Option<(String, String)>,
    pub markdown: String,
    pub metadata: Vec<Meta>,
    pub actions: Vec<ExtAction>,
    pub fields: Vec<Field>,
    pub dropdown: Option<Dropdown>,
}

/// A search-bar dropdown: options as (value, title), the current value, and the field it sets.
pub type Dropdown = (Vec<(String, String)>, String, Option<String>);

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Object(o) => o.get("value").or(o.get("text")).map(s).unwrap_or_default(),
        _ => String::new(),
    }
}

fn fn_id(v: &Value) -> Option<String> {
    v.get("$fn").and_then(Value::as_str).map(str::to_string)
}

fn kids(n: &Value) -> impl Iterator<Item = &Value> {
    n["children"].as_array().into_iter().flatten().filter(|c| c.is_object())
}

fn slot<'a>(n: &'a Value, name: &str) -> Option<&'a Value> {
    kids(n).find(|c| c["type"] == "slot" && c["props"]["name"] == name).and_then(|c| kids(c).next())
}

fn icon(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Object(o) => o.get("source").or(o.get("dark")).or(o.get("value")).and_then(icon),
        _ => None,
    }
}

/// Flattens an ActionPanel: sections inline, submenus as "Submenu › Item".
fn actions(panel: Option<&Value>) -> Vec<ExtAction> {
    let mut out = vec![];
    fn walk(n: &Value, prefix: &str, out: &mut Vec<ExtAction>) {
        for c in kids(n) {
            match c["type"].as_str().unwrap_or_default() {
                "Action" => out.push(ExtAction { title: format!("{prefix}{}", s(&c["props"]["title"])), on_action: fn_id(&c["props"]["onAction"]) }),
                "ActionPanel.Submenu" => walk(c, &format!("{prefix}{} › ", s(&c["props"]["title"])), out),
                _ => walk(c, prefix, out),
            }
        }
    }
    if let Some(p) = panel {
        walk(p, "", &mut out);
    }
    out
}

fn accessory(a: &Value) -> Option<String> {
    let t = s(&a["text"]);
    if !t.is_empty() {
        return Some(t);
    }
    let tag = s(&a["tag"]);
    if !tag.is_empty() {
        return Some(tag);
    }
    a["date"].as_str().map(|d| d.chars().take(10).collect())
}

/// A Detail.Metadata row: a Label's or Link's text, or a TagList's tags (text, Raycast colour
/// such as "color:Red", empty for none).
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub title: String,
    pub text: String,
    pub tags: Vec<(String, String)>,
}

impl Meta {
    pub fn label(title: impl Into<String>, text: impl Into<String>) -> Self {
        Self { title: title.into(), text: text.into(), tags: vec![] }
    }
}

/// Metadata.Label / Link / TagList rows; separators and untitled rows drop.
fn metadata(m: &Value) -> Vec<Meta> {
    kids(m)
        .filter_map(|c| {
            let p = &c["props"];
            let title = s(&p["title"]);
            if title.is_empty() {
                return None;
            }
            Some(match c["type"].as_str()? {
                "Metadata.TagList" => Meta { title, text: String::new(), tags: kids(c).map(|t| (s(&t["props"]["text"]), s(&t["props"]["color"]))).collect() },
                _ => Meta::label(title, s(&p["text"])),
            })
        })
        .collect()
}

fn item(n: &Value, section: Option<&str>) -> ExtItem {
    let p = &n["props"];
    let detail_node = slot(n, "detail");
    let detail = detail_node.map(|d| s(&d["props"]["markdown"]));
    let detail_meta = detail_node.and_then(|d| slot(d, "metadata")).map(metadata).unwrap_or_default();
    ExtItem {
        detail_meta,
        title: s(&p["title"]),
        subtitle: s(&p["subtitle"]),
        accessories: p["accessories"].as_array().into_iter().flatten().filter_map(accessory).collect(),
        keywords: p["keywords"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_string)).collect(),
        icon: icon(&p["icon"]).or_else(|| icon(&p["content"])),
        section: section.map(str::to_string),
        detail,
        actions: actions(slot(n, "actions")),
    }
}

fn field(n: &Value) -> Option<Field> {
    let p = &n["props"];
    let ty = n["type"].as_str()?.strip_prefix("Form.")?;
    let kind = match ty {
        "TextField" | "DatePicker" | "FilePicker" | "TagPicker" => FieldKind::Text,
        "PasswordField" => FieldKind::Password,
        "TextArea" => FieldKind::TextArea,
        "Checkbox" => FieldKind::Checkbox,
        "Dropdown" => {
            let mut opts = vec![];
            fn collect(n: &Value, opts: &mut Vec<(String, String)>) {
                for c in kids(n) {
                    if c["type"] == "Form.Dropdown.Item" {
                        opts.push((s(&c["props"]["value"]), s(&c["props"]["title"])));
                    } else {
                        collect(c, opts);
                    }
                }
            }
            collect(n, &mut opts);
            FieldKind::Dropdown(opts)
        }
        "Description" => FieldKind::Description,
        "Separator" => FieldKind::Separator,
        _ => return None,
    };
    let controlled = !p["value"].is_null();
    let mut value = if controlled { p["value"].clone() } else { p["defaultValue"].clone() };
    if value.is_null() {
        value = match &kind {
            FieldKind::Checkbox => Value::Bool(false),
            FieldKind::Dropdown(opts) => Value::String(opts.first().map(|o| o.0.clone()).unwrap_or_default()),
            FieldKind::Description => Value::String(s(&p["text"])),
            _ => Value::String(String::new()),
        };
    }
    Some(Field {
        id: s(&p["id"]),
        title: if matches!(kind, FieldKind::Checkbox) { s(&p["label"]) } else { s(&p["title"]) },
        placeholder: s(&p["placeholder"]),
        kind,
        value,
        controlled,
        on_change: fn_id(&p["onChange"]),
        error: Some(s(&p["error"])).filter(|e| !e.is_empty()),
    })
}

/// The top-level element of a rendered page -> view model.
pub fn parse_view(tree: &Value) -> View {
    let Some(root) = tree.as_array().into_iter().flatten().find(|n| n.is_object()) else { return View::default() };
    let p = &root["props"];
    let kind = root["type"].as_str().unwrap_or_default().to_string();
    let mut v = View {
        kind: kind.clone(),
        title: p["navigationTitle"].as_str().map(str::to_string),
        loading: p["isLoading"].as_bool().unwrap_or(false),
        placeholder: p["searchBarPlaceholder"].as_str().map(str::to_string),
        search_text: p["searchText"].as_str().map(str::to_string),
        show_detail: p["isShowingDetail"].as_bool().unwrap_or(false),
        on_search: fn_id(&p["onSearchTextChange"]).filter(|_| !p["filtering"].as_bool().unwrap_or(false)),
        load_more: fn_id(&p["pagination"]["onLoadMore"]).filter(|_| p["pagination"]["hasMore"].as_bool().unwrap_or(false)),
        actions: actions(slot(root, "actions")),
        ..View::default()
    };
    match kind.as_str() {
        "List" | "Grid" => {
            fn walk(n: &Value, section: Option<&str>, v: &mut View) {
                for c in kids(n) {
                    match c["type"].as_str().unwrap_or_default() {
                        "List.Item" | "Grid.Item" => v.items.push(item(c, section)),
                        "List.Section" | "Grid.Section" => walk(c, Some(&s(&c["props"]["title"])), v),
                        "List.EmptyView" => {
                            v.empty = Some((s(&c["props"]["title"]), s(&c["props"]["description"])));
                            v.actions = actions(slot(c, "actions"));
                        }
                        _ => {}
                    }
                }
            }
            walk(root, None, &mut v);
            if let Some(d) = slot(root, "searchBarAccessory") {
                let mut opts = vec![];
                fn collect(n: &Value, opts: &mut Vec<(String, String)>) {
                    for c in kids(n) {
                        if c["type"] == "List.Dropdown.Item" {
                            opts.push((s(&c["props"]["value"]), s(&c["props"]["title"])));
                        } else {
                            collect(c, opts);
                        }
                    }
                }
                collect(d, &mut opts);
                let cur = s(&d["props"]["value"]);
                let cur = if cur.is_empty() { s(&d["props"]["defaultValue"]) } else { cur };
                v.dropdown = Some((opts, cur, fn_id(&d["props"]["onChange"])));
            }
        }
        "Detail" => {
            v.markdown = s(&p["markdown"]);
            if let Some(m) = slot(root, "metadata") {
                v.metadata = metadata(m);
            }
        }
        "Form" => v.fields = kids(root).filter_map(field).collect(),
        // A menu-bar command opened in procast: its menu becomes a list.
        "MenuBarExtra" => {
            fn flat(nodes: &[MenuNode], section: Option<&str>, v: &mut View) {
                for n in nodes {
                    match n {
                        MenuNode::Item { title, on_action } => v.items.push(ExtItem {
                            title: title.clone(),
                            section: section.map(str::to_string),
                            actions: vec![ExtAction { title: title.clone(), on_action: on_action.clone() }],
                            ..ExtItem::default()
                        }),
                        MenuNode::Section { title, children } | MenuNode::Submenu { title, children } => {
                            flat(children, Some(title).filter(|t| !t.is_empty()).map(String::as_str).or(section), v)
                        }
                        MenuNode::Separator => {}
                    }
                }
            }
            if let Some(bar) = parse_menubar(tree) {
                v.kind = "List".into();
                v.title = Some(bar.title).filter(|t| !t.is_empty());
                flat(&bar.items, None, &mut v);
            }
        }
        _ => {}
    }
    v
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuNode {
    Item { title: String, on_action: Option<String> },
    Section { title: String, children: Vec<MenuNode> },
    Submenu { title: String, children: Vec<MenuNode> },
    Separator,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct MenuBar {
    pub title: String,
    pub tooltip: String,
    pub icon: Option<String>,
    pub items: Vec<MenuNode>,
}

/// A rendered `MenuBarExtra` tree -> tray menu model.
pub fn parse_menubar(tree: &Value) -> Option<MenuBar> {
    let root = tree.as_array()?.iter().find(|n| n["type"] == "MenuBarExtra")?;
    fn nodes(n: &Value) -> Vec<MenuNode> {
        kids(n)
            .filter_map(|c| {
                let p = &c["props"];
                let (t, sub) = (s(&p["title"]), s(&p["subtitle"]));
                Some(match c["type"].as_str()? {
                    "MenuBarExtra.Item" => MenuNode::Item { title: if sub.is_empty() { t } else { format!("{t}  {sub}") }, on_action: fn_id(&p["onAction"]) },
                    "MenuBarExtra.Section" => MenuNode::Section { title: s(&p["title"]), children: nodes(c) },
                    "MenuBarExtra.Submenu" => MenuNode::Submenu { title: s(&p["title"]), children: nodes(c) },
                    "MenuBarExtra.Separator" => MenuNode::Separator,
                    _ => return None,
                })
            })
            .collect()
    }
    let p = &root["props"];
    Some(MenuBar { title: s(&p["title"]), tooltip: s(&p["tooltip"]), icon: icon(&p["icon"]), items: nodes(root) })
}

/// Markdown -> display lines (heading level 0-3, text). 6 is a quote, 7 a rule (`---`), 9 a
/// code line, and 8 an image: its text is the
/// source, with Raycast's `?raycast-height=N` hint (an `<img height>` folds into it), one line
/// per image after the text it sat in. Links keep their text; other HTML tags drop.
pub fn markdown_lines(md: &str) -> Vec<(u8, String)> {
    let mut out = vec![];
    let mut code = false;
    for raw in md.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            code = !code;
            continue;
        }
        if code {
            out.push((9, line.to_string()));
            continue;
        }
        let bare = line.trim();
        if bare.len() >= 3 && ['-', '*', '_'].iter().any(|c| bare.chars().all(|x| x == *c)) {
            out.push((7, String::new()));
            continue;
        }
        let quote = bare.strip_prefix('>');
        let level = if quote.is_some() { 6 } else { line.chars().take_while(|c| *c == '#').count().min(3) as u8 };
        let mut text = quote.unwrap_or(line).trim_start_matches('#').trim().to_string();
        // ![alt](src) and <img src> -> image lines, [text](url) -> text, **b** / __b__ / `c` -> b / c
        let mut images = vec![];
        while let Some(i) = text.find("![") {
            let end = text[i..].find(')').map(|e| i + e + 1).unwrap_or(text.len());
            if let Some(src) = text[i..end].split_once("](").and_then(|(_, rest)| rest.trim_end_matches(')').split_whitespace().next()) {
                images.push(src.to_string());
            }
            text.replace_range(i..end, "");
        }
        // ASCII lowercasing keeps byte offsets.
        while let Some(i) = text.to_ascii_lowercase().find("<img") {
            let end = text[i..].find('>').map(|e| i + e + 1).unwrap_or(text.len());
            let tag = &text[i..end];
            if let Some(src) = attr(tag, "src") {
                images.push(match attr(tag, "height").map(|h| h.trim_end_matches("px").to_string()).filter(|h| h.parse::<f32>().is_ok()) {
                    Some(h) => format!("{src}{}raycast-height={h}", if src.contains('?') { '&' } else { '?' }),
                    None => src,
                });
            }
            text.replace_range(i..end, "");
        }
        let mut from = 0;
        while let Some(i) = text[from..].find('<').map(|i| from + i) {
            let tag = text[i + 1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '/');
            match text[i..].find('>') {
                Some(e) if tag => text.replace_range(i..i + e + 1, ""),
                _ => from = i + 1,
            }
        }
        text = text.trim().to_string();
        while let (Some(a), Some(b)) = (text.find('['), text.find("](")) {
            if b < a {
                break;
            }
            let end = text[b..].find(')').map(|e| b + e + 1).unwrap_or(text.len());
            let label = text[a + 1..b].to_string();
            text.replace_range(a..end, &label);
        }
        // Entities decode after tags strip, so "&lt;b&gt;" stays as text; &amp; goes last.
        let text = text.replace("**", "").replace("__", "").replace('`', "");
        let text = [("&nbsp;", " "), ("&quot;", "\""), ("&#39;", "'"), ("&apos;", "'"), ("&lt;", "<"), ("&gt;", ">"), ("&amp;", "&")].iter().fold(text, |t, (e, c)| t.replace(e, c));
        let text = match text.strip_prefix("- ").or(text.strip_prefix("* ")) {
            Some(rest) => format!("• {rest}"),
            None => text,
        };
        let blank_after_blank = text.is_empty() && out.last().is_some_and(|l: &(u8, String)| l.1.is_empty());
        if !(blank_after_blank || text.is_empty() && !images.is_empty()) {
            out.push((level, text));
        }
        out.extend(images.into_iter().map(|src| (8, src)));
    }
    out
}

/// An HTML attribute's value: `name="v"`, `name='v'` or `name=v`.
fn attr(tag: &str, name: &str) -> Option<String> {
    let at = tag.to_ascii_lowercase().find(&format!(" {name}="))? + name.len() + 2;
    let rest = &tag[at..];
    match rest.chars().next()? {
        q @ ('"' | '\'') => rest[1..].split(q).next().map(str::to_string),
        _ => rest.split([' ', '>', '/']).next().map(str::to_string),
    }
}

/// An image line's source without Raycast's size hints, and its `raycast-height`.
pub fn image_src(line: &str) -> (String, Option<f32>) {
    let Some((base, query)) = line.split_once('?') else { return (line.to_string(), None) };
    let mut height = None;
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| match p.split_once('=') {
            Some(("raycast-height", h)) => {
                height = h.parse().ok();
                false
            }
            Some((k, _)) => !k.starts_with("raycast-"),
            None => true,
        })
        .collect();
    (if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) }, height)
}

/// A PNG asset as a GPUI image (BGRA), shrunk to icon size.
pub fn load_image(path: &Path) -> Option<std::sync::Arc<gpui::RenderImage>> {
    load_image_sized(path, 64)
}

/// Like `load_image`, with the longest side at most `max` px (the aspect ratio stays).
pub fn load_image_sized(path: &Path, max: u32) -> Option<std::sync::Arc<gpui::RenderImage>> {
    // Format from the bytes: cached web images have no extension.
    let img = image::load_from_memory(&std::fs::read(path).ok()?).ok()?.to_rgba8();
    let longest = img.width().max(img.height());
    let img = if longest > max {
        let scaled = |side: u32| (side as u64 * max as u64 / longest as u64).max(1) as u32;
        image::imageops::thumbnail(&img, scaled(img.width()), scaled(img.height()))
    } else {
        img
    };
    let (w, h) = img.dimensions();
    let mut buf = img.into_raw();
    for p in buf.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let frame = image::Frame::new(image::RgbaImage::from_raw(w, h, buf)?);
    Some(std::sync::Arc::new(gpui::RenderImage::new(vec![frame])))
}

/// A PNG as straight RGBA at tray size (32 px), for tray-icon.
pub fn load_rgba(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::open(path).ok()?.to_rgba8();
    let img = image::imageops::resize(&img, 32, 32, image::imageops::FilterType::Triangle);
    Some((img.into_raw(), 32, 32))
}

/// Raycast icon names -> icon names (`design::g`); anything unknown gets the command icon.
pub fn icon_glyph(name: &str) -> &'static str {
    use crate::design::g;
    match name {
        "Checkmark" | "CheckCircle" | "Check" | "CheckList" => g::CHECK,
        "XMarkCircle" | "XMarkCircleFilled" | "Xmark" | "XMark" | "Stop" => g::CANCEL,
        "Star" | "StarFilled" => g::STAR,
        "Document" | "Text" | "TextDocument" | "Paragraph" | "BlankDocument" => g::DOCUMENT,
        "Folder" | "Finder" => g::FOLDER,
        "Link" => g::LINK,
        "Globe" | "Network" => g::GLOBE,
        "MagnifyingGlass" => g::SEARCH,
        "Clock" => g::CLOCK,
        "Hourglass" | "Stopwatch" | "Alarm" => g::TIMER,
        "Calendar" => g::CALENDAR,
        "Trash" => g::DELETE,
        "Pencil" | "Pen" => g::EDIT,
        "Plus" | "PlusCircle" | "Download" => g::DOWNLOAD,
        "Gear" | "Cog" => g::SETTING,
        "Gauge" | "Speedometer" => g::GAUGE,
        "Moon" => g::SLEEP,
        "Sun" | "Swatch" => g::PERSONALIZE,
        "Brush" => g::BRUSH,
        "Play" | "PlayFilled" => g::PLAY,
        "Terminal" => g::TERMINAL,
        "Code" | "CodeBlock" => g::CODE,
        "Wifi" => g::WIFI,
        "Plug" => g::NETWORK,
        "Bell" | "BellDisabled" => g::BELL,
        "Hashtag" => g::HASH,
        "Monitor" | "Desktop" => g::MONITOR,
        "HardDrive" => g::HARD_DRIVE,
        "MemoryChip" => g::CPU,
        "GameController" => g::GAMEPAD_2,
        "Info" | "QuestionMark" | "QuestionMarkCircle" => g::INFO,
        "Warning" | "ExclamationMark" => g::WARNING,
        "Lock" | "LockUnlocked" | "Key" => g::LOCK,
        "Shield" => g::SHIELD,
        "Clipboard" | "CopyClipboard" => g::PASTE,
        "Window" | "AppWindow" | "AppWindowGrid3x3" => g::TILES,
        "Keyboard" => g::KEYBOARD,
        "Envelope" => g::MAIL,
        "Pin" | "Tack" => g::PIN,
        "List" | "BulletPoints" => g::BULLETS,
        "Filter" => g::FILTER,
        "ArrowClockwise" | "RotateClockwise" => g::REFRESH,
        "Power" => g::POWER,
        "Person" | "PersonCircle" | "TwoPeople" => g::SIGN_OUT,
        _ => g::TERMINAL,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest_prefs_and_views() {
        let m = json!({
            "name": "notes", "title": "Notes",
            "preferences": [{"name": "vault", "type": "directory", "required": true, "title": "Vault"}],
            "commands": [
                {"name": "search", "title": "Search Notes", "mode": "view"},
                {"name": "sync", "title": "Sync", "mode": "no-view", "interval": "5m",
                 "preferences": [{"name": "deep", "type": "checkbox", "required": false, "default": true}]}
            ]
        });
        let ext = parse_manifest(Path::new("C:\\x"), &m).unwrap();
        assert_eq!(ext.commands[1].interval, Some(Duration::from_secs(300)));
        assert_eq!(parse_interval("2s"), Some(Duration::from_secs(10)), "Raycast minimum is 10 s");
        assert_eq!(preference_values(&ext, &ext.commands[0], None), Err(vec!["vault".to_string()]));
        let cfg: toml::Table = toml::from_str("vault = 'D:\\Notes'").unwrap();
        let v = preference_values(&ext, &ext.commands[1], Some(&cfg)).unwrap();
        assert_eq!((v["vault"].as_str(), v["deep"].as_bool()), (Some("D:\\Notes"), Some(true)));

        // A list with a section, an item with two actions (one in a submenu) and a detail.
        let tree = json!([{ "type": "List", "props": {"isLoading": false, "onSearchTextChange": {"$fn": "p1.0:onSearchTextChange"}, "isShowingDetail": true},
            "children": [{ "type": "List.Section", "props": {"title": "Recent"}, "children": [
                { "type": "List.Item", "props": {"title": "Note A", "subtitle": {"value": "today"}, "icon": "icon:Document", "accessories": [{"text": "3 KB"}, {"tag": {"value": "work"}}]},
                  "children": [
                    {"type": "slot", "props": {"name": "actions"}, "children": [{"type": "ActionPanel", "props": {}, "children": [
                        {"type": "Action", "props": {"title": "Open", "onAction": {"$fn": "a:onAction"}}},
                        {"type": "ActionPanel.Submenu", "props": {"title": "Move"}, "children": [{"type": "Action", "props": {"title": "Archive", "onAction": {"$fn": "b:onAction"}}}]}
                    ]}]},
                    {"type": "slot", "props": {"name": "detail"}, "children": [{"type": "List.Item.Detail", "props": {"markdown": "# Note A\nbody"}}]}
                  ]}
            ]}]}]);
        let v = parse_view(&tree);
        assert_eq!((v.kind.as_str(), v.on_search.as_deref(), v.show_detail), ("List", Some("p1.0:onSearchTextChange"), true));
        assert_eq!(v.load_more, None, "no pagination prop, nothing to load");
        let paged = |more: bool| parse_view(&json!([{"type": "List", "props": {"pagination": {"pageSize": 10, "hasMore": more, "onLoadMore": {"$fn": "p1.0:pagination.onLoadMore"}}}}])).load_more;
        assert_eq!((paged(true).as_deref(), paged(false)), (Some("p1.0:pagination.onLoadMore"), None));
        let it = &v.items[0];
        assert_eq!((it.title.as_str(), it.subtitle.as_str(), it.section.as_deref()), ("Note A", "today", Some("Recent")));
        assert_eq!(it.accessories, ["3 KB", "work"]);
        assert_eq!(it.actions.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), ["Open", "Move › Archive"]);
        assert_eq!(it.detail.as_deref(), Some("# Note A\nbody"));

        let form = json!([{ "type": "Form", "props": {}, "children": [
            {"type": "Form.Dropdown", "props": {"id": "vault", "title": "Vault", "onChange": {"$fn": "d:onChange"}}, "children": [
                {"type": "Form.Dropdown.Item", "props": {"value": "v1", "title": "Personal"}},
                {"type": "Form.Dropdown.Item", "props": {"value": "v2", "title": "Work"}}]},
            {"type": "Form.TextArea", "props": {"id": "text", "title": "Note", "value": "hi"}},
            {"type": "Form.Checkbox", "props": {"id": "pin", "label": "Pin it"}}
        ]}]);
        let f = parse_view(&form).fields;
        assert_eq!(f[0].value, json!("v1"), "dropdown defaults to its first option");
        assert!(f[1].controlled && f[1].value == json!("hi"));
        assert_eq!((f[2].title.as_str(), &f[2].value), ("Pin it", &json!(false)));

        let repo = crate::test_dir("store-test");
        for (name, manifest) in [
            ("b-ext", json!({"name": "b-ext", "title": "Beta", "author": "ann", "platforms": ["macOS", "Windows"], "commands": [{}, {}]})),
            ("a-ext", json!({"name": "a-ext", "title": "alpha", "description": "mac only"})),
        ] {
            std::fs::create_dir_all(repo.join("extensions").join(name)).unwrap();
            std::fs::write(repo.join("extensions").join(name).join("package.json"), manifest.to_string()).unwrap();
        }
        let store = read_store(&repo);
        let _ = std::fs::remove_dir_all(&repo);
        assert_eq!(store.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["alpha", "Beta"]);
        assert_eq!((store[0].windows, store[1].windows, store[1].commands), (false, true, 2));

        let bar = json!([{"type": "MenuBarExtra", "props": {"title": "3 ports", "icon": "icon:Network"}, "children": [
            {"type": "MenuBarExtra.Section", "props": {"title": "Listening"}, "children": [
                {"type": "MenuBarExtra.Item", "props": {"title": "node", "subtitle": ":3000", "onAction": {"$fn": "k:onAction"}}}]},
            {"type": "MenuBarExtra.Separator", "props": {}},
            {"type": "MenuBarExtra.Submenu", "props": {"title": "More"}, "children": [{"type": "MenuBarExtra.Item", "props": {"title": "Refresh"}}]}
        ]}]);
        let mb = parse_menubar(&bar).unwrap();
        assert_eq!((mb.title.as_str(), mb.icon.as_deref()), ("3 ports", Some("icon:Network")));
        assert_eq!(mb.items[0], MenuNode::Section { title: "Listening".into(), children: vec![MenuNode::Item { title: "node  :3000".into(), on_action: Some("k:onAction".into()) }] });
        assert_eq!(mb.items[1], MenuNode::Separator);
        assert!(matches!(&mb.items[2], MenuNode::Submenu { children, .. } if children.len() == 1));
        let v = parse_view(&bar);
        assert_eq!((v.kind.as_str(), v.title.as_deref(), v.items.len()), ("List", Some("3 ports"), 2));
        assert_eq!((v.items[0].section.as_deref(), v.items[0].actions[0].on_action.as_deref()), (Some("Listening"), Some("k:onAction")));
        assert_eq!(v.items[1].section.as_deref(), Some("More"));

        let with_args = parse_manifest(Path::new("C:\\x"), &json!({"name": "gh", "commands": [{"name": "issue", "title": "New Issue", "arguments": [
            {"name": "title", "placeholder": "Title", "type": "text", "required": true},
            {"name": "token", "placeholder": "Token", "type": "password"},
            {"name": "repo", "placeholder": "Repo", "type": "dropdown", "data": [{"title": "Procast", "value": "pc"}, {"title": "Docs", "value": "docs"}]}]}]}))
        .unwrap();
        let args = &with_args.commands[0].args;
        assert_eq!(args.iter().map(|a| (a.name.as_ref(), a.placeholder.as_ref(), a.optional)).collect::<Vec<_>>(), [("title", "Title", false), ("token", "Token", true), ("repo", "Repo", true)]);
        use crate::search::ArgKind;
        assert_eq!((&args[0].kind, &args[1].kind), (&ArgKind::Text, &ArgKind::Secret));
        assert_eq!(args[2].kind, ArgKind::Choice(vec![("pc".into(), "Procast".into()), ("docs".into(), "Docs".into())]));

        let md = markdown_lines("# Title\nSee [docs](https://x.y) and **bold**\n![img](a.png)\n- item\n```\ncode\n```");
        assert_eq!(md, [(1, "Title".to_string()), (0, "See docs and bold".into()), (8, "a.png".into()), (0, "• item".into()), (9, "code".into())]);
    }

    #[test]
    fn html_images_become_image_lines_and_other_tags_drop() {
        let md = markdown_lines("<img src=\"https://m.media-amazon.com/a.jpg\" height=\"300\" />\n\nAn <b>insomniac</b> worker<br/>\n![poster](https://x.y/p.png?raycast-width=200 \"Poster\")\na < b");
        assert_eq!(
            md,
            [
                (8, "https://m.media-amazon.com/a.jpg?raycast-height=300".to_string()),
                (0, "".into()),
                (0, "An insomniac worker".into()),
                (8, "https://x.y/p.png?raycast-width=200".into()),
                (0, "a < b".into()),
            ]
        );
        assert_eq!(markdown_lines("Tom &amp; Jerry&nbsp;&quot;1940&quot;\n---\n> A cat chases a mouse\n***"), [(0, "Tom & Jerry \"1940\"".to_string()), (7, "".into()), (6, "A cat chases a mouse".into()), (7, "".into())]);
        assert_eq!(image_src("https://x.y/p.png?v=2&raycast-height=300"), ("https://x.y/p.png?v=2".into(), Some(300.)));
        assert_eq!(image_src("poster.png?raycast-width=80"), ("poster.png".into(), None));
    }

    #[test]
    fn tag_lists_keep_their_tags() {
        let m = json!({"type": "slot", "children": [
            {"type": "Metadata.Label", "props": {"title": "Director", "text": "David Fincher"}},
            {"type": "Metadata.TagList", "props": {"title": "Genres"}, "children": [
                {"type": "Metadata.TagList.Item", "props": {"text": "Drama", "color": "color:Red"}},
                {"type": "Metadata.TagList.Item", "props": {"text": "Thriller"}}
            ]},
            {"type": "Metadata.Separator", "props": {}}
        ]});
        let rows = metadata(&m);
        assert_eq!(rows[0], Meta { title: "Director".into(), text: "David Fincher".into(), tags: vec![] });
        assert_eq!(rows[1].tags, [("Drama".to_string(), "color:Red".to_string()), ("Thriller".into(), String::new())]);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn images_are_capped_on_their_longest_side() {
        let dir = crate::test_dir("image-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.png");
        image::RgbaImage::new(400, 200).save(&path).unwrap();
        let size = |max| {
            let size = load_image_sized(&path, max).unwrap().size(0);
            (size.width.0, size.height.0)
        };
        assert_eq!((size(160), size(64), size(800)), ((160, 80), (64, 32), (400, 200)));
        assert_eq!(load_image(&path).unwrap().size(0).width.0, 64);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
