//! The app's side of the experimental Glass backdrop (the engine is `rf_win::glass`): the switch that forces it for one run, the reason it
//! was given up (for the settings page) and the one rule the app's own popups follow.

use std::sync::{Mutex, OnceLock};

/// `RUSTYFILES_GLASS=1`: every main window uses the Glass backdrop for this run, whatever the saved setting says (and nothing is saved).
/// Read once. "0" and an empty value mean off.
pub fn forced() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUSTYFILES_GLASS").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// Why the engine gave up on a window, for the settings page. Only the latest failure is kept: a failed window is on Mica, the page
/// says so under the backdrop card.
static FAILURE: Mutex<Option<String>> = Mutex::new(None);

pub fn failure() -> Option<String> {
    FAILURE.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

pub fn set_failure(why: Option<String>) {
    *FAILURE.lock().unwrap_or_else(|p| p.into_inner()) = why;
}

/// The windows whose glass is on the screen right now (their scene is see-through). The palette asks for every window it paints, so a
/// flag answers first when no window has glass at all.
static SHOWN: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static SHOWN_ANY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_shown(hwnd: isize, on: bool) {
    let mut v = SHOWN.lock().unwrap_or_else(|p| p.into_inner());
    v.retain(|&h| h != hwnd);
    if on && hwnd != 0 {
        v.push(hwnd);
    }
    SHOWN_ANY.store(!v.is_empty(), std::sync::atomic::Ordering::Relaxed);
}

/// Whether this window has its glass on the screen.
pub fn shown(hwnd: isize) -> bool {
    SHOWN_ANY.load(std::sync::atomic::Ordering::Relaxed) && SHOWN.lock().unwrap_or_else(|p| p.into_inner()).contains(&hwnd)
}

/// A popup, menu or flyout of ours (its HWND at hand) stays out of the screen capture while some window has glass. Otherwise the glass
/// would show a copy of the popup, bent, behind the popup itself. Called for every such window, glass or not: the engine remembers it,
/// applies the exclusion when glass starts (also for a window that is already open) and lifts it when the last glass goes.
pub(crate) fn keep_out_of_capture(hwnd: isize) {
    if hwnd != 0 {
        rf_win::glass::exclude_from_capture(hwnd, true);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn failure_is_kept_and_cleared() {
        super::set_failure(Some("no duplication".into()));
        assert_eq!(super::failure().as_deref(), Some("no duplication"));
        super::set_failure(None);
        assert_eq!(super::failure(), None);
    }

    #[test]
    fn shown_windows_are_remembered_one_by_one() {
        assert!(!super::shown(41));
        super::set_shown(41, true);
        super::set_shown(42, true);
        assert!(super::shown(41) && super::shown(42) && !super::shown(43));
        super::set_shown(41, false);
        assert!(!super::shown(41) && super::shown(42));
        super::set_shown(42, false);
        assert!(!super::shown(42));
    }

    #[test]
    fn a_popup_without_glass_is_left_alone() {
        // 0 is no window: nothing is remembered or touched
        super::keep_out_of_capture(0);
    }
}
