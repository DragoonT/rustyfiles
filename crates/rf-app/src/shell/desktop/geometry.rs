//! Window pixels <-> canvas pixels. Pure.
//!
//! THE CANVAS. The layout store keeps cells per monitor, in the grid of that monitor at its own size and scale, so the view draws a
//! CANVAS: one monitor's surface in physical pixels. The real desktop (shell plan F4) shows a canvas in a window that IS the monitor,
//! so the two sizes are equal and the shown scale is 1. The preview window is an ordinary window of any size: it shows the same
//! canvas, and a canvas that does not fit is SCALED DOWN to fit, never scrolled (the real desktop never scrolls; a scrolled preview
//! would show something the desktop can not). The view says so in its status line.
//!
//! A canvas that is smaller than the window is shown at scale 1, anchored at the top-left corner like a monitor's own surface; the
//! rest of the window is empty.
//!
//! UNITS. GPUI lays out in logical pixels: physical / `window_scale`. The canvas is in the physical pixels of the monitor it stands
//! for. One canvas pixel is therefore `shown / window_scale` logical pixels long, with `shown` between 0 and 1.

use rf_core::desktop::geom::{Rect, Size};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    pub canvas: Size,
    /// How much of its own size the canvas is shown at (1.0 or less).
    pub shown: f32,
    /// The window's own scale factor (1.0 at 100 %, 1.5 at 150 %).
    pub window_scale: f32,
}

impl Mapping {
    /// `window` is the window's content size in logical pixels.
    pub fn new(canvas: Size, window: (f32, f32), window_scale: f32) -> Mapping {
        let sf = if window_scale.is_finite() && window_scale > 0.0 { window_scale } else { 1.0 };
        let (ww, wh) = (window.0 * sf, window.1 * sf);
        let shown = if canvas.is_empty() || !(ww > 0.0 && wh > 0.0) {
            1.0
        } else {
            (ww / canvas.w as f32).min(wh / canvas.h as f32).min(1.0)
        };
        Mapping { canvas, shown, window_scale: sf }
    }

    /// Logical pixels per canvas pixel.
    pub fn unit(&self) -> f32 {
        self.shown / self.window_scale
    }

    /// The canvas does not fit at its own size.
    pub fn scaled_down(&self) -> bool {
        self.shown < 0.9995
    }

    /// 100 when the canvas is shown whole-size, 62 when it is shown at 62 %.
    pub fn percent(&self) -> u32 {
        (self.shown * 100.0).round() as u32
    }

    /// A point of the window (logical pixels) as a canvas pixel. Outside the canvas the result is outside `0..canvas`.
    pub fn to_canvas(&self, x: f32, y: f32) -> (i32, i32) {
        let u = self.unit();
        ((x / u).floor() as i32, (y / u).floor() as i32)
    }

    /// A canvas pixel as a window point (logical pixels).
    pub fn to_window(&self, cx: i32, cy: i32) -> (f32, f32) {
        let u = self.unit();
        (cx as f32 * u, cy as f32 * u)
    }

    /// A canvas length as logical pixels.
    pub fn len(&self, canvas_px: f32) -> f32 {
        canvas_px * self.unit()
    }

    /// (left, top, width, height) in logical pixels of a canvas rectangle.
    pub fn rect(&self, r: Rect) -> (f32, f32, f32, f32) {
        let (x, y) = self.to_window(r.left, r.top);
        (x, y, self.len(r.width() as f32), self.len(r.height() as f32))
    }

    /// The canvas as it is shown, in logical pixels: its width and height.
    pub fn shown_size(&self) -> (f32, f32) {
        (self.len(self.canvas.w as f32), self.len(self.canvas.h as f32))
    }

    /// The canvas rectangle the pixel falls in or touches, clamped to the canvas (for a drag that leaves it).
    pub fn clamp_to_canvas(&self, p: (i32, i32)) -> (i32, i32) {
        (p.0.clamp(0, self.canvas.w as i32 - 1), p.1.clamp(0, self.canvas.h as i32 - 1))
    }

