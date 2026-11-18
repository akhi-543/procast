//! YouTube Music inside the launcher: its screens, and what the companion's answers and the
//! media session's events do to them.

use std::{
    collections::{HashMap, HashSet},
    mem,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use gpui::{ClipboardItem, Context, RenderImage, Window};
use tray_icon::TrayIcon;

use crate::{
    Launcher, Mode, Tone,
    commands::{self, Cmd},
    design::g,
    empty_row, ext,
    media::{self, Media, NowPlaying, Transport},
    music_tray::{self, Action},
    net,
    search::{self, Act, Icon, Item},
    win::Msg,
    ytmdesktop,
    ytmusic::{self, Control, Filter, Kind, MusicItem, Reply, Screen, Shelf},
    ytmusic_api,
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
const IMPORT_SESSION: &str = "Import YouTube Music Session";


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
    fn catalog(&self) -> Catalog {
        match self {
            Self::Search { query, filter } => Catalog::Search { query: query.clone(), filter: *filter },
            Self::Shelf(shelf) => Catalog::Browse { id: shelf.browse_id().into() },
            Self::Page(id) => Catalog::Browse { id: id.clone() },
        }
    }

    fn op(&self) -> &'static str {
        match self {
            Self::Search { .. } => "search",
            Self::Shelf(_) | Self::Page(_) => "browse",
        }
    }
}

/// InnerTube search or browse, not a companion `{"t":"ytm"}` envelope.
#[derive(Clone, Debug, PartialEq)]
enum Catalog {
    Search { query: String, filter: Filter },
    Browse { id: String },
}

fn catalog_blocked_by_browsers(_connected: bool) -> bool {
    false
}

fn catalog_reply(ask: &Ask, result: Result<Reply, String>) -> Reply {
    match result {
        Ok(reply) => reply,
        Err(_) => Reply { op: ask.op().into(), error: Some(ytmusic_api::UNREACHABLE.into()), ..Default::default() },
    }
}

fn playlists_from_desktop(list: Vec<(String, String)>) -> Vec<MusicItem> {
    list.into_iter()
        .map(|(id, title)| {
            let playlist_id = id.strip_prefix("VL").unwrap_or(&id).to_string();
            let id = if id.starts_with("VL") { id } else { format!("VL{playlist_id}") };
            MusicItem { kind: Kind::Playlist, id, playlist_id, title, ..Default::default() }
        })
        .collect()
}

fn playlists_need_desktop(reply: &Reply) -> bool {
    reply.error.as_deref() == Some(ytmusic_api::SIGN_IN) || reply.items.is_empty()
}

/// Saved library (liked albums, artists, songs) is empty from unsigned InnerTube.
fn library_needs_browser(reply: &Reply) -> bool {
    reply.items.is_empty()
}

/// The empty row's title is the real reason. A generic "Couldn't load" contradicts a correct detail.
fn load_failure(error: &str) -> (String, String) {
    if error == ytmusic_api::SIGN_IN {
        ("Sign in to see your library".into(), "Import YouTube Music Session, or open YouTube Music in your browser".into())
    } else if error == ytmusic_api::UNREACHABLE {
        ("Couldn't reach YouTube Music".into(), String::new())
    } else {
        (error.to_string(), String::new())
    }
}

/// Words for a desktop command that succeeded, taken from the state it just read.
fn desktop_notice(control: Control, state: &ytmdesktop::State) -> Option<String> {
    let volume = || {
        if state.muted {
            "Muted".into()
        } else {
            format!("Volume {}%", state.volume)
        }
    };
    match control {
        Control::Like if state.like_status == 2 => Some("Liked".into()),
        Control::Like if state.like_status == 1 || state.like_status == 0 => Some("Removed like".into()),
        Control::Dislike if state.like_status == 0 => Some("Disliked".into()),
        Control::Dislike if state.like_status == 1 || state.like_status == 2 => Some("Removed dislike".into()),
        Control::Mute | Control::VolumeLow | Control::VolumeMedium | Control::VolumeHigh | Control::VolumeUp | Control::VolumeDown => Some(volume()),
        _ => None,
    }
}

