//! A low-level keyboard hook for what RegisterHotKey can't do: the Hyper key (one key
//! held = Ctrl+Alt+Shift+Win), double-tapping a modifier to open procast, and snippet
//! keywords typed in any app (snippets.rs).
//! The hook runs only while one of them is switched on; its callback must stay quick,
//! since every keystroke on the machine passes through it.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use futures::channel::mpsc::UnboundedSender;
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetKeyState, GetKeyboardLayout, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
            SendInput, ToUnicodeEx, VIRTUAL_KEY,
        },
        WindowsAndMessaging::{
            CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PostThreadMessageW,
            SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
        },
    },
};

use crate::{snippets::Expander, win::Msg};

/// Keys the Hyper key can be. Values are what config.toml stores.
pub const HYPER_KEYS: &[&str] = &["off", "capslock", "rightalt", "rightctrl", "scrolllock"];
/// Modifiers a double tap can use.
pub const TAP_KEYS: &[&str] = &["off", "ctrl", "shift", "alt"];

const CTRL: u16 = 0xA2;
const RCTRL: u16 = 0xA3;
const SHIFT: u16 = 0xA0;
const RSHIFT: u16 = 0xA1;
const ALT: u16 = 0xA4;
const RALT: u16 = 0xA5;
const LWIN: u16 = 0x5B;
/// `dwExtraInfo` on every key procast sends, so the hook can tell them from typing.
pub const OURS: usize = 0x7072_6f63;
/// Unassigned virtual key: pressed between Win down and up so Windows doesn't open Start.
const NOTHING: u16 = 0xE8;
/// A tap is a press shorter than this; two taps closer than `GAP` open procast.
const TAP: Duration = Duration::from_millis(250);
const GAP: Duration = Duration::from_millis(400);

fn hyper_vk(name: &str) -> Option<u16> {
    Some(match name {
        "capslock" => 0x14,
        "rightalt" => RALT,
        "rightctrl" => RCTRL,
        "scrolllock" => 0x91,
        _ => return None,
    })
}

fn tap_vks(name: &str) -> &'static [u16] {
    match name {
        "ctrl" => &[CTRL, RCTRL],
        "shift" => &[SHIFT, RSHIFT],
        "alt" => &[ALT, RALT],
        _ => &[],
    }
}

/// The hook's state machine, separate from Win32 so it can be tested.
#[derive(Default)]
struct State {
    hyper: Option<u16>,
    tap: &'static [u16],
    /// Hyper key is held (its modifiers are down).
    holding: bool,
    /// Another key was pressed while Hyper was held.
    used: bool,
    /// The tap modifier went down at, with no other key since.
    tap_down: Option<Instant>,
    last_tap: Option<Instant>,
}

/// What the hook should do with one key event.
#[derive(Debug, PartialEq)]
enum Out {
    Pass,
    /// Swallow it and press (true) or release the Hyper modifiers; `nothing` taps the
    /// spare key first so a lone Win release doesn't open Start.
    Hyper { down: bool, nothing: bool },
    Swallow,
    /// Pass it on; the double tap just completed.
    Open,
}

impl State {
    fn key(&mut self, vk: u16, down: bool, now: Instant) -> Out {
        if Some(vk) == self.hyper {
            return match (down, self.holding) {
                (true, true) => Out::Swallow, // auto-repeat
                (true, false) => {
                    self.holding = true;
                    self.used = false;
                    Out::Hyper { down: true, nothing: false }
                }
                (false, _) => {
                    self.holding = false;
                    Out::Hyper { down: false, nothing: !self.used }
                }
            };
        }
        if self.tap.contains(&vk) {
            if down {
                self.tap_down.get_or_insert(now);
            } else if let Some(at) = self.tap_down.take() {
                if now - at < TAP {
                    if self.last_tap.is_some_and(|t| now - t < GAP) {
                        self.last_tap = None;
                        return Out::Open;
                    }
                    self.last_tap = Some(now);
                } else {
                    self.last_tap = None;
                }
            }
            return Out::Pass;
        }
        if down {
            self.used = true;
            self.tap_down = None;
            self.last_tap = None;
        }
        Out::Pass
    }
}

struct Shared {
    state: State,
    expander: Option<Expander>,
    /// The window typing last went to; a new one starts the typed text over.
    window: isize,
    tx: Option<UnboundedSender<Msg>>,
}

static SHARED: Mutex<Shared> =
    Mutex::new(Shared { state: State { hyper: None, tap: &[], holding: false, used: false, tap_down: None, last_tap: None }, expander: None, window: 0, tx: None });
static THREAD: Mutex<u32> = Mutex::new(0);

