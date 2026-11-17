//! YouTube Music inside the launcher: its screens, and what the companion's answers and the
//! media session's events do to them.

use std::collections::HashSet;

use gpui::Context;
use serde_json::Value;

use crate::{
    Launcher, Mode, Tone,
    commands::{self, Cmd},
    design::g,
    empty_row,
    media::{Media, NowPlaying, Transport},
    search::{self, Act, Icon, Item},
    win::{self, Msg},
    ytmusic::{self, Control, MusicItem, Reply, Screen},
};

#[derive(Clone, Debug, PartialEq)]
pub enum MusicAct {
    #[allow(dead_code)]
    Open(MusicItem),
    Play { video: Option<String>, playlist: Option<String>, radio: bool },
    /// "like", "dislike" or "unlike", for a video (None: the track now playing).
    #[allow(dead_code)]
    Rate(&'static str, Option<String>),
    /// A playlist id and a video (None: the track now playing).
    #[allow(dead_code)]
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

#[derive(Default)]
pub struct MusicState {
    screen: Screen,
    /// The last request id handed out.
    req: u64,
    /// The request whose reply fills the view.
    pending: Option<u64>,
    /// Volume requests in flight: their replies say the new level.
    volume_reqs: HashSet<u64>,
    #[allow(dead_code)]
    items: Vec<MusicItem>,
    #[allow(dead_code)]
    loading: bool,
    #[allow(dead_code)]
    error: Option<String>,
    now: Option<NowPlaying>,
}

impl Launcher {
    fn media(&mut self) -> &Media {
        let tx = &self.tx;
        self.media.get_or_insert_with(|| Media::start(tx.clone()))
    }

    pub fn music_open(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.media();
        let music = &mut self.music;
        music.screen = screen;
        music.pending = None;
        music.loading = false;
        music.items.clear();
        music.error = None;
        self.set_mode(Mode::Music, cx);
    }

    pub fn music_label(&self) -> String {
        let screen = match self.music.screen {
            Screen::Search => "Search",
            Screen::Library => "Library",
            Screen::NowPlaying => "Now Playing",
            Screen::QuickActions => "Quick Actions",
        };
        format!("YouTube Music · {screen}")
    }

    pub fn music_placeholder(&self) -> &'static str {
        match self.music.screen {
            Screen::NowPlaying | Screen::QuickActions => "Search actions…",
            Screen::Search | Screen::Library => "Search YouTube Music…",
        }
    }

    pub fn music_rows(&mut self, query: &str) -> Vec<Item> {
        let mut rows = match self.music.screen {
            Screen::NowPlaying => vec![self.track_row()],
            Screen::QuickActions => vec![],
            Screen::Search | Screen::Library => return vec![],
        };
        let connected = self.browsers.any();
        let controls = control_rows(self.cfg.ytmusic.volume_step).into_iter().filter(|row| connected || matches!(row.act, Act::Music(MusicAct::Control(c)) if transport(c).is_some())).collect();
        rows.extend(search::filter(query, controls, &mut self.matcher));
        if !connected {
            rows.push(empty_row(g::GLOBE, "No browser connected".into(), "Run Set Up Browser Extension, then load the extension in Chrome, Edge or Brave".into(), vec![]));
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

    /// Esc: a page of Library or Search goes back to its list. False when there is none to leave.
    pub fn music_back(&mut self, _cx: &mut Context<Self>) -> bool {
        false
    }

    pub fn music_act(&mut self, act: MusicAct, cx: &mut Context<Self>) {
        let sent = match act {
            MusicAct::Control(control) => return self.music_control(control, cx),
            MusicAct::Open(item) => {
                win::open(&item.url());
                true
            }
            MusicAct::Play { video, playlist, radio } => self.music_ask(|req| Some(ytmusic::play(req, video.as_deref(), playlist.as_deref(), radio))),
            MusicAct::Rate(op, video) => self.music_ask(|req| Some(ytmusic::rate(req, op, video.as_deref()))),
            MusicAct::AddToPlaylist(playlist, video) => self.music_ask(|req| Some(ytmusic::add_to_playlist(req, &playlist, video.as_deref()))),
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

    pub fn music_reply(&mut self, reply: Reply, cx: &mut Context<Self>) {
        if matches!(reply.op.as_str(), "search" | "browse") {
            if self.music.pending == Some(reply.req) {
                self.music.pending = None;
                self.music.loading = false;
                self.music.error = reply.error;
                self.music.items = reply.items;
                if self.visible && self.mode == Mode::Music {
                    self.refresh(cx);
                }
            }
            return;
        }
        let volume = self.music.volume_reqs.remove(&reply.req);
        if let Some((ok, text)) = toast(&reply, volume) {
            let _ = self.tx.unbounded_send(Msg::Toast(ok, text));
        }
    }

    pub fn music_now(&mut self, now: Option<NowPlaying>, cx: &mut Context<Self>) {
        self.music.now = now;
        if self.visible && self.mode == Mode::Music && self.music.screen == Screen::NowPlaying {
            let selected = self.selected;
            self.refresh(cx);
            self.selected = selected.min(self.items.len().saturating_sub(1));
            self.scroll_to_selected();
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

fn announces_volume(control: Control) -> bool {
    matches!(control, Control::Mute | Control::VolumeLow | Control::VolumeMedium | Control::VolumeHigh | Control::VolumeUp | Control::VolumeDown)
}

fn toast(reply: &Reply, volume: bool) -> Option<(bool, String)> {
    if let Some(error) = &reply.error {
        return Some((false, error.clone()));
    }
    match reply.op.as_str() {
        "like" => Some((true, "Liked".into())),
        "dislike" => Some((true, "Disliked".into())),
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
        assert_eq!(toast(&reply("like", None, None), false), Some((true, "Liked".into())));
        assert_eq!(toast(&reply("dislike", None, None), false), Some((true, "Disliked".into())));
        assert_eq!(toast(&reply("control", None, level.clone()), true), Some((true, "Volume 40%".into())));
        assert_eq!(toast(&reply("control", None, muted), true), Some((true, "Muted".into())));
        assert_eq!(toast(&reply("control", None, level.clone()), false), None, "shuffle and repeat stay quiet");
        assert_eq!(toast(&reply("control", None, None), true), None);
        assert_eq!(toast(&reply("play", None, None), false), None);
        assert_eq!(toast(&reply("control", Some("Open YouTube Music first."), level), true), Some((false, "Open YouTube Music first.".into())));
    }
}
