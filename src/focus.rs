//! Focus sessions: for a set time, listed apps are closed the moment they take focus (or, in
//! allow mode, every app but the listed ones), and listed websites are redirected by the
//! browser companion. The session lives in `%APPDATA%\procast\focus.json`, so a restart keeps it.
//! Everything here is plain data and decisions; main.rs acts on them.

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{GetWindowThreadProcessId, PostMessageW, WM_CLOSE},
};

use crate::config::FocusCategory;

/// Never closed or redirected, whatever the lists say: procast itself and the Windows shell.
/// ApplicationFrameHost draws every Store app, so it can't tell them apart.
const NEVER: &[&str] = &[
    "procast.exe",
    "explorer.exe",
    "shellexperiencehost.exe",
    "searchhost.exe",
    "startmenuexperiencehost.exe",
    "lockapp.exe",
    "applicationframehost.exe",
    "textinputhost.exe",
    "shellhost.exe",
    "credentialuibroker.exe",
];

/// While a session runs, the browser is given the site filter again this often: it forgets it
/// when its extension restarts.
const RESEND: Duration = Duration::from_secs(15);

/// A blocked app that comes back within this long is left alone: it is most likely the "save
/// changes?" box of the window just closed.
const SAVE_PROMPT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The listed apps and sites are off limits.
    Block,
    /// Only the listed apps and sites are allowed.
    Allow,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub title: String,
    pub mode: Mode,
    /// Lower-case program file names.
    pub apps: Vec<String>,
    /// Lower-case domains.
    pub sites: Vec<String>,
    /// Length in minutes (Restart starts over with it); 0 = no limit.
    pub minutes: u32,
    /// Wall-clock end while a timed session runs; None while paused (and with no limit).
    pub until: Option<SystemTime>,
    /// Paused: what was left (timed) or how long it had run (no limit).
    pub paused_left: Option<Duration>,
    /// Wall-clock start while a no-limit session runs (moved forward by pauses); None otherwise.
    #[serde(default)]
    pub since: Option<SystemTime>,
    /// Apps allowed until the time, by file name.
    #[serde(default)]
    pub snoozed: HashMap<String, SystemTime>,
    /// Allow mode with no sites listed lets every site through (`[focus] allow_all_on_empty_sites`).
    #[serde(default)]
    pub allow_empty_sites: bool,
    /// Full paths of the apps Focus closed, to reopen at the end (`[focus] restore_apps`).
    #[serde(default)]
    pub closed: Vec<String>,
}

fn after(now: SystemTime, d: Duration) -> SystemTime {
    now.checked_add(d).unwrap_or(now)
}

impl Session {
    pub fn new(title: &str, mode: Mode, apps: &[String], sites: &[String], minutes: u32, allow_empty_sites: bool, now: SystemTime) -> Self {
        let clean = |l: &[String], f: fn(&str) -> String| l.iter().map(|s| f(s)).filter(|s| !s.is_empty()).collect();
        Self {
            title: title.trim().to_string(),
            mode,
            apps: clean(apps, norm_app),
            sites: clean(sites, norm_site),
            minutes,
            until: (minutes > 0).then(|| after(now, Duration::from_secs(minutes as u64 * 60))),
            paused_left: None,
            since: (minutes == 0).then_some(now),
            snoozed: HashMap::new(),
            allow_empty_sites,
            closed: vec![],
        }
    }

    pub fn unlimited(&self) -> bool {
        self.minutes == 0
    }

    /// Counting (not paused). A timed session may be over; see `expired`.
    pub fn running(&self) -> bool {
        self.until.is_some() || self.since.is_some()
    }

    /// Running: not paused and not over.
    pub fn active(&self, now: SystemTime) -> bool {
        self.since.is_some() || self.until.is_some_and(|u| now < u)
    }

    pub fn expired(&self, now: SystemTime) -> bool {
        self.until.is_some_and(|u| now >= u)
    }

    /// Time left in a timed session (zero with no limit).
    pub fn left(&self, now: SystemTime) -> Duration {
        if self.unlimited() {
            return Duration::ZERO;
        }
        self.paused_left.or_else(|| self.until.map(|u| u.duration_since(now).unwrap_or_default())).unwrap_or_default()
    }

    /// How long a no-limit session has run (zero for a timed one).
    pub fn elapsed(&self, now: SystemTime) -> Duration {
        if !self.unlimited() {
            return Duration::ZERO;
        }
        self.paused_left.or_else(|| self.since.map(|s| now.duration_since(s).unwrap_or_default())).unwrap_or_default()
    }

