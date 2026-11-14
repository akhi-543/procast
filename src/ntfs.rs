//! Whole-drive file search from the NTFS master file table, kept live by the USN change
//! journal (the approach Everything uses). Reading a volume needs admin, so an elevated
//! helper (`procast-indexer.exe --indexer`, started at logon by a scheduled task) streams
//! file records over a named pipe; procast keeps the index and searches it.
//!
//! Security: the helper runs from Program Files (only admins can replace it), only does
//! the privileged reads, and its pipe admits only the installing user. procast checks the
//! pipe's server is that helper before trusting it.

use std::{
    io::Read,
    os::windows::io::{AsRawHandle, FromRawHandle},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use futures::channel::mpsc::UnboundedSender;
use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GENERIC_READ, GetLastError, HANDLE, HLOCAL, LocalFree},
        Security::{
            Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
            GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ,
            FILE_SHARE_WRITE, GetDriveTypeW, GetVolumeInformationW, OPEN_EXISTING, PIPE_ACCESS_OUTBOUND, WriteFile,
        },
        System::{
            IO::{CancelIoEx, DeviceIoControl},
            Ioctl::{
                CREATE_USN_JOURNAL_DATA, FSCTL_CREATE_USN_JOURNAL, FSCTL_ENUM_USN_DATA, FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL,
                MFT_ENUM_DATA_V0, READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0, USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE,
                USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME, USN_RECORD_V2,
            },
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId, PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            },
            Threading::{CreateMutexW, GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
        },
    },
    core::{HSTRING, PWSTR, w},
};

use crate::{files, search::Item, win::Msg};

const PIPE: &str = r"\\.\pipe\procast-ntfs";
const TASK: &str = r"procast\Indexer";

/// The helper's home: under Program Files, so only admins can change what runs elevated.
pub fn helper_path() -> PathBuf {
    PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into())).join("procast").join("procast-indexer.exe")
}

pub fn installed() -> bool {
    helper_path().exists()
}

/// The helper as installed under the app's old name (task, folder, program).
fn old_helper() -> (String, PathBuf) {
    let old = crate::config::OLD_NAME;
    let dir = PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into())).join(old);
    (format!(r"{old}\Indexer"), dir.join(format!("{old}-indexer.exe")))
}

/// Installed under the old name and not yet replaced: whole-drive search is off until
/// "Enable Fast File Search" runs once more (it needs admin).
pub fn old_helper_installed() -> bool {
    !installed() && old_helper().1.exists()
}

/// Ends and removes the helper installed under the old name (elevated).
fn remove_old_helper() {
    let (task, exe) = old_helper();
    if !exe.exists() {
        return;
    }
    let _ = schtasks(&["/End", "/TN", &task]);
    let _ = schtasks(&["/Delete", "/TN", &task, "/F"]);
    for _ in 0..20 {
        if !exe.exists() || std::fs::remove_file(&exe).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = std::fs::remove_dir(exe.parent().unwrap());
}

// ---- wire format ----------------------------------------------------------------------
// Per record: kind u8, drive letter u8, MFT record u64, parent record u64, flags u8,
// name length u16, UTF-8 name. Flags: bit 0 folder, bit 1 hidden. (Helpers from before
// the hidden bit sent just 0 or 1, which still reads right.)

const UPSERT: u8 = 0;
const DELETE: u8 = 1;
/// A drive's snapshot is complete; live changes follow.
const READY: u8 = 2;
/// First record of every connection: the helper's protocol version, in the record field.
/// Helpers from before it never send one.
const HELLO: u8 = 3;
/// 2: flags byte with the hidden bit, removable drives on request.
const HELPER_VERSION: u64 = 2;
const W_DIR: u8 = 1;
const W_HIDDEN: u8 = 2;
// GetDriveTypeW results.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const HEADER: usize = 1 + 1 + 8 + 8 + 1 + 2;

fn put(out: &mut Vec<u8>, kind: u8, drive: u8, rec: u64, parent: u64, flags: u8, name: &str) {
    out.extend([kind, drive]);
    out.extend(rec.to_le_bytes());
    out.extend(parent.to_le_bytes());
    out.push(flags);
    out.extend((name.len() as u16).to_le_bytes());
    out.extend(name.as_bytes());
}

/// A file reference number's MFT record (the low 48 bits; the rest is a reuse counter).
fn record(frn: u64) -> u64 {
    frn & 0xFFFF_FFFF_FFFF
}

/// The root folder's MFT record.
const ROOT: u64 = 5;

// ---- the elevated helper ----------------------------------------------------------------

struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// One client connection; drive threads share it.
struct Pipe {
    h: Handle,
    lock: Mutex<()>,
}

impl Pipe {
    fn write(&self, mut buf: &[u8]) -> Option<()> {
        let _held = self.lock.lock().unwrap();
        while !buf.is_empty() {
            let mut n = 0u32;
            unsafe { WriteFile(self.h.0, Some(buf), Some(&mut n), None) }.ok()?;
            buf = &buf[n as usize..];
        }
        Some(())
    }

    /// Ends the connection; procast reconnects and gets a fresh snapshot.
    fn kill(&self) {
        unsafe {
            let _ = DisconnectNamedPipe(self.h.0);
        }
    }
}

/// `--indexer`: serve every client a snapshot of each NTFS drive, then live changes.
pub fn serve() {
    unsafe {
        let _ = CreateMutexW(None, true, w!("Local\\procast-indexer"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
    }
    let Some(sid) = user_sid() else { return };
    // SYSTEM, Administrators and the user who runs the helper; nobody else.
    let sddl = HSTRING::from(format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{sid})"));
    let mut sd = PSECURITY_DESCRIPTOR::default();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut sd, None) }.is_err() {
        return;
    }
    let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
    let mut first = true;
    loop {
        // The first instance claims the name, so nothing else can be listening on it.
        let flags = if first { PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE } else { PIPE_ACCESS_OUTBOUND };
        let h = unsafe {
            CreateNamedPipeW(&HSTRING::from(PIPE), flags, PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, 1 << 20, 0, 0, Some(&sa))
        };
        if h.is_invalid() {
            return;
        }
        first = false;
        let h = Handle(h);
        if unsafe { ConnectNamedPipe(h.0, None) }.is_err() && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            continue;
        }
        let pipe = Arc::new(Pipe { h, lock: Mutex::new(()) });
        let mut hello = vec![];
        put(&mut hello, HELLO, 0, HELPER_VERSION, 0, 0, "");
        if pipe.write(&hello).is_none() {
            continue;
        }
        // The pipe only flows outward, so the user's setting is read here, per connection. An
        // unreadable or invalid config means no removable drives.
        // ponytail: removable drives are a snapshot: no journal is followed, so no handle stays open
        // to veto "Safely remove". Changes on a stick show at the next helper connection (rescan).
        let removable = crate::config::removable_wanted(&std::fs::read_to_string(crate::config::path()).unwrap_or_default());
        for (drive, usb) in ntfs_drives(removable) {
            let pipe = pipe.clone();
            std::thread::spawn(move || {
                // A fixed drive failing restarts the whole stream; a stick (write-protected,
                // ejected) only ends its own thread.
                if stream_drive(drive, usb, &pipe).is_none() && !usb {
                    pipe.kill();
                }
            });
        }
    }
}

