//! Where everything stands in the GROWN Start panel: the numbers, pure and tested. `grow_host.rs` hands them to the panel's view,
//! `start_row.rs` draws the header and `start_home.rs` the field.
//!
//! WHY. When the left island's glass has grown into the Start panel its bottom row is the island's own row, left exactly as it is with
//! the panel closed: the logo, the search field in its place and the weather. The field in that row is the PANEL's search field (it has
//! the keyboard; the island's window is never activated), drawn where the island's own field stands, the same size, the same look.
//! What used to stand in the field's place at the top of the panel moved into a HEADER there: the account's picture and name at the left,
//! the settings gear and the power button at the right, a hairline under it. The content (pinned, recommended, all apps, results) sits
//! between the header and the island's row.
//!
//! TOP TO BOTTOM, in logical px of the panel's window (its origin is the grown glass's top left corner):
//!
//! * the header, `HEAD_H` = 60 px high: exactly the room the panel's field used to take (24 px of padding and the 36 px pill), so the
//!   content below it has the height it always had and the page layout (`startpage::home_layout`) needs no new case. Its last pixel row
//!   is the hairline;
//! * `SECTION_GAP` (24 px) of room, then the content, down to the panel's own bottom edge;
//! * the 8 px gap that is part of the glass, then the hairline over the island's row (the right island's panels have the same), then
//!   the island's row itself, as high as the island: the field stands in it at the island's own place.
//!
//! The header's lengths are the panel's own logical px (the footer it replaces drew them at these sizes); the field's are dock units times
//! the dock's size, exactly as the island's own lengths are (`plan.rs`, `left.rs`).

use rf_win::dock::layout::Rect;
use rf_win::dock::startpage as page;

use super::plan;

/// The header's height: the room the panel's field took at the top (the padding above the pill and the pill).
pub const HEAD_H: f32 = page::TOP_PAD + page::SEARCH_H;
/// The account's picture, the gear's and the power button's side and picture, and the plate's radius (what the panel's own footer drew).
pub const AVATAR: f32 = 28.0;
pub const NAME_TEXT: f32 = 14.0;
pub const NAME_GAP: f32 = 12.0;
pub const BUTTON: f32 = 36.0;
pub const ICON: f32 = 18.0;
pub const BUTTON_RADIUS: f32 = 8.0;
/// Between the gear and the power button.
pub const BUTTONS_GAP: f32 = 8.0;
/// Between the power menu and the button above it.
pub const MENU_GAP: f32 = 6.0;

/// The header: where the things of it stand, logical px in the panel's window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Head {
    pub width: f32,
    /// The header's height; its last pixel row (`line_y`) is the hairline under it.
    pub height: f32,
    pub line_y: f32,
    /// The middle of the space above the hairline: what everything in the header is centred on.
    pub centre: f32,
    /// Between the glass's sides and the account / the power button.
    pub side: f32,
    pub avatar: f32,
    pub name_gap: f32,
    pub name_text: f32,
    pub button: f32,
    pub icon: f32,
    pub button_radius: f32,
    pub buttons_gap: f32,
    pub menu_gap: f32,
}

impl Head {
    /// How far below the window's top edge the power menu's top edge is: just under the power button.
    pub fn power_menu_top(&self) -> f32 {
        self.centre + self.button / 2.0 + self.menu_gap
    }
}

/// The whole grown panel: the header, the content, the island's row and the field in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub head: Head,
    /// The content's rectangle (x, y, w, h): between the header (and the room under it) and the panel's own bottom edge.
    pub content: [f32; 4],
    /// The island's row: its top edge (where the hairline over it is) and its height; it ends at the window's bottom edge.
    pub row_top: f32,
    pub row_height: f32,
    /// The panel's search field, in the island's row at the island's own place for it (x, y, w, h), and the dock's size factor its
    /// lengths follow (the field's padding, picture and text are theirs times this).
    pub field: [f32; 4],
    pub field_k: f32,
    /// The field's width when the glass is fully grown: from its place to the row's side padding at the right (the weather is put
    /// away meanwhile, `grow_host::IslandFade::weather`). Never less than the island's own field.
    pub field_wide: f32,
}

