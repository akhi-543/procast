//! YouTube Music in the browser: what the companion extension answers with, the requests
//! procast sends it, and the library lists kept on disk between runs.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Song,
    Album,
    Artist,
    Playlist,
}

/// A search or library result. `id` is a video id for songs and a browse id for the rest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct MusicItem {
    pub kind: Kind,
    pub id: String,
    pub playlist_id: String,
    pub title: String,
    pub subtitle: String,
    pub thumb: String,
    pub duration: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Current {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub playlist_id: String,
    #[serde(deserialize_with = "percent")]
    pub volume: u32,
    pub muted: bool,
}

/// The page's player may report a fractional volume.
fn percent<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    Ok(f64::deserialize(d)?.round().clamp(0., 100.) as u32)
}

/// The companion's answer to one request.
#[derive(Clone, Debug, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct Reply {
    pub req: u64,
    pub op: String,
    pub title: String,
    pub items: Vec<MusicItem>,
    pub current: Option<Current>,
    pub error: Option<String>,
}

/// A view of YouTube Music inside the launcher; also what the root commands open.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Screen {
    Search,
    Library,
    #[default]
    NowPlaying,
    QuickActions,
}

impl Screen {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Library => "library",
            Self::NowPlaying => "now",
            Self::QuickActions => "actions",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Filter {
    #[default]
    Songs,
    Albums,
    Artists,
    Playlists,
}

impl Filter {
    pub fn next(self) -> Self {
        match self {
            Self::Songs => Self::Albums,
            Self::Albums => Self::Artists,
            Self::Artists => Self::Playlists,
            Self::Playlists => Self::Songs,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Songs => "songs",
            Self::Albums => "albums",
            Self::Artists => "artists",
            Self::Playlists => "playlists",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Songs => "Songs",
            Self::Albums => "Albums",
            Self::Artists => "Artists",
            Self::Playlists => "Playlists",
        }
    }

    pub fn placeholder(self) -> String {
        format!("Search {}…", self.label().to_lowercase())
    }

    /// InnerTube `params` for a filtered search. Same strings as `FILTERS` in `browser/ytmusic.js`.
    pub fn params(self) -> &'static str {
        match self {
            Self::Songs => "EgWKAQIIAWoMEA4QChADEAQQCRAF",
            Self::Albums => "EgWKAQIYAWoMEA4QChADEAQQCRAF",
            Self::Artists => "EgWKAQIgAWoMEA4QChADEAQQCRAF",
            Self::Playlists => "EgWKAQIoAWoMEA4QChADEAQQCRAF",
        }
    }
}

const RENDERERS: [&str; 2] = ["musicTwoRowItemRenderer", "musicResponsiveListItemRenderer"];

/// Depth-first, document order. `visit` returning false skips that value's subtree.
fn walk<'a>(node: &'a Value, visit: &mut dyn FnMut(&'a str, &'a Value) -> bool) {
    match node {
        Value::Array(xs) => {
            for n in xs {
                walk(n, visit);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                if visit(k, v) {
                    walk(v, visit);
                }
            }
        }
        _ => {}
    }
}

fn collect<'a>(node: &'a Value, keys: &[&str]) -> Vec<&'a Value> {
    let mut out = Vec::new();
    walk(node, &mut |k, v| {
        if keys.iter().any(|want| *want == k) {
            out.push(v);
        }
        true
    });
    out
}

fn first<'a>(node: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    collect(node, keys).into_iter().next()
}

fn runs_of(text: &Value) -> &[Value] {
    text.get("runs").and_then(Value::as_array).map(|a| a.as_slice()).unwrap_or(&[])
}

fn joined(runs: &[Value]) -> String {
    runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect()
}

