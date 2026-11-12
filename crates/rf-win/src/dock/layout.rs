//! Where the three islands go. Pure arithmetic: a work area (device px) and a scale in, three rectangles out.
//!
//! All the design numbers are logical px and are scaled by the monitor's DPI only here, with one rounding rule (round half up per
//! number, never per sum), so the same inputs give the same rectangles on every machine and a test can pin them.

/// A rectangle in device px, `right` and `bottom` exclusive (the same shape as Win32's `RECT`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect { left, top, right, bottom }
    }

    pub const fn from_xywh(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { left: x, top: y, right: x + w, bottom: y + h }
    }

    pub const fn width(&self) -> i32 {
        self.right - self.left
    }

    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }

    /// Whether `self` covers every pixel of `other` (an empty `other` is covered by anything that is not itself inverted).
    pub const fn covers(&self, other: &Rect) -> bool {
        self.left <= other.left && self.top <= other.top && self.right >= other.right && self.bottom >= other.bottom
    }

    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Distance of every island's bottom edge from the bottom of the work area, logical px.
pub const MARGIN_BOTTOM: f32 = 10.0;
/// Distance of the left and the right island from the work area's left and right edge, logical px.
pub const MARGIN_SIDE: f32 = 12.0;
/// The least gap kept between two neighbouring islands, logical px (the middle island gives way when the screen is too narrow).
pub const MIN_GAP: f32 = 8.0;
/// The middle island (the dock).
pub const MIDDLE_HEIGHT: f32 = 72.0;
/// The left and the right island.
pub const SIDE_HEIGHT: f32 = 48.0;
/// Corner radius of every island (a capsule for the 48 px ones).
pub const RADIUS: f32 = 24.0;
/// An icon at rest, the padding inside the middle island and the gap between two items of it, logical px.
pub const ICON_REST: f32 = 44.0;
pub const DOCK_PADDING: f32 = 12.0;
pub const ICON_GAP: f32 = 10.0;
/// Width of the thin separator in the middle island.
pub const SEPARATOR_WIDTH: f32 = 1.0;
/// What the application desktop toolbar reserves above the bottom of the work area: the middle island's height and a margin above
/// it as big as the one below it, so a maximised window does not touch the dock. Logical px.
pub const RESERVED_HEIGHT: f32 = MIDDLE_HEIGHT + 2.0 * MARGIN_BOTTOM;

/// Logical px to device px.
pub fn px(logical: f32, scale: f32) -> i32 {
    (logical * scale).round() as i32
}

/// The logical widths of the three islands' contents (what the app measured or computed): see [`middle_width`] for the middle one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widths {
    pub left: f32,
    pub middle: f32,
    pub right: f32,
}

/// The three islands' rectangles, device px, screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Islands {
    pub left: Rect,
    pub middle: Rect,
    pub right: Rect,
}

/// The rectangles for a work area. All three bottoms are `MARGIN_BOTTOM` above the work area's bottom; the left island sits
/// `MARGIN_SIDE` from the work area's left edge, the right one the same from the right edge, the middle one is centred on the work
/// area. When the three do not fit (a narrow screen, a long search field) the middle one is squeezed (its width shrinks and it is
/// shifted off the centre as far as needed) so that `MIN_GAP` stays between neighbours; the side ones are never squeezed. The
/// middle island's width is never negative.
pub fn islands(work: Rect, scale: f32, widths: Widths) -> Islands {
    let bottom = work.bottom - px(MARGIN_BOTTOM, scale);
    let (lw, mw, rw) = (px(widths.left, scale).max(0), px(widths.middle, scale).max(0), px(widths.right, scale).max(0));
    let (sh, mh) = (px(SIDE_HEIGHT, scale), px(MIDDLE_HEIGHT, scale));
    let side = px(MARGIN_SIDE, scale);
    let gap = px(MIN_GAP, scale);

    let left = Rect::from_xywh(work.left + side, bottom - sh, lw, sh);
    let right = Rect::from_xywh(work.right - side - rw, bottom - sh, rw, sh);

    let room_l = left.right + gap;
    let room_r = right.left - gap;
    let room = (room_r - room_l).max(0);
    let w = mw.min(room);
    let centred = work.left + (work.width() - w) / 2;
    let x = centred.clamp(room_l, (room_r - w).max(room_l));
    let middle = Rect::from_xywh(x, bottom - mh, w, mh);
    Islands { left, middle, right }
}

/// The width of the middle island for these item sizes (logical px, icons only, in order) and this many thin separators: the
/// padding on both sides, the items and a gap between every two neighbouring items (separators are items).
pub fn middle_width(icon_sizes: &[f32], separators: usize) -> f32 {
    let items = icon_sizes.len() + separators;
    if items == 0 {
        return 2.0 * DOCK_PADDING;
    }
    2.0 * DOCK_PADDING + icon_sizes.iter().sum::<f32>() + separators as f32 * SEPARATOR_WIDTH + (items - 1) as f32 * ICON_GAP
}

/// The band the application desktop toolbar asks for, device px thick.
pub fn reserved_thickness(scale: f32) -> i32 {
    px(RESERVED_HEIGHT, scale)
}

