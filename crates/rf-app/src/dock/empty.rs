//! The boxes of the islands' controls, in dock units from the island's glass corner, and what a right click on the island's empty glass
//! does. The decision is `rf_win::dock::empty_spot`; this file knows where the controls of each island are (from the numbers the views
//! are built from, `plan.rs` and the tray strip's) so that a right click on the logo, the field, a symbol, the clock, the bell, a tray
//! icon or a dock icon is theirs and a right click on any other glass starts Windows' Task Manager (`actions::open_task_manager`).
//!
//! The weather and the thin separators are drawn but are not controls: a right click on them is a right click on the glass. The running
//! mark under an icon is part of the icon (the user sees it as the icon's): a right click on it is the icon's, see [`IconBox`].

use rf_win::dock::empty_spot::{self, Box4};
use rf_win::dock::layout::{RADIUS, SIDE_HEIGHT};
use rf_win::dock::tray_strip as ts;

use super::plan;

/// The left island's controls: the logo and the search field, centred in the island's height.
pub fn left_controls() -> Vec<Box4> {
    let y = |h: f32| (SIDE_HEIGHT - h) / 2.0;
    vec![[plan::SIDE_PAD, y(plan::LOGO), plan::LOGO, plan::LOGO], [plan::SIDE_PAD + plan::LOGO + plan::LOGO_GAP, y(plan::SEARCH_H), plan::SEARCH_W, plan::SEARCH_H]]
}

/// A secondary monitor's left island's controls: the logo and the search (a field, or a button as big as the logo).
pub fn left_controls_secondary(search: rf_win::dock::monitors::Search) -> Vec<Box4> {
    use rf_win::dock::monitors::Search;
    let y = |h: f32| (SIDE_HEIGHT - h) / 2.0;
    let (w, h) = match search {
        Search::Field => (plan::SEARCH_W, plan::SEARCH_H),
        Search::Button => (plan::SEARCH_BUTTON, plan::SEARCH_BUTTON),
    };
    vec![[plan::SIDE_PAD, y(plan::LOGO), plan::LOGO, plan::LOGO], [plan::SIDE_PAD + plan::LOGO + plan::LOGO_GAP, y(h), w, h]]
}

/// A secondary monitor's right island's controls: the clock and, when there is one, the bell, in the order and at the distances
/// `right.rs` builds them (padding, clock, gap, bell).
pub fn right_controls_secondary(clock_width: f32, bell: bool) -> Vec<Box4> {
    let y = |h: f32| (SIDE_HEIGHT - h) / 2.0;
    let mut out = vec![[plan::SIDE_PAD, y(plan::CLOCK_H), clock_width, plan::CLOCK_H]];
    if bell {
        out.push([plan::SIDE_PAD + clock_width + plan::RIGHT_GAP, y(plan::BUTTON), plan::BUTTON, plan::BUTTON]);
    }
    out
}

/// What the tray strip shows in the right island: how many icon cells and whether there is a chevron.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayShape {
    pub visible: usize,
    pub chevron: bool,
}

