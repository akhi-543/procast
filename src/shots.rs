//! Screenshot search: the pictures (and, if asked, videos) in the screenshot folders, with the
//! text Windows' OCR finds in each, kept in %APPDATA%\procast\screenshots.json.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Cursor,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::HWND;

use crate::{
    calendar, clipboard, config,
    design::{Tint, g},
    search::{Act, Icon, Item},
};

const IMAGES: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif"];
const VIDEOS: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "wmv", "m4v"];
/// Bigger pictures are listed but not read for text.
pub const MAX_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Shot {
    pub path: PathBuf,
    /// Modified time, seconds since the epoch.
    pub mtime: u64,
    pub text: String,
    /// Reading its text failed; not tried again until the file changes.
    pub failed: bool,
    /// Listed under Pinned and never pruned. Set from the pins file (`set_pins`), not stored
    /// here: an index rebuilt with OCR off starts empty.
    #[serde(skip)]
    pub pinned: bool,
    /// The file name and the text in lower case, made once (`lower`) rather than per keystroke.
    #[serde(skip)]
    lname: String,
    #[serde(skip)]
    ltext: String,
}

impl Shot {
    fn new(path: PathBuf, mtime: u64, text: String, failed: bool, pinned: bool) -> Self {
        let mut s = Shot { path, mtime, text, failed, pinned, lname: String::new(), ltext: String::new() };
        s.lower();
        s
    }

    fn lower(&mut self) {
        self.lname = self.name().to_lowercase();
        self.ltext = self.text.to_lowercase();
    }

    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn is_video(&self) -> bool {
        video(&self.path)
    }
}

/// Newest first.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ShotIndex {
    entries: Vec<Shot>,
    /// The pinned paths (`pins`); a file that is indexed again keeps its pin.
    #[serde(skip)]
    pins: HashSet<PathBuf>,
}

/// How an update scans: videos too, how old a file may be, and how many files to read per call.
#[derive(Clone, Copy)]
pub struct Opts {
    pub all_media: bool,
    pub keep: Option<Duration>,
    pub batch: usize,
    /// Read the pictures for their text. Off: they are listed with no text and never read.
    pub ocr: bool,
    /// List OneDrive files that are only in the cloud (reading them downloads them).
    pub cloud: bool,
}

fn ext(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_lowercase())
}

fn video(path: &Path) -> bool {
    ext(path).is_some_and(|e| VIDEOS.contains(&e.as_str()))
}

/// A picture file, by its extension (what a thumbnail may be decoded from).
pub fn picture(path: &Path) -> bool {
    ext(path).is_some_and(|e| IMAGES.contains(&e.as_str()))
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Whether a file with these attributes is listed: one OneDrive keeps only in the cloud
/// only with `cloud`, since reading it downloads it.
fn listed(attrs: u32, cloud: bool) -> bool {
    cloud || !crate::files::is_cloud_only(attrs)
}

/// The pictures (and videos) directly inside `folders` with their modified times. Folders
/// that are missing are skipped, and cloud-only files too unless `cloud`.
// ponytail: top level only; add recursion if people sort screenshots into subfolders.
fn scan(folders: &[PathBuf], all_media: bool, cloud: bool) -> HashMap<PathBuf, u64> {
    use std::os::windows::fs::MetadataExt;
    let mut found = HashMap::new();
    for entry in folders.iter().filter_map(|f| fs::read_dir(f).ok()).flatten().flatten() {
        let path = entry.path();
        let wanted = picture(&path) || (all_media && video(&path));
        // From the directory listing: the file itself is not opened.
        let Ok(meta) = entry.metadata() else { continue };
        // The index is stored as JSON: a path that isn't Unicode can't be.
        if wanted && meta.is_file() && listed(meta.file_attributes(), cloud) && path.to_str().is_some() {
            found.insert(path, meta.modified().map(secs).unwrap_or(0));
        }
    }
    found
}

fn cutoff(keep: Duration) -> u64 {
    secs(SystemTime::now()).saturating_sub(keep.as_secs())
}

impl ShotIndex {
    fn file() -> PathBuf {
        config::dir().join("screenshots.json")
    }

    /// The saved index; empty when there is none or it doesn't parse.
    pub fn load() -> Self {
        fs::read(Self::file()).ok().and_then(|b| Self::parse(&b)).unwrap_or_default()
    }

    /// Pins `pins` (`pins()`), and unpins the rest.
    pub fn set_pins(&mut self, pins: HashSet<PathBuf>) {
        for e in &mut self.entries {
            e.pinned = pins.contains(&e.path);
        }
        self.pins = pins;
    }

    /// Forgets a file (deleted).
    pub fn remove(&mut self, path: &Path) {
        self.entries.retain(|e| e.path != path);
    }

    /// The text read in the file at `path`; empty when none was.
    pub fn text(&self, path: &Path) -> &str {
        self.entries.iter().find(|e| e.path == path).map_or("", |e| e.text.as_str())
    }

    fn parse(json: &[u8]) -> Option<Self> {
        let mut index: Self = serde_json::from_slice(json).ok()?;
        index.entries.iter_mut().for_each(Shot::lower);
        Some(index)
    }

    fn save(&self) {
        let path = Self::file();
        let tmp = path.with_extension("tmp");
        if let Ok(json) = serde_json::to_vec(self)
            && fs::write(&tmp, json).is_ok()
        {
            let _ = fs::rename(tmp, path);
        }
    }

    /// Brings the index in line with `folders`: drops what is gone or too old, then indexes up
    /// to `opts.batch` new or changed files, newest first. With `opts.ocr` each is read and
    /// its text recognised: a picture that can't be recognised is remembered as `failed` and
    /// not tried again until it changes, but one that can't be read right now (locked by
    /// OneDrive or an antivirus) is left out so the next scan retries it. Without `opts.ocr`
    /// nothing is read. A pin survives the file changing, and keeps a file older than
    /// `opts.keep`. Returns how many were indexed.
    /// (A scan plans once and indexes batch by batch: `scan_loop`.)
    #[cfg(test)]
    fn update(&mut self, folders: &[PathBuf], opts: Opts, ocr: impl Fn(&[u8]) -> Option<String>) -> usize {
        let mut todo = self.plan(folders, opts);
        self.index(&mut todo, opts, ocr)
    }

    /// Drops what is gone or too old; returns the new or changed files, oldest first (the
    /// newest is popped first).
    fn plan(&mut self, folders: &[PathBuf], opts: Opts) -> Vec<(PathBuf, u64)> {
        self.prune(opts.keep);
        let mut found = scan(folders, opts.all_media, opts.cloud);
        if let Some(keep) = opts.keep {
            let at = cutoff(keep);
            found.retain(|p, m| *m >= at || self.pins.contains(p));
        }
        self.entries.retain(|e| found.contains_key(&e.path));
        let known: HashMap<&PathBuf, u64> = self.entries.iter().map(|e| (&e.path, e.mtime)).collect();
        let mut todo: Vec<(PathBuf, u64)> = found.into_iter().filter(|(p, m)| known.get(p) != Some(m)).collect();
        todo.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)));
        todo
    }

    /// Indexes files from the end of `todo` (a `plan`) until `opts.batch` are indexed or none
    /// are left; a file that can't be read right now is dropped from `todo`. Returns how many
    /// were indexed.
    fn index(&mut self, todo: &mut Vec<(PathBuf, u64)>, opts: Opts, ocr: impl Fn(&[u8]) -> Option<String>) -> usize {
        let mut fresh = Vec::new();
        while fresh.len() < opts.batch
            && let Some((path, mtime)) = todo.pop()
        {
            let pinned = self.pins.contains(&path);
            let (text, failed) = if video(&path) || !opts.ocr {
                (String::new(), false)
            } else {
                let Ok(meta) = fs::metadata(&path) else { continue };
                if meta.len() > MAX_BYTES {
                    (String::new(), true)
                } else {
                    let Ok(bytes) = fs::read(&path) else { continue };
                    ocr(&bytes).map_or((String::new(), true), |t| (t, false))
                }
            };
            fresh.push(Shot::new(path, mtime, text, failed, pinned));
        }
        let redo: HashSet<&PathBuf> = fresh.iter().map(|s| &s.path).collect();
        self.entries.retain(|e| !redo.contains(&e.path));
        let indexed = fresh.len();
        self.entries.extend(fresh);
        self.entries.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.path.cmp(&b.path)));
        indexed
    }

    /// Entries of `kind` matching the filter `q` (`parse_filter`), newest first; all of
    /// them when `q` is empty.
    pub fn search(&self, q: &str, kind: Kind, now: SystemTime) -> Vec<&Shot> {
        let f = parse_filter(q);
        self.entries.iter().filter(|s| kind.has(s) && matches(s, &f, now)).collect()
    }

    /// Forgets entries older than `keep`, except pinned ones.
    pub fn prune(&mut self, keep: Option<Duration>) {
        if let Some(keep) = keep {
            let at = cutoff(keep);
            self.entries.retain(|e| e.pinned || e.mtime >= at);
        }
    }
}

