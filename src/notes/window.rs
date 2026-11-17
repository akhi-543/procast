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
    App, Bounds, ClipboardItem, Context, FocusHandle, FontStyle, FontWeight, HighlightStyle, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, ScrollHandle, SharedString, StrikethroughStyle, StyledText, Subscription, TextLayout, UnderlineStyle, Window, WindowControlArea,
    PaintQuad, canvas, div, point, prelude::*, size,
};
use gpui_ce_elements::editable_text::BLINK_INTERVAL_500MS;
use serde::{Deserialize, Serialize};

use super::{
    editor::{self, Block, Buffer, Mark, Span, SpanCache, Style},
    store::{Exists, Id, Store, title_of},
};
use crate::{
    config,
    design::{self, Tint, Tokens, g, ic, mix, px, r, sp, ts},
    focus,
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

fn now() -> i64 {
    crate::search::now() as i64
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
}

impl Doc {
    fn new(id: Id, text: String) -> Doc {
        Doc { id, buffer: Buffer::new(&text), saved: Some(text) }
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
    _subs: Vec<Subscription>,
}

impl NotesWindow {
    pub fn new(notes: Notes, tx: UnboundedSender<Msg>, theme: config::Theme, window: &mut Window, cx: &mut Context<Self>) -> Self {
        win::round_corners(win::hwnd(window));
        let subs = vec![
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
        if replaces(self.doc.as_ref(), &id, wrote) {
            self.save(cx);
            self.doc = Some(Doc::new(id, text));
        }
        self.reveal(window, cx);
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
        window.set_visible(false);
    }

    /// The notes folder changed (Settings): the note on screen is written to the old store, then
    /// the window hides and starts over on the new one.
    pub fn use_store(&mut self, notes: Notes, window: &mut Window, cx: &mut Context<Self>) {
        self.hide(window, cx);
        self.notes = notes;
        self.doc = None;
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
        self.save(cx);
        let at = now();
        self.notes.run(
            cx,
            move |s| {
                let id = created(s.create(at)?);
                s.open_note(&id, at)?;
                Ok((s.text(&id)?, id))
            },
            |this, r, cx| match r {
                Ok((text, id)) => {
                    // Keys typed while the create ran are written before the new note shows.
                    if replaces(this.doc.as_ref(), &id, false) {
                        this.save(cx);
                        this.doc = Some(Doc::new(id, text));
                    }
                    cx.notify();
                }
                Err(e) => _ = this.tx.unbounded_send(Msg::error(format!("Couldn't create a note: {e}"))),
            },
        );
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = k.modifiers;
        match k.key.as_str() {
            "escape" if !m.modified() => self.hide(window, cx),
            "n" if m.control && !m.shift && !m.alt => self.new_note(cx),
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
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.drag_to(ev, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.selecting = false));
        self.rows.clear();
        let Some(doc) = &self.doc else { return body.child(div().text_color(t.text_faint).child("Start writing…")) };
        let text = doc.buffer.text();
        let (cursor, (sel_from, sel_to)) = (doc.buffer.cursor(), doc.buffer.selection());
        let here = text[..cursor].matches('\n').count();
        let empty = text.is_empty();
        let (mut start, mut fenced) = (0, false);
        let mut lines = Vec::new();
        // ponytail: every line is laid out each frame (only changed lines are restyled); a
        // virtual list (gpui `list`) when notes run to thousands of lines.
        for (i, line) in text.split('\n').enumerate() {
            let fence = editor::is_fence(line);
            let code = fenced || fence;
            fenced ^= fence;
            let raw = i == here;
            let (len, map, st, style) = if code {
                // off the cursor's line a fence is hidden: the block's padding
                let shown = if fence && !raw { "" } else { line };
                (shown.len(), None, styled(shown, &[], t), Style::default())
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
            lines.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(sp::S8))
                    .when_some(heading, |d, size| d.text_size(px(size)).font_weight(FontWeight::SEMIBOLD))
                    .when(style.quote, |d| d.border_l(px(design::QUOTE_BAR)).border_color(t.hairline).pl(px(sp::S8)).text_color(t.text_muted))
                    .when(code, |d| d.font_family(design::FONT_MONO).text_size(px(ts::BODY_SM)).bg(t.surface).px(px(sp::S8)).text_color(t.text_muted))
                    .when(empty, |d| d.text_color(t.text_faint))
                    .children(check)
                    .child(div().relative().flex_1().min_w_0().child(overlay(layout.clone(), sel, (raw && caret).then_some(cursor - start), t)).child(st)),
            );
            self.rows.push(Row { start, len: if empty { 0 } else { len }, map, layout });
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
                    // Actions (Ctrl+K) and Browse (Ctrl+P) open Task 20's lists.
                    .child(tool("actions", g::SQUARE_MENU))
                    .child(tool("browse", g::NOTEBOOK_TABS))
                    .child(tool("new", g::PLUS).on_click(cx.listener(|this, _, _, cx| this.new_note(cx))))
                    .child(drag("drag-left")),
            )
            .child(div().id("drag").window_control_area(WindowControlArea::Drag).min_w_0().h_full().flex().items_center().font_weight(FontWeight::SEMIBOLD).text_color(muted).child(div().truncate().child(title)))
            .child(div().flex_1().h_full().flex().items_center().child(drag("drag-right")).child(tool("close", g::CANCEL).on_click(cx.listener(|this, _, window, cx| this.hide(window, cx)))))
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

/// The character count centred; the Text style toggle on the right (Task 20 shows its bar).
fn footer(text: &str, t: &Tokens) -> impl IntoElement + use<> {
    div()
        .h(px(design::CONTROL))
        .flex_none()
        .px(px(sp::S12))
        .flex()
        .items_center()
        .text_size(px(ts::SMALL))
        .text_color(t.text_faint)
        .child(div().flex_1())
        .child(char_count_text(text.chars().count()))
        .child(div().flex_1().flex().justify_end().child(design::icon(g::TYPE, ic::MD, t.text_muted)))
}

impl Render for NotesWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drawn at the interface size alone, like Settings.
        design::scale_settings();
        let t = Tokens::from_theme(&self.theme, window);
        let text = self.doc.as_ref().map_or(String::new(), |d| d.buffer.text().to_owned());
        let caret = self.caret_on && window.is_window_active();
        design::glass_window(&t)
            .size_full()
            .font_family(design::FONT_UI)
            .text_size(px(ts::BODY))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .child(self.header(title_of(&text), &t, cx))
            .child(self.body(&t, caret, cx))
            .child(footer(&text, &t))
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
        let doc = Doc { id: "a".into(), buffer: b, saved: Some("one two\nthree".into()) };
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
