//! Snippet text: placeholders, `{argument}`s, and keywords typed in any app. The keyboard
//! hook (keys.rs) turns keystrokes into characters and feeds them to an `Expander`; a match
//! goes to the launcher, which fills the snippet in and `insert`s it.

use std::time::Duration;

use windows::Win32::{
    Foundation::HWND,
    System::{DataExchange::GetClipboardSequenceNumber, Diagnostics::Debug::MessageBeep, Threading::GetCurrentProcessId},
    UI::{
        Input::KeyboardAndMouse::{VK_BACK, VK_LEFT},
        WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, MESSAGEBOX_STYLE},
    },
};

use crate::clipboard;

/// One `{argument}` a snippet asks for before it goes in.
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub name: String,
    pub default: String,
}

/// `{word key="value" …}` -> (word, attributes); None for text that isn't a placeholder.
fn placeholder(inner: &str) -> Option<(&str, Vec<(&str, String)>)> {
    let inner = inner.trim();
    let (word, mut rest) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
    if !matches!(word, "clipboard" | "selection" | "date" | "time" | "cursor" | "argument") {
        return None;
    }
    let mut attrs = vec![];
    loop {
        rest = rest.trim_start();
        let Some((key, after)) = rest.split_once("=\"") else { break };
        let (value, after) = after.split_once('"')?;
        attrs.push((key.trim(), value.to_string()));
        rest = after;
    }
    Some((word, attrs))
}

enum Part<'a> {
    Text(&'a str),
    /// A placeholder: its word and attributes.
    Ph(&'a str, Vec<(&'a str, String)>),
}

/// The template as text and placeholders, in order.
fn walk(template: &str, mut f: impl FnMut(Part)) {
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|c| open + c) else { break };
        match placeholder(&rest[open + 1..close]) {
            Some((word, attrs)) => {
                f(Part::Text(&rest[..open]));
                f(Part::Ph(word, attrs));
                rest = &rest[close + 1..];
            }
            None => {
                f(Part::Text(&rest[..=open]));
                rest = &rest[open + 1..];
            }
        }
    }
    f(Part::Text(rest));
}

fn attr<'a>(attrs: &'a [(&str, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
}

/// The arguments a snippet asks for, in order; a name used twice is asked once. Unnamed ones
/// are "Argument 1", "Argument 2", ….
pub fn arguments(template: &str) -> Vec<Arg> {
    let mut out: Vec<Arg> = vec![];
    let mut unnamed = 0;
    walk(template, |p| {
        let Part::Ph("argument", attrs) = p else { return };
        let name = attr(&attrs, "name").map(String::from).unwrap_or_else(|| {
            unnamed += 1;
            format!("Argument {unnamed}")
        });
        if !out.iter().any(|a| a.name == name) {
            out.push(Arg { name, default: attr(&attrs, "default").unwrap_or_default().into() });
        }
    });
    out
}

/// What placeholders are filled with.
#[derive(Default)]
pub struct Fill {
    pub clipboard: String,
    pub selection: String,
    pub date: String,
    pub time: String,
    /// Values for `arguments()`, in that order; an empty one takes its default.
    pub args: Vec<String>,
}

/// The snippet's text, and how many characters follow `{cursor}` (Left presses after it goes in).
pub fn expand(template: &str, fill: &Fill) -> (String, usize) {
    let args = arguments(template);
    let mut out = String::new();
    let mut cursor = None;
    let mut unnamed = 0;
    walk(template, |p| match p {
        Part::Text(t) => out.push_str(t),
        Part::Ph("clipboard", _) => out.push_str(&fill.clipboard),
        Part::Ph("selection", _) => out.push_str(&fill.selection),
        Part::Ph("date", _) => out.push_str(&fill.date),
        Part::Ph("time", _) => out.push_str(&fill.time),
        Part::Ph("cursor", _) => cursor = cursor.or(Some(out.chars().filter(|&c| c != '\r').count())),
        Part::Ph(_, attrs) => {
            let name = attr(&attrs, "name").map(String::from).unwrap_or_else(|| {
                unnamed += 1;
                format!("Argument {unnamed}")
            });
            if let Some(i) = args.iter().position(|a| a.name == name) {
                let v = fill.args.get(i).filter(|v| !v.is_empty()).unwrap_or(&args[i].default);
                out.push_str(v);
            }
        }
    });
    let back = cursor.map_or(0, |at| out.chars().filter(|&c| c != '\r').count() - at);
    (out, back)
}

pub fn needs_selection(template: &str) -> bool {
    let mut yes = false;
    walk(template, |p| yes |= matches!(p, Part::Ph("selection", _)));
    yes
}

// ---- typed keywords ----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Trigger {
    Immediately,
    /// After a space or punctuation; `true` keeps that character after the snippet.
    Delimiter(bool),
}

