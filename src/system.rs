//! System commands beyond power and theme: volume, radios, Explorer, drives, temp files,
//! and the Windows Settings pages. Everything here runs off the UI thread.

use std::path::Path;

use windows::{
    Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState},
    Win32::{
        Foundation::CloseHandle,
        Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW, OPEN_EXISTING},
        System::{
            IO::DeviceIoControl,
            Ioctl::{IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY, StorageDeviceProperty},
        },
        UI::{
            Input::KeyboardAndMouse::{INPUT, SendInput, VIRTUAL_KEY, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP},
            WindowsAndMessaging::FindWindowW,
        },
    },
    core::{HSTRING, w},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Volume {
    Up,
    Down,
    Mute,
}

/// Presses a media volume key: Windows changes the volume and shows its own flyout.
pub fn volume(v: Volume) {
    let vk = match v {
        Volume::Up => VK_VOLUME_UP,
        Volume::Down => VK_VOLUME_DOWN,
        Volume::Mute => VK_VOLUME_MUTE,
    };
    tap(vk);
}

fn tap(vk: VIRTUAL_KEY) {
    let key = crate::clipboard::key;
    unsafe { SendInput(&[key(vk, false), key(vk, true)], size_of::<INPUT>() as i32) };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RadioOf {
    WiFi,
    Bluetooth,
}

/// Turns the first radio of that kind on or off, whichever it isn't. Returns "Wi-Fi on" and the like.
pub fn toggle_radio(of: RadioOf) -> Result<String, String> {
    let (kind, name) = match of {
        RadioOf::WiFi => (RadioKind::WiFi, "Wi-Fi"),
        RadioOf::Bluetooth => (RadioKind::Bluetooth, "Bluetooth"),
    };
    let fail = |e: windows::core::Error| format!("{name}: {}", e.message());
    let access = Radio::RequestAccessAsync().and_then(|a| a.join()).map_err(fail)?;
    if access != RadioAccessStatus::Allowed {
        return Err(format!("Windows doesn't let apps switch {name}: Settings → Privacy & security → Radios"));
    }
    let radios = Radio::GetRadiosAsync().and_then(|a| a.join()).map_err(fail)?;
    let radio = radios.into_iter().find(|r| r.Kind().is_ok_and(|k| k == kind)).ok_or_else(|| format!("No {name} adapter"))?;
    let on = radio.State().map_err(fail)? != RadioState::On;
    let done = radio.SetStateAsync(if on { RadioState::On } else { RadioState::Off }).and_then(|a| a.join()).map_err(fail)?;
    if done != RadioAccessStatus::Allowed {
        return Err(format!("Could not switch {name}"));
    }
    Ok(format!("{name} {}", if on { "on" } else { "off" }))
}

/// Flips Explorer's "Show hidden files". Through the shell, not the registry: Explorer
/// caches the setting and only this tells open windows. Returns the new state.
pub fn toggle_hidden_files() -> bool {
    use windows::Win32::UI::Shell::{SHELLSTATEA, SHGetSetSettings, SSF_SHOWALLOBJECTS};
    let mut ss = SHELLSTATEA::default();
    unsafe {
        SHGetSetSettings(Some(&mut ss), SSF_SHOWALLOBJECTS, false);
        // Bit 0 is fShowAllObjects.
        ss._bitfield1 ^= 1;
        SHGetSetSettings(Some(&mut ss), SSF_SHOWALLOBJECTS, true);
    }
    ss._bitfield1 & 1 == 1
}

/// Ends Explorer (taskbar, desktop and File Explorer windows) and starts it again.
pub fn restart_explorer() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let killed = std::process::Command::new("taskkill").args(["/f", "/im", "explorer.exe"]).creation_flags(CREATE_NO_WINDOW).status();
    if !killed.is_ok_and(|s| s.success()) {
        return Err("Could not end Explorer".into());
    }
    // Windows usually brings the shell back by itself; start it only if it didn't.
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if unsafe { FindWindowW(w!("Shell_TrayWnd"), None) }.is_ok() {
            return Ok(());
        }
    }
    std::process::Command::new("explorer.exe").spawn().map(drop).map_err(|e| format!("Could not start Explorer: {e}"))
}

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
/// STORAGE_BUS_TYPE BusTypeUsb.
const BUS_USB: i32 = 7;

/// Ejects every USB stick and USB disk, like "Safely Remove Hardware". Returns the letters ejected.
pub fn eject_drives() -> Result<String, String> {
    let drives: Vec<char> = (b'A'..=b'Z').map(char::from).filter(|&d| ejectable(d)).collect();
    if drives.is_empty() {
        return Err("No USB drive to eject".into());
    }
    let (mut done, mut busy) = (vec![], vec![]);
    for d in drives {
        if eject(d) { done.push(format!("{d}:")) } else { busy.push(format!("{d}:")) }
    }
    match (done.is_empty(), busy.is_empty()) {
        (_, true) => Ok(format!("Ejected {}", done.join(" "))),
        (true, false) => Err(format!("{} is in use", busy.join(" "))),
        (false, false) => Err(format!("Ejected {}; {} is in use", done.join(" "), busy.join(" "))),
    }
}

