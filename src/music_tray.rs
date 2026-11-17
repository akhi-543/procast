//! The YouTube Music tray icon: its menu as data, and the tray icon built from it.

use futures::channel::mpsc::UnboundedSender;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
};

use crate::{
    ext,
    media::NowPlaying,
    win::{Msg, TrayCmd},
    ytmusic::MusicItem,
};

const PREFIX: &str = "ytm|";
const LAUNCHER: &str = "ytm|launcher";
/// Windows cuts tray tooltips at 127 characters.
const TOOLTIP_CHARS: usize = 120;

#[derive(Debug, PartialEq)]
pub enum Node {
    Item { label: String, id: String, enabled: bool },
    Separator,
    Submenu(String, Vec<Node>),
}

#[derive(Debug, PartialEq)]
pub enum Action {
    PlayPause,
    Like,
    Next,
    Previous,
    Radio,
    Copy,
    Open,
    /// The playlist's id, without the `VL` of its browse id.
    Add(String),
}

impl Action {
    pub fn parse(id: &str) -> Option<Self> {
        let action = id.strip_prefix(PREFIX)?;
        Some(match action {
            "playpause" => Self::PlayPause,
            "like" => Self::Like,
            "next" => Self::Next,
            "previous" => Self::Previous,
            "radio" => Self::Radio,
            "copy" => Self::Copy,
            "open" => Self::Open,
            _ => Self::Add(action.strip_prefix("add|").filter(|playlist| !playlist.is_empty())?.to_string()),
        })
    }
}

/// What a click on the menu item `id` asks of the launcher, if the item is one of ours.
pub fn message(id: &str) -> Option<Msg> {
    if id == LAUNCHER {
        return Some(Msg::Tray(TrayCmd::Open));
    }
    Action::parse(id).map(|_| Msg::TrayMusic(id.to_string()))
}

/// Takes the menu clicks when no other tray icon of procast is there to.
pub fn listen(tx: UnboundedSender<Msg>) {
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if let Some(msg) = message(&e.id.0) {
            let _ = tx.unbounded_send(msg);
        }
    }));
}

fn item(label: &str, id: &str) -> Node {
    Node::Item { label: label.into(), id: id.into(), enabled: true }
}

pub fn menu_model(now: Option<&NowPlaying>, playlists: &[MusicItem]) -> Vec<Node> {
    let header = Node::Item { label: now.map_or("Nothing playing", |now| now.title.as_str()).into(), id: "ytm|title".into(), enabled: false };
    let mut nodes = vec![
        header,
        Node::Separator,
        item(if now.is_some_and(|now| now.playing) { "Pause" } else { "Play" }, "ytm|playpause"),
        item("Like", "ytm|like"),
        item("Next", "ytm|next"),
        item("Previous", "ytm|previous"),
        Node::Separator,
        item("Start Radio", "ytm|radio"),
    ];
    let adds: Vec<Node> = playlists
        .iter()
        .filter_map(|playlist| {
            let id = playlist.id.strip_prefix("VL").filter(|id| id.starts_with("PL"))?;
            Some(item(&playlist.title, &format!("ytm|add|{id}")))
        })
        .collect();
    if !adds.is_empty() {
        nodes.push(Node::Submenu("Add to Playlist".into(), adds));
    }
    nodes.extend([Node::Separator, item("Copy URL", "ytm|copy"), item("Open YouTube Music", "ytm|open"), Node::Separator, item("Open procast", LAUNCHER)]);
    nodes
}

pub fn tooltip(now: Option<&NowPlaying>) -> String {
    let Some(now) = now else { return "YouTube Music".into() };
    let text = if now.artist.is_empty() { now.title.clone() } else { format!("{} — {}", now.title, now.artist) };
    text.chars().take(TOOLTIP_CHARS).collect()
}

fn icon(now: Option<&NowPlaying>) -> Icon {
    now.and_then(|now| now.cover.as_deref())
        .and_then(ext::load_rgba)
        .and_then(|(rgba, w, h)| Icon::from_rgba(rgba, w, h).ok())
        .unwrap_or_else(crate::tray_icon_image)
}

fn append(nodes: &[Node], push: &dyn Fn(&dyn IsMenuItem)) {
    for node in nodes {
        match node {
            Node::Item { label, id, enabled } => push(&MenuItem::with_id(MenuId::new(id), label, *enabled, None)),
            Node::Separator => push(&PredefinedMenuItem::separator()),
            Node::Submenu(label, children) => {
                let sub = Submenu::new(label, true);
                append(children, &|item| drop(sub.append(item)));
                push(&sub);
            }
        }
    }
}

fn menu(now: Option<&NowPlaying>, playlists: &[MusicItem]) -> Menu {
    let menu = Menu::new();
    append(&menu_model(now, playlists), &|item| drop(menu.append(item)));
    menu
}