/// Applies the settings: starts the hook when any feature is on, stops it otherwise.
pub fn configure(hyper: &str, double_tap: &str, expander: Option<Expander>, tx: UnboundedSender<Msg>) {
    let (hyper, tap) = (hyper_vk(hyper), tap_vks(double_tap));
    let expander = expander.filter(|x| !x.is_idle());
    let expands = expander.is_some();
    {
        let mut s = SHARED.lock().unwrap();
        s.state = State { hyper, tap, ..Default::default() };
        s.expander = expander;
        s.tx = Some(tx);
    }
    let mut thread = THREAD.lock().unwrap();
    let wanted = hyper.is_some() || !tap.is_empty() || expands;
    if wanted && *thread == 0 {
        let (tid_tx, tid_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || unsafe {
            let _ = tid_tx.send(GetCurrentThreadId());
            let Ok(hook) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), None, 0) else { return };
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
            let _ = UnhookWindowsHookEx(hook);
        });
        *thread = tid_rx.recv().unwrap_or(0);
    } else if !wanted && *thread != 0 {
        unsafe { _ = PostThreadMessageW(*thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        *thread = 0;
    }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        // Our own SendInput comes back through here; let it be. Keys other programs send
        // (the touch keyboard, remote tools) still type snippet keywords, but never act as Hyper.
        let injected = k.flags.0 & LLKHF_INJECTED.0 != 0;
        if !(injected && k.dwExtraInfo == OURS) {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let (out, tx, hit) = {
                let mut s = SHARED.lock().unwrap();
                let out = if injected { Out::Pass } else { s.state.key(k.vkCode as u16, down, Instant::now()) };
                let hit = if down && out == Out::Pass { typed(&mut s, k) } else { None };
                (out, s.tx.clone(), hit)
            };
            if let (Some(hit), Some(tx)) = (hit, &tx) {
                let _ = tx.unbounded_send(Msg::Expand(hit));
            }
            match out {
                Out::Pass => {}
                Out::Open => {
                    if let Some(tx) = tx {
                        let _ = tx.unbounded_send(Msg::Hotkey(0));
                    }
                }
                Out::Swallow => return LRESULT(1),
                Out::Hyper { down, nothing } => {
                    send_hyper(down, nothing);
                    return LRESULT(1);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Feeds one key press to the snippet expander as the character it types in the focused app.
fn typed(s: &mut Shared, k: &KBDLLHOOKSTRUCT) -> Option<crate::snippets::Hit> {
    let x = s.expander.as_mut()?;
    let fg = unsafe { GetForegroundWindow() };
    if fg.0 as isize != s.window {
        s.window = fg.0 as isize;
        x.reset();
    }
    let held = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) } < 0;
    let (ctrl, alt, win) = (held(0x11), held(0x12), held(LWIN) || held(0x5C));
    match k.vkCode as u16 {
        // Modifiers and lock keys type nothing.
        0x10..=0x12 | 0x14 | 0x5B | 0x5C | 0x90 | 0x91 | SHIFT..=RALT => return None,
        0x08 if !ctrl && !alt => {
            x.backspace();
            return None;
        }
        _ if win || ctrl != alt => {
            // A shortcut (Ctrl+V, Alt+Tab, …), not typing. Ctrl+Alt is AltGr, which types.
            x.reset();
            return None;
        }
        _ => {}
    }
    let mut state = [0u8; 256];
    for vk in [0x10u16, 0x11, 0x12, SHIFT, RSHIFT, CTRL, RCTRL, ALT, RALT] {
        if held(vk) {
            state[vk as usize] = 0x80;
        }
    }
    state[0x14] = (unsafe { GetKeyState(0x14) } & 1) as u8;
    let mut buf = [0u16; 8];
    let hkl = unsafe { GetKeyboardLayout(GetWindowThreadProcessId(fg, None)) };
    // Flag 4: leave the keyboard state (and so dead keys) as the app will need it.
    let n = unsafe { ToUnicodeEx(k.vkCode, k.scanCode, &state, &mut buf, 4, Some(hkl)) };
    let c = (n == 1).then(|| char::from_u32(buf[0] as u32)).flatten().filter(|c| !c.is_control());
    match c {
        Some(c) => x.typed(c),
        // Enter, Tab, arrows, Escape: the caret may be anywhere now.
        None => {
            x.reset();
            None
        }
    }
}

fn send_hyper(down: bool, nothing: bool) {
    let key = |vk: u16, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, dwExtraInfo: OURS, ..Default::default() } },
    };
    let mods = [CTRL, SHIFT, ALT, LWIN];
    let mut inputs: Vec<INPUT> = vec![];
    if down {
        inputs.extend(mods.iter().map(|&m| key(m, false)));
    } else {
        if nothing {
            inputs.extend([key(NOTHING, false), key(NOTHING, true)]);
        }
        inputs.extend(mods.iter().rev().map(|&m| key(m, true)));
    }
    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyper_and_double_tap() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut s = State { hyper: hyper_vk("capslock"), tap: tap_vks("ctrl"), ..Default::default() };

        // Hyper: press, auto-repeat, another key, release.
        assert_eq!(s.key(0x14, true, ms(0)), Out::Hyper { down: true, nothing: false });
        assert_eq!(s.key(0x14, true, ms(30)), Out::Swallow);
        assert_eq!(s.key(b'K' as u16, true, ms(40)), Out::Pass);
        assert_eq!(s.key(0x14, false, ms(80)), Out::Hyper { down: false, nothing: false });
        // Alone: the spare key stops Start from opening.
        s.key(0x14, true, ms(100));
        assert_eq!(s.key(0x14, false, ms(150)), Out::Hyper { down: false, nothing: true });

        // Double tap: two quick Ctrl taps open; a key in between or a slow press doesn't.
        s.key(CTRL, true, ms(1000));
        assert_eq!(s.key(CTRL, false, ms(1050)), Out::Pass);
        s.key(RCTRL, true, ms(1200));
        assert_eq!(s.key(RCTRL, false, ms(1250)), Out::Open);
        s.key(CTRL, true, ms(2000));
        s.key(CTRL, false, ms(2050));
        s.key(CTRL, true, ms(2100));
        s.key(b'C' as u16, true, ms(2120)); // Ctrl+C
        assert_eq!(s.key(CTRL, false, ms(2150)), Out::Pass);
        s.key(CTRL, true, ms(3000));
        s.key(CTRL, false, ms(3600)); // held, not tapped
        s.key(CTRL, true, ms(3700));
        assert_eq!(s.key(CTRL, false, ms(3750)), Out::Pass);
    }
}