/// A removable drive, or a fixed one on the USB bus (external disks report as fixed).
fn ejectable(d: char) -> bool {
    let kind = unsafe { GetDriveTypeW(&HSTRING::from(format!("{d}:\\"))) };
    kind == DRIVE_REMOVABLE || kind == DRIVE_FIXED && bus_type(d) == Some(BUS_USB)
}

fn bus_type(d: char) -> Option<i32> {
    unsafe {
        // No access rights needed to ask about the device.
        let h = CreateFileW(&HSTRING::from(format!(r"\\.\{d}:")), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None).ok()?;
        let query = STORAGE_PROPERTY_QUERY { PropertyId: StorageDeviceProperty, QueryType: PropertyStandardQuery, ..Default::default() };
        let mut out = [0u8; 1024];
        let mut n = 0u32;
        let ok = DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query as *const _ as _),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(out.as_mut_ptr() as _),
            out.len() as u32,
            Some(&mut n),
            None,
        );
        let _ = CloseHandle(h);
        ok.ok()?;
        (n as usize >= size_of::<STORAGE_DEVICE_DESCRIPTOR>()).then(|| (*(out.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR)).BusType.0)
    }
}

/// Explorer's Eject verb on the drive. It refuses (and says so itself) while files are open.
fn eject(d: char) -> bool {
    use windows::Win32::{
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
        UI::{
            Shell::{BHID_SFUIObject, CMINVOKECOMMANDINFO, IContextMenu, IShellItem, SHCreateItemFromParsingName},
            WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, SW_HIDE},
        },
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(format!("{d}:\\")), None) else { return false };
        let Ok(menu) = item.BindToHandler::<_, IContextMenu>(None, &BHID_SFUIObject) else { return false };
        // Handlers only answer verbs they've been asked to put on a menu.
        let Ok(popup) = CreatePopupMenu() else { return false };
        let _ = menu.QueryContextMenu(popup, 0, 1, 0x7FFF, 0);
        let info = CMINVOKECOMMANDINFO { cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32, lpVerb: windows::core::s!("eject"), nShow: SW_HIDE.0, ..Default::default() };
        let ok = menu.InvokeCommand(&info).is_ok();
        let _ = DestroyMenu(popup);
        // The verb returns before the volume is gone; give it a moment, then check.
        for _ in 0..30 {
            if !ok || !ejectable(d) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        ok && !ejectable(d)
    }
}

/// Deletes what it can inside `dir` (files in use stay). Returns bytes freed.
pub fn empty_dir(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut freed = 0;
    for e in entries.flatten() {
        let path = e.path();
        // Not following links: a junction in Temp must not empty its target.
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if meta.is_dir() && !meta.file_type().is_symlink() {
            freed += empty_dir(&path);
            let _ = std::fs::remove_dir(&path);
        } else if std::fs::remove_file(&path).is_ok() || meta.is_symlink() && std::fs::remove_dir(&path).is_ok() {
            freed += meta.len();
        }
    }
    freed
}

/// Empties the user's temp folder; returns "Freed 1.2 GB" and the like.
pub fn empty_temp() -> String {
    let freed = empty_dir(&std::env::temp_dir());
    format!("Freed {}", size_text(freed))
}

fn size_text(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.0} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{} KB", b.div_ceil(1024)),
    }
}

