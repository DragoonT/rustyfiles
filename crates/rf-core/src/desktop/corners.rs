//! The rounded corners of the desktop's wallpaper (a setting, off by default: the user wanted to see how it looks beside the dock's
//! round islands, and then asked for ALL the corners, also the ones where two monitors meet).
//!
//! What is left to work out is the bottom edge. A cover window is its monitor less the bottom pixel row (`cover::geometry`: a window
//! that reaches every edge of its monitor is taken for a full-screen program), and in that row Explorer's own desktop shows. Nobody
//! sees it while the wallpaper is square, because the row under ours is the same picture. Under a rounded corner, which is black in
//! our window, it shows as a short line of wallpaper (the user saw it at once: "altta 1-2 piksel arkaplan görselinin kayması"). The
//! rows the window leaves out are covered there by small black windows of their own (`rf_win::desktop::cornerpatch`); this file says
//! where they are. Pure.

use super::geom::Rect;

/// How many pixels of a pixel row lie outside a rounded corner of `radius` (device px), counted from the side edge the corner is on.
/// `from_edge`: how far the row is from the corner's top or bottom edge (0: the row on the edge). The middle of the row is measured,
/// and a pixel that is more than half outside counts.
pub fn black_in_row(radius: i32, from_edge: i32) -> i32 {
    if radius <= 0 || from_edge < 0 {
        return 0;
    }
    let r = f64::from(radius);
    let dy = r - (f64::from(from_edge) + 0.5);
    if dy <= 0.0 {
        return 0;
    }
    (r - (r * r - dy * dy).sqrt()).round() as i32
}

/// The black rectangles (virtual-screen device px) under the two bottom corners of `monitor`, for the `rows` bottom pixel rows its
/// cover window leaves out: one for each row and corner, as wide as the corner is black in that row. Nothing for square corners.
pub fn bottom_patches(monitor: Rect, radius: i32, rows: i32) -> Vec<Rect> {
    let mut out = Vec::new();
    for k in 0..rows.max(0) {
        // (two corners never meet in the middle: a radius larger than half the monitor is not a corner any more)
        let w = black_in_row(radius, k).min(monitor.width() / 2);
        if w <= 0 || monitor.height() <= k {
            continue;
        }
        let (top, bottom) = (monitor.bottom - k - 1, monitor.bottom - k);
        out.push(Rect::new(monitor.left, top, monitor.left + w, bottom));
        out.push(Rect::new(monitor.right - w, top, monitor.right, bottom));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bottom_row_of_a_corner_is_black_for_most_of_the_radius() {
        // r - sqrt(r - 0.25): 12 -> 8.57, 8 -> 5.22, 16 -> 12.03, 24 -> 19.13, 18 (12 at 150 %) -> 13.79
        assert_eq!(black_in_row(12, 0), 9);
        assert_eq!(black_in_row(8, 0), 5);
        assert_eq!(black_in_row(16, 0), 12);
        assert_eq!(black_in_row(24, 0), 19);
        assert_eq!(black_in_row(18, 0), 14);
        // square corners, and rows the corner does not reach
        assert_eq!(black_in_row(0, 0), 0);
        assert_eq!(black_in_row(-4, 0), 0);
        assert_eq!(black_in_row(12, 12), 0);
        assert_eq!(black_in_row(12, 40), 0);
        assert_eq!(black_in_row(12, -1), 0);
    }

    #[test]
    fn the_corner_gets_narrower_row_by_row_away_from_the_edge() {
        let widths: Vec<i32> = (0..12).map(|k| black_in_row(12, k)).collect();
        assert!(widths.windows(2).all(|w| w[0] >= w[1]), "{widths:?}");
        assert_eq!(widths[11], 0, "the row at the arc's own height is whole");
        assert!(widths[0] < 12, "never the whole radius: the arc ends on the edge");
    }

    #[test]
    fn the_patches_lie_in_the_rows_the_cover_window_leaves_out() {
        // the user's main monitor, 12 px, one row left out: nine pixels at each end of the last row
        let wide = Rect::new(0, 0, 2560, 1440);
        assert_eq!(bottom_patches(wide, 12, 1), [Rect::new(0, 1439, 9, 1440), Rect::new(2551, 1439, 2560, 1440)]);
        // a monitor that is not at the origin (the user's second one, to the right and reaching above the first)
        let tall = Rect::new(2560, -618, 4000, 1942);
        assert_eq!(bottom_patches(tall, 12, 1), [Rect::new(2560, 1941, 2569, 1942), Rect::new(3991, 1941, 4000, 1942)]);
        // two rows left out: the second row is narrower
        let two = bottom_patches(wide, 12, 2);
        assert_eq!(two.len(), 4);
        assert_eq!(two[2], Rect::new(0, 1438, black_in_row(12, 1), 1439));
        assert!(two[2].width() < two[0].width());
    }

    #[test]
    fn square_corners_and_no_missing_row_need_no_patch() {
        let m = Rect::new(0, 0, 1920, 1080);
        assert!(bottom_patches(m, 0, 1).is_empty());
        assert!(bottom_patches(m, 12, 0).is_empty());
        assert!(bottom_patches(m, 12, -3).is_empty());
        assert!(bottom_patches(Rect::default(), 12, 1).is_empty(), "no monitor, nothing to patch");
        // a radius as large as the monitor still leaves the two patches apart
        let small = Rect::new(0, 0, 20, 20);
        let p = bottom_patches(small, 500, 1);
        assert!(p.iter().all(|r| r.width() <= 10), "{p:?}");
    }
}
