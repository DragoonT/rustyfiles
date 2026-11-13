//! The geometry of the screen snip, pure: pixels of the VIRTUAL SCREEN (physical, the origin is the primary monitor's top-left, so a
//! monitor left of or above it has negative coordinates), the region a drag makes, the monitor and the window under a point.
//!
//! Why physical pixels everywhere: the picture that ends up on the clipboard is a copy of the screen as it is drawn, so its size is in
//! device pixels whatever the scale of the monitor it came from. The overlay windows are drawn in the toolkit's logical pixels (physical
//! divided by the monitor's scale); [`to_screen`] is the one place that turns a pointer position of such a window back.

use rf_core::desktop::geom::Rect;

/// A drag shorter than this on both axes is a click. Physical pixels: a pointer that wobbles by a pixel or two while the button goes
/// down must not start a region.
pub const DRAG_MIN_PX: i32 = 4;

/// A point of the virtual screen, physical pixels.
pub type Pt = (i32, i32);

/// One monitor as the overlay needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    /// Its rectangle in the virtual screen, physical pixels.
    pub rect: Rect,
    /// Physical pixels per logical pixel of the toolkit (1.0 at 100 %, 1.5 at 150 %).
    pub scale: f32,
}

/// A top-level window as the window mode needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Win {
    pub hwnd: isize,
    /// What the user sees of the window (without the invisible resize border Windows 10 and 11 give it), virtual-screen pixels.
    pub rect: Rect,
}

/// A pointer position in the logical pixels of a window that lies on `m` (its top-left is `m`'s top-left), as a point of the virtual
/// screen. Also right for positions OUTSIDE that window: the toolkit keeps delivering the pointer to the window that was pressed
/// when the button is held, with negative coordinates or ones beyond its size, and a drag that runs into another monitor is measured
/// with the scale and the origin of the window it started in.
pub fn to_screen(m: &Monitor, local: (f32, f32)) -> Pt {
    let s = if m.scale.is_finite() && m.scale > 0.0 { m.scale } else { 1.0 };
    (m.rect.left + (local.0 * s).round() as i32, m.rect.top + (local.1 * s).round() as i32)
}

/// The other way: a rectangle of the virtual screen in the logical pixels of the window on `m`: (x, y, width, height). The parts of the
/// rectangle outside `m` are kept (they are simply drawn outside the window and clipped there).
pub fn to_local(m: &Monitor, r: Rect) -> (f32, f32, f32, f32) {
    let s = if m.scale.is_finite() && m.scale > 0.0 { m.scale } else { 1.0 };
    ((r.left - m.rect.left) as f32 / s, (r.top - m.rect.top) as f32 / s, r.width() as f32 / s, r.height() as f32 / s)
}

/// Whether going from `a` to `b` is a drag and not a click.
pub fn is_drag(a: Pt, b: Pt) -> bool {
    (b.0 - a.0).abs() >= DRAG_MIN_PX || (b.1 - a.1).abs() >= DRAG_MIN_PX
}

/// The rectangle between two corners, whichever way the drag went, cut to `limit` (the virtual screen). Right and bottom are exclusive:
/// a drag from (10, 10) to (110, 60) is 100 by 50 pixels, the size the overlay prints. `None` when nothing is left (a click, or a drag
/// that stayed outside `limit`).
pub fn region(a: Pt, b: Pt, limit: &Rect) -> Option<Rect> {
    let r = Rect::new(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)).intersect(limit);
    (!r.is_empty()).then_some(r)
}

/// The region a finished drag takes: [`region`] if it is a drag at all.
pub fn region_of_drag(a: Pt, b: Pt, limit: &Rect) -> Option<Rect> {
    if !is_drag(a, b) {
        return None;
    }
    region(a, b, limit)
}

/// The index of the monitor holding `p`.
pub fn monitor_at(p: Pt, monitors: &[Monitor]) -> Option<usize> {
    monitors.iter().position(|m| m.rect.contains(p.0, p.1))
}

/// Like [`monitor_at`] but never `None` for a non-empty list: a point in a gap between monitors of different heights (the virtual screen
/// is their bounding box, not their union) belongs to the nearest one.
pub fn nearest_monitor(p: Pt, monitors: &[Monitor]) -> Option<usize> {
    if let Some(i) = monitor_at(p, monitors) {
        return Some(i);
    }
    let dist = |r: &Rect| {
        let dx = (r.left - p.0).max(0).max(p.0 - (r.right - 1)) as i64;
        let dy = (r.top - p.1).max(0).max(p.1 - (r.bottom - 1)) as i64;
        dx * dx + dy * dy
    };
    monitors.iter().enumerate().min_by_key(|(_, m)| dist(&m.rect)).map(|(i, _)| i)
}

/// The bounding box of the monitors: the virtual screen.
pub fn virtual_screen(monitors: &[Monitor]) -> Rect {
    monitors.iter().fold(Rect::default(), |acc, m| acc.union(&m.rect))
}