impl Trigger {
    pub fn of(s: &str) -> Self {
        match s {
            "delimiter" => Trigger::Delimiter(true),
            "delimiter_discard" => Trigger::Delimiter(false),
            _ => Trigger::Immediately,
        }
    }
}

/// A keyword just typed.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    /// Snippet `index` (into `[[snippet]]`); `erase` characters typed go first, `then` comes after it.
    Snippet { index: usize, erase: usize, then: String },
    /// ":name:" for an emoji.
    Emoji { emoji: &'static str, erase: usize },
}

/// Watches typed characters for snippet keywords and emoji shortcodes.
pub struct Expander {
    /// (snippet index, keyword).
    keywords: Vec<(usize, String)>,
    trigger: Trigger,
    within_words: bool,
    shortcodes: bool,
    typed: String,
}

impl Expander {
    pub fn new(keywords: Vec<(usize, String)>, trigger: Trigger, within_words: bool, shortcodes: bool) -> Self {
        Self { keywords: keywords.into_iter().filter(|(_, k)| !k.is_empty()).collect(), trigger, within_words, shortcodes, typed: String::new() }
    }

    pub fn is_idle(&self) -> bool {
        self.keywords.is_empty() && !self.shortcodes
    }

    /// The caret may have moved (Enter, arrows, another window): start over.
    pub fn reset(&mut self) {
        self.typed.clear();
    }

    pub fn backspace(&mut self) {
        self.typed.pop();
    }

    /// The longest keyword `text` ends with, on a word start unless `within_words`.
    fn keyword_at_end(&self, text: &str) -> Option<(usize, usize)> {
        self.keywords
            .iter()
            .filter(|(_, k)| text.ends_with(k.as_str()))
            .filter(|(_, k)| self.within_words || text[..text.len() - k.len()].chars().next_back().is_none_or(|c| !c.is_alphanumeric()))
            .max_by_key(|(_, k)| k.len())
            .map(|(i, k)| (*i, k.chars().count()))
    }

    pub fn typed(&mut self, c: char) -> Option<Hit> {
        let before = self.typed.clone();
        self.typed.push(c);
        if self.typed.chars().count() > 64 {
            self.typed.remove(0);
        }
        let hit = self.hit(&before, c);
        if hit.is_some() {
            self.typed.clear();
        }
        hit
    }

    fn hit(&self, before: &str, c: char) -> Option<Hit> {
        if self.shortcodes
            && c == ':'
            && let Some(start) = before.rfind(':')
        {
            let name = &before[start + 1..];
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
                && let Some(e) = emojis::get_by_shortcode(name)
            {
                return Some(Hit::Emoji { emoji: e.as_str(), erase: name.len() + 2 });
            }
        }
        match self.trigger {
            Trigger::Immediately => self.keyword_at_end(&self.typed).map(|(index, n)| Hit::Snippet { index, erase: n, then: String::new() }),
            Trigger::Delimiter(keep) if c.is_whitespace() || c.is_ascii_punctuation() => self
                .keyword_at_end(before)
                .map(|(index, n)| Hit::Snippet { index, erase: n + 1, then: if keep { c.to_string() } else { String::new() } }),
            Trigger::Delimiter(_) => None,
        }
    }
}

/// Whether a procast window has focus (typing in its own search bar never expands).
pub fn procast_focused() -> bool {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)) };
    pid == unsafe { GetCurrentProcessId() }
}

/// `[snippets] speed` as a pause.
pub fn pause(speed: &str) -> Duration {
    Duration::from_millis(match speed {
        "instant" => 0,
        "quick" => 25,
        "delayed" => 150,
        "extended" => 400,
        _ => 60,
    })
}

/// `[snippets] sound` as a MessageBeep type.
pub fn sound(name: &str) -> Option<u32> {
    Some(match name {
        "default" => 0,
        "asterisk" => 0x40,
        "exclamation" => 0x30,
        "question" => 0x20,
        _ => return None,
    })
}

/// A filled-in snippet on its way into the focused app.
pub struct Job {
    /// A procast window, to own the clipboard while pasting.
    pub owner: HWND,
    pub template: String,
    pub args: Vec<String>,
    /// Characters of the typed keyword to delete first.
    pub erase: usize,
    /// Typed after the snippet (a kept delimiter).
    pub then: String,
    pub pause: Duration,
    pub sound: Option<u32>,
}

