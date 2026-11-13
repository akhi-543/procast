//! Clipboard history: text, images and copied files, newest first, encrypted at rest with
//! DPAPI (user scope). Images are kept as encrypted PNGs in `clips\`, next to the history.

use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures::channel::mpsc::UnboundedSender;
use serde::{Deserialize, Serialize};
use windows::{
    Win32::{
        Foundation::{HANDLE, HGLOBAL, HLOCAL, HWND, LocalFree, POINT},
        Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
                OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT},
            Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
        },
        UI::{
            Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_V},
            Shell::{DROPFILES, DragQueryFileW, HDROP},
            WindowsAndMessaging::GetWindowThreadProcessId,
        },
    },
    core::{PWSTR, w},
};

use crate::win::Msg;

const MAX_ENTRY_BYTES: usize = 64 * 1024;
/// Larger copied images (as PNG) are not recorded.
const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Debug)]
pub enum Kind {
    #[default]
    Text,
    /// An encrypted PNG in `clips\<id>.png`.
    Image { id: String, w: u32, h: u32, bytes: usize },
    /// Copied files and folders, full paths.
    Files(Vec<String>),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Entry {
    /// The text; for an image the text recognised in it (may be empty), for files their paths, one per line.
    pub text: String,
    pub at: u64,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub pinned: bool,
    /// The program it was copied from ("chrome.exe"), when known.
    #[serde(default)]
    pub app: String,
}

impl Entry {
    #[cfg(test)]
    pub fn text(text: impl Into<String>, at: u64) -> Self {
        Self { text: text.into(), at, kind: Kind::Text, pinned: false, app: String::new() }
    }

    /// What tells entries apart; actions name an entry by it.
    pub fn key(&self) -> String {
        match &self.kind {
            Kind::Text => format!("text:{}", self.text),
            Kind::Image { id, .. } => format!("img:{id}"),
            Kind::Files(_) => format!("files:{}", self.text),
        }
    }
}

pub struct History {
    path: PathBuf,
    pub entries: Vec<Entry>,
    cap: usize,
    /// Entries older than this many seconds go (pinned ones stay). None: the count `cap` rules.
    keep: Option<u64>,
}

/// `[clipboard] keep` -> how long entries live: None = by count, Some(u64::MAX) = forever.
pub fn keep_secs(keep: &str) -> Option<u64> {
    const H: u64 = 3600;
    const D: u64 = 24 * H;
    Some(match keep {
        "1h" => H,
        "1d" => D,
        "7d" => 7 * D,
        "30d" => 30 * D,
        "90d" => 90 * D,
        "180d" => 180 * D,
        "365d" => 365 * D,
        "unlimited" => u64::MAX,
        _ => return None,
    })
}

pub const KEEP: &[&str] = &["count", "1h", "1d", "7d", "30d", "90d", "180d", "365d", "unlimited"];

impl History {
    pub fn load(path: PathBuf, cap: usize, keep: &str) -> Self {
        let entries = fs::read(&path).ok().and_then(|b| unprotect(&b)).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut h = Self { path, entries, cap, keep: keep_secs(keep) };
        h.prune(crate::search::now());
        h
    }

    fn clips(&self) -> PathBuf {
        self.path.with_file_name("clips")
    }

    pub fn image_path(&self, id: &str) -> PathBuf {
        self.clips().join(format!("{id}.png"))
    }

    /// New limits from the settings; applied at once.
    pub fn set_limits(&mut self, cap: usize, keep: &str) {
        (self.cap, self.keep) = (cap, keep_secs(keep));
        self.prune(crate::search::now());
    }

    /// Drops what's past the limits; saves only when something went.
    pub fn prune(&mut self, now: u64) {
        let before = self.entries.len();
        match self.keep {
            Some(secs) => self.entries.retain(|e| e.pinned || now.saturating_sub(e.at) < secs),
            None => {
                let mut unpinned = 0;
                let cap = self.cap.max(1);
                self.entries.retain(|e| {
                    unpinned += !e.pinned as usize;
                    e.pinned || unpinned <= cap
                });
            }
        }
        if self.entries.len() != before {
            self.save();
        }
    }

