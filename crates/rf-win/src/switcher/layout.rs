//! Where the cards of the switcher go. Pure arithmetic: a number of windows and a monitor's work area (device px) and scale in, the
//! panel's rectangle and every card's rectangles out. The view draws at these positions and DWM draws the live pictures at the same
//! ones, so there is one source for both.
//!
//! THE DESIGN (logical px at full size): a card is 224 x 176: a 204 x 124 box for the picture 10 px inside the card, and under it a
//! 24 px row with the app's small icon and the title. Cards are 12 px apart and the panel keeps 24 px around them. Cards wrap into
//! rows, the number of columns is the widest that fits and then evened out (12 windows are 2 rows of 6, not 10 and 2). The panel
//! takes at most 92 % of the work area's width and 80 % of its height. Up to about 12 windows fit at full size on an ordinary
//! monitor; more scale the CARDS down (not the gaps), in steps of 5 %, to half the full size at the least. Beyond what fits at half
//! size the oldest windows are not shown ([`Layout::visible`]).
//!
//! The last row is centred when it is not full.

use crate::dock::layout::{Rect, px};

pub const CARD_W: f32 = 224.0;
pub const CARD_H: f32 = 176.0;
/// Space between the card's edge and its picture box / row, logical px.
pub const INSET: f32 = 10.0;
/// The picture box's height; its width is the card's less two insets.
pub const THUMB_H: f32 = 124.0;
/// The row under the picture: the small icon and the title.
pub const ROW_H: f32 = 24.0;
pub const GAP: f32 = 12.0;
pub const PAD: f32 = 24.0;
pub const MAX_WIDTH_SHARE: f32 = 0.92;
pub const MAX_HEIGHT_SHARE: f32 = 0.80;
pub const MIN_FACTOR: f32 = 0.5;
const FACTOR_STEP: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Where the panel's client area goes, device px, screen coordinates, centred in the work area.
    pub window: Rect,
    pub cols: usize,
    pub rows: usize,
    /// The cards shown (all of them unless even half size does not fit).
    pub visible: usize,
    /// Card size relative to full size.
    pub factor: f32,
    /// Device px per logical px.
    pub scale: f32,
    card_w: i32,
    card_h: i32,
    gap: i32,
    pad: i32,
    inset: i32,
    thumb_h: i32,
    row_h: i32,
    /// The number of cards in the last row (1..=cols).
    last_row: usize,
}

fn columns_for(n: usize, avail_w: f32, card_w: f32) -> usize {
    let fit = (((avail_w + GAP) / (card_w + GAP)).floor() as usize).clamp(1, n.max(1));
    // even the rows out: the fewest columns that keep the same number of rows
    let rows = n.div_ceil(fit).max(1);
    n.div_ceil(rows).max(1)
}

/// The layout of `n` cards in `work` (device px) at `scale`.
pub fn plan(n: usize, work: Rect, scale: f32) -> Layout {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let n = n.max(1);
    let (work_w, work_h) = (work.width() as f32 / scale, work.height() as f32 / scale);
    let (max_w, max_h) = (work_w * MAX_WIDTH_SHARE, work_h * MAX_HEIGHT_SHARE);
    let avail_w = max_w - 2.0 * PAD;
    let mut factor = 1.0f32;
    let (mut cols, mut rows);
    loop {
        let cw = CARD_W * factor;
        cols = columns_for(n, avail_w, cw);
        rows = n.div_ceil(cols);
        let height = rows as f32 * CARD_H * factor + (rows - 1) as f32 * GAP + 2.0 * PAD;
        let width = cols as f32 * cw + (cols - 1) as f32 * GAP + 2.0 * PAD;
        if (height <= max_h && width <= max_w) || factor <= MIN_FACTOR + 1e-4 {
            break;
        }
        factor = (factor - FACTOR_STEP).max(MIN_FACTOR);
    }
    let card_w = px(CARD_W * factor, scale);
    let card_h = px(CARD_H * factor, scale);
    let (gap, pad) = (px(GAP, scale), px(PAD, scale));
    // what fits at this size, when even half size is not enough
    let mut visible = n;
    let height_of = |r: usize| r as f32 * CARD_H * factor + (r.saturating_sub(1)) as f32 * GAP + 2.0 * PAD;
    if height_of(rows) > max_h {
        let mut r = rows;
        while r > 1 && height_of(r) > max_h {
            r -= 1;
        }
        rows = r;
        visible = (rows * cols).min(n);
    }
    let last_row = visible - (rows - 1) * cols;
    let w = cols as i32 * card_w + (cols as i32 - 1) * gap + 2 * pad;
    let h = rows as i32 * card_h + (rows as i32 - 1) * gap + 2 * pad;
    let left = work.left + (work.width() - w) / 2;
    let top = work.top + (work.height() - h) / 2;
    Layout {
        window: Rect::from_xywh(left, top, w, h),
        cols,
        rows,
        visible,
        factor,
        scale,
        card_w,
        card_h,
        gap,
        pad,
        inset: px(INSET * factor, scale),
        thumb_h: px(THUMB_H * factor, scale),
        row_h: px(ROW_H * factor, scale),
        last_row,
    }
}

