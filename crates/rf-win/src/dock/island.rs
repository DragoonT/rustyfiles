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
    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetClassLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_MOUSEACTIVATE, WM_NCCALCSIZE, WM_NCDESTROY, WNDPROC,
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

/// The island-styled windows whose frame was put away ([`make_popup`]): the client area of each is its whole window.
static FRAMELESS: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());

fn is_frameless(hwnd: isize) -> bool {
    FRAMELESS.lock().unwrap_or_else(|p| p.into_inner()).contains(&hwnd)
}

/// The reply of a window with no frame to "how big is your client area" (`WM_NCCALCSIZE`): 0 with the rectangle left as it came, which
/// is "all of the window". GPUI's own answer takes the system's resize frame off on three sides (8 px at 100 %).
pub fn frameless_reply(msg: u32, frameless: bool) -> Option<isize> {
    (frameless && msg == WM_NCCALCSIZE).then_some(0)
}

unsafe extern "system" fn island_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(reply) = island_reply(msg) {
        return LRESULT(reply);
    }
    if let Some(reply) = frameless_reply(msg, msg == WM_NCCALCSIZE && is_frameless(hwnd.0 as isize)) {
        return LRESULT(reply);
    }
    // an island is not minimized: its own menu's command is refused, and one that another program minimized comes back (`stayup.rs`)
    if crate::stayup::is_minimize_command(msg, wp.0) {
        return LRESULT(0);
    }
    if crate::stayup::was_minimized(msg, wp.0) {
        crate::stayup::bring_back(hwnd.0 as isize);
    }
    let Some(prev) = previous_proc(hwnd.0 as isize) else { return unsafe { DefWindowProcW(hwnd, msg, wp, lp) } };
    // the window is going: forget it (a handle may be reused by another window later)
    if msg == WM_NCDESTROY {
        PREVIOUS.lock().unwrap_or_else(|p| p.into_inner()).retain(|(h, _)| *h != hwnd.0 as isize);
        FRAMELESS.lock().unwrap_or_else(|p| p.into_inner()).retain(|h| *h != hwnd.0 as isize);
        crate::stayup::forget(hwnd.0 as isize);
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

/// [`make_island`] for a POP-UP that belongs to an island and has the system's round corners (the hover preview, the tray's tip): the
/// window also loses its frame, so that its client area is the whole window.
///
/// WHY: an island's window keeps the system's invisible resize frame (8 px left, right and below at 100 %), and `make_island` switches
/// DWM's non-client rendering off. With that off DWM takes the WHOLE window for the frame, invisible part included (read from a running
/// dock: its "extended frame bounds" are the window's rectangle, where a panel that still has non-client rendering reports its client
/// area and one pixel). An island asks for no rounding and no shadow, so nothing of that is seen. A pop-up that asks for round corners
/// got them, and its shadow, around the outer rectangle: the picture the app draws stood square-cornered inside a see-through band
/// with a rounded shadow around the band (seen on a user's screen over a light map, 2026-11-13). With no frame there is one rectangle:
/// what the app draws, what the glass covers, what DWM rounds and shadows and what a DWM thumbnail's destination is measured from.
pub fn make_popup(hwnd: isize) -> anyhow::Result<()> {
    make_island(hwnd)?;
    if previous_proc(hwnd).is_none() {
        anyhow::bail!("{hwnd:#x} has no island procedure to answer for its frame");
    }
    {
        let mut list = FRAMELESS.lock().unwrap_or_else(|p| p.into_inner());
        if !list.contains(&hwnd) {
            list.push(hwnd);
        }
    }
    // (the frame is worked out again now: the window keeps its place and its outer size, the client area becomes all of it)
    unsafe {
        let _ = SetWindowPos(hwnd_of(hwnd), None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
    Ok(())
}

/// Moves and sizes the island so that its CLIENT AREA is `r` (device px, screen coordinates) and keeps it on top, without activating
/// it and without changing whether it is shown. Returns false when Windows refused.
///
/// The client area, not the window: the islands' windows keep the system's invisible resize frame (8 px left, right and below at
/// 100 %), and everything the dock works out (where the glass is, where a panel opens, the window's region) is in the area the app
/// draws in. Placed by the outer rectangle, the client area came out 16 px narrower and 8 px shorter than the layout's window: the
/// glass, laid out for the full size, lost its right end and its bottom edge (measured on a running dock: a 278 x 50 glass in a
/// 268 x 48 client area).
pub fn place(hwnd: isize, r: Rect) -> bool {
    let o = super::panel::outer_for_client(r, super::panel::frame_insets(hwnd));
    unsafe { SetWindowPos(hwnd_of(hwnd), Some(HWND_TOPMOST), o.left, o.top, o.width().max(1), o.height().max(1), SWP_NOACTIVATE | SWP_NOOWNERZORDER).is_ok() }
}

/// Puts the island back above the other topmost windows (a full-screen application or another always-on-top window may have taken
/// the place) without moving it or activating it.
pub fn raise(hwnd: isize) {
    unsafe {
        let _ = SetWindowPos(hwnd_of(hwnd), Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// Whether `above` is higher in the z order than `below` (among the top-level windows). A read-only look, no message is sent: what a
/// panel that covers an island asks before it puts the island back on top (the activation of a topmost window raises it over the other
/// topmost windows, the islands included). `false` when either window is gone.
pub fn is_above(above: isize, below: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GW_HWNDPREV, GetWindow, IsWindow};
    if above == 0 || below == 0 || above == below {
        return false;
    }
    unsafe {
        if !IsWindow(Some(hwnd_of(above))).as_bool() || !IsWindow(Some(hwnd_of(below))).as_bool() {
            return false;
        }
        // from `below` up to the top: `above` is among the windows in front of it. A bound on the walk, in case the list changes under us.
        let mut cur = hwnd_of(below);
        for _ in 0..4096 {
            match GetWindow(cur, GW_HWNDPREV) {
                Ok(prev) if !prev.0.is_null() => {
                    if prev == hwnd_of(above) {
                        return true;
                    }
                    cur = prev;
                }
                _ => return false,
            }
        }
    }
    false
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
    fn only_a_frameless_window_answers_for_its_frame_and_only_that_message() {
        assert_eq!(frameless_reply(WM_NCCALCSIZE, true), Some(0), "all of the window is client area");
        assert_eq!(frameless_reply(WM_NCCALCSIZE, false), None, "an island keeps the frame its window gives it");
        assert_eq!(frameless_reply(WM_MOUSEACTIVATE, true), None);
        assert_eq!(frameless_reply(0x0005 /* WM_SIZE */, true), None);
        assert!(!is_frameless(0) && !is_frameless(0x1234));
        assert!(make_popup(0).is_err());
    }

    /// A REAL WINDOW, never shown: a pop-up with a title bar's style (what the toolkit makes) has a frame, and after `make_popup` its
    /// client area is its whole window, wherever it is moved and whatever size it is given, and DWM's frame is that rectangle too.
    #[test]
    fn a_popup_made_from_a_framed_window_has_no_frame_left() {
        use crate::dock::panel::{frame_insets, outer_for_client};
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_CAPTION, WS_CLIPCHILDREN};
        use windows::core::w;
        let h = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!(""), WS_CAPTION | WS_CLIPCHILDREN, 300, 200, 220, 174, None, None, None, None) }.expect("a hidden window");
        let key = h.0 as isize;
        let before = frame_insets(key);
        assert!(before.iter().any(|i| *i > 0), "a window with a title bar's style has a frame: {before:?}");
        make_popup(key).expect("the window exists");
        assert!(is_frameless(key));
        assert_eq!(frame_insets(key), [0; 4], "no frame left");
        // placed by its client area, as the pop-ups are: the window is that rectangle
        let want = Rect::from_xywh(640, 480, 204, 164);
        let o = outer_for_client(want, frame_insets(key));
        assert_eq!(o, want);
        unsafe { SetWindowPos(h, None, o.left, o.top, o.width(), o.height(), SWP_NOZORDER | SWP_NOACTIVATE) }.expect("moved");
        assert_eq!(frame_insets(key), [0; 4], "and none after a move and a new size");
        assert_eq!(rect(key), Some(want));
        assert_eq!(crate::dock::panel::client_origin_in_window(key), (0, 0), "a DWM picture's destination needs no shift");
        let mut f = RECT::default();
        if unsafe { DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut f as *mut RECT as *mut _, size_of::<RECT>() as u32) }.is_ok() {
            assert_eq!((f.left, f.top, f.right, f.bottom), (want.left, want.top, want.right, want.bottom), "what DWM rounds and shadows is the client area");
        }
        // a second call changes nothing; the window's going forgets it
        make_popup(key).expect("again");
        assert_eq!(frame_insets(key), [0; 4]);
        unsafe { DestroyWindow(h) }.expect("destroyed");
        assert!(!is_frameless(key), "forgotten with the window");
    }

    #[test]
    fn a_non_window_is_an_error_not_a_crash() {
        assert!(make_island(0).is_err());
        assert!(!place(0, Rect::from_xywh(0, 0, 10, 10)));
        assert!(rect(0).is_none());
    }
}