    pub fn pause(&mut self, now: SystemTime) {
        if let Some(u) = self.until.take() {
            self.paused_left = Some(u.duration_since(now).unwrap_or_default());
        } else if let Some(s) = self.since.take() {
            self.paused_left = Some(now.duration_since(s).unwrap_or_default());
        }
    }

    pub fn resume(&mut self, now: SystemTime) {
        if let Some(d) = self.paused_left.take() {
            if self.unlimited() {
                self.since = Some(now.checked_sub(d).unwrap_or(now));
            } else {
                self.until = Some(after(now, d));
            }
        }
    }

    /// Starts over with the full length.
    pub fn restart(&mut self, now: SystemTime) {
        if self.unlimited() {
            self.since = Some(now);
        } else {
            self.until = Some(after(now, Duration::from_secs(self.minutes as u64 * 60)));
        }
        self.paused_left = None;
        self.snoozed.clear();
    }

    /// What Toggle starts again.
    pub fn preset(&self) -> Preset {
        Preset { title: self.title.clone(), mode: self.mode, apps: self.apps.clone(), sites: self.sites.clone(), minutes: self.minutes }
    }

    /// Lets `key` (an app's file name) through for `mins` minutes. The overlay's choices are 1, 3, 5, 10.
    pub fn snooze(&mut self, key: &str, mins: u8) {
        self.snoozed.insert(key.to_lowercase(), after(SystemTime::now(), Duration::from_secs(mins as u64 * 60)));
    }

    /// Whether the program `exe` (file name) has to close now. An empty name (a protected
    /// process Windows won't name) is unknown, never blocked. Allow mode with no apps listed
    /// blocks nothing.
    pub fn blocked_app(&self, exe: &str, now: SystemTime) -> bool {
        let exe = exe.to_lowercase();
        if !self.active(now) || exe.is_empty() || NEVER.contains(&exe.as_str()) || self.snoozed.get(&exe).is_some_and(|&t| now < t) {
            return false;
        }
        let listed = self.apps.contains(&exe);
        match self.mode {
            Mode::Block => listed,
            Mode::Allow => !self.apps.is_empty() && !listed,
        }
    }

    /// The website filter the browser applies right now, if any.
    fn site_rule(&self, now: SystemTime) -> Option<(Mode, &[String])> {
        let nothing = self.sites.is_empty() && (self.mode == Mode::Block || self.allow_empty_sites);
        (self.active(now) && !nothing).then_some((self.mode, &self.sites[..]))
    }

    /// Whether `host` is off limits: a listed domain (or a subdomain of it) in block mode,
    /// anything else in allow mode. Allow mode with no sites blocks every site unless
    /// `allow_empty_sites`.
    pub fn blocked_site(&self, host: &str, now: SystemTime) -> bool {
        let Some((mode, sites)) = self.site_rule(now) else { return false };
        let host = host.to_lowercase();
        (mode == Mode::Block) == sites.iter().any(|s| host == *s || host.ends_with(&format!(".{s}")))
    }

    /// The root row: title and time left (or, with no limit, time run).
    pub fn row(&self, now: SystemTime) -> (String, String) {
        let time = if self.unlimited() { format!("{} elapsed", crate::timers::fmt_duration(self.elapsed(now))) } else { format!("{} left", crate::timers::fmt_duration(self.left(now))) };
        (self.title.clone(), if self.paused_left.is_some() { format!("Paused · {time}") } else { time })
    }
}

/// A session's settings, kept for the next Toggle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub title: String,
    pub mode: Mode,
    pub apps: Vec<String>,
    pub sites: Vec<String>,
    pub minutes: u32,
}

impl Preset {
    pub fn start(self) -> Start {
        Start { title: self.title, minutes: self.minutes, filter: Some((self.mode, self.apps, self.sites)) }
    }
}

/// The minutes Snooze allows: 0 in `[focus] snooze_minutes` turns it off.
pub fn snooze_for(configured: u8) -> Option<u8> {
    (configured > 0).then_some(configured)
}

/// Whether the browser is due the site filter again: a session counts down and the last push
/// (None: never) is `RESEND` old.
pub fn resend_due(running: bool, last: Option<Instant>, now: Instant) -> bool {
    running && last.is_none_or(|l| now.duration_since(l) >= RESEND)
}

/// A file's text, where a missing file is just empty. Any other failure (a UTF-16 file, a
/// sharing violation) is an error, so nothing is ever written over text that could not be read.
pub fn text_or_empty(read: std::io::Result<String>) -> Result<String, String> {
    match read {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("Could not read config.toml: {e}")),
    }
}