/// Off the UI thread: reads the selection and clipboard if asked for, deletes the keyword,
/// pastes the text, puts the caret on `{cursor}`, then gives the clipboard back.
pub fn insert(job: Job) {
    let owner = job.owner.0 as isize;
    std::thread::spawn(move || {
        let owner = HWND(owner as _);
        // Focus has just gone back to the app (launcher), or the keyword's last key is still on its way.
        std::thread::sleep(job.pause.max(Duration::from_millis(40)));
        let old = clipboard::text();
        let mut fill = Fill { clipboard: old.clone().unwrap_or_default(), args: job.args, ..Default::default() };
        (fill.date, fill.time) = crate::win::local_date_time();
        if needs_selection(&job.template) {
            let seq = unsafe { GetClipboardSequenceNumber() };
            copy();
            for _ in 0..20 {
                std::thread::sleep(Duration::from_millis(20));
                if unsafe { GetClipboardSequenceNumber() } != seq {
                    fill.selection = clipboard::text().unwrap_or_default();
                    break;
                }
            }
        }
        let (mut text, back) = expand(&job.template, &fill);
        text.push_str(&job.then);
        let back = back + job.then.chars().count();
        clipboard::tap(VK_BACK, job.erase);
        if !clipboard::set(owner, clipboard::Put::Text(&text), true) {
            return;
        }
        clipboard::send_paste();
        std::thread::sleep(job.pause.max(Duration::from_millis(30)));
        clipboard::tap(VK_LEFT, back);
        if let Some(s) = job.sound {
            unsafe { _ = MessageBeep(MESSAGEBOX_STYLE(s)) };
        }
        // The app reads the paste a moment later; then the clipboard gets its text back.
        // ponytail: only text comes back; an image or files on the clipboard before are replaced.
        std::thread::sleep(Duration::from_millis(300));
        if let Some(old) = old {
            clipboard::set(owner, clipboard::Put::Text(&old), true);
        }
    });
}

/// Ctrl+C into the focused app (for `{selection}`).
fn copy() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, SendInput, VK_C, VK_CONTROL};
    let key = clipboard::key;
    let seq = [key(VK_CONTROL, false), key(VK_C, false), key(VK_C, true), key(VK_CONTROL, true)];
    unsafe { SendInput(&seq, size_of::<INPUT>() as i32) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_arguments_and_cursor() {
        let fill = Fill { clipboard: "Ann".into(), date: "2026-11-03".into(), time: "10:00".into(), ..Default::default() };
        assert_eq!(expand("Cheers, {clipboard} {date} {time}", &fill).0, "Cheers, Ann 2026-11-03 10:00");
        assert_eq!(expand("a {b} {unknown thing} }{", &fill).0, "a {b} {unknown thing} }{", "other braces stay");
        assert_eq!(expand("<b>{cursor}</b>", &fill), ("<b></b>".into(), 4));
        assert_eq!(expand("x{cursor}\r\ny", &fill), ("x\r\ny".into(), 2), "a line break is one Left");

        let t = r#"Hi {argument name="To" default="there"}, {argument} and {argument}. Bye {argument name="To"}"#;
        assert_eq!(arguments(t).iter().map(|a| (a.name.as_str(), a.default.as_str())).collect::<Vec<_>>(), [("To", "there"), ("Argument 1", ""), ("Argument 2", "")]);
        let fill = Fill { args: vec!["".into(), "A".into(), "B".into()], ..Default::default() };
        assert_eq!(expand(t, &fill).0, "Hi there, A and B. Bye there", "empty takes the default; a name repeats its value");
        assert!(needs_selection("Re: {selection}") && !needs_selection("{clipboard}"));
    }

    #[test]
    fn keywords_typed_anywhere() {
        let type_all = |x: &mut Expander, s: &str| s.chars().filter_map(|c| x.typed(c)).collect::<Vec<_>>();
        let kws = || vec![(0, ";sig".to_string()), (1, ";d".to_string()), (2, ";date".to_string())];

        let mut x = Expander::new(kws(), Trigger::Immediately, false, true);
        assert_eq!(type_all(&mut x, "hello ;sig"), [Hit::Snippet { index: 0, erase: 4, then: String::new() }]);
        assert_eq!(type_all(&mut x, "my;sig"), [], "not inside a word");
        x.reset();
        assert_eq!(type_all(&mut x, ";si"), []);
        x.backspace();
        x.backspace();
        assert_eq!(type_all(&mut x, "d"), [Hit::Snippet { index: 1, erase: 2, then: String::new() }], "Backspace is followed");
        assert_eq!(type_all(&mut x, "nice :thumbsup:"), [Hit::Emoji { emoji: "👍", erase: 10 }]);
        assert_eq!(type_all(&mut x, "at 10:30:"), [], "not a shortcode");

        let mut x = Expander::new(kws(), Trigger::Delimiter(true), false, false);
        assert_eq!(type_all(&mut x, ";date"), [], "waits for the delimiter");
        assert_eq!(type_all(&mut x, " "), [Hit::Snippet { index: 2, erase: 6, then: " ".into() }], "longest keyword, delimiter kept");
        let mut x = Expander::new(kws(), Trigger::Delimiter(false), true, false);
        assert_eq!(type_all(&mut x, "my;sig."), [Hit::Snippet { index: 0, erase: 5, then: String::new() }], "within words when asked");
        assert!(Expander::new(vec![(0, String::new())], Trigger::Immediately, false, false).is_idle());
    }
}
