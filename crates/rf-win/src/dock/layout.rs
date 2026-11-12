//! Where the three islands go. Pure arithmetic: a work area (device px) and a scale in, three rectangles out.
//!
//! All the design numbers are logical px and are scaled by the monitor's DPI only here, with one rounding rule (round half up per
//! number, never per sum), so the same inputs give the same rectangles on every machine and a test can pin them.
//!
//! THE `scale` OF EVERY FUNCTION HERE IS THE DOCK UNIT: device px per design number, which is the monitor's DPI scale times the dock's
//! size factor (`size::unit`). At the default size the factor is 1.0 and the unit is the DPI scale itself, so the numbers below are
//! the dock the way it always was; the size setting multiplies every length of the dock by passing a different unit, nothing else.

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

/// Room kept around every island's glass inside its window, logical px: the glass's anti aliased edge and the pixel the window region
/// keeps beyond it must lie inside the window, and nothing more. This is all a window has by default, because of what the region does
/// (see `hit`): the system shows nothing of a window outside its region, so room for a shadow would be paid for and never seen.
pub const EDGE_MARGIN: f32 = 4.0;
/// The middle island's room above its glass: an icon grown 1.55 times and standing 12 px above the island's bottom reaches 8 px over
/// the glass's top edge, and the region keeps a pixel beyond the icon.
pub const HEADROOM: f32 = 12.0;
/// Room around every glass when the dock's shadow is switched on (`shadow_on`): the shadow falls 4 px down and up to 22 px out, and a
/// little over. It also holds the middle island's headroom.
pub const SHADOW_MARGIN: f32 = 28.0;
/// What the middle island's window keeps beyond its side margin on each side, for the glass widening while icons grow, logical px.
/// `magnify::the_widest_the_glass_ever_gets_fits_the_reserve` keeps it honest.
pub const MIDDLE_RESERVE: f32 = 40.0;

/// Whether the dock draws its soft shadow (`RUSTYFILES_DOCK_SHADOW=1`; off otherwise). WHY AN EXPERIMENT AND NOT A SETTING: the
/// islands' windows have a window region so that the pointer passes through what is not the island's, and Windows' documentation
/// says the region is where the system permits drawing; the shadow lies outside the region by design (a shadow that took clicks
/// would be a dead band over the taskbar and over the windows above the dock). So the shadow is expected to be clipped away, and this
/// switch lets the first live run see whether it is. It decides the windows' margins (`placements`) and the glass's shadow.
pub fn shadow_on() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUSTYFILES_DOCK_SHADOW").is_ok_and(|v| v == "1"))
}

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

/// An island's window and the glass inside it, device px, screen coordinates. The window is bigger than the glass: the margin holds the
/// shadow and, for the middle island, what grows out of the glass and the room for it to widen. The margin is transparent and lets
/// the pointer through (see `hit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub window: Rect,
    /// The glass at rest: exactly the rectangle `islands` gave.
    pub glass: Rect,
}

impl Placement {
    /// Distances from the window's edges to the glass: left, top, right, bottom.
    pub fn margins(&self) -> [i32; 4] {
        [self.glass.left - self.window.left, self.glass.top - self.window.top, self.window.right - self.glass.right, self.window.bottom - self.glass.bottom]
    }

    /// The glass in the window's own coordinates: x, y, w, h.
    pub fn glass_in_window(&self) -> [f32; 4] {
        let [l, t, ..] = self.margins();
        [l as f32, t as f32, self.glass.width() as f32, self.glass.height() as f32]
    }
}

/// The three windows with their glass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placements {
    pub left: Placement,
    pub middle: Placement,
    pub right: Placement,
}

