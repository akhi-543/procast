//! Built-in commands: system actions and window management.
//! `run` is called off the UI thread: several of these broadcast or send window messages
//! that would re-enter GPUI if issued from inside an update.

use crate::{
    config,
    design::Tint,
    desktops,
    system::{self, RadioOf, Volume},
    ytmusic::{Control, Screen},
};
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
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            Shell::{SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW},
            WindowsAndMessaging::{
                GetClassNameW, GetWindowRect, HWND_BROADCAST, IsIconic, IsWindow, IsZoomed, PostMessageW, SMTO_ABORTIFHUNG, SW_MAXIMIZE,
                SW_MINIMIZE, SW_RESTORE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageTimeoutW, SetWindowPos,
                ShowWindow, WM_CLOSE, WM_SETTINGCHANGE,
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
    Volume(Volume),
    Radio(RadioOf),
    ToggleHiddenFiles,
    RestartExplorer,
    EjectDrives,
    EmptyTemp,
    Snap(Snap),
    Minimize,
    /// Minimize every window except the one you came from.
    MinimizeOthers,
    /// Ask every window except the one you came from to close.
    CloseOthers,
    ToggleMaximize,
    Settings,
    EditConfig,
    ReloadApps,
    InstallExtension,
    BrowseStore,
    /// Install (true) or remove the admin helper for whole-drive file search.
    FastFileSearch(bool),
    /// List running apps to quit or force-quit.
    QuitApps,
    /// Open the Quick AI view to type a question.
    AskAi,
    /// List the tabs open in browsers with the procast extension.
    SearchTabs,
    /// Show the browser extension's folder, to load it in the browser.
    BrowserSetup,
    /// Open a YouTube Music screen.
    YtMusic(Screen),
    /// Control YouTube Music playback, from anywhere.
    YtControl(Control),
    Desktop(Desk),
    Quit,
}

/// A virtual desktop command. Desktop numbers are 1-based (1 to 10).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Desk {
    /// Switch to the desktop.
    Open(u8),
    /// Move the window there, without switching.
    MoveTo(u8),
    MoveNext,
    MovePrev,
    /// Close the current desktop; its windows go to the one on the left.
    Close,
    /// Needs a name: the launcher asks for it, then calls `desktops::rename`.
    Rename,
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
    TopLeftQuarter,
    TopRightQuarter,
    BottomLeftQuarter,
    BottomRightQuarter,
    TopLeftSixth,
    TopCenterSixth,
    TopRightSixth,
    BottomLeftSixth,
    BottomCenterSixth,
    BottomRightSixth,
    TopThird,
    MiddleThird,
    BottomThird,
    ReasonableSize,
}

use crate::design::g;

