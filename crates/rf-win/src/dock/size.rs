//! The dock's size: ONE factor that scales the whole dock (heights, paddings, corner radius, icon cells and icons, the magnification, the
//! logo and the field, the right island's symbols and clock, the tray strip, the gaps between the islands and to the screen edge, the
//! band the dock reserves, and with them everything that opens above an island).
//!
//! HOW IT IS APPLIED. Every length in `layout`, `magnify`, `hover`, `tray_strip` and `rf-app`'s `plan` is a number of "dock units": the
//! logical px of the default size (Orta), the numbers the dock always had. A unit becomes device px by [`unit`], the monitor's DPI
//! scale times this factor, and that product is the ONLY thing the pure layout functions take where they took the monitor's scale
//! before (`layout::islands(area, unit, widths)`, `layout::placements`, `layout::growth_room`, `layout::reserved_thickness`,
//! `tray_strip::cell_rect`, ...). The default size has factor 1.0, so `unit(dpi, 1.0) == dpi` exactly and every number the dock had
//! stays what it was. The views (GPUI logical px) multiply the same unit lengths by the factor.
//!
//! WHAT DOES NOT SCALE: the panels' contents (the Start panel, the control centre, the calendar, the notification panel, the tray's
//! overflow panel, the app menu, the preview cards) are windows of their own and keep the monitor's DPI scale; they follow the dock
//! only by where they are anchored (the island's glass rectangle, which the layout gives).

/// The four sizes the setting offers. `Medium` is the dock as it always was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DockSize {
    Small,
    #[default]
    Medium,
    Large,
    XLarge,
}

impl DockSize {
    pub const ALL: [DockSize; 4] = [DockSize::Small, DockSize::Medium, DockSize::Large, DockSize::XLarge];

    /// The factor that multiplies every length of the dock.
    pub const fn factor(self) -> f32 {
        match self {
            DockSize::Small => 0.85,
            DockSize::Medium => 1.0,
            DockSize::Large => 1.2,
            DockSize::XLarge => 1.4,
        }
    }
}

/// The factor a caller may have been handed by something that is not a [`DockSize`] (a stale global, a test): anything that is not a
/// sensible positive number reads as the default, and the rest is kept between half and three times the default so a wrong value
/// cannot make the dock vanish or fill the screen.
pub fn sane(factor: f32) -> f32 {
    if factor.is_finite() && factor > 0.0 { factor.clamp(0.5, 3.0) } else { 1.0 }
}

