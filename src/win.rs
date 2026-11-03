//! Everything that talks to Win32 directly.

use std::sync::{Arc, Mutex};

use futures::channel::mpsc::UnboundedSender;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::{
    Win32::{
        Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, POINT, RECT, SIZE},
        Graphics::{
            Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute},
            Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW,
                MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, ReleaseDC,
            },
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
                GetWindowLongPtrW, MSG, PostThreadMessageW, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER,
                SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, WM_APP, WM_HOTKEY, WS_EX_TOOLWINDOW,
            },
        },
    },
    core::{HSTRING, Interface, PCWSTR, w},
};

use crate::config::parse_hotkey;

/// Messages from Win32 threads (hotkeys, tray, app scan, clipboard) to the UI thread.
pub enum Msg {
    Hotkey(Bind),
    HotkeyStatus(Bind, Result<(), String>),
    Apps(Vec<App>),
    Tray(TrayCmd),
    Clip(String),
    /// File search results for one query generation.
    Files(u64, Vec<crate::search::Item>),
    FilesIndexed,
    /// A script finished: (script index, succeeded, output).
    ScriptDone(usize, bool, String),
}

pub enum TrayCmd {
    Open,
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

/// What a global hotkey opens. The discriminant is the RegisterHotKey id.
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
pub struct Hotkeys {
    thread: u32,
    wanted: Arc<Mutex<[String; 4]>>,
}

impl Hotkeys {
    pub fn spawn(specs: [String; 4], tx: UnboundedSender<Msg>) -> Self {
        let wanted = Arc::new(Mutex::new(specs));
        let (tid_tx, tid_rx) = std::sync::mpsc::channel();
        let shared = wanted.clone();
        std::thread::spawn(move || unsafe {
            let _ = tid_tx.send(GetCurrentThreadId());
            let register = || {
                let specs = shared.lock().unwrap().clone();
                for (bind, spec) in BINDS.into_iter().zip(specs) {
                    let id = bind as i32;
                    let _ = UnregisterHotKey(None, id);
                    if spec.trim().is_empty() {
                        continue;
                    }
                    let status = parse_hotkey(&spec).and_then(|(mods, vk)| {
                        RegisterHotKey(None, id, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk)
                            .map_err(|_| format!("hotkey {spec} is already used by another app; change it in config.toml"))
                    });
                    let _ = tx.unbounded_send(Msg::HotkeyStatus(bind, status));
                }
            };
            register();
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                match msg.message {
                    WM_HOTKEY => {
                        if let Some(&bind) = BINDS.iter().find(|b| **b as usize == msg.wParam.0) {
                            let _ = tx.unbounded_send(Msg::Hotkey(bind));
                        }
                    }
                    WM_APP => register(),
                    _ => {}
                }
            }
        });
        Self { thread: tid_rx.recv().unwrap_or(0), wanted }
    }

    pub fn set(&self, specs: [String; 4]) {
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

/// Cloaked windows keep their swap chain, so un-cloaking shows the next frame
/// (~15 ms) instead of re-creating the surface (~45 ms) like ShowWindow does.
pub fn cloak(hwnd: HWND, on: bool) {
    let v = on as i32;
    unsafe { _ = DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, &v as *const _ as _, 4) };
}

pub fn foreground() -> HWND {
    unsafe { GetForegroundWindow() }
}

pub fn focus(hwnd: HWND) {
    unsafe { _ = SetForegroundWindow(hwnd) };
}

/// Places the window horizontally centred, a quarter down the monitor's work area.
/// Physical size follows the monitor's DPI. Returns the monitor handle used.
pub fn place(hwnd: HWND, monitor: &str, width: f32, height: f32) -> isize {
    unsafe {
        let mut pt = POINT::default();
        let mon = if monitor == "primary" {
            MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY)
        } else {
            let _ = GetCursorPos(&mut pt);
            MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
        };
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let scale = dx as f32 / 96.;
        let (w, h) = ((width * scale) as i32, (height * scale) as i32);
        let RECT { left, top, right, bottom } = info.rcWork;
        let x = left + (right - left - w) / 2;
        let y = top + (bottom - top) / 4;
        // Moving/resizing makes GPUI rebuild its surface; skip it when nothing changed.
        let mut cur = RECT::default();
        let _ = GetWindowRect(hwnd, &mut cur);
        if (cur.left, cur.top, cur.right - cur.left, cur.bottom - cur.top) != (x, y, w, h) {
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        mon.0 as isize
    }
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

/// Fills in icons. Extraction is ~70 ms per app, so it's split over a few COM threads
/// and done after the names are already searchable.
pub fn load_icons(apps: &mut [App]) {
    let chunk = apps.len().div_ceil(4).max(1);
    std::thread::scope(|s| {
        for part in apps.chunks_mut(chunk) {
            s.spawn(move || unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                for app in part {
                    let path = HSTRING::from(format!("shell:AppsFolder\\{}", app.id));
                    if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&path, None) {
                        app.icon = icon(&item, ICON_PX);
                    }
                }
            });
        }
    });
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

/// "YYYY-MM-DD", "HH:MM" in local time, for snippet placeholders.
pub fn local_date_time() -> (String, String) {
    let t = unsafe { GetLocalTime() };
    (format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay), format!("{:02}:{:02}", t.wHour, t.wMinute))
}

/// Windows Run: first token is the program, the rest its arguments.
pub fn run(cmdline: &str) {
    let (file, args) = cmdline.split_once(' ').unwrap_or((cmdline, ""));
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        ShellExecuteW(None, w!("open"), &HSTRING::from(file), &HSTRING::from(args), PCWSTR::null(), SW_SHOWNORMAL);
    }
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
    open(&format!("shell:AppsFolder\\{id}"));
}

/// Elevated launch (UAC prompt). Works for desktop apps; Store apps can't be elevated.
pub fn run_as_admin(id: &str) {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        ShellExecuteW(None, w!("runas"), &HSTRING::from(format!("shell:AppsFolder\\{id}")), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Opens Explorer with the item selected.
pub fn reveal(path: &str) {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        let args = HSTRING::from(format!("/select,\"{path}\""));
        ShellExecuteW(None, w!("open"), w!("explorer.exe"), &args, PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// ShellExecute for paths, URLs and shell: targets. Lets the started app take focus.
pub fn open(target: &str) {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        ShellExecuteW(None, w!("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

// ---- process ------------------------------------------------------------------------

/// False when another akscast is already running.
pub fn single_instance() -> bool {
    unsafe {
        let _ = CreateMutexW(None, true, w!("Local\\akscast-single-instance"));
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

pub fn set_start_at_login(on: bool) {
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return };
            let value: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, key, w!("akscast"), REG_SZ.0, Some(value.as_ptr() as _), (value.len() * 2) as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, key, w!("akscast"));
        }
    }
}
