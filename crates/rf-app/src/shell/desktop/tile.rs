//! The inside of one icon's tile: where the picture and the label sit in a grid cell, at every display scale. Pure.
//!
//! All lengths are CANVAS pixels (the monitor's own physical pixels); the view turns them into window pixels with
//! [`super::geometry::Mapping`]. The numbers come from the 100 % figures (a 75 pixel cell for the 48 pixel icon, which is the one
//! size seen on a real machine, `rf_core::desktop::grid`) times the display scale, rounded half up like the grid's own.
//!
//! The tile is the cell shrunk by a margin and as tall as its content: the picture on top, then one or two lines of label. On the
//! bare square grid (75 pixels for the 48 pixel icon) a cell cannot hold the picture and two lines of text, so the tile of a
//! two-line label reaches a few pixels past its cell; on Windows' own desktop grid (75 x 99, `Spacing::WINDOWS_11`) it fits.
//!
//! THE LABEL'S WIDTH. Explorer's labels on this machine are up to 71 pixels wide in the 75 pixel cell ("Bu bilgisayar", 67 pixels,
//! stands on one line there). The tile is therefore the cell less ONE pixel on each side and the text one pixel inside the tile:
//! 71 pixels at 100 %. With the 2 + 3 pixels of the first version (65 pixels of text) common names wrapped that Explorer shows on
//! one line.
//!
//! WHERE THE PICTURE AND THE LABEL SIT. Measured against Explorer's desktop on the same screen (medium icons, 100 %): the picture's
//! top is 6 pixels below the cell's top and the label's first line begins 3 pixels under the picture. So: 1 pixel of margin, 5 above
//! the picture, 3 between picture and label, 2 under the label; a two-line tile is then 88 pixels tall and fits the 99 pixel row.

use rf_core::desktop::geom::Rect;
use rf_core::desktop::grid::{GridMetrics, scaled};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileLayout {
    /// Gap between the cell's edge and the tile.
    pub margin: u32,
    pub tile_w: u32,
    pub icon: u32,
    /// Gap above the picture.
    pub top: u32,
    /// Gap between the picture and the label.
    pub gap: u32,
    /// Gap under the label (under the picture when there is no label).
    pub pad: u32,
    /// Horizontal gap between the tile's edge and the text.
    pub text_pad: u32,
    /// Font size in canvas pixels and the height of one label line.
    pub font: u32,
    pub line_h: u32,
    /// The height of a cell (the next row's tile starts one `margin` below it).
    pub pitch_y: u32,
}

impl TileLayout {
    pub fn new(m: GridMetrics, scale_percent: u32) -> TileLayout {
        let s = |v: u32| scaled(v, scale_percent.max(50));
        let margin = s(1);
        let font = s(12);
        TileLayout {
            margin,
            tile_w: m.pitch_x.saturating_sub(2 * margin),
            icon: m.icon_px,
            top: s(5),
            gap: s(3),
            pad: s(2),
            text_pad: s(1),
            font,
            line_h: (font * 6).div_ceil(5), // 1.2 times the font, rounded up
            pitch_y: m.pitch_y,
        }
    }

    /// Where the picture goes, inside the tile (x, y).
    pub fn icon_at(&self) -> (u32, u32) {
        (self.tile_w.saturating_sub(self.icon) / 2, self.top)
    }

    /// Where the label's first line begins, from the tile's top.
    pub fn label_top(&self) -> u32 {
        self.top + self.icon + self.gap
    }

    /// The width a line of the label may be.
    pub fn text_w(&self) -> u32 {
        self.tile_w.saturating_sub(2 * self.text_pad)
    }

    /// The tile's height for a label of `lines` lines (0 for none).
    pub fn height(&self, lines: usize) -> u32 {
        self.top + self.icon + if lines == 0 { self.pad } else { self.gap + lines as u32 * self.line_h + self.pad }
    }

    /// The tile for the cell at (`left`, `top`) in canvas pixels, `lines` tall.
    pub fn rect(&self, cell_left: i32, cell_top: i32, lines: usize) -> Rect {
        Rect::from_size(cell_left + self.margin as i32, cell_top + self.margin as i32, self.tile_w, self.height(lines))
    }

    /// What a click or the rubber band must touch before the label's lines are known: as tall as a two-line tile.
    pub fn hit(&self, cell_left: i32, cell_top: i32) -> Rect {
        self.hit_for(cell_left, cell_top, 2)
    }

