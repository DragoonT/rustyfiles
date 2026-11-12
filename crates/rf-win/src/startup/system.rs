//! The real machine behind the traits.
//!
//! READ-ONLY and safe to call: [`SystemRegistry`] (opens keys with `KEY_QUERY_VALUE` only), [`SystemFolders`] (lists the Startup
//! folders), [`SystemShortcuts`] (reads a `.lnk` on the shell thread), [`SystemProcesses`], [`SystemFiles`], [`SystemEnv`],
//! [`is_safe_mode`].
//!
//! COMPILED AND NEVER RUN (nothing in this increment starts a start-up program or deletes a value): [`ShellLauncher`] starts a
//! program, [`RegistryRunOnce`] deletes one RunOnce value, [`ThreadPacer`] waits, [`FileSessionMarker`] remembers that a sign-in
//! ran. The first two cannot be built without a [`ShellModeToken`] (see `gate.rs`).
//!
//! Every function here may block (disk, network paths in a Run value, the shell thread); none belongs on the UI thread. Two bounds
//! keep a slow machine from holding the caller for long: after the shell thread fails to answer once in time, the remaining
//! shortcuts are not asked (they count as unreadable); and a file probe on a network path runs on a thread of its own with a limit,
//! after which every other network path counts as absent.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, ERROR_ELEVATION_REQUIRED, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_STATISTICS, TokenStatistics};
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLongPathNameW};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_BINARY, REG_DWORD,
    REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegDeleteValueW, RegEnumValueW, RegOpenKeyExW, RegQueryInfoKeyW,
};
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW};
use windows::Win32::System::Threading::{
    CreateProcessW, GetCurrentProcess, GetCurrentProcessId, GetExitCodeProcess, OpenProcessToken, PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonStartup, FOLDERID_Startup, KF_FLAG_DEFAULT, SEE_MASK_DOENVSUBST, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS,
    SHELLEXECUTEINFOW, SHGetKnownFolderPath, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CLEANBOOT, SW_SHOWNORMAL};
use windows::core::{GUID, PCWSTR, PWSTR};

use super::collect::{FolderFile, Machine, ProcessSnapshot, RegistryRead, RunningImages, RunningSet, ShortcutData, Shortcuts, StartupFolders, collect};
use super::command::{Environment, Files, expand_with};
use super::gate::ShellModeToken;
use super::model::{Hive, Presence, RegData, RegValue, Source, SourceKind, StartupEntry, View};
use super::plan::PlanOptions;
use super::run::{LaunchError, LaunchOutcome, LaunchRequest, Launcher, Pacer, RunOnceStore, SessionMarker};
use crate::wide::{from_wide, wide};

/// The longest the shell thread is waited for when reading one shortcut.
const SHELL_LIMIT: Duration = Duration::from_secs(5);
/// The longest the shell thread is waited for to start one program.
const LAUNCH_LIMIT: Duration = Duration::from_secs(15);
/// The longest a file probe on a network path is waited for.
const NETWORK_PROBE_LIMIT: Duration = Duration::from_secs(3);
/// How often a wait for a launched command looks for a stop request.
const STOP_POLL_MS: u32 = 250;
/// A registry value longer than this is read as a value of an unknown kind (nothing that starts at sign-in is this big).
const MAX_VALUE_BYTES: u32 = 4 * 1024 * 1024;

/// Whether the session is a Safe Mode session (`SM_CLEANBOOT` is not zero).
pub fn is_safe_mode() -> bool {
    // SAFETY: a plain query.
    unsafe { GetSystemMetrics(SM_CLEANBOOT) != 0 }
}

/// The plan options for this session: the defaults, with Safe Mode read from the system.
pub fn session_options() -> PlanOptions {
    PlanOptions { safe_mode: is_safe_mode(), ..PlanOptions::default() }
}

/// Every entry Windows would start at sign-in for this user, read from the real machine. Read-only. May block.
pub fn list() -> Vec<StartupEntry> {
    collect(&Machine {
        registry: &SystemRegistry,
        folders: &SystemFolders,
        shortcuts: &SystemShortcuts::default(),
        processes: &SystemProcesses,
        files: &SystemFiles::default(),
        env: &SystemEnv,
    })
}

// ------------------------------------------------------------------------------------------------ registry (read)

fn root_of(hive: Hive) -> HKEY {
    match hive {
        Hive::CurrentUser => HKEY_CURRENT_USER,
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
    }
}

fn view_flag(view: View) -> windows::Win32::System::Registry::REG_SAM_FLAGS {
    match view {
        View::Native => KEY_WOW64_64KEY,
        View::Wow32 => KEY_WOW64_32KEY,
    }
}