    /// `png` comes with a new image entry.
    pub fn push(&mut self, mut e: Entry, png: Option<Vec<u8>>) {
        let empty = match &e.kind {
            Kind::Text => e.text.trim().is_empty() || e.text.len() > MAX_ENTRY_BYTES,
            Kind::Files(f) => f.is_empty(),
            Kind::Image { .. } => png.is_none(),
        };
        if empty {
            return;
        }
        if let (Kind::Image { id, .. }, Some(png)) = (&e.kind, png) {
            let path = self.image_path(id);
            if !path.exists() {
                let _ = fs::create_dir_all(self.clips());
                match protect(&png) {
                    Some(b) if fs::write(&path, &b).is_ok() => {}
                    _ => return,
                }
            }
        }
        let key = e.key();
        if let Some(old) = self.entries.iter().position(|x| x.key() == key) {
            let old = self.entries.remove(old);
            e.pinned |= old.pinned;
            // Pasted back from history: procast is not where it came from.
            let me = std::env::current_exe().ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_lowercase()));
            if me.as_deref() == Some(e.app.as_str()) || e.app.is_empty() {
                e.app = old.app;
            }
            // Copying an image again keeps the text found in it.
            if e.text.is_empty() {
                e.text = old.text;
            }
        }
        self.entries.insert(0, e);
        self.prune(crate::search::now());
        self.save();
    }

    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key() == key)
    }

    pub fn remove(&mut self, key: &str) {
        self.entries.retain(|e| e.key() != key);
        self.save();
    }

    pub fn pin(&mut self, key: &str, on: bool) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.key() == key) {
            e.pinned = on;
            self.save();
        }
    }

    /// Everything but pinned entries.
    pub fn clear(&mut self) {
        self.entries.retain(|e| e.pinned);
        self.save();
    }

    /// Also deletes image files no entry points at any more.
    fn save(&self) {
        if let Some(bytes) = serde_json::to_vec(&self.entries).ok().and_then(|j| protect(&j)) {
            let _ = fs::write(&self.path, bytes);
        }
        let live: Vec<String> = self.entries.iter().filter_map(|e| if let Kind::Image { id, .. } = &e.kind { Some(format!("{id}.png")) } else { None }).collect();
        for f in fs::read_dir(self.clips()).into_iter().flatten().flatten() {
            if !live.iter().any(|l| *l == f.file_name().to_string_lossy()) {
                let _ = fs::remove_file(f.path());
            }
        }
    }

    /// An image entry's PNG, decrypted.
    pub fn png(&self, id: &str) -> Option<Vec<u8>> {
        unprotect(&fs::read(self.image_path(id)).ok()?)
    }
}

/// A PNG as a GPUI image (BGRA), fitted inside `max` × `max`.
pub fn render_image(png: &[u8], max: u32) -> Option<Arc<gpui::RenderImage>> {
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let img = if img.width() > max || img.height() > max { image::imageops::thumbnail(&img, max, max) } else { img };
    let (w, h) = img.dimensions();
    let mut buf = img.into_raw();
    for p in buf.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let frame = image::Frame::new(image::RgbaImage::from_raw(w, h, buf)?);
    Some(Arc::new(gpui::RenderImage::new(vec![frame])))
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe { _ = LocalFree(Some(HLOCAL(out.pbData as _))) };
    v
}

fn protect(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptProtectData(&blob(data), w!("procast clipboard"), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()? };
    Some(take(out))
}

fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptUnprotectData(&blob(data), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()? };
    Some(take(out))
}

static IGNORE: Mutex<Vec<String>> = Mutex::new(Vec::new());
static OCR: AtomicBool = AtomicBool::new(false);

/// The watcher's settings; takes effect from the next copy.
pub fn configure(ignore_apps: &[String], ocr: bool) {
    *IGNORE.lock().unwrap() = ignore_apps.iter().map(|a| a.to_lowercase()).collect();
    OCR.store(ocr, Ordering::SeqCst);
}

/// Polls the clipboard sequence number and forwards each new copy.
// ponytail: 300 ms poll; switch to AddClipboardFormatListener on a message-only window
// if the latency or wakeups ever matter.
pub fn watch(tx: UnboundedSender<Msg>) {
    std::thread::spawn(move || {
        // Text recognition is a WinRT API.
        unsafe { _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
        let mut last = unsafe { GetClipboardSequenceNumber() };
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let seq = unsafe { GetClipboardSequenceNumber() };
            if seq == last {
                continue;
            }
            last = seq;
            let ignore = IGNORE.lock().unwrap().clone();
            let Some((mut entry, png)) = read(&ignore) else { continue };
            if let (Some(png), true) = (&png, OCR.load(Ordering::SeqCst)) {
                entry.text = recognise(png).unwrap_or_default();
            }
            if tx.unbounded_send(Msg::Clip(entry, png)).is_err() {
                return;
            }
        }
    });
}

