//! Virtual desktops, through winvd (the library Raycast for Windows uses). It drives Explorer's
//! undocumented COM interfaces, which change between Windows builds, so every call goes
//! through `call`: one thread owns the COM objects, panics and errors become messages, and a
//! hung Explorer times out instead of blocking.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        OnceLock,
        mpsc::{Sender, channel},
    },
    time::Duration,
};
use windows::{
    Wdk::System::SystemServices::RtlGetVersion,
    Win32::{
        Foundation::HWND,
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx},
            SystemInformation::OSVERSIONINFOW,
        },
    },
};

/// Windows 11 23H2: the oldest build whose desktop interfaces are checked here.
const MIN_BUILD: u32 = 22631;

/// How long a caller waits for Explorer before giving up.
const WAIT: Duration = Duration::from_secs(3);

type Job = Box<dyn FnOnce() + Send>;

fn meets(build: u32) -> bool {
    build >= MIN_BUILD
}

fn build() -> Option<u32> {
    let mut v = OSVERSIONINFOW { dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32, ..Default::default() };
    unsafe { RtlGetVersion(&mut v) }.is_ok().then_some(v.dwBuildNumber)
}

/// Whether this Windows has desktop commands (decided once).
pub fn supported() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| build().is_some_and(meets))
}

/// The one thread every winvd call runs on (it keeps its COM objects per thread), started on
/// first use. Jobs are boxed closures so other users, like an event listener, can share it.
fn thread() -> Option<&'static Sender<Job>> {
    static TX: OnceLock<Option<Sender<Job>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let started = std::thread::Builder::new().name("desktops".into()).spawn(move || {
            unsafe { _ = CoInitializeEx(None, COINIT_MULTITHREADED) };
            rx.into_iter().for_each(|job| job());
        });
        started.ok().map(|_| tx)
    })
    .as_ref()
}

/// Runs `f` on the desktop thread and waits for it. A winvd error or panic comes back as
/// "<what>: <reason>" (and is logged); no answer in `WAIT` is "<what>: no answer from Explorer".
pub fn call<T: Send + 'static>(what: &str, f: impl FnOnce() -> Result<T, winvd::Error> + Send + 'static) -> Result<T, String> {
    let (reply, answer) = channel();
    let job: Job = Box::new(move || _ = reply.send(catch_unwind(AssertUnwindSafe(f))));
    let failure = match thread().map(|t| t.send(job)) {
        Some(Ok(())) => match answer.recv_timeout(WAIT) {
            Ok(Ok(Ok(v))) => return Ok(v),
            Ok(Ok(Err(e))) => format!("{what}: {e:?}"),
            Ok(Err(p)) => {
                let why = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                format!("{what}: crashed ({why})")
            }
            Err(_) => format!("{what}: no answer from Explorer"),
        },
        _ => format!("{what}: the desktop thread is not running"),
    };
    eprintln!("procast: {failure}");
    Err(failure)
}

/// The 0-based index of desktop `n` (1-based) among `count`.
fn check_index(n: u8, count: u32) -> Result<u32, String> {
    if (1..=count).contains(&u32::from(n)) { Ok(u32::from(n) - 1) } else { Err(format!("There is no desktop {n}")) }
}

/// The desktop after `cur`, or None when `cur` is the last of `count` (a new one is needed).
fn next_index(cur: u32, count: u32) -> Option<u32> {
    (cur + 1 < count).then_some(cur + 1)
}

fn prev_index(cur: u32) -> Option<u32> {
    cur.checked_sub(1)
}

/// Where a closed desktop's windows go: left, or the next one from the first.
fn fallback(cur: u32) -> u32 {
    prev_index(cur).unwrap_or(1)
}

/// The 0-based index of desktop `n` (1-based), or the "no such desktop" message.
fn index(n: u8) -> Result<u32, String> {
    check_index(n, call("Count desktops", winvd::get_desktop_count)?)
}

