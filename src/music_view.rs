//! YouTube Music inside the launcher: its screens, and what the companion's answers and the
//! media session's events do to them.

use std::{
    collections::{HashMap, HashSet},
    mem,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use gpui::{Context, Div, IntoElement, ObjectFit, ParentElement, RenderImage, Styled, StyledImage, div, img};
use serde_json::Value;

use crate::{
    Launcher, Mode, Tone,
    commands::{self, Cmd},
    design::{self, Tokens, g, px},
    empty_row, ext,
    media::{Media, NowPlaying, Transport},
    net,
    search::{self, Act, Icon, Item},
    win::Msg,
    ytmusic::{self, Control, Filter, Kind, MusicItem, Reply, Screen, Shelf},
};

/// Rows past this many show a glyph instead of a cover, so the cover cache never churns.
const COVERS: usize = 200;
/// Grid covers are decoded at this size (list rows use the 64 px icon size).
const GRID_COVER: u32 = 160;
/// How long typing must pause before a search is sent.
const TYPING_PAUSE: Duration = Duration::from_millis(300);

const CELL_PAD: f32 = 6.;
const CELL_GAP: f32 = 4.;
const TITLE_HEIGHT: f32 = 20.;
const SUBTITLE_HEIGHT: f32 = 18.;
/// The grid's horizontal padding (both sides) and the window's border.
const GRID_INSET: f32 = 18.;

#[derive(Clone, Debug, PartialEq)]
pub enum MusicAct {
    Open(MusicItem),
    Play { video: Option<String>, playlist: Option<String>, radio: bool },
    /// "like", "dislike" or "unlike", for a video (None: the track now playing).
    Rate(&'static str, Option<String>),
    /// A playlist id and a video (None: the track now playing).
    AddToPlaylist(String, Option<String>),
    Control(Control),
}

impl MusicAct {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Open(_) => "Open",
            Self::Play { .. } => "Play",
            Self::Rate("dislike", _) => "Dislike",
            Self::Rate("unlike", _) => "Remove Like",
            Self::Rate(..) => "Like",
            Self::AddToPlaylist(..) => "Add to Playlist",
            Self::Control(control) => control.title(),
        }
    }
}

/// An album, playlist or artist page on top of a screen.
struct Page {
    title: String,
    playlist: Option<String>,
}

/// A screen left for a page: what to put back when the page closes.
struct Frame {
    page: Option<Page>,
    items: Vec<MusicItem>,
    query: String,
    searched: String,
    selected: usize,
}

/// What Search and Library show; opening a screen starts it afresh.
#[derive(Default)]
struct View {
    /// The request whose reply fills the view.
    pending: Option<u64>,
    loading: bool,
    items: Vec<MusicItem>,
    error: Option<String>,
    filter: Filter,
    shelf: Shelf,
    /// The query `items` answer (or are being fetched for).
    searched: String,
    page: Option<Page>,
    stack: Vec<Frame>,
    /// The row to select once the query a closed page restores has been typed into the bar.
    reselect: Option<usize>,
}

#[derive(Default)]
pub struct MusicState {
    screen: Screen,
    /// The last request id handed out.
    req: u64,
    /// Counts query changes: a search waits out the typing pause and runs only if none came since.
    generation: u64,
    /// Volume requests in flight: their replies say the new level.
    volume_reqs: HashSet<u64>,
    /// Add to Playlist requests in flight, with the playlist's name.
    added: HashMap<u64, String>,
    view: View,
    /// The playlists in the library, for the Add to Playlist actions.
    playlists: Vec<MusicItem>,
    covers: HashMap<PathBuf, Option<Arc<RenderImage>>>,
    now: Option<NowPlaying>,
}

impl MusicState {
    /// A cover file changed on disk.
    pub fn drop_cover(&mut self, path: &Path) {
        self.covers.remove(path);
    }
}

impl Launcher {
    fn media(&mut self) -> &Media {
        let tx = &self.tx;
        self.media.get_or_insert_with(|| Media::start(tx.clone()))
    }

    pub fn music_open(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.media();
        self.music.screen = screen;
        self.music.view = View::default();
        self.music.generation += 1;
        self.music.playlists = match screen {
            Screen::Search | Screen::Library => ytmusic::load_cached(Shelf::Playlists.browse_id()).map(|(items, _)| items).unwrap_or_default(),
            Screen::NowPlaying | Screen::QuickActions => vec![],
        };
        if screen == Screen::Library {
            self.music_shelf();
        }
        self.set_mode(Mode::Music, cx);
    }