impl Layout {
    /// The field's width when the glass has grown by `u` (the glass's own progress, 0 to 1): the island's own field at 0, the whole
    /// row at 1, so the field widens with the glass and narrows with it (the user: when Start opens the search field widens too).
    pub fn field_width(&self, u: f32) -> f32 {
        self.field[2] + (self.field_wide - self.field[2]) * u.clamp(0.0, 1.0)
    }
}

/// How far the kit's input stands in from its own left edge at the size the row's field uses (`Size::Size`: 8 px): the field takes
/// it back, so the text and the caret start exactly where the island's own text starts.
pub const INPUT_INSET: f32 = 8.0;

/// The opacity of the island's weather while the glass has grown by `u`: gone by a fifth of the way (the widening field takes its
/// room), back in the last fifth of the way home.
pub fn weather_alpha(u: f32) -> f32 {
    (1.0 - u / 0.2).clamp(0.0, 1.0)
}

/// The layout of the glass `grown` that has grown out of `island` for a panel whose own rectangle is `panel` (all device px, screen
/// coordinates). `unit` is device px per dock unit (`host::dock_unit`), `scale` the monitor's.
pub fn layout(island: Rect, panel: Rect, grown: Rect, unit: f32, scale: f32) -> Layout {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let k = unit / s;
    let width = grown.width() as f32 / s;
    let row_top = (island.top - grown.top) as f32 / s;
    let row_height = island.height() as f32 / s;
    let content_top = HEAD_H + page::SECTION_GAP;
    let content_bottom = ((panel.bottom - grown.top) as f32 / s).max(content_top);
    let field_h = plan::SEARCH_H * k;
    let field_x = (island.left - grown.left) as f32 / s + plan::SEARCH_LEFT * k;
    let field_w = plan::SEARCH_W * k;
    Layout {
        head: Head {
            width,
            height: HEAD_H,
            line_y: HEAD_H - page::DIVIDER_H,
            centre: (HEAD_H - page::DIVIDER_H) / 2.0,
            side: page::SIDE_PAD,
            avatar: AVATAR,
            name_gap: NAME_GAP,
            name_text: NAME_TEXT,
            button: BUTTON,
            icon: ICON,
            button_radius: BUTTON_RADIUS,
            buttons_gap: BUTTONS_GAP,
            menu_gap: MENU_GAP,
        },
        content: [0.0, content_top, width, content_bottom - content_top],
        row_top,
        row_height,
        // (left.rs: padding, the logo, the gap, then the field, centred upright in the row)
        field: [field_x, row_top + (row_height - field_h) / 2.0, field_w, field_h],
        field_k: k,
        field_wide: (width - field_x - plan::SIDE_PAD * k).max(field_w),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The field widens with the glass: the island's own width at the start, the row up to the side padding at the end, and the
    /// weather is gone before the field has covered much of its room.
    #[test]
    fn the_field_widens_with_the_glass_and_the_weather_makes_room() {
        for (dpi, factor) in [(1.0, 1.0), (1.0, 0.75), (1.5, 1.0), (1.0, 1.5)] {
            let (island, panel_rect, grown, unit) = setup(dpi, factor, 330.0);
            let l = layout(island, panel_rect, grown, unit, dpi);
            let k = l.field_k;
            assert_eq!(l.field_width(0.0), l.field[2], "closed: the island's own field");
            assert!((l.field_width(1.0) - l.field_wide).abs() < 0.01);
            assert!((l.field[0] + l.field_wide + plan::SIDE_PAD * k - grown.width() as f32 / dpi).abs() < 0.01, "open: up to the row's padding");
            assert!(l.field_wide > l.field[2] + 100.0, "the panel is much wider than the island's field");
            let mid = l.field_width(0.5);
            assert!(mid > l.field[2] && mid < l.field_wide);
            assert_eq!(l.field_width(7.0), l.field_wide);
            assert_eq!(l.field_width(-1.0), l.field[2]);
        }
        assert_eq!(weather_alpha(0.0), 1.0);
        assert_eq!(weather_alpha(0.2), 0.0);
        assert_eq!(weather_alpha(1.0), 0.0);
        assert!(weather_alpha(0.1) > 0.4 && weather_alpha(0.1) < 0.6);
    }
    use rf_win::dock::grow::{self, Corner, Spec};
    use rf_win::dock::layout::{self, Widths};
    use rf_win::dock::panel::{self, Side};
    use rf_win::dock::size::{self, DockSize};

    /// The island, the panel's own rectangle and the grown glass of the Start panel for a 2560 x 1440 monitor at this scale and dock
    /// size, built the way the dock builds them (the panel gives up its own bottom row, as `panel.rs` places it).
    fn setup(dpi: f32, factor: f32, left_width: f32) -> (Rect, Rect, Rect, f32) {
        let work = Rect::new(0, 0, 2560, 1440 - (48.0 * dpi).round() as i32);
        let unit = size::unit(dpi, factor);
        let isl = layout::islands(work, unit, Widths { left: left_width, middle: 600.0, right: 280.0 });
        let h = grow::height_without_own_row(page::PANEL_MAX_H);
        let panel_rect = panel::place(isl.left, (page::PANEL_W, h), dpi, Side::Left, work);
        let spec = Spec::new(isl.left, panel_rect, Corner::BottomLeft, 24.0 * unit, 8.0 * dpi);
        (isl.left, panel_rect, spec.grown, unit)
    }

    const DPIS: [f32; 3] = [1.0, 1.25, 1.5];

    #[test]
    fn on_the_users_screen_at_100_percent_the_header_the_content_and_the_row_stand_so() {
        // the left glass (12, 1338, 342, 1386); the panel, 65 px shorter than 720, 8 px above it; the grown glass (12, 675, 654, 1386)
        let (island, panel_rect, grown, unit) = setup(1.0, 1.0, 330.0);
        assert_eq!(island, Rect::new(12, 1338, 342, 1386));
        assert_eq!(grown, Rect::new(12, 675, 654, 1386));
        assert_eq!(panel_rect, Rect::new(12, 675, 654, 1330));
        let l = layout(island, panel_rect, grown, unit, 1.0);
        assert_eq!(l.head.width, 642.0);
        assert_eq!((l.head.height, l.head.line_y, l.head.centre), (60.0, 59.0, 29.5));
        // the content: 24 under the header, down to the panel's bottom edge (655 high in the window)
        assert_eq!(l.content, [0.0, 84.0, 642.0, 655.0 - 84.0]);
        // the island's row: 8 px of glass under the content, then the row; it ends at the window's bottom edge (711 high)
        assert_eq!((l.row_top, l.row_height), (663.0, 48.0));
        assert_eq!(l.row_top + l.row_height, 711.0);
        // the field: padding 8 + logo 32 + gap 8 from the island's left edge, 32 high, centred in the 48 px row
        assert_eq!(l.field, [48.0, 663.0 + 8.0, 220.0, 32.0]);
        assert_eq!(l.field_k, 1.0);
        assert_eq!(l.head.power_menu_top(), 29.5 + 18.0 + 6.0);
    }

    /// THE ROW STAYS EXACTLY AS IT IS: the field's rectangle and the row's are the island's own at every dock size and every scale, in the
    /// island's coordinates (the island's left edge, top edge and height on the screen).
    #[test]
    fn the_rows_rectangles_are_the_islands_own_at_every_size() {
        for dpi in DPIS {
            for f in DockSize::ALL.map(DockSize::factor) {
                let (island, panel_rect, grown, unit) = setup(dpi, f, 330.0);
                let l = layout(island, panel_rect, grown, unit, dpi);
                // the row is the island's rectangle (screen px)
                assert!((l.row_top * dpi + grown.top as f32 - island.top as f32).abs() < 1e-3, "{dpi} {f}");
                assert!(((l.row_top + l.row_height) * dpi - grown.height() as f32).abs() < 1e-3, "the row ends where the glass does: {dpi} {f}");
                assert!((l.row_height * dpi - island.height() as f32).abs() < 1e-3);
                // the field, in screen px, is where the island's own field is: padding, logo and gap in from the island's left edge,
                // SEARCH_W by SEARCH_H dock units, centred upright in the island
                let [fx, fy, fw, fh] = l.field.map(|v| v * dpi);
                let (want_x, want_w, want_h) = (island.left as f32 + plan::SEARCH_LEFT * unit, plan::SEARCH_W * unit, plan::SEARCH_H * unit);
                assert!((fx + grown.left as f32 - want_x).abs() < 1e-3, "x {dpi} {f}");
                assert!((fw - want_w).abs() < 1e-3 && (fh - want_h).abs() < 1e-3, "size {dpi} {f}");
                let want_y = island.top as f32 + (island.height() as f32 - want_h) / 2.0;
                assert!((fy + grown.top as f32 - want_y).abs() < 1e-3, "y {dpi} {f}");
                // and it lies inside the row, with room above and below
                assert!(fy > l.row_top * dpi && fy + fh < (l.row_top + l.row_height) * dpi);
            }
        }
    }

    /// The island's width is what its window is made as wide as (`plan::left_width`, which `left.rs` lays its row out to fit), so the
    /// field's right edge plus the island's side padding is the island's right edge when there is no weather. This ties the field's
    /// horizontal place to the island's real width (the island is laid out by `islands` from it), not only to the constants `layout`
    /// itself is made of: a padding or gap that `left_width` counts differently from `layout` makes this fail.
    #[test]
    fn the_field_ends_where_the_islands_own_width_says_it_does() {
        for dpi in DPIS {
            for f in DockSize::ALL.map(DockSize::factor) {
                let (island, panel_rect, grown, unit) = setup(dpi, f, plan::left_width(None));
                let l = layout(island, panel_rect, grown, unit, dpi);
                let right_edge = (island.right - grown.left) as f32 / dpi;
                let field_end = l.field[0] + l.field[2];
                assert!((field_end + plan::SIDE_PAD * l.field_k - right_edge).abs() < 1.0, "{dpi} {f}: field ends {field_end}, island ends {right_edge}");
            }
        }
    }

    /// The field's centre line is the logo's, which `left.rs` centres in the view's box: the window less the margins the placement gives
    /// (with and without the shadow's margin). Compared with where the island's own layout puts things, not with this module's sums.
    #[test]
    fn the_field_stands_on_the_logos_centre_line_in_the_islands_view() {
        let monitor = Rect::new(0, 0, 2560, 1440);
        for dpi in DPIS {
            for f in DockSize::ALL.map(DockSize::factor) {
                for shadow in [true, false] {
                    let (island, panel_rect, grown, unit) = setup(dpi, f, 330.0);
                    let work = Rect::new(0, 0, 2560, 1440 - (48.0 * dpi).round() as i32);
                    let isl = layout::islands(work, unit, Widths { left: 330.0, middle: 600.0, right: 280.0 });
                    let pl = layout::placements(&isl, monitor, unit, shadow).left;
                    assert_eq!(pl.glass, island);
                    let [_, mt, _, mb] = pl.margins();
                    let (box_top, box_h) = (pl.window.top + mt, pl.window.height() - mt - mb);
                    let logo_centre = box_top as f32 + box_h as f32 / 2.0;
                    let l = layout(island, panel_rect, grown, unit, dpi);
                    let field_centre = (l.field[1] + l.field[3] / 2.0) * dpi + grown.top as f32;
                    assert!((field_centre - logo_centre).abs() < 1e-3, "{dpi} {f} shadow {shadow}");
                }
            }
        }
    }

    /// The content has the height it had before the field and the footer moved: the old panel (720 high, field on top, footer under)
    /// gave the content `720 - 60 - 24 - 65`; the grown panel (655 high, header on top, no footer) gives the same.
    #[test]
    fn the_content_keeps_the_height_it_had() {
        for dpi in DPIS {
            let (island, panel_rect, grown, unit) = setup(dpi, 1.0, 330.0);
            let l = layout(island, panel_rect, grown, unit, dpi);
            let panel_h = panel_rect.height() as f32 / dpi;
            let old = grow::height_with_own_row(panel_h) - (page::TOP_PAD + page::SEARCH_H) - page::SECTION_GAP - (page::DIVIDER_H + page::FOOTER_H);
            assert!((l.content[3] - old).abs() < 0.51, "{dpi}: {} against {old}", l.content[3]);
            // the page layout is worked out from the height the panel had, as it always was
            assert!((grow::height_with_own_row(panel_h) - 720.0).abs() < 0.51);
            assert_eq!(l.content[1], 84.0, "the content starts 24 under the header whatever the scale: the lengths are the panel's own");
        }
    }

    #[test]
    fn the_headers_things_fit_it_at_every_size() {
        for dpi in DPIS {
            for f in DockSize::ALL.map(DockSize::factor) {
                let (island, panel_rect, grown, unit) = setup(dpi, f, 330.0);
                let h = layout(island, panel_rect, grown, unit, dpi).head;
                // the hairline is the last pixel row; everything is centred above it
                assert_eq!(h.line_y + page::DIVIDER_H, h.height);
                assert!(h.avatar < h.line_y && h.button < h.line_y, "{dpi} {f}");
                assert!(h.icon < h.button && h.avatar <= h.button);
                // the same lengths the panel's own footer drew: the account and the buttons end where the content's side padding is
                assert_eq!(h.side, page::SIDE_PAD);
                // the power menu opens under the button and clear of the hairline
                assert!(h.power_menu_top() > h.centre + h.button / 2.0);
            }
        }
    }

    #[test]
    fn the_header_does_not_depend_on_the_island_so_a_wide_weather_text_cannot_cover_it() {
        // the island's window lies over the island's rectangle only; the header is at the top of the glass, far above it, so the
        // check that used to refuse a growth when the island lay over the gear is gone. A wide island changes the field's place not at all.
        let (island, panel_rect, grown, unit) = setup(1.0, 1.0, 330.0);
        let wide = Rect::new(island.left, island.top, island.left + 600, island.bottom);
        let a = layout(island, panel_rect, grown, unit, 1.0);
        let b = layout(wide, panel_rect, grown, unit, 1.0);
        assert_eq!(a.head, b.head);
        assert_eq!(a.field, b.field);
    }

    /// A windowless probe: prints the numbers for three screens (nothing is drawn, read or written).
    /// `cargo test --offline -p rf-app --lib print_the_numbers -- --ignored --nocapture`
    #[test]
    #[ignore = "prints the numbers for a look"]
    fn print_the_numbers_for_three_screens() {
        for (dpi, f) in [(1.0, 1.0), (1.25, 1.0), (1.5, 1.4)] {
            let (island, panel_rect, grown, unit) = setup(dpi, f, 330.0);
            let l = layout(island, panel_rect, grown, unit, dpi);
            println!("dpi {dpi} dock size {f} (unit {unit}): island {island:?}, panel {panel_rect:?}, grown glass {grown:?}");
            println!("  header {}x{} (hairline at y {}), centre {}, sides {}", l.head.width, l.head.height, l.head.line_y, l.head.centre, l.head.side);
            println!("  content x {} y {} w {} h {}", l.content[0], l.content[1], l.content[2], l.content[3]);
            println!("  island row y {} h {} (ends at {}), field x {} y {} w {} h {} (dock size {})", l.row_top, l.row_height, l.row_top + l.row_height, l.field[0], l.field[1], l.field[2], l.field[3], l.field_k);
        }
    }

    #[test]
    fn a_broken_scale_reads_as_100_percent() {
        let (island, panel_rect, grown, unit) = setup(1.0, 1.0, 330.0);
        assert_eq!(layout(island, panel_rect, grown, unit, 0.0), layout(island, panel_rect, grown, unit, 1.0));
    }
}