/// Grows `glass` by `margin` on every side but keeps it inside `monitor` (a margin that would stick out of the screen holds nothing
/// that could be seen, and a window must not reach into the neighbouring monitor).
fn inflate_within(glass: Rect, left: i32, right: i32, top: i32, bottom: i32, monitor: Rect) -> Rect {
    Rect {
        left: (glass.left - left).max(monitor.left.min(glass.left)),
        top: (glass.top - top).max(monitor.top.min(glass.top)),
        right: (glass.right + right).min(monitor.right.max(glass.right)),
        bottom: (glass.bottom + bottom).min(monitor.bottom.max(glass.bottom)),
    }
}

/// The windows for these glass rectangles: every glass keeps `EDGE_MARGIN` around it (`SHADOW_MARGIN` with `shadow`; less where the
/// screen ends), the middle one `HEADROOM` above it for the icons that grow out of it and `MIDDLE_RESERVE` more on the left and the
/// right. The glass stays exactly where `islands` put it, so the gaps, the bottom margin and the band the toolbar reserves are
/// untouched: only the windows are bigger.
pub fn placements(islands: &Islands, monitor: Rect, scale: f32, shadow: bool) -> Placements {
    let edge = px(if shadow { SHADOW_MARGIN } else { EDGE_MARGIN }, scale);
    let (head, reserve) = (px(if shadow { SHADOW_MARGIN } else { HEADROOM }, scale), px(MIDDLE_RESERVE, scale));
    let make = |glass: Rect, side: i32, top: i32| Placement { window: inflate_within(glass, side, side, top, edge, monitor), glass };
    Placements { left: make(islands.left, edge, edge), middle: make(islands.middle, edge + reserve, head), right: make(islands.right, edge, edge) }
}

