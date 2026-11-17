//! The Notes window: one note at a time in a floating glass window, above the other windows
//! while it shows. The note store belongs to one background worker (`Notes`), so no store call
//! runs on the UI thread and the store's work happens in the order it was asked for.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use futures::{
    StreamExt,
    channel::{
        mpsc::{self, UnboundedSender},
        oneshot,
    },
};
use gpui::{App, Bounds, Context, FocusHandle, FontWeight, KeyDownEvent, Pixels, Subscription, Window, WindowControlArea, div, prelude::*};
use serde::{Deserialize, Serialize};

use super::{
    editor::Buffer,
    store::{Exists, Id, Store, title_of},
};
use crate::{
    config,
    design::{self, Tokens, g, ic, px, sp, ts},
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

pub struct NotesWindow {
    notes: Notes,
    tx: UnboundedSender<Msg>,
    pub theme: config::Theme,
    doc: Option<Doc>,
    /// Bumped by every edit and every move: a settle timer acts only if nothing came after it.
    edits: u64,
    moves: u64,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl NotesWindow {
    pub fn new(notes: Notes, tx: UnboundedSender<Msg>, theme: config::Theme, window: &mut Window, cx: &mut Context<Self>) -> Self {
        win::round_corners(win::hwnd(window));
        let subs = vec![
            cx.observe_window_bounds(window, |this, window, cx| this.moved(window, cx)),
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
        NotesWindow { notes, tx, theme, doc: None, edits: 0, moves: 0, focus: cx.focus_handle(), _subs: subs }
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
        cx.notify();
    }

    /// Esc, ×, Alt+F4: the note and the window's place are written, the window hides.
    pub fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save(cx);
        let place = Place::of(window.window_bounds().get_bounds());
        cx.background_executor().spawn(async move { drop(save_place(&place_path(), place)) }).detach();
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
    #[expect(dead_code, reason = "Task 19's editor calls it after every change")]
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
            _ => return,
        }
        cx.stop_propagation();
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

/// The note's lines as plain text (Task 19 renders the markdown); an empty note, the placeholder.
fn body(text: &str, t: &Tokens) -> impl IntoElement + use<> {
    let b = div().flex_1().min_h_0().overflow_hidden().p(px(sp::S16)).flex().flex_col();
    if text.is_empty() {
        return b.child(div().text_color(t.text_faint).child("Start writing…"));
    }
    // An empty line still takes a line's height.
    b.text_color(t.text).children(text.lines().map(|l| div().child(if l.is_empty() { " ".to_string() } else { l.to_string() })))
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
        design::glass_window(&t)
            .size_full()
            .font_family(design::FONT_UI)
            .text_size(px(ts::BODY))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .child(self.header(title_of(&text), &t, cx))
            .child(body(&text, &t))
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
