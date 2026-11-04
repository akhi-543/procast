//! Built-in commands: system actions and window management.
//! `run` is called off the UI thread: several of these broadcast or send window messages
//! that would re-enter GPUI if issued from inside an update.

use crate::design::Tint;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, RECT, WPARAM},
        Graphics::{
            Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
            Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
        },
        System::{
            Power::SetSuspendState,
            Registry::{HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW},
            Shutdown::{EWX_LOGOFF, ExitWindowsEx, LockWorkStation, SHTDN_REASON_NONE},
        },
        UI::{
            Shell::{SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW},
            WindowsAndMessaging::{
                GetClassNameW, GetWindowRect, HWND_BROADCAST, IsIconic, IsWindow, IsZoomed, SMTO_ABORTIFHUNG, SW_MAXIMIZE, SW_RESTORE,
                SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageTimeoutW, SetWindowPos, ShowWindow,
                WM_SETTINGCHANGE,
            },
        },
    },
    core::{BOOL, PCWSTR, w},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    SearchFiles,
    ClipboardHistory,
    ClearClipboardHistory,
    ClearSearchHistory,
    Emoji,
    Lock,
    Sleep,
    Restart,
    ShutDown,
    SignOut,
    EmptyBin,
    ToggleDarkMode,
    Snap(Snap),
    Settings,
    EditConfig,
    ReloadApps,
    InstallExtension,
    BrowseStore,
    /// Install (true) or remove the admin helper for whole-drive file search.
    FastFileSearch(bool),
    /// List running apps to quit or force-quit.
    QuitApps,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Snap {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    LeftThird,
    CenterThird,
    RightThird,
    FirstTwoThirds,
    LastTwoThirds,
    Maximize,
    AlmostMaximize,
    Center,
    Restore,
    NextDisplay,
}

use crate::design::g;

/// (command, title, extra search words, Fluent icon)
pub const ALL: &[(Cmd, &str, &str, char)] = &[
    (Cmd::SearchFiles, "Search Files", "find documents folders", g::FOLDER),
    (Cmd::ClipboardHistory, "Clipboard History", "paste copy", g::PASTE),
    (Cmd::ClearClipboardHistory, "Clear Clipboard History", "delete forget", g::DELETE),
    (Cmd::ClearSearchHistory, "Clear Search History", "recent queries up arrow forget", g::DELETE),
    (Cmd::Emoji, "Search Emoji", "emoji symbols", g::EMOJI),
    (Cmd::QuitApps, "Quit Applications", "close kill force end task running processes", g::CANCEL),
    (Cmd::Lock, "Lock Screen", "lock", g::LOCK),
    (Cmd::Sleep, "Sleep", "suspend", g::SLEEP),
    (Cmd::Restart, "Restart", "reboot", g::REFRESH),
    (Cmd::ShutDown, "Shut Down", "power off", g::POWER),
    (Cmd::SignOut, "Sign Out", "log out logoff", g::SIGN_OUT),
    (Cmd::EmptyBin, "Empty Recycle Bin", "trash", g::DELETE),
    (Cmd::ToggleDarkMode, "Toggle Dark Mode", "theme appearance light", g::PERSONALIZE),
    (Cmd::Snap(Snap::LeftHalf), "Left Half", "window", g::DOCK_LEFT),
    (Cmd::Snap(Snap::RightHalf), "Right Half", "window", g::DOCK_RIGHT),
    (Cmd::Snap(Snap::TopHalf), "Top Half", "window", g::UP),
    (Cmd::Snap(Snap::BottomHalf), "Bottom Half", "window", g::DOCK_BOTTOM),
    (Cmd::Snap(Snap::LeftThird), "First Third", "window left", g::DOCK_LEFT),
    (Cmd::Snap(Snap::CenterThird), "Center Third", "window middle", g::TILES),
    (Cmd::Snap(Snap::RightThird), "Last Third", "window right", g::DOCK_RIGHT),
    (Cmd::Snap(Snap::FirstTwoThirds), "First Two Thirds", "window left", g::DOCK_LEFT),
    (Cmd::Snap(Snap::LastTwoThirds), "Last Two Thirds", "window right", g::DOCK_RIGHT),
    (Cmd::Snap(Snap::Maximize), "Maximize", "window full", g::FULL_SCREEN),
    (Cmd::Snap(Snap::AlmostMaximize), "Almost Maximize", "window", g::FULL_SCREEN),
    (Cmd::Snap(Snap::Center), "Center Window", "window middle", g::TILES),
    (Cmd::Snap(Snap::Restore), "Restore Window", "window", g::RESTORE),
    (Cmd::Snap(Snap::NextDisplay), "Move to Next Display", "window monitor screen", g::SWITCH_APPS),
    (Cmd::Settings, "akscast Settings", "preferences options configure", g::SETTING),
    (Cmd::EditConfig, "Edit config.toml", "config preferences toml settings file", g::EDIT),
    (Cmd::ReloadApps, "Reload Apps", "rescan refresh", g::REFRESH),
    (Cmd::InstallExtension, "Install Extension", "raycast store add plugin", g::DOWNLOAD),
    (Cmd::BrowseStore, "Browse Extension Store", "raycast store extensions catalog install", g::SHOP),
    (Cmd::FastFileSearch(true), "Enable Fast File Search", "ntfs everything all drives admin index mft", g::SHIELD),
    (Cmd::FastFileSearch(false), "Disable Fast File Search", "ntfs everything all drives admin index mft", g::SHIELD),
    (Cmd::Quit, "Quit akscast", "exit close", g::CANCEL),
];

