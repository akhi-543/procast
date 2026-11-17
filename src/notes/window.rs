//! The Notes window: one note at a time in a floating glass window, above the other windows
//! while it shows. The note store belongs to one background worker (`Notes`), so no store call
//! runs on the UI thread and the store's work happens in the order it was asked for.

use std::{
    io,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use futures::{
    StreamExt,
    channel::{
        mpsc::{self, UnboundedSender},
        oneshot,
    },
};
use gpui::{
    AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, DispatchPhase, Entity, FocusHandle, Focusable, FontStyle, FontWeight, HighlightStyle, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, ScrollHandle, SharedString, StrikethroughStyle, StyledText, Subscription,
    TextLayout, UnderlineStyle, Window, WindowControlArea, PaintQuad, canvas, div, point, prelude::*, size,
};
use gpui_ce_elements::editable_text::{BLINK_INTERVAL_500MS, EditableTextState, StringStorage, TextChanged, text_input};
use serde::{Deserialize, Serialize};

use super::{
    editor::{self, Block, Buffer, Mark, Span, SpanCache, Style},
    store::{Exists, Id, NoteMeta, Store, title_of},
};
use crate::{
    Dismiss, Launch, OpenDropdown, SelectNext, SelectPrev, ToggleActions, config,
    design::{self, Tint, Tokens, g, ic, mix, px, r, sp, ts},
    focus,
    menu::{self, Entry, Menu},
    timers,
    win::{self, Msg},
};

/// Quiet time after the last edit (or move) before the note (or the window's place) is written.
const SETTLE: Duration = Duration::from_millis(500);

type Job = Box<dyn FnOnce(&mut io::Result<Store>) + Send>;

/// The note store, owned by one task on the background executor that runs the jobs sent to it
/// one at a time, in order: a save always lands before the create or search asked after it.
/// Cloning gives another handle to the same store; the task ends when the last one goes.
#[derive(Clone)]
pub struct Notes(UnboundedSender<Job>);

impl Notes {
    /// Opens the store at `root` on the worker; if that fails, every job gets the error.
    pub fn start(root: PathBuf, cx: &App) -> Notes {
        let (tx, mut rx) = mpsc::unbounded::<Job>();
        cx.background_executor()
            .spawn(async move {
                let mut store = Store::open(root.clone());
                while let Some(job) = rx.next().await {
                    // A transient failure (a lock at startup) is retried; a damaged index stays an error.
                    if store.as_ref().is_err_and(|e| e.kind() != io::ErrorKind::InvalidData) {
                        store = Store::open(root.clone());
                    }
                    // A panicking job only drops its answer.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut store)));
                }
            })
            .detach();
        Notes(tx)
    }

    /// Queues `job`; its answer comes back on the receiver.
    fn ask<R: Send + 'static>(&self, job: impl FnOnce(&mut Store) -> io::Result<R> + Send + 'static) -> oneshot::Receiver<io::Result<R>> {
        let (tx, rx) = oneshot::channel();
        let _ = self.0.unbounded_send(Box::new(move |store: &mut io::Result<Store>| {
            let r = match store {
                Ok(s) => job(s),
                Err(e) => Err(io::Error::new(e.kind(), format!("the notes folder can't be opened: {e}"))),
            };
            let _ = tx.send(r);
        }));
        rx
    }

    /// Runs `job` on the worker, then `done` with its answer on the entity.
    pub fn run<T: 'static, R: Send + 'static>(
        &self,
        cx: &mut Context<T>,
        job: impl FnOnce(&mut Store) -> io::Result<R> + Send + 'static,
        done: impl FnOnce(&mut T, io::Result<R>, &mut Context<T>) + 'static,
    ) {
        let answer = self.ask(job);
        cx.spawn(async move |this, cx| {
            if let Ok(r) = answer.await {
                _ = this.update(cx, |this, cx| done(this, r, cx));
            }
        })
        .detach();
    }
}

/// The id `create` gave, or the empty note it found instead.
pub fn created(r: Result<Id, Exists>) -> Id {
    r.unwrap_or_else(|Exists(id)| id)
}

/// The footer's count: "1 character", "1,013 characters".
pub fn char_count_text(n: usize) -> String {
    if n == 1 { "1 character".into() } else { format!("{} characters", design::thousands(n)) }
}

/// A note's second line in Browse Notes and Search Notes: "● Current · 186 characters" for the
/// note on screen (`current`: its length with its edits), else "Opened 2 days ago · 397
/// characters" ("Edited …" for a note never opened).
pub fn note_line(m: &NoteMeta, n: usize, current: Option<usize>, now: i64) -> String {
    let ago = |t: i64| timers::ago(now.saturating_sub(t).max(0) as u64);
    match current {
        Some(n) => format!("● Current · {}", char_count_text(n)),
        None if m.opened > 0 => format!("Opened {} · {}", ago(m.opened), char_count_text(n)),
        None => format!("Edited {} · {}", ago(m.modified), char_count_text(n)),
    }
}

fn now() -> i64 {
    crate::search::now() as i64
}

/// How the window came to the note it shows, for its history.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Nav {
    Visit,
    Back,
    Forward,
    /// The note that was on screen was deleted.
    Gone,
}

/// Go Back and Go Forward: the notes this window showed before and after the one on screen.
/// No step ever leads to the note on screen, and no two steps in a row lead to the same note.
#[derive(Default)]
struct History {
    back: Vec<Id>,
    forward: Vec<Id>,
}

impl History {
    /// The window went from note `from` to `to`. A visit starts a new way forward; Back and
    /// Forward step along the way; after `Gone` the deleted `from` is forgotten.
    fn went(&mut self, from: Option<Id>, to: &str, nav: Nav) {
        let Some(from) = from.filter(|f| f != to) else { return };
        let (taken, other) = match nav {
            Nav::Visit => {
                self.forward.clear();
                return self.back.push(from);
            }
            Nav::Gone => return self.forget(&from, to),
            Nav::Back => (&mut self.back, &mut self.forward),
            Nav::Forward => (&mut self.forward, &mut self.back),
        };
        if taken.last().is_some_and(|l| l == to) {
            taken.pop();
        }
        other.push(from);
    }

    /// Where Go Back (`Nav::Back`) or Go Forward leads; None: that action is off.
    fn next(&self, nav: Nav) -> Option<&Id> {
        match nav {
            Nav::Back => self.back.last(),
            Nav::Forward => self.forward.last(),
            _ => None,
        }
    }

    /// Note `gone` was deleted: it leaves both ways, as does every step to `current`.
    fn forget(&mut self, gone: &str, current: &str) {
        for way in [&mut self.back, &mut self.forward] {
            way.retain(|id| id != gone && id != current);
            way.dedup();
        }
    }
}

/// The Ctrl+K list (also the header's Actions); its ids are `NotesWindow::act`'s.
fn actions(pinned: bool, h: &History) -> Vec<Entry> {
    let e = |id: &str, label: &str, icon: &'static str, keys: &'static str| Entry { id: id.into(), label: label.into(), icon: Some(icon), keys, ..Default::default() };
    vec![
        e("new", "New Note", g::PLUS, "Ctrl+N"),
        e("duplicate", "Duplicate Note", g::COPY, ""),
        e("pin", if pinned { "Unpin Note" } else { "Pin Note" }, g::PIN, "Ctrl+Shift+P"),
        e("browse", "Browse Notes", g::NOTEBOOK_TABS, "Ctrl+P"),
        Entry { off: h.next(Nav::Back).is_none(), ..e("back", "Go Back", g::BACK, "Ctrl+[") },
        Entry { off: h.next(Nav::Forward).is_none(), ..e("forward", "Go Forward", g::FORWARD, "Ctrl+]") },
        e("copy", "Copy as Markdown", g::COPY, ""),
        Entry { danger: true, ..e("delete", "Delete Note", g::DELETE, "") },
    ]
}

/// Browse Notes' last row: the launcher's Recently Deleted Notes.
const DELETED: &str = "recently-deleted";

/// Browse Notes' rows in the store's order (pinned first, then last opened), each note on two
/// lines, then Recently Deleted. `current`: the note on screen, its title and length counted
/// with its edits. The store has searched them (their text too), so each shows whatever is typed.
fn browse_entries(rows: &[(NoteMeta, usize)], current: Option<(&str, String, usize)>, now: i64) -> Vec<Entry> {
    let mut v: Vec<Entry> = rows
        .iter()
        .map(|(m, n)| {
            let cur = current.as_ref().filter(|c| c.0 == m.id);
            Entry {
                id: m.id.clone(),
                label: cur.map_or_else(|| m.title.clone(), |c| c.1.clone()),
                section: Some("Notes".into()),
                two_line: Some(note_line(m, *n, cur.map(|c| c.2), now)),
                always: true,
                ..Default::default()
            }
        })
        .collect();
    if let Some(last) = v.last_mut() {
        last.rule_after = true;
    }
    v.push(Entry { id: DELETED.into(), label: "Recently Deleted".into(), icon: Some(g::ARCHIVE_RESTORE), always: true, ..Default::default() });
    v
}

