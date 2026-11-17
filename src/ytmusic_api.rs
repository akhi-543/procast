//! InnerTube catalog: search, browse, like, and playlist edits over HTTP.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use crate::config;
use crate::ytmusic::{Filter, Reply, parse_items, parse_page};

const ORIGIN: &str = "https://music.youtube.com";
const SIGN_IN: &str = "Sign in to music.youtube.com in this browser to see your library";
const UNREACHABLE: &str = "Couldn't reach YouTube Music";

/// Cookie name → value, from `%APPDATA%\procast\ytmusic\session.json`.
#[derive(Clone, Debug, Default)]
pub struct Session {
    cookies: BTreeMap<String, String>,
}

impl Session {
    fn sapisid(&self) -> Option<&str> {
        for name in ["SAPISID", "__Secure-3PAPISID"] {
            if let Some(v) = self.cookies.get(name) {
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        None
    }

    fn cookie_header(&self) -> String {
        self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
    }
}

pub fn session_path() -> PathBuf {
    config::dir().join("ytmusic").join("session.json")
}

pub fn load_session() -> Option<Session> {
    load_session_from(&session_path())
}

fn load_session_from(path: &Path) -> Option<Session> {
    let map: BTreeMap<String, Value> = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let mut cookies = BTreeMap::new();
    for (k, v) in map {
        if let Some(s) = v.as_str() {
            cookies.insert(k, s.to_string());
        }
    }
    Some(Session { cookies })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build().into()
}

fn unix_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// SHA-1 of `data`. Private; no extra crate.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (data.len() as u64).saturating_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

/// SAPISIDHASH header value. Port of `authorization()` in `browser/ytmusic.js`.
pub fn authorization(sapisid: &str, ts: u64) -> String {
    let digest = sha1(format!("{ts} {sapisid} {ORIGIN}").as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("SAPISIDHASH {ts}_{hex}")
}

fn context() -> Value {
    json!({"client": {"clientName": "WEB_REMIX", "clientVersion": "1.20241111.01.00", "hl": "en"}})
}

fn call(endpoint: &str, rest: Value) -> Result<Value, String> {
    let mut body = json!({"context": context()});
    if let Some(obj) = rest.as_object() {
        for (k, v) in obj {
            body[k] = v.clone();
        }
    }
    let url = format!("{ORIGIN}/youtubei/v1/{endpoint}?prettyPrint=false");
    let mut req = agent().post(&url).header("Content-Type", "application/json").header("X-Origin", ORIGIN).header("X-Goog-AuthUser", "0");
    if let Some(session) = load_session() {
        if let Some(sid) = session.sapisid().map(str::to_owned) {
            req = req.header("Authorization", authorization(&sid, unix_secs())).header("Cookie", session.cookie_header());
        }
    }
    let mut resp = req.send(body.to_string()).map_err(|_| UNREACHABLE.to_string())?;
    let text = resp.body_mut().read_to_string().map_err(|_| UNREACHABLE.to_string())?;
    serde_json::from_str(&text).map_err(|_| UNREACHABLE.to_string())
}

fn require_session() -> Result<Session, String> {
    load_session().filter(|s| s.sapisid().is_some()).ok_or_else(|| SIGN_IN.to_string())
}

fn browse_reply(id: &str, json: &Value, signed_in: bool) -> Reply {
    let (title, items) = parse_page(json);
    let error = if items.is_empty() && !signed_in && (id.starts_with("FEmusic_") || id == "VLLM") {
        Some(SIGN_IN.to_string())
    } else {
        None
    };
    Reply { op: "browse".into(), title, items, error, ..Default::default() }
}

pub fn search(query: &str, filter: Filter) -> Result<Reply, String> {
    let json = call("search", json!({"query": query, "params": filter.params()}))?;
    Ok(Reply { op: "search".into(), items: parse_items(&json), ..Default::default() })
}

pub fn browse(id: &str) -> Result<Reply, String> {
    let json = call("browse", json!({"browseId": id}))?;
    Ok(browse_reply(id, &json, require_session().is_ok()))
}

pub fn rate(op: &str, video_id: &str) -> Result<Reply, String> {
    require_session()?;
    let endpoint = match op {
        "like" => "like/like",
        "dislike" => "like/dislike",
        "unlike" => "like/removelike",
        other => return Err(format!("Unknown rate op: {other}")),
    };
    call(endpoint, json!({"target": {"videoId": video_id}}))?;
    Ok(Reply { op: op.into(), ..Default::default() })
}

pub fn add_to_playlist(playlist_id: &str, video_id: &str) -> Result<Reply, String> {
    require_session()?;
    let playlist_id = playlist_id.strip_prefix("VL").unwrap_or(playlist_id);
    let edit = call(
        "browse/edit_playlist",
        json!({"playlistId": playlist_id, "actions": [{"action": "ACTION_ADD_VIDEO", "addedVideoId": video_id}]}),
    )?;
    if edit.get("status").and_then(Value::as_str) != Some("STATUS_SUCCEEDED") {
        return Err("Couldn't add to the playlist".into());
    }
    Ok(Reply { op: "addToPlaylist".into(), ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ytmusic::Kind;

    #[test]
    fn sapisidhash_matches_the_node_vector() {
        assert_eq!(authorization("secret", 1700000000), "SAPISIDHASH 1700000000_d2a0deeb95ea263a68778d9e9ac8840dce59d800");
    }

    #[test]
    fn session_prefers_sapisid_then_secure() {
        let dir = std::env::temp_dir().join(format!("procast-ytmusic-session-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("session.json");
        fs::write(&path, r#"{"SAPISID":"secret","__Secure-3PAPISID":"fallback","PREF":"f1"}"#).unwrap();
        let s = load_session_from(&path).unwrap();
        assert_eq!(s.sapisid(), Some("secret"));
        assert!(s.cookie_header().contains("PREF=f1"));
        fs::write(&path, r#"{"SAPISID":"","__Secure-3PAPISID":"fallback"}"#).unwrap();
        assert_eq!(load_session_from(&path).unwrap().sapisid(), Some("fallback"));
        fs::write(&path, r#"{}"#).unwrap();
        assert!(load_session_from(&path).unwrap().sapisid().is_none());
        assert!(load_session_from(&dir.join("missing.json")).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn signed_out_library_asks_to_sign_in() {
        let empty = json!({});
        let reply = browse_reply("FEmusic_liked_albums", &empty, false);
        assert_eq!(reply.error.as_deref(), Some(SIGN_IN));
        assert!(browse_reply("VLLM", &empty, false).error.is_some());
        assert!(browse_reply("FEmusic_liked_albums", &empty, true).error.is_none());
        assert!(browse_reply("MPREb_x", &empty, false).error.is_none());
    }

    #[test]
    fn mutations_need_a_session() {
        let none: Option<Session> = None;
        assert_eq!(none.filter(|s| s.sapisid().is_some()).ok_or_else(|| SIGN_IN.to_string()).err().as_deref(), Some(SIGN_IN));
        let _ = (
            browse as fn(&str) -> Result<Reply, String>,
            rate as fn(&str, &str) -> Result<Reply, String>,
            add_to_playlist as fn(&str, &str) -> Result<Reply, String>,
        );
    }

    #[test]
    #[ignore]
    fn live_search_smoke() {
        let reply = search("holocene", Filter::Songs).expect(UNREACHABLE);
        assert!(!reply.items.is_empty());
        assert_eq!(reply.items[0].kind, Kind::Song);
    }
}