fn decode_value(ty: u32, bytes: &[u8]) -> RegData {
    let text = |b: &[u8]| {
        let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        from_wide(&units)
    };
    if ty == REG_SZ.0 {
        RegData::Sz(text(bytes))
    } else if ty == REG_EXPAND_SZ.0 {
        RegData::ExpandSz(text(bytes))
    } else if ty == REG_BINARY.0 {
        RegData::Binary(bytes.to_vec())
    } else if ty == REG_DWORD.0 && bytes.len() == 4 {
        RegData::Dword(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    } else {
        RegData::Other(ty)
    }
}

/// The real registry, through `KEY_QUERY_VALUE` only: no write right is ever asked for.
pub struct SystemRegistry;

impl RegistryRead for SystemRegistry {
    fn values(&self, hive: Hive, view: View, key: &str) -> Option<Vec<RegValue>> {
        let sub = wide(key);
        let mut h = HKEY::default();
        // SAFETY: `sub` is NUL-terminated and outlives the call; `h` is a valid out-pointer; the key is closed below.
        unsafe {
            if RegOpenKeyExW(root_of(hive), PCWSTR(sub.as_ptr()), None, KEY_QUERY_VALUE | view_flag(view), &mut h) != ERROR_SUCCESS {
                return None;
            }
            let out = enum_values(h);
            let _ = RegCloseKey(h);
            Some(out)
        }
    }
}

/// SAFETY: `h` is an open key with the query-value right.
unsafe fn enum_values(h: HKEY) -> Vec<RegValue> {
    let (mut count, mut max_name, mut max_data) = (0u32, 0u32, 0u32);
    // SAFETY: the out-pointers are valid for the call.
    let info = unsafe { RegQueryInfoKeyW(h, None, None, None, None, None, None, Some(&mut count), Some(&mut max_name), Some(&mut max_data), None, None) };
    if info != ERROR_SUCCESS {
        return Vec::new();
    }
    let max_data = max_data.min(MAX_VALUE_BYTES);
    let mut name_buf = vec![0u16; max_name as usize + 2];
    let mut data_buf = vec![0u8; max_data as usize + 2];
    let mut out = Vec::new();
    for i in 0..count {
        let mut name_len = name_buf.len() as u32;
        let mut data_len = data_buf.len() as u32;
        let mut ty = 0u32;
        // SAFETY: the buffers are as long as the lengths say.
        let r = unsafe { RegEnumValueW(h, i, Some(PWSTR(name_buf.as_mut_ptr())), &mut name_len, None, Some(&mut ty), Some(data_buf.as_mut_ptr()), Some(&mut data_len)) };
        if r == ERROR_NO_MORE_ITEMS {
            break;
        }
        if r == ERROR_MORE_DATA {
            // grew since the size was asked for: not a start-up command, and not worth a second try
            if let Some(name) = name_buf.get(..name_len as usize) {
                out.push(RegValue { name: String::from_utf16_lossy(name), data: RegData::Other(ty) });
            }
            continue;
        }
        if r != ERROR_SUCCESS {
            continue;
        }
        let name = String::from_utf16_lossy(&name_buf[..(name_len as usize).min(name_buf.len())]);
        if name.is_empty() {
            // the key's (Default) value: not a named entry
            continue;
        }
        out.push(RegValue { name, data: decode_value(ty, &data_buf[..(data_len as usize).min(data_buf.len())]) });
    }
    out
}

// ------------------------------------------------------------------------------------------------ folders, shortcuts

fn known_folder(id: GUID) -> Option<PathBuf> {
    crate::sta::call(SHELL_LIMIT, move || unsafe {
        let p = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    })
    .flatten()
}

/// The two Startup folders, listed (never changed).
pub struct SystemFolders;

impl StartupFolders for SystemFolders {
    fn files(&self, source: Source) -> Option<Vec<FolderFile>> {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        let id = match source {
            Source::StartupFolderUser => FOLDERID_Startup,
            Source::StartupFolderCommon => FOLDERID_CommonStartup,
            _ => return None,
        };
        let dir = known_folder(id)?;
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                continue;
            }
            out.push(FolderFile {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path().to_string_lossy().into_owned(),
                hidden: meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0,
            });
        }
        Some(out)
    }
}

/// Stops asking once one answer did not come in time. A shell thread that is busy (the dock extracting icons at sign-in) would
/// otherwise make N shortcuts wait N times the limit.
#[derive(Default)]
pub(crate) struct GiveUp(Cell<bool>);

impl GiveUp {
    /// Runs `ask` unless an earlier ask gave no answer. `ask` returns `None` for "no answer in time".
    pub(crate) fn through<T>(&self, ask: impl FnOnce() -> Option<T>) -> Option<T> {
        if self.0.get() {
            return None;
        }
        let answer = ask();
        if answer.is_none() {
            self.0.set(true);
        }
        answer
    }
}

/// Shortcuts, read on the shell thread (`sta::call`: a worker that calls the shell itself hangs it).
#[derive(Default)]
pub struct SystemShortcuts {
    gave_up: GiveUp,
}