/// `[screenshots] keep` as a duration; None keeps everything.
pub fn keep(setting: &str) -> Option<Duration> {
    clipboard::keep_secs(setting).filter(|s| *s != u64::MAX).map(Duration::from_secs)
}

/// The folders to search: `[screenshots] folders`, or Pictures\Screenshots.
pub fn folders(setting: &[String]) -> Vec<PathBuf> {
    if setting.is_empty() { vec![pictures().join("Screenshots")] } else { setting.iter().map(PathBuf::from).collect() }
}

fn pictures() -> PathBuf {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Pictures, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    };
    let known = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None).ok().and_then(|p| {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as _));
            s
        })
    };
    known.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Pictures"))
}

/// The newest picture in the folders (cloud-only ones too: pasting it is asked for).
pub fn latest(folders: &[PathBuf]) -> Option<PathBuf> {
    scan(folders, false, true).into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0))).map(|(p, _)| p)
}

/// A picture file as PNG bytes, for the clipboard (which gets the PNG and a bitmap made from it).
pub fn png_of(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|e| format!("Could not read the screenshot: {e}"))?;
    if bytes.starts_with(b"\x89PNG") {
        return Ok(bytes);
    }
    let img = image::load_from_memory(&bytes).map_err(|_| "Could not read that picture".to_string())?;
    let mut png = Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(png.into_inner())
}

/// Puts a screenshot (the newest one when `path` is None) on the clipboard as an image;
/// returns which.
pub fn put_image(owner: HWND, path: Option<PathBuf>, folders: &[PathBuf]) -> Result<PathBuf, String> {
    let path = path.or_else(|| latest(folders)).ok_or("No screenshots found")?;
    let png = png_of(&path)?;
    if clipboard::set(owner, clipboard::Put::Image(&png), false) { Ok(path) } else { Err("Could not put it on the clipboard".into()) }
}

/// Paste Latest Screenshot's toast: "Pasted Screenshot 14:59", the time it was taken; just
/// "Pasted Screenshot" when that could not be read.
pub fn paste_toast(mtime: Option<SystemTime>) -> String {
    match mtime {
        Some(t) => format!("Pasted Screenshot {}", calendar::clock(calendar::utc_to_local(secs(t) as i64))),
        None => "Pasted Screenshot".into(),
    }
}

/// Quick Look's line under the name: "2.4 MB · Today, 14:59 · Screenshots" (size, `caption`,
/// folder name). None when the file is gone.
pub fn detail(path: &Path, now: SystemTime) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let folder = path.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some(format!("{} · {} · {folder}", crate::files::size_text(meta.len()), caption(meta.modified().ok()?, now)))
}

