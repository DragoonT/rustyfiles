//! The process side: start and watch the host, start Explorer, find out whether Explorer's desktop is up.
//! `spawn_host` (a child that is a small windowless program) and the waits are meant to be exercised
//! by a test; `start_explorer` with no replacement program starts the real `explorer.exe` and is NOT RUN
//! by the author of this crate.

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess,
    WaitForMultipleObjects, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetShellWindow, GetWindowThreadProcessId, IsHungAppWindow, IsWindowVisible};
use windows::core::{BOOL, PCWSTR, PWSTR};

use super::wide;
use crate::machine::Millis;
use crate::sys::{HostLaunch, PidWait, Processes, Wait};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct RealProcesses {
    child: Option<Child>,
    ready: Option<isize>,
    ready_seen: bool,
    /// `--test-shell --test-explorer <exe>`: this program stands in for Explorer, and "Explorer is up" means it is running.
    explorer_stand_in: Option<PathBuf>,
    /// The explorer.exe (or stand-in) this process started. Kept so that a second `start_explorer` does not start another
    /// one while the first is still coming up: Explorer that is slow to show its desktop would answer a second start by
    /// opening a stray folder window.
    explorer_child: Option<Child>,
    explorer_starts: u32,
    /// A host this process did not start (see `Processes::adopt_host`).
    adopted: Option<(u32, PathBuf)>,
    /// Hide the console of children (tests spawn console programs).
    no_window: bool,
}

impl RealProcesses {
    pub fn new(explorer_stand_in: Option<PathBuf>) -> RealProcesses {
        RealProcesses { child: None, ready: None, ready_seen: false, explorer_stand_in, explorer_child: None, explorer_starts: 0, adopted: None, no_window: false }
    }

    /// How many times `start_explorer` really started a program (for tests).
    pub fn explorer_starts(&self) -> u32 {
        self.explorer_starts
    }

    /// For tests: children get `CREATE_NO_WINDOW`.
    pub fn without_windows(mut self) -> RealProcesses {
        self.no_window = true;
        self
    }

    fn close_ready(&mut self) {
        if let Some(h) = self.ready.take() {
            // SAFETY: created in `spawn_host`, closed once.
            unsafe {
                let _ = CloseHandle(HANDLE(h as *mut _));
            }
        }
    }
}

fn sleep_ms(ms: Millis) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn explorer_path() -> PathBuf {
    let root = std::env::var_os("SystemRoot").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("explorer.exe")
}