/// The window's own keys, wherever the keys are (a menu's search field too): an `act` id.
fn shortcut(k: &Keystroke) -> Option<&'static str> {
    let m = k.modifiers;
    if !m.control || m.alt || m.platform {
        return None;
    }
    Some(match (k.key.as_str(), m.shift) {
        ("k", false) => "actions",
        ("p", false) => "browse",
        ("n", false) => "new",
        ("p", true) => "pin",
        ("[", false) => "back",
        ("]", false) => "forward",
        _ => return None,
    })
}

/// A formatting bar item, or an entry of its menus, applied to the note; false for an unknown id.
fn restyle(b: &mut Buffer, id: &str) -> bool {
    match id {
        "h1" => b.set_block(Block::H1),
        "h2" => b.set_block(Block::H2),
        "h3" => b.set_block(Block::H3),
        "p" => b.set_block(Block::Paragraph),
        "quote" => b.set_block(Block::Quote),
        "task" => b.set_block(Block::Task),
        "bullet" => b.set_block(Block::Bullet),
        "numbered" => b.set_block(Block::Numbered),
        "bold" => b.toggle_mark(Mark::Bold),
        "italic" => b.toggle_mark(Mark::Italic),
        "underline" => b.toggle_mark(Mark::Underline),
        "strike" => b.toggle_mark(Mark::Strike),
        "highlight" => b.toggle_mark(Mark::Highlight),
        "code" => b.toggle_mark(Mark::Code),
        "link" => b.link(false),
        "image" => b.link(true),
        _ => return false,
    }
    true
}

/// A formatting bar menu: (id, label, icon, keys) per entry.
fn bar_menu(items: &[(&str, &str, Option<&'static str>, &'static str)]) -> Vec<Entry> {
    items.iter().map(|&(id, label, icon, keys)| Entry { id: id.into(), label: label.into(), icon, keys, ..Default::default() }).collect()
}

fn heading_menu() -> Vec<Entry> {
    bar_menu(&[("h1", "Heading 1", None, "Ctrl+Alt+1"), ("h2", "Heading 2", None, "Ctrl+Alt+2"), ("h3", "Heading 3", None, "Ctrl+Alt+3"), ("p", "Paragraph", None, "Ctrl+Alt+0")])
}

fn style_menu() -> Vec<Entry> {
    bar_menu(&[
        ("bold", "Bold", None, "Ctrl+B"),
        ("italic", "Italic", None, "Ctrl+I"),
        ("underline", "Underline", None, "Ctrl+U"),
        ("strike", "Strikethrough", None, "Ctrl+Shift+S"),
        ("highlight", "Highlight", None, "Ctrl+Shift+M"),
    ])
}

fn list_menu() -> Vec<Entry> {
    bar_menu(&[("bullet", "Bulleted List", Some(g::BULLETS), ""), ("numbered", "Numbered List", Some(g::LIST_ORDERED), "")])
}

/// The menus the window opens.
#[derive(Clone, Copy, PartialEq)]
enum Pop {
    Actions,
    Browse,
    Heading,
    Style,
    List,
}

struct OpenMenu {
    pop: Pop,
    menu: Menu,
    scroll: ScrollHandle,
}

/// Where the window was, logical px: saved to `notes-window.json` on move, resize and hide.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Place {
    fn of(b: Bounds<Pixels>) -> Place {
        Place { x: b.origin.x.into(), y: b.origin.y.into(), w: b.size.width.into(), h: b.size.height.into() }
    }

    pub fn bounds(self) -> Bounds<Pixels> {
        Bounds::new(gpui::point(gpui::px(self.x), gpui::px(self.y)), gpui::size(gpui::px(self.w), gpui::px(self.h)))
    }
}

pub fn place_path() -> PathBuf {
    config::dir().join("notes-window.json")
}

/// The saved place, when the file holds one with a size.
pub fn load_place(path: &Path) -> Option<Place> {
    let text = focus::text_or_empty(std::fs::read_to_string(path)).ok()?;
    serde_json::from_str::<Place>(&text).ok().filter(|p| p.w >= 1. && p.h >= 1.)
}

fn save_place(path: &Path, place: Place) -> io::Result<()> {
    focus::write_atomic(path, &serde_json::to_string(&place).map_err(io::Error::other)?)
}

/// The note on screen: its id, the text being edited, and the text the file has (None when
/// unknown: the last write failed).
struct Doc {
    id: Id,
    buffer: Buffer,
    saved: Option<String>,
    /// Pinned in the store (asked for once it shows; Pin Note keeps it).
    pinned: bool,
}

impl Doc {
    fn new(id: Id, text: String) -> Doc {
        Doc { id, buffer: Buffer::new(&text), saved: Some(text), pinned: false }
    }

    /// The note and its text, when the file doesn't hold that text yet.
    fn unsaved(&self) -> Option<(Id, String)> {
        (self.saved.as_deref() != Some(self.buffer.text())).then(|| (self.id.clone(), self.buffer.text().to_owned()))
    }
}

/// Whether a read of note `id` replaces the doc on screen. Another note does. The same note keeps
/// its buffer, since the read may be older than it, unless the reader itself just wrote that
/// text (`wrote`: Create Note's title into the empty note) and the buffer is empty and saved.
fn replaces(doc: Option<&Doc>, id: &str, wrote: bool) -> bool {
    doc.is_none_or(|d| d.id != id || (wrote && d.unsaved().is_none() && d.buffer.text().trim().is_empty()))
}

/// Writing `text` to note `id` failed. If that note is still on screen it is marked unsaved (its
/// next settle or hide tries again); otherwise the write comes back, to be tried once more.
fn failed(doc: Option<&mut Doc>, id: Id, text: String) -> Option<(Id, String)> {
    match doc {
        Some(d) if d.id == id => {
            d.saved = None;
            None
        }
        _ => Some((id, text)),
    }
}

/// What an editor key did. The window finishes the clipboard ones: `Copy` goes to the
/// clipboard (a cut has already left the note), `Paste` inserts the clipboard's text.
#[derive(Debug, PartialEq)]
enum Did {
    Moved,
    Edited,
    Copy { text: String, cut: bool },
    Paste,
}

/// Applies a key to the note; `None` when the key is not the editor's.
fn press(b: &mut Buffer, k: &Keystroke) -> Option<Did> {
    let m = k.modifiers;
    let (shift, plain, cmd) = (m.shift, !m.control && !m.alt, m.control && !m.alt);
    // Ctrl+Alt is also AltGr: a key that types a character with it types it.
    let chord = m.control && m.alt && k.key_char.is_none();
    let end = b.text().len();
    Some(match k.key.as_str() {
        "enter" if plain => {
            b.newline();
            Did::Edited
        }
        "backspace" if plain => {
            b.backspace();
            Did::Edited
        }
        "delete" if plain => {
            b.delete();
            Did::Edited
        }
        "left" | "right" | "up" | "down" | "home" | "end" if !m.alt => {
            match (k.key.as_str(), m.control) {
                ("left", false) => b.left(shift),
                ("left", true) => b.word_left(shift),
                ("right", false) => b.right(shift),
                ("right", true) => b.word_right(shift),
                ("up", _) => b.up(shift),
                ("down", _) => b.down(shift),
                ("home", false) => b.home(shift),
                ("home", true) => b.set_cursor(0, shift),
                ("end", false) => b.end(shift),
                _ => b.set_cursor(end, shift),
            }
            Did::Moved
        }
        "a" if cmd && !shift => {
            b.set_cursor(0, false);
            b.set_cursor(end, true);
            Did::Moved
        }
        "c" | "x" if cmd => {
            let (from, to) = b.selection();
            if from == to {
                return Some(Did::Moved);
            }
            let text = b.text()[from..to].to_owned();
            let cut = k.key == "x";
            if cut {
                b.backspace();
            }
            Did::Copy { text, cut }
        }
        "v" if cmd => Did::Paste,
        "z" if cmd => {
            if shift { b.redo() } else { b.undo() }
            Did::Edited
        }
        "y" if cmd && !shift => {
            b.redo();
            Did::Edited
        }
        "b" if cmd => {
            if shift { b.set_block(Block::Quote) } else { b.toggle_mark(Mark::Bold) }
            Did::Edited
        }
        "i" | "u" | "e" if cmd && !shift => {
            b.toggle_mark(match k.key.as_str() {
                "i" => Mark::Italic,
                "u" => Mark::Underline,
                _ => Mark::Code,
            });
            Did::Edited
        }
        "s" | "m" if cmd && shift => {
            b.toggle_mark(if k.key == "s" { Mark::Strike } else { Mark::Highlight });
            Did::Edited
        }
        "c" | "0" | "1" | "2" | "3" if chord => {
            b.set_block(match k.key.as_str() {
                "c" => Block::Code,
                "0" => Block::Paragraph,
                "1" => Block::H1,
                "2" => Block::H2,
                _ => Block::H3,
            });
            Did::Edited
        }
        _ => {
            // ponytail: key_char only, EntityInputHandler for IME
            let text = k.key_char.as_deref().filter(|_| plain || (m.control && m.alt))?;
            b.insert(text);
            Did::Edited
        }
    })
}

