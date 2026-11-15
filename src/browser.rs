//! The browser companion (`browser/`, a Chrome/Edge extension). The browser starts
//! `procast.exe chrome-extension://<id>/` as its native messaging host; that host connects
//! to the running procast over a named pipe and relays. procast asks, the extension answers,
//! strictly one request at a time, so no pipe handle is ever read and written at once.
//! Frames on both legs are Chrome's: a u32 little-endian length, then JSON.

use std::{
    fs::File,
    io::{Read, Write},
    os::windows::io::{AsRawHandle, FromRawHandle},
    sync::{
        Arc, Mutex,
        mpsc::{Sender, channel},
    },
    time::Duration,
};

use futures::channel::mpsc::UnboundedSender;
use serde::Deserialize;
use serde_json::{Value, json};
use windows::{
    Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX},
        System::{
            Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PeekNamedPipe, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT},
            Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW},
        },
    },
    core::HSTRING,
};

use crate::{config, win::Msg};

const HOST: &str = "com.procast.browser";
/// Fixed by the public key in `browser/manifest.json`.
pub const EXTENSION_ID: &str = "mdbobfibhbhbeeogppclpodmbkjkbkal";
const MANIFEST: &str = include_str!("../browser/manifest.json");
const SCRIPT: &str = include_str!("../browser/background.js");
/// Where Focus sends a blocked site's tab.
const BLOCKED_PAGE: &str = include_str!("../browser/blocked.html");
const BLOCKED_SCRIPT: &str = include_str!("../browser/blocked.js");
/// Chrome's own cap on a message to the browser; anything bigger is a broken stream.
const MAX_FRAME: u32 = 64 << 20;

fn pipe_name() -> String {
    format!(r"\\.\pipe\procast-browser-{}", std::env::var("USERNAME").unwrap_or_default())
}

fn read_frame(r: &mut impl Read) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).ok()?;
    let len = u32::from_le_bytes(len);
    if len > MAX_FRAME {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn write_frame(w: &mut impl Write, body: &[u8]) -> Option<()> {
    w.write_all(&(body.len() as u32).to_le_bytes()).ok()?;
    w.write_all(body).ok()?;
    w.flush().ok()
}

// ---- the host (a procast.exe the browser starts) -------------------------------------

/// Relays between the browser (stdin/stdout) and the running procast (the pipe), waiting
/// for procast when it isn't running. Ends when the browser closes stdin.
pub fn host() {
    // While it waits for procast (or for procast's next request) nothing reads stdin, so a
    // browser that went away would go unnoticed: watch the pipe and leave with it.
    let stdin_handle = std::io::stdin().as_raw_handle() as isize;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            if unsafe { PeekNamedPipe(HANDLE(stdin_handle as _), None, 0, None, None, None) }.is_err() {
                std::process::exit(0);
            }
        }
    });
    let (mut stdin, mut stdout) = (std::io::stdin().lock(), std::io::stdout().lock());
    loop {
        let Ok(mut pipe) = std::fs::OpenOptions::new().read(true).write(true).open(pipe_name()) else {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        // procast asks, the browser answers.
        while let Some(req) = read_frame(&mut pipe) {
            if write_frame(&mut stdout, &req).is_none() {
                return;
            }
            let Some(reply) = read_frame(&mut stdin) else { return };
            if write_frame(&mut pipe, &reply).is_none() {
                break;
            }
        }
    }
}

// ---- procast's side ----------------------------------------------------------------

/// One tab in a connected browser.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Tab {
    pub id: i64,
    pub window: i64,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub icon: String,
}

/// A browser's answer.
#[derive(Debug)]
pub enum Reply {
    /// Browser name and its tabs.
    Tabs(String, Vec<Tab>),
    /// Done (activate, open) or failed.
    Done(Result<(), String>),
}

/// A connected browser: its id and the queue of requests for its worker.
type Conn = (u32, Sender<Value>);

/// The connected browsers.
#[derive(Clone, Default)]
pub struct Browsers(Arc<Mutex<Vec<Conn>>>);

impl Browsers {
    pub fn any(&self) -> bool {
        !self.0.lock().unwrap().is_empty()
    }

    /// Sends `req` to every browser; each answers with `Msg::Browser`.
    pub fn ask_all(&self, req: Value) {
        self.0.lock().unwrap().retain(|(_, q)| q.send(req.clone()).is_ok());
    }

    /// Sends `req` to one browser (the newest connected when `id` is None). False if it's gone.
    pub fn ask(&self, id: Option<u32>, req: Value) -> bool {
        let list = self.0.lock().unwrap();
        let q = match id {
            Some(id) => list.iter().find(|(i, _)| *i == id),
            None => list.last(),
        };
        q.is_some_and(|(_, q)| q.send(req).is_ok())
    }
}

