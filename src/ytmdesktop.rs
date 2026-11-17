use std::{fs, path::PathBuf, time::Duration};

use serde_json::{Value, json};

use crate::{config, media::NowPlaying, ytmusic::Control};


pub const OPEN_APP: &str = "Open YouTube Music Desktop App (companion server on)";
const REAUTH: &str = "Re-authorize procast in YouTube Music Desktop App";

pub struct Desktop {
    pub token_path: PathBuf,
    pub base: String,
}

impl Desktop {
    pub fn new(base: impl Into<String>) -> Self {
        Self { token_path: token_path(), base: base.into() }
    }
}

pub fn token_path() -> PathBuf {
    config::dir().join("ytmusic").join("desktop-token")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build().into()
}

fn auth_agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(35))).build().into()
}

fn root(base: &str) -> &str {
    base.trim_end_matches('/')
}

fn map_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(401) => {
            let _ = fs::remove_file(token_path());
            REAUTH.into()
        }
        ureq::Error::StatusCode(code) => format!("YouTube Music Desktop App returned {code}"),
        _ => OPEN_APP.into(),
    }
}

fn post_json(agent: &ureq::Agent, url: &str, token: Option<&str>, body: &Value) -> Result<ureq::http::Response<ureq::Body>, String> {
    let mut req = agent.post(url).header("Content-Type", "application/json");
    if let Some(token) = token {
        req = req.header("Authorization", token);
    }
    req.send(body.to_string()).map_err(map_err)
}

