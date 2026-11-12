//! Choosing a window: bringing it to the front reliably. A small sequence behind a trait, so that the order of calls is tested with a
//! fake and the real calls are made only by the user's own choice in the switcher.
//!
//! THE FOREGROUND RULES (docs for `SetForegroundWindow`; research note 4-desktop.md 6.4): a process may take the foreground when it
//! has it, or received the last input, and so on. The switcher HOLDS the foreground while it is open (it is the window in front,
//! and the chord's keys or the click went to it), so the first `SetForegroundWindow` is allowed. The sequence:
//!
//! 1. the window is gone: stop (nothing to do, the switcher closes);
//! 2. the window is minimized: restore it first, WITHOUT waiting for its thread (`ShowWindowAsync`: a window that hangs must not
//!    hang the switcher), then wait up to 250 ms for it to stop being minimized (a minimized window cannot be brought forward);
//! 3. `SetForegroundWindow`; when the window (or its root owner) is in front now AND is not cloaked: done. A cloaked window can be
//!    "the foreground window" without anything to be seen (the shell cloaks a suspended Store app and the leftover frame of a
//!    closed one); the call is given a moment (UNCLOAK_WAIT_MS) to uncloak it, as it does a restored suspended app, and if the
//!    window stays cloaked it is not in front, whatever `GetForegroundWindow` says;
//! 4. once more through the thread-input route (`AttachThreadInput` to the window in front, `BringWindowToTop`, then
//!    `SetForegroundWindow`), which succeeds in cases the plain call does not;
//! 5. still not in front: flash its taskbar button (the program refuses to come forward; the user is told, nothing is forced).
//!
//! No synthetic key is ever sent (the "Alt tap" trick lights up menu bars of the program in front); the flash is the fallback.

/// The calls the sequence makes.
pub trait ActivateOps {
    fn exists(&mut self, hwnd: isize) -> bool;
    fn is_minimized(&mut self, hwnd: isize) -> bool;
    /// Whether DWM cloaks the window (any of the cloak reasons): it is not on the screen, whatever else is true of it.
    fn is_cloaked(&mut self, hwnd: isize) -> bool;
    /// Asks for the window to be restored, without waiting for it.
    fn restore(&mut self, hwnd: isize);
    fn sleep_ms(&mut self, ms: u64);
    /// `SetForegroundWindow`; the answer is the call's own.
    fn set_foreground(&mut self, hwnd: isize) -> bool;
    /// The window in front now, as its root owner (0: none).
    fn foreground_root(&mut self) -> isize;
    /// The root owner of a window.
    fn root_of(&mut self, hwnd: isize) -> isize;
    /// The thread-input route (step 4).
    fn raise_attached(&mut self, hwnd: isize);
    fn flash(&mut self, hwnd: isize);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The window is in front.
    Done,
    /// The window no longer exists.
    Gone,
    /// The window would not come forward; its button flashes.
    Flashed,
}

/// How long a restore is waited for.
const RESTORE_WAIT_MS: u64 = 250;
const RESTORE_POLL_MS: u64 = 10;
/// How long a window that is in front but cloaked is waited for to uncloak (a restored suspended app does, a moment later).
const UNCLOAK_WAIT_MS: u64 = 150;

/// Whether the window is in front and can be seen.
fn in_front(ops: &mut dyn ActivateOps, hwnd: isize) -> bool {
    let want = ops.root_of(hwnd);
    let front = ops.foreground_root();
    if front == 0 || (front != hwnd && front != want) {
        return false;
    }
    let mut waited = 0;
    while ops.is_cloaked(hwnd) {
        if waited >= UNCLOAK_WAIT_MS {
            return false;
        }
        ops.sleep_ms(RESTORE_POLL_MS);
        waited += RESTORE_POLL_MS;
    }
    true
}

/// Brings `hwnd` to the front.
pub fn activate(ops: &mut dyn ActivateOps, hwnd: isize) -> Outcome {
    if !ops.exists(hwnd) {
        return Outcome::Gone;
    }
    if ops.is_minimized(hwnd) {
        ops.restore(hwnd);
        let mut waited = 0;
        while ops.is_minimized(hwnd) && waited < RESTORE_WAIT_MS {
            ops.sleep_ms(RESTORE_POLL_MS);
            waited += RESTORE_POLL_MS;
        }
    }
    ops.set_foreground(hwnd);
    if in_front(ops, hwnd) {
        return Outcome::Done;
    }
    ops.raise_attached(hwnd);
    if in_front(ops, hwnd) {
        return Outcome::Done;
    }
    ops.flash(hwnd);
    Outcome::Flashed
}

/// The real calls.
pub struct Win32Ops;

fn h(hwnd: isize) -> windows::Win32::Foundation::HWND {
    windows::Win32::Foundation::HWND(hwnd as *mut _)
}

impl ActivateOps for Win32Ops {
    fn exists(&mut self, hwnd: isize) -> bool {
        crate::window::exists(hwnd)
    }

