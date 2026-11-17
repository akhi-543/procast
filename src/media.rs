//! What the browser is playing and its transport controls, through Windows' System Media
//! Transport Controls: in-process, pushed by Windows events, nothing polls.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::mpsc::{Receiver, Sender, channel},
};

use futures::channel::mpsc::UnboundedSender;
use windows::{
    Foundation::TypedEventHandler,
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionMediaProperties as Properties,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    },
    Storage::Streams::DataReader,
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize},
    core::RuntimeType,
};

use crate::win::Msg;

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub cover: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transport {
    PlayPause,
    Next,
    Previous,
}

#[derive(Clone, Copy)]
enum Event {
    Changed,
    SessionsChanged,
    Do(Transport),
}

pub struct Media(Sender<Event>);

impl Media {
    /// Starts the worker thread; it ends quietly when Windows has no media session manager.
    pub fn start(tx: UnboundedSender<Msg>) -> Media {
        let (events, rx) = channel();
        let own = events.clone();
        std::thread::spawn(move || run(own, rx, tx));
        Media(events)
    }

    pub fn send(&self, transport: Transport) {
        let _ = self.0.send(Event::Do(transport));
    }
}

fn run(events: Sender<Event>, rx: Receiver<Event>, out: UnboundedSender<Msg>) {
    unsafe { _ = RoInitialize(RO_INIT_MULTITHREADED) };
    let Ok(manager) = manager() else { return };
    remove_old_covers();
    let _ = manager.SessionsChanged(&notify(&events, Event::SessionsChanged));
    let _ = manager.CurrentSessionChanged(&notify(&events, Event::SessionsChanged));
    let mut worker = Worker { events, out, tracked: None, sent: None };
    worker.track(&manager);
    worker.publish();
    while let Ok(first) = rx.recv() {
        let (mut retrack, mut changed) = (false, false);
        for event in std::iter::once(first).chain(rx.try_iter()) {
            match event {
                Event::Do(transport) => worker.transport(transport),
                Event::SessionsChanged => retrack = true,
                Event::Changed => changed = true,
            }
        }
        if retrack {
            worker.track(&manager);
        }
        if retrack || changed {
            worker.publish();
        }
    }
}

struct Worker {
    events: Sender<Event>,
    out: UnboundedSender<Msg>,
    tracked: Option<Tracked>,
    /// The last state sent to the UI (`Some(None)`: "nothing playing" was sent).
    sent: Option<Option<NowPlaying>>,
}

impl Worker {
    fn track(&mut self, manager: &Manager) {
        let Ok(sessions) = sessions(manager) else { return };
        self.tracked = choose(&sessions).and_then(|i| Tracked::new(sessions[i].clone(), &self.events));
    }

    fn publish(&mut self) {
        let prev = self.sent.clone().flatten();
        let next = match &self.tracked {
            Some(tracked) => match read(&tracked.session, prev.as_ref()) {
                Ok(now) => Some(now),
                Err(_) => return,
            },
            None => None,
        };
        if self.sent.as_ref() == Some(&next) {
            return;
        }
        if let Some(old) = prev.and_then(|p| p.cover)
            && next.as_ref().and_then(|n| n.cover.as_ref()) != Some(&old)
        {
            let _ = std::fs::remove_file(old);
        }
        self.sent = Some(next.clone());
        let _ = self.out.unbounded_send(Msg::NowPlaying(next));
    }

    fn transport(&self, transport: Transport) {
        let Some(tracked) = &self.tracked else {
            let _ = self.out.unbounded_send(Msg::Toast(false, "Nothing is playing in a browser".into()));
            return;
        };
        let session = &tracked.session;
        let _ = match transport {
            Transport::PlayPause => session.TryTogglePlayPauseAsync(),
            Transport::Next => session.TrySkipNextAsync(),
            Transport::Previous => session.TrySkipPreviousAsync(),
        }
        .and_then(|a| a.join());
    }
}

/// A subscribed session; dropping it unsubscribes.
struct Tracked {
    session: Session,
    tokens: [i64; 2],
}

impl Tracked {
    fn new(session: Session, events: &Sender<Event>) -> Option<Tracked> {
        let properties = session.MediaPropertiesChanged(&notify(events, Event::Changed)).ok()?;
        let playback = match session.PlaybackInfoChanged(&notify(events, Event::Changed)) {
            Ok(token) => token,
            Err(_) => {
                let _ = session.RemoveMediaPropertiesChanged(properties);
                return None;
            }
        };
        Some(Tracked { session, tokens: [properties, playback] })
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        let _ = self.session.RemoveMediaPropertiesChanged(self.tokens[0]);
        let _ = self.session.RemovePlaybackInfoChanged(self.tokens[1]);
    }
}

/// A handler that only wakes the worker.
fn notify<S: RuntimeType + 'static, A: RuntimeType + 'static>(events: &Sender<Event>, event: Event) -> TypedEventHandler<S, A> {
    let events = events.clone();
    TypedEventHandler::new(move |_, _| {
        let _ = events.send(event);
        Ok(())
    })
}