impl Cmd {
    /// Asks for a second Enter before running.
    pub fn destructive(self) -> bool {
        matches!(self, Cmd::Restart | Cmd::ShutDown | Cmd::SignOut | Cmd::EmptyBin | Cmd::ClearClipboardHistory)
    }

    pub fn key(self) -> String {
        format!("cmd:{self:?}")
    }

    /// The group a command belongs to, shown after its title.
    pub fn subtitle(self) -> &'static str {
        match self {
            Cmd::Snap(_) => "Window",
            Cmd::SearchFiles => "Files",
            Cmd::ClipboardHistory | Cmd::ClearClipboardHistory => "Clipboard",
            Cmd::Emoji => "Emoji",
            Cmd::ClearSearchHistory => "Search",
            Cmd::QuitApps => "Apps",
            Cmd::Settings | Cmd::EditConfig | Cmd::ReloadApps | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::FastFileSearch(_) | Cmd::Quit => "akscast",
            _ => "System",
        }
    }

    /// The hue of the command's icon tile.
    pub fn tint(self) -> Tint {
        match self {
            Cmd::Snap(_) => Tint::Window,
            Cmd::SearchFiles => Tint::File,
            Cmd::ClipboardHistory | Cmd::Emoji => Tint::Paste,
            Cmd::ClearClipboardHistory | Cmd::ClearSearchHistory | Cmd::QuitApps => Tint::Danger,
            Cmd::Settings | Cmd::EditConfig | Cmd::ReloadApps | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::FastFileSearch(_) | Cmd::Quit => Tint::Accent,
            _ => Tint::System,
        }
    }
}

/// Runs a system or window command. `target` is the window that was active before akscast.
pub fn run(cmd: Cmd, target: HWND) {
    unsafe {
        match cmd {
            Cmd::Lock => drop(LockWorkStation()),
            Cmd::Sleep => drop(SetSuspendState(false, false, false)),
            Cmd::Restart => shutdown("/r"),
            Cmd::ShutDown => shutdown("/s"),
            Cmd::SignOut => drop(ExitWindowsEx(EWX_LOGOFF, SHTDN_REASON_NONE)),
            Cmd::EmptyBin => drop(SHEmptyRecycleBinW(None, PCWSTR::null(), SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND)),
            Cmd::ToggleDarkMode => toggle_dark_mode(),
            Cmd::Snap(s) => snap(target, s),
            // Windows asks for admin once; the helper then starts at every logon.
            Cmd::FastFileSearch(on) => crate::win::run_self_as_admin(if on { "--install-indexer" } else { "--uninstall-indexer" }),
            // Handled on the UI thread.
            Cmd::SearchFiles | Cmd::ClipboardHistory | Cmd::ClearClipboardHistory | Cmd::ClearSearchHistory | Cmd::Emoji | Cmd::Settings | Cmd::EditConfig | Cmd::ReloadApps | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::Quit | Cmd::QuitApps => {}
        }
    }
}

fn shutdown(flag: &str) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("shutdown").args([flag, "/t", "0"]).creation_flags(CREATE_NO_WINDOW).spawn();
}