fn last_thumb(node: &Value) -> String {
    first(node, &["thumbnails"])
        .and_then(Value::as_array)
        .and_then(|t| t.last())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn playlist_of(node: &Value) -> String {
    collect(node, &["watchPlaylistEndpoint", "watchEndpoint"])
        .into_iter()
        .find_map(|e| e.get("playlistId").and_then(Value::as_str).filter(|s| !s.is_empty()))
        .unwrap_or("")
        .to_string()
}

fn fields(r: &Value) -> (Option<&Value>, Vec<&[Value]>) {
    if r.get("flexColumns").is_none() {
        return (runs_of(&r["title"]).first(), vec![runs_of(&r["subtitle"])]);
    }
    let Some(flex) = r["flexColumns"].as_array() else {
        return (None, Vec::new());
    };
    let columns: Vec<&[Value]> = flex.iter().map(|c| runs_of(&c["musicResponsiveListItemFlexColumnRenderer"]["text"])).collect();
    let title_run = columns.first().and_then(|c| c.first());
    let rest = columns.iter().skip(1).copied().collect();
    (title_run, rest)
}

fn is_duration(text: &str) -> bool {
    let mut parts = text.split(':');
    let Some(first) = parts.next() else { return false };
    if first.is_empty() || !first.bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some(second) = parts.next() else { return false };
    if second.len() != 2 || !second.bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(third) => third.len() == 2 && third.bytes().all(|c| c.is_ascii_digit()) && parts.next().is_none(),
    }
}

fn duration_of(r: &Value, columns: &[&[Value]]) -> String {
    let fixed = joined(runs_of(&r["fixedColumns"][0]["musicResponsiveListItemFixedColumnRenderer"]["text"]));
    if !fixed.is_empty() {
        return fixed;
    }
    columns
        .iter()
        .copied()
        .flatten()
        .find_map(|run| {
            let t = run.get("text").and_then(Value::as_str)?;
            is_duration(t).then(|| t.to_string())
        })
        .unwrap_or_default()
}

fn kind_of(browse: &Value) -> Option<Kind> {
    let page_type = browse["browseEndpointContextSupportedConfigs"]["browseEndpointContextMusicConfig"]["pageType"].as_str();
    match page_type {
        Some("MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK") => return Some(Kind::Album),
        Some("MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL") => return Some(Kind::Artist),
        Some("MUSIC_PAGE_TYPE_PLAYLIST") => return Some(Kind::Playlist),
        _ => {}
    }
    let id = browse.get("browseId").and_then(Value::as_str)?;
    if id.starts_with("MPRE") {
        Some(Kind::Album)
    } else if id.starts_with("UC") {
        Some(Kind::Artist)
    } else if id.starts_with("VL") {
        Some(Kind::Playlist)
    } else {
        None
    }
}

fn identity(r: &Value, title_run: Option<&Value>) -> Option<(Kind, String, String)> {
    let watch = title_run.and_then(|t| t.pointer("/navigationEndpoint/watchEndpoint")).or_else(|| r.pointer("/navigationEndpoint/watchEndpoint"));
    let video_id = r.pointer("/playlistItemData/videoId").and_then(Value::as_str).or_else(|| watch.and_then(|w| w.get("videoId")).and_then(Value::as_str));
    if let Some(video_id) = video_id.filter(|s| !s.is_empty()) {
        let playlist_id = watch.and_then(|w| w.get("playlistId")).and_then(Value::as_str).unwrap_or("").to_string();
        return Some((Kind::Song, video_id.to_string(), playlist_id));
    }
    let browse = r.pointer("/navigationEndpoint/browseEndpoint").or_else(|| title_run.and_then(|t| t.pointer("/navigationEndpoint/browseEndpoint")))?;
    let id = browse.get("browseId").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    let kind = kind_of(browse)?;
    let from = playlist_of(r);
    let playlist_id = if !from.is_empty() {
        from
    } else if kind == Kind::Playlist {
        id.strip_prefix("VL").unwrap_or(id).to_string()
    } else {
        String::new()
    };
    Some((kind, id.to_string(), playlist_id))
}