impl Shortcuts for SystemShortcuts {
    fn read(&self, path: &str) -> Option<ShortcutData> {
        let p = PathBuf::from(path);
        let d = self.gave_up.through(|| crate::sta::call(SHELL_LIMIT, move || crate::shortcut::details(&p).ok()))??;
        Some(ShortcutData { target: d.target, args: d.args, working_dir: d.working_dir, show: d.show, run_as_admin: d.run_as_admin })
    }
}

// ------------------------------------------------------------------------------------------------ processes, disk, environment

// `ProcessIdToSessionId` is in a windows-crate feature this workspace does not switch on; it is one kernel32 function.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn ProcessIdToSessionId(process_id: u32, session_id: *mut u32) -> i32;
}

fn session_of(pid: u32) -> Option<u32> {
    let mut s = 0u32;
    // SAFETY: `s` is a valid out-pointer.
    (unsafe { ProcessIdToSessionId(pid, &mut s) } != 0).then_some(s)
}

/// The image paths of the processes of THIS session that can be asked. A process the user's token may not open (an elevated one, a
/// protected one) has no readable path; its file name is kept in the snapshot's second list instead.
pub struct SystemProcesses;

impl RunningImages for SystemProcesses {
    fn images(&self) -> Vec<String> {
        self.snapshot().images
    }

    fn snapshot(&self) -> ProcessSnapshot {
        let mut out = ProcessSnapshot::default();
        // SAFETY: the snapshot handle is used and closed here; `e.dwSize` is set as the API asks.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
            let ours = session_of(GetCurrentProcessId());
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut more = Process32FirstW(snap, &mut e).is_ok();
            while more {
                let pid = e.th32ProcessID;
                if pid != 0 && ours.is_some() && session_of(pid) == ours {
                    match crate::tasklist::exe_of(pid) {
                        Some(p) => out.images.push(p.to_string_lossy().into_owned()),
                        None => out.unreadable_names.push(from_wide(&e.szExeFile)),
                    }
                }
                more = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        out
    }
}

/// A path on another machine: a UNC path (not the `\\?\` form of a local one), or a drive letter that maps to one.
pub(crate) fn is_network_path(path: &str, drive_is_remote: impl Fn(char) -> bool) -> bool {
    let p = path.trim_start();
    if let Some(rest) = p.strip_prefix(r"\\?\") {
        // `\\?\UNC\server\share` is a network path; `\\?\C:\x` is a local one that is only spelled long
        return rest.get(..4).is_some_and(|u| u.eq_ignore_ascii_case(r"UNC\"));
    }
    if p.starts_with(r"\\") || p.starts_with("//") {
        return true;
    }
    let b = p.as_bytes();
    b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() && drive_is_remote(b[0] as char)
}

/// `GetDriveTypeW`'s answer for a network drive (`DRIVE_REMOTE`).
const DRIVE_REMOTE: u32 = 4;

fn drive_is_remote(letter: char) -> bool {
    let root = wide(format!("{letter}:\\"));
    // SAFETY: `root` is NUL-terminated. This asks the system for the drive's type; it does not touch the network.
    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_REMOTE }
}

/// The disk. A probe of a path on another machine runs on a thread of its own with a limit: an offline server must not hold the
/// list for the length of the network's own time-out, once per entry. After the first probe that runs out of time, every other
/// network path counts as absent without being asked.
#[derive(Default)]
pub struct SystemFiles {
    network_down: GiveUp,
}

impl SystemFiles {
    fn probe(&self, path: &str, want: impl Fn(&std::fs::Metadata) -> bool + Send + 'static) -> bool {
        if path.contains('\0') {
            return false;
        }
        if !is_network_path(path, drive_is_remote) {
            return std::fs::metadata(path).is_ok_and(|m| want(&m));
        }
        let p = path.to_string();
        let answer = self.network_down.through(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(std::fs::metadata(&p).is_ok_and(|m| want(&m)));
            });
            rx.recv_timeout(NETWORK_PROBE_LIMIT).ok()
        });
        answer.unwrap_or(false)
    }
}

impl Files for SystemFiles {
    fn is_file(&self, path: &str) -> bool {
        self.probe(path, |m| m.is_file())
    }
    fn exists(&self, path: &str) -> bool {
        self.probe(path, |_| true)
    }
    fn long_path(&self, path: &str) -> String {
        if is_network_path(path, drive_is_remote) {
            // GetLongPathName would ask the server again
            return path.to_string();
        }
        let w = wide(path);
        let mut buf = vec![0u16; 1024];
        // SAFETY: `w` is NUL-terminated; the buffer is as long as it is said to be.
        let n = unsafe { GetLongPathNameW(PCWSTR(w.as_ptr()), Some(&mut buf)) } as usize;
        if n == 0 || n > buf.len() {
            return path.to_string();
        }
        String::from_utf16_lossy(&buf[..n])
    }
}

/// This process's environment, which at sign-in is the user's.
pub struct SystemEnv;

