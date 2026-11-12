//! Everything that talks to Win32 directly.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use futures::channel::mpsc::UnboundedSender;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, POINT, RECT, SIZE},
        Graphics::{
            Dwm::{DWMWA_CLOAK, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute},
            Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateRoundRectRgn, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW,
                MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, MonitorFromWindow, ReleaseDC, SetWindowRgn,
            },
        },
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
            FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
        },
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree},
            Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW},
            SystemInformation::GetLocalTime,
            Threading::{CreateMutexW, GetCurrentThreadId},
        },
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey},
            Shell::{
                BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItemImageFactory, KF_FLAG_DEFAULT,
                SHCreateItemFromParsingName, SHGetKnownFolderItem, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
                SIIGBF_ICONONLY, ShellExecuteW,
            },
            WindowsAndMessaging::{
                ASFW_ANY, AllowSetForegroundWindow, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowRect,
                GetWindowLongPtrW, MSG, PostThreadMessageW, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
                SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, WM_APP, WM_HOTKEY, WS_EX_TOOLWINDOW,
            },
        },
    },
    core::{HSTRING, Interface, PCWSTR, w},
};

use crate::config::parse_hotkey;

/// Messages from Win32 threads (hotkeys, tray, app scan, clipboard) to the UI thread.
pub enum Msg {
    /// A global hotkey, by its index in the list given to `Hotkeys` (the first four are `BINDS`).
    Hotkey(usize),
    HotkeyStatus(usize, Result<(), String>),
    Apps(Vec<App>),
    Tray(TrayCmd),
    Clip(String),
    /// File search results for one query generation.
    Files(u64, Vec<crate::search::Item>),
    FilesIndexed,
    /// The window's content height changed (compact mode); resize between updates.
    Resize,
    /// Something was installed or removed: the app list should be rescanned.
    AppsChanged,
    /// A script finished: (script index, succeeded, output).
    ScriptDone(usize, bool, String),
    /// A line from the extension host.
    Ext(serde_json::Value),
    /// Extension install progress: (message, Some(ok) when finished).
    Install(String, Option<bool>),
    /// Time to check for extension commands with an `interval`.
    Tick,
    /// A web image finished downloading into the cache.
    ImageReady(std::path::PathBuf),
    /// The store index finished loading.
    Store(Vec<crate::ext::StoreEntry>),
    /// Quit Applications: running apps with their icons.
    Running(Vec<(crate::apps::Running, Option<Arc<gpui::RenderImage>>)>),
    /// A tray menu item of a menu-bar command was clicked: (run, handler).
    TrayExt(u64, String),
}

pub enum TrayCmd {
    Open,
    Settings,
    EditConfig,
    Quit,
}

#[derive(Clone)]
pub struct App {
    pub name: String,
    /// Parsing name inside shell:AppsFolder (an AUMID or a path/known-folder id).
    pub id: String,
    pub icon: Option<Arc<gpui::RenderImage>>,
}

// ---- hotkeys ------------------------------------------------------------------------

/// What the first four global hotkeys open; the rest run commands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bind {
    Launcher = 1,
    Clipboard = 2,
    Emoji = 3,
    Files = 4,
}

pub const BINDS: [Bind; 4] = [Bind::Launcher, Bind::Clipboard, Bind::Emoji, Bind::Files];

/// Owns RegisterHotKey on a dedicated thread with its own message loop.
/// `set` re-registers everything; failures are reported, never fatal. Empty spec = unbound.
/// RegisterHotKey ids are list index + 1.
pub struct Hotkeys {
    thread: u32,
    wanted: Arc<Mutex<Vec<String>>>,
}