/// (command, title, extra search words, icon name)
pub const ALL: &[(Cmd, &str, &str, &str)] = &[
    (Cmd::SearchFiles, "Search Files", "find documents folders", g::FOLDER_SEARCH),
    (Cmd::ClipboardHistory, "Clipboard History", "paste copy", g::PASTE),
    (Cmd::ClearClipboardHistory, "Clear Clipboard History", "delete forget", g::CLIPBOARD_X),
    (Cmd::ClearSearchHistory, "Clear Search History", "recent queries up arrow forget", g::ERASER),
    (Cmd::Emoji, "Search Emoji", "emoji symbols", g::EMOJI),
    (Cmd::QuitApps, "Quit Applications", "close kill force end task running processes", g::OCTAGON_X),
    (Cmd::AskAi, "Ask AI", "chat question claude assistant", g::SPARKLES),
    (Cmd::SearchTabs, "Search Browser Tabs", "chrome edge brave open tabs switch web pages", g::PANELS_TOP_LEFT),
    (Cmd::BrowserSetup, "Set Up Browser Extension", "chrome edge brave install companion tabs", g::PUZZLE),
    (Cmd::YtMusic(Screen::Search), "Search YouTube Music", "youtube music song album artist playlist find", g::SEARCH),
    (Cmd::YtMusic(Screen::Library), "YouTube Music Library", "youtube music liked songs albums playlists artists", g::BULLETS),
    (Cmd::YtMusic(Screen::NowPlaying), "YouTube Music Now Playing", "youtube music current track song playing", g::PLAY),
    (Cmd::YtMusic(Screen::QuickActions), "YouTube Music Quick Actions", "youtube music controls volume like shuffle repeat", g::SLIDERS_HORIZONTAL),
    (Cmd::YtControl(Control::PlayPause), "Play/Pause Music", "youtube music song resume stop toggle", g::PLAY),
    (Cmd::YtControl(Control::Next), "Next Track", "youtube music song skip forward", g::FORWARD),
    (Cmd::YtControl(Control::Previous), "Previous Track", "youtube music song skip back", g::BACK),
    (Cmd::YtControl(Control::Like), "Like Current Song", "youtube music song favorite thumbs up", g::STAR),
    (Cmd::YtControl(Control::Dislike), "Dislike Current Song", "youtube music song thumbs down", g::CANCEL),
    (Cmd::YtControl(Control::Mute), "Mute Music", "youtube music song volume silence", g::VOLUME_X),
    (Cmd::YtControl(Control::VolumeLow), "Music Volume Low", "youtube music song quiet sound", g::VOLUME_1),
    (Cmd::YtControl(Control::VolumeMedium), "Music Volume Medium", "youtube music song sound", g::VOLUME_2),
    (Cmd::YtControl(Control::VolumeHigh), "Music Volume High", "youtube music song loud sound", g::VOLUME_2),
    (Cmd::YtControl(Control::VolumeUp), "Music Volume Up", "youtube music song louder sound", g::VOLUME_2),
    (Cmd::YtControl(Control::VolumeDown), "Music Volume Down", "youtube music song quieter sound", g::VOLUME_1),
    (Cmd::YtControl(Control::Shuffle), "Shuffle Music", "youtube music song queue random", g::REFRESH_CCW),
    (Cmd::YtControl(Control::Repeat), "Repeat Mode", "youtube music song loop one all", g::ROTATE_CW),
    (Cmd::Lock, "Lock Screen", "lock", g::LOCK),
    (Cmd::Sleep, "Sleep", "suspend", g::SLEEP),
    (Cmd::Restart, "Restart", "reboot", g::ROTATE_CW),
    (Cmd::ShutDown, "Shut Down", "power off", g::POWER),
    (Cmd::SignOut, "Sign Out", "log out logoff", g::SIGN_OUT),
    (Cmd::EmptyBin, "Empty Recycle Bin", "trash", g::DELETE),
    (Cmd::ToggleDarkMode, "Toggle Dark Mode", "theme appearance light", g::SUN_MOON),
    (Cmd::Volume(Volume::Up), "Volume Up", "sound louder audio", g::VOLUME_2),
    (Cmd::Volume(Volume::Down), "Volume Down", "sound quieter audio", g::VOLUME_1),
    (Cmd::Volume(Volume::Mute), "Toggle Mute", "sound volume silence audio unmute", g::VOLUME_X),
    (Cmd::Radio(RadioOf::WiFi), "Toggle Wi-Fi", "wireless network wlan on off", g::WIFI),
    (Cmd::Radio(RadioOf::Bluetooth), "Toggle Bluetooth", "wireless bt on off", g::BLUETOOTH),
    (Cmd::ToggleHiddenFiles, "Toggle Hidden Files", "explorer show hide hidden files folders", g::EYE_OFF),
    (Cmd::RestartExplorer, "Restart Explorer", "taskbar shell explorer.exe reload", g::REFRESH),
    (Cmd::EjectDrives, "Eject USB Drives", "eject safely remove usb stick disk drive unmount", g::USB),
    (Cmd::EmptyTemp, "Empty Temp Folder", "clean temporary files cleanup disk space", g::BRUSH),
    (Cmd::Snap(Snap::LeftHalf), "Left Half", "window", g::WIN_LEFT_HALF),
    (Cmd::Snap(Snap::RightHalf), "Right Half", "window", g::WIN_RIGHT_HALF),
    (Cmd::Snap(Snap::TopHalf), "Top Half", "window", g::WIN_TOP_HALF),
    (Cmd::Snap(Snap::BottomHalf), "Bottom Half", "window", g::WIN_BOTTOM_HALF),
    (Cmd::Snap(Snap::LeftThird), "First Third", "window left", g::WIN_FIRST_THIRD),
    (Cmd::Snap(Snap::CenterThird), "Center Third", "window middle", g::WIN_CENTER_THIRD),
    (Cmd::Snap(Snap::RightThird), "Last Third", "window right", g::WIN_LAST_THIRD),
    (Cmd::Snap(Snap::FirstTwoThirds), "First Two Thirds", "window left", g::WIN_FIRST_TWO_THIRDS),
    (Cmd::Snap(Snap::LastTwoThirds), "Last Two Thirds", "window right", g::WIN_LAST_TWO_THIRDS),
    (Cmd::Snap(Snap::TopLeftQuarter), "Top Left Quarter", "window", g::WIN_TOP_LEFT_QUARTER),
    (Cmd::Snap(Snap::TopRightQuarter), "Top Right Quarter", "window", g::WIN_TOP_RIGHT_QUARTER),
    (Cmd::Snap(Snap::BottomLeftQuarter), "Bottom Left Quarter", "window", g::WIN_BOTTOM_LEFT_QUARTER),
    (Cmd::Snap(Snap::BottomRightQuarter), "Bottom Right Quarter", "window", g::WIN_BOTTOM_RIGHT_QUARTER),
    (Cmd::Snap(Snap::TopLeftSixth), "Top Left Sixth", "window", g::WIN_TOP_LEFT_SIXTH),
    (Cmd::Snap(Snap::TopCenterSixth), "Top Center Sixth", "window", g::WIN_TOP_CENTER_SIXTH),
    (Cmd::Snap(Snap::TopRightSixth), "Top Right Sixth", "window", g::WIN_TOP_RIGHT_SIXTH),
    (Cmd::Snap(Snap::BottomLeftSixth), "Bottom Left Sixth", "window", g::WIN_BOTTOM_LEFT_SIXTH),
    (Cmd::Snap(Snap::BottomCenterSixth), "Bottom Center Sixth", "window", g::WIN_BOTTOM_CENTER_SIXTH),
    (Cmd::Snap(Snap::BottomRightSixth), "Bottom Right Sixth", "window", g::WIN_BOTTOM_RIGHT_SIXTH),
    (Cmd::Snap(Snap::TopThird), "Top Third", "window", g::WIN_TOP_THIRD),
    (Cmd::Snap(Snap::MiddleThird), "Middle Third", "window", g::WIN_MIDDLE_THIRD),
    (Cmd::Snap(Snap::BottomThird), "Bottom Third", "window", g::WIN_BOTTOM_THIRD),
    (Cmd::Snap(Snap::ReasonableSize), "Reasonable Size", "window", g::WIN_REASONABLE),
    (Cmd::Snap(Snap::Maximize), "Maximize", "window full", g::WIN_MAXIMIZE),
    (Cmd::Snap(Snap::AlmostMaximize), "Almost Maximize", "window", g::WIN_ALMOST_MAXIMIZE),
    (Cmd::Snap(Snap::Center), "Center Window", "window middle", g::WIN_CENTER),
    (Cmd::Snap(Snap::Restore), "Restore Window", "window", g::WIN_RESTORE),
    (Cmd::Snap(Snap::NextDisplay), "Move to Next Display", "window monitor screen", g::WIN_NEXT_DISPLAY),
    (Cmd::Minimize, "Minimize Window", "window hide", g::WIN_MINIMIZE),
    (Cmd::MinimizeOthers, "Minimize Other Windows", "window hide", g::WIN_MINIMIZE_OTHERS),
    (Cmd::CloseOthers, "Close Other Windows", "window quit", g::WIN_CLOSE_OTHERS),
    (Cmd::ToggleMaximize, "Toggle Maximize", "window full restore", g::WIN_TOGGLE_MAXIMIZE),
    (Cmd::Desktop(Desk::Open(1)), "Open Desktop 1", "virtual desktop switch", g::DESK_OPEN),
    (Cmd::Desktop(Desk::Open(2)), "Open Desktop 2", "virtual desktop switch", g::DESK_OPEN),
    (Cmd::Desktop(Desk::Open(3)), "Open Desktop 3", "virtual desktop switch", g::DESK_OPEN),
    (Cmd::Desktop(Desk::Open(4)), "Open Desktop 4", "virtual desktop switch", g::DESK_OPEN),
    (Cmd::Desktop(Desk::Open(5)), "Open Desktop 5", "virtual desktop switch", g::DESK_OPEN),
    (Cmd::Desktop(Desk::Open(6)), "Open Desktop 6", "virtual desktop switch", g::DESK_OPEN),
    (Cmd::Desktop(Desk::Open(7)), "Open Desktop 7", "virtual desktop switch", g::DESK_OPEN),
    (Cmd::Desktop(Desk::Open(8)), "Open Desktop 8", "virtual desktop switch", g::DESK_OPEN),
    (Cmd::Desktop(Desk::Open(9)), "Open Desktop 9", "virtual desktop switch", g::DESK_OPEN),
    (Cmd::Desktop(Desk::Open(10)), "Open Desktop 10", "virtual desktop switch", g::DESK_OPEN),
    (Cmd::Desktop(Desk::MoveTo(1)), "Move to Desktop 1", "virtual desktop window", g::DESK_MOVE_TO),
    (Cmd::Desktop(Desk::MoveTo(2)), "Move to Desktop 2", "virtual desktop window", g::DESK_MOVE_TO),
    (Cmd::Desktop(Desk::MoveTo(3)), "Move to Desktop 3", "virtual desktop window", g::DESK_MOVE_TO),
    (Cmd::Desktop(Desk::MoveTo(4)), "Move to Desktop 4", "virtual desktop window", g::DESK_MOVE_TO),
    (Cmd::Desktop(Desk::MoveTo(5)), "Move to Desktop 5", "virtual desktop window", g::DESK_MOVE_TO),
    (Cmd::Desktop(Desk::MoveTo(6)), "Move to Desktop 6", "virtual desktop window", g::DESK_MOVE_TO),
    (Cmd::Desktop(Desk::MoveTo(7)), "Move to Desktop 7", "virtual desktop window", g::DESK_MOVE_TO),
    (Cmd::Desktop(Desk::MoveTo(8)), "Move to Desktop 8", "virtual desktop window", g::DESK_MOVE_TO),
    (Cmd::Desktop(Desk::MoveTo(9)), "Move to Desktop 9", "virtual desktop window", g::DESK_MOVE_TO),
    (Cmd::Desktop(Desk::MoveTo(10)), "Move to Desktop 10", "virtual desktop window", g::DESK_MOVE_TO),
    (Cmd::Desktop(Desk::MoveNext), "Move to Next Desktop", "virtual desktop window", g::DESK_NEXT),
    (Cmd::Desktop(Desk::MovePrev), "Move to Previous Desktop", "virtual desktop window", g::DESK_PREV),
    (Cmd::Desktop(Desk::Close), "Close Desktop", "virtual desktop remove", g::DESK_CLOSE),
    (Cmd::Desktop(Desk::Rename), "Rename Desktop", "virtual desktop name", g::DESK_RENAME),
    (Cmd::Settings, "procast Settings", "preferences options configure", g::SETTING),
    (Cmd::EditConfig, "Edit config.toml", "config preferences toml settings file", g::FILE_COG),
    (Cmd::ReloadApps, "Reload Apps", "rescan refresh", g::REFRESH_CCW),
    (Cmd::InstallExtension, "Install Extension", "raycast store add plugin", g::PACKAGE_PLUS),
    (Cmd::BrowseStore, "Browse Extension Store", "raycast store extensions catalog install", g::SHOP),
    (Cmd::FastFileSearch(true), "Enable Fast File Search", "ntfs everything all drives admin index mft", g::ZAP),
    (Cmd::FastFileSearch(false), "Disable Fast File Search", "ntfs everything all drives admin index mft", g::ZAP_OFF),
    (Cmd::Quit, "Quit procast", "exit close", g::POWER_OFF),
];