    pub fn music_label(&self) -> String {
        if let Some(page) = &self.music.view.page {
            return page.title.clone();
        }
        let screen = match self.music.screen {
            Screen::Search => "Search",
            Screen::Library => "Library",
            Screen::NowPlaying => "Now Playing",
            Screen::QuickActions => "Quick Actions",
        };
        format!("YouTube Music · {screen}")
    }

    pub fn music_placeholder(&self) -> String {
        let view = &self.music.view;
        match self.music.screen {
            Screen::NowPlaying | Screen::QuickActions => "Search actions…".into(),
            _ if view.page.is_some() => "Search this page…".into(),
            Screen::Search => view.filter.placeholder(),
            Screen::Library => view.shelf.placeholder(),
        }
    }

    pub fn music_loading(&self) -> bool {
        self.music.view.loading
    }

    /// Tab changes what Search looks for and which shelf Library shows (not on a page).
    pub fn music_tabs(&self) -> bool {
        self.mode == Mode::Music && matches!(self.music.screen, Screen::Search | Screen::Library) && self.music.view.page.is_none()
    }

    /// Albums and playlists are drawn as a grid of covers.
    pub fn music_grid(&self) -> bool {
        let view = &self.music.view;
        self.mode == Mode::Music
            && view.page.is_none()
            && match self.music.screen {
                Screen::Search => matches!(view.filter, Filter::Albums | Filter::Playlists),
                Screen::Library => matches!(view.shelf, Shelf::Albums | Shelf::Playlists),
                Screen::NowPlaying | Screen::QuickActions => false,
            }
    }

    pub fn music_rows(&mut self, query: &str) -> Vec<Item> {
        let mut rows = match self.music.screen {
            Screen::NowPlaying => vec![self.track_row()],
            Screen::QuickActions => vec![],
            Screen::Search | Screen::Library => return self.music_list(query),
        };
        let connected = self.browsers.any();
        let controls = control_rows(self.cfg.ytmusic.volume_step).into_iter().filter(|row| connected || matches!(row.act, Act::Music(MusicAct::Control(c)) if transport(c).is_some())).collect();
        rows.extend(search::filter(query, controls, &mut self.matcher));
        if !connected {
            rows.push(no_browser());
        }
        rows
    }

    fn track_row(&mut self) -> Item {
        let Some(now) = self.music.now.clone() else {
            return empty_row(g::PLAY, "Nothing playing".into(), "Play something on music.youtube.com".into(), vec![]);
        };
        let icon = now.cover.clone().and_then(|path| self.image(path)).map_or(Icon::Vector(g::PLAY), Icon::Image);
        track_item(&now, icon)
    }

    fn music_list(&mut self, query: &str) -> Vec<Item> {
        let items = self.music.view.items.clone();
        let grid = self.music_grid();
        let page = self.music.view.page.as_ref().map(|page| (page.title.clone(), page.playlist.clone()));
        let local = page.is_some() || self.music.screen == Screen::Library;
        let (title, playlist) = page.unwrap_or_default();
        let section = self.music_section();
        let mut rows: Vec<Item> = items
            .iter()
            .enumerate()
            .map(|(at, item)| {
                let row = self.music_row(item, at, grid, playlist.as_deref());
                match section {
                    Some(section) => row.sec(section),
                    None => row,
                }
            })
            .collect();
        if local {
            rows = search::filter(query, rows, &mut self.matcher);
        }
        if rows.is_empty() {
            return self.music_none(query);
        }
        if let Some(playlist) = playlist.filter(|_| query.trim().is_empty()) {
            rows.insert(0, Item::new("Play All", title, Icon::Vector(g::PLAY), Act::Music(MusicAct::Play { video: None, playlist: Some(playlist), radio: false })));
        }
        rows
    }

