//! Turning a GPUI window into a dock island and placing it. Everything takes the window's `HWND` as an `isize`, like the rest of
//! rf-win, and none of it needs a message loop of its own: call it from the UI thread after the window exists.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_EXCLUDED_FROM_PEEK, DWMWA_NCRENDERING_POLICY,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GCL_STYLE, GWL_EXSTYLE, GWLP_WNDPROC, GetClassLongPtrW, GetWindowLongPtrW, HWND_TOPMOST, IsWindowVisible, SW_HIDE, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetClassLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_MOUSEACTIVATE, WM_NCDESTROY, WNDPROC,
    WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

use super::layout::Rect;

/// `CS_DROPSHADOW`: the window class asks for a shadow around its windows.
const CS_DROPSHADOW: usize = 0x0002_0000;

fn hwnd_of(h: isize) -> HWND {
    HWND(h as *mut _)
}

/// The extended style an island has: a tool window (no taskbar button, not in Alt+Tab) that a click never activates.
pub const ISLAND_EX_ADD: u32 = WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0;

/// The extended style after the change: the island bits on, the "give me a taskbar button" bit off.
pub fn island_ex_style(ex: u32) -> u32 {
    (ex | ISLAND_EX_ADD) & !WS_EX_APPWINDOW.0
}

/// `MA_NOACTIVATE`: the reply to `WM_MOUSEACTIVATE` that keeps the click from activating the window but still delivers it.
const MA_NOACTIVATE: isize = 3;

/// The reply an island gives to a message itself, before its window's own procedure sees it. GPUI answers `WM_MOUSEACTIVATE` with
/// `MA_ACTIVATE` for every window, and an explicit reply like that is not something `WS_EX_NOACTIVATE` is known to override: the
/// island answers it first, so a click on it never takes the keyboard from the app the user was typing in.
pub fn island_reply(msg: u32) -> Option<isize> {
    (msg == WM_MOUSEACTIVATE).then_some(MA_NOACTIVATE)
}

/// The procedures the island windows had before [`make_island`] put its own in front: `(window, procedure)`.
static PREVIOUS: std::sync::Mutex<Vec<(isize, isize)>> = std::sync::Mutex::new(Vec::new());

fn previous_proc(hwnd: isize) -> Option<isize> {
    PREVIOUS.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(h, _)| *h == hwnd).map(|(_, p)| *p)
}

unsafe extern "system" fn island_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(reply) = island_reply(msg) {
        return LRESULT(reply);
    }
    let Some(prev) = previous_proc(hwnd.0 as isize) else { return unsafe { DefWindowProcW(hwnd, msg, wp, lp) } };
    // the window is going: forget it (a handle may be reused by another window later)
    if msg == WM_NCDESTROY {
        PREVIOUS.lock().unwrap_or_else(|p| p.into_inner()).retain(|(h, _)| *h != hwnd.0 as isize);
    }
    // SAFETY: `prev` is the procedure `SetWindowLongPtrW(GWLP_WNDPROC)` returned for this very window
    unsafe { CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(prev), hwnd, msg, wp, lp) }
}

/// Puts the island's own procedure in front of the window's (once per window). Must run on the thread that owns the window.
fn answer_mouse_activation(h: HWND) {
    let key = h.0 as isize;
    if previous_proc(key).is_some() {
        return;
    }
    let old = unsafe { SetWindowLongPtrW(h, GWLP_WNDPROC, island_proc as *const () as usize as isize) };
    if old == 0 {
        log::warn!("dock: the mouse activation of {key:#x} could not be answered");
        return;
    }
    PREVIOUS.lock().unwrap_or_else(|p| p.into_inner()).push((key, old));
}