/// Moves a file to the Recycle Bin (procast asked first). One the Recycle Bin can't take (on
/// a network or removable drive, or too big) is deleted for good only once Windows has asked
/// and been told Yes. Run on a worker thread: it may wait on that question.
pub fn delete(path: &Path) -> Result<(), String> {
    use windows::{
        Win32::{
            System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
            UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW, SHFileOperationW},
        },
        core::PCWSTR,
    };
    // Undo (the Recycle Bin) only works on a full path.
    let full = std::path::absolute(path).map_err(|e| format!("Could not delete it: {e}"))?;
    // The Recycle Bin is the shell's, which wants COM on this thread.
    unsafe { _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    // A list of paths, each ending in a nul, the list in a second one.
    let from: Vec<u16> = full.as_os_str().encode_wide().chain([0, 0]).collect();
    // No "are you sure" (procast asked), except before a delete that can't be undone.
    let flags = FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOF_NOERRORUI | FOF_SILENT;
    let mut op = SHFILEOPSTRUCTW { wFunc: FO_DELETE, pFrom: PCWSTR(from.as_ptr()), fFlags: flags.0 as u16, ..Default::default() };
    let failed = unsafe { SHFileOperationW(&mut op) };
    match (failed, op.fAnyOperationsAborted.as_bool()) {
        (0, false) => Ok(()),
        (0, true) => Err("Not deleted".into()),
        _ => Err("Could not delete it".into()),
    }
}

fn pins_file() -> PathBuf {
    config::dir().join("screenshot-pins.txt")
}

/// The pinned screenshots' paths, one per line in %APPDATA%\procast\screenshot-pins.txt, kept
/// apart from the index so pins outlive a rebuild (and the scan thread never writes them).
pub fn pins() -> HashSet<PathBuf> {
    read_pins(&pins_file())
}

fn read_pins(file: &Path) -> HashSet<PathBuf> {
    parse_pins(&fs::read_to_string(file).unwrap_or_default())
}

fn parse_pins(text: &str) -> HashSet<PathBuf> {
    text.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect()
}

/// Pins (`on`) or unpins `path`; returns every pin.
// ponytail: a pin outlives its file being removed outside procast; prune the list if it grows.
pub fn pin(path: &Path, on: bool) -> Result<HashSet<PathBuf>, String> {
    pin_in(&pins_file(), path, on)
}

fn pin_in(file: &Path, path: &Path, on: bool) -> Result<HashSet<PathBuf>, String> {
    let text = crate::focus::text_or_empty(fs::read_to_string(file)).map_err(|_| "Could not read the pinned screenshots".to_string())?;
    let mut pins = parse_pins(&text);
    if on {
        pins.insert(path.to_path_buf());
    } else {
        pins.remove(path);
    }
    let mut lines: Vec<String> = pins.iter().map(|p| p.display().to_string() + "\n").collect();
    lines.sort();
    crate::focus::write_atomic(file, &lines.concat()).map_err(|e| format!("Could not save the pin: {e}"))?;
    Ok(pins)
}

/// Brings the saved index up to date as `[screenshots]` says, reading new screenshots' text
/// a batch at a time; `show` gets the index after each step, with how many pictures have
/// been read of how many there are to read (None when nothing is read). `pins` are kept
/// whatever their age. Run on a worker thread. Without OCR (switched off, or Windows has no
/// OCR language installed) nothing is read or saved and no picture is marked failed: the
/// folders are just listed.
pub fn rescan(s: &config::Screenshots, pins: HashSet<PathBuf>, show: impl FnMut(&ShotIndex, Option<(usize, usize)>)) {
    // Text recognition is a WinRT API.
    unsafe { _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
    let ocr = s.ocr && clipboard::ocr_ready();
    let mut index = if ocr { ShotIndex::load() } else { ShotIndex::default() };
    index.set_pins(pins);
    // Listing costs a stat per file, so its batches are bigger than reading ones.
    let opts = Opts { all_media: s.all_media, keep: keep(&s.keep), batch: if ocr { 20 } else { 500 }, ocr, cloud: s.cloud };
    scan_loop(&mut index, &folders(&s.folders), opts, clipboard::recognise, show, ShotIndex::save);
}

/// Scans `folders` once, then indexes what changed batch by batch, showing the index before
/// and after each batch with how many pictures have been read of how many (videos are never
/// read) and, when reading text, saving it whenever it changed. Files added meanwhile wait
/// for the next scan.
fn scan_loop(index: &mut ShotIndex, folders: &[PathBuf], opts: Opts, ocr: impl Fn(&[u8]) -> Option<String>, mut show: impl FnMut(&ShotIndex, Option<(usize, usize)>), mut save: impl FnMut(&ShotIndex)) {
    let before = index.entries.len();
    let mut todo = index.plan(folders, opts);
    let pictures = |todo: &[(PathBuf, u64)]| todo.iter().filter(|(p, _)| !video(p)).count();
    let of = pictures(&todo);
    let progress = |todo: &[(PathBuf, u64)]| opts.ocr.then(|| (of - pictures(todo), of));
    show(index, progress(&todo));
    let mut gone = index.entries.len() != before;
    loop {
        let indexed = index.index(&mut todo, opts, &ocr);
        if opts.ocr && (indexed > 0 || gone) {
            save(index);
        }
        gone = false;
        show(index, progress(&todo));
        if todo.is_empty() {
            break;
        }
    }
}

/// The grid's one section.
pub const SECTION: &str = "Screenshots & Recordings";

/// What the bar's dropdown shows: everything, pictures, videos (listed with Include all
/// media), or the pinned ones.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Kind {
    #[default]
    All,
    Screenshots,
    Recordings,
    Pinned,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::All, Kind::Screenshots, Kind::Recordings, Kind::Pinned];

    pub fn label(self) -> &'static str {
        match self {
            Kind::All => "All",
            Kind::Screenshots => "Screenshots",
            Kind::Recordings => "Recordings",
            Kind::Pinned => "Pinned",
        }
    }

    fn has(self, s: &Shot) -> bool {
        match self {
            Kind::All => true,
            Kind::Screenshots => !s.is_video(),
            Kind::Recordings => s.is_video(),
            Kind::Pinned => s.pinned,
        }
    }
}

