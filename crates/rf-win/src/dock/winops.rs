//! The few things the dock's menu and previews ask of other programs' windows: a title, whether it is minimised, a request to close.
//! None of them waits for the other program: a window that hangs must not hang the dock.
//!
//! A window is named by a [`WinRef`]: its handle AND the process it belonged to when the dock looked at it. Window handles are reused
//! by Windows after a window is destroyed, and a menu can stay open (or a preview) for a long time after the list of the app's windows
//! was taken; acting on a bare handle could then close a window of another program. An action therefore checks, at the moment it is
//! asked for, that the handle still belongs to the same process (and is still a visible window) before it does anything.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PostMessageW, SC_CLOSE, WM_SYSCOMMAND,
};

fn h(hwnd: isize) -> HWND {
    HWND(hwnd as *mut _)
}

/// The window's title; empty when it has none or is gone. For a window of another process Windows answers from its own copy of the
/// caption, without sending the window a message, so a program that does not respond cannot hold this up.
pub fn window_title(hwnd: isize) -> String {
    if hwnd == 0 {
        return String::new();
    }
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h(hwnd), &mut buf) };
    if n <= 0 { String::new() } else { String::from_utf16_lossy(&buf[..n as usize]) }
}

/// Whether the window is minimised.
pub fn is_minimized(hwnd: isize) -> bool {
    hwnd != 0 && unsafe { IsIconic(h(hwnd)).as_bool() }
}

/// A window as the dock saw it: its handle and the process it belonged to then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinRef {
    pub hwnd: isize,
    pub pid: u32,
}

/// What [`WinRef`] checks and does, so that the decision can be tested without another program's windows.
pub trait WinApi {
    /// The process the window belongs to now; `None` when there is no such window.
    fn owner(&self, hwnd: isize) -> Option<u32>;
    fn visible(&self, hwnd: isize) -> bool;
    /// Posts the window the command its own close button gives (`WM_SYSCOMMAND` with `SC_CLOSE`). False when it could not be posted.
    fn post_close(&self, hwnd: isize) -> bool;
}

/// The real windows.
pub struct Win32;

impl WinApi for Win32 {
    fn owner(&self, hwnd: isize) -> Option<u32> {
        if hwnd == 0 || !unsafe { IsWindow(Some(h(hwnd))).as_bool() } {
            return None;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h(hwnd), Some(&mut pid)) };
        (pid != 0).then_some(pid)
    }

    fn visible(&self, hwnd: isize) -> bool {
        hwnd != 0 && unsafe { IsWindowVisible(h(hwnd)).as_bool() }
    }

    fn post_close(&self, hwnd: isize) -> bool {
        unsafe { PostMessageW(Some(h(hwnd)), WM_SYSCOMMAND, WPARAM(SC_CLOSE as usize), LPARAM(0)).is_ok() }
    }
}

impl WinRef {
    /// The window now, or `None` when `hwnd` is no window.
    pub fn of_with(api: &dyn WinApi, hwnd: isize) -> Option<WinRef> {
        api.owner(hwnd).map(|pid| WinRef { hwnd, pid })
    }

    pub fn of(hwnd: isize) -> Option<WinRef> {
        Self::of_with(&Win32, hwnd)
    }

    /// Whether the handle still names the window that was seen: it exists, belongs to the same process and is visible (a minimised
    /// window is visible).
    pub fn is_still_with(&self, api: &dyn WinApi) -> bool {
        api.owner(self.hwnd) == Some(self.pid) && api.visible(self.hwnd)
    }

    pub fn is_still(&self) -> bool {
        self.is_still_with(&Win32)
    }
}

/// The windows now, those that are gone left out.
pub fn refs(hwnds: &[isize]) -> Vec<WinRef> {
    hwnds.iter().filter_map(|&w| WinRef::of(w)).collect()
}

/// Asks a window to close, the way its own close button does (`WM_SYSCOMMAND` with `SC_CLOSE`, posted: the program decides, may ask the
/// user about unsaved work, and a window that has disabled its close command, or has no close button, is not sent a request the
/// taskbar would not have sent either). Only when the handle is still the window that was seen (see the module's note). False when
/// nothing was posted.
pub fn close_window_with(api: &dyn WinApi, w: WinRef) -> bool {
    w.is_still_with(api) && api.post_close(w.hwnd)
}

pub fn close_window(w: WinRef) -> bool {
    close_window_with(&Win32, w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Windows that exist only in the test: (handle, process, visible).
    struct Fake {
        windows: Vec<(isize, u32, bool)>,
        posted: RefCell<Vec<isize>>,
    }

    impl WinApi for Fake {
        fn owner(&self, hwnd: isize) -> Option<u32> {
            self.windows.iter().find(|w| w.0 == hwnd).map(|w| w.1)
        }
        fn visible(&self, hwnd: isize) -> bool {
            self.windows.iter().any(|w| w.0 == hwnd && w.2)
        }
        fn post_close(&self, hwnd: isize) -> bool {
            self.posted.borrow_mut().push(hwnd);
            true
        }
    }

    fn fake(windows: Vec<(isize, u32, bool)>) -> Fake {
        Fake { windows, posted: RefCell::new(Vec::new()) }
    }

    #[test]
    fn a_window_that_is_not_there_has_no_title_is_not_minimised_and_is_not_seen() {
        assert_eq!(window_title(0), "");
        assert!(!is_minimized(0));
        assert_eq!(WinRef::of(0), None);
        // a number that is no window (never a real handle)
        assert_eq!(WinRef::of(0x7FFF_FFF1), None);
        assert!(!close_window(WinRef { hwnd: 0x7FFF_FFF1, pid: 1 }));
        assert!(!close_window(WinRef { hwnd: 0, pid: 1 }));
        assert_eq!(window_title(0x7FFF_FFF1), "");
    }

    #[test]
    fn a_window_that_is_still_the_one_seen_is_asked_to_close() {
        let api = fake(vec![(10, 500, true)]);
        let w = WinRef::of_with(&api, 10).unwrap();
        assert_eq!(w, WinRef { hwnd: 10, pid: 500 });
        assert!(close_window_with(&api, w));
        assert_eq!(*api.posted.borrow(), vec![10]);
    }

    #[test]
    fn a_handle_that_another_program_has_taken_over_is_never_sent_a_close() {
        // the menu saw window 10 of process 500; the window went and Windows gave the number to a window of process 900
        let seen = WinRef { hwnd: 10, pid: 500 };
        let api = fake(vec![(10, 900, true)]);
        assert!(!seen.is_still_with(&api));
        assert!(!close_window_with(&api, seen));
        assert!(api.posted.borrow().is_empty(), "nothing is posted to a window that is not the one that was seen");
    }

    #[test]
    fn a_window_that_is_gone_or_hidden_is_not_sent_a_close_either() {
        let seen = WinRef { hwnd: 10, pid: 500 };
        let gone = fake(vec![]);
        assert!(!close_window_with(&gone, seen));
        let hidden = fake(vec![(10, 500, false)]);
        assert!(!close_window_with(&hidden, seen));
        assert!(gone.posted.borrow().is_empty() && hidden.posted.borrow().is_empty());
    }

    #[test]
    fn the_request_is_the_command_of_the_close_button_not_a_bare_close_message() {
        // WM_SYSCOMMAND / SC_CLOSE is what the title bar's button and the taskbar send; a window that disabled its close command
        // refuses it, which a posted WM_CLOSE would not
        assert_eq!(SC_CLOSE, 0xF060);
        assert_eq!(WM_SYSCOMMAND, 0x0112);
    }
}
