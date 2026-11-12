//! The rectangle of a cover window. Pure.
//!
//! THE ONE-PIXEL RULE (research note 4, 2.2). While Explorer is the shell, a window that covers a whole monitor looks like a
//! full-screen application to everything that asks "is something full screen here?": Explorer's taskbar hides itself when it
//! receives `ABN_FULLSCREENAPP`, other programs' full-screen tests (and our own dock's) hide their windows, and notifications and
//! overlays step aside. Cairo, another shell written for Windows, leaves its desktop window one pixel short for exactly this reason.
//! So the window is the monitor less ONE pixel row.
//!
//! WHICH EDGE: the BOTTOM one. The usual taskbar lives on the bottom edge, and so does the thin strip of an auto-hiding taskbar
//! that the pointer has to touch; both are topmost windows, so in the common layout the missing row is behind one of them and the
//! user cannot see it. A row missing at the top or on a side would show Explorer's wallpaper along a whole edge. (If the taskbar
//! is on the top or a side, the row shows as a one-pixel line of Explorer's desktop along the bottom of that monitor.)
//!
//! The canvas the view draws stays the whole monitor (the layout store's cells are in monitor pixels); the missing row is simply
//! not on the screen.

use crate::desktop::geom::Rect;

/// The edge that is left out. A constant so that the tests and the report say the same thing as the code.
pub const TRIMMED_EDGE: &str = "bottom";

/// How many pixel rows are left out.
pub const TRIMMED_ROWS: i32 = 1;

/// The cover window's rectangle (virtual-screen device pixels) for a monitor's full rectangle. A monitor too small to give a row
/// away (an empty one, or one row high) comes back unchanged: there is nothing to draw on a window with no height.
pub fn cover_rect(monitor: Rect) -> Rect {
    if monitor.height() <= TRIMMED_ROWS || monitor.width() <= 0 {
        return monitor;
    }
    Rect::new(monitor.left, monitor.top, monitor.right, monitor.bottom - TRIMMED_ROWS)
}

/// The outer rectangle a window must be set to after it was placed, or `None` when it is fine. `got`: what `GetWindowRect` says now;
/// `limit`: the cover rectangle. The one-pixel rule is about the OUTER rectangle (it is what the full-screen tests read), so a window
/// that came out larger than the cover rectangle on any side (an invisible resize frame that was not expected) is set to exactly it;
/// one that is smaller or equal is left alone (it cannot be taken for a full-screen window).
pub fn outer_correction(got: Rect, limit: Rect) -> Option<Rect> {
    let inside = got.left >= limit.left && got.top >= limit.top && got.right <= limit.right && got.bottom <= limit.bottom;
    if inside || limit.is_empty() { None } else { Some(limit) }
}

/// What a full-screen test asks: does the window reach every edge of the monitor?
pub fn covers_monitor(window: Rect, monitor: Rect) -> bool {
    !monitor.is_empty() && window.left <= monitor.left && window.top <= monitor.top && window.right >= monitor.right && window.bottom >= monitor.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: Rect = Rect::new(0, 0, 2560, 1440);
    /// The portrait monitor of the development machine: right of the primary one, its top above the primary's (negative origin).
    const PORTRAIT: Rect = Rect::new(2560, -618, 4000, 1942);

    #[test]
    fn the_window_is_the_monitor_less_its_bottom_row() {
        assert_eq!(cover_rect(PRIMARY), Rect::new(0, 0, 2560, 1439));
        assert_eq!(cover_rect(PORTRAIT), Rect::new(2560, -618, 4000, 1941));
        assert_eq!(TRIMMED_EDGE, "bottom");
    }

    #[test]
    fn so_no_full_screen_test_takes_it_for_a_full_screen_window() {
        for m in [PRIMARY, PORTRAIT, Rect::new(-1440, -560, 0, 2000)] {
            assert!(!covers_monitor(cover_rect(m), m), "{m:?}");
            // a window that was the whole monitor would be taken for one
            assert!(covers_monitor(m, m));
        }
    }

    #[test]
    fn it_stays_inside_its_own_monitor() {
        let w = cover_rect(PORTRAIT);
        assert_eq!((w.left, w.top, w.right), (PORTRAIT.left, PORTRAIT.top, PORTRAIT.right));
        assert!(w.intersect(&PRIMARY).is_empty(), "it does not reach into the monitor next to it");
        assert_eq!(w.width(), PORTRAIT.width());
        assert_eq!(w.height(), PORTRAIT.height() - TRIMMED_ROWS);
    }

    #[test]
    fn a_monitor_with_no_row_to_spare_is_left_alone() {
        assert_eq!(cover_rect(Rect::default()), Rect::default());
        assert_eq!(cover_rect(Rect::new(0, 0, 100, 1)), Rect::new(0, 0, 100, 1));
        assert_eq!(cover_rect(Rect::new(0, 0, 0, 100)), Rect::new(0, 0, 0, 100));
        assert!(!covers_monitor(Rect::new(0, 0, 10, 10), Rect::default()), "an empty monitor is covered by nothing");
    }

    #[test]
    fn a_placed_window_that_came_out_too_big_is_set_to_the_cover_rectangle() {
        let want = cover_rect(PRIMARY);
        assert_eq!(outer_correction(want, want), None, "as asked: nothing to do");
        // an invisible 8 px frame on the left, right and bottom, as the dock's pop-ups have
        let framed = Rect::new(want.left - 8, want.top, want.right + 8, want.bottom + 8);
        assert_eq!(outer_correction(framed, want), Some(want));
        // the bottom row alone would already make it a full-screen window
        assert_eq!(outer_correction(PRIMARY, want), Some(want));
        assert!(!covers_monitor(outer_correction(framed, want).unwrap(), PRIMARY));
        // smaller is fine (it cannot be taken for a full-screen window)
        assert_eq!(outer_correction(Rect::new(0, 0, 2559, 1438), want), None);
        assert_eq!(outer_correction(framed, Rect::default()), None, "no monitor, nothing to correct to");
    }

    #[test]
    fn a_window_a_few_pixels_bigger_than_the_monitor_still_counts_as_covering() {
        // borderless games are often a little bigger than the monitor: the test must catch those, and must not catch ours
        assert!(covers_monitor(Rect::new(-1, -1, 2561, 1441), PRIMARY));
        assert!(!covers_monitor(Rect::new(0, 0, 2560, 1439), PRIMARY));
    }
}