/// Clipboard text as the note keeps it: `\n` line ends.
fn pasted(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// A line as drawn, for the mouse: where it starts in the note, how many display bytes it has,
/// its display-to-source map (`None`: drawn as typed), and its laid-out text.
struct Row {
    start: usize,
    len: usize,
    map: Option<Rc<[usize]>>,
    layout: TextLayout,
}

pub struct NotesWindow {
    notes: Notes,
    tx: UnboundedSender<Msg>,
    pub theme: config::Theme,
    doc: Option<Doc>,
    /// Bumped by every edit and every move: a settle timer acts only if nothing came after it.
    edits: u64,
    moves: u64,
    focus: FocusHandle,
    cache: SpanCache,
    /// The lines the last frame drew.
    rows: Vec<Row>,
    scroll: ScrollHandle,
    /// A press in the body is dragging out a selection.
    selecting: bool,
    /// The caret's blink phase, and the blink loop's generation (bumping it stops the loop).
    caret_on: bool,
    blink: u64,
    history: History,
    /// An open menu; it takes Up, Down, Enter and Esc. Its search text is `menu_input`.
    menu: Option<OpenMenu>,
    menu_input: Entity<EditableTextState>,
    /// Browse Notes' search generation: an answer is shown only if nothing newer was asked.
    browse_gen: u64,
    /// The footer's Text style toggle is on: the formatting bar shows instead.
    bar: bool,
    /// Delete Note asks first: the note and its title.
    confirm: Option<(Id, String)>,
    _subs: Vec<Subscription>,
}

impl NotesWindow {
    pub fn new(notes: Notes, tx: UnboundedSender<Msg>, theme: config::Theme, window: &mut Window, cx: &mut Context<Self>) -> Self {
        win::round_corners(win::hwnd(window));
        let menu_input = cx.new(|cx| EditableTextState::new(StringStorage::default(), cx));
        let subs = vec![
            cx.subscribe(&menu_input, |this, input, _: &TextChanged, cx| {
                let q = input.read(cx).as_str().to_owned();
                let Some(open) = &mut this.menu else { return };
                open.menu.set_filter(&q);
                if open.pop == Pop::Browse {
                    this.browse(false, cx);
                }
                cx.notify();
            }),
            cx.observe_window_bounds(window, |this, window, cx| this.moved(window, cx)),
            // The caret blinks only while the window is active.
            cx.observe_window_activation(window, |this, window, cx| if window.is_window_active() { this.wake(cx) } else { this.blink += 1 }),
            // Quitting writes what the last settle didn't (the worker runs it after any save before it).
            cx.on_app_quit(|this, _| {
                let written = this.doc.as_ref().and_then(Doc::unsaved).map(|(id, text)| this.notes.ask(move |s| s.save(&id, &text, now())));
                async move {
                    if let Some(w) = written {
                        let _ = w.await;
                    }
                }
            }),
        ];
        // Alt+F4 and the taskbar's Close hide the window like Esc: its note and place stay.
        let me = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            _ = me.update(cx, |this, cx| this.hide(window, cx));
            false
        });
        NotesWindow {
            notes,
            tx,
            theme,
            doc: None,
            edits: 0,
            moves: 0,
            focus: cx.focus_handle(),
            cache: SpanCache::default(),
            rows: Vec::new(),
            scroll: ScrollHandle::new(),
            selecting: false,
            caret_on: true,
            blink: 0,
            history: History::default(),
            menu: None,
            menu_input,
            browse_gen: 0,
            bar: false,
            confirm: None,
            _subs: subs,
        }
    }

    pub fn has_note(&self) -> bool {
        self.doc.is_some()
    }

    /// The note on screen and its length in characters, edits included.
    pub fn current(&self) -> Option<(Id, usize)> {
        self.doc.as_ref().map(|d| (d.id.clone(), d.buffer.text().chars().count()))
    }

    /// Shows note `id` with `text` as the store had it (see `replaces`), saving the note on
    /// screen first.
    pub fn show(&mut self, id: Id, text: String, wrote: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.switch(id, text, wrote, Nav::Visit, cx);
        self.reveal(window, cx);
    }

    /// Note `id` with `text` from the store goes on screen unless `replaces` keeps the buffer;
    /// the note there is written first and `nav` is the step history records. Its pin is asked
    /// for, and an open Browse Notes lists it as Current.
    fn switch(&mut self, id: Id, text: String, wrote: bool, nav: Nav, cx: &mut Context<Self>) {
        if replaces(self.doc.as_ref(), &id, wrote) {
            self.save(cx);
            self.history.went(self.doc.as_ref().map(|d| d.id.clone()), &id, nav);
            self.doc = Some(Doc::new(id.clone(), text));
            let ask = id.clone();
            self.notes.run(cx, move |s| Ok(s.list().iter().any(|m| m.id == ask && m.pinned)), move |this, r, cx| {
                if let (Ok(p), Some(d)) = (r, this.doc.as_mut().filter(|d| d.id == id)) {
                    d.pinned = p;
                    cx.notify();
                }
            });
            self.browse(true, cx);
        }
        cx.notify();
    }

    /// Writes the note on screen, then `pick`s a note on the store's worker, marks it opened and
    /// shows it (`nav`: the step history records). `failed` starts the error toast.
    fn go(&mut self, pick: impl FnOnce(&mut Store, i64) -> io::Result<Id> + Send + 'static, nav: Nav, failed: &'static str, cx: &mut Context<Self>) {
        self.save(cx);
        let at = now();
        self.notes.run(
            cx,
            move |s| {
                let id = pick(s, at)?;
                s.open_note(&id, at)?;
                Ok((s.text(&id)?, id))
            },
            move |this, r, cx| match r {
                // Keys typed while the store worked are written before another note shows.
                Ok((text, id)) => this.switch(id, text, false, nav, cx),
                Err(e) => _ = this.tx.unbounded_send(Msg::error(format!("{failed}: {e}"))),
            },
        );
    }

    /// An action by id: the Ctrl+K list's, a window shortcut's, a header button's.
    fn act(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match id {
            "actions" => return self.toggle_pop(Pop::Actions, window, cx),
            "browse" => return self.toggle_pop(Pop::Browse, window, cx),
            _ => self.close_menu(window, cx),
        }
        let current = self.doc.as_ref().map(|d| d.id.clone());
        match (id, current) {
            ("new", _) => self.new_note(cx),
            ("duplicate", Some(id)) => self.go(move |s, at| s.duplicate(&id, at), Nav::Visit, "Couldn't duplicate the note", cx),
            ("pin", Some(id)) => self.toggle_pin(id, cx),
            ("back" | "forward", Some(_)) => {
                let nav = if id == "back" { Nav::Back } else { Nav::Forward };
                if let Some(to) = self.history.next(nav).cloned() {
                    self.go(move |_, _| Ok(to), nav, "Couldn't open the note", cx);
                }
            }
            ("copy", Some(_)) => {
                let text = self.doc.as_ref().map_or_else(String::new, |d| d.buffer.text().to_owned());
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                _ = self.tx.unbounded_send(Msg::done("Copied as Markdown"));
            }
            ("delete", Some(id)) => {
                let title = self.doc.as_ref().map_or_else(String::new, |d| title_of(d.buffer.text()));
                self.ask_delete(id, title, window, cx);
            }
            _ => {}
        }
    }

    /// Opens menu `pop`, or closes it when it is the one open.
    fn toggle_pop(&mut self, pop: Pop, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.as_ref().is_some_and(|o| o.pop == pop) {
            return self.close_menu(window, cx);
        }
        let pinned = self.doc.as_ref().is_some_and(|d| d.pinned);
        let above = menu::Place::Above(px(design::CONTROL + sp::S4));
        let (entries, search, place) = match pop {
            Pop::Actions => (actions(pinned, &self.history), Some("Search for actions…"), menu::Place::Center(px(design::POP_CENTER_TOP))),
            Pop::Browse => (vec![], Some("Search notes…"), menu::Place::Center(px(design::POP_CENTER_TOP_HEAD))),
            Pop::Heading => (heading_menu(), None, above),
            Pop::Style => (style_menu(), None, above),
            Pop::List => (list_menu(), None, above),
        };
        self.confirm = None;
        self.menu = Some(OpenMenu { pop, menu: Menu::new(entries, search.map(Into::into), place), scroll: ScrollHandle::new() });
        if search.is_some() {
            self.menu_input.update(cx, |s, cx| s.emplace("", cx));
            window.focus(&self.menu_input.focus_handle(cx), cx);
        } else {
            window.focus(&self.focus, cx);
        }
        self.browse(false, cx);
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Up, Down, Enter and Esc while the delete confirm or a menu is open.
    fn key(&mut self, k: menu::Key, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, _)) = &self.confirm {
            let id = id.clone();
            match k {
                menu::Key::Enter => {
                    self.close_confirm(window, cx);
                    self.delete(id, cx);
                }
                menu::Key::Esc => self.close_confirm(window, cx),
                _ => {}
            }
            return;
        }
        let Some(open) = &mut self.menu else { return };
        match open.menu.key(k) {
            menu::Event::Picked(i) => self.pick(i, window, cx),
            menu::Event::Closed => self.close_menu(window, cx),
            menu::Event::None => {
                if let Some(r) = open.menu.selected_row() {
                    open.scroll.scroll_to_item(r);
                }
                cx.notify();
            }
        }
    }

    /// Entry `i` of the open menu was chosen: the menu closes and the entry acts.
    fn pick(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu.take() else { return };
        let Some(id) = open.menu.entries.get(i).map(|e| e.id.clone()) else { return };
        window.focus(&self.focus, cx);
        cx.notify();
        match open.pop {
            Pop::Actions => self.act(&id, window, cx),
            Pop::Browse if id == DELETED => {
                self.hide(window, cx);
                _ = self.tx.unbounded_send(Msg::RecentlyDeletedNotes);
            }
            Pop::Browse => self.go(move |_, _| Ok(id), Nav::Visit, "Couldn't open the note", cx),
            Pop::Heading | Pop::Style | Pop::List => self.restyle_note(&id, cx),
        }
    }

    /// A formatting bar item or one of its menus' entries, on the note.
    fn restyle_note(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        if restyle(&mut doc.buffer, id) {
            self.changed(true, cx);
        }
    }

    /// Runs Browse Notes' search on the store's worker when it is open; the answer fills it if
    /// nothing newer was asked (`keep`: the selected note stays selected).
    fn browse(&mut self, keep: bool, cx: &mut Context<Self>) {
        if !self.menu.as_ref().is_some_and(|o| o.pop == Pop::Browse) {
            return;
        }
        self.browse_gen += 1;
        let (generation, q) = (self.browse_gen, self.menu_input.read(cx).as_str().to_owned());
        self.notes.run(cx, move |s| Ok(s.browse(false, &q)), move |this, r, cx| {
            if this.browse_gen != generation {
                return;
            }
            let rows = match r {
                Ok(rows) => rows,
                Err(e) => {
                    _ = this.tx.unbounded_send(Msg::error(format!("Couldn't read the notes: {e}")));
                    return;
                }
            };
            let current = this.doc.as_ref().map(|d| (d.id.as_str(), title_of(d.buffer.text()), d.buffer.text().chars().count()));
            let entries = browse_entries(&rows, current, now());
            let Some(open) = this.menu.as_mut().filter(|o| o.pop == Pop::Browse) else { return };
            let was = open.menu.picked().map(|e| e.id.clone()).filter(|_| keep);
            open.menu.set_entries(entries);
            if let Some(i) = was.and_then(|id| open.menu.entries.iter().position(|e| e.id == id)) {
                open.menu.selected = i;
            }
            cx.notify();
        });
    }

    /// Pin Note / Unpin Note, on the note on screen or a Browse Notes row.
    fn toggle_pin(&mut self, id: Id, cx: &mut Context<Self>) {
        let ask = id.clone();
        self.notes.run(
            cx,
            move |s| {
                let on = !s.list().iter().any(|m| m.id == ask && m.pinned);
                s.pin(&ask, on)?;
                Ok(on)
            },
            move |this, r, cx| match r {
                Ok(on) => {
                    if let Some(d) = this.doc.as_mut().filter(|d| d.id == id) {
                        d.pinned = on;
                    }
                    this.browse(true, cx);
                    _ = this.tx.unbounded_send(Msg::done(if on { "Note pinned" } else { "Note unpinned" }));
                    cx.notify();
                }
                Err(e) => _ = this.tx.unbounded_send(Msg::error(format!("Couldn't pin the note: {e}"))),
            },
        );
    }

    /// Delete Note (and a Browse Notes row's Delete) asks first.
    fn ask_delete(&mut self, id: Id, title: String, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = Some((id, title));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The confirm closes; the keys go back to the menu's search, or the note.
    fn close_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = None;
        let searching = self.menu.as_ref().is_some_and(|o| o.menu.search.is_some());
        let handle = if searching { self.menu_input.focus_handle(cx) } else { self.focus.clone() };
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Moves note `id` to Recently Deleted. The note on screen gives way to the one opened last
    /// (a new one when none is left); history forgets it either way.
    fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.doc.as_ref().is_some_and(|d| d.id == id) {
            return self.go(
                move |s, at| {
                    s.delete(&id, at)?;
                    s.last().map_or_else(|| s.create(at).map(created), Ok)
                },
                Nav::Gone,
                "Couldn't delete the note",
                cx,
            );
        }
        self.save(cx);
        let (gone, at) = (id.clone(), now());
        self.notes.run(cx, move |s| s.delete(&gone, at), move |this, r, cx| match r {
            Ok(()) => {
                let current = this.doc.as_ref().map_or_else(String::new, |d| d.id.clone());
                this.history.forget(&id, &current);
                this.browse(true, cx);
            }
            Err(e) => _ = this.tx.unbounded_send(Msg::error(format!("Couldn't delete the note: {e}"))),
        });
    }

    /// Brings the window up, above the others, with the keys.
    pub fn reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_visible(true);
        window.activate();
        window.focus(&self.focus, cx);
        win::set_topmost(win::hwnd(window), true);
        self.wake(cx);
        cx.notify();
    }

    /// Shows the caret and (re)starts its blink.
    fn wake(&mut self, cx: &mut Context<Self>) {
        self.caret_on = true;
        self.blink += 1;
        let n = self.blink;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL_500MS).await;
                let live = this.update(cx, |this, cx| {
                    let live = this.blink == n;
                    if live {
                        this.caret_on = !this.caret_on;
                        cx.notify();
                    }
                    live
                });
                if live.ok() != Some(true) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Esc, ×, Alt+F4: the note and the window's place are written, the window hides.
    pub fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save(cx);
        let place = Place::of(window.window_bounds().get_bounds());
        cx.background_executor().spawn(async move { drop(save_place(&place_path(), place)) }).detach();
        self.blink += 1;
        self.menu = None;
        self.confirm = None;
        window.set_visible(false);
    }

    /// The notes folder changed (Settings): the note on screen is written to the old store, then
    /// the window hides and starts over on the new one.
    pub fn use_store(&mut self, notes: Notes, window: &mut Window, cx: &mut Context<Self>) {
        self.hide(window, cx);
        self.notes = notes;
        self.doc = None;
        self.history = History::default();
    }

    /// The editor changed the note: it is written once typing has paused for `SETTLE`.
    fn edited(&mut self, cx: &mut Context<Self>) {
        self.edits += 1;
        let n = self.edits;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            _ = this.update(cx, |this, cx| {
                if this.edits == n {
                    this.save(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Writes the note on screen if the file doesn't have its text yet. The launcher calls it
    /// before it reads a note, so the read comes after this write on the store's worker.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some((id, text)) = doc.unsaved() else { return };
        doc.saved = Some(text.clone());
        self.write(id, text, true, cx);
    }

    /// Writes `text` to note `id`; a failure is handled by `failed`, and a write for a note no
    /// longer on screen is tried again once when `retry`.
    fn write(&mut self, id: Id, text: String, retry: bool, cx: &mut Context<Self>) {
        let (to, what, at) = (id.clone(), text.clone(), now());
        self.notes.run(cx, move |s| s.save(&to, &what, at), move |this, r, cx| {
            let Err(e) = r else { return };
            if let Some((id, text)) = failed(this.doc.as_mut(), id, text)
                && retry
            {
                return this.write(id, text, false, cx);
            }
            _ = this.tx.unbounded_send(Msg::error(format!("Couldn't save the note: {e}")));
        });
    }

    /// The window moved or changed size: its place is written once it rests.
    fn moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.moves += 1;
        let (n, place) = (self.moves, Place::of(window.window_bounds().get_bounds()));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            if this.update(cx, |this, _| this.moves == n).unwrap_or(false) {
                cx.background_executor().spawn(async move { drop(save_place(&place_path(), place)) }).await;
            }
        })
        .detach();
    }

    /// Ctrl+N and the header's New: a new note, or the empty one there already.
    fn new_note(&mut self, cx: &mut Context<Self>) {
        self.go(|s, at| s.create(at).map(created), Nav::Visit, "Couldn't create a note", cx);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = k.modifiers;
        if let Some(id) = shortcut(k) {
            self.act(id, window, cx);
            return cx.stop_propagation();
        }
        // A menu's search field sends Up, Down, Enter and Esc as actions (`Render`); these come
        // from a menu without one, or the confirm.
        if self.menu.is_some() || self.confirm.is_some() {
            let key = match k.key.as_str() {
                "up" => menu::Key::Up,
                "down" => menu::Key::Down,
                "tab" if m.shift => menu::Key::Up,
                "tab" => menu::Key::Down,
                "enter" => menu::Key::Enter,
                "escape" => menu::Key::Esc,
                // typing is the search field's
                _ => return,
            };
            self.key(key, window, cx);
            return cx.stop_propagation();
        }
        match k.key.as_str() {
            "escape" if !m.modified() => self.hide(window, cx),
            _ => {
                let Some(doc) = self.doc.as_mut() else { return };
                let Some(did) = press(&mut doc.buffer, k) else { return };
                let edited = match did {
                    Did::Moved => false,
                    Did::Edited => true,
                    Did::Copy { text, cut } => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        cut
                    }
                    Did::Paste => {
                        let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                        doc.buffer.insert(&pasted(&text));
                        true
                    }
                };
                let b = &doc.buffer;
                self.scroll.scroll_to_item(b.text()[..b.cursor()].matches('\n').count());
                self.changed(edited, cx);
            }
        }
        cx.stop_propagation();
    }

    /// After a key or a click: the caret shows, and an edit is saved once typing pauses.
    fn changed(&mut self, edited: bool, cx: &mut Context<Self>) {
        self.wake(cx);
        if edited { self.edited(cx) } else { cx.notify() }
    }

    /// The note offset under `p` (window coordinates), through the lines the last frame drew.
    fn offset_at(&self, p: Point<Pixels>) -> Option<usize> {
        let row = self.rows.iter().find(|r| p.y < r.layout.bounds().bottom()).or(self.rows.last())?;
        let b = row.layout.bounds();
        let shaped = row.layout.line_layout_for_index(0)?;
        let i = shaped.closest_byte_index_for_pixel_point(p - b.origin, row.layout.line_height()).unwrap_or_else(|i| i).min(row.len);
        Some(row.start + row.map.as_ref().map_or(i, |m| m[i]))
    }

    /// A press in the body puts the cursor there (Shift: extends the selection) and starts a drag.
    fn press_at(&mut self, p: Point<Pixels>, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(at) = self.offset_at(p) else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        doc.buffer.set_cursor(at, extend);
        self.selecting = true;
        self.changed(false, cx);
    }

    fn drag_to(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if ev.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
        }
        if !self.selecting {
            return;
        }
        let Some(at) = self.offset_at(ev.position) else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        doc.buffer.set_cursor(at, true);
        self.changed(false, cx);
    }

    /// A task's box was clicked: the task on the line at `at` flips, the cursor stays.
    fn tick(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        let b = &mut doc.buffer;
        let (cursor, anchor) = (b.cursor(), b.anchor());
        b.set_cursor(at, false);
        b.toggle_task(false);
        // a flip keeps the text's length, so both ends are still where they were
        b.set_cursor(anchor, false);
        b.set_cursor(cursor, true);
        self.changed(true, cx);
    }

    /// The note, a line per row: marks hidden except on the cursor's line, fenced code as typed.
    fn body(&mut self, t: &Tokens, caret: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let me = cx.entity().downgrade();
        let body = div()
            .id("body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(px(sp::S16))
            .flex()
            .flex_col()
            .text_color(t.text)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| this.press_at(ev.position, ev.modifiers.shift, window, cx)))
            // every move, so a drag keeps selecting past the body's edge (`drag_to` gates on `selecting`)
            .on_mouse_move_all(move |ev: &MouseMoveEvent, phase, _, _, cx| {
                if phase == DispatchPhase::Bubble {
                    _ = me.update(cx, |this, cx| this.drag_to(ev, cx));
                }
            })
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.selecting = false));
        self.rows.clear();
        let Some(doc) = &self.doc else { return body.child(div().text_color(t.text_faint).child("Start writing…")) };
        let text = doc.buffer.text();
        let (cursor, (sel_from, sel_to)) = (doc.buffer.cursor(), doc.buffer.selection());
        let here = text[..cursor].matches('\n').count();
        let empty = text.is_empty();
        let last = text.matches('\n').count();
        let (mut start, mut fenced) = (0, false);
        let mut lines = Vec::new();
        // ponytail: every line is laid out each frame (only changed lines are restyled); a
        // virtual list (gpui `list`) when notes run to thousands of lines.
        for (i, line) in text.split('\n').enumerate() {
            let fence = editor::is_fence(line);
            let code = fenced || fence;
            // A fenced run is one block (`.pc-notes__code`): rounded and padded at its first and
            // last rows, no gap between its rows. Off the cursor's line a fence draws nothing.
            let (open, close) = (fence && !fenced, (fence && fenced) || (code && i == last));
            fenced ^= fence;
            let raw = i == here;
            let hidden = fence && !raw;
            let (len, map, st, style) = if code {
                (line.len(), None, styled(line, &[], t), Style::default())
            } else {
                let s = self.cache.get(line);
                let style = s.spans.first().map_or_else(Style::default, |s| s.style);
                if raw { (line.len(), None, styled(line, &s.spans, t), style) } else { (s.shown.len(), Some(s.map.clone()), styled(&s.shown, &s.hidden, t), style) }
            };
            let to_shown = |src: usize| map.as_ref().map_or(src, |m| m.partition_point(|&v| v < src));
            let end = start + line.len();
            let (from, to) = (sel_from.max(start), sel_to.min(end));
            let sel = (from < to).then(|| (to_shown(from - start), to_shown(to - start)));
            let st = if empty { StyledText::new("Start writing…") } else { st };
            let layout = st.layout().clone();
            let check = style.task.filter(|_| !raw).map(|done| {
                div()
                    .flex_none()
                    .size(px(design::NOTE_BOX))
                    .rounded(px(r::XS))
                    .border(px(design::NOTE_BOX_BORDER))
                    .border_color(if done { t.accent } else { t.text_muted })
                    .when(done, |d| d.bg(t.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.tick(start, cx);
                        }),
                    )
            });
            let heading = match style.heading {
                0 => None,
                1 => Some(ts::DISPLAY),
                2 => Some(ts::HEADING),
                _ => Some(ts::BODY),
            };
            let row = div()
                .flex()
                .items_center()
                .gap(px(sp::S8))
                // `.pc-notes__body`'s gap between rows, except inside a code block
                .when(i > 0 && (open || !code), |d| d.mt(px(sp::S8)))
                .when_some(heading, |d, size| d.text_size(px(size)).font_weight(FontWeight::SEMIBOLD))
                .when(style.quote, |d| d.border_l(px(design::QUOTE_BAR)).border_color(t.hairline).pl(px(sp::S8)).text_color(t.text_muted))
                .when(code, |d| {
                    d.font_family(design::FONT_MONO)
                        .text_size(px(ts::BODY_SM))
                        .bg(t.surface)
                        .px(px(sp::S8))
                        .text_color(t.text_muted)
                        .when(open, |d| d.pt(px(sp::S8)).rounded_t(px(r::SM)))
                        .when(close, |d| d.pb(px(sp::S8)).rounded_b(px(r::SM)))
                })
                .when(empty, |d| d.text_color(t.text_faint));
            if hidden {
                lines.push(row);
            } else {
                lines.push(row.children(check).child(div().relative().flex_1().min_w_0().child(overlay(layout.clone(), sel, (raw && caret).then_some(cursor - start), t)).child(st)));
                self.rows.push(Row { start, len: if empty { 0 } else { len }, map, layout });
            }
            start = end + 1;
        }
        self.cache.sweep();
        body.children(lines)
    }

    fn header(&self, title: String, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = t.text_muted;
        let tool = move |id: &'static str, glyph: &'static str| div().id(id).flex().items_center().child(design::icon(glyph, ic::MD, muted));
        // Drag only where there is no button: gpui takes the first control area under the mouse.
        let drag = |id: &'static str| div().id(id).window_control_area(WindowControlArea::Drag).flex_1().h_full();
        div()
            .h(px(design::CONTROL))
            .flex_none()
            .px(px(sp::S12))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(t.hairline)
            .text_size(px(ts::BODY_SM))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(sp::S12))
                    .child(tool("actions", g::SQUARE_MENU).on_click(cx.listener(|this, _, window, cx| this.act("actions", window, cx))))
                    .child(tool("browse", g::NOTEBOOK_TABS).on_click(cx.listener(|this, _, window, cx| this.act("browse", window, cx))))
                    .child(tool("new", g::PLUS).on_click(cx.listener(|this, _, _, cx| this.new_note(cx))))
                    .child(drag("drag-left")),
            )
            .child(div().id("drag").window_control_area(WindowControlArea::Drag).min_w_0().h_full().flex().items_center().font_weight(FontWeight::SEMIBOLD).text_color(muted).child(div().truncate().child(title)))
            .child(div().flex_1().h_full().flex().items_center().child(drag("drag-right")).child(tool("close", g::CANCEL).on_click(cx.listener(|this, _, window, cx| this.hide(window, cx)))))
    }

    /// The character count centred, the Text style toggle on the right; while that is on, the
    /// formatting bar (`.pc-notes__bar`) instead.
    fn footer(&self, text: &str, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let muted = t.text_muted;
        let item = |id: &'static str, glyph: &'static str, menu: bool| {
            div().id(id).flex().items_center().gap(px(sp::S4)).child(design::icon(glyph, ic::MD, muted)).when(menu, |d| d.child(design::icon(g::CHEVRON_DOWN, ic::XS, muted)))
        };
        let bar = |on: bool| cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.bar = on;
            cx.notify();
        });
        if !self.bar {
            return div()
                .h(px(design::CONTROL))
                .flex_none()
                .px(px(sp::S12))
                .flex()
                .items_center()
                .text_size(px(ts::SMALL))
                .text_color(t.text_faint)
                .child(div().flex_1())
                .child(char_count_text(text.chars().count()))
                .child(div().flex_1().flex().justify_end().child(item("type", g::TYPE, false).on_click(bar(true))))
                .into_any_element();
        }
        let pop = |pop: Pop| cx.listener(move |this, _: &ClickEvent, window, cx| this.toggle_pop(pop, window, cx));
        let apply = |id: &'static str| cx.listener(move |this, _: &ClickEvent, _, cx| this.restyle_note(id, cx));
        div()
            .relative()
            .h(px(design::CONTROL))
            .flex_none()
            .px(px(sp::S12))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(sp::S8))
            .child(item("heading", g::HEADING, true).on_click(pop(Pop::Heading)))
            .child(item("style", g::TYPE, true).on_click(pop(Pop::Style)))
            .child(item("link", g::LINK, false).on_click(apply("link")))
            .child(item("image", g::PICTURE, false).on_click(apply("image")))
            .child(item("code", g::CODE, false).on_click(apply("code")))
            .child(design::vdivider(t))
            .child(item("quote", g::QUOTE, false).on_click(apply("quote")))
            .child(item("task", g::LIST_CHECKS, false).on_click(apply("task")))
            .child(item("list", g::BULLETS, true).on_click(pop(Pop::List)))
            .child(div().absolute().right(px(sp::S12)).flex().child(item("close-bar", g::CANCEL, false).on_click(bar(false))))
            .into_any_element()
    }

    /// The open menu over the window; Browse Notes' rows end in Pin and Delete.
    fn pop(&self, t: &Tokens, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let open = self.menu.as_ref()?;
        let me = cx.weak_entity();
        let pick = move |i: usize, window: &mut Window, cx: &mut App| _ = me.update(cx, |this, cx| this.pick(i, window, cx));
        let me = cx.weak_entity();
        let outside = move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| _ = me.update(cx, |this, cx| this.close_menu(window, cx));
        let field = open.menu.search.as_ref().map(|placeholder| {
            text_input("notes-menu-search").state(self.menu_input.downgrade()).placeholder(placeholder.clone()).placeholder_color(t.text_faint).caret_color(t.accent).flex_1().whitespace_nowrap().into_any_element()
        });
        let notes: Vec<Option<(Id, String)>> = open.menu.entries.iter().map(|e| (open.pop == Pop::Browse && e.id != DELETED).then(|| (e.id.clone(), e.label.clone()))).collect();
        let (me, muted) = (cx.weak_entity(), t.text_muted);
        let trail = move |i: usize, on: bool| {
            let (id, title) = notes.get(i).cloned().flatten()?;
            let tool = |name: &'static str, glyph: &'static str, delete: bool| {
                let (me, id, title) = (me.clone(), id.clone(), title.clone());
                div().id(name).flex().child(design::icon(glyph, ic::MD, muted)).on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    _ = me.update(cx, |this, cx| if delete { this.ask_delete(id.clone(), title.clone(), window, cx) } else { this.toggle_pin(id.clone(), cx) });
                })
            };
            // `.pc-notes__rowtools`: on the selected row, or the one under the mouse
            Some(div().ml_auto().flex().gap(px(sp::S8)).when(!on, |d| d.invisible().group_hover("pop-row", |s| s.visible())).child(tool("pin", g::PIN, false)).child(tool("delete", g::DELETE, true)).into_any_element())
        };
        Some(design::pop_menu(&open.menu, field, &open.scroll, pick, outside, trail, t))
    }

    /// Delete Note's confirm, over the whole window.
    fn dialog(&self, t: &Tokens, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let (id, title) = self.confirm.clone()?;
        let delete = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.close_confirm(window, cx);
            this.delete(id.clone(), cx);
        });
        let cancel = cx.listener(|this, _: &ClickEvent, window, cx| this.close_confirm(window, cx));
        let buttons = [design::button("Delete", design::Button::Danger, true, t).id("delete").w_full().on_click(delete), design::button("Cancel", design::Button::Quiet, false, t).id("cancel").w_full().on_click(cancel)];
        Some(design::dialog(true, "Delete Note?", &title, buttons, t))
    }
}

