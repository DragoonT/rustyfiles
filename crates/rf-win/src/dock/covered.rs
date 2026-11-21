//! Whether a program's window lies over a part of the screen: what auto-hide asks when it hides the dock only while a window covers
//! it (a user on Discord, 2026-11-20: "make the dock disappear when [a] window [is] open but once I minimize/close the window, it
//! appears back").
//!
//! A window counts when it is a task window (visible, not minimised, not cloaked: one on another virtual desktop or a suspended app's is
//! cloaked; no owner, not a tool window, not one that never takes the focus), it belongs to another process than this one (the dock's
//! own windows), it is not the desktop's, and its visible frame (`DWMWA_EXTENDED_FRAME_BOUNDS`, without the invisible resize border)
//! meets the rectangle. One `EnumWindows` pass: well under a millisecond with a few dozen windows.

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::BOOL;

use super::layout::Rect;

/// Whether two rectangles share any area.
pub fn meets(a: Rect, b: Rect) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// The classes of the desktop's own windows, which cover the whole screen and are never "a window over the dock".
fn is_desktop_class(class: &str) -> bool {
    matches!(class, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

struct Walk<'a> {
    areas: &'a [Rect],
    own_pid: u32,
    found: bool,
}

/// Whether a window of another program lies over one of `areas` (screen device px: the dock's islands).
pub fn window_over(areas: &[Rect]) -> bool {
    if areas.is_empty() {
        return false;
    }
    let mut walk = Walk { areas, own_pid: std::process::id(), found: false };
    // SAFETY: `walk` lives on this stack frame through EnumWindows, which calls back on this thread only.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut walk as *mut Walk<'_> as isize));
    }
    walk.found
}

unsafe extern "system" fn each(h: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the address of the `Walk` of `window_over`, alive during EnumWindows; the calls only read.
    unsafe {
        let walk = &mut *(lparam.0 as *mut Walk<'_>);
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
            return BOOL(1);
        }
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        if ex & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0 || GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
            return BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == walk.own_pid {
            return BOOL(1);
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, size_of::<u32>() as u32);
        if cloaked != 0 {
            return BOOL(1);
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(h, &mut buf) as usize;
        if is_desktop_class(&String::from_utf16_lossy(&buf[..n.min(buf.len())])) {
            return BOOL(1);
        }
        let mut r = RECT::default();
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, size_of::<RECT>() as u32).is_err() {
            return BOOL(1);
        }
        let frame = Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
        if walk.areas.iter().any(|a| meets(frame, *a)) {
            walk.found = true;
            // (stops the walk)
            return BOOL(0);
        }
        BOOL(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_meet_only_when_they_share_area() {
        let dock = Rect { left: 0, top: 1380, right: 2560, bottom: 1440 };
        assert!(meets(Rect { left: 100, top: 100, right: 900, bottom: 1400 }, dock), "a window reaching into the dock's band");
        assert!(!meets(Rect { left: 100, top: 100, right: 900, bottom: 1380 }, dock), "a window ending at its top edge");
        assert!(!meets(Rect { left: 2560, top: 1300, right: 3000, bottom: 1440 }, dock), "a window on the next monitor");
        assert!(is_desktop_class("WorkerW") && !is_desktop_class("Chrome_WidgetWin_1"));
    }
}