/// How far the middle island's glass may widen on each side, device px: what its window reserves, and what the neighbouring islands
/// leave (their least gap is kept), never negative.
pub fn growth_room(islands: &Islands, scale: f32) -> i32 {
    let reserve = px(MIDDLE_RESERVE, scale);
    let gap = px(MIN_GAP, scale);
    let (to_left, to_right) = (islands.middle.left - (islands.left.right + gap), (islands.right.left - gap) - islands.middle.right);
    reserve.min(to_left).min(to_right).max(0)
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

    const MONITOR: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn the_glass_stays_exactly_where_it_was_and_only_the_windows_grow() {
        for shadow in [false, true] {
            for (work, scale) in [(Rect::new(0, 0, 1920, 1032), 1.0), (Rect::new(100, 50, 2980, 1630), 1.5), (Rect::new(0, 0, 2560, 1440), 1.0)] {
                let i = islands(work, scale, W);
                let monitor = Rect::new(work.left.min(0), work.top.min(0), work.right, work.bottom + 48);
                let p = placements(&i, monitor, scale, shadow);
                assert_eq!((p.left.glass, p.middle.glass, p.right.glass), (i.left, i.middle, i.right), "the visible glass is where it always was");
                for pl in [p.left, p.middle, p.right] {
                    assert!(pl.window.covers(&pl.glass));
                    assert_eq!(pl.glass_in_window()[2] as i32, pl.glass.width());
                    let [l, t, r, b] = pl.margins();
                    assert!(l >= 0 && t >= 0 && r >= 0 && b >= 0);
                    assert_eq!(pl.window.width(), l + pl.glass.width() + r);
                }
                // the bottom of every glass is still 10 px above the work area's bottom
                assert_eq!(p.middle.glass.bottom, work.bottom - px(MARGIN_BOTTOM, scale));
            }
        }
    }

    #[test]
    fn the_windows_keep_a_little_room_and_the_middle_one_the_headroom_and_the_widening() {
        let work = Rect::new(0, 0, 1920, 1032);
        let i = islands(work, 1.0, W);
        let p = placements(&i, MONITOR, 1.0, false);
        // the left island is 12 px from the screen's edge: its window stops at the edge, the other sides keep the edge margin
        assert_eq!(p.left.margins(), [4, 4, 4, 4]);
        assert_eq!(p.right.margins(), [4, 4, 4, 4]);
        // the middle: the edge margin and 40 for the widening on each side, 12 above (growing icons), 4 below
        assert_eq!(p.middle.margins(), [44, 12, 44, 4]);
        assert_eq!(p.middle.window, Rect::new(660 - 44, 1022 - 72 - 12, 1260 + 44, 1022 + 4));
        // at 150 % the margins scale like everything else
        let i15 = islands(Rect::new(0, 0, 2880, 1548), 1.5, W);
        let p15 = placements(&i15, Rect::new(0, 0, 2880, 1620), 1.5, false);
        assert_eq!(p15.middle.margins(), [66, 18, 66, 6]);
        // with the shadow switched on every side keeps room for it, the middle one the reserve on top
        let s = placements(&i, MONITOR, 1.0, true);
        assert_eq!(s.left.margins(), [12, 28, 28, 28], "12 px from the screen's edge: the window stops there");
        assert_eq!(s.right.margins(), [28, 28, 12, 28]);
        assert_eq!(s.middle.margins(), [68, 28, 68, 28]);
        // a window with a margin is much smaller than one that holds a shadow: what the default saves
        let area = |r: Rect| r.width() * r.height();
        assert!(area(p.left.window) * 2 < area(s.left.window), "{} vs {}", area(p.left.window), area(s.left.window));
    }

    #[test]
    fn a_margin_never_reaches_past_the_monitor() {
        // an auto-hiding taskbar: the work area is the monitor, the glass is 10 px above its bottom
        let monitor = Rect::new(0, 0, 2560, 1440);
        for shadow in [false, true] {
            let p = placements(&islands(monitor, 1.0, W), monitor, 1.0, shadow);
            for pl in [p.left, p.middle, p.right] {
                assert!(monitor.covers(&pl.window), "{pl:?}");
            }
            let want = if shadow { 10 } else { 4 };
            assert_eq!(p.middle.margins()[3], want.min(10), "the margin below is at most the 10 px the glass floats above the screen's edge");
        }
        // a monitor that is not at the origin
        let second = Rect::new(-1920, 0, 0, 1080);
        let q = placements(&islands(Rect::new(-1920, 0, 0, 1032), 1.0, W), second, 1.0, true);
        assert!(second.covers(&q.left.window) && second.covers(&q.middle.window) && second.covers(&q.right.window));
    }

    #[test]
    fn the_headroom_holds_the_biggest_icon_and_the_reserve_holds_the_widening() {
        // an icon at 1.55 stands 12 px (the baseline) above the island's bottom: how far does it reach over the glass's top edge?
        let over = 12.0 + ICON_REST * 1.55 - MIDDLE_HEIGHT;
        assert!(over > 0.0 && over + 1.0 <= HEADROOM, "{over} px over the glass (and a pixel of the region's slack), the headroom is {HEADROOM}");
        // the glass's edge and the region's slack fit the edge margin
        const { assert!(EDGE_MARGIN >= 2.0 && HEADROOM <= SHADOW_MARGIN) };
        // the shadow: 4 px down and 22 px of fall-off
        const { assert!(SHADOW_MARGIN >= 4.0 + 22.0) };
    }

    #[test]
    fn the_glass_widens_only_into_free_room() {
        let i = islands(Rect::new(0, 0, 1920, 1032), 1.0, W);
        assert_eq!(growth_room(&i, 1.0), 40, "plenty of room: what the window reserves");
        assert_eq!(growth_room(&islands(Rect::new(0, 0, 2880, 1548), 1.5, W), 1.5), 60, "the reserve scales with the monitor");
        // a squeezed middle island has no room to widen
        let tight = islands(Rect::new(0, 0, 1000, 700), 1.0, W);
        assert_eq!(growth_room(&tight, 1.0), 0);
        // a little room is a little growth
        let some = islands(Rect::new(0, 0, 1000, 700), 1.0, Widths { left: 330.0, middle: 280.0, right: 280.0 });
        let room = growth_room(&some, 1.0);
        assert_eq!(room, 10);
        assert!(some.middle.left - room >= some.left.right + 8 && some.middle.right + room <= some.right.left - 8, "never closer than the least gap");
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