/// The line's text with its spans' styles (`spans` in the text's bytes); an empty line is a
/// space, so it keeps a line's height and a place for the caret.
fn styled(text: &str, spans: &[Span], t: &Tokens) -> StyledText {
    let hl: Vec<_> = spans.iter().map(|s| (s.range.clone(), highlight(s.style, t))).collect();
    let mono: Vec<_> = spans.iter().filter(|s| s.style.code).map(|s| (s.range.clone(), SharedString::from(design::FONT_MONO))).collect();
    StyledText::new(if text.is_empty() { " ".to_owned() } else { text.to_owned() }).with_highlights(hl).with_font_family_overrides(mono)
}

/// One span's look. A heading's size and weight, a quote's bar and a task's box are the row's.
fn highlight(s: Style, t: &Tokens) -> HighlightStyle {
    let stroke = || px(design::TEXT_LINE);
    let mut h = HighlightStyle::default();
    if s.bold {
        h.font_weight = Some(FontWeight::SEMIBOLD);
    }
    if s.italic {
        h.font_style = Some(FontStyle::Italic);
    }
    if s.underline {
        h.underline = Some(UnderlineStyle { thickness: stroke(), ..Default::default() });
    }
    // a done task's line is struck and faint, like strike
    if s.strike || s.task == Some(true) {
        h.strikethrough = Some(StrikethroughStyle { thickness: stroke(), ..Default::default() });
        h.color = Some(t.text_faint);
    }
    if s.highlight {
        h.background_color = Some(design::alpha(Tint::Paste.hue(t), mix::TAG_FILL));
    }
    if s.code {
        h.background_color = Some(t.surface);
    }
    if s.link {
        h.color = Some(t.accent);
    }
    h
}

