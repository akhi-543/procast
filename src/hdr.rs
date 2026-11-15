//! HDR on and off per display (the Windows "advanced color" switch), for Toggle HDR and for
//! Auto HDR: HDR comes on while a game has focus and goes off after, but only if procast
//! was the one that turned it on.

use std::{
    sync::mpsc::{Sender, channel},
    thread,
};

use windows::Win32::{
    Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
        DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo,
        GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    },
    Foundation::{HWND, LUID},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromWindow},
};

/// One active display.
#[derive(Clone, Debug)]
pub struct HdrDisplay {
    adapter: LUID,
    id: u32,
    /// Windows' name for the display ("\\.\DISPLAY1"): matches a window's monitor to it.
    gdi: String,
    pub supported: bool,
    pub enabled: bool,
}

/// (HDR can be used, HDR is on) from the advanced-color bits: 0 supported, 1 enabled, 3 force-disabled.
fn decode(bits: u32) -> (bool, bool) {
    (bits & 1 != 0 && bits & 8 == 0, bits & 2 != 0)
}

fn name_of(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())])
}

fn header(kind: windows::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_TYPE, size: usize, adapter: LUID, id: u32) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: kind, size: size as u32, adapterId: adapter, id }
}

/// Every active display, with whether it can do HDR and whether it is on. Empty on any error.
pub fn displays() -> Vec<HdrDisplay> {
    unsafe {
        let (mut np, mut nm) = (0, 0);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).0 != 0 {
            return vec![];
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).0 != 0 {
            return vec![];
        }
        paths.truncate(np as usize);
        paths
            .iter()
            .filter_map(|p| {
                let (adapter, id) = (p.targetInfo.adapterId, p.targetInfo.id);
                let mut color = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
                color.header = header(DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, size_of_val(&color), adapter, id);
                if DisplayConfigGetDeviceInfo(&mut color.header) != 0 {
                    return None;
                }
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
                source.header = header(DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, size_of_val(&source), p.sourceInfo.adapterId, p.sourceInfo.id);
                if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                    return None;
                }
                let (supported, enabled) = decode(color.Anonymous.value);
                Some(HdrDisplay { adapter, id, gdi: name_of(&source.viewGdiDeviceName), supported, enabled })
            })
            .collect()
    }
}

/// The display a window is on.
fn display_of(all: &[HdrDisplay], hwnd: HWND) -> Option<&HdrDisplay> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };
    let name = name_of(&info.szDevice);
    all.iter().find(|d| ok.as_bool() && d.gdi == name)
}

/// Turns HDR on or off on one display.
pub fn set(target: &HdrDisplay, on: bool) -> Result<(), String> {
    let mut state = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE::default();
    state.header = header(DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE, size_of_val(&state), target.adapter, target.id);
    state.Anonymous.value = on as u32;
    match unsafe { DisplayConfigSetDeviceInfo(&state.header) } {
        0 => Ok(()),
        e => Err(format!("Windows refused to change HDR (error {e})")),
    }
}

/// Toggle HDR: flips every HDR-capable display. Says the state of the first one afterwards.
pub fn toggle() -> Result<String, String> {
    let all: Vec<HdrDisplay> = displays().into_iter().filter(|d| d.supported).collect();
    let Some(first) = all.first() else { return Err("No HDR display".into()) };
    let mut failed = None;
    for d in &all {
        if let Err(e) = set(d, !d.enabled) {
            failed.get_or_insert(e);
        }
    }
    failed.map_or_else(|| Ok(format!("HDR {}", if first.enabled { "off" } else { "on" })), Err)
}

/// What Auto HDR does on a focus change: `ours` is whether procast turned HDR on (and not
/// off since), `hdr_on` the state of the game's display.
pub fn next(auto: bool, game: bool, ours: bool, hdr_on: bool) -> Option<bool> {
    match (auto, game) {
        (false, _) => None,
        (true, true) => (!hdr_on).then_some(true),
        (true, false) => (ours && hdr_on).then_some(false),
    }
}

/// Whether procast still owns the on-state after acting: a switch it made, or one the user
/// has not undone.
fn owned(done: Option<bool>, ours: bool, hdr_on: bool) -> bool {
    done.unwrap_or(ours && hdr_on)
}

