//! Tells the app which window has focus and whether it is a game: one WinEvent hook on
//! its own thread, shared by Auto HDR and Focus. The callback stays small; the UI thread
//! decides what a change means.

use std::{sync::OnceLock, time::Duration};

use futures::channel::mpsc::UnboundedSender;
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
    UI::{
        Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
        Shell::{QUERY_USER_NOTIFICATION_STATE, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState},
        WindowsAndMessaging::{
            EVENT_SYSTEM_FOREGROUND, GWL_STYLE, GetForegroundWindow, GetMessageW, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, MSG, OBJID_WINDOW,
            WINEVENT_OUTOFCONTEXT, WS_CAPTION,
        },
    },
};

use crate::win::Msg;

/// Game library folders, as consecutive path segments (compared lower case): Steam, Epic,
/// Xbox / Game Pass, Riot. Not WindowsApps: it holds every Store app, video players included.
const LIBRARIES: &[&[&str]] = &[&["steamapps", "common"], &["epic games"], &["xboxgames"], &["riot games"]];
/// D3D full-screen state lags the focus event (and outlives an alt-tab), so it is read again
/// this long after each change.
const RECHECK: Duration = Duration::from_secs(1);

static TX: OnceLock<UnboundedSender<Msg>> = OnceLock::new();

/// Starts the hook (once): every change of the foreground window sends `Msg::Foreground`.
pub fn watch(tx: UnboundedSender<Msg>) {
    if TX.set(tx).is_err() {
        return;
    }
    std::thread::spawn(|| unsafe {
        let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(changed), 0, 0, WINEVENT_OUTOFCONTEXT);
        if hook.is_invalid() {
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        let _ = UnhookWinEvent(hook);
    });
}

unsafe extern "system" fn changed(_: HWINEVENTHOOK, _: u32, hwnd: HWND, object: i32, _: i32, _: u32, _: u32) {
    if hwnd.0.is_null() || object != OBJID_WINDOW.0 {
        return;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let path = crate::apps::exe_path(pid).unwrap_or_default();
    // ponytail: no user game list yet (the config key comes with the features that use it).
    let game = is_game(&path, d3d_full(), covers_monitor(hwnd), &[]);
    let Some(tx) = TX.get() else { return };
    let _ = tx.unbounded_send(Msg::Foreground { hwnd: hwnd.0 as isize, pid, exe: file_name(&path), game });
    let at = hwnd.0 as isize;
    std::thread::spawn(move || {
        std::thread::sleep(RECHECK);
        let hwnd = HWND(at as *mut _);
        let now = is_game(&path, d3d_full(), covers_monitor(hwnd), &[]);
        if resend(unsafe { GetForegroundWindow() } == hwnd, game, now) {
            let _ = tx.unbounded_send(Msg::Foreground { hwnd: at, pid, exe: file_name(&path), game: now });
        }
    });
}

/// A second report is due when the window still has focus and the re-check disagrees.
fn resend(still_foreground: bool, sent: bool, now: bool) -> bool {
    still_foreground && sent != now
}

/// "c:\\games\\foo.exe" -> "foo.exe", lower case.
fn file_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or_default().to_lowercase()
}

/// A window's program file name, lower case ("chrome.exe").
#[allow(dead_code)] // Focus (task 7) uses it
pub fn exe_of(hwnd: HWND) -> Option<String> {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    crate::apps::exe_path(pid).map(|p| file_name(&p))
}

/// `exe` is the program's full path (or just its file name, which can only match `games`).
/// A game is: a listed program, a D3D full-screen app, or a borderless full-monitor window
/// of a program installed in a game library folder (so Chrome on F11 is not one).
pub fn is_game(exe: &str, d3d_full: bool, covers_monitor: bool, games: &[String]) -> bool {
    let name = file_name(exe);
    if games.iter().any(|g| g.to_lowercase() == name) || d3d_full {
        return true;
    }
    let segments: Vec<String> = exe.split(['\\', '/']).map(str::to_lowercase).collect();
    covers_monitor && LIBRARIES.iter().any(|lib| segments.windows(lib.len()).any(|w| w.iter().zip(*lib).all(|(a, b)| a == b)))
}

fn notification() -> Option<QUERY_USER_NOTIFICATION_STATE> {
    unsafe { SHQueryUserNotificationState() }.ok()
}

fn d3d_full() -> bool {
    notification() == Some(QUNS_RUNNING_D3D_FULL_SCREEN)
}

/// A full-screen app or game has focus (game mode turns hotkeys off then).
/// ponytail: the hotkey is still registered, so the key never reaches the game; unregister
/// on a foreground change if that matters.
pub fn full_screen_app() -> bool {
    notification().is_some_and(|s| [QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN, QUNS_PRESENTATION_MODE].contains(&s))
}

/// The window fills its monitor and has no title bar.
fn covers_monitor(hwnd: HWND) -> bool {
    unsafe {
        if GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CAPTION.0 != 0 {
            return false;
        }
        let mut rect = RECT::default();
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetWindowRect(hwnd, &mut rect).is_ok() && GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() && rect == info.rcMonitor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_detection() {
        let games = vec!["Game.EXE".to_string()];
        // Listed, whatever the case or folder.
        assert!(is_game(r"C:\Tools\game.exe", false, false, &games));
        assert!(is_game("GAME.exe", false, false, &games));
        // D3D full-screen.
        assert!(is_game(r"C:\x\a.exe", true, false, &[]));
        // Borderless on a monitor: a game only from a library folder.
        let chrome = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
        assert!(!is_game(chrome, false, true, &games));
        assert!(is_game(r"D:\SteamLibrary\steamapps\common\Foo\foo.exe", false, true, &[]));
        assert!(is_game(r"C:\Program Files\Epic Games\Fortnite\f.exe", false, true, &[]));
        assert!(is_game(r"C:\XboxGames\Halo\halo.exe", false, true, &[]));
        assert!(is_game("c:/program files (x86)/STEAM/STEAMAPPS/Common/Foo/foo.exe", false, true, &[]));
        assert!(is_game(r"D:\Games\EPIC GAMES\Fortnite.exe", false, true, &[]));
        // Store apps (video players, Terminal) are not games when borderless.
        assert!(!is_game(r"C:\Program Files\WindowsApps\Microsoft.Photos_1\Photos.exe", false, true, &[]));
        assert!(is_game(r"C:\Riot Games\VALORANT\v.exe", false, true, &[]));
        // Library folder, but not full-monitor; or full-monitor, wrong place.
        assert!(!is_game(r"D:\steamapps\common\Foo\foo.exe", false, false, &[]));
        assert!(!is_game(r"D:\steamapps\music\foo.exe", false, true, &[]));
        // Neither.
        assert!(!is_game(chrome, false, false, &games));
        assert!(!is_game("", false, false, &[]));
    }

    #[test]
    fn recheck_resends_only_changes_of_the_same_window() {
        assert!(resend(true, true, false)); // alt-tabbed out of the game: no longer one
        assert!(resend(true, false, true)); // the game's D3D state caught up
        assert!(!resend(true, true, true));
        assert!(!resend(false, true, false)); // focus moved on: the next event covers it
    }
}