impl Cmd {
    /// Asks for a second Enter before running.
    pub fn destructive(self) -> bool {
        matches!(self, Cmd::Restart | Cmd::ShutDown | Cmd::SignOut | Cmd::EmptyBin | Cmd::EmptyTemp | Cmd::RestartExplorer | Cmd::ClearClipboardHistory | Cmd::ClearSearchHistory | Cmd::CloseOthers | Cmd::Desktop(Desk::Close))
    }

    pub fn key(self) -> String {
        match self {
            Cmd::YtMusic(screen) => format!("ytm:{}", screen.slug()),
            Cmd::YtControl(control) => format!("ytm:{}", control.slug()),
            _ => format!("cmd:{self:?}"),
        }
    }

    /// The group a command belongs to, shown after its title.
    pub fn subtitle(self) -> &'static str {
        match self {
            Cmd::Snap(_) | Cmd::Minimize | Cmd::MinimizeOthers | Cmd::CloseOthers | Cmd::ToggleMaximize | Cmd::Desktop(_) => "Window",
            Cmd::SearchFiles => "Files",
            Cmd::ClipboardHistory | Cmd::ClearClipboardHistory => "Clipboard",
            Cmd::Emoji => "Emoji",
            Cmd::ClearSearchHistory => "Search",
            Cmd::QuitApps => "Apps",
            Cmd::SearchTabs | Cmd::BrowserSetup => "Browser",
            Cmd::YtMusic(_) | Cmd::YtControl(_) => "YouTube Music",
            Cmd::Settings | Cmd::EditConfig | Cmd::ReloadApps | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::FastFileSearch(_) | Cmd::Quit => "procast",
            _ => "System",
        }
    }

    /// The hue of the command's icon tile.
    pub fn tint(self) -> Tint {
        match self {
            Cmd::Snap(_) | Cmd::Minimize | Cmd::MinimizeOthers | Cmd::CloseOthers | Cmd::ToggleMaximize | Cmd::Desktop(_) => Tint::Window,
            Cmd::SearchFiles => Tint::File,
            Cmd::ClipboardHistory | Cmd::Emoji => Tint::Paste,
            Cmd::SearchTabs | Cmd::AskAi | Cmd::YtMusic(_) | Cmd::YtControl(_) => Tint::Web,
            Cmd::ClearClipboardHistory | Cmd::ClearSearchHistory | Cmd::EmptyBin | Cmd::EmptyTemp | Cmd::QuitApps | Cmd::Quit => Tint::Danger,
            Cmd::Settings | Cmd::EditConfig | Cmd::ReloadApps | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::BrowserSetup | Cmd::FastFileSearch(_) => Tint::Accent,
            _ => Tint::System,
        }
    }
}