/// The text Windows' own OCR finds in an image (offline, in your Windows display languages).
fn recognise(png: &[u8]) -> Option<String> {
    use windows::{
        Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
        Media::Ocr::OcrEngine,
        Storage::Streams::DataWriter,
    };
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let max = OcrEngine::MaxImageDimension().ok()?;
    let img = if img.width() > max || img.height() > max { image::imageops::thumbnail(&img, max, max) } else { img };
    let (w, h) = img.dimensions();
    let mut bgra = img.into_raw();
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let writer = DataWriter::new().ok()?;
    writer.WriteBytes(&bgra).ok()?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&writer.DetachBuffer().ok()?, BitmapPixelFormat::Bgra8, w as i32, h as i32).ok()?;
    let result = OcrEngine::TryCreateFromUserProfileLanguages().ok()?.RecognizeAsync(&bitmap).ok()?.join().ok()?;
    let lines: Vec<String> = result.Lines().ok()?.into_iter().filter_map(|l| l.Text().ok()).map(|t| t.to_string()).collect();
    Some(lines.join("\n"))
}

/// Formats password managers set to opt out of clipboard history; procast sets the first one
/// on its own snippet pastes.
fn excluded() -> bool {
    unsafe {
        for name in [w!("ExcludeClipboardContentFromMonitorProcessing"), w!("Clipboard Viewer Ignore")] {
            if IsClipboardFormatAvailable(RegisterClipboardFormatW(name)).is_ok() {
                return true;
            }
        }
        let can = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        if IsClipboardFormatAvailable(can).is_ok()
            && let Ok(h) = GetClipboardData(can)
        {
            let p = GlobalLock(HGLOBAL(h.0)) as *const u32;
            let allowed = p.is_null() || *p != 0;
            let _ = GlobalUnlock(HGLOBAL(h.0));
            return !allowed;
        }
        false
    }
}

fn owner_exe() -> String {
    unsafe { GetClipboardOwner() }.map(exe_of).unwrap_or_default()
}

/// A window's program file name, lower case ("chrome.exe").
pub fn exe_of(hwnd: HWND) -> String {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let _ = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().unwrap_or_default().to_lowercase()
    }
}

/// Runs `f` with the clipboard open; another app may hold it for a moment.
fn with_clipboard<T>(owner: Option<HWND>, f: impl FnOnce() -> Option<T>) -> Option<T> {
    unsafe {
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(owner).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !opened {
            return None;
        }
        let out = f();
        let _ = CloseClipboard();
        out
    }
}

/// A clipboard format's bytes.
unsafe fn bytes_of(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let h: HANDLE = GetClipboardData(format).ok()?;
        let g = HGLOBAL(h.0);
        let p = GlobalLock(g) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, GlobalSize(g)).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

fn utf16_z(bytes: &[u8]) -> String {
    let s: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let len = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..len])
}

/// The clipboard as text, if it holds any (for snippet `{clipboard}`).
pub fn text() -> Option<String> {
    with_clipboard(None, || Some(utf16_z(&unsafe { bytes_of(CF_UNICODETEXT.0 as u32) }?)))
}