    fn is_minimized(&mut self, hwnd: isize) -> bool {
        unsafe { windows::Win32::UI::WindowsAndMessaging::IsIconic(h(hwnd)).as_bool() }
    }

    fn restore(&mut self, hwnd: isize) {
        use windows::Win32::UI::WindowsAndMessaging::{SW_RESTORE, ShowWindowAsync};
        unsafe {
            let _ = ShowWindowAsync(h(hwnd), SW_RESTORE);
        }
    }

    fn is_cloaked(&mut self, hwnd: isize) -> bool {
        use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
        let mut v: u32 = 0;
        unsafe { DwmGetWindowAttribute(h(hwnd), DWMWA_CLOAKED, &mut v as *mut u32 as *mut _, 4) }.is_ok() && v != 0
    }

    fn sleep_ms(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    fn set_foreground(&mut self, hwnd: isize) -> bool {
        unsafe { windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(h(hwnd)).as_bool() }
    }

    fn foreground_root(&mut self) -> isize {
        crate::dock::foreground_root()
    }

    fn root_of(&mut self, hwnd: isize) -> isize {
        use windows::Win32::UI::WindowsAndMessaging::{GA_ROOTOWNER, GetAncestor};
        let r = unsafe { GetAncestor(h(hwnd), GA_ROOTOWNER) };
        if r.0.is_null() { hwnd } else { r.0 as isize }
    }

    fn raise_attached(&mut self, hwnd: isize) {
        use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
        use windows::Win32::UI::WindowsAndMessaging::{BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow};
        unsafe {
            let front = GetForegroundWindow();
            let theirs = GetWindowThreadProcessId(front, None);
            let mine = GetCurrentThreadId();
            let attached = theirs != 0 && theirs != mine && AttachThreadInput(mine, theirs, true).as_bool();
            let _ = BringWindowToTop(h(hwnd));
            let _ = SetForegroundWindow(h(hwnd));
            if attached {
                let _ = AttachThreadInput(mine, theirs, false);
            }
        }
    }

    fn flash(&mut self, hwnd: isize) {
        use windows::Win32::UI::WindowsAndMessaging::{FLASHW_ALL, FLASHW_TIMERNOFG, FLASHWINFO, FlashWindowEx};
        let info = FLASHWINFO { cbSize: size_of::<FLASHWINFO>() as u32, hwnd: h(hwnd), dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG, uCount: 3, dwTimeout: 0 };
        unsafe {
            let _ = FlashWindowEx(&info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted window system that records every call.
    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        exists: bool,
        /// Polls of `is_minimized` that answer yes, in all (the first call included).
        minimized_answers: usize,
        asked_min: usize,
        /// Polls of `is_cloaked` that answer yes, in all.
        cloaked_answers: usize,
        asked_cloak: usize,
        /// Whether the plain `SetForegroundWindow` puts the window in front.
        plain_works: bool,
        /// Whether the attached route does.
        attached_works: bool,
        front: isize,
        root: isize,
    }

    impl ActivateOps for Fake {
        fn exists(&mut self, hwnd: isize) -> bool {
            self.calls.push(format!("exists {hwnd}"));
            self.exists
        }
        fn is_minimized(&mut self, _: isize) -> bool {
            self.asked_min += 1;
            let yes = self.asked_min <= self.minimized_answers;
            self.calls.push(format!("minimized? {yes}"));
            yes
        }
        fn is_cloaked(&mut self, _: isize) -> bool {
            self.asked_cloak += 1;
            let yes = self.asked_cloak <= self.cloaked_answers;
            self.calls.push(format!("cloaked? {yes}"));
            yes
        }
        fn restore(&mut self, hwnd: isize) {
            self.calls.push(format!("restore {hwnd}"));
        }
        fn sleep_ms(&mut self, ms: u64) {
            self.calls.push(format!("sleep {ms}"));
        }
        fn set_foreground(&mut self, hwnd: isize) -> bool {
            self.calls.push(format!("set_foreground {hwnd}"));
            if self.plain_works {
                self.front = hwnd;
            }
            self.plain_works
        }
        fn foreground_root(&mut self) -> isize {
            self.calls.push("foreground".into());
            self.front
        }
        fn root_of(&mut self, hwnd: isize) -> isize {
            self.calls.push(format!("root_of {hwnd}"));
            if self.root != 0 { self.root } else { hwnd }
        }
        fn raise_attached(&mut self, hwnd: isize) {
            self.calls.push(format!("raise_attached {hwnd}"));
            if self.attached_works {
                self.front = hwnd;
            }
        }
        fn flash(&mut self, hwnd: isize) {
            self.calls.push(format!("flash {hwnd}"));
        }
    }

    fn calls(f: &Fake) -> Vec<&str> {
        f.calls.iter().map(String::as_str).collect()
    }

    #[test]
    fn an_ordinary_window_is_asked_to_the_front_once() {
        let mut f = Fake { exists: true, plain_works: true, front: 99, ..Default::default() };
        assert_eq!(activate(&mut f, 7), Outcome::Done);
        assert_eq!(calls(&f), vec!["exists 7", "minimized? false", "set_foreground 7", "root_of 7", "foreground", "cloaked? false"]);
    }

    #[test]
    fn a_window_that_is_gone_is_left_alone() {
        let mut f = Fake { exists: false, ..Default::default() };
        assert_eq!(activate(&mut f, 7), Outcome::Gone);
        assert_eq!(calls(&f), vec!["exists 7"]);
    }

    #[test]
    fn a_minimized_window_is_restored_first_then_waited_for_then_brought_forward() {
        // minimized at the first look and at the first poll, restored at the second poll
        let mut f = Fake { exists: true, minimized_answers: 2, plain_works: true, front: 99, ..Default::default() };
        assert_eq!(activate(&mut f, 7), Outcome::Done);
        assert_eq!(
            calls(&f),
            vec![
                "exists 7",
                "minimized? true",
                "restore 7",
                "minimized? true",
                "sleep 10",
                "minimized? false",
                "set_foreground 7",
                "root_of 7",
                "foreground",
                "cloaked? false"
            ]
        );
    }

    #[test]
    fn a_restore_that_never_finishes_is_not_waited_for_for_ever() {
        let mut f = Fake { exists: true, minimized_answers: usize::MAX, plain_works: true, front: 99, ..Default::default() };
        assert_eq!(activate(&mut f, 7), Outcome::Done);
        let sleeps = f.calls.iter().filter(|c| c.starts_with("sleep")).count();
        assert_eq!(sleeps as u64, RESTORE_WAIT_MS / RESTORE_POLL_MS);
        // and the foreground call still follows
        assert!(f.calls.iter().any(|c| c == "set_foreground 7"));
    }

    #[test]
    fn a_refusal_is_tried_once_more_through_the_thread_input_route() {
        let mut f = Fake { exists: true, plain_works: false, attached_works: true, front: 99, ..Default::default() };
        assert_eq!(activate(&mut f, 7), Outcome::Done);
        assert_eq!(
            calls(&f),
            vec![
                "exists 7",
                "minimized? false",
                "set_foreground 7",
                "root_of 7",
                "foreground",
                "raise_attached 7",
                "root_of 7",
                "foreground",
                "cloaked? false"
            ]
        );
    }

    #[test]
    fn a_window_that_is_in_front_but_stays_cloaked_is_not_done_it_is_flashed() {
        // the foreground window is reported as the one chosen, but DWM never shows it (a leftover frame of a closed Store app)
        let mut f = Fake { exists: true, plain_works: true, cloaked_answers: usize::MAX, front: 99, ..Default::default() };
        assert_eq!(activate(&mut f, 7), Outcome::Flashed);
        assert_eq!(f.calls.last().map(String::as_str), Some("flash 7"));
        // it was waited for, but not for ever: UNCLOAK_WAIT_MS per look, two looks (the plain call and the attached route)
        let sleeps = f.calls.iter().filter(|c| c.starts_with("sleep")).count() as u64;
        assert_eq!(sleeps, 2 * UNCLOAK_WAIT_MS / RESTORE_POLL_MS);
    }

    #[test]
    fn a_restored_suspended_app_that_uncloaks_a_moment_later_is_done() {
        let mut f = Fake { exists: true, minimized_answers: 1, plain_works: true, cloaked_answers: 3, front: 99, ..Default::default() };
        assert_eq!(activate(&mut f, 7), Outcome::Done);
        assert!(!f.calls.iter().any(|c| c.starts_with("flash") || c.starts_with("raise_attached")));
    }

    #[test]
    fn a_window_that_still_refuses_is_flashed_and_nothing_is_forced() {
        let mut f = Fake { exists: true, plain_works: false, attached_works: false, front: 99, ..Default::default() };
        assert_eq!(activate(&mut f, 7), Outcome::Flashed);
        assert_eq!(f.calls.last().map(String::as_str), Some("flash 7"));
        // (that no key is ever sent is checked on the code itself: `nothing_in_the_switcher_sends_a_key...` in mod.rs)
    }

    #[test]
    fn the_root_owner_in_front_counts_as_the_window_in_front() {
        // the window is a dialog; its owner is what GetForegroundWindow reports after the call
        let mut f = Fake { exists: true, plain_works: false, attached_works: false, front: 50, root: 50, ..Default::default() };
        assert_eq!(activate(&mut f, 7), Outcome::Done);
        assert!(!f.calls.iter().any(|c| c.starts_with("raise_attached")));
    }

    #[test]
    fn nothing_in_front_is_not_success() {
        let mut f = Fake { exists: true, plain_works: false, attached_works: false, front: 0, ..Default::default() };
        assert_eq!(activate(&mut f, 7), Outcome::Flashed);
    }

    #[test]
    fn the_real_calls_do_nothing_to_a_window_that_does_not_exist() {
        // (0 is not a window: the sequence stops at the first call and touches nothing)
        assert_eq!(activate(&mut Win32Ops, 0), Outcome::Gone);
    }
}