impl Environment for SystemEnv {
    fn expand(&self, s: &str) -> String {
        expand_with(s, &|name| std::env::var_os(name).map(|v| v.to_string_lossy().into_owned()))
    }

    /// The system folder, the Windows folder, then `PATH`. The application's folder and the current folder, which come first in a
    /// real `CreateProcess` search, mean nothing for a line read at sign-in.
    fn search_dirs(&self) -> Vec<String> {
        let mut dirs = Vec::new();
        let mut buf = vec![0u16; 1024];
        // SAFETY: the buffer is as long as it is said to be.
        unsafe {
            let n = GetSystemDirectoryW(Some(&mut buf)) as usize;
            if n > 0 && n <= buf.len() {
                dirs.push(String::from_utf16_lossy(&buf[..n]));
            }
            let n = GetWindowsDirectoryW(Some(&mut buf)) as usize;
            if n > 0 && n <= buf.len() {
                dirs.push(String::from_utf16_lossy(&buf[..n]));
            }
        }
        if let Some(path) = std::env::var_os("PATH") {
            for d in std::env::split_paths(&path) {
                let d = d.to_string_lossy().trim().trim_matches('"').to_string();
                if !d.is_empty() {
                    dirs.push(self.expand(&d));
                }
            }
        }
        dirs
    }
}

// ------------------------------------------------------------------------------------------------ the parts that act (never run here)

/// The Windows folder (`C:\Windows`), or "" when the system does not say.
pub(crate) fn windows_dir() -> String {
    let mut buf = vec![0u16; 512];
    // SAFETY: the buffer is as long as it is said to be.
    let n = unsafe { GetWindowsDirectoryW(Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() { String::new() } else { String::from_utf16_lossy(&buf[..n]) }
}

/// Starts programs.
///
/// * A program that is an `.exe` (a registry command, a plain file in a Startup folder, a shortcut's target) is started with
///   `CreateProcess` on the program's exact path, with the shortcut's arguments, working folder and show command. `CreateProcess` does
///   NOT raise a UAC prompt: a program whose manifest asks for administrator rights fails with `ERROR_ELEVATION_REQUIRED` and is
///   reported as not started. `ShellExecuteEx` shows the consent dialog for such a program (`SEE_MASK_FLAG_NO_UI` only hides error
///   boxes), so it is never used for an `.exe`. (A shortcut that itself asks to run as administrator is skipped by the plan before it
///   gets here.)
/// * Anything else (a `.bat`, a document run through its association, a shortcut to a non-file target) goes through `ShellExecuteEx`
///   on the shell thread, as the file itself. That path cannot be shown to be free of a prompt: an association whose handler elevates
///   would show one. It is what Explorer does for such files.
///
/// Built only with a [`ShellModeToken`]. COMPILED, NEVER RUN.
pub struct ShellLauncher(());

impl ShellLauncher {
    pub fn new(_token: &ShellModeToken) -> ShellLauncher {
        ShellLauncher(())
    }
}

fn shell_execute(file: &str, params: &str, dir: &str, show: i32, keep_process: bool) -> Result<Option<isize>, LaunchError> {
    let (file, params, dir) = (wide(file), wide(params), wide(dir));
    let mut mask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI | SEE_MASK_DOENVSUBST;
    if keep_process {
        mask |= SEE_MASK_NOCLOSEPROCESS;
    }
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: mask,
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: if params.len() > 1 { PCWSTR(params.as_ptr()) } else { PCWSTR::null() },
        lpDirectory: if dir.len() > 1 { PCWSTR(dir.as_ptr()) } else { PCWSTR::null() },
        nShow: show,
        ..Default::default()
    };
    crate::shell::ensure_com();
    // SAFETY: the strings outlive the call; `info` is initialised as the API asks.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| LaunchError::Failed(e.to_string()))?;
    Ok((keep_process && !info.hProcess.is_invalid()).then_some(info.hProcess.0 as isize))
}

/// `CreateProcess` on `program` (an absolute path), the command line `"program" args`. The process handle comes back when `keep_process`.
fn create_process(program: &str, args: &str, dir: &str, show: i32, keep_process: bool) -> Result<Option<isize>, LaunchError> {
    let app = wide(program);
    // CreateProcessW may write into the command line buffer, so it is a buffer of our own
    let mut cmd = wide(if args.is_empty() { format!("\"{program}\"") } else { format!("\"{program}\" {args}") });
    let dir_w = wide(dir);
    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: u16::try_from(show).unwrap_or(1),
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    // SAFETY: every string outlives the call and is NUL-terminated; `cmd` is writable; `si` and `pi` are valid.
    let created = unsafe {
        CreateProcessW(
            PCWSTR(app.as_ptr()),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            Default::default(),
            None,
            if dir.is_empty() { PCWSTR::null() } else { PCWSTR(dir_w.as_ptr()) },
            &si,
            &mut pi,
        )
    };
    if let Err(e) = created {
        return Err(LaunchError::Failed(if e.code() == windows::core::HRESULT::from_win32(ERROR_ELEVATION_REQUIRED.0) {
            "the program asks for administrator rights; not started".to_string()
        } else {
            e.to_string()
        }));
    }
    // SAFETY: handles CreateProcess just gave us.
    unsafe {
        let _ = CloseHandle(pi.hThread);
        if keep_process {
            Ok(Some(pi.hProcess.0 as isize))
        } else {
            let _ = CloseHandle(pi.hProcess);
            Ok(None)
        }
    }
}

fn is_exe(program: &str) -> bool {
    program.rsplit(['\\', '/']).next().is_some_and(|n| n.to_lowercase().ends_with(".exe"))
}

/// What one slice of waiting for a process found.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Slice {
    Ended(u32),
    NotYet,
    Broken,
}