/// Where a filter word looks: anywhere (name or text), the name, the text, or the day taken.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field {
    Any,
    Name,
    Text,
    Date(DateQ),
}

/// `date:today`, `date:yesterday`, `date:2026-11-12`, `date:2026-11`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DateQ {
    Today,
    Yesterday,
    Day(i32, u32, u32),
    Month(i32, u32),
}

fn date_q(v: &str) -> Option<DateQ> {
    let n: Vec<&str> = v.split('-').collect();
    let month = |m: &str| m.parse().ok().filter(|m| (1..=12).contains(m));
    Some(match (v, n.as_slice()) {
        ("today", _) => DateQ::Today,
        ("yesterday", _) => DateQ::Yesterday,
        (_, [y, m]) => DateQ::Month(y.parse().ok()?, month(m)?),
        (_, [y, m, d]) => DateQ::Day(y.parse().ok()?, month(m)?, d.parse().ok().filter(|d| (1..=31).contains(d))?),
        _ => return None,
    })
}

/// The bar's filter, word by word in lower case: `name:`, `text:` and `date:` limit a word to
/// one field, plain words search name and text. A field with nothing after it yet is left
/// out; a date that doesn't parse is a plain word (and so matches nothing).
pub fn parse_filter(q: &str) -> Vec<(Field, String)> {
    q.split_whitespace()
        .filter_map(|w| {
            let w = w.to_lowercase();
            let (field, v) = match w.split_once(':') {
                Some(("name", v)) => (Field::Name, v),
                Some(("text", v)) => (Field::Text, v),
                Some(("date", "")) => return None,
                Some(("date", v)) => date_q(v).map_or((Field::Any, w.as_str()), |d| (Field::Date(d), v)),
                _ => (Field::Any, w.as_str()),
            };
            (!v.is_empty()).then(|| (field, v.to_string()))
        })
        .collect()
}

/// This PC's local day number (days since 1970-01-01) of a Unix time.
fn local_day(unix: u64) -> i64 {
    calendar::utc_to_local(unix as i64).div_euclid(calendar::DAY)
}