/// What was just copied: files, else text, else an image (Office puts a picture of copied
/// cells next to their text; the text is what you want back).
fn read(ignore: &[String]) -> Option<(Entry, Option<Vec<u8>>)> {
    let png_format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    with_clipboard(None, || unsafe {
        if excluded() {
            return None;
        }
        let app = owner_exe();
        if ignore.contains(&app) {
            return None;
        }
        let at = crate::search::now();
        let entry = |text: String, kind| Entry { text, at, kind, pinned: false, app: app.clone() };
        if let Ok(h) = GetClipboardData(CF_HDROP.0 as u32) {
            let drop = HDROP(h.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            let files: Vec<String> = (0..n)
                .map(|i| {
                    let mut buf = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
                    let len = DragQueryFileW(drop, i, Some(&mut buf));
                    String::from_utf16_lossy(&buf[..len as usize])
                })
                .collect();
            if !files.is_empty() {
                return Some((entry(files.join("\n"), Kind::Files(files)), None));
            }
        }
        if let Some(b) = bytes_of(CF_UNICODETEXT.0 as u32) {
            return Some((entry(utf16_z(&b), Kind::Text), None));
        }
        let png = bytes_of(png_format).filter(|b| b.starts_with(b"\x89PNG")).or_else(|| dib_to_png(&bytes_of(CF_DIB.0 as u32)?))?;
        if png.len() > MAX_IMAGE_BYTES {
            return None;
        }
        let (w, h) = image::load_from_memory(&png).ok().map(|i| (i.width(), i.height()))?;
        Some((entry(String::new(), Kind::Image { id: hash(&png), w, h, bytes: png.len() }), Some(png)))
    })
}

fn hash(bytes: &[u8]) -> String {
    // FNV-1a: names the image file; the same picture copied twice is one entry.
    let h = bytes.iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    format!("{h:016x}")
}

/// A packed DIB (CF_DIB: BITMAPINFOHEADER then pixels) of 24 or 32 bits -> PNG.
fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |o: usize| Some(u32::from_le_bytes(dib.get(o..o + 4)?.try_into().ok()?));
    let (size, w, h) = (u32_at(0)? as usize, u32_at(4)? as i32, u32_at(8)? as i32);
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?) as usize;
    let compression = u32_at(16)?;
    let colors = u32_at(32)? as usize;
    // BI_RGB, or BI_BITFIELDS with the masks after a plain header.
    if !matches!(bits, 24 | 32) || !matches!(compression, 0 | 3) || w <= 0 || h == 0 {
        return None;
    }
    let offset = size + if compression == 3 && size == 40 { 12 } else { 0 } + colors * 4;
    let (w, rows) = (w as usize, h.unsigned_abs() as usize);
    let stride = (w * bits).div_ceil(32) * 4;
    let px = dib.get(offset..offset + stride * rows)?;
    let mut rgba = Vec::with_capacity(w * rows * 4);
    for y in 0..rows {
        // Positive height: bottom-up rows.
        let row = &px[if h > 0 { rows - 1 - y } else { y } * stride..][..stride];
        for x in 0..w {
            let p = &row[x * bits / 8..];
            rgba.extend_from_slice(&[p[2], p[1], p[0], if bits == 32 { p[3] } else { 255 }]);
        }
    }
    // Most 32-bit DIBs leave alpha at zero: that means opaque, not invisible.
    if bits == 32 && rgba.chunks_exact(4).all(|p| p[3] == 0) {
        rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }
    let img = image::RgbaImage::from_raw(w as u32, rows as u32, rgba)?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

unsafe fn put(format: u32, data: &[u8]) -> bool {
    unsafe {
        let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, data.len()) else { return false };
        let p = GlobalLock(g) as *mut u8;
        if p.is_null() {
            return false;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        let _ = GlobalUnlock(g);
        SetClipboardData(format, Some(HANDLE(g.0))).is_ok()
    }
}

