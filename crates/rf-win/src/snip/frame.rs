//! Pictures of the screen held in memory, pure: one captured monitor ([`Frame`]), the capture of all of them ([`Shot`]) and the cut of a
//! region out of it. Everything is BGRA, top row first, tightly packed, alpha 255: the order Windows gives screen pixels in and the
//! order the toolkit uploads textures in, so the overlay can show a frame without converting it.

use rf_core::desktop::geom::Rect;

/// One monitor's picture, at the monitor's own physical size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, B G R A, top row first.
    pub bgra: Vec<u8>,
}

impl Frame {
    /// A frame of one colour (opaque black for the gaps of a cut).
    pub fn filled(width: u32, height: u32, bgra: [u8; 4]) -> Frame {
        let mut v = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width as usize * height as usize {
            v.extend_from_slice(&bgra);
        }
        Frame { width, height, bgra: v }
    }

    /// Whether the buffer is the size the width and the height say.
    pub fn is_consistent(&self) -> bool {
        self.bgra.len() == self.width as usize * self.height as usize * 4 && self.width > 0 && self.height > 0
    }

    /// The pixel at (x, y) as `[b, g, r, a]`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.bgra[i], self.bgra[i + 1], self.bgra[i + 2], self.bgra[i + 3]]
    }
}

/// The capture of every monitor taken at one moment: each frame with the rectangle it covers in the virtual screen (physical pixels, a
/// frame is exactly as large as its rectangle).
#[derive(Clone, Debug, Default)]
pub struct Shot {
    pub parts: Vec<(Rect, Frame)>,
}

/// One monitor's picture borrowed from wherever it lives (the app keeps the pixels inside the textures it shows, and cuts from there:
/// a second copy of a 4K screen would be 33 MB for nothing).
#[derive(Clone, Copy, Debug)]
pub struct Part<'a> {
    pub rect: Rect,
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, B G R A, top row first.
    pub bgra: &'a [u8],
}

impl Part<'_> {
    fn is_consistent(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.bgra.len() == self.width as usize * self.height as usize * 4
            && self.width as i32 == self.rect.width()
            && self.height as i32 == self.rect.height()
    }
}

/// The copy of `region` (virtual-screen pixels) as one picture. Where the region lies over a monitor its pixels are copied as they are;
/// where it lies over no monitor (the virtual screen is the monitors' bounding box and two monitors of different heights leave a corner
/// empty) the picture is opaque black. `None` for an empty region or a part that is not the size of its rectangle.
pub fn crop_parts(parts: &[Part<'_>], region: Rect) -> Option<Frame> {
    if region.is_empty() || parts.iter().any(|p| !p.is_consistent()) {
        return None;
    }
    let w = region.width() as usize;
    let mut out = Frame::filled(region.width() as u32, region.height() as u32, [0, 0, 0, 255]);
    for p in parts {
        let cut = p.rect.intersect(&region);
        if cut.is_empty() {
            continue;
        }
        let run = cut.width() as usize * 4;
        for y in cut.top..cut.bottom {
            let src = ((y - p.rect.top) as usize * p.width as usize + (cut.left - p.rect.left) as usize) * 4;
            let dst = ((y - region.top) as usize * w + (cut.left - region.left) as usize) * 4;
            out.bgra[dst..dst + run].copy_from_slice(&p.bgra[src..src + run]);
        }
    }
    Some(out)
}

impl Shot {
    /// [`crop_parts`] over the frames of this capture.
    pub fn crop(&self, region: Rect) -> Option<Frame> {
        let parts: Vec<Part<'_>> = self.parts.iter().map(|(rect, f)| Part { rect: *rect, width: f.width, height: f.height, bgra: &f.bgra }).collect();
        crop_parts(&parts, region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame whose pixel at (x, y) says where it is: b = x low byte, g = y low byte, r = `tag`.
    fn marked(w: u32, h: u32, tag: u8) -> Frame {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[x as u8, y as u8, tag, 255]);
            }
        }
        Frame { width: w, height: h, bgra: v }
    }

    fn two() -> Shot {
        // a 200x100 primary and a 120x300 monitor left of it and higher, like the portrait one in the geometry tests but small
        Shot { parts: vec![(Rect::new(0, 0, 200, 100), marked(200, 100, 1)), (Rect::new(-120, -50, 0, 250), marked(120, 300, 2))] }
    }

    #[test]
    fn a_region_inside_one_monitor_is_copied_pixel_for_pixel() {
        let s = two();
        let f = s.crop(Rect::new(10, 20, 60, 50)).unwrap();
        assert_eq!((f.width, f.height), (50, 30));
        assert_eq!(f.pixel(0, 0), [10, 20, 1, 255]);
        assert_eq!(f.pixel(49, 29), [59, 49, 1, 255]);
    }

    #[test]
    fn a_region_across_two_monitors_takes_each_part_from_its_own_picture() {
        let s = two();
        // from x = -20 to x = 30, y = 0..40: the left 20 columns are the left monitor's (its x runs 100..120 there, its y is 50..90)
        let f = s.crop(Rect::new(-20, 0, 30, 40)).unwrap();
        assert_eq!((f.width, f.height), (50, 40));
        assert_eq!(f.pixel(0, 0), [100, 50, 2, 255]);
        assert_eq!(f.pixel(19, 39), [119, 89, 2, 255]);
        assert_eq!(f.pixel(20, 0), [0, 0, 1, 255], "the primary's first column");
        assert_eq!(f.pixel(49, 39), [29, 39, 1, 255]);
    }

    #[test]
    fn the_corner_no_monitor_covers_is_opaque_black_and_the_sizes_are_in_device_pixels() {
        let s = two();
        // below the primary (which ends at y = 100) and right of the left monitor: nothing there
        let f = s.crop(Rect::new(-10, 90, 20, 130)).unwrap();
        assert_eq!((f.width, f.height), (30, 40));
        assert_eq!(f.pixel(0, 0), [110, 140, 2, 255], "left monitor, still on it");
        assert_eq!(f.pixel(10, 5), [0, 95, 1, 255], "the primary, its last rows");
        assert_eq!(f.pixel(10, 20), [0, 0, 0, 255], "below the primary: black");
        assert_eq!(f.pixel(29, 39), [0, 0, 0, 255]);
    }

    #[test]
    fn a_region_the_picture_cannot_fill_is_refused() {
        let s = two();
        assert_eq!(s.crop(Rect::new(5, 5, 5, 50)), None, "empty");
        let broken = Shot { parts: vec![(Rect::new(0, 0, 10, 10), marked(9, 10, 0))] };
        assert_eq!(broken.crop(Rect::new(0, 0, 5, 5)), None, "a frame that is not the size of its rectangle");
        let short = Shot { parts: vec![(Rect::new(0, 0, 2, 2), Frame { width: 2, height: 2, bgra: vec![0; 15] })] };
        assert_eq!(short.crop(Rect::new(0, 0, 1, 1)), None);
    }

    #[test]
    fn a_whole_monitor_is_its_frame() {
        let s = two();
        assert_eq!(s.crop(Rect::new(0, 0, 200, 100)).unwrap(), marked(200, 100, 1));
    }
}
