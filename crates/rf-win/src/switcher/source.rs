//! Reading the facts [`super::rules`] decides on, from Windows. Read-only: nothing here changes a window.
//!
//! * [`gather`] walks the top-level windows once (`EnumWindows`, top of the z-order first). Every window is recorded with the few
//!   cheap facts the Alt+Tab walk needs (its style, its root owner, its last active popup); a visible one also gets its class,
//!   title, size, cloak bits and process. No window is sent a message: titles are read the way the taskbar's list reads them.
//! * [`desktops`] asks the virtual desktop manager (`IVirtualDesktopManager`, documented, in-process) for the windows that need it:
//!   only those cloaked by the shell, usually a handful. It is COM, so it runs on the shell thread (`sta::call`, with a limit)
//!   unless the caller says [`ComRoute::CallingThread`] (the windowless probe, which has no shell thread and creates no window).
//! * [`app_process`] finds the process a Store app's frame really belongs to (the frame is a window of `ApplicationFrameHost`; the
//!   app's own window is a `CoreWindow` child of it), so that the program name, the icon and "windows of this app" are the app's.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GA_ROOTOWNER, GWL_EXSTYLE, GetAncestor, GetClassNameW, GetLastActivePopup, GetPropW, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, RemovePropW, SetPropW,
};
use windows::core::{BOOL, w};

use super::rules::{Desktop, WindowFacts};

/// How long the shell thread may take to answer the virtual desktop questions.
pub const DESKTOP_LIMIT: Duration = Duration::from_millis(400);

/// Where the COM calls run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComRoute {
    /// On the shell STA thread through `sta::call` (what the app uses).
    ShellThread,
    /// On the calling thread, which becomes an apartment (the windowless probe: no shell thread, so no window at all).
    CallingThread,
}

fn hwnd(h: isize) -> HWND {
    HWND(h as *mut _)
}

// ---------------------------------------------------------------- our own shell windows

/// The window property that marks one of our shell windows. Any process can read it, which is how the switcher in one process
/// leaves out the dock's windows of another.
const MARK: windows::core::PCWSTR = w!("rustyfiles.shell-window");

/// Marks a window of ours as part of the shell (dock island, panel, desktop window, the switcher): it is never listed. Take the
/// mark off again before the window is destroyed ([`unmark_shell_window`]): Windows keeps window properties until then.
pub fn mark_shell_window(h: isize) {
    if h != 0 {
        unsafe {
            // (any non-null value will do: the property is only a flag)
            let _ = SetPropW(hwnd(h), MARK, Some(windows::Win32::Foundation::HANDLE(std::ptr::dangling_mut())));
        }
    }
}

pub fn unmark_shell_window(h: isize) {
    if h != 0 {
        unsafe {
            let _ = RemovePropW(hwnd(h), MARK);
        }
    }
}

pub fn is_shell_window(h: isize) -> bool {
    h != 0 && !unsafe { GetPropW(hwnd(h), MARK) }.is_invalid()
}

// ---------------------------------------------------------------- the facts

fn cloak_bits(h: HWND) -> u32 {
    let mut v: u32 = 0;
    if unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut u32 as *mut _, 4) }.is_ok() { v } else { 0 }
}

fn title_of(h: HWND) -> String {
    let n = unsafe { GetWindowTextLengthW(h) };
    if n <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; n as usize + 1];
    let got = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..got.max(0) as usize])
}

fn class_of(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn facts_of(h: HWND) -> WindowFacts {
    let raw = h.0 as isize;
    let visible = unsafe { IsWindowVisible(h) }.as_bool();
    let ex_style = unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) } as u32;
    let root = unsafe { GetAncestor(h, GA_ROOTOWNER) };
    let popup = unsafe { GetLastActivePopup(h) };
    let mut f = WindowFacts {
        hwnd: raw,
        visible,
        ex_style,
        root_owner: if root.0.is_null() { raw } else { root.0 as isize },
        last_popup: if popup.0.is_null() { raw } else { popup.0 as isize },
        ..Default::default()
    };
    // an invisible window is only a step of the walk: nothing else about it is asked
    if !visible {
        return f;
    }
    f.cloak = cloak_bits(h);
    f.minimized = unsafe { IsIconic(h) }.as_bool();
    f.class = class_of(h);
    f.title = title_of(h);
    let mut r = RECT::default();
    if unsafe { GetWindowRect(h, &mut r) }.is_ok() {
        f.width = r.right - r.left;
        f.height = r.bottom - r.top;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    f.pid = pid;
    f.shell_window = is_shell_window(raw);
    f
}

/// Every top-level window, top of the z-order first.
pub fn gather() -> Vec<WindowFacts> {
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let out = unsafe { &mut *(l.0 as *mut Vec<WindowFacts>) };
        out.push(facts_of(h));
        BOOL(1)
    }
    let mut out: Vec<WindowFacts> = Vec::with_capacity(512);
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut Vec<WindowFacts> as isize));
    }
    out
}

// ---------------------------------------------------------------- virtual desktops