/// What procast can put on the clipboard besides plain text.
pub enum Put<'a> {
    Text(&'a str),
    /// PNG bytes: written as PNG (keeps transparency) and as a DIB for every other app.
    Image(&'a [u8]),
    Files(&'a [String]),
}

/// Replaces the clipboard. `owner` must be a window of ours (Windows refuses data from a
/// clipboard opened without one). `hidden` keeps procast's own history from recording it.
pub fn set(owner: HWND, what: Put, hidden: bool) -> bool {
    with_clipboard(Some(owner), || unsafe {
        EmptyClipboard().ok()?;
        let ok = match what {
            Put::Text(s) => put(CF_UNICODETEXT.0 as u32, &s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect::<Vec<_>>()),
            Put::Image(png) => {
                let dib = png_to_dib(png)?;
                put(RegisterClipboardFormatW(w!("PNG")), png) & put(CF_DIB.0 as u32, &dib)
            }
            Put::Files(paths) => {
                let head = DROPFILES { pFiles: size_of::<DROPFILES>() as u32, pt: POINT::default(), fNC: false.into(), fWide: true.into() };
                let mut data = std::slice::from_raw_parts(&head as *const _ as *const u8, size_of::<DROPFILES>()).to_vec();
                for p in paths {
                    data.extend(p.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
                }
                data.extend([0, 0]);
                // DROPEFFECT_COPY: Explorer copies the files on paste instead of moving them.
                put(CF_HDROP.0 as u32, &data) & put(RegisterClipboardFormatW(w!("Preferred DropEffect")), &1u32.to_le_bytes())
            }
        };
        if hidden {
            put(RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")), &[0]);
        }
        ok.then_some(())
    })
    .is_some()
}

/// PNG -> packed 32-bit bottom-up DIB.
fn png_to_dib(png: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut out = Vec::with_capacity(40 + (w * h * 4) as usize);
    for v in [40u32, w, h] {
        out.extend(v.to_le_bytes());
    }
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend([0u8; 24]); // BI_RGB, then sizes and colour counts left at zero
    for row in img.rows().rev() {
        for p in row {
            out.extend([p[2], p[1], p[0], p[3]]);
        }
    }
    Some(out)
}

/// A key event marked as procast's own (`keys::OURS`).
pub fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let ki = KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, dwExtraInfo: crate::keys::OURS, ..Default::default() };
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }
}

/// Ctrl+V into whatever window has focus.
pub fn send_paste() {
    let seq = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
    unsafe { SendInput(&seq, size_of::<INPUT>() as i32) };
}

/// Ctrl+V after the window we hand focus back to has had a moment to get it.
pub fn paste_soon() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(80));
        send_paste();
    });
}

/// Presses a key `n` times (Backspace before a snippet, Left to its `{cursor}`).
pub fn tap(vk: VIRTUAL_KEY, n: usize) {
    let seq: Vec<INPUT> = (0..n).flat_map(|_| [key(vk, false), key(vk, true)]).collect();
    if !seq.is_empty() {
        unsafe { SendInput(&seq, size_of::<INPUT>() as i32) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Windows OCR on a picture: PROCAST_OCR_PNG=<file> cargo test ocr -- --ignored"]
    fn ocr_reads_text_in_a_picture() {
        unsafe { _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
        let png = fs::read(std::env::var("PROCAST_OCR_PNG").unwrap()).unwrap();
        let text = recognise(&png).unwrap();
        println!("recognised: {text:?}");
        assert!(text.to_lowercase().contains("procast"), "{text}");
    }

    #[test]
    fn dib_round_trip_and_limits() {
        let img = image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([x as u8 * 80, y as u8 * 100, 7, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let back = dib_to_png(&png_to_dib(png.get_ref()).unwrap()).unwrap();
        assert_eq!(image::load_from_memory(&back).unwrap().to_rgba8(), img, "PNG -> DIB -> PNG keeps every pixel");

        let dir = std::env::temp_dir().join(format!("procast-clip-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let mut h = History { path: dir.join("clipboard.bin"), entries: vec![], cap: 2, keep: None };
        for (i, t) in ["a", "b", "c"].iter().enumerate() {
            h.push(Entry::text(*t, 100 + i as u64), None);
        }
        assert_eq!(h.entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["c", "b"], "count cap");
        h.pin("text:b", true);
        h.push(Entry::text("d", 200), None);
        h.push(Entry::text("e", 201), None);
        assert_eq!(h.entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["e", "d", "b"], "pinned entries stay past the cap");
        h.push(Entry::text("b", 300), None);
        assert!(h.entries[0].pinned, "copying a pinned entry again keeps it pinned");

        h.set_limits(2, "1h");
        h.push(Entry::text("old", 0), None);
        h.prune(10_000);
        assert_eq!(h.entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["b"], "by age, pinned stays");
        h.clear();
        assert_eq!(h.entries.len(), 1, "clearing keeps pinned entries");

        let id = hash(png.get_ref());
        h.push(Entry { text: String::new(), at: crate::search::now(), kind: Kind::Image { id: id.clone(), w: 3, h: 2, bytes: 1 }, pinned: false, app: String::new() }, Some(png.get_ref().clone()));
        assert_eq!(h.png(&id).as_deref(), Some(png.get_ref().as_slice()), "images are stored encrypted and read back");
        h.remove(&format!("img:{id}"));
        assert!(!h.image_path(&id).exists(), "a removed image's file goes too");
        let _ = fs::remove_dir_all(dir);
        assert_eq!((keep_secs("count"), keep_secs("1d")), (None, Some(86400)));
    }
}