/// Waits for a process to end, in slices, looking for a stop request before each. `wait_slice(ms)` waits that long at most. The time
/// that passed is counted from the slices, not read from a clock, so that the loop is the same on a fake.
pub(crate) fn wait_in_slices(total_ms: u32, stop: &dyn Fn() -> bool, mut wait_slice: impl FnMut(u32) -> Slice) -> Result<LaunchOutcome, LaunchError> {
    let mut waited = 0u32;
    loop {
        if stop() {
            return Err(LaunchError::Interrupted);
        }
        if waited >= total_ms {
            return Err(LaunchError::TimedOut);
        }
        let slice = STOP_POLL_MS.min(total_ms - waited);
        match wait_slice(slice) {
            Slice::Ended(code) => return Ok(LaunchOutcome::Exited(code)),
            Slice::NotYet => waited += slice,
            Slice::Broken => return Err(LaunchError::Failed("waiting for the process failed".into())),
        }
    }
}

fn wait_for_handle(h: HANDLE, total_ms: u32, stop: &dyn Fn() -> bool) -> Result<LaunchOutcome, LaunchError> {
    // SAFETY: `h` is a process handle the caller owns for the length of the wait.
    wait_in_slices(total_ms, stop, |ms| unsafe {
        let r = WaitForSingleObject(h, ms);
        if r == WAIT_OBJECT_0 {
            let mut code = 0u32;
            match GetExitCodeProcess(h, &mut code) {
                Ok(()) => Slice::Ended(code),
                // it ended, but its code is unknown: the command ran, which is what the caller needs to know
                Err(_) => Slice::Ended(0),
            }
        } else if r == WAIT_TIMEOUT {
            Slice::NotYet
        } else {
            Slice::Broken
        }
    })
}

impl Launcher for ShellLauncher {
    fn already_running(&mut self, entry: &StartupEntry) -> bool {
        // a fresh look: the list was read some time ago
        let now = RunningSet::new(&SystemProcesses.snapshot());
        matches!(now.state_of(&entry.program, entry.presence == Presence::Found), super::model::Running::Yes | super::model::Running::Maybe)
    }

    fn launch(&mut self, req: &LaunchRequest) -> Result<LaunchOutcome, LaunchError> {
        let e = req.entry;
        let wait = req.wait_exit_ms;
        let show = e.show.unwrap_or(SW_SHOWNORMAL.0);
        let handle = if !e.program.is_empty() && is_exe(&e.program) {
            create_process(&e.program, &e.args, &e.working_dir, show, wait.is_some())?
        } else {
            let (file, params, dir) = match &e.file_path {
                Some(p) => (p.clone(), String::new(), String::new()),
                None if e.program.is_empty() => return Err(LaunchError::Failed("the command names no program".into())),
                None => (e.program.clone(), e.args.clone(), e.working_dir.clone()),
            };
            // When the shell thread does not answer in time, the job stays queued there and may still start the program later: the
            // answer is "unknown", and it is said so.
            crate::sta::call(LAUNCH_LIMIT, move || shell_execute(&file, &params, &dir, show, wait.is_some()))
                .ok_or_else(|| LaunchError::Failed("the shell thread did not answer in time; the program may still start".into()))??
        };
        let (Some(h), Some(ms)) = (handle, wait) else {
            if let Some(h) = handle {
                // SAFETY: a process handle this call opened.
                unsafe {
                    let _ = CloseHandle(HANDLE(h as *mut _));
                }
            }
            return Ok(LaunchOutcome::Started);
        };
        let h = HANDLE(h as *mut _);
        let r = wait_for_handle(h, ms, req.stop);
        // SAFETY: the process handle this call opened.
        unsafe {
            let _ = CloseHandle(h);
        }
        r
    }
}

/// Deletes one RunOnce value. The only registry write of this module, refused for anything that is not a RunOnce source.
///
/// Built only with a [`ShellModeToken`]. COMPILED, NEVER RUN.
pub struct RegistryRunOnce(());