/// Windows Settings pages: (title, `ms-settings:` page, extra search words).
pub const SETTINGS_PAGES: &[(&str, &str, &str)] = &[
    ("Display", "display", "screen resolution scale brightness monitor hdr"),
    ("Night Light", "nightlight", "blue light warm"),
    ("Sound", "sound", "audio speakers microphone output input"),
    ("Volume Mixer", "apps-volume", "audio app sound levels"),
    ("Notifications", "notifications", "alerts do not disturb"),
    ("Focus", "quiethours", "do not disturb focus assist"),
    ("Power & Battery", "powersleep", "sleep screen timeout energy battery"),
    ("Storage", "storagesense", "disk space cleanup"),
    ("Clipboard", "clipboard", "copy paste history"),
    ("Multitasking", "multitasking", "snap windows alt tab"),
    ("About This PC", "about", "system info name specs windows version"),
    ("Bluetooth & Devices", "bluetooth", "pair devices"),
    ("Printers & Scanners", "printers", "print"),
    ("Mouse", "mousetouchpad", "pointer scroll"),
    ("Touchpad", "devices-touchpad", "gestures trackpad"),
    ("Typing", "typing", "keyboard autocorrect"),
    ("AutoPlay", "autoplay", "usb removable"),
    ("Network & Internet", "network", "internet connection"),
    ("Wi-Fi", "network-wifi", "wireless wlan"),
    ("Ethernet", "network-ethernet", "wired lan"),
    ("VPN", "network-vpn", "private network"),
    ("Proxy", "network-proxy", "network"),
    ("Mobile Hotspot", "network-mobilehotspot", "share internet tethering"),
    ("Airplane Mode", "network-airplanemode", "flight radios"),
    ("Personalization", "personalization", "look"),
    ("Background", "personalization-background", "wallpaper desktop picture"),
    ("Colors", "colors", "accent dark light mode theme"),
    ("Themes", "themes", "theme"),
    ("Lock Screen", "lockscreen", "lock screen picture"),
    ("Start", "personalization-start", "start menu"),
    ("Taskbar", "taskbar", "tray pinned"),
    ("Fonts", "fonts", "typeface"),
    ("Installed Apps", "appsfeatures", "uninstall programs remove"),
    ("Default Apps", "defaultapps", "browser open with"),
    ("Startup Apps", "startupapps", "login boot"),
    ("Optional Features", "optionalfeatures", "add features"),
    ("Your Info", "yourinfo", "account profile microsoft"),
    ("Sign-in Options", "signinoptions", "password pin windows hello"),
    ("Email & Accounts", "emailandaccounts", "accounts"),
    ("Date & Time", "dateandtime", "clock time zone"),
    ("Language & Region", "regionlanguage", "locale keyboard layout"),
    ("Speech", "speech", "voice"),
    ("Gaming", "gaming-gamebar", "game bar xbox"),
    ("Game Mode", "gaming-gamemode", "games performance"),
    ("Accessibility", "easeofaccess", "ease of access"),
    ("Text Size", "easeofaccess-display", "bigger font accessibility"),
    ("Privacy & Security", "privacy", "permissions"),
    ("Camera Privacy", "privacy-webcam", "webcam permissions"),
    ("Microphone Privacy", "privacy-microphone", "permissions"),
    ("Location", "privacy-location", "gps permissions"),
    ("Windows Security", "windowsdefender", "antivirus defender firewall"),
    ("Windows Update", "windowsupdate", "updates patches"),
    ("Recovery", "recovery", "reset pc"),
    ("Activation", "activation", "license product key"),
    ("For Developers", "developers", "developer mode sudo"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dir_deletes_inside_only() {
        let root = crate::test_dir("empty");
        let keep = root.join("keep");
        let temp = root.join("temp");
        std::fs::create_dir_all(temp.join("sub")).unwrap();
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(temp.join("a.txt"), [0u8; 2048]).unwrap();
        std::fs::write(temp.join("sub").join("b.txt"), [0u8; 1024]).unwrap();
        std::fs::write(keep.join("c.txt"), b"keep").unwrap();
        assert_eq!(empty_dir(&temp), 3072);
        assert!(temp.exists() && std::fs::read_dir(&temp).unwrap().next().is_none(), "the folder stays, empty");
        assert!(keep.join("c.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(size_text(0), "0 KB");
        assert_eq!(size_text(1500), "2 KB");
        assert_eq!(size_text(5 << 20), "5 MB");
        assert_eq!(size_text(3 << 29), "1.5 GB");
    }

    #[test]
    #[ignore = "reads this machine's radios and drives (changes nothing); run with --ignored"]
    fn radios_and_drives_are_found() {
        let access = Radio::RequestAccessAsync().unwrap().join().unwrap();
        let radios: Vec<(String, i32, i32)> = Radio::GetRadiosAsync().unwrap().join().unwrap().into_iter().map(|r| (r.Name().unwrap().to_string(), r.Kind().unwrap().0, r.State().unwrap().0)).collect();
        let drives: Vec<(char, Option<i32>)> = (b'A'..=b'Z').map(char::from).filter(|&d| unsafe { GetDriveTypeW(&HSTRING::from(format!("{d}:\\"))) } >= DRIVE_REMOVABLE).map(|d| (d, bus_type(d))).collect();
        println!("access {access:?}; radios (name, kind, state) {radios:?}; drives (letter, bus) {drives:?}; ejectable {:?}", drives.iter().filter(|d| ejectable(d.0)).collect::<Vec<_>>());
        assert_eq!(access, RadioAccessStatus::Allowed);
    }

    #[test]
    fn settings_pages_are_unique() {
        let mut pages: Vec<&str> = SETTINGS_PAGES.iter().map(|p| p.1).collect();
        pages.sort();
        pages.dedup();
        assert_eq!(pages.len(), SETTINGS_PAGES.len());
    }
}