/// Runs a system or window command. `target` is the window that was active before procast.
/// Some say how it went ("Wi-Fi off"), for a toast.
pub fn run(cmd: Cmd, target: HWND, w: &config::Window) -> Result<Option<String>, String> {
    unsafe {
        match cmd {
            Cmd::Volume(v) => system::volume(v),
            Cmd::Radio(r) => return system::toggle_radio(r).map(Some),
            Cmd::ToggleHiddenFiles => return Ok(Some(if system::toggle_hidden_files() { "Hidden files shown" } else { "Hidden files hidden" }.into())),
            Cmd::RestartExplorer => return system::restart_explorer().map(|_| None),
            Cmd::EjectDrives => return system::eject_drives().map(Some),
            Cmd::EmptyTemp => return Ok(Some(system::empty_temp())),
            Cmd::Lock => drop(LockWorkStation()),
            Cmd::Sleep => drop(SetSuspendState(false, false, false)),
            Cmd::Restart => shutdown("/r"),
            Cmd::ShutDown => shutdown("/s"),
            Cmd::SignOut => drop(ExitWindowsEx(EWX_LOGOFF, SHTDN_REASON_NONE)),
            Cmd::EmptyBin => drop(SHEmptyRecycleBinW(None, PCWSTR::null(), SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND)),
            Cmd::ToggleDarkMode => toggle_dark_mode(),
            Cmd::Snap(s) => snap(target, s, w),
            Cmd::Desktop(Desk::MoveTo(_) | Desk::MoveNext | Desk::MovePrev) if !IsWindow(Some(target)).as_bool() || is_shell(target) => return Err("No window to act on".into()),
            Cmd::Desktop(d) => return desktop(d, target).map(|_| None),
            Cmd::Minimize | Cmd::MinimizeOthers | Cmd::CloseOthers | Cmd::ToggleMaximize if !IsWindow(Some(target)).as_bool() || is_shell(target) => return Err("No window to act on".into()),
            Cmd::Minimize => _ = ShowWindow(target, SW_MINIMIZE),
            Cmd::ToggleMaximize => _ = ShowWindow(target, if IsZoomed(target).as_bool() { SW_RESTORE } else { SW_MAXIMIZE }),
            Cmd::MinimizeOthers => others(target).into_iter().for_each(|h| _ = ShowWindow(h, SW_MINIMIZE)),
            // The windows get to ask about unsaved work, so some may stay open.
            Cmd::CloseOthers => others(target).into_iter().for_each(|h| drop(PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0)))),
            // Windows asks for admin once; the helper then starts at every logon.
            Cmd::FastFileSearch(on) => crate::win::run_self_as_admin(if on { "--install-indexer" } else { "--uninstall-indexer" }),
            // Handled on the UI thread.
            Cmd::SearchFiles | Cmd::ClipboardHistory | Cmd::ClearClipboardHistory | Cmd::ClearSearchHistory | Cmd::Emoji | Cmd::Settings | Cmd::EditConfig | Cmd::ReloadApps | Cmd::InstallExtension | Cmd::BrowseStore | Cmd::Quit | Cmd::QuitApps | Cmd::AskAi | Cmd::SearchTabs | Cmd::BrowserSetup | Cmd::YtMusic(_) | Cmd::YtControl(_) => {}
        }
    }
    Ok(None)
}