/// The drives to stream, from (letter, GetDriveTypeW result): fixed ones, and removable
/// ones only when wanted. Each comes with whether it is removable.
fn pick_drives(types: &[(u8, u32)], removable: bool) -> Vec<(u8, bool)> {
    types.iter().filter(|(_, t)| *t == DRIVE_FIXED || (removable && *t == DRIVE_REMOVABLE)).map(|&(d, t)| (d, t == DRIVE_REMOVABLE)).collect()
}

/// Fixed (and, when asked, removable) drives formatted NTFS, as letters.
fn ntfs_drives(removable: bool) -> Vec<(u8, bool)> {
    let types: Vec<(u8, u32)> = (b'A'..=b'Z').map(|d| (d, unsafe { GetDriveTypeW(&HSTRING::from(format!("{}:\\", d as char))) })).collect();
    pick_drives(&types, removable)
        .into_iter()
        .filter(|(d, _)| unsafe {
            let root = HSTRING::from(format!("{}:\\", *d as char));
            let mut fs = [0u16; 16];
            GetVolumeInformationW(&root, None, None, None, None, Some(&mut fs)).is_ok() && String::from_utf16_lossy(&fs).trim_end_matches('\0') == "NTFS"
        })
        .collect()
}

/// Enumerates one drive's MFT, then follows its USN journal until the client goes away. A
/// removable drive (`snapshot_only`) stops after the snapshot and closes its volume handle.
fn stream_drive(drive: u8, snapshot_only: bool, pipe: &Pipe) -> Option<()> {
    let h = unsafe {
        CreateFileW(
            &HSTRING::from(format!(r"\\.\{}:", drive as char)),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .ok()?;
    let h = Handle(h);
    let journal = query_journal(&h).or_else(|| {
        let create = CREATE_USN_JOURNAL_DATA { MaximumSize: 32 << 20, AllocationDelta: 4 << 20 };
        ioctl(&h, FSCTL_CREATE_USN_JOURNAL, &create, &mut [])?;
        query_journal(&h)
    })?;

    // u64 elements keep USN records 8-byte aligned.
    let mut buf = vec![0u64; 1 << 17];
    let mut out = Vec::with_capacity(2 << 20);
    let mut enum_data = MFT_ENUM_DATA_V0 { StartFileReferenceNumber: 0, LowUsn: 0, HighUsn: journal.NextUsn };
    // Fails with ERROR_HANDLE_EOF after the last record.
    while let Some(bytes) = ioctl(&h, FSCTL_ENUM_USN_DATA, &enum_data, &mut buf) {
        if bytes.len() <= 8 {
            break;
        }
        enum_data.StartFileReferenceNumber = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        each_record(&bytes[8..], |r, name| {
            put(&mut out, UPSERT, drive, record(r.FileReferenceNumber), record(r.ParentFileReferenceNumber), flags(r), name)
        });
        if out.len() > 1 << 20 {
            pipe.write(&out)?;
            out.clear();
        }
    }
    put(&mut out, READY, drive, 0, 0, 0, "");
    pipe.write(&out)?;
    if snapshot_only {
        return Some(());
    }

    // ponytail: the reason mask has no attribute changes, so hiding or unhiding an existing file
    // is seen only at the next full pass (reconnect). Add USN_REASON_BASIC_INFO_CHANGE if that bites.
    let mut read = READ_USN_JOURNAL_DATA_V0 {
        StartUsn: journal.NextUsn,
        ReasonMask: USN_REASON_FILE_CREATE | USN_REASON_FILE_DELETE | USN_REASON_RENAME_OLD_NAME | USN_REASON_RENAME_NEW_NAME,
        ReturnOnlyOnClose: 0,
        Timeout: 0,
        BytesToWaitFor: 1,
        UsnJournalID: journal.UsnJournalID,
    };
    loop {
        // Blocks until there's something new.
        let bytes = ioctl(&h, FSCTL_READ_USN_JOURNAL, &read, &mut buf)?;
        if bytes.len() < 8 {
            continue;
        }
        read.StartUsn = i64::from_le_bytes(bytes[..8].try_into().unwrap());
        out.clear();
        each_record(&bytes[8..], |r, name| {
            let (rec, parent) = (record(r.FileReferenceNumber), record(r.ParentFileReferenceNumber));
            // Reasons accumulate until the file is closed; the latest state wins.
            let gone = r.Reason & USN_REASON_FILE_DELETE != 0
                || (r.Reason & USN_REASON_RENAME_OLD_NAME != 0 && r.Reason & USN_REASON_RENAME_NEW_NAME == 0);
            put(&mut out, if gone { DELETE } else { UPSERT }, drive, rec, parent, flags(r), name)
        });
        if !out.is_empty() {
            pipe.write(&out)?;
        }
    }
}

fn flags(r: &USN_RECORD_V2) -> u8 {
    let bit = |attr: u32, flag: u8| if r.FileAttributes & attr != 0 { flag } else { 0 };
    bit(FILE_ATTRIBUTE_DIRECTORY.0, W_DIR) | bit(FILE_ATTRIBUTE_HIDDEN.0, W_HIDDEN)
}

/// Drive letters of removable drives (USB sticks and the like).
fn removable_drives() -> Vec<u8> {
    (b'A'..=b'Z').filter(|d| unsafe { GetDriveTypeW(&HSTRING::from(format!("{}:\\", *d as char))) } == DRIVE_REMOVABLE).collect()
}

fn query_journal(h: &Handle) -> Option<USN_JOURNAL_DATA_V0> {
    let mut buf = [0u64; size_of::<USN_JOURNAL_DATA_V0>() / 8 + 1];
    let bytes = ioctl(h, FSCTL_QUERY_USN_JOURNAL, &(), &mut buf)?;
    (bytes.len() >= size_of::<USN_JOURNAL_DATA_V0>()).then(|| unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const USN_JOURNAL_DATA_V0) })
}