fn ask_desktops(hwnds: &[isize]) -> HashMap<isize, Desktop> {
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
    crate::shell::ensure_com();
    let mut out = HashMap::new();
    let mgr: windows::core::Result<IVirtualDesktopManager> = unsafe { CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_ALL) };
    let Ok(mgr) = mgr else {
        log::debug!("switcher: no virtual desktop manager");
        return out;
    };
    for &h in hwnds {
        let d = match unsafe { mgr.IsWindowOnCurrentVirtualDesktop(hwnd(h)) } {
            Ok(b) if b.as_bool() => Desktop::Current,
            Ok(_) => Desktop::Other,
            Err(_) => Desktop::Unknown,
        };
        out.insert(h, d);
    }
    out
}

/// Where each of `hwnds` is. A window the manager could not answer for (or all of them, when the shell thread did not answer in
/// [`DESKTOP_LIMIT`]) is missing from the map, which the rules read as `Unknown`.
pub fn desktops(hwnds: Vec<isize>, route: ComRoute) -> HashMap<isize, Desktop> {
    if hwnds.is_empty() {
        return HashMap::new();
    }
    match route {
        ComRoute::CallingThread => ask_desktops(&hwnds),
        ComRoute::ShellThread => crate::sta::call(DESKTOP_LIMIT, move || ask_desktops(&hwnds)).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------- the monitor

/// The monitor the pointer is on (the nearest one when it is between monitors), as the dock describes a monitor: its rectangle, its
/// work area and its DPI, all in device px. The switcher opens on this monitor.
pub fn monitor_at_cursor() -> Option<crate::dock::system::MonitorInfo> {
    use crate::dock::layout::Rect;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext};
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    // rectangles in device px whatever the process was started as, for as long as this call lasts
    let before = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let found = (|| {
        let mut p = POINT::default();
        unsafe { GetCursorPos(&mut p).ok()? };
        let mon = unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST) };
        if mon.is_invalid() {
            return None;
        }
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() {
            return None;
        }
        let (mut dx, mut dy) = (0u32, 0u32);
        let dpi = if unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_ok() && dx > 0 { dx } else { 96 };
        let r = |r: RECT| Rect::new(r.left, r.top, r.right, r.bottom);
        Some(crate::dock::system::MonitorInfo { handle: mon.0 as isize, rect: r(mi.rcMonitor), work: r(mi.rcWork), dpi })
    })();
    unsafe { SetThreadDpiAwarenessContext(before) };
    found
}

// ---------------------------------------------------------------- whose window

/// The process a window should be attributed to: its own, except for a Store app's frame, which is attributed to the app's process
/// (found through the hosted `CoreWindow` child), when that can be found.
pub fn app_process(h: isize, class: &str, frame_pid: u32) -> u32 {
    if !class.eq_ignore_ascii_case("ApplicationFrameWindow") {
        return frame_pid;
    }
    struct Search {
        frame_pid: u32,
        found: u32,
    }
    unsafe extern "system" fn each(child: HWND, l: LPARAM) -> BOOL {
        let s = unsafe { &mut *(l.0 as *mut Search) };
        if class_of(child).eq_ignore_ascii_case("Windows.UI.Core.CoreWindow") {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(child, Some(&mut pid)) };
            if pid != 0 && pid != s.frame_pid {
                s.found = pid;
                return BOOL(0);
            }
        }
        BOOL(1)
    }
    let mut s = Search { frame_pid, found: 0 };
    unsafe {
        let _ = EnumChildWindows(Some(hwnd(h)), Some(each), LPARAM(&mut s as *mut Search as isize));
    }
    if s.found != 0 { s.found } else { frame_pid }
}

/// The program of a process.
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    crate::tasklist::exe_of(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_of_this_machine_have_their_basic_facts() {
        // read-only: whatever is open, each visible window has a process and a class; the first fact of the walk is consistent
        let facts = gather();
        assert!(!facts.is_empty());
        // (a window can close between the walk and the reads of its facts, which then come back empty: the few that did are not
        // asked for a process and a class, but the machine has some window that has both)
        let visible: Vec<&WindowFacts> = facts.iter().filter(|f| f.visible).collect();
        assert!(visible.iter().any(|f| f.pid != 0 && !f.class.is_empty()));
        for f in &visible {
            assert!(f.root_owner != 0 && f.last_popup != 0);
        }
    }

    #[test]
    fn the_monitor_under_the_pointer_has_a_work_area_inside_it() {
        // (reads the pointer's place; moves nothing)
        // (a runner with no display or no pointer has none: nothing to check there)
        let Some(m) = monitor_at_cursor() else { return };
        assert!(m.rect.width() > 0 && m.rect.height() > 0);
        assert!(m.rect.covers(&m.work), "{:?} in {:?}", m.work, m.rect);
        assert!(m.dpi >= 96);
    }

    #[test]
    fn no_desktop_questions_for_no_windows() {
        assert!(desktops(Vec::new(), ComRoute::ShellThread).is_empty());
        assert!(desktops(Vec::new(), ComRoute::CallingThread).is_empty());
    }

    #[test]
    fn a_property_mark_on_no_window_is_harmless() {
        mark_shell_window(0);
        unmark_shell_window(0);
        assert!(!is_shell_window(0));
    }
}