    /// What a click or the rubber band must touch for a label of `lines` lines: the tile, but never lower than where the next row's
    /// tile begins. A two-line tile is taller than its cell, so without the cut the last few pixels of its label would also be the
    /// first few pixels of the item below, and a click there would go to whichever is on top.
    pub fn hit_for(&self, cell_left: i32, cell_top: i32, lines: usize) -> Rect {
        let mut r = self.rect(cell_left, cell_top, lines);
        let limit = cell_top + self.pitch_y as i32 + self.margin as i32;
        if self.pitch_y > 0 && r.bottom > limit {
            r.bottom = limit;
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_core::desktop::grid::{IconSize, metrics};

    #[test]
    fn the_medium_tile_at_100_percent() {
        let t = TileLayout::new(metrics(IconSize::Medium, 100), 100);
        assert_eq!((t.margin, t.tile_w, t.icon, t.top, t.gap, t.pad, t.text_pad, t.font, t.line_h), (1, 73, 48, 5, 3, 2, 1, 12, 15));
        assert_eq!(t.icon_at(), (12, 5), "the picture is centred in the tile, 6 pixels under the cell's top with the margin");
        assert_eq!(t.label_top(), 5 + 48 + 3);
        assert_eq!(t.text_w(), 71, "as wide as Explorer's labels in the 75 pixel cell");
        assert_eq!(t.height(1), 5 + 48 + 3 + 15 + 2);
        assert_eq!(t.height(2), 5 + 48 + 3 + 30 + 2);
        assert_eq!(t.height(0), 5 + 48 + 2);
        assert_eq!(t.rect(75, 150, 2), Rect::from_size(76, 151, 73, 88));
    }

    #[test]
    fn the_tile_scales_with_the_display() {
        let t150 = TileLayout::new(metrics(IconSize::Medium, 150), 150);
        assert_eq!((t150.icon, t150.margin, t150.font), (72, 2, 18));
        assert_eq!(t150.tile_w, 113 - 4);
        assert_eq!(t150.line_h, 22); // 18 * 1.2 = 21.6, rounded up
        let t200 = TileLayout::new(metrics(IconSize::Small, 200), 200);
        assert_eq!((t200.icon, t200.tile_w), (64, 128 - 4));
    }

    #[test]
    fn the_picture_never_leaves_the_tile_sideways_and_nothing_underflows() {
        for size in IconSize::ALL {
            for pct in [100, 125, 150, 175, 200, 250] {
                let t = TileLayout::new(metrics(size, pct), pct);
                let (x, _) = t.icon_at();
                assert!(x + t.icon <= t.tile_w, "{size:?} {pct}");
                assert!(t.text_w() > 0);
            }
        }
        // a nonsense scale and a metrics of zero do not panic
        let z = TileLayout::new(GridMetrics { icon_px: 0, pitch_x: 0, pitch_y: 0 }, 0);
        assert_eq!((z.tile_w, z.text_w()), (0, 0));
    }

    #[test]
    fn the_hit_rectangle_is_the_tile_cut_where_the_next_row_begins() {
        let t = TileLayout::new(metrics(IconSize::Medium, 100), 100);
        // 5 + 48 + 3 + 30 + 2 = 88 tall from y = 1 would end at 89; the next row's tile starts at 75 + 1
        assert_eq!(t.hit(0, 0), Rect::from_size(1, 1, 73, 75));
        // on Windows' own grid (99 down) the two-line tile fits in its cell and nothing is cut
        let w = TileLayout::new(rf_core::desktop::grid::metrics_spaced(IconSize::Medium, 100, Some(rf_core::desktop::grid::Spacing::WINDOWS_11)), 100);
        assert_eq!(w.hit(0, 0), w.rect(0, 0, 2));
        assert_eq!(t.hit_for(0, 0, 1), t.rect(0, 0, 1), "a one-line label's tile (73 tall from y = 1) fits in its cell: no cut");
        assert!(t.hit(0, 0).height() > t.hit_for(0, 0, 1).height());
    }

    #[test]
    fn the_hit_rectangles_of_a_column_never_overlap_at_any_scale_or_size() {
        for size in IconSize::ALL {
            for pct in [100, 125, 150, 175, 200] {
                let m = metrics(size, pct);
                let t = TileLayout::new(m, pct);
                for lines in 0..=2 {
                    let upper = t.hit_for(0, 0, lines);
                    let lower = t.hit_for(0, m.pitch_y as i32, 2);
                    assert!(upper.intersect(&lower).is_empty(), "{size:?} {pct} {lines}");
                }
            }
        }
        // a metrics of zero is not cut (and does not panic)
        let z = TileLayout::new(GridMetrics { icon_px: 0, pitch_x: 0, pitch_y: 0 }, 100);
        let _ = z.hit_for(0, 0, 2);
    }
}