/// DeviceIoControl with a typed input; returns the bytes written into `out`.
fn ioctl<'a, T>(h: &Handle, code: u32, input: &T, out: &'a mut [u64]) -> Option<&'a [u8]> {
    let mut got = 0u32;
    let (in_ptr, in_len) = if size_of::<T>() == 0 { (None, 0) } else { (Some(input as *const T as _), size_of::<T>() as u32) };
    unsafe {
        DeviceIoControl(h.0, code, in_ptr, in_len, Some(out.as_mut_ptr() as _), (out.len() * 8) as u32, Some(&mut got), None).ok()?;
        Some(std::slice::from_raw_parts(out.as_ptr() as *const u8, got as usize))
    }
}

/// Walks USN_RECORD_V2 entries, handing each one and its name to `f`.
fn each_record(mut bytes: &[u8], mut f: impl FnMut(&USN_RECORD_V2, &str)) {
    while bytes.len() >= size_of::<USN_RECORD_V2>() {
        let r = unsafe { &*(bytes.as_ptr() as *const USN_RECORD_V2) };
        let len = r.RecordLength as usize;
        if len == 0 || len > bytes.len() {
            break;
        }
        if r.MajorVersion == 2 {
            let at = r.FileNameOffset as usize;
            let units = unsafe { std::slice::from_raw_parts(bytes.as_ptr().add(at) as *const u16, r.FileNameLength as usize / 2) };
            f(r, &String::from_utf16_lossy(units));
        }
        bytes = &bytes[len..];
    }
}

/// The current user's SID as a string, e.g. "S-1-5-21-…".
fn user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let token = Handle(token);
        let mut buf = vec![0u64; 64];
        let mut len = 0u32;
        GetTokenInformation(token.0, TokenUser, Some(buf.as_mut_ptr() as _), (buf.len() * 8) as u32, &mut len).ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let sid = s.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(s.0 as _)));
        sid
    }
}

// ---- install ------------------------------------------------------------------------------

/// `--install-indexer` (elevated): copy the helper to Program Files and register a logon
/// task that runs it with highest privileges, then start it.
pub fn install() -> Result<(), String> {
    remove_old_helper();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dest = helper_path();
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    let _ = schtasks(&["/End", "/TN", TASK]);
    // A running helper keeps its file locked; it exits once the task ends it.
    let mut copied = Err(String::new());
    for _ in 0..20 {
        copied = std::fs::copy(&exe, &dest).map(|_| ()).map_err(|e| e.to_string());
        if copied.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    copied?;
    let user = format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default());
    let xml = task_xml(&dest, &user);
    let file = std::env::temp_dir().join("procast-indexer-task.xml");
    // schtasks reads task XML as UTF-16.
    let bytes: Vec<u8> = [0xFF, 0xFE].into_iter().chain(xml.encode_utf16().flat_map(u16::to_le_bytes)).collect();
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let made = schtasks(&["/Create", "/TN", TASK, "/XML", &file.to_string_lossy(), "/F"]);
    let _ = std::fs::remove_file(&file);
    made?;
    schtasks(&["/Run", "/TN", TASK])
}

/// `--uninstall-indexer` (elevated).
pub fn uninstall() -> Result<(), String> {
    remove_old_helper();
    let _ = schtasks(&["/End", "/TN", TASK]);
    let _ = schtasks(&["/Delete", "/TN", TASK, "/F"]);
    let dest = helper_path();
    for _ in 0..20 {
        if !dest.exists() || std::fs::remove_file(&dest).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = std::fs::remove_dir(dest.parent().unwrap());
    Ok(())
}

fn schtasks(args: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("schtasks").args(args).creation_flags(0x0800_0000).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

fn task_xml(exe: &Path, user: &str) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>procast whole-drive file index (reads the NTFS file table)</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>--indexer</Arguments></Exec></Actions>
</Task>"#,
        user = esc(user),
        exe = esc(&exe.to_string_lossy()),
    )
}

// ---- procast's side: the index ----------------------------------------------------------

#[derive(Clone, Copy)]
struct Node {
    name_at: u32,
    name_len: u16,
    flags: u8,
    drive: u8,
    parent: u64,
}

const DIR: u8 = 1;
const DEAD: u8 = 2;
const EXCLUDED: u8 = 4;
const HIDDEN: u8 = 8;
const NONE: u32 = u32::MAX;

/// What whole-drive search leaves out (Settings → Files). Left-out entries aren't kept in
/// memory at all, so skipping big system folders shrinks the index.
#[derive(Clone, Default)]
pub struct Filter {
    /// Folder or file names skipped anywhere (AppData, node_modules, …).
    pub exclude: Vec<String>,
    /// Full paths skipped with everything under them (C:\Windows, …).
    pub skip: Vec<String>,
    /// Drive letters to index; empty = every NTFS drive.
    pub drives: Vec<String>,
    /// Keep files and folders with the hidden attribute.
    pub hidden: bool,
    /// Keep removable drives.
    pub removable: bool,
}

