//! The real taskbar windows (shell plan E1): list them, hide them, show them. THE ONLY FILE THAT HIDES OR SHOWS A WINDOW OF EXPLORER'S.
//! NOT RUN by the author: `hide` and `show` change another process's windows, and no test or example calls them (a test in
//! `tests/tb_static.rs` scans the sources to keep it that way). `list` and the process queries only read.
//!
//! * A window is found by enumerating top-level windows and reading the class; the owner is asked for its process id, its start time
//!   and whether it runs `explorer.exe` from the Windows folder. Nothing is sent to any window to find out.
//! * `set_visible` checks again, at the moment of the call, that the handle still is a window of the recorded class owned by the
//!   recorded process (id and start time), and refuses otherwise.
//! * The change is queued with `SWP_ASYNCWINDOWPOS`: a plain `SetWindowPos` on another process's window waits for that process's
//!   thread, and Explorer may be hung or restarting at the very moment the taskbar has to come back (or the host's thread would hang
//!   with it). Queued, the call returns at once. Whether the change happened is then read with `IsWindowVisible`.
//! * Only `SWP_HIDEWINDOW` / `SWP_SHOWWINDOW` with no move, size, z-order or activation change. No `ShowWindow`, no message, no
//!   `ABM_SETSTATE`, no registry.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM, RECT};
use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, SWP_ASYNCWINDOWPOS, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
    SetWindowPos,
};
use windows::core::{BOOL, PWSTR};

use crate::sys::TaskbarVisibility;
use crate::tbmodel::{self, HideRecord, LiveWindow, Target};

/// When the process started, in FILETIME ticks; None when it cannot be asked (gone, or not ours to ask).
pub fn process_start_time(pid: u32) -> Option<u64> {
    // SAFETY: a query-only handle, used for one call and closed.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let r = GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user);
        let _ = CloseHandle(h);
        r.ok()?;
        Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
}

fn windows_explorer_path() -> PathBuf {
    let root = std::env::var_os("SystemRoot").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("explorer.exe")
}

/// Does the process run `explorer.exe` from the Windows folder? (A program with that name from anywhere else is not Explorer.)
pub fn runs_windows_explorer(pid: u32) -> bool {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: query-only handle; `buf` holds `len` units.
    let path = unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        if r.is_err() {
            return false;
        }
        PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))
    };
    path.as_os_str().eq_ignore_ascii_case(windows_explorer_path().as_os_str())
}

struct Collect {
    found: Vec<LiveWindow>,
    explorer: HashMap<u32, bool>,
}

unsafe extern "system" fn collect_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the address of the `Collect` on the caller's stack, alive during EnumWindows; the queries only read.
    unsafe {
        let c = &mut *(lparam.0 as *mut Collect);
        let mut buf = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut buf);
        if n <= 0 {
            return BOOL(1);
        }
        let class = String::from_utf16_lossy(&buf[..n as usize]);
        if !tbmodel::is_taskbar_class(&class) {
            return BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let explorer = *c.explorer.entry(pid).or_insert_with(|| pid != 0 && runs_windows_explorer(pid));
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        c.found.push(LiveWindow {
            handle: hwnd.0 as isize,
            class,
            pid,
            start: if pid != 0 { process_start_time(pid).unwrap_or(0) } else { 0 },
            explorer,
            visible: IsWindowVisible(hwnd).as_bool(),
            rect: [r.left, r.top, r.right, r.bottom],
        });
    }
    BOOL(1)
}

/// The taskbar-class windows that exist now. Read-only.
pub fn list_taskbar_windows() -> Result<Vec<LiveWindow>, String> {
    let mut c = Collect { found: Vec::new(), explorer: HashMap::new() };
    // SAFETY: `c` outlives the call.
    unsafe { EnumWindows(Some(collect_cb), LPARAM(&mut c as *mut Collect as isize)) }.map_err(|e| e.to_string())?;
    Ok(c.found)
}

pub fn is_visible(handle: isize) -> bool {
    // SAFETY: a plain query.
    unsafe { IsWindowVisible(HWND(handle as *mut c_void)).as_bool() }
}

/// Hides or shows ONE window that is still the window `t` names. Queued (see the module docs): `Ok` means "asked", not "done".
pub fn set_visible(t: &Target, visible: bool) -> Result<(), String> {
    let hwnd = HWND(t.handle as *mut c_void);
    // SAFETY: plain queries, then one SetWindowPos on a window whose class and owner were just checked.
    unsafe {
        let mut buf = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut buf);
        if n <= 0 || String::from_utf16_lossy(&buf[..n as usize]) != t.class || !tbmodel::is_taskbar_class(&t.class) {
            return Err("the window is not the one that was recorded (class)".into());
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != t.pid || t.start == 0 || process_start_time(pid) != Some(t.start) {
            return Err("the window is not the one that was recorded (owner)".into());
        }
        let action = if visible { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
        SetWindowPos(hwnd, None, 0, 0, 0, 0, action | SWP_ASYNCWINDOWPOS | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE).map_err(|e| e.to_string())
    }
}

/// Waits (briefly, in small steps: half a second at most) until `targets` are visible and returns how many still are not. The
/// change asked for by `set_visible` is queued in Explorer's own thread, so this is the only way to know that it happened. A
/// handle that is not a window any more is counted as hidden here; callers pass windows they have just seen exist.
pub fn wait_visible(targets: &[Target]) -> usize {
    for _ in 0..10 {
        if targets.iter().all(|t| is_visible(t.handle)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    targets.iter().filter(|t| !is_visible(t.handle)).count()
}

/// Waits (briefly, in small steps: half a second at most) until `targets` are hidden and returns the ones that still are visible.
/// The change asked for by `set_visible` is queued in Explorer's own thread, so this is the only way to know that the hide
/// happened; nothing else (the dock's edge, the work areas) is touched for a window that is still visible. A handle that is not a
/// window any more counts as hidden.
pub fn wait_hidden(targets: &[Target]) -> Vec<Target> {
    for _ in 0..10 {
        if targets.iter().all(|t| !is_visible(t.handle)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    targets.iter().filter(|t| is_visible(t.handle)).cloned().collect()
}

/// Shows `targets` and waits until they are visible. `Err` names how many did not come back.
fn show_and_verify(targets: &[Target]) -> Result<(), String> {
    let mut failed = 0;
    for t in targets {
        if set_visible(t, true).is_err() {
            failed += 1;
        }
    }
    let hidden = wait_visible(targets);
    if failed > 0 || hidden > 0 { Err(format!("{} taskbar window(s) did not come back", failed.max(hidden))) } else { Ok(()) }
}

/// Shows every hidden taskbar window of Explorer's, with no record to go by. Used by the panic hook and by the sweep.
pub fn sweep_show_all() -> Result<usize, String> {
    let live = list_taskbar_windows()?;
    let targets = tbmodel::plan_sweep(&live);
    show_and_verify(&targets)?;
    Ok(live.iter().filter(|w| w.explorer && tbmodel::is_taskbar_class(&w.class)).count())
}

/// The guard's real taskbar side.
pub struct RealTaskbar;

impl TaskbarVisibility for RealTaskbar {
    fn show_windows_taskbar(&mut self) -> Result<usize, String> {
        sweep_show_all()
    }

    fn show_recorded(&mut self, rec: &HideRecord) -> Result<usize, String> {
        let live = list_taskbar_windows()?;
        let plan = tbmodel::plan_restore(rec, &live);
        show_and_verify(&plan.show)?;
        Ok(plan.restored_count())
    }
}