/// Writes to `<file>.tmp` first, then renames over the file, so a crash never leaves half a file.
pub fn write_atomic(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, path))
}

/// The message that gives the browser companion the site filter (or switches it off).
pub fn sites_message(session: Option<&Session>, now: SystemTime) -> Value {
    match session.and_then(|s| s.site_rule(now)) {
        Some((mode, sites)) => json!({"t": "focusSites", "mode": if mode == Mode::Block { "block" } else { "allow" }, "sites": sites}),
        None => json!({"t": "focusSites", "mode": "off", "sites": []}),
    }
}

/// "Discord.EXE" -> "discord.exe"; a path keeps its file name; "slack" and a Mac "Slack.app"
/// both become "slack.exe".
pub fn norm_app(s: &str) -> String {
    let name = s.trim().rsplit(['\\', '/']).next().unwrap_or_default().to_lowercase();
    let name = name.strip_suffix(".app").unwrap_or(&name);
    if name.is_empty() || name.ends_with(".exe") { name.to_string() } else { format!("{name}.exe") }
}

/// "https://www.Reddit.com/r/rust?x" -> "reddit.com".
pub fn norm_site(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let s = s.rsplit("://").next().unwrap_or_default();
    let host = s.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or_default().split(':').next().unwrap_or_default();
    host.trim_matches('.').strip_prefix("www.").unwrap_or(host.trim_matches('.')).to_string()
}

/// The domain of an http(s) link; None for anything else.
pub fn host_of(url: &str) -> Option<String> {
    let u = url.trim().to_lowercase();
    (u.starts_with("http://") || u.starts_with("https://")).then(|| norm_site(&u)).filter(|h| !h.is_empty())
}

/// "Notepad" for "notepad.exe".
pub fn app_label(exe: &str) -> String {
    let stem = exe.strip_suffix(".exe").unwrap_or(exe);
    let mut c = stem.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Whole minutes left, rounded up, at least 1 (for "blocked for 25 min").
pub fn minutes_left(d: Duration) -> u64 {
    d.as_secs().div_ceil(60).max(1)
}

/// The system "ding" that ends a session.
pub fn beep() {
    crate::timers::beep();
}

/// Asks a window to close (the program may ask to save first) and returns the full path of
/// its program, empty if unknown.
pub fn close_window(hwnd: isize) -> String {
    let hwnd = HWND(hwnd as *mut _);
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let path = crate::apps::exe_path(pid).unwrap_or_default();
    unsafe { _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    path
}

// ---- the running state ---------------------------------------------------------------

/// What `focus.json` holds.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    session: Option<Session>,
    preset: Option<Preset>,
}

/// The session and where it is saved.
#[derive(Default)]
pub struct Focus {
    pub session: Option<Session>,
    /// The settings of the latest session started, for Toggle.
    pub preset: Option<Preset>,
    path: Option<PathBuf>,
    /// When the browser last got the site filter.
    pushed: Option<Instant>,
    /// The last app Focus closed: file name, full path, when.
    last: Option<(String, String, Instant)>,
}

impl Focus {
    pub fn load(path: PathBuf) -> Self {
        let saved: Saved = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { session: saved.session, preset: saved.preset, path: Some(path), ..Default::default() }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if self.session.is_none() && self.preset.is_none() {
            return drop(fs::remove_file(path));
        }
        let saved = Saved { session: self.session.clone(), preset: self.preset.clone() };
        let _ = write_atomic(path, &serde_json::to_string(&saved).unwrap_or_default());
    }

    /// Starts, replaces or (None) ends the session; a new one is also kept as the preset.
    pub fn set(&mut self, session: Option<Session>) {
        if let Some(s) = &session {
            self.preset = Some(s.preset());
        }
        self.session = session;
        self.save();
    }

    /// Changes the session (if any) and saves.
    pub fn update(&mut self, f: impl FnOnce(&mut Session)) {
        if let Some(s) = &mut self.session {
            f(s);
            self.save();
        }
    }

    /// The tick is needed while a session counts (a paused one does not).
    pub fn ticking(&self) -> bool {
        self.session.as_ref().is_some_and(Session::running)
    }

    pub fn push_due(&self, now: Instant) -> bool {
        resend_due(self.ticking(), self.pushed, now)
    }

    /// Notes that the browser just got the site filter.
    pub fn pushed(&mut self, now: Instant) {
        self.pushed = Some(now);
    }

    /// False for an app just closed (see `SAVE_PROMPT`).
    pub fn fresh(&self, exe: &str, now: Instant) -> bool {
        self.last.as_ref().is_none_or(|(e, _, at)| e != exe || now.duration_since(*at) > SAVE_PROMPT)
    }

    /// Notes an app Focus closed; with `restore` its path is kept for the end of the session.
    pub fn closed_app(&mut self, exe: &str, path: String, restore: bool, now: Instant) {
        if restore && !path.is_empty() {
            self.update(|s| {
                if !s.closed.contains(&path) {
                    s.closed.push(path.clone());
                }
            });
        }
        self.last = Some((exe.to_string(), path, now));
    }

    /// The last app Focus closed: file name and path.
    pub fn last_closed(&self) -> Option<(String, String)> {
        self.last.as_ref().map(|(e, p, _)| (e.clone(), p.clone()))
    }

    /// The domain of `url` if the session keeps it out now.
    pub fn blocked_url(&self, url: &str, now: SystemTime) -> Option<String> {
        let host = host_of(url)?;
        self.session.as_ref().is_some_and(|s| s.blocked_site(&host, now)).then_some(host)
    }
}

// ---- what the launcher asks ----------------------------------------------------------

/// A session as asked for. `filter` None: the third answer was empty.
#[derive(Debug, PartialEq)]
pub struct Start {
    pub title: String,
    pub minutes: u32,
    pub filter: Option<(Mode, Vec<String>, Vec<String>)>,
}

pub fn category<'a>(cats: &'a [FocusCategory], name: &str) -> Option<&'a FocusCategory> {
    cats.iter().find(|c| c.name.eq_ignore_ascii_case(name.trim()))
}

