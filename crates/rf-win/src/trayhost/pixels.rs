//! Icon pictures, pure: the arithmetic that turns what GDI hands over (a colour bitmap and a mask, both 32-bit BGRA rows read with
//! `GetDIBits`) into the straight-alpha BGRA the model keeps, and shrinks anything larger than [`MAX_ICON_PX`] to fit. The GDI calls
//! themselves are `win::Win32Pixels`; nothing in this file touches Windows.
//!
//! Three kinds of icon exist:
//! * a colour icon WITH an alpha channel (nearly all of them today): the colour bitmap's alpha is used as it is;
//! * a colour icon WITHOUT one (the alpha bytes are all zero): opaque where the mask is black, transparent where it is white;
//! * a monochrome icon (no colour bitmap; the mask bitmap is twice as tall and holds the AND plane over the XOR plane).

use super::model::{IconBitmap, MAX_ICON_PX};

/// The largest source picture that is read at all. An icon bigger than this (a 256 x 256 one is the largest Windows normally
/// makes) is not worth the memory of a copy; the picture is left out and the old one stays.
pub const MAX_SOURCE_PX: u32 = 512;

/// Straight-alpha BGRA from a colour bitmap and (optionally) its mask. Both are `w * h * 4` bytes (the mask as a 32-bit read: a
/// black pixel is 0 in the blue byte, a white one 255). `None` when the sizes do not fit.
pub fn from_color_and_mask(color: &[u8], mask: Option<&[u8]>, w: u32, h: u32) -> Option<Vec<u8>> {
    let n = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if color.len() != n || mask.is_some_and(|m| m.len() != n) {
        return None;
    }
    let mut out = color.to_vec();
    let has_alpha = color.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
    if !has_alpha {
        for (i, p) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let opaque = mask.is_none_or(|m| m[i * 4] == 0);
            p[3] = if opaque { 255 } else { 0 };
        }
    }
    Some(out)
}

/// BGRA from a monochrome icon's mask bitmap: `mask` is `w * 2h * 4` bytes, the AND plane in the upper half and the XOR plane in the
/// lower, each pixel 0 or 255 in the blue byte. AND 1 / XOR 0 is transparent, AND 0 / XOR 0 black, AND 0 / XOR 1 white, and AND 1 /
/// XOR 1 (invert what is behind) is drawn white, which is what an inverting icon shows on a dark taskbar.
pub fn from_monochrome(mask: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let plane = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if mask.len() != plane.checked_mul(2)? {
        return None;
    }
    let (and, xor) = mask.split_at(plane);
    let mut out = vec![0u8; plane];
    for i in 0..(plane / 4) {
        let a = and[i * 4] != 0;
        let x = xor[i * 4] != 0;
        let (v, alpha) = match (a, x) {
            (true, false) => (0, 0),
            (false, false) => (0, 255),
            (false, true) => (255, 255),
            (true, true) => (255, 255),
        };
        out[i * 4..i * 4 + 4].copy_from_slice(&[v, v, v, alpha]);
    }
    Some(out)
}

/// Shrinks a picture so that neither side exceeds `max`, by averaging the source pixels under each target pixel (premultiplied, so a
/// transparent pixel does not bleed its colour). A picture that fits is returned as it is. The aspect ratio is kept.
pub fn shrink_to_fit(bgra: &[u8], w: u32, h: u32, max: u32) -> Option<(u32, u32, Vec<u8>)> {
    if w == 0 || h == 0 || bgra.len() != (w as usize).checked_mul(h as usize)?.checked_mul(4)? {
        return None;
    }
    if w <= max && h <= max {
        return Some((w, h, bgra.to_vec()));
    }
    let scale = f64::from(max) / f64::from(w.max(h));
    let tw = ((f64::from(w) * scale).round() as u32).clamp(1, max);
    let th = ((f64::from(h) * scale).round() as u32).clamp(1, max);
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        // the source rows and columns that fall under this target pixel (at least one)
        let y0 = (u64::from(ty) * u64::from(h) / u64::from(th)) as u32;
        let y1 = ((u64::from(ty + 1) * u64::from(h)).div_ceil(u64::from(th)) as u32).clamp(y0 + 1, h);
        for tx in 0..tw {
            let x0 = (u64::from(tx) * u64::from(w) / u64::from(tw)) as u32;
            let x1 = ((u64::from(tx + 1) * u64::from(w)).div_ceil(u64::from(tw)) as u32).clamp(x0 + 1, w);
            let (mut b, mut g, mut r, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = ((sy * w + sx) * 4) as usize;
                    let al = u64::from(bgra[p + 3]);
                    b += u64::from(bgra[p]) * al;
                    g += u64::from(bgra[p + 1]) * al;
                    r += u64::from(bgra[p + 2]) * al;
                    a += al;
                    n += 1;
                }
            }
            let o = ((ty * tw + tx) * 4) as usize;
            if let (Some(bb), Some(gg), Some(rr)) = (b.checked_div(a), g.checked_div(a), r.checked_div(a)) {
                out[o] = bb as u8;
                out[o + 1] = gg as u8;
                out[o + 2] = rr as u8;
            }
            out[o + 3] = (a / n.max(1)) as u8;
        }
    }
    Some((tw, th, out))
}