impl Layout {
    /// Card `i`'s rectangle in the panel's client coordinates (device px).
    pub fn card(&self, i: usize) -> Rect {
        let (row, col) = (i / self.cols, i % self.cols);
        let in_row = if row + 1 == self.rows { self.last_row } else { self.cols };
        // a row that is not full is centred
        let shift = (self.cols - in_row) as i32 * (self.card_w + self.gap) / 2;
        let x = self.pad + shift + col as i32 * (self.card_w + self.gap);
        let y = self.pad + row as i32 * (self.card_h + self.gap);
        Rect::from_xywh(x, y, self.card_w, self.card_h)
    }

    /// The box the live picture (or the large icon) goes in, panel client coordinates.
    pub fn picture(&self, i: usize) -> Rect {
        let c = self.card(i);
        Rect::from_xywh(c.left + self.inset, c.top + self.inset, c.width() - 2 * self.inset, self.thumb_h)
    }

    /// The row with the small icon and the title.
    pub fn label(&self, i: usize) -> Rect {
        let c = self.card(i);
        let top = c.top + self.inset + self.thumb_h + px(6.0 * self.factor, self.scale);
        Rect::from_xywh(c.left + self.inset, top, c.width() - 2 * self.inset, self.row_h)
    }

    /// The small icon's side, device px.
    pub fn small_icon(&self) -> i32 {
        px(20.0 * self.factor, self.scale)
    }

    /// The large icon shown in place of the picture of a minimized window, device px.
    pub fn large_icon(&self) -> i32 {
        px(64.0 * self.factor, self.scale)
    }

    /// The card under a point of the panel's client area.
    pub fn card_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.visible).find(|&i| self.card(i).contains(x, y))
    }

    /// The panel's size, logical px (what the window is created with).
    pub fn window_logical(&self) -> (f32, f32) {
        (self.window.width() as f32 / self.scale, self.window.height() as f32 / self.scale)
    }
}