/// The answers to Start or Edit: title, minutes, and "<category> [allow|block]".
pub fn parse_start(got: &[String], cats: &[FocusCategory], default_minutes: u32) -> Result<Start, String> {
    let answer = |i: usize| got.get(i).map_or("", |s| s.trim());
    let title = if answer(0).is_empty() { "Focus".to_string() } else { answer(0).to_string() };
    let minutes = match answer(1) {
        "" => default_minutes,
        t => match t.parse::<u32>() {
            Ok(n) => n,
            Err(_) => crate::timers::parse_duration(t).ok_or("Minutes: a number like 25 (0 for no limit), or 1h30m")?.as_secs().div_ceil(60) as u32,
        },
    };
    let mut words: Vec<&str> = answer(2).split_whitespace().collect();
    let mode = match words.last().map(|w| w.to_lowercase()).as_deref() {
        Some("allow") => Some(Mode::Allow),
        Some("block") => Some(Mode::Block),
        _ => None,
    };
    if mode.is_some() {
        words.pop();
    }
    let name = words.join(" ");
    let filter = match (name.is_empty(), mode) {
        (true, None) => None,
        (true, Some(m)) => Some((m, vec![], vec![])),
        (false, m) => {
            let c = category(cats, &name).ok_or_else(|| format!("No category named {name}. Create it first"))?;
            Some((m.unwrap_or(Mode::Block), c.apps.clone(), c.sites.clone()))
        }
    };
    Ok(Start { title, minutes, filter })
}

/// "a, b ;c" -> ["a", "b", "c"].
fn list(s: &str) -> Vec<String> {
    s.split([',', ';']).map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// The answers to Create Category: name, apps, sites.
pub fn parse_category(got: &[String]) -> Result<FocusCategory, String> {
    let answer = |i: usize| got.get(i).map_or("", |s| s.trim());
    if answer(0).is_empty() {
        return Err("Name is required".into());
    }
    let (apps, sites) = (list(answer(1)).iter().map(|a| norm_app(a)).collect(), list(answer(2)).iter().map(|s| norm_site(s)).filter(|s| !s.is_empty()).collect());
    Ok(FocusCategory { name: answer(0).to_string(), apps, sites })
}

/// Raycast's export: an array of `{name, apps, websites}`. An app is a name or an object with
/// `exe`, `path` or `name`; the name of a Mac app becomes the same-named .exe, which only
/// matches when the Windows program is called the same.
pub fn import(json: &str) -> Result<Vec<FocusCategory>, String> {
    let v: Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| format!("Not a focus categories file: {e}"))?;
    let all = v.as_array().ok_or("Not a focus categories file: expected a list")?;
    let strs = |v: &Value, f: fn(&str) -> String| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e.as_str().or_else(|| ["exe", "path", "name"].iter().find_map(|k| e[k].as_str())))
            .map(f)
            .filter(|s| !s.is_empty())
            .collect()
    };
    let cats: Vec<FocusCategory> = all
        .iter()
        .filter_map(|c| Some(FocusCategory { name: c["name"].as_str()?.trim().to_string(), apps: strs(&c["apps"], norm_app), sites: strs(&c["websites"], norm_site) }))
        .filter(|c| !c.name.is_empty())
        .collect();
    if cats.is_empty() { Err("No categories in that file".into()) } else { Ok(cats) }
}