/// Listens for browser hosts. Each connection gets a worker that sends requests and
/// passes the answers on as `Msg::Browser`.
pub fn serve(tx: UnboundedSender<Msg>) -> Browsers {
    let browsers = Browsers::default();
    let list = browsers.clone();
    std::thread::spawn(move || {
        let name = HSTRING::from(pipe_name());
        let mut first = true;
        for id in 0u32.. {
            // The first instance claims the name, so nothing else can be listening on it.
            let flags = if first { PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE } else { PIPE_ACCESS_DUPLEX };
            let h = unsafe { CreateNamedPipeW(&name, flags, PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, 1 << 16, 1 << 16, 0, None) };
            if h.is_invalid() {
                return;
            }
            first = false;
            // A client that connected between Create and Connect reports ERROR_PIPE_CONNECTED: fine.
            let _ = unsafe { ConnectNamedPipe(h, None) };
            let mut pipe = unsafe { File::from_raw_handle(h.0) };
            let (q, requests) = channel::<Value>();
            list.0.lock().unwrap().push((id, q));
            let _ = tx.unbounded_send(Msg::BrowserUp);
            let (tx, list) = (tx.clone(), list.clone());
            std::thread::spawn(move || {
                for req in requests {
                    let answer = write_frame(&mut pipe, req.to_string().as_bytes()).and_then(|_| read_frame(&mut pipe));
                    let Some(answer) = answer else {
                        // A link procast was asked to open still opens, the usual way.
                        if req["t"] == "open"
                            && let Some(url) = req["url"].as_str()
                        {
                            crate::win::open(url);
                        }
                        break;
                    };
                    let reply = parse_reply(&answer);
                    let _ = tx.unbounded_send(Msg::Browser(id, reply));
                }
                list.0.lock().unwrap().retain(|(i, _)| *i != id);
            });
        }
    });
    browsers
}

fn parse_reply(body: &[u8]) -> Reply {
    let v: Value = serde_json::from_slice(body).unwrap_or_default();
    if let Some(e) = v["error"].as_str() {
        return Reply::Done(Err(e.to_string()));
    }
    match v["t"].as_str() {
        Some("tabs") => Reply::Tabs(v["browser"].as_str().unwrap_or("Browser").to_string(), serde_json::from_value(v["tabs"].clone()).unwrap_or_default()),
        _ => Reply::Done(Ok(())),
    }
}

/// Where the extension is unpacked for "Load unpacked".
pub fn extension_dir() -> std::path::PathBuf {
    config::dir().join("browser-extension")
}

/// Registers procast.exe as the native messaging host for Chrome, Edge and Brave, and
/// unpacks the extension. Cheap and repeated at every start, so a moved exe stays registered.
pub fn register() -> std::io::Result<()> {
    let dir = extension_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("manifest.json"), MANIFEST)?;
    std::fs::write(dir.join("background.js"), SCRIPT)?;
    std::fs::write(dir.join("blocked.html"), BLOCKED_PAGE)?;
    std::fs::write(dir.join("blocked.js"), BLOCKED_SCRIPT)?;
    let exe = std::env::current_exe()?;
    let manifest = json!({
        "name": HOST,
        "description": "procast",
        "path": exe,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    let file = config::dir().join("browser-host.json");
    std::fs::write(&file, serde_json::to_string_pretty(&manifest)?)?;
    let value: Vec<u16> = file.to_string_lossy().encode_utf16().chain([0]).collect();
    for browser in [r"Google\Chrome", r"Microsoft\Edge", r"BraveSoftware\Brave-Browser"] {
        let key = HSTRING::from(format!(r"Software\{browser}\NativeMessagingHosts\{HOST}"));
        let _ = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, &key, None, REG_SZ.0, Some(value.as_ptr() as _), (value.len() * 2) as u32) };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = vec![];
        write_frame(&mut buf, br#"{"t":"tabs"}"#).unwrap();
        assert_eq!(&buf[..4], &12u32.to_le_bytes());
        assert_eq!(read_frame(&mut buf.as_slice()).unwrap(), br#"{"t":"tabs"}"#);
        assert!(read_frame(&mut &buf[..6]).is_none(), "a cut-off frame is the end");
        assert!(read_frame(&mut &u32::MAX.to_le_bytes()[..]).is_none(), "an absurd length is refused");
    }

    #[test]
    fn replies_parse() {
        let tabs = br#"{"t":"tabs","browser":"Edge","tabs":[{"id":4,"window":1,"title":"Docs","url":"https://d/","icon":""}]}"#;
        match parse_reply(tabs) {
            Reply::Tabs(b, t) => assert_eq!((b.as_str(), t[0].id, t[0].title.as_str()), ("Edge", 4, "Docs")),
            r => panic!("{r:?}"),
        }
        assert!(matches!(parse_reply(br#"{"t":"open","ok":true}"#), Reply::Done(Ok(()))));
        assert!(matches!(parse_reply(br#"{"t":"activate","error":"No tab with id: 9."}"#), Reply::Done(Err(_))));
    }

    #[test]
    fn the_blocked_page_is_plain_text_from_its_own_script() {
        assert!(BLOCKED_PAGE.contains(r#"src="blocked.js""#) && !BLOCKED_PAGE.contains("<style"), "no inline script (manifest v3), no styling");
        assert!(BLOCKED_SCRIPT.contains("is blocked by procast Focus"));
        assert!(SCRIPT.contains("blocked.html") && SCRIPT.contains(r#"case "focusSites""#));
    }

    #[test]
    fn manifest_has_a_key_and_native_messaging() {
        let m: Value = serde_json::from_str(MANIFEST).unwrap();
        assert!(m["permissions"].as_array().unwrap().iter().any(|p| p == "nativeMessaging"));
        assert!(m["key"].as_str().is_some_and(|k| k.len() > 300));
        assert_eq!(EXTENSION_ID.len(), 32);
    }
}