    pub fn contains(&self, p: (i32, i32)) -> bool {
        p.0 >= 0 && p.1 >= 0 && (p.0 as u32) < self.canvas.w && (p.1 as u32) < self.canvas.h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FHD: Size = Size::new(1920, 1080);

    #[test]
    fn a_window_as_large_as_the_canvas_shows_it_whole_at_100_percent() {
        let m = Mapping::new(FHD, (1920.0, 1080.0), 1.0);
        assert_eq!((m.shown, m.percent(), m.scaled_down()), (1.0, 100, false));
        assert_eq!(m.to_canvas(100.0, 200.0), (100, 200));
        assert_eq!(m.to_window(100, 200), (100.0, 200.0));
        assert_eq!(m.rect(Rect::new(75, 150, 150, 225)), (75.0, 150.0, 75.0, 75.0));
    }

    #[test]
    fn at_150_percent_a_logical_pixel_is_one_and_a_half_physical_ones() {
        // a 1920x1080 logical window on a 150 % screen is 2880x1620 physical: it holds a canvas of that size whole
        let canvas = Size::new(2880, 1620);
        let m = Mapping::new(canvas, (1920.0, 1080.0), 1.5);
        assert_eq!((m.shown, m.scaled_down()), (1.0, false));
        assert_eq!(m.to_canvas(100.0, 100.0), (150, 150));
        assert_eq!(m.to_window(150, 150), (100.0, 100.0));
        // and a 1920x1080 canvas at the same scale is shown whole, 1280x720 logical, with room to spare
        let m = Mapping::new(FHD, (1920.0, 1080.0), 1.5);
        assert_eq!(m.shown, 1.0);
        assert_eq!(m.shown_size(), (1280.0, 720.0));
    }

    #[test]
    fn a_window_smaller_than_the_canvas_scales_it_down_to_fit_by_the_tighter_side() {
        let m = Mapping::new(FHD, (960.0, 800.0), 1.0);
        assert_eq!(m.shown, 0.5); // the width is the tight side
        assert!(m.scaled_down());
        assert_eq!(m.percent(), 50);
        assert_eq!(m.to_canvas(480.0, 270.0), (960, 540));
        assert_eq!(m.to_window(1920, 1080), (960.0, 540.0));
        let m = Mapping::new(FHD, (1920.0, 540.0), 1.0);
        assert_eq!(m.shown, 0.5); // now the height
        // at 150 %: a 1000x600 logical window is 1500x900 physical
        let m = Mapping::new(FHD, (1000.0, 600.0), 1.5);
        assert!((m.shown - 1500.0 / 1920.0).abs() < 1e-6, "{}", m.shown); // 1500 x 900 physical: the width is the tight side
        let back = m.to_window(m.to_canvas(500.0, 300.0).0, m.to_canvas(500.0, 300.0).1);
        assert!((back.0 - 500.0).abs() < 2.0 && (back.1 - 300.0).abs() < 2.0, "{back:?}");
    }

    #[test]
    fn a_larger_window_does_not_enlarge_the_canvas() {
        let m = Mapping::new(FHD, (4000.0, 3000.0), 1.0);
        assert_eq!((m.shown, m.scaled_down()), (1.0, false));
        assert!(!m.contains(m.to_canvas(3000.0, 100.0)), "beyond the canvas is outside it");
    }

    #[test]
    fn degenerate_inputs_do_not_divide_by_zero() {
        for m in [
            Mapping::new(Size::new(0, 0), (100.0, 100.0), 1.0),
            Mapping::new(FHD, (0.0, 0.0), 1.0),
            Mapping::new(FHD, (100.0, 100.0), 0.0),
            Mapping::new(FHD, (f32::NAN, 100.0), f32::NAN),
        ] {
            assert!(m.shown > 0.0 && m.unit().is_finite() && m.unit() > 0.0, "{m:?}");
        }
    }

    #[test]
    fn clamping_and_containment() {
        let m = Mapping::new(FHD, (1920.0, 1080.0), 1.0);
        assert_eq!(m.clamp_to_canvas((-50, 5000)), (0, 1079));
        assert!(m.contains((0, 0)) && m.contains((1919, 1079)) && !m.contains((1920, 0)) && !m.contains((-1, 0)));
    }
}