/// The right island's controls: the tray strip's cells and chevron (when there is a strip, which makes the island `tray_extra` units
/// wider), the network and volume buttons, the clock and the bell, in the order and at the distances `right.rs` builds them.
pub fn right_controls(clock_width: f32, tray: Option<TrayShape>, tray_extra: f32, sys: usize) -> Vec<Box4> {
    let y = |h: f32| (SIDE_HEIGHT - h) / 2.0;
    let mut out = Vec::new();
    let mut x = plan::SIDE_PAD;
    if let Some(t) = tray {
        for j in 0..t.visible {
            out.push([x + j as f32 * ts::CELL, y(ts::CELL), ts::CELL, ts::CELL]);
        }
        if t.chevron {
            out.push([x + t.visible as f32 * ts::CELL, y(ts::CELL), ts::CHEVRON_W, ts::CELL]);
        }
        x += tray_extra;
    }
    // the island's own buttons before the network symbol (safe removal for each removable drive, Windows Security): `right.rs`
    for i in 0..sys {
        out.push([x + i as f32 * (plan::BUTTON + plan::RIGHT_GAP), y(plan::BUTTON), plan::BUTTON, plan::BUTTON]);
    }
    x += sys as f32 * (plan::BUTTON + plan::RIGHT_GAP);
    let (network, volume) = (x, x + plan::BUTTON + plan::RIGHT_GAP);
    let clock = volume + plan::BUTTON + plan::RIGHT_SEP_GAP + rf_win::dock::layout::SEPARATOR_WIDTH + plan::RIGHT_SEP_GAP;
    let bell = clock + clock_width + plan::RIGHT_GAP;
    out.push([network, y(plan::BUTTON), plan::BUTTON, plan::BUTTON]);
    out.push([volume, y(plan::BUTTON), plan::BUTTON, plan::BUTTON]);
    out.push([clock, y(plan::CLOCK_H), clock_width, plan::CLOCK_H]);
    out.push([bell, y(plan::BUTTON), plan::BUTTON, plan::BUTTON]);
    out
}

/// A side island's empty glass at `point` (dock units from its glass's top left corner)? `width` is the island's glass width.
pub fn side_is_empty(width: f32, controls: &[Box4], point: (f32, f32)) -> bool {
    empty_spot::is_empty([0.0, 0.0, width, SIDE_HEIGHT], RADIUS, controls, point)
}

/// One icon of the middle island as a right click sees it: the icon's own square and, under it, the strip down to the running mark's
/// lower edge. The mark is a child of the icon in the view but drawn outside the icon's square, so the toolkit does not count a click on
/// it as a click on the icon; here it is the icon's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconBox {
    /// The index of the icon's item (the slot index; slots and items run in step).
    pub ix: usize,
    /// The icon's square.
    pub icon: Box4,
    /// The strip under the square down to the mark's lower edge, as wide as the square.
    pub mark: Box4,
}

impl IconBox {
    /// The icon and the strip under it as one box.
    pub fn whole(&self) -> Box4 {
        [self.icon[0], self.icon[1], self.icon[2], self.icon[3] + self.mark[3]]
    }
}

/// The middle island's empty glass at `point` (dock units in the window's own coordinates, like `plan::DockPicture`): on the glass as
/// it is drawn at this moment and on none of the icons or the marks under them (an icon grown out of the glass is a control over the
/// glass too).
pub fn middle_is_empty(glass: Box4, icons: &[IconBox], point: (f32, f32)) -> bool {
    let wholes: Vec<Box4> = icons.iter().map(IconBox::whole).collect();
    empty_spot::is_empty(glass, RADIUS, &wholes, point)
}

/// The icon whose running-mark strip `point` is on (not on its square, which has its own click): a right click there opens that icon's
/// menu, as one on the icon does.
pub fn mark_owner(icons: &[IconBox], point: (f32, f32)) -> Option<usize> {
    icons.iter().find(|b| empty_spot::contains(b.mark, point)).map(|b| b.ix)
}

