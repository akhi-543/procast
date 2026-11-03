//! Built-in commands: system actions and window management.
//! `run` is called off the UI thread: several of these broadcast or send window messages
//! that would re-enter GPUI if issued from inside an update.

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
                GetWindowRect, HWND_BROADCAST, IsIconic, IsWindow, IsZoomed, SMTO_ABORTIFHUNG, SW_MAXIMIZE, SW_RESTORE,
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

/// (command, title, extra search words, glyph)
pub const ALL: &[(Cmd, &str, &str, &str)] = &[
    (Cmd::SearchFiles, "Search Files", "find documents folders", "📂"),
    (Cmd::ClipboardHistory, "Clipboard History", "paste copy", "📋"),
    (Cmd::ClearClipboardHistory, "Clear Clipboard History", "delete forget", "🧹"),
    (Cmd::Emoji, "Search Emoji", "emoji symbols", "😀"),
    (Cmd::Lock, "Lock Screen", "lock", "🔒"),
    (Cmd::Sleep, "Sleep", "suspend", "🌙"),
    (Cmd::Restart, "Restart", "reboot", "🔄"),
    (Cmd::ShutDown, "Shut Down", "power off", "⏻"),
    (Cmd::SignOut, "Sign Out", "log out logoff", "🚪"),
    (Cmd::EmptyBin, "Empty Recycle Bin", "trash", "🗑"),
    (Cmd::ToggleDarkMode, "Toggle Dark Mode", "theme appearance light", "🌓"),
    (Cmd::Snap(Snap::LeftHalf), "Left Half", "window", "◧"),
    (Cmd::Snap(Snap::RightHalf), "Right Half", "window", "◨"),
    (Cmd::Snap(Snap::TopHalf), "Top Half", "window", "⬒"),
    (Cmd::Snap(Snap::BottomHalf), "Bottom Half", "window", "⬓"),
    (Cmd::Snap(Snap::LeftThird), "First Third", "window left", "▏"),
    (Cmd::Snap(Snap::CenterThird), "Center Third", "window middle", "▕"),
    (Cmd::Snap(Snap::RightThird), "Last Third", "window right", "▕"),
    (Cmd::Snap(Snap::FirstTwoThirds), "First Two Thirds", "window left", "◧"),
    (Cmd::Snap(Snap::LastTwoThirds), "Last Two Thirds", "window right", "◨"),
    (Cmd::Snap(Snap::Maximize), "Maximize", "window full", "⬜"),
    (Cmd::Snap(Snap::AlmostMaximize), "Almost Maximize", "window", "▣"),
    (Cmd::Snap(Snap::Center), "Center Window", "window middle", "⊡"),
    (Cmd::Snap(Snap::Restore), "Restore Window", "window", "↩"),
    (Cmd::Snap(Snap::NextDisplay), "Move to Next Display", "window monitor screen", "🖥"),
    (Cmd::Settings, "akscast Settings", "preferences options configure", "⚙"),
    (Cmd::EditConfig, "Edit config.toml", "config preferences toml settings file", "📝"),
    (Cmd::ReloadApps, "Reload Apps", "rescan refresh", "↻"),
    (Cmd::InstallExtension, "Install Extension", "raycast store add plugin", "🧩"),
    (Cmd::BrowseStore, "Browse Extension Store", "raycast store extensions catalog install", "🛍"),
    (Cmd::Quit, "Quit akscast", "exit close", "✕"),
];

impl Cmd {
    /// Asks for a second Enter before running.
    pub fn destructive(self) -> bool {
        matches!(self, Cmd::Restart | Cmd::ShutDown | Cmd::SignOut | Cmd::EmptyBin | Cmd::ClearClipboardHistory)
    }

    pub fn key(self) -> String {
        format!("cmd:{self:?}")
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Cmd::Snap(_) => "Window Management",
            Cmd::SearchFiles | Cmd::ClipboardHistory | Cmd::ClearClipboardHistory | Cmd::Emoji => "Command",
            Cmd::Settings | Cmd::EditConfig | Cmd::ReloadApps | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::Quit => "akscast",
            _ => "System",
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
            // Handled on the UI thread.
            Cmd::SearchFiles | Cmd::ClipboardHistory | Cmd::ClearClipboardHistory | Cmd::Emoji | Cmd::Settings | Cmd::EditConfig | Cmd::ReloadApps | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::Quit => {}
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

fn snap(hwnd: HWND, s: Snap) {
    unsafe {
        if hwnd.is_invalid() || !IsWindow(Some(hwnd)).as_bool() {
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