/// Present while procast owns the on-state, so a crash or restart can still switch it off.
fn flag() -> std::path::PathBuf {
    crate::config::dir().join("hdr-on")
}

fn set_flag(on: bool) {
    let _ = if on { std::fs::write(flag(), b"") } else { std::fs::remove_file(flag()) };
}

/// Auto HDR runs on its own thread, one focus change at a time (display calls take a moment).
pub struct Auto(Sender<(bool, bool, isize)>);

impl Auto {
    /// Starts the thread. If the last run left HDR on and no game has focus, switches it off.
    pub fn start() -> Self {
        let (tx, rx) = channel::<(bool, bool, isize)>();
        let _ = thread::Builder::new().spawn(move || {
            // The display procast turned HDR on for.
            let mut ours: Option<String> = None;
            if flag().exists() {
                let all = displays();
                if crate::foreground::game_in_front() {
                    ours = display_of(&all, crate::win::foreground()).map(|d| d.gdi.clone());
                } else {
                    // ponytail: which display was ours isn't stored, so every HDR display goes off.
                    all.iter().filter(|d| d.enabled).for_each(|d| _ = set(d, false));
                    set_flag(false);
                }
            }
            // ponytail: one owned display; a game moved to another HDR display leaves the first on.
            for (auto, game, hwnd) in rx {
                if !auto && ours.is_none() {
                    continue;
                }
                let all = displays();
                let target = if game { display_of(&all, HWND(hwnd as *mut _)) } else { ours.as_ref().and_then(|g| all.iter().find(|d| &d.gdi == g)) };
                let Some(t) = target.filter(|t| t.supported) else {
                    if !game && ours.take().is_some() {
                        set_flag(false);
                    }
                    continue;
                };
                let mine = ours.as_ref() == Some(&t.gdi);
                let want = next(auto, game, mine, t.enabled);
                let done = want.filter(|&on| set(t, on).is_ok());
                if want.is_some() && done.is_none() {
                    continue; // Windows refused: try again at the next change.
                }
                if owned(done, mine, t.enabled) {
                    ours = Some(t.gdi.clone());
                } else if mine {
                    ours = None;
                }
                set_flag(ours.is_some());
            }
        });
        Self(tx)
    }

    /// The foreground window changed.
    pub fn foreground(&self, auto: bool, game: bool, hwnd: isize) {
        let _ = self.0.send((auto, game, hwnd));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_hdr_decisions() {
        // A game in front with HDR off: on. Already on: leave it.
        assert_eq!(next(true, true, false, false), Some(true));
        assert_eq!(next(true, true, false, true), None);
        // The game is gone and procast turned it on: off.
        assert_eq!(next(true, false, true, true), Some(false));
        // HDR the user turned on is never turned off.
        assert_eq!(next(true, false, false, true), None);
        assert_eq!(next(true, false, false, false), None);
        // The user already turned it off again.
        assert_eq!(next(true, false, true, false), None);
        // Off means off, whatever the state.
        for (game, ours, on) in [(true, false, false), (false, true, true), (true, true, true), (false, false, true)] {
            assert_eq!(next(false, game, ours, on), None);
        }
    }

    #[test]
    fn ownership_ends_when_the_user_switches_off() {
        assert!(owned(Some(true), false, false)); // we just turned it on
        assert!(!owned(Some(false), true, true)); // we just turned it off
        assert!(owned(None, true, true)); // untouched
        assert!(!owned(None, true, false)); // the user turned it off
        assert!(!owned(None, false, true)); // theirs, not ours
    }

    #[test]
    fn advanced_color_bits() {
        assert_eq!(decode(0b0011), (true, true));
        assert_eq!(decode(0b0001), (true, false));
        assert_eq!(decode(0), (false, false));
        assert_eq!(decode(0b1001), (false, false)); // force-disabled
    }

    #[test]
    #[ignore = "reads this machine's displays (changes nothing); run with --ignored --nocapture"]
    fn displays_are_found() {
        let all = displays();
        println!("{all:?}");
        let fg = display_of(&all, crate::win::foreground()).map(|d| d.gdi.clone());
        println!("foreground window is on {fg:?}");
        assert!(!all.is_empty());
    }
}