impl RegistryRunOnce {
    pub fn new(_token: &ShellModeToken) -> RegistryRunOnce {
        RegistryRunOnce(())
    }
}

impl RunOnceStore for RegistryRunOnce {
    fn delete(&mut self, source: Source, name: &str) -> Result<(), String> {
        if source.kind() != SourceKind::RunOnce {
            return Err("not a RunOnce source".into());
        }
        if name.is_empty() || name.contains('\0') {
            return Err("no valid value name".into());
        }
        let Some((hive, view, key)) = source.registry_key() else { return Err("no registry key".into()) };
        let (sub, value) = (wide(key), wide(name));
        let mut h = HKEY::default();
        // SAFETY: the strings are NUL-terminated and outlive the calls; the key is closed below.
        unsafe {
            let opened = RegOpenKeyExW(root_of(hive), PCWSTR(sub.as_ptr()), None, KEY_SET_VALUE | view_flag(view), &mut h);
            if opened != ERROR_SUCCESS {
                return Err(format!("opening the RunOnce key failed ({})", opened.0));
            }
            let deleted = RegDeleteValueW(h, PCWSTR(value.as_ptr()));
            let _ = RegCloseKey(h);
            if deleted == ERROR_SUCCESS { Ok(()) } else { Err(format!("deleting the value failed ({})", deleted.0)) }
        }
    }
}

/// An identifier of this sign-in: the logon session's LUID from the process token. A new sign-in is a new logon session, so the
/// identifier is the same for a host the guard restarts and different at the next sign-in. Read-only.
pub fn logon_session_key() -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: the out-pointers are valid; the token is closed below.
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut stats = TOKEN_STATISTICS::default();
        let mut returned = 0u32;
        let got = GetTokenInformation(token, TokenStatistics, Some(&mut stats as *mut _ as *mut _), std::mem::size_of::<TOKEN_STATISTICS>() as u32, &mut returned);
        let _ = CloseHandle(token);
        got.ok()?;
        Some(format!("{:x}-{:x}", stats.AuthenticationId.HighPart, stats.AuthenticationId.LowPart))
    }
}

/// Remembers, in a small file, which sign-in ran the start-up list: the file holds the sign-in's key. The file lives with the host's
/// other data; the guard restarting the host does not erase it, and a new sign-in has another key.
///
/// COMPILED, NEVER RUN against a real data folder (the tests use a temp folder and made-up keys).
pub struct FileSessionMarker {
    path: PathBuf,
    key: String,
}

impl FileSessionMarker {
    pub const FILE_NAME: &'static str = "shell-startup-session.txt";

    pub fn new(dir: &Path, key: &str) -> FileSessionMarker {
        FileSessionMarker { path: dir.join(Self::FILE_NAME), key: key.trim().to_string() }
    }

    /// For this sign-in; `None` when the logon session cannot be read (the caller then decides what to do without a memory).
    pub fn for_this_sign_in(dir: &Path) -> Option<FileSessionMarker> {
        logon_session_key().map(|k| FileSessionMarker::new(dir, &k))
    }
}

impl SessionMarker for FileSessionMarker {
    fn already_ran(&self) -> bool {
        !self.key.is_empty() && std::fs::read_to_string(&self.path).is_ok_and(|t| t.trim() == self.key)
    }

