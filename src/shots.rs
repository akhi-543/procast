//! Screenshot search: the pictures (and, if asked, videos) in the screenshot folders, with the
//! text Windows' OCR finds in each, kept in %APPDATA%\procast\screenshots.json.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::HWND;

use crate::{
    clipboard, config,
    design::{Tint, g},
    search::{Act, Icon, Item, ago},
};

const IMAGES: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif"];
const VIDEOS: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "wmv", "m4v"];
/// Bigger pictures are listed but not read for text.
const MAX_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Shot {
    pub path: PathBuf,
    /// Modified time, seconds since the epoch.
    pub mtime: u64,
    pub text: String,
    /// Reading its text failed; not tried again until the file changes.
    pub failed: bool,
}

impl Shot {
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
}

/// How an update scans: videos too, how old a file may be, and how many files to read per call.
#[derive(Clone, Copy)]
pub struct Opts {
    pub all_media: bool,
    pub keep: Option<Duration>,
    pub batch: usize,
}

fn ext(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_lowercase())
}

fn video(path: &Path) -> bool {
    ext(path).is_some_and(|e| VIDEOS.contains(&e.as_str()))
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The pictures (and videos) directly inside `folders` with their modified times. Folders
/// that are missing are skipped.
// ponytail: top level only; add recursion if people sort screenshots into subfolders.
fn scan(folders: &[PathBuf], all_media: bool) -> HashMap<PathBuf, u64> {
    let mut found = HashMap::new();
    for entry in folders.iter().filter_map(|f| fs::read_dir(f).ok()).flatten().flatten() {
        let path = entry.path();
        let wanted = ext(&path).is_some_and(|e| IMAGES.contains(&e.as_str())) || (all_media && video(&path));
        let Ok(meta) = entry.metadata() else { continue };
        // The index is stored as JSON: a path that isn't Unicode can't be.
        if wanted && meta.is_file() && path.to_str().is_some() {
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
        fs::read(Self::file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
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

    /// Brings the index in line with `folders`: drops what is gone or too old, then reads the
    /// text of up to `opts.batch` new or changed files, newest first (a failed read is
    /// remembered, not retried). Returns how many were read.
    pub fn update(&mut self, folders: &[PathBuf], opts: Opts, ocr: impl Fn(&[u8]) -> Option<String>) -> usize {
        self.prune(opts.keep);
        let mut found = scan(folders, opts.all_media);
        if let Some(keep) = opts.keep {
            let at = cutoff(keep);
            found.retain(|_, m| *m >= at);
        }
        let known: HashMap<&PathBuf, u64> = self.entries.iter().map(|e| (&e.path, e.mtime)).collect();
        let mut todo: Vec<(PathBuf, u64)> = found.iter().filter(|(p, m)| known.get(p) != Some(m)).map(|(p, m)| (p.clone(), *m)).collect();
        todo.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        todo.truncate(opts.batch);
        let redo: HashSet<&PathBuf> = todo.iter().map(|t| &t.0).collect();
        self.entries.retain(|e| found.contains_key(&e.path) && !redo.contains(&e.path));
        let read = todo.len();
        for (path, mtime) in todo {
            let (text, failed) = if video(&path) {
                (String::new(), false)
            } else {
                let bytes = fs::metadata(&path).ok().filter(|m| m.len() <= MAX_BYTES).and_then(|_| fs::read(&path).ok());
                match bytes.and_then(|b| ocr(&b)) {
                    Some(text) => (text, false),
                    None => (String::new(), true),
                }
            };
            self.entries.push(Shot { path, mtime, text, failed });
        }
        self.entries.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.path.cmp(&b.path)));
        read
    }

    /// Entries matching `q` (every word in the file name or the text): name matches first,
    /// best first, then text matches; newest first among equals, and all of them when `q`
    /// is empty.
    pub fn search(&self, q: &str) -> Vec<&Shot> {
        if q.trim().is_empty() {
            return self.entries.iter().collect();
        }
        let mut matcher = Matcher::new(Config::DEFAULT);
        let fuzzy = Pattern::new(q, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy);
        let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
        let mut buf = Vec::new();
        let mut hits: Vec<(u32, &Shot)> = self
            .entries
            .iter()
            .filter_map(|s| {
                let name = s.name();
                // A name is typed loosely ("scrn"); OCR text is long, so a fuzzy match there
                // would hit almost everything and it must contain the words as written.
                // (nucleo's substring matcher misses inside non-ASCII text, which OCR text often is.)
                let by_name = fuzzy.score(Utf32Str::new(&name, &mut buf), &mut matcher).map(|v| v + 1);
                let hay = format!("{name}\n{}", s.text).to_lowercase();
                let by_text = words.iter().all(|w| hay.contains(w.as_str())).then_some(1);
                by_name.max(by_text).map(|score| (score, s))
            })
            .collect();
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        hits.into_iter().map(|h| h.1).collect()
    }

    /// Forgets entries older than `keep`.
    pub fn prune(&mut self, keep: Option<Duration>) {
        if let Some(keep) = keep {
            let at = cutoff(keep);
            self.entries.retain(|e| e.mtime >= at);
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

/// The newest picture in the folders.
pub fn latest(folders: &[PathBuf]) -> Option<PathBuf> {
    scan(folders, false).into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0))).map(|(p, _)| p)
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

/// Puts a screenshot (the newest one when `path` is None) on the clipboard as an image.
pub fn put_image(owner: HWND, path: Option<PathBuf>, folders: &[PathBuf]) -> Result<(), String> {
    let path = path.or_else(|| latest(folders)).ok_or("No screenshots found")?;
    let png = png_of(&path)?;
    if clipboard::set(owner, clipboard::Put::Image(&png), false) { Ok(()) } else { Err("Could not put it on the clipboard".into()) }
}

/// Brings the saved index up to date, reading new screenshots' text a batch at a time;
/// `show` gets the index after each step. Run on a worker thread. Without OCR nothing is read
/// or saved: the folders are just listed.
pub fn rescan(folders: &[PathBuf], keep: Option<Duration>, all_media: bool, ocr: bool, mut show: impl FnMut(&ShotIndex)) {
    // Text recognition is a WinRT API.
    unsafe { _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
    let mut index = if ocr { ShotIndex::load() } else { ShotIndex::default() };
    show(&index);
    let opts = Opts { all_media, keep, batch: if ocr { 20 } else { usize::MAX } };
    loop {
        let before = index.entries.len();
        let read = index.update(folders, opts, |png| if ocr { clipboard::recognise(png) } else { Some(String::new()) });
        if ocr && (read > 0 || index.entries.len() != before) {
            index.save();
        }
        show(&index);
        if read < opts.batch {
            break;
        }
    }
}

/// The result rows: the primary action first (`primary`: copy | paste | open), videos only open.
pub fn rows(index: &ShotIndex, query: &str, primary: &str, max: usize) -> Vec<Item> {
    index
        .search(query)
        .into_iter()
        .take(max)
        .map(|s| {
            let path = s.path.to_string_lossy().into_owned();
            let (copy, paste, open) = (Act::CopyShot(path.clone()), Act::PasteShot(path.clone()), Act::File(path.clone()));
            let mut acts = match (s.is_video(), primary) {
                (true, _) => vec![open],
                (_, "paste") => vec![paste, copy, open],
                (_, "open") => vec![open, copy, paste],
                _ => vec![copy, paste, open],
            };
            acts.push(Act::Reveal(path));
            let first = s.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(120).collect::<String>();
            let icon = Icon::Vector(if s.is_video() { g::VIDEO } else { g::PICTURE });
            let mut it = Item::new(s.name(), first, icon, acts[0].clone()).acc(ago(s.mtime)).tinted(Tint::File);
            it.extra = acts.into_iter().map(|a| (a.label().to_string(), a)).collect();
            it
        })
        .collect()
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

    const OPTS: Opts = Opts { all_media: false, keep: None, batch: 100 };

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
        assert_eq!(names(idx.search("")), ["new.png", "Screenshot receipt.png", "old.png"]);
        assert_eq!(idx.search("invoice").len(), 3, "OCR text is searched");
        assert_eq!(names(idx.search("receipt")), ["Screenshot receipt.png"], "so are names");
        assert_eq!(idx.search("\u{e9}t\u{e9}").len(), 3, "non-ASCII text is fine");
        assert!(idx.search("zebra").is_empty());
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

        let all = Opts { all_media: true, keep: None, batch: 100 };
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
        let rows = rows(&idx, "", "paste", 10);
        assert_eq!(rows.iter().map(&label).collect::<Vec<_>>(), ["Open,Show in Explorer", "Paste Image,Copy Image,Open,Show in Explorer"], "a video can only open");
        assert_eq!(rows[1].subtitle, "Total due", "the first line of text");
        assert_eq!(label(&super::rows(&idx, "a.png", "open", 10)[0]), "Open,Copy Image,Paste Image,Show in Explorer");
        assert_eq!(label(&super::rows(&idx, "a.png", "copy", 10)[0]), "Copy Image,Paste Image,Open,Show in Explorer");
        assert_eq!(super::rows(&idx, "", "copy", 1).len(), 1);
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