/// Every kept file and folder on the NTFS drives. Names live in one arena; a node points
/// at its parent by MFT record, so records can arrive in any order. Each full pass drops
/// whatever is filtered out and compacts the rest.
// ponytail: deleted entries stay (marked dead) until the next full pass or reconnect.
#[derive(Default)]
pub struct Index {
    nodes: Vec<Node>,
    names: String,
    /// Per drive letter: MFT record -> node.
    slots: Vec<Vec<u32>>,
    exclude: Vec<String>,
    /// Skipped full paths, lowercase, and their last parts (to find candidates cheaply).
    skip: Vec<String>,
    skip_names: Vec<String>,
    /// Drive letters kept; empty = all.
    drives: Vec<u8>,
    /// Drive letters left out: the removable ones, unless wanted.
    removable: Vec<u8>,
    hidden: bool,
    /// The helper's protocol version (its HELLO record); 0 until one arrives.
    helper: u64,
    live: usize,
    /// Exclusion flags need a full pass: before the first one, or after a folder moved.
    dirty: bool,
    /// Drives whose snapshot is complete: their changes are filtered one by one. Until then
    /// records arrive in MFT order (children before parents), so nothing is judged early.
    complete: Vec<u8>,
    /// When the latest drive finished its snapshot (wall clock, for Settings).
    ready_at: Option<std::time::SystemTime>,
}

impl Index {
    pub fn new(f: &Filter) -> Self {
        let skip: Vec<String> = f.skip.iter().map(|p| p.trim().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase()).filter(|p| p.len() > 2).collect();
        Self {
            slots: vec![vec![]; 26],
            exclude: f.exclude.iter().map(|e| e.to_ascii_lowercase()).collect(),
            skip_names: skip.iter().filter_map(|p| p.rsplit('\\').next().map(str::to_string)).collect(),
            skip,
            drives: f.drives.iter().filter_map(|d| d.trim().bytes().next()).map(|b| b.to_ascii_uppercase()).collect(),
            removable: if f.removable { vec![] } else { removable_drives() },
            hidden: f.hidden,
            ..Self::default()
        }
    }

    pub fn len(&self) -> usize {
        self.live
    }

    /// The drives with a complete snapshot, as letters, and when the last one finished.
    pub fn ready_drives(&self) -> (Vec<char>, Option<std::time::SystemTime>) {
        let mut drives: Vec<char> = self.complete.iter().map(|&d| d as char).collect();
        drives.sort_unstable();
        drives.dedup();
        (drives, self.ready_at)
    }

    /// The helper predates this build's protocol: it ignores the hidden and removable settings.
    fn helper_outdated(&self) -> bool {
        self.helper < HELPER_VERSION
    }

    fn slot(&self, drive: u8, rec: u64) -> Option<usize> {
        let s = *self.slots.get((drive.wrapping_sub(b'A')) as usize)?.get(rec as usize)?;
        (s != NONE).then_some(s as usize)
    }

    fn name(&self, n: &Node) -> &str {
        &self.names[n.name_at as usize..n.name_at as usize + n.name_len as usize]
    }

    fn upsert(&mut self, drive: u8, rec: u64, parent: u64, wire: u8, name: &str) {
        let dir = wire & W_DIR != 0;
        let flags = (if dir { DIR } else { 0 }) | (if wire & W_HIDDEN != 0 { HIDDEN } else { 0 });
        let node = Node { name_at: self.names.len() as u32, name_len: name.len() as u16, flags, drive, parent };
        // After the first pass, something filtered out (or under a dropped folder) isn't kept.
        if self.complete.contains(&drive) {
            let orphan = parent != ROOT && self.slot(drive, parent).is_none();
            if orphan || self.left_out(&node, name) {
                if self.slot(drive, rec).is_some() {
                    self.delete(drive, rec);
                    self.dirty |= dir;
                }
                return;
            }
        }
        let Some(slots) = self.slots.get_mut((drive.wrapping_sub(b'A')) as usize) else { return };
        if rec as usize >= slots.len() {
            slots.resize(rec as usize + 1, NONE);
        }
        match slots[rec as usize] {
            NONE => {
                slots[rec as usize] = self.nodes.len() as u32;
                self.nodes.push(node);
                self.live += 1;
            }
            i => {
                let old = self.nodes[i as usize];
                // Unchanged (a repeat record for the same file): nothing to do.
                if old.parent == parent && old.flags == flags && self.name(&old) == name {
                    return;
                }
                // A folder that moved or was renamed changes what's under it.
                self.dirty |= dir;
                self.nodes[i as usize] = node;
            }
        }
        self.names.push_str(name);
        if !self.complete.contains(&drive) {
            self.dirty = true;
        }
    }

    /// Filtered out by name (AppData, NTFS's own `$` files), by the hidden attribute, or by full path (C:\Windows).
    fn left_out(&self, n: &Node, name: &str) -> bool {
        if (n.flags & HIDDEN != 0 && !self.hidden) || crate::files::excluded(&self.exclude, name) || (n.parent == ROOT && name.starts_with('$')) {
            return true;
        }
        let lower = name.to_lowercase();
        self.skip_names.contains(&lower) && self.skip.contains(&format!("{}\\{lower}", self.folder_of(n).to_lowercase()))
    }

    /// The folder a node is in, e.g. "C:\Users\me" ("C:" at the root).
    fn folder_of(&self, n: &Node) -> String {
        match self.slot(n.drive, n.parent) {
            Some(p) if n.parent != ROOT => self.path(p),
            _ => format!("{}:", n.drive as char),
        }
    }

    fn delete(&mut self, drive: u8, rec: u64) {
        if let Some(i) = self.slot(drive, rec) {
            self.nodes[i].flags |= DEAD;
            self.slots[(drive - b'A') as usize][rec as usize] = NONE;
            self.live -= 1;
        }
    }

    /// Applies whole records from `bytes`; returns how many bytes were used and the drives
    /// that finished their snapshot.
    fn apply(&mut self, bytes: &[u8]) -> (usize, Vec<u8>) {
        let (mut at, mut ready) = (0, vec![]);
        while bytes.len() - at >= HEADER {
            let b = &bytes[at..];
            let len = u16::from_le_bytes([b[19], b[20]]) as usize;
            if b.len() < HEADER + len {
                break;
            }
            let (kind, drive) = (b[0], b[1]);
            let rec = u64::from_le_bytes(b[2..10].try_into().unwrap());
            let parent = u64::from_le_bytes(b[10..18].try_into().unwrap());
            let name = std::str::from_utf8(&b[HEADER..HEADER + len]).unwrap_or("");
            let wanted = (self.drives.is_empty() || self.drives.contains(&drive)) && !self.removable.contains(&drive);
            match kind {
                UPSERT | DELETE if !wanted => {}
                UPSERT => self.upsert(drive, rec, parent, b[18], name),
                DELETE => self.delete(drive, rec),
                READY => {
                    self.complete.push(drive);
                    self.ready_at = Some(std::time::SystemTime::now());
                    self.dirty = true;
                    ready.push(drive);
                }
                HELLO => self.helper = rec,
                _ => {}
            }
            at += HEADER + len;
        }
        (at, ready)
    }