    fn music_section(&self) -> Option<&'static str> {
        let view = &self.music.view;
        match self.music.screen {
            _ if view.page.is_some() => None,
            Screen::Search => Some(view.filter.label()),
            Screen::Library => Some(view.shelf.label()),
            Screen::NowPlaying | Screen::QuickActions => None,
        }
    }

    fn music_row(&mut self, item: &MusicItem, at: usize, grid: bool, page: Option<&str>) -> Item {
        let icon = self.music_icon(item, at, grid);
        let actions = item_actions(item, page, &self.music.playlists);
        let mut row = Item::new(&item.title, &item.subtitle, icon, actions[0].1.clone()).acc(&item.duration).with_hay(format!("{} {}", item.title, item.subtitle));
        row.extra = actions;
        row
    }

    fn music_icon(&mut self, item: &MusicItem, at: usize, grid: bool) -> Icon {
        let glyph = match item.kind {
            Kind::Song | Kind::Album => g::AUDIO,
            Kind::Playlist => g::BULLETS,
            Kind::Artist => g::STAR,
        };
        match (at < COVERS, grid) {
            (false, _) => Icon::Vector(glyph),
            (true, true) => self.music_cover(&item.thumb, glyph),
            (true, false) => self.web_icon(&item.thumb, glyph),
        }
    }

    /// A web image from the cache at grid size, `glyph` until it is downloaded.
    fn music_cover(&mut self, url: &str, glyph: &'static str) -> Icon {
        if !url.starts_with("http") {
            return Icon::Vector(glyph);
        }
        let path = net::image_path(url);
        if !path.exists() {
            net::fetch_image(url, &self.tx);
            return Icon::Vector(glyph);
        }
        let covers = &mut self.music.covers;
        if covers.len() > COVERS && !covers.contains_key(&path) {
            covers.clear();
        }
        let image = covers.entry(path.clone()).or_insert_with(|| ext::load_image_sized(&path, GRID_COVER)).clone();
        image.map_or(Icon::Vector(glyph), Icon::Image)
    }

    /// The one row that says why the list is empty.
    fn music_none(&self, query: &str) -> Vec<Item> {
        let view = &self.music.view;
        let searching = self.music.screen == Screen::Search && view.page.is_none();
        let typed = !query.trim().is_empty();
        if searching && !typed {
            return vec![empty_row(g::SEARCH, "Search YouTube Music".into(), "Songs, albums, artists or playlists · Tab changes what to search".into(), vec![])];
        }
        if !self.browsers.any() {
            return vec![no_browser()];
        }
        let waiting = view.loading || searching && query.trim() != view.searched;
        let (glyph, title, hint) = if let Some(error) = &view.error {
            (g::WARNING, "Couldn't load YouTube Music", error.as_str())
        } else if waiting {
            (g::CLOCK, if searching { "Searching…" } else { "Loading…" }, "")
        } else if searching {
            (g::SEARCH, "No results", "Try another search")
        } else if typed {
            (g::SEARCH, "No matches", "")
        } else {
            (g::AUDIO, "Nothing here yet", "")
        };
        vec![empty_row(glyph, title.into(), hint.into(), vec![])]
    }

    /// Esc on a page: back to the screen under it. False when there is none.
    pub fn music_back(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(frame) = self.music.view.stack.pop() else { return false };
        self.music.generation += 1;
        let view = &mut self.music.view;
        view.page = frame.page;
        view.items = frame.items;
        view.searched = frame.searched;
        view.pending = None;
        view.loading = false;
        view.error = None;
        view.reselect = Some(frame.selected);
        self.input.update(cx, |input, cx| input.emplace(&frame.query, cx));
        self.refresh(cx);
        true
    }

    fn music_page(&mut self, item: MusicItem, cx: &mut Context<Self>) {
        let (query, selected) = (self.query(cx), self.selected);
        let view = &mut self.music.view;
        let page = Page { title: item.title.clone(), playlist: playlist_of(&item) };
        let frame = Frame { page: view.page.replace(page), items: mem::take(&mut view.items), query, searched: mem::take(&mut view.searched), selected };
        view.stack.push(frame);
        self.music_fetch(|req| ytmusic::browse(req, &item.id));
        self.input.update(cx, |input, cx| input.emplace("", cx));
        self.selected = 0;
        self.refresh(cx);
    }

    pub fn music_act(&mut self, act: MusicAct, cx: &mut Context<Self>) {
        let sent = match act {
            MusicAct::Control(control) => return self.music_control(control, cx),
            MusicAct::Open(item) => return self.music_page(item, cx),
            MusicAct::Play { video, playlist, radio } => self.music_ask(|req| Some(ytmusic::play(req, video.as_deref(), playlist.as_deref(), radio))),
            MusicAct::Rate(op, video) => self.music_ask(|req| Some(ytmusic::rate(req, op, video.as_deref()))),
            MusicAct::AddToPlaylist(playlist, video) => {
                let sent = self.music_ask(|req| Some(ytmusic::add_to_playlist(req, &playlist, video.as_deref())));
                if sent {
                    let name = self.music.playlists.iter().find(|item| item.id == playlist).map(|item| item.title.clone());
                    self.music.added.insert(self.music.req, name.unwrap_or_default());
                }
                sent
            }
        };
        self.music_done(sent, cx);
    }

    fn music_control(&mut self, control: Control, cx: &mut Context<Self>) {
        let step = self.cfg.ytmusic.volume_step;
        let sent = match transport(control) {
            Some(transport) => {
                self.media().send(transport);
                true
            }
            None => {
                self.media();
                let sent = self.music_ask(|req| ytmusic::control(req, control, step));
                if sent && announces_volume(control) {
                    self.music.volume_reqs.insert(self.music.req);
                }
                sent
            }
        };
        self.music_done(sent, cx);
    }

    /// Sends a request to the newest connected browser; false if there is none.
    fn music_ask(&mut self, request: impl FnOnce(u64) -> Option<Value>) -> bool {
        self.music.req += 1;
        request(self.music.req).is_some_and(|request| self.browsers.ask(None, request))
    }

    /// Asks for what the view shows; its reply fills the view.
    fn music_fetch(&mut self, request: impl FnOnce(u64) -> Value) {
        let sent = self.music_ask(|req| Some(request(req)));
        let view = &mut self.music.view;
        view.pending = sent.then_some(self.music.req);
        view.loading = sent;
        view.error = None;
    }

    fn music_done(&mut self, sent: bool, cx: &mut Context<Self>) {
        if !sent {
            let message = "No browser connected: run Set Up Browser Extension".to_string();
            if self.visible && self.mode == Mode::Music {
                self.status = Some((Tone::Error, message));
                return cx.notify();
            }
            let _ = self.tx.unbounded_send(Msg::Toast(false, message));
        }
        self.hide(true, cx);
    }

    /// The query changed: Search asks for it once typing pauses, and a page just closed
    /// gets its row back.
    pub fn music_typed(&mut self, cx: &mut Context<Self>) {
        if self.mode != Mode::Music {
            return;
        }
        if let Some(selected) = self.music.view.reselect.take() {
            self.selected = selected.min(self.items.len().saturating_sub(1));
            self.scroll_to_selected();
            self.grid_reveal();
            cx.notify();
        }
        if self.music.screen != Screen::Search || self.music.view.page.is_some() {
            return;
        }
        self.music.generation += 1;
        let query = self.query(cx).trim().to_string();
        if query == self.music.view.searched {
            return;
        }
        if query.is_empty() {
            let view = &mut self.music.view;
            view.searched.clear();
            view.items.clear();
            view.pending = None;
            view.loading = false;
            view.error = None;
            return self.refresh(cx);
        }
        let generation = self.music.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPING_PAUSE).await;
            this.update(cx, |this, cx| {
                if this.music.generation == generation {
                    this.music_search(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn music_search(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx).trim().to_string();
        if query.is_empty() || !self.visible || !self.music_tabs() || self.music.screen != Screen::Search {
            return;
        }
        let filter = self.music.view.filter;
        self.music.view.searched = query.clone();
        self.music_fetch(|req| ytmusic::search(req, &query, filter));
        self.refresh(cx);
    }

    /// Tab: the next thing to search for, or the next library shelf.
    pub fn music_cycle(&mut self, cx: &mut Context<Self>) {
        self.music.generation += 1;
        let view = &mut self.music.view;
        view.pending = None;
        view.loading = false;
        self.selected = 0;
        match self.music.screen {
            Screen::Search => {
                view.filter = view.filter.next();
                self.music_search(cx);
            }
            Screen::Library => {
                view.shelf = view.shelf.next();
                self.music_shelf();
            }
            Screen::NowPlaying | Screen::QuickActions => {}
        }
        self.refresh(cx);
    }

    /// Shows the saved list of the shelf, and fetches it again when there is none or it is old.
    fn music_shelf(&mut self) {
        let id = self.music.view.shelf.browse_id();
        let cached = ytmusic::load_cached(id);
        let stale = cached.as_ref().is_none_or(|(_, age)| *age > ytmusic::STALE);
        let view = &mut self.music.view;
        view.items = cached.map(|(items, _)| items).unwrap_or_default();
        view.error = None;
        view.pending = None;
        view.loading = false;
        if stale {
            self.music_fetch(|req| ytmusic::browse(req, id));
        }
    }

    pub fn music_reply(&mut self, reply: Reply, cx: &mut Context<Self>) {
        if matches!(reply.op.as_str(), "search" | "browse") {
            if self.music.view.pending == Some(reply.req) {
                self.music_fill(reply, cx);
            }
            return;
        }
        let volume = self.music.volume_reqs.remove(&reply.req);
        let added = self.music.added.remove(&reply.req);
        if let Some((ok, text)) = toast(&reply, volume, added.as_deref()) {
            let _ = self.tx.unbounded_send(Msg::Toast(ok, text));
        }
    }

    fn music_fill(&mut self, reply: Reply, cx: &mut Context<Self>) {
        let screen = self.music.screen;
        let view = &mut self.music.view;
        view.pending = None;
        view.loading = false;
        view.error = reply.error;
        match &mut view.page {
            Some(page) if !reply.title.is_empty() => page.title = reply.title,
            None if screen == Screen::Library && view.error.is_none() => {
                ytmusic::save_cached(view.shelf.browse_id(), &reply.items);
                if view.shelf == Shelf::Playlists {
                    self.music.playlists = reply.items.clone();
                }
            }
            _ => {}
        }
        view.items = reply.items;
        self.selected = 0;
        if self.visible && self.mode == Mode::Music {
            self.refresh(cx);
        }
    }

    pub fn music_now(&mut self, now: Option<NowPlaying>, cx: &mut Context<Self>) {
        self.music.now = now;
        if self.visible && self.mode == Mode::Music && self.music.screen == Screen::NowPlaying {
            self.refresh(cx);
        }
    }
}

fn no_browser() -> Item {
    empty_row(g::GLOBE, "No browser connected".into(), "Run Set Up Browser Extension, then load the extension in Chrome, Edge or Brave".into(), vec![])
}

/// The playlist an album or playlist plays: its own, or the one its browse id names.
fn playlist_of(item: &MusicItem) -> Option<String> {
    if !matches!(item.kind, Kind::Album | Kind::Playlist) {
        return None;
    }
    Some(item.playlist_id.clone()).filter(|id| !id.is_empty()).or_else(|| item.id.strip_prefix("VL").map(String::from))
}

/// The Ctrl+K actions of a result, the one Enter runs first. `page` is the playlist of the
/// page a song is on; `playlists` the library's.
fn item_actions(item: &MusicItem, page: Option<&str>, playlists: &[MusicItem]) -> Vec<(String, Act)> {
    let act = |label: &str, act: MusicAct| (label.to_string(), Act::Music(act));
    let mut actions = match item.kind {
        Kind::Song => {
            let video = Some(item.id.clone());
            let mut actions = vec![
                act("Play", MusicAct::Play { video: video.clone(), playlist: page.map(String::from), radio: false }),
                act("Start Radio", MusicAct::Play { video: video.clone(), playlist: None, radio: true }),
                act("Like", MusicAct::Rate("like", video.clone())),
                act("Dislike", MusicAct::Rate("dislike", video.clone())),
            ];
            let playlists = playlists.iter().filter(|playlist| playlist.id.starts_with("VLPL"));
            actions.extend(playlists.map(|playlist| act(&format!("Add to Playlist › {}", playlist.title), MusicAct::AddToPlaylist(playlist.id.clone(), video.clone()))));
            actions
        }
        Kind::Album | Kind::Playlist => {
            let play = playlist_of(item).map(|playlist| act("Play", MusicAct::Play { video: None, playlist: Some(playlist), radio: false }));
            std::iter::once(act("Open", MusicAct::Open(item.clone()))).chain(play).collect()
        }
        Kind::Artist => vec![act("Open", MusicAct::Open(item.clone()))],
    };
    actions.push(("Copy URL".into(), Act::Copy(item.url())));
    actions.push(("Open in YouTube Music".into(), Act::Open(item.url())));
    actions
}

/// An album cell's width: `columns` of them side by side across a window `window` wide.
pub fn cell_width(window: f32, columns: usize) -> f32 {
    let gaps = CELL_GAP * columns.saturating_sub(1) as f32;
    ((window - GRID_INSET - gaps) / columns.max(1) as f32).floor()
}

fn cell_height(width: f32) -> f32 {
    width + 2. * CELL_GAP + TITLE_HEIGHT + SUBTITLE_HEIGHT
}

/// An album or playlist in the grid: its cover, then its title and subtitle on one line each.
pub fn cover_cell(icon: &Icon, title: &str, subtitle: &str, width: f32, t: &Tokens) -> Div {
    let side = width - 2. * CELL_PAD;
    let cover = match icon {
        Icon::Image(image) => img(image.clone()).size(px(side)).object_fit(ObjectFit::Cover).rounded(px(design::r::SM)).into_any_element(),
        Icon::Vector(name) => div().size(px(side)).flex().items_center().justify_center().rounded(px(design::r::SM)).bg(t.surface).child(design::icon(name, design::ic::XL, t.text_muted)).into_any_element(),
        Icon::Glyph(_) => div().size(px(side)).into_any_element(),
    };
    div()
        .w(px(width))
        .h(px(cell_height(width)))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(CELL_GAP))
        .p(px(CELL_PAD))
        .child(cover)
        .child(div().h(px(TITLE_HEIGHT)).truncate().text_color(t.text).child(title.to_string()))
        .child(div().h(px(SUBTITLE_HEIGHT)).truncate().text_size(px(design::ts::BODY_SM)).text_color(t.text_muted).child(subtitle.to_string()))
}

fn transport(control: Control) -> Option<Transport> {
    match control {
        Control::PlayPause => Some(Transport::PlayPause),
        Control::Next => Some(Transport::Next),
        Control::Previous => Some(Transport::Previous),
        _ => None,
    }
}

fn announces_volume(control: Control) -> bool {
    matches!(control, Control::Mute | Control::VolumeLow | Control::VolumeMedium | Control::VolumeHigh | Control::VolumeUp | Control::VolumeDown)
}

/// dded is the name of the playlist an Add to Playlist request named.
fn toast(reply: &Reply, volume: bool, added: Option<&str>) -> Option<(bool, String)> {
    if let Some(error) = &reply.error {
        return Some((false, error.clone()));
    }
    match reply.op.as_str() {
        "like" => Some((true, "Liked".into())),
        "dislike" => Some((true, "Disliked".into())),
        "addToPlaylist" => Some((true, added.filter(|name| !name.is_empty()).map_or_else(|| "Added to playlist".into(), |name| format!("Added to {name}")))),
        "control" if volume => reply.current.as_ref().map(|c| (true, if c.muted { "Muted".into() } else { format!("Volume {}%", c.volume) })),
        _ => None,
    }
}

fn now_subtitle(now: &NowPlaying) -> String {
    [now.artist.as_str(), now.album.as_str()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" · ")
}

fn track_item(now: &NowPlaying, icon: Icon) -> Item {
    let play_pause = MusicAct::Control(Control::PlayPause);
    let radio = MusicAct::Play { video: None, playlist: None, radio: true };
    let mut item = Item::new(&now.title, now_subtitle(now), icon, Act::Music(play_pause.clone())).acc(if now.playing { "Playing" } else { "Paused" }).sec("Now Playing");
    item.extra = [("Play/Pause", play_pause), ("Like", MusicAct::Control(Control::Like)), ("Dislike", MusicAct::Control(Control::Dislike)), ("Start Radio", radio)]
        .map(|(label, act)| (label.to_string(), Act::Music(act)))
        .to_vec();
    item
}

fn control_rows(step: u32) -> Vec<Item> {
    Control::ALL
        .iter()
        .map(|&control| {
            let command = Cmd::YtControl(control);
            let glyph = commands::ALL.iter().find(|entry| entry.0 == command).map_or(g::PLAY, |entry| entry.3);
            Item::new(control.title(), control.subtitle(step), Icon::Vector(glyph), Act::Music(MusicAct::Control(control))).sec("Quick Actions").tinted(command.tint())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ytmusic::Current;

    fn now(title: &str, artist: &str, album: &str, playing: bool) -> NowPlaying {
        NowPlaying { title: title.into(), artist: artist.into(), album: album.into(), playing, cover: None }
    }

    fn reply(op: &str, error: Option<&str>, current: Option<Current>) -> Reply {
        Reply { op: op.into(), error: error.map(String::from), current, ..Default::default() }
    }

    #[test]
    fn subtitle_skips_empty_parts() {
        assert_eq!(now_subtitle(&now("One", "Band", "Record", true)), "Band · Record");
        assert_eq!(now_subtitle(&now("One", "Band", "", true)), "Band");
        assert_eq!(now_subtitle(&now("One", "", "Record", true)), "Record");
        assert_eq!(now_subtitle(&now("One", "", "", true)), "");
    }

    #[test]
    fn the_track_row_plays_and_pauses() {
        let row = track_item(&now("One", "Band", "Record", true), Icon::Vector(g::PLAY));
        assert_eq!((row.title.as_str(), row.subtitle.as_str(), row.accessory.as_str()), ("One", "Band · Record", "Playing"));
        assert_eq!(row.act, Act::Music(MusicAct::Control(Control::PlayPause)));
        let labels: Vec<_> = row.extra.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["Play/Pause", "Like", "Dislike", "Start Radio"]);
        assert_eq!(row.extra[3].1, Act::Music(MusicAct::Play { video: None, playlist: None, radio: true }));
        assert_eq!(track_item(&now("One", "", "", false), Icon::Vector(g::PLAY)).accessory, "Paused");
    }

    #[test]
    fn every_control_has_a_row() {
        let rows = control_rows(15);
        assert_eq!(rows.len(), Control::ALL.len());
        let volume_up = rows.iter().find(|row| row.title == "Volume Up").unwrap();
        assert_eq!(volume_up.subtitle, "Increase the volume by 15%");
        assert_eq!(volume_up.act, Act::Music(MusicAct::Control(Control::VolumeUp)));
        assert!(rows.iter().all(|row| row.section.as_deref() == Some("Quick Actions")));
    }

    #[test]
    fn controls_go_through_the_right_channel() {
        assert!(matches!(transport(Control::PlayPause), Some(Transport::PlayPause)));
        assert!(matches!(transport(Control::Next), Some(Transport::Next)));
        assert!(matches!(transport(Control::Previous), Some(Transport::Previous)));
        assert!(Control::ALL.iter().all(|&c| transport(c).is_some() == ytmusic::control(1, c, 10).is_none()));
        assert!(announces_volume(Control::VolumeUp) && announces_volume(Control::Mute));
        assert!(!announces_volume(Control::Shuffle) && !announces_volume(Control::Like));
    }

    #[test]
    fn actions_are_labelled() {
        let label = |act: MusicAct| Act::Music(act).label();
        assert_eq!(label(MusicAct::Open(MusicItem::default())), "Open");
        assert_eq!(label(MusicAct::Play { video: None, playlist: None, radio: false }), "Play");
        assert_eq!((label(MusicAct::Rate("like", None)), label(MusicAct::Rate("dislike", None)), label(MusicAct::Rate("unlike", None))), ("Like", "Dislike", "Remove Like"));
        assert_eq!(label(MusicAct::AddToPlaylist("PL1".into(), None)), "Add to Playlist");
        assert_eq!(label(MusicAct::Control(Control::VolumeDown)), "Volume Down");
        assert_eq!(Act::Cmd(Cmd::YtMusic(Screen::NowPlaying)).label(), "Open");
    }

    #[test]
    fn replies_become_toasts() {
        let level = Some(Current { volume: 40, ..Default::default() });
        let muted = Some(Current { muted: true, ..Default::default() });
        assert_eq!(toast(&reply("like", None, None), false, None), Some((true, "Liked".into())));
        assert_eq!(toast(&reply("dislike", None, None), false, None), Some((true, "Disliked".into())));
        assert_eq!(toast(&reply("control", None, level.clone()), true, None), Some((true, "Volume 40%".into())));
        assert_eq!(toast(&reply("control", None, muted), true, None), Some((true, "Muted".into())));
        assert_eq!(toast(&reply("control", None, level.clone()), false, None), None, "shuffle and repeat stay quiet");
        assert_eq!(toast(&reply("control", None, None), true, None), None);
        assert_eq!(toast(&reply("play", None, None), false, None), None);
        assert_eq!(toast(&reply("control", Some("Open YouTube Music first."), level), true, None), Some((false, "Open YouTube Music first.".into())));
        assert_eq!(toast(&reply("addToPlaylist", None, None), false, Some("Road Trip")), Some((true, "Added to Road Trip".into())));
        assert_eq!(toast(&reply("addToPlaylist", None, None), false, Some("")), Some((true, "Added to playlist".into())));
        assert_eq!(toast(&reply("addToPlaylist", Some("Sign in first."), None), false, Some("Road Trip")), Some((false, "Sign in first.".into())));
    }

    fn item(kind: Kind, id: &str, playlist: &str) -> MusicItem {
        MusicItem { kind, id: id.into(), playlist_id: playlist.into(), title: format!("Title {id}"), ..Default::default() }
    }

    fn labels(actions: &[(String, Act)]) -> Vec<&str> {
        actions.iter().map(|(label, _)| label.as_str()).collect()
    }

    #[test]
    fn a_song_offers_playback_rating_playlists_and_links() {
        let playlists = [item(Kind::Playlist, "VLPL1", "PL1"), item(Kind::Playlist, "VLLM", "LM"), item(Kind::Playlist, "VLPL2", "PL2")];
        let song = item(Kind::Song, "abc", "");
        let actions = item_actions(&song, Some("PLpage"), &playlists);
        assert_eq!(labels(&actions), ["Play", "Start Radio", "Like", "Dislike", "Add to Playlist › Title VLPL1", "Add to Playlist › Title VLPL2", "Copy URL", "Open in YouTube Music"]);
        assert_eq!(actions[0].1, Act::Music(MusicAct::Play { video: Some("abc".into()), playlist: Some("PLpage".into()), radio: false }));
        assert_eq!(actions[1].1, Act::Music(MusicAct::Play { video: Some("abc".into()), playlist: None, radio: true }));
        assert_eq!(actions[2].1, Act::Music(MusicAct::Rate("like", Some("abc".into()))));
        assert_eq!(actions[4].1, Act::Music(MusicAct::AddToPlaylist("VLPL1".into(), Some("abc".into()))));
        assert_eq!(actions[6].1, Act::Copy("https://music.youtube.com/watch?v=abc".into()));
        assert_eq!(actions[7].1, Act::Open("https://music.youtube.com/watch?v=abc".into()));
        assert_eq!(labels(&item_actions(&song, None, &[])).len(), 6, "no playlists, no submenu rows");
    }

    #[test]
    fn an_album_opens_on_enter_and_plays_on_ctrl_enter() {
        let album = item(Kind::Album, "MPREb_1", "OLAK1");
        let actions = item_actions(&album, None, &[item(Kind::Playlist, "VLPL1", "PL1")]);
        assert_eq!(labels(&actions), ["Open", "Play", "Copy URL", "Open in YouTube Music"]);
        assert_eq!(actions[0].1, Act::Music(MusicAct::Open(album.clone())));
        assert_eq!(actions[1].1, Act::Music(MusicAct::Play { video: None, playlist: Some("OLAK1".into()), radio: false }));
        assert_eq!(actions[2].1, Act::Copy("https://music.youtube.com/browse/MPREb_1".into()));
        let playlist = item(Kind::Playlist, "VLPL9", "");
        assert_eq!(item_actions(&playlist, None, &[])[1].1, Act::Music(MusicAct::Play { video: None, playlist: Some("PL9".into()), radio: false }), "the browse id names the playlist");
        assert_eq!(labels(&item_actions(&item(Kind::Album, "MPREb_2", ""), None, &[])), ["Open", "Copy URL", "Open in YouTube Music"]);
    }

    #[test]
    fn an_artist_only_opens() {
        let actions = item_actions(&item(Kind::Artist, "UC1", "RDAO1"), None, &[]);
        assert_eq!(labels(&actions), ["Open", "Copy URL", "Open in YouTube Music"]);
        assert_eq!(actions[1].1, Act::Copy("https://music.youtube.com/browse/UC1".into()));
        assert_eq!(playlist_of(&item(Kind::Artist, "UC1", "RDAO1")), None);
        assert_eq!(playlist_of(&item(Kind::Song, "abc", "PL1")), None);
    }

    #[test]
    fn cells_fill_the_window_width() {
        let width = cell_width(750., 5);
        assert!((width * 5. + 4. * 4. + GRID_INSET) <= 750. && width > 130.);
        assert!(cell_width(750., 3) > cell_width(750., 8));
        assert_eq!(cell_width(750., 0), cell_width(750., 1));
        assert_eq!(cell_height(100.), 100. + 2. * CELL_GAP + TITLE_HEIGHT + SUBTITLE_HEIGHT);
    }
}