fn manager() -> windows::core::Result<Manager> {
    Manager::RequestAsync()?.join()
}

fn sessions(manager: &Manager) -> windows::core::Result<Vec<Session>> {
    Ok(manager.GetSessions()?.into_iter().collect())
}

fn choose(sessions: &[Session]) -> Option<usize> {
    let info: Vec<(String, bool)> = sessions
        .iter()
        .map(|s| (s.SourceAppUserModelId().map(|id| id.to_string()).unwrap_or_default(), is_playing(s)))
        .collect();
    pick(&info)
}

fn is_playing(session: &Session) -> bool {
    session.GetPlaybackInfo().and_then(|info| info.PlaybackStatus()).is_ok_and(|status| status == Status::Playing)
}

/// The first playing browser session, else the first browser session; `(app id, playing)` each.
fn pick(sessions: &[(String, bool)]) -> Option<usize> {
    let browsers = || sessions.iter().enumerate().filter(|(_, (id, _))| is_browser(id));
    browsers().find(|(_, (_, playing))| *playing).or_else(|| browsers().next()).map(|(i, _)| i)
}

fn is_browser(aumid: &str) -> bool {
    let aumid = aumid.to_lowercase();
    ["chrome", "msedge", "brave", "music.youtube.com"].iter().any(|b| aumid.contains(b))
}

/// `prev` is what was last sent: its cover is kept while the track is the same.
fn read(session: &Session, prev: Option<&NowPlaying>) -> windows::core::Result<NowPlaying> {
    let properties = session.TryGetMediaPropertiesAsync()?.join()?;
    let title = properties.Title()?.to_string();
    let artist = properties.Artist()?.to_string();
    let cover = match prev {
        Some(p) if p.title == title && p.artist == artist && p.cover.is_some() => p.cover.clone(),
        _ => save_cover(&properties, &title, &artist),
    };
    Ok(NowPlaying { album: properties.AlbumTitle()?.to_string(), playing: is_playing(session), title, artist, cover })
}

/// The name is per track because the UI caches decoded images by path.
fn cover_name(title: &str, artist: &str) -> String {
    let mut hasher = DefaultHasher::new();
    (title, artist).hash(&mut hasher);
    format!("cover-{:016x}", hasher.finish())
}

fn covers_dir() -> PathBuf {
    crate::config::dir().join("ytmusic")
}

fn remove_old_covers() {
    let Ok(entries) = std::fs::read_dir(covers_dir()) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("cover-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn save_cover(properties: &Properties, title: &str, artist: &str) -> Option<PathBuf> {
    let bytes = thumbnail(properties)?;
    let dir = covers_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(cover_name(title, artist));
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

fn thumbnail(properties: &Properties) -> Option<Vec<u8>> {
    let stream = properties.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = u32::try_from(stream.Size().ok()?).ok().filter(|&size| size > 0)?;
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut bytes = vec![0; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str, playing: bool) -> (String, bool) {
        (id.to_string(), playing)
    }

    #[test]
    fn browsers_are_recognised() {
        assert!(is_browser("Chrome"));
        assert!(is_browser("MSEdge"));
        assert!(is_browser("Brave"));
        assert!(is_browser("music.youtube.com-abc"));
        assert!(!is_browser("Spotify.exe"));
    }

    #[test]
    fn a_playing_browser_wins() {
        assert_eq!(pick(&[s("Chrome", false), s("MSEdge", true)]), Some(1));
        assert_eq!(pick(&[s("Spotify.exe", true), s("Chrome", false)]), Some(1));
        assert_eq!(pick(&[s("Chrome", false), s("Brave", false)]), Some(0));
    }

    #[test]
    fn other_players_are_ignored() {
        assert_eq!(pick(&[s("Spotify.exe", true)]), None);
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn cover_names_follow_the_track() {
        assert_eq!(cover_name("Song", "Artist"), cover_name("Song", "Artist"));
        assert_ne!(cover_name("Song", "Artist"), cover_name("Other", "Artist"));
        assert_ne!(cover_name("Song", "Artist"), cover_name("Song", "Someone"));
        assert_ne!(cover_name("ab", "c"), cover_name("a", "bc"));
    }

    #[test]
    #[ignore = "reads the live Windows media session"]
    fn live_session() {
        unsafe { _ = RoInitialize(RO_INIT_MULTITHREADED) };
        let sessions = sessions(&manager().unwrap()).unwrap();
        let ids: Vec<String> = sessions.iter().map(|s| s.SourceAppUserModelId().unwrap().to_string()).collect();
        println!("sessions: {ids:?}");
        match choose(&sessions) {
            Some(i) => println!("{:#?}", read(&sessions[i], None).unwrap()),
            None => println!("no browser session"),
        }
    }
}