/// Makes the window an island: always on top, never activated by a click, no taskbar button, not in Alt+Tab, no system corner
/// rounding, no border colour, no shadow, and left out of Aero Peek. Does not move or resize it, does not activate it, and can be
/// called again (it changes nothing the second time). Errors are for the window itself (not a window); the DWM attributes only exist
/// on newer Windows and are ignored where the system does not know them.
pub fn make_island(hwnd: isize) -> anyhow::Result<()> {
    let h = hwnd_of(hwnd);
    if !crate::window::exists(hwnd) {
        anyhow::bail!("{hwnd:#x} is not a window");
    }
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        let new = island_ex_style(ex);
        if new != ex {
            // Windows only takes a changed TOOLWINDOW / APPWINDOW into account for the taskbar when the window is hidden and shown again
            let was_visible = IsWindowVisible(h).as_bool();
            if was_visible {
                let _ = ShowWindow(h, SW_HIDE);
            }
            SetWindowLongPtrW(h, GWL_EXSTYLE, new as isize);
            let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            if was_visible {
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            }
        }
        // the shadow: no drop-shadow class style, and no non-client rendering (DWM draws the frame shadow as part of it)
        let class_style = GetClassLongPtrW(h, GCL_STYLE);
        if class_style & CS_DROPSHADOW != 0 {
            SetClassLongPtrW(h, GCL_STYLE, (class_style & !CS_DROPSHADOW) as isize);
        }
        let set = |attr, value: u32| {
            if let Err(e) = DwmSetWindowAttribute(h, attr, &value as *const u32 as *const _, size_of::<u32>() as u32) {
                log::debug!("dock: DWM attribute {attr:?} not set on {hwnd:#x}: {e}");
            }
        };
        set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND.0 as u32);
        set(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
        set(DWMWA_NCRENDERING_POLICY, DWMNCRP_DISABLED.0 as u32);
        set(DWMWA_EXCLUDED_FROM_PEEK, 1);
        answer_mouse_activation(h);
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
    Ok(())
}

/// Moves and sizes the island (device px, screen coordinates) and keeps it on top, without activating it and without changing
/// whether it is shown. Returns false when Windows refused.
pub fn place(hwnd: isize, r: Rect) -> bool {
    unsafe { SetWindowPos(hwnd_of(hwnd), Some(HWND_TOPMOST), r.left, r.top, r.width().max(1), r.height().max(1), SWP_NOACTIVATE | SWP_NOOWNERZORDER).is_ok() }
}

/// Puts the island back above the other topmost windows (a full-screen application or another always-on-top window may have taken
/// the place) without moving it or activating it.
pub fn raise(hwnd: isize) {
    unsafe {
        let _ = SetWindowPos(hwnd_of(hwnd), Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// Shows or hides the island without activating anything (a full-screen application is in front: hide; it left: show).
pub fn set_visible(hwnd: isize, visible: bool) {
    unsafe {
        let _ = ShowWindow(hwnd_of(hwnd), if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
    if visible {
        raise(hwnd);
    }
}

/// The window's rectangle now, device px (to see where an island really is).
pub fn rect(hwnd: isize) -> Option<Rect> {
    crate::window::rect(hwnd).map(|(r, _)| Rect::new(r[0], r[1], r[2], r[3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn island_style_adds_tool_window_and_no_activate_and_drops_app_window() {
        let ex = WS_EX_APPWINDOW.0 | 0x0000_0008 /* WS_EX_TOPMOST */ | 0x0020_0000 /* WS_EX_NOREDIRECTIONBITMAP */;
        let new = island_ex_style(ex);
        assert_ne!(new & WS_EX_TOOLWINDOW.0, 0);
        assert_ne!(new & WS_EX_NOACTIVATE.0, 0);
        assert_eq!(new & WS_EX_APPWINDOW.0, 0, "no taskbar button");
        assert_ne!(new & 0x0000_0008, 0, "unrelated bits stay");
        assert_ne!(new & 0x0020_0000, 0, "unrelated bits stay");
        assert_eq!(island_ex_style(new), new, "idempotent");
    }

    #[test]
    fn a_click_never_activates_an_island_and_nothing_else_is_answered_early() {
        assert_eq!(island_reply(WM_MOUSEACTIVATE), Some(3));
        assert_eq!(island_reply(0x0200 /* WM_MOUSEMOVE */), None);
        assert_eq!(island_reply(0x0201 /* WM_LBUTTONDOWN */), None);
        assert_eq!(island_reply(0), None);
    }

    #[test]
    fn a_non_window_is_an_error_not_a_crash() {
        assert!(make_island(0).is_err());
        assert!(!place(0, Rect::from_xywh(0, 0, 10, 10)));
        assert!(rect(0).is_none());
    }
}