fn read_json(mut resp: ureq::http::Response<ureq::Body>) -> Result<Value, String> {
    let text = resp.body_mut().read_to_string().map_err(|_| OPEN_APP.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub fn change_video(video: Option<&str>, playlist: Option<&str>) -> Result<Value, String> {
    if video.is_none() && playlist.is_none() {
        return Err("videoId or playlistId is required".into());
    }
    let mut data = serde_json::Map::new();
    if let Some(video) = video {
        data.insert("videoId".into(), json!(video));
    }
    if let Some(playlist) = playlist {
        data.insert("playlistId".into(), json!(playlist));
    }
    Ok(json!({"command": "changeVideo", "data": data}))
}

fn set_volume(level: u32) -> Value {
    json!({"command": "setVolume", "data": level})
}

pub fn transport(control: Control, step: u32) -> Value {
    let _ = step;
    match control {
        Control::PlayPause => json!({"command": "playPause"}),
        Control::Next => json!({"command": "next"}),
        Control::Previous => json!({"command": "previous"}),
        Control::Like => json!({"command": "toggleLike"}),
        Control::Dislike => json!({"command": "toggleDislike"}),
        Control::Mute => json!({"command": "mute"}),
        Control::VolumeLow => set_volume(33),
        Control::VolumeMedium => set_volume(66),
        Control::VolumeHigh => set_volume(100),
        Control::VolumeUp => json!({"command": "volumeUp"}),
        Control::VolumeDown => json!({"command": "volumeDown"}),
        Control::Shuffle => json!({"command": "shuffle"}),
        Control::Repeat => json!({"command": "repeatMode", "data": 1}),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub video_id: String,
    pub playlist_id: String,
    pub playing: bool,
    pub volume: u32,
    pub muted: bool,
    pub like_status: i64,
    pub thumb_url: String,
}

pub fn parse_state(v: &Value) -> State {
    let player = &v["player"];
    let video = &v["video"];
    let missing = video.is_null();
    let track = player["trackState"].as_i64().unwrap_or(0);
    let volume = player["volume"].as_f64().unwrap_or(0.) as u32;
    let muted = player["muted"].as_bool().unwrap_or(false);
    let thumb_url = video["thumbnails"]
        .as_array()
        .and_then(|t| t.last())
        .and_then(|t| t["url"].as_str())
        .unwrap_or("")
        .to_string();
    State {
        title: if missing { String::new() } else { video["title"].as_str().unwrap_or("").into() },
        artist: video["author"].as_str().unwrap_or("").into(),
        album: video["album"].as_str().unwrap_or("").into(),
        video_id: video["id"].as_str().unwrap_or("").into(),
        playlist_id: v["playlistId"].as_str().unwrap_or("").into(),
        playing: track == 1 || track == 2,
        volume,
        muted,
        like_status: if missing { -1 } else { video["likeStatus"].as_i64().unwrap_or(-1) },
        thumb_url,
    }
}

/// Maps companion `/state` onto the Now Playing row. Cover is filled by the caller
/// (`net::cached_image` on `thumb_url`); video/playlist ids are for Copy URL.
pub fn to_now_playing(state: &State) -> (NowPlaying, String, String) {
    (
        NowPlaying {
            title: state.title.clone(),
            artist: state.artist.clone(),
            album: state.album.clone(),
            playing: state.playing,
            cover: None,
            video_id: state.video_id.clone(),
            playlist_id: state.playlist_id.clone(),
        },
        state.video_id.clone(),
        state.playlist_id.clone(),
    )
}

pub fn radio_playlist(video_id: &str) -> String {
    format!("RDAMVM{video_id}")
}

pub fn metadata(base: &str) -> Result<bool, String> {
    agent().get(&format!("{}/metadata", root(base))).call().map(|_| true).map_err(|_| OPEN_APP.to_string())
}

pub fn ensure_token(desktop: &Desktop) -> Result<String, String> {
    if let Ok(existing) = fs::read_to_string(&desktop.token_path) {
        let existing = existing.trim();
        if !existing.is_empty() {
            return Ok(existing.to_string());
        }
    }
    let base = root(&desktop.base);
    let code_body = json!({
        "appId": "procast",
        "appName": "procast",
        "appVersion": env!("CARGO_PKG_VERSION"),
    });
    let code_resp = post_json(&agent(), &format!("{base}/api/v1/auth/requestcode"), None, &code_body)?;
    let code = read_json(code_resp)?;
    let code = code["code"].as_str().ok_or_else(|| "YouTube Music Desktop App did not return an auth code".to_string())?;
    let token_resp = post_json(&auth_agent(), &format!("{base}/api/v1/auth/request"), None, &json!({"appId": "procast", "code": code}))?;
    let token_json = read_json(token_resp)?;
    let token = token_json["token"].as_str().ok_or_else(|| "YouTube Music Desktop App did not return a token".to_string())?;
    if let Some(dir) = desktop.token_path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&desktop.token_path, token).map_err(|e| e.to_string())?;
    Ok(token.to_string())
}

pub fn command(token: &str, base: &str, body: Value) -> Result<(), String> {
    post_json(&agent(), &format!("{}/api/v1/command", root(base)), Some(token), &body)?;
    Ok(())
}

pub fn play(token: &str, base: &str, video: Option<&str>, playlist: Option<&str>) -> Result<(), String> {
    command(token, base, change_video(video, playlist)?)
}

pub fn state(token: &str, base: &str) -> Result<State, String> {
    let mut resp = agent().get(&format!("{}/api/v1/state", root(base))).header("Authorization", token).call().map_err(map_err)?;
    let text = resp.body_mut().read_to_string().map_err(|_| OPEN_APP.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(parse_state(&v))
}

pub fn playlists(token: &str, base: &str) -> Result<Vec<(String, String)>, String> {
    let mut resp = agent().get(&format!("{}/api/v1/playlists", root(base))).header("Authorization", token).call().map_err(map_err)?;
    let text = resp.body_mut().read_to_string().map_err(|_| OPEN_APP.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let list = v.as_array().ok_or_else(|| "YouTube Music Desktop App did not return playlists".to_string())?;
    Ok(list
        .iter()
        .filter_map(|p| Some((p["id"].as_str()?.to_string(), p["title"].as_str()?.to_string())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn change_video_json() {
        assert_eq!(
            change_video(Some("dQw4w9WgXcQ"), Some("MLCT")).unwrap(),
            json!({"command":"changeVideo","data":{"videoId":"dQw4w9WgXcQ","playlistId":"MLCT"}})
        );
        assert_eq!(
            change_video(Some("dQw4w9WgXcQ"), None).unwrap(),
            json!({"command":"changeVideo","data":{"videoId":"dQw4w9WgXcQ"}})
        );
        assert_eq!(
            change_video(None, Some("MLCT")).unwrap(),
            json!({"command":"changeVideo","data":{"playlistId":"MLCT"}})
        );
        assert!(change_video(None, None).is_err());
        assert!(play("unused", "http://127.0.0.1:1", None, None).is_err());
    }

    #[test]
    fn ensure_token_errors_when_app_closed() {
        let dir = std::env::temp_dir().join(format!("procast-ytmdesktop-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let desktop = Desktop { token_path: dir.join("desktop-token"), base: "http://127.0.0.1:1".into() };
        let err = ensure_token(&desktop).unwrap_err();
        assert_eq!(err, OPEN_APP);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn play_pause_and_set_volume_json() {
        assert_eq!(transport(Control::PlayPause, 10), json!({"command":"playPause"}));
        assert_eq!(set_volume(50), json!({"command":"setVolume","data":50}));
        assert_eq!(transport(Control::VolumeLow, 10), json!({"command":"setVolume","data":33}));
        assert_eq!(transport(Control::VolumeMedium, 10), json!({"command":"setVolume","data":66}));
        assert_eq!(transport(Control::VolumeHigh, 10), json!({"command":"setVolume","data":100}));
        assert_eq!(transport(Control::VolumeUp, 15), json!({"command":"volumeUp"}));
        assert_eq!(transport(Control::VolumeDown, 15), json!({"command":"volumeDown"}));
        assert_eq!(transport(Control::Repeat, 10), json!({"command":"repeatMode","data":1}));
        assert_eq!(transport(Control::Next, 10), json!({"command":"next"}));
        assert_eq!(transport(Control::Previous, 10), json!({"command":"previous"}));
        assert_eq!(transport(Control::Like, 10), json!({"command":"toggleLike"}));
        assert_eq!(transport(Control::Dislike, 10), json!({"command":"toggleDislike"}));
        assert_eq!(transport(Control::Mute, 10), json!({"command":"mute"}));
        assert_eq!(transport(Control::Shuffle, 10), json!({"command":"shuffle"}));
    }

    #[test]
    fn state_maps_to_now_playing_and_radio_playlist() {
        let state = State {
            title: "One More Time".into(),
            artist: "Daft Punk".into(),
            album: "Discovery".into(),
            video_id: "FGBhQbmPwH8".into(),
            playlist_id: "OLAK5".into(),
            playing: true,
            volume: 40,
            muted: false,
            like_status: 2,
            thumb_url: "https://example.test/large.jpg".into(),
        };
        let (now, video, playlist) = to_now_playing(&state);
        assert_eq!(now.title, "One More Time");
        assert_eq!(now.artist, "Daft Punk");
        assert_eq!(now.album, "Discovery");
        assert!(now.playing);
        assert_eq!(now.cover, None);
        assert_eq!((now.video_id.as_str(), now.playlist_id.as_str()), ("FGBhQbmPwH8", "OLAK5"));
        assert_eq!(video, "FGBhQbmPwH8");
        assert_eq!(playlist, "OLAK5");
        assert_eq!(radio_playlist("FGBhQbmPwH8"), "RDAMVMFGBhQbmPwH8");
    }

    #[test]
    fn state_parses_fixture() {
        let v: Value = serde_json::from_str(
            r#"{
  "player": {"trackState": 1, "videoProgress": 12, "volume": 40, "muted": false, "adPlaying": false},
  "video": {
    "author": "Daft Punk",
    "title": "One More Time",
    "album": "Discovery",
    "likeStatus": 2,
    "thumbnails": [{"url": "https://example.test/small.jpg", "width": 60, "height": 60}, {"url": "https://example.test/large.jpg", "width": 544, "height": 544}],
    "id": "FGBhQbmPwH8"
  },
  "playlistId": "OLAK5"
}"#,
        )
        .unwrap();
        let s = parse_state(&v);
        assert_eq!(s.title, "One More Time");
        assert_eq!(s.artist, "Daft Punk");
        assert_eq!(s.album, "Discovery");
        assert_eq!(s.video_id, "FGBhQbmPwH8");
        assert_eq!(s.playlist_id, "OLAK5");
        assert!(s.playing);
        assert_eq!(s.volume, 40);
        assert!(!s.muted);
        assert_eq!(s.like_status, 2);
        assert_eq!(s.thumb_url, "https://example.test/large.jpg");
    }

    #[test]
    fn state_handles_null_video_and_mute() {
        let none: Value = serde_json::from_str(r#"{"player":{"trackState":0,"volume":0},"video":null,"playlistId":""}"#).unwrap();
        let s = parse_state(&none);
        assert_eq!(
            (s.title.as_str(), s.artist.as_str(), s.album.as_str(), s.video_id.as_str(), s.playing, s.volume, s.muted, s.like_status, s.thumb_url.as_str()),
            ("", "", "", "", false, 0, false, -1, "")
        );
        let buffering: Value = json!({"player":{"trackState":2,"volume":10},"video":{"title":"x","author":"y","id":"z","likeStatus":0,"thumbnails":[]},"playlistId":"P"});
        let b = parse_state(&buffering);
        assert!(b.playing);
        assert_eq!((b.like_status, b.thumb_url.as_str(), b.playlist_id.as_str()), (0, "", "P"));
        let paused: Value = json!({"player":{"trackState":-1,"volume":5,"muted":true},"video":{"title":"t"}});
        let p = parse_state(&paused);
        assert!(!p.playing);
        assert!(p.muted);
        assert_eq!(p.like_status, -1);
    }

    #[test]
    #[ignore]
    fn desktop_live_metadata() {
        if metadata("http://127.0.0.1:9863").is_err() {
            return;
        }
        let desktop = Desktop::new("http://127.0.0.1:9863");
        if !desktop.token_path.exists() {
            return;
        }
        let Ok(token) = fs::read_to_string(&desktop.token_path) else { return };
        let token = token.trim();
        if token.is_empty() {
            return;
        }
        let _ = state(token, &desktop.base);
        let _ = playlists(token, &desktop.base);
    }
}