    /// Drops everything filtered out (by name or path, and anything under it), plus entries
    /// whose folder is unknown, then compacts what's left.
    fn refresh_excluded(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        // 0 unknown, 1 kept, 2 excluded. Drives still loading are left alone.
        let mut state: Vec<u8> = self.nodes.iter().map(|n| if self.complete.contains(&n.drive) { 0 } else { 1 }).collect();
        let mut chain = vec![];
        for start in 0..self.nodes.len() {
            let mut i = start;
            let verdict = loop {
                if state[i] != 0 {
                    break state[i];
                }
                let n = self.nodes[i];
                if n.flags & DEAD != 0 || self.left_out(&n, self.name(&n)) {
                    state[i] = 2;
                    break 2;
                }
                chain.push(i);
                match self.slot(n.drive, n.parent) {
                    _ if n.parent == ROOT => break 1,
                    Some(p) if !chain.contains(&p) => i = p,
                    Some(_) => break 1,
                    // Its folder isn't in the index: unreachable, so not searchable.
                    None => break 2,
                }
            };
            for c in chain.drain(..) {
                state[c] = verdict;
            }
        }
        self.compact(&state);
    }

    /// Keeps only nodes whose state is 1, with a fresh name arena; slots are remapped.
    fn compact(&mut self, state: &[u8]) {
        let kept = state.iter().filter(|s| **s == 1).count();
        let mut remap = vec![NONE; self.nodes.len()];
        let mut nodes = Vec::with_capacity(kept);
        let mut names = String::with_capacity(self.names.len().min(kept * 24));
        for (i, n) in self.nodes.iter().enumerate() {
            if state[i] == 1 {
                remap[i] = nodes.len() as u32;
                let name = self.name(n);
                nodes.push(Node { name_at: names.len() as u32, ..*n });
                names.push_str(name);
            }
        }
        for slots in &mut self.slots {
            for s in slots.iter_mut().filter(|s| **s != NONE) {
                *s = remap[*s as usize];
            }
            while slots.last() == Some(&NONE) {
                slots.pop();
            }
            slots.shrink_to_fit();
        }
        names.shrink_to_fit();
        (self.nodes, self.names, self.live) = (nodes, names, kept);
    }

    pub fn path(&self, i: usize) -> String {
        let n = self.nodes[i];
        let mut parts = vec![self.name(&n)];
        let mut cur = n;
        while cur.parent != ROOT
            && let Some(p) = self.slot(cur.drive, cur.parent)
            && parts.len() < 256
        {
            cur = self.nodes[p];
            parts.push(self.name(&cur));
        }
        parts.reverse();
        format!("{}:\\{}", n.drive as char, parts.join("\\"))
    }

    /// Fuzzy search over every name, split across cores. A cheap case-insensitive
    /// subsequence check runs first, so the scorer only sees plausible names.
    /// A query with `\` matches its last part against names, then scores whole paths.
    pub fn search(&self, query: &str, max: usize) -> Vec<Item> {
        let q = query.trim();
        if q.is_empty() {
            return vec![];
        }
        let whole_path = q.contains(['\\', '/']);
        let needle = if whole_path { q.rsplit(['\\', '/']).next().unwrap_or(q) } else { q };
        let atoms: Vec<Vec<u8>> = needle.split_whitespace().map(|a| a.to_ascii_lowercase().into_bytes()).collect();
        let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
        let chunk = self.nodes.len().div_ceil(threads).max(1);
        let keep = max * 4;
        let mut hits: Vec<(u32, usize)> = std::thread::scope(|s| {
            let jobs: Vec<_> = self
                .nodes
                .chunks(chunk)
                .enumerate()
                .map(|(c, nodes)| {
                    let (atoms, pattern) = (&atoms, &pattern);
                    s.spawn(move || {
                        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT.match_paths());
                        let mut buf = vec![];
                        let mut found: Vec<(u32, usize)> = vec![];
                        for (k, n) in nodes.iter().enumerate() {
                            if n.flags & (DEAD | EXCLUDED) != 0 {
                                continue;
                            }
                            let name = self.name(n);
                            if !atoms.iter().all(|a| subsequence(a, name.as_bytes())) {
                                continue;
                            }
                            let i = c * chunk + k;
                            let score = if whole_path {
                                let p = self.path(i);
                                pattern.score(Utf32Str::new(&p, &mut buf), &mut matcher)
                            } else {
                                pattern.score(Utf32Str::new(name, &mut buf), &mut matcher)
                            };
                            if let Some(score) = score {
                                found.push((score, i));
                            }
                        }
                        top(&mut found, keep);
                        found
                    })
                })
                .collect();
            jobs.into_iter().flat_map(|j| j.join().unwrap_or_default()).collect()
        });
        top(&mut hits, keep);
        let mut ranked: Vec<(u32, String, bool)> = hits.into_iter().map(|(s, i)| (s, self.path(i), self.nodes[i].flags & DIR != 0)).collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.len().cmp(&b.1.len())));
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        ranked.into_iter().take(max).map(|(_, path, dir)| files::item(&path, dir, &home)).collect()
    }
}

/// Keeps the `n` best-scoring hits (unordered).
fn top(hits: &mut Vec<(u32, usize)>, n: usize) {
    if hits.len() > n {
        hits.select_nth_unstable_by(n, |a, b| b.0.cmp(&a.0));
        hits.truncate(n);
    }
}

/// `needle` (lowercase) appears in order within `hay`, ignoring ASCII case.
fn subsequence(needle: &[u8], hay: &[u8]) -> bool {
    let mut it = needle.iter().peekable();
    for &b in hay {
        match it.peek() {
            Some(&&c) if c == b.to_ascii_lowercase() => {
                it.next();
            }
            None => return true,
            _ => {}
        }
    }
    it.peek().is_none()
}