pub fn create(now: Option<&NowPlaying>, playlists: &[MusicItem]) -> Option<TrayIcon> {
    TrayIconBuilder::new().with_menu(Box::new(menu(now, playlists))).with_tooltip(tooltip(now)).with_icon(icon(now)).build().ok()
}

pub fn update(tray: &TrayIcon, now: Option<&NowPlaying>, playlists: &[MusicItem]) {
    tray.set_menu(Some(Box::new(menu(now, playlists))));
    let _ = tray.set_tooltip(Some(tooltip(now)));
    let _ = tray.set_icon(Some(icon(now)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ytmusic::Kind;

    fn now(title: &str, artist: &str, playing: bool) -> NowPlaying {
        NowPlaying { title: title.into(), artist: artist.into(), album: String::new(), playing, cover: None }
    }

    fn playlist(id: &str, title: &str) -> MusicItem {
        MusicItem { kind: Kind::Playlist, id: id.into(), title: title.into(), ..Default::default() }
    }

    fn labels(nodes: &[Node]) -> Vec<&str> {
        nodes
            .iter()
            .map(|node| match node {
                Node::Item { label, .. } | Node::Submenu(label, _) => label.as_str(),
                Node::Separator => "-",
            })
            .collect()
    }

    fn find<'a>(nodes: &'a [Node], wanted: &str) -> Option<&'a Node> {
        nodes.iter().find(|node| matches!(node, Node::Item { label, .. } if label == wanted))
    }

    #[test]
    fn the_menu_follows_the_track() {
        let playing = menu_model(Some(&now("One", "Band", true)), &[]);
        assert_eq!(labels(&playing), ["One", "-", "Pause", "Like", "Next", "Previous", "-", "Start Radio", "-", "Copy URL", "Open YouTube Music", "-", "Open procast"]);
        assert_eq!(playing[0], Node::Item { label: "One".into(), id: "ytm|title".into(), enabled: false });
        assert_eq!(find(&playing, "Pause"), Some(&item("Pause", "ytm|playpause")));
        let paused = menu_model(Some(&now("One", "Band", false)), &[]);
        assert_eq!(labels(&paused)[2], "Play");
    }

    #[test]
    fn nothing_playing_has_a_disabled_header() {
        let idle = menu_model(None, &[]);
        assert_eq!(idle[0], Node::Item { label: "Nothing playing".into(), id: "ytm|title".into(), enabled: false });
        assert_eq!(labels(&idle)[2], "Play");
        assert!(matches!(idle.last(), Some(Node::Item { id, .. }) if id == "ytm|launcher"));
    }

    #[test]
    fn playlists_make_a_submenu_when_there_are_any() {
        let library = [playlist("VLPL1", "Road Trip"), playlist("VLLM", "Liked"), playlist("VLPL2", "Focus")];
        let nodes = menu_model(None, &library);
        let Some(Node::Submenu(label, adds)) = nodes.iter().find(|node| matches!(node, Node::Submenu(..))) else { panic!("no submenu") };
        assert_eq!(label, "Add to Playlist");
        assert_eq!(adds, &[item("Road Trip", "ytm|add|PL1"), item("Focus", "ytm|add|PL2")]);
        assert_eq!(labels(&nodes)[8], "Add to Playlist");
        let only_liked = menu_model(None, &[playlist("VLLM", "Liked")]);
        assert!(!only_liked.iter().any(|node| matches!(node, Node::Submenu(..))));
    }

    #[test]
    fn the_tooltip_names_the_track() {
        assert_eq!(tooltip(Some(&now("One", "Band", true))), "One — Band");
        assert_eq!(tooltip(Some(&now("One", "", true))), "One");
        assert_eq!(tooltip(None), "YouTube Music");
        assert_eq!(tooltip(Some(&now(&"x".repeat(300), "", true))).chars().count(), TOOLTIP_CHARS);
    }

    #[test]
    fn ids_become_actions() {
        assert_eq!(Action::parse("ytm|add|PLabc"), Some(Action::Add("PLabc".into())));
        assert_eq!(Action::parse("ytm|playpause"), Some(Action::PlayPause));
        assert_eq!(Action::parse("ytm|radio"), Some(Action::Radio));
        assert_eq!(Action::parse("ytm|add|"), None);
        assert_eq!(Action::parse("ytm|title"), None);
        assert_eq!(Action::parse("ytm|launcher"), None);
        assert_eq!(Action::parse("ext|1|f"), None);
    }

    #[test]
    fn clicks_become_messages() {
        assert!(matches!(message("ytm|launcher"), Some(Msg::Tray(TrayCmd::Open))));
        assert!(matches!(message("ytm|add|PL1"), Some(Msg::TrayMusic(id)) if id == "ytm|add|PL1"));
        assert!(message("ytm|title").is_none() && message("ext|1|f").is_none());
    }
}