    fn mark_ran(&mut self) {
        if self.key.is_empty() {
            return;
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // written beside and renamed over, so that a crash in the middle leaves the old key, not half of a new one
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, &self.key).is_ok() && std::fs::rename(&tmp, &self.path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Waits with the thread: a plain sleep, or, when a stop channel is given, a wait that ends as soon as the host says stop.
/// `ready` carries the host's "the shell is ready" (one message); with none, the shell counts as ready at once.
///
/// COMPILED, NEVER RUN.
#[derive(Default)]
pub struct ThreadPacer {
    ready: Option<Receiver<()>>,
    ready_seen: bool,
    stop: Option<Receiver<()>>,
    stopped: bool,
}

impl ThreadPacer {
    pub fn new(ready: Option<Receiver<()>>, stop: Option<Receiver<()>>) -> ThreadPacer {
        ThreadPacer { ready, ready_seen: false, stop, stopped: false }
    }
}

impl Pacer for ThreadPacer {
    fn wait(&mut self, ms: u32) {
        let d = Duration::from_millis(u64::from(ms));
        match &self.stop {
            None => std::thread::sleep(d),
            Some(rx) => {
                if !matches!(rx.recv_timeout(d), Err(RecvTimeoutError::Timeout)) {
                    self.stopped = true;
                }
            }
        }
    }

    fn wait_shell_ready(&mut self, limit_ms: u32) -> bool {
        if self.ready_seen {
            return true;
        }
        let Some(rx) = &self.ready else { return true };
        self.ready_seen = rx.recv_timeout(Duration::from_millis(u64::from(limit_ms))).is_ok();
        self.ready_seen
    }

    fn stop_requested(&mut self) -> bool {
        if !self.stopped
            && let Some(rx) = &self.stop
        {
            self.stopped = !matches!(rx.try_recv(), Err(TryRecvError::Empty));
        }
        self.stopped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only: lists the machine's own sources. Whatever is there, the list must be consistent.
    #[test]
    fn the_real_list_is_consistent() {
        let list = list();
        for (i, e) in list.iter().enumerate() {
            assert!(e.ordinal >= 1, "entry {i}");
            assert!(!e.name.is_empty());
            // an entry's approval is "not applicable" exactly for the sources without an approval key
            assert_eq!(e.approval == super::super::model::Approval::NotApplicable, e.source.approval_key().is_none(), "{}", e.source.label());
        }
        // ordinals count up from 1 within a source
        for s in Source::ALL {
            let o: Vec<usize> = list.iter().filter(|e| e.source == s).map(|e| e.ordinal).collect();
            assert_eq!(o, (1..=o.len()).collect::<Vec<_>>(), "{}", s.label());
        }
        // sources come in order
        assert!(list.windows(2).all(|p| p[0].source <= p[1].source));
    }

    #[test]
    fn a_key_that_is_not_there_is_none_and_a_key_that_is_there_lists_values() {
        let r = SystemRegistry;
        assert!(r.values(Hive::CurrentUser, View::Native, r"Software\rustyfiles-no-such-key-9f3a").is_none());
        // every Windows has this key; its values (if any) have names
        let v = r.values(Hive::LocalMachine, View::Native, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion").unwrap();
        assert!(v.iter().any(|x| x.name == "ProductName" && matches!(x.data, RegData::Sz(_))));
    }

    #[test]
    fn the_real_environment_expands_and_has_search_folders() {
        let e = SystemEnv;
        let windir = std::env::var("SystemRoot").unwrap();
        assert_eq!(e.expand("%SystemRoot%\\x"), format!("{windir}\\x"));
        assert_eq!(e.expand("%rustyfiles_no_such_var_7%"), "%rustyfiles_no_such_var_7%");
        let dirs = e.search_dirs();
        assert!(dirs.iter().any(|d| d.to_lowercase().ends_with("system32")));
    }

    #[test]
    fn the_real_disk_tells_a_file_from_a_folder() {
        let f = SystemFiles::default();
        let windir = std::env::var("SystemRoot").unwrap();
        assert!(!f.is_file(&windir) && f.exists(&windir));
        assert!(f.is_file(&format!("{windir}\\System32\\cmd.exe")));
        assert!(!f.exists(&format!("{windir}\\no-such-file-9f3a.exe")));
        assert!(!f.is_file("C:\\a\0b"));
        // the long form of a long name is itself
        let cmd = format!("{windir}\\System32\\cmd.exe");
        assert_eq!(f.long_path(&cmd).to_lowercase(), cmd.to_lowercase());
    }

    #[test]
    fn this_process_is_among_the_running_images_of_its_session() {
        let me = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        let images = SystemProcesses.images();
        assert!(images.iter().any(|p| super::super::command::path_key(p) == super::super::command::path_key(&me)), "{} images", images.len());
    }

    #[test]
    fn the_pacer_waits_for_its_events_and_for_nothing_else() {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let mut p = ThreadPacer::new(Some(ready_rx), Some(stop_rx));
        assert!(!p.wait_shell_ready(20), "nothing was sent: the limit runs out");
        ready_tx.send(()).unwrap();
        assert!(p.wait_shell_ready(1_000));
        assert!(p.wait_shell_ready(0), "once seen, always ready");
        assert!(!p.stop_requested());
        p.wait(10);
        assert!(!p.stop_requested());
        stop_tx.send(()).unwrap();
        let t = std::time::Instant::now();
        p.wait(5_000);
        assert!(t.elapsed() < Duration::from_secs(2), "a stop ends a wait at once");
        assert!(p.stop_requested());
        // without channels: ready at once, never stopped
        let mut q = ThreadPacer::default();
        assert!(q.wait_shell_ready(0));
        q.wait(1);
        assert!(!q.stop_requested());
    }

    #[test]
    fn the_run_once_store_refuses_everything_that_is_not_a_runonce_source_without_touching_the_registry() {
        let mut s = RegistryRunOnce::new(&ShellModeToken::for_tests());
        for src in [Source::RunUser, Source::RunMachine, Source::PolicyRunUser, Source::StartupFolderUser] {
            assert!(s.delete(src, "x").is_err(), "{}", src.label());
        }
        // an empty or NUL-holding name is refused before any key is opened
        assert!(s.delete(Source::RunOnceUser, "").is_err());
        assert!(s.delete(Source::RunOnceUser, "a\0b").is_err());
    }

    #[test]
    fn after_one_missing_answer_the_rest_are_not_asked() {
        let gate = GiveUp::default();
        let asked = Cell::new(0);
        assert_eq!(gate.through(|| { asked.set(asked.get() + 1); Some(1) }), Some(1));
        assert_eq!(gate.through(|| { asked.set(asked.get() + 1); None::<i32> }), None, "this one gets no answer");
        assert_eq!(gate.through(|| { asked.set(asked.get() + 1); Some(3) }), None, "and the next is not even asked");
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn network_paths_are_told_from_local_ones() {
        let none = |_: char| false;
        let z = |c: char| c == 'Z';
        assert!(is_network_path(r"\\server\share\x.exe", none));
        assert!(is_network_path("//server/share/x.exe", none));
        assert!(is_network_path(r"\\?\UNC\server\share\x.exe", none));
        assert!(!is_network_path(r"\\?\C:\x.exe", none), "long spelling of a local path");
        assert!(!is_network_path(r"C:\x.exe", none));
        assert!(is_network_path(r"Z:\x.exe", z));
        assert!(!is_network_path(r"Y:\x.exe", z));
        assert!(!is_network_path("x.exe", z) && !is_network_path("", z) && !is_network_path("Z", z));
    }

    #[test]
    fn a_wait_ends_with_the_process_the_stop_request_or_the_time() {
        let never = || false;
        // the process ends in the third slice
        let mut n = 0;
        let r = wait_in_slices(10_000, &never, |ms| { n += 1; assert!(ms <= STOP_POLL_MS); if n == 3 { Slice::Ended(7) } else { Slice::NotYet } });
        assert_eq!(r, Ok(LaunchOutcome::Exited(7)));
        // it never ends: the slices add up to the limit, the last one cut short
        let mut slices = Vec::new();
        let r = wait_in_slices(600, &never, |ms| { slices.push(ms); Slice::NotYet });
        assert_eq!(r, Err(LaunchError::TimedOut));
        assert_eq!(slices, vec![250, 250, 100]);
        // a stop request is looked for before every slice
        let asked = Cell::new(0);
        let stop = || { asked.set(asked.get() + 1); asked.get() > 2 };
        let mut slices = 0;
        let r = wait_in_slices(60_000, &stop, |_| { slices += 1; Slice::NotYet });
        assert_eq!(r, Err(LaunchError::Interrupted));
        assert_eq!(slices, 2, "two slices were waited, the third question was a stop");
        // a stop that is already there waits not at all
        let r = wait_in_slices(60_000, &|| true, |_| panic!("no wait"));
        assert_eq!(r, Err(LaunchError::Interrupted));
        // a broken wait is a failure, and a zero limit is a time-out at once
        assert!(matches!(wait_in_slices(1_000, &never, |_| Slice::Broken), Err(LaunchError::Failed(_))));
        assert_eq!(wait_in_slices(0, &never, |_| panic!("no wait")), Err(LaunchError::TimedOut));
    }

    #[test]
    fn only_exe_files_are_started_without_the_shell() {
        assert!(is_exe(r"C:\a\b.exe") && is_exe(r"C:\a\B.EXE") && is_exe("c:/a/b.exe"));
        assert!(!is_exe(r"C:\a\b.bat") && !is_exe(r"C:\a\b.exe.txt") && !is_exe("") && !is_exe(r"C:\a\.exe\x"));
    }

    #[test]
    fn the_logon_session_key_is_there_and_does_not_change() {
        let a = logon_session_key().expect("a process token can be read");
        assert!(!a.is_empty());
        assert_eq!(logon_session_key().as_deref(), Some(a.as_str()));
    }

    #[test]
    fn the_file_marker_remembers_its_own_sign_in_only() {
        let dir = std::env::temp_dir().join(format!("rf-startup-marker-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let mut first = FileSessionMarker::new(&dir.join("deeper"), "1a-2b");
        assert!(!first.already_ran(), "no file yet");
        first.mark_ran();
        assert!(first.already_ran());
        // a host restarted in the same sign-in sees it; the next sign-in (another key) does not
        assert!(FileSessionMarker::new(&dir.join("deeper"), " 1a-2b\n").already_ran());
        assert!(!FileSessionMarker::new(&dir.join("deeper"), "1a-2c").already_ran());
        // the new sign-in marks over the old key
        let mut second = FileSessionMarker::new(&dir.join("deeper"), "1a-2c");
        second.mark_ran();
        assert!(second.already_ran() && !first.already_ran());
        assert!(!dir.join("deeper").join("shell-startup-session.tmp").exists(), "no half-written file is left");
        // a marker with no key never claims anything and writes nothing
        let mut none = FileSessionMarker::new(&dir.join("other"), "  ");
        none.mark_ran();
        assert!(!none.already_ran() && !dir.join("other").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_snapshot_of_this_session_has_this_process_and_a_file_name_for_every_unreadable_one() {
        let snap = SystemProcesses.snapshot();
        assert!(!snap.images.is_empty());
        assert!(snap.unreadable_names.iter().all(|n| !n.contains('\0')));
    }
}