/// winvd wants the HWND of its own `windows` crate version; the handle crosses as an integer.
fn handle(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

fn winvd_hwnd(h: isize) -> windows058::Win32::Foundation::HWND {
    windows058::Win32::Foundation::HWND(h as *mut std::ffi::c_void)
}

pub fn open(n: u8) -> Result<(), String> {
    let i = index(n)?;
    call("Open Desktop", move || winvd::switch_desktop(i))
}

/// Moves the window without switching to its new desktop.
pub fn move_window(hwnd: HWND, n: u8) -> Result<(), String> {
    let (i, h) = (index(n)?, handle(hwnd));
    call("Move to Desktop", move || winvd::move_window_to_desktop(i, &winvd_hwnd(h)))
}

/// To the desktop after the window's, creating one past the last.
pub fn move_next(hwnd: HWND) -> Result<(), String> {
    let h = handle(hwnd);
    call("Move to Next Desktop", move || {
        let h = winvd_hwnd(h);
        let cur = winvd::get_desktop_by_window(h)?.get_index()?;
        let to = match next_index(cur, winvd::get_desktop_count()?) {
            Some(i) => i,
            None => winvd::create_desktop()?.get_index()?,
        };
        winvd::move_window_to_desktop(to, &h)
    })
}

/// To the desktop before the window's; nothing on the first.
pub fn move_prev(hwnd: HWND) -> Result<(), String> {
    let h = handle(hwnd);
    call("Move to Previous Desktop", move || {
        let h = winvd_hwnd(h);
        match prev_index(winvd::get_desktop_by_window(h)?.get_index()?) {
            Some(i) => winvd::move_window_to_desktop(i, &h),
            None => Ok(()),
        }
    })
}

/// Closes the current desktop; its windows move to the one on the left.
pub fn close() -> Result<(), String> {
    let closed = call("Close Desktop", || {
        if winvd::get_desktop_count()? < 2 {
            return Ok(false);
        }
        let cur = winvd::get_current_desktop()?.get_index()?;
        winvd::remove_desktop(cur, fallback(cur)).map(|()| true)
    })?;
    if closed { Ok(()) } else { Err("There is only one desktop".into()) }
}

/// Names the current desktop.
pub fn rename(name: &str) -> Result<(), String> {
    let name = name.to_string();
    call("Rename Desktop", move || winvd::get_current_desktop()?.set_name(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_turns_panics_into_errors() {
        let e = call("Switch", || -> Result<(), winvd::Error> { panic!("boom") }).unwrap_err();
        assert!(e.contains("Switch") && e.contains("boom"), "{e}");
        // The desktop thread survives a panic.
        assert_eq!(call("Again", || Ok(7)), Ok(7));
    }

    #[test]
    fn call_names_the_failed_step() {
        let e = call("Rename Desktop", || -> Result<(), winvd::Error> { Err(winvd::Error::DesktopNotFound) }).unwrap_err();
        assert!(e.starts_with("Rename Desktop: "), "{e}");
    }

    #[test]
    fn out_of_range_is_a_message() {
        assert_eq!(check_index(2, 4), Ok(1));
        assert_eq!(check_index(4, 4), Ok(3));
        assert_eq!(check_index(5, 4), Err("There is no desktop 5".into()));
        assert_eq!(check_index(11, 4), Err("There is no desktop 11".into()));
        assert_eq!(check_index(0, 4), Err("There is no desktop 0".into()));
    }

    #[test]
    fn neighbours_and_fallback() {
        assert_eq!(next_index(0, 3), Some(1));
        assert_eq!(next_index(2, 3), None, "the last desktop gets a new one after it");
        assert_eq!(prev_index(2), Some(1));
        assert_eq!(prev_index(0), None, "nothing before the first");
        assert_eq!(fallback(3), 2, "windows of a closed desktop go left");
        assert_eq!(fallback(0), 1, "the first has no left: the next one");
    }

    #[test]
    fn the_gate_is_a_build_number() {
        assert!(build().is_some_and(|b| b > 0));
        assert!(meets(22631) && meets(26200) && !meets(22630) && !meets(19045));
    }

    /// Read-only: `cargo test desktops_on_this_machine -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn desktops_on_this_machine() {
        let count = call("Count", winvd::get_desktop_count).unwrap();
        let current = call("Current", || winvd::get_current_desktop()?.get_index()).unwrap();
        println!("supported: {}, desktops: {count}, current index: {current}", supported());
    }
}