/// The selection and the caret (display bytes) under the line's text. It paints from the
/// text's layout, which every element has by the time anything paints.
fn overlay(layout: TextLayout, sel: Option<(usize, usize)>, caret: Option<usize>, t: &Tokens) -> impl IntoElement {
    let (sel_color, caret_color) = (t.sel_ring, t.accent);
    canvas(
        |_, _, _| {},
        move |_, (), window, _| {
            let (lh, b) = (layout.line_height(), layout.bounds());
            if let Some((from, to)) = sel
                && let (Some(p), Some(q)) = (layout.position_for_index(from), layout.position_for_index(to))
            {
                let band = |x0: Pixels, y0: Pixels, x1: Pixels, y1: Pixels| PaintQuad::new(Bounds::from_corners(point(x0, y0), point(x1, y1))).background(sel_color);
                if p.y == q.y {
                    window.paint_quad(band(p.x, p.y, q.x, p.y + lh));
                } else {
                    // wrapped: to the right edge, whole lines between, then from the left edge
                    window.paint_quad(band(p.x, p.y, b.right(), p.y + lh));
                    window.paint_quad(band(b.left(), p.y + lh, b.right(), q.y));
                    window.paint_quad(band(b.left(), q.y, q.x, q.y + lh));
                }
            }
            if let Some(c) = caret
                && let Some(p) = layout.position_for_index(c)
            {
                window.paint_quad(PaintQuad::new(Bounds::new(p, size(px(design::CARET), lh))).background(caret_color));
            }
        },
    )
    .absolute()
    .size_full()
}

