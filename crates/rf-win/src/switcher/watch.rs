//! When an open switcher puts itself away. Pure: the app looks at the foreground and the mouse every 50 ms while the switcher is
//! open (and only then: nothing runs while it is closed) and asks [`should_dismiss`].
//!
//! Why it is needed: the switcher closes at once when it LOSES the foreground it had, but a window that never got the foreground
//! (a launcher or a foreground lock refused it) is never "lost" and, with the keyboard going elsewhere, Escape would not reach it
//! either; it would sit on the screen over up to 92 % of the monitor until a card is clicked. So a click outside always puts it
//! away, and so does a foreground that stays somebody else's for longer than the grace the dock's panels use.

use crate::dock::Rect;
use crate::dock::panel;

/// One look at the foreground and the mouse.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// The switcher's own window.
    pub hwnd: isize,
    /// The root window in front now (0: none, for the moment between two windows).
    pub foreground: isize,
    /// Whether the switcher has been in front since it opened (this look included).
    pub been_front: bool,
    pub since_open_ms: u64,
    /// A mouse button was down at the last look / is now.
    pub was_down: bool,
    pub down: bool,
    /// The pointer, device px.
    pub cursor: Option<(i32, i32)>,
    /// The switcher's window, device px.
    pub rect: Rect,
}

/// Whether to close. `hooked`: the keys come from the host's keyboard hook, so the switcher's window is not expected to be in front
/// at all and only a click outside puts it away (the host closes it on Alt's release, Escape and so on).
pub fn should_dismiss(look: &Look, hooked: bool) -> bool {
    let outside = look.cursor.is_some_and(|c| panel::outside_click(look.was_down, look.down, c, look.rect, Rect::default()));
    outside || (!hooked && panel::lost_foreground(look.hwnd, look.foreground, look.been_front, look.since_open_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: isize = 10;
    const OTHER: isize = 20;

    fn look() -> Look {
        Look {
            hwnd: ME,
            foreground: ME,
            been_front: true,
            since_open_ms: 100,
            was_down: false,
            down: false,
            cursor: Some((500, 500)),
            rect: Rect::new(100, 100, 900, 700),
        }
    }

    #[test]
    fn a_switcher_in_front_with_the_mouse_still_stays() {
        assert!(!should_dismiss(&look(), false));
        assert!(!should_dismiss(&look(), true));
    }

    #[test]
    fn a_window_that_never_got_the_foreground_goes_after_the_grace() {
        // the foreground was refused: somebody else's window stays in front and the switcher never was
        let refused = Look { foreground: OTHER, been_front: false, ..look() };
        let early = Look { since_open_ms: panel::FOREGROUND_GRACE_MS - 1, ..refused };
        assert!(!should_dismiss(&early, false), "Windows hands the foreground over; the old window is in front for a moment");
        let late = Look { since_open_ms: panel::FOREGROUND_GRACE_MS, ..refused };
        assert!(should_dismiss(&late, false));
    }

    #[test]
    fn losing_the_foreground_after_having_it_closes_at_once() {
        let lost = Look { foreground: OTHER, been_front: true, since_open_ms: 50, ..look() };
        assert!(should_dismiss(&lost, false));
    }

    #[test]
    fn between_two_windows_nothing_is_in_front_and_nothing_closes() {
        let none = Look { foreground: 0, been_front: false, since_open_ms: 10_000, ..look() };
        assert!(!should_dismiss(&none, false));
    }

    #[test]
    fn a_click_outside_closes_in_both_modes_a_click_on_it_does_not() {
        let outside = Look { down: true, cursor: Some((50, 50)), ..look() };
        assert!(should_dismiss(&outside, false));
        assert!(should_dismiss(&outside, true), "also when the host's hook feeds the keys");
        let inside = Look { down: true, cursor: Some((500, 500)), ..look() };
        assert!(!should_dismiss(&inside, false));
        // a button that was already down at the last look is a drag that began earlier, not a click now
        let held = Look { was_down: true, down: true, cursor: Some((50, 50)), ..look() };
        assert!(!should_dismiss(&held, false));
        // and no pointer is no click
        assert!(!should_dismiss(&Look { cursor: None, down: true, ..look() }, false));
    }

    #[test]
    fn with_the_hooks_keys_the_foreground_is_not_the_switchers_business() {
        let elsewhere = Look { foreground: OTHER, been_front: false, since_open_ms: 60_000, ..look() };
        assert!(!should_dismiss(&elsewhere, true));
    }
}