fn item(r: &Value) -> Option<MusicItem> {
    let (title_run, columns) = fields(r);
    let title = title_run?.get("text").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        return None;
    }
    let (kind, id, playlist_id) = identity(r, title_run)?;
    let subtitle = columns.iter().map(|runs| joined(runs)).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ").replace(" • ", " · ");
    let thumb = {
        let t = last_thumb(&r["thumbnail"]);
        if t.is_empty() { last_thumb(&r["thumbnailRenderer"]) } else { t }
    };
    Some(MusicItem { kind, id, playlist_id, title: title.to_string(), subtitle: subtitle.trim().to_string(), thumb, duration: duration_of(r, &columns) })
}

fn kind_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Song => "song",
        Kind::Album => "album",
        Kind::Artist => "artist",
        Kind::Playlist => "playlist",
    }
}

/// InnerTube search/browse JSON → catalog rows. Port of `items()` in `browser/ytmusic.js`.
pub fn parse_items(json: &Value) -> Vec<MusicItem> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in collect(json, &RENDERERS) {
        let Some(it) = item(r) else { continue };
        let key = format!("{}{}", kind_key(it.kind), it.id);
        if !seen.insert(key) {
            continue;
        }
        out.push(it);
    }
    out
}

fn header(json: &Value) -> Option<&Value> {
    let mut found = None;
    walk(json, &mut |k, v| {
        if found.is_some() || k.ends_with("ShelfRenderer") {
            return false;
        }
        if k.ends_with("HeaderRenderer") && !runs_of(&v["title"]).is_empty() {
            found = Some(v);
        }
        true
    });
    found
}

/// Page title and rows. Port of `page()` in `browser/ytmusic.js`, without header playlistId/thumb
/// except copying the header thumb onto songs that have none.
pub fn parse_page(json: &Value) -> (String, Vec<MusicItem>) {
    let head = header(json);
    let thumb = head.map(last_thumb).unwrap_or_default();
    let title = head.and_then(|h| runs_of(&h["title"]).first()).and_then(|r| r.get("text")).and_then(Value::as_str).unwrap_or("").to_string();
    let items = parse_items(json)
        .into_iter()
        .map(|mut it| {
            if it.kind == Kind::Song && it.thumb.is_empty() {
                it.thumb = thumb.clone();
            }
            it
        })
        .collect();
    (title, items)
}

/// A part of the user's library.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Shelf {
    #[default]
    Albums,
    Playlists,
    Artists,
    Liked,
}

impl Shelf {
    pub fn next(self) -> Self {
        match self {
            Self::Albums => Self::Playlists,
            Self::Playlists => Self::Artists,
            Self::Artists => Self::Liked,
            Self::Liked => Self::Albums,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Albums => "Albums",
            Self::Playlists => "Playlists",
            Self::Artists => "Artists",
            Self::Liked => "Liked Songs",
        }
    }

    pub fn placeholder(self) -> String {
        format!("Search your {}…", self.label().to_lowercase())
    }