/// Whether `shot` passes every word of a parsed filter; `now` says which day is today.
pub fn matches(shot: &Shot, f: &[(Field, String)], now: SystemTime) -> bool {
    f.iter().all(|(field, w)| match field {
        Field::Any => shot.lname.contains(w.as_str()) || shot.ltext.contains(w.as_str()),
        Field::Name => shot.lname.contains(w.as_str()),
        Field::Text => shot.ltext.contains(w.as_str()),
        Field::Date(q) => {
            let day = local_day(shot.mtime);
            match *q {
                DateQ::Today => day == local_day(secs(now)),
                DateQ::Yesterday => day == local_day(secs(now)) - 1,
                DateQ::Day(y, m, d) => day == calendar::days_from_civil(y.into(), m.into(), d.into()),
                DateQ::Month(y, m) => {
                    let (yy, mm, _) = calendar::civil_from_days(day);
                    (yy, mm) == (y.into(), m.into())
                }
            }
        }
    })
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// A thumbnail's caption: "Today, 14:59", "Yesterday, 18:07", then "12 Nov, 09:50".
pub fn caption(mtime: SystemTime, now: SystemTime) -> String {
    let at = calendar::utc_to_local(secs(mtime) as i64);
    let day = at.div_euclid(calendar::DAY);
    let clock = calendar::clock(at);
    match local_day(secs(now)) - day {
        0 => format!("Today, {clock}"),
        1 => format!("Yesterday, {clock}"),
        _ => {
            let (_, m, d) = calendar::civil_from_days(day);
            format!("{d} {}, {clock}", MONTHS[(m - 1) as usize])
        }
    }
}

/// For a `text:` filter: the words of the picture's text that hold each of `words` (lower
/// case), as written there; None unless every one is found.
pub fn text_hit(shot: &Shot, words: &[String]) -> Option<String> {
    // The cached lower-case text lines up with the text byte for byte unless lowering changed
    // a character's length; only then is each word lowered on its own.
    let aligned = shot.ltext.len() == shot.text.len();
    let hit = |w: &String| match shot.ltext.find(w.as_str()) {
        Some(i) if aligned && shot.text.is_char_boundary(i) => Some(word_at(&shot.text, i)),
        None if aligned => None,
        _ => shot.text.split_whitespace().find(|t| t.to_lowercase().contains(w.as_str())),
    };
    let hits: Option<Vec<&str>> = words.iter().map(hit).collect();
    hits.filter(|h| !h.is_empty()).map(|h| h.join(" "))
}

/// The whitespace-separated word of `text` around byte `i`.
fn word_at(text: &str, i: usize) -> &str {
    let start = text[..i].char_indices().rev().find(|c| c.1.is_whitespace()).map_or(0, |(j, c)| j + c.len_utf8());
    let end = text[i..].find(char::is_whitespace).map_or(text.len(), |j| i + j);
    &text[start..end]
}

/// A grid cell's thumbnail key, `t:{mtime}|{path}` (the time, so a file saved over gets a
/// fresh one); None for a video, whose file is never read for one.
pub fn thumb_key(s: &Shot) -> Option<String> {
    (!s.is_video()).then(|| format!("t:{}|{}", s.mtime, s.path.display()))
}

/// The grid's cells, newest first: the file name as title, the caption (or, for a `text:`
/// filter, the words found) as accessory, and the primary action first (`primary`: copy |
/// paste | open), then Open With…, Quick Look, Show in File Explorer, Pin, Copy Text and
/// Delete; a video is not copied as a picture. With each, its `thumb_key`.
pub fn rows(index: &ShotIndex, query: &str, kind: Kind, primary: &str) -> (Vec<Item>, Vec<Option<String>>) {
    let now = SystemTime::now();
    let words: Vec<String> = parse_filter(query).into_iter().filter(|f| f.0 == Field::Text).map(|f| f.1).collect();
    index
        .search(query, kind, now)
        .into_iter()
        .map(|s| {
            let path = s.path.to_string_lossy().into_owned();
            let (copy, paste, open) = (Act::CopyShot(path.clone()), Act::PasteShot(path.clone()), Act::File(path.clone()));
            let mut acts = match (s.is_video(), primary) {
                (true, _) => vec![open],
                (_, "paste") => vec![paste, copy, open],
                (_, "open") => vec![open, copy, paste],
                _ => vec![copy, paste, open],
            };
            acts.extend([Act::OpenWith(path.clone()), Act::QuickLook(path.clone()), Act::Reveal(path.clone()), Act::PinShot(path.clone(), !s.pinned)]);
            if !s.is_video() {
                acts.push(Act::CopyText(path.clone()));
            }
            acts.push(Act::DeleteShot(path));
            let caption = text_hit(s, &words).unwrap_or_else(|| caption(UNIX_EPOCH + Duration::from_secs(s.mtime), now));
            let icon = Icon::Vector(if s.is_video() { g::VIDEO } else { g::PICTURE });
            let mut it = Item::new(s.name(), "", icon, acts[0].clone()).acc(caption).tinted(Tint::File).sec(SECTION);
            it.extra = acts.into_iter().map(|a| (a.label().to_string(), a)).collect();
            (it, thumb_key(s))
        })
        .unzip()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, fs, time::SystemTime};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("procast-shots-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(path: &Path, age_secs: u64) {
        fs::write(path, b"x").unwrap();
        let at = SystemTime::now() - Duration::from_secs(age_secs);
        fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
    }

    const OPTS: Opts = Opts { all_media: false, keep: None, batch: 100, ocr: true, cloud: false };

    #[test]
    fn new_files_are_read_once() {
        let d = dir("once");
        touch(&d.join("a.png"), 10);
        touch(&d.join("notes.txt"), 10);
        let (calls, mut idx) = (Cell::new(0), ShotIndex::default());
        let ocr = |_: &[u8]| {
            calls.set(calls.get() + 1);
            Some("Invoice 42".to_string())
        };
        assert_eq!(idx.update(std::slice::from_ref(&d), OPTS, ocr), 1, "the picture, not the text file");
        assert_eq!(idx.update(std::slice::from_ref(&d), OPTS, ocr), 0);
        assert_eq!(calls.get(), 1, "an unchanged file is not read again");
        touch(&d.join("a.png"), 5);
        assert_eq!(idx.update(std::slice::from_ref(&d), OPTS, ocr), 1, "a changed file is");
        assert_eq!(calls.get(), 2);
        assert_eq!(idx.entries.len(), 1);
        fs::remove_file(d.join("a.png")).unwrap();
        idx.update(std::slice::from_ref(&d), OPTS, ocr);
        assert!(idx.entries.is_empty(), "a deleted file goes");
    }

    #[test]
    fn failures_are_not_retried_until_the_file_changes() {
        let d = dir("failed");
        touch(&d.join("a.png"), 10);
        let (calls, mut idx) = (Cell::new(0), ShotIndex::default());
        let ocr = |_: &[u8]| {
            calls.set(calls.get() + 1);
            None
        };
        assert_eq!(idx.update(std::slice::from_ref(&d), OPTS, ocr), 1);
        assert!(idx.entries[0].failed && idx.entries[0].text.is_empty());
        assert_eq!(idx.update(std::slice::from_ref(&d), OPTS, ocr), 0);
        assert_eq!(calls.get(), 1);
        touch(&d.join("a.png"), 1);
        assert_eq!(idx.update(&[d], OPTS, ocr), 1);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn search_reads_text_and_names_newest_first() {
        let d = dir("search");
        touch(&d.join("old.png"), 300);
        touch(&d.join("Screenshot receipt.png"), 200);
        touch(&d.join("new.png"), 100);
        let mut idx = ShotIndex::default();
        idx.update(std::slice::from_ref(&d), OPTS, |_| Some("Invoice no. 7 \u{e9}t\u{e9}".into()));
        let names = |v: Vec<&Shot>| v.iter().map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
        let search = |q: &str| idx.search(q, Kind::All, SystemTime::now());
        assert_eq!(names(search("")), ["new.png", "Screenshot receipt.png", "old.png"]);
        assert_eq!(search("invoice").len(), 3, "OCR text is searched");
        assert_eq!(names(search("RECEIPT")), ["Screenshot receipt.png"], "so are names, in any case");
        assert_eq!(search("\u{e9}t\u{e9}").len(), 3, "non-ASCII text is fine");
        assert!(search("zebra").is_empty());
        assert!(search("name:invoice").is_empty() && search("text:receipt").is_empty(), "a field limits the word");
        assert_eq!(search("date:today").len(), 3);
    }

    /// A wall-clock time on this PC as the instant it is.
    fn at(y: i64, m: i64, d: i64, h: i64, min: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(calendar::local_to_utc(calendar::days_from_civil(y, m, d) * calendar::DAY + h * 3600 + min * 60) as u64)
    }

    fn shot(name: &str, when: SystemTime, text: &str) -> Shot {
        Shot::new(PathBuf::from(format!("C:\\Shots\\{name}")), secs(when), text.into(), false, false)
    }

    #[test]
    fn filters_parse_by_field() {
        use Field::*;
        assert_eq!(
            parse_filter("name:Inv text:total date:yesterday Foo"),
            [(Name, "inv".into()), (Text, "total".into()), (Date(DateQ::Yesterday), "yesterday".into()), (Any, "foo".into())]
        );
        assert_eq!(parse_filter("date:today date:2026-11 date:2026-11-12"), [(Date(DateQ::Today), "today".into()), (Date(DateQ::Month(2026, 11)), "2026-11".into()), (Date(DateQ::Day(2026, 11, 12)), "2026-11-12".into())]);
        assert!(parse_filter("name: text: date:  ").is_empty(), "a field still being typed filters nothing");
        assert_eq!(parse_filter("date:2026-13 date:soon"), [(Any, "date:2026-13".into()), (Any, "date:soon".into())], "a bad date is a plain word");
    }

    #[test]
    fn dates_match_the_local_day() {
        let now = at(2026, 11, 17, 15, 0);
        let (late, yesterday, early, october) = (shot("a.png", at(2026, 11, 17, 0, 5), ""), shot("b.png", at(2026, 11, 16, 23, 55), ""), shot("c.png", at(2026, 11, 1, 9, 0), ""), shot("d.png", at(2026, 10, 31, 23, 0), ""));
        let hits = |q: &str| [&late, &yesterday, &early, &october].into_iter().filter(|s| matches(s, &parse_filter(q), now)).map(|s| s.name()).collect::<Vec<_>>();
        assert_eq!(hits("date:yesterday"), ["b.png"]);
        assert_eq!(hits("date:today"), ["a.png"]);
        assert_eq!(hits("date:2026-11"), ["a.png", "b.png", "c.png"]);
        assert_eq!(hits("date:2026-11-01"), ["c.png"]);
        assert_eq!(hits("date:2026-10 name:d"), ["d.png"]);
    }

    #[test]
    fn captions_say_today_yesterday_or_the_date() {
        let now = at(2026, 11, 17, 15, 0);
        assert_eq!(caption(at(2026, 11, 17, 14, 59), now), "Today, 14:59");
        assert_eq!(caption(at(2026, 11, 16, 18, 7), now), "Yesterday, 18:07");
        assert_eq!(caption(at(2026, 11, 12, 9, 50), now), "12 Nov, 09:50");
    }

    #[test]
    fn a_text_hit_shows_the_words_as_written() {
        let s = shot("a.png", SystemTime::now(), "Invoice 42\nTotal: 42.00 due");
        assert_eq!(text_hit(&s, &["total".into()]).as_deref(), Some("Total:"));
        assert_eq!(text_hit(&s, &["inv".into(), "due".into()]).as_deref(), Some("Invoice due"));
        assert_eq!(text_hit(&s, &["total".into(), "zebra".into()]), None, "every word or none");
        assert_eq!(text_hit(&s, &[]), None);
        let mut idx = ShotIndex { entries: vec![s], ..Default::default() };
        assert_eq!(rows(&idx, "text:TOTAL", Kind::All, "copy").0[0].accessory, "Total:", "the caption of a text: match");
        idx.entries[0].mtime = secs(SystemTime::now());
        assert!(rows(&idx, "total", Kind::All, "copy").0[0].accessory.starts_with("Today, "), "a plain word keeps the time");
        let odd = shot("b.png", SystemTime::now(), "\u{130}stanbul TOTAL");
        assert_eq!(text_hit(&odd, &["total".into()]).as_deref(), Some("TOTAL"), "text whose lower case is longer still finds the word");
    }

    #[test]
    fn kinds_pick_pictures_videos_or_pins() {
        let now = SystemTime::now();
        let mut pinned = shot("p.png", now, "");
        pinned.pinned = true;
        let idx = ShotIndex { entries: vec![pinned, shot("a.png", now, ""), shot("v.mp4", now, "")], ..Default::default() };
        let names = |k: Kind| idx.search("", k, now).iter().map(|s| s.name()).collect::<Vec<_>>();
        assert_eq!(names(Kind::Pinned), ["p.png"]);
        assert_eq!(names(Kind::Screenshots), ["p.png", "a.png"]);
        assert_eq!(names(Kind::Recordings), ["v.mp4"]);
        assert_eq!(names(Kind::All).len(), 3);
    }

    #[test]
    fn an_index_from_before_pins_still_loads() {
        let old = br#"{"entries":[{"path":"C:\\Shots\\Invoice.png","mtime":5,"text":"Total Due","failed":false}]}"#;
        let idx = ShotIndex::parse(old).expect("parses");
        assert!(!idx.entries[0].pinned);
        assert_eq!(idx.search("invoice due", Kind::All, SystemTime::now()).len(), 1, "and its lower-case copies are made");
    }

    #[test]
    fn a_pin_survives_the_file_changing() {
        let d = dir("pin");
        touch(&d.join("a.png"), 10);
        let mut idx = ShotIndex::default();
        idx.update(std::slice::from_ref(&d), OPTS, |_| Some(String::new()));
        idx.set_pins(HashSet::from([d.join("a.png")]));
        assert!(idx.entries[0].pinned);
        touch(&d.join("a.png"), 1);
        assert_eq!(idx.update(std::slice::from_ref(&d), OPTS, |_| Some(String::new())), 1);
        assert!(idx.entries[0].pinned);
    }

    #[test]
    fn prune_and_keep_spare_pinned_shots() {
        let d = dir("pinkeep");
        touch(&d.join("old.png"), 7 * 86400);
        touch(&d.join("new.png"), 10);
        let day = Some(Duration::from_secs(86400));
        let mut idx = ShotIndex::default();
        idx.set_pins(HashSet::from([d.join("old.png")]));
        idx.update(std::slice::from_ref(&d), Opts { keep: day, ..OPTS }, |_| Some(String::new()));
        assert_eq!(idx.entries.len(), 2, "a pinned file older than keep is still listed");
        idx.prune(day);
        assert_eq!(idx.entries.len(), 2, "and prune keeps it");
        idx.set_pins(HashSet::new());
        idx.prune(day);
        assert_eq!(idx.entries.iter().map(Shot::name).collect::<Vec<_>>(), ["new.png"], "unpinned, it goes");
    }

    #[test]
    fn pins_survive_a_rebuild_with_ocr_off() {
        let d = dir("pinnocr");
        touch(&d.join("a.png"), 10);
        touch(&d.join("b.png"), 20);
        // OCR off starts from an empty index each scan: the pins come from their own file.
        let mut idx = ShotIndex::default();
        idx.set_pins(HashSet::from([d.join("b.png")]));
        scan_loop(&mut idx, std::slice::from_ref(&d), Opts { ocr: false, ..OPTS }, |_| None, |_, _| {}, |_| {});
        let pinned: Vec<String> = idx.search("", Kind::Pinned, SystemTime::now()).iter().map(|s| s.name()).collect();
        assert_eq!(pinned, ["b.png"]);
    }

    #[test]
    fn pins_are_saved_in_their_own_file() {
        let d = dir("pinfile");
        let file = d.join("pins.txt");
        let (a, b) = (PathBuf::from("C:\\Shots\\a.png"), PathBuf::from("C:\\Shots\\b b.png"));
        assert_eq!(pin_in(&file, &a, true).unwrap(), HashSet::from([a.clone()]));
        assert_eq!(pin_in(&file, &b, true).unwrap(), HashSet::from([a.clone(), b.clone()]));
        assert_eq!(read_pins(&file), HashSet::from([a.clone(), b.clone()]), "read back from the file");
        assert_eq!(pin_in(&file, &a, false).unwrap(), HashSet::from([b.clone()]));
        assert_eq!(read_pins(&d.join("none.txt")), HashSet::new(), "no file: no pins");
    }

    #[test]
    fn cloud_only_files_are_listed_only_when_asked() {
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS};
        let cloud = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0 | FILE_ATTRIBUTE_ARCHIVE.0;
        assert!(!listed(cloud, false), "reading it would download it");
        assert!(listed(cloud, true));
        assert!(listed(FILE_ATTRIBUTE_ARCHIVE.0, false), "a file on disk is always listed");
    }

    #[test]
    fn paste_latest_says_when_the_shot_was_taken() {
        assert_eq!(paste_toast(Some(at(2026, 11, 17, 14, 59))), "Pasted Screenshot 14:59");
        assert_eq!(paste_toast(None), "Pasted Screenshot", "its time could not be read: still a toast");
    }

    #[test]
    fn quick_look_details_give_size_time_and_folder() {
        let d = dir("detail");
        let f = d.join("a.png");
        fs::write(&f, vec![0u8; 2048]).unwrap();
        let line = detail(&f, SystemTime::now()).unwrap();
        let folder = d.file_name().unwrap().to_string_lossy().into_owned();
        assert!(line.starts_with("2.0 KB · Today, ") && line.ends_with(&format!(" · {folder}")), "{line}");
        assert_eq!(detail(&d.join("gone.png"), SystemTime::now()), None);
    }

    #[test]
    fn retention_and_media_and_batches() {
        let d = dir("keep");
        touch(&d.join("week.png"), 7 * 86400);
        touch(&d.join("b.png"), 20);
        touch(&d.join("c.png"), 10);
        touch(&d.join("clip.mp4"), 5);
        let (calls, mut idx) = (Cell::new(0), ShotIndex::default());
        let ocr = |_: &[u8]| {
            calls.set(calls.get() + 1);
            Some(String::new())
        };
        let day = Some(Duration::from_secs(86400));
        let first = Opts { batch: 1, keep: day, ..OPTS };
        assert_eq!(idx.update(std::slice::from_ref(&d), first, ocr), 1);
        assert_eq!(idx.entries[0].path.file_name().unwrap(), "c.png", "newest first");
        assert_eq!(idx.update(std::slice::from_ref(&d), first, ocr), 1);
        assert_eq!(idx.update(std::slice::from_ref(&d), first, ocr), 0, "older than keep, and videos, are not listed");
        assert_eq!(idx.entries.len(), 2);

        let all = Opts { all_media: true, ..OPTS };
        assert_eq!(idx.update(std::slice::from_ref(&d), all, ocr), 2, "the week-old picture and the video");
        let video = idx.entries.iter().find(|s| s.path.ends_with("clip.mp4")).unwrap();
        assert!(!video.failed && video.text.is_empty());
        assert_eq!(calls.get(), 3, "videos are listed, never read");

        idx.prune(Some(Duration::from_secs(86400)));
        assert_eq!(idx.entries.len(), 3, "prune drops the old entry");
        idx.prune(None);
        assert_eq!(idx.entries.len(), 3);
    }

    #[test]
    fn rows_put_the_primary_action_first() {
        let d = dir("rows");
        touch(&d.join("a.png"), 10);
        touch(&d.join("b.mp4"), 5);
        let mut idx = ShotIndex::default();
        idx.update(std::slice::from_ref(&d), Opts { all_media: true, ..OPTS }, |_| Some("\n  Total due\nmore".into()));
        let label = |it: &Item| it.extra.iter().map(|a| a.0.as_str()).collect::<Vec<_>>().join(",");
        let (rows, thumbs) = rows(&idx, "", Kind::All, "paste");
        let rest = "Open With…,Quick Look,Show in File Explorer,Pin";
        assert_eq!(rows.iter().map(&label).collect::<Vec<_>>(), [format!("Open,{rest},Delete"), format!("Paste,Copy to Clipboard,Open,{rest},Copy Text,Delete")], "a video can't be copied as a picture");
        assert_eq!((rows[1].title.as_str(), rows[1].section.as_deref()), ("a.png", Some(SECTION)), "the whole name, in the one section");
        let png = &idx.entries[1];
        assert_eq!(thumbs, [None, Some(format!("t:{}|{}", png.mtime, png.path.display()))], "a video's file is never read for a thumbnail");
        assert_eq!(label(&super::rows(&idx, "a.png", Kind::All, "open").0[0]), format!("Open,Copy to Clipboard,Paste,{rest},Copy Text,Delete"));
        assert_eq!(label(&super::rows(&idx, "a.png", Kind::All, "copy").0[0]), format!("Copy to Clipboard,Paste,Open,{rest},Copy Text,Delete"));
        idx.set_pins(HashSet::from([png.path.clone()]));
        assert!(label(&super::rows(&idx, "a.png", Kind::All, "copy").0[0]).contains(",Unpin,"), "a pinned shot offers Unpin");
    }

    #[test]
    fn unreadable_files_are_left_for_the_next_scan() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = dir("locked");
        touch(&d.join("a.png"), 10);
        touch(&d.join("b.png"), 20);
        let (calls, mut idx) = (Cell::new(0), ShotIndex::default());
        let ocr = |_: &[u8]| {
            calls.set(calls.get() + 1);
            Some("text".to_string())
        };
        // Held without sharing, like a file OneDrive or an antivirus has open.
        let lock = fs::File::options().read(true).share_mode(0).open(d.join("a.png")).unwrap();
        assert_eq!(idx.update(std::slice::from_ref(&d), Opts { batch: 1, ..OPTS }, ocr), 1, "the batch is filled from the files that can be read");
        assert_eq!(idx.entries.len(), 1);
        assert!(idx.entries.iter().all(|s| !s.failed && s.path.ends_with("b.png")), "the locked one is not recorded as failed");
        drop(lock);
        assert_eq!(idx.update(std::slice::from_ref(&d), OPTS, ocr), 1, "and is read once it is free");
        assert_eq!(idx.entries.len(), 2);
    }

    #[test]
    fn without_ocr_nothing_is_read_or_saved() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = dir("nocr");
        touch(&d.join("a.png"), 10);
        touch(&d.join("b.png"), 20);
        let _lock = fs::File::options().read(true).share_mode(0).open(d.join("a.png")).unwrap();
        let (calls, saves, shown) = (Cell::new(0), Cell::new(0), Cell::new(0));
        let mut idx = ShotIndex::default();
        let ocr = |_: &[u8]| {
            calls.set(calls.get() + 1);
            None
        };
        // Also what happens when Windows has no OCR language: no text, no `failed`, no file.
        let show = |_: &ShotIndex, progress: Option<(usize, usize)>| {
            assert_eq!(progress, None, "nothing is read, so no progress");
            shown.set(shown.get() + 1);
        };
        scan_loop(&mut idx, std::slice::from_ref(&d), Opts { batch: 1, ocr: false, ..OPTS }, ocr, show, |_| saves.set(saves.get() + 1));
        assert_eq!((calls.get(), saves.get()), (0, 0));
        assert!(shown.get() >= 2, "the list fills in batch by batch");
        assert_eq!(idx.entries.len(), 2, "a file that can't be opened is still listed");
        assert!(idx.entries.iter().all(|s| !s.failed && s.text.is_empty()));
    }

    #[test]
    fn with_ocr_batches_are_saved() {
        let d = dir("save");
        for (i, n) in ["a", "b", "c"].iter().enumerate() {
            touch(&d.join(format!("{n}.png")), 10 * (i as u64 + 1));
        }
        let (saves, mut idx, mut shown) = (Cell::new(0), ShotIndex::default(), vec![]);
        scan_loop(&mut idx, std::slice::from_ref(&d), Opts { batch: 2, ..OPTS }, |_| Some("x".into()), |_, p| shown.push(p), |_| saves.set(saves.get() + 1));
        assert_eq!((idx.entries.len(), saves.get()), (3, 2), "two batches, one save each");
        assert_eq!(shown, [Some((0, 3)), Some((2, 3)), Some((3, 3))], "read of to read, known before the first batch");
    }

    #[test]
    fn progress_counts_pictures_only() {
        let d = dir("progress");
        touch(&d.join("a.png"), 10);
        touch(&d.join("clip.mp4"), 5);
        let (mut idx, mut shown) = (ShotIndex::default(), vec![]);
        scan_loop(&mut idx, std::slice::from_ref(&d), Opts { all_media: true, ..OPTS }, |_| Some("x".into()), |_, p| shown.push(p), |_| {});
        assert_eq!(idx.entries.len(), 2, "the video is listed");
        assert_eq!(shown, [Some((0, 1)), Some((1, 1))], "but never counted as read");
    }

    #[test]
    fn latest_is_the_newest_picture() {
        let d = dir("latest");
        assert_eq!(latest(std::slice::from_ref(&d)), None);
        touch(&d.join("a.png"), 50);
        touch(&d.join("b.jpg"), 10);
        touch(&d.join("c.mp4"), 1);
        assert_eq!(latest(&[d, PathBuf::from("Z:\nope")]).unwrap().file_name().unwrap(), "b.jpg");
    }

    #[test]
    fn clipboard_gets_png_whatever_the_file_is() {
        let d = dir("png");
        let img = image::RgbaImage::from_pixel(4, 3, image::Rgba([200, 30, 30, 255]));
        let jpg = d.join("a.jpg");
        image::DynamicImage::ImageRgba8(img.clone()).to_rgb8().save(&jpg).unwrap();
        let png = png_of(&jpg).unwrap();
        assert!(png.starts_with(b"\x89PNG"), "a JPEG is re-encoded");
        assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8().dimensions(), (4, 3));
        let same = d.join("a.png");
        img.save(&same).unwrap();
        assert_eq!(png_of(&same).unwrap(), fs::read(&same).unwrap(), "a PNG goes as it is");
        fs::write(d.join("bad.png"), b"nope").unwrap();
        assert!(png_of(&d.join("bad.png")).is_err());
    }
}