/// The boxes of the icons of a picture of the middle island, window coordinates in dock units (the icons stand on one baseline;
/// `mark_drop` is how far the running mark's lower edge is below the icons' bottom edge). An iterator so that the view, which makes
/// them on every frame of the magnification, can reuse one list.
pub fn middle_icon_boxes<'a>(
    slots: &'a [rf_win::dock::hover::Slot],
    pic: &'a plan::DockPicture,
    margin_top: f32,
    baseline: f32,
    mark_drop: f32,
) -> impl Iterator<Item = IconBox> + 'a {
    let bottom = margin_top + rf_win::dock::layout::MIDDLE_HEIGHT - baseline;
    slots
        .iter()
        .zip(&pic.items)
        .enumerate()
        .filter(|(_, (s, _))| **s == rf_win::dock::hover::Slot::Icon)
        .map(move |(ix, (_, &(x, w)))| IconBox { ix, icon: [x, bottom - w, w, w], mark: [x, bottom, w, mark_drop] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_win::dock::clock::ClockLines;

    #[test]
    fn a_secondary_islands_controls_end_where_their_widths_do() {
        use rf_win::dock::monitors::Search;
        let lines = ClockLines { time: "14:05".into(), date: "12.11.2026".into() };
        let cw = plan::clock_width(&lines);
        for search in [Search::Field, Search::Button] {
            let c = left_controls_secondary(search);
            let last = c.last().unwrap();
            assert_eq!(last[0] + last[2] + plan::SIDE_PAD, plan::left_width_secondary(search), "{search:?}");
        }
        for bell in [true, false] {
            let c = right_controls_secondary(cw, bell);
            let last = c.last().unwrap();
            assert_eq!(last[0] + last[2] + if bell { plan::SIDE_PAD } else { plan::SIDE_PAD }, plan::right_width_secondary(cw, bell), "bell {bell}");
        }
        // the field's own box is the primary's, so a right click on it is the field's on every monitor
        assert_eq!(left_controls_secondary(Search::Field), left_controls());
        // the clock and the bell are controls; between them is empty glass
        let w = plan::right_width_secondary(cw, true);
        let c = right_controls_secondary(cw, true);
        assert!(!side_is_empty(w, &c, (plan::SIDE_PAD + 10.0, 24.0)));
        assert!(side_is_empty(w, &c, (plan::SIDE_PAD + cw + 1.0, 24.0)), "the gap between the clock and the bell");
        assert!(!side_is_empty(w, &c, (plan::SIDE_PAD + cw + plan::RIGHT_GAP + 10.0, 24.0)), "the bell");
    }

    #[test]
    fn the_left_island_has_two_controls_and_the_rest_is_empty_glass() {
        let w = plan::left_width(Some("12°"));
        let c = left_controls();
        assert_eq!(c.len(), 2);
        // on the logo, on the field's two ends
        for p in [(24.0, 24.0), (50.0, 24.0), (265.0, 24.0)] {
            assert!(!side_is_empty(w, &c, p), "{p:?}");
        }
        // between them, above the field, and the weather (shown, not a control)
        for p in [(44.0, 24.0), (150.0, 4.0), (150.0, 44.0), (w - 30.0, 24.0), (w - 5.0, 24.0)] {
            assert!(side_is_empty(w, &c, p), "{p:?}");
        }
    }

    #[test]
    fn the_controls_are_where_the_view_puts_them() {
        // the view: padding, the logo, the gap, the field (flex row, items centred in the 48 px island)
        let c = left_controls();
        assert_eq!(c[0], [8.0, 8.0, 32.0, 32.0]);
        assert_eq!(c[1], [48.0, 8.0, 220.0, 32.0]);
        // the field ends where the island's content does, without the weather
        assert_eq!(c[1][0] + c[1][2] + plan::SIDE_PAD, plan::left_width(None));
    }

    #[test]
    fn the_right_island_without_a_strip() {
        let lines = ClockLines { time: "14:05".into(), date: "12.11.2026".into() };
        let cw = plan::clock_width(&lines);
        let c = right_controls(cw, None, 0.0, 0);
        assert_eq!(c.len(), 4);
        let w = plan::right_width(&lines);
        // the bell ends where the content does
        let bell = c[3];
        assert_eq!(bell[0] + bell[2] + plan::SIDE_PAD, w);
        // a symbol, the clock, the bell are theirs; the separator between the volume symbol and the clock, and the padding, are glass
        assert!(!side_is_empty(w, &c, (8.0 + 16.0, 24.0)));
        assert!(!side_is_empty(w, &c, (c[2][0] + 3.0, 24.0)));
        assert!(!side_is_empty(w, &c, (bell[0] + 16.0, 24.0)));
        let sep = c[1][0] + c[1][2] + plan::RIGHT_SEP_GAP;
        assert!(side_is_empty(w, &c, (sep + 0.5, 24.0)), "the separator is not a control");
        assert!(side_is_empty(w, &c, (c[0][0] + c[0][2] + 1.0, 24.0)), "the gap between the two symbols");
        assert!(side_is_empty(w, &c, (c[2][0] + 3.0, 2.0)), "above the clock's block");
    }

    #[test]
    fn the_tray_strip_cells_and_the_chevron_are_controls_and_push_the_symbols_right() {
        let lines = ClockLines { time: "14:05".into(), date: "12.11.2026".into() };
        let cw = plan::clock_width(&lines);
        let shape = TrayShape { visible: 3, chevron: true };
        let extra = ts::Layout { visible: vec![0, 1, 2], overflow: vec![], chevron: true }.extra_width();
        let c = right_controls(cw, Some(shape), extra, 0);
        assert_eq!(c.len(), 3 + 1 + 4);
        let w = plan::right_width(&lines) + extra;
        for j in 0..3 {
            assert!(!side_is_empty(w, &c, (8.0 + j as f32 * ts::CELL + 12.0, 24.0)), "cell {j}");
        }
        assert!(!side_is_empty(w, &c, (8.0 + 3.0 * ts::CELL + 12.0, 24.0)), "the chevron");
        // the thin separator after the strip and its gaps are glass
        assert!(side_is_empty(w, &c, (8.0 + 3.0 * ts::CELL + ts::CHEVRON_W + 2.0, 24.0)));
        // the first symbol starts after the strip, its separator and gaps; the bell still ends where the content does
        assert_eq!(c[4][0], 8.0 + extra);
        let bell = c.last().unwrap();
        assert_eq!(bell[0] + bell[2] + plan::SIDE_PAD, w);
        // a strip with nothing in it but the chevron
        let only = right_controls(cw, Some(TrayShape { visible: 0, chevron: true }), ts::Layout { visible: vec![], overflow: vec![0], chevron: true }.extra_width(), 0);
        assert_eq!(only.len(), 1 + 4);
    }

    #[test]
    fn the_middle_island_is_empty_between_the_icons_and_not_on_them() {
        use rf_win::dock::hover::Slot;
        let slots = [Slot::Icon, Slot::Icon, Slot::Separator, Slot::Icon];
        let width = plan::middle_width_at_rest(3, 1);
        let rest = plan::rest_layout(&slots, width);
        let margins = [44.0, 12.0, 44.0, 4.0];
        let frame = rf_win::dock::magnify::Frame::at_rest(&rest, width);
        let pic = plan::picture(&frame, &slots, margins, 12.0);
        let boxes: Vec<IconBox> = middle_icon_boxes(&slots, &pic, margins[1], 12.0, 7.0).collect();
        assert_eq!(boxes.len(), 3);
        assert_eq!(boxes.iter().map(|b| b.ix).collect::<Vec<_>>(), vec![0, 1, 3], "the separator's slot is not an icon, the item indices run on");
        // the first icon: 44 px wide, standing 12 px above the island's bottom (the glass is 72 high, from y 12)
        assert_eq!(boxes[0].icon, [44.0 + 12.0, 12.0 + 72.0 - 12.0 - 44.0, 44.0, 44.0]);
        let glass = pic.glass;
        let centre_of = |b: Box4| (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
        for b in &boxes {
            assert!(!middle_is_empty(glass, &boxes, centre_of(b.icon)));
        }
        // the 10 px gap between the first two icons, the padding at the left, the separator
        let first = boxes[0].icon;
        let gap_x = first[0] + first[2] + 5.0;
        assert!(middle_is_empty(glass, &boxes, (gap_x, first[1] + 20.0)));
        assert!(middle_is_empty(glass, &boxes, (glass[0] + 8.0, glass[1] + 36.0)));
        // the running mark: 7 px under the icons' bottom edge (the glass ends 12 px under it). The strip down to the mark's lower edge is
        // the icon's: not empty glass, and a right click there is that icon's; under it, down to the glass's edge, is empty again
        let bottom = first[1] + first[3];
        for dy in [0.5, 3.0, 6.9] {
            let p = (first[0] + 20.0, bottom + dy);
            assert!(!middle_is_empty(glass, &boxes, p), "the mark under the icon is the icon's, +{dy}");
            assert_eq!(mark_owner(&boxes, p), Some(0));
        }
        assert!(middle_is_empty(glass, &boxes, (first[0] + 20.0, bottom + 7.5)), "under the mark");
        assert_eq!(mark_owner(&boxes, (first[0] + 20.0, bottom + 7.5)), None);
        assert_eq!(mark_owner(&boxes, centre_of(first)), None, "the icon's own square has its own click");
        assert_eq!(mark_owner(&boxes, (boxes[2].icon[0] + 1.0, bottom + 1.0)), Some(3), "the third icon is item 3");
        assert_eq!(mark_owner(&boxes, (gap_x, bottom + 1.0)), None, "between two marks");
        let sep = pic.items[2];
        assert!(middle_is_empty(glass, &boxes, (sep.0 + 0.5, glass[1] + 36.0)), "the separator");
        // the margin of the window above the glass is nobody's
        assert!(!middle_is_empty(glass, &boxes, (gap_x, 2.0)));
    }

    #[test]
    fn an_icon_grown_out_of_the_glass_keeps_its_box_while_the_pointer_is_on_it() {
        use rf_win::dock::hover::Slot;
        let slots = vec![Slot::Icon; 5];
        let width = plan::middle_width_at_rest(5, 0);
        let rest = plan::rest_layout(&slots, width);
        let centre = rest[2].0 + rest[2].1 / 2.0;
        let margins = [44.0, 12.0, 44.0, 4.0];
        let frame = rf_win::dock::magnify::frame(&slots, &rest, width, centre, 1.0, rf_win::dock::magnify::Strength::Full, width + 80.0);
        let pic = plan::picture(&frame, &slots, margins, 12.0);
        let boxes: Vec<IconBox> = middle_icon_boxes(&slots, &pic, margins[1], 12.0, 7.0).collect();
        // the biggest icon reaches over the glass's top edge, and its top is a control, not empty glass
        let big = boxes.iter().map(|b| b.icon).fold([0.0, 0.0, 0.0, 0.0], |a, b| if b[3] > a[3] { b } else { a });
        assert!(big[1] < pic.glass[1], "it stands out of the glass: {big:?}");
        assert!(!middle_is_empty(pic.glass, &boxes, (big[0] + big[2] / 2.0, big[1] + 1.0)));
    }

    /// The island's own buttons (safe removal, Windows Security) stand before the network symbol, each a button and a gap wide, and
    /// push everything after them: the row is exactly that much wider.
    #[test]
    fn the_islands_own_buttons_stand_before_the_network_symbol() {
        let cw = 64.0;
        let (plain, with) = (right_controls(cw, None, 0.0, 0), right_controls(cw, None, 0.0, 2));
        let step = plan::BUTTON + plan::RIGHT_GAP;
        assert_eq!(with.len(), plain.len() + 2);
        assert_eq!(with[0], [plan::SIDE_PAD, (SIDE_HEIGHT - plan::BUTTON) / 2.0, plan::BUTTON, plan::BUTTON]);
        assert_eq!(with[1][0], plan::SIDE_PAD + step);
        for (a, b) in plain.iter().zip(&with[2..]) {
            assert_eq!([a[0] + 2.0 * step, a[1], a[2], a[3]], *b);
        }
    }
}