    pub fn browse_id(self) -> &'static str {
        match self {
            Self::Albums => "FEmusic_liked_albums",
            Self::Playlists => "FEmusic_liked_playlists",
            Self::Artists => "FEmusic_library_corpus_track_artists",
            Self::Liked => "VLLM",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Control {
    PlayPause,
    Next,
    Previous,
    Like,
    Dislike,
    Mute,
    VolumeLow,
    VolumeMedium,
    VolumeHigh,
    VolumeUp,
    VolumeDown,
    Shuffle,
    Repeat,
}

impl Control {
    pub const ALL: [Control; 13] = [
        Self::Like,
        Self::Dislike,
        Self::PlayPause,
        Self::Next,
        Self::Previous,
        Self::Mute,
        Self::VolumeLow,
        Self::VolumeMedium,
        Self::VolumeHigh,
        Self::VolumeUp,
        Self::VolumeDown,
        Self::Shuffle,
        Self::Repeat,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Like => "Like",
            Self::Dislike => "Dislike",
            Self::PlayPause => "Play/Pause",
            Self::Next => "Next",
            Self::Previous => "Previous",
            Self::Mute => "Volume Mute",
            Self::VolumeLow => "Volume Low",
            Self::VolumeMedium => "Volume Medium",
            Self::VolumeHigh => "Volume High",
            Self::VolumeUp => "Volume Up",
            Self::VolumeDown => "Volume Down",
            Self::Shuffle => "Shuffle",
            Self::Repeat => "Repeat",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Like => "like",
            Self::Dislike => "dislike",
            Self::PlayPause => "play-pause",
            Self::Next => "next",
            Self::Previous => "previous",
            Self::Mute => "mute",
            Self::VolumeLow => "volume-low",
            Self::VolumeMedium => "volume-medium",
            Self::VolumeHigh => "volume-high",
            Self::VolumeUp => "volume-up",
            Self::VolumeDown => "volume-down",
            Self::Shuffle => "shuffle",
            Self::Repeat => "repeat",
        }
    }

    pub fn subtitle(self, step: u32) -> String {
        match self {
            Self::Like => "Like the current track".into(),
            Self::Dislike => "Dislike the current track".into(),
            Self::PlayPause => "Play or pause the current track".into(),
            Self::Next => "Skip to the next track".into(),
            Self::Previous => "Skip to the previous track".into(),
            Self::Mute => "Change the volume to 0%".into(),
            Self::VolumeLow => "Change the volume to 33%".into(),
            Self::VolumeMedium => "Change the volume to 66%".into(),
            Self::VolumeHigh => "Change the volume to 100%".into(),
            Self::VolumeUp => format!("Increase the volume by {step}%"),
            Self::VolumeDown => format!("Decrease the volume by {step}%"),
            Self::Shuffle => "Shuffle the queue".into(),
            Self::Repeat => "Change the repeat mode".into(),
        }
    }
}

pub fn search(req: u64, query: &str, filter: Filter) -> Value {
    json!({"t": "ytm", "req": req, "op": "search", "query": query, "filter": filter.name()})
}

pub fn browse(req: u64, id: &str) -> Value {
    json!({"t": "ytm", "req": req, "op": "browse", "id": id})
}

/// `op` is "like", "dislike" or "unlike". Without `video` it rates the track now playing.
pub fn rate(req: u64, op: &str, video: Option<&str>) -> Value {
    let mut v = json!({"t": "ytm", "req": req, "op": op});
    if let Some(video) = video {
        v["videoId"] = video.into();
    }
    v
}

pub fn add_to_playlist(req: u64, playlist_id: &str, video: Option<&str>) -> Value {
    let mut v = json!({"t": "ytm", "req": req, "op": "addToPlaylist", "playlistId": playlist_id.strip_prefix("VL").unwrap_or(playlist_id)});
    if let Some(video) = video {
        v["videoId"] = video.into();
    }
    v
}

/// With `radio` and no `video`, the radio of the track now playing.
#[allow(dead_code)]
pub fn play(req: u64, video: Option<&str>, playlist: Option<&str>, radio: bool) -> Value {
    let mut v = json!({"t": "ytm", "req": req, "op": "play"});
    if let Some(video) = video {
        v["videoId"] = video.into();
    }
    if let Some(playlist) = playlist {
        v["playlistId"] = playlist.into();
    }
    if radio {
        v["radio"] = true.into();
    }
    v
}

#[allow(dead_code)]
pub fn current(req: u64) -> Value {
    json!({"t": "ytm", "req": req, "op": "current"})
}

/// None for the controls that go through the Windows media session instead.
#[allow(dead_code)]
pub fn control(req: u64, control: Control, step: u32) -> Option<Value> {
    let action = |action: &str| json!({"t": "ytm", "req": req, "op": "control", "action": action});
    let valued = |action: &str, value: u32| json!({"t": "ytm", "req": req, "op": "control", "action": action, "value": value});
    match control {
        Control::PlayPause | Control::Next | Control::Previous => None,
        Control::Like => Some(rate(req, "like", None)),
        Control::Dislike => Some(rate(req, "dislike", None)),
        Control::Mute => Some(action("mute")),
        Control::VolumeLow => Some(valued("volume", 33)),
        Control::VolumeMedium => Some(valued("volume", 66)),
        Control::VolumeHigh => Some(valued("volume", 100)),
        Control::VolumeUp => Some(valued("volumeUp", step)),
        Control::VolumeDown => Some(valued("volumeDown", step)),
        Control::Shuffle => Some(action("shuffle")),
        Control::Repeat => Some(action("repeat")),
    }
}

pub fn watch_url(video: &str, playlist: &str) -> String {
    if playlist.is_empty() {
        format!("https://music.youtube.com/watch?v={video}")
    } else {
        format!("https://music.youtube.com/watch?v={video}&list={playlist}")
    }
}

#[allow(dead_code)]
pub fn playlist_url(playlist: &str) -> String {
    format!("https://music.youtube.com/watch?list={playlist}")
}

pub fn browse_url(browse_id: &str) -> String {
    format!("https://music.youtube.com/browse/{browse_id}")
}

impl MusicItem {
    pub fn url(&self) -> String {
        match self.kind {
            Kind::Song => watch_url(&self.id, &self.playlist_id),
            Kind::Album | Kind::Artist | Kind::Playlist => browse_url(&self.id),
        }
    }
}

/// Library lists older than this are fetched again.
pub const STALE: Duration = Duration::from_secs(600);

#[derive(Serialize, Deserialize)]
struct Saved {
    saved: u64,
    items: Vec<MusicItem>,
}

pub fn cache_path() -> PathBuf {
    config::dir().join("ytmusic").join("library.json")
}

/// The saved list of browse id `id` and how long ago it was saved.
pub fn load_cached(id: &str) -> Option<(Vec<MusicItem>, Duration)> {
    load_from(&cache_path(), id)
}

pub fn save_cached(id: &str, items: &[MusicItem]) {
    let _ = save_to(&cache_path(), id, items);
}

fn read_all(path: &Path) -> Option<BTreeMap<String, Saved>> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn load_from(path: &Path, id: &str) -> Option<(Vec<MusicItem>, Duration)> {
    let saved = read_all(path)?.remove(id)?;
    Some((saved.items, Duration::from_secs(now().saturating_sub(saved.saved))))
}

fn save_to(path: &Path, id: &str, items: &[MusicItem]) -> std::io::Result<()> {
    let mut all = read_all(path).unwrap_or_default();
    all.insert(id.to_string(), Saved { saved: now(), items: items.to_vec() });
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, serde_json::to_vec(&all)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> MusicItem {
        MusicItem { id: id.into(), title: format!("Song {id}"), ..Default::default() }
    }

    #[test]
    fn requests_follow_the_contract() {
        assert_eq!(search(1, "daft punk", Filter::Albums), json!({"t": "ytm", "req": 1, "op": "search", "query": "daft punk", "filter": "albums"}));
        assert_eq!(browse(2, "VLLM"), json!({"t": "ytm", "req": 2, "op": "browse", "id": "VLLM"}));
        assert_eq!(rate(3, "like", None), json!({"t": "ytm", "req": 3, "op": "like"}));
        assert_eq!(rate(3, "unlike", Some("abc")), json!({"t": "ytm", "req": 3, "op": "unlike", "videoId": "abc"}));
        assert_eq!(add_to_playlist(4, "VLPL1", Some("abc")), json!({"t": "ytm", "req": 4, "op": "addToPlaylist", "playlistId": "PL1", "videoId": "abc"}));
        assert_eq!(add_to_playlist(4, "PL1", None), json!({"t": "ytm", "req": 4, "op": "addToPlaylist", "playlistId": "PL1"}));
        assert_eq!(play(5, Some("abc"), None, false), json!({"t": "ytm", "req": 5, "op": "play", "videoId": "abc"}));
        assert_eq!(play(5, None, Some("PL1"), false), json!({"t": "ytm", "req": 5, "op": "play", "playlistId": "PL1"}));
        assert_eq!(play(5, None, None, true), json!({"t": "ytm", "req": 5, "op": "play", "radio": true}));
        assert_eq!(current(6), json!({"t": "ytm", "req": 6, "op": "current"}));
    }

    #[test]
    fn controls_map_to_requests() {
        let sent = |c| control(9, c, 10);
        assert_eq!(sent(Control::Mute), Some(json!({"t": "ytm", "req": 9, "op": "control", "action": "mute"})));
        assert_eq!(sent(Control::VolumeLow), Some(json!({"t": "ytm", "req": 9, "op": "control", "action": "volume", "value": 33})));
        assert_eq!(sent(Control::VolumeUp), Some(json!({"t": "ytm", "req": 9, "op": "control", "action": "volumeUp", "value": 10})));
        assert_eq!(sent(Control::Shuffle), Some(json!({"t": "ytm", "req": 9, "op": "control", "action": "shuffle"})));
        assert_eq!(sent(Control::Like), Some(json!({"t": "ytm", "req": 9, "op": "like"})));
        assert_eq!(sent(Control::Next), None);
        assert_eq!(Control::VolumeDown.subtitle(15), "Decrease the volume by 15%");
        assert_eq!(Control::ALL.len(), 13);
    }

    #[test]
    fn slugs_are_unique() {
        let mut slugs: Vec<_> = Control::ALL.iter().map(|c| c.slug()).collect();
        slugs.extend([Screen::Search, Screen::Library, Screen::NowPlaying, Screen::QuickActions].map(Screen::slug));
        let before = slugs.len();
        slugs.sort();
        slugs.dedup();
        assert_eq!(slugs.len(), before);
        assert_eq!((Control::VolumeLow.slug(), Screen::NowPlaying.slug()), ("volume-low", "now"));
    }

    #[test]
    fn filters_and_shelves_cycle() {
        let filters = std::iter::successors(Some(Filter::Songs), |f| Some(f.next()));
        let names: Vec<_> = filters.take(5).map(Filter::name).collect();
        assert_eq!(names, ["songs", "albums", "artists", "playlists", "songs"]);
        assert_eq!(Filter::Playlists.label(), "Playlists");
        let shelves = std::iter::successors(Some(Shelf::Albums), |s| Some(s.next()));
        let ids: Vec<_> = shelves.take(5).map(Shelf::browse_id).collect();
        assert_eq!(ids, ["FEmusic_liked_albums", "FEmusic_liked_playlists", "FEmusic_library_corpus_track_artists", "VLLM", "FEmusic_liked_albums"]);
        assert_eq!(Shelf::Liked.label(), "Liked Songs");
    }

    #[test]
    fn placeholders_name_what_is_searched() {
        assert_eq!((Filter::Songs.placeholder().as_str(), Filter::Playlists.placeholder().as_str()), ("Search songs…", "Search playlists…"));
        assert_eq!((Shelf::Albums.placeholder().as_str(), Shelf::Liked.placeholder().as_str()), ("Search your albums…", "Search your liked songs…"));
        assert_eq!((Filter::default(), Shelf::default()), (Filter::Songs, Shelf::Albums));
    }

    #[test]
    fn urls() {
        assert_eq!(watch_url("abc", ""), "https://music.youtube.com/watch?v=abc");
        assert_eq!(watch_url("abc", "PL1"), "https://music.youtube.com/watch?v=abc&list=PL1");
        assert_eq!(playlist_url("PL1"), "https://music.youtube.com/watch?list=PL1");
        assert_eq!(browse_url("MPREb_1"), "https://music.youtube.com/browse/MPREb_1");
        assert_eq!(MusicItem { playlist_id: "PL1".into(), ..song("abc") }.url(), "https://music.youtube.com/watch?v=abc&list=PL1");
        assert_eq!(MusicItem { kind: Kind::Album, ..song("MPREb_1") }.url(), "https://music.youtube.com/browse/MPREb_1");
    }

    #[test]
    fn reply_deserializes() {
        let r: Reply = serde_json::from_str(
            r#"{"t":"ytm","req":4,"op":"current","title":"Music","items":[{"kind":"playlist","id":"VLPL1","playlistId":"PL1","title":"Mix","subtitle":"","thumb":"","duration":""}],
                "current":{"videoId":"abc","title":"One","artist":"Band","playlistId":"","volume":40,"muted":true}}"#,
        )
        .unwrap();
        assert_eq!((r.req, r.op.as_str(), r.error), (4, "current", None));
        assert_eq!(r.items[0].kind, Kind::Playlist);
        assert_eq!(r.current, Some(Current { video_id: "abc".into(), title: "One".into(), artist: "Band".into(), volume: 40, muted: true, ..Default::default() }));
        let level = |json: &str| serde_json::from_str::<Current>(json).unwrap().volume;
        assert_eq!((level(r#"{"volume":33.4}"#), level(r#"{"volume":140}"#), level(r#"{"volume":-2.5}"#), level("{}")), (33, 100, 0, 0));
        let bare: Reply = serde_json::from_str(r#"{"req":1,"op":"like","current":null,"items":[{"title":"x"}]}"#).unwrap();
        assert_eq!((bare.current, bare.items[0].kind, bare.items[0].thumb.as_str()), (None, Kind::Song, ""));
    }

    #[test]
    fn cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("procast-ytmusic-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("ytmusic").join("library.json");
        assert!(load_from(&path, "VLLM").is_none(), "no file yet");
        save_to(&path, "VLLM", &[song("a"), song("b")]).unwrap();
        save_to(&path, "FEmusic_liked_albums", &[song("c")]).unwrap();
        let (items, age) = load_from(&path, "VLLM").unwrap();
        assert_eq!(items, [song("a"), song("b")]);
        assert!(age < Duration::from_secs(5));
        assert_eq!(load_from(&path, "FEmusic_liked_albums").unwrap().0, [song("c")]);
        assert!(load_from(&path, "other").is_none());
        fs::write(&path, "{ not json").unwrap();
        assert!(load_from(&path, "VLLM").is_none(), "a corrupt file is a miss");
        save_to(&path, "VLLM", &[song("d")]).unwrap();
        assert_eq!(load_from(&path, "VLLM").unwrap().0, [song("d")], "saving replaces a corrupt file");
        let _ = fs::remove_dir_all(&dir);
    }

    fn fixture(name: &str) -> Value {
        let raw = match name {
            "search-songs" => include_str!("../browser/test-fixtures/search-songs.json"),
            "search-albums" => include_str!("../browser/test-fixtures/search-albums.json"),
            "search-artists" => include_str!("../browser/test-fixtures/search-artists.json"),
            "search-playlists" => include_str!("../browser/test-fixtures/search-playlists.json"),
            "album" => include_str!("../browser/test-fixtures/album.json"),
            _ => panic!("unknown fixture {name}"),
        };
        serde_json::from_str(raw).unwrap()
    }

    fn first_is(items: &[MusicItem], kind: Kind, id: &str, title: &str) {
        let it = items.first().expect("expected at least one item");
        assert_eq!((it.kind, it.id.as_str(), it.title.as_str()), (kind, id, title));
    }

    #[test]
    fn songs_search_fixture() {
        let items = parse_items(&fixture("search-songs"));
        assert!(items.len() >= 3);
        for it in &items {
            assert_eq!(it.kind, Kind::Song);
            assert_eq!(it.id.len(), 11);
            assert!(!it.title.is_empty());
            assert!(it.thumb.starts_with("https://"));
            assert!(regex_duration(&it.duration), "{}", it.duration);
        }
        first_is(&items, Kind::Song, "pvC5YD-IjL0", "Holocene");
    }

    fn regex_duration(s: &str) -> bool {
        let parts: Vec<&str> = s.split(':').collect();
        matches!(parts.as_slice(), [m, sec] if !m.is_empty() && m.bytes().all(|c| c.is_ascii_digit()) && sec.len() == 2 && sec.bytes().all(|c| c.is_ascii_digit()))
            || matches!(parts.as_slice(), [h, m, sec] if !h.is_empty() && h.bytes().all(|c| c.is_ascii_digit()) && m.len() == 2 && sec.len() == 2 && m.bytes().all(|c| c.is_ascii_digit()) && sec.bytes().all(|c| c.is_ascii_digit()))
    }

    #[test]
    fn albums_search_fixture() {
        let items = parse_items(&fixture("search-albums"));
        assert!(items.len() >= 3);
        for it in &items {
            assert_eq!(it.kind, Kind::Album);
            assert!(it.id.starts_with("MPRE"));
            assert!(it.playlist_id.starts_with("OLAK5uy_"));
        }
        first_is(&items, Kind::Album, "MPREb_CV3KFsT8IdK", "Bon Iver, Bon Iver");
    }

    #[test]
    fn artists_search_fixture() {
        let items = parse_items(&fixture("search-artists"));
        assert!(items.len() >= 1);
        for it in &items {
            assert_eq!(it.kind, Kind::Artist);
            assert!(it.id.starts_with("UC"));
        }
        first_is(&items, Kind::Artist, "UCx-cwf1tCXoZr0KtnpaDwyA", "Bon Iver");
    }

    #[test]
    fn playlists_search_fixture() {
        let items = parse_items(&fixture("search-playlists"));
        assert!(items.len() >= 1);
        for it in &items {
            assert_eq!(it.kind, Kind::Playlist);
            assert!(!it.playlist_id.is_empty());
            assert!(!it.playlist_id.starts_with("VL"));
        }
        first_is(&items, Kind::Playlist, "VLPLyVScyoi1XjMBUmsLaWv_xlOOBtphtc3q", "Bon Iver Radio");
    }

    #[test]
    fn subtitles_use_the_middle_dot() {
        for it in parse_items(&fixture("search-songs")) {
            assert!(!it.subtitle.contains('•'), "{}", it.subtitle);
        }
    }

    #[test]
    fn album_page_fixture() {
        let (title, items) = parse_page(&fixture("album"));
        assert_eq!(title, "i,i");
        let songs: Vec<_> = items.iter().filter(|it| it.kind == Kind::Song).collect();
        assert!(songs.len() >= 3);
        for song in &songs {
            assert!(song.thumb.starts_with("https://"));
        }
        first_is(&items, Kind::Song, "XQCPmECH8IU", "Yi");
    }

    #[test]
    fn items_follow_object_key_parse_order() {
        let json: Value = serde_json::from_str(r#"{"z":{"musicTwoRowItemRenderer":{"title":{"runs":[{"text":"Zed","navigationEndpoint":{"browseEndpoint":{"browseId":"MPREb_z"}}}]}}},"a":{"musicTwoRowItemRenderer":{"title":{"runs":[{"text":"Aye","navigationEndpoint":{"browseEndpoint":{"browseId":"MPREb_a"}}}]}}}}"#).unwrap();
        first_is(&parse_items(&json), Kind::Album, "MPREb_z", "Zed");
    }

    #[test]
    fn malformed_renderer_is_skipped() {
        let json = json!({"musicResponsiveListItemRenderer": {"flexColumns": []}});
        assert_eq!(parse_items(&json).len(), 0);
    }

    #[test]
    fn thumbnail_renderer_fills_empty_thumbnail() {
        let json = json!({
            "musicTwoRowItemRenderer": {
                "title": {"runs": [{"text": "Album", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_x"}}}]},
                "thumbnail": {},
                "thumbnailRenderer": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://t/a"}]}}}
            }
        });
        assert_eq!(parse_items(&json)[0].thumb, "https://t/a");
    }

    #[test]
    fn wrapped_playlist_header_title() {
        let json = json!({
            "header": {
                "musicEditablePlaylistDetailHeaderRenderer": {
                    "header": {
                        "musicResponsiveHeaderRenderer": {
                            "title": {"runs": [{"text": "Mine"}]},
                            "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://t/small"}, {"url": "https://t/big"}]}}},
                            "buttons": [{"watchPlaylistEndpoint": {"playlistId": "PL1"}}]
                        }
                    }
                }
            }
        });
        let (title, _) = parse_page(&json);
        assert_eq!(title, "Mine");
    }
}
