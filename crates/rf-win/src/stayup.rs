//! Windows of ours that must not stay minimized: the dock's islands and the desktop cover.
//!
//! A user's report: while another program worked on the screen (an assistant that drives the computer and puts every window away
//! first) the dock and the desktop were gone, and nothing brought them back: the islands and the covers had been MINIMIZED like
//! ordinary windows (they stood as 160 x 28 stubs at the bottom left, since they have no taskbar button). A taskbar and a desktop
//! are not windows the user minimizes. So such a window refuses the minimize command of its own system menu, and when it finds
//! itself minimized by someone else (which cannot be refused: `ShowWindow` from another process just happens) it asks to be shown
//! again, without taking the foreground.
//!
//! Bounded: a program that minimizes it again and again wins after [`MOST`] returns in [`WINDOW_MS`] (the window then stays down
//! until the next change), so two programs never fight over a window for ever.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindowAsync};

const WM_SIZE: u32 = 0x0005;
const WM_SYSCOMMAND: u32 = 0x0112;
const SIZE_MINIMIZED: usize = 1;
const SC_MINIMIZE: usize = 0xF020;

/// The most returns from "minimized" in one stretch of [`WINDOW_MS`].
pub const MOST: u32 = 12;
pub const WINDOW_MS: u64 = 10_000;

/// The window has just been minimized (`WM_SIZE` with `SIZE_MINIMIZED`).
pub fn was_minimized(msg: u32, wp: usize) -> bool {
    msg == WM_SIZE && wp == SIZE_MINIMIZED
}

/// The minimize command of the window's own system menu or of a key (the low four bits of the command are the system's own).
pub fn is_minimize_command(msg: u32, wp: usize) -> bool {
    msg == WM_SYSCOMMAND && (wp & 0xFFF0) == SC_MINIMIZE
}

/// How many returns a window has had in the stretch that began at `since`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    since: u64,
    count: u32,
}

impl Budget {
    /// May the window come back at `now` (ms on any clock that only goes forward)? Counts the return when it may.
    pub fn allow(&mut self, now: u64) -> bool {
        if now.saturating_sub(self.since) >= WINDOW_MS {
            *self = Budget { since: now, count: 0 };
        }
        if self.count >= MOST {
            return false;
        }
        self.count += 1;
        true
    }
}

static BUDGETS: Mutex<Option<(Instant, HashMap<isize, Budget>)>> = Mutex::new(None);

/// Asks the minimized window to be shown again, where and as large as it was, without activating it. Queued to the window's own
/// thread, so it is safe from inside the window's procedure. False: it has come back too often lately and is left as it is.
pub fn bring_back(hwnd: isize) -> bool {
    let allowed = {
        let mut g = BUDGETS.lock().unwrap_or_else(|p| p.into_inner());
        let (t0, map) = g.get_or_insert_with(|| (Instant::now(), HashMap::new()));
        let now = t0.elapsed().as_millis() as u64;
        map.entry(hwnd).or_default().allow(now)
    };
    if !allowed {
        log::warn!("window {hwnd:#x} was minimized again and again; left minimized");
        return false;
    }
    log::info!("window {hwnd:#x} was minimized by another program; showing it again");
    // SAFETY: a queued request for a window handle; a handle that is gone is refused by the system.
    let _ = unsafe { ShowWindowAsync(HWND(hwnd as *mut _), SW_SHOWNOACTIVATE) };
    true
}

/// The window is gone: its count goes with it (a handle may be reused).
pub fn forget(hwnd: isize) {
    if let Some((_, map)) = BUDGETS.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        map.remove(&hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_minimize_is_a_minimize() {
        assert!(was_minimized(WM_SIZE, 1));
        assert!(!was_minimized(WM_SIZE, 0) && !was_minimized(WM_SIZE, 2), "restored and maximized are not");
        assert!(!was_minimized(WM_SYSCOMMAND, 1));
        assert!(is_minimize_command(WM_SYSCOMMAND, 0xF020) && is_minimize_command(WM_SYSCOMMAND, 0xF022), "the low bits are the system's");
        assert!(!is_minimize_command(WM_SYSCOMMAND, 0xF030) && !is_minimize_command(WM_SYSCOMMAND, 0xF060) && !is_minimize_command(WM_SIZE, 0xF020));
    }

    #[test]
    fn a_window_comes_back_a_bounded_number_of_times_in_a_stretch_and_again_in_the_next() {
        let mut b = Budget::default();
        for i in 0..MOST {
            assert!(b.allow(100 + i as u64 * 10), "return {i}");
        }
        assert!(!b.allow(500), "a program that keeps minimizing it wins");
        assert!(!b.allow(WINDOW_MS - 1), "still the same stretch (a new budget's stretch begins at 0)");
        assert!(b.allow(WINDOW_MS), "a new stretch");
    }
}
