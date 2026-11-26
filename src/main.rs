#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod apps;
mod browser;
mod calendar;
mod clipboard;
mod commands;
mod config;
mod design;
mod dict;
mod desktops;
mod ext;
mod files;
mod focus;
mod foreground;
mod hdr;
mod hud;
mod keys;
mod menu;
mod media;
mod music_tray;
mod music_view;
mod net;
mod notes;
mod ntfs;
mod scripts;
mod search;
mod settings;
mod shots;
mod snippets;
mod system;
mod timers;
mod toast;
mod translate;
mod wellbeing;
mod wellbeing_http;
mod win;
mod ytmdesktop;
mod ytmusic;
mod ytmusic_api;

use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use futures::{
    StreamExt,
    channel::mpsc::{self, UnboundedSender},
};
use gpui::{
    App, Bounds, ClipboardItem, Context, Entity, Focusable, FontWeight, KeyBinding, QuitMode, RenderImage,
    ScrollHandle, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window, WindowBounds, WindowKind, WindowOptions,
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

use commands::{Cmd, Desk, FocusCmd};
use config::Config;
use design::{Tokens, g, px};
use ext::FieldKind;
use notes::{
    store::{NoteMeta, Store},
    window::{Notes, NotesWindow, Place, created},
};
use scripts::{Output, Script};
use search::{Act, Frecency, Icon, Item};
use win::{BINDS, Bind, Msg, TrayCmd};

actions!(procast, [SelectPrev, SelectNext, Dismiss, Launch, ToggleActions, Submit, NextSection, PrevSection]);
// Left and Right in the emoji grid (the search bar keeps them elsewhere).
actions!(procast, [SelectLeft, SelectRight]);
// Ctrl+P: the search bar's Dropdown (the translator's language sets).
actions!(procast, [OpenDropdown]);
// Tab and Shift+Tab while argument boxes show.
actions!(procast, [NextBox, PrevBox]);
// Backspace on a token field (or in its menu's empty search) takes the last token.
actions!(procast, [PopToken]);
// Tab in YouTube Music's Search and Library: the next filter.
actions!(procast, [NextFilter]);
// Navigation-key styles; each does something only when `[launcher] navigation` picks it.
actions!(procast, [EmacsNext, EmacsPrev, EmacsBack, VimNext, VimPrev, VimBack]);
// An action's own shortcut (`search::act_keys`) on the selected row: Ctrl+O, Ctrl+Shift+O, Ctrl+Y, Ctrl+Alt+O.
actions!(procast, [KeyOpen, KeyOpenWith, KeyQuickLook, KeyReveal]);

use design::{BAR, FOOTER, LIST_PAD, ROW};
use search::MAX_ARGS;

fn window_height(cfg: &Config) -> f32 {
    // The window's 1 px border top and bottom, the bar, whole rows (results plus a section
    // header), the list padding and the action bar.
    2. + BAR + ROW * (cfg.max_results.max(1) + 1) as f32 + 2. * LIST_PAD + FOOTER
}

/// Compact mode's favourites strip under the search bar.
const FAV_ROW: f32 = 48.;

/// The launcher's width before scaling. `width` ≤ 1 is a share of the screen: the window
/// keeps that share (and its proportions) on every monitor, and its content scales with it.
const BASE_WIDTH: f32 = 750.;

/// The launcher's backdrop: acrylic under the translucent glass body; with glass off the body
/// is solid and the acrylic goes (Transparent, not Opaque: Opaque clears to white, which would
/// show in the anti-aliased corners inside the rounded window region).
fn backdrop(glass: bool) -> gpui::WindowsWindowBackground {
    if glass { gpui::WindowsWindowBackground::Blurred } else { gpui::WindowsWindowBackground::Transparent }
}

fn base_width(cfg: &Config) -> f32 {
    if cfg.width <= 1. { BASE_WIDTH } else { cfg.width }
}

/// What kind of message the action bar is showing.
#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Info,
    Ok,
    Error,
}

impl Tone {
    fn of(kind: toast::Kind) -> Self {
        match kind {
            toast::Kind::Done => Tone::Ok,
            toast::Kind::Error => Tone::Error,
            toast::Kind::Info => Tone::Info,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Root,
    Clipboard,
    Emoji,
    Files,
    /// One command row with its argument boxes focused (see `Launcher::args_row`): a hotkey,
    /// a snippet keyword typed in an app, or a command run from elsewhere.
    Args,
    /// Showing a script's output (see `Launcher::output`).
    Output,
    /// An extension command's view (see `Launcher::extv`).
    Ext,
    /// Asking what to install.
    Install,
    /// Browsing the Raycast store index.
    Store,
    /// Running apps, to quit.
    Running,
    /// Tabs open in connected browsers.
    Tabs,
    /// A YouTube Music screen (see `Launcher::music`).
    Music,
    /// A Quick AI answer (see `Launcher::ai`).
    Ai,
    /// A translation (see `Launcher::translation`).
    Translate,
    /// A dictionary answer (see `Launcher::define`).
    Define,
    /// Screenshot search (see `Launcher::shots`).
    Shots,
    /// My Schedule: today's and tomorrow's events.
    Schedule,
    /// A native form (see `Launcher::form`).
    Form(FormKind),
    /// Manage Language Sets: the translator's sets, to pick, add, edit or delete.
    LanguageSets,
    /// Search Notes or Recently Deleted Notes.
    Notes(NotesList),
    /// Screen Time: a day's, week's or month's categories, apps and sites (see `Launcher::st_stats`).
    ScreenTime(wellbeing::Period),
}

/// Which notes `Mode::Notes` lists.
#[derive(Clone, Copy, PartialEq, Debug)]
enum NotesList {
    All,
    Deleted,
}

/// A native launcher form: what it is for, which names its title and primary action.
#[derive(Clone, Copy, PartialEq, Debug)]
enum FormKind {
    Focus,
    AddCalendar,
    /// Some(i) edits language set `i`.
    AddLanguageSet(Option<usize>),
    ImportScreenTime,
}

impl FormKind {
    /// The ActionBar's icon and title on the left, and the primary action (Ctrl+Enter) on the right.
    fn bar(self) -> (&'static str, &'static str, &'static str) {
        match self {
            FormKind::Focus => (g::SHIELD_CHECK, "Focus", "Start Focus Session"),
            FormKind::AddCalendar => (g::CALENDAR_CLOCK, "Add Calendar", "Add Calendar"),
            FormKind::AddLanguageSet(None) => (g::LANGUAGES, "Add Language Set", "Add Language Set"),
            FormKind::AddLanguageSet(Some(_)) => (g::LANGUAGES, "Edit Language Set", "Save Language Set"),
            FormKind::ImportScreenTime => (g::IMPORT, "Import Screen Time", "Import Screen Time"),
        }
    }
}

/// One control of a launcher form.
#[derive(Clone)]
enum Ctl {
    /// A text box, two lines high when `tall`, with `icon` after the text; Down opens a menu of
    /// `presets` when there are any, and the pick is typed in.
    Text { placeholder: SharedString, icon: Option<&'static str>, tall: bool, presets: &'static [&'static str] },
    /// Picks one entry's id from a menu; true gives the menu a search field.
    Select(Vec<menu::Entry>, bool),
    /// A checkbox named by the field's label; its value is "true" when on.
    Check,
    /// A token field (Focus's Block): the value is the picked `focus::Picks`, one id a line.
    Tokens,
    /// Add Calendar's colours (`calendar::COLOURS`): the value is the picked one's name; Left
    /// and Right move.
    Swatches,
    /// Muted text (the value); with no label it is a `form_note`.
    Note,
    Rule,
}

/// One row of a launcher form, native or an extension's.
#[derive(Clone)]
struct FormField {
    label: SharedString,
    ctl: Ctl,
    /// The text typed, or the picked entry's id.
    value: String,
    /// A Text control's box.
    input: Option<Entity<EditableTextState>>,
    error: Option<String>,
}

impl FormField {
    fn focusable(&self) -> bool {
        !matches!(self.ctl, Ctl::Note | Ctl::Rule)
    }
}

/// A native form's values by label: the text typed, or the picked id.
fn form_values(fields: &[FormField]) -> HashMap<&str, &str> {
    fields.iter().filter(|f| f.focusable()).map(|f| (f.label.as_ref(), f.value.as_str())).collect()
}

/// The field Tab (`d` 1) or Shift+Tab (-1) moves to from `i`, past notes and rules, wrapping at either end.
fn next_field(fields: &[FormField], i: usize, d: isize) -> usize {
    let n = fields.len() as isize;
    let mut k = i as isize;
    for _ in 0..n {
        k = (k + d).rem_euclid(n);
        if fields[k as usize].focusable() {
            return k as usize;
        }
    }
    i
}

/// The menu a Select field (or a Text field with presets) opens at `at`, on its current value;
/// None for other controls.
fn field_menu(f: &FormField, at: gpui::Point<gpui::Pixels>) -> Option<menu::Menu> {
    let (entries, search) = match &f.ctl {
        Ctl::Select(entries, search) => (entries.clone(), *search),
        Ctl::Text { presets, .. } if !presets.is_empty() => (presets.iter().map(|p| menu::Entry { id: p.to_string(), label: p.to_string(), ..Default::default() }).collect(), false),
        _ => return None,
    };
    let mut m = menu::Menu::new(entries, search.then(|| "Search…".to_string()), menu::Place::At(at));
    m.selected = m.entries.iter().position(|e| e.id.eq_ignore_ascii_case(f.value.trim())).unwrap_or(m.selected);
    Some(m)
}

/// The Focus form: Goal, Duration (presets on Down), Mode, and the Block token field.
fn focus_form(goal: &str, duration: &str, mode: focus::Mode, picks: &[String]) -> Vec<FormField> {
    let field = |label: &str, ctl: Ctl, value: String| FormField { label: label.to_string().into(), ctl, value, input: None, error: None };
    let modes = vec![
        menu::Entry { id: "block".into(), label: "Block Apps & Websites".into(), ..Default::default() },
        menu::Entry { id: "allow".into(), label: "Allow Apps & Websites".into(), ..Default::default() },
    ];
    vec![
        field("Goal", Ctl::Text { placeholder: "Goal".into(), icon: None, tall: false, presets: &[] }, goal.to_string()),
        field("Duration", Ctl::Text { placeholder: "25 min".into(), icon: Some(g::CLOCK), tall: false, presets: &focus::DURATIONS }, duration.to_string()),
        field("Mode", Ctl::Select(modes, true), if mode == focus::Mode::Allow { "allow" } else { "block" }.into()),
        field("Block", Ctl::Tokens, focus::Picks(picks.to_vec()).value()),
    ]
}

/// The form Start or Edit Focus Session opens with these box values, prefilled; None to act at
/// once. Start opens it with Goal empty; Edit (with a session) also when its boxes are empty or
/// still hold the session as `edit_seed` fills them, and then starts from the session, its mode too.
fn focus_form_for(f: FocusCmd, got: &[String], session: Option<&focus::Session>, cats: &[config::FocusCategory], default_minutes: u32) -> Option<Result<Vec<FormField>, String>> {
    let edit = match (f, session) {
        (FocusCmd::Edit, Some(s)) => Some(s),
        (FocusCmd::Start, _) => None,
        _ => return None,
    };
    let blank = got.iter().all(|g| g.trim().is_empty());
    let seed = match edit {
        Some(s) if blank || got.iter().map(|g| g.trim()).eq(focus::edit_seed(s).iter().map(|g| g.trim())) => focus::edit_seed(s),
        _ if got.first().is_none_or(|g| g.trim().is_empty()) => got.to_vec(),
        _ => return None,
    };
    Some(focus::parse_start(&seed, cats, default_minutes).map(|start| {
        let mode = start.filter.as_ref().map(|f| f.0).or(edit.map(|s| s.mode)).unwrap_or(focus::Mode::Block);
        focus_form(seed.first().map_or("", |g| g.trim()), &focus::minutes_label(start.minutes), mode, &start.picks)
    }))
}

/// The Block menu: Focus Categories, then Applications (`apps`: file name and name), less
/// what is picked already.
fn block_entries(cats: &[config::FocusCategory], apps: &[(String, String)], picks: &focus::Picks) -> Vec<menu::Entry> {
    let cats = cats.iter().map(|c| menu::Entry { id: format!("cat:{}", c.name), label: c.name.clone(), icon: Some(focus::category_icon(&c.name)), section: Some("Focus Categories".into()), ..Default::default() });
    let apps = apps.iter().map(|(exe, name)| menu::Entry { id: format!("app:{exe}"), label: name.clone(), icon: Some(g::APP), section: Some("Applications".into()), ..Default::default() });
    cats.chain(apps).filter(|e| !picks.0.contains(&e.id)).collect()
}

/// Add Calendar's answer for the submit from form `generation`: whether it drives the form on
/// screen (`on_form`, the latest one, `form_gen`), and which fetch is still pending after it.
fn add_answer(pending: Option<u64>, generation: u64, form_gen: u64, on_form: bool) -> (bool, Option<u64>) {
    (on_form && generation == form_gen, pending.filter(|p| *p != generation))
}

/// The Add Calendar form: ICS address (focused), Name, Colour, and where to find the address.
fn calendar_form() -> Vec<FormField> {
    let field = |label: &str, ctl: Ctl, value: &str| FormField { label: label.to_string().into(), ctl, value: value.into(), input: None, error: None };
    let text = |placeholder: &'static str, icon: Option<&'static str>| Ctl::Text { placeholder: placeholder.into(), icon, tall: false, presets: &[] };
    vec![
        field("ICS address", text("https://calendar.google.com/…/basic.ics", Some(g::LINK)), ""),
        field("Name", text("Work", None), ""),
        field("Colour", Ctl::Swatches, calendar::COLOURS[0].0),
        field("", Ctl::Note, "Google: Settings → your calendar → Secret address in iCal format. Outlook: Settings → Shared calendars → Publish → ICS."),
    ]
}

/// Import Screen Time from micro bot: its folder, and what to do if that was imported before.
fn import_screen_time_form() -> Vec<FormField> {
    let field = |label: &str, ctl: Ctl, value: &str| FormField { label: label.to_string().into(), ctl, value: value.into(), input: None, error: None };
    let again = vec![
        menu::Entry { id: "stop".into(), label: "Stop and tell me".into(), ..Default::default() },
        menu::Entry { id: "again".into(), label: "Add the hours again".into(), ..Default::default() },
    ];
    vec![
        field("micro bot folder", Ctl::Text { placeholder: r"D:\Pro\micro bot".into(), icon: Some(g::FOLDER), tall: false, presets: &[] }, r"D:\Pro\micro bot"),
        field("If imported before", Ctl::Select(again, false), "stop"),
        field("", Ctl::Note, "Stops and disables micro bot's WellbeingTracker task, adds data.db, categories.json and settings.json, and turns Screen Time on. Running it twice adds the hours twice, and so does turning Screen Time on before importing. Imported apps and sites join procast's categories, so Focus blocks using them (e.g. Messaging, Gaming) include them."),
    ]
}

/// Typed text that looks like a website or names a program puts "Block x.com" / "Block
/// steam.exe" (or "Allow …", by `verb`) at the top of the Block menu, whatever the search matches.
fn site_lead(entries: &mut Vec<menu::Entry>, verb: &str, typed: &str) {
    entries.retain(|e| !e.always);
    if let Some(id) = focus::typed_pick(typed) {
        let (glyph, _) = focus::pick_look(&id);
        let name = id.split_once(':').map_or("", |(_, v)| v);
        entries.insert(0, menu::Entry { label: format!("{verb} {name}"), icon: Some(glyph), always: true, id, ..Default::default() });
    }
}

/// An extension's Form field as a form row.
fn ext_field(f: &ext::Field, ev: &ExtView) -> FormField {
    let value = match ev.values.get(&f.id).unwrap_or(&f.value) {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "true".into(),
        _ => String::new(),
    };
    let ctl = match &f.kind {
        FieldKind::Text | FieldKind::Password | FieldKind::TextArea => Ctl::Text { placeholder: f.placeholder.clone().into(), icon: None, tall: f.kind == FieldKind::TextArea, presets: &[] },
        FieldKind::Checkbox => Ctl::Check,
        FieldKind::Dropdown(opts) => Ctl::Select(opts.iter().map(|(v, t)| menu::Entry { id: v.clone(), label: t.clone(), ..Default::default() }).collect(), true),
        FieldKind::Description => Ctl::Note,
        FieldKind::Separator => Ctl::Rule,
    };
    FormField { label: f.title.clone().into(), ctl, value, input: ev.inputs.get(&f.id).cloned(), error: f.error.clone() }
}

/// A native form's rows and the selected one (Mode::Form).
struct NativeForm {
    fields: Vec<FormField>,
    field: usize,
    _subs: Vec<Subscription>,
}

/// A Quick AI chat: one Claude session, kept while the view is open.
#[derive(Default)]
struct AiChat {
    turns: Vec<AiTurn>,
    /// The last turn is still streaming.
    busy: bool,
    /// Keep the newest question at the top of the view (until Up or Down).
    follow: bool,
}

struct AiTurn {
    id: u64,
    prompt: String,
    text: String,
}

impl AiChat {
    /// The chat as markdown, and how many lines come before the newest question.
    fn markdown(&self) -> (String, usize) {
        let parts: Vec<String> = self
            .turns
            .iter()
            .map(|t| {
                let text = if t.text.is_empty() { "Thinking…" } else { t.text.trim_end() };
                // An answer cut off inside a code block must not swallow the next question.
                let open = text.lines().filter(|l| l.trim_start().starts_with("```")).count() % 2 == 1;
                format!("### {}\n\n{text}{}\n\n", t.prompt, if open { "\n```" } else { "" })
            })
            .collect();
        let before = parts.len().saturating_sub(1);
        (parts.concat(), ext::markdown_lines(&parts[..before].concat()).len())
    }
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

/// The sampler, plus the dashboard on 127.0.0.1:8737 (which stays up if Screen Time is switched off and on).
fn start_screen_time(tx: &UnboundedSender<Msg>, browsers: &browser::Browsers, rules: &Arc<RwLock<wellbeing::Rules>>, st: &Arc<RwLock<config::ScreenTime>>) -> wellbeing::Tracker {
    wellbeing_http::serve(tx.clone(), || _ = wellbeing::flush_pending());
    wellbeing::Tracker::start(tx.clone(), browsers.clone(), rules.clone(), st.clone())
}

fn start_files(cfg: &Config, tx: &UnboundedSender<Msg>) -> Option<files::Files> {
    cfg.files.enabled.then(|| files::Files::start(&cfg.files, tx.clone()))
}

/// Hide This Event's store, in `config::dir()`.
const CALENDAR_HIDDEN: &str = "calendar-hidden.json";

/// Section marker for a row that stands in for an empty view (drawn as an empty state).
const EMPTY: &str = "\u{0}empty";
/// The section of running timers and stopwatches at the head of root search.
const RUNNING: &str = "Running";
/// The Focus session's section, above Running.
const FOCUS: &str = "Focus";
/// The next meeting's section, under Running.
const NEXT: &str = "Next";
/// The ActionBar while Add Calendar's fetch runs.
const READING: &str = "Reading the calendar…";

/// List slots for items: None = a section header, Some(i) = item i. Consecutive items
/// with the same section share one header; `headed` heads a section even when it is the only
/// one (My Schedule's Now, Today and Tomorrow).
fn display_rows(items: &[Item], headed: bool) -> Vec<Option<usize>> {
    let mut out = Vec::with_capacity(items.len() + 4);
    let mut last: Option<&str> = None;
    // A header says something only when there is more than one section (Focus, Running with
    // its count, Next and Did you mean always head their rows), and "Results" right under the
    // search field says nothing.
    let one = items.windows(2).all(|w| w[0].section == w[1].section);
    for (i, it) in items.iter().enumerate() {
        if is_answer(items, i) {
            continue;
        }
        let sec = it.section.as_deref().filter(|s| *s != EMPTY);
        if sec.is_some() && sec != last && (!one || headed || matches!(sec, Some(FOCUS | RUNNING | NEXT | DID_YOU_MEAN))) && !(sec == Some("Results") && out.is_empty()) {
            out.push(None);
        }
        last = sec;
        out.push(Some(i));
    }
    out
}

/// The calculator's answer leads root results and is drawn as a card above the list.
fn is_answer(items: &[Item], i: usize) -> bool {
    i == 0 && items.first().is_some_and(|it| it.accessory == "Calculator")
}

/// A timer's or stopwatch's name and icon.
fn timer_kind(stopwatch: bool) -> (&'static str, &'static str) {
    if stopwatch { ("Stopwatch", g::TIMER) } else { ("Timer", g::ALARM_CLOCK) }
}

/// One Running row. Enter pauses or resumes, Ctrl+Enter (the second action) stops, Ctrl+K
/// also offers Restart, Rename and Add 5 min; a done timer's Enter or Ctrl+Enter dismisses it.
fn running_row(r: timers::Row) -> Item {
    let id = r.id;
    let (kind, glyph) = timer_kind(r.stopwatch);
    let acts = if r.done {
        // Enter or Ctrl+Enter (Stop) silences it; only Ctrl+K restarts it.
        vec![("Dismiss", Act::StopTimer(id)), ("Stop", Act::StopTimer(id)), ("Restart", Act::RestartTimer(id))]
    } else {
        let flip = if r.paused { Act::ResumeTimer(id) } else { Act::PauseTimer(id) };
        vec![(flip.label(), flip), ("Stop", Act::StopTimer(id)), ("Restart", Act::RestartTimer(id)), ("Rename", Act::RenameTimer(id)), ("Add 5 min", Act::AddTime(id, 300))]
    };
    let mut it = Item::new(&r.title, kind, Icon::Vector(glyph), acts[0].1.clone()).acc(r.accessory()).sec(RUNNING).tinted(design::Tint::System);
    it.extra = acts.into_iter().map(|(l, a)| (l.to_string(), a)).collect();
    it.live = true;
    it
}

/// The Focus session's row, heading root search: the goal, what it blocks, and the time left.
/// Enter pauses or resumes, Ctrl+Enter (shown in the ActionBar) completes; Ctrl+K also edits,
/// extends (a timed session), restarts or cancels.
fn focus_row(s: &focus::Session, now: SystemTime) -> Item {
    let (title, left) = s.row(now);
    let cmd = |label: &str, c: FocusCmd| (label.to_string(), Act::Cmd(Cmd::Focus(c)));
    let mut extra = vec![if s.running() { cmd("Pause", FocusCmd::Pause) } else { cmd("Resume", FocusCmd::Resume) }, cmd("Complete", FocusCmd::Complete), cmd("Edit", FocusCmd::Edit)];
    if !s.unlimited() {
        extra.push(("Extend".into(), Act::FocusExtend));
    }
    extra.extend([cmd("Restart", FocusCmd::Restart), cmd("Cancel", FocusCmd::Cancel)]);
    let mut it = Item::new(title, focus::blocking_line(s), Icon::Vector(g::SHIELD_CHECK), extra[0].1.clone()).acc(left).sec(FOCUS).tinted(design::Tint::System);
    it.extra = extra;
    it.live = true;
    it.bar_second = Some("Complete".into());
    it
}

/// Root search as shown: Focus, Running and Next (`above`) head the results, except that the
/// row the query names (an exact alias, or a command with its boxes: `search::heads`) goes
/// first of all.
fn head_rows(mut above: Vec<Item>, mut found: Vec<Item>, query: &str, cfg: &Config) -> Vec<Item> {
    if let Some(i) = found.iter().position(|it| search::heads(it, query.trim_start(), cfg)) {
        above.insert(0, found.remove(i));
    }
    above.extend(found);
    above
}

/// Where root search's selection starts: on the row the query names (`search::heads`), so its
/// boxes show; else, while something is typed, on the first row past the Focus and Running
/// rows (Enter on "te" opens what was searched for, it never pauses a timer); else on the Next
/// row after a meeting reminder (`pick_next`), so Enter joins; else the top.
fn root_selection(items: &[Item], query: &str, cfg: &Config, pick_next: bool) -> usize {
    let typed = !query.trim().is_empty();
    let head = |it: &Item| matches!(it.section.as_deref(), Some(RUNNING | FOCUS | NEXT));
    let next = || items.iter().position(|it| pick_next && !typed && it.section.as_deref() == Some(NEXT));
    items.iter().position(|it| search::heads(it, query.trim_start(), cfg)).or_else(|| items.iter().position(|it| typed && !head(it))).or_else(next).unwrap_or(0)
}

/// Where the row `was` sits in the rebuilt `items`: the same timer, else the same frecency key
/// (or, without one, the same title) in the same section.
fn same_row(items: &[Item], was: &Item) -> Option<usize> {
    if let Some(id) = timer_id(was) {
        return items.iter().position(|it| timer_id(it) == Some(id));
    }
    items.iter().position(|it| it.section == was.section && it.key == was.key && (was.key.is_some() || it.title == was.title))
}

/// The action bar's left text for the selected row: what kind of thing it is.
fn row_footer(it: Option<&Item>) -> String {
    match it.filter(|it| it.section.as_deref() != Some(EMPTY)) {
        // A Running row's accessory is its time; its kind is the subtitle. The session row is Focus.
        Some(it) if it.section.as_deref() == Some(FOCUS) => FOCUS.into(),
        Some(it) if it.section.as_deref() == Some(NEXT) => "My Schedule".into(),
        Some(it) if it.live => it.subtitle.clone(),
        // A clipboard row's accessory is when it was copied ("yesterday"), not a kind.
        Some(it) if !it.accessory.is_empty() && !matches!(it.act, Act::Clip(_)) => it.accessory.clone(),
        _ => "procast".into(),
    }
}

/// The timer or stopwatch a Running row stands for.
fn timer_id(it: &Item) -> Option<u64> {
    it.live.then(|| it.extra.iter().find_map(|(_, a)| if let Act::StopTimer(id) = a { Some(*id) } else { None })).flatten()
}

/// Toggle HDR says the state Enter would change, in its subtitle and its ActionBar label.
/// Until the first read is in (`None`), it keeps its plain subtitle and label.
fn hdr_state(items: &mut [Item], status: Option<&[hdr::HdrDisplay]>) {
    let Some(status) = status else { return };
    for it in items.iter_mut().filter(|it| it.act == Act::Cmd(Cmd::ToggleHdr)) {
        it.subtitle = hdr::subtitle(status);
        it.extra = vec![(hdr::bar_label(status).into(), it.act.clone())];
    }
}

/// Takes an HDR read into `cache` unless a newer one was asked for since (`asked`, the last
/// read's number); false when it is dropped.
fn take_hdr_read(cache: &mut Option<Vec<hdr::HdrDisplay>>, asked: u64, seq: u64, status: Vec<hdr::HdrDisplay>) -> bool {
    if seq != asked {
        return false;
    }
    *cache = Some(status);
    true
}

/// Root search shows the Auto HDR row for a query with "hdr" in it (or a start of it: "hd").
fn wants_auto_hdr(query: &str) -> bool {
    query.split_whitespace().any(|w| {
        let w = w.to_lowercase();
        w.contains("hdr") || (w.len() > 1 && "hdr".starts_with(&w))
    })
}

/// Root search's Auto HDR row: Enter flips `[games] auto_hdr`.
fn auto_hdr_row(on: bool) -> Item {
    let mut it = Item::new("Auto HDR for games", "On while a full-screen game is in front", Icon::Vector(g::SUN_MEDIUM), Act::SetAutoHdr(!on)).tinted(design::Tint::System).sec("Auto HDR");
    it.toggle = Some(on);
    it
}

fn empty_row(glyph: &'static str, title: String, hint: String, actions: Vec<(String, Act)>) -> Item {
    let mut it = Item::new(title, hint, Icon::Vector(glyph), actions.first().map(|a| a.1.clone()).unwrap_or(Act::None)).sec(EMPTY);
    it.extra = actions;
    it
}

/// Hook settings: Hyper, double tap, and the snippet keywords and emoji shortcodes typed anywhere.
fn configure_keys(cfg: &Config, tx: &UnboundedSender<Msg>) {
    let keywords = cfg.snippet.iter().enumerate().filter(|(_, s)| cfg.snippets.expand && !cfg.launcher.is_disabled(&format!("sn:{}", s.name))).map(|(i, s)| (i, s.keyword.clone())).collect();
    let x = snippets::Expander::new(keywords, snippets::Trigger::of(&cfg.snippets.trigger), cfg.snippets.within_words, cfg.emoji.shortcodes);
    keys::configure(&cfg.keyboard, Some(x), tx.clone());
}

/// What `configure_keys` reads, to tell when a reload must redo it.
fn key_settings(cfg: &Config) -> (config::Keyboard, Vec<config::Snippet>, config::Snippets, bool, Vec<String>) {
    (cfg.keyboard.clone(), cfg.snippet.clone(), cfg.snippets.clone(), cfg.emoji.shortcodes, cfg.launcher.disabled.clone())
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
    /// The primary action destroys something (the extension says so).
    danger: bool,
    /// A native confirm: what Confirm does. None: an extension's confirmAlert, answered by reply.
    then: Option<OnConfirm>,
}

type OnConfirm = Box<dyn FnOnce(&mut Launcher, &mut Context<Launcher>)>;

/// A confirm drawn as a dialog; these two rows only drive selection and Enter.
fn confirm_rows(c: &Confirm) -> Vec<Item> {
    let mut ok = Item::new(c.primary.clone(), "", Icon::Vector(g::CHECK), Act::Confirm(true));
    ok.extra = vec![(c.primary.clone(), Act::Confirm(true))];
    vec![ok, Item::new("Cancel", "", Icon::Vector(g::CANCEL), Act::Confirm(false))]
}

/// What a menu's opener does with the picked `Entry.id`.
type OnPick = Box<dyn Fn(&mut Launcher, String, &mut Window, &mut Context<Launcher>)>;

/// The translator view: the newest request (its text is what the bar holds) and the answers
/// shown, with the one picked on open. While `busy`, those are the previous answers, dimmed.
struct TrView {
    job: translate::Job,
    rows: Vec<translate::Translation>,
    first: usize,
    busy: bool,
}

impl TrView {
    /// The language the text is in: as Google recognised it, else as asked.
    fn detected(&self) -> String {
        self.rows.first().map(|t| t.detected.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| self.job.from.clone())
    }
}

/// How long typing must pause before the translator asks again.
const TYPING_QUIET: Duration = Duration::from_millis(300);

/// The section of other spellings in the define view; headed even when it is the only one.
const DID_YOU_MEAN: &str = "Did you mean";

/// The define view: the word shown, whether its lookup downloads the dictionary first, and the
/// answer (None while the lookup runs; Ok(None): the dictionary is not downloaded yet).
struct DefView {
    word: String,
    fetch: bool,
    answer: Option<Result<Option<dict::Answer>, String>>,
}

/// The define view's list: senses by part of speech (Enter copies the gloss), then Did you
/// mean; or an empty view that says what to do. `web` is the web search URL with `{query}`.
fn define_rows(v: &DefView, web: &str) -> Vec<Item> {
    let word = &v.word;
    match &v.answer {
        None => {
            let title = if v.fetch { "Downloading the dictionary (16 MB, once)…" } else { "Looking it up…" };
            vec![empty_row(g::CLOCK, title.into(), String::new(), vec![])]
        }
        Some(Err(e)) => vec![empty_row(g::WARNING, e.clone(), String::new(), vec![])],
        Some(Ok(None)) => vec![empty_row(g::DOWNLOAD, "Download Dictionary".into(), "WordNet, 16 MB, downloaded once".into(), vec![("Download Dictionary".into(), Act::DownloadDictionary(word.clone()))])],
        Some(Ok(Some(a))) if a.senses.is_empty() && a.maybe.is_empty() => {
            let search = ("Search the Web".to_string(), Act::Open(web.replace("{query}", &search::url_encode(word))));
            vec![empty_row(g::BOOK_OPEN, dict::missing_title(word), "Check the spelling, or search the web".into(), vec![search])]
        }
        Some(Ok(Some(a))) => {
            let senses = dict::group(&a.senses).into_iter().flat_map(|(_, s)| s).map(|s| {
                let mut it = Item::new(&s.gloss, "", Icon::Vector(g::BOOK_OPEN), Act::Copy(s.gloss.clone())).sec(s.pos.name()).tinted(design::Tint::Web);
                let similar = (!s.synonyms.is_empty()).then(|| ("Define Similar Word".to_string(), Act::SimilarWords(s.synonyms.clone())));
                it.extra = [Some(("Copy Definition".to_string(), it.act.clone())), Some(("Copy Word".into(), Act::Copy(word.clone()))), similar, Some(("Open in Browser".into(), Act::Open(dict::wiktionary(word))))].into_iter().flatten().collect();
                it
            });
            let maybe = a.maybe.iter().map(|(w, pos)| Item::new(w, pos.name(), Icon::Vector(g::SEARCH), Act::Define(w.clone())).sec(DID_YOU_MEAN));
            senses.chain(maybe).collect()
        }
    }
}

/// The word typing in the define view asks for once it pauses: the bar trimmed, unless it is
/// empty or the word already shown.
fn typed_word<'a>(shown: &str, query: &'a str) -> Option<&'a str> {
    Some(query.trim()).filter(|w| !w.is_empty() && *w != shown)
}

/// The word a root Define row's box holds, given the box values shown for it.
fn define_word<'a>(it: &Item, values: &'a [String]) -> Option<&'a str> {
    values.first().map(|v| v.trim()).filter(|v| it.act == Act::Cmd(Cmd::Define) && !v.is_empty())
}

/// The translator's list: a row per target (title the translation, accessory the language),
/// then the pronunciation when Google gives one. Enter copies or pastes per `action`;
/// Ctrl+Enter does the other.
fn translation_rows(tr: &TrView, cfg: &config::Translator) -> Vec<Item> {
    let source = &tr.job.text;
    let detected = tr.detected();
    let mut items: Vec<Item> = tr
        .rows
        .iter()
        .map(|t| {
            let copy = ("Copy Translation".to_string(), Act::Copy(t.text.clone()));
            let paste = ("Paste to Active App".to_string(), Act::Paste(t.text.clone()));
            let (enter, other) = if cfg.action == "paste" { (paste, copy) } else { (copy, paste) };
            // The other way round, this once: the translation becomes the text.
            let swap = translate::Job { text: t.text.clone(), from: t.to.clone(), to: vec![detected.clone()], ..tr.job.clone() };
            let mut it = Item::new(t.text.replace('\n', " "), "", Icon::Vector(g::LANGUAGES), enter.1.clone()).acc(translate::language_name(&t.to)).tinted(design::Tint::Web);
            it.extra = vec![
                enter,
                other,
                ("Copy Source".into(), Act::Copy(source.clone())),
                ("Swap Languages".into(), Act::Translate(swap)),
                ("Open in Google Translate".into(), Act::Open(translate::google_url(&tr.job.from, &t.to, source))),
                ("Change Target".into(), Act::LangMenu),
            ];
            it
        })
        .collect();
    let say = tr.rows.iter().find_map(|t| t.pronunciation.clone());
    items.extend(say.map(|p| Item::new(&p, "Pronunciation", Icon::Vector(g::AUDIO_LINES), Act::Copy(p.clone())).acc(translate::language_name(&detected))));
    items
}

/// The translator bar's Dropdown menu: a search field, Manage Language Sets…, a rule, then
/// every set with the current one selected. Ids: "manage", or the set's index.
fn lang_set_menu(sets: &[translate::LangSet], current: usize) -> menu::Menu {
    let manage = menu::Entry { id: "manage".into(), label: "Manage Language Sets…".into(), icon: Some(g::EDIT), rule_after: true, ..Default::default() };
    let sets = sets.iter().enumerate().map(|(i, s)| menu::Entry { id: i.to_string(), label: s.label(), icon: Some(g::LANGUAGES), ..Default::default() });
    let mut m = menu::Menu::new(std::iter::once(manage).chain(sets).collect(), Some("Search…".into()), menu::Place::Right);
    m.selected = current + 1;
    m
}

/// Screen Time's actions that need no row: the dashboard and the CSV.
fn screen_time_actions() -> Vec<(String, Act)> {
    vec![("Open Dashboard".into(), Act::Cmd(Cmd::OpenScreenTimeDashboard)), (Act::ExportScreenTime.label().into(), Act::ExportScreenTime)]
}

/// Screen Time while it is off: nothing is recorded, so one row says how to start.
fn screen_time_off_row() -> Vec<Item> {
    let actions = vec![("Import from micro bot".to_string(), Act::Cmd(Cmd::ImportScreenTime)), (Act::EnableScreenTime.label().to_string(), Act::EnableScreenTime)];
    vec![empty_row(g::MONITOR, "Screen Time is off".into(), "Turn it on to start tracking, or bring micro bot's history in".into(), actions)]
}

/// Screen Time's list: the span's header (its total against the span before), then a row for
/// each category, app and site. An app or site moves to another category; a category cannot.
fn screen_time_rows(p: wellbeing::Period, stats: &Value) -> Vec<Item> {
    let secs = |key: &str| stats[key].as_u64().unwrap_or(0);
    let mut head = Item::new(wellbeing::header(secs("total"), secs("previous"), p, win::local_now().0), "", Icon::Vector(g::MONITOR), Act::Cmd(Cmd::OpenScreenTimeDashboard));
    head.extra = screen_time_actions();
    let rows = wellbeing::rows(stats).into_iter().map(|r| {
        let (icon, sub, act) = match r.section {
            "Categories" => (focus::category_icon(&r.name), String::new(), Act::Cmd(Cmd::OpenScreenTimeDashboard)),
            section => (if section == "Apps" { g::APP } else { g::GLOBE }, r.category.clone(), Act::MoveToCategory(r.name.clone())),
        };
        let mut it = Item::new(&r.name, sub, Icon::Vector(icon), act.clone()).acc(wellbeing::duration(r.seconds as f64)).sec(r.section).with_hay(format!("{} {}", r.name, r.category));
        it.extra = if matches!(act, Act::MoveToCategory(_)) { vec![(act.label().to_string(), act)] } else { vec![] };
        it.extra.extend(screen_time_actions());
        it
    });
    std::iter::once(head).chain(rows).collect()
}

/// Search Notes' rows (Enter opens the note), or Recently Deleted Notes' (Enter restores and
/// opens it; Ctrl+K also deletes it for good): the title, then "Opened 2 days ago · 397
/// characters" ("● Current · 186 characters" for the Notes window's note, `current`, counted
/// with its edits; "Edited …" for a note never opened; "Deleted …" in Recently Deleted).
fn note_rows(list: NotesList, rows: &[(NoteMeta, usize)], current: Option<(String, usize)>, now: i64, searching: bool, keep_days: u32) -> Vec<Item> {
    let ago = |t: i64| timers::ago(now.saturating_sub(t).max(0) as u64);
    let items: Vec<Item> = rows
        .iter()
        .map(|(m, n)| {
            let it = match (list, m.deleted_at) {
                (NotesList::Deleted, Some(at)) => {
                    let mut it = Item::new(&m.title, format!("Deleted {} · {}", ago(at), notes::window::char_count_text(*n)), Icon::Vector(g::ARCHIVE_RESTORE), Act::RestoreNote(m.id.clone())).sec("Recently Deleted");
                    it.extra = [Act::RestoreNote(m.id.clone()), Act::PurgeNote(m.id.clone())].map(|a| (a.label().to_string(), a)).to_vec();
                    it
                }
                _ => {
                    let cur = current.as_ref().filter(|(id, _)| list == NotesList::All && *id == m.id).map(|(_, n)| *n);
                    Item::new(&m.title, notes::window::note_line(m, *n, cur, now), Icon::Vector(g::NOTEBOOK_PEN), Act::OpenNote(m.id.clone())).sec("Notes")
                }
            };
            it.tinted(design::Tint::Paste)
        })
        .collect();
    if !items.is_empty() {
        return items;
    }
    let row = match (list, searching) {
        (_, true) => empty_row(g::SEARCH, "No matching notes".into(), String::new(), vec![]),
        (NotesList::All, false) => empty_row(g::NOTEBOOK_TABS, "No notes yet".into(), "Create Note starts one".into(), vec![(Act::Cmd(Cmd::CreateNote).label().into(), Act::Cmd(Cmd::CreateNote))]),
        (NotesList::Deleted, false) => empty_row(g::ARCHIVE_RESTORE, "No recently deleted notes".into(), format!("Deleted notes stay {keep_days} days"), vec![]),
    };
    vec![row]
}

/// Manage Language Sets: Add first, then every set, the current one ticked. Enter makes a set
/// current; Ctrl+K also edits or deletes it (not the first: that one is Settings → Translator's).
fn lang_set_rows(sets: &[translate::LangSet], current: usize) -> Vec<Item> {
    let add = Item::new("Add Language Set…", "", Icon::Vector(g::PLUS), Act::EditLangSet(None));
    let rows = sets.iter().enumerate().map(|(i, s)| {
        let on = i == current;
        let mut it = Item::new(s.label(), "", Icon::Vector(if on { g::CHECK } else { g::LANGUAGES }), Act::UseLangSet(i)).acc(if on { "Current" } else { "" });
        let delete = (i > 0).then_some(Act::DeleteLangSet(i));
        it.extra = [Act::UseLangSet(i), Act::EditLangSet(Some(i))].into_iter().chain(delete).map(|a| (a.label().to_string(), a)).collect();
        it
    });
    std::iter::once(add).chain(rows).collect()
}

/// The Add (or Edit) Language Set form: three selects with search, on `set` or Auto-Detect → English.
fn lang_set_form(set: Option<&translate::LangSet>) -> Vec<FormField> {
    let select = |label: &str, lead: &[(&str, &str)], value: String| FormField {
        label: label.to_string().into(),
        ctl: Ctl::Select(translate::language_entries(lead.iter().map(|(id, l)| (id.to_string(), l.to_string()))), true),
        value,
        input: None,
        error: None,
    };
    let to = |n: usize, or: &str| set.and_then(|s| s.to.get(n).cloned()).unwrap_or_else(|| or.into());
    vec![
        select("Source language", &[("auto", "Auto-Detect")], set.map_or_else(|| "auto".into(), |s| s.from.clone())),
        select("Target language 1", &[], to(0, "en")),
        select("Target language 2", &[("", "None")], to(1, "")),
    ]
}

/// An open Dropdown: the menu, its opener's `OnPick`, and the focus to return to when it closes.
/// `site` ("Block" or "Allow"): the Focus form's Block menu, where a typed website shows as a row.
struct OpenMenu {
    menu: menu::Menu,
    on_pick: OnPick,
    back: Option<gpui::FocusHandle>,
    scroll: ScrollHandle,
    site: Option<&'static str>,
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
    /// Removes a desktop you leave once it is empty; present while the setting is on.
    desktop_watch: Option<desktops::Listener>,
    /// The index line Settings → Files shows, kept current as the index changes.
    index_status: Arc<RwLock<String>>,
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
    /// Favicon URLs already asked for this session: a site without one isn't asked again.
    favicons: HashSet<String>,
    /// Store index (None until loaded) and whether a load is running.
    store: Option<Vec<ext::StoreEntry>>,
    /// Browsers connected through the procast extension.
    browsers: browser::Browsers,
    /// Search Browser Tabs: (connection, browser name, tab) as they answer.
    tabs: Vec<(u32, String, browser::Tab)>,
    music: music_view::MusicState,
    /// Started the first time YouTube Music is used.
    media: Option<media::Media>,
    /// Quit Applications: the running apps, once listed (None while loading).
    running: Option<Vec<(apps::Running, Option<Arc<RenderImage>>)>>,
    /// Program icons by path, so the running list fills in at once next time.
    exe_icons: HashMap<String, Option<Arc<RenderImage>>>,
    autoquit: apps::AutoQuit,
    /// The Screen Time sampler while `[screen_time] enabled`, with the settings it shares.
    tracker: Option<wellbeing::Tracker>,
    st_rules: Arc<RwLock<wellbeing::Rules>>,
    st_cfg: Arc<RwLock<config::ScreenTime>>,
    /// Running timers and stopwatches, and the flag that stops the 1 s tick thread (present
    /// only while something runs).
    timers: timers::Timers,
    timer_tick: Option<Arc<AtomicBool>>,
    /// The Focus session; its countdown rides on the same tick.
    focus: focus::Focus,
    /// Calendar feeds' events; fetched and reminded of on the 10 s `Msg::Tick`.
    calendar: calendar::Calendar,
    /// Hide This Event's keys (`calendar::hidden_key`), from `calendar-hidden.json`.
    hidden_events: HashSet<String>,
    /// A meeting reminder went out: the next open selects root's Next row, so Enter joins.
    pick_next: bool,
    /// Each `open_form` is a new form: an answer for an older one never drives the one on screen.
    form_gen: u64,
    /// The form (by `form_gen`) whose Add Calendar fetch is on its way: its Ctrl+Enter waits.
    adding_calendar: Option<u64>,
    store_loading: bool,
    settings_window: Option<gpui::WindowHandle<settings::Settings>>,
    /// The note store's worker, and the Notes window once it has opened.
    notes: Notes,
    notes_window: Option<gpui::WindowHandle<NotesWindow>>,
    /// `Mode::Notes`: the last answer for the search (None until the first), and the search's
    /// number (older answers are dropped).
    note_rows: Option<Vec<(NoteMeta, usize)>>,
    note_gen: u64,
    /// `Mode::ScreenTime`: the span's stats once read (None until then), and the read's number
    /// (older answers are dropped).
    st_stats: Option<Result<Value, String>>,
    /// Its apps' and sites' time per hour, when the span is one day.
    st_hours: wellbeing::Hours,
    /// Each day's categories over the last 371 days, for the trend line and the heatmap.
    st_days: Option<Value>,
    st_gen: u64,
    matcher: Matcher,
    mode: Mode,
    items: Vec<Item>,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// Ctrl+K: the selected item's actions and which one is highlighted.
    panel: Option<(Item, usize)>,
    /// An open Dropdown; it takes the keys before anything else. Its search text is `menu_input`.
    menu: Option<OpenMenu>,
    /// The native form on screen (Mode::Form).
    form: Option<NativeForm>,
    /// Holds the keys while a form's selected control has no text box.
    form_focus: gpui::FocusHandle,
    /// Below the selected form control's left edge, as last drawn: its menu opens here.
    form_at: Rc<Cell<gpui::Point<gpui::Pixels>>>,
    menu_input: Entity<EditableTextState>,
    /// Destructive command waiting for a second Enter.
    armed: Option<usize>,
    /// A timer rebuild skipped while a panel or armed row waited; done on the next free tick.
    timers_stale: bool,
    /// The selected row's argument boxes (`arg_row`, at most `MAX_ARGS` show), and that row's key.
    args: Vec<Entity<EditableTextState>>,
    args_for: Option<String>,
    /// The box Enter found empty; it rings in danger until it is typed in.
    arg_missing: Option<usize>,
    /// By row key, this visit: the query values the boxes were last filled from, and what they hold.
    arg_memory: HashMap<String, (Vec<String>, Vec<String>)>,
    /// Each box's left edge as last drawn: a list opens under it.
    arg_left: Rc<[Cell<gpui::Pixels>; MAX_ARGS]>,
    /// Mode::Args: its one row, and for a snippet keyword typed in an app, what to erase and type after.
    args_row: Option<Item>,
    typed: (usize, String),
    /// The query before its last change: a space typed after a command's name jumps into its boxes.
    last_query: String,
    /// Clipboard image thumbnails (by image id, `t:`) and previews (`p:`), each with its last
    /// use (`file_tick`) for `evict`.
    clip_images: HashMap<String, (Option<Arc<RenderImage>>, u64)>,
    /// File Search detail pane: the selected file's information (read when the selection
    /// changes), and its decoded pictures (`p:`), shell icons (`i:`) and Search Screenshots'
    /// thumbnails (`t:`) by path, each with its last use (`file_tick`) for `evict`.
    file_info: Option<(String, Option<files::FileInfo>)>,
    file_images: HashMap<String, (Option<Arc<RenderImage>>, u64)>,
    file_tick: u64,
    /// Counts selection changes in File Search (and Quick Look's pictures asked for); a preview
    /// worker waits for its count to hold.
    file_seq: Arc<AtomicU64>,
    /// The emoji and screenshot grids' rows; only the ones on screen are laid out.
    grid_list: gpui::ListState,
    /// Script output view: (index, output, still running).
    output: Option<(usize, String, bool)>,
    ai: Option<AiChat>,
    /// The translator view (None until it first opens).
    translation: Option<TrView>,
    /// Number of the newest translation request; answers to older ones are dropped.
    translate_seq: u64,
    /// The language set in use before the last switch, for Enter in the empty translator.
    prev_set: Option<usize>,
    /// The define view (None until it first opens).
    define: Option<DefView>,
    /// Number of the newest lookup; answers to older ones are dropped.
    define_seq: u64,
    /// Number of the newest change typed in the define view; a pause timer older than it does nothing.
    define_typed: u64,
    /// Whether the dictionary is downloaded, checked off the UI thread each time the launcher
    /// shows (None until that check answers).
    dict_ready: Option<bool>,
    /// The root Define row's subtitles: each word's first short sense (None: not in the
    /// dictionary), and the newest request's number.
    senses: HashMap<String, Option<String>>,
    sense_seq: u64,
    /// Screenshot search: the index as last scanned, the newest scan's number (older scans' answers are dropped) and whether it is still reading pictures.
    shots: shots::ShotIndex,
    shots_seq: u64,
    shots_busy: bool,
    /// How many pictures the scan has read of how many, while it reads them: the banner.
    shots_progress: Option<(usize, usize)>,
    /// The bar's dropdown: which screenshots show.
    shot_kind: shots::Kind,
    /// Each grid cell's thumbnail key (`shots::thumb_key`; None for a video).
    shot_thumbs: Vec<Option<String>>,
    /// The pinned screenshots (`shots::pins`), read as each scan starts.
    shot_pins: HashSet<PathBuf>,
    /// Quick Look (Ctrl+Y): the selected screenshot shows large in place of the grid.
    quick_look: bool,
    /// The path Quick Look shows and its `shots::detail` line, read once per shot.
    quick_look_detail: (String, String),
    /// The grid row at the top of the screenshot grid: a decode for a row scrolled far from
    /// it is dropped before it starts.
    grid_top: Arc<AtomicUsize>,
    ai_scroll: ScrollHandle,
    /// Short-lived message in the footer.
    /// The action bar message, with how it should look.
    status: Option<(Tone, String)>,
    /// Config or extension problem, shown above the results.
    notice: Option<String>,
    hotkey_errors: Vec<Option<String>>,
    hwnd: HWND,
    prev: HWND,
    visible: bool,
    /// The toast window (toast.rs), set once it exists.
    toast: Option<gpui::WindowHandle<toast::Toast>>,
    /// The procast notification-area icon (Settings → General → Show in tray).
    tray_icon: Option<TrayIcon>,
    /// Auto HDR (own thread).
    hdr: hdr::Auto,
    /// The displays' HDR state for Toggle HDR's row, read off the UI thread (`Msg::Hdr`);
    /// None until the first read is in.
    hdr_status: Option<Vec<hdr::HdrDisplay>>,
    /// The number of the last HDR read asked for (`hdr_read`).
    hdr_seq: u64,
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
        let menu_input = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
        let args: Vec<_> = (0..MAX_ARGS).map(|_| cx.new(|cx| EditableTextState::new(StringStorage::default(), cx))).collect();
        let mut subs = vec![
            cx.subscribe_in(&input, window, |this, _, _: &TextChanged, window, cx| this.query_changed(window, cx)),
            cx.subscribe(&menu_input, |this, input, _: &TextChanged, cx| {
                if let Some(open) = &mut this.menu {
                    let typed = input.read(cx).as_str().to_string();
                    if let Some(verb) = open.site {
                        site_lead(&mut open.menu.entries, verb, &typed);
                    }
                    open.menu.set_filter(&typed);
                    cx.notify();
                } else if let Some((_, sel)) = &mut this.panel {
                    // The ActionPanel's search field: its first match is selected.
                    *sel = 0;
                    cx.notify();
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if this.visible && !window.is_window_active() {
                    this.hide(false, cx);
                }
            }),
        ];
        for (i, a) in args.iter().enumerate() {
            subs.push(cx.subscribe_in(a, window, move |this, _, _: &TextChanged, window, cx| this.arg_changed(i, window, cx)));
        }
        clipboard::configure(&cfg.clipboard.ignore_apps, cfg.clipboard.ocr);
        clipboard::watch(tx.clone());
        for (dir, recursive) in win::app_dirs() {
            let tx = tx.clone();
            std::thread::spawn(move || win::watch_dir(&dir, recursive, |_| tx.unbounded_send(Msg::AppsChanged).is_ok()));
        }
        foreground::set_games(&cfg.games.list);
        foreground::watch(tx.clone());
        std::thread::spawn(|| _ = browser::register());
        let browsers = browser::serve(tx.clone());
        let auto_hdr = hdr::Auto::start(tx.clone());
        let notes = Notes::start(cfg.notes.dir(), cx);
        let st_rules = Arc::new(RwLock::new(wellbeing::rules(&focus::categories(&cfg.focus_category))));
        let st_cfg = Arc::new(RwLock::new(cfg.screen_time.clone()));
        let tracker = cfg.screen_time.enabled.then(|| start_screen_time(&tx, &browsers, &st_rules, &st_cfg));
        let mut this = Self {
            input,
            hotkeys: {
                configure_keys(&cfg, &tx);
                win::Hotkeys::spawn(hotkey_specs(&cfg), tx.clone())
            },
            history: clipboard::History::load(config::dir().join("clipboard.bin"), cfg.clipboard.history, &cfg.clipboard.keep),
            files: start_files(&cfg, &tx),
            desktop_watch: cfg.window.auto_close_empty_desktops.then(desktops::Listener::start).flatten(),
            index_status: Arc::default(),
            scripts: scripts::scan(&cfg.scripts.dirs()),
            exts: vec![],
            catalog: vec![],
            cfg,
            tx,
            apps: vec![],
            scanned: None,
            running: None,
            browsers,
            tabs: vec![],
            music: Default::default(),
            media: None,
            exe_icons: HashMap::new(),
            autoquit: apps::AutoQuit::default(),
            tracker,
            st_rules,
            st_cfg,
            timers: timers::Timers::load(config::dir().join("timers.json")),
            timer_tick: None,
            focus: focus::Focus::load(config::dir().join("focus.json")),
            calendar: calendar::Calendar::default(),
            hidden_events: calendar::load_hidden(&config::dir().join(CALENDAR_HIDDEN)),
            pick_next: false,
            form_gen: 0,
            adding_calendar: None,
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
            favicons: HashSet::new(),
            store: None,
            store_loading: false,
            settings_window: None,
            notes,
            notes_window: None,
            note_rows: None,
            note_gen: 0,
            st_stats: None,
            st_hours: Default::default(),
            st_days: None,
            st_gen: 0,
            matcher: Matcher::new(nucleo_matcher::Config::DEFAULT),
            mode: Mode::Root,
            items: vec![],
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            panel: None,
            menu: None,
            menu_input,
            form: None,
            form_focus: cx.focus_handle(),
            form_at: Rc::new(Cell::new(gpui::Point::default())),
            armed: None,
            timers_stale: false,
            args,
            args_for: None,
            arg_missing: None,
            arg_memory: HashMap::new(),
            arg_left: Rc::new(std::array::from_fn(|_| Cell::new(gpui::px(0.)))),
            args_row: None,
            typed: (0, String::new()),
            last_query: String::new(),
            clip_images: HashMap::new(),
            file_info: None,
            file_images: HashMap::new(),
            file_tick: 0,
            file_seq: Arc::new(AtomicU64::new(0)),
            grid_list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(200.)),
            output: None,
            translation: None,
            translate_seq: 0,
            prev_set: None,
            define: None,
            define_seq: 0,
            define_typed: 0,
            dict_ready: None,
            senses: HashMap::new(),
            sense_seq: 0,
            shots: shots::ShotIndex::default(),
            shots_seq: 0,
            shots_busy: false,
            shots_progress: None,
            shot_kind: shots::Kind::All,
            shot_thumbs: vec![],
            shot_pins: HashSet::new(),
            quick_look: false,
            quick_look_detail: Default::default(),
            grid_top: Arc::new(AtomicUsize::new(0)),
            ai: None,
            ai_scroll: ScrollHandle::new(),
            status: None,
            notice,
            hotkey_errors: vec![],
            hwnd: HWND::default(),
            prev: HWND::default(),
            visible: false,
            toast: None,
            tray_icon: None,
            hdr: auto_hdr,
            hdr_status: None,
            hdr_seq: 0,
            hidden_at: None,
            searches: std::fs::read_to_string(config::dir().join("search-history.txt")).unwrap_or_default().lines().map(String::from).collect(),
            search_pos: None,
            peek: false,
            height: 0.,
            _subs: subs,
        };
        this.load_extensions();
        this.watch_timers();
        this.purge_old_notes(cx);
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
        let value = toml_edit::Value::Array(list.iter().map(String::as_str).collect());
        match config::edit_file(&config::path(), |t| config::write_value(t, &["launcher".into(), name.into()], value)) {
            Ok(_) => {
                if name == "favorites" {
                    self.cfg.launcher.favorites = list;
                } else {
                    self.cfg.launcher.hidden = list;
                }
                self.rebuild_catalog(cx);
            }
            Err(e) => self.status = Some((Tone::Error, e)),
        }
        cx.notify();
    }

    fn query_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.query(cx);
        let space_typed = q.strip_suffix(' ') == Some(self.last_query.as_str());
        self.last_query = q.clone();
        self.panel = None;
        self.send_search(cx);
        if self.mode == Mode::Translate {
            self.translate_soon(cx);
        }
        if self.mode == Mode::Define {
            self.define_soon(cx);
        }
        self.selected = 0;
        self.refresh(cx);
        if space_typed && self.arg_row().is_some_and(|it| search::named_in(it, q.trim_start(), &self.cfg) == Some("")) {
            self.focus_arg(0, window, cx);
        }
        self.music_typed(cx);
    }

    // ---- argument boxes (the decisions are in search.rs) ----------------------------

    /// The row whose boxes show: the selected one in root search (or Mode::Args), when it has arguments.
    fn arg_row(&self) -> Option<&Item> {
        matches!(self.mode, Mode::Root | Mode::Args).then(|| self.items.get(self.selected)).flatten().filter(|it| !it.args.is_empty())
    }

    fn arg_values(&self, cx: &App) -> Vec<String> {
        let n = self.arg_row().map_or(0, |it| it.args.len());
        self.args[..n].iter().map(|a| a.read(cx).as_str().to_string()).collect()
    }

    /// The box with keyboard focus, if one shows.
    fn arg_focused(&self, window: &Window, cx: &App) -> Option<usize> {
        let n = self.arg_row().map_or(0, |it| it.args.len());
        self.args[..n].iter().position(|a| a.focus_handle(cx).is_focused(window))
    }

    fn focus_arg(&self, i: usize, window: &mut Window, cx: &mut App) {
        window.focus(&self.args[i].focus_handle(cx), cx);
    }

    /// Fills the boxes when another row is selected or the query fills them anew: a new query
    /// value wins, else what this row's boxes held earlier in the visit.
    fn sync_args(&mut self, cx: &mut Context<Self>) {
        let Some(it) = self.arg_row() else {
            self.args_for = None;
            return;
        };
        let key = it.key.clone().unwrap_or_else(|| it.title.clone());
        let mut seed = it.values.clone();
        // Edit Focus Session starts from the running session: goal, minutes and what it blocks.
        if seed.is_empty()
            && it.act == Act::Cmd(Cmd::Focus(FocusCmd::Edit))
            && let Some(s) = &self.focus.session
        {
            seed = focus::edit_seed(s);
        }
        let kept = self.arg_memory.get(&key).filter(|(s, _)| *s == seed).map(|m| m.1.clone());
        if self.args_for.as_ref() == Some(&key) && kept.is_some() {
            return;
        }
        let values = kept.unwrap_or_else(|| seed.clone());
        self.arg_memory.insert(key.clone(), (seed, values.clone()));
        self.args_for = Some(key);
        self.arg_missing = None;
        for (i, a) in self.args.iter().enumerate() {
            let v = values.get(i).cloned().unwrap_or_default();
            a.update(cx, |s, cx| s.emplace(&v, cx));
        }
    }

    /// Box `i` changed: it is no longer missing, a space in a one-word box moves on, and the
    /// row remembers the values.
    fn arg_changed(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(arg) = self.arg_row().and_then(|it| it.args.get(i)).cloned() else { return };
        if self.arg_missing == Some(i) {
            self.arg_missing = None;
            self.status = None;
        }
        let text = self.args[i].read(cx).as_str().to_string();
        if arg.word
            && let Some((w, rest)) = text.split_once(' ')
        {
            let (w, rest) = (w.to_string(), rest.trim().to_string());
            self.args[i].update(cx, |s, cx| s.emplace(&w, cx));
            if i + 1 < self.arg_row().map_or(0, |it| it.args.len()) {
                if !rest.is_empty() {
                    self.args[i + 1].update(cx, |s, cx| s.emplace(&rest, cx));
                }
                self.focus_arg(i + 1, window, cx);
            }
            return;
        }
        let values = self.arg_values(cx);
        if let Some(m) = self.args_for.as_ref().and_then(|k| self.arg_memory.get_mut(k)) {
            m.1 = values;
        }
        self.ask_sense(cx);
        cx.notify();
    }

    /// Tab (+1) and Shift+Tab (-1) while boxes show: from the query into box 0, box to box,
    /// from box 0 back to the query. Never to another row.
    fn step_arg(&mut self, d: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.arg_row().map_or(0, |it| it.args.len());
        match (self.arg_focused(window, cx), d) {
            _ if self.menu.is_some() => {}
            (None, 1) => self.focus_arg(0, window, cx),
            (Some(0), -1) => window.focus(&self.input.focus_handle(cx), cx),
            (Some(i), 1) if i + 1 < n => self.focus_arg(i + 1, window, cx),
            (Some(i), -1) => self.focus_arg(i - 1, window, cx),
            _ => {}
        }
        cx.notify();
    }

    /// Down on a list box, or a click on it: its menu with a search field, under the box.
    /// Typing goes to that field, never to the query behind it.
    fn open_choice(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search::ArgKind::Choice(opts)) = self.arg_row().and_then(|it| it.args.get(i)).map(|a| a.kind.clone()) else { return };
        let cur = self.args[i].read(cx).as_str().trim().to_string();
        let entries = opts
            .iter()
            .enumerate()
            .map(|(k, (id, label))| menu::Entry { id: label.clone(), label: label.clone(), off: id.is_empty(), rule_after: opts.get(k + 1).is_some_and(|n| n.0.is_empty()), ..Default::default() })
            .collect();
        // Kept inside the window on the right.
        let room = window.viewport_size().width - px(design::POP_RIGHT_W + design::sp::S12);
        let left = self.arg_left[i].get();
        let mut m = menu::Menu::new(entries, Some("Search…".into()), menu::Place::Under(if left > room { room } else { left }));
        m.selected = opts.iter().position(|o| o.1.eq_ignore_ascii_case(&cur)).unwrap_or(m.selected);
        self.focus_arg(i, window, cx);
        self.open_menu(m, move |this, label, _, cx| this.args[i].update(cx, |s, cx| s.emplace(&label, cx)), window, cx);
    }

    /// Opens `it` alone with its boxes, box 0 focused (Mode::Args).
    fn ask_args(&mut self, it: Item, window: &mut Window, cx: &mut Context<Self>) {
        self.args_row = Some(it);
        if self.visible {
            self.set_mode(Mode::Args, cx);
        } else {
            self.show(Mode::Args, window, cx);
        }
        self.focus_arg(0, window, cx);
    }

    /// When `act` is a row with arguments run without their values (a hotkey, a favourite,
    /// another command), asks for them in Mode::Args and says true.
    fn ask_args_for(&mut self, act: &Act, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(it) = self.catalog.iter().find(|it| it.act == *act && !it.args.is_empty()).cloned() else { return false };
        if !self.asks_args(act) {
            if let Ok(act) = search::with_args(&it, &[]) {
                self.run(act, None, window, cx);
            }
            return true;
        }
        self.ask_args(it, window, cx);
        true
    }

    /// Whether running `act` without values opens its boxes. A command whose boxes are all
    /// optional and that shows nothing else (Start Stopwatch, Translate filled with the
    /// selection, Start and Edit Focus Session, which open their form, an extension command
    /// without a view) runs at once instead.
    fn asks_args(&self, act: &Act) -> bool {
        let Some(it) = self.catalog.iter().find(|it| it.act == *act && !it.args.is_empty()) else { return false };
        let at_once = match act {
            Act::Cmd(Cmd::StartStopwatch | Cmd::Translate | Cmd::CreateNote | Cmd::Focus(FocusCmd::Start | FocusCmd::Edit)) => true,
            Act::ExtRun(e, c) => self.exts.get(*e).and_then(|(x, _)| x.commands.get(*c)).is_some_and(|c| c.mode != "view"),
            _ => false,
        };
        !(at_once && it.args.iter().all(|a| a.optional))
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
        // ponytail: dropped wholesale past 200 images; per-entry LRU if lists get that big.
        if self.images.len() > 200 && !self.images.contains_key(&path) {
            self.images.clear();
        }
        self.images.entry(path.clone()).or_insert_with(|| ext::load_image(&path)).clone()
    }

    /// An extension icon value: "icon:Name", an emoji, an asset file or a URL.
    /// A web image (http only): drawn from the cache once downloaded, `glyph` until then.
    fn web_icon(&mut self, url: &str, glyph: &'static str) -> Icon {
        net::cached_image(url, &self.tx).and_then(|path| self.image(path)).map_or(Icon::Vector(glyph), Icon::Image)
    }

    fn ext_icon(&mut self, ext: usize, icon: Option<&str>) -> Icon {
        let Some(icon) = icon else { return Icon::Vector(g::BULLETS) };
        if let Some(name) = icon.strip_prefix("icon:") {
            return Icon::Vector(ext::icon_glyph(name));
        }
        if icon.starts_with("http") {
            return self.web_icon(icon, g::GLOBE);
        }
        if icon.starts_with("data:") {
            return Icon::Vector(g::DOCUMENT);
        }
        if icon.chars().count() <= 4 && !icon.contains('.') {
            return Icon::glyph(icon.to_string());
        }
        let dir = self.exts[ext].0.dir.join("assets");
        let path = if std::path::Path::new(icon).is_absolute() { PathBuf::from(icon) } else { dir.join(icon) };
        match self.image(path) {
            Some(i) => Icon::Image(i),
            None => Icon::Vector(g::BULLETS),
        }
    }

    fn load_extensions(&mut self) {
        let exts = ext::scan();
        let mut out = Vec::with_capacity(exts.len());
        for e in exts {
            let icon = e.icon.clone().and_then(|p| self.image(p)).map(Icon::Image).unwrap_or(Icon::Vector(g::PUZZLE));
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
                let mut row = Item::new(view.title.clone().unwrap_or_default(), "", Icon::Vector(g::BULLETS), view_actions.first().map(|a| a.1.clone()).unwrap_or(Act::None));
                row.extra = view_actions;
                vec![row]
            }
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let max = self.cfg.max_results.max(1);
        let keep_selection = matches!(self.mode, Mode::Ext | Mode::Music);
        self.armed = None;
        self.items = match self.mode {
            _ if let Some(c) = &self.confirm => confirm_rows(c),
            Mode::Root => {
                // Matching files arrive later (Msg::Files) and slot in above the fallbacks.
                if self.cfg.launcher.files
                    && query.trim().chars().count() >= 3
                    && let Some(f) = &self.files
                {
                    self.file_gen += 1;
                    f.search(self.file_gen, &query, 5);
                }
                let mut rows = self.focus_rows(&query);
                rows.extend(self.timer_rows(&query));
                if query.trim().is_empty() {
                    rows.extend(self.next_rows());
                }
                let found = search::root(&query, &self.catalog, &self.cfg, &self.frec, &mut self.matcher);
                let mut rows = head_rows(rows, found, &query, &self.cfg);
                hdr_state(&mut rows, self.hdr_status.as_deref());
                if wants_auto_hdr(&query) {
                    // Under the results, above the "Use … with" fallbacks.
                    let at = rows.iter().position(|it| it.section.as_deref().is_some_and(|s| s.starts_with("Use \""))).unwrap_or(rows.len());
                    rows.insert(at, auto_hdr_row(self.cfg.games.auto_hdr));
                }
                rows
            }
            Mode::Clipboard => {
                let (history, cache, tick) = (&self.history, &mut self.clip_images, &mut self.file_tick);
                let mut thumb = |id: &str| used(cache, tick, format!("t:{id}"), || history.png(id).and_then(|p| clipboard::render_image(&p, 64)));
                let rows = search::clipboard(&query, &history.entries, 200, &mut self.matcher, &mut thumb);
                // After the rows took theirs, so the ones on screen are the newest used.
                release(evict(&mut self.clip_images, FILE_IMAGES_MAX, FILE_IMAGES_KEEP), cx);
                rows
            }
            Mode::Emoji => search::emoji(&query, 400, &mut self.matcher, &self.cfg.emoji),
            Mode::Shots => {
                let (rows, thumbs) = shots::rows(&self.shots, &query, self.shot_kind, &self.cfg.screenshots.primary);
                self.shot_thumbs = thumbs;
                rows
            }
            // A form draws its own body.
            Mode::Form(_) => vec![],
            Mode::Schedule => {
                let rows = self.schedule_rows();
                search::filter(&query, rows, &mut self.matcher)
            }
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
            Mode::Args => self.args_row.iter().cloned().collect(),
            // Typed text is the next question; with an empty bar, Enter copies the answer.
            Mode::Ai if !query.trim().is_empty() => vec![Item::new("Ask", "", Icon::Vector(g::SPARKLES), Act::Ask(query.trim().to_string()))],
            Mode::Ai => self.ai.iter().filter_map(|a| a.turns.last()).filter(|t| !t.text.is_empty()).map(|t| Item::new("Copy answer", "", Icon::Vector(g::COPY), Act::Copy(t.text.clone()))).collect(),
            Mode::Translate => match &self.translation {
                _ if query.trim().is_empty() => {
                    let back = self.prev_set.filter(|&p| p < translate::sets(&self.cfg).len()).map(|p| ("Previous Language Set".to_string(), Act::UseLangSet(p)));
                    vec![empty_row(g::LANGUAGES, "Type to translate".into(), "Or select text anywhere before opening Translate".into(), back.into_iter().collect())]
                }
                Some(tr) => translation_rows(tr, &self.cfg.translator),
                None => vec![],
            },
            Mode::LanguageSets => search::filter(&query, lang_set_rows(&translate::sets(&self.cfg), translate::current(&self.cfg)), &mut self.matcher),
            Mode::Notes(list) => {
                // The store answers on its worker (`search_notes`); the last answer shows meanwhile.
                self.search_notes(list, &query, cx);
                self.note_items(list, &query, cx)
            }
            Mode::ScreenTime(p) => self.screen_time_items(p, &query),
            Mode::Define => match &self.define {
                _ if query.trim().is_empty() => vec![empty_row(g::BOOK_OPEN, "Type a word to define".into(), String::new(), vec![])],
                Some(v) => define_rows(v, &self.cfg.search.fallback),
                None => vec![],
            },
            Mode::Output => match &self.output {
                Some((_, _, true)) => vec![empty_row(g::CLOCK, "Running…".into(), String::new(), vec![])],
                Some((_, text, false)) => {
                    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                    let shown = if lines.is_empty() { vec!["(no output)"] } else { lines };
                    shown.iter().map(|l| Item::new(*l, "", Icon::Vector(g::TERMINAL), Act::Copy(text.clone())).sec("Output")).collect()
                }
                None => vec![],
            },
            Mode::Ext => self.ext_rows(&query),
            Mode::Store => match &self.store {
                None => vec![empty_row(g::CLOCK, "Loading the store index…".into(), "The first time fetches every extension's manifest (~1 min)".into(), vec![])],
                Some(entries) => {
                    let installed: Vec<&str> = self.exts.iter().map(|(x, _)| x.name.as_str()).collect();
                    let rows: Vec<Item> = entries
                        .iter()
                        .filter(|s| s.windows)
                        .map(|s| {
                            let done = installed.contains(&s.name.as_str());
                            let mut it = Item::new(&s.title, &s.description, Icon::Vector(if done { g::CHECK } else { g::PUZZLE }), Act::Install(s.name.clone()))
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
            Mode::Running => match &self.running {
                None => vec![empty_row(g::CLOCK, "Looking for running apps…".into(), String::new(), vec![])],
                Some(list) => {
                    let rows: Vec<Item> = list
                        .iter()
                        .map(|(r, icon)| {
                            let file = std::path::Path::new(&r.exe).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                            let windows = if r.windows.len() > 1 { format!("{} windows", r.windows.len()) } else { String::new() };
                            let name = if r.name.is_empty() { file.clone() } else { r.name.clone() };
                            // "Reddit - Google Chrome" -> "Reddit"; nothing when it just repeats the name.
                            let mut title = r.title.as_str();
                            for sep in [" - ", " – ", " — "] {
                                if let Some((head, tail)) = title.rsplit_once(sep)
                                    && name.split_whitespace().any(|w| tail.contains(w))
                                {
                                    title = head;
                                }
                            }
                            let title = if title == name { "" } else { title };
                            Item::new(&name, title, icon.clone().map(Icon::Image).unwrap_or(Icon::Vector(g::APP)), Act::QuitApp(r.pid))
                                .acc(windows)
                                .with_hay(format!("{name} {} {file}", r.title))
                        })
                        .collect();
                    search::filter(&query, rows, &mut self.matcher)
                }
            },
            Mode::Tabs => {
                if !self.browsers.any() {
                    vec![empty_row(g::GLOBE, "No browser connected".into(), "Run Set Up Browser Extension, then load the extension in Chrome, Edge or Brave".into(), vec![])]
                } else if self.tabs.is_empty() {
                    vec![empty_row(g::CLOCK, "Asking your browsers…".into(), String::new(), vec![])]
                } else {
                    let rows: Vec<Item> = self
                        .tabs
                        .clone()
                        .into_iter()
                        .map(|(conn, browser, tab)| {
                            let host = tab.url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or(&tab.url).to_string();
                            let icon = self.web_icon(&tab.icon, g::GLOBE);
                            let title = if tab.title.is_empty() { tab.url.clone() } else { tab.title.clone() };
                            Item::new(&title, &host, icon, Act::Tab(conn, tab.id)).acc(browser).with_hay(format!("{title} {}", tab.url))
                        })
                        .collect();
                    search::filter(&query, rows, &mut self.matcher)
                }
            }
            Mode::Music => self.music_rows(&query),
            Mode::Install => {
                let what = query.trim();
                let title = if what.is_empty() { "Install an extension".to_string() } else { format!("Install {what}") };
                vec![empty_row(g::DOWNLOAD, title, "A store name (e.g. obsidian) or a folder path, then Enter · needs git, Node and npm".into(), vec![])]
            }
        };
        if self.items.is_empty() && self.mode == Mode::Clipboard {
            let hint = if query.trim().is_empty() { "Text, images and files you copy show up here" } else { "Nothing copied matches" };
            self.items = vec![empty_row(g::PASTE, if query.trim().is_empty() { "Nothing copied yet" } else { "No matches" }.into(), hint.into(), vec![])];
        }
        if self.items.is_empty() && self.mode == Mode::Shots {
            let empty = shots_empty(self.shots_busy, !query.trim().is_empty() || self.shot_kind != shots::Kind::All);
            let hint = match empty {
                ShotsEmpty::Reading => "Text in new pictures is found in the background".to_string(),
                ShotsEmpty::NoMatches => "Nothing in your screenshots matches".to_string(),
                ShotsEmpty::NoScreenshots => shots::folders(&self.cfg.screenshots.folders).iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(", "),
            };
            let add = (empty == ShotsEmpty::NoScreenshots).then(|| (Act::AddShotFolder.label().to_string(), Act::AddShotFolder));
            self.items = vec![empty_row(g::SCAN_SEARCH, empty.title().into(), hint, add.into_iter().collect())];
        }
        if self.items.is_empty() && self.mode == Mode::Emoji {
            self.items = vec![empty_row(g::EMOJI, "No emoji found".into(), "Try another word, e.g. heart or smile".into(), vec![])];
        }
        if matches!(self.mode, Mode::Emoji | Mode::Shots) {
            let n = grid_rows(&self.items, self.grid_cols()).len();
            // A batch of read text keeps the screenshot grid where it is scrolled.
            if self.mode == Mode::Emoji || n != self.grid_list.item_count() {
                self.grid_list.reset(n);
            }
        }
        let _ = max;
        self.selected = if keep_selection {
            self.selected.min(self.items.len().saturating_sub(1))
        } else if self.mode == Mode::Root {
            root_selection(&self.items, &query, &self.cfg, self.pick_next)
        } else if self.mode == Mode::Translate {
            self.translation.as_ref().map_or(0, |tr| tr.first)
        } else {
            0
        };
        if self.mode == Mode::Music && self.grid() {
            // The visible rows redraw every frame: only a changed layout needs the list told.
            let rows = grid_rows(&self.items, self.grid_cols()).len();
            if !(keep_selection && self.selected > 0 && rows == self.grid_list.item_count()) {
                self.grid_list.reset(rows);
                if self.selected > 0 {
                    self.scroll_to_selected();
                }
            }
        }
        self.sync_args(cx);
        self.ask_sense(cx);
        self.scroll_to_selected();
        self.request_resize(cx);
        cx.notify();
    }

    /// `refresh`, with the selection kept on the row it was on (`same_row`): pausing the second
    /// timer, or a tick that adds a Running or Next row above, must not move Enter onto another row.
    fn refresh_keeping(&mut self, cx: &mut Context<Self>) {
        let was = self.items.get(self.selected).cloned();
        self.refresh(cx);
        if let Some(i) = was.and_then(|w| same_row(&self.items, &w)) {
            self.selected = i;
            self.sync_args(cx);
            self.scroll_to_selected();
        }
    }

    /// Running timers and stopwatches as the Running section (`Timers::matching` picks them).
    fn timer_rows(&self, query: &str) -> Vec<Item> {
        self.timers.matching(SystemTime::now(), query).into_iter().map(running_row).collect()
    }

    /// Starts the 1 s tick while a timer runs; the thread ends when its flag is set.
    fn watch_timers(&mut self) {
        if self.timer_tick.is_some() || (!self.timers.ticking() && !self.focus.ticking()) {
            return;
        }
        let (stop, tx) = (Arc::new(AtomicBool::new(false)), self.tx.clone());
        self.timer_tick = Some(stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(1));
                if stop.load(Ordering::Relaxed) || tx.unbounded_send(Msg::TimerTick).is_err() {
                    break;
                }
            }
        });
    }

    /// Announces timers that ran out (a sticky toast and the system sound), keeps the visible
    /// Running rows current, ends the tick when nothing counts any more.
    fn on_timer_tick(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now();
        self.on_focus_tick(cx);
        let due = self.timers.due(now);
        for (_, label, length) in &due {
            self.say(Msg::sticky(toast::Kind::Done, timers::done_text(label, *length)));
            timers::beep();
        }
        if !self.timers.ticking()
            && !self.focus.ticking()
            && let Some(stop) = self.timer_tick.take()
        {
            stop.store(true, Ordering::Relaxed);
        }
        // Hidden: nothing to redraw; the toasts above still go out.
        if !self.visible || self.mode != Mode::Root {
            return;
        }
        let shown: Vec<u64> = self.items.iter().filter_map(timer_id).collect();
        let rows = self.timers.matching(now, &self.query(cx));
        // A timer that finished, appeared or went away rebuilds the list; otherwise only the times
        // change. An open panel or an armed row (Join's second Enter) waits, as in `calendar_tick`:
        // a rebuild would close the question; the times still tick in place.
        let waits = self.panel.is_some() || self.armed.is_some();
        let changed = !due.is_empty() || timers::rows_changed(&shown, &rows) || self.timers_stale;
        self.timers_stale = waits && changed;
        if !waits && changed {
            return self.refresh_keeping(cx);
        }
        for it in &mut self.items {
            if let Some(id) = timer_id(it)
                && let Some(r) = rows.iter().find(|r| r.id == id)
            {
                it.accessory = r.accessory();
            }
        }
        cx.notify();
    }

    // ---- My Schedule (the decisions are in calendar.rs) ------------------------------

    /// The `[[calendar_feed]]` addresses, in order (an event's `feed` indexes them).
    fn feed_urls(&self) -> Vec<String> {
        self.cfg.calendar_feed.iter().map(|f| f.url.clone()).collect()
    }

    fn calendar_events(&self) -> Vec<calendar::Event> {
        self.calendar.events(&self.feed_urls(), &self.cfg.calendar.ignore, &self.hidden_events)
    }

    /// An event's row, with its calendar's name and address.
    fn event_row(&self, e: &calendar::Event, accessory: String) -> Item {
        let feed = self.cfg.calendar_feed.get(e.feed);
        calendar::row(e, (feed.map_or("", |f| f.name.as_str()), feed.map_or("", |f| f.url.as_str())), accessory)
    }

    /// The Next section of an empty root search: the meeting about to start (`Calendar::next`).
    fn next_rows(&self) -> Vec<Item> {
        let now = search::now() as i64;
        let events = self.calendar_events();
        self.calendar.next(&events, now, self.cfg.calendar.next_in_search).map(|e| self.event_row(e, calendar::in_minutes(e, now)).sec(NEXT)).into_iter().collect()
    }

    /// My Schedule: Now, Today and Tomorrow, or why there is nothing.
    fn schedule_rows(&self) -> Vec<Item> {
        if self.cfg.calendar_feed.iter().all(|f| f.url.trim().is_empty()) {
            let add = vec![(Act::AddCalendar.label().to_string(), Act::AddCalendar)];
            return vec![empty_row(g::CALENDAR_CLOCK, "No calendar connected".into(), "Paste your calendar's ICS address to see today's meetings here".into(), add)];
        }
        let now = search::now() as i64;
        let sections = calendar::sections(&self.calendar_events(), now);
        if sections.is_empty() {
            let text = if self.calendar.pending() { "Fetching your calendar…" } else { "Nothing on today or tomorrow" };
            // Enter is Add Calendar only with no feed; here it waits under Ctrl+K.
            let mut it = empty_row(g::CALENDAR_CLOCK, text.into(), String::new(), vec![]);
            it.extra = vec![(Act::AddCalendar.label().to_string(), Act::AddCalendar)];
            return vec![it];
        }
        let mut rows = vec![];
        for (name, events) in sections {
            for e in events {
                let on = name == "Now";
                let mut it = self.event_row(&e, if on { calendar::until(&e) } else { calendar::span(&e) }).sec(name);
                it.now = on;
                it.extra.push((Act::AddCalendar.label().to_string(), Act::AddCalendar));
                rows.push(it);
            }
        }
        rows
    }

    /// The meeting Enter on `act` opens, (title, link): a Join row's, or Join Meeting's (`calendar::joinable`).
    fn join_target(&self, act: &Act) -> Option<(String, String)> {
        match act {
            Act::Join(title, url) => Some((title.clone(), url.clone())),
            Act::Cmd(Cmd::JoinMeeting) => calendar::joinable(&self.calendar_events(), search::now() as i64).and_then(|e| Some((e.title.clone(), e.join.clone()?))),
            _ => None,
        }
    }

    /// Join Meeting's hotkey with `confirm_join`: root search opens on the meeting's row, armed,
    /// so the bar asks "Join <title> on <host>? Press ↵ again" as Enter on the row does.
    fn ask_join(&mut self, cx: &mut Context<Self>) {
        let now = search::now() as i64;
        let events = self.calendar_events();
        let Some(e) = calendar::joinable(&events, now) else { return };
        let row = if e.start <= now { self.event_row(e, calendar::until(e)).sec("Now") } else { self.event_row(e, calendar::in_minutes(e, now)).sec(NEXT) };
        self.items.retain(|it| it.section.as_deref() != Some(NEXT));
        self.items.insert(0, row);
        self.selected = 0;
        self.armed = Some(0);
        self.scroll_to_selected();
        cx.notify();
    }

    /// Ctrl+Enter on Add Calendar: fetches the address once on the background executor; the
    /// answer comes back as `Msg::CalendarAdded`.
    fn add_calendar(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.form else { return };
        if self.adding_calendar == Some(self.form_gen) {
            self.status = Some((Tone::Info, READING.into()));
            return cx.notify();
        }
        let v = form_values(&form.fields);
        let url = v["ICS address"].trim().to_string();
        if self.cfg.calendar_feed.iter().any(|f| calendar::same_feed(&f.url, &url)) {
            self.status = Some((Tone::Error, calendar::ALREADY_ADDED.into()));
            return cx.notify();
        }
        let name = match v["Name"].trim() {
            "" => calendar::provider(&url),
            n => n.to_string(),
        };
        let feed = config::CalendarFeed { name, colour: v["Colour"].to_string(), url };
        let generation = self.form_gen;
        self.adding_calendar = Some(generation);
        self.status = Some((Tone::Info, READING.into()));
        let tx = self.tx.clone();
        cx.background_executor()
            .spawn(async move {
                let r = std::panic::catch_unwind(|| calendar::fetch(&feed.url)).unwrap_or_else(|_| Err("the fetch failed".into()));
                let _ = tx.unbounded_send(Msg::CalendarAdded(generation, feed, r));
            })
            .detach();
        cx.notify();
    }

    /// Add Calendar's fetch for form `generation` came back: a calendar that reads is added
    /// ("Added Work") and, from the form still on screen, My Schedule opens on it; one that does
    /// not keeps that form, the ActionBar saying so. An older form's answer only toasts.
    fn calendar_added(&mut self, generation: u64, feed: config::CalendarFeed, result: Result<Vec<calendar::Event>, String>, window: &mut Window, cx: &mut Context<Self>) {
        let (here, pending) = add_answer(self.adding_calendar, generation, self.form_gen, self.mode == Mode::Form(FormKind::AddCalendar));
        self.adding_calendar = pending;
        if pending.is_none() && self.status.as_ref().is_some_and(|(_, s)| s == READING) {
            self.status = None;
        }
        let added = result
            .map_err(|_| "Couldn't read this calendar".to_string())
            .and_then(|events| calendar::save_feeds(&config::path(), |t| calendar::add_feed(t, &feed)).map(|(c, _)| (c, events)));
        let (c, events) = match added {
            Ok(v) => v,
            Err(e) if here => {
                self.status = Some((Tone::Error, e));
                return cx.notify();
            }
            Err(e) => {
                self.say(Msg::error(format!("{}: {e}", feed.name)));
                return cx.notify();
            }
        };
        self.cfg.calendar_feed = c.calendar_feed;
        self.say(Msg::done(format!("Added {}", feed.name)));
        self.calendar.added(feed.url, events);
        if let Some(h) = self.settings_window {
            drop(h.update(cx, |s, _, cx| s.reread(cx)));
        }
        if here {
            self.status = None;
            self.set_mode(Mode::Schedule, cx);
            window.focus(&self.input.focus_handle(cx), cx);
        }
    }

    /// Left or Right on Add Calendar's Colour: the next swatch. False elsewhere.
    fn step_swatch(&mut self, d: isize, cx: &mut Context<Self>) -> bool {
        if !matches!(self.mode, Mode::Form(_)) || self.panel.is_some() || self.menu.is_some() {
            return false;
        }
        let Some(f) = self.form.as_mut().and_then(|f| f.fields.get_mut(f.field)).filter(|f| matches!(f.ctl, Ctl::Swatches)) else { return false };
        f.value = calendar::step_colour(&f.value, d).into();
        cx.notify();
        true
    }

    /// Every 10 s: fetches the feeds when due (off the UI thread), reminds of meetings about to
    /// start, and keeps root's Next row current.
    fn calendar_tick(&mut self, cx: &mut Context<Self>) {
        let urls: Vec<String> = self.feed_urls().into_iter().filter(|u| !u.trim().is_empty()).collect();
        let every = Duration::from_secs(u64::from(self.cfg.calendar.refresh_minutes.max(5)) * 60);
        if self.calendar.wants_fetch(urls.len(), Instant::now(), every) {
            self.calendar.fetching(Instant::now());
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                // The result is always sent, so a crash cannot leave "fetching" on.
                let results = urls
                    .into_iter()
                    .map(|u| {
                        let r = std::panic::catch_unwind(|| calendar::fetch(&u)).unwrap_or_else(|_| Err("the fetch failed".into()));
                        (u, r)
                    })
                    .collect();
                let _ = tx.unbounded_send(Msg::Calendar(results));
            });
        }
        let events = self.calendar_events();
        for text in self.calendar.due(&events, search::now() as i64, self.cfg.calendar.remind_minutes) {
            self.say(Msg::sticky(toast::Kind::Info, text));
            self.pick_next = true;
        }
        // An armed row waits for its second Enter: a refresh would move it.
        if !self.visible || self.mode != Mode::Root || self.panel.is_some() || self.armed.is_some() || !self.query(cx).trim().is_empty() {
            return;
        }
        let next = self.next_rows();
        let shown: Vec<&Item> = self.items.iter().filter(|it| it.section.as_deref() == Some(NEXT)).collect();
        if shown.len() != next.len() || shown.iter().zip(&next).any(|(a, b)| a.title != b.title) {
            return self.refresh_keeping(cx);
        }
        for (it, n) in self.items.iter_mut().filter(|it| it.section.as_deref() == Some(NEXT)).zip(next) {
            it.accessory = n.accessory;
        }
        cx.notify();
    }

    // ---- Focus sessions (the decisions are in focus.rs) -----------------------------

    /// The number for a new HDR read (`Msg::Hdr`): only the last one asked for is taken.
    fn hdr_read(&mut self) -> u64 {
        self.hdr_seq += 1;
        self.hdr_seq
    }

    /// Shows a toast (`Msg::done`, `Msg::error`, …).
    fn say(&self, toast: Msg) {
        let _ = self.tx.unbounded_send(toast);
    }

    /// Gives the browser companion the site filter (or switches it off).
    fn focus_push(&mut self) {
        self.focus.pushed(Instant::now());
        self.browsers.ask_all(focus::sites_message(self.focus.session.as_ref(), focus::snooze_for(self.cfg.focus.snooze_minutes)));
    }

    /// The session as a root row with its actions, and the categories in a `focus …` search.
    fn focus_rows(&self, query: &str) -> Vec<Item> {
        let q = query.trim().to_lowercase();
        let mut rows = vec![];
        if let Some(s) = &self.focus.session
            && (q.is_empty() || s.title.to_lowercase().contains(&q))
        {
            rows.push(focus_row(s, SystemTime::now()));
        }
        if let Some(rest) = q.strip_prefix("focus ") {
            for c in focus::categories(&self.cfg.focus_category).into_iter().filter(|c| c.name.to_lowercase().contains(rest.trim())) {
                let start = |m| Act::FocusStart(c.name.clone(), m);
                let sub = focus::category_line(&c);
                let mut row = Item::new(c.name.clone(), sub, Icon::Vector(focus::category_icon(&c.name)), start(focus::Mode::Block)).acc("Focus category").sec("Focus Categories");
                row.extra = [focus::Mode::Block, focus::Mode::Allow].map(|m| (start(m).label().to_string(), start(m))).to_vec();
                rows.push(row);
            }
        }
        rows
    }

    /// Starts (or, editing, replaces) the session; returns what to tell the user.
    fn focus_begin(&mut self, start: focus::Start, edit: bool) -> String {
        let focus::Start { title, minutes, filter, picks } = start;
        let old = self.focus.session.take();
        let (mode, apps, sites, picks) = match (filter, &old) {
            (Some((m, a, s)), _) => (m, a, s, picks),
            (None, Some(o)) if edit => (o.mode, o.apps.clone(), o.sites.clone(), o.picks.clone()),
            (None, _) => (focus::Mode::Block, vec![], vec![], vec![]),
        };
        let mut session = focus::Session::new(&title, mode, &apps, &sites, minutes, self.cfg.focus.allow_all_on_empty_sites, SystemTime::now());
        session.picks = picks;
        // The apps already closed still come back at the end.
        session.closed = old.map(|o| o.closed).unwrap_or_default();
        let text = if minutes == 0 { format!("Focus started: {} (no limit)", session.title) } else { format!("Focus started: {} for {minutes} min", session.title) };
        self.focus.set(Some(session));
        self.watch_timers();
        self.focus_push();
        text
    }

    /// Adds categories to config.toml (the config reload applies them within a couple of seconds).
    fn focus_save_categories(cats: &[config::FocusCategory]) -> Result<String, String> {
        let mut n = 0;
        config::edit_file(&config::path(), |text| {
            let (text, added) = focus::add_categories(text, cats)?;
            n = added;
            if n == 0 { Err("Those categories are already in config.toml".into()) } else { Ok(text) }
        })?;
        Ok(format!("Added {n} focus categor{}", if n == 1 { "y" } else { "ies" }))
    }

    /// Enter with a Focus command's boxes: start (or edit) the session, or save the categories.
    /// Start with Goal empty, and Edit as seeded, open the form instead (`focus_form_for`). A
    /// refusal keeps the boxes and says why.
    fn focus_args(&mut self, f: FocusCmd, got: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let cats = focus::categories(&self.cfg.focus_category);
        if let Some(form) = focus_form_for(f, &got, self.focus.session.as_ref(), &cats, self.cfg.focus.default_minutes) {
            return match form {
                Ok(fields) => {
                    self.load_running();
                    self.open_form(FormKind::Focus, fields, window, cx);
                }
                Err(e) => self.focus_refused(e, cx),
            };
        }
        let done = match f {
            FocusCmd::Edit if self.focus.session.is_none() => Err("No Focus session is running".to_string()),
            FocusCmd::Start | FocusCmd::Edit => focus::parse_start(&got, &cats, self.cfg.focus.default_minutes).map(|s| self.focus_begin(s, f == FocusCmd::Edit)),
            FocusCmd::CreateCategory => focus::parse_category(&got).and_then(|c| Self::focus_save_categories(&[c])),
            FocusCmd::ImportCategories => {
                // A typed path may be a slow or missing drive: read off the UI thread.
                let path = got[0].trim().trim_matches('"').to_string();
                let read = cx.background_executor().spawn(async move { std::fs::read_to_string(path).map_err(|e| format!("Could not read the file: {e}")).and_then(|t| focus::import(&t)) });
                cx.spawn(async move |this, cx| {
                    let cats = read.await;
                    _ = this.update(cx, |this, cx| this.focus_saved(cats.and_then(|c| Self::focus_save_categories(&c)), cx));
                })
                .detach();
                return;
            }
            _ => return,
        };
        self.focus_saved(done, cx);
    }

    /// A Focus command's answer: done hides the launcher with a toast; a refusal says why.
    fn focus_saved(&mut self, done: Result<String, String>, cx: &mut Context<Self>) {
        match done {
            Ok(text) => {
                self.say(Msg::done(text));
                // A late answer (Import reads off the UI thread) never takes focus back.
                if self.visible {
                    self.hide(true, cx);
                }
            }
            Err(e) => self.focus_refused(e, cx),
        }
    }

    /// Why a Focus command did nothing: in the ActionBar, or (run from a hotkey with the
    /// window hidden) as a toast.
    fn focus_refused(&mut self, e: String, cx: &mut Context<Self>) {
        if !self.visible {
            return self.say(Msg::error(e));
        }
        self.status = Some((Tone::Error, e));
        cx.notify();
    }

    /// Ctrl+Enter in Import Screen Time: stops micro bot and brings its history in on a thread of
    /// its own (it waits on SQLite and `schtasks`); the answer is a toast.
    fn import_screen_time(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else { return };
        let v = form_values(&form.fields);
        let folder = PathBuf::from(v["micro bot folder"].trim().trim_matches('"'));
        let force = v["If imported before"] == "again";
        if folder.as_os_str().is_empty() {
            if let Some(i) = form.fields.iter().position(|f| f.label == "micro bot folder") {
                form.fields[i].error = Some("Type the folder micro bot is in".into());
                form.field = i;
            }
            return self.focus_field(window, cx);
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            // The answer is always sent, so a crash cannot leave the user waiting.
            let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wellbeing::import_from(&folder, force, &tx))).unwrap_or_else(|_| Err("the import failed".into()));
            let _ = tx.unbounded_send(match done {
                Ok(text) => Msg::done(text),
                Err(e) => Msg::error(format!("Import: {e}")),
            });
        });
        self.say(Msg::info("Importing Screen Time…"));
        self.set_mode(Mode::Root, cx);
        self.hide(true, cx);
    }

    /// Ctrl+Enter in the Focus form: starts the session (replacing one that runs) and closes.
    /// A Duration that does not parse stays, with why under it.
    fn submit_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else { return };
        let value = |label: &str| form.fields.iter().find(|f| f.label == label).map(|f| f.value.clone()).unwrap_or_default();
        let (goal, duration, mode, picks) = (value("Goal"), value("Duration"), value("Mode"), focus::Picks::of(&value("Block")).0);
        let cats = focus::categories(&self.cfg.focus_category);
        let mut start = match focus::parse_start(&[goal, duration], &cats, self.cfg.focus.default_minutes) {
            Ok(s) => s,
            Err(e) => {
                if let Some(i) = form.fields.iter().position(|f| f.label == "Duration") {
                    form.fields[i].error = Some(e);
                    form.field = i;
                }
                return self.focus_field(window, cx);
            }
        };
        let (apps, sites) = focus::resolve(&picks, &cats);
        start.filter = Some((if mode == "allow" { focus::Mode::Allow } else { focus::Mode::Block }, apps, sites));
        start.picks = picks;
        let text = self.focus_begin(start, false);
        self.say(Msg::done(text));
        self.set_mode(Mode::Root, cx);
        self.hide(true, cx);
    }

    /// Programs for the Block menu by file name, with their names: the running ones (as Quit
    /// Applications lists them) and the installed ones that are programs, A to Z.
    fn focus_apps(&self) -> Vec<(String, String)> {
        let running = self.running.iter().flatten().map(|(r, _)| (focus::norm_app(&r.exe), r.name.clone()));
        let installed = self.apps.iter().filter(|a| a.id.to_lowercase().ends_with(".exe")).map(|a| (focus::norm_app(&a.id), a.name.clone()));
        let mut out: Vec<(String, String)> = vec![];
        for (exe, name) in running.chain(installed) {
            if !exe.is_empty() && !out.iter().any(|(e, _)| *e == exe) {
                let name = if name.trim().is_empty() { focus::app_label(&exe) } else { name };
                out.push((exe, name));
            }
        }
        out.sort_by_key(|(_, n)| n.to_lowercase());
        out
    }

    /// The Block menu's entries for form field `i`, and whether a typed website is blocked or allowed.
    fn block_menu(&self, i: usize) -> (Vec<menu::Entry>, &'static str) {
        let Some(form) = &self.form else { return (vec![], "Block") };
        let picks = form.fields.get(i).map_or_else(focus::Picks::default, |f| focus::Picks::of(&f.value));
        let verb = if form_values(&form.fields).get("Mode") == Some(&"allow") { "Allow" } else { "Block" };
        (block_entries(&focus::categories(&self.cfg.focus_category), &self.focus_apps(), &picks), verb)
    }

    /// Opens the Block menu under token field `i`. A pick becomes a token and the menu opens
    /// again, without it, for the next.
    fn open_block_menu(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (entries, verb) = self.block_menu(i);
        let m = menu::Menu::new(entries, Some("Search…".into()), menu::Place::At(self.form_at.get()));
        self.open_menu(
            m,
            move |this, id, window, cx| {
                this.edit_picks(i, |p| p.add(&id));
                this.open_block_menu(i, window, cx);
            },
            window,
            cx,
        );
        if let Some(open) = &mut self.menu {
            open.site = Some(verb);
        }
    }

    /// Changes token field `i`'s picks.
    fn edit_picks(&mut self, i: usize, f: impl FnOnce(&mut focus::Picks)) {
        if let Some(field) = self.form.as_mut().and_then(|form| form.fields.get_mut(i)) {
            let mut p = focus::Picks::of(&field.value);
            f(&mut p);
            field.value = p.value();
        }
    }

    /// Backspace on the selected token field, or in its menu's empty search: the last token
    /// goes back into the menu. False when the key is the text's (anywhere else).
    fn pop_token(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(i) = self.form.as_ref().filter(|f| matches!(self.mode, Mode::Form(_)) && matches!(f.fields.get(f.field).map(|c| &c.ctl), Some(Ctl::Tokens))).map(|f| f.field) else { return false };
        if self.menu.as_ref().is_some_and(|m| m.site.is_none() || !self.menu_input.read(cx).as_str().is_empty()) {
            return false;
        }
        self.edit_picks(i, |p| _ = p.pop());
        let (entries, _) = self.block_menu(i);
        if let Some(open) = &mut self.menu {
            open.menu.set_entries(entries);
        }
        cx.notify();
        true
    }

    fn focus_cmd(&mut self, f: FocusCmd, window: &mut Window, cx: &mut Context<Self>) {
        let now = SystemTime::now();
        let needs_session = matches!(f, FocusCmd::Pause | FocusCmd::Resume | FocusCmd::Complete | FocusCmd::Cancel | FocusCmd::Restart | FocusCmd::Edit | FocusCmd::Snooze);
        if needs_session && self.focus.session.is_none() {
            return self.say(Msg::error("No Focus session running: start one first"));
        }
        match f {
            // They take arguments: `run` asks for them (Mode::Args) before they get here.
            FocusCmd::Start | FocusCmd::Edit | FocusCmd::CreateCategory | FocusCmd::ImportCategories => return,
            FocusCmd::Toggle if self.focus.session.is_none() => {
                let Some(last) = self.focus.preset.clone() else {
                    self.ask_args_for(&Act::Cmd(Cmd::Focus(FocusCmd::Start)), window, cx);
                    return;
                };
                let text = self.focus_begin(last.start(), false);
                self.say(Msg::done(text));
                if self.visible {
                    self.hide(true, cx);
                }
                return;
            }
            FocusCmd::Toggle | FocusCmd::Complete => return self.focus_end(true, cx),
            FocusCmd::Cancel => return self.focus_end(false, cx),
            FocusCmd::SearchCategories => return self.input.update(cx, |s, cx| s.emplace("focus ", cx)),
            FocusCmd::Pause => self.focus.update(|s| s.pause(now)),
            FocusCmd::Resume => {
                self.focus.update(|s| s.resume(now));
                self.watch_timers();
            }
            FocusCmd::Restart => {
                self.focus.update(|s| s.restart(now));
                self.watch_timers();
            }
            FocusCmd::Snooze => {
                let mins = focus::snooze_for(self.cfg.focus.snooze_minutes);
                let Some((exe, path)) = self.focus.last_closed() else { return self.say(Msg::info("Focus has not closed an app yet")) };
                self.focus.update(|s| {
                    s.snooze(&exe, mins);
                    s.closed.retain(|p| *p != path);
                });
                if !path.is_empty() {
                    win::launch_app(&path);
                }
                return self.say(Msg::done(format!("Focus: {} allowed for {mins} min", focus::app_label(&exe))));
            }
        }
        self.focus_push();
        let said = match f {
            FocusCmd::Pause => "Focus paused",
            FocusCmd::Resume => "Focus resumed",
            _ => "Focus restarted",
        };
        self.say(Msg::done(said));
        if self.visible && self.mode == Mode::Root {
            self.refresh(cx);
        }
    }

    /// Five more minutes (the notice goes: the time is no longer up).
    fn focus_extend(&mut self) {
        self.focus.update(|s| s.extend(SystemTime::now()));
        self.focus_push();
        self.say(Msg::done("Focus extended by 5 min"));
    }

    /// Ends the session: done (completed) or cancelled.
    fn focus_end(&mut self, done: bool, cx: &mut Context<Self>) {
        let Some(s) = self.focus.session.clone() else { return };
        self.focus.set(None);
        for path in &s.closed {
            win::launch_app(path);
        }
        self.say(Msg::done(format!("Focus {}: {}", if done { "done" } else { "cancelled" }, s.title)));
        self.focus_push();
        if self.visible && self.mode == Mode::Root {
            self.refresh(cx);
        }
    }

    /// Once a second while a session counts down: sounds when the time is up (the notice shows
    /// then, `hud`, and the session keeps blocking until Complete or Extend), repeats the site
    /// filter now and then (the browser forgets it when its extension restarts), keeps the
    /// row's time current.
    fn on_focus_tick(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now();
        // A paused session has nothing to count, repeat or redraw (a timer may keep the tick alive).
        if !self.focus.session.as_ref().is_some_and(focus::Session::running) {
            return;
        }
        if self.focus.time_up(now) && self.cfg.focus.sound {
            focus::beep();
        }
        let left = self.focus.session.as_ref().map(|s| s.row(now).1).unwrap_or_default();
        if self.focus.push_due(Instant::now()) {
            self.focus_push();
        }
        if self.visible && self.mode == Mode::Root {
            for it in &mut self.items {
                if it.section.as_deref() == Some(FOCUS) {
                    it.accessory = left.clone();
                }
            }
            cx.notify();
        }
    }

    /// A window took focus: the window of a blocked app is asked to close.
    fn focus_front(&mut self, hwnd: isize, exe: &str) {
        let now = SystemTime::now();
        let Some(s) = &self.focus.session else { return };
        if !s.blocked_app(exe, now) || !self.focus.fresh(exe, Instant::now()) {
            return;
        }
        let text = format!("{} is blocked · {}", focus::app_label(exe), s.title);
        let path = focus::close_window(hwnd);
        self.focus.closed_app(exe, path, self.cfg.focus.restore_apps, Instant::now());
        self.say(Msg::done(text));
    }

    /// What the Focus HUD shows now: the pill while a session exists (unless `[focus] pill` is
    /// off), the notice while its time is up.
    fn hud(&self) -> hud::State {
        let now = SystemTime::now();
        let s = self.focus.session.as_ref();
        let time = |s: &focus::Session| focus::pill_text(if s.unlimited() { s.elapsed(now) } else { s.left(now) });
        hud::State { pill: s.filter(|_| self.cfg.focus.pill).map(|s| (time(s), s.title.clone())), notice: s.is_some_and(|s| s.expired(now)), theme: self.cfg.theme.clone() }
    }

    /// Scrolls the selected row into view (section headers take list slots too).
    fn scroll_to_selected(&self) {
        if self.grid() {
            if let Some(at) = grid_rows(&self.items, self.grid_cols()).iter().position(|row| matches!(row, GridRow::Cells(c) if c.contains(&self.selected))) {
                self.grid_list.scroll_to_reveal_item(at);
            }
            return;
        }
        let at = display_rows(&self.items, self.mode == Mode::Schedule).iter().position(|r| *r == Some(self.selected)).unwrap_or(0);
        self.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
    }

    fn rebuild_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog = search::catalog(&self.apps, &self.scripts, &self.exts, &self.cfg);
        self.add_favicons();
        if self.mode == Mode::Root {
            self.refresh(cx);
        }
    }

    /// Web quicklinks show their site's icon once it's downloaded; the link glyph until then.
    fn add_favicons(&mut self) {
        for i in 0..self.catalog.len() {
            let Some(name) = self.catalog[i].key.as_deref().and_then(|k| k.strip_prefix("ql:")) else { continue };
            let Some(url) = self.cfg.quicklink.iter().find(|q| q.title() == name).and_then(|q| net::favicon_url(&q.url)) else { continue };
            let path = net::image_path(&url);
            if path.exists() {
                // ponytail: a site that answers /favicon.ico with a page leaves an undecodable file and the glyph for good.
                if let Some(img) = self.image(path) {
                    self.catalog[i].icon = Icon::Image(img);
                }
            } else if self.favicons.insert(url.clone()) {
                net::fetch_image(&url, &self.tx);
            }
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
        let (folders, deep) = (self.cfg.apps.folders.clone(), self.cfg.apps.deep);
        let mut known = if fresh_icons { vec![] } else { self.apps.clone() };
        std::thread::spawn(move || {
            if known.is_empty() && !fresh_icons {
                known = win::load_apps(&cache);
                if !known.is_empty() {
                    let _ = tx.unbounded_send(Msg::Apps(known.clone()));
                }
            }
            let mut apps = win::scan_apps();
            apps.extend(apps::folder_apps(&folders, deep));
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
        if self.mode == Mode::Shots && mode != Mode::Shots {
            self.drop_thumbs(cx);
        }
        // A typed snippet keyword's erase belongs to its own Args session only.
        self.typed = Default::default();
        if mode != Mode::Ext {
            self.stop_ext_view();
        }
        if !matches!(mode, Mode::Form(_)) {
            self.form = None;
        }
        // Add Calendar's wait is the form's: it does not follow the user elsewhere.
        if self.status.as_ref().is_some_and(|(_, s)| s == READING) {
            self.status = None;
        }
        if mode != Mode::Ai && self.ai.take().is_some() {
            self.send(json!({"t": "aiStop"}));
        }
        self.mode = mode;
        self.panel = None;
        self.menu = None;
        self.arg_memory.clear();
        self.args_for = None;
        if mode == Mode::Files
            && let Some(f) = &self.files
        {
            f.refresh_if_stale(Duration::from_secs(3600));
        }
        if mode == Mode::Shots {
            self.scan_shots();
        }
        if mode == Mode::Store {
            self.load_store();
        }
        if mode == Mode::Running {
            self.load_running();
        }
        if matches!(mode, Mode::Notes(_)) {
            self.note_rows = None;
        }
        if let Mode::ScreenTime(p) = mode {
            (self.st_stats, self.st_days) = (None, None);
            self.load_screen_time(p, focus::categories(&self.cfg.focus_category), cx);
        }
        if mode == Mode::Tabs {
            self.tabs.clear();
            self.browsers.ask_all(json!({"t": "tabs"}));
        }
        self.input.update(cx, |s, cx| s.emplace("", cx));
        self.selected = 0;
        self.refresh(cx);
    }

    fn show(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        window.set_background_appearance(backdrop(design::refresh_glass()).into());
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
        // A sticky toast closes; its text shows once in the ActionBar instead.
        self.status = self.toast.and_then(|t| t.update(cx, |t, _, cx| t.dismiss(cx)).ok().flatten()).map(|(kind, text)| (Tone::of(kind), text));
        // Within the pop-to-root timeout the launcher reopens where it was.
        if mode == Mode::Root && self.keeps_state() {
            // Reopened where it was: the numbers moved on meanwhile.
            if let Mode::ScreenTime(p) = self.mode {
                self.load_screen_time(p, focus::categories(&self.cfg.focus_category), cx);
            }
            self.refresh(cx);
        } else {
            self.peek = false;
            self.search_pos = None;
            self.set_mode(mode, cx);
        }
        if let Ok(view) = std::env::var("PROCAST_SHOW") {
            let mode = match view.as_str() {
                "clipboard" => Mode::Clipboard,
                "emoji" => Mode::Emoji,
                "files" => Mode::Files,
                "shots" => Mode::Shots,
                "running" => Mode::Running,
                "tabs" => Mode::Tabs,
                _ => Mode::Root,
            };
            self.set_mode(mode, cx);
            let q = std::env::var("PROCAST_QUERY").unwrap_or_default();
            self.input.update(cx, |s, cx| s.emplace(&q, cx));
        }
        win::cloak(self.hwnd, false);
        win::focus(self.hwnd);
        window.activate();
        window.focus(&self.input.focus_handle(cx), cx);
        self.visible = true;
        self.scan_apps(false, false);
        if self.cfg.dictionary.enabled {
            let tx = self.tx.clone();
            std::thread::spawn(move || drop(tx.unbounded_send(Msg::DictReady(dict::installed().is_some()))));
        }
        let (seq, tx) = (self.hdr_read(), self.tx.clone());
        std::thread::spawn(move || drop(tx.unbounded_send(Msg::Hdr(seq, hdr::displays()))));
        // The window was placed before this view existed (a notice, a kept search or a
        // query can each change its height), so size it to what it shows now. Without this,
        // `height` could claim a size the window doesn't have, and typing in compact mode
        // would never grow it. win::resize does nothing when the size is already right.
        self.height = 0.;
        self.request_resize(cx);
    }

    /// `restore` hands focus back to the window that was active before we opened;
    /// false only when focus already moved elsewhere (deactivation).
    fn hide(&mut self, restore: bool, cx: &mut Context<Self>) {
        self.typed = Default::default();
        win::cloak(self.hwnd, true);
        self.visible = false;
        self.pick_next = false;
        self.hidden_at = Some(Instant::now());
        self.drop_thumbs(cx);
        self.panel = None;
        self.menu = None;
        if self.mode == Mode::Music {
            self.music.retire_covers();
        }
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

    // ---- clipboard history and snippets --------------------------------------------

    /// Puts a history entry (text, image or files) back on the clipboard.
    fn put_clip(&mut self, key: &str) -> bool {
        let Some(e) = self.history.get(key) else { return false };
        match &e.kind {
            clipboard::Kind::Text => clipboard::set(self.hwnd, clipboard::Put::Text(&e.text), false),
            clipboard::Kind::Image { id, .. } => self.history.png(id).is_some_and(|png| clipboard::set(self.hwnd, clipboard::Put::Image(&png), false)),
            clipboard::Kind::Files(f) => clipboard::set(self.hwnd, clipboard::Put::Files(f), false),
        }
    }

    /// A clipboard image fitted for the detail pane, cached.
    fn clip_preview(&mut self, id: &str) -> Option<Arc<RenderImage>> {
        let history = &self.history;
        used(&mut self.clip_images, &mut self.file_tick, format!("p:{id}"), || history.png(id).and_then(|p| clipboard::render_image(&p, 640)))
    }

    /// `file_images[key]`, marked as just used; None when it was never asked for.
    fn cached_image(&mut self, key: &str) -> Option<Option<Arc<RenderImage>>> {
        self.file_tick += 1;
        let entry = self.file_images.get_mut(key)?;
        entry.1 = self.file_tick;
        Some(entry.0.clone())
    }

    /// A file's picture (`p:`, fitted) or shell icon (`i:`), made on a worker thread: None
    /// until it arrives as Msg::FilePreview.
    fn file_preview(&mut self, key: String) -> Option<Arc<RenderImage>> {
        if let Some(img) = self.cached_image(&key) {
            return img;
        }
        self.file_images.insert(key.clone(), (None, self.file_tick));
        let (tx, seq) = (self.tx.clone(), self.file_seq.clone());
        let at = seq.load(Ordering::Relaxed);
        std::thread::spawn(move || {
            // Settle first: arrowing through a folder only decodes where the selection stops.
            std::thread::sleep(Duration::from_millis(100));
            if seq.load(Ordering::Relaxed) != at {
                let _ = tx.unbounded_send(Msg::FilePreviewDropped(key));
                return;
            }
            let path = &key[2..];
            let img = if key.starts_with("p:") { std::fs::read(path).ok().and_then(|b| clipboard::render_image(&b, 640)) } else { win::file_icon(path) };
            let _ = tx.unbounded_send(Msg::FilePreview(key, img));
        });
        None
    }

    /// A screenshot grid cell's thumbnail (`shots::thumb_key`), `THUMB_PX` wide, for grid
    /// row `row`: decoded on the background executor with no settle (a grid wants them all
    /// at once), unless the grid has scrolled far from the row by its turn. Only a picture
    /// no bigger than `shots::MAX_BYTES` is read. None until it arrives as Msg::FilePreview.
    fn shot_thumb(&mut self, key: &str, row: usize, cx: &App) -> Option<Arc<RenderImage>> {
        if let Some(img) = self.cached_image(key) {
            return img;
        }
        self.file_images.insert(key.to_string(), (None, self.file_tick));
        let (tx, top, key) = (self.tx.clone(), self.grid_top.clone(), key.to_string());
        cx.background_executor()
            .spawn(async move {
                if !near(row, top.load(Ordering::Relaxed)) {
                    // Asked again if it comes back into view.
                    let _ = tx.unbounded_send(Msg::FilePreviewDropped(key));
                    return;
                }
                let img = shot_file(&key).and_then(|p| std::fs::read(p).ok()).and_then(|b| clipboard::render_width(&b, THUMB_PX));
                let _ = tx.unbounded_send(Msg::FilePreview(key, img));
            })
            .detach();
        None
    }

    /// Quick Look's picture (`q:` with the thumbnail key's time and path), fitted inside
    /// `QUICK_LOOK_PX`: decoded on a worker once the selection rests, and the one shown before
    /// it freed. None until it arrives as Msg::FilePreview.
    fn quick_look_image(&mut self, key: &str, cx: &mut App) -> Option<Arc<RenderImage>> {
        if let Some(img) = self.cached_image(key) {
            return img;
        }
        self.drop_images(&["q:"], cx);
        self.file_images.insert(key.to_string(), (None, self.file_tick));
        let at = self.file_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, seq, key) = (self.tx.clone(), self.file_seq.clone(), key.to_string());
        std::thread::spawn(move || {
            // Settle first: arrowing through the shots only decodes where the selection stops.
            std::thread::sleep(Duration::from_millis(100));
            // Asked again since: that request freed this key's entry (`drop_images`), and a
            // drop sent now could remove the entry of a newer request for the same key.
            if seq.load(Ordering::Relaxed) != at {
                return;
            }
            let img = shot_file(&key).and_then(|p| std::fs::read(p).ok()).and_then(|b| clipboard::render_image(&b, QUICK_LOOK_PX));
            let _ = tx.unbounded_send(Msg::FilePreview(key, img));
        });
        None
    }

    /// Forgets the screenshot thumbnails, frees their GPU textures and closes Quick Look (the
    /// grid closed).
    fn drop_thumbs(&mut self, cx: &mut App) {
        self.close_quick_look(cx);
        self.drop_images(&["t:"], cx);
    }

    /// Closes Quick Look and frees its picture.
    fn close_quick_look(&mut self, cx: &mut App) {
        self.quick_look = false;
        self.quick_look_detail = Default::default();
        self.drop_images(&["q:"], cx);
    }

    /// Forgets the decoded images whose keys start with one of `prefixes` and frees their GPU textures.
    fn drop_images(&mut self, prefixes: &[&str], cx: &mut App) {
        let gone = self.file_images.extract_if(|k, _| prefixes.iter().any(|p| k.starts_with(p))).filter_map(|(_, v)| v.0).collect();
        release(gone, cx);
    }

    /// A snippet chosen in the launcher (`erase` 0) or typed in an app: asks for its
    /// arguments first if it has any, then goes into the app.
    fn start_snippet(&mut self, name: String, template: String, erase: usize, then: String, window: &mut Window, cx: &mut Context<Self>) {
        let key = format!("sn:{name}");
        if let Some(it) = self.catalog.iter().find(|it| it.key.as_ref() == Some(&key) && !it.args.is_empty()).cloned() {
            self.ask_args(it, window, cx);
            // After ask_args: opening the session clears what an abandoned one left.
            self.typed = (erase, then);
            return;
        }
        self.insert_snippet(template, vec![], erase, then, cx);
    }

    fn insert_snippet(&mut self, template: String, args: Vec<String>, erase: usize, then: String, cx: &mut Context<Self>) {
        let opened = self.visible;
        if opened {
            self.hide(true, cx);
        }
        let s = &self.cfg.snippets;
        // From the launcher: the app needs a moment to get focus back first.
        let pause = if opened { snippets::pause(&s.speed).max(Duration::from_millis(80)) } else { snippets::pause(&s.speed) };
        let sound = if erase > 0 { snippets::sound(&s.sound) } else { None };
        snippets::insert(snippets::Job { owner: self.hwnd, template, args, erase, then, pause, sound });
    }

    /// Whether a typed keyword should expand where it was typed.
    fn expands_here(&self) -> bool {
        !snippets::procast_focused() && !self.cfg.snippets.ignore_apps.iter().any(|a| a.eq_ignore_ascii_case(&clipboard::exe_of(win::foreground())))
    }

    /// The snippet a typed keyword asks for, when it must open the launcher for its arguments.
    fn expand_opens(&self, hit: &snippets::Hit) -> bool {
        matches!(hit, snippets::Hit::Snippet { index, .. } if self.cfg.snippet.get(*index).is_some_and(|s| !snippets::arguments(&s.text).is_empty())) && self.expands_here()
    }

    fn on_expand(&mut self, hit: snippets::Hit, window: &mut Window, cx: &mut Context<Self>) {
        if !self.expands_here() {
            return;
        }
        match hit {
            snippets::Hit::Emoji { emoji, erase } => {
                let e = search::toned(emoji, &self.cfg.emoji.skin_tone).to_string();
                self.insert_snippet(e, vec![], erase, String::new(), cx);
            }
            snippets::Hit::Snippet { index, erase, then } => {
                let Some(s) = self.cfg.snippet.get(index).cloned() else { return };
                let fg = win::foreground();
                if fg != self.hwnd {
                    self.prev = fg;
                }
                self.start_snippet(s.name, s.text, erase, then, window, cx);
            }
        }
    }

    // ---- translator ------------------------------------------------------------------

    /// Translates `job`. Typed text opens the translator at once, its text in the bar, the last
    /// answers kept (dimmed) until the new ones land; the selection (or a paste-over-selection
    /// job) hides the launcher first, so the app you came from has focus.
    fn translate(&mut self, mut job: translate::Job, cx: &mut Context<Self>) {
        self.translate_seq += 1;
        let (seq, hidden) = (self.translate_seq, job.after != translate::After::Open || job.text.trim().is_empty());
        if hidden {
            if self.visible {
                self.hide(true, cx);
            }
        } else {
            self.status = None;
            if self.mode != Mode::Translate || self.translation.is_none() {
                self.translation = Some(TrView { job: job.clone(), rows: vec![], first: 0, busy: true });
                self.set_mode(Mode::Translate, cx);
            }
            if let Some(tr) = &mut self.translation {
                tr.job = job.clone();
                tr.busy = true;
            }
            self.fill_query(&job.text, cx);
        }
        let (tx, owner, proxy) = (self.tx.clone(), self.hwnd.0 as isize, self.cfg.translator.proxy.clone());
        std::thread::spawn(move || {
            if job.text.trim().is_empty() {
                match snippets::grab_selection(HWND(owner as _)).filter(|s| !s.trim().is_empty()) {
                    Some(sel) => job.text = sel,
                    // Nothing selected: the translator opens empty; Instant Translate says why.
                    None if job.after == translate::After::Open => return drop(tx.unbounded_send(Msg::Translated { seq, job, hidden, result: Ok(vec![]) })),
                    None => return drop(tx.unbounded_send(Msg::error("Select some text first"))),
                }
            }
            let result = translate::run(&job, &proxy);
            let _ = tx.unbounded_send(Msg::Translated { seq, job, hidden, result });
        });
    }

    /// The bar's text, unless it already holds it (setting it fires `query_changed`).
    fn fill_query(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.query(cx) != text {
            self.input.update(cx, |s, cx| s.emplace(text, cx));
        }
        cx.notify();
    }

    /// Typing in the translator: asks again once typing pauses for `TYPING_QUIET`; the answers
    /// shown stay, dimmed, until the new ones land. An empty bar shows the empty view.
    fn translate_soon(&mut self, cx: &mut Context<Self>) {
        let text = self.query(cx);
        let Some(tr) = &mut self.translation else { return };
        if text == tr.job.text {
            return;
        }
        // Answers still on their way are for older text.
        self.translate_seq += 1;
        let seq = self.translate_seq;
        tr.job.text = text.clone();
        tr.busy = !text.trim().is_empty();
        if !tr.busy {
            tr.rows.clear();
            self.status = None;
            return;
        }
        let job = tr.job.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPING_QUIET).await;
            _ = this.update(cx, |this, cx| {
                if this.translate_seq == seq && this.mode == Mode::Translate {
                    this.translate(job, cx);
                }
            });
        })
        .detach();
    }

    fn on_translated(&mut self, seq: u64, job: translate::Job, hidden: bool, result: Result<Vec<translate::Translation>, String>, window: &mut Window, cx: &mut Context<Self>) {
        if seq != self.translate_seq {
            return;
        }
        let rows = match result {
            Ok(rows) => rows,
            // The last answers stay; the action bar says what went wrong.
            Err(e) if self.visible && self.mode == Mode::Translate => {
                if let Some(tr) = &mut self.translation {
                    tr.busy = false;
                }
                self.status = Some((Tone::Error, e));
                return cx.notify();
            }
            Err(e) => return drop(self.tx.unbounded_send(Msg::error(e))),
        };
        if job.after != translate::After::Open {
            let Some(t) = rows.first() else { return };
            cx.write_to_clipboard(ClipboardItem::new_string(t.text.clone()));
            let verb = if job.after == translate::After::Copy {
                "Copied"
            } else {
                // Over the selection the launcher gave focus back to.
                clipboard::paste_soon();
                "Pasted"
            };
            return drop(self.tx.unbounded_send(Msg::done(translate::instant_toast(verb, &t.detected, &t.to))));
        }
        let detected = rows.first().map(|t| t.detected.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| job.from.clone());
        let (rows, first) = translate::order(rows, &detected, self.cfg.translator.cross_first);
        let text = job.text.clone();
        self.translation = Some(TrView { job, rows, first, busy: false });
        self.status = None;
        if hidden && !self.visible {
            self.show(Mode::Translate, window, cx);
        }
        if self.mode == Mode::Translate {
            self.fill_query(&text, cx);
            self.refresh(cx);
        }
    }

    /// Writes a config.toml edit and takes its translator settings and language sets now (the
    /// reload follows); false, with the error in the ActionBar, when it fails.
    fn save_translator(&mut self, edit: impl FnOnce(&str) -> Result<String, String>) -> bool {
        match config::edit_file(&config::path(), edit) {
            Ok((c, _)) => {
                self.cfg.translator = c.translator;
                self.cfg.language_set = c.language_set;
                true
            }
            Err(e) => {
                self.status = Some((Tone::Error, e));
                false
            }
        }
    }

    /// The bar's Dropdown: the language set menu over the translator.
    fn open_lang_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = lang_set_menu(&translate::sets(&self.cfg), translate::current(&self.cfg));
        let pick = |this: &mut Launcher, id: String, window: &mut Window, cx: &mut Context<Launcher>| match id.parse() {
            Ok(i) => this.use_lang_set(i, window, cx),
            Err(_) => this.set_mode(Mode::LanguageSets, cx),
        };
        self.open_menu(m, pick, window, cx);
    }

    /// Makes set `i` current (the one before becomes the previous set) and re-translates.
    fn use_lang_set(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let was = translate::current(&self.cfg);
        if i != was {
            if !self.save_translator(|t| translate::set_current(t, i)) {
                return cx.notify();
            }
            self.prev_set = Some(was);
        }
        self.retranslate(window, cx);
    }

    /// Ctrl+Enter in the language set form: adds the set and makes it current, or saves an
    /// edit and goes back to the list.
    fn save_lang_set(&mut self, edit: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.form else { return };
        let v = form_values(&form.fields);
        let set = translate::LangSet::new(v["Source language"], &[v["Target language 1"].to_string(), v["Target language 2"].to_string()]);
        let was = translate::current(&self.cfg);
        match edit {
            None if self.save_translator(|t| translate::add_set(t, &set)) => {
                self.prev_set = Some(was);
                self.retranslate(window, cx);
            }
            Some(i) if self.save_translator(|t| translate::replace_set(t, i, &set)) => {
                self.status = Some((Tone::Ok, "Language set saved".into()));
                self.set_mode(Mode::LanguageSets, cx);
                window.focus(&self.input.focus_handle(cx), cx);
            }
            _ => cx.notify(),
        }
    }

    /// Back to the translator in the current language set; typed text translates again at
    /// once, the last answers dimmed meanwhile.
    fn retranslate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Translate {
            let keep = self.translation.take();
            self.set_mode(Mode::Translate, cx);
            self.translation = keep;
        }
        window.focus(&self.input.focus_handle(cx), cx);
        let set = translate::current_set(&self.cfg);
        let job = match &self.translation {
            Some(tr) => translate::Job { from: set.from, to: set.to, ..tr.job.clone() },
            None => translate::Job { text: String::new(), from: set.from, to: set.to, all: true, after: translate::After::Open },
        };
        if job.text.trim().is_empty() {
            self.translation = Some(TrView { job, rows: vec![], first: 0, busy: false });
            return self.refresh(cx);
        }
        self.translate(job, cx);
    }

    // ---- dictionary ------------------------------------------------------------------

    /// Looks `word` up off the UI thread, opening the define view with the word in the bar;
    /// `fetch` downloads the dictionary first when it is not there. Within the view, the
    /// answer shown stays until the next one lands.
    fn define(&mut self, word: String, fetch: bool, cx: &mut Context<Self>) {
        self.define_seq += 1;
        let (seq, tx) = (self.define_seq, self.tx.clone());
        self.status = None;
        let keep = self.mode == Mode::Define && !fetch;
        let answer = self.define.take().and_then(|v| v.answer).filter(|_| keep);
        if self.mode != Mode::Define {
            self.set_mode(Mode::Define, cx);
        }
        // Not downloaded (as checked on show): say so without asking the worker.
        let missing = !fetch && self.dict_ready == Some(false);
        self.define = Some(DefView { word: word.clone(), fetch, answer: if missing { Some(Ok(None)) } else { answer } });
        // Typed with a space after it ("ice "): the bar stays as typed.
        if self.query(cx).trim() != word {
            self.fill_query(&word, cx);
        }
        self.refresh(cx);
        if missing {
            return;
        }
        std::thread::spawn(move || {
            let result = dict::define(&word, fetch);
            let _ = tx.unbounded_send(Msg::Defined { seq, word, result });
        });
    }

    /// Typing in the define view: looks the word up again once typing pauses for `TYPING_QUIET`.
    fn define_soon(&mut self, cx: &mut Context<Self>) {
        // Any change outdates a pause still being waited out ("dogs" back to "dog").
        self.define_typed += 1;
        let seq = self.define_typed;
        let query = self.query(cx);
        let Some(word) = self.define.as_ref().and_then(|v| typed_word(&v.word, &query)).map(str::to_string) else { return };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPING_QUIET).await;
            _ = this.update(cx, |this, cx| {
                if this.define_typed == seq && this.mode == Mode::Define && this.query(cx).trim() == word {
                    this.define(word, false, cx);
                }
            });
        })
        .detach();
    }

    fn on_defined(&mut self, seq: u64, word: String, result: Result<Option<dict::Answer>, String>, cx: &mut Context<Self>) {
        if seq != self.define_seq {
            return;
        }
        if let Ok(a) = &result {
            self.dict_ready = Some(a.is_some());
        }
        self.define = Some(DefView { word, fetch: false, answer: Some(result) });
        if self.mode == Mode::Define {
            self.refresh(cx);
        }
    }

    /// Define Similar Word: a menu of the synonyms; the pick is defined.
    fn open_similar(&mut self, words: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let entries = words.into_iter().map(|w| menu::Entry { id: w.clone(), label: w, icon: Some(g::BOOK_OPEN), ..Default::default() }).collect();
        let m = menu::Menu::new(entries, Some("Search…".into()), menu::Place::Right);
        self.open_menu(m, |this, word, _, cx| this.define(word, false, cx), window, cx);
    }

    /// Root search's Define row with a word in its box: asks a worker for that word's first
    /// sense (its subtitle), unless it is known already or the dictionary is not downloaded.
    fn ask_sense(&mut self, cx: &App) {
        if self.mode != Mode::Root || self.dict_ready != Some(true) {
            return;
        }
        let shown = self.arg_values(cx);
        let boxed = self.arg_row().is_some();
        let word = self.items.iter().enumerate().find_map(|(i, it)| define_word(it, if i == self.selected && boxed { &shown } else { &it.values })).map(str::to_lowercase);
        let Some(word) = word.filter(|w| !self.senses.contains_key(w)) else { return };
        self.sense_seq += 1;
        let (seq, tx) = (self.sense_seq, self.tx.clone());
        std::thread::spawn(move || {
            let sense = dict::first_sense(&word);
            let _ = tx.unbounded_send(Msg::Sense { seq, word, sense });
        });
    }

    fn on_sense(&mut self, seq: u64, word: String, sense: Option<String>, cx: &mut Context<Self>) {
        if seq != self.sense_seq {
            return;
        }
        // ponytail: forgets everything past 256 words; an LRU if that ever shows.
        if self.senses.len() > 256 {
            self.senses.clear();
        }
        self.senses.insert(word, sense);
        cx.notify();
    }

    // ---- screenshots -----------------------------------------------------------------

    /// Brings the screenshot index up to date on a worker thread; the view fills in as
    /// each batch of pictures has been read.
    fn scan_shots(&mut self) {
        // One scan at a time: a second would read the same pictures again and race on the saved file.
        // ponytail: reopening during a scan shows its results; pictures added meanwhile wait for the next open.
        if self.shots_busy {
            return;
        }
        self.shots_seq += 1;
        self.shots_busy = true;
        self.shot_pins = shots::pins();
        let (seq, tx, s, pins) = (self.shots_seq, self.tx.clone(), self.cfg.screenshots.clone(), self.shot_pins.clone());
        std::thread::spawn(move || {
            let run = || shots::rescan(&s, pins, |index, read| drop(tx.unbounded_send(Msg::Shots(seq, Some(index.clone()), read))));
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
            let _ = tx.unbounded_send(Msg::Shots(seq, None, None));
        });
    }

    fn on_shots(&mut self, seq: u64, index: Option<shots::ShotIndex>, read: Option<(usize, usize)>, cx: &mut Context<Self>) {
        if seq != self.shots_seq {
            return;
        }
        self.shots_progress = read;
        match index {
            // A pin made during the scan is the launcher's, not yet the scan's.
            Some(mut index) => {
                index.set_pins(self.shot_pins.clone());
                self.shots = index
            }
            None => self.shots_busy = false,
        }
        if self.mode == Mode::Shots {
            // New results arrive while you browse: keep the selection where it is.
            self.refresh_in_place(cx);
        }
    }

    /// A screenshot delete finished (`shots::delete`): it leaves the grid and its pin, and the
    /// selection goes back to where the shot was.
    fn on_shot_deleted(&mut self, path: PathBuf, at: usize, done: Result<(), String>, cx: &mut Context<Self>) {
        let said = match done {
            Ok(()) => {
                self.shots.remove(&path);
                if self.shot_pins.contains(&path)
                    && let Ok(pins) = shots::pin(&path, false)
                {
                    self.shot_pins = pins;
                }
                (Tone::Ok, "Screenshot deleted".to_string())
            }
            Err(e) => (Tone::Error, e),
        };
        if !self.visible {
            // Windows' question took focus and closed the launcher: say it in a toast.
            return self.say(if said.0 == Tone::Ok { Msg::done(said.1) } else { Msg::error(said.1) });
        }
        self.status = Some(said);
        if self.mode == Mode::Shots {
            self.selected = at;
            self.refresh_in_place(cx);
        }
        cx.notify();
    }

    /// Redraws the list with the selection kept at the same place.
    fn refresh_in_place(&mut self, cx: &mut Context<Self>) {
        let at = self.selected;
        self.refresh(cx);
        self.selected = at.min(self.items.len().saturating_sub(1));
        self.scroll_to_selected();
    }

    /// Puts a screenshot (the newest when `path` is None) on the clipboard as an image, and
    /// pastes it into the app you came from if `paste`. The work runs off the UI thread.
    fn shot_to_clipboard(&mut self, path: Option<PathBuf>, paste: bool, cx: &mut Context<Self>) {
        self.hide(true, cx);
        let (owner, tx, folders, latest) = (self.hwnd.0 as isize, self.tx.clone(), shots::folders(&self.cfg.screenshots.folders), path.is_none());
        std::thread::spawn(move || {
            let put = || shots::put_image(HWND(owner as _), path, &folders);
            let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(put)).unwrap_or_else(|_| Err("Could not read that screenshot".into()));
            match done {
                Ok(shot) if paste => {
                    clipboard::paste_soon();
                    // Paste Latest Screenshot has no view: its toast says which one went in.
                    if latest {
                        let at = std::fs::metadata(&shot).and_then(|m| m.modified()).ok();
                        drop(tx.unbounded_send(Msg::done(shots::paste_toast(at))));
                    }
                }
                Ok(_) => drop(tx.unbounded_send(Msg::done("Screenshot copied"))),
                Err(e) => drop(tx.unbounded_send(Msg::error(e))),
            }
        });
    }

    // ---- Quick AI -------------------------------------------------------------------

    /// Opens an empty chat and starts Claude, so it is ready by the time a question is typed.
    fn open_ai(&mut self, cx: &mut Context<Self>) {
        if !self.ensure_host() {
            return self.refresh(cx);
        }
        self.status = None;
        self.send(json!({"t": "aiOpen", "model": self.cfg.ai.model}));
        self.ai = Some(AiChat::default());
        self.set_mode(Mode::Ai, cx);
    }

    /// Asks in the open chat (or starts one); earlier questions stay as context.
    fn ask(&mut self, prompt: String, cx: &mut Context<Self>) {
        if !self.ensure_host() {
            return self.refresh(cx);
        }
        self.next_run += 1;
        let id = self.next_run;
        self.status = None;
        self.send(json!({"t": "ai", "id": id, "prompt": prompt, "model": self.cfg.ai.model}));
        let chat = self.ai.get_or_insert_default();
        chat.turns.push(AiTurn { id, prompt, text: String::new() });
        chat.busy = true;
        chat.follow = true;
        self.set_mode(Mode::Ai, cx);
    }

    fn on_ai(&mut self, v: &Value, cx: &mut Context<Self>) {
        if v["t"] == "aiClosed" {
            // The sidecar closed an idle chat. Out of sight it just ends; in view the
            // answers stay, but the next question starts without their context.
            if self.mode == Mode::Ai && !self.visible {
                self.set_mode(Mode::Root, cx);
            } else if self.ai.as_ref().is_some_and(|a| !a.turns.is_empty()) {
                self.status = Some((Tone::Info, "Idle chat closed: the next question starts a new one".into()));
                self.refresh(cx);
            }
            return;
        }
        // Only the newest question's answer is still wanted.
        let Some(chat) = self.ai.as_mut().filter(|a| a.turns.last().map(|t| t.id) == v["id"].as_u64()) else { return };
        if let (Some(text), Some(turn)) = (v["text"].as_str(), chat.turns.last_mut()) {
            turn.text.push_str(text);
        }
        if v["t"] == "aiDone" {
            chat.busy = false;
            if let Some(e) = v["error"].as_str() {
                self.status = Some((Tone::Error, e.to_string()));
            }
        }
        self.refresh(cx);
    }

    /// Up and Down read the chat.
    fn scroll_ai(&mut self, dy: f32, cx: &mut Context<Self>) {
        if let Some(chat) = &mut self.ai {
            chat.follow = false;
        }
        let (offset, max) = (self.ai_scroll.offset(), self.ai_scroll.max_offset());
        let y = (offset.y - px(dy)).min(px(0.)).max(-max.y);
        self.ai_scroll.set_offset(gpui::point(offset.x, y));
        cx.notify();
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
            self.menu = None;
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
        let tooltip = [bar.tooltip.as_str(), bar.title.as_str(), cmd_title.as_str()].into_iter().find(|t| !t.is_empty()).unwrap_or("procast").to_string();

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
                            danger: p["destructive"].as_bool().unwrap_or(false),
                            then: None,
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
                    m => self.reply(run, id, Err(format!("{m} is not supported in procast yet"))),
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
            "aiDelta" | "aiDone" | "aiClosed" => self.on_ai(&v, cx),
            "hostExit" => {
                self.host = None;
                self.bg_runs.clear();
                if let Some(chat) = self.ai.as_mut().filter(|a| a.busy) {
                    chat.busy = false;
                    self.status = Some((Tone::Error, "The extension host stopped".into()));
                    self.refresh(cx);
                }
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
                    let tone = match p["style"].as_str().unwrap_or_default().to_lowercase().as_str() {
                        "failure" => Tone::Error,
                        "success" => Tone::Ok,
                        _ => Tone::Info,
                    };
                    (tone, if m.is_empty() { t } else { format!("{t}: {m}") })
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
                self.status = Some((Tone::Error, "Move to Recycle Bin isn't supported yet".into()));
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

    /// The form on screen as rows, and the selected one: a native form or an extension's.
    fn form_fields(&self) -> Option<(Vec<FormField>, usize)> {
        if matches!(self.mode, Mode::Form(_)) {
            return self.form.as_ref().map(|f| (f.fields.clone(), f.field));
        }
        let ev = self.extv.as_ref().filter(|_| self.is_form())?;
        Some((ev.view.fields.iter().map(|f| ext_field(f, ev)).collect(), ev.field))
    }

    /// Opens a native form on `fields` (text controls get their boxes here), the first control focused.
    fn open_form(&mut self, kind: FormKind, mut fields: Vec<FormField>, window: &mut Window, cx: &mut Context<Self>) {
        let mut subs = vec![];
        for (i, f) in fields.iter_mut().enumerate().filter(|(_, f)| matches!(f.ctl, Ctl::Text { .. })) {
            let input = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
            input.update(cx, |s, cx| s.emplace(&f.value, cx));
            subs.push(cx.subscribe(&input, move |this, input, _: &TextChanged, cx| {
                let value = input.read(cx).as_str().to_string();
                if let Some(f) = this.form.as_mut().and_then(|f| f.fields.get_mut(i)) {
                    f.value = value;
                    f.error = None;
                }
            }));
            f.input = Some(input);
        }
        let field = fields.iter().position(FormField::focusable).unwrap_or(0);
        self.form = Some(NativeForm { fields, field, _subs: subs });
        self.form_gen += 1;
        if self.visible {
            self.set_mode(Mode::Form(kind), cx);
        } else {
            self.show(Mode::Form(kind), window, cx);
        }
        self.focus_field(window, cx);
    }

    fn focus_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((fields, i)) = self.form_fields() else { return };
        let handle = fields.get(i).and_then(|f| f.input.as_ref()).map_or_else(|| self.form_focus.clone(), |input| input.focus_handle(cx));
        window.focus(&handle, cx);
        cx.notify();
    }

    fn move_field(&mut self, d: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((fields, i)) = self.form_fields() else { return };
        let i = next_field(&fields, i, d);
        match (&mut self.form, &mut self.extv) {
            (Some(f), _) if matches!(self.mode, Mode::Form(_)) => f.field = i,
            (_, Some(ev)) => ev.field = i,
            _ => {}
        }
        self.focus_field(window, cx);
    }

    /// Down or Enter on a Select control (or a token field), Down on a Text control with
    /// presets: its menu under it. False for other controls.
    fn open_field_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((fields, i)) = self.form_fields() else { return false };
        if matches!(self.mode, Mode::Form(_)) && matches!(fields.get(i).map(|f| &f.ctl), Some(Ctl::Tokens)) {
            self.open_block_menu(i, window, cx);
            return true;
        }
        let Some(m) = fields.get(i).and_then(|f| field_menu(f, self.form_at.get())) else { return false };
        // An extension may re-render while the menu is open: its field is known by id, not place.
        let ext_id = self.extv.as_ref().filter(|_| self.mode == Mode::Ext).and_then(|e| e.view.fields.get(i)).map(|f| f.id.clone());
        self.open_menu(
            m,
            move |this, id, window, cx| match (&mut this.form, &ext_id) {
                (Some(f), _) if matches!(this.mode, Mode::Form(_)) => {
                    if let Some(f) = f.fields.get_mut(i) {
                        // A text box's preset is typed in.
                        if let Some(input) = f.input.clone() {
                            input.update(cx, |s, cx| s.emplace(&id, cx));
                        }
                        f.value = id;
                    }
                    cx.notify();
                }
                (_, Some(fid)) => this.run(Act::FormSet(fid.clone(), id), None, window, cx),
                _ => {}
            },
            window,
            cx,
        );
        true
    }

    /// Enter on an extension's checkbox.
    fn toggle_check(&mut self, cx: &mut Context<Self>) {
        let Some(ev) = &mut self.extv else { return };
        let Some(f) = ev.view.fields.get(ev.field).cloned() else { return };
        let on = !ev.values.get(&f.id).and_then(Value::as_bool).unwrap_or(false);
        ev.values.insert(f.id.clone(), json!(on));
        if let Some(h) = f.on_change {
            let run = ev.run;
            self.send(json!({"t": "event", "run": run, "fn": h, "args": [on]}));
        }
        cx.notify();
    }

    fn submit_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Mode::Form(kind) = self.mode {
            // Each form acts on `form_values`.
            match kind {
                FormKind::Focus => self.submit_focus(window, cx),
                FormKind::AddCalendar => self.add_calendar(cx),
                FormKind::AddLanguageSet(edit) => self.save_lang_set(edit, window, cx),
                FormKind::ImportScreenTime => self.import_screen_time(window, cx),
            }
            return;
        }
        let Some(ev) = &self.extv else { return };
        if let Some(f) = ev.view.actions.first().and_then(|a| a.on_action.clone()) {
            let (run, values) = (ev.run, json!(ev.values));
            self.send(json!({"t": "event", "run": run, "fn": f, "args": [values]}));
        }
        cx.notify();
    }

    /// A form is on screen or under Ctrl+K or a confirm; its keys live on the root (`form_focus`).
    fn form_under(&self) -> bool {
        matches!(self.mode, Mode::Form(_)) || (self.mode == Mode::Ext && self.extv.as_ref().is_some_and(|e| e.view.kind == "Form"))
    }

    /// Ctrl+K or a confirm over a form stops drawing its text box, and a focused box that is
    /// not drawn leaves gpui no key context: the root's `form_focus` takes the keys meanwhile,
    /// and gives them back to the selected box once the form shows again.
    fn guard_form_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.form_under() || self.menu.is_some() {
            return;
        }
        // The ActionPanel's search field keeps the keys while the panel is open.
        if self.panel_filtered() {
            return;
        }
        let on_root = self.form_focus.is_focused(window);
        if self.panel.is_some() || self.confirm.is_some() {
            if !on_root && !self.input.focus_handle(cx).is_focused(window) {
                window.focus(&self.form_focus, cx);
            }
        } else if on_root && self.form_fields().is_some_and(|(f, i)| f.get(i).is_some_and(|f| f.input.is_some())) {
            self.focus_field(window, cx);
        }
    }

    /// A form is on screen: a native one, or an extension's Form.
    fn is_form(&self) -> bool {
        matches!(self.mode, Mode::Form(_)) || (self.mode == Mode::Ext && self.confirm.is_none() && self.extv.as_ref().is_some_and(|e| e.view.kind == "Form"))
    }

    // ---- actions --------------------------------------------------------------------

    /// Enter on the selected row (or on the highlighted Ctrl+K action).
    fn launch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((item, i)) = &self.panel {
            // Nothing matches the panel's search: Enter does nothing.
            let Some((at, (_, act))) = self.panel_rows(cx).into_iter().nth(*i) else { return };
            let (plain, key) = (item.args.is_empty(), item.key.clone());
            self.panel = None;
            // The primary action of a row with arguments goes through its boxes, like Enter.
            if at > 0 || plain {
                return self.run(act, key, window, cx);
            }
        }
        if self.mode == Mode::Install {
            let what = self.query(cx).trim().to_string();
            return self.install(what, cx);
        }
        if let Some((fields, i)) = self.form_fields() {
            match fields.get(i).map(|f| &f.ctl) {
                Some(Ctl::Select(..) | Ctl::Tokens) => _ = self.open_field_menu(window, cx),
                Some(Ctl::Check) => self.toggle_check(cx),
                _ if Some(i) == fields.iter().rposition(FormField::focusable) => self.submit_form(window, cx),
                _ => self.move_field(1, window, cx),
            }
            return;
        }
        self.sync_args(cx);
        let Some(item) = self.items.get(self.selected).cloned() else { return };
        let act = if item.args.is_empty() || !matches!(self.mode, Mode::Root | Mode::Args) {
            item.act.clone()
        } else {
            match search::with_args(&item, &self.arg_values(cx)) {
                Ok(act) => act,
                Err((i, missing)) => {
                    self.arg_missing = Some(i);
                    self.status = Some((Tone::Error, missing));
                    self.focus_arg(i, window, cx);
                    return cx.notify();
                }
            }
        };
        let needs_confirm = match item.act {
            Act::Join(..) | Act::Cmd(Cmd::JoinMeeting) => self.cfg.calendar.confirm_join && self.join_target(&item.act).is_some(),
            Act::Cmd(cmd) => cmd.destructive(),
            Act::Script(i) => self.scripts[i].confirm,
            _ => false,
        };
        if needs_confirm && self.armed != Some(self.selected) {
            self.armed = Some(self.selected);
            return cx.notify();
        }
        self.run(act, item.key, window, cx);
    }

    fn run(&mut self, act: Act, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.ask_args_for(&act, window, cx) {
            return;
        }
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
                self.status = Some((Tone::Ok, if add { "Added to Favorites" } else { "Removed from Favorites" }.into()));
                return self.save_launcher_list("favorites", list, cx);
            }
            Act::Hide(key) => {
                let mut list = self.cfg.launcher.hidden.clone();
                if !list.contains(&key) {
                    list.push(key);
                }
                self.status = Some((Tone::Info, "Hidden from root search. Undo in Settings → Launcher".into()));
                return self.save_launcher_list("hidden", list, cx);
            }
            Act::Cmd(Cmd::ClearSearchHistory) => {
                self.searches.clear();
                let _ = std::fs::remove_file(config::dir().join("search-history.txt"));
            }
            Act::App(id) => win::launch_app(&id),
            Act::RunAsAdmin(id) => win::run_as_admin(&id),
            // A quicklink switches to a tab already showing it, when a browser is connected.
            // The browser companion can't see a link procast opens itself, so check it here.
            Act::Open(url) if self.focus.blocked_url(&url).is_some() => {
                self.say(Msg::error(format!("{} is blocked until Focus ends", focus::host_of(&url).unwrap_or_default())));
            }
            Act::FocusStart(name, mode) => {
                let Some(c) = focus::category(&focus::categories(&self.cfg.focus_category), &name).cloned() else { return };
                let picks = vec![format!("cat:{}", c.name)];
                let start = focus::Start { title: name, minutes: self.cfg.focus.default_minutes, filter: Some((mode, c.apps, c.sites)), picks };
                let text = self.focus_begin(start, false);
                self.say(Msg::done(text));
            }
            Act::Cmd(Cmd::Focus(f)) => return self.focus_cmd(f, window, cx),
            Act::FocusExtend => {
                self.focus_extend();
                return self.refresh(cx);
            }
            Act::With(act, got) => return self.run_with(*act, got, window, cx),
            Act::Open(url) if self.cfg.search.existing_tab && key.as_deref().is_some_and(|k| k.starts_with("ql:")) && url.starts_with("http") && self.browsers.any() => {
                win::allow_foreground();
                if !self.browsers.ask(None, json!({"t": "open", "url": url})) {
                    win::open(&url);
                }
                self.hide(false, cx);
                return;
            }
            Act::Open(url) if snippets::link_needs_fill(&url) => {
                self.hide(true, cx);
                return snippets::open_link(self.hwnd, url, self.tx.clone());
            }
            Act::Open(target) | Act::File(target) => win::open(&target),
            Act::Reveal(path) => win::reveal(&path),
            Act::Run(cmdline) => win::run(&cmdline, &self.cfg.search),
            Act::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Act::Paste(text) => return self.paste(text, cx),
            Act::Clip(key) => {
                if self.put_clip(&key) {
                    self.hide(true, cx);
                    return clipboard::paste_soon();
                }
                self.status = Some((Tone::Error, "Could not put that on the clipboard".into()));
                return cx.notify();
            }
            Act::CopyClip(key) => {
                if !self.put_clip(&key) {
                    self.status = Some((Tone::Error, "Could not put that on the clipboard".into()));
                    return cx.notify();
                }
            }
            Act::PinClip(key, on) => {
                self.history.pin(&key, on);
                self.status = Some((Tone::Ok, if on { "Pinned" } else { "Unpinned" }.into()));
                return self.refresh(cx);
            }
            Act::DeleteClip(key) => {
                self.history.remove(&key);
                self.status = Some((Tone::Ok, "Deleted from clipboard history".into()));
                return self.refresh(cx);
            }
            Act::Snippet(template) => {
                let name = key.as_deref().and_then(|k| k.strip_prefix("sn:")).unwrap_or("snippet").to_string();
                return self.start_snippet(name, template, 0, String::new(), window, cx);
            }
            Act::Script(i) => return self.start_script(i, vec![], cx),
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
                    match c.then {
                        Some(then) if ok => then(self, cx),
                        Some(_) => {}
                        None => self.reply(c.run, c.id, Ok(json!(ok))),
                    }
                }
                return self.refresh(cx);
            }
            Act::UninstallExt(name) => {
                let dir = ext::dir().join(&name);
                if dir.starts_with(ext::dir()) && dir.join("package.json").exists() {
                    let _ = std::fs::remove_dir_all(&dir);
                }
                self.load_extensions();
                self.status = Some((Tone::Ok, format!("Uninstalled {name}")));
                return self.refresh(cx);
            }
            Act::None => return,
            Act::Cmd(Cmd::SearchFiles) => return self.set_mode(Mode::Files, cx),
            Act::Cmd(Cmd::SearchScreenshots) => return self.set_mode(Mode::Shots, cx),
            Act::Cmd(Cmd::MySchedule) => return self.set_mode(Mode::Schedule, cx),
            Act::Cmd(Cmd::ScreenTime) => return self.set_mode(Mode::ScreenTime(wellbeing::Period { range: wellbeing::Range::Day, anchor: win::local_now().0 }), cx),
            Act::Cmd(Cmd::ScreenTimeWeek) => return self.set_mode(Mode::ScreenTime(wellbeing::Period { range: wellbeing::Range::Week, anchor: win::local_now().0 }), cx),
            // The dashboard is served only while Screen Time is on; asking again retries a bind that failed (a no-op while it runs).
            Act::Cmd(Cmd::OpenScreenTimeDashboard) if self.cfg.screen_time.enabled => {
                wellbeing_http::serve(self.tx.clone(), || _ = wellbeing::flush_pending());
                win::open(wellbeing_http::URL)
            }
            Act::Cmd(Cmd::OpenScreenTimeDashboard) => self.say(Msg::info("Screen Time is off. Run Screen Time to turn it on")),
            Act::MoveToCategory(name) => return self.open_category_menu(name, window, cx),
            Act::ExportScreenTime => return self.export_screen_time(cx),
            Act::EnableScreenTime => {
                match config::edit_file(&config::path(), |t| config::write_value(t, &["screen_time".into(), "enabled".into()], true.into())) {
                    // The config poll starts the sampler and the dashboard.
                    Ok(_) => {
                        self.say(Msg::done("Screen Time is on"));
                        self.set_mode(Mode::Root, cx);
                        self.hide(true, cx);
                    }
                    Err(e) => {
                        self.status = Some((Tone::Error, e));
                        cx.notify();
                    }
                }
                return;
            }
            Act::Cmd(Cmd::SearchNotes) => return self.set_mode(Mode::Notes(NotesList::All), cx),
            Act::Cmd(Cmd::RecentlyDeletedNotes) => return self.set_mode(Mode::Notes(NotesList::Deleted), cx),
            Act::Cmd(Cmd::OpenNotes) => {
                self.hide(false, cx);
                // An open window already shows the note opened last.
                if let Some(h) = self.notes_window
                    && h.update(cx, |w, window, cx| w.has_note() && {
                        w.reveal(window, cx);
                        true
                    })
                    .unwrap_or(false)
                {
                    return;
                }
                return self.show_note(
                    false,
                    |s, now| {
                        Ok(match s.last() {
                            Some(id) => id,
                            None => created(s.create(now)?),
                        })
                    },
                    cx,
                );
            }
            Act::OpenNote(id) => {
                self.hide(false, cx);
                return self.show_note(false, move |_, _| Ok(id), cx);
            }
            Act::RestoreNote(id) => {
                self.hide(false, cx);
                return self.show_note(false, move |s, _| s.restore(&id).map(|_| id), cx);
            }
            Act::PurgeNote(id) => {
                let title = self.note_rows.iter().flatten().find(|(m, _)| m.id == id).map(|(m, _)| m.title.clone()).unwrap_or_default();
                let then = move |this: &mut Launcher, cx: &mut Context<Launcher>| {
                    this.notes.run(cx, move |s| s.purge(&id), |this, r, cx| {
                        this.status = Some(match r {
                            Ok(()) => (Tone::Ok, "Note deleted permanently".into()),
                            Err(e) => (Tone::Error, format!("Couldn't delete the note: {e}")),
                        });
                        if matches!(this.mode, Mode::Notes(_)) {
                            this.refresh_in_place(cx);
                        }
                    });
                };
                self.confirm = Some(Confirm { run: 0, id: 0, title: "Delete Note Permanently?".into(), message: title, primary: "Delete".into(), danger: true, then: Some(Box::new(then)) });
                self.selected = 0;
                return self.refresh(cx);
            }
            Act::Join(_, url) => win::open(&url),
            Act::Cmd(Cmd::JoinMeeting) => match self.join_target(&Act::Cmd(Cmd::JoinMeeting)) {
                Some((title, url)) => {
                    win::open(&url);
                    self.say(Msg::done(format!("Joining {title}")));
                }
                None => self.say(Msg::error("No meeting with a link right now")),
            },
            Act::HideEvent(key) => {
                match calendar::hide(&config::dir().join(CALENDAR_HIDDEN), key) {
                    Ok(set) => self.hidden_events = set,
                    Err(e) => self.status = Some((Tone::Error, e)),
                }
                return self.refresh(cx);
            }
            Act::AddCalendar => return self.open_form(FormKind::AddCalendar, calendar_form(), window, cx),
            Act::Cmd(Cmd::ImportScreenTime) => return self.open_form(FormKind::ImportScreenTime, import_screen_time_form(), window, cx),
            Act::Cmd(Cmd::PasteLatestScreenshot) => return self.shot_to_clipboard(None, true, cx),
            Act::CopyShot(path) => return self.shot_to_clipboard(Some(path.into()), false, cx),
            Act::PasteShot(path) => return self.shot_to_clipboard(Some(path.into()), true, cx),
            Act::OpenWith(path) => win::open_with(&path),
            Act::QuickLook(_) => {
                if self.quick_look {
                    self.close_quick_look(cx);
                } else {
                    self.quick_look = true;
                }
                return cx.notify();
            }
            Act::PinShot(path, on) => {
                match shots::pin(Path::new(&path), on) {
                    Ok(pins) => {
                        self.shot_pins = pins.clone();
                        self.shots.set_pins(pins);
                        self.status = Some((Tone::Ok, if on { "Pinned" } else { "Unpinned" }.into()));
                    }
                    Err(e) => self.status = Some((Tone::Error, e)),
                }
                return self.refresh_in_place(cx);
            }
            Act::CopyText(path) => {
                let text = self.shots.text(Path::new(&path)).trim().to_string();
                if text.is_empty() {
                    self.status = Some((Tone::Error, "No text found in this screenshot".into()));
                    return cx.notify();
                }
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.say(Msg::done("Copied text"));
            }
            Act::DeleteShot(path) => {
                let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let at = self.selected;
                let then = move |this: &mut Launcher, _: &mut Context<Launcher>| {
                    let tx = this.tx.clone();
                    // Windows may ask before a delete it can't undo: let its question come forward.
                    win::allow_foreground();
                    std::thread::spawn(move || {
                        let path = PathBuf::from(path);
                        let done = shots::delete(&path);
                        let _ = tx.unbounded_send(Msg::ShotDeleted(path, at, done));
                    });
                };
                self.confirm = Some(Confirm { run: 0, id: 0, title: "Delete Screenshot?".into(), message: name, primary: "Delete".into(), danger: true, then: Some(Box::new(then)) });
                self.selected = 0;
                return self.refresh(cx);
            }
            Act::Cmd(Cmd::ClipboardHistory) => return self.set_mode(Mode::Clipboard, cx),
            Act::Cmd(Cmd::Emoji) => return self.set_mode(Mode::Emoji, cx),
            Act::Cmd(Cmd::InstallExtension) => return self.set_mode(Mode::Install, cx),
            Act::Cmd(Cmd::BrowseStore) => return self.set_mode(Mode::Store, cx),
            Act::Cmd(Cmd::QuitApps) => return self.set_mode(Mode::Running, cx),
            Act::Cmd(Cmd::SearchTabs) => return self.set_mode(Mode::Tabs, cx),
            Act::Cmd(Cmd::YtMusic(screen)) => return self.music_open(screen, cx),
            Act::Cmd(Cmd::YtImportSession) => return self.music_import_session(cx),
            Act::Cmd(Cmd::YtControl(control)) => return self.music_act(music_view::MusicAct::Control(control), cx),
            Act::Music(act) => return self.music_act(act, cx),
            Act::Cmd(Cmd::BrowserSetup) => {
                if let Err(e) = browser::register() {
                    self.status = Some((Tone::Error, format!("Could not set up the extension: {e}")));
                    return cx.notify();
                }
                win::open(&browser::extension_dir().to_string_lossy());
                // An instruction to follow while the folder opens: it stays until dismissed.
                let _ = self.tx.unbounded_send(Msg::sticky(toast::Kind::Info, "Load unpacked in chrome://extensions, Developer mode on"));
            }
            Act::Tab(conn, id) => {
                let window = self.tabs.iter().find(|(c, _, t)| *c == conn && t.id == id).map_or(0, |t| t.2.window);
                // The browser brings its own window forward.
                win::allow_foreground();
                if !self.browsers.ask(Some(conn), json!({"t": "activate", "id": id, "window": window})) {
                    self.status = Some((Tone::Error, "That browser has gone".into()));
                    return cx.notify();
                }
                self.hide(false, cx);
                return;
            }
            Act::Cmd(Cmd::AskAi) => return self.open_ai(cx),
            Act::Translate(mut job) => {
                // Translate's boxes leave the languages empty: the current language set's.
                let set = translate::current_set(&self.cfg);
                if job.from.is_empty() {
                    job.from = set.from;
                }
                if job.to.is_empty() {
                    job.to = set.to;
                }
                return self.translate(job, cx);
            }
            Act::Cmd(cmd @ (Cmd::InstantTranslateCopy | Cmd::InstantTranslatePaste)) => {
                // Into the primary language, or the secondary when the text already is in it.
                let after = if cmd == Cmd::InstantTranslateCopy { translate::After::Copy } else { translate::After::Replace };
                let set = self.cfg.translator.set();
                return self.translate(translate::Job { text: String::new(), from: set.from, to: set.to, all: false, after }, cx);
            }
            Act::Define(word) => return self.define(word, false, cx),
            Act::DownloadDictionary(word) => return self.define(word, true, cx),
            Act::SimilarWords(words) => return self.open_similar(words, window, cx),
            Act::LangMenu => return self.open_lang_menu(window, cx),
            Act::UseLangSet(i) => return self.use_lang_set(i, window, cx),
            Act::SetAutoHdr(on) => {
                match config::edit_file(&config::path(), |t| config::write_value(t, &["games".into(), "auto_hdr".into()], on.into())) {
                    Ok((c, _)) => {
                        self.cfg.games.auto_hdr = c.games.auto_hdr;
                        // Redrawn in place: the selection stays on the row.
                        for it in self.items.iter_mut().filter(|it| matches!(it.act, Act::SetAutoHdr(_))) {
                            *it = auto_hdr_row(c.games.auto_hdr);
                        }
                    }
                    Err(e) => self.status = Some((Tone::Error, e)),
                }
                return cx.notify();
            }
            Act::EditLangSet(i) => {
                let set = i.and_then(|i| translate::sets(&self.cfg).get(i).cloned());
                return self.open_form(FormKind::AddLanguageSet(i), lang_set_form(set.as_ref()), window, cx);
            }
            Act::DeleteLangSet(i) => {
                let label = translate::sets(&self.cfg).get(i).map(translate::LangSet::label).unwrap_or_default();
                let then = move |this: &mut Launcher, _: &mut Context<Launcher>| {
                    if this.save_translator(|t| translate::delete_set(t, i)) {
                        // The sets after it moved up one.
                        this.prev_set = None;
                        this.status = Some((Tone::Ok, "Language set deleted".into()));
                    }
                };
                self.confirm = Some(Confirm { run: 0, id: 0, title: "Delete Language Set?".into(), message: label, primary: "Delete".into(), danger: true, then: Some(Box::new(then)) });
                self.selected = 0;
                return self.refresh(cx);
            }
            Act::StartTimer(secs, label) => {
                let d = Duration::from_secs(secs);
                self.timers.start(label.clone(), d);
                self.watch_timers();
                self.say(Msg::done(["Timer set", &timers::fmt_spoken(d), &label].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")));
            }
            Act::StopTimer(id) => {
                // A stopwatch's time goes to the clipboard; a timer is cancelled, a done one dismissed.
                let now = SystemTime::now();
                let done = self.timers.rows(now).iter().any(|r| r.id == id && r.done);
                self.status = match self.timers.elapsed(id, now).map(timers::fmt_duration) {
                    Some(time) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(time.clone()));
                        Some((Tone::Ok, format!("Stopped at {time}, copied")))
                    }
                    None if done => None,
                    None => Some((Tone::Info, "Timer cancelled".into())),
                };
                self.timers.cancel(id);
                return self.refresh(cx);
            }
            Act::PauseTimer(id) => {
                self.timers.pause(id, SystemTime::now());
                return self.refresh_keeping(cx);
            }
            Act::ResumeTimer(id) => {
                self.timers.resume(id, SystemTime::now());
                self.watch_timers();
                return self.refresh_keeping(cx);
            }
            Act::RestartTimer(id) => {
                self.timers.restart(id, SystemTime::now());
                self.watch_timers();
                return self.refresh_keeping(cx);
            }
            Act::AddTime(id, secs) => {
                self.timers.add(id, Duration::from_secs(secs));
                return self.refresh_keeping(cx);
            }
            Act::RenameTimer(id) => {
                let Some(r) = self.timers.rows(SystemTime::now()).into_iter().find(|r| r.id == id) else { return };
                let (kind, glyph) = timer_kind(r.stopwatch);
                let name = search::Arg { optional: true, placeholder: "Type name".into(), ..search::Arg::new("Name") };
                let mut it = Item::new(format!("Rename {kind}"), "", Icon::Vector(glyph), Act::RenameTimer(id)).tinted(design::Tint::System).boxes(vec![name], &[]);
                it.values = vec![r.title];
                return self.ask_args(it, window, cx);
            }
            Act::Ask(prompt) => return self.ask(prompt, cx),
            Act::QuitApp(pid) => {
                apps::quit(pid);
                self.status = Some((Tone::Info, "Asked to quit; it may ask to save first".into()));
                return self.load_running_after(Duration::from_millis(700));
            }
            Act::ForceQuit(pid) => {
                self.status = Some(if apps::force_quit(pid) { (Tone::Ok, "Force quit".into()) } else { (Tone::Error, "Could not end it (it may run as administrator)".into()) });
                return self.load_running_after(Duration::from_millis(300));
            }
            Act::Install(name) => return self.install(name, cx),
            Act::Cmd(Cmd::ClearClipboardHistory) => self.history.clear(),
            Act::Cmd(Cmd::Settings) => {
                self.hide(false, cx);
                return self.open_settings(None, cx);
            }
            Act::AddShotFolder => {
                self.hide(false, cx);
                return self.open_settings(Some("Screenshots"), cx);
            }
            Act::Cmd(Cmd::EditConfig) => win::open(&config::path().to_string_lossy()),
            Act::Cmd(Cmd::ReloadApps) => self.scan_apps(true, true),
            Act::Cmd(Cmd::Quit) => return self.quit(cx),
            Act::Layout(name) => {
                self.hide(true, cx);
                let Some(layout) = self.cfg.window.layouts.iter().find(|l| l.name == name).cloned() else { return };
                let w = self.cfg.window.clone();
                let ids: Vec<String> = self.apps.iter().map(|a| a.id.clone()).collect();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    if let Err(e) = commands::run_layout(&layout, &w, &ids) {
                        let _ = tx.unbounded_send(Msg::error(e));
                    }
                });
                return;
            }
            Act::Cmd(cmd) => {
                // Hide first so window commands act on the window you came from. Open Desktop
                // must not hand focus back: Windows would follow that window to its desktop
                // after the switch, undoing it.
                self.hide(!matches!(cmd, Cmd::Desktop(Desk::Open(_))), cx);
                let target = self.prev.0 as isize;
                let w = self.cfg.window.clone();
                let tx = self.tx.clone();
                let hdr_seq = (cmd == Cmd::ToggleHdr).then(|| self.hdr_read());
                std::thread::spawn(move || {
                    let said = match commands::run(cmd, HWND(target as _), &w) {
                        Ok(done) => done.map(Msg::done),
                        Err(e) => Some(Msg::error(e)),
                    };
                    if let Some(toast) = said {
                        let _ = tx.unbounded_send(toast);
                    }
                    if let Some(seq) = hdr_seq {
                        let _ = tx.unbounded_send(Msg::Hdr(seq, hdr::displays()));
                    }
                });
                return;
            }
        }
        // Hand focus back even when launching: a cloaked launcher that stays foreground
        // would swallow keys typed before the new app's window appears.
        self.hide(true, cx);
    }

    /// Enter on a row whose act takes its boxes' values as they are (`Act::With`).
    fn run_with(&mut self, act: Act, got: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Act::Script(i) => self.start_script(i, got, cx),
            Act::Snippet(template) => {
                let (erase, then) = std::mem::take(&mut self.typed);
                self.mode = Mode::Root;
                self.insert_snippet(template, got, erase, then, cx);
            }
            Act::Cmd(Cmd::Focus(f)) => self.focus_args(f, got, window, cx),
            Act::Cmd(Cmd::CreateNote) => {
                self.hide(false, cx);
                let title = got[0].trim().to_string();
                self.show_note(
                    !title.is_empty(),
                    move |s, now| {
                        let id = created(s.create(now)?);
                        if !title.is_empty() {
                            s.save(&id, &format!("# {title}\n"), now)?;
                        }
                        Ok(id)
                    },
                    cx,
                );
            }
            Act::Cmd(Cmd::StartStopwatch) => {
                self.timers.stopwatch(got[0].clone());
                self.watch_timers();
                self.say(Msg::done("Stopwatch started"));
                // A hotkey starts it without opening the launcher.
                if self.visible {
                    self.hide(true, cx);
                }
            }
            Act::RenameTimer(id) => {
                self.timers.rename(id, got[0].trim().to_string());
                self.args_row = None;
                self.set_mode(Mode::Root, cx);
                window.focus(&self.input.focus_handle(cx), cx);
            }
            Act::Cmd(Cmd::Desktop(Desk::Rename)) => {
                self.hide(true, cx);
                let (tx, name) = (self.tx.clone(), got[0].clone());
                std::thread::spawn(move || {
                    if let Err(e) = desktops::rename(&name) {
                        let _ = tx.unbounded_send(Msg::error(e));
                    }
                });
            }
            Act::ExtRun(e, c) => {
                // The extension gets its arguments by name; empty ones are left out.
                let names = self.exts.get(e).and_then(|(x, _)| x.commands.get(c)).map(|c| c.args.iter().map(|a| a.name.to_string()).collect::<Vec<_>>()).unwrap_or_default();
                let args: serde_json::Map<String, Value> = names.into_iter().zip(got).filter(|(_, v)| !v.is_empty()).map(|(n, v)| (n, json!(v))).collect();
                self.start_ext(e, c, false, Value::Object(args), cx);
            }
            act => self.run(act, None, window, cx),
        }
    }

    /// Lists running apps in the background; icons come from the cache or are read once.
    fn load_running(&mut self) {
        self.load_running_after(Duration::ZERO);
    }

    fn load_running_after(&mut self, delay: Duration) {
        let (tx, known) = (self.tx.clone(), self.exe_icons.clone());
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let list = apps::running().into_iter().map(|r| {
                let icon = match known.get(&r.exe) {
                    Some(i) => i.clone(),
                    None => win::file_icon(&r.exe),
                };
                (r, icon)
            });
            let _ = tx.unbounded_send(Msg::Running(list.collect()));
        });
    }

    /// The command a hotkey past `BINDS` runs (None if it's switched off or gone).
    fn hotkey_item(&self, i: usize) -> Option<&Item> {
        let key = self.cfg.launcher.command_hotkeys().nth(i.checked_sub(BINDS.len())?)?.0;
        self.catalog.iter().find(|it| it.key.as_ref() == Some(key))
    }

    /// Whether running this needs the launcher on screen (it opens a view or asks for input).
    fn needs_window(&self, act: &Act) -> bool {
        if self.asks_args(act) {
            return true;
        }
        match act {
            Act::Cmd(Cmd::Focus(FocusCmd::Toggle)) => self.focus.session.is_none() && self.focus.preset.is_none(),
            // Join Meeting asks first: its hotkey opens the launcher on the question (`ask_join`).
            Act::Cmd(Cmd::JoinMeeting) => self.cfg.calendar.confirm_join && self.join_target(act).is_some(),
            Act::Cmd(Cmd::Focus(FocusCmd::Start | FocusCmd::Edit | FocusCmd::SearchCategories | FocusCmd::CreateCategory | FocusCmd::ImportCategories) | Cmd::ImportScreenTime | Cmd::ScreenTime | Cmd::ScreenTimeWeek) => true,
            Act::Cmd(Cmd::SearchFiles | Cmd::SearchScreenshots | Cmd::SearchNotes | Cmd::RecentlyDeletedNotes | Cmd::ClipboardHistory | Cmd::Emoji | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::QuitApps | Cmd::AskAi | Cmd::YtMusic(_)) | Act::Define(_) => true,
            Act::Script(i) => self.scripts.get(*i).is_some_and(|s| matches!(s.output, Output::Full)),
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
        // An app's hotkey brings its open window forward (or hides it) instead of starting another.
        if let Act::App(id) = &it.act
            && self.cfg.apps.hotkey_action != "launch"
        {
            let open = apps::windows_of(id, win::app_target(id).as_deref());
            let fg = win::foreground();
            if self.cfg.apps.hotkey_action == "toggle" && open.contains(&fg) {
                return apps::minimize(fg);
            }
            if let Some(&w) = open.first() {
                if self.visible {
                    self.hide(false, cx);
                }
                return apps::bring_forward(w);
            }
        }
        if self.needs_window(&it.act) {
            self.show(Mode::Root, window, cx);
            if it.act == Act::Cmd(Cmd::JoinMeeting) {
                return self.ask_join(cx);
            }
        } else if !self.visible {
            let fg = win::foreground();
            if fg != self.hwnd {
                self.prev = fg;
            }
        }
        self.run(it.act, it.key, window, cx);
    }

    /// Once at startup, on the store's worker: notes deleted longer ago than `[notes]
    /// keep_deleted_days` go for good.
    fn purge_old_notes(&mut self, cx: &mut Context<Self>) {
        if let Some(e) = self.cfg.notes.folder_error() {
            self.say(Msg::error(e));
        }
        let (now, days) = (search::now() as i64, self.cfg.notes.keep_days());
        self.notes.run(cx, move |s| s.purge_expired(now, days), |this, r, _| {
            if let Err(e) = r {
                this.say(Msg::error(format!("Notes: {e}")));
            }
        });
    }

    /// Shows a note in the Notes window, opening the window the first time. `pick` chooses the
    /// note on the store's worker (and may change the store); the note is then marked opened
    /// and read there too.
    /// `wrote`: `pick` writes the note's text (Create Note's title), which the window then shows
    /// even over its own copy of that note.
    fn show_note(&mut self, wrote: bool, pick: impl FnOnce(&mut Store, i64) -> std::io::Result<String> + Send + 'static, cx: &mut Context<Self>) {
        let now = search::now() as i64;
        let place = self.notes_window.is_none().then(notes::window::place_path);
        // The note on screen is written before `pick` reads anything.
        if let Some(h) = self.notes_window {
            drop(h.update(cx, |w, _, cx| w.save(cx)));
        }
        self.notes.run(
            cx,
            move |s| {
                let id = pick(s, now)?;
                s.open_note(&id, now)?;
                let text = s.text(&id)?;
                Ok((id, text, place.and_then(|p| notes::window::load_place(&p))))
            },
            move |this, r, cx| match r {
                Ok((id, text, place)) => this.note_window(id, text, wrote, place, cx),
                Err(e) => this.note_failed(format!("Couldn't open the note: {e}")),
            },
        );
    }

    /// Opening a note failed: the launcher hid without handing focus back (the window was to take
    /// it), so the app you came from gets it again, and a toast says why.
    fn note_failed(&mut self, why: String) {
        if !self.visible && !self.prev.is_invalid() {
            win::focus(self.prev);
        }
        self.say(Msg::error(why));
    }

    /// The Notes window on note `id`: reused while it exists, else opened where it was last
    /// (`place`, when that is still on a screen) or centred.
    fn note_window(&mut self, id: String, text: String, wrote: bool, place: Option<Place>, cx: &mut Context<Self>) {
        if let Some(h) = self.notes_window
            && h.update(cx, |w, window, cx| w.show(id.clone(), text.clone(), wrote, window, cx)).is_ok()
        {
            return;
        }
        let bounds = place
            .map(Place::bounds)
            .filter(|b| cx.displays().iter().any(|d| d.bounds().intersects(b)))
            .unwrap_or_else(|| Bounds::centered(None, size(gpui::px(design::NOTES_W), gpui::px(design::NOTES_H)), cx));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // Hidden: the header is the caption (drag area and close).
            titlebar: Some(gpui::TitlebarOptions { title: Some("Notes".into()), appears_transparent: true, ..Default::default() }),
            windows_window_background: backdrop(design::refresh_glass()),
            ..Default::default()
        };
        let (notes, tx, theme) = (self.notes.clone(), self.tx.clone(), self.cfg.theme.clone());
        self.notes_window = cx.open_window(options, |window, cx| cx.new(|cx| NotesWindow::new(notes, tx, theme, window, cx))).ok();
        match self.notes_window {
            Some(h) => drop(h.update(cx, |w, window, cx| w.show(id, text, wrote, window, cx))),
            None => self.note_failed("Couldn't open the Notes window".into()),
        }
    }

    /// Asks the store for `Mode::Notes`' rows; the answer replaces the list if nothing newer was asked.
    fn search_notes(&mut self, list: NotesList, query: &str, cx: &mut Context<Self>) {
        self.note_gen += 1;
        let (generation, q) = (self.note_gen, query.to_string());
        self.notes.run(cx, move |s| Ok(s.browse(list == NotesList::Deleted, &q)), move |this, r, cx| {
            if this.note_gen != generation || this.mode != Mode::Notes(list) {
                return;
            }
            this.note_rows = Some(r.unwrap_or_else(|e| {
                this.status = Some((Tone::Error, format!("Couldn't read the notes: {e}")));
                vec![]
            }));
            if this.confirm.is_none() {
                let q = this.query(cx);
                this.items = this.note_items(list, &q, cx);
                this.selected = this.selected.min(this.items.len().saturating_sub(1));
                this.scroll_to_selected();
            }
            cx.notify();
        });
    }

    fn note_items(&self, list: NotesList, query: &str, cx: &App) -> Vec<Item> {
        match &self.note_rows {
            None => vec![empty_row(g::CLOCK, "Loading…".into(), String::new(), vec![])],
            Some(rows) => {
                let current = self.notes_window.and_then(|h| h.read(cx).ok()).and_then(NotesWindow::current);
                note_rows(list, rows, current, search::now() as i64, !query.trim().is_empty(), self.cfg.notes.keep_days())
            }
        }
    }

    /// Reads the span's stats on the background executor, after writing the sampler's pending
    /// counters so the numbers are current; the answer fills the list (`screen_time_loaded`).
    /// `cats` are the categories to file names under: the config's, or a move's fresh ones.
    fn load_screen_time(&mut self, p: wellbeing::Period, cats: Vec<config::FocusCategory>, cx: &mut Context<Self>) {
        if !self.cfg.screen_time.enabled {
            return;
        }
        self.st_gen += 1;
        let today = win::local_now().0;
        let (generation, (span, before), rules) = (self.st_gen, wellbeing::span(p, today), wellbeing::rules(&cats));
        let read = cx.background_executor().spawn(async move {
            let _ = wellbeing::flush_pending();
            let con = wellbeing::open(&wellbeing::db_path()).map_err(|e| e.to_string())?;
            let stats = wellbeing::stats(&con, span.0, span.1, before, &rules).map_err(|e| e.to_string())?;
            let hours = if span.0 == span.1 { wellbeing::hours_by_name(&con, span.0).map_err(|e| e.to_string())? } else { Default::default() };
            let days = wellbeing::days(&con, today - 370, today, &rules).map_err(|e| e.to_string())?;
            Ok((stats, hours, days))
        });
        cx.spawn(async move |this, cx| {
            let r = read.await;
            _ = this.update(cx, |this, cx| this.screen_time_loaded(p, generation, r, cx));
        })
        .detach();
    }

    fn screen_time_loaded(&mut self, p: wellbeing::Period, generation: u64, r: Result<(Value, wellbeing::Hours, Value), String>, cx: &mut Context<Self>) {
        let Mode::ScreenTime(current) = self.mode else { return };
        if !wellbeing::fresh(current, p, self.st_gen, generation) {
            return;
        }
        let (stats, hours, days) = match r {
            Ok((stats, hours, days)) => (Ok(stats), hours, Some(days)),
            Err(e) => (Err(e), Default::default(), None),
        };
        (self.st_stats, self.st_hours, self.st_days) = (Some(stats), hours, days);
        if self.confirm.is_none() {
            let q = self.query(cx);
            self.items = self.screen_time_items(p, &q);
            self.selected = self.selected.min(self.items.len().saturating_sub(1));
            self.scroll_to_selected();
        }
        cx.notify();
    }

    fn screen_time_items(&mut self, p: wellbeing::Period, query: &str) -> Vec<Item> {
        match &self.st_stats {
            _ if !self.cfg.screen_time.enabled => screen_time_off_row(),
            None => vec![empty_row(g::CLOCK, "Loading…".into(), String::new(), vec![])],
            Some(Err(e)) => vec![empty_row(g::MONITOR, "Couldn't read screen time".into(), e.clone(), vec![])],
            Some(Ok(stats)) => {
                let mut rows = screen_time_rows(p, stats);
                // Filtered rows come back best match first: keep each section together.
                let mut found = search::filter(query, rows.split_off(1), &mut self.matcher);
                found.sort_by_key(|it| ["Categories", "Apps", "Sites"].iter().position(|s| Some(*s) == it.section.as_deref()));
                if found.is_empty() {
                    let (title, hint) = if query.trim().is_empty() { ("Nothing tracked yet", "Screen Time records while procast runs") } else { ("No matches", "") };
                    return vec![empty_row(g::MONITOR, title.into(), hint.into(), screen_time_actions())];
                }
                rows.extend(found);
                rows
            }
        }
    }

    /// Screen Time's right-hand pane: the selected row's numbers, the span's on the header row.
    fn screen_time_detail(&self, t: &Tokens) -> gpui::AnyElement {
        let Some(Ok(stats)) = &self.st_stats else { return div().flex_1().into_any_element() };
        let row = self.items.get(self.selected).and_then(|it| wellbeing::rows(stats).into_iter().find(|r| Some(r.section) == it.section.as_deref() && r.name == it.title));
        div().id("screen-time-detail").flex_1().min_h_0().overflow_y_scroll().px(px(16.)).py(px(12.)).flex().flex_col().children(info_rows(&wellbeing::detail(stats, row.as_ref(), &self.st_hours), t)).into_any_element()
    }

    /// Move to Category…: a menu of the categories; the pick files `name` there.
    fn open_category_menu(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let entries = focus::categories(&self.cfg.focus_category).into_iter().map(|c| menu::Entry { icon: Some(focus::category_icon(&c.name)), id: c.name.clone(), label: c.name, ..Default::default() }).collect();
        let m = menu::Menu::new(entries, Some("Search…".into()), menu::Place::Right);
        self.open_menu(m, move |this, category, _, cx| this.move_to_category(name.clone(), category, cx), window, cx);
    }

    /// Writes the move to config.toml off the UI thread, then reads the span again with the new
    /// categories (the config poll takes a moment to hand them over).
    fn move_to_category(&mut self, name: String, category: String, cx: &mut Context<Self>) {
        let Mode::ScreenTime(p) = self.mode else { return };
        let (what, to) = (name.clone(), category.clone());
        let write = cx.background_executor().spawn(async move {
            config::edit_file(&config::path(), |text| {
                let cats = focus::categories(&config::parse(text)?.focus_category);
                wellbeing::move_name(text, &cats, &name, &category)
            })
        });
        cx.spawn(async move |this, cx| {
            let done = write.await;
            _ = this.update(cx, |this, cx| match done {
                Ok((c, _)) => {
                    this.status = Some((Tone::Ok, format!("Moved {what} to {to}")));
                    if this.mode == Mode::ScreenTime(p) {
                        this.load_screen_time(p, focus::categories(&c.focus_category), cx);
                    }
                    cx.notify();
                }
                Err(e) => {
                    this.status = Some((Tone::Error, e));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Writes the span as a CSV in Downloads on the background executor; the answer is a toast.
    fn export_screen_time(&mut self, cx: &mut Context<Self>) {
        let Mode::ScreenTime(p) = self.mode else { return };
        let Ok(home) = std::env::var("USERPROFILE") else { return self.say(Msg::error("Couldn't find your Downloads folder")) };
        let ((from, to), tx) = (wellbeing::span(p, win::local_now().0).0, self.tx.clone());
        let rules = wellbeing::rules(&focus::categories(&self.cfg.focus_category));
        let name = format!("screen-time-{}-to-{}.csv", wellbeing::iso(from), wellbeing::iso(to));
        let path = PathBuf::from(home).join("Downloads").join(&name);
        cx.background_executor()
            .spawn(async move {
                let _ = wellbeing::flush_pending();
                let done = wellbeing::open(&wellbeing::db_path())
                    .and_then(|con| wellbeing::export_csv(&con, from, to, &rules))
                    .map_err(|e| e.to_string())
                    .and_then(|csv| std::fs::write(&path, csv).map_err(|e| e.to_string()));
                let _ = tx.unbounded_send(match done {
                    Ok(()) => Msg::done(format!("Saved {name}")),
                    Err(e) => Msg::error(format!("Couldn't export: {e}")),
                });
            })
            .detach();
        self.set_mode(Mode::Root, cx);
        self.hide(true, cx);
    }

    /// Brings up the settings window, reusing it if it's already open; on `page` when given.
    fn open_settings(&mut self, page: Option<&str>, cx: &mut Context<Self>) {
        let items = search::all_items(&self.apps, &self.scripts, &self.exts, &self.cfg);
        if let Some(h) = self.settings_window
            && h.update(cx, |s, window, cx| {
                s.set_items(items.clone());
                if let Some(page) = page {
                    s.show_section(page, window, cx);
                }
                window.activate()
            })
            .is_ok()
        {
            return;
        }
        let bounds = Bounds::centered(None, size(px(880.), px(620.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // Hidden: Settings draws its own caption bar in the theme (`settings::title_bar`).
            titlebar: Some(gpui::TitlebarOptions { title: Some("procast Settings".into()), appears_transparent: true, ..Default::default() }),
            windows_window_background: gpui::WindowsWindowBackground::MicaBackdrop,
            ..Default::default()
        };
        design::refresh_glass();
        self.refresh_index_status(cx);
        let (tx, status, feeds) = (self.tx.clone(), self.index_status.clone(), self.calendar.status.clone());
        self.settings_window = cx.open_window(options, |window, cx| cx.new(|cx| settings::Settings::new(items, tx, status, feeds, window, cx))).ok();
        if let (Some(h), Some(page)) = (self.settings_window, page) {
            drop(h.update(cx, |s, window, cx| s.show_section(page, window, cx)));
        }
    }

    /// Rewrites the Settings index line from the live index and repaints Settings.
    fn refresh_index_status(&self, cx: &mut Context<Self>) {
        let line = self.files.as_ref().map_or_else(|| "File search is off".to_string(), |f| files::status_line(&f.status(), SystemTime::now()));
        *self.index_status.write().unwrap() = line;
        if let Some(h) = self.settings_window {
            drop(h.update(cx, |_, _, cx| cx.notify()));
        }
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
            self.notice = Some("exthost folder not found next to procast.exe".into());
            return cx.notify();
        };
        let (node, tx, dir) = (self.cfg.extensions.node.clone(), self.tx.clone(), ext::dir());
        self.status = Some((Tone::Info, format!("Installing {what}…")));
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

    fn start_script(&mut self, i: usize, args: Vec<String>, cx: &mut Context<Self>) {
        let script = self.scripts[i].clone();
        match script.output {
            Output::Full => {
                self.output = Some((i, String::new(), true));
                self.set_mode(Mode::Output, cx);
            }
            Output::Compact => {
                self.status = Some((Tone::Info, format!("Running {}…", script.title)));
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

    /// Quit, writing Screen Time's last seconds first.
    fn quit(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.tracker.take() {
            t.stop();
        }
        cx.quit()
    }

    /// New `[screen_time]` and categories for the sampler; starts or stops it when `enabled` flips.
    fn reload_screen_time(&mut self, cfg: &Config) {
        if cfg.focus_category != self.cfg.focus_category {
            *self.st_rules.write().unwrap_or_else(|e| e.into_inner()) = wellbeing::rules(&focus::categories(&cfg.focus_category));
        }
        *self.st_cfg.write().unwrap_or_else(|e| e.into_inner()) = cfg.screen_time.clone();
        match (cfg.screen_time.enabled, self.tracker.is_some()) {
            (true, false) => self.tracker = Some(start_screen_time(&self.tx, &self.browsers, &self.st_rules, &self.st_cfg)),
            // Off the UI thread: the last write goes to SQLite.
            (false, true) => drop(self.tracker.take().map(|t| std::thread::spawn(move || t.stop()))),
            _ => {}
        }
    }

    /// The caller refreshes the glass decision first (it holds the window, for the backdrop).
    fn reload(&mut self, cx: &mut Context<Self>) {
        match config::load() {
            Ok(cfg) => {
                if hotkey_specs(&cfg) != hotkey_specs(&self.cfg) {
                    self.hotkey_errors.clear();
                    self.hotkeys.set(hotkey_specs(&cfg));
                }
                if key_settings(&cfg) != key_settings(&self.cfg) {
                    configure_keys(&cfg, &self.tx);
                }
                if cfg.clipboard != self.cfg.clipboard {
                    clipboard::configure(&cfg.clipboard.ignore_apps, cfg.clipboard.ocr);
                    self.history.set_limits(cfg.clipboard.history, &cfg.clipboard.keep);
                }
                if cfg.games.list != self.cfg.games.list {
                    foreground::set_games(&cfg.games.list);
                }
                if cfg.apps.folders != self.cfg.apps.folders || cfg.apps.deep != self.cfg.apps.deep {
                    self.scanned = None;
                }
                if cfg.start_at_login != self.cfg.start_at_login {
                    win::set_start_at_login(cfg.start_at_login);
                }
                if cfg.tray != self.cfg.tray {
                    self.tray_icon = if cfg.tray { tray(self.tx.clone()) } else { None };
                    // The music tray's own handler takes over when the main one is gone.
                    if self.tray_icon.is_none() {
                        self.music_tray_listen();
                    }
                }
                design::set_size(&cfg.size);
                if cfg.window.auto_close_empty_desktops != self.cfg.window.auto_close_empty_desktops {
                    self.desktop_watch = cfg.window.auto_close_empty_desktops.then(desktops::Listener::start).flatten();
                }
                let same_files = cfg.files.enabled == self.files.is_some()
                    && self.files.as_ref().is_none_or(|f| f.same_settings(&cfg.files));
                if !same_files {
                    self.files = start_files(&cfg, &self.tx);
                    self.refresh_index_status(cx);
                }
                if cfg.calendar_feed.iter().map(|f| &f.url).ne(self.cfg.calendar_feed.iter().map(|f| &f.url)) {
                    self.calendar.reset(&cfg.calendar_feed.iter().map(|f| f.url.clone()).collect::<Vec<_>>());
                }
                if cfg.notes.folder != self.cfg.notes.folder
                    && let Some(e) = cfg.notes.folder_error()
                {
                    self.say(Msg::error(e));
                }
                if cfg.notes.dir() != self.cfg.notes.dir() {
                    self.notes = Notes::start(cfg.notes.dir(), cx);
                    let notes = self.notes.clone();
                    if let Some(h) = self.notes_window {
                        drop(h.update(cx, |w, window, cx| w.use_store(notes, window, cx)));
                    }
                }
                if let Some(h) = self.notes_window {
                    let theme = cfg.theme.clone();
                    drop(h.update(cx, |w, _, cx| {
                        w.theme = theme;
                        cx.notify();
                    }));
                }
                self.scripts = scripts::scan(&cfg.scripts.dirs());
                self.reload_screen_time(&cfg);
                self.cfg = cfg;
                self.music_tray_sync();
                self.notice = None;
            }
            Err(e) => self.notice = Some(e),
        }
        self.rebuild_catalog(cx);
    }

    fn on_msg(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<Self>) {
        match msg {
            Msg::Hotkey(_) if self.cfg.launcher.game_mode && !self.visible && foreground::full_screen_app() => {}
            Msg::Hotkey(i) if i >= BINDS.len() => self.run_hotkey(i, window, cx),
            Msg::Hotkey(i) if self.visible && self.mode == Mode::of(BINDS[i]) => self.hide(true, cx),
            Msg::Hotkey(i) => self.show(Mode::of(BINDS[i]), window, cx),
            Msg::Tray(TrayCmd::Open) => self.show(Mode::Root, window, cx),
            // A fresh root search, list showing: its top row is the session's.
            Msg::Hud(hud::Click::Open) => {
                self.hidden_at = None;
                self.show(Mode::Root, window, cx);
                self.peek = true;
                self.refresh(cx);
            }
            Msg::Hud(hud::Click::Extend) => {
                self.focus_extend();
                if self.visible && self.mode == Mode::Root {
                    self.refresh(cx);
                }
            }
            Msg::Hud(hud::Click::Complete) => self.focus_end(true, cx),
            Msg::Tray(TrayCmd::Settings) => self.open_settings(None, cx),
            Msg::Tray(TrayCmd::EditConfig) => win::open(&config::path().to_string_lossy()),
            Msg::Tray(TrayCmd::Quit) => self.quit(cx),
            Msg::Apps(apps) => {
                self.apps = apps;
                self.rebuild_catalog(cx);
            }
            Msg::Expand(hit) => self.on_expand(hit, window, cx),
            Msg::Clip(entry, png) => {
                if self.cfg.clipboard.enabled {
                    self.history.push(entry, png);
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
            // The old index is dropped; a new one re-crawls the folders (never the saved copy)
            // and reconnects the helper, which scans every drive's MFT afresh.
            Msg::AddCalendar => self.open_form(FormKind::AddCalendar, calendar_form(), window, cx),
            Msg::RecentlyDeletedNotes => {
                let mode = Mode::Notes(NotesList::Deleted);
                if self.visible { self.set_mode(mode, cx) } else { self.show(mode, window, cx) }
            }
            Msg::CalendarAdded(generation, feed, result) => self.calendar_added(generation, feed, result, window, cx),
            Msg::RebuildIndex => {
                self.files = start_files(&self.cfg, &self.tx);
                self.refresh_index_status(cx);
            }
            Msg::FilesIndexed => {
                self.refresh_index_status(cx);
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
                            self.status = (!ok).then(|| (Tone::Error, format!("{} failed", script.title)));
                            self.refresh(cx);
                        }
                    }
                    Output::Compact => {
                        self.status = Some(if ok { (Tone::Ok, if last.is_empty() { format!("{} done", script.title) } else { last }) } else { (Tone::Error, format!("{} failed: {last}", script.title)) });
                        cx.notify();
                    }
                    Output::Silent if !ok => {
                        self.notice = Some(format!("{} failed: {last}", script.title));
                        self.show(Mode::Root, window, cx);
                    }
                    Output::Silent => {}
                }
            }
            // Handled by the message loop, between updates.
            Msg::Toast(..) => {}
            Msg::NowPlaying(now) => self.music_now(now, cx),
            Msg::ToastClose => {
                if let Some(t) = self.toast {
                    _ = t.update(cx, |t, _, cx| t.dismiss(cx));
                }
            }
            Msg::TimerTick => self.on_timer_tick(cx),
            Msg::Translated { seq, job, hidden, result } => self.on_translated(seq, job, hidden, result, window, cx),
            Msg::Defined { seq, word, result } => self.on_defined(seq, word, result, cx),
            Msg::DictReady(ready) => {
                self.dict_ready = Some(ready);
                self.ask_sense(cx);
            }
            Msg::Sense { seq, word, sense } => self.on_sense(seq, word, sense, cx),
            Msg::Shots(seq, index, read) => self.on_shots(seq, index, read, cx),
            Msg::ShotDeleted(path, at, done) => self.on_shot_deleted(path, at, done, cx),
            Msg::Hdr(seq, status) => {
                if take_hdr_read(&mut self.hdr_status, self.hdr_seq, seq, status) {
                    hdr_state(&mut self.items, self.hdr_status.as_deref());
                    cx.notify();
                }
            }
            Msg::Foreground { hwnd, game, exe } => {
                self.hdr.foreground(self.cfg.games.auto_hdr, game, hwnd, exe.clone());
                self.focus_front(hwnd, &exe);
            }
            Msg::Notice(e) => {
                self.notice = Some(e.lines().next().unwrap_or(&e).chars().take(200).collect());
                self.show(Mode::Root, window, cx);
            }
            Msg::Ext(v) => self.on_ext(v, window, cx),
            Msg::Install(message, done) => {
                self.status = Some((match done { Some(true) => Tone::Ok, Some(false) => Tone::Error, None => Tone::Info }, message));
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
                // Entries kept for a time leave once it's up.
                self.history.prune(search::now());
                let idle = Duration::from_secs(self.cfg.apps.auto_quit_minutes.max(1) * 60);
                self.autoquit.tick(&self.cfg.apps.auto_quit, idle, win::foreground());
                self.calendar_tick(cx);
                self.tick(cx);
            }
            Msg::Calendar(results) => {
                for text in self.calendar.done(results, search::now()) {
                    self.say(Msg::error(text));
                }
                // Settings → Calendar says when each feed came.
                if let Some(h) = self.settings_window {
                    drop(h.update(cx, |_, _, cx| cx.notify()));
                }
                // An armed row (Join's confirm) waits for its second Enter: a refresh would move it.
                if self.visible && self.armed.is_none() && self.panel.is_none() && (self.mode == Mode::Schedule || self.mode == Mode::Root && self.query(cx).trim().is_empty()) {
                    self.refresh_keeping(cx);
                }
            }
            Msg::ImageReady(path) => {
                self.images.remove(&path);
                self.music.drop_cover(&path);
                if self.music.cover_is(&path) {
                    self.music_tray_refresh();
                }
                if self.cfg.quicklink.iter().any(|q| net::favicon_url(&q.url).is_some_and(|u| net::image_path(&u) == path)) {
                    self.rebuild_catalog(cx);
                }
                if self.visible && matches!(self.mode, Mode::Ext | Mode::Tabs) {
                    self.refresh(cx);
                }
                if self.visible && self.mode == Mode::Music {
                    self.music_covers_arrived(cx);
                }
            }
            Msg::FilePreview(key, img) => {
                match self.file_images.get_mut(&key) {
                    // A second decode of the same key (reopened meanwhile): free the first.
                    Some(entry) => release(std::mem::replace(&mut entry.0, img).into_iter().collect(), cx),
                    // Forgotten meanwhile (evicted, or the grid closed): not kept.
                    None => release(img.into_iter().collect(), cx),
                }
                release(evict(&mut self.file_images, FILE_IMAGES_MAX, FILE_IMAGES_KEEP), cx);
                if self.visible && matches!(self.mode, Mode::Files | Mode::Shots) {
                    cx.notify();
                }
            }
            Msg::FilePreviewDropped(key) => {
                release(self.file_images.remove(&key).and_then(|e| e.0).into_iter().collect(), cx);
                // Re-selected meanwhile? The next render asks again.
                if self.visible && matches!(self.mode, Mode::Files | Mode::Shots) {
                    cx.notify();
                }
            }
            Msg::BrowserUp => self.focus_push(),
            Msg::Browser(conn, reply) => match reply {
                browser::Reply::Tabs(name, tabs) => {
                    self.tabs.retain(|(c, ..)| *c != conn);
                    self.tabs.extend(tabs.into_iter().map(|t| (conn, name.clone(), t)));
                    if self.mode == Mode::Tabs {
                        self.refresh(cx);
                    }
                }
                browser::Reply::Done(Err(e)) => {
                    let _ = self.tx.unbounded_send(Msg::error(e));
                }
                browser::Reply::Done(Ok(())) => {}
                // Screen Time's active tab: the newest answer from a window that has OS focus wins.
                browser::Reply::Active { focused, host, audible } => {
                    if focused {
                        *wellbeing::TAB.write().unwrap_or_else(|e| e.into_inner()) = Some(wellbeing::Tab { host, audible });
                    }
                }
                browser::Reply::Music(reply) => self.music_reply(reply, cx),
            },
            Msg::Music(reply) => self.music_reply(reply, cx),
            Msg::Running(list) => {
                for (r, icon) in &list {
                    self.exe_icons.insert(r.exe.clone(), icon.clone());
                }
                self.running = Some(list);
                if self.mode == Mode::Running {
                    self.refresh(cx);
                }
            }
            Msg::Store(entries) => {
                self.store_loading = false;
                self.status = Some((Tone::Info, format!("{} Windows-capable extensions", entries.iter().filter(|s| s.windows).count())));
                self.store = Some(entries);
                if self.mode == Mode::Store {
                    self.refresh(cx);
                }
            }
            Msg::TrayMusic(id) => self.music_tray_act(&id, cx),
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
            Mode::Running => Some("Quit Applications".into()),
            Mode::Tabs => Some("Browser Tabs".into()),
            Mode::Music => Some(self.music_label()),
            Mode::Args => self.args_row.as_ref().map(|it| it.title.clone()),
            Mode::Output => self.output.as_ref().map(|(i, _, _)| self.scripts[*i].title.clone()),
            Mode::Translate | Mode::LanguageSets => Some("Translate".into()),
            Mode::Define => self.define.as_ref().map(|v| format!("Define {}", v.word)),
            Mode::Shots => Some("Search Screenshots".into()),
            Mode::Schedule => Some("My Schedule".into()),
            Mode::Notes(NotesList::All) => Some("Search Notes".into()),
            Mode::Notes(NotesList::Deleted) => Some("Recently Deleted Notes".into()),
            Mode::ScreenTime(p) => Some(match p.range {
                wellbeing::Range::Day => "Screen Time",
                wellbeing::Range::Week => "Screen Time This Week",
                wellbeing::Range::Month => "Screen Time This Month",
            }
            .into()),
            Mode::Form(kind) => Some(kind.bar().1.into()),
            Mode::Ai => Some(self.ai.as_ref().and_then(|a| a.turns.last()).map_or("Ask AI".into(), |t| t.prompt.clone())),
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
            Mode::Running => "Search running apps…".into(),
            Mode::Tabs => "Search open tabs…".into(),
            Mode::Music => self.music_placeholder().into(),
            Mode::Ext => self.extv.as_ref().and_then(|e| e.view.placeholder.clone()).unwrap_or_else(|| "Search…".into()).into(),
            Mode::Args => self.args_row.as_ref().map(|it| it.title.clone()).unwrap_or_default().into(),
            Mode::Output => "Enter copies the output".into(),
            Mode::Define => "Word to define…".into(),
            Mode::Shots => "Filter screenshots by name:, text: or date:…".into(),
            Mode::Schedule => "Search events…".into(),
            Mode::Notes(NotesList::All) => "Search notes…".into(),
            Mode::Notes(NotesList::Deleted) => "Search deleted notes…".into(),
            Mode::ScreenTime(_) => "Search categories, apps and sites…".into(),
            Mode::Form(_) => "".into(),
            Mode::Translate => "Text to translate…".into(),
            Mode::LanguageSets => "Search language sets…".into(),
            Mode::Ai if self.ai.as_ref().is_some_and(|a| !a.turns.is_empty()) => "Ask a follow-up, or Enter copies the answer".into(),
            Mode::Ai => "Ask a question…".into(),
        }
    }

    /// Text on the left of the action bar: a toast, index status, or where you are.
    fn footer_left(&self) -> String {
        if let Some((_, s)) = &self.status {
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
        if self.mode == Mode::Schedule
            && let Some(line) = self.calendar.updated_line(search::now())
        {
            return line;
        }
        if self.mode == Mode::Music && self.music_loading() {
            return "Loading…".into();
        }
        // The emoji and cover grids show no names: the selected cell's is here.
        if matches!(self.mode, Mode::Emoji | Mode::Music) && self.grid() && self.panel.is_none()
            && let Some(it) = self.items.get(self.selected)
        {
            return it.title.clone();
        }
        if let Some(label) = self.label() {
            return label;
        }
        row_footer(self.items.get(self.selected))
    }

    /// The icon on the left of the action bar.
    fn context_icon(&self) -> Icon {
        match self.mode {
            Mode::Ext => self.extv.as_ref().map(|e| self.exts[e.ext].1.clone()).unwrap_or(Icon::Vector(g::PUZZLE)),
            Mode::Root => Icon::Vector(g::SEARCH),
            Mode::Clipboard => Icon::Vector(g::PASTE),
            Mode::Emoji => Icon::Vector(g::EMOJI),
            Mode::Files => Icon::Vector(g::FOLDER),
            Mode::Store => Icon::Vector(g::SHOP),
            Mode::Install => Icon::Vector(g::DOWNLOAD),
            Mode::Running => Icon::Vector(g::CANCEL),
            Mode::Tabs => Icon::Vector(g::GLOBE),
            Mode::Music => Icon::Vector(g::PLAY),
            Mode::Args => self.args_row.as_ref().map_or(Icon::Vector(g::TERMINAL), |it| it.icon.clone()),
            Mode::Output => Icon::Vector(g::TERMINAL),
            Mode::Ai => Icon::Vector(g::SPARKLES),
            Mode::Translate | Mode::LanguageSets => Icon::Vector(g::LANGUAGES),
            Mode::Define => Icon::Vector(g::BOOK_OPEN),
            Mode::Shots => Icon::Vector(g::SCAN_SEARCH),
            Mode::Schedule => Icon::Vector(g::CALENDAR_CLOCK),
            Mode::Notes(NotesList::All) => Icon::Vector(g::NOTEBOOK_TABS),
            Mode::Notes(NotesList::Deleted) => Icon::Vector(g::ARCHIVE_RESTORE),
            Mode::ScreenTime(p) => Icon::Vector(if p.range == wellbeing::Range::Day { g::MONITOR } else { g::CALENDAR_CLOCK }),
            Mode::Form(kind) => Icon::Vector(kind.bar().0),
        }
    }

    /// Esc: close a popover/dialog, pop a pushed view, leave a mode, or hide.
    /// An Emacs or Vim navigation key: acts as `action` when that style is chosen.
    fn nav(&mut self, style: &str, action: Box<dyn gpui::Action>, window: &mut Window, cx: &mut Context<Self>) {
        if self.cfg.launcher.navigation == style {
            window.dispatch_action(action, cx);
        }
    }

    /// Opens `menu` over the window. With a search field, typing goes to `menu_input`.
    fn open_menu(&mut self, menu: menu::Menu, on_pick: impl Fn(&mut Launcher, String, &mut Window, &mut Context<Launcher>) + 'static, window: &mut Window, cx: &mut Context<Self>) {
        let back = window.focused(cx);
        if menu.search.is_some() {
            self.menu_input.update(cx, |s, cx| s.emplace("", cx));
            window.focus(&self.menu_input.focus_handle(cx), cx);
        }
        self.menu = Some(OpenMenu { menu, on_pick: Box::new(on_pick), back, scroll: ScrollHandle::new(), site: None });
        cx.notify();
    }

    /// Hands a key to the open menu; false when none is open.
    fn menu_key(&mut self, key: menu::Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(open) = &mut self.menu else { return false };
        match open.menu.key(key) {
            menu::Event::Picked(i) => self.menu_pick(i, window, cx),
            menu::Event::Closed => _ = self.close_menu(window, cx),
            menu::Event::None => {
                if let Some(k) = open.menu.selected_row() {
                    open.scroll.scroll_to_item(k);
                }
                cx.notify()
            }
        }
        true
    }

    /// Closes the menu and gives the opener the id of entry `i`.
    fn menu_pick(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.close_menu(window, cx) {
            (open.on_pick)(self, open.menu.entries[i].id.clone(), window, cx);
        }
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<OpenMenu> {
        let open = self.menu.take()?;
        if let Some(back) = &open.back {
            window.focus(back, cx);
        }
        cx.notify();
        Some(open)
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel.take().is_some() {
            cx.notify();
            if self.is_form() {
                self.focus_field(window, cx);
            }
        } else if let Some(c) = self.confirm.take() {
            if c.then.is_none() {
                self.reply(c.run, c.id, Ok(json!(false)));
            }
            self.refresh(cx);
        } else if self.quick_look {
            self.close_quick_look(cx);
            cx.notify();
        } else if self.mode == Mode::Ext && self.extv.as_ref().is_some_and(|e| e.depth > 1) {
            let run = self.extv.as_ref().unwrap().run;
            self.send(json!({"t": "pop", "run": run}));
        } else if self.mode == Mode::Music && self.music_back(cx) {
        } else if self.mode == Mode::Root {
            self.hide(true, cx)
        } else if self.cfg.launcher.escape == "close" {
            self.args_row = None;
            self.set_mode(Mode::Root, cx);
            self.hide(true, cx);
            // Popped to root: the next open starts fresh whatever pop_to_root says.
            self.hidden_at = None;
        } else if self.mode == Mode::LanguageSets {
            self.retranslate(window, cx);
        } else if let Mode::Form(FormKind::AddLanguageSet(_)) = self.mode {
            self.set_mode(Mode::LanguageSets, cx);
            window.focus(&self.input.focus_handle(cx), cx);
        } else {
            self.args_row = None;
            self.set_mode(Mode::Root, cx);
            window.focus(&self.input.focus_handle(cx), cx);
        }
    }

    /// Esc while boxes show: from a box back to the query, then clears the boxes. False when
    /// neither applies, for the usual Esc.
    fn dismiss_args(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.panel.is_some() || self.arg_row().is_none() {
            return false;
        }
        if self.arg_focused(window, cx).is_some() {
            window.focus(&self.input.focus_handle(cx), cx);
        } else if self.arg_values(cx).iter().any(|v| !v.is_empty()) {
            self.arg_missing = None;
            self.status = None;
            for a in &self.args {
                a.update(cx, |s, cx| s.emplace("", cx));
            }
        } else {
            return false;
        }
        cx.notify();
        true
    }

    /// `.pc-form--center`: the rows centred under the bar, the selected control focused.
    fn render_form(&self, t: &Tokens, me: gpui::WeakEntity<Self>) -> gpui::AnyElement {
        let Some((fields, selected)) = self.form_fields() else { return div().into_any_element() };
        let rows = fields.into_iter().enumerate().map(|(i, f)| {
            let on = i == selected;
            let control = match &f.ctl {
                Ctl::Text { placeholder, icon, tall, .. } => match &f.input {
                    Some(input) => {
                        let text = text_input(SharedString::from(format!("field-{i}")))
                            .state(input.downgrade())
                            .placeholder(placeholder.clone())
                            .placeholder_color(t.text_faint)
                            .caret_color(t.accent)
                            .w_full()
                            .whitespace_nowrap();
                        design::field_with_icon(on, text, *icon, t).when(*tall, |d| d.h(px(2. * design::CONTROL)).items_start().py(px(design::sp::S8))).into_any_element()
                    }
                    None => div().into_any_element(),
                },
                Ctl::Select(entries, _) => {
                    let title = entries.iter().find(|e| e.id == f.value).map(|e| e.label.clone()).unwrap_or_default();
                    design::field_with_icon(on, title, Some(g::CHEVRON_DOWN), t).into_any_element()
                }
                Ctl::Check => {
                    let checked = f.value == "true";
                    div()
                        .h(px(design::CONTROL))
                        .flex()
                        .items_center()
                        .gap(px(design::sp::S8))
                        .child(design::icon(if checked { g::CHECKBOX_ON } else { g::CHECKBOX }, design::ic::LG, if checked { t.accent } else { t.text_muted }))
                        .child(div().text_color(if on { t.text } else { t.text_muted }).child(f.label.clone()))
                        .into_any_element()
                }
                // Each token's × removes it.
                Ctl::Tokens => {
                    let tokens = focus::Picks::of(&f.value).0.into_iter().map(|id| {
                        let (glyph, name) = focus::pick_look(&id);
                        let me = me.clone();
                        let x = div().id(SharedString::from(format!("token-{id}"))).flex().child(design::token_x(t)).on_click(move |_, _, cx| {
                            me.update(cx, |this, cx| {
                                this.edit_picks(i, |p| p.remove(&id));
                                cx.notify();
                            })
                            .ok();
                        });
                        design::token(glyph, &name, x, t).into_any_element()
                    });
                    design::token_field(on, tokens, t).into_any_element()
                }
                // A click picks a swatch.
                Ctl::Swatches => div()
                    .h(px(design::CONTROL))
                    .flex()
                    .items_center()
                    .gap(px(design::sp::S8))
                    .children(calendar::COLOURS.iter().map(|&(name, tint)| {
                        let me = me.clone();
                        design::swatch(tint, f.value == name, t).id(SharedString::from(format!("swatch-{name}"))).on_click(move |_, _, cx| {
                            me.update(cx, |this, cx| {
                                if let Some(f) = this.form.as_mut().and_then(|f| f.fields.get_mut(i)) {
                                    f.value = name.into();
                                }
                                cx.notify();
                            })
                            .ok();
                        })
                    }))
                    .into_any_element(),
                Ctl::Note if f.label.is_empty() => return design::form_note(f.value.clone(), t).into_any_element(),
                Ctl::Note => div().text_size(px(design::ts::BODY_SM)).text_color(t.text_muted).child(f.value.clone()).into_any_element(),
                Ctl::Rule => div().h(px(1.)).w_full().bg(t.hairline).into_any_element(),
            };
            // A menu opens under the selected control.
            let at = self.form_at.clone();
            let control = div().id(("form-ctl", i)).flex().flex_col().when(on, |d| d.on_prepaint(move |p, _, _| at.set(p.bounds.bottom_left() + gpui::point(px(0.), px(design::sp::S4))))).child(control);
            let label = if matches!(f.ctl, Ctl::Check | Ctl::Rule) { SharedString::default() } else { f.label.clone() };
            let error = f.error.clone().map(|e| div().text_size(px(design::ts::SMALL)).text_color(t.danger).child(e).into_any_element());
            design::form_row(label, std::iter::once(control.into_any_element()).chain(error), t).into_any_element()
        });
        div()
            .id("form")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(design::sp::S12))
            .pt(px(design::sp::S24))
            .children(rows)
            .into_any_element()
    }

    fn render_markdown(md: &str, metadata: &[(String, String)], t: &Tokens, id: &'static str, scroll: Option<&ScrollHandle>) -> gpui::AnyElement {
        div()
            .id(id)
            .when_some(scroll, |d, h| d.track_scroll(h))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(24.))
            .py(px(16.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .children(ext::markdown_lines(md).into_iter().map(|(level, text)| {
                let line = div().line_height(px(21.)).child(if text.is_empty() { " ".to_string() } else { text });
                match level {
                    1 => line.pt(px(4.)).line_height(px(28.)).font_family(design::FONT_DISPLAY).text_size(px(design::ts::DISPLAY)).font_weight(FontWeight::SEMIBOLD),
                    2 => line.pt(px(4.)).text_size(px(design::ts::HEADING)).font_weight(FontWeight::SEMIBOLD),
                    3 => line.pt(px(4.)).font_weight(FontWeight::SEMIBOLD),
                    9 => line.font_family(design::FONT_MONO).text_size(px(design::ts::BODY_SM)).px(px(12.)).bg(t.surface).rounded(px(design::r::XS)),
                    _ => line.text_color(t.text),
                }
            }))
            .when(!metadata.is_empty(), |d| d.children(info_rows(metadata, t)))
            .into_any_element()
    }

    /// Clipboard History's right-hand pane: the whole entry, then where and when it came from.
    fn clip_detail(&mut self, t: &Tokens) -> gpui::AnyElement {
        let key = match self.items.get(self.selected).map(|it| &it.act) {
            Some(Act::Clip(k)) => k.clone(),
            _ => return div().flex_1().into_any_element(),
        };
        let Some(e) = self.history.get(&key).cloned() else { return div().flex_1().into_any_element() };
        let lines = detail_lines;
        let mut meta: Vec<(String, String)> = vec![];
        let top = match &e.kind {
            clipboard::Kind::Text => {
                let (chars, words) = (e.text.chars().count(), e.text.split_whitespace().count());
                meta.push(("Content".into(), format!("{chars} characters · {words} words")));
                lines(&e.text, e.text.contains(['{', ';']) && e.text.lines().count() > 1).into_any_element()
            }
            clipboard::Kind::Image { id, w, h, bytes } => {
                meta.push(("Dimensions".into(), format!("{w} × {h}")));
                meta.push(("Size".into(), format!("{:.1} KB", *bytes as f64 / 1024.)));
                let preview = self.clip_preview(id);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(image_box(t).children(preview.map(|p| gpui::img(p).max_w_full().max_h(px(196.)).object_fit(gpui::ObjectFit::Contain))))
                    .when(!e.text.is_empty(), |d| d.child(div().text_color(t.text_muted).child(lines(&e.text, false))))
                    .into_any_element()
            }
            clipboard::Kind::Files(files) => {
                meta.push(("Files".into(), files.len().to_string()));
                lines(&e.text, false).into_any_element()
            }
        };
        meta.insert(0, ("Application".into(), if e.app.is_empty() { "Unknown".into() } else { e.app.clone() }));
        meta.push(("Copied".into(), format!("{} · {}", win::local_time_of(e.at), timers::ago(search::now().saturating_sub(e.at)))));
        if e.pinned {
            meta.push(("Pinned".into(), "Yes".into()));
        }
        div()
            .id("clip-detail")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(16.))
            .py(px(12.))
            .flex()
            .flex_col()
            .child(top)
            .children(info_rows(&meta, t))
            .into_any_element()
    }

    /// The translator's right-hand pane: the selected translation, then under a hairline its
    /// source and target languages as tags.
    fn translate_detail(&self, t: &Tokens) -> gpui::AnyElement {
        use design::sp;
        let Some(tr) = &self.translation else { return div().flex_1().into_any_element() };
        // The pronunciation row (last, past the translations) shows with the first one's languages.
        let (text, row) = match tr.rows.get(self.selected) {
            Some(r) => (r.text.clone(), r),
            None => (self.items.get(self.selected).map(|it| it.title.clone()).unwrap_or_default(), &tr.rows[0]),
        };
        let meta = |key: &str, tags: Vec<gpui::Div>| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(sp::S8))
                .child(div().text_size(px(design::ts::BODY_SM)).text_color(t.text_muted).child(key.to_string()))
                .child(div().flex().gap(px(sp::S4)).children(tags))
        };
        let mut source = vec![];
        if tr.job.from == "auto" {
            source.push(design::tag("Auto-Detect", design::Tint::System, t));
        }
        source.push(design::tag(&translate::language_name(&tr.detected()), design::Tint::Web, t));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().id("translation").flex_1().min_h_0().overflow_y_scroll().p(px(sp::S16)).text_color(t.text).child(text))
            .child(
                div()
                    .flex_none()
                    .px(px(sp::S16))
                    .py(px(sp::S12))
                    .flex()
                    .flex_col()
                    .gap(px(sp::S8))
                    .border_t_1()
                    .border_color(t.hairline)
                    .child(meta("Source Language", source))
                    .child(meta("Target Language", vec![design::tag(&translate::language_name(&row.to), design::Tint::Paste, t)])),
            )
            .into_any_element()
    }

    /// The define view's right-hand pane: the word, "noun · 1 sense · WordNet", the selected
    /// sense's gloss, its examples and its synonyms; for a Did you mean row, that word and its
    /// part of speech.
    fn define_detail(&self, t: &Tokens) -> gpui::AnyElement {
        use design::{sp, ts};
        let pane = div().id("define").flex_1().min_h_0().overflow_y_scroll().p(px(sp::S16)).flex().flex_col().gap(px(sp::S8));
        let Some((v, Some(Ok(Some(a))))) = self.define.as_ref().map(|v| (v, v.answer.as_ref())) else { return pane.into_any_element() };
        let big = |w: &str| div().font_family(design::FONT_DISPLAY).text_size(px(ts::DISPLAY)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(t.text).child(w.to_string());
        let meta = |m: String| div().font_family(design::FONT_SMALL).text_size(px(ts::SMALL)).text_color(t.text_faint).child(m);
        let muted = |m: String| div().text_size(px(ts::BODY)).text_color(t.text_muted).child(m);
        let groups = dict::group(&a.senses);
        match groups.iter().flat_map(|(_, s)| s).nth(self.selected) {
            Some(s) => {
                let n = groups.iter().find(|(p, _)| *p == s.pos).map_or(1, |(_, g)| g.len());
                pane.child(big(&v.word))
                    .child(meta(dict::meta_line(s.pos, n)))
                    .child(muted(s.gloss.clone()))
                    .children(s.examples.iter().map(|e| muted(format!("\"{e}\""))))
                    .children(dict::similar_line(&s.synonyms).map(muted))
            }
            None => match a.maybe.get(self.selected.saturating_sub(a.senses.len())) {
                Some((w, pos)) => pane.child(big(w)).child(meta(pos.name().to_lowercase())),
                None => pane,
            },
        }
        .into_any_element()
    }

    /// File Search's right-hand pane: a preview, then the file's information.
    fn file_detail(&mut self, t: &Tokens) -> gpui::AnyElement {
        let path = match self.items.get(self.selected).map(|it| &it.act) {
            Some(Act::File(p)) => p.clone(),
            _ => return div().flex_1().into_any_element(),
        };
        if self.file_info.as_ref().is_none_or(|f| f.0 != path) {
            let info = files::file_info(&path, &std::env::var("USERPROFILE").unwrap_or_default());
            self.file_info = Some((path.clone(), info));
            self.file_seq.fetch_add(1, Ordering::Relaxed);
        }
        let pane = div().id("file-detail").flex_1().min_h_0().overflow_y_scroll().px(px(16.)).py(px(12.)).flex().flex_col();
        let Some(info) = self.file_info.as_ref().and_then(|f| f.1.as_ref()) else {
            return pane.child(div().text_size(px(design::ts::BODY_SM)).text_color(t.text_muted).child("Not found")).into_any_element();
        };
        let (kind, size, modified, created, folder) = (info.kind, info.size, info.modified, info.created, info.folder.clone());
        let (text, count, image) = match &info.preview {
            files::Preview::Text(s) => (Some(s.clone()), None, false),
            files::Preview::Folder(n) => (None, Some(*n), false),
            files::Preview::Image(_) => (None, None, true),
            files::Preview::Icon => (None, None, false),
        };
        let picture = if image { self.file_preview(format!("p:{path}")) } else { None };
        let top = if let Some(s) = text {
            detail_lines(&s, true).into_any_element()
        } else if let Some(n) = count {
            detail_lines(&if n >= 10_000 { "10000+ items".to_string() } else if n == 1 { "1 item".to_string() } else { format!("{n} items") }, false).into_any_element()
        } else {
            let icon = if picture.is_none() { self.file_preview(format!("i:{path}")) } else { None };
            let shown = picture.map(|p| gpui::img(p).max_w_full().max_h(px(196.)).object_fit(gpui::ObjectFit::Contain)).or_else(|| icon.map(|i| gpui::img(i).size(px(design::ic::XL))));
            image_box(t).children(shown).into_any_element()
        };
        let when = |t: Option<SystemTime>| t.map(|t| win::local_time_of(t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs()));
        let mut meta: Vec<(String, String)> = vec![("Kind".into(), kind.into())];
        meta.extend(size.map(|s| ("Size".to_string(), files::size_text(s))));
        meta.extend(when(modified).map(|w| ("Modified".to_string(), w)));
        meta.extend(when(created).map(|w| ("Created".to_string(), w)));
        meta.push(("Where".into(), folder));
        pane.child(top).children(info_rows(&meta, t)).into_any_element()
    }

    /// The grids: moves the selection by `dx` cells or `dy` grid rows.
    fn grid_move(&mut self, dx: isize, dy: isize, cx: &mut Context<Self>) {
        let rows = grid_rows(&self.items, self.grid_cols());
        let cells: Vec<&Vec<usize>> = rows.iter().filter_map(|r| if let GridRow::Cells(c) = r { Some(c) } else { None }).collect();
        let Some((r, c)) = cells.iter().enumerate().find_map(|(r, row)| row.iter().position(|&i| i == self.selected).map(|c| (r, c))) else { return };
        self.selected = if dy != 0 {
            let r = (r as isize + dy).clamp(0, cells.len() as isize - 1) as usize;
            cells[r][c.min(cells[r].len() - 1)]
        } else {
            (self.selected as isize + dx).clamp(0, self.items.len() as isize - 1) as usize
        };
        self.scroll_to_selected();
        cx.notify();
    }

    /// The emoji picker, Search Screenshots and YouTube Music's albums show a grid (each falls back to the list for
    /// its empty state, and for a confirm's two rows).
    fn grid(&self) -> bool {
        (matches!(self.mode, Mode::Emoji | Mode::Shots) || self.music_grid()) && self.confirm.is_none() && self.items.first().is_some_and(|it| it.section.as_deref() != Some(EMPTY))
    }

    /// Where Left and Right move through a grid, and where Tab cycles YouTube Music's filters.
    fn grid_keys(&self) -> Option<&'static str> {
        if self.panel.is_some() || self.menu.is_some() {
            return None;
        }
        match (self.grid(), self.music_tabs()) {
            (true, true) => Some("EmojiGrid Music"),
            (true, false) => Some("EmojiGrid"),
            (false, true) => Some("Music"),
            (false, false) => None,
        }
    }

    /// The open ActionPanel's actions, each with its place among all of them; with a search
    /// field (`panel_filtered`), only those whose label holds what is typed in it.
    fn panel_rows(&self, cx: &App) -> Vec<(usize, (String, Act))> {
        let Some((item, _)) = &self.panel else { return vec![] };
        let typed = if self.panel_filtered() { self.menu_input.read(cx).as_str().to_string() } else { String::new() };
        search::panel_filter(item.actions(), &typed)
    }

    /// The open ActionPanel has a search field: more than `PANEL_FILTER_AT` actions. Typing
    /// goes to it (`menu_input`) while it is open.
    fn panel_filtered(&self) -> bool {
        self.panel.as_ref().is_some_and(|(item, _)| item.actions().len() > search::PANEL_FILTER_AT)
    }

    /// An action's own shortcut (`search::act_keys`, "Ctrl+O"): runs that action of the open
    /// panel's row, or else of the selected row, without the panel. Otherwise the keys go on.
    fn shortcut(&mut self, keys: &str, window: &mut Window, cx: &mut Context<Self>) {
        let item = match &self.panel {
            Some((item, _)) => Some(item.clone()),
            None => self.items.get(self.selected).cloned(),
        };
        let act = item.as_ref().and_then(|it| it.actions().into_iter().map(|a| a.1).find(|a| search::act_keys(a) == Some(keys)));
        match (item, act) {
            (Some(item), Some(act)) if self.menu.is_none() => {
                self.panel = None;
                self.run(act, item.key, window, cx)
            }
            _ => cx.propagate(),
        }
    }

    /// Quick Look (Ctrl+Y): the selected screenshot large, contain-fit, in place of the grid,
    /// with its name (weight 600) and "2.4 MB · Today, 14:59 · Screenshots" under it.
    fn quick_look_view(&mut self, t: &Tokens, cx: &mut Context<Self>) -> gpui::AnyElement {
        let path = self.items.get(self.selected).and_then(|it| it.extra.iter().find_map(|(_, a)| if let Act::QuickLook(p) = a { Some(p.clone()) } else { None })).unwrap_or_default();
        let image = self.shot_thumbs.get(self.selected).cloned().flatten().and_then(|key| self.quick_look_image(&key.replacen("t:", "q:", 1), cx));
        // A stat once per shot shown, not per redraw.
        if self.quick_look_detail.0 != path {
            let line = shots::detail(Path::new(&path), SystemTime::now()).unwrap_or_default();
            self.quick_look_detail = (path.clone(), line);
        }
        let meta = self.quick_look_detail.1.clone();
        let path = Path::new(&path);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let picture = match image {
            Some(image) => gpui::img(image).size_full().object_fit(gpui::ObjectFit::Contain).into_any_element(),
            None => design::icon(if shots::picture(path) { g::PICTURE } else { g::VIDEO }, design::ic::XL, t.text_faint).into_any_element(),
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .px(px(design::sp::S16))
            .py(px(design::sp::S12))
            .gap(px(design::sp::S4))
            .child(div().flex_1().min_h_0().pb(px(design::sp::S8)).flex().items_center().justify_center().child(picture))
            .child(div().truncate().font_weight(gpui::FontWeight::SEMIBOLD).child(name))
            .child(div().truncate().text_size(px(design::ts::BODY_SM)).text_color(t.text_muted).child(meta))
            .into_any_element()
    }

    /// Cells per grid row: `[screenshots] columns` (3 to 6), `[ytmusic] columns` (3 to 6) or `[emoji] columns` (4 to 16).
    fn grid_cols(&self) -> usize {
        match self.mode {
            Mode::Shots => self.cfg.screenshots.columns.clamp(3, 6),
            Mode::Music => self.cfg.ytmusic.columns.clamp(3, 6),
            _ => self.cfg.emoji.columns.clamp(4, 16),
        }
    }

    fn grid_view(&self, t: &Tokens, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.mode == Mode::Shots {
            return self.shot_grid(t, cx);
        }
        if self.mode == Mode::Music {
            return self.cover_grid(t, cx);
        }
        let cols = self.grid_cols();
        let me = cx.weak_entity();
        let (selected, t) = (self.selected, *t);
        let glyphs: Rc<Vec<SharedString>> = Rc::new(self.items.iter().map(|it| if let Icon::Glyph(s) = &it.icon { s.clone() } else { SharedString::default() }).collect());
        let rows = Rc::new(grid_rows(&self.items, cols));
        // ponytail: rows are laid out only while on screen; 400 emoji cells per keystroke made typing lag.
        let list = gpui::list(self.grid_list.clone(), move |k, _, _| match &rows[k] {
                GridRow::Header(s) => design::section_header(s, None, &t).into_any_element(),
                GridRow::Cells(cells) => {
                    let pad = cols - cells.len();
                    div()
                        .flex()
                        .w_full()
                        .gap(px(4.))
                        .children(cells.iter().map(|&i| {
                            let on = i == selected;
                            let me = me.clone();
                            let glyph = glyphs[i].clone();
                            let t = &t;
                            let hover = t.hover;
                            div()
                                .id(("emoji", i))
                                .flex_1()
                                .h(px(EMOJI_CELL))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(design::r::MD))
                                .map(|d| if on { design::lozenge(d, t) } else { d.hover(move |s| s.bg(hover)) })
                                .font_family(design::FONT_EMOJI)
                                .text_size(px(30.))
                                .child(glyph)
                                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                                    me.update(cx, |this, cx| {
                                        this.selected = i;
                                        this.launch(window, cx);
                                    })
                                    .ok();
                                })
                                .into_any_element()
                        }))
                        .children((0..pad).map(|_| div().flex_1().into_any_element()))
                        .into_any_element()
                }
            });
        div().flex_1().min_h_0().flex().flex_col().px(px(8.)).py(px(LIST_PAD)).child(list.flex_1().min_h_0()).into_any_element()
    }

    /// YouTube Music's albums and playlists: `.pc-thumb` covers, square, `[ytmusic] columns`
    /// across, laid out like Search Screenshots' grid.
    fn cover_grid(&self, t: &Tokens, cx: &mut Context<Self>) -> gpui::AnyElement {
        let cols = self.grid_cols();
        let me = cx.weak_entity();
        let (selected, t) = (self.selected, *t);
        let cells: Rc<Vec<Item>> = Rc::new(self.items.clone());
        let rows = Rc::new(grid_rows(&self.items, cols));
        let last = rows.len().saturating_sub(1);
        let list = gpui::list(self.grid_list.clone(), move |k, _, _| match &rows[k] {
            GridRow::Header(s) => design::section_header(s, None, &t).into_any_element(),
            GridRow::Cells(row) => div()
                .flex()
                .w_full()
                .px(px(design::sp::S16))
                .pb(px(if k == last { design::sp::S8 } else { design::sp::S12 }))
                .gap(px(design::sp::S12))
                .children(row.iter().map(|&i| {
                    let it = &cells[i];
                    let (image, glyph) = match &it.icon {
                        Icon::Image(image) => (Some(image.clone()), g::AUDIO),
                        Icon::Vector(glyph) => (None, *glyph),
                        _ => (None, g::AUDIO),
                    };
                    let me = me.clone();
                    design::cover(image, glyph, &it.title, &it.subtitle, i == selected, &t)
                        .id(("cover", i))
                        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                            me.update(cx, |this, cx| {
                                this.selected = i;
                                this.launch(window, cx);
                            })
                            .ok();
                        })
                        .into_any_element()
                }))
                .children((row.len()..cols).map(|_| div().flex_1().into_any_element()))
                .into_any_element(),
        });
        div().flex_1().min_h_0().flex().flex_col().child(list.flex_1().min_h_0()).into_any_element()
    }

    /// Search Screenshots' grid (`.pc-grid`): the section with its count, then the thumbnails
    /// `grid_cols` across, each decoded off the UI thread as its row comes on screen.
    fn shot_grid(&self, t: &Tokens, cx: &mut Context<Self>) -> gpui::AnyElement {
        let cols = self.grid_cols();
        let me = cx.weak_entity();
        let (selected, t, count) = (self.selected, *t, self.items.len());
        let cells: Rc<Vec<(Option<String>, String)>> = Rc::new(self.items.iter().enumerate().map(|(i, it)| (self.shot_thumbs.get(i).cloned().flatten(), it.accessory.clone())).collect());
        let rows = Rc::new(grid_rows(&self.items, cols));
        self.grid_top.store(self.grid_list.logical_scroll_top().item_ix, Ordering::Relaxed);
        let last = rows.len().saturating_sub(1);
        let list = gpui::list(self.grid_list.clone(), move |k, _, cx| match &rows[k] {
            GridRow::Header(s) => design::section_header(s, Some(count), &t).into_any_element(),
            GridRow::Cells(row) => div()
                .flex()
                .w_full()
                .px(px(design::sp::S16))
                .pb(px(if k == last { design::sp::S8 } else { design::sp::S12 }))
                .gap(px(design::sp::S12))
                .children(row.iter().map(|&i| {
                    let (key, caption) = &cells[i];
                    let image = key.as_ref().and_then(|key| me.update(cx, |this, cx| this.shot_thumb(key, k, cx)).ok().flatten());
                    let me = me.clone();
                    design::thumb(image, caption, i == selected, &t)
                        .id(("shot", i))
                        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                            me.update(cx, |this, cx| {
                                this.selected = i;
                                this.launch(window, cx);
                            })
                            .ok();
                        })
                        .into_any_element()
                }))
                .children((row.len()..cols).map(|_| div().flex_1().into_any_element()))
                .into_any_element(),
        });
        div().flex_1().min_h_0().flex().flex_col().child(list.flex_1().min_h_0()).into_any_element()
    }

    /// Search Screenshots' dropdown: All, Screenshots, Recordings, Pinned.
    fn open_shot_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = shots::Kind::ALL.iter().map(|k| menu::Entry { id: k.label().into(), label: k.label().into(), ..Default::default() }).collect();
        let mut m = menu::Menu::new(entries, None, menu::Place::Right);
        m.selected = shots::Kind::ALL.iter().position(|k| *k == self.shot_kind).unwrap_or(0);
        let pick = |this: &mut Launcher, id: String, _: &mut Window, cx: &mut Context<Launcher>| {
            this.shot_kind = shots::Kind::ALL.into_iter().find(|k| k.label() == id).unwrap_or_default();
            this.refresh(cx);
        };
        self.open_menu(m, pick, window, cx);
    }
}

/// The width a screenshot thumbnail is decoded at.
const THUMB_PX: u32 = 320;
/// Quick Look's picture is decoded to fit inside this many pixels each way.
const QUICK_LOOK_PX: u32 = 1280;

/// The picture a thumbnail or Quick Look key (`t:`/`q:{mtime}|{path}`) names, if it may be
/// read: no bigger than `shots::MAX_BYTES`.
fn shot_file(key: &str) -> Option<&Path> {
    key.split_once('|').map(|(_, p)| Path::new(p)).filter(|p| shots::picture(p) && std::fs::metadata(p).is_ok_and(|m| m.len() <= shots::MAX_BYTES))
}

/// A thumbnail decode is dropped when its row is further than this from the grid's top row.
const NEAR_ROWS: usize = 12;
/// `file_images` past `FILE_IMAGES_MAX` entries is cut to the `FILE_IMAGES_KEEP` used last.
const FILE_IMAGES_MAX: usize = 300;
const FILE_IMAGES_KEEP: usize = 200;

/// Whether grid row `row` is near enough to `top` (the row at the top of the view) to decode.
fn near(row: usize, top: usize) -> bool {
    row + NEAR_ROWS >= top && row <= top + NEAR_ROWS
}

/// Past `max` entries, forgets the least recently used (by tick) down to `keep`; returns the
/// images among them, for `release`.
fn evict<V>(cache: &mut HashMap<String, (Option<V>, u64)>, max: usize, keep: usize) -> Vec<V> {
    if cache.len() <= max {
        return vec![];
    }
    let mut old: Vec<(u64, String)> = cache.iter().map(|(k, v)| (v.1, k.clone())).collect();
    old.sort_unstable();
    old.truncate(cache.len() - keep);
    old.into_iter().filter_map(|(_, k)| cache.remove(&k)?.0).collect()
}

/// `cache[key]`, made by `make` the first time, marked as just used for `evict`.
fn used<V: Clone>(cache: &mut HashMap<String, (Option<V>, u64)>, tick: &mut u64, key: String, make: impl FnOnce() -> Option<V>) -> Option<V> {
    *tick += 1;
    let entry = cache.entry(key).or_insert_with(|| (make(), 0));
    entry.1 = *tick;
    entry.0.clone()
}

/// Frees images' GPU textures (gpui keeps an image's atlas entry until `drop_image`). Deferred
/// until no window is being drawn, so every window lets go of them.
fn release(images: Vec<Arc<RenderImage>>, cx: &mut App) {
    if !images.is_empty() {
        cx.defer(move |cx| images.into_iter().for_each(|i| cx.drop_image(i, None)));
    }
}

/// What Search Screenshots says with nothing to show.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ShotsEmpty {
    Reading,
    NoMatches,
    /// The only one whose Enter is Add Folder.
    NoScreenshots,
}

impl ShotsEmpty {
    fn title(self) -> &'static str {
        match self {
            ShotsEmpty::Reading => "Reading screenshots…",
            ShotsEmpty::NoMatches => "No matches",
            ShotsEmpty::NoScreenshots => "No screenshots found",
        }
    }
}

/// `filtered`: a query, or a dropdown choice other than All, may be what hides them.
fn shots_empty(busy: bool, filtered: bool) -> ShotsEmpty {
    match (busy, filtered) {
        (true, _) => ShotsEmpty::Reading,
        (_, true) => ShotsEmpty::NoMatches,
        _ => ShotsEmpty::NoScreenshots,
    }
}

/// An emoji cell's height.
const EMOJI_CELL: f32 = 56.;

enum GridRow {
    Header(String),
    /// Item indices, at most `columns`.
    Cells(Vec<usize>),
}

/// A grid's layout: each section's header, then its cells in rows of `cols`.
fn grid_rows(items: &[Item], cols: usize) -> Vec<GridRow> {
    let cols = cols.max(1);
    let mut out = vec![];
    let mut last: Option<&str> = None;
    for (i, it) in items.iter().enumerate() {
        let sec = it.section.as_deref();
        let new_sec = i == 0 || sec != last;
        if new_sec && let Some(s) = sec {
            out.push(GridRow::Header(s.to_string()));
        }
        match out.last_mut() {
            Some(GridRow::Cells(c)) if !new_sec && c.len() < cols => c.push(i),
            _ => out.push(GridRow::Cells(vec![i])),
        }
        last = sec;
    }
    out
}

/// A detail pane's text, line by line; mono for code.
fn detail_lines(text: &str, mono: bool) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .when(mono, |d| d.font_family(design::FONT_MONO))
        .text_size(px(design::ts::BODY_SM))
        .line_height(px(20.))
        .children(text.lines().take(300).map(|l| div().child(if l.is_empty() { " ".to_string() } else { l.chars().take(2000).collect() })))
}

/// The framed box a detail pane's picture sits in.
fn image_box(t: &Tokens) -> gpui::Div {
    div().h(px(200.)).flex().items_center().justify_center().rounded(px(design::r::MD)).bg(t.surface).border_1().border_color(t.hairline).overflow_hidden()
}

/// "Information" and key / value rows under a detail view.
fn info_rows(metadata: &[(String, String)], t: &Tokens) -> Vec<gpui::AnyElement> {
    let head = div().mt(px(12.)).pt(px(12.)).border_t_1().border_color(t.hairline).font_family(design::FONT_SMALL).text_size(px(design::ts::SMALL)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_faint).child("Information");
    std::iter::once(head.into_any_element())
        .chain(metadata.iter().map(|(k, v)| {
            div()
                .flex()
                .justify_between()
                .gap(px(16.))
                .py(px(8.))
                .border_b_1()
                .border_color(t.hairline)
                .text_size(px(design::ts::BODY_SM))
                .child(div().flex_none().text_color(t.text_muted).child(k.clone()))
                .child(div().min_w_0().truncate().child(v.clone()))
                .into_any_element()
        }))
        .collect()
}

/// Icon for an action in the Ctrl+K panel.
fn act_glyph(act: &Act) -> &'static str {
    match act {
        Act::Favorite(..) => g::STAR,
        Act::Hide(_) => g::CANCEL,
        Act::Copy(_) | Act::CopyClip(_) | Act::CopyShot(_) | Act::CopyText(_) => g::COPY,
        Act::PinClip(..) | Act::PinShot(..) => g::PIN,
        Act::Paste(_) | Act::Clip(_) | Act::Snippet(_) | Act::PasteShot(_) => g::PASTE,
        Act::Open(_) => g::GLOBE,
        // g::PDF is the `file` icon.
        Act::File(_) => g::PDF,
        Act::OpenWith(_) => g::APP,
        Act::QuickLook(_) => g::EYE,
        Act::Reveal(_) => g::FOLDER_SEARCH,
        Act::RunAsAdmin(_) => g::SHIELD,
        Act::DeleteClip(_) | Act::UninstallExt(_) | Act::DeleteShot(_) => g::DELETE,
        Act::Install(_) | Act::DownloadDictionary(_) => g::DOWNLOAD,
        Act::Define(_) | Act::SimilarWords(_) => g::BOOK_OPEN,
        Act::MoveToCategory(_) => g::LIST_CHECKS,
        Act::ExportScreenTime => g::DOWNLOAD,
        Act::EnableScreenTime => g::CHECK,
        Act::Run(_) | Act::Script(_) | Act::Cmd(_) | Act::Layout(_) | Act::ExtRun(..) => g::PLAY,
        Act::ExtSet(..) | Act::FormSet(..) => g::FILTER,
        Act::Confirm(true) => g::CHECK,
        Act::Confirm(false) => g::CANCEL,
        Act::Translate(_) | Act::LangMenu => g::LANGUAGES,
        Act::UseLangSet(_) => g::CHECK,
        Act::EditLangSet(_) => g::EDIT,
        Act::AddCalendar | Act::AddShotFolder => g::PLUS,
        Act::DeleteLangSet(_) => g::DELETE,
        _ => g::PLAY,
    }
}

/// Test seam: with PROCAST_E2E=1, each render writes what's on screen to
/// %TEMP%\<file> so spikes/e2e.py can assert on it.
fn dump_ui(file: &str, state: Value) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("PROCAST_E2E").is_some()) {
        let _ = std::fs::write(std::env::temp_dir().join(file), state.to_string());
    }
}

/// %APPDATA%\procast\extensions.log: extension errors and console output.
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
    Row { idx: usize, title: String, subtitle: String, accessory: String, hotkey: String, icon: Icon, tint: Option<design::Tint>, live: bool, now: bool, toggle: Option<bool> },
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        design::scale_launcher();
        self.music_release(window);
        // Rem-based spacing follows the scale too.
        window.set_rem_size(px(16.));
        let t = Tokens::from_theme(&self.cfg.theme, window);
        // A menu closed without `close_menu` (a mode change, an extension exiting) would
        // strand focus on its no longer drawn field, where no key binding matches.
        // The ActionPanel's search field is the menu's while the panel is open.
        if self.menu.is_none() && !self.panel_filtered() && self.menu_input.focus_handle(cx).is_focused(window) {
            if self.is_form() {
                self.focus_field(window, cx);
            } else {
                window.focus(&self.input.focus_handle(cx), cx);
            }
        }
        self.guard_form_focus(window, cx);
        // The same for a box whose row is no longer selected.
        let shown = self.arg_row().map_or(0, |it| it.args.len());
        if self.args[shown..].iter().any(|a| a.focus_handle(cx).is_focused(window)) {
            window.focus(&self.input.focus_handle(cx), cx);
        }
        let notices: Vec<String> = self.notice.iter().chain(self.hotkey_errors.iter().flatten()).cloned().collect();
        let empty = (self.items.len() == 1 && self.items[0].section.as_deref() == Some(EMPTY)).then(|| self.items[0].clone());
        // Views with their own body: the emoji grid, and the detail panes of clipboard history and file search.
        let grid = self.grid().then(|| if self.quick_look { self.quick_look_view(&t, cx) } else { self.grid_view(&t, cx) });
        // The translator's previous answers, dimmed while the next ones are on their way.
        let lt = if self.mode == Mode::Translate && self.translation.as_ref().is_some_and(|tr| tr.busy) { Tokens { text: t.text_faint, text_muted: t.text_faint, ..t } } else { t };
        let side = match self.mode {
            Mode::Clipboard if empty.is_none() => Some(self.clip_detail(&t)),
            Mode::Files if empty.is_none() => Some(self.file_detail(&t)),
            Mode::Translate if empty.is_none() && self.translation.as_ref().is_some_and(|tr| !tr.rows.is_empty()) => Some(self.translate_detail(&lt)),
            Mode::Define if empty.is_none() && !self.items.is_empty() => Some(self.define_detail(&t)),
            Mode::ScreenTime(_) if empty.is_none() && matches!(self.st_stats, Some(Ok(_))) => Some(self.screen_time_detail(&t)),
            _ => None,
        };
        let selected_item = self.items.get(self.selected);
        let actions = selected_item.map(|it| it.actions()).unwrap_or_default();
        let primary = actions.first().map(|a| a.0.clone()).filter(|l| !l.is_empty());
        let bar_second = selected_item.and_then(|it| it.bar_second.clone()).filter(|_| actions.len() > 1);
        let arg_row = self.arg_row().cloned();
        let arg_values = self.arg_values(cx);
        let arg_focus = self.arg_focused(window, cx);
        // While boxes show: "Next Box" from the query, then the command's verb ("Start").
        let arg_keys = arg_row.as_ref().filter(|_| self.panel.is_none() && self.armed != Some(self.selected)).map(|it| (arg_focus.is_none(), it.act.label().split(' ').next().unwrap_or_default().to_string()));

        // Footer right side, also dumped for tests.
        let footer_action = if let Some((next, verb)) = &arg_keys {
            format!("{}{verb}  ↵", if *next { "Next Box  Tab    " } else { "" })
        } else {
            match (&self.panel, selected_item) {
            (Some(_), _) => "Run  ↵    Close  Esc".to_string(),
            _ if let Mode::Form(kind) = self.mode => format!("{}  Ctrl+↵", kind.bar().2),
            _ if self.is_form() => "Submit  Ctrl+↵    Actions  Ctrl+K".to_string(),
            (None, Some(it)) if self.armed == Some(self.selected) => match self.join_target(&it.act) {
                Some((title, url)) => format!("Join {title} on {}? Press ↵ again", calendar::link_host(&url).unwrap_or_default()),
                None => format!("Press ↵ again to {}", it.title),
            },
            (None, Some(_)) => match &primary {
                Some(p) if actions.len() > 1 => format!("{p}  ↵    {}Actions  Ctrl+K", bar_second.as_ref().map(|s| format!("{s}  Ctrl+↵    ")).unwrap_or_default()),
                Some(p) => format!("{p}  ↵"),
                None => String::new(),
            },
            (None, None) => String::new(),
            }
        };
        let view = self.extv.as_ref().filter(|_| self.mode == Mode::Ext && self.panel.is_none() && self.confirm.is_none()).map(|e| e.view.clone());
        let panel_rows = self.panel_rows(cx);
        dump_ui(
            "procast-ui.json",
            json!({
                "visible": self.visible,
                "bg": format!("{:?}", t.bg),
                "query": self.query(cx),
                "label": self.label(),
                "rows": match &self.panel {
                    Some(_) => panel_rows.iter().map(|(_, a)| a.0.clone()).collect::<Vec<_>>(),
                    None => self.items.iter().filter(|it| it.section.as_deref() != Some(EMPTY)).map(|it| it.title.clone()).collect(),
                },
                "selected": self.panel.as_ref().map_or(self.selected, |p| p.1),
                "panel": self.panel.is_some(),
                "menu": self.menu.as_ref().map(|o| o.menu.visible().into_iter().map(|i| o.menu.entries[i].label.clone()).collect::<Vec<_>>()),
                "footer": [self.footer_left(), footer_action.clone()],
                "ai": self.ai.as_ref().map(|a| json!({"text": a.turns.last().map_or("", |t| t.text.as_str()), "busy": a.busy, "turns": a.turns.len()})),
                "notice": self.notice,
                "view": view.as_ref().map(|v| &v.kind),
                "fields": self.form_fields().map(|(f, _)| f.iter().map(|f| f.label.to_string()).collect::<Vec<_>>()),
                "field": self.form_fields().map(|(_, i)| i),
                "trays": self.trays.keys().collect::<Vec<_>>(),
                "args": arg_values,
                "arg_missing": self.arg_missing,
                "quick_look": self.quick_look,
            }),
        );

        let collapsed = self.collapsed(cx);

        // ---- results list ----
        let slots: Vec<Slot> = {
            let mut out = vec![];
            let rows = display_rows(&self.items, self.mode == Mode::Schedule);
            let count = |sec: &Option<String>| self.items.iter().filter(|it| &it.section == sec).count();
            // The right-hand column shows what tells rows apart. In root search that is a
            // row's own hotkey or alias (its kind is in the action bar); elsewhere it is the
            // row's accessory, said once per run of rows that share it (the translator names
            // every row's language).
            let root = self.mode == Mode::Root;
            let mut above = String::new();
            for (k, r) in rows.iter().enumerate() {
                match r {
                    None => {
                        let it = &self.items[rows[k + 1].unwrap_or(0)];
                        let sec = it.section.clone();
                        above.clear();
                        let counted = (self.mode != Mode::Root || sec.as_deref() == Some(RUNNING)) && sec.as_deref() != Some(DID_YOU_MEAN);
                        out.push(Slot::Header(sec.clone().unwrap_or_default(), counted.then(|| count(&sec))));
                    }
                    Some(i) => {
                        let it = &self.items[*i];
                        let hotkey = it.key.as_ref().filter(|_| root).and_then(|k| self.cfg.launcher.hotkeys.get(k)).cloned().unwrap_or_default();
                        // A row with arguments says how many until it is selected.
                        let accessory = if it.live {
                            it.accessory.clone()
                        } else if root {
                            if it.alias.is_empty() && *i != self.selected { it.arg_count().unwrap_or_default() } else { it.alias.clone() }
                        } else if it.accessory == above && self.mode != Mode::Translate {
                            String::new()
                        } else {
                            above = it.accessory.clone();
                            above.clone()
                        };
                        // Its subtitle repeats what its boxes hold.
                        let values = if *i == self.selected && arg_row.is_some() { &arg_values } else { &it.values };
                        let mirrored = search::mirror(it, values);
                        // Define's subtitle is the word's first sense, once known.
                        let subtitle = match define_word(it, values) {
                            Some(w) => self.senses.get(&w.to_lowercase()).cloned().flatten().unwrap_or_default(),
                            None if mirrored.is_empty() => it.subtitle.clone(),
                            None => mirrored,
                        };
                        out.push(Slot::Row { idx: *i, title: it.title.clone(), subtitle, accessory, hotkey, icon: it.icon.clone(), tint: it.tint, live: it.live, now: it.now, toggle: it.toggle });
                    }
                }
            }
            out
        };
        let selected = self.selected;
        let me = cx.weak_entity();
        let slots = Rc::new(slots);
        // Narrow split lists (detail on the right) get a tighter accessory column.
        let acc_max = if view.as_ref().is_some_and(|v| v.show_detail) || matches!(self.mode, Mode::Clipboard | Mode::Translate | Mode::ScreenTime(_)) { 110. } else { design::ACCESSORY_MAX };
        let live_w = design::live_width(window);
        let list = {
            let slots = slots.clone();
            uniform_list("rows", slots.len(), move |range, _, _| {
                range
                    .map(|k| match &slots[k] {
                        Slot::Header(title, count) => design::section_header(title, *count, &t).into_any_element(),
                        Slot::Row { idx, title, subtitle, accessory, hotkey, icon, tint, live, now, toggle } => {
                            let (i, on) = (*idx, *idx == selected);
                            let me = me.clone();
                            let accessory = if let Some(state) = toggle {
                                Some(design::toggle(*state, &lt).into_any_element())
                            } else if !hotkey.is_empty() {
                                Some(design::key_caps(hotkey, &t).into_any_element())
                            } else if *live {
                                // The event on now: "until 11:00" in the accent, weight 600 (`.pc-now`).
                                let color = if *now { t.accent } else if accessory == timers::DONE { t.success } else { t.text_muted };
                                Some(design::live(accessory, live_w, color).when(*now, |d| d.font_weight(gpui::FontWeight::SEMIBOLD)).into_any_element())
                            } else if !accessory.is_empty() {
                                Some(accessory.clone().into_any_element())
                            } else {
                                None
                            };
                            design::list_row(design::icon_tile(icon, &lt, on, *tint), title, subtitle, accessory, acc_max, on, &lt)
                                .id(k)
                                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                                    me.update(cx, |this, cx| {
                                        this.selected = i;
                                        this.launch(window, cx);
                                    })
                                    .ok();
                                })
                                .into_any_element()
                        }
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1()
            .min_h_0()
            .py(px(LIST_PAD))
        };

        // ---- calculator answer card (two rows tall, so the list below keeps whole rows) ----
        let answer = (self.mode == Mode::Root && is_answer(&self.items, 0)).then(|| {
            let it = &self.items[0];
            let full = match &it.act {
                Act::Copy(v) => v.as_str(),
                _ => "",
            };
            let me = cx.weak_entity();
            design::answer_slot(design::answer_card(&it.subtitle, &it.title, full, self.selected == 0, &t).id("answer").on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                me.update(cx, |this, cx| {
                    this.selected = 0;
                    this.launch(window, cx);
                })
                .ok();
            }))
        });

        // ---- body ----
        let split = |list: gpui::AnyElement, detail: gpui::AnyElement, cfg: &Config| {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().w(px(base_width(cfg) * design::SPLIT_LIST)).flex_none().flex().flex_col().min_h_0().child(list))
                .child(div().flex_1().min_w_0().flex().flex_col().border_l_1().border_color(t.hairline).child(detail))
                .into_any_element()
        };
        let body = if let Some(g) = grid {
            g
        } else if let Some(d) = side {
            split(list.into_any_element(), d, &self.cfg)
        } else {
            match (&empty, &view) {
            _ if self.mode == Mode::Ai => {
                match self.ai.as_ref().filter(|a| !a.turns.is_empty()) {
                    None => Self::render_markdown("Type a question, then press Enter.", &[], &t, "ai", Some(&self.ai_scroll)),
                    Some(chat) => {
                        let (md, newest) = chat.markdown();
                        if chat.follow {
                            self.ai_scroll.scroll_to_top_of_item(newest);
                        }
                        Self::render_markdown(&md, &[], &t, "ai", Some(&self.ai_scroll))
                    }
                }
            }
            (Some(e), _) => {
                let glyph = if let Icon::Vector(c) = e.icon { c } else { g::SEARCH };
                design::empty_state(glyph, &e.title, &e.subtitle, &t).into_any_element()
            }
            _ if self.panel.is_none() && self.is_form() => self.render_form(&t, cx.weak_entity()),
            (_, Some(v)) if v.kind == "Detail" => Self::render_markdown(&v.markdown, &v.metadata, &t, "detail", None),
            (_, Some(v)) if v.show_detail && !v.items.is_empty() => {
                let it = self.items.get(self.selected).and_then(|row| v.items.iter().find(|it| it.title == row.title));
                let md = it.and_then(|it| it.detail.clone()).unwrap_or_default();
                let meta = it.map(|it| it.detail_meta.clone()).unwrap_or_default();
                split(list.into_any_element(), Self::render_markdown(&md, &meta, &t, "item-detail", None), &self.cfg)
            }
            _ if self.confirm.is_some() => div().flex_1().into_any_element(),
            _ => div().flex_1().min_h_0().flex().flex_col().children(answer).child(list).into_any_element(),
            }
        };

        // ---- compact mode: favourites as icons ----
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
                    .size(px(ROW - 4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(design::r::MD))
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.run(act.clone(), key.clone(), window, cx)))
                    .child(design::icon_view(&it.icon, &t, false))
            }));

        // ---- search bar ----
        let at_root = self.mode == Mode::Root;
        let leading = if at_root { design::search_mark(&t).into_any_element() } else { design::back_tile(&t).id("back").on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx))).into_any_element() };
        let dropdown = self.extv.as_ref().filter(|_| self.mode == Mode::Ext).and_then(|e| e.view.dropdown.clone()).map(|(opts, cur, on_change)| {
            let title = opts.iter().find(|o| o.0 == cur).map(|o| o.1.clone()).unwrap_or_default();
            design::dropdown(g::FILTER, &title, &t)
                .id("dropdown")
                .on_click(cx.listener(move |this, _, window, cx| {
                    let Some(f) = on_change.clone() else { return };
                    let entries = opts.iter().map(|(v, title)| menu::Entry { id: v.clone(), label: title.clone(), ..Default::default() }).collect();
                    let mut m = menu::Menu::new(entries, Some("Search…".into()), menu::Place::Right);
                    m.selected = opts.iter().position(|o| o.0 == cur).unwrap_or(m.selected);
                    this.open_menu(m, move |this, v, window, cx| this.run(Act::ExtSet(f.clone(), v), None, window, cx), window, cx);
                }))
                .into_any_element()
        });
        // The translator's languages; a click (or Ctrl+P) opens the language set menu.
        let dropdown = dropdown.or_else(|| {
            let tr = self.translation.as_ref().filter(|_| self.mode == Mode::Translate)?;
            let label = translate::LangSet { from: tr.job.from.clone(), to: tr.job.to.clone() }.label();
            Some(design::dropdown(g::FILTER, &label, &t).id("lang-set").on_click(cx.listener(|this, _, window, cx| this.open_lang_menu(window, cx))).into_any_element())
        });
        let dropdown = dropdown.or_else(|| {
            (self.mode == Mode::Shots).then(|| design::dropdown(g::BULLETS, self.shot_kind.label(), &t).id("shot-kind").on_click(cx.listener(|this, _, window, cx| this.open_shot_menu(window, cx))).into_any_element())
        });
        // While the scan reads text: "Reading text in screenshots  94 of 996", static.
        let banner = self.shots_progress.filter(|(done, of)| self.mode == Mode::Shots && done < of).map(|(done, of)| design::banner(g::SCAN_TEXT, "Reading text in screenshots", &format!("{} of {}", design::thousands(done), design::thousands(of)), &t));
        // Compact root with nothing typed: say what Down does, so the bare bar is not a dead end.
        let hint = (collapsed && !self.fav_strip()).then(|| design::search_hint("Recent", "↓", &t).into_any_element());
        let query_input = text_input("query")
            .state(self.input.downgrade())
            .placeholder(self.placeholder())
            .placeholder_color(t.text_faint)
            .caret_color(t.accent)
            .caret_blink_interval_500ms()
            .font_family(design::FONT_DISPLAY)
            .text_size(px(design::ts::QUERY))
            .whitespace_nowrap();
        // With boxes the query takes its own width (it gives way first), then the command's
        // icon and the boxes, together at most 60% of the bar.
        let (query, arg_group) = match &arg_row {
            // A native form's bar holds only Back.
            None if matches!(self.mode, Mode::Form(_)) => (div().flex_1().into_any_element(), None),
            None => (query_input.flex_1().into_any_element(), None),
            Some(it) => {
                let q = Some(self.query(cx)).filter(|q| !q.is_empty()).unwrap_or_else(|| self.placeholder().to_string());
                let w = design::text_width(&q, design::ts::QUERY, window) + px(design::sp::S4);
                let query = div().flex_shrink(1.).min_w_0().w(w).overflow_hidden().child(query_input.w_full()).into_any_element();
                let hue = it.tint.map_or(t.text_muted, |k| k.hue(&t));
                let mark = match &it.icon {
                    Icon::Vector(ch) => design::icon(ch, design::ic::LG, hue).into_any_element(),
                    Icon::Image(image) => gpui::img(image.clone()).size(px(design::ic::LG)).flex_none().into_any_element(),
                    Icon::Glyph(s) => div().font_family(design::FONT_EMOJI).text_size(px(design::ic::LG)).child(s.clone()).into_any_element(),
                };
                let boxes = it.args.iter().enumerate().map(|(i, a)| {
                    let v = arg_values[i].as_str();
                    let secret = a.kind == search::ArgKind::Secret;
                    let dots = secret.then(|| "•".repeat(v.chars().count()));
                    let tag = a.optional && v.is_empty();
                    let help = (!a.help.is_empty()).then(|| SharedString::from(a.help.clone()));
                    let measured = if v.is_empty() { a.placeholder.as_ref() } else { dots.as_deref().unwrap_or(v) };
                    let width = design::arg_width(measured, tag, help.is_some(), window);
                    let input = text_input(SharedString::from(format!("arg-{i}")))
                        .state(self.args[i].downgrade())
                        .placeholder(a.placeholder.clone())
                        .placeholder_color(t.text_faint)
                        .caret_color(t.accent)
                        .w_full()
                        .whitespace_nowrap()
                        .when(secret, |e| e.text_color(gpui::transparent_black()));
                    let left = self.arg_left.clone();
                    let look = design::ArgBox { width, tag, help, dots, focused: arg_focus == Some(i), missing: self.arg_missing == Some(i) };
                    let b = design::arg_box(i, input, look, &t).on_prepaint(move |p, _, _| left[i].set(p.bounds.origin.x));
                    if matches!(a.kind, search::ArgKind::Choice(_)) { b.on_click(cx.listener(move |this, _, window, cx| this.open_choice(i, window, cx))).into_any_element() } else { b.into_any_element() }
                });
                let group = div().flex_none().max_w(gpui::relative(0.6)).ml(px(design::sp::S4)).flex().items_center().gap(px(design::sp::S8)).overflow_hidden().child(mark).children(boxes);
                (query, Some(group.into_any_element()))
            }
        };
        let search_bar = design::search_bar(leading, query, arg_group.into_iter().chain(hint).chain(dropdown), &t);

        // ---- action bar ----
        let tone = self.status.as_ref().map(|(tone, _)| match tone {
            Tone::Error => t.danger,
            Tone::Ok => t.success,
            Tone::Info => t.accent,
        });
        // A status shows its tone dot; in root search the selected row's own icon in its hue; else the view's icon.
        let mark = match tone {
            Some(c) => design::status_dot(c).into_any_element(),
            None => match selected_item.filter(|it| (at_root || self.mode == Mode::Args) && it.section.as_deref() != Some(EMPTY)).map_or_else(|| (self.context_icon(), t.text_muted), |it| (it.icon.clone(), it.tint.map_or(t.text_muted, |k| k.hue(&t)))) {
                (Icon::Vector(ch), hue) => design::icon(ch, design::ic::MD, hue).into_any_element(),
                (Icon::Image(image), _) => gpui::img(image).size(px(design::ic::LG)).flex_none().into_any_element(),
                (Icon::Glyph(s), _) => div().font_family(design::FONT_EMOJI).text_size(px(design::ic::MD)).child(s).into_any_element(),
            },
        };
        let keys = |label: &str, keys: &[&str]| design::shortcut(label, keys, &t).into_any_element();
        let right = if self.panel.is_some() {
            vec![keys("Run", &["↵"]), design::vdivider(&t).into_any_element(), keys("Close", &["Esc"])]
        } else if let Mode::Form(kind) = self.mode {
            vec![keys(kind.bar().2, &["Ctrl", "↵"])]
        } else if self.is_form() {
            vec![keys("Submit", &["Ctrl", "↵"]), design::vdivider(&t).into_any_element(), keys("Actions", &["Ctrl", "K"])]
        } else if self.armed == Some(self.selected) {
            vec![div().text_color(t.warning).child("Press ↵ again to confirm").into_any_element()]
        } else if let Some((next, verb)) = &arg_keys {
            let mut v = vec![];
            if *next {
                v.extend([keys("Next Box", &["Tab"]), design::vdivider(&t).into_any_element()]);
            }
            v.push(keys(verb, &["↵"]));
            v
        } else if let Some(p) = &primary {
            let mut v = vec![keys(p, &["↵"])];
            if let Some(second) = &bar_second {
                v.extend([design::vdivider(&t).into_any_element(), keys(second, &["Ctrl", "↵"])]);
            }
            if actions.len() > 1 {
                v.extend([design::vdivider(&t).into_any_element(), keys("Actions", &["Ctrl", "K"])]);
            }
            v
        } else {
            vec![]
        };
        let footer = design::action_bar(mark, &self.footer_left(), tone.is_some(), right, &t);

        // ---- overlays ----
        let search = self.panel_filtered().then(|| {
            let input = text_input("panel-search").state(self.menu_input.downgrade()).placeholder("Search for actions…").placeholder_color(t.text_faint).caret_color(t.accent).w_full().whitespace_nowrap();
            design::panel_search(input, &t)
        });
        let popover = self.panel.as_ref().map(|(item, sel)| {
            let me = cx.weak_entity();
            let rows = panel_rows.into_iter().enumerate().map(|(i, (at, (label, act)))| {
                let me = me.clone();
                // The first two take Enter and Ctrl+Enter by place; the rest their own keys.
                let keys: Vec<&str> = match at {
                    0 => vec!["↵"],
                    1 => vec!["Ctrl", "↵"],
                    _ => search::act_keys(&act).map_or(vec![], |k| k.split('+').collect()),
                };
                design::panel_row(act_glyph(&act), &label, &keys, i == *sel, act.destructive(), &t).id(("act", i)).on_click(move |_, window, cx| {
                    me.update(cx, |this, cx| {
                        if let Some((_, p)) = &mut this.panel {
                            *p = i;
                        }
                        this.launch(window, cx);
                    })
                    .ok();
                })
            });
            // The window steps back under `scrim_soft` so the panel is plainly the top layer.
            // A click on it closes the panel; the panel occludes it, so clicks inside stay with the panel.
            [
                design::scrim(t.scrim_soft).id("panel-scrim").occlude().on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx))).into_any_element(),
                design::action_panel(design::panel_head(design::icon_tile(&item.icon, &t, false, item.tint), &item.title, &t), rows, &t).children(search).occlude().into_any_element(),
            ]
        });
        let dialog = self.confirm.as_ref().map(|c| {
            let me = cx.weak_entity();
            let button = |i: usize, label: &str, kind: design::Button| {
                let me = me.clone();
                design::button(label, kind, i == selected, &t).id(("dialog", i)).w_full().on_click(move |_, window, cx| {
                    me.update(cx, |this, cx| {
                        this.selected = i;
                        this.launch(window, cx);
                    })
                    .ok();
                })
            };
            let primary = if c.danger { design::Button::Danger } else { design::Button::Primary };
            design::dialog(c.danger, &c.title, &c.message, [button(0, &c.primary, primary), button(1, "Cancel", design::Button::Quiet)], &t)
        });

        let pop = self.menu.as_ref().map(|open| {
            let me = cx.weak_entity();
            let pick = move |i: usize, window: &mut Window, cx: &mut App| _ = me.update(cx, |this, cx| this.menu_pick(i, window, cx));
            let me = cx.weak_entity();
            let outside = move |_: &gpui::MouseDownEvent, window: &mut Window, cx: &mut App| _ = me.update(cx, |this, cx| _ = this.close_menu(window, cx));
            let field = open.menu.search.as_ref().map(|placeholder| {
                let input = text_input("menu-search").state(self.menu_input.downgrade()).placeholder(placeholder.clone()).placeholder_color(t.text_faint).caret_color(t.accent).w_full().whitespace_nowrap();
                // In the Block menu, Backspace in the empty search takes the last token.
                div().flex_1().min_w_0().when(open.site.is_some(), |d| d.key_context("TokenMenu")).child(input).into_any_element()
            });
            design::pop_menu(&open.menu, field, &open.scroll, pick, outside, |_, _| None, &t)
        });

        // A form's keys live on the root, which stays drawn while Ctrl+K or a confirm covers
        // the form: a focused control that is not drawn then leaves the keys here.
        design::glass_window(&t)
            .when(self.form_under(), |d| d.track_focus(&self.form_focus).key_context("Form"))
            .size_full()
            .font_family(design::FONT_UI)
            .text_size(px(design::ts::BODY))
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                if this.menu_key(menu::Key::Down, window, cx) {
                    return;
                }
                // In a box Down opens a list's menu, and otherwise does nothing.
                if this.panel.is_none()
                    && let Some(i) = this.arg_focused(window, cx)
                {
                    return this.open_choice(i, window, cx);
                }
                if this.panel.is_none() && this.mode == Mode::Ai {
                    return this.scroll_ai(ROW * 3., cx);
                }
                // On a select Down opens its menu; elsewhere in a form it moves on.
                if this.panel.is_none() && this.is_form() {
                    if !this.open_field_menu(window, cx) {
                        this.move_field(1, window, cx);
                    }
                    return;
                }
                if this.collapsed(cx) {
                    this.peek = true;
                    return this.refresh(cx);
                }
                if this.panel.is_none() && this.grid() {
                    return this.grid_move(0, 1, cx);
                }
                let shown = this.panel_rows(cx).len();
                match &mut this.panel {
                    Some((_, i)) => *i = (*i + 1).min(shown.saturating_sub(1)),
                    None => this.selected = (this.selected + 1).min(this.items.len().saturating_sub(1)),
                }
                this.armed = None;
                this.sync_args(cx);
                this.scroll_to_selected();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| {
                if this.menu_key(menu::Key::Up, window, cx) || (this.panel.is_none() && this.arg_focused(window, cx).is_some()) {
                    return;
                }
                if this.panel.is_none() && this.mode == Mode::Ai {
                    return this.scroll_ai(-ROW * 3., cx);
                }
                if this.panel.is_none() && this.is_form() {
                    return this.move_field(-1, window, cx);
                }
                if this.panel.is_none() && this.selected == 0 && this.recall_search(cx) {
                    return;
                }
                if this.panel.is_none() && this.grid() {
                    return this.grid_move(0, -1, cx);
                }
                match &mut this.panel {
                    Some((_, i)) => *i = i.saturating_sub(1),
                    None => this.selected = this.selected.saturating_sub(1),
                }
                this.armed = None;
                this.sync_args(cx);
                this.scroll_to_selected();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleActions, window, cx| {
                if this.menu.is_some() {
                    return;
                }
                this.panel = match this.panel.take() {
                    Some(_) => None,
                    None => this.items.get(this.selected).cloned().filter(|it| it.section.as_deref() != Some(EMPTY) || !it.extra.is_empty()).filter(|it| it.act != Act::None || !it.extra.is_empty()).map(|it| (it, 0)),
                };
                // A long panel has a search field, and typing goes there.
                if this.panel_filtered() {
                    this.menu_input.update(cx, |s, cx| s.emplace("", cx));
                    window.focus(&this.menu_input.focus_handle(cx), cx);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &KeyOpen, window, cx| this.shortcut("Ctrl+O", window, cx)))
            .on_action(cx.listener(|this, _: &KeyOpenWith, window, cx| this.shortcut("Ctrl+Shift+O", window, cx)))
            .on_action(cx.listener(|this, _: &KeyQuickLook, window, cx| this.shortcut("Ctrl+Y", window, cx)))
            .on_action(cx.listener(|this, _: &KeyReveal, window, cx| this.shortcut("Ctrl+Alt+O", window, cx)))
            .on_action(cx.listener(|this, _: &OpenDropdown, window, cx| {
                if this.menu.is_none() && this.panel.is_none() {
                    match this.mode {
                        Mode::Translate => this.open_lang_menu(window, cx),
                        Mode::Shots => this.open_shot_menu(window, cx),
                        _ => {}
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| {
                if !this.step_swatch(-1, cx) {
                    this.grid_move(-1, 0, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| {
                if !this.step_swatch(1, cx) {
                    this.grid_move(1, 0, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &NextFilter, _, cx| this.music_cycle(cx)))
            // Left and Right move through a grid instead of the search text; Tab cycles YouTube Music's filters.
            .when_some(self.grid_keys(), |d, context| d.key_context(context))
            // Tab and Shift+Tab move between the query and the boxes.
            .when(arg_row.is_some() && self.panel.is_none(), |d| d.key_context("ArgBoxes"))
            // Anywhere but a token field (or its menu's empty search), Backspace edits the text.
            .on_action(cx.listener(|this, _: &PopToken, _, cx| {
                if !this.pop_token(cx) {
                    cx.propagate();
                }
            }))
            // Over a form, an open menu takes Tab and Shift+Tab as Down and Up.
            .on_action(cx.listener(|this, _: &NextBox, window, cx| match this.is_form() {
                true if this.menu_key(menu::Key::Down, window, cx) => {}
                true => this.move_field(1, window, cx),
                false => this.step_arg(1, window, cx),
            }))
            .on_action(cx.listener(|this, _: &PrevBox, window, cx| match this.is_form() {
                true if this.menu_key(menu::Key::Up, window, cx) => {}
                true => this.move_field(-1, window, cx),
                false => this.step_arg(-1, window, cx),
            }))
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                if !this.menu_key(menu::Key::Esc, window, cx) && !this.dismiss_args(window, cx) {
                    this.dismiss(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &EmacsNext, window, cx| this.nav("emacs", Box::new(SelectNext), window, cx)))
            .on_action(cx.listener(|this, _: &EmacsPrev, window, cx| this.nav("emacs", Box::new(SelectPrev), window, cx)))
            .on_action(cx.listener(|this, _: &EmacsBack, window, cx| this.nav("emacs", Box::new(Dismiss), window, cx)))
            .on_action(cx.listener(|this, _: &VimNext, window, cx| this.nav("vim", Box::new(SelectNext), window, cx)))
            .on_action(cx.listener(|this, _: &VimPrev, window, cx| this.nav("vim", Box::new(SelectPrev), window, cx)))
            .on_action(cx.listener(|this, _: &VimBack, window, cx| this.nav("vim", Box::new(Dismiss), window, cx)))
            .on_action(cx.listener(|this, _: &Launch, window, cx| {
                if !this.menu_key(menu::Key::Enter, window, cx) {
                    this.launch(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Submit, window, cx| {
                // The Block menu stays open between picks; Ctrl+Enter there still starts the session.
                if this.menu.as_ref().is_some_and(|m| m.site.is_some()) {
                    this.close_menu(window, cx);
                } else if this.menu.is_some() {
                    return;
                }
                if this.is_form() && this.panel.is_none() {
                    this.submit_form(window, cx)
                } else if this.panel.is_none()
                    && let Some(item) = this.items.get(this.selected).cloned()
                    && item.actions().len() > 1
                {
                    // Ctrl+Enter: the secondary action, as shown in the action panel.
                    let act = item.actions().swap_remove(1).1;
                    this.run(act, item.key, window, cx);
                } else {
                    this.launch(window, cx)
                }
            }))
            .child(search_bar)
            .children(notices.into_iter().map(|n| {
                div()
                    .mx(px(16.))
                    .mt(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(design::r::SM))
                    .bg(t.surface)
                    .text_size(px(design::ts::BODY_SM))
                    .child(design::icon(g::WARNING, design::ic::MD, t.warning))
                    .child(div().min_w_0().child(n))
            }))
            .when(collapsed && self.fav_strip(), |d| d.child(fav_strip))
            .when(!collapsed, |d| d.children(banner).child(body).child(footer))
            .children(popover.into_iter().flatten())
            .children(dialog)
            .children(pop)
    }
}

/// Every key binding: the text box defaults first, then ours, added later at the same depth so
/// they win for these keys.
fn key_bindings() -> Vec<KeyBinding> {
    let ours = [
        KeyBinding::new("up", SelectPrev, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("tab", SelectNext, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("shift-tab", SelectPrev, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("enter", Launch, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-enter", Submit, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-k", ToggleActions, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-p", OpenDropdown, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-o", KeyOpen, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-shift-o", KeyOpenWith, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-y", KeyQuickLook, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-alt-o", KeyReveal, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-down", NextSection, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-up", PrevSection, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("alt-n", EmacsNext, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("alt-p", EmacsPrev, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("alt-b", EmacsBack, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("alt-j", VimNext, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("alt-k", VimPrev, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("alt-h", VimBack, Some(DEFAULT_INPUT_CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(&format!("EmojiGrid > {DEFAULT_INPUT_CONTEXT}"))),
        KeyBinding::new("right", SelectRight, Some(&format!("EmojiGrid > {DEFAULT_INPUT_CONTEXT}"))),
        KeyBinding::new("tab", NextFilter, Some(&format!("Music > {DEFAULT_INPUT_CONTEXT}"))),
        KeyBinding::new("tab", NextBox, Some(&format!("ArgBoxes > {DEFAULT_INPUT_CONTEXT}"))),
        KeyBinding::new("shift-tab", PrevBox, Some(&format!("ArgBoxes > {DEFAULT_INPUT_CONTEXT}"))),
        // In a form's text box Tab moves to the next control, never into a menu (Down does that).
        KeyBinding::new("tab", NextBox, Some(&format!("Form > {DEFAULT_INPUT_CONTEXT}"))),
        KeyBinding::new("shift-tab", PrevBox, Some(&format!("Form > {DEFAULT_INPUT_CONTEXT}"))),
        // A form control without a text box (a select, a checkbox) holds the keys itself.
        KeyBinding::new("tab", NextBox, Some("Form")),
        KeyBinding::new("shift-tab", PrevBox, Some("Form")),
        KeyBinding::new("up", SelectPrev, Some("Form")),
        KeyBinding::new("down", SelectNext, Some("Form")),
        KeyBinding::new("enter", Launch, Some("Form")),
        KeyBinding::new("ctrl-enter", Submit, Some("Form")),
        KeyBinding::new("escape", Dismiss, Some("Form")),
        KeyBinding::new("ctrl-k", ToggleActions, Some("Form")),
        KeyBinding::new("backspace", PopToken, Some("Form")),
        // Add Calendar's swatches; a text box keeps its own Left and Right.
        KeyBinding::new("left", SelectLeft, Some("Form")),
        KeyBinding::new("right", SelectRight, Some("Form")),
        KeyBinding::new("backspace", PopToken, Some(&format!("TokenMenu > {DEFAULT_INPUT_CONTEXT}"))),
        // The settings window when no text field has focus.
        KeyBinding::new("up", SelectPrev, Some("Settings")),
        KeyBinding::new("down", SelectNext, Some("Settings")),
        KeyBinding::new("tab", SelectNext, Some("Settings")),
        KeyBinding::new("shift-tab", SelectPrev, Some("Settings")),
        KeyBinding::new("enter", Launch, Some("Settings")),
        KeyBinding::new("escape", Dismiss, Some("Settings")),
        KeyBinding::new("ctrl-down", NextSection, Some("Settings")),
        KeyBinding::new("ctrl-up", PrevSection, Some("Settings")),
    ];
    default_bindings().as_keybindings(Some(DEFAULT_INPUT_CONTEXT)).chain(ours).collect()
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
    let open = MenuItem::new("Open procast", true, None);
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
        if let Some(msg) = music_tray::message(&e.id.0) {
            let _ = menu_tx.unbounded_send(msg);
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
        // Only procast's own icon opens the launcher; extension icons just show their menu.
        if let TrayIconEvent::Click { id, button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e
            && id.0 == "procast"
        {
            let _ = tx.unbounded_send(Msg::Tray(TrayCmd::Open));
        }
    }));
    TrayIconBuilder::new().with_id("procast").with_menu(Box::new(menu)).with_tooltip("procast").with_icon(tray_icon_image()).build().ok()
}

fn main() {
    config::migrate_old_data();
    // The whole-drive file index helper (see ntfs.rs).
    match std::env::args().nth(1).as_deref() {
        Some("--indexer") => return ntfs::serve(),
        // A browser starting its native messaging host (browser.rs).
        Some(origin) if origin.starts_with("chrome-extension://") => return browser::host(),
        Some(flag @ ("--install-indexer" | "--uninstall-indexer")) => {
            let done = if flag == "--install-indexer" { ntfs::install() } else { ntfs::uninstall() };
            if let Err(e) = done {
                let _ = std::fs::write(std::env::temp_dir().join("procast-indexer-error.txt"), e);
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
    gpui_ce_platform::application().with_assets(design::Icons).with_quit_mode(QuitMode::Explicit).run(|cx: &mut App| {
        design::register_fonts(cx);
        cx.bind_keys(key_bindings());

        let (cfg, notice) = match config::load() {
            Ok(cfg) if cfg.files.ntfs && ntfs::old_helper_installed() => {
                (cfg, Some("Whole-drive file search is off since the rename to procast. Run \"Enable Fast File Search\" once to turn it back on.".into()))
            }
            Ok(cfg) => (cfg, None),
            Err(e) => (Config::default(), Some(e)),
        };
        win::set_start_at_login(cfg.start_at_login);
        net::load_rates();
        design::set_size(&cfg.size);
        // Before the first render, so the body is not drawn as glass on a machine with transparency off.
        design::refresh_glass();
        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        let tray = if cfg.tray { tray(tx.clone()) } else { None };
        let tick_tx = tx.clone();

        let bounds = Bounds::centered(None, size(px(base_width(&cfg)), px(window_height(&cfg))), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            // Shown at once and cloaked before its view is built: gpui shows it with
            // SW_SHOWNOACTIVATE only here. `show: false` then `set_visible` replays a placement
            // that activates it, taking the foreground (and the user's typing) at startup.
            show: true,
            focus: false,
            // No backdrop to draw before the cloak; `show` sets the real one before un-cloaking.
            windows_window_background: gpui::WindowsWindowBackground::Transparent,
            ..Default::default()
        };
        let handle = cx
            .open_window(options, |window, cx| {
                win::cloak(win::hwnd(window), true);
                cx.new(|cx| Launcher::new(cfg, notice, tx, window, cx))
            })
            .unwrap();

        // Shown once, cloaked: from now on hotkey -> un-cloak -> next frame.
        let _ = handle.update(cx, |this, window, _| {
            this.hwnd = win::hwnd(window);
            win::make_tool_window(this.hwnd);
            win::round_corners(this.hwnd);
            this.tray_icon = tray;
            this.music_tray_sync();
            this.scan_apps(true, false);
            // For screenshots and checks of the settings window without clicking through.
            if std::env::var_os("PROCAST_OPEN_SETTINGS").is_some() {
                let _ = this.tx.unbounded_send(Msg::Tray(TrayCmd::Settings));
            }
            // PROCAST_SHOW=root|clipboard|emoji|files|running with PROCAST_QUERY: opens there.
            if std::env::var_os("PROCAST_SHOW").is_some() {
                let _ = this.tx.unbounded_send(Msg::Tray(TrayCmd::Open));
            }
        });

        // PROCAST_TYPE: text put in the search bar a second after opening, through the same
        // change event as typing (checks of compact mode without sending keys).
        if let Ok(text) = std::env::var("PROCAST_TYPE") {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = handle.update(cx, |this, _, cx| this.input.update(cx, |s, cx| s.emplace(&text, cx)));
            })
            .detach();
        }

        cx.spawn(async move |cx| {
            let toast = toast::open(cx);
            _ = handle.update(cx, |this, _, _| this.toast = toast);
            let mut hud = handle.update(cx, |this, _, _| this.tx.clone()).map(|tx| hud::Hud::open(cx, tx)).unwrap_or_default();
            while let Some(msg) = rx.next().await {
                // Between updates (it moves windows): what the action that sent this message
                // did to the session (and, after `on_msg` below, what the message did).
                if let Ok(state) = handle.update(cx, |this, _, _| this.hud()) {
                    hud.sync(cx, state);
                }
                if let Msg::Resize = msg {
                    if let Ok((hwnd, h)) = handle.update(cx, |this, _, _| (this.hwnd, this.height)) {
                        win::resize(hwnd, h * design::launcher_scale());
                    }
                    continue;
                }
                if let Msg::Toast(kind, stay, text) = msg {
                    // A sticky toast would take the open launcher's Esc: it goes in the ActionBar instead.
                    let theme = handle.update(cx, |this, _, cx| {
                        if this.visible && stay == toast::Stay::Sticky {
                            this.status = Some((Tone::of(kind), text.clone()));
                            cx.notify();
                            return None;
                        }
                        Some(this.cfg.theme.clone())
                    });
                    if let Ok(Some(theme)) = theme {
                        toast::show(cx, toast, kind, stay, &text, theme);
                    }
                    continue;
                }
                // SetWindowPos sends WM_SIZE/WM_DPICHANGED synchronously and GPUI's window
                // proc re-borrows the app, so the window is moved here, between updates.
                let opens = match msg {
                    Msg::Hotkey(i) => handle.update(cx, |this, _, _| this.hotkey_opens(i) && !(this.cfg.launcher.game_mode && !this.visible && foreground::full_screen_app())).unwrap_or(false),
                    Msg::Expand(ref hit) => handle.update(cx, |this, _, _| this.expand_opens(hit)).unwrap_or(false),
                    _ => matches!(msg, Msg::Tray(TrayCmd::Open) | Msg::Hud(hud::Click::Open) | Msg::HotkeyStatus(0, Err(_)) | Msg::ScriptDone(..) | Msg::Notice(_)),
                };
                let opens = opens || match &msg {
                    // The launcher was hidden to read the selection: the answer brings it back.
                    Msg::Translated { hidden: true, job, result: Ok(_), .. } => job.after == translate::After::Open,
                    _ => false,
                };
                let opens_root = matches!(msg, Msg::Hotkey(0) | Msg::Tray(TrayCmd::Open) | Msg::Hud(hud::Click::Open));
                // The pill opens on the session's row, so with the list even in compact mode.
                let pill = matches!(msg, Msg::Hud(hud::Click::Open));
                let geometry = handle.update(cx, |this, _, cx| {
                    // Opening compact root search: just the bar, unless the last search is kept.
                    let compact = opens_root && !pill && this.cfg.window_mode == "compact" && (!this.keeps_state() || this.collapsed(cx));
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
                if let Ok(state) = handle.update(cx, |this, _, _| this.hud()) {
                    hud.sync(cx, state);
                }
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
                    let _ = handle.update(cx, |this, window, cx| {
                        window.set_background_appearance(backdrop(design::refresh_glass()).into());
                        this.reload(cx);
                    });
                }
                if ticks.is_multiple_of(5) {
                    let _ = tick_tx.unbounded_send(Msg::Tick);
                }
                ticks += 1;
            }
        })
        .detach();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_rows_say_when_each_note_was_opened_and_how_long_it_is() {
        let meta = |id: &str, title: &str, opened: i64, deleted_at: Option<i64>| NoteMeta { id: id.into(), pinned: false, created: 0, modified: 100, deleted_at, opened, title: title.into() };
        let now = 3 * 86_400 + 100;
        let notes = [(meta("a", "Groceries", now - 120, None), 186), (meta("b", "Plans", 0, None), 1), (meta("c", "Trip", now - 2 * 86_400, None), 397)];
        let rows = note_rows(NotesList::All, &notes, Some(("c".into(), 400)), now, false, 30);
        let line = |it: &Item| (it.title.clone(), it.subtitle.clone(), it.act.clone(), it.section.clone());
        assert_eq!(line(&rows[0]), ("Groceries".into(), "Opened 2 min ago · 186 characters".into(), Act::OpenNote("a".into()), Some("Notes".into())));
        assert_eq!(line(&rows[1]).1, "Edited 3 days ago · 1 character", "never opened: its last edit");
        assert_eq!(line(&rows[2]).1, "● Current · 400 characters", "the window's note, counted with its edits");
        assert_eq!(note_rows(NotesList::All, &notes, None, now, false, 30)[2].subtitle, "Opened 2 days ago · 397 characters");
        assert!(rows.iter().all(|it| it.tint == Some(design::Tint::Paste)));

        let gone = note_rows(NotesList::Deleted, &[(meta("c", "Old", 0, Some(now - 86_400)), 0)], Some(("c".into(), 0)), now, false, 30);
        assert_eq!(line(&gone[0]), ("Old".into(), "Deleted yesterday · 0 characters".into(), Act::RestoreNote("c".into()), Some("Recently Deleted".into())));
        let acts: Vec<(String, bool)> = gone[0].actions().into_iter().map(|(l, a)| (l, a.destructive())).collect();
        assert_eq!(acts, [("Restore".to_string(), false), ("Delete Permanently".to_string(), true)]);

        assert_eq!(note_rows(NotesList::All, &[], None, now, false, 30)[0].act, Act::Cmd(Cmd::CreateNote));
        assert_eq!(note_rows(NotesList::All, &[], None, now, true, 30)[0].title, "No matching notes");
        assert_eq!(note_rows(NotesList::Deleted, &[], None, now, false, 7)[0].subtitle, "Deleted notes stay 7 days");
    }

    #[test]
    fn create_note_takes_an_optional_title_and_runs_from_a_hotkey_at_once() {
        let cfg = Config::default();
        let it = search::catalog(&[], &[], &[], &cfg).into_iter().find(|it| it.act == Act::Cmd(Cmd::CreateNote)).unwrap();
        assert_eq!((it.args.len(), it.args[0].name.as_ref(), it.args[0].optional), (1, "Title", true));
        assert_eq!(search::prefill(&it, "Create Note Groceries"), ["Groceries"]);
        assert_eq!(search::with_args(&it, &["Groceries".into()]).ok(), Some(Act::With(Box::new(Act::Cmd(Cmd::CreateNote)), vec!["Groceries".into()])));
    }

    #[test]
    fn an_older_hdr_read_never_replaces_a_newer_one() {
        // Read 1 (the launcher showed), then read 2 (after Toggle HDR); 1 lands last.
        let mut cache = None;
        assert!(take_hdr_read(&mut cache, 2, 2, vec![]));
        assert!(cache.is_some());
        cache = None;
        assert!(!take_hdr_read(&mut cache, 2, 1, vec![]));
        assert!(cache.is_none());
    }

    #[test]
    fn hdr_rows_in_root_search() {
        // Toggle HDR stays with no HDR display, and says so; other rows are left alone.
        let mut rows = vec![Item::new("Toggle HDR", "System", Icon::Vector(g::SUN_MEDIUM), Act::Cmd(Cmd::ToggleHdr)), Item::new("Lock Screen", "System", Icon::Vector(g::LOCK), Act::Cmd(Cmd::Lock))];
        // Before the first read: the plain subtitle and label, no claim either way.
        hdr_state(&mut rows, None);
        assert_eq!((rows[0].subtitle.as_str(), rows[0].actions()[0].0.as_str()), ("System", "Run Command"));
        hdr_state(&mut rows, Some(&[]));
        assert_eq!((rows[0].subtitle.as_str(), rows[0].actions()[0].0.as_str()), ("No HDR display", "Turn HDR On"));
        assert_eq!((rows[1].subtitle.as_str(), rows[1].extra.len()), ("System", 0));
        // The Auto HDR row shows for "hdr", and Enter flips what its switch shows.
        for q in ["hdr", "HD", "toggle hdr", "auto hdr"] {
            assert!(wants_auto_hdr(q), "{q}");
        }
        for q in ["", "h", "hello", "dark"] {
            assert!(!wants_auto_hdr(q), "{q}");
        }
        let on = auto_hdr_row(true);
        assert_eq!((on.toggle, on.act.clone(), on.section.as_deref()), (Some(true), Act::SetAutoHdr(false), Some("Auto HDR")));
        assert_eq!(auto_hdr_row(false).act.label(), "Turn Auto HDR On");
    }

    fn field(label: &'static str, ctl: Ctl, value: &str) -> FormField {
        FormField { label: label.into(), ctl, value: value.into(), input: None, error: None }
    }

    fn text(label: &'static str, value: &str) -> FormField {
        field(label, Ctl::Text { placeholder: "".into(), icon: None, tall: false, presets: &[] }, value)
    }

    fn entry(id: &str, label: &str) -> menu::Entry {
        menu::Entry { id: id.into(), label: label.into(), ..Default::default() }
    }

    #[test]
    fn form_values_collects_text_and_picked_ids_by_label() {
        let mode = Ctl::Select(vec![entry("block", "Block Apps & Websites"), entry("allow", "Allow Apps & Websites")], true);
        let fields = [text("Goal", "Deep work"), field("", Ctl::Note, "a note"), field("Mode", mode, "allow"), field("", Ctl::Rule, ""), text("Duration", "")];
        let v = form_values(&fields);
        assert_eq!(v["Goal"], "Deep work");
        assert_eq!(v["Mode"], "allow", "the picked id, not its label");
        assert_eq!(v["Duration"], "");
        assert_eq!(v.len(), 3, "notes and rules carry no value: {v:?}");
    }

    #[test]
    fn moving_past_the_last_field_wraps_and_skips_notes() {
        let fields = [field("", Ctl::Note, "intro"), text("Goal", ""), field("", Ctl::Rule, ""), text("Duration", ""), field("", Ctl::Note, "note")];
        assert_eq!(next_field(&fields, 1, 1), 3, "past the rule");
        assert_eq!(next_field(&fields, 3, 1), 1, "Tab on the last field wraps to the first");
        assert_eq!(next_field(&fields, 1, -1), 3, "Shift+Tab on the first wraps to the last");
        assert_eq!(next_field(&[], 0, 1), 0);
    }

    #[test]
    fn a_select_opens_its_menu_on_the_current_value_and_a_text_field_has_none() {
        let at = gpui::point(px(10.), px(20.));
        let mode = field("Mode", Ctl::Select(vec![entry("block", "Block Apps & Websites"), entry("allow", "Allow Apps & Websites")], true), "allow");
        let m = field_menu(&mode, at).expect("a select opens a menu");
        assert_eq!(m.picked().map(|e| e.id.as_str()), Some("allow"));
        assert_eq!(m.search.as_deref(), Some("Search…"));
        assert!(matches!(m.place, menu::Place::At(p) if p == at));
        let plain = field("Mode", Ctl::Select(vec![entry("a", "A")], false), "");
        assert!(field_menu(&plain, at).is_some_and(|m| m.search.is_none()), "no search field when asked for none");
        assert!(field_menu(&text("Goal", ""), at).is_none());
    }

    #[test]
    fn an_extension_dropdown_becomes_a_select_on_its_value() {
        let dd = ext::Field { id: "lang".into(), kind: FieldKind::Dropdown(vec![("en".into(), "English".into()), ("de".into(), "German".into())]), title: "Language".into(), placeholder: String::new(), value: json!("en"), controlled: false, on_change: None, error: None };
        let check = ext::Field { id: "ok".into(), kind: FieldKind::Checkbox, title: "Remember".into(), value: json!(false), ..dd.clone() };
        let view = ext::View { kind: "Form".into(), fields: vec![dd.clone(), check.clone()], ..Default::default() };
        let values = HashMap::from([("lang".to_string(), json!("de")), ("ok".to_string(), json!(true))]);
        let ev = ExtView { run: 1, ext: 0, view, depth: 1, values, inputs: HashMap::new(), field: 0, sent_query: None, sent_recent: vec![], _subs: vec![] };
        let f = ext_field(&dd, &ev);
        assert_eq!((f.label.as_ref(), f.value.as_str()), ("Language", "de"), "the value as last set, not the default");
        let m = field_menu(&f, gpui::point(px(0.), px(0.))).expect("the dropdown opens the shared menu");
        assert_eq!(m.picked().map(|e| e.label.as_str()), Some("German"));
        let c = ext_field(&check, &ev);
        assert!(matches!(c.ctl, Ctl::Check) && c.value == "true");
    }

    #[test]
    fn translator_rows_copy_or_paste_per_action_and_end_with_the_pronunciation() {
        let row = |text: &str, to: &str, say: Option<&str>| translate::Translation { text: text.into(), detected: "de".into(), to: to.into(), pronunciation: say.map(String::from) };
        let job = translate::Job { text: "guten Morgen".into(), from: "auto".into(), to: vec!["en".into(), "es".into()], all: true, after: translate::After::Open };
        let tr = TrView { job, rows: vec![row("good morning", "en", Some("ˈɡuːtn̩")), row("buenos días", "es", None)], first: 0, busy: false };
        let rows = translation_rows(&tr, &config::Translator::default());
        let shown: Vec<(&str, &str, &str)> = rows.iter().map(|it| (it.title.as_str(), it.subtitle.as_str(), it.accessory.as_str())).collect();
        assert_eq!(shown, [("good morning", "", "English"), ("buenos días", "", "Spanish"), ("ˈɡuːtn̩", "Pronunciation", "German")]);
        let labels: Vec<String> = rows[0].actions().into_iter().map(|a| a.0).collect();
        assert_eq!(labels, ["Copy Translation", "Paste to Active App", "Copy Source", "Swap Languages", "Open in Google Translate", "Change Target"]);
        assert_eq!(rows[0].act, Act::Copy("good morning".into()), "Enter");
        let swap = &rows[0].actions()[3].1;
        assert!(matches!(swap, Act::Translate(j) if j.text == "good morning" && j.from == "en" && j.to == ["de"]), "{swap:?}");
        let paste = config::Translator { action: "paste".into(), ..Default::default() };
        let rows = translation_rows(&tr, &paste);
        assert_eq!((rows[0].act.clone(), rows[0].actions()[1].0.as_str()), (Act::Paste("good morning".into()), "Copy Translation"));
        assert_eq!(rows[0].actions()[5].1, Act::LangMenu, "Change Target opens the language set menu");
    }

    fn two_sets() -> Vec<translate::LangSet> {
        let v = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        vec![translate::LangSet::new("auto", &v(&["en", "es"])), translate::LangSet::new("de", &v(&["en"]))]
    }

    #[test]
    fn the_language_set_menu_offers_manage_then_the_sets_with_the_current_one_selected() {
        let m = lang_set_menu(&two_sets(), 1);
        let shown: Vec<(&str, &str, Option<&str>)> = m.entries.iter().map(|e| (e.id.as_str(), e.label.as_str(), e.icon)).collect();
        assert_eq!(shown, [("manage", "Manage Language Sets…", Some(g::EDIT)), ("0", "Auto-Detect → English, Spanish", Some(g::LANGUAGES)), ("1", "German → English", Some(g::LANGUAGES))]);
        assert_eq!(m.rows()[1], menu::Row::Rule, "a rule under Manage");
        assert_eq!(m.search.as_deref(), Some("Search…"));
        assert!(matches!(m.place, menu::Place::Right));
        assert_eq!(m.picked().map(|e| e.id.as_str()), Some("1"));
    }

    #[test]
    fn manage_language_sets_lists_add_then_the_sets_the_current_one_ticked() {
        let rows = lang_set_rows(&two_sets(), 0);
        let shown: Vec<(&str, &str)> = rows.iter().map(|it| (it.title.as_str(), it.accessory.as_str())).collect();
        assert_eq!(shown, [("Add Language Set…", ""), ("Auto-Detect → English, Spanish", "Current"), ("German → English", "")]);
        assert!(matches!((&rows[0].icon, &rows[1].icon, &rows[2].icon), (Icon::Vector(g::PLUS), Icon::Vector(g::CHECK), Icon::Vector(g::LANGUAGES))));
        assert_eq!((rows[0].act.clone(), rows[0].act.label()), (Act::EditLangSet(None), "Add Language Set"));
        let acts = |i: usize| rows[i].actions().into_iter().map(|a| a.0).collect::<Vec<_>>();
        assert_eq!(acts(2), ["Make Current", "Edit", "Delete"]);
        assert_eq!(acts(1), ["Make Current", "Edit"], "the first set is Settings': no Delete");
        assert_eq!((rows[2].act.clone(), rows[2].actions()[2].1.destructive()), (Act::UseLangSet(1), true));
    }

    #[test]
    fn the_language_set_form_reads_back_as_a_set() {
        let fields = lang_set_form(None);
        let v = form_values(&fields);
        assert_eq!((v["Source language"], v["Target language 1"], v["Target language 2"]), ("auto", "en", ""));
        let title = |f: &FormField| if let Ctl::Select(e, search) = &f.ctl { (e.iter().find(|e| e.id == f.value).map(|e| e.label.clone()), *search) } else { (None, false) };
        assert_eq!(fields.iter().map(title).collect::<Vec<_>>(), [(Some("Auto-Detect".into()), true), (Some("English".into()), true), (Some("None".into()), true)]);
        let set = &two_sets()[1];
        let fields = lang_set_form(Some(set));
        let v = form_values(&fields);
        assert_eq!(translate::LangSet::new(v["Source language"], &[v["Target language 1"].into(), v["Target language 2"].into()]), *set, "Edit is prefilled");
    }

    #[test]
    fn screenshots_say_no_matches_when_a_filter_hides_them() {
        assert_eq!(shots_empty(false, false), ShotsEmpty::NoScreenshots, "only then Add Folder");
        assert_eq!(shots_empty(false, true), ShotsEmpty::NoMatches, "a query or Pinned / Recordings");
        assert_eq!(shots_empty(true, true), ShotsEmpty::Reading);
    }

    #[test]
    fn thumbnail_cache_forgets_the_least_recently_used() {
        let mut cache: HashMap<String, (Option<u32>, u64)> = (0..10u32).map(|i| (format!("t:{i}"), ((i % 2 == 0).then_some(i), u64::from(i)))).collect();
        assert!(evict(&mut cache, 10, 4).is_empty(), "at the limit nothing goes");
        cache.insert("t:new".into(), (Some(99), 20));
        let mut gone = evict(&mut cache, 10, 4);
        gone.sort_unstable();
        assert_eq!(gone, [0, 2, 4, 6], "the oldest seven go; their images come back to be freed");
        let mut kept: Vec<&String> = cache.keys().collect();
        kept.sort();
        assert_eq!(kept, ["t:7", "t:8", "t:9", "t:new"]);
    }

    #[test]
    fn clipboard_images_are_made_once_and_the_unused_ones_freed() {
        let (mut cache, mut tick, mut made) = (HashMap::new(), 0, 0);
        for i in 0..12u32 {
            used(&mut cache, &mut tick, format!("t:{i}"), || Some(i));
        }
        assert_eq!(used(&mut cache, &mut tick, "t:0".into(), || { made += 1; Some(0) }), Some(0), "cached");
        assert_eq!(made, 0, "not made again");
        let mut gone = evict(&mut cache, 10, 4);
        gone.sort_unstable();
        assert_eq!(gone, (1..=8).collect::<Vec<_>>(), "the ones not used lately come back to be freed");
        assert!(cache.contains_key("t:0"), "used last, kept");
    }

    #[test]
    fn far_rows_are_not_decoded() {
        assert!(near(0, 0) && near(12, 0) && !near(13, 0));
        assert!(near(28, 40) && !near(27, 40) && near(52, 40));
    }

    #[test]
    fn emoji_grid_rows_break_at_sections_and_columns() {
        let it = |sec: &str| Item::new("x", "", Icon::glyph("x"), Act::None).sec(sec);
        let items: Vec<Item> = (0..6).map(|_| it("Smileys")).chain((0..2).map(|_| it("People"))).collect();
        let shape: Vec<String> = grid_rows(&items, 4).iter().map(|r| match r { GridRow::Header(h) => h.clone(), GridRow::Cells(c) => format!("{c:?}") }).collect();
        assert_eq!(shape, ["Smileys", "[0, 1, 2, 3]", "[4, 5]", "People", "[6, 7]"]);
        let searched: Vec<Item> = (0..5).map(|_| Item::new("x", "", Icon::glyph("x"), Act::None)).collect();
        assert_eq!(grid_rows(&searched, 4).len(), 2, "no sections: just rows");
    }

    #[test]
    fn running_is_always_headed_and_results_are_headed_below_another_section() {
        let it = |sec: &str| Item::new("x", "", Icon::glyph("x"), Act::None).sec(sec);
        assert_eq!(display_rows(&[it("Running")], false), [None, Some(0)], "alone, still headed");
        assert_eq!(display_rows(&[it("Results"), it("Running"), it("Results")], false), [Some(0), None, Some(1), None, Some(2)]);
        assert_eq!(display_rows(&[it("Results"), it("Results")], false), [Some(0), Some(1)]);
        // My Schedule heads even a lone section; root heads a lone Next.
        assert_eq!(display_rows(&[it("Today"), it("Today")], true), [None, Some(0), Some(1)]);
        assert_eq!(display_rows(&[it("Today")], false), [Some(0)]);
        assert_eq!(display_rows(&[it(NEXT)], false), [None, Some(0)]);
    }

    #[test]
    fn define_view_groups_senses_offers_spellings_and_says_when_nothing_is_found() {
        let sense = |pos, gloss: &str, synonyms: &[&str]| dict::Sense { pos, gloss: gloss.into(), examples: vec![], synonyms: synonyms.iter().map(|s| s.to_string()).collect() };
        let view = |word: &str, answer| DefView { word: word.into(), fetch: false, answer: Some(Ok(answer)) };
        let web = "https://www.google.com/search?q={query}";

        let found = dict::Answer { senses: vec![sense(dict::Pos::Adj, "lucky", &[]), sense(dict::Pos::Noun, "good luck", &["fluke", "luck"])], maybe: vec![("serendipitous".into(), dict::Pos::Adj)] };
        let rows = define_rows(&view("serendipity", Some(found)), web);
        let shape: Vec<(&str, Option<&str>)> = rows.iter().map(|it| (it.title.as_str(), it.section.as_deref())).collect();
        assert_eq!(shape, [("good luck", Some("Noun")), ("lucky", Some("Adjective")), ("serendipitous", Some(DID_YOU_MEAN))]);
        assert_eq!(rows[0].tint, Some(design::Tint::Web));
        let labels: Vec<String> = rows[0].actions().into_iter().map(|a| a.0).collect();
        assert_eq!(labels, ["Copy Definition", "Copy Word", "Define Similar Word", "Open in Browser"]);
        assert_eq!(rows[0].act, Act::Copy("good luck".into()), "Enter copies the gloss");
        assert_eq!(rows[0].actions()[2].1, Act::SimilarWords(vec!["fluke".into(), "luck".into()]));
        assert_eq!(rows[0].actions()[3].1, Act::Open("https://en.wiktionary.org/wiki/serendipity".into()));
        assert!(!rows[1].actions().iter().any(|a| a.0 == "Define Similar Word"), "no synonyms, no menu");
        assert_eq!(rows[2].act, Act::Define("serendipitous".into()));

        // Misspelled: Did you mean alone, still headed; its first row (the closest match) is where the selection starts.
        let rows = define_rows(&view("serendipty", Some(dict::Answer { senses: vec![], maybe: vec![("serendipity".into(), dict::Pos::Noun)] })), web);
        assert_eq!(display_rows(&rows, false), [None, Some(0)]);
        assert_eq!((rows[0].title.as_str(), rows[0].subtitle.as_str()), ("serendipity", "Noun"));

        // Unknown: the empty view, Enter searches the web.
        let rows = define_rows(&view("xyzq", Some(dict::Answer { senses: vec![], maybe: vec![] })), web);
        assert_eq!((rows[0].title.as_str(), rows[0].subtitle.as_str(), rows[0].section.as_deref()), ("No definition for 'xyzq'", "Check the spelling, or search the web", Some(EMPTY)));
        assert_eq!(rows[0].actions()[0], ("Search the Web".to_string(), Act::Open("https://www.google.com/search?q=xyzq".into())));

        // Not downloaded: the empty view's Enter downloads it.
        let rows = define_rows(&view("dog", None), web);
        assert_eq!((rows[0].title.as_str(), rows[0].act.clone()), ("Download Dictionary", Act::DownloadDictionary("dog".into())));
    }

    #[test]
    fn typing_in_the_define_view_asks_only_for_a_new_word() {
        // "dog" shown; "s" queues "dogs"; Backspace before the pause is back to "dog": nothing new
        // is queued (and the bump in `define_soon` outdates the "dogs" timer).
        assert_eq!(typed_word("dog", "dogs"), Some("dogs"));
        assert_eq!(typed_word("dog", "dog"), None);
        assert_eq!(typed_word("dog", " dog "), None);
        assert_eq!(typed_word("dog", "  "), None);
        // A trailing space asks for the word without it.
        assert_eq!(typed_word("ic", "ice "), Some("ice"));
    }

    #[test]
    fn the_root_define_row_names_its_word_only_with_one_typed() {
        let define = Item::new("Define", "", Icon::glyph("x"), Act::Cmd(Cmd::Define));
        assert_eq!(define_word(&define, &[" Dog ".into()]), Some("Dog"));
        assert_eq!(define_word(&define, &["  ".into()]), None);
        assert_eq!(define_word(&define, &[]), None);
        assert_eq!(define_word(&Item::new("Start Timer", "", Icon::glyph("x"), Act::Cmd(Cmd::StartTimer)), &["dog".into()]), None);
    }

    #[test]
    fn running_rows_pause_on_enter_stop_on_ctrl_enter_and_dismiss_when_done() {
        let row = |paused, done, stopwatch| running_row(timers::Row { id: 4, title: "tea".into(), stopwatch, paused, done, time: "4:12".into() });
        let labels = |it: &Item| it.actions().into_iter().map(|a| a.0).collect::<Vec<_>>();
        let it = row(false, false, false);
        assert_eq!((it.subtitle.as_str(), it.accessory.as_str(), it.section.as_deref(), it.live), ("Timer", "4:12 left", Some(RUNNING), true));
        assert!(matches!(it.icon, Icon::Vector(g::ALARM_CLOCK)) && matches!(row(false, false, true).icon, Icon::Vector(g::TIMER)));
        assert_eq!((it.act.clone(), it.actions()[1].1.clone()), (Act::PauseTimer(4), Act::StopTimer(4)));
        assert_eq!(labels(&it), ["Pause", "Stop", "Restart", "Rename", "Add 5 min"]);
        assert_eq!(it.extra[4].1, Act::AddTime(4, 300));
        let paused = row(true, false, false);
        assert_eq!((paused.act, paused.accessory.as_str()), (Act::ResumeTimer(4), "Paused · 4:12"));
        let done = row(false, true, false);
        assert_eq!((done.act.clone(), done.accessory.as_str(), labels(&done)), (Act::StopTimer(4), timers::DONE, vec!["Dismiss".to_string(), "Stop".into(), "Restart".into()]));
        assert_eq!(done.actions()[1].1, Act::StopTimer(4), "Ctrl+Enter stops a done timer, never restarts it");
        assert_eq!(timer_id(&done), Some(4));
    }

    #[test]
    fn an_alias_heads_root_over_a_keyword_and_a_lone_letter_keyword_leads_nothing() {
        let mut cfg = Config::default();
        cfg.launcher.aliases.insert(Cmd::Settings.key(), "t".into());
        let cat = search::catalog(&[], &[], &[], &cfg);
        let running = running_row(timers::Row { id: 1, title: "tea".into(), stopwatch: false, paused: false, done: false, time: "4:12".into() });
        let shown = |q: &str| {
            let found = search::root(q, &cat, &cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT));
            let rows = head_rows(vec![running.clone()], found, q, &cfg);
            let at = root_selection(&rows, q, &cfg, false);
            (rows, at)
        };
        let (rows, at) = shown("t");
        assert_eq!((rows[0].act.clone(), at), (Act::Cmd(Cmd::Settings), 0), "the alias, not Start Timer");
        let (rows, at) = shown("t 90");
        assert_eq!((rows[0].title.as_str(), at), ("Start Timer", 0), "a keyword with a value leads");
        let (rows, at) = shown("timer");
        assert_eq!((rows[0].title.as_str(), at), ("Start Timer", 0));
        cfg.launcher.aliases.clear();
        let cat = search::catalog(&[], &[], &[], &cfg);
        let found = search::root("t", &cat, &cfg, &Frecency::default(), &mut Matcher::new(nucleo_matcher::Config::DEFAULT));
        assert!(found.iter().all(|it| !search::leads(it, "t", &cfg)), "a bare one-letter keyword is just a letter");
        assert!(found.iter().any(|it| it.section.as_deref().is_some_and(|s| s.starts_with("Use "))), "so the fallbacks stay");
    }

    #[test]
    fn a_typed_query_starts_past_the_running_rows() {
        let cfg = Config::default();
        let running = running_row(timers::Row { id: 1, title: "tea".into(), stopwatch: false, paused: false, done: false, time: "4:12".into() });
        let rows = vec![running, Item::new("Terminal", "", Icon::glyph("x"), Act::None).sec("Results")];
        assert_eq!(root_selection(&rows, "te", &cfg, false), 1, "Enter on \"te\" opens Terminal, it does not pause tea");
        assert_eq!(root_selection(&rows, "", &cfg, false), 0, "an empty search starts on the top row");
        assert_eq!(root_selection(&rows[..1], "tea", &cfg, false), 0, "only Running rows: the first of them");
    }

    #[test]
    fn after_a_reminder_the_empty_search_starts_on_the_next_meeting() {
        let cfg = Config::default();
        let running = running_row(timers::Row { id: 1, title: "tea".into(), stopwatch: false, paused: false, done: false, time: "4:12".into() });
        let next = Item::new("Standup", "Work · Teams", Icon::glyph("x"), Act::Join("Standup".into(), "https://teams.live.com/meet/1".into())).sec(NEXT);
        let rows = vec![running, next, Item::new("Terminal", "", Icon::glyph("x"), Act::None).sec("Results")];
        assert_eq!(root_selection(&rows, "", &cfg, true), 1, "Enter joins");
        assert_eq!(root_selection(&rows, "", &cfg, false), 0, "no reminder: the top row");
        assert_eq!(root_selection(&rows, "te", &cfg, true), 2, "typing still starts on the results");
    }

    #[test]
    fn the_action_bar_names_a_row_by_its_kind_never_its_age() {
        let app = Item::new("Terminal", "", Icon::glyph("x"), Act::None).acc("Application".to_string());
        assert_eq!(row_footer(Some(&app)), "Application");
        for when in ["yesterday", "just now", "3 min ago"] {
            let clip = Item::new("text", "", Icon::glyph("x"), Act::Clip("k".into())).acc(when.to_string());
            assert_eq!(row_footer(Some(&clip)), "procast", "{when}");
        }
        assert_eq!(row_footer(None), "procast");
    }

    #[test]
    fn a_rebuild_keeps_the_selection_on_the_same_row() {
        let timer = |id, title: &str| running_row(timers::Row { id, title: title.into(), stopwatch: false, paused: false, done: false, time: "4:12".into() });
        let app = |title: &str, key: Option<&str>| {
            let mut it = Item::new(title, "", Icon::glyph("x"), Act::None).sec("Results");
            it.key = key.map(Into::into);
            it
        };
        let next = Item::new("Standup", "", Icon::glyph("x"), Act::Join("Standup".into(), "https://teams.live.com/meet/1".into())).sec(NEXT);
        let before = vec![timer(1, "Timer"), app("Terminal", Some("app:term")), app("Notes", None)];
        // A timer and a meeting came in above: the rows moved down, the selection follows them.
        let after = vec![next.clone(), timer(2, "Timer"), timer(1, "Timer"), app("Terminal", Some("app:term")), app("Notes", None)];
        assert_eq!(same_row(&after, &before[0]), Some(2), "the same timer, not the first one with its title");
        assert_eq!(same_row(&after, &before[1]), Some(3), "the same key");
        assert_eq!(same_row(&after, &before[2]), Some(4), "no key: the same title in the same section");
        assert_eq!(same_row(&after, &next), Some(0));
        assert_eq!(same_row(&before, &next), None, "gone: refresh's own choice stands");
        let mut elsewhere = app("Notes", None);
        elsewhere.section = Some("Commands".into());
        assert_eq!(same_row(&after, &elsewhere), None, "a title in another section is another row");
    }

    fn deep_work(minutes: u32) -> focus::Session {
        let mut s = focus::Session::new("Deep work", focus::Mode::Block, &[], &[], minutes, false, SystemTime::now());
        s.picks = vec!["cat:Messaging".into(), "cat:Social".into()];
        s
    }

    #[test]
    fn the_focus_row_heads_root_with_its_time_and_keys() {
        let now = SystemTime::now();
        let mut s = deep_work(25);
        let it = focus_row(&s, now);
        assert_eq!((it.title.as_str(), it.subtitle.as_str(), it.accessory.as_str(), it.section.as_deref()), ("Deep work", "Blocking 2 categories", "25:00 left", Some(FOCUS)));
        assert!(matches!(it.icon, Icon::Vector(g::SHIELD_CHECK)) && it.tint == Some(design::Tint::System) && it.live);
        let labels = |it: &Item| it.actions().into_iter().map(|a| a.0).collect::<Vec<_>>();
        assert_eq!(labels(&it), ["Pause", "Complete", "Edit", "Extend", "Restart", "Cancel"]);
        assert_eq!((it.act.clone(), it.actions()[1].1.clone(), it.bar_second.as_deref()), (Act::Cmd(Cmd::Focus(FocusCmd::Pause)), Act::Cmd(Cmd::Focus(FocusCmd::Complete)), Some("Complete")), "Enter pauses, Ctrl+Enter completes");
        s.pause(now);
        let paused = focus_row(&s, now);
        assert_eq!((paused.act, paused.accessory.as_str()), (Act::Cmd(Cmd::Focus(FocusCmd::Resume)), "Paused · 25:00"));
        assert!(!labels(&focus_row(&deep_work(0), now)).contains(&"Extend".to_string()), "no limit: nothing to extend");
        // Focus heads root above Running, headed even alone; a typed query starts past both.
        let running = running_row(timers::Row { id: 1, title: "tea".into(), stopwatch: false, paused: false, done: false, time: "4:12".into() });
        let rows = vec![it, running, Item::new("Terminal", "", Icon::glyph("x"), Act::None).sec("Results")];
        assert_eq!(display_rows(&rows[..1], false), [None, Some(0)]);
        assert_eq!(root_selection(&rows, "de", &Config::default(), false), 2);
    }

    #[test]
    fn the_focus_form_starts_from_its_seed_and_reads_back() {
        let picks = vec!["cat:Messaging".to_string(), "site:x.com".into()];
        let fields = focus_form("Deep work", "1 hour 30 minutes", focus::Mode::Allow, &picks);
        assert_eq!(fields.iter().map(|f| f.label.to_string()).collect::<Vec<_>>(), ["Goal", "Duration", "Mode", "Block"]);
        let v = form_values(&fields);
        assert_eq!((v["Goal"], v["Duration"], v["Mode"]), ("Deep work", "1 hour 30 minutes", "allow"));
        assert_eq!(focus::Picks::of(v["Block"]).0, picks);
        assert!(matches!(&fields[1].ctl, Ctl::Text { icon: Some(g::CLOCK), presets, .. } if presets.len() == 5));
        let menu = field_menu(&fields[1], gpui::Point::default()).unwrap();
        assert_eq!((menu.entries.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), menu.selected, menu.search.is_some()), (focus::DURATIONS.to_vec(), 3, false), "Down: the presets, on the current one");
        let Ctl::Select(modes, search) = &fields[2].ctl else { panic!("Mode is a select") };
        assert_eq!((modes.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), *search), (vec!["Block Apps & Websites", "Allow Apps & Websites"], true));
        assert!(field_menu(&fields[3], gpui::Point::default()).is_none(), "the token field opens the Block menu instead");
    }

    #[test]
    fn edit_opens_the_form_from_the_session_and_keeps_its_mode() {
        let cats = focus::categories(&[]);
        let strings = |a: [&str; 4]| a.map(String::from).to_vec();
        let values = |r: Option<Result<Vec<FormField>, String>>| r.map(|f| f.unwrap().into_iter().map(|f| f.value).collect::<Vec<_>>());
        let open = |f: FocusCmd, got: &[&str], s: Option<&focus::Session>| values(focus_form_for(f, &got.iter().map(|g| g.to_string()).collect::<Vec<_>>(), s, &cats, 25));
        let s = deep_work(50);
        let from_session = Some(strings(["Deep work", "50 min", "block", "cat:Messaging\ncat:Social"]));
        let seeded = focus::edit_seed(&s);
        let seeded: Vec<&str> = seeded.iter().map(String::as_str).collect();
        assert_eq!(open(FocusCmd::Edit, &seeded, Some(&s)), from_session, "Enter on the boxes as seeded opens the form; the clock does not restart");
        assert_eq!(open(FocusCmd::Edit, &["", "", ""], Some(&s)), from_session, "run on its own (Ctrl+K Edit, a hotkey)");
        assert_eq!(open(FocusCmd::Edit, &["Essay", "50", "Messaging, Social"], Some(&s)), None, "changed boxes apply at once");
        assert_eq!(open(FocusCmd::Edit, &["", "", ""], None), None, "no session: refused later");
        assert_eq!(open(FocusCmd::Start, &["", "50", ""], None), Some(strings(["", "50 min", "block", ""])));
        assert_eq!(open(FocusCmd::Start, &["Read", "", ""], None), None, "a goal starts at once");
        // An allow session with nothing picked stays an allow session.
        let open_allow = focus::Session::new("Open", focus::Mode::Allow, &[], &[], 25, false, SystemTime::now());
        assert_eq!(open(FocusCmd::Edit, &["", "", ""], Some(&open_allow)).map(|v| v[2].clone()).as_deref(), Some("allow"));
    }

    #[test]
    fn tab_in_a_form_text_box_moves_on_and_backspace_reaches_the_token_menu_first() {
        let keymap = gpui::Keymap::new(key_bindings());
        let actions = |key: &str, stack: &[&str]| {
            let stack: Vec<gpui::KeyContext> = stack.iter().map(|c| gpui::KeyContext::parse(c).unwrap()).collect();
            keymap.bindings_for_input(&[gpui::Keystroke::parse(key).unwrap()], &stack).0.iter().map(|b| b.action().name().rsplit("::").next().unwrap_or_default().to_string()).collect::<Vec<_>>()
        };
        let first = |key: &str, stack: &[&str]| actions(key, stack).first().cloned().unwrap_or_default();
        let input = DEFAULT_INPUT_CONTEXT;
        assert_eq!(first("tab", &["Form", input]), "NextBox", "Tab on Duration moves on; it never opens the presets");
        assert_eq!(first("shift-tab", &["Form", input]), "PrevBox");
        assert_eq!(first("down", &["Form", input]), "SelectNext", "Down still opens them");
        assert_eq!(first("tab", &[input]), "SelectNext", "the query keeps its Tab");
        assert_eq!(first("tab", &["Form", "TokenMenu", input]), "NextBox", "in a menu over a form too: its handler steps the open menu");
        assert_eq!(actions("backspace", &["Form", "TokenMenu", input])[..2], ["PopToken", "DeleteLeft"], "the token first, then (propagated) the text");
        assert_eq!(first("backspace", &["Form", input]), "DeleteLeft", "Goal's Backspace edits Goal");
        assert_eq!(first("backspace", &["Form"]), "PopToken", "on the token field itself");
        assert_eq!((first("left", &["Form"]), first("right", &["Form"])), ("SelectLeft".into(), "SelectRight".into()), "Left and Right move between swatches");
        assert_ne!(first("left", &["Form", input]), "SelectLeft", "a text box keeps its caret keys");
    }

    #[test]
    fn an_add_calendar_answer_drives_only_the_form_that_asked() {
        assert_eq!(add_answer(Some(1), 1, 1, true), (true, None), "the form on screen follows its answer");
        assert_eq!(add_answer(Some(1), 1, 1, false), (false, None), "left for root: no form to drive");
        // A was sent from form 1, Esc, form 2 sent B: A's answer only toasts and B still waits.
        assert_eq!(add_answer(Some(2), 1, 2, true), (false, Some(2)));
        assert_eq!(add_answer(Some(2), 2, 2, true), (true, None));
    }

    #[test]
    fn add_calendar_asks_for_the_address_first_then_a_name_and_a_colour() {
        let f = calendar_form();
        assert_eq!(f.iter().position(FormField::focusable), Some(0), "the address is focused first");
        assert!(matches!(&f[0].ctl, Ctl::Text { placeholder, icon: Some(g::LINK), .. } if placeholder.as_ref() == "https://calendar.google.com/…/basic.ics"));
        assert!(matches!(&f[1].ctl, Ctl::Text { placeholder, icon: None, .. } if placeholder.as_ref() == "Work"));
        assert!(matches!(f[2].ctl, Ctl::Swatches) && f[2].value == "system" && f[2].label.as_ref() == "Colour");
        assert!(matches!(f[3].ctl, Ctl::Note) && f[3].label.is_empty());
        assert_eq!(f[3].value, "Google: Settings → your calendar → Secret address in iCal format. Outlook: Settings → Shared calendars → Publish → ICS.");
        assert_eq!(form_values(&f).get("Colour"), Some(&"system"));
        assert_eq!(FormKind::AddCalendar.bar(), (g::CALENDAR_CLOCK, "Add Calendar", "Add Calendar"));
        assert_eq!(Act::AddCalendar.label(), "Add Calendar");
    }

    #[test]
    fn the_block_menu_lists_what_is_not_picked_and_a_typed_website() {
        let cats = focus::categories(&[]);
        let apps = vec![("discord.exe".to_string(), "Discord".to_string())];
        let picks = focus::Picks(vec!["cat:Messaging".into(), "cat:Social".into()]);
        let mut entries = block_entries(&cats, &apps, &picks);
        let m = menu::Menu::new(entries.clone(), Some("Search…".into()), menu::Place::Right);
        assert_eq!(m.rows()[0], menu::Row::Header("Focus Categories"));
        assert_eq!(entries.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Streaming", "Travel", "News", "Shopping", "Gaming", "Development", "Productivity", "Browsing", "Other", "Discord"], "picked ones leave the menu");
        assert_eq!((entries[0].id.as_str(), entries[0].icon, entries[9].id.as_str(), entries[9].section.as_deref()), ("cat:Streaming", Some(g::CIRCLE_PLAY), "app:discord.exe", Some("Applications")));
        site_lead(&mut entries, "Block", "Reddit.com");
        assert_eq!((entries[0].label.as_str(), entries[0].id.as_str(), entries[0].always), ("Block reddit.com", "site:reddit.com", true));
        site_lead(&mut entries, "Allow", "x.com");
        assert_eq!((entries[0].label.as_str(), entries.len()), ("Allow x.com", 11), "the typed row is replaced, never stacked");
        site_lead(&mut entries, "Block", "Steam.exe");
        assert_eq!((entries[0].label.as_str(), entries[0].id.as_str(), entries[0].icon), ("Block steam.exe", "app:steam.exe", Some(g::APP)), "a program, never a website");
        site_lead(&mut entries, "Block", "news");
        assert_eq!(entries.len(), 10, "not a website: no row");
    }

    #[test]
    fn album_grid_rows_follow_the_column_count() {
        let albums: Vec<Item> = (0..7).map(|_| Item::new("x", "", Icon::Vector(g::AUDIO), Act::None).sec("Albums")).collect();
        let shape = |cols| grid_rows(&albums, cols).iter().map(|r| match r { GridRow::Header(h) => h.clone(), GridRow::Cells(c) => c.len().to_string() }).collect::<Vec<_>>();
        assert_eq!(shape(3), ["Albums", "3", "3", "1"]);
        assert_eq!(shape(8), ["Albums", "7"]);
    }

    #[test]
    fn ai_chat_markdown_pins_the_newest_question() {
        let turn = |prompt: &str, text: &str| AiTurn { id: 0, prompt: prompt.into(), text: text.into() };
        let mut chat = AiChat::default();
        chat.turns.push(turn("first", "one\n```rust\nfn main() {"));
        chat.turns.push(turn("second", ""));
        let (md, newest) = chat.markdown();
        let lines = ext::markdown_lines(&md);
        assert_eq!(lines[newest], (3, "second".to_string()), "the pinned line is the newest question");
        assert_eq!(lines.last().unwrap(), &(0, String::new()));
        assert!(lines.contains(&(0, "Thinking…".to_string())), "an unanswered question shows a placeholder");
        assert!(lines.iter().any(|l| l == &(9, "fn main() {".to_string())), "code stays code");
        assert!(lines[newest..].iter().all(|l| l.0 != 9), "a cut-off code block does not swallow the next question");
    }

    #[test]
    fn screen_time_rows_and_their_actions() {
        let stats = json!({
            "total": 900, "previous": 600,
            "categories": [{"name": "Development", "seconds": 600}],
            "apps": [{"name": "code.exe", "seconds": 600, "category": "Development"}],
            "sites": [{"name": "youtube.com", "seconds": 200, "category": "Streaming"}]
        });
        let rows = screen_time_rows(wellbeing::Period { range: wellbeing::Range::Day, anchor: win::local_now().0 }, &stats);
        assert_eq!(rows[0].title, "15m today · 5m more than yesterday");
        assert_eq!(rows.iter().map(|r| r.section.as_deref()).collect::<Vec<_>>(), [None, Some("Categories"), Some("Apps"), Some("Sites")]);
        let labels = |it: &Item| it.actions().into_iter().map(|a| a.0).collect::<Vec<_>>();
        assert_eq!(labels(&rows[1]), ["Open Dashboard", "Export CSV"], "a category has nothing to move");
        assert_eq!((labels(&rows[2]), rows[2].act.clone()), (vec!["Move to Category…".into(), "Open Dashboard".into(), "Export CSV".into()], Act::MoveToCategory("code.exe".into())));
        assert_eq!((rows[3].title.as_str(), rows[3].subtitle.as_str(), rows[3].accessory.as_str()), ("youtube.com", "Streaming", "3m"));
        let off = screen_time_off_row();
        assert_eq!((off.len(), labels(&off[0])), (1, vec!["Import from micro bot".to_string(), "Turn On Screen Time".into()]));
        assert_eq!(off[0].act, Act::Cmd(Cmd::ImportScreenTime));
    }
}