fn desktop(d: Desk, target: HWND) -> Result<(), String> {
    match d {
        // The launcher hid without handing focus back (see desktops::open); no switch, no new
        // focus, so give it back here.
        Desk::Open(n) => desktops::open(n).inspect_err(|_| crate::win::focus(target)),
        Desk::MoveTo(n) => desktops::move_window(target, n),
        Desk::MoveNext => desktops::move_next(target),
        Desk::MovePrev => desktops::move_prev(target),
        Desk::Close => desktops::close(),
        // Asked for on the UI thread, which then calls `desktops::rename`.
        Desk::Rename => Ok(()),
    }
}

/// Every Alt+Tab window except `target` and procast's own windows (Settings and the like).
fn others(target: HWND) -> Vec<HWND> {
    others_of(&crate::apps::app_windows(), target, std::process::id())
}

fn others_of(windows: &[(HWND, u32)], target: HWND, me: u32) -> Vec<HWND> {
    windows.iter().filter(|&&(h, pid)| h != target && pid != me).map(|&(h, _)| h).collect()
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
        Snap::TopLeftQuarter => r(x, y, w / 2, h / 2),
        Snap::TopRightQuarter => r(x + w - w / 2, y, w / 2, h / 2),
        Snap::BottomLeftQuarter => r(x, y + h - h / 2, w / 2, h / 2),
        Snap::BottomRightQuarter => r(x + w - w / 2, y + h - h / 2, w / 2, h / 2),
        Snap::TopLeftSixth => r(x, y, w / 3, h / 2),
        Snap::TopCenterSixth => r(x + w / 3, y, w / 3, h / 2),
        Snap::TopRightSixth => r(x + w - w / 3, y, w / 3, h / 2),
        Snap::BottomLeftSixth => r(x, y + h - h / 2, w / 3, h / 2),
        Snap::BottomCenterSixth => r(x + w / 3, y + h - h / 2, w / 3, h / 2),
        Snap::BottomRightSixth => r(x + w - w / 3, y + h - h / 2, w / 3, h / 2),
        Snap::TopThird => r(x, y, w, h / 3),
        Snap::MiddleThird => r(x, y + h / 3, w, h / 3),
        Snap::BottomThird => r(x, y + h - h / 3, w, h / 3),
        Snap::ReasonableSize => r(x + (w - w * 6 / 10) / 2, y + (h - h * 7 / 10) / 2, w * 6 / 10, h * 7 / 10),
        Snap::Maximize | Snap::Restore | Snap::NextDisplay => cur,
    }
}

/// Window and screen gaps in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gaps {
    pub gap: i32,
    pub screen: i32,
}

impl Gaps {
    pub fn for_dpi(w: &config::Window, dpi: u32) -> Gaps {
        let px = |v: u32| (v * dpi / 96) as i32;
        Gaps { gap: px(w.gap), screen: px(w.screen_gap) }
    }
}

/// `wa` shrunk by `by` on every side: the screen gap.
pub fn inset(wa: RECT, by: i32) -> RECT {
    RECT { left: wa.left + by, top: wa.top + by, right: wa.right - by, bottom: wa.bottom - by }
}

/// Moves every edge of `target` that is not on `wa`'s edge inward by half the gap,
/// so two neighbours end up `gap` apart. `wa` is the work area after the screen gap.
pub fn with_gaps(target: RECT, wa: RECT, gap: i32) -> RECT {
    let h = gap / 2;
    RECT {
        left: target.left + if target.left != wa.left { h } else { 0 },
        top: target.top + if target.top != wa.top { h } else { 0 },
        right: target.right - if target.right != wa.right { h } else { 0 },
        bottom: target.bottom - if target.bottom != wa.bottom { h } else { 0 },
    }
}

/// Whether `s` tiles against neighbours, so the window gap applies. The others
/// (centred and floating sizes) only respect the screen gap.
fn takes_gap(s: Snap) -> bool {
    !matches!(s, Snap::Center | Snap::AlmostMaximize | Snap::ReasonableSize)
}

/// Final rect for `s` on work area `wa` with the screen gap, then the window gap where it applies.
pub fn target_rect(s: Snap, wa: RECT, cur: RECT, g: Gaps) -> RECT {
    let wa = inset(wa, g.screen);
    let t = layout(s, wa, cur);
    if takes_gap(s) { with_gaps(t, wa, g.gap) } else { t }
}

