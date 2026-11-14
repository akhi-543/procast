//! Running applications: their windows, quitting them, bringing an app's window forward
//! for its hotkey, closing idle apps, and apps from extra folders.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM},
        Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
        Storage::Packaging::Appx::GetApplicationUserModelId,
        System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess},
        UI::{
            Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath},
            WindowsAndMessaging::{
                EnumChildWindows, EnumWindows, GA_ROOTOWNER, GW_OWNER, GWL_EXSTYLE, GetAncestor, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW,
                GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, PostMessageW, SW_MINIMIZE, SW_RESTORE, ShowWindow, WM_CLOSE,
                WS_EX_TOOLWINDOW,
            },
        },
    },
    core::{BOOL, GUID, PWSTR},
};

/// One running app: a process with at least one ordinary top-level window.
#[derive(Clone, Debug)]
pub struct Running {
    pub pid: u32,
    /// Full path of the program.
    pub exe: String,
    /// The program's own name ("Google Chrome"), from its version information.
    pub name: String,
    /// The first window's title.
    pub title: String,
    pub windows: Vec<isize>,
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { _ = CloseHandle(self.0) };
    }
}

pub fn exe_path(pid: u32) -> Option<String> {
    unsafe {
        let proc = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(proc.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// The packaged app a process belongs to (Store and MSIX apps); None for plain programs.
fn process_aumid(pid: u32) -> Option<String> {
    unsafe {
        let proc = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        GetApplicationUserModelId(proc.0, &mut len, Some(PWSTR(buf.as_mut_ptr()))).ok().ok()?;
        Some(String::from_utf16_lossy(&buf[..len.saturating_sub(1) as usize]))
    }
}

fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// Store apps draw inside ApplicationFrameHost; the app's own process owns a child window.
fn real_pid(hwnd: HWND, pid: u32) -> u32 {
    unsafe extern "system" fn child(h: HWND, l: LPARAM) -> BOOL {
        let (frame, out) = unsafe { &mut *(l.0 as *mut (u32, u32)) };
        let p = pid_of(h);
        if p != *frame {
            *out = p;
            return false.into();
        }
        true.into()
    }
    let mut state = (pid, pid);
    unsafe { _ = EnumChildWindows(Some(hwnd), Some(child), LPARAM(&mut state as *mut _ as isize)) };
    state.1
}

/// Windows that show in Alt+Tab: visible, unowned, not tool windows, not cloaked, titled.
pub fn app_windows() -> Vec<(HWND, u32)> {
    windows(false).into_iter().map(|(h, pid, _)| (h, pid)).collect()
}

/// Every top-level window that could keep a virtual desktop in use, as (window, pid, cloak).
/// Not the Alt+Tab list: windows of other desktops are cloaked by the shell (DWM_CLOAKED_SHELL),
/// so cloak bits other than "shell" are tolerated, an unreadable cloak state or an empty title
/// does not drop a window (a desktop is only removed when sure it is empty). Tool windows stay
/// out: they are palettes and helpers of another window, not something a desktop is kept for.
pub fn app_windows_everywhere() -> Vec<(HWND, u32, u32)> {
    windows(true)
}

/// DWM_CLOAKED_SHELL: hidden by the shell, as a window of another desktop is.
pub const CLOAKED_SHELL: u32 = 2;

/// Whether a window with this cloak state (None: unreadable) is listed. Alt+Tab wants no cloak at
/// all; "everywhere" wants the shell's cloak (another desktop) whatever other bits come with it,
/// and keeps a window whose state cannot be read.
fn cloak_ok(other_desktops: bool, cloak: Option<u32>) -> bool {
    match (other_desktops, cloak) {
        (false, c) => c == Some(0),
        (true, None) => true,
        (true, Some(c)) => c == 0 || c & CLOAKED_SHELL != 0,
    }
}

fn windows(other_desktops: bool) -> Vec<(HWND, u32, u32)> {
    struct Scan(bool, Vec<(HWND, u32, u32)>);
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let Scan(other_desktops, out) = unsafe { &mut *(l.0 as *mut Scan) };
        unsafe {
            let mut cloaked = 0u32;
            let read = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as _, 4).is_ok();
            let shown = IsWindowVisible(h).as_bool()
                && GetWindow(h, GW_OWNER).is_err()
                && GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_TOOLWINDOW.0 as isize == 0
                && (*other_desktops || GetWindowTextLengthW(h) > 0)
                && cloak_ok(*other_desktops, read.then_some(cloaked))
                && GetAncestor(h, GA_ROOTOWNER) == h;
            if shown {
                out.push((h, pid_of(h), if read { cloaked } else { 0 }));
            }
        }
        true.into()
    }
    let mut scan = Scan(other_desktops, vec![]);
    unsafe { _ = EnumWindows(Some(each), LPARAM(&mut scan as *mut _ as isize)) };
    scan.1
}

fn title(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// The process and program behind a window from `app_windows`: Store apps draw inside
/// ApplicationFrameHost, so those map to the app's own process. The path is empty when unknown.
pub fn window_exe(h: HWND, pid: u32) -> (u32, String) {
    let exe = exe_path(pid).unwrap_or_default();
    if exe.to_lowercase().ends_with("\\applicationframehost.exe") {
        let real = real_pid(h, pid);
        return (real, exe_path(real).unwrap_or(exe));
    }
    (pid, exe)
}

/// Running apps, most recently used window first (EnumWindows walks the z-order).
pub fn running() -> Vec<Running> {
    let me = std::process::id();
    let mut out: Vec<Running> = vec![];
    for (h, pid) in app_windows() {
        let (pid, exe) = window_exe(h, pid);
        if pid == me || exe.is_empty() || exe.to_lowercase().ends_with("\\explorer.exe") && title(h) == "Program Manager" {
            continue;
        }
        match out.iter_mut().find(|r| r.pid == pid) {
            Some(r) => r.windows.push(h.0 as isize),
            None => out.push(Running { pid, name: app_name(&exe), exe, title: title(h), windows: vec![h.0 as isize] }),
        }
    }
    out
}

/// A program's display name (FileDescription), or "" when it has none.
pub fn app_name(exe: &str) -> String {
    use windows::{
        Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
        core::{HSTRING, w},
    };
    unsafe {
        let path = HSTRING::from(exe);
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return String::new();
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(&path, None, size, data.as_mut_ptr() as _).is_err() {
            return String::new();
        }
        // The first language/code page pair names the string table to read.
        let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
        if !VerQueryValueW(data.as_ptr() as _, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len).as_bool() || len < 4 {
            return String::new();
        }
        let pair = std::slice::from_raw_parts(ptr as *const u16, 2);
        let key = HSTRING::from(format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription", pair[0], pair[1]));
        if !VerQueryValueW(data.as_ptr() as _, &key, &mut ptr, &mut len).as_bool() || len == 0 {
            return String::new();
        }
        let s = std::slice::from_raw_parts(ptr as *const u16, len as usize);
        String::from_utf16_lossy(s).trim_end_matches('\0').trim().to_string()
    }
}

/// Asks the app to close its windows (it may ask to save first).
pub fn quit(pid: u32) {
    for (h, p) in app_windows() {
        if p == pid || real_pid(h, p) == pid {
            unsafe { _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
    }
}

/// Ends the process at once; unsaved work is lost.
pub fn force_quit(pid: u32) -> bool {
    unsafe { OpenProcess(PROCESS_TERMINATE, false, pid).is_ok_and(|h| TerminateProcess(Handle(h).0, 1).is_ok()) }
}

/// The program an AppsFolder id points at, when it's a plain path ("{known folder GUID}\rest"
/// or a full path).
fn id_path(id: &str) -> Option<PathBuf> {
    if Path::new(id).is_absolute() {
        return Some(PathBuf::from(id));
    }
    let (folder, rest) = id.strip_prefix('{')?.split_once("}\\")?;
    let guid = GUID::try_from(folder).ok()?;
    let base = unsafe { SHGetKnownFolderPath(&guid, KF_FLAG_DEFAULT, None).ok()?.to_string().ok()? };
    Some(Path::new(&base).join(rest))
}

/// Open windows of a launcher app (an AppsFolder id), front-most first.
pub fn windows_of(id: &str, target: Option<&str>) -> Vec<HWND> {
    let path = target.map(PathBuf::from).or_else(|| id_path(id));
    let same = |exe: &str| path.as_deref().is_some_and(|p| p.as_os_str().eq_ignore_ascii_case(exe));
    let mut seen: HashMap<u32, bool> = HashMap::new();
    app_windows()
        .into_iter()
        .filter(|&(h, pid)| {
            let pid = real_pid(h, pid);
            *seen.entry(pid).or_insert_with(|| process_aumid(pid).is_some_and(|a| a.eq_ignore_ascii_case(id)) || exe_path(pid).is_some_and(|e| same(&e)))
        })
        .map(|(h, _)| h)
        .collect()
}

pub fn bring_forward(h: HWND) {
    unsafe {
        if IsIconic(h).as_bool() {
            _ = ShowWindow(h, SW_RESTORE);
        }
    }
    crate::win::focus(h);
}

pub fn minimize(h: HWND) {
    unsafe { _ = ShowWindow(h, SW_MINIMIZE) };
}

/// Programs and shortcuts in extra folders, as launcher apps (id = full path).
pub fn folder_apps(folders: &[String], deep: bool) -> Vec<crate::win::App> {
    fn walk(dir: &Path, deep: bool, depth: u32, out: &mut Vec<crate::win::App>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                if deep && depth < 4 {
                    walk(&path, deep, depth + 1, out);
                }
                continue;
            }
            let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if matches!(ext.as_str(), "exe" | "lnk" | "url" | "appref-ms") {
                let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                if !name.to_lowercase().contains("uninst") {
                    out.push(crate::win::App { name, id: path.to_string_lossy().into_owned(), icon: None });
                }
            }
        }
    }
    let mut out = vec![];
    for f in folders {
        walk(Path::new(f), deep, 0, &mut out);
    }
    out
}

/// Closes listed programs that haven't had focus for a while. Called every few seconds
/// with the foreground window; remembers when each program last had it.
#[derive(Default)]
pub struct AutoQuit {
    seen: HashMap<String, Instant>,
}

impl AutoQuit {
    pub fn tick(&mut self, names: &[String], idle: Duration, foreground: HWND) {
        if names.is_empty() {
            return;
        }
        let now = Instant::now();
        let name_of = |exe: &str| Path::new(exe).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if let Some(exe) = exe_path(pid_of(foreground)) {
            self.seen.insert(name_of(&exe), now);
        }
        for r in running() {
            let name = name_of(&r.exe);
            if !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                continue;
            }
            // Started while we weren't looking: the clock starts now.
            let last = *self.seen.entry(name).or_insert(now);
            if now - last >= idle {
                quit(r.pid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Shell::FOLDERID_ProgramFiles;

    #[test]
    fn known_folder_ids_resolve_to_paths() {
        let pf = unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, None).unwrap().to_string().unwrap() };
        let id = format!("{{{:?}}}\\Tool\\tool.exe", FOLDERID_ProgramFiles);
        assert_eq!(id_path(&id), Some(Path::new(&pf).join("Tool\\tool.exe")));
        assert_eq!(id_path("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"), None);
        assert_eq!(id_path(r"D:\Portable\x.exe"), Some(PathBuf::from(r"D:\Portable\x.exe")));
    }

    #[test]
    fn cloaked_windows_are_judged_for_their_use() {
        // Alt+Tab: only uncloaked, and unreadable is out.
        assert!(cloak_ok(false, Some(0)));
        assert!(!cloak_ok(false, Some(CLOAKED_SHELL)) && !cloak_ok(false, None));
        // Everywhere: shell cloak alone or with APP (1) / INHERITED (4); unreadable is kept.
        assert!([0, 2, 3, 6, 7].into_iter().all(|c| cloak_ok(true, Some(c))));
        assert!(cloak_ok(true, None));
        assert!(!cloak_ok(true, Some(1)) && !cloak_ok(true, Some(4)), "cloaked by the app alone is not another desktop");
    }

    #[test]
    fn running_apps_are_listed() {
        // Whatever is open on this machine; at least the call works and finds real programs.
        for r in running() {
            assert!(r.exe.contains('\\') && !r.windows.is_empty(), "{r:?}");
        }
    }

    #[test]
    #[ignore = "depends on what is open on this machine"]
    fn app_windows_lists_this_machine() {
        println!("{} app windows", app_windows().len());
    }
}

#[cfg(test)]
mod live {
    /// READ-ONLY (switches, creates and removes nothing): `cargo test print_everywhere -- --ignored --nocapture`.
    /// Every window `app_windows_everywhere` finds, with what the desktop code sees of it.
    #[test]
    #[ignore]
    fn print_everywhere() {
        let wins: Vec<_> = super::app_windows_everywhere().into_iter().map(|(h, pid, cloak)| (h.0 as isize, pid, cloak)).collect();
        let rows = crate::desktops::call("List", move || {
            let ids = winvd::get_desktops()?.iter().map(|d| d.get_id()).collect::<Result<Vec<_>, _>>()?;
            Ok(wins
                .into_iter()
                .map(|(h, pid, cloak)| {
                    let hwnd = windows058::Win32::Foundation::HWND(h as *mut std::ffi::c_void);
                    let pinned = (winvd::is_pinned_window(hwnd), winvd::is_pinned_app(hwnd));
                    let at = winvd::get_desktop_by_window(hwnd).and_then(|d| d.get_id()).map(|g| ids.iter().position(|i| *i == g));
                    (h, pid, cloak, pinned, at)
                })
                .collect::<Vec<_>>())
        })
        .unwrap();
        for (h, pid, cloak, pinned, at) in rows {
            let hwnd = windows::Win32::Foundation::HWND(h as *mut std::ffi::c_void);
            let exe = super::exe_path(pid).unwrap_or_default();
            println!("{h:#x} pid {pid} cloak {cloak} pinned {pinned:?} desktop {at:?} | {} | {:?}", exe.rsplit('\\').next().unwrap_or(""), super::title(hwnd));
        }
    }

    /// `cargo test print_running -- --ignored --nocapture`: what this machine has open.
    #[test]
    #[ignore]
    fn print_running() {
        for r in super::running() {
            println!("{:>6} {:<60} {} ({} windows)", r.pid, r.exe, r.title, r.windows.len());
        }
        for app in crate::win::scan_apps().iter().filter(|a| ["Terminal", "Google Chrome", "Visual Studio Code", "File Explorer", "Notepad", "Claude"].contains(&a.name.as_str())) {
            let target = crate::win::app_target(&app.id);
            println!("{} | {} | target {:?} | open {}", app.name, app.id, target, super::windows_of(&app.id, target.as_deref()).len());
        }
    }
}