fn toggle_dark_mode() {
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    unsafe {
        let (mut light, mut size) = (1u32, 4u32);
        let _ = RegGetValueW(HKEY_CURRENT_USER, key, w!("AppsUseLightTheme"), RRF_RT_REG_DWORD, None, Some(&mut light as *mut _ as _), Some(&mut size));
        let next: u32 = (light == 0) as u32;
        for name in [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")] {
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, key, name, REG_DWORD.0, Some(&next as *const _ as _), 4);
        }
        let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(w!("ImmersiveColorSet").as_ptr() as isize), SMTO_ABORTIFHUNG, 200, None);
    }
}

fn work_area(mon: HMONITOR) -> RECT {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { _ = GetMonitorInfoW(mon, &mut info) };
    info.rcWork
}

fn monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn add(mon: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        unsafe { (*(list.0 as *mut Vec<HMONITOR>)).push(mon) };
        true.into()
    }
    let mut list: Vec<HMONITOR> = vec![];
    unsafe { _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&mut list as *mut _ as isize)) };
    list
}

/// Target rect for `s` inside work area `wa`, given the window's current rect.
pub fn layout(s: Snap, wa: RECT, cur: RECT) -> RECT {
    let (x, y, w, h) = (wa.left, wa.top, wa.right - wa.left, wa.bottom - wa.top);
    let r = |l: i32, t: i32, rw: i32, rh: i32| RECT { left: l, top: t, right: l + rw, bottom: t + rh };
    let (cw, ch) = (cur.right - cur.left, cur.bottom - cur.top);
    match s {
        Snap::LeftHalf => r(x, y, w / 2, h),
        Snap::RightHalf => r(x + w - w / 2, y, w / 2, h),
        Snap::TopHalf => r(x, y, w, h / 2),
        Snap::BottomHalf => r(x, y + h - h / 2, w, h / 2),
        Snap::LeftThird => r(x, y, w / 3, h),
        Snap::CenterThird => r(x + w / 3, y, w / 3, h),
        Snap::RightThird => r(x + w - w / 3, y, w / 3, h),
        Snap::FirstTwoThirds => r(x, y, w * 2 / 3, h),
        Snap::LastTwoThirds => r(x + w - w * 2 / 3, y, w * 2 / 3, h),
        Snap::AlmostMaximize => r(x + w / 20, y + h / 20, w * 9 / 10, h * 9 / 10),
        Snap::Center => r(x + (w - cw.min(w)) / 2, y + (h - ch.min(h)) / 2, cw.min(w), ch.min(h)),
        Snap::Maximize | Snap::Restore | Snap::NextDisplay => cur,
    }
}

/// The desktop and taskbar: opening akscast from them leaves no window to move.
fn is_shell(hwnd: HWND) -> bool {
    let mut buf = [0u16; 32];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    matches!(String::from_utf16_lossy(&buf[..n]).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

fn snap(hwnd: HWND, s: Snap) {
    unsafe {
        if hwnd.is_invalid() || !IsWindow(Some(hwnd)).as_bool() || is_shell(hwnd) {
            return;
        }
        match s {
            Snap::Maximize | Snap::Restore => {
                let _ = ShowWindow(hwnd, if s == Snap::Maximize { SW_MAXIMIZE } else { SW_RESTORE });
                return;
            }
            _ => {}
        }
        if IsZoomed(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let mut cur = RECT::default();
        let _ = GetWindowRect(hwnd, &mut cur);
        // Windows 10/11 frames have invisible resize borders; size the visible frame instead.
        let mut vis = cur;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut vis as *mut _ as _, size_of::<RECT>() as u32);
        let here = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut t = if s == Snap::NextDisplay {
            let all = monitors();
            let Some(i) = all.iter().position(|m| *m == here) else { return };
            let next = all[(i + 1) % all.len()];
            let (a, b) = (work_area(here), work_area(next));
            let sx = (b.right - b.left) as f32 / (a.right - a.left) as f32;
            let sy = (b.bottom - b.top) as f32 / (a.bottom - a.top) as f32;
            let map = |v: i32, from: i32, to: i32, k: f32| to + ((v - from) as f32 * k) as i32;
            RECT {
                left: map(vis.left, a.left, b.left, sx),
                top: map(vis.top, a.top, b.top, sy),
                right: map(vis.right, a.left, b.left, sx),
                bottom: map(vis.bottom, a.top, b.top, sy),
            }
        } else {
            layout(s, work_area(here), vis)
        };
        t.left -= vis.left - cur.left;
        t.top -= vis.top - cur.top;
        t.right += cur.right - vis.right;
        t.bottom += cur.bottom - vis.bottom;
        let _ = SetWindowPos(hwnd, None, t.left, t.top, t.right - t.left, t.bottom - t.top, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
    }
}
