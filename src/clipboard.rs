//! Clipboard history: text only, newest first, encrypted at rest with DPAPI (user scope).

use std::{fs, path::PathBuf, time::Duration};

use futures::channel::mpsc::UnboundedSender;
use serde::{Deserialize, Serialize};
use windows::{
    Win32::{
        Foundation::{HANDLE, HGLOBAL, HLOCAL, LocalFree},
        Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData},
        System::{
            DataExchange::{
                CloseClipboard, GetClipboardData, GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
                OpenClipboard, RegisterClipboardFormatW,
            },
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Ole::CF_UNICODETEXT,
            Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
        },
        UI::{
            Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_V},
            WindowsAndMessaging::GetWindowThreadProcessId,
        },
    },
    core::{PWSTR, w},
};

use crate::win::Msg;

const MAX_ENTRY_BYTES: usize = 64 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub text: String,
    pub at: u64,
}

pub struct History {
    path: PathBuf,
    pub entries: Vec<Entry>,
    cap: usize,
}

impl History {
    pub fn load(path: PathBuf, cap: usize) -> Self {
        let entries = fs::read(&path).ok().and_then(|b| unprotect(&b)).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self { path, entries, cap }
    }

    pub fn push(&mut self, text: String, at: u64) {
        if text.trim().is_empty() || text.len() > MAX_ENTRY_BYTES {
            return;
        }
        self.entries.retain(|e| e.text != text);
        self.entries.insert(0, Entry { text, at });
        self.entries.truncate(self.cap.max(1));
        self.save();
    }

    pub fn remove(&mut self, text: &str) {
        self.entries.retain(|e| e.text != text);
        self.save();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.save();
    }

    fn save(&self) {
        if let Some(bytes) = serde_json::to_vec(&self.entries).ok().and_then(|j| protect(&j)) {
            let _ = fs::write(&self.path, bytes);
        }
    }
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe { _ = LocalFree(Some(HLOCAL(out.pbData as _))) };
    v
}

fn protect(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptProtectData(&blob(data), w!("akscast clipboard"), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()? };
    Some(take(out))
}

fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptUnprotectData(&blob(data), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()? };
    Some(take(out))
}

/// Polls the clipboard sequence number and forwards new text.
// ponytail: 300 ms poll; switch to AddClipboardFormatListener on a message-only window
// if the latency or wakeups ever matter.
pub fn watch(ignore_apps: Vec<String>, tx: UnboundedSender<Msg>) {
    std::thread::spawn(move || {
        let ignore: Vec<String> = ignore_apps.iter().map(|a| a.to_lowercase()).collect();
        let mut last = unsafe { GetClipboardSequenceNumber() };
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let seq = unsafe { GetClipboardSequenceNumber() };
            if seq == last {
                continue;
            }
            last = seq;
            if let Some(text) = read_text(&ignore)
                && tx.unbounded_send(Msg::Clip(text)).is_err()
            {
                return;
            }
        }
    });
}

/// Formats password managers set to opt out of clipboard history.
fn excluded() -> bool {
    unsafe {
        for name in [w!("ExcludeClipboardContentFromMonitorProcessing"), w!("Clipboard Viewer Ignore")] {
            if IsClipboardFormatAvailable(RegisterClipboardFormatW(name)).is_ok() {
                return true;
            }
        }
        let can = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        if IsClipboardFormatAvailable(can).is_ok()
            && let Ok(h) = GetClipboardData(can)
        {
            let p = GlobalLock(HGLOBAL(h.0)) as *const u32;
            let allowed = p.is_null() || *p != 0;
            let _ = GlobalUnlock(HGLOBAL(h.0));
            return !allowed;
        }
        false
    }
}

fn owner_exe() -> String {
    unsafe {
        let Ok(owner) = GetClipboardOwner() else { return String::new() };
        let mut pid = 0;
        GetWindowThreadProcessId(owner, Some(&mut pid));
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let _ = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().unwrap_or_default().to_lowercase()
    }
}

fn read_text(ignore: &[String]) -> Option<String> {
    unsafe {
        // Another app may hold the clipboard open for a moment.
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !opened {
            return None;
        }
        let text = (|| {
            if excluded() || ignore.contains(&owner_exe()) {
                return None;
            }
            let h: HANDLE = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() {
                return None;
            }
            let max = GlobalSize(g) / 2;
            let s = std::slice::from_raw_parts(p, max);
            let len = s.iter().position(|&c| c == 0).unwrap_or(max);
            let text = String::from_utf16_lossy(&s[..len]);
            let _ = GlobalUnlock(g);
            Some(text)
        })();
        let _ = CloseClipboard();
        text
    }
}

/// Ctrl+V into whatever window has focus, after it has had a moment to get it back.
pub fn paste_soon() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(80));
        let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, ..Default::default() } },
        };
        let seq = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
        unsafe { SendInput(&seq, size_of::<INPUT>() as i32) };
    });
}
