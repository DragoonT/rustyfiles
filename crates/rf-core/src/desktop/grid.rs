//! The icon grid: icon size steps, the size of a cell at every display scale, cells and their column-major order.
//!
//! WHERE THE NUMBERS COME FROM. The only value read on a real machine (note 4, 5.3) is the one for the default, medium, size: icon
//! 48, `SM_CXICONSPACING` = `SM_CYICONSPACING` = 75 pixels at 100 %. Small = 32 and Large = 96 are the sizes the same note reports
//! from memory (UNVERIFIED). The rule for the cell is this file's own proposal: icon + 27 pixels, but never narrower than 64 (room
//! for a two-line label), which gives exactly 75 for medium. At other scales every length is the 100 % one times the scale, rounded
//! half up. Windows' own desktop uses the same numbers only for medium; compare with the real desktop before trusting the others.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::geom::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IconSize {
    Small,
    Medium,
    Large,
}

impl IconSize {
    pub const ALL: [IconSize; 3] = [IconSize::Small, IconSize::Medium, IconSize::Large];

    /// The icon's edge at 100 %.
    pub const fn px(self) -> u32 {
        match self {
            IconSize::Small => 32,
            IconSize::Medium => 48,
            IconSize::Large => 96,
        }
    }
    /// The step whose size is nearest to `px` (the layout file stores the number; 48 is what Explorer keeps for medium). Halfway
    /// between two steps the larger one.
    pub fn from_px(px: u32) -> IconSize {
        IconSize::ALL.into_iter().min_by_key(|s| ((s.px() as i64 - px as i64).abs(), u32::MAX - s.px())).unwrap_or(IconSize::Medium)
    }
    /// One step bigger / smaller (Ctrl + wheel); the same size at the ends.
    pub fn larger(self) -> IconSize {
        match self {
            IconSize::Small => IconSize::Medium,
            _ => IconSize::Large,
        }
    }
    pub fn smaller(self) -> IconSize {
        match self {
            IconSize::Large => IconSize::Medium,
            _ => IconSize::Small,
        }
    }
    /// The cell at 100 %: icon + 27, at least 64, square.
    pub const fn pitch(self) -> u32 {
        let p = self.px() + 27;
        if p < 64 { 64 } else { p }
    }
}

/// `v * pct / 100` rounded half up.
pub fn scaled(v: u32, pct: u32) -> u32 {
    ((v as u64 * pct as u64 * 2 + 100) / 200) as u32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridMetrics {
    pub icon_px: u32,
    pub pitch_x: u32,
    pub pitch_y: u32,
}

impl GridMetrics {
    /// The gap on each side of the icon inside its cell (the icon is centred horizontally; the label goes below).
    pub fn icon_inset_x(&self) -> u32 {
        self.pitch_x.saturating_sub(self.icon_px) / 2
    }
}

/// The metrics of a size at a display scale in percent (100, 125, 150, 175, 200 ...).
pub fn metrics(size: IconSize, scale_percent: u32) -> GridMetrics {
    let pct = scale_percent.max(50);
    GridMetrics { icon_px: scaled(size.px(), pct), pitch_x: scaled(size.pitch(), pct), pitch_y: scaled(size.pitch(), pct) }
}

/// One grid cell. The derived order is column-major (compare the column first), which is the order Windows fills the desktop in: down
/// the first column, then the next column to the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cell {
    pub col: u32,
    pub row: u32,
}

impl Cell {
    pub const fn new(col: u32, row: u32) -> Self {
        Cell { col, row }
    }
}

/// The grid of one monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    /// The top-left of cell (0,0), in monitor pixels (the work area's top-left: icons stay clear of a taskbar or dock).
    pub origin: (i32, i32),
    pub metrics: GridMetrics,
    pub cols: u32,
    pub rows: u32,
}