/// Appends `[[focus_category]]` tables to config.toml's text, leaving comments alone. Categories
/// whose name is already there are skipped. Returns the new text and how many were added.
pub fn add_categories(text: &str, cats: &[FocusCategory]) -> Result<(String, usize), String> {
    let mut doc = text.trim_start_matches('\u{feff}').parse::<toml_edit::DocumentMut>().map_err(|e| format!("config.toml: {e}"))?;
    if !doc.contains_key("focus_category") {
        doc.insert("focus_category", toml_edit::Item::ArrayOfTables(Default::default()));
    }
    let tables = doc["focus_category"].as_array_of_tables_mut().ok_or("focus_category is not a list of [[focus_category]] tables in config.toml")?;
    let mut added = 0;
    for c in cats {
        if tables.iter().any(|t| t.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.eq_ignore_ascii_case(&c.name))) {
            continue;
        }
        let mut t = toml_edit::Table::new();
        t.insert("name", toml_edit::value(c.name.as_str()));
        t.insert("apps", toml_edit::value(toml_edit::Array::from_iter(c.apps.iter().map(String::as_str))));
        t.insert("sites", toml_edit::value(toml_edit::Array::from_iter(c.sites.iter().map(String::as_str))));
        tables.push(t);
        added += 1;
    }
    Ok((doc.to_string(), added))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(mode: Mode, apps: &[&str], sites: &[&str], now: SystemTime) -> Session {
        let own = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        Session::new("Write", mode, &own(apps), &own(sites), 25, false, now)
    }

    fn mins(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }

    #[test]
    fn block_mode_blocks_listed_apps_only_while_running() {
        let now = SystemTime::now();
        let mut s = session(Mode::Block, &["Notepad.exe"], &[], now);
        assert!(s.blocked_app("notepad.exe", now));
        assert!(s.blocked_app("NOTEPAD.EXE", now), "case does not matter");
        assert!(!s.blocked_app("code.exe", now), "unlisted apps are fine");
        assert!(!s.blocked_app("notepad.exe", now + mins(26)), "not after the end");
        s.pause(now + mins(5));
        assert!(!s.blocked_app("notepad.exe", now + mins(6)), "not while paused");
        s.resume(now + mins(10));
        assert!(s.blocked_app("notepad.exe", now + mins(11)));
        assert_eq!(s.left(now + mins(10)), mins(20), "a pause stops the clock");
    }

    #[test]
    fn allow_mode_blocks_unlisted_apps_but_never_the_shell() {
        let now = SystemTime::now();
        let s = session(Mode::Allow, &["code.exe"], &[], now);
        assert!(s.blocked_app("discord.exe", now));
        assert!(!s.blocked_app("code.exe", now));
        for shell in [
            "procast.exe",
            "explorer.exe",
            "ShellExperienceHost.exe",
            "searchhost.exe",
            "startmenuexperiencehost.exe",
            "lockapp.exe",
            "applicationframehost.exe",
            "textinputhost.exe",
            "shellhost.exe",
            "credentialuibroker.exe",
        ] {
            assert!(!s.blocked_app(shell, now), "{shell}");
        }
        assert!(!s.blocked_app("", now), "an unnamed process is unknown, not blocked");
        let all = session(Mode::Block, &["explorer.exe", "procast.exe"], &[], now);
        assert!(!all.blocked_app("explorer.exe", now) && !all.blocked_app("procast.exe", now), "even when listed");
        let empty = session(Mode::Allow, &[], &[], now);
        assert!(!empty.blocked_app("discord.exe", now), "an empty allowlist blocks no app");
    }

    #[test]
    fn snooze_lets_an_app_through_for_a_while() {
        let now = SystemTime::now();
        let mut s = session(Mode::Block, &["notepad.exe"], &[], now);
        s.snooze("Notepad.exe", 3);
        assert!(!s.blocked_app("notepad.exe", now + Duration::from_secs(60)));
        assert!(s.blocked_app("notepad.exe", now + mins(4)), "blocked again afterwards");
        s.snooze("notepad.exe", 3);
        s.restart(now);
        assert!(s.blocked_app("notepad.exe", now), "restart clears snoozes");
    }

    #[test]
    fn sites_match_by_domain_suffix() {
        let now = SystemTime::now();
        let s = session(Mode::Block, &[], &["https://www.reddit.com/r/rust"], now);
        assert_eq!(s.sites, ["reddit.com"]);
        assert!(s.blocked_site("www.reddit.com", now));
        assert!(s.blocked_site("reddit.com", now));
        assert!(s.blocked_site("Old.Reddit.com", now));
        assert!(!s.blocked_site("notreddit.com", now));
        assert!(!s.blocked_site("reddit.com.evil.org", now));
        assert!(!s.blocked_site("reddit.com", now + mins(30)), "not after the end");
        let allow = session(Mode::Allow, &[], &["docs.rs"], now);
        assert!(!allow.blocked_site("docs.rs", now) && !allow.blocked_site("a.docs.rs", now));
        assert!(allow.blocked_site("reddit.com", now));
    }

    #[test]
    fn allow_mode_with_no_sites_blocks_every_site_unless_told_otherwise() {
        let now = SystemTime::now();
        let strict = session(Mode::Allow, &["code.exe"], &[], now);
        assert!(strict.blocked_site("example.com", now));
        assert_eq!(sites_message(Some(&strict), now)["mode"], "allow");
        let mut open = strict.clone();
        open.allow_empty_sites = true;
        assert!(!open.blocked_site("example.com", now));
        assert_eq!(sites_message(Some(&open), now)["mode"], "off");
        let none = session(Mode::Block, &["a.exe"], &[], now);
        assert!(!none.blocked_site("example.com", now), "block mode with nothing listed blocks nothing");
    }

    #[test]
    fn the_browser_message_follows_the_session() {
        let now = SystemTime::now();
        let mut s = session(Mode::Block, &[], &["x.com", "reddit.com"], now);
        assert_eq!(sites_message(Some(&s), now), json!({"t": "focusSites", "mode": "block", "sites": ["x.com", "reddit.com"]}));
        s.pause(now);
        assert_eq!(sites_message(Some(&s), now)["mode"], "off", "paused: sites are free");
        assert_eq!(sites_message(None, now)["mode"], "off");
    }

    #[test]
    fn links_are_judged_by_their_domain() {
        let now = SystemTime::now();
        let f = Focus { session: Some(session(Mode::Block, &[], &["reddit.com"], now)), ..Default::default() };
        assert_eq!(f.blocked_url("https://www.reddit.com/r/rust?x=1", now), Some("reddit.com".into()));
        assert_eq!(f.blocked_url("https://user@old.reddit.com:443/", now), Some("old.reddit.com".into()));
        assert_eq!(f.blocked_url("https://example.com/reddit.com", now), None);
        assert_eq!(f.blocked_url("mailto:a@reddit.com", now), None);
        assert_eq!(f.blocked_url("C:\\reddit.com", now), None);
    }

    #[test]
    fn names_are_normalised() {
        assert_eq!(norm_app(r"C:\Games\Foo.EXE"), "foo.exe");
        assert_eq!(norm_app("Slack"), "slack.exe");
        assert_eq!(norm_app("/Applications/Slack.app"), "slack.exe");
        assert_eq!(norm_app("  "), "");
        assert_eq!(norm_site("HTTPS://WWW.X.com/home"), "x.com");
        assert_eq!(norm_site("x.com:8080"), "x.com");
        assert_eq!(norm_site("ünï.example"), "ünï.example", "no byte slicing");
        assert_eq!(app_label("notepad.exe"), "Notepad");
        assert_eq!(app_label("éa.exe"), "Éa");
        assert_eq!((minutes_left(Duration::from_secs(1)), minutes_left(mins(25)), minutes_left(Duration::from_secs(1501))), (1, 25, 26));
    }

    #[test]
    fn the_row_shows_time_left_or_paused() {
        let now = SystemTime::now();
        let mut s = session(Mode::Block, &[], &[], now);
        assert_eq!(s.row(now), ("Write".into(), "25:00 left".into()));
        s.pause(now + mins(5));
        assert_eq!(s.row(now + mins(9)).1, "Paused · 20:00 left");
    }

    #[test]
    fn start_answers_parse() {
        let cats = vec![FocusCategory { name: "Deep Work".into(), apps: vec!["discord.exe".into()], sites: vec!["x.com".into()] }];
        let ask = |a: [&str; 3]| parse_start(&a.map(String::from), &cats, 25);
        assert_eq!(ask(["", "", ""]), Ok(Start { title: "Focus".into(), minutes: 25, filter: None }));
        let s = ask(["Essay", "50", "deep work"]).unwrap();
        assert_eq!((s.title.as_str(), s.minutes), ("Essay", 50));
        assert_eq!(s.filter, Some((Mode::Block, vec!["discord.exe".into()], vec!["x.com".into()])));
        assert_eq!(ask(["", "1h", "Deep Work ALLOW"]).unwrap().minutes, 60);
        assert!(matches!(ask(["", "", "Deep Work allow"]).unwrap().filter, Some((Mode::Allow, ..))));
        assert_eq!(ask(["", "", "allow"]).unwrap().filter, Some((Mode::Allow, vec![], vec![])));
        assert!(ask(["", "", "nope"]).unwrap_err().contains("No category named nope"));
        assert_eq!(ask(["", "0", ""]).unwrap().minutes, 0, "0 is no limit");
        assert!(ask(["", "soon", ""]).is_err());
    }

    #[test]
    fn a_category_is_typed_or_imported() {
        let typed = parse_category(&["Social".into(), "Discord, slack.exe".into(), "https://www.reddit.com, x.com".into()]).unwrap();
        assert_eq!((typed.apps, typed.sites), (vec!["discord.exe".to_string(), "slack.exe".into()], vec!["reddit.com".to_string(), "x.com".into()]));
        assert!(parse_category(&[" ".into()]).is_err());
        let sample = r#"[
            {"name": "Social", "apps": [{"name": "Slack", "path": "/Applications/Slack.app"}, "Discord"], "websites": ["facebook.com", "https://www.x.com/home"]},
            {"name": "News", "apps": [], "websites": ["bbc.co.uk"]},
            {"apps": ["nameless"]}
        ]"#;
        let cats = import(sample).unwrap();
        assert_eq!(cats.len(), 2, "a category without a name is skipped");
        assert_eq!((cats[0].name.as_str(), cats[0].apps.clone(), cats[0].sites.clone()), ("Social", vec!["slack.exe".to_string(), "discord.exe".into()], vec!["facebook.com".to_string(), "x.com".into()]));
        assert!(import("{}").is_err() && import("nonsense").is_err() && import("[]").is_err());
    }

    #[test]
    fn categories_are_added_to_config_text_once() {
        let cats = import(r#"[{"name": "Social", "apps": ["discord"], "websites": ["x.com"]}]"#).unwrap();
        let (text, n) = add_categories("# mine\n[focus]\nsound = true\n", &cats).unwrap();
        assert_eq!(n, 1);
        assert!(text.starts_with("# mine"));
        let c = crate::config::parse(&text).unwrap();
        assert_eq!(c.focus_category, cats);
        let (again, n) = add_categories(&text, &cats).unwrap();
        assert_eq!((n, again), (0, text));
    }

    #[test]
    fn a_session_survives_a_restart_and_the_closed_apps_are_kept() {
        let path = std::env::temp_dir().join(format!("procast-focus-test-{}.json", std::process::id()));
        let now = SystemTime::now();
        let mut f = Focus::load(path.clone());
        assert!(f.session.is_none());
        f.set(Some(session(Mode::Block, &["notepad.exe"], &[], now)));
        f.closed_app("notepad.exe", r"C:\Windows\notepad.exe".into(), true, Instant::now());
        f.closed_app("notepad.exe", r"C:\Windows\notepad.exe".into(), true, Instant::now());
        let back = Focus::load(path.clone());
        assert_eq!(back.session, f.session);
        assert_eq!(back.session.as_ref().unwrap().closed, [r"C:\Windows\notepad.exe"], "once");
        assert!(!PathBuf::from(format!("{}.tmp", path.display())).exists());
        f.set(None);
        let back = Focus::load(path.clone());
        assert!(back.session.is_none() && back.preset.is_some(), "the settings stay for Toggle");
        let mut fresh = Focus { path: Some(path.clone()), ..Default::default() };
        fresh.set(None);
        assert!(!path.exists(), "nothing to keep, no file");
    }

    #[test]
    fn a_paused_session_survives_a_restart_still_paused() {
        let path = std::env::temp_dir().join(format!("procast-focus-paused-test-{}.json", std::process::id()));
        let now = SystemTime::now();
        for minutes in [25, 0] {
            let mut f = Focus::load(path.clone());
            let mut s = Session::new("Write", Mode::Block, &["notepad.exe".into()], &[], minutes, false, now);
            s.pause(now + mins(5));
            f.set(Some(s));
            let back = Focus::load(path.clone());
            let s = back.session.as_ref().unwrap();
            assert_eq!(Some(s), f.session.as_ref());
            assert!(!back.ticking() && s.paused_left == Some(if minutes == 0 { mins(5) } else { mins(20) }));
            assert!(!s.blocked_app("notepad.exe", now + mins(60)), "paused stays paused across the restart");
        }
        let _ = fs::remove_file(path);
    }

    #[test]
    fn toggle_starts_again_with_the_last_settings() {
        let path = std::env::temp_dir().join(format!("procast-focus-preset-test-{}.json", std::process::id()));
        let now = SystemTime::now();
        let mut f = Focus::load(path.clone());
        assert!(f.preset.is_none());
        f.set(Some(Session::new("Essay", Mode::Allow, &["Code.exe".into()], &["docs.rs".into()], 50, false, now)));
        f.set(None);
        let start = Focus::load(path.clone()).preset.unwrap().start();
        assert_eq!(start, Start { title: "Essay".into(), minutes: 50, filter: Some((Mode::Allow, vec!["code.exe".into()], vec!["docs.rs".into()])) });
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_session_with_no_limit_never_ends_but_can_pause() {
        let now = SystemTime::now();
        let mut s = session(Mode::Block, &["notepad.exe"], &[], now);
        s = Session::new("Open", s.mode, &s.apps, &s.sites, 0, false, now);
        assert!(s.unlimited() && s.running() && s.until.is_none());
        let later = now + mins(60 * 24);
        assert!(s.active(later) && !s.expired(later) && s.blocked_app("notepad.exe", later));
        assert_eq!(s.row(now + mins(3)), ("Open".into(), "3:00 elapsed".into()));
        s.pause(now + mins(5));
        assert!(!s.running() && !s.blocked_app("notepad.exe", later), "paused: not blocking");
        assert_eq!(s.row(later).1, "Paused · 5:00 elapsed", "a pause stops the clock");
        s.resume(now + mins(10));
        assert_eq!(s.elapsed(now + mins(12)), mins(7));
        assert!(s.blocked_app("notepad.exe", now + mins(12)));
        s.restart(now + mins(20));
        assert_eq!(s.elapsed(now + mins(21)), mins(1));
        assert_eq!(sites_message(Some(&s), now + mins(21))["mode"], "off", "nothing listed, nothing to filter");
    }

    #[test]
    fn snooze_can_be_turned_off() {
        assert_eq!((snooze_for(0), snooze_for(1), snooze_for(3), snooze_for(10)), (None, Some(1), Some(3), Some(10)));
    }

    #[test]
    fn the_site_filter_is_repeated_only_while_counting_down_and_every_15_seconds() {
        let t = Instant::now();
        let s15 = Duration::from_secs(15);
        assert!(!resend_due(false, None, t), "paused or no session: never");
        assert!(!resend_due(false, Some(t), t + s15 * 4));
        assert!(resend_due(true, None, t), "never pushed: now");
        assert!(!resend_due(true, Some(t), t + Duration::from_secs(14)));
        assert!(resend_due(true, Some(t), t + s15));
        assert!(resend_due(true, Some(t), t + Duration::from_secs(31)), "a late tick still catches up");
        let mut f = Focus { session: Some(session(Mode::Block, &[], &["x.com"], SystemTime::now())), ..Default::default() };
        assert!(f.push_due(t));
        f.pushed(t);
        assert!(!f.push_due(t + Duration::from_secs(1)) && f.push_due(t + s15));
        f.update(|s| s.pause(SystemTime::now()));
        assert!(!f.push_due(t + s15 * 9), "a paused session is not ticking");
    }

    #[test]
    fn only_a_missing_config_counts_as_empty() {
        use std::io::{Error, ErrorKind};
        assert_eq!(text_or_empty(Ok("a = 1".into())), Ok("a = 1".into()));
        assert_eq!(text_or_empty(Err(Error::from(ErrorKind::NotFound))), Ok(String::new()));
        for kind in [ErrorKind::InvalidData, ErrorKind::PermissionDenied, ErrorKind::Other] {
            assert!(text_or_empty(Err(Error::from(kind))).is_err(), "{kind:?}");
        }
        let path = std::env::temp_dir().join(format!("procast-atomic-test-{}.toml", std::process::id()));
        write_atomic(&path, "one").unwrap();
        write_atomic(&path, "two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert!(!PathBuf::from(format!("{}.tmp", path.display())).exists());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn an_app_that_comes_straight_back_is_left_alone() {
        let mut f = Focus::default();
        let t = Instant::now();
        assert!(f.fresh("notepad.exe", t));
        f.closed_app("notepad.exe", String::new(), false, t);
        assert!(!f.fresh("notepad.exe", t + Duration::from_secs(1)));
        assert!(f.fresh("code.exe", t + Duration::from_secs(1)));
        assert!(f.fresh("notepad.exe", t + Duration::from_secs(4)));
        assert_eq!(f.last_closed().map(|l| l.0), Some("notepad.exe".into()));
    }
}