// ---- procast's side: the connection -----------------------------------------------------

/// Shared with file search: the index, whether it's complete enough to search, and the
/// footer's count / "indexing" flag.
pub struct Client {
    pub index: Arc<RwLock<Index>>,
    pub ready: Arc<AtomicBool>,
    stop: Arc<Stop>,
}

/// Dropping the client ends its connection: the thread sees `stopped` and a read blocked on
/// the pipe is cancelled, so the pipe closes (the helper's next write to it fails and its drive
/// thread ends) and the thread's copy of the index is freed.
impl Drop for Client {
    fn drop(&mut self) {
        self.stop.stop();
    }
}

/// Tells the connect thread its owner is gone, and wakes it if it is blocked reading.
#[derive(Default)]
struct Stop {
    stopped: AtomicBool,
    /// The pipe being read (raw handle), while there is one.
    pipe: Mutex<Option<isize>>,
}

struct Registered<'a>(&'a Stop);

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        *self.0.pipe.lock().unwrap() = None;
    }
}

impl Stop {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Records the pipe the thread is about to read, until the guard drops (every way out of
    /// the read loop, so `stop` never cancels a handle that has been closed).
    /// Declare the guard after the pipe: it must drop first.
    fn register(&self, pipe: &std::fs::File) -> Registered<'_> {
        *self.pipe.lock().unwrap() = Some(pipe.as_raw_handle() as isize);
        Registered(self)
    }

    // ponytail: a stop landing between the thread's `stopped()` check and its ReadFile cancels
    // nothing; that read then returns with the helper's next write (live changes, or at once
    // while a snapshot streams). Make the read overlapped if that window ever matters.
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(h) = *self.pipe.lock().unwrap() {
            unsafe {
                let _ = CancelIoEx(HANDLE(h as *mut _), None);
            }
        }
    }
}

/// Keeps a connection to the helper for as long as procast runs, reconnecting (and
/// restarting the helper's task) as needed.
pub fn connect(filter: Filter, count: Arc<AtomicUsize>, indexing: Arc<AtomicBool>, tx: UnboundedSender<Msg>) -> Client {
    let index = Arc::new(RwLock::new(Index::new(&filter)));
    let ready = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(Stop::default());
    let client = Client { index: index.clone(), ready: ready.clone(), stop: stop.clone() };
    std::thread::spawn(move || {
        let mut fails = 0u32;
        // Once per client, not per connection: a reconnect loop must not reopen the launcher.
        let mut warned = false;
        while !stop.stopped() {
            let Some(mut pipe) = open_pipe() else {
                fails += 1;
                // The helper should start at logon; nudge it if it isn't there.
                if fails % 20 == 2 {
                    let _ = schtasks(&["/Run", "/TN", TASK]);
                }
                std::thread::sleep(Duration::from_secs(3));
                continue;
            };
            fails = 0;
            let started = Instant::now();
            crate::ext_log("file index", "connected to the whole-drive helper");
            ready.store(false, Ordering::SeqCst);
            indexing.store(true, Ordering::SeqCst);
            // A rebuild shows "Indexing…" at once instead of waiting for the first second of data.
            if tx.unbounded_send(Msg::FilesIndexed).is_err() {
                return;
            }
            *index.write().unwrap() = Index::new(&filter);
            let mut buf = vec![0u8; 4 << 20];
            let mut have = 0;
            let mut last_note = Instant::now();
            let _registered = stop.register(&pipe);
            while let Ok(n) = pipe.read(&mut buf[have..]) {
                if n == 0 || stop.stopped() {
                    break;
                }
                have += n;
                let mut ix = index.write().unwrap();
                let (used, done) = ix.apply(&buf[..have]);
                buf.copy_within(used..have, 0);
                have -= used;
                if !done.is_empty() || ready.load(Ordering::SeqCst) {
                    ix.refresh_excluded();
                }
                count.store(ix.len(), Ordering::SeqCst);
                // The first bytes decide: an old helper sends no HELLO and ignores these settings.
                let outdated = !warned && used > 0 && ix.helper_outdated() && (!filter.hidden || filter.removable);
                drop(ix);
                if outdated {
                    warned = true;
                    let note = "The whole-drive helper is out of date, so the hidden-file and removable-drive settings are ignored. Run \"Enable Fast File Search\" again to update it.";
                    let _ = tx.unbounded_send(Msg::Notice(note.into()));
                }
                if !done.is_empty() {
                    let n = index.read().unwrap().len();
                    crate::ext_log("file index", &format!("drive {} ready: {n} entries kept after {:.1?}", done.iter().map(|d| *d as char).collect::<String>(), started.elapsed()));
                    ready.store(true, Ordering::SeqCst);
                    indexing.store(false, Ordering::SeqCst);
                }
                if !done.is_empty() || last_note.elapsed() > Duration::from_secs(1) {
                    last_note = Instant::now();
                    if tx.unbounded_send(Msg::FilesIndexed).is_err() {
                        return;
                    }
                }
            }
            ready.store(false, Ordering::SeqCst);
            if !stop.stopped() {
                crate::ext_log("file index", "helper disconnected; reconnecting");
            }
        }
    });
    client
}

