//! A press on a file in a window that is not in front leaves the window where it is until the press turns out not to be a drag, as
//! Explorer does.
//!
//! WHY (issue 169: "RustyFiles stays on top when drag and dropping a file into another program": a media player over part of the
//! window vanished behind it as soon as a file was pressed, so the file could not be dropped on it). Windows activates a window,
//! and raises it, at the press itself. Explorer's list answers `WM_MOUSEACTIVATE` with "do not activate" and decides at the release.
//!
//! What a main window does here, when it is not the active window and gets a LEFT press in its client area:
//! * the activation is refused (`MA_NOACTIVATE`): the press still reaches the view;
//! * the view handles the press; when it was on an item (the view calls [`press_on_item`]), the window is activated at the release
//!   that follows, and a drag (Windows' drag loop takes the release) leaves it where it was;
//! * any other press (the toolbar, a tab, empty space) activates the window as soon as the view has handled it, so everything else
//!   behaves as before.
//!
//! The title bar, the frame and the other buttons are not touched. Runs on the window's thread (its procedure).

use std::cell::Cell;
use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GWLP_WNDPROC, SetForegroundWindow, SetWindowLongPtrW, WM_ACTIVATE, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEACTIVATE, WM_NCDESTROY, WNDPROC,
};

const HTCLIENT: u32 = 1;
const MA_NOACTIVATE: isize = 3;
const WA_INACTIVE: usize = 0;

/// (window, its procedure before ours).
static PREVIOUS: Mutex<Vec<(isize, isize)>> = Mutex::new(Vec::new());

thread_local! {
    /// The window whose last press was let in without activating it (until its WM_LBUTTONDOWN is handled).
    static HELD: Cell<isize> = const { Cell::new(0) };
    /// Set by the view while it handles that press: it was on an item.
    static ON_ITEM: Cell<bool> = const { Cell::new(false) };
    /// The window to activate at the next release (a press on an item that may still become a drag).
    static AT_RELEASE: Cell<isize> = const { Cell::new(0) };
}

/// What happens to a press, decided from `WM_MOUSEACTIVATE`'s hit-test code and mouse message. Pure.
pub fn holds_activation(hit: u32, mouse_msg: u32) -> bool {
    hit == HTCLIENT && mouse_msg == WM_LBUTTONDOWN
}

/// Called by the file list while it handles a left press on an item.
pub fn press_on_item() {
    ON_ITEM.with(|c| c.set(true));
}

fn previous_proc(key: isize) -> Option<isize> {
    PREVIOUS.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(h, _)| *h == key).map(|(_, p)| *p)
}

fn activate(hwnd: HWND) {
    // (the release or the press is the input being handled, so this thread may take the foreground)
    let _ = unsafe { SetForegroundWindow(hwnd) };
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let key = hwnd.0 as isize;
    let Some(prev) = previous_proc(key) else { return unsafe { DefWindowProcW(hwnd, msg, wp, lp) } };
    // SAFETY: `prev` is what `SetWindowLongPtrW(GWLP_WNDPROC)` returned for this very window
    let call = |msg, wp, lp| unsafe { CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(prev), hwnd, msg, wp, lp) };
    match msg {
        WM_MOUSEACTIVATE => {
            let hit = (lp.0 as u32) & 0xFFFF;
            let mouse = ((lp.0 as u32) >> 16) & 0xFFFF;
            if holds_activation(hit, mouse) {
                HELD.with(|c| c.set(key));
                return LRESULT(MA_NOACTIVATE);
            }
        }
        WM_LBUTTONDOWN if HELD.with(|c| c.get()) == key => {
            HELD.with(|c| c.set(0));
            ON_ITEM.with(|c| c.set(false));
            let r = call(msg, wp, lp);
            if ON_ITEM.with(|c| c.replace(false)) {
                AT_RELEASE.with(|c| c.set(key));
            } else {
                activate(hwnd);
            }
            return r;
        }
        WM_LBUTTONUP if AT_RELEASE.with(|c| c.get()) == key => {
            AT_RELEASE.with(|c| c.set(0));
            // activated first: the view's click handlers then run in an active window, as before
            activate(hwnd);
        }
        WM_ACTIVATE if wp.0 & 0xFFFF != WA_INACTIVE => {
            if AT_RELEASE.with(|c| c.get()) == key {
                AT_RELEASE.with(|c| c.set(0));
            }
        }
        WM_NCDESTROY => {
            PREVIOUS.lock().unwrap_or_else(|p| p.into_inner()).retain(|(h, _)| *h != key);
        }
        _ => {}
    }
    call(msg, wp, lp)
}

/// Puts this behaviour in front of the window's procedure (once per window). Must run on the thread that owns the window.
pub fn install(hwnd: isize) {
    if hwnd == 0 || previous_proc(hwnd).is_some() {
        return;
    }
    let h = HWND(hwnd as *mut _);
    let old = unsafe { SetWindowLongPtrW(h, GWLP_WNDPROC, proc as *const () as usize as isize) };
    if old == 0 {
        log::warn!("press_activate: the procedure of {hwnd:#x} could not be wrapped");
        return;
    }
    PREVIOUS.lock().unwrap_or_else(|p| p.into_inner()).push((hwnd, old));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_left_press_in_the_client_area_is_held() {
        assert!(holds_activation(HTCLIENT, WM_LBUTTONDOWN));
        // the title bar (HTCAPTION), a frame edge (HTLEFT), a right press
        assert!(!holds_activation(2, WM_LBUTTONDOWN));
        assert!(!holds_activation(10, WM_LBUTTONDOWN));
        assert!(!holds_activation(HTCLIENT, windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONDOWN));
    }
}
