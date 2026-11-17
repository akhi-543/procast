//! YouTube Music inside the launcher: its screens, and what the companion's answers and the
//! media session's events do to them.

use std::{
    collections::{HashMap, HashSet},
    mem,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use gpui::{ClipboardItem, Context, Div, IntoElement, ObjectFit, ParentElement, RenderImage, Styled, StyledImage, Window, div, img};
use serde_json::Value;
use tray_icon::TrayIcon;

use crate::{
    Launcher, Mode, Tone,
    commands::{self, Cmd},
    design::{self, Tokens, g, px},
    empty_row, ext,
    media::{Media, NowPlaying, Transport},
    music_tray::{self, Action},
    net,
    search::{self, Act, Icon, Item},
    win::{self, Msg},
    ytmusic::{self, Control, Filter, Kind, MusicItem, Reply, Screen, Shelf},
};

/// Rows past this many show a glyph instead of a cover, so the cover cache never churns.
const COVERS: usize = 200;
/// Grid covers are decoded at this size (list rows use the 64 px icon size).
const GRID_COVER: u32 = 160;
/// How long typing must pause before a search is sent.
const TYPING_PAUSE: Duration = Duration::from_millis(300);
/// Arrived covers redraw the list at most this often.
const COVER_REFRESH: Duration = Duration::from_millis(100);

const NO_BROWSER: &str = "No browser connected: run Set Up Browser Extension";
const HOME: &str = "https://music.youtube.com/";

const CELL_PAD: f32 = 6.;
const CELL_GAP: f32 = 4.;
const TITLE_HEIGHT: f32 = 20.;
const SUBTITLE_HEIGHT: f32 = 18.;
/// The grid's horizontal padding (both sides) and the window's border.
const GRID_INSET: f32 = 18.;

/// What to do with the link of the track now playing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Link {
    Copy,
    Open,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MusicAct {
    Open(MusicItem),
    Play { video: Option<String>, playlist: Option<String>, radio: bool },
    /// "like", "dislike" or "unlike", for a video (None: the track now playing).
    Rate(&'static str, Option<String>),
    /// A playlist id and a video (None: the track now playing).
    AddToPlaylist(String, Option<String>),
    Control(Control),
    Current(Link),
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
            Self::Current(Link::Copy) => "Copy URL",
            Self::Current(Link::Open) => "Open in YouTube Music",
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

/// What the view wants from the companion.
#[derive(Clone, Debug, PartialEq)]
enum Ask {
    Search { query: String, filter: Filter },
    Shelf(Shelf),
    Page(String),
}

impl Ask {
    fn request(&self, req: u64) -> Value {
        match self {
            Self::Search { query, filter } => ytmusic::search(req, query, *filter),
            Self::Shelf(shelf) => ytmusic::browse(req, shelf.browse_id()),
            Self::Page(id) => ytmusic::browse(req, id),
        }
    }
}

/// A view request the companion has not answered yet; `shelf` is the library shelf it fetches.
struct Flight {
    req: u64,
    shelf: Option<Shelf>,
}

/// The companion answers one request at a time: one is in flight, and of the ones asked
/// meanwhile only the newest is kept.
#[derive(Default)]
struct Relay {
    flight: Option<Flight>,
    wanted: Option<Ask>,
}

impl Relay {
    /// What to send now, if anything: the wanted ask, once nothing is in flight.
    fn next(&mut self) -> Option<Ask> {
        if self.flight.is_some() { None } else { self.wanted.take() }
    }

    fn sent(&mut self, req: u64, ask: &Ask) {
        let shelf = if let Ask::Shelf(shelf) = ask { Some(*shelf) } else { None };
        self.flight = Some(Flight { req, shelf });
    }

    /// A reply came: the request it answered, if that is the one in flight.
    fn landed(&mut self, req: u64) -> Option<Flight> {
        self.flight.take_if(|flight| flight.req == req)
    }
}

#[derive(Default)]
pub struct MusicState {
    screen: Screen,
    /// The last request id handed out.
    req: u64,
    /// Counts query changes: a search waits out the typing pause and runs only if none came since.
    generation: u64,
    relay: Relay,
    /// Volume requests in flight: their replies say the new level.
    volume_reqs: HashSet<u64>,
    /// Add to Playlist requests in flight, with the playlist's name.
    added: HashMap<u64, String>,
    /// `current` requests in flight: their replies say which track to link to.
    links: HashMap<u64, Link>,
    view: View,
    /// The playlists in the library, for the Add to Playlist actions.
    playlists: Vec<MusicItem>,
    covers: HashMap<PathBuf, Option<Arc<RenderImage>>>,
    /// Covers out of `covers` whose texture a window still has to free.
    retired: Vec<Arc<RenderImage>>,
    /// A refresh for arrived covers is already waiting.
    refresh_due: bool,
    now: Option<NowPlaying>,
    tray: Option<TrayIcon>,
}

impl MusicState {
    /// A cover file changed on disk.
    pub fn drop_cover(&mut self, path: &Path) {
        self.retired.extend(self.covers.remove(path).flatten());
    }

    pub fn retire_covers(&mut self) {
        self.retired.extend(self.covers.drain().filter_map(|(_, image)| image));
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
        self.music.relay.wanted = None;
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
            .map(|item| {
                let row = music_row(item, playlist.as_deref(), &self.music.playlists);
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
        let by_id: HashMap<&str, &MusicItem> = items.iter().map(|item| (item.id.as_str(), item)).collect();
        for row in rows.iter_mut().take(COVERS) {
            if let Some(item) = music_id(&row.act).and_then(|id| by_id.get(id)) {
                row.icon = self.music_icon(item, grid);
            }
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

    fn music_icon(&mut self, item: &MusicItem, grid: bool) -> Icon {
        let glyph = glyph_of(item.kind);
        let Some(path) = net::cached_image(&item.thumb, &self.tx) else { return Icon::Vector(glyph) };
        let image = if grid { self.music_cover(path) } else { self.image(path) };
        image.map_or(Icon::Vector(glyph), Icon::Image)
    }

    /// A cached web image at grid size.
    fn music_cover(&mut self, path: PathBuf) -> Option<Arc<RenderImage>> {
        let covers = &mut self.music.covers;
        if covers.len() > COVERS && !covers.contains_key(&path) {
            self.music.retire_covers();
        }
        self.music.covers.entry(path.clone()).or_insert_with(|| ext::load_image_sized(&path, GRID_COVER)).clone()
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
        self.music.relay.wanted = None;
        self.input.update(cx, |input, cx| input.emplace(&frame.query, cx));
        self.refresh(cx);
        true
    }

    fn music_page(&mut self, item: MusicItem, cx: &mut Context<Self>) {
        let (query, selected) = (self.query(cx), self.selected);
        let view = &mut self.music.view;
        let page = Page { title: item.title.clone(), playlist: playlist_of(&item) };
        let frame = Frame { page: view.page.replace(page), items: mem::take(&mut view.items), query, searched: if view.loading { String::new() } else { mem::take(&mut view.searched) }, selected };
        view.stack.push(frame);
        self.music_fetch(Ask::Page(item.id));
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
                let name = playlist_name(&self.music.playlists, &playlist);
                self.music_add(&playlist, video.as_deref(), name)
            }
            MusicAct::Current(link) => self.music_link(link),
        };
        self.music_done(sent, cx);
    }

    fn music_add(&mut self, playlist: &str, video: Option<&str>, name: String) -> bool {
        let sent = self.music_ask(|req| Some(ytmusic::add_to_playlist(req, playlist, video)));
        if sent {
            self.music.added.insert(self.music.req, name);
        }
        sent
    }

    fn music_control(&mut self, control: Control, cx: &mut Context<Self>) {
        let sent = self.music_send_control(control);
        self.music_done(sent, cx);
    }

    fn music_send_control(&mut self, control: Control) -> bool {
        let step = self.cfg.ytmusic.volume_step;
        if let Some(transport) = transport(control) {
            self.media().send(transport);
            return true;
        }
        self.media();
        let sent = self.music_ask(|req| ytmusic::control(req, control, step));
        if sent && announces_volume(control) {
            self.music.volume_reqs.insert(self.music.req);
        }
        sent
    }

    /// Asks the browser which track plays, to copy or open its link when it answers. Opening
    /// without a browser goes to YouTube Music's home page; false when copying has nothing to ask.
    fn music_link(&mut self, link: Link) -> bool {
        let sent = self.music_ask(|req| Some(ytmusic::current(req)));
        if sent {
            self.music.links.insert(self.music.req, link);
        } else if link == Link::Open {
            win::open(HOME);
            return true;
        }
        sent
    }

    fn music_linked(&mut self, link: Link, reply: &Reply, cx: &mut Context<Self>) {
        let url = current_url(reply);
        let toast = match (link, url) {
            (Link::Open, url) => {
                win::open(url.as_deref().unwrap_or(HOME));
                None
            }
            (Link::Copy, Some(url)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(url));
                Some((true, "Copied".to_string()))
            }
            (Link::Copy, None) => Some((false, reply.error.clone().unwrap_or_else(|| "Nothing playing on YouTube Music".into()))),
        };
        if let Some((ok, text)) = toast {
            let _ = self.tx.unbounded_send(Msg::Toast(ok, text));
        }
    }

    /// Sends a request to the newest connected browser; false if there is none.
    fn music_ask(&mut self, request: impl FnOnce(u64) -> Option<Value>) -> bool {
        self.music.req += 1;
        request(self.music.req).is_some_and(|request| self.browsers.ask(None, request))
    }

    /// Asks for what the view shows. It goes out when the companion has answered what it was
    /// asked before; its reply then fills the view.
    fn music_fetch(&mut self, ask: Ask) {
        let view = &mut self.music.view;
        view.pending = None;
        view.loading = true;
        view.error = None;
        self.music.relay.wanted = Some(ask);
        self.music_send();
    }

    fn music_send(&mut self) {
        if !self.browsers.any() {
            self.music.relay.flight = None;
        }
        let Some(ask) = self.music.relay.next() else { return };
        let sent = self.music_ask(|req| Some(ask.request(req)));
        let view = &mut self.music.view;
        if sent {
            self.music.relay.sent(self.music.req, &ask);
            view.pending = Some(self.music.req);
        } else {
            view.loading = false;
        }
    }

    fn music_done(&mut self, sent: bool, cx: &mut Context<Self>) {
        if !sent {
            let message = NO_BROWSER.to_string();
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
            self.music.relay.wanted = None;
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
        self.music.view.searched.clone_from(&query);
        self.music_fetch(Ask::Search { query, filter });
        self.refresh(cx);
    }

    /// Tab: the next thing to search for, or the next library shelf.
    pub fn music_cycle(&mut self, cx: &mut Context<Self>) {
        self.music.generation += 1;
        let view = &mut self.music.view;
        view.pending = None;
        view.loading = false;
        self.music.relay.wanted = None;
        self.selected = 0;
        match self.music.screen {
            Screen::Search => {
                view.filter = view.filter.next();
                view.items.clear();
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
        let shelf = self.music.view.shelf;
        let cached = ytmusic::load_cached(shelf.browse_id());
        let stale = cached.as_ref().is_none_or(|(_, age)| *age > ytmusic::STALE);
        let view = &mut self.music.view;
        view.items = cached.map(|(items, _)| items).unwrap_or_default();
        view.error = None;
        view.pending = None;
        view.loading = false;
        self.music.relay.wanted = None;
        if stale {
            self.music_fetch(Ask::Shelf(shelf));
        }
    }

    pub fn music_reply(&mut self, reply: Reply, cx: &mut Context<Self>) {
        if matches!(reply.op.as_str(), "search" | "browse") {
            let flight = self.music.relay.landed(reply.req);
            if let Some(shelf) = flight.and_then(|flight| flight.shelf).filter(|_| reply.error.is_none()) {
                ytmusic::save_cached(shelf.browse_id(), &reply.items);
                if shelf == Shelf::Playlists {
                    self.music.playlists.clone_from(&reply.items);
                    self.music_tray_refresh();
                }
            }
            if self.music.view.pending == Some(reply.req) {
                self.music_fill(reply, cx);
            }
            self.music_send();
            return;
        }
        if let Some(link) = self.music.links.remove(&reply.req) {
            return self.music_linked(link, &reply, cx);
        }
        let volume = self.music.volume_reqs.remove(&reply.req);
        let added = self.music.added.remove(&reply.req);
        if let Some((ok, text)) = toast(&reply, volume, added.as_deref()) {
            let _ = self.tx.unbounded_send(Msg::Toast(ok, text));
        }
    }

    /// Puts a reply in the view. A refreshed library shelf keeps the row selected.
    fn music_fill(&mut self, reply: Reply, cx: &mut Context<Self>) {
        let refreshed = self.music.screen == Screen::Library && self.music.view.page.is_none();
        let kept = self.items.get(self.selected).and_then(|row| music_id(&row.act)).map(String::from).filter(|_| refreshed);
        let view = &mut self.music.view;
        view.pending = None;
        view.loading = false;
        view.error = reply.error;
        if let Some(page) = &mut view.page
            && !reply.title.is_empty()
        {
            page.title = reply.title;
        }
        view.items = reply.items;
        if !refreshed {
            self.selected = 0;
        }
        if self.visible && self.mode == Mode::Music {
            self.refresh(cx);
            if let Some(at) = kept.and_then(|id| row_of(&self.items, &id)) {
                self.selected = at;
                self.scroll_to_selected();
                self.grid_reveal();
            }
        }
    }
    /// A cover arrived: the list is redrawn after a moment, once for all that came meanwhile.
    pub fn music_covers_arrived(&mut self, cx: &mut Context<Self>) {
        if mem::replace(&mut self.music.refresh_due, true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COVER_REFRESH).await;
            this.update(cx, |this, cx| {
                this.music.refresh_due = false;
                if this.visible && this.mode == Mode::Music {
                    this.refresh(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Frees the textures of the covers the cache let go of.
    pub fn music_release(&mut self, window: &mut Window) {
        for image in mem::take(&mut self.music.retired) {
            let _ = window.drop_image(image);
        }
    }

    pub fn music_now(&mut self, now: Option<NowPlaying>, cx: &mut Context<Self>) {
        let changed = self.music.now != now;
        self.music.now = now;
        if changed {
            self.music_tray_refresh();
        }
        if self.visible && self.mode == Mode::Music && self.music.screen == Screen::NowPlaying {
            let selected = self.selected;
            self.refresh(cx);
            self.selected = selected.min(self.items.len().saturating_sub(1));
            self.scroll_to_selected();
        }
    }

    /// Brings the tray icon in line with the config: there is one while YouTube Music and its tray are on.
    pub fn music_tray_sync(&mut self) {
        let wanted = self.cfg.ytmusic.enabled && self.cfg.ytmusic.tray;
        if wanted && self.music.tray.is_none() {
            self.media();
            let playlists = self.tray_playlists();
            self.music.tray = music_tray::create(self.music.now.as_ref(), &playlists);
            self.music_tray_listen();
        } else if !wanted {
            self.music.tray = None;
        }
    }

    /// Installs the music tray's menu handler when it is the only tray that needs one.
    pub fn music_tray_listen(&self) {
        if self.music.tray.is_some() && self.tray_icon.is_none() {
            music_tray::listen(self.tx.clone());
        }
    }

    fn music_tray_refresh(&mut self) {
        if self.music.tray.is_none() {
            return;
        }
        let playlists = self.tray_playlists();
        if let Some(tray) = &self.music.tray {
            music_tray::update(tray, self.music.now.as_ref(), &playlists);
        }
    }

    /// The library's playlists: the ones the views loaded, or else the saved list.
    fn tray_playlists(&self) -> Vec<MusicItem> {
        if self.music.playlists.is_empty() {
            return ytmusic::load_cached(Shelf::Playlists.browse_id()).map(|(items, _)| items).unwrap_or_default();
        }
        self.music.playlists.clone()
    }

    pub fn music_tray_act(&mut self, id: &str) {
        let Some(action) = Action::parse(id) else { return };
        let sent = match action {
            Action::PlayPause => self.music_send_control(Control::PlayPause),
            Action::Like => self.music_send_control(Control::Like),
            Action::Next => self.music_send_control(Control::Next),
            Action::Previous => self.music_send_control(Control::Previous),
            Action::Radio => self.music_ask(|req| Some(ytmusic::play(req, None, None, true))),
            Action::Add(playlist) => {
                let name = playlist_name(&self.tray_playlists(), &playlist);
                self.music_add(&playlist, None, name)
            }
            Action::Copy => self.music_link(Link::Copy),
            Action::Open => self.music_link(Link::Open),
        };
        if !sent {
            let _ = self.tx.unbounded_send(Msg::Toast(false, NO_BROWSER.into()));
        }
    }
}

/// The name of the library playlist with this id, with or without the `VL` of its browse id.
fn playlist_name(playlists: &[MusicItem], id: &str) -> String {
    let bare = |id: &str| id.strip_prefix("VL").unwrap_or(id).to_string();
    playlists.iter().find(|playlist| bare(&playlist.id) == bare(id)).map(|playlist| playlist.title.clone()).unwrap_or_default()
}

/// The link to the track a `current` reply says is playing.
fn current_url(reply: &Reply) -> Option<String> {
    let current = reply.current.as_ref().filter(|current| reply.error.is_none() && !current.video_id.is_empty())?;
    Some(ytmusic::watch_url(&current.video_id, &current.playlist_id))
}

fn glyph_of(kind: Kind) -> &'static str {
    match kind {
        Kind::Song | Kind::Album => g::AUDIO,
        Kind::Playlist => g::BULLETS,
        Kind::Artist => g::STAR,
    }
}

fn music_row(item: &MusicItem, page: Option<&str>, playlists: &[MusicItem]) -> Item {
    let actions = item_actions(item, page, playlists);
    let mut row = Item::new(&item.title, &item.subtitle, Icon::Vector(glyph_of(item.kind)), actions[0].1.clone()).acc(&item.duration).with_hay(format!("{} {}", item.title, item.subtitle));
    row.extra = actions;
    row
}

/// The id of the song or page a row stands for.
fn music_id(act: &Act) -> Option<&str> {
    match act {
        Act::Music(MusicAct::Open(item)) => Some(&item.id),
        Act::Music(MusicAct::Play { video: Some(video), .. }) => Some(video),
        _ => None,
    }
}

fn row_of(rows: &[Item], id: &str) -> Option<usize> {
    rows.iter().position(|row| music_id(&row.act) == Some(id))
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

/// `added` is the name of the playlist an Add to Playlist request named.
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
    item.extra = [
        ("Play/Pause", play_pause),
        ("Like", MusicAct::Control(Control::Like)),
        ("Dislike", MusicAct::Control(Control::Dislike)),
        ("Start Radio", radio),
        ("Copy URL", MusicAct::Current(Link::Copy)),
        ("Open in YouTube Music", MusicAct::Current(Link::Open)),
    ]
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
        assert_eq!(labels, ["Play/Pause", "Like", "Dislike", "Start Radio", "Copy URL", "Open in YouTube Music"]);
        assert_eq!(row.extra[3].1, Act::Music(MusicAct::Play { video: None, playlist: None, radio: true }));
        assert_eq!(row.extra[4].1, Act::Music(MusicAct::Current(Link::Copy)));
        assert_eq!(row.extra[5].1, Act::Music(MusicAct::Current(Link::Open)));
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
        assert_eq!((label(MusicAct::Current(Link::Copy)), label(MusicAct::Current(Link::Open))), ("Copy URL", "Open in YouTube Music"));
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

    #[test]
    fn the_current_track_has_a_link() {
        let playing = |video: &str, playlist: &str| Current { video_id: video.into(), playlist_id: playlist.into(), ..Default::default() };
        assert_eq!(current_url(&reply("current", None, Some(playing("abc", "")))), Some("https://music.youtube.com/watch?v=abc".into()));
        assert_eq!(current_url(&reply("current", None, Some(playing("abc", "PL1")))), Some("https://music.youtube.com/watch?v=abc&list=PL1".into()));
        assert_eq!(current_url(&reply("current", None, Some(playing("", "PL1")))), None);
        assert_eq!(current_url(&reply("current", None, None)), None);
        assert_eq!(current_url(&reply("current", Some("Open YouTube Music first."), Some(playing("abc", "")))), None);
    }

    #[test]
    fn playlists_are_named_with_or_without_the_browse_prefix() {
        let library = [MusicItem { id: "VLPL1".into(), title: "Road Trip".into(), ..Default::default() }];
        assert_eq!((playlist_name(&library, "PL1").as_str(), playlist_name(&library, "VLPL1").as_str(), playlist_name(&library, "PL2").as_str()), ("Road Trip", "Road Trip", ""));
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

    #[test]
    fn the_newest_ask_waits_for_the_one_in_flight() {
        let search = |query: &str| Ask::Search { query: query.into(), filter: Filter::Songs };
        let mut relay = Relay { wanted: Some(search("a")), ..Default::default() };
        let first = relay.next().unwrap();
        relay.sent(1, &first);
        relay.wanted = Some(search("ab"));
        relay.wanted = Some(search("abc"));
        assert_eq!(relay.next(), None, "one at a time");
        assert!(relay.landed(7).is_none(), "an answer to another request changes nothing");
        assert_eq!(relay.next(), None);
        assert!(relay.landed(1).is_some());
        assert_eq!(relay.next(), Some(search("abc")), "the newest wins");
        assert_eq!(relay.next(), None);
    }

    #[test]
    fn a_flight_remembers_its_shelf() {
        let mut relay = Relay::default();
        relay.sent(2, &Ask::Shelf(Shelf::Playlists));
        assert_eq!(relay.landed(2).unwrap().shelf, Some(Shelf::Playlists));
        relay.sent(3, &Ask::Page("MPREb_1".into()));
        assert_eq!(relay.landed(3).unwrap().shelf, None);
        assert_eq!(Ask::Shelf(Shelf::Liked).request(4), ytmusic::browse(4, "VLLM"));
        assert_eq!(Ask::Page("MPREb_1".into()).request(5), ytmusic::browse(5, "MPREb_1"));
        assert_eq!(Ask::Search { query: "x".into(), filter: Filter::Albums }.request(6), ytmusic::search(6, "x", Filter::Albums));
    }

    #[test]
    fn rows_are_found_by_what_they_open_or_play() {
        let rows = [music_row(&item(Kind::Album, "MPREb_1", "OLAK1"), None, &[]), music_row(&item(Kind::Song, "abc", ""), None, &[])];
        assert_eq!((row_of(&rows, "MPREb_1"), row_of(&rows, "abc"), row_of(&rows, "zzz")), (Some(0), Some(1), None));
        assert_eq!(music_id(&Act::Music(MusicAct::Play { video: None, playlist: Some("PL1".into()), radio: false })), None);
        assert_eq!(music_id(&Act::None), None);
    }
}