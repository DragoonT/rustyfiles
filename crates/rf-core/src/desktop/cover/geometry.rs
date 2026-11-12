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

/// `SWP_NOSIZE` and `SWP_NOMOVE` of `SetWindowPos`, as they arrive in `WINDOWPOS::flags`.
pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;

/// What a window is about to be given by `SetWindowPos` (the `WINDOWPOS` of `WM_WINDOWPOSCHANGING`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    pub flags: u32,
}

/// A cover window refuses every rectangle but its own. Whoever asks (the toolkit after a scale change, the system maximizing the
/// window, a restored "maximized" placement, a work-area change) gets the cover rectangle: position and size are forced to `want` and
/// the "no move / no size" bits are cleared, so that even a call that only changes the z-order leaves the window where it belongs if
/// something had already pushed it away. The result is the position the system is told to apply. `None` for `want` (the window has
/// not been placed yet) leaves the request as it is.
pub fn pinned(asked: Pos, want: Option<Rect>) -> Pos {
    let Some(w) = want.filter(|w| !w.is_empty()) else { return asked };
    Pos { x: w.left, y: w.top, cx: w.width(), cy: w.height(), flags: asked.flags & !(SWP_NOSIZE | SWP_NOMOVE) }
}

/// Whether [`pinned`] changed anything of the request (what is logged: it names a program that tried to move the desktop).
pub fn was_overruled(asked: Pos, kept: Pos) -> bool {
    (asked.flags & SWP_NOMOVE == 0 && (asked.x, asked.y) != (kept.x, kept.y)) || (asked.flags & SWP_NOSIZE == 0 && (asked.cx, asked.cy) != (kept.cx, kept.cy))
}

/// Whether the CLIENT area of a placed cover window is the whole cover rectangle. The outer rectangle is pinned to it, so a client
/// area of another size can only come from a non-client frame (a sizing frame the toolkit put back after the style was cleared): the
/// content (wallpaper, icons) is laid out for `want` and would be cut off at the right and bottom, with the bare window behind.
pub fn client_is_whole(want: Rect, client_w: i32, client_h: i32) -> bool {
    (client_w, client_h) == (want.width(), want.height())
}

/// What `WM_GETMINMAXINFO` is answered with: the largest the window can become and the smallest and largest it can be dragged to are
/// all the cover rectangle's size, and "maximized" is the cover rectangle itself. Left alone, a maximize would make the window the
/// monitor's work area plus the sizing frame (the 2576 x 1447 window at -8,-4 that hid the dock).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinMax {
    pub max_size: (i32, i32),
    /// Relative to the monitor the window is on (the system adds the monitor's origin).
    pub max_pos: (i32, i32),
    pub min_track: (i32, i32),
    pub max_track: (i32, i32),
}

/// [`MinMax`] for the cover rectangle `want` of a window on the monitor `monitor`.
pub fn min_max(want: Rect, monitor: Rect) -> MinMax {
    let size = (want.width().max(1), want.height().max(1));
    MinMax { max_size: size, max_pos: (want.left - monitor.left, want.top - monitor.top), min_track: size, max_track: size }
}

/// Whether a system command (`WM_SYSCOMMAND`, `wParam & 0xFFF0`) changes the size or place of the window or minimizes it: a cover
/// window does none of that.
pub fn refused_command(cmd: usize) -> bool {
    matches!(cmd & 0xFFF0, SC_SIZE | SC_MOVE | SC_MINIMIZE | SC_MAXIMIZE)
}

const SC_SIZE: usize = 0xF000;
const SC_MOVE: usize = 0xF010;
const SC_MINIMIZE: usize = 0xF020;
const SC_MAXIMIZE: usize = 0xF030;

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

    #[test]
    fn a_cover_window_refuses_every_rectangle_but_its_own() {
        let want = cover_rect(PRIMARY);
        let at = |x, y, cx, cy, flags| Pos { x, y, cx, cy, flags };
        // the maximized frame of the trial: 2576 x 1447 at -8,-4
        let kept = pinned(at(-8, -4, 2576, 1447, 0), Some(want));
        assert_eq!((kept.x, kept.y, kept.cx, kept.cy), (0, 0, 2560, 1439));
        assert!(was_overruled(at(-8, -4, 2576, 1447, 0), kept));
        // a call that only moves the z-order keeps the rectangle too (and puts it right if it was pushed away)
        let z = pinned(at(0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE), Some(want));
        assert_eq!((z.x, z.y, z.cx, z.cy, z.flags), (0, 0, 2560, 1439, 0));
        assert!(!was_overruled(at(0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE), z), "a z-order call asked for no rectangle, so nothing was overruled");
        // the right rectangle passes unchanged and is not reported
        let same = pinned(at(0, 0, 2560, 1439, 0x10), Some(want));
        assert!(!was_overruled(at(0, 0, 2560, 1439, 0x10), same));
        assert_eq!(same.flags, 0x10, "other flags (no activate ...) stay");
        // other flags survive the clearing of the two
        assert_eq!(pinned(at(5, 5, 5, 5, 0x10 | SWP_NOSIZE), Some(want)).flags, 0x10);
        // not placed yet: the request is the toolkit's own
        assert_eq!(pinned(at(1, 2, 3, 4, 0), None), at(1, 2, 3, 4, 0));
        assert_eq!(pinned(at(1, 2, 3, 4, 0), Some(Rect::default())), at(1, 2, 3, 4, 0));
        // a monitor with a negative origin (the portrait one)
        let p = cover_rect(PORTRAIT);
        let k = pinned(at(0, 0, 1, 1, 0), Some(p));
        assert_eq!((k.x, k.y, k.cx, k.cy), (2560, -618, 1440, 2559));
    }

    #[test]
    fn a_client_area_short_of_the_cover_rectangle_is_noticed() {
        let want = cover_rect(PRIMARY);
        assert!(client_is_whole(want, 2560, 1439));
        // the toolkit's invisible 8 px sizing frame on an outer rectangle that is pinned: 2544 x 1431, the grey band of the trial
        assert!(!client_is_whole(want, 2544, 1431));
        assert!(!client_is_whole(want, 2560, 1431));
    }

    #[test]
    fn maximizing_gives_the_cover_rectangle_and_nothing_larger() {
        let want = cover_rect(PRIMARY);
        let m = min_max(want, PRIMARY);
        assert_eq!((m.max_size, m.max_pos, m.min_track, m.max_track), ((2560, 1439), (0, 0), (2560, 1439), (2560, 1439)));
        assert!(m.max_size.1 < PRIMARY.height(), "never the whole monitor: that is a full-screen window to the dock");
        // on the portrait monitor the position is relative to the monitor
        let m = min_max(cover_rect(PORTRAIT), PORTRAIT);
        assert_eq!((m.max_pos, m.max_size), ((0, 0), (1440, 2559)));
        // a window that is not yet sized gets a size of at least one pixel
        assert_eq!(min_max(Rect::default(), Rect::default()).max_size, (1, 1));
    }

    #[test]
    fn the_system_commands_that_would_resize_move_or_minimize_the_desktop_are_refused() {
        for (cmd, name) in [(0xF000, "size"), (0xF010, "move"), (0xF020, "minimize"), (0xF030, "maximize")] {
            assert!(refused_command(cmd), "{name}");
            assert!(refused_command(cmd | 0x2), "{name}: the low four bits carry the mouse side");
        }
        // restore, close, the menu and the task list are not ours to refuse here
        for cmd in [0xF120usize, 0xF060, 0xF100, 0xF130, 0xF140, 0xF170] {
            assert!(!refused_command(cmd), "{cmd:#x}");
        }
    }
}