/// Like `target_rect`, but pressing a half again while the window already sits on it
/// steps through ½ → ⅔ → ⅓ → ½ (same edge). Anything else, or `cycle` off, is `target_rect`.
pub fn cycle_target(s: Snap, wa: RECT, vis: RECT, g: Gaps, cycle: bool) -> RECT {
    let base = target_rect(s, wa, vis, g);
    if !cycle || !matches!(s, Snap::LeftHalf | Snap::RightHalf | Snap::TopHalf | Snap::BottomHalf) {
        return base;
    }
    let wa = inset(wa, g.screen);
    let (w, h) = (wa.right - wa.left, wa.bottom - wa.top);
    let step = |n: i32, d: i32| {
        let t = match s {
            Snap::LeftHalf => RECT { right: wa.left + w * n / d, ..wa },
            Snap::RightHalf => RECT { left: wa.right - w * n / d, ..wa },
            Snap::TopHalf => RECT { bottom: wa.top + h * n / d, ..wa },
            _ => RECT { top: wa.bottom - h * n / d, ..wa },
        };
        with_gaps(t, wa, g.gap)
    };
    let steps = [step(1, 2), step(2, 3), step(1, 3)];
    let near = |a: RECT, b: RECT| {
        (a.left - b.left).abs() <= 2 && (a.top - b.top).abs() <= 2 && (a.right - b.right).abs() <= 2 && (a.bottom - b.bottom).abs() <= 2
    };
    match steps.iter().position(|&t| near(vis, t)) {
        Some(i) => steps[(i + 1) % 3],
        None => steps[0],
    }
}

/// The desktop and taskbar: opening procast from them leaves no window to move.
pub fn is_shell(hwnd: HWND) -> bool {
    let mut buf = [0u16; 32];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    matches!(String::from_utf16_lossy(&buf[..n]).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

/// A `Snap` by its variant name ("FirstTwoThirds"), as layouts name them. "FirstThird" and
/// "LastThird" are accepted for `LeftThird` and `RightThird` (the titles those two show).
pub fn snap_by_name(name: &str) -> Option<Snap> {
    let name = match name {
        "FirstThird" => "LeftThird",
        "LastThird" => "RightThird",
        n => n,
    };
    ALL.iter().find_map(|&(c, ..)| match c {
        Cmd::Snap(s) if format!("{s:?}") == name => Some(s),
        _ => None,
    })
}

/// Programs whose windows belong to a different exe than the one that starts them:
/// (window exe, launch alias on PATH).
const LAUNCH_ALIASES: &[(&str, &str)] = &[("WindowsTerminal.exe", "wt.exe")];

/// Whether two exe file names are the same program: equal, or a window exe and its launch alias.
fn same_program(a: &str, b: &str) -> bool {
    let is = |x: &str, y: &str| x.eq_ignore_ascii_case(y);
    is(a, b) || LAUNCH_ALIASES.iter().any(|&(win, alias)| is(a, win) && is(b, alias) || is(a, alias) && is(b, win))
}

/// A visible window of the program named `app` (a file name, any case), procast's own excluded.
fn window_of(app: &str) -> Option<HWND> {
    let want = std::path::Path::new(app).file_name()?.to_string_lossy().into_owned();
    let me = std::process::id();
    crate::apps::app_windows()
        .into_iter()
        .find(|&(h, pid)| {
            let (pid, exe) = crate::apps::window_exe(h, pid);
            pid != me && std::path::Path::new(&exe).file_name().is_some_and(|n| same_program(&n.to_string_lossy(), &want))
        })
        .map(|(h, _)| h)
}

/// How a layout starts a program that has no window.
#[derive(Debug, PartialEq)]
enum Launch {
    /// A Start-menu app, by launcher id.
    App(String),
    /// A full path or bare name, for ShellExecute.
    Open(String),
}

/// A full path opens as is; a bare exe name goes through the first launcher app (from `ids`)
/// whose shortcut targets that program, else its launch alias (Windows Terminal's `wt.exe`);
/// anything else is left to ShellExecute's own search.
fn pick_launch(app: &str, ids: &[String], target_of: impl Fn(&str) -> Option<String>) -> Launch {
    let path = std::path::Path::new(app);
    if let Some(name) = path.file_name().filter(|_| !path.is_absolute()) {
        let same = |id: &&String| target_of(id).is_some_and(|t| std::path::Path::new(&t).file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)));
        if let Some(id) = ids.iter().find(same) {
            return Launch::App(id.clone());
        }
    }
    let alias = LAUNCH_ALIASES.iter().find(|(win, _)| app.eq_ignore_ascii_case(win)).map_or(app, |&(_, alias)| alias);
    Launch::Open(alias.to_string())
}

/// Runs a layout preset: each slot's program gets a window (started if none is open,
/// waiting up to 5 s) and is snapped, never cycled. A failing slot doesn't stop the
/// rest; the first error is returned. `ids` are the launcher apps (Start menu) a bare exe
/// name is looked up in before falling back to ShellExecute.
pub fn run_layout(l: &config::Layout, w: &config::Window, ids: &[String]) -> Result<(), String> {
    let mut first = None;
    for slot in &l.slots {
        if let Err(e) = run_slot(slot, w, ids) {
            first.get_or_insert(format!("Layout {}: {e}", l.name));
        }
    }
    first.map_or(Ok(()), Err)
}

fn run_slot(slot: &config::Slot, w: &config::Window, ids: &[String]) -> Result<(), String> {
    let s = snap_by_name(&slot.snap).ok_or_else(|| format!("unknown snap {}", slot.snap))?;
    let mon = match slot.display {
        Some(n) => Some(n.checked_sub(1).and_then(|i| monitors().get(i).copied()).ok_or_else(|| format!("no display {n}"))?),
        None => None,
    };
    let hwnd = match window_of(&slot.app) {
        Some(h) => h,
        None => {
            match pick_launch(&slot.app, ids, crate::win::app_target) {
                Launch::App(id) => crate::win::launch_app_wait(&id),
                Launch::Open(t) => crate::win::open_wait(&t),
            }?;
            let h = (0..50).find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                window_of(&slot.app)
            });
            let h = h.ok_or_else(|| format!("{} didn't open a window", slot.app))?;
            // Let a fresh window finish sizing itself before moving it.
            std::thread::sleep(std::time::Duration::from_millis(300));
            h
        }
    };
    snap_with(hwnd, s, w, false, mon);
    Ok(())
}

