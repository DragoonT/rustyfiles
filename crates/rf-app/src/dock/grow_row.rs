//! The ONE bottom row of the grown Start panel: the numbers, pure and tested. `grow_host.rs` hands them to the panel's view, `start_row.rs`
//! draws them.
//!
//! WHY. When the left island has grown into the Start panel its glass already has a bottom row: the island's own, with the logo in it
//! where it always was. The panel used to bring a bottom row of its own as well (the account's picture and name, the gear, the power
//! button, a line above them), so the glass showed two rows with the lower one holding nothing but the logo. Now the panel's row is not
//! drawn and its three things stand in the island's row instead:
//!
//! * the line sits directly above the island's row (the row's top edge), across the whole glass;
//! * the account's picture and name start where the island's search field started, right after the logo with the island's own gap
//!   (the field has travelled to the top of the panel);
//! * the gear and the power button end at the right edge with the island's own inner margin;
//! * all of it on the logo's centre line, in sizes that suit the island's row height, every length a dock unit times the dock's size
//!   exactly as the island's own lengths are (`plan.rs`).
//!
//! The 8 px gap between the panel's content and the island stays inside the glass, above the line, as a little room under the content.
//! All lengths here are logical px of the panel's window (its origin is the grown glass's top left corner).

use rf_win::dock::layout::Rect;

use super::plan;

/// The account's picture, in dock units (the size the panel's own row drew it at).
pub const AVATAR: f32 = 28.0;
/// The gear's and the power button's picture, and the account's name, in dock units.
pub const ICON: f32 = 16.0;
pub const NAME_TEXT: f32 = 14.0;
/// The corner radius of a button's plate (the logo's plate has the same one).
pub const BUTTON_RADIUS: f32 = 8.0;
/// Between the power menu's bottom edge and the button's top edge.
pub const MENU_GAP: f32 = 6.0;

/// Where the things of the row stand, logical px in the panel's window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BottomRow {
    /// The window's width.
    pub width: f32,
    /// The row's top edge, where the line is, and the row's height (the island's); the row ends at the window's bottom edge.
    pub top: f32,
    pub height: f32,
    /// The row's middle: the logo's centre line, which everything in the row is centred on.
    pub centre: f32,
    /// Where the account's picture starts (where the island's search field started).
    pub account_left: f32,
    pub avatar: f32,
    /// Between the picture and the name, and the name's text size.
    pub name_gap: f32,
    pub name_text: f32,
    /// A button (square), its picture, the plate's radius and the space between the two buttons.
    pub button: f32,
    pub icon: f32,
    pub button_radius: f32,
    pub buttons_gap: f32,
    /// Between the power button and the glass's right edge.
    pub right_margin: f32,
    /// Between the power menu and the button.
    pub menu_gap: f32,
}

/// The row of the glass `grown` that has grown out of `island` (both device px, screen coordinates). `unit` is device px per dock unit
/// (`host::dock_unit`), `scale` the monitor's: the window's logical px are device px over `scale`, the island's lengths are dock units
/// times `unit`.
pub fn bottom_row(island: Rect, grown: Rect, unit: f32, scale: f32) -> BottomRow {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let k = unit / s;
    let top = (island.top - grown.top) as f32 / s;
    let height = island.height() as f32 / s;
    BottomRow {
        width: grown.width() as f32 / s,
        top,
        height,
        centre: top + height / 2.0,
        account_left: (island.left - grown.left) as f32 / s + (plan::SIDE_PAD + plan::LOGO + plan::LOGO_GAP) * k,
        avatar: AVATAR * k,
        name_gap: plan::LOGO_GAP * k,
        name_text: NAME_TEXT * k,
        button: plan::BUTTON * k,
        icon: ICON * k,
        button_radius: BUTTON_RADIUS * k,
        buttons_gap: plan::RIGHT_GAP * k,
        right_margin: plan::SIDE_PAD * k,
        menu_gap: MENU_GAP * k,
    }
}

impl BottomRow {
    /// Where the gear's left edge is.
    pub fn buttons_left(&self) -> f32 {
        self.width - self.right_margin - 2.0 * self.button - self.buttons_gap
    }

    /// How far above the window's bottom edge the power menu's bottom edge is: just over the power button.
    pub fn power_menu_bottom(&self) -> f32 {
        self.height / 2.0 + self.button / 2.0 + self.menu_gap
    }
}