/// Device px per dock unit: the monitor's DPI scale times the dock's size factor. A broken DPI scale reads as 1.
pub fn unit(dpi_scale: f32, factor: f32) -> f32 {
    let dpi = if dpi_scale.is_finite() && dpi_scale > 0.0 { dpi_scale } else { 1.0 };
    dpi * sane(factor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::hover::{Slot, slot_spans};
    use crate::dock::layout::{self, ICON_GAP, ICON_REST, MIDDLE_HEIGHT, MIN_GAP, Rect, SIDE_HEIGHT, Widths, px};
    use crate::dock::{magnify, tray_strip};

    const SIZES: [(DockSize, f32); 4] = [(DockSize::Small, 0.85), (DockSize::Medium, 1.0), (DockSize::Large, 1.2), (DockSize::XLarge, 1.4)];
    const W: Widths = Widths { left: 330.0, middle: 600.0, right: 280.0 };

    #[test]
    fn the_four_sizes_and_the_default() {
        for (size, factor) in SIZES {
            assert_eq!(size.factor(), factor);
        }
        assert_eq!(DockSize::default(), DockSize::Medium);
        assert_eq!(DockSize::ALL.map(DockSize::factor), [0.85, 1.0, 1.2, 1.4]);
    }

    #[test]
    fn the_default_size_leaves_the_monitors_scale_exactly_as_it_is() {
        for dpi in [1.0f32, 1.25, 1.5, 1.75, 2.0, 3.0] {
            assert_eq!(unit(dpi, DockSize::Medium.factor()), dpi);
        }
    }

    #[test]
    fn nonsense_reads_as_the_default() {
        assert_eq!(sane(f32::NAN), 1.0);
        assert_eq!(sane(0.0), 1.0);
        assert_eq!(sane(-2.0), 1.0);
        assert_eq!(sane(f32::INFINITY), 1.0);
        assert_eq!(sane(0.1), 0.5);
        assert_eq!(sane(9.0), 3.0);
        assert_eq!(unit(0.0, 1.2), 1.2);
        assert_eq!(unit(f32::NAN, 1.0), 1.0);
    }

    #[test]
    fn the_islands_are_as_high_as_their_size_says_at_every_monitor_scale() {
        for dpi in [1.0f32, 1.5, 2.0] {
            for (size, factor) in SIZES {
                let u = unit(dpi, factor);
                let i = layout::islands(Rect::new(0, 0, 6000, 1392), u, W);
                assert_eq!(i.middle.height(), px(MIDDLE_HEIGHT, u), "{size:?} at {dpi}");
                assert_eq!(i.left.height(), px(SIDE_HEIGHT, u));
                assert_eq!(i.right.height(), i.left.height());
                // the bottom edge floats the scaled margin above the work area, all three on one line
                let bottom = 1392 - px(layout::MARGIN_BOTTOM, u);
                assert_eq!((i.left.bottom, i.middle.bottom, i.right.bottom), (bottom, bottom, bottom));
                // the widths are the units times the unit
                assert_eq!((i.left.width(), i.right.width(), i.middle.width()), (px(330.0, u), px(280.0, u), px(600.0, u)));
            }
        }
        // the numbers of each size at 100 %
        let heights: Vec<(i32, i32)> = SIZES
            .iter()
            .map(|&(_, f)| {
                let i = layout::islands(Rect::new(0, 0, 1920, 1032), unit(1.0, f), W);
                (i.middle.height(), i.left.height())
            })
            .collect();
        assert_eq!(heights, vec![(61, 41), (72, 48), (86, 58), (101, 67)]);
    }

    #[test]
    fn the_default_size_is_the_dock_as_it_was() {
        let work = Rect::new(0, 0, 1920, 1032);
        let i = layout::islands(work, unit(1.0, 1.0), W);
        assert_eq!(i.left, Rect::new(12, 1032 - 4 - 48, 12 + 330, 1032 - 4));
        assert_eq!(i.middle, Rect::new(660, 1032 - 4 - 72, 1260, 1032 - 4));
        assert_eq!(layout::reserved_thickness(unit(1.0, 1.0)), 80);
        let p = layout::placements(&i, Rect::new(0, 0, 1920, 1080), unit(1.0, 1.0), false);
        assert_eq!(p.middle.margins(), [44, 12, 44, 4]);
        assert_eq!(layout::growth_room(&i, unit(1.0, 1.0)), 40);
    }

    #[test]
    fn the_cell_pitch_and_the_rest_centre_of_each_icon_follow_the_size() {
        let slots = [Slot::Icon, Slot::Icon, Slot::Separator, Slot::Icon, Slot::Icon, Slot::Icon];
        let sizes = [ICON_REST; 5];
        let spans = slot_spans(&slots, &sizes);
        for (size, factor) in SIZES {
            let u = unit(1.0, factor);
            let glass_left = 500;
            // the pitch between two neighbouring icons is the icon and the gap (54 units), times the unit
            let pitch = (spans[1].0 - spans[0].0) * u;
            assert!((pitch - (ICON_REST + ICON_GAP) * factor).abs() < 1e-3, "{size:?}: {pitch}");
            // the centre of icon i on the screen, device px, as the app works it out (`middle::screen_center_x`, which has its own test at
            // these units): written out here as numbers, for the first icon and the one after the separator
            let centres: Vec<f32> = spans.iter().map(|&(x, w)| glass_left as f32 + (x + w / 2.0) * u).collect();
            let (first, fourth, pitch_px) = match size {
                DockSize::Small => (528.9, 630.05, 45.9),
                DockSize::Medium => (534.0, 653.0, 54.0),
                DockSize::Large => (540.8, 683.6, 64.8),
                DockSize::XLarge => (547.6, 714.2, 75.6),
            };
            assert!((centres[0] - first).abs() < 0.01, "{size:?}: {}", centres[0]);
            assert!((centres[3] - fourth).abs() < 0.01, "{size:?}: {}", centres[3]);
            assert!((pitch - pitch_px).abs() < 0.01, "{size:?}: {pitch}");
            // the bell reaches the same number of slots (in units, so the same at every size)
            assert!((magnify::REACH - 2.5 * 54.0).abs() < 1e-3);
        }
    }

    #[test]
    fn the_growth_room_and_the_windows_follow_the_size() {
        for (size, factor) in SIZES {
            let u = unit(1.0, factor);
            let i = layout::islands(Rect::new(0, 0, 3000, 1032), u, W);
            // at 100 % the room the window keeps on each side of the glass and the window's margins (left, top, right, bottom), in device px,
            // as numbers: 40 units of room, 4 of edge, 12 of headroom
            let (room, margins) = match size {
                DockSize::Small => (34, [37, 10, 37, 3]),
                DockSize::Medium => (40, [44, 12, 44, 4]),
                DockSize::Large => (48, [53, 14, 53, 5]),
                DockSize::XLarge => (56, [62, 17, 62, 6]),
            };
            assert_eq!(layout::growth_room(&i, u), room, "{size:?}: plenty of room is what the window reserves");
            let p = layout::placements(&i, Rect::new(0, 0, 3000, 1080), u, false);
            assert_eq!(p.middle.margins(), margins, "{size:?}");
            // the biggest icon the magnification really makes still fits the headroom at this size: icons stand 12 units over the island's
            // bottom (`middle.rs` BASELINE), the island is 72 units high; the frame is the real one with the pointer on the middle icon
            let five = vec![Slot::Icon; 5];
            let rest5 = slot_spans(&five, &[ICON_REST; 5]);
            let w5 = magnify::natural_width(&five, &[ICON_REST; 5]);
            let f5 = magnify::frame(&five, &rest5, w5, rest5[2].0 + rest5[2].1 / 2.0, 1.0, magnify::Strength::Full, w5 + 1000.0);
            let biggest = f5.spans.iter().map(|s| s.1).fold(0.0f32, f32::max);
            assert!(biggest > ICON_REST * 1.5, "the bell really grows the middle icon: {biggest}");
            let over_px = (12.0 + biggest - MIDDLE_HEIGHT) * u;
            assert!(over_px > 0.0 && over_px <= px(layout::HEADROOM, u) as f32 + 1.0, "{size:?}: {over_px} px over the glass, the headroom is {}", margins[1]);
            // the widest the glass ever gets (many icons, the pointer anywhere) fits what the window reserves, at every size: the glass
            // is worked out in units and both numbers scale by the same factor
            let slots = vec![Slot::Icon; 30];
            let rest = slot_spans(&slots, &[ICON_REST; 30]);
            let rest_w = magnify::natural_width(&slots, &[ICON_REST; 30]);
            let mut worst: f32 = 0.0;
            let mut pointer = 0.0;
            while pointer < rest_w {
                let f = magnify::frame(&slots, &rest, rest_w, pointer, 1.0, magnify::Strength::Full, rest_w + 1000.0);
                worst = worst.max(f.width - rest_w);
                pointer += 3.1;
            }
            assert!(worst * u / 2.0 <= px(layout::MIDDLE_RESERVE, u) as f32 + 1.0, "{size:?}: each side needs {} device px, the window keeps {}", worst * u / 2.0, px(layout::MIDDLE_RESERVE, u));
            assert_eq!(layout::reserved_thickness(u), px(layout::RESERVED_HEIGHT, u));
        }
    }

    #[test]
    fn the_island_widths_for_a_count_of_icons() {
        for (_, factor) in SIZES {
            let u = unit(1.0, factor);
            for icons in [0usize, 1, 5, 12] {
                let units = layout::middle_width(&vec![ICON_REST; icons], 1);
                let i = layout::islands(Rect::new(0, 0, 3840, 2100), u, Widths { left: 330.0, middle: units, right: 280.0 });
                assert_eq!(i.middle.width(), px(units, u), "{icons} icons at {factor}");
            }
        }
        // 12 icons and a separator: the usual formula, in units
        assert_eq!(layout::middle_width(&[44.0; 12], 1), 24.0 + 528.0 + 1.0 + 12.0 * 10.0);
    }

    /// 1366 px wide is the narrowest screen the dock is planned for. At the smallest size nothing overlaps and the least gap holds, with a
    /// full set of icons and the tray strip's room.
    #[test]
    fn nothing_overlaps_on_a_1366_screen_at_any_size() {
        let work = Rect::new(0, 0, 1366, 720);
        let widest_right = 280.0 + tray_strip::MAX_STRIP_W.min(1366.0 * 0.25) + 2.0 * tray_strip::SEP_GAP + tray_strip::SEP_W;
        for (size, factor) in SIZES {
            let u = unit(1.0, factor);
            for middle in [layout::middle_width(&[44.0; 4], 1), layout::middle_width(&[44.0; 14], 1)] {
                for right in [280.0, widest_right] {
                    let i = layout::islands(work, u, Widths { left: 330.0 + 58.0, middle, right });
                    assert!(i.left.right + px(MIN_GAP, u) <= i.middle.left || i.middle.width() == 0, "{size:?}: left {i:?}");
                    assert!(i.middle.right + px(MIN_GAP, u) <= i.right.left || i.middle.width() == 0, "{size:?}: right {i:?}");
                    assert!(i.left.right <= i.right.left, "{size:?}: the side islands meet: {i:?}");
                    assert!(work.covers(&i.left) && work.covers(&i.right) && work.covers(&i.middle), "{size:?}: inside the screen");
                    // the glass never widens into a neighbour
                    let room = layout::growth_room(&i, u);
                    assert!(i.middle.left - room >= i.left.right + px(MIN_GAP, u) || room == 0, "{size:?}");
                    assert!(i.middle.right + room <= i.right.left - px(MIN_GAP, u) || room == 0, "{size:?}");
                }
            }
        }
        // the smallest size on the narrowest screen: even the widest right island and a long search field leave the middle one room
        let u = unit(1.0, DockSize::Small.factor());
        let i = layout::islands(work, u, Widths { left: 388.0, middle: layout::middle_width(&[44.0; 14], 1), right: widest_right });
        assert!(i.middle.width() > 0, "{i:?}");
    }

    #[test]
    fn the_tray_strip_cells_are_the_size_too() {
        let glass = Rect::new(1000, 900, 1300, 1000);
        for (size, factor) in SIZES {
            let u = unit(1.0, factor);
            let cell = tray_strip::cell_rect(glass, u, 2);
            assert_eq!(cell.width(), px(tray_strip::CELL, u), "{size:?}");
            assert_eq!(cell.left, glass.left + px(tray_strip::SIDE_PAD, u) + 2 * px(tray_strip::CELL, u));
            // centred in the glass's height
            assert_eq!(cell.top, glass.top + (glass.height() - cell.height()) / 2);
        }
    }
}