/// The bitmap the model keeps, from straight BGRA of any size up to [`MAX_SOURCE_PX`]. `None` for a picture that is empty, larger than
/// that, or whose length does not match.
pub fn to_icon_bitmap(bgra: Vec<u8>, w: u32, h: u32) -> Option<IconBitmap> {
    if w == 0 || h == 0 || w > MAX_SOURCE_PX || h > MAX_SOURCE_PX {
        return None;
    }
    let (tw, th, data) = shrink_to_fit(&bgra, w, h, MAX_ICON_PX)?;
    IconBitmap::new(tw, th, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(b: u8, g: u8, r: u8, a: u8) -> [u8; 4] {
        [b, g, r, a]
    }

    #[test]
    fn a_colour_icon_with_alpha_keeps_its_alpha() {
        let color: Vec<u8> = [px(1, 2, 3, 200), px(4, 5, 6, 0)].concat();
        let mask: Vec<u8> = [px(255, 255, 255, 0), px(255, 255, 255, 0)].concat();
        let out = from_color_and_mask(&color, Some(&mask), 2, 1).unwrap();
        assert_eq!(out, color, "alpha present: the mask is not looked at");
    }

    #[test]
    fn a_colour_icon_without_alpha_takes_it_from_the_mask() {
        let color: Vec<u8> = [px(1, 2, 3, 0), px(4, 5, 6, 0)].concat();
        let mask: Vec<u8> = [px(0, 0, 0, 0), px(255, 255, 255, 0)].concat();
        let out = from_color_and_mask(&color, Some(&mask), 2, 1).unwrap();
        assert_eq!(out[3], 255, "black in the mask is opaque");
        assert_eq!(out[7], 0, "white in the mask is transparent");
        assert_eq!(&out[0..3], &[1, 2, 3]);
        // no mask at all: opaque
        let out = from_color_and_mask(&color, None, 2, 1).unwrap();
        assert_eq!((out[3], out[7]), (255, 255));
    }

    #[test]
    fn wrong_sizes_are_refused() {
        assert!(from_color_and_mask(&[0; 7], None, 2, 1).is_none());
        assert!(from_color_and_mask(&[0; 8], Some(&[0; 4]), 2, 1).is_none());
        assert!(from_monochrome(&[0; 8], 2, 1).is_none(), "the mask must be twice as tall");
        assert!(from_color_and_mask(&[], None, u32::MAX, u32::MAX).is_none(), "no overflow on absurd sizes");
        assert!(shrink_to_fit(&[0; 3], 1, 1, 64).is_none());
        assert!(shrink_to_fit(&[], 0, 0, 64).is_none());
    }

    #[test]
    fn monochrome_planes() {
        // 4 pixels of one row: AND plane then XOR plane
        let and: Vec<u8> = [255u8, 0, 0, 255].iter().flat_map(|&v| px(v, v, v, 0)).collect();
        let xor: Vec<u8> = [0u8, 0, 255, 255].iter().flat_map(|&v| px(v, v, v, 0)).collect();
        let out = from_monochrome(&[and, xor].concat(), 4, 1).unwrap();
        assert_eq!(&out[0..4], &[0, 0, 0, 0], "AND 1, XOR 0: transparent");
        assert_eq!(&out[4..8], &[0, 0, 0, 255], "AND 0, XOR 0: black");
        assert_eq!(&out[8..12], &[255, 255, 255, 255], "AND 0, XOR 1: white");
        assert_eq!(&out[12..16], &[255, 255, 255, 255], "AND 1, XOR 1: inverted, drawn white");
    }

    #[test]
    fn a_picture_that_fits_is_left_alone() {
        let src = vec![9u8; 16 * 16 * 4];
        let (w, h, out) = shrink_to_fit(&src, 16, 16, 64).unwrap();
        assert_eq!((w, h), (16, 16));
        assert_eq!(out, src);
        let (w, h, _) = shrink_to_fit(&vec![0u8; 64 * 64 * 4], 64, 64, 64).unwrap();
        assert_eq!((w, h), (64, 64));
    }

    #[test]
    fn a_big_picture_shrinks_keeping_the_aspect_and_the_colour() {
        let src: Vec<u8> = px(10, 20, 30, 255).iter().copied().cycle().take(256 * 256 * 4).collect();
        let (w, h, out) = shrink_to_fit(&src, 256, 256, 64).unwrap();
        assert_eq!((w, h), (64, 64));
        assert!(out.as_chunks::<4>().0.iter().all(|p| p == &[10, 20, 30, 255]), "a flat picture stays flat");
        // not square
        let src = vec![255u8; 200 * 100 * 4];
        let (w, h, out) = shrink_to_fit(&src, 200, 100, 64).unwrap();
        assert_eq!((w, h), (64, 32));
        assert_eq!(out.len(), 64 * 32 * 4);
        // a very thin one is at least one pixel
        let (w, h, _) = shrink_to_fit(&vec![255u8; 500 * 4], 500, 1, 64).unwrap();
        assert_eq!((w, h), (64, 1));
    }

    #[test]
    fn transparent_pixels_do_not_bleed_their_colour() {
        // 2x1 -> 1x1 would need max 1: one opaque red pixel and one transparent green one
        let src = [px(0, 0, 255, 255), px(0, 255, 0, 0)].concat();
        let (w, h, out) = shrink_to_fit(&src, 2, 1, 1).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(&out[0..3], &[0, 0, 255], "the colour is that of the opaque pixel");
        assert_eq!(out[3], 127, "half the alpha");
    }

    #[test]
    fn to_icon_bitmap_bounds() {
        let b = to_icon_bitmap(vec![255; 128 * 128 * 4], 128, 128).unwrap();
        assert_eq!((b.width, b.height), (64, 64));
        assert!(to_icon_bitmap(vec![0; 513 * 4], 513, 1).is_none(), "beyond the source limit");
        assert!(to_icon_bitmap(vec![], 0, 0).is_none());
        assert!(to_icon_bitmap(vec![0; 10], 4, 4).is_none());
    }
}
