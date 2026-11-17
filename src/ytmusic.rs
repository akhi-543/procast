//! YouTube Music in the browser: what the companion extension answers with, the requests
//! procast sends it, and the library lists kept on disk between runs.

use std::{
    collections::BTreeMap,
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
}