/// The rectangle of a picture of `src` (width, height) drawn inside `boxr`: as large as fits without changing its proportions and
/// without growing past its own size, centred. An empty source gives the whole box.
pub fn fit_picture(src: (i32, i32), boxr: Rect) -> Rect {
    if src.0 <= 0 || src.1 <= 0 {
        return boxr;
    }
    let k = (boxr.width() as f32 / src.0 as f32).min(boxr.height() as f32 / src.1 as f32).min(1.0);
    let (w, h) = (((src.0 as f32 * k).round() as i32).max(1), ((src.1 as f32 * k).round() as i32).max(1));
    Rect::from_xywh(boxr.left + (boxr.width() - w) / 2, boxr.top + (boxr.height() - h) / 2, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FHD: Rect = Rect::new(0, 0, 1920, 1032);
    const QHD: Rect = Rect::new(0, 0, 2560, 1392);

    #[test]
    fn a_few_windows_make_one_row_at_full_size() {
        let l = plan(4, FHD, 1.0);
        assert_eq!((l.cols, l.rows, l.visible), (4, 1, 4));
        assert_eq!(l.factor, 1.0);
        assert_eq!(l.card(0).width(), 224);
        assert_eq!(l.card(0).height(), 176);
    }

    #[test]
    fn twelve_windows_fit_at_full_size_on_an_ordinary_monitor() {
        for (work, scale) in [(FHD, 1.0), (QHD, 1.0), (Rect::new(0, 0, 2880, 1560), 1.5), (QHD, 1.25)] {
            let l = plan(12, work, scale);
            assert_eq!(l.factor, 1.0, "12 windows on {work:?} at {scale}");
            assert_eq!(l.visible, 12);
            assert!(work.covers(&l.window), "{:?} in {work:?}", l.window);
        }
    }

    #[test]
    fn rows_are_evened_out() {
        // 1920 wide: 7 columns would fit; 12 windows are two rows of 6, not 7 and 5
        let l = plan(12, FHD, 1.0);
        assert_eq!((l.cols, l.rows), (6, 2));
        // 8 on a wide monitor: one row if it fits
        let l = plan(8, QHD, 1.0);
        assert_eq!((l.cols, l.rows), (8, 1));
    }

    #[test]
    fn more_windows_scale_the_cards_down() {
        let l = plan(40, FHD, 1.0);
        assert!(l.factor < 1.0 && l.factor >= MIN_FACTOR, "{}", l.factor);
        assert_eq!(l.visible, 40);
        assert!(l.window.width() as f32 <= 1920.0 * MAX_WIDTH_SHARE + 1.0);
        assert!(l.window.height() as f32 <= 1032.0 * MAX_HEIGHT_SHARE + 1.0);
        assert!(l.card(0).width() < 224);
    }

    #[test]
    fn beyond_half_size_the_oldest_are_not_shown() {
        let l = plan(400, FHD, 1.0);
        assert_eq!(l.factor, MIN_FACTOR);
        assert!(l.visible < 400 && l.visible > 12, "{}", l.visible);
        assert!(FHD.covers(&l.window));
        assert!(l.window.height() as f32 <= 1032.0 * MAX_HEIGHT_SHARE + 1.0);
    }

    #[test]
    fn a_small_screen_scales_down_for_twelve() {
        let l = plan(12, Rect::new(0, 0, 1280, 680), 1.0);
        assert!(l.factor < 1.0);
        assert_eq!(l.visible, 12);
    }

    #[test]
    fn the_panel_is_centred_in_the_work_area_including_a_negative_origin() {
        let work = Rect::new(-1080, -300, 0, 1620); // a portrait monitor left of the primary
        let l = plan(3, work, 1.0);
        let (cx, cy) = ((l.window.left + l.window.right) / 2, (l.window.top + l.window.bottom) / 2);
        assert!((cx - (work.left + work.right) / 2).abs() <= 1);
        assert!((cy - (work.top + work.bottom) / 2).abs() <= 1);
        assert!(work.covers(&l.window));
    }

    #[test]
    fn cards_do_not_overlap_and_stay_inside_the_panel() {
        for n in [1, 2, 5, 7, 12, 13, 30] {
            let l = plan(n, FHD, 1.0);
            let client = Rect::new(0, 0, l.window.width(), l.window.height());
            for i in 0..l.visible {
                assert!(client.covers(&l.card(i)), "card {i} of {n}");
                assert!(l.card(i).covers(&l.picture(i)) && l.card(i).covers(&l.label(i)), "card {i} of {n}");
                for j in i + 1..l.visible {
                    let (a, b) = (l.card(i), l.card(j));
                    let apart = a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top;
                    assert!(apart, "cards {i} and {j} of {n} overlap: {a:?} {b:?}");
                }
            }
        }
    }

    #[test]
    fn the_picture_box_is_above_the_label_row() {
        let l = plan(6, FHD, 1.0);
        assert!(l.picture(0).bottom <= l.label(0).top);
    }

    #[test]
    fn a_short_last_row_is_centred() {
        // 7 cards, 4 columns -> rows of 4 and 3 (7 on a width that fits 4)
        let work = Rect::new(0, 0, 1100, 1000);
        let l = plan(7, work, 1.0);
        assert_eq!(l.cols, 4);
        let first_row_left = l.card(0).left;
        let last_row_left = l.card(4).left;
        assert!(last_row_left > first_row_left, "{last_row_left} vs {first_row_left}");
        // symmetric: the space left of the row equals the space right of it
        let client_w = l.window.width();
        let left_space = l.card(4).left;
        let right_space = client_w - l.card(6).right;
        assert!((left_space - right_space).abs() <= 1, "{left_space} vs {right_space}");
    }

    #[test]
    fn at_150_percent_the_numbers_scale_once() {
        let l = plan(3, Rect::new(0, 0, 2880, 1500), 1.5);
        assert_eq!(l.card(0).width(), 336);
        assert_eq!(l.card(0).height(), 264);
        assert_eq!(l.window_logical().0, l.window.width() as f32 / 1.5);
    }

    #[test]
    fn a_card_is_found_by_a_point_inside_it_and_not_by_the_gap() {
        let l = plan(4, FHD, 1.0);
        let c1 = l.card(1);
        assert_eq!(l.card_at(c1.left + 5, c1.top + 5), Some(1));
        let gap_x = l.card(0).right + 2;
        assert_eq!(l.card_at(gap_x, l.card(0).top + 10), None);
        assert_eq!(l.card_at(-5, -5), None);
    }

    #[test]
    fn zero_windows_and_a_bad_scale_do_not_panic() {
        let l = plan(0, FHD, 0.0);
        assert_eq!(l.visible, 1);
        assert!(l.window.width() > 0);
    }

    #[test]
    fn a_picture_keeps_its_proportions_and_never_grows_past_its_size() {
        let b = Rect::from_xywh(100, 100, 204, 124);
        // a wide window is limited by the width
        let r = fit_picture((1920, 1080), b);
        assert_eq!(r.width(), 204);
        assert!((r.height() as f32 - 204.0 * 1080.0 / 1920.0).abs() <= 1.0);
        assert!(b.covers(&r));
        // a tall one by the height
        let r = fit_picture((600, 1200), b);
        assert_eq!(r.height(), 124);
        // a small one stays small and centred
        let r = fit_picture((100, 50), b);
        assert_eq!((r.width(), r.height()), (100, 50));
        assert_eq!((r.left + r.right) / 2, (b.left + b.right) / 2);
        // nothing known: the whole box
        assert_eq!(fit_picture((0, 0), b), b);
    }
}