/// The window under `p`. `windows` is in z order, TOPMOST FIRST (the order `EnumWindows` gives), so the first that holds the point is
/// the one the user sees there. Each window counts only where it is on the screen: `limit` (the virtual screen) cuts it first.
pub fn window_at(p: Pt, windows: &[Win], limit: &Rect) -> Option<usize> {
    windows.iter().position(|w| w.rect.intersect(limit).contains(p.0, p.1))
}

/// What a click in the window mode takes: the window's rectangle cut to the screen, `None` when nothing of it is on the screen.
pub fn window_region(w: &Win, limit: &Rect) -> Option<Rect> {
    let r = w.rect.intersect(limit);
    (!r.is_empty()).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1920x1080 primary at 100 % and a 1440x2560 portrait monitor at 150 % LEFT of it and higher (negative origin).
    fn two() -> Vec<Monitor> {
        vec![Monitor { rect: Rect::new(0, 0, 1920, 1080), scale: 1.0 }, Monitor { rect: Rect::new(-1440, -560, 0, 2000), scale: 1.5 }]
    }

    #[test]
    fn a_pointer_position_in_a_windows_logical_pixels_is_a_point_of_the_virtual_screen() {
        let m = two();
        assert_eq!(to_screen(&m[0], (100.0, 50.0)), (100, 50));
        // 150 %: logical 200,100 is physical 300,150 from the monitor's origin (-1440,-560)
        assert_eq!(to_screen(&m[1], (200.0, 100.0)), (-1140, -410));
        assert_eq!(to_screen(&m[1], (0.0, 0.0)), (-1440, -560));
        // half a logical pixel at 150 % is 0.75 device pixels: rounded
        assert_eq!(to_screen(&m[1], (0.5, 0.5)), (-1439, -559));
    }

    #[test]
    fn a_pointer_outside_the_window_it_was_pressed_in_is_still_a_right_point() {
        let m = two();
        // pressed on the portrait monitor, dragged right onto the primary: the portrait window's coordinates go beyond its width of 960
        // logical pixels (1440 / 1.5): logical 1000,200 is physical 1500,300 from its origin: (60, -260) on the virtual screen
        assert_eq!(to_screen(&m[1], (1000.0, 200.0)), (60, -260));
        // and the primary's window, pressed there, dragged left: negative coordinates
        assert_eq!(to_screen(&m[0], (-300.0, 10.0)), (-300, 10));
    }

    #[test]
    fn a_nonsense_scale_is_taken_as_one() {
        let bad = Monitor { rect: Rect::new(10, 20, 110, 120), scale: 0.0 };
        assert_eq!(to_screen(&bad, (5.0, 5.0)), (15, 25));
        let nan = Monitor { rect: Rect::new(10, 20, 110, 120), scale: f32::NAN };
        assert_eq!(to_screen(&nan, (5.0, 5.0)), (15, 25));
    }

    #[test]
    fn to_local_undoes_to_screen() {
        let m = two();
        for mon in &m {
            let p = to_screen(mon, (124.0, 46.0));
            let r = Rect::new(p.0, p.1, p.0 + (30.0 * mon.scale) as i32, p.1 + (20.0 * mon.scale) as i32);
            let (x, y, w, h) = to_local(mon, r);
            assert!((x - 124.0).abs() < 0.01 && (y - 46.0).abs() < 0.01 && (w - 30.0).abs() < 0.01 && (h - 20.0).abs() < 0.01, "{x} {y} {w} {h}");
        }
    }

    #[test]
    fn a_click_is_not_a_drag_and_a_short_wobble_neither() {
        assert!(!is_drag((50, 50), (50, 50)));
        assert!(!is_drag((50, 50), (53, 52)));
        assert!(!is_drag((50, 50), (47, 47)));
        assert!(is_drag((50, 50), (54, 50)), "four pixels on one axis is a drag");
        assert!(is_drag((50, 50), (50, 46)));
        let limit = Rect::new(0, 0, 100, 100);
        assert_eq!(region_of_drag((50, 50), (52, 51), &limit), None);
        assert_eq!(region_of_drag((50, 50), (50, 50), &limit), None);
    }

    #[test]
    fn a_drag_makes_the_rectangle_between_its_corners_whichever_way_it_went() {
        let limit = Rect::new(0, 0, 1920, 1080);
        let want = Some(Rect::new(10, 10, 110, 60));
        assert_eq!(region_of_drag((10, 10), (110, 60), &limit), want, "down and right");
        assert_eq!(region_of_drag((110, 60), (10, 10), &limit), want, "up and left");
        assert_eq!(region_of_drag((110, 10), (10, 60), &limit), want, "down and left");
        assert_eq!(region_of_drag((10, 60), (110, 10), &limit), want, "up and right");
        let r = want.unwrap();
        assert_eq!((r.width(), r.height()), (100, 50), "the size printed beside the rectangle");
    }

    #[test]
    fn a_drag_is_cut_to_the_virtual_screen_and_a_drag_wholly_outside_it_takes_nothing() {
        let limit = virtual_screen(&two());
        assert_eq!(limit, Rect::new(-1440, -560, 1920, 2000));
        assert_eq!(region_of_drag((1800, 900), (2500, 1500), &limit), Some(Rect::new(1800, 900, 1920, 1500)));
        assert_eq!(region_of_drag((-1600, -700), (-1300, -400), &limit), Some(Rect::new(-1440, -560, -1300, -400)));
        assert_eq!(region_of_drag((2000, 100), (2400, 300), &limit), None);
        // a drag whose cut is a sliver of one pixel is still a region: it is what the user dragged
        assert_eq!(region_of_drag((1919, 100), (2400, 300), &limit), Some(Rect::new(1919, 100, 1920, 300)));
    }

    #[test]
    fn a_drag_across_two_monitors_of_different_scales_is_measured_in_device_pixels() {
        let m = two();
        let limit = virtual_screen(&m);
        // pressed on the portrait monitor at logical (900, 300) (physical 1350,450 -> screen -90,-110) and released on the primary at
        // logical (400, 200) (screen 400,200)
        let a = to_screen(&m[1], (900.0, 300.0));
        let b = to_screen(&m[0], (400.0, 200.0));
        assert_eq!((a, b), ((-90, -110), (400, 200)));
        let r = region_of_drag(a, b, &limit).unwrap();
        assert_eq!(r, Rect::new(-90, -110, 400, 200));
        assert_eq!((r.width(), r.height()), (490, 310));
    }

    #[test]
    fn the_monitor_under_a_point_and_the_nearest_one_for_a_point_in_a_gap() {
        let m = two();
        assert_eq!(monitor_at((10, 10), &m), Some(0));
        assert_eq!(monitor_at((-10, 10), &m), Some(1));
        assert_eq!(monitor_at((-10, -600), &m), None, "above both");
        // the portrait monitor reaches to y = 2000, the primary only to 1080: below the primary, right of the portrait one, is a gap
        assert_eq!(monitor_at((500, 1500), &m), None);
        assert_eq!(nearest_monitor((500, 1500), &m), Some(0));
        assert_eq!(nearest_monitor((-10, -600), &m), Some(1));
        assert_eq!(nearest_monitor((-5000, 0), &m), Some(1));
        assert_eq!(nearest_monitor((5, 5), &[]), None);
        // the edges: left/top inclusive, right/bottom exclusive
        assert_eq!(monitor_at((1919, 1079), &m), Some(0));
        assert_eq!(monitor_at((1920, 1079), &m), None);
        assert_eq!(monitor_at((0, 0), &m), Some(0));
    }

    fn win(hwnd: isize, l: i32, t: i32, r: i32, b: i32) -> Win {
        Win { hwnd, rect: Rect::new(l, t, r, b) }
    }

    #[test]
    fn the_window_under_a_point_is_the_topmost_one_that_holds_it() {
        let limit = Rect::new(0, 0, 1920, 1080);
        // topmost first: a small dialog over a big editor over the desktop-sized window
        let ws = [win(1, 600, 300, 900, 500), win(2, 100, 100, 1500, 900), win(3, 0, 0, 1920, 1080)];
        assert_eq!(window_at((700, 400), &ws, &limit), Some(0));
        assert_eq!(window_at((200, 200), &ws, &limit), Some(1));
        assert_eq!(window_at((1700, 1000), &ws, &limit), Some(2));
        assert_eq!(window_at((2000, 1000), &ws, &limit), None, "off the screen");
        assert_eq!(window_at((5, 5), &[], &limit), None);
        // right and bottom are exclusive
        assert_eq!(window_at((900, 400), &ws, &limit), Some(1));
        assert_eq!(window_at((899, 499), &ws, &limit), Some(0));
    }

    #[test]
    fn a_window_dragged_half_off_the_screen_is_taken_as_far_as_it_is_on_it() {
        let limit = Rect::new(-1440, -560, 1920, 2000);
        let w = win(7, 1500, 900, 2300, 1400);
        assert_eq!(window_region(&w, &limit), Some(Rect::new(1500, 900, 1920, 1400)));
        assert_eq!(window_at((1800, 1000), &[w], &limit), Some(0));
        assert_eq!(window_at((2100, 1000), &[w], &limit), None, "the part beyond the screen is nobody's");
        assert_eq!(window_region(&win(8, 3000, 0, 3500, 100), &limit), None);
        // a window with a negative origin (on the monitor left of the primary) is whole
        let left = win(9, -1400, -500, -400, 500);
        assert_eq!(window_region(&left, &limit), Some(left.rect));
    }
}