/// The area the islands are placed in: the primary monitor's work area with the band our own toolbar took out of it put back (the
/// islands float inside that band, so they must not be placed relative to the work area it left, or they would climb a band's
/// height up the screen and then again at every re-registration). `band` is the rectangle the toolbar reserved, if it did.
pub fn placement_area(work: Rect, band: Option<Rect>, monitor: Rect) -> Rect {
    match band {
        Some(b) if b.bottom > work.bottom && b.bottom <= monitor.bottom => Rect { bottom: b.bottom, ..work },
        _ => work,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Widths = Widths { left: 330.0, middle: 600.0, right: 280.0 };

    #[test]
    fn rounding_is_per_number() {
        assert_eq!(px(10.0, 1.25), 13); // 12.5 rounds up
        assert_eq!(px(12.0, 1.5), 18);
        assert_eq!(px(72.0, 1.0), 72);
        assert_eq!(px(0.0, 3.0), 0);
    }

    #[test]
    fn full_hd_at_100_percent() {
        // a 1920x1080 monitor with the Windows taskbar (48 px) visible: the work area ends at 1032
        let work = Rect::new(0, 0, 1920, 1032);
        let i = islands(work, 1.0, W);
        assert_eq!(i.left, Rect::new(12, 1032 - 10 - 48, 12 + 330, 1032 - 10));
        assert_eq!(i.right, Rect::new(1920 - 12 - 280, 1032 - 10 - 48, 1920 - 12, 1032 - 10));
        assert_eq!(i.middle, Rect::new(660, 1032 - 10 - 72, 1260, 1032 - 10));
        for r in [i.left, i.middle, i.right] {
            assert_eq!(r.bottom, 1022, "every bottom is 10 px above the work area's bottom");
        }
        assert_eq!((i.left.height(), i.middle.height(), i.right.height()), (48, 72, 48));
        // centred
        assert_eq!(i.middle.left + i.middle.right, work.left + work.right);
    }

    #[test]
    fn scales_with_the_dpi_and_follows_an_offset_work_area() {
        // a second work area origin (a monitor to the left of the primary can make others negative; here the work area starts at 100)
        let work = Rect::new(100, 50, 100 + 2880, 50 + 1580);
        let i = islands(work, 1.5, W);
        assert_eq!(i.left.left, 100 + 18);
        assert_eq!(i.left.height(), 72);
        assert_eq!(i.middle.height(), 108);
        assert_eq!(i.right.right, 100 + 2880 - 18);
        assert_eq!(i.middle.bottom, 50 + 1580 - 15);
        assert_eq!(i.left.width(), 495);
        assert_eq!(i.middle.width(), 900);
    }

    #[test]
    fn auto_hide_taskbar_means_the_work_area_is_the_monitor() {
        let i = islands(Rect::new(0, 0, 2560, 1440), 1.0, W);
        assert_eq!(i.middle.bottom, 1430);
    }

    #[test]
    fn a_narrow_screen_squeezes_only_the_middle() {
        let work = Rect::new(0, 0, 1000, 700);
        let i = islands(work, 1.0, W);
        // 12 + 330 + 8 .. 1000 - 12 - 280 - 8 = 350 .. 700: 350 px of room
        assert_eq!(i.left.width(), 330);
        assert_eq!(i.right.width(), 280);
        assert_eq!(i.middle.left, 350);
        assert_eq!(i.middle.right, 700);
        assert!(i.middle.left >= i.left.right + 8 && i.right.left >= i.middle.right + 8);
    }

    #[test]
    fn a_middle_that_cannot_be_centred_is_shifted_not_overlapped() {
        // the left island is much wider than the right one: the centred middle would run into it
        let work = Rect::new(0, 0, 1400, 800);
        let i = islands(work, 1.0, Widths { left: 700.0, middle: 500.0, right: 100.0 });
        assert!(i.middle.left >= i.left.right + 8, "{i:?}");
        assert!(i.middle.right <= i.right.left - 8, "{i:?}");
        assert_eq!(i.middle.width(), 500);
        // and nothing fits at all: the middle shrinks to nothing rather than going negative
        let tiny = islands(Rect::new(0, 0, 500, 300), 1.0, Widths { left: 400.0, middle: 500.0, right: 400.0 });
        assert_eq!(tiny.middle.width(), 0);
    }

    #[test]
    fn middle_width_counts_padding_items_and_gaps() {
        assert_eq!(middle_width(&[], 0), 24.0);
        assert_eq!(middle_width(&[44.0], 0), 24.0 + 44.0);
        // three icons, one separator, two more icons: 6 items, 5 gaps
        let sizes = [44.0; 5];
        assert_eq!(middle_width(&sizes, 1), 24.0 + 220.0 + 1.0 + 50.0);
    }

    #[test]
    fn the_band_is_put_back_into_the_placement_area() {
        let monitor = Rect::new(0, 0, 1920, 1080);
        let band = Rect::new(0, 940, 1920, 1032); // we took 92 px above a 48 px taskbar
        let work_after = Rect::new(0, 0, 1920, 940);
        let a = placement_area(work_after, Some(band), monitor);
        assert_eq!(a.bottom, 1032);
        assert_eq!(placement_area(Rect::new(0, 0, 1920, 1032), None, monitor).bottom, 1032);
        // a band that does not belong to this work area (stale, below the monitor) is ignored
        assert_eq!(placement_area(work_after, Some(Rect::new(0, 990, 1920, 1200)), monitor), work_after);
        // the work area has not shrunk yet (registered but not applied): nothing to put back
        assert_eq!(placement_area(Rect::new(0, 0, 1920, 1032), Some(band), monitor).bottom, 1032);
    }

    #[test]
    fn reserved_band_is_the_dock_plus_its_margins() {
        assert_eq!(reserved_thickness(1.0), 92);
        assert_eq!(reserved_thickness(2.0), 184);
    }

    #[test]
    fn rect_helpers() {
        let r = Rect::from_xywh(10, 20, 30, 40);
        assert_eq!((r.width(), r.height()), (30, 40));
        assert!(r.contains(10, 20) && !r.contains(40, 20) && !r.contains(10, 60));
        assert!(Rect::new(0, 0, 100, 100).covers(&r));
        assert!(!r.covers(&Rect::new(0, 0, 100, 100)));
    }
}