/// Opens the helper's pipe, but only if the process serving it is the installed helper.
fn open_pipe() -> Option<std::fs::File> {
    unsafe {
        let h = CreateFileW(&HSTRING::from(PIPE), GENERIC_READ.0, FILE_SHARE_READ, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None).ok()?;
        let file = std::fs::File::from_raw_handle(h.0);
        let mut pid = 0u32;
        GetNamedPipeServerProcessId(h, &mut pid).ok()?;
        let proc = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut name = [0u16; 1024];
        let mut len = name.len() as u32;
        QueryFullProcessImageNameW(proc.0, PROCESS_NAME_WIN32, PWSTR(name.as_mut_ptr()), &mut len).ok()?;
        let server = String::from_utf16_lossy(&name[..len as usize]);
        server.eq_ignore_ascii_case(&helper_path().to_string_lossy()).then_some(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopping_wakes_a_blocked_read() {
        use windows::Win32::System::Pipes::CreatePipe;
        let (mut r, mut w) = (HANDLE::default(), HANDLE::default());
        unsafe { CreatePipe(&mut r, &mut w, None, 0) }.unwrap();
        let mut pipe = unsafe { std::fs::File::from_raw_handle(r.0) };
        let stop = Arc::new(Stop::default());
        let registered = stop.register(&pipe);
        let (done_tx, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // Nothing is ever written: only a cancel can end this read.
            let _ = done_tx.send(pipe.read(&mut [0u8; 8]).is_err());
        });
        assert!(done.recv_timeout(Duration::from_millis(300)).is_err(), "the read should be blocked");
        stop.stop();
        assert_eq!(done.recv_timeout(Duration::from_secs(5)), Ok(true), "stop cancels the read");
        assert!(stop.stopped());
        unsafe { CloseHandle(w).unwrap() };
        drop(registered);
    }

    #[test]
    fn the_pipe_is_unregistered_on_every_way_out() {
        let stop = Stop::default();
        let file = std::fs::File::open("NUL").unwrap();
        // Leaving the scope any way at all (here a plain return, in `connect` a `return` or `break`) drops the guard.
        let seen = || {
            let _registered = stop.register(&file);
            stop.pipe.lock().unwrap().is_some()
        };
        assert!(seen());
        assert!(stop.pipe.lock().unwrap().is_none(), "the handle is unregistered once the guard is gone");
    }

    /// Times searches over this machine's real index (needs the helper running).
    #[test]
    #[ignore = "cargo test bench_real_index -- --ignored --nocapture"]
    fn bench_real_index() {
        let cfg = crate::config::load().unwrap().files;
        let filter = Filter { exclude: cfg.exclude.clone(), skip: cfg.skip.clone(), drives: cfg.drives.clone(), hidden: cfg.hidden, removable: cfg.removable };
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        let t = Instant::now();
        let client = connect(filter, Arc::default(), Arc::default(), tx);
        while !client.ready.load(Ordering::SeqCst) || client.index.read().unwrap().complete.len() < 3 {
            assert!(t.elapsed() < Duration::from_secs(300), "index not ready");
            std::thread::sleep(Duration::from_millis(200));
        }
        std::thread::sleep(Duration::from_secs(2));
        let ix = client.index.read().unwrap();
        println!("ready after {:.1?}: {} nodes, {} live", t.elapsed(), ix.nodes.len(), ix.len());
        for q in ["pro", "proc", "procast", "readme", "cargo toml", r"pro\cargo"] {
            let mut times = vec![];
            for _ in 0..5 {
                let t = Instant::now();
                std::hint::black_box(ix.search(q, 5));
                times.push(t.elapsed());
            }
            times.sort();
            println!("{q:>12}: median {:.1?}  max {:.1?}", times[2], times[4]);
        }
        drop(ix);
        let mut ix = client.index.write().unwrap();
        ix.dirty = true;
        let t = Instant::now();
        ix.refresh_excluded();
        println!("refresh_excluded: {:.1?}", t.elapsed());
    }

    #[test]
    fn index_builds_paths_applies_changes_and_searches() {
        let mut wire = vec![];
        // Children before parents, as the MFT may order them.
        put(&mut wire, UPSERT, b'C', 40, 30, 0, "Quarterly Report.docx");
        put(&mut wire, UPSERT, b'C', 30, 20, W_DIR, "Docs");
        put(&mut wire, UPSERT, b'C', 20, ROOT, W_DIR, "Users");
        put(&mut wire, UPSERT, b'C', 50, 20, W_DIR, "AppData");
        put(&mut wire, UPSERT, b'C', 51, 50, 0, "report-cache.tmp");
        put(&mut wire, UPSERT, b'C', 55, 20, 0, "report-scratch.TMP");
        put(&mut wire, UPSERT, b'C', 60, ROOT, 0, "$Secure");
        put(&mut wire, UPSERT, b'C', 70, ROOT, W_DIR, "Windows");
        put(&mut wire, UPSERT, b'C', 71, 70, 0, "report-system.log");
        put(&mut wire, UPSERT, b'C', 80, 999, 0, "report-orphan.txt");
        put(&mut wire, UPSERT, b'E', 40, ROOT, 0, "report-on-e.txt");
        // D: is still loading when C: finishes; its child arrives before its parent.
        put(&mut wire, UPSERT, b'D', 91, 90, 0, "report-on-d.txt");
        put(&mut wire, READY, b'C', 0, 0, 0, "");
        let filter = Filter { exclude: vec!["AppData".into(), "*.tmp".into()], skip: vec![r"C:\Windows\".into()], drives: vec!["c".into(), "D".into()], hidden: true, removable: true };
        let mut ix = Index::new(&filter);
        // Split mid-record: the tail is kept for the next read.
        let (used, ready) = ix.apply(&wire[..wire.len() - 3]);
        assert!(ready.is_empty() && used < wire.len());
        let (_, ready) = ix.apply(&wire[used..]);
        assert_eq!(ready, [b'C']);
        ix.refresh_excluded();
        // D: finishes later; its records survived C:'s pass and its parent now arrives.
        let mut d = vec![];
        put(&mut d, UPSERT, b'D', 90, ROOT, W_DIR, "Notes");
        put(&mut d, READY, b'D', 0, 0, 0, "");
        ix.apply(&d);
        ix.refresh_excluded();
        assert_eq!(ix.path(ix.slot(b'D', 91).unwrap()), r"D:\Notes\report-on-d.txt");
        // Kept: Users, Docs, the report, and D:'s two; nothing that was filtered out.
        assert_eq!(ix.len(), 5, r"AppData, $Secure, C:\Windows, the orphan and drive E: are dropped from memory");
        let titles = |ix: &Index, q: &str| {
            let mut t: Vec<String> = ix.search(q, 10).into_iter().map(|i| i.title).collect();
            t.sort();
            t
        };
        assert_eq!(titles(&ix, "report"), ["Quarterly Report.docx", "report-on-d.txt"], "AppData is excluded");
        assert!(titles(&ix, "secure").is_empty(), "NTFS metadata is hidden");
        assert_eq!(ix.path(ix.slot(b'C', 40).unwrap()), r"C:\Users\Docs\Quarterly Report.docx");
        assert_eq!(titles(&ix, r"docs\quarterly"), ["Quarterly Report.docx"]);

        // Rename the folder, delete the file, add one.
        let mut live = vec![];
        put(&mut live, UPSERT, b'C', 30, 20, W_DIR, "Papers");
        put(&mut live, DELETE, b'C', 40, 30, 0, "Quarterly Report.docx");
        put(&mut live, UPSERT, b'C', 41, 30, 0, "report v2.txt");
        ix.apply(&live);
        ix.refresh_excluded();
        assert_eq!(titles(&ix, "report"), ["report v2.txt", "report-on-d.txt"]);
        assert_eq!(ix.path(ix.slot(b'C', 41).unwrap()), r"C:\Users\Papers\report v2.txt");
        assert_eq!(ix.len(), 5, "Users, Papers, report v2.txt, Notes, report-on-d.txt");

        // After the drive is complete, a new file is judged on its own (no full pass needed).
        let mut more = vec![];
        put(&mut more, UPSERT, b'C', 52, 50, 0, "report-new.tmp");
        ix.apply(&more);
        assert!(!ix.dirty, "a new file doesn't trigger a full pass");
        assert_eq!(titles(&ix, "report"), ["report v2.txt", "report-on-d.txt"], "still excluded under AppData");
    }

    #[test]
    fn hidden_files_follow_the_filter() {
        let mut wire = vec![];
        put(&mut wire, UPSERT, b'C', 20, ROOT, W_DIR | W_HIDDEN, "Vault");
        put(&mut wire, UPSERT, b'C', 21, 20, 0, "inner.txt");
        put(&mut wire, UPSERT, b'C', 22, ROOT, W_HIDDEN, "secret.txt");
        put(&mut wire, UPSERT, b'C', 23, ROOT, 0, "plain.txt");
        put(&mut wire, READY, b'C', 0, 0, 0, "");
        let kept = |hidden| {
            let mut ix = Index::new(&Filter { hidden, removable: true, ..Filter::default() });
            ix.apply(&wire);
            ix.refresh_excluded();
            // A hidden file that shows up after the snapshot is judged on its own.
            let mut live = vec![];
            put(&mut live, UPSERT, b'C', 24, ROOT, W_HIDDEN, "later.txt");
            put(&mut live, UPSERT, b'C', 25, 20, 0, "later-inner.txt");
            ix.apply(&live);
            let mut t: Vec<String> = ["txt", "vault"].iter().flat_map(|q| ix.search(q, 10)).map(|i| i.title).collect();
            t.sort();
            t.dedup();
            t
        };
        assert_eq!(kept(true), ["Vault", "inner.txt", "later-inner.txt", "later.txt", "plain.txt", "secret.txt"]);
        assert_eq!(kept(false), ["plain.txt"], "hidden files go, and so does everything inside a hidden folder");
    }

    #[test]
    fn removable_drives_follow_the_filter() {
        let mut wire = vec![];
        put(&mut wire, UPSERT, b'C', 30, ROOT, 0, "on-fixed.txt");
        put(&mut wire, UPSERT, b'E', 30, ROOT, 0, "on-usb.txt");
        put(&mut wire, READY, b'E', 0, 0, 0, "");
        let mut ix = Index::new(&Filter { hidden: true, removable: false, ..Filter::default() });
        ix.removable = vec![b'E'];
        ix.apply(&wire);
        ix.refresh_excluded();
        assert_eq!(ix.len(), 1, "the USB drive's records are dropped");
        let mut ix = Index::new(&Filter { hidden: true, removable: true, ..Filter::default() });
        ix.apply(&wire);
        assert_eq!(ix.len(), 2, "wanted: nothing is left out");
    }

    #[test]
    fn drives_are_picked_by_type_and_the_removable_setting() {
        // A: floppy (2), C: fixed, D: fixed, E: removable, F: CD-ROM (5), G: network (4), H: unknown (0).
        let types = [(b'A', 2), (b'C', 3), (b'D', 3), (b'E', 2), (b'F', 5), (b'G', 4), (b'H', 0)];
        let letters = |removable| pick_drives(&types, removable).into_iter().map(|(d, usb)| format!("{}{}", d as char, if usb { "*" } else { "" })).collect::<Vec<_>>();
        assert_eq!(letters(false), ["C", "D"], "removable drives are never opened by default");
        assert_eq!(letters(true), ["A*", "C", "D", "E*"], "removable ones are flagged so a failure can't take the pipe down");
    }

    #[test]
    fn an_old_helper_is_detected_by_its_missing_hello() {
        let mut old = vec![];
        put(&mut old, UPSERT, b'C', 20, ROOT, 0, "Docs");
        let mut ix = Index::new(&Filter { removable: true, ..Filter::default() });
        ix.apply(&old);
        assert!(ix.helper_outdated(), "no HELLO: an older helper");

        let mut new = vec![];
        put(&mut new, HELLO, 0, HELPER_VERSION, 0, 0, "");
        put(&mut new, UPSERT, b'C', 20, ROOT, 0, "Docs");
        let mut ix = Index::new(&Filter { removable: true, ..Filter::default() });
        ix.apply(&new);
        assert!(!ix.helper_outdated());
        assert_eq!(ix.len(), 1, "HELLO adds no entry");

        let mut older = vec![];
        put(&mut older, HELLO, 0, HELPER_VERSION - 1, 0, 0, "");
        let mut ix = Index::new(&Filter::default());
        ix.apply(&older);
        assert!(ix.helper_outdated(), "an earlier version number counts too");
    }

    #[test]
    fn an_old_helper_flag_still_decodes() {
        // An older helper sent 0 or 1 in the flags byte; 1 is a plain folder.
        let mut wire = vec![];
        put(&mut wire, UPSERT, b'C', 20, ROOT, 0, "Docs");
        wire[18] = 1;
        let mut ix = Index::new(&Filter { hidden: false, removable: true, ..Filter::default() });
        ix.apply(&wire);
        let n = ix.nodes[ix.slot(b'C', 20).unwrap()];
        assert_eq!(n.flags, DIR);
    }

    #[test]
    fn subsequence_ignores_case() {
        assert!(subsequence(b"qrep", b"Quarterly Report"));
        assert!(!subsequence(b"xyz", b"Quarterly Report"));
        assert!(subsequence(b"", b"anything"));
    }
}
