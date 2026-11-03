//! Everything that talks to Win32 directly.

use std::sync::{Arc, Mutex};

use futures::channel::mpsc::UnboundedSender;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::{
    Win32::{
        Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, POINT, RECT},
        Graphics::{
            Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute},
            Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint},
        },
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree},
            Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW},
            Threading::{CreateMutexW, GetCurrentThreadId},
        },
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey},
            Shell::{
                BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, KF_FLAG_DEFAULT, SHGetKnownFolderItem,
                SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING, ShellExecuteW,
            },
            WindowsAndMessaging::{
                ASFW_ANY, AllowSetForegroundWindow, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetMessageW,
                GetWindowLongPtrW, MSG, PostThreadMessageW, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER,
                SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, WM_APP, WM_HOTKEY, WS_EX_TOOLWINDOW,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::config::parse_hotkey;

/// Messages from Win32 threads (hotkey, tray, app scan) to the UI thread.
pub enum Msg {
    Hotkey,
    HotkeyStatus(Result<(), String>),
    Apps(Vec<App>),
    Tray(TrayCmd),
}

pub enum TrayCmd {
    Open,
    EditConfig,
    Quit,
}

#[derive(Clone, Debug)]
pub struct App {
    pub name: String,
    /// Parsing name inside shell:AppsFolder (an AUMID or a path/known-folder id).
    pub id: String,
}

// ---- hotkey -------------------------------------------------------------------------

/// Owns RegisterHotKey on a dedicated thread with its own message loop.
/// `set` re-registers from the shared string; failures are reported, never fatal.
pub struct Hotkey {
    thread: u32,
    wanted: Arc<Mutex<String>>,
}

impl Hotkey {
    pub fn spawn(hotkey: String, tx: UnboundedSender<Msg>) -> Self {
        let wanted = Arc::new(Mutex::new(hotkey));
        let (tid_tx, tid_rx) = std::sync::mpsc::channel();
        let shared = wanted.clone();
        std::thread::spawn(move || unsafe {
            let _ = tid_tx.send(GetCurrentThreadId());
            let register = || {
                let _ = UnregisterHotKey(None, 1);
                let spec = shared.lock().unwrap().clone();
                let status = parse_hotkey(&spec).and_then(|(mods, vk)| {
                    RegisterHotKey(None, 1, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk)
                        .map_err(|_| format!("hotkey {spec} is already used by another app; change it in config.toml"))
                });
                let _ = tx.unbounded_send(Msg::HotkeyStatus(status));
            };
            register();
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                match msg.message {
                    WM_HOTKEY => drop(tx.unbounded_send(Msg::Hotkey)),
                    WM_APP => register(),
                    _ => {}
                }
            }
        });
        Self { thread: tid_rx.recv().unwrap_or(0), wanted }
    }

    pub fn set(&self, hotkey: &str) {
        *self.wanted.lock().unwrap() = hotkey.to_string();
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
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
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
            let app = App { name: name.to_string().unwrap_or_default(), id: id.to_string().unwrap_or_default() };
            CoTaskMemFree(Some(name.0 as _));
            CoTaskMemFree(Some(id.0 as _));
            let lower = app.name.to_lowercase();
            if !app.name.is_empty() && !lower.starts_with("uninstall") && !lower.ends_with("uninstall") {
                apps.push(app);
            }
        }
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps.dedup_by(|a, b| a.name == b.name && a.id == b.id);
    apps
}

#[cfg(test)]
#[test]
#[ignore = "reads this machine's Start menu; run with --ignored"]
fn scan_lists_installed_apps() {
    let t = std::time::Instant::now();
    let apps = scan_apps();
    println!("{} apps in {:?}; e.g. {:?}", apps.len(), t.elapsed(), apps.iter().take(8).map(|a| &a.name).collect::<Vec<_>>());
    assert!(apps.len() > 10);
}

pub fn launch_app(id: &str) {
    open(&format!("shell:AppsFolder\\{id}"));
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
