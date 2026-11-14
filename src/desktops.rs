//! Virtual desktops, through winvd (the library Raycast for Windows uses). It drives Explorer's
//! undocumented COM interfaces, which change between Windows builds, so every call goes
//! through `call`: one thread owns the COM objects, panics and errors become messages, and a
//! hung Explorer times out instead of blocking.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering::Relaxed},
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
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
    call_within(what, WAIT, f)
}

/// `call` with its own wait. A job still queued after its caller gave up is dropped unrun, so
/// a hung Explorer that recovers cannot replay old work (a switch, a confirmed close).
fn call_within<T: Send + 'static>(what: &str, wait: Duration, f: impl FnOnce() -> Result<T, winvd::Error> + Send + 'static) -> Result<T, String> {
    let (reply, answer) = channel();
    let deadline = Instant::now() + wait;
    let job: Job = Box::new(move || {
        if Instant::now() <= deadline {
            _ = reply.send(catch_unwind(AssertUnwindSafe(f)));
        }
    });
    let failure = match thread().map(|t| t.send(job)) {
        Some(Ok(())) => match answer.recv_timeout(wait) {
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
    crate::ext_log("virtual desktops", &failure);
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

/// How long a desktop must stay left alone before it counts as abandoned (a quick pass through
/// it, or a window still moving out, should not remove it).
const SETTLE: Duration = Duration::from_millis(500);

/// Whether to remove desktop `left` (0-based, among `count`) now that `current` is shown: only
/// an empty one that is not the current one, and never the last.
fn should_remove(left: u32, current: u32, count: u32, windows_on_left: usize) -> bool {
    count > 1 && left != current && windows_on_left == 0
}

type Id = windows058::core::GUID;

/// Where a window lives, as far as the desktop code could tell.
#[derive(Debug, PartialEq)]
enum Lives {
    On(Id),
    /// No desktop for it (winvd's `WindowNotFound`).
    Nowhere,
    /// Any other failure: unknown.
    Unknown,
}

/// What `close_if_empty` learned about one window.
struct Seen {
    pid: u32,
    pinned: bool,
    cloak: u32,
    lives: Lives,
}

/// How many of `seen` keep desktop `old` in use. Every doubt counts the window (a desktop is only
/// removed when sure it is empty), except:
/// - procast's own windows (`me`), which never keep a desktop;
/// - pinned windows, which show on every desktop and so belong to none;
/// - ghosts: a shell-cloaked window with no desktop at all (suspended or preloaded Store app
///   frames, TextInputHost: seen on a real machine, each answers `WindowNotFound`). One of them
///   would otherwise count on every desktop and auto-close would never fire. A window that is not
///   shell-cloaked and has no desktop still counts.
fn windows_on(seen: &[Seen], old: Id, me: u32) -> usize {
    seen.iter()
        .filter(|w| w.pid != me && !w.pinned)
        .filter(|w| match &w.lives {
            Lives::On(id) => *id == old,
            Lives::Nowhere => w.cloak & crate::apps::CLOAKED_SHELL == 0,
            Lives::Unknown => true,
        })
        .count()
}

fn look_at(h: isize, pid: u32, cloak: u32) -> Seen {
    let hwnd = winvd_hwnd(h);
    let pinned = winvd::is_pinned_window(hwnd).unwrap_or(false) || winvd::is_pinned_app(hwnd).unwrap_or(false);
    let lives = match winvd::get_desktop_by_window(hwnd).and_then(|d| d.get_id()) {
        Ok(id) => Lives::On(id),
        Err(winvd::Error::WindowNotFound) => Lives::Nowhere,
        Err(_) => Lives::Unknown,
    };
    Seen { pid, pinned, cloak, lives }
}

/// Removes desktop `old` (by id: indexes shift) if it still exists, is not current and holds no
/// app windows (see `windows_on`).
fn close_if_empty(old: Id) -> Result<(), winvd::Error> {
    let ids = winvd::get_desktops()?.iter().map(|d| d.get_id()).collect::<Result<Vec<_>, _>>()?;
    let Some(left) = ids.iter().position(|&i| i == old) else { return Ok(()) };
    let current = winvd::get_current_desktop()?;
    let seen: Vec<Seen> = crate::apps::app_windows_everywhere().into_iter().map(|(h, pid, cloak)| look_at(handle(h), pid, cloak)).collect();
    if should_remove(left as u32, current.get_index()?, ids.len() as u32, windows_on(&seen, old, std::process::id())) { winvd::remove_desktop(winvd::get_desktop(old), current) } else { Ok(()) }
}

/// Watches desktop changes and removes the desktop you left once it is empty. Dropping it stops
/// the watching.
pub struct Listener {
    events: Option<winvd::DesktopEventThread>,
    stop: Arc<AtomicBool>,
}

impl Listener {
    /// None when this Windows is not supported or the listener could not start (logged).
    pub fn start() -> Option<Listener> {
        if !supported() {
            return None;
        }
        let (tx, rx) = channel();
        let events = winvd::listen_desktop_events::<winvd::DesktopEvent, _>(tx).inspect_err(|e| crate::ext_log("virtual desktops", &format!("Auto close desktops: {e:?}"))).ok()?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new().name("desktops-auto-close".into()).spawn(move || watch(rx, flag)).ok()?;
        Some(Listener { events: Some(events), stop })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        // Stopping joins winvd's thread, which can wait on Explorer: not on the caller's thread.
        // The watcher ends by itself once that thread drops the event sender.
        if let Some(events) = self.events.take() {
            _ = std::thread::Builder::new().name("desktops-stop".into()).spawn(move || drop(events));
        }
    }
}

fn watch(rx: Receiver<winvd::DesktopEvent>, stop: Arc<AtomicBool>) {
    for event in rx {
        let winvd::DesktopEvent::DesktopChanged { old, .. } = event else { continue };
        // Failures are logged by `call`; background work shows no notice.
        let Ok(id) = call("Auto close: identify desktop", move || old.get_id()) else { continue };
        std::thread::sleep(SETTLE);
        if stop.load(Relaxed) {
            return;
        }
        _ = call("Auto close: remove empty desktop", move || close_if_empty(id));
    }
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
    fn a_job_that_timed_out_never_runs() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering::SeqCst},
        };
        let wait = Duration::from_millis(100);
        // The first job holds the desktop thread past `wait`, like a hung Explorer.
        let slow = call_within("Slow", wait, || {
            std::thread::sleep(Duration::from_millis(500));
            Ok(())
        });
        assert!(slow.unwrap_err().contains("no answer from Explorer"));
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        let late = call_within("Late", wait, move || {
            flag.store(true, SeqCst);
            Ok(())
        });
        assert!(late.unwrap_err().contains("no answer from Explorer"));
        // Let the thread get past the slow job and reach the queued one.
        assert_eq!(call("Drain", || Ok(1)), Ok(1));
        assert!(!ran.load(SeqCst), "the late job ran after its caller gave up");
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
    fn only_empty_left_desktops_go() {
        assert!(should_remove(1, 2, 3, 0));
        assert!(!should_remove(1, 2, 3, 2), "a desktop with windows stays");
        assert!(!should_remove(2, 2, 3, 0), "the current desktop stays");
        assert!(!should_remove(0, 0, 1, 0), "the last desktop is never removed");
        assert!(!should_remove(1, 0, 1, 0));
    }

    fn seen(pid: u32, pinned: bool, cloak: u32, lives: Lives) -> Seen {
        Seen { pid, pinned, cloak, lives }
    }

    #[test]
    fn what_keeps_a_desktop_in_use() {
        let (old, other) = (Id::from_u128(1), Id::from_u128(2));
        let me = 100;
        let count = |v: Vec<Seen>| windows_on(&v, old, me);
        assert_eq!(count(vec![]), 0);
        assert_eq!(count(vec![seen(5, false, 2, Lives::On(old))]), 1, "a window on it");
        assert_eq!(count(vec![seen(5, false, 2, Lives::On(other)), seen(6, false, 0, Lives::On(other))]), 0, "windows elsewhere");
        assert_eq!(count(vec![seen(5, false, 2, Lives::Unknown)]), 1, "a failed lookup counts");
        assert_eq!(count(vec![seen(5, false, 0, Lives::Nowhere)]), 1, "a visible window with no desktop counts");
        assert_eq!(count(vec![seen(5, false, 1, Lives::Nowhere)]), 1, "only a shell cloak marks a ghost");
        assert_eq!(count(vec![seen(5, false, 2, Lives::Nowhere), seen(6, false, 3, Lives::Nowhere)]), 0, "ghosts do not count");
        assert_eq!(count(vec![seen(5, true, 0, Lives::On(old))]), 0, "pinned windows belong to no desktop");
        assert_eq!(count(vec![seen(me, false, 0, Lives::On(old))]), 0, "procast's own windows do not count");
        assert_eq!(count(vec![seen(5, false, 2, Lives::Nowhere), seen(6, false, 2, Lives::On(old))]), 1, "a ghost beside a real window");
    }

    #[test]
    fn the_gate_is_a_build_number() {
        assert!(build().is_some_and(|b| b > 0));
        assert!(meets(22631) && meets(26200) && !meets(22630) && !meets(19045));
    }

    /// Registers and removes an event listener with Explorer; creates, switches and removes nothing.
    #[test]
    #[ignore]
    fn listener_starts_and_stops() {
        let l = Listener::start();
        println!("listener: {}", l.is_some());
        drop(l);
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