/// The library shelf to fetch again once a session is imported: the one showing, unless a page covers it.
fn shelf_after_import(screen: Screen, shelf: Option<Shelf>, page_open: bool) -> Option<Shelf> {
    shelf.filter(|_| screen == Screen::Library && !page_open)
}

fn apply_desktop_playlists(shelf: Shelf, mut reply: Reply, desktop: Option<Vec<(String, String)>>) -> Reply {
    if shelf != Shelf::Playlists || !playlists_need_desktop(&reply) {
        return reply;
    }
    let Some(list) = desktop else { return reply };
    reply.items = playlists_from_desktop(list);
    reply.error = None;
    reply
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
    /// Last video/playlist ids from desktop `/state` (Copy URL / Open / song radio).
    video_id: String,
    playlist_id: String,
    /// Last desktop player volume from `/state`, for Volume Up/Down `setVolume`.
    volume: Option<u32>,
    /// Companion browse already sent for this library reply, so an empty answer is not asked again.
    browser_library: Option<u64>,
    tray: Option<TrayIcon>,
}

impl MusicState {
    pub fn cover_is(&self, path: &Path) -> bool {
        self.now.as_ref().and_then(|now| now.cover.as_deref()) == Some(path)
    }

    /// A cover file changed on disk.
    pub fn drop_cover(&mut self, path: &Path) {
        self.retired.extend(self.covers.remove(path).flatten());
    }

    pub fn retire_covers(&mut self) {
        self.retired.extend(self.covers.drain().filter_map(|(_, image)| image));
    }
}

enum DesktopJob {
    Play { video: Option<String>, playlist: Option<String> },
    Command(Control),
    State,
}

impl Launcher {
    fn media(&mut self) -> &Media {
        let tx = &self.tx;
        self.media.get_or_insert_with(|| Media::start(tx.clone()))
    }