/// The full path of the executable behind an open process handle (it needs the query right).
fn image_path_of(h: HANDLE) -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` units; `h` is open for the call.
    unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

/// The file name of the executable behind a process id.
fn image_name(pid: u32) -> Option<String> {
    // SAFETY: opens a query-only handle, reads the name, closes the handle.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let path = image_path_of(h);
        let _ = CloseHandle(h);
        path?.file_name().map(|n| n.to_string_lossy().into_owned())
    }
}

/// Do two paths name the same file? Case does not matter on Windows; a path that cannot be resolved (the file moved)
/// is compared as written.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x.as_os_str().eq_ignore_ascii_case(y.as_os_str()),
        _ => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
    }
}

/// Ends the process `pid` if, and only if, it runs `image`. True when it was ended.
fn end_process_running(pid: u32, image: &Path) -> bool {
    // SAFETY: the handle is opened, used and closed here.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid) else { return false };
        let ours = image_path_of(h).is_some_and(|p| same_file(&p, image));
        if ours {
            let _ = TerminateProcess(h, 1);
            // Short, bounded: the next step (Explorer) must not start while the host's window is still on the screen.
            WaitForSingleObject(h, 3_000);
        }
        let _ = CloseHandle(h);
        ours
    }
}

/// Is `GetShellWindow()` a visible window of explorer.exe?
fn explorer_owns_shell_window() -> bool {
    // SAFETY: plain queries.
    unsafe {
        let hwnd = GetShellWindow();
        if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid != 0 && image_name(pid).is_some_and(|n| n.eq_ignore_ascii_case("explorer.exe"))
    }
}

struct HungSearch {
    pid: u32,
    hung: bool,
}

unsafe extern "system" fn hung_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the address of the `HungSearch` on the caller's stack, alive during EnumWindows.
    let s = unsafe { &mut *(lparam.0 as *mut HungSearch) };
    let mut pid = 0u32;
    // SAFETY: plain queries.
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == s.pid && IsWindowVisible(hwnd).as_bool() && IsHungAppWindow(hwnd).as_bool() {
            s.hung = true;
            return BOOL(0);
        }
    }
    BOOL(1)
}

/// Waits on up to three handles and returns the index that fired (`None`: timeout or failure).
fn wait_handles(handles: &[HANDLE], timeout_ms: Option<Millis>) -> Option<usize> {
    if handles.is_empty() {
        sleep_ms(timeout_ms.unwrap_or(1_000).min(60_000));
        return None;
    }
    let ms = timeout_ms.map(|m| m.min(u64::from(u32::MAX - 1)) as u32).unwrap_or(INFINITE);
    // SAFETY: the handles are open for the call.
    let r = unsafe { WaitForMultipleObjects(handles, false, ms) };
    if r == WAIT_TIMEOUT {
        return None;
    }
    if r == WAIT_FAILED {
        // Do not spin if a handle went bad.
        sleep_ms(100);
        return None;
    }
    let idx = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
    (idx < handles.len()).then_some(idx)
}

impl Processes for RealProcesses {
    fn spawn_host(&mut self, launch: &HostLaunch) -> Result<u32, String> {
        self.close_ready();
        self.ready_seen = false;
        let name = wide(&launch.ready_event);
        // SAFETY: `name` is NUL-terminated. Manual reset, not signalled; the host opens it by name from the environment.
        let ev = unsafe { CreateEventW(None, true, false, PCWSTR(name.as_ptr())) }.map_err(|e| format!("the ready event: {e}"))?;
        self.ready = Some(ev.0 as isize);
        let mut cmd = Command::new(&launch.exe);
        cmd.args(&launch.args).env("RUSTYFILES_SHELL_READY_EVENT", &launch.ready_event).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if self.no_window {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().map_err(|e| format!("{}: {e}", launch.exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))?;
        let pid = child.id();
        self.child = Some(child);
        Ok(pid)
    }

    fn wait(&mut self, extra: isize, timeout_ms: Option<Millis>) -> Wait {
        let mut handles: Vec<HANDLE> = Vec::new();
        let mut kinds: Vec<u8> = Vec::new(); // 0 host, 1 ready, 2 extra
        if let Some(c) = &self.child {
            handles.push(HANDLE(c.as_raw_handle()));
            kinds.push(0);
        }
        // The ready event is manual-reset: once seen it stays signalled, so it is left out of later waits.
        if let (Some(h), false) = (self.ready, self.ready_seen)
            && self.child.is_some()
        {
            handles.push(HANDLE(h as *mut _));
            kinds.push(1);
        }
        if extra != 0 {
            handles.push(HANDLE(extra as *mut _));
            kinds.push(2);
        }
        match wait_handles(&handles, timeout_ms).map(|i| kinds[i]) {
            Some(0) => {
                let Some(mut child) = self.child.take() else { return Wait::Timeout };
                let pid = child.id();
                let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
                Wait::HostExited { pid, code }
            }
            Some(1) => {
                self.ready_seen = true;
                Wait::HostReady
            }
            Some(_) => Wait::Extra,
            None => Wait::Timeout,
        }
    }

    fn wait_pid(&mut self, pid: u32, extra: isize, timeout_ms: Option<Millis>) -> PidWait {
        let mut handles: Vec<HANDLE> = Vec::new();
        let mut proc_handle = None;
        if pid != 0 {
            // SAFETY: opens a handle that can only be waited on.
            match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
                Ok(h) => {
                    handles.push(h);
                    proc_handle = Some(h);
                }
                // Cannot be opened: it does not exist (or is not ours, which a host of ours never is).
                Err(_) => return PidWait::Gone,
            }
        }
        if extra != 0 {
            handles.push(HANDLE(extra as *mut _));
        }
        let fired = wait_handles(&handles, timeout_ms);
        if let Some(h) = proc_handle {
            // SAFETY: opened above, closed once.
            unsafe {
                let _ = CloseHandle(h);
            }
        }
        match fired {
            Some(0) if proc_handle.is_some() => PidWait::Gone,
            Some(_) => PidWait::Extra,
            None => PidWait::Timeout,
        }
    }

    fn host_hung(&mut self) -> bool {
        let Some(c) = &self.child else { return false };
        let mut s = HungSearch { pid: c.id(), hung: false };
        // SAFETY: `s` outlives the call; the callback only reads and writes it.
        unsafe {
            let _ = EnumWindows(Some(hung_cb), LPARAM(&mut s as *mut HungSearch as isize));
        }
        s.hung
    }

    fn kill_host(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        } else if let Some((pid, image)) = self.adopted.take() {
            end_process_running(pid, &image);
        }
    }

    fn adopt_host(&mut self, pid: u32, image: &Path) {
        if pid != 0 && pid != std::process::id() {
            self.adopted = Some((pid, image.to_path_buf()));
        }
    }

    fn explorer_owns_shell_window(&mut self) -> bool {
        // With a stand-in (`--test-shell`) the real Explorer is none of this run's business.
        self.explorer_stand_in.is_none() && explorer_owns_shell_window()
    }

    /// The host binary is a file that starts like a program. It is not run: the host has no `--check` yet, and
    /// asking an unknown program to start "to see if it works" could open a window.
    fn host_self_test(&mut self, launch: &HostLaunch) -> bool {
        use std::io::Read;
        let mut magic = [0u8; 2];
        std::fs::File::open(&launch.exe).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"MZ"
    }

    fn start_explorer(&mut self) -> Result<(), String> {
        let path = self.explorer_stand_in.clone().unwrap_or_else(explorer_path);
        if !path.is_file() {
            return Err("the program to start was not found".into());
        }
        // The one we started is still running (slow to show its desktop, or the wait ran out too early): a second
        // explorer.exe would hand over to it and open a folder window. Keep waiting on the first instead.
        if self.explorer_child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None))) {
            return Ok(());
        }
        let mut cmd = Command::new(&path);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if self.no_window {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().map_err(|e| e.to_string())?;
        self.explorer_starts += 1;
        // Dropping a `Child` does not end the process; the handle is kept only to ask whether it still runs.
        self.explorer_child = Some(child);
        Ok(())
    }

    fn explorer_shell_up(&mut self, wait_ms: Millis) -> bool {
        let started = Instant::now();
        loop {
            let up = match (&self.explorer_stand_in, self.explorer_child.as_mut()) {
                (Some(_), Some(c)) => matches!(c.try_wait(), Ok(None)),
                (Some(_), None) => false,
                (None, _) => explorer_owns_shell_window(),
            };
            if up {
                return true;
            }
            if started.elapsed() >= Duration::from_millis(wait_ms) {
                return false;
            }
            // There is no event for "Explorer's desktop appeared" that works before it exists; this runs only in the fall-back, for at most ten seconds.
            sleep_ms(250);
        }
    }
}

impl Drop for RealProcesses {
    fn drop(&mut self) {
        self.close_ready();
    }
}