/// Whether the gear and the power button lie clear of the island's own window, which is raised above the panel's and takes the pointer
/// over its whole rectangle: a button under it could not be clicked. True on every screen and dock size the panel can grow on; a very
/// wide island (a long weather text at the largest dock size) says no, and the panel then opens the ordinary way with its own bottom row.
pub fn buttons_clear_of_island(island: Rect, grown: Rect, unit: f32, scale: f32) -> bool {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let row = bottom_row(island, grown, unit, scale);
    // (the island's right edge in the window's logical px)
    let island_right = (island.right - grown.left) as f32 / s;
    row.buttons_left() >= island_right
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_win::dock::grow::{Corner, Spec};
    use rf_win::dock::layout::{self, Widths};
    use rf_win::dock::panel::{self, Side};
    use rf_win::dock::size::{self, DockSize};
    use rf_win::dock::{grow, startpage};

    /// The island and the grown glass of the Start panel for a 2560 x 1440 monitor at this scale and dock size, built the way the dock
    /// builds them (the panel gives up its own bottom row, as `panel.rs` places it).
    fn setup(dpi: f32, factor: f32, left_width: f32) -> (Rect, Rect, f32) {
        let work = Rect::new(0, 0, 2560, 1440 - (48.0 * dpi).round() as i32);
        let unit = size::unit(dpi, factor);
        let isl = layout::islands(work, unit, Widths { left: left_width, middle: 600.0, right: 280.0 });
        let h = grow::height_without_own_row(startpage::PANEL_MAX_H);
        let panel_rect = panel::place(isl.left, (startpage::PANEL_W, h), dpi, Side::Left, work);
        let spec = Spec::new(isl.left, panel_rect, Corner::BottomLeft, 24.0 * unit, 8.0 * dpi);
        (isl.left, spec.grown, unit)
    }

    const DPIS: [f32; 3] = [1.0, 1.25, 1.5];

    #[test]
    fn on_the_users_screen_at_100_percent_the_row_is_the_islands_row() {
        // the left glass (12, 1334, 342, 1382) and the grown glass (12, 671, 654, 1382): 711 high, the island's 48 at the bottom
        let (island, grown, unit) = setup(1.0, 1.0, 330.0);
        assert_eq!(island, Rect::new(12, 1334, 342, 1382));
        assert_eq!(grown, Rect::new(12, 671, 654, 1382));
        let r = bottom_row(island, grown, unit, 1.0);
        assert_eq!((r.width, r.top, r.height, r.centre), (642.0, 663.0, 48.0, 687.0));
        assert_eq!(r.top + r.height, 711.0, "the row ends at the window's bottom edge: there is no second row under it");
        // right after the logo with the island's own gap: padding 8 + logo 32 + gap 8
        assert_eq!(r.account_left, 48.0);
        assert_eq!((r.avatar, r.name_gap, r.name_text), (28.0, 8.0, 14.0));
        // the island's own inner margin at the right end
        assert_eq!((r.button, r.icon, r.buttons_gap, r.right_margin), (32.0, 16.0, 4.0, 8.0));
        assert_eq!(r.buttons_left(), 642.0 - 8.0 - 64.0 - 4.0);
        assert!(buttons_clear_of_island(island, grown, unit, 1.0), "the gear starts at 566, the island ends at 330");
        assert_eq!(r.power_menu_bottom(), 24.0 + 16.0 + 6.0);
    }

    #[test]
    fn everything_stands_on_the_logos_centre_line_and_the_line_is_the_rows_top_edge() {
        for dpi in DPIS {
            for f in DockSize::ALL.map(DockSize::factor) {
                let (island, grown, unit) = setup(dpi, f, 330.0);
                let r = bottom_row(island, grown, unit, dpi);
                // the logo is centred in the island's row (left.rs: items_center): its centre line in screen px
                let logo_centre = island.top as f32 + island.height() as f32 / 2.0;
                assert!((r.centre * dpi + grown.top as f32 - logo_centre).abs() < 1e-3, "{dpi} {f}: {r:?}");
                // the line is directly above the island's row
                assert!((r.top * dpi + grown.top as f32 - island.top as f32).abs() < 1e-3, "{dpi} {f}");
                assert!(((r.top + r.height) * dpi - grown.height() as f32).abs() < 1e-3, "the row ends where the glass does");
                // everything fits the row, with room above and below
                assert!(r.button < r.height && r.avatar < r.height && r.name_text < r.height, "{dpi} {f}");
                assert!(r.avatar <= r.button + 1e-3 && r.icon < r.button);
            }
        }
    }

    /// The test above takes the logo's centre from the island's rectangle. The logo is drawn by the island's VIEW: `host::Frame` insets
    /// it by the window's margins (so it sees a box exactly as big as the glass) and `left.rs` centres the logo in that box. Here the box
    /// is taken from the real window and margins that `layout::placements` gives, with and without the shadow's margin, so the row's
    /// centre is compared with where the view's own layout puts the logo, not with the arithmetic of the row itself. (What this cannot
    /// see is the toolkit's rounding of a centred box to whole pixels: at most a pixel, and a thing for a screen look.)
    #[test]
    fn the_row_centre_is_where_the_islands_view_centres_the_logo() {
        let monitor = Rect::new(0, 0, 2560, 1440);
        for dpi in DPIS {
            for f in DockSize::ALL.map(DockSize::factor) {
                for shadow in [true, false] {
                    let (island, grown, unit) = setup(dpi, f, 330.0);
                    let work = Rect::new(0, 0, 2560, 1440 - (48.0 * dpi).round() as i32);
                    let isl = layout::islands(work, unit, Widths { left: 330.0, middle: 600.0, right: 280.0 });
                    let pl = layout::placements(&isl, monitor, unit, shadow).left; // (as the host does: `geo_for`, with the dock unit)
                    assert_eq!(pl.glass, island);
                    // the view's box: the window less the margins (device px, screen coordinates)
                    let [_, mt, _, mb] = pl.margins();
                    let (box_top, box_h) = (pl.window.top + mt, pl.window.height() - mt - mb);
                    let logo_centre = box_top as f32 + box_h as f32 / 2.0;
                    let r = bottom_row(island, grown, unit, dpi);
                    assert!((r.centre * dpi + grown.top as f32 - logo_centre).abs() < 1e-3, "{dpi} {f} shadow {shadow}: {r:?} {pl:?}");
                }
            }
        }
    }

    #[test]
    fn the_account_starts_where_the_islands_search_field_started_and_the_buttons_end_with_the_islands_margin() {
        for dpi in DPIS {
            for f in DockSize::ALL.map(DockSize::factor) {
                let (island, grown, unit) = setup(dpi, f, 330.0);
                let r = bottom_row(island, grown, unit, dpi);
                // the field's left edge in the island (left.rs / grow_host::field_places): padding, logo, gap, in dock units
                let field_left = island.left as f32 + (plan::SIDE_PAD + plan::LOGO + plan::LOGO_GAP) * unit;
                assert!((r.account_left * dpi + grown.left as f32 - field_left).abs() < 1e-3, "{dpi} {f}");
                // the power button's right edge is one island padding from the glass's right edge
                let power_right = r.width - r.right_margin;
                assert!(((r.width - power_right) * dpi - plan::SIDE_PAD * unit).abs() < 1e-3);
                // the gear lies left of the power button with the island's button gap, and clear of the island
                assert!((r.buttons_left() + r.button + r.buttons_gap + r.button - power_right).abs() < 1e-3);
                assert!(buttons_clear_of_island(island, grown, unit, dpi), "{dpi} {f}");
            }
        }
    }

    #[test]
    fn every_length_scales_with_the_dock_size_like_the_islands_own() {
        let small = bottom_row(Rect::new(12, 1334, 342, 1382), Rect::new(12, 671, 654, 1382), 1.0, 1.0);
        // the same screen with the dock 1.2 times as big: the island is 57.6 high, lengths follow
        let (island, grown, unit) = setup(1.0, 1.2, 330.0);
        let big = bottom_row(island, grown, unit, 1.0);
        for (a, b) in [(small.avatar, big.avatar), (small.button, big.button), (small.icon, big.icon), (small.name_text, big.name_text), (small.right_margin, big.right_margin), (small.buttons_gap, big.buttons_gap), (small.name_gap, big.name_gap)] {
            assert!((b - a * 1.2).abs() < 1e-3, "{a} -> {b}");
        }
        assert!((big.height - 57.6).abs() < 0.6, "the island's row: {}", big.height);
        // and the monitor's scale does not change the logical lengths of a dock of the same size
        let (island, grown, unit) = setup(1.5, 1.0, 330.0);
        let at150 = bottom_row(island, grown, unit, 1.5);
        assert!((at150.avatar - small.avatar).abs() < 1e-3 && (at150.button - small.button).abs() < 1e-3);
    }

    #[test]
    fn a_very_wide_island_leaves_the_panel_its_own_bottom_row() {
        // the island's window lies over the gear: it could not be clicked, so the panel does not grow there (grow_host::fits)
        let (island, grown, unit) = setup(1.0, 1.0, 330.0);
        let wide = Rect::new(island.left, island.top, island.left + 600, island.bottom);
        assert!(!buttons_clear_of_island(wide, grown, unit, 1.0));
        // the largest dock with a long weather text
        let (island, grown, unit) = setup(1.0, 1.4, 420.0);
        assert!(!buttons_clear_of_island(island, grown, unit, 1.0), "{island:?} {grown:?}");
        // the largest dock with the usual width
        let (island, grown, unit) = setup(1.0, 1.4, 330.0);
        assert!(buttons_clear_of_island(island, grown, unit, 1.0));
    }

    #[test]
    fn a_broken_scale_reads_as_100_percent() {
        let a = bottom_row(Rect::new(12, 1334, 342, 1382), Rect::new(12, 671, 654, 1382), 1.0, 0.0);
        let b = bottom_row(Rect::new(12, 1334, 342, 1382), Rect::new(12, 671, 654, 1382), 1.0, 1.0);
        assert_eq!(a, b);
    }
}