impl Grid {
    /// `work`: the part of the monitor the icons may use, in the monitor's own coordinates.
    pub fn new(work: Rect, metrics: GridMetrics) -> Grid {
        let (w, h) = (work.width().max(0) as u32, work.height().max(0) as u32);
        Grid {
            origin: (work.left, work.top),
            metrics,
            cols: w.checked_div(metrics.pitch_x).unwrap_or(0),
            rows: h.checked_div(metrics.pitch_y).unwrap_or(0),
        }
    }
    pub fn len(&self) -> usize {
        self.cols as usize * self.rows as usize
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn contains(&self, c: Cell) -> bool {
        c.col < self.cols && c.row < self.rows
    }
    /// The cell's rectangle in monitor pixels.
    pub fn cell_rect(&self, c: Cell) -> Rect {
        let (px, py) = (self.metrics.pitch_x as i32, self.metrics.pitch_y as i32);
        Rect::from_size(self.origin.0 + c.col as i32 * px, self.origin.1 + c.row as i32 * py, px as u32, py as u32)
    }
    fn index(&self, c: Cell) -> usize {
        c.col as usize * self.rows as usize + c.row as usize
    }
    fn at_index(&self, i: usize) -> Cell {
        Cell { col: (i / self.rows as usize) as u32, row: (i % self.rows as usize) as u32 }
    }
    /// The first cell in column-major order that is not in `occupied`.
    pub fn first_free(&self, occupied: &HashSet<Cell>) -> Option<Cell> {
        (0..self.len()).map(|i| self.at_index(i)).find(|c| !occupied.contains(c))
    }
    /// The first free cell after `from` in column-major order, wrapping round to the start; `None` when the grid is full.
    pub fn next_free_after(&self, from: Cell, occupied: &HashSet<Cell>) -> Option<Cell> {
        let n = self.len();
        if n == 0 {
            return None;
        }
        let start = if self.contains(from) { self.index(from) } else { n - 1 };
        (1..=n).map(|k| self.at_index((start + k) % n)).find(|c| !occupied.contains(c))
    }
    /// The free cell closest to `target` (distance between cell centres; the earlier in column-major order wins a tie): where a
    /// dropped icon goes when the cell it was dropped on is taken.
    pub fn nearest_free(&self, target: Cell, occupied: &HashSet<Cell>) -> Option<Cell> {
        let d = |c: Cell| {
            let (dc, dr) = (c.col as i64 - target.col as i64, c.row as i64 - target.row as i64);
            (dc * self.metrics.pitch_x as i64).pow(2) + (dr * self.metrics.pitch_y as i64).pow(2)
        };
        (0..self.len()).map(|i| self.at_index(i)).filter(|c| !occupied.contains(c)).min_by_key(|c| (d(*c), *c))
    }
    /// A dragged icon's top-left corner (monitor pixels) to a cell and an offset inside it. Aligned: the nearest cell, no offset.
    /// Free: the cell the corner is in, and how far into it. Outside the grid the nearest edge cell is taken and the offset is 0.
    pub fn snap(&self, x: i32, y: i32, align_to_grid: bool) -> Option<(Cell, i32, i32)> {
        if self.is_empty() {
            return None;
        }
        let (px, py) = (self.metrics.pitch_x as i32, self.metrics.pitch_y as i32);
        let (rx, ry) = (x - self.origin.0, y - self.origin.1);
        let (cx, cy) = if align_to_grid { ((rx + px / 2).div_euclid(px), (ry + py / 2).div_euclid(py)) } else { (rx.div_euclid(px), ry.div_euclid(py)) };
        let col = cx.clamp(0, self.cols as i32 - 1);
        let row = cy.clamp(0, self.rows as i32 - 1);
        let inside = cx == col && cy == row;
        let (dx, dy) = if align_to_grid || !inside { (0, 0) } else { (rx - col * px, ry - row * py) };
        Some((Cell { col: col as u32, row: row as u32 }, dx, dy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_size_steps() {
        assert_eq!(IconSize::ALL.map(|s| s.px()), [32, 48, 96]);
        assert_eq!(IconSize::from_px(48), IconSize::Medium);
        assert_eq!(IconSize::from_px(40), IconSize::Medium);
        assert_eq!(IconSize::from_px(33), IconSize::Small);
        assert_eq!(IconSize::from_px(256), IconSize::Large);
        assert_eq!(IconSize::from_px(0), IconSize::Small);
        assert_eq!(IconSize::Small.larger(), IconSize::Medium);
        assert_eq!(IconSize::Large.larger(), IconSize::Large);
        assert_eq!(IconSize::Large.smaller(), IconSize::Medium);
        assert_eq!(IconSize::Small.smaller(), IconSize::Small);
    }

    #[test]
    fn metrics_at_every_scale() {
        // medium: 48 / 75 at 100 % (the cell size seen on a real machine), then scaled and rounded half up
        let m = |s, p| {
            let g = metrics(s, p);
            (g.icon_px, g.pitch_x, g.pitch_y)
        };
        assert_eq!(m(IconSize::Medium, 100), (48, 75, 75));
        assert_eq!(m(IconSize::Medium, 125), (60, 94, 94)); // 93.75
        assert_eq!(m(IconSize::Medium, 150), (72, 113, 113)); // 112.5 rounds up
        assert_eq!(m(IconSize::Medium, 200), (96, 150, 150));
        assert_eq!(m(IconSize::Small, 100), (32, 64, 64)); // 32 + 27 = 59, raised to the minimum
        assert_eq!(m(IconSize::Small, 125), (40, 80, 80));
        assert_eq!(m(IconSize::Small, 150), (48, 96, 96));
        assert_eq!(m(IconSize::Small, 200), (64, 128, 128));
        assert_eq!(m(IconSize::Large, 100), (96, 123, 123));
        assert_eq!(m(IconSize::Large, 125), (120, 154, 154)); // 153.75
        assert_eq!(m(IconSize::Large, 150), (144, 185, 185)); // 184.5
        assert_eq!(m(IconSize::Large, 200), (192, 246, 246));
        assert_eq!(metrics(IconSize::Medium, 100).icon_inset_x(), 13);
    }

    #[test]
    fn a_grid_fits_whole_cells_into_the_work_area() {
        let work = Rect::new(0, 0, 1920, 1080);
        let dims = |p| {
            let g = Grid::new(work, metrics(IconSize::Medium, p));
            (g.cols, g.rows)
        };
        assert_eq!(dims(100), (25, 14)); // 1920 / 75, 1080 / 75
        assert_eq!(dims(125), (20, 11));
        assert_eq!(dims(150), (16, 9));
        assert_eq!(dims(200), (12, 7));
        // a dock that takes the bottom 100 px: the work area ends at 980
        let g = Grid::new(Rect::new(0, 0, 1920, 980), metrics(IconSize::Medium, 100));
        assert_eq!((g.cols, g.rows), (25, 13));
        // a bar on the left shifts the origin
        let g = Grid::new(Rect::new(60, 0, 1920, 1080), metrics(IconSize::Medium, 100));
        assert_eq!(g.origin, (60, 0));
        assert_eq!(g.cell_rect(Cell::new(1, 2)), Rect::new(135, 150, 210, 225));
        // nothing fits: an empty grid, not a panic
        let g = Grid::new(Rect::new(0, 0, 50, 50), metrics(IconSize::Medium, 100));
        assert!(g.is_empty() && g.first_free(&HashSet::new()).is_none() && g.snap(0, 0, true).is_none());
        assert!(Grid::new(Rect::default(), metrics(IconSize::Medium, 100)).is_empty());
    }

    fn grid() -> Grid {
        // 3 columns x 2 rows of 75
        Grid::new(Rect::new(0, 0, 225, 150), metrics(IconSize::Medium, 100))
    }

    #[test]
    fn the_order_is_down_the_column_then_to_the_right() {
        let g = grid();
        let mut occ = HashSet::new();
        let mut got = Vec::new();
        while let Some(c) = g.first_free(&occ) {
            got.push((c.col, c.row));
            occ.insert(c);
        }
        assert_eq!(got, [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)]);
        let mut cells = vec![Cell::new(1, 0), Cell::new(0, 1), Cell::new(0, 0)];
        cells.sort();
        assert_eq!(cells, [Cell::new(0, 0), Cell::new(0, 1), Cell::new(1, 0)]);
    }

    #[test]
    fn free_cell_searches() {
        let g = grid();
        let occ: HashSet<Cell> = [Cell::new(0, 0), Cell::new(1, 0)].into_iter().collect();
        assert_eq!(g.first_free(&occ), Some(Cell::new(0, 1)));
        // after (1,0): the next in order is (1,1)
        assert_eq!(g.next_free_after(Cell::new(1, 0), &occ), Some(Cell::new(1, 1)));
        // after the last cell: wraps to the first free one
        assert_eq!(g.next_free_after(Cell::new(2, 1), &occ), Some(Cell::new(0, 1)));
        // a cell outside the grid counts as "after the end"
        assert_eq!(g.next_free_after(Cell::new(9, 9), &occ), Some(Cell::new(0, 1)));
        // a full grid has none
        let full: HashSet<Cell> = (0..3).flat_map(|c| (0..2).map(move |r| Cell::new(c, r))).collect();
        assert_eq!((g.first_free(&full), g.next_free_after(Cell::new(0, 0), &full), g.nearest_free(Cell::new(0, 0), &full)), (None, None, None));
        // nearest: target (1,0) is taken, (1,1) and (0,0)... (0,0) is taken too; (1,1) is one cell away, (2,0) is one cell away: the earlier wins
        let occ2: HashSet<Cell> = [Cell::new(0, 0), Cell::new(1, 0)].into_iter().collect();
        assert_eq!(g.nearest_free(Cell::new(1, 0), &occ2), Some(Cell::new(1, 1)));
        assert_eq!(g.nearest_free(Cell::new(0, 0), &occ2), Some(Cell::new(0, 1)));
    }

    #[test]
    fn snapping_a_dropped_icon() {
        let g = grid();
        // aligned: the corner at (80, 10) is nearest to cell (1, 0)
        assert_eq!(g.snap(80, 10, true), Some((Cell::new(1, 0), 0, 0)));
        assert_eq!(g.snap(36, 36, true), Some((Cell::new(0, 0), 0, 0)));
        assert_eq!(g.snap(38, 38, true), Some((Cell::new(1, 1), 0, 0)));
        // free: the cell the corner is in, and the offset
        assert_eq!(g.snap(80, 10, false), Some((Cell::new(1, 0), 5, 10)));
        // off the grid on any side: the edge cell, no offset
        assert_eq!(g.snap(-500, -500, false), Some((Cell::new(0, 0), 0, 0)));
        assert_eq!(g.snap(5000, 5000, true), Some((Cell::new(2, 1), 0, 0)));
        assert_eq!(g.snap(5000, 10, false), Some((Cell::new(2, 0), 0, 0)));
    }
}