    fn music_desktop(&mut self, job: DesktopJob) {
        let step = self.cfg.ytmusic.volume_step;
        let volume = self.music.volume;
        let fallback = match job {
            DesktopJob::Command(control) => transport(control).map(|t| (self.media().clone(), t)),
            _ => {
                self.media();
                None
            }
        };
        let base = self.cfg.ytmusic.desktop_url.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let desktop = ytmdesktop::Desktop::new(&base);
            let commanded = (|| -> Result<_, String> {
                let token = ytmdesktop::ensure_token(&desktop)?;
                match &job {
                    DesktopJob::Play { video, playlist } => ytmdesktop::play(&token, &desktop.base, video.as_deref(), playlist.as_deref())?,
                    DesktopJob::Command(control) => ytmdesktop::command(&token, &desktop.base, ytmdesktop::transport(*control, step, volume))?,
                    DesktopJob::State => {}
                }
                Ok(token)
            })();
            let token = match commanded {
                Ok(token) => token,
                Err(e) => {
                    if e == ytmdesktop::OPEN_APP {
                        media::set_desktop_active(false);
                        if let Some((media, t)) = fallback {
                            media.send(t);
                            return;
                        }
                        if matches!(job, DesktopJob::State) {
                            return;
                        }
                    }
                    let _ = tx.unbounded_send(Msg::error(e));
                    return;
                }
            };
            // The command already happened. A failed /state refresh must not toast a
            // different failure over a play, like, or volume change that succeeded.
            let Ok(state) = ytmdesktop::state(&token, &desktop.base) else { return };
            media::set_desktop_active(true);
            if let DesktopJob::Command(control) = job
                && let Some(text) = desktop_notice(control, &state)
            {
                let _ = tx.unbounded_send(Msg::done(text));
            }
            let (mut now, video_id, playlist_id) = ytmdesktop::to_now_playing(&state);
            now.video_id = video_id;
            now.playlist_id = playlist_id;
            now.cover = if state.thumb_url.is_empty() { None } else { net::cached_image(&state.thumb_url, &tx).or_else(|| Some(net::image_path(&state.thumb_url))) };
            let _ = tx.unbounded_send(Msg::NowPlaying(Some(now).filter(|n| !n.title.is_empty() || !n.video_id.is_empty())));
        });
    }

    fn music_fail(&mut self, message: &str, cx: &mut Context<Self>) {
        if self.visible && self.mode == Mode::Music {
            self.status = Some((Tone::Error, message.into()));
            return cx.notify();
        }
        let _ = self.tx.unbounded_send(Msg::error(message));
    }

    pub fn music_open(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.media();
        self.music_desktop(DesktopJob::State);
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
        let controls = control_rows(self.cfg.ytmusic.volume_step);
        rows.extend(search::filter(query, controls, &mut self.matcher));
        rows
    }

    fn track_row(&mut self) -> Item {
        let Some(now) = self.music.now.clone() else {
            return empty_row(g::PLAY, "Nothing playing".into(), ytmdesktop::OPEN_APP.into(), vec![]);
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
            return vec![empty_row(g::SEARCH, "Search YouTube Music".into(), "Type a song, album, artist or playlist".into(), vec![])];
        }
        let waiting = view.loading || searching && query.trim() != view.searched;
        let (glyph, title, hint) = if let Some(error) = &view.error {
            let (title, hint) = load_failure(error);
            (g::WARNING, title, hint)
        } else if waiting {
            (g::CLOCK, (if searching { "Searching…" } else { "Loading…" }).to_string(), String::new())
        } else if searching {
            (g::SEARCH, "No results".into(), "Try another search".into())
        } else if typed {
            (g::SEARCH, "No matches".into(), String::new())
        } else {
            (g::AUDIO, "Nothing here yet".into(), String::new())
        };
        vec![empty_row(glyph, title, hint, vec![])]
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
        match act {
            MusicAct::Control(control) => self.music_control(control, cx),
            MusicAct::Open(item) => self.music_page(item, cx),
            MusicAct::Play { video, playlist, radio } => self.music_play(video, playlist, radio, true, cx),
            MusicAct::Rate(op, video) => self.music_rate(op, video, cx),
            MusicAct::AddToPlaylist(playlist, video) => {
                let name = playlist_name(&self.music.playlists, &playlist);
                self.music_add(&playlist, video.as_deref(), name, true, cx);
            }
            MusicAct::Current(link) => self.music_link(link, true, cx),
        }
    }

    pub fn music_import_session(&mut self, cx: &mut Context<Self>) {
        self.music.req += 1;
        let req = self.music.req;
        if !self.browsers.ask(None, ytmusic::export_session(req)) {
            let _ = self.tx.unbounded_send(Msg::error(NO_BROWSER));
        }
        self.hide(true, cx);
    }

    fn music_play(&mut self, video: Option<String>, playlist: Option<String>, radio: bool, hide: bool, cx: &mut Context<Self>) {
        if radio {
            let video = video.filter(|id| !id.is_empty()).or_else(|| nonempty(&self.music.video_id));
            let Some(video) = video else {
                return self.music_fail("Nothing playing on YouTube Music", cx);
            };
            self.music_desktop(DesktopJob::Play { video: Some(video.clone()), playlist: Some(ytmdesktop::radio_playlist(&video)) });
        } else {
            self.music_desktop(DesktopJob::Play { video, playlist });
        }
        if hide {
            self.music_done(true, cx);
        }
    }

    fn music_rate(&mut self, op: &'static str, video: Option<String>, cx: &mut Context<Self>) {
        let video = video.filter(|id| !id.is_empty());
        let current = nonempty(&self.music.video_id);
        let on_now = video.as_ref().is_none_or(|id| current.as_ref() == Some(id));
        if on_now {
            match op {
                "like" => return self.music_control(Control::Like, cx),
                "dislike" => return self.music_control(Control::Dislike, cx),
                "unlike" => {
                    let _ = self.tx.unbounded_send(Msg::error(IMPORT_SESSION));
                    return self.music_done(true, cx);
                }
                _ => {}
            }
        }
        let Some(video) = video else {
            let _ = self.tx.unbounded_send(Msg::error(IMPORT_SESSION));
            return self.music_done(true, cx);
        };
        if !ytmusic_api::signed_in() {
            let _ = self.tx.unbounded_send(Msg::error(IMPORT_SESSION));
            return self.music_done(true, cx);
        }
        self.music_innertube(op.to_string(), None, move || ytmusic_api::rate(op, &video));
        self.music_done(true, cx);
    }

    fn music_add(&mut self, playlist: &str, video: Option<&str>, name: String, hide: bool, cx: &mut Context<Self>) {
        let video = video.filter(|id| !id.is_empty()).map(str::to_string).or_else(|| nonempty(&self.music.video_id));
        let Some(video) = video else {
            return self.music_fail("Nothing playing on YouTube Music", cx);
        };
        if !ytmusic_api::signed_in() {
            let _ = self.tx.unbounded_send(Msg::error(IMPORT_SESSION));
            if hide {
                self.music_done(true, cx);
            }
            return;
        }
        let playlist = playlist.to_string();
        self.music_innertube("addToPlaylist".into(), Some(name), move || ytmusic_api::add_to_playlist(&playlist, &video));
        if hide {
            self.music_done(true, cx);
        }
    }

    fn music_innertube(&mut self, op: String, added: Option<String>, job: impl FnOnce() -> Result<Reply, String> + Send + 'static) {
        self.music.req += 1;
        let req = self.music.req;
        if let Some(name) = added {
            self.music.added.insert(req, name);
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut reply = job().unwrap_or_else(|error| Reply { op, error: Some(error), ..Default::default() });
            reply.req = req;
            let _ = tx.unbounded_send(Msg::Music(reply));
        });
    }

    fn music_control(&mut self, control: Control, cx: &mut Context<Self>) {
        let sent = self.music_send_control(control);
        self.music_done(sent, cx);
    }

    fn music_send_control(&mut self, control: Control) -> bool {
        self.music_desktop(DesktopJob::Command(control));
        true
    }

    /// Copy URL uses desktop video/playlist ids. Open sends `changeVideo` for those ids.
    fn music_link(&mut self, link: Link, hide: bool, cx: &mut Context<Self>) {
        let video = nonempty(&self.music.video_id);
        let playlist = nonempty(&self.music.playlist_id);
        match link {
            Link::Copy => {
                let Some(video) = video else {
                    return self.music_fail("Nothing playing on YouTube Music", cx);
                };
                let url = ytmusic::watch_url(&video, playlist.as_deref().unwrap_or(""));
                cx.write_to_clipboard(ClipboardItem::new_string(url));
                let _ = self.tx.unbounded_send(Msg::done("Copied"));
            }
            Link::Open => {
                if video.is_none() && playlist.is_none() {
                    return self.music_fail("Nothing playing on YouTube Music", cx);
                }
                self.music_desktop(DesktopJob::Play { video, playlist });
            }
        }
        if hide {
            self.music_done(true, cx);
        }
    }

    fn music_linked(&mut self, link: Link, reply: &Reply, cx: &mut Context<Self>) {
        match link {
            Link::Open => {
                let Some(current) = reply.current.as_ref().filter(|current| !current.video_id.is_empty()) else { return };
                self.music_desktop(DesktopJob::Play { video: Some(current.video_id.clone()), playlist: nonempty(&current.playlist_id) });
            }
            Link::Copy => {
                let url = current_url(reply);
                let toast = match url {
                    Some(url) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(url));
                        (true, "Copied".to_string())
                    }
                    None => (false, reply.error.clone().unwrap_or_else(|| "Nothing playing on YouTube Music".into())),
                };
                let _ = self.tx.unbounded_send(toast_msg(toast.0, toast.1));
            }
        }
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
        if catalog_blocked_by_browsers(self.browsers.any()) {
            self.music.relay.flight = None;
            return;
        }
        let Some(ask) = self.music.relay.next() else { return };
        self.music.req += 1;
        let req = self.music.req;
        self.music.relay.sent(req, &ask);
        self.music.view.pending = Some(req);
        let tx = self.tx.clone();
        let base = self.cfg.ytmusic.desktop_url.clone();
        std::thread::spawn(move || {
            let result = match ask.catalog() {
                Catalog::Search { query, filter } => ytmusic_api::search(&query, filter),
                Catalog::Browse { id } => ytmusic_api::browse(&id),
            };
            let mut reply = catalog_reply(&ask, result);
            if let Ask::Shelf(Shelf::Playlists) = &ask
                && playlists_need_desktop(&reply) {
                    let desktop = ytmdesktop::Desktop::new(&base);
                    let list = ytmdesktop::ensure_token(&desktop).and_then(|token| ytmdesktop::playlists(&token, &desktop.base)).ok();
                    reply = apply_desktop_playlists(Shelf::Playlists, reply, list);
                }
            reply.req = req;
            let _ = tx.unbounded_send(Msg::Music(reply));
        });
    }

    fn music_done(&mut self, sent: bool, cx: &mut Context<Self>) {
        if !sent {
            let message = NO_BROWSER.to_string();
            if self.visible && self.mode == Mode::Music {
                self.status = Some((Tone::Error, message));
                return cx.notify();
            }
            let _ = self.tx.unbounded_send(Msg::error(message));
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
            self.scroll_to_selected();
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
        if reply.op == "exportSession" {
            let toast = if let Some(error) = reply.error {
                (false, error)
            } else {
                match ytmusic_api::save_session(reply.cookies.iter().map(|c| (c.name.as_str(), c.value.as_str()))) {
                    Ok(()) => (true, "Imported YouTube Music session".into()),
                    Err(error) => (false, error),
                }
            };
            let view = &self.music.view;
            let reload = if toast.0 { shelf_after_import(self.music.screen, Some(view.shelf), view.page.is_some()) } else { None };
            let _ = self.tx.unbounded_send(toast_msg(toast.0, toast.1));
            if let Some(shelf) = reload {
                self.music_fetch(Ask::Shelf(shelf));
            }
            return;
        }
        if matches!(reply.op.as_str(), "search" | "browse") {
            let flight = self.music.relay.landed(reply.req);
            let shelf = flight.as_ref().and_then(|flight| flight.shelf);
            if let Some(shelf) = shelf.filter(|_| library_needs_browser(&reply) && self.music.browser_library != Some(reply.req) && self.browsers.any()) {
                self.music.req += 1;
                let req = self.music.req;
                if self.browsers.ask(None, ytmusic::browse(req, shelf.browse_id())) {
                    self.music.browser_library = Some(req);
                    self.music.relay.sent(req, &Ask::Shelf(shelf));
                    self.music.view.pending = Some(req);
                    self.music.view.loading = true;
                    if self.visible && self.mode == Mode::Music {
                        self.refresh(cx);
                    }
                    return;
                }
            }
            self.music.browser_library = None;
            if let Some(shelf) = shelf.filter(|_| reply.error.is_none()) {
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
            let _ = self.tx.unbounded_send(toast_msg(ok, text));
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
                self.scroll_to_selected();
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
        remember_track(&mut self.music.video_id, &mut self.music.playlist_id, now.as_ref());
        if let Some(volume) = now.as_ref().and_then(|n| n.volume) {
            self.music.volume = Some(volume);
        }
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
            self.music_desktop(DesktopJob::State);
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

    pub fn music_tray_refresh(&mut self) {
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

    pub fn music_tray_act(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(action) = Action::parse(id) else { return };
        match action {
            Action::PlayPause => {
                self.music_send_control(Control::PlayPause);
            }
            Action::Like => {
                self.music_send_control(Control::Like);
            }
            Action::Next => {
                self.music_send_control(Control::Next);
            }
            Action::Previous => {
                self.music_send_control(Control::Previous);
            }
            Action::Radio => self.music_play(None, None, true, false, cx),
            Action::Add(playlist) => {
                let name = playlist_name(&self.tray_playlists(), &playlist);
                self.music_add(&playlist, None, name, false, cx);
            }
            Action::Copy => self.music_link(Link::Copy, false, cx),
            Action::Open => self.music_link(Link::Open, false, cx),
        }
    }
}

/// The name of the library playlist with this id, with or without the `VL` of its browse id.
fn nonempty(id: &str) -> Option<String> {
    (!id.is_empty()).then(|| id.to_string())
}

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
        Kind::Artist => g::USERS,
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
    if let Some(play) = desktop_play(item) {
        actions.push(("Open in YouTube Music".into(), Act::Music(play)));
    }
    actions
}

/// Play in the desktop app: a song's video id, or an album/playlist's `playlist_of`. Artists have none.
fn desktop_play(item: &MusicItem) -> Option<MusicAct> {
    match item.kind {
        Kind::Song => Some(MusicAct::Play { video: Some(item.id.clone()), playlist: None, radio: false }),
        Kind::Album | Kind::Playlist => playlist_of(item).map(|playlist| MusicAct::Play { video: None, playlist: Some(playlist), radio: false }),
        Kind::Artist => None,
    }
}

fn remember_track(video_id: &mut String, playlist_id: &mut String, now: Option<&NowPlaying>) {
    match now.filter(|n| !n.video_id.is_empty()) {
        Some(n) => {
            video_id.clone_from(&n.video_id);
            playlist_id.clone_from(&n.playlist_id);
        }
        None => {
            video_id.clear();
            playlist_id.clear();
        }
    }
}

fn transport(control: Control) -> Option<Transport> {
    match control {
        Control::PlayPause => Some(Transport::PlayPause),
        Control::Next => Some(Transport::Next),
        Control::Previous => Some(Transport::Previous),
        _ => None,
    }
}

#[cfg(test)]
fn announces_volume(control: Control) -> bool {
    matches!(control, Control::Mute | Control::VolumeLow | Control::VolumeMedium | Control::VolumeHigh | Control::VolumeUp | Control::VolumeDown)
}

/// `added` is the name of the playlist an Add to Playlist request named.
/// A done (true) or error toast.
fn toast_msg(ok: bool, text: String) -> Msg {
    if ok { Msg::done(text) } else { Msg::error(text) }
}

fn toast(reply: &Reply, volume: bool, added: Option<&str>) -> Option<(bool, String)> {
    if let Some(error) = &reply.error {
        return Some((false, error.clone()));
    }
    match reply.op.as_str() {
        "like" => Some((true, "Liked".into())),
        "dislike" => Some((true, "Disliked".into())),
        "unlike" => Some((true, "Removed like".into())),
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
        NowPlaying {
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            playing,
            cover: None,
            video_id: String::new(),
            playlist_id: String::new(),
            volume: None,
        }
    }

    fn reply(op: &str, error: Option<&str>, current: Option<Current>) -> Reply {
        Reply { op: op.into(), error: error.map(String::from), current, ..Default::default() }
    }

    #[test]
    fn empty_now_playing_clears_stored_track_ids() {
        let mut video = "abc".to_string();
        let mut playlist = "PL1".to_string();
        remember_track(&mut video, &mut playlist, None);
        assert!(video.is_empty() && playlist.is_empty());
        let mut with_id = now("One", "Band", "Record", true);
        with_id.video_id = "xyz".into();
        with_id.playlist_id = "PL2".into();
        remember_track(&mut video, &mut playlist, Some(&with_id));
        assert_eq!((video.as_str(), playlist.as_str()), ("xyz", "PL2"));
        let empty_id = now("One", "Band", "Record", true);
        remember_track(&mut video, &mut playlist, Some(&empty_id));
        assert!(video.is_empty() && playlist.is_empty());
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
        assert!(transport(Control::Like).is_none() && transport(Control::VolumeUp).is_none() && transport(Control::Mute).is_none());
        assert_eq!(ytmdesktop::radio_playlist("abc"), "RDAMVMabc");
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
        assert_eq!(actions[7].1, Act::Music(MusicAct::Play { video: Some("abc".into()), playlist: None, radio: false }));
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
        assert_eq!(actions[3].1, Act::Music(MusicAct::Play { video: None, playlist: Some("OLAK1".into()), radio: false }));
        let playlist = item(Kind::Playlist, "VLPL9", "");
        assert_eq!(item_actions(&playlist, None, &[])[1].1, Act::Music(MusicAct::Play { video: None, playlist: Some("PL9".into()), radio: false }), "the browse id names the playlist");
        assert_eq!(item_actions(&playlist, None, &[])[3].1, Act::Music(MusicAct::Play { video: None, playlist: Some("PL9".into()), radio: false }));
        assert_eq!(labels(&item_actions(&item(Kind::Album, "MPREb_2", ""), None, &[])), ["Open", "Copy URL"]);
    }

    #[test]
    fn an_artist_only_opens() {
        let actions = item_actions(&item(Kind::Artist, "UC1", "RDAO1"), None, &[]);
        assert_eq!(labels(&actions), ["Open", "Copy URL"]);
        assert_eq!(actions[1].1, Act::Copy("https://music.youtube.com/browse/UC1".into()));
        assert_eq!(playlist_of(&item(Kind::Artist, "UC1", "RDAO1")), None);
        assert_eq!(playlist_of(&item(Kind::Song, "abc", "PL1")), None);
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
        assert_eq!(Ask::Shelf(Shelf::Liked).catalog(), Catalog::Browse { id: "VLLM".into() });
        assert_eq!(Ask::Page("MPREb_1".into()).catalog(), Catalog::Browse { id: "MPREb_1".into() });
        assert_eq!(Ask::Search { query: "x".into(), filter: Filter::Albums }.catalog(), Catalog::Search { query: "x".into(), filter: Filter::Albums });
    }

    #[test]
    fn catalog_ask_is_innertube_not_companion() {
        let companion = ytmusic::search(1, "daft punk", Filter::Albums);
        assert_eq!((companion["t"].as_str(), companion["op"].as_str()), (Some("ytm"), Some("search")));
        assert_eq!(
            Ask::Search { query: "daft punk".into(), filter: Filter::Albums }.catalog(),
            Catalog::Search { query: "daft punk".into(), filter: Filter::Albums }
        );
        assert_eq!(Ask::Shelf(Shelf::Albums).catalog(), Catalog::Browse { id: "FEmusic_liked_albums".into() });
        assert_eq!(Ask::Shelf(Shelf::Playlists).catalog(), Catalog::Browse { id: "FEmusic_liked_playlists".into() });
        assert_eq!(Ask::Shelf(Shelf::Artists).catalog(), Catalog::Browse { id: "FEmusic_library_corpus_track_artists".into() });
        assert!(!catalog_blocked_by_browsers(false));
        assert!(!catalog_blocked_by_browsers(true));
    }

    #[test]
    fn catalog_failures_are_not_browser_extension_errors() {
        let ask = Ask::Search { query: "x".into(), filter: Filter::Songs };
        let reply = catalog_reply(&ask, Err("timeout".into()));
        assert_eq!(reply.error.as_deref(), Some("Couldn't reach YouTube Music"));
        assert_eq!(reply.op, "search");
        let signed_out = Reply { op: "browse".into(), error: Some("Sign in to music.youtube.com in this browser to see your library".into()), ..Default::default() };
        assert_eq!(catalog_reply(&Ask::Shelf(Shelf::Albums), Ok(signed_out.clone())).error, signed_out.error);
    }

    #[test]
    fn import_success_reloads_the_open_library_shelf() {
        assert_eq!(shelf_after_import(Screen::Library, Some(Shelf::Liked), false), Some(Shelf::Liked));
        assert_eq!(shelf_after_import(Screen::Library, Some(Shelf::Albums), true), None);
        assert_eq!(shelf_after_import(Screen::Search, Some(Shelf::Liked), false), None);
    }

    #[test]
    fn empty_library_asks_the_browser_and_says_why() {
        let signed_out = Reply { op: "browse".into(), error: Some(ytmusic_api::SIGN_IN.into()), ..Default::default() };
        assert!(library_needs_browser(&signed_out));
        assert!(!library_needs_browser(&Reply { op: "browse".into(), items: vec![MusicItem { title: "Discovery".into(), ..Default::default() }], ..Default::default() }));
        let ask = ytmusic::browse(4, Shelf::Albums.browse_id());
        assert_eq!((ask["op"].as_str(), ask["id"].as_str()), (Some("browse"), Some("FEmusic_liked_albums")));
        assert_eq!(
            load_failure(ytmusic_api::SIGN_IN),
            ("Sign in to see your library".into(), "Import YouTube Music Session, or open YouTube Music in your browser".into())
        );
        assert_eq!(load_failure(ytmusic_api::UNREACHABLE).0, "Couldn't reach YouTube Music");
        assert_ne!(load_failure(ytmusic_api::SIGN_IN).0, "Couldn't load YouTube Music");
    }

    #[test]
    fn desktop_notice_matches_the_player() {
        let state = |like_status, volume, muted| ytmdesktop::State {
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            video_id: String::new(),
            playlist_id: String::new(),
            playing: false,
            volume,
            muted,
            like_status,
            thumb_url: String::new(),
        };
        assert_eq!(desktop_notice(Control::Like, &state(2, 40, false)).as_deref(), Some("Liked"));
        assert_eq!(desktop_notice(Control::Like, &state(1, 40, false)).as_deref(), Some("Removed like"));
        assert_eq!(desktop_notice(Control::Dislike, &state(0, 40, false)).as_deref(), Some("Disliked"));
        assert_eq!(desktop_notice(Control::VolumeUp, &state(-1, 55, false)).as_deref(), Some("Volume 55%"));
        assert_eq!(desktop_notice(Control::Mute, &state(-1, 55, true)).as_deref(), Some("Muted"));
        assert_eq!(desktop_notice(Control::Like, &state(-1, 40, false)), None);
        assert_eq!(desktop_notice(Control::Next, &state(2, 40, false)), None);
    }

    #[test]
    fn playlists_shelf_can_fill_from_the_desktop_app() {
        let sign_in = Some("Sign in to music.youtube.com in this browser to see your library");
        let empty = Reply { op: "browse".into(), ..Default::default() };
        let unsigned = Reply { op: "browse".into(), error: sign_in.map(str::to_string), ..Default::default() };
        let rows = vec![("PLabc".into(), "Road Trip".into()), ("VLPL2".into(), "Focus".into())];
        let items = playlists_from_desktop(rows.clone());
        assert_eq!(items[0], MusicItem { kind: Kind::Playlist, id: "VLPLabc".into(), playlist_id: "PLabc".into(), title: "Road Trip".into(), ..Default::default() });
        assert_eq!(items[1].id.as_str(), "VLPL2");
        assert_eq!(items[1].playlist_id.as_str(), "PL2");
        let filled = apply_desktop_playlists(Shelf::Playlists, unsigned.clone(), Some(rows.clone()));
        assert!(filled.error.is_none());
        assert_eq!(filled.items, items);
        let from_empty = apply_desktop_playlists(Shelf::Playlists, empty, Some(rows.clone()));
        assert_eq!(from_empty.items, items);
        let loaded = Reply { op: "browse".into(), items: vec![MusicItem { title: "Mine".into(), ..Default::default() }], ..Default::default() };
        assert!(!playlists_need_desktop(&loaded));
        assert!(playlists_need_desktop(&unsigned));
        assert_eq!(apply_desktop_playlists(Shelf::Playlists, loaded.clone(), Some(rows.clone())).items[0].title, "Mine");
        let albums = apply_desktop_playlists(Shelf::Albums, unsigned.clone(), Some(rows));
        assert_eq!(albums.error.as_deref(), sign_in);
        assert!(albums.items.is_empty());
        let down = apply_desktop_playlists(Shelf::Playlists, unsigned, None);
        assert_eq!(down.error.as_deref(), sign_in);
    }

    #[test]
    fn rows_are_found_by_what_they_open_or_play() {
        let rows = [music_row(&item(Kind::Album, "MPREb_1", "OLAK1"), None, &[]), music_row(&item(Kind::Song, "abc", ""), None, &[])];
        assert_eq!((row_of(&rows, "MPREb_1"), row_of(&rows, "abc"), row_of(&rows, "zzz")), (Some(0), Some(1), None));
        assert_eq!(music_id(&Act::Music(MusicAct::Play { video: None, playlist: Some("PL1".into()), radio: false })), None);
        assert_eq!(music_id(&Act::None), None);
    }
}