impl Hotkeys {
    pub fn spawn(specs: Vec<String>, tx: UnboundedSender<Msg>) -> Self {
        let wanted = Arc::new(Mutex::new(specs));
        let (tid_tx, tid_rx) = std::sync::mpsc::channel();
        let shared = wanted.clone();
        std::thread::spawn(move || unsafe {
            let _ = tid_tx.send(GetCurrentThreadId());
            let mut registered = 0;
            let mut register = || {
                let specs = shared.lock().unwrap().clone();
                for id in 1..=registered.max(specs.len()) as i32 {
                    let _ = UnregisterHotKey(None, id);
                }
                registered = specs.len();
                let mut taken: Vec<(u32, u32)> = vec![];
                for (i, spec) in specs.iter().enumerate() {
                    if spec.trim().is_empty() {
                        continue;
                    }
                    let status = parse_hotkey(spec).and_then(|key| {
                        if taken.contains(&key) {
                            return Err(format!("hotkey {spec} is set twice in procast; change one in Settings → Shortcuts"));
                        }
                        taken.push(key);
                        RegisterHotKey(None, i as i32 + 1, HOT_KEY_MODIFIERS(key.0) | MOD_NOREPEAT, key.1)
                            .map_err(|_| format!("hotkey {spec} is already used by another app; change it in Settings"))
                    });
                    let _ = tx.unbounded_send(Msg::HotkeyStatus(i, status));
                }
            };
            register();
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                match msg.message {
                    WM_HOTKEY if msg.wParam.0 >= 1 => {
                        let _ = tx.unbounded_send(Msg::Hotkey(msg.wParam.0 - 1));
                    }
                    WM_APP => register(),
                    _ => {}
                }
            }
        });
        Self { thread: tid_rx.recv().unwrap_or(0), wanted }
    }

    pub fn set(&self, specs: Vec<String>) {
        *self.wanted.lock().unwrap() = specs;
        unsafe { _ = PostThreadMessageW(self.thread, WM_APP, Default::default(), Default::default()) };
    }
}

// ---- window -------------------------------------------------------------------------

pub fn hwnd(window: &gpui::Window) -> HWND {
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => HWND(h.hwnd.get() as _),
        _ => HWND::default(),
    }
}

/// Keep the launcher out of the taskbar and Alt+Tab.
pub fn make_tool_window(hwnd: HWND) {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW.0 as isize);
    }
}

/// Windows 11 rounded corners (no-op on Windows 10). DWM rounds to 8 px; kept as the fallback
/// and for the system's own shadow/animation, while `round_region` cuts the window to r-xl.
pub fn round_corners(hwnd: HWND) {
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: i32 = 2;
    unsafe { _ = DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(DWMWA_WINDOW_CORNER_PREFERENCE as i32), &DWMWCP_ROUND as *const _ as _, 4) };
}

/// Clips the launcher to the r-xl corner radius (16 x launcher scale, in physical pixels), so no
/// bare backdrop shows between the body's corners and DWM's 8 px ones. Call after every
/// `SetWindowPos` that may change the size. The system owns the region once it is set.
/// To disable: set `ROUND_REGION` to false (DWM's corners remain, with a ~3 px wedge on acrylic).
fn round_region(hwnd: HWND) {
    const ROUND_REGION: bool = true;
    if !ROUND_REGION {
        return;
    }
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_err() {
            return;
        }
        let diameter = (2. * crate::design::r::XL * crate::design::launcher_scale() * dx as f32 / 96.).round() as i32;
        // The right and bottom edges are exclusive in a region: one more than the size.
        let rgn = CreateRoundRectRgn(0, 0, r.right - r.left + 1, r.bottom - r.top + 1, diameter, diameter);
        if SetWindowRgn(hwnd, Some(rgn), true) == 0 {
            let _ = DeleteObject(rgn.into());
        }
    }
}

/// A dark or light caption bar (DWMWA_USE_IMMERSIVE_DARK_MODE), so it follows procast's mode
/// rather than Windows'.
pub fn dark_title_bar(hwnd: HWND, dark: bool) {
    let v = dark as i32;
    unsafe { _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const _ as _, std::mem::size_of::<i32>() as u32) };
}

/// Cloaked windows keep their swap chain, so un-cloaking shows the next frame
/// (~15 ms) instead of re-creating the surface (~45 ms) like ShowWindow does.
pub fn cloak(hwnd: HWND, on: bool) {
    let v = on as i32;
    unsafe { _ = DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, &v as *const _ as _, 4) };
}

pub fn foreground() -> HWND {
    unsafe { GetForegroundWindow() }
}

/// A full-screen app or game has focus (game mode turns hotkeys off then).
/// ponytail: the hotkey is still registered, so the key never reaches the game; unregister
/// on a foreground-change hook if that matters.
pub fn full_screen_app() -> bool {
    use windows::Win32::UI::Shell::{QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|s| [QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN, QUNS_PRESENTATION_MODE].contains(&s))
}

pub fn focus(hwnd: HWND) {
    unsafe { _ = SetForegroundWindow(hwnd) };
}

/// The monitor the launcher opens on: its work area (physical pixels) and DPI scale.
pub struct Screen {
    pub work: RECT,
    pub dpi: f32,
}

impl Screen {
    /// Work-area width in logical pixels.
    pub fn width(&self) -> f32 {
        (self.work.right - self.work.left) as f32 / self.dpi
    }
}

/// `cursor` (under the mouse), `active` (the focused window's) or `primary`.
pub fn screen(pref: &str) -> Screen {
    unsafe {
        let mut pt = POINT::default();
        let mon = match pref {
            "primary" => MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY),
            "active" => MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTONEAREST),
            _ => {
                let _ = GetCursorPos(&mut pt);
                MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
            }
        };
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Screen { work: info.rcWork, dpi: dx as f32 / 96. }
    }
}

/// Places the window horizontally centred, a quarter down the screen's work area, at
/// `width` × `height` logical pixels.
pub fn place(hwnd: HWND, screen: &Screen, width: f32, height: f32) {
    let (w, h) = ((width * screen.dpi) as i32, (height * screen.dpi) as i32);
    let RECT { left, top, right, bottom } = screen.work;
    let (x, y) = (left + (right - left - w) / 2, top + (bottom - top) / 4);
    unsafe {
        // Crossing onto a monitor with another DPI makes Windows (via GPUI) resize the window
        // to its own suggestion; setting the size again afterwards makes ours stick.
        for _ in 0..2 {
            let mut cur = RECT::default();
            let _ = GetWindowRect(hwnd, &mut cur);
            // Moving/resizing makes GPUI rebuild its surface; skip it when nothing changed.
            if (cur.left, cur.top, cur.right - cur.left, cur.bottom - cur.top) == (x, y, w, h) {
                break;
            }
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
    round_region(hwnd);
}

/// Changes only the window's height (logical px), keeping its top-left corner.
pub fn resize(hwnd: HWND, height: f32) {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let mut cur = RECT::default();
        let _ = GetWindowRect(hwnd, &mut cur);
        let h = (height * dx as f32 / 96.) as i32;
        if cur.bottom - cur.top != h {
            let _ = SetWindowPos(hwnd, None, 0, 0, cur.right - cur.left, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
    round_region(hwnd);
}

// ---- apps ---------------------------------------------------------------------------

/// Lists shell:AppsFolder: Start-menu desktop apps and Store apps in one place.
pub fn scan_apps() -> Vec<App> {
    let mut apps = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(folder) = SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) else {
            return apps;
        };
        let Ok(items) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else {
            return apps;
        };
        loop {
            let mut batch = [None];
            let mut fetched = 0;
            if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = batch[0].take() else { break };
            let (Ok(name), Ok(id)) = (item.GetDisplayName(SIGDN_NORMALDISPLAY), item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING)) else {
                continue;
            };
            let (name_s, id_s) = (name.to_string().unwrap_or_default(), id.to_string().unwrap_or_default());
            CoTaskMemFree(Some(name.0 as _));
            CoTaskMemFree(Some(id.0 as _));
            let lower = name_s.to_lowercase();
            if !name_s.is_empty() && !lower.starts_with("uninstall") && !lower.ends_with("uninstall") {
                apps.push(App { name: name_s, id: id_s, icon: None });
            }
        }
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps.dedup_by(|a, b| a.name == b.name && a.id == b.id);
    apps
}

/// Fills in missing icons. Extraction is ~70 ms per app, so it's split over a few COM
/// threads and done after the names are already searchable.
pub fn load_icons(apps: &mut [App]) {
    let mut missing: Vec<&mut App> = apps.iter_mut().filter(|a| a.icon.is_none()).collect();
    let chunk = missing.len().div_ceil(4).max(1);
    std::thread::scope(|s| {
        for part in missing.chunks_mut(chunk) {
            s.spawn(move || unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                for app in part.iter_mut() {
                    let path = HSTRING::from(if Path::new(&app.id).is_absolute() { app.id.clone() } else { format!("shell:AppsFolder\\{}", app.id) });
                    if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&path, None) {
                        app.icon = icon(&item, ICON_PX);
                    }
                }
            });
        }
    });
}

/// A file's shell icon (programs, documents).
pub fn file_icon(path: &str) -> Option<Arc<gpui::RenderImage>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        icon(&item, ICON_PX)
    }
}

/// Source pixels for list icons: 24 logical px look sharp up to 200% scaling.
const ICON_PX: i32 = 48;

/// The shell's icon for an item, as straight-alpha BGRA (what GPUI's RenderImage wants).
fn icon(item: &IShellItem, px: i32) -> Option<Arc<gpui::RenderImage>> {
    unsafe {
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let bmp = factory.GetImage(SIZE { cx: px, cy: px }, SIIGBF_ICONONLY).ok()?;
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: px,
                biHeight: -px, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (px * px * 4) as usize];
        let dc = GetDC(None);
        let rows = GetDIBits(dc, bmp, 0, px as u32, Some(buf.as_mut_ptr() as _), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        let _ = DeleteObject(bmp.into());
        if rows == 0 {
            return None;
        }
        // Shell bitmaps are premultiplied.
        for p in buf.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a != 0 && a != 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        let frame = image::Frame::new(image::RgbaImage::from_raw(px as u32, px as u32, buf)?);
        Some(Arc::new(gpui::RenderImage::new(vec![frame])))
    }
}

/// The app list with icons, cached so apps are searchable the moment procast starts.
/// Layout: per app, name and id as u32-length strings, then a flag and 48×48 RGBA.
pub fn save_apps(path: &Path, apps: &[App]) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(apps.len() * (ICON_PX * ICON_PX * 4 + 64) as usize);
    for a in apps {
        for s in [&a.name, &a.id] {
            out.extend((s.len() as u32).to_le_bytes());
            out.extend(s.as_bytes());
        }
        match a.icon.as_ref().and_then(|i| i.as_bytes(0)).filter(|b| b.len() == (ICON_PX * ICON_PX * 4) as usize) {
            Some(px) => {
                out.push(1);
                out.extend(px);
            }
            None => out.push(0),
        }
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(tmp, path)
}

/// The cached app list; empty when there's no cache or it doesn't parse.
pub fn load_apps(path: &Path) -> Vec<App> {
    let Ok(data) = std::fs::read(path) else { return vec![] };
    let mut at = 0;
    let take = |at: &mut usize, n: usize| -> Option<&[u8]> {
        let b = data.get(*at..*at + n)?;
        *at += n;
        Some(b)
    };
    let text = |at: &mut usize| -> Option<String> {
        let len = u32::from_le_bytes(take(at, 4)?.try_into().ok()?) as usize;
        String::from_utf8(take(at, len)?.to_vec()).ok()
    };
    let mut apps = vec![];
    while at < data.len() {
        let (Some(name), Some(id)) = (text(&mut at), text(&mut at)) else { return vec![] };
        let icon = match take(&mut at, 1) {
            Some([1]) => {
                let Some(px) = take(&mut at, (ICON_PX * ICON_PX * 4) as usize) else { return vec![] };
                let frame = image::Frame::new(image::RgbaImage::from_raw(ICON_PX as u32, ICON_PX as u32, px.to_vec()).unwrap());
                Some(Arc::new(gpui::RenderImage::new(vec![frame])))
            }
            Some([0]) => None,
            _ => return vec![],
        };
        apps.push(App { name, id, icon });
    }
    apps
}

/// Where installs and uninstalls show up: Start Menu shortcuts and Store packages.
pub fn app_dirs() -> Vec<(PathBuf, bool)> {
    let env = |k: &str| std::env::var(k).map(PathBuf::from).ok();
    let menu = |base: PathBuf| base.join("Microsoft").join("Windows").join("Start Menu").join("Programs");
    [env("APPDATA").map(menu).map(|p| (p, true)), env("ProgramData").map(menu).map(|p| (p, true)), env("LOCALAPPDATA").map(|p| (p.join("Packages"), false))]
        .into_iter()
        .flatten()
        .collect()
}

/// Blocks, calling `on_change` with each batch of paths (relative to `dir`) whose file or
/// folder was created, deleted or renamed.
/// An empty batch means changes were lost (buffer overflow): rescan everything.
/// Returns when the folder can't be watched or `on_change` returns false.
// ponytail: synchronous, so a watcher only notices it should stop at the next change.
pub fn watch_dir(dir: &Path, recursive: bool, mut on_change: impl FnMut(Vec<PathBuf>) -> bool) {
    unsafe {
        let Ok(handle) = CreateFileW(
            &HSTRING::from(dir),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        ) else {
            return;
        };
        // u32 elements keep FILE_NOTIFY_INFORMATION records aligned.
        let mut buf = vec![0u32; 16 * 1024];
        loop {
            let mut got = 0u32;
            let ok = ReadDirectoryChangesW(
                handle,
                buf.as_mut_ptr() as _,
                (buf.len() * 4) as u32,
                recursive,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME,
                Some(&mut got),
                None,
                None,
            );
            if ok.is_err() {
                break;
            }
            let mut changes = vec![];
            let mut at = 0usize;
            while got > 0 {
                let rec = &*((buf.as_ptr() as *const u8).add(at) as *const FILE_NOTIFY_INFORMATION);
                let name = std::slice::from_raw_parts(rec.FileName.as_ptr(), rec.FileNameLength as usize / 2);
                // 1 added, 2 removed, 4 renamed from, 5 renamed to (3, modified, can't occur with these filters).
                if rec.Action.0 != 3 {
                    changes.push(PathBuf::from(String::from_utf16_lossy(name)));
                }
                if rec.NextEntryOffset == 0 {
                    break;
                }
                at += rec.NextEntryOffset as usize;
            }
            if got > 0 && changes.is_empty() {
                continue;
            }
            if !on_change(changes) {
                break;
            }
        }
        let _ = CloseHandle(handle);
    }
}

/// "YYYY-MM-DD", "HH:MM" in local time, for snippet placeholders.
pub fn local_date_time() -> (String, String) {
    let t = unsafe { GetLocalTime() };
    (format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay), format!("{:02}:{:02}", t.wHour, t.wMinute))
}

/// Windows Run: first token is the program, the rest its arguments.
pub fn run(cmdline: &str) {
    let (file, args) = cmdline.split_once(' ').unwrap_or((cmdline, ""));
    shell("open", file.into(), args.into());
}

#[cfg(test)]
#[test]
#[ignore = "reads this machine's Start menu; run with --ignored"]
fn scan_lists_installed_apps() {
    let t = std::time::Instant::now();
    let mut apps = scan_apps();
    let names = t.elapsed();
    load_icons(&mut apps);
    let with_icon = apps.iter().filter(|a| a.icon.is_some()).count();
    println!("{} apps: names in {names:?}, {with_icon} icons by {:?}; e.g. {:?}", apps.len(), t.elapsed(), apps.iter().take(8).map(|a| &a.name).collect::<Vec<_>>());
    assert!(apps.len() > 10);
    assert!(with_icon * 10 >= apps.len() * 9, "at least 90% of apps should have icons");
}

pub fn launch_app(id: &str) {
    // Apps from extra folders are plain paths.
    if Path::new(id).is_absolute() {
        return open(id);
    }
    open(&format!("shell:AppsFolder\\{id}"));
}

/// The program a Start menu entry's shortcut points at (None for Store apps).
pub fn app_target(id: &str) -> Option<String> {
    use windows::Win32::{Storage::EnhancedStorage::PKEY_Link_TargetParsingPath, UI::Shell::IShellItem2};
    if Path::new(id).is_absolute() {
        return Some(id.to_string());
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let item: IShellItem2 = SHCreateItemFromParsingName(&HSTRING::from(format!("shell:AppsFolder\\{id}")), None).ok()?;
        let s = item.GetString(&PKEY_Link_TargetParsingPath).ok()?;
        let out = s.to_string().ok();
        CoTaskMemFree(Some(s.0 as _));
        out.filter(|p| p.to_lowercase().ends_with(".exe"))
    }
}

/// Elevated launch (UAC prompt). Works for desktop apps; Store apps can't be elevated.
pub fn run_as_admin(id: &str) {
    shell("runas", format!("shell:AppsFolder\\{id}").into(), HSTRING::new());
}

/// Runs procast itself elevated with one argument (Windows shows the admin prompt).
pub fn run_self_as_admin(arg: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    shell("runas", exe.as_os_str().into(), arg.into());
}

/// Opens Explorer with the item selected.
pub fn reveal(path: &str) {
    shell("open", "explorer.exe".into(), format!("/select,\"{path}\"").into());
}

/// ShellExecute for paths, URLs and shell: targets.
pub fn open(target: &str) {
    shell("open", target.into(), HSTRING::new());
}

/// ShellExecute on its own thread, letting the started app take focus. It pumps Windows
/// messages while it works; on the UI thread, a gpui timer run from that pump panics on the
/// App borrow held by the calling action, which aborts procast.
fn shell(verb: &'static str, file: HSTRING, args: HSTRING) {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        ShellExecuteW(None, &HSTRING::from(verb), &file, &args, PCWSTR::null(), SW_SHOWNORMAL);
    });
}

// ---- process ------------------------------------------------------------------------

/// False when another procast is already running.
pub fn single_instance() -> bool {
    unsafe {
        let _ = CreateMutexW(None, true, w!("Local\\procast-single-instance"));
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

pub fn set_start_at_login(on: bool) {
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return };
            let value: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, key, w!("procast"), REG_SZ.0, Some(value.as_ptr() as _), (value.len() * 2) as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, key, w!("procast"));
        }
        // The entry the app made under its old name.
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, key, &HSTRING::from(crate::config::OLD_NAME));
    }
}