fn snap(hwnd: HWND, s: Snap, w: &config::Window) {
    snap_with(hwnd, s, w, w.cycle, None)
}

/// `snap`, with cycling chosen by the caller and optionally a monitor to snap on
/// instead of the window's own.
fn snap_with(hwnd: HWND, s: Snap, w: &config::Window, cycle: bool, on: Option<HMONITOR>) {
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
        let was_big = IsZoomed(hwnd).as_bool() || IsIconic(hwnd).as_bool();
        if was_big {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let mut cur = RECT::default();
        let _ = GetWindowRect(hwnd, &mut cur);
        // Windows 10/11 frames have invisible resize borders; size the visible frame instead.
        let mut vis = cur;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut vis as *mut _ as _, size_of::<RECT>() as u32);
        // A just-restored window is not part of a cycle: its first press gives the plain half.
        let cycle = cycle && !was_big;
        let here = on.unwrap_or_else(|| MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST));
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
            let (mut dx, mut dy) = (96, 96);
            let _ = GetDpiForMonitor(here, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            cycle_target(s, work_area(here), vis, Gaps::for_dpi(w, dx), cycle)
        };
        t.left -= vis.left - cur.left;
        t.top -= vis.top - cur.top;
        t.right += cur.right - vis.right;
        t.bottom += cur.bottom - vis.bottom;
        let _ = SetWindowPos(hwnd, None, t.left, t.top, t.right - t.left, t.bottom - t.top, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rc(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: r, bottom: b }
    }
    fn same(a: RECT, b: RECT) -> bool {
        (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
    }
    const WA: RECT = RECT { left: 0, top: 0, right: 1200, bottom: 900 };

    #[test]
    fn gaps_scale_with_dpi() {
        let w = config::Window { gap: 10, screen_gap: 4, ..Default::default() };
        assert_eq!(Gaps::for_dpi(&w, 144), Gaps { gap: 15, screen: 6 });
    }

    #[test]
    fn gaps_separate_neighbours() {
        let cur = rc(100, 100, 500, 400);
        assert!(same(with_gaps(layout(Snap::LeftHalf, WA, cur), WA, 10), rc(0, 0, 595, 900)));
        assert!(same(with_gaps(layout(Snap::RightHalf, WA, cur), WA, 10), rc(605, 0, 1200, 900)));
    }

    #[test]
    fn center_ignores_window_gap() {
        let g = Gaps { gap: 16, screen: 0 };
        let t = target_rect(Snap::Center, WA, rc(100, 100, 500, 400), g);
        assert!(same(t, rc(400, 300, 800, 600)), "{t:?}");
    }

    #[test]
    fn almost_maximize_and_reasonable_size_use_only_the_screen_gap() {
        let g = Gaps { gap: 16, screen: 8 };
        let cur = rc(100, 100, 500, 400);
        for s in [Snap::AlmostMaximize, Snap::ReasonableSize] {
            assert!(same(target_rect(s, WA, cur, g), layout(s, inset(WA, 8), cur)), "{s:?}");
        }
        // Halves still get the gap.
        assert!(same(target_rect(Snap::LeftHalf, WA, cur, Gaps { gap: 10, screen: 0 }), rc(0, 0, 595, 900)));
    }

    const NO_GAPS: Gaps = Gaps { gap: 0, screen: 0 };

    #[test]
    fn left_half_cycles() {
        let step = |vis| cycle_target(Snap::LeftHalf, WA, vis, NO_GAPS, true);
        assert!(same(step(rc(100, 100, 500, 400)), rc(0, 0, 600, 900)));
        assert!(same(step(rc(0, 0, 600, 900)), rc(0, 0, 800, 900)));
        assert!(same(step(rc(0, 0, 801, 899)), rc(0, 0, 400, 900)));
        assert!(same(step(rc(0, 0, 400, 900)), rc(0, 0, 600, 900)));
    }

    #[test]
    fn cycle_off_repeats_half() {
        let t = cycle_target(Snap::LeftHalf, WA, rc(0, 0, 600, 900), NO_GAPS, false);
        assert!(same(t, rc(0, 0, 600, 900)));
    }

    #[test]
    fn bottom_half_cycles_height() {
        let t = cycle_target(Snap::BottomHalf, WA, rc(0, 450, 1200, 900), NO_GAPS, true);
        assert!(same(t, rc(0, 300, 1200, 900)));
    }

    #[test]
    fn cycling_recognises_the_step_with_gaps() {
        let g = Gaps { gap: 10, screen: 0 };
        let t = cycle_target(Snap::LeftHalf, WA, rc(0, 0, 595, 900), g, true);
        assert!(same(t, rc(0, 0, 795, 900)), "{t:?}");
    }

    #[test]
    fn screen_gap_insets_the_work_area() {
        let wa = inset(WA, 8);
        let cur = rc(100, 100, 500, 400);
        for s in [Snap::AlmostMaximize, Snap::LeftHalf] {
            let t = with_gaps(layout(s, wa, cur), wa, 0);
            assert!(t.left >= 8 && t.top >= 8 && t.right <= 1192 && t.bottom <= 892, "{s:?} {t:?}");
        }
    }

    #[test]
    fn others_drop_the_target_and_our_own_windows() {
        let h = |n: usize| HWND(n as _);
        let list = [(h(1), 10), (h(2), 20), (h(3), 99), (h(4), 30)];
        assert_eq!(others_of(&list, h(2), 99), vec![h(1), h(4)]);
    }

    #[test]
    fn snap_names_round_trip() {
        assert_eq!(snap_by_name("FirstTwoThirds"), Some(Snap::FirstTwoThirds));
        assert_eq!(snap_by_name("LastThird"), Some(Snap::RightThird), "the spec's name for the last third");
        assert_eq!(snap_by_name("Nope"), None);
        assert_eq!(snap_by_name("lefthalf"), None, "case-sensitive like the variant names");
        let snaps: Vec<Snap> = ALL.iter().filter_map(|&(c, ..)| if let Cmd::Snap(s) = c { Some(s) } else { None }).collect();
        assert_eq!(snaps.len(), 28, "every Snap variant is in the command table");
        for s in snaps {
            assert_eq!(snap_by_name(&format!("{s:?}")), Some(s), "{s:?}");
        }
    }

    #[test]
    fn launch_prefers_start_menu_then_path_then_shell() {
        let ids = [
            r"{Programs}\Visual Studio Code\Code.lnk".to_string(),
            "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App".to_string(),
            r"C:\Tools\x.lnk".to_string(),
        ];
        let target = |id: &str| match id {
            i if i.contains("Code") => Some(r"C:\Users\a\AppData\Local\Programs\Microsoft VS Code\Code.exe".to_string()),
            r"C:\Tools\x.lnk" => Some(r"D:\x\Tool.exe".to_string()),
            _ => None,
        };
        assert_eq!(pick_launch("code.exe", &ids, target), Launch::App(ids[0].clone()), "case-insensitive exe name");
        assert_eq!(pick_launch("Tool.exe", &ids, target), Launch::App(ids[2].clone()));
        assert_eq!(pick_launch("WindowsTerminal.exe", &ids, target), Launch::Open("wt.exe".into()), "an AUMID has no exe to match: its alias starts it");
        assert_eq!(pick_launch("wt.exe", &ids, target), Launch::Open("wt.exe".into()));
        assert_eq!(pick_launch("Unknown.exe", &ids, target), Launch::Open("Unknown.exe".into()));
        assert_eq!(pick_launch(r"C:\Tools\Code.exe", &ids, target), Launch::Open(r"C:\Tools\Code.exe".into()), "a full path opens as is");
    }

    #[test]
    fn terminal_and_its_alias_are_one_program() {
        assert!(same_program("WindowsTerminal.exe", "wt.exe") && same_program("WT.EXE", "windowsterminal.exe"));
        assert!(same_program("Code.exe", "code.exe"));
        assert!(!same_program("Code.exe", "wt.exe") && !same_program("WindowsTerminal.exe", "Code.exe"));
    }

    #[test]
    fn layout_errors_name_the_layout() {
        let l = config::Layout { name: "Coding".into(), slots: vec![config::Slot { app: "x.exe".into(), snap: "Nope".into(), display: None }] };
        assert_eq!(run_layout(&l, &config::Window::default(), &[]), Err("Layout Coding: unknown snap Nope".into()));
    }

    #[test]
    fn desktop_commands_are_listed_once_each() {
        let titles: Vec<&str> = ALL.iter().filter(|(c, ..)| matches!(c, Cmd::Desktop(_))).map(|&(_, t, ..)| t).collect();
        assert_eq!(titles.len(), 24);
        assert_eq!((titles[0], titles[9], titles[10], titles[19]), ("Open Desktop 1", "Open Desktop 10", "Move to Desktop 1", "Move to Desktop 10"));
        assert!(ALL.iter().any(|&(c, t, ..)| c == Cmd::Desktop(Desk::Rename) && t == "Rename Desktop"));
        assert!(ALL.iter().filter(|(c, ..)| matches!(c, Cmd::Desktop(_))).all(|&(c, ..)| c.subtitle() == "Window"));
    }

    #[test]
    fn music_commands_are_listed_with_their_own_keys() {
        let music: Vec<_> = ALL.iter().filter(|(c, ..)| matches!(c, Cmd::YtMusic(_) | Cmd::YtControl(_))).collect();
        assert_eq!(music.len(), 4 + Control::ALL.len());
        assert!(Control::ALL.iter().all(|&c| music.iter().any(|&&(cmd, ..)| cmd == Cmd::YtControl(c))));
        assert!(music.iter().all(|&&(c, ..)| c.subtitle() == "YouTube Music" && c.key().starts_with("ytm:")));
        assert_eq!(Cmd::YtMusic(Screen::QuickActions).key(), "ytm:actions");
        assert_eq!(Cmd::YtControl(Control::PlayPause).key(), "ytm:play-pause");
        assert_eq!(Cmd::Lock.key(), "cmd:Lock");
        let mut keys: Vec<String> = ALL.iter().map(|&(c, ..)| c.key()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), ALL.len(), "every command has its own key");
    }

    #[test]
    fn close_others_asks_twice() {
        assert!(Cmd::CloseOthers.destructive() && Cmd::Desktop(Desk::Close).destructive());
        assert!(!Cmd::Desktop(Desk::MoveNext).destructive() && !Cmd::Desktop(Desk::Open(2)).destructive());
        assert!(!Cmd::MinimizeOthers.destructive());
    }
}