impl Render for NotesWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drawn at the interface size alone, like Settings.
        design::scale_settings();
        let t = Tokens::from_theme(&self.theme, window);
        let text = self.doc.as_ref().map_or(String::new(), |d| d.buffer.text().to_owned());
        // The keys are the menu's or the confirm's while either is open.
        let caret = self.caret_on && window.is_window_active() && self.menu.is_none() && self.confirm.is_none();
        let (pop, dialog) = (self.pop(&t, cx), self.dialog(&t, cx));
        design::glass_window(&t)
            .size_full()
            .font_family(design::FONT_UI)
            .text_size(px(ts::BODY))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            // A menu's search field sends these (its text box's bindings).
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| this.key(menu::Key::Down, window, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| this.key(menu::Key::Up, window, cx)))
            .on_action(cx.listener(|this, _: &Launch, window, cx| this.key(menu::Key::Enter, window, cx)))
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| this.key(menu::Key::Esc, window, cx)))
            .on_action(cx.listener(|this, _: &ToggleActions, window, cx| this.act("actions", window, cx)))
            .on_action(cx.listener(|this, _: &OpenDropdown, window, cx| this.act("browse", window, cx)))
            .child(self.header(title_of(&text), &t, cx))
            .child(self.body(&t, caret, cx))
            .child(self.footer(&text, &t, cx))
            .children(pop)
            .children(dialog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("procast-notes-window-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn char_counts_read_like_the_footer() {
        assert_eq!(char_count_text(0), "0 characters");
        assert_eq!(char_count_text(1), "1 character");
        assert_eq!(char_count_text(186), "186 characters");
        assert_eq!(char_count_text(1013), "1,013 characters");
    }

    #[test]
    fn the_window_place_round_trips_through_its_file() {
        let dir = tmp("place");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes-window.json");
        assert_eq!(load_place(&path), None, "no file: centred");
        let p = Place { x: -1200.5, y: 80., w: 560., h: 440. };
        save_place(&path, p).unwrap();
        assert_eq!(load_place(&path), Some(p));
        assert_eq!(Place::of(p.bounds()), p);
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_place(&path), None);
        std::fs::write(&path, r#"{"x":0,"y":0,"w":0,"h":440}"#).unwrap();
        assert_eq!(load_place(&path), None, "a window with no width is not a place");
    }

    #[test]
    fn a_read_of_the_note_on_screen_never_replaces_its_buffer() {
        let mut doc = Doc::new("a".into(), "newer".into());
        assert!(replaces(None, "a", false), "nothing on screen");
        assert!(replaces(Some(&doc), "b", false), "another note");
        assert!(!replaces(Some(&doc), "a", false), "the same note, saved: the read may be older");
        assert!(!replaces(Some(&doc), "a", true), "a saved note with text is never replaced");
        assert!(replaces(Some(&Doc::new("a".into(), String::new())), "a", true), "the reader wrote the empty note (Create Note's title)");
        doc.buffer.insert(" typed");
        assert!(!replaces(Some(&doc), "a", false));
        assert!(!replaces(Some(&doc), "a", true), "unsaved keys always stay");
    }

    #[test]
    fn a_failed_write_marks_only_its_own_note_and_comes_back_for_another() {
        let mut a = Doc::new("a".into(), "text".into());
        assert_eq!(failed(Some(&mut a), "a".into(), "text".into()), None);
        assert_eq!(a.saved, None, "still on screen: unsaved, the next settle or hide tries again");
        let mut b = Doc::new("b".into(), "other".into());
        assert_eq!(failed(Some(&mut b), "a".into(), "lost?".into()), Some(("a".into(), "lost?".into())), "moved on: tried again");
        assert_eq!(b.saved.as_deref(), Some("other"), "the note on screen is not touched");
        assert_eq!(failed(None, "a".into(), "t".into()), Some(("a".into(), "t".into())));
    }

    fn key(b: &mut Buffer, spec: &str) -> Option<Did> {
        press(b, &Keystroke::parse(spec).unwrap())
    }

    /// Types `s` as the keyboard sends it: each character as its key's `key_char`.
    fn typed(b: &mut Buffer, s: &str) {
        for c in s.chars() {
            let k = if c == ' ' { "space".to_owned() } else { c.to_lowercase().to_string() };
            assert_eq!(key(b, &format!("{k}->{c}")), Some(Did::Edited), "{c}");
        }
    }

    #[test]
    fn typing_and_editing_keys_change_the_note() {
        let mut b = Buffer::new("");
        typed(&mut b, "# Title");
        assert_eq!(key(&mut b, "enter"), Some(Did::Edited));
        typed(&mut b, "**bold** x");
        assert_eq!(b.text(), "# Title\n**bold** x");
        assert_eq!(key(&mut b, "backspace"), Some(Did::Edited));
        assert_eq!(key(&mut b, "home"), Some(Did::Moved));
        assert_eq!(key(&mut b, "delete"), Some(Did::Edited));
        assert_eq!(b.text(), "# Title\n*bold** ");
        assert_eq!(key(&mut b, "ctrl-z"), Some(Did::Edited));
        assert_eq!(b.text(), "# Title\n**bold** ");
        assert_eq!(key(&mut b, "ctrl-y"), Some(Did::Edited));
        assert_eq!(b.text(), "# Title\n*bold** ");
        key(&mut b, "ctrl-z");
        assert_eq!(key(&mut b, "ctrl-shift-z"), Some(Did::Edited), "Ctrl+Shift+Z redoes too");
        assert_eq!(b.text(), "# Title\n*bold** ");
        // AltGr is Ctrl+Alt: a character it types is typed, not a shortcut
        assert_eq!(key(&mut b, "ctrl-alt-0->}"), Some(Did::Edited));
        assert_eq!(b.text(), "# Title
}*bold** ");
        // not the editor's: left to the window (Tab, Alt+letter, unknown chords)
        assert_eq!(key(&mut b, "tab"), None);
        assert_eq!(key(&mut b, "alt-a->a"), None);
        assert_eq!(key(&mut b, "ctrl-q"), None);
    }

    #[test]
    fn moves_select_with_shift_and_never_edit() {
        let mut b = Buffer::new("one two\nthree");
        for (k, at) in [("ctrl-home", 0), ("ctrl-right", 3), ("right", 4), ("end", 7), ("down", 13), ("up", 7), ("ctrl-left", 4), ("left", 3), ("ctrl-end", 13), ("home", 8)] {
            assert_eq!(key(&mut b, k), Some(Did::Moved), "{k}");
            assert_eq!(b.cursor(), at, "{k}");
        }
        assert_eq!(key(&mut b, "shift-end"), Some(Did::Moved));
        assert_eq!(b.selection(), (8, 13));
        key(&mut b, "ctrl-shift-home");
        assert_eq!((b.cursor(), b.anchor()), (0, 8));
        key(&mut b, "ctrl-a");
        assert_eq!(b.selection(), (0, 13));
        let doc = Doc { id: "a".into(), buffer: b, saved: Some("one two\nthree".into()), pinned: false };
        assert_eq!(doc.unsaved(), None, "moving writes nothing");
    }

    #[test]
    fn mark_and_block_shortcuts() {
        for (k, out) in [
            ("ctrl-b", "a **b**"),
            ("ctrl-i", "a *b*"),
            ("ctrl-u", "a <u>b</u>"),
            ("ctrl-shift-s", "a ~~b~~"),
            ("ctrl-e", "a `b`"),
            ("ctrl-shift-m", "a ==b=="),
            ("ctrl-shift-b", "> a b"),
            ("ctrl-alt-1", "# a b"),
            ("ctrl-alt-2", "## a b"),
            ("ctrl-alt-3", "### a b"),
            ("ctrl-alt-c", "```\na b\n```"),
        ] {
            let mut b = Buffer::new("a b");
            key(&mut b, "shift-left");
            assert_eq!(key(&mut b, k), Some(Did::Edited), "{k}");
            assert_eq!(b.text(), out, "{k}");
            key(&mut b, "ctrl-z");
            assert_eq!(b.text(), "a b", "{k} undoes");
        }
        let mut b = Buffer::new("## a");
        key(&mut b, "ctrl-alt-0");
        assert_eq!(b.text(), "a", "Paragraph");
    }

    #[test]
    fn copy_cut_and_paste_go_through_the_window() {
        let mut b = Buffer::new("héllo");
        assert_eq!(key(&mut b, "ctrl-c"), Some(Did::Moved), "nothing selected: nothing copied");
        key(&mut b, "ctrl-a");
        assert_eq!(key(&mut b, "ctrl-c"), Some(Did::Copy { text: "héllo".into(), cut: false }));
        assert_eq!(b.text(), "héllo");
        assert_eq!(key(&mut b, "ctrl-x"), Some(Did::Copy { text: "héllo".into(), cut: true }));
        assert_eq!(b.text(), "");
        assert_eq!(key(&mut b, "ctrl-v"), Some(Did::Paste));
        assert_eq!(pasted("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn history_goes_back_and_forward_and_a_new_visit_forgets_forward() {
        let mut h = History::default();
        let off = |h: &History| (h.next(Nav::Back).is_none(), h.next(Nav::Forward).is_none());
        assert_eq!(off(&h), (true, true), "a new window: both off");
        h.went(None, "a", Nav::Visit);
        h.went(Some("a".into()), "b", Nav::Visit);
        h.went(Some("b".into()), "c", Nav::Visit);
        h.went(Some("c".into()), "c", Nav::Visit);
        assert_eq!(h.next(Nav::Back).map(String::as_str), Some("b"), "the same note again is no step");
        h.went(Some("c".into()), "b", Nav::Back);
        h.went(Some("b".into()), "a", Nav::Back);
        assert_eq!(off(&h), (true, false), "at the start: Go Back off");
        assert_eq!(h.next(Nav::Forward).map(String::as_str), Some("b"));
        h.went(Some("a".into()), "b", Nav::Forward);
        assert_eq!((h.next(Nav::Back).map(String::as_str), h.next(Nav::Forward).map(String::as_str)), (Some("a"), Some("c")));
        h.went(Some("b".into()), "d", Nav::Visit);
        assert_eq!(off(&h), (false, true), "a new note: Go Forward off");
        h.went(Some("d".into()), "b", Nav::Gone);
        assert_eq!(h.back, ["a"], "d was deleted and b shows: no step leads to either");
        // a deleted note leaves both ways, and no step leads to the note on screen
        let mut h = History { back: vec!["a".into(), "x".into(), "a".into(), "e".into()], forward: vec!["x".into()] };
        h.forget("x", "e");
        assert_eq!((h.back.as_slice(), h.forward.as_slice()), (&["a".to_string()][..], &[][..]));
    }

    #[test]
    fn the_action_list_has_eight_actions_and_go_back_is_off_without_history() {
        let mut h = History::default();
        let a = actions(false, &h);
        let shown: Vec<(&str, &str, bool)> = a.iter().map(|e| (e.label.as_str(), e.keys, e.off)).collect();
        assert_eq!(
            shown,
            [
                ("New Note", "Ctrl+N", false),
                ("Duplicate Note", "", false),
                ("Pin Note", "Ctrl+Shift+P", false),
                ("Browse Notes", "Ctrl+P", false),
                ("Go Back", "Ctrl+[", true),
                ("Go Forward", "Ctrl+]", true),
                ("Copy as Markdown", "", false),
                ("Delete Note", "", false),
            ]
        );
        assert!(a[7].danger && a.iter().filter(|e| e.danger).count() == 1, "Delete Note is the danger row");
        assert!(a.iter().all(|e| e.icon.is_some()));
        h.went(Some("a".into()), "b", Nav::Visit);
        let a = actions(true, &h);
        assert_eq!((a[2].label.as_str(), a[4].off, a[5].off), ("Unpin Note", false, true));
        let mut m = menu::Menu::new(a, Some("Search for actions…".into()), menu::Place::Center(px(design::POP_CENTER_TOP)));
        m.set_filter("go");
        assert_eq!(m.picked().map(|e| e.id.as_str()), Some("back"));
    }

    #[test]
    fn the_window_shortcuts() {
        let at = |s: &str| shortcut(&Keystroke::parse(s).unwrap());
        for (k, id) in [("ctrl-k", "actions"), ("ctrl-p", "browse"), ("ctrl-n", "new"), ("ctrl-shift-p", "pin"), ("ctrl-[", "back"), ("ctrl-]", "forward")] {
            assert_eq!(at(k), Some(id), "{k}");
        }
        for k in ["k->k", "ctrl-alt-p", "ctrl-shift-k", "ctrl-b", "alt-n"] {
            assert_eq!(at(k), None, "{k}");
        }
    }

    #[test]
    fn browse_rows_say_current_or_when_opened_and_end_with_recently_deleted() {
        const DAY: i64 = 86_400;
        let now = 100 * DAY;
        let meta = |id: &str, title: &str, pinned: bool, opened: i64, modified: i64| NoteMeta { id: id.into(), pinned, created: 0, modified, deleted_at: None, opened, title: title.into() };
        // the store's order (pinned first, then last opened) is kept
        let rows = [(meta("p", "Pinned", true, now - 9 * DAY, 0), 5), (meta("g", "Groceries", false, now, 0), 180), (meta("t", "Trip", false, now - 2 * DAY, 0), 397), (meta("n", "Never", false, 0, now - DAY), 1)];
        let e = browse_entries(&rows, Some(("g", "Groceries list".into(), 186)), now);
        let shown: Vec<(&str, &str, Option<&str>)> = e.iter().map(|e| (e.id.as_str(), e.label.as_str(), e.two_line.as_deref())).collect();
        assert_eq!(
            shown,
            [
                ("p", "Pinned", Some("Opened 9 days ago · 5 characters")),
                ("g", "Groceries list", Some("● Current · 186 characters")),
                ("t", "Trip", Some("Opened 2 days ago · 397 characters")),
                ("n", "Never", Some("Edited yesterday · 1 character")),
                (DELETED, "Recently Deleted", None),
            ]
        );
        assert!(e[..4].iter().all(|e| e.section.as_deref() == Some("Notes") && e.always));
        assert!(e[3].rule_after && e[4].section.is_none() && e[4].icon == Some(g::ARCHIVE_RESTORE));
        assert_eq!(browse_entries(&[], None, now).len(), 1, "no notes: only Recently Deleted");
    }

    #[test]
    fn the_formatting_bar_formats() {
        for (id, out) in [
            ("h1", "# a b"),
            ("h2", "## a b"),
            ("h3", "### a b"),
            ("bold", "a **b**"),
            ("italic", "a *b*"),
            ("underline", "a <u>b</u>"),
            ("strike", "a ~~b~~"),
            ("highlight", "a ==b=="),
            ("code", "a `b`"),
            ("link", "a [b]()"),
            ("image", "a ![b]()"),
            ("quote", "> a b"),
            ("task", "- [ ] a b"),
            ("bullet", "- a b"),
            ("numbered", "1. a b"),
        ] {
            let mut b = Buffer::new("a b");
            b.left(true);
            assert!(restyle(&mut b, id), "{id}");
            assert_eq!(b.text(), out, "{id}");
        }
        let mut b = Buffer::new("## a");
        assert!(restyle(&mut b, "p"));
        assert_eq!(b.text(), "a", "Paragraph");
        assert!(!restyle(&mut b, "nope"));
        for m in [heading_menu(), style_menu(), list_menu()] {
            assert!(m.iter().all(|e| restyle(&mut Buffer::new("x"), &e.id)), "every menu entry formats");
        }
        assert_eq!(heading_menu().iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Heading 1", "Heading 2", "Heading 3", "Paragraph"]);
        assert_eq!(style_menu().iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Bold", "Italic", "Underline", "Strikethrough", "Highlight"]);
        assert_eq!(list_menu().iter().map(|e| (e.label.as_str(), e.icon)).collect::<Vec<_>>(), [("Bulleted List", Some(g::BULLETS)), ("Numbered List", Some(g::LIST_ORDERED))]);
    }

    /// Autosave and hide write `Doc::unsaved` through `Store::save`, as `NotesWindow::save` does.
    #[test]
    fn an_edited_note_is_written_once_and_reads_back() {
        let dir = tmp("autosave");
        let mut s = Store::open(dir.clone()).unwrap();
        let id = created(s.create(10).unwrap());
        let mut doc = Doc::new(id.clone(), s.text(&id).unwrap());
        assert_eq!(doc.unsaved(), None, "a note just opened has nothing to write");
        doc.buffer.insert("# Groceries\nmilk");
        let (to, text) = doc.unsaved().unwrap();
        assert_eq!((to.as_str(), text.as_str()), (id.as_str(), "# Groceries\nmilk"));
        s.save(&to, &text, 11).unwrap();
        doc.saved = Some(text);
        assert_eq!(doc.unsaved(), None);
        doc.saved = None;
        assert!(doc.unsaved().is_some(), "after a failed write the text is still unsaved");
        let s = Store::open(dir).unwrap();
        assert_eq!(s.text(&id).unwrap(), "# Groceries\nmilk");
        assert_eq!(s.list()[0].title, "Groceries");
    }
}
