//! Our own resampling for the dock's icons: a Lanczos-3 filter run on premultiplied colour, and a colour bleed into the transparent
//! pixels afterwards. Pure: bitmaps in, bitmaps out.
//!
//! WHY NOT THE TOOLKIT'S SAMPLING. GPUI draws a picture with one bilinear tap per pixel, no mip chain, and its blend multiplies the
//! colour by the alpha AFTER filtering (the pictures are straight alpha). Two things follow. A picture drawn much smaller than it is
//! shimmers and looks soft (a bilinear tap skips most of the source pixels), so we keep pictures at the sizes they are drawn at
//! (`iconpick::level_sizes`) and make each one here with a filter that keeps the edges. And the colour of a fully transparent pixel
//! leaks into its neighbours when the sampler blends the two: black there makes a dark fringe around every icon, so the transparent
//! pixels next to the picture are given the colour of the picture ([`bleed`]).

use crate::thumbs::Bgra;

/// The Lanczos window: 3 lobes each side.
const LOBES: f32 = 3.0;

fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-6 {
        1.0
    } else {
        let p = std::f32::consts::PI * x;
        p.sin() / p
    }
}

fn lanczos(x: f32) -> f32 {
    let x = x.abs();
    if x >= LOBES { 0.0 } else { sinc(x) * sinc(x / LOBES) }
}

/// The weights of one output position along one axis: the first source index and the (normalised) weights from there on.
struct Taps {
    start: usize,
    weights: Vec<f32>,
}

/// The filter taps for resizing a line of `src_len` samples to `dst_len`. When reducing, the filter is stretched by the ratio so every
/// source sample counts (that is what a box filter does too, with a better shape); when enlarging it is the plain interpolation.
fn taps(src_len: usize, dst_len: usize) -> Vec<Taps> {
    let ratio = src_len as f32 / dst_len as f32;
    let stretch = ratio.max(1.0);
    let support = LOBES * stretch;
    (0..dst_len)
        .map(|i| {
            let centre = (i as f32 + 0.5) * ratio;
            let left = (centre - support).floor().max(0.0) as usize;
            let right = ((centre + support).ceil().max(0.0) as usize).min(src_len).max(left + 1).min(src_len);
            let left = left.min(src_len - 1);
            let mut weights: Vec<f32> = (left..right.max(left + 1)).map(|j| lanczos((j as f32 + 0.5 - centre) / stretch)).collect();
            let sum: f32 = weights.iter().sum();
            if sum.abs() > 1e-6 {
                weights.iter_mut().for_each(|w| *w /= sum);
            } else {
                // (cannot happen with a window this wide; the nearest sample is the safe answer)
                let last = left + weights.len() - 1;
                weights.iter_mut().for_each(|w| *w = 0.0);
                weights[(centre as usize).clamp(left, last) - left] = 1.0;
            }
            Taps { start: left, weights }
        })
        .collect()
}

/// For each output position along one axis, the source samples `[lo, hi)` within one filter stretch (one output pixel) of its centre:
/// the footprint of the main lobe. The filter's response may not leave the range of the source's alpha over this footprint ([`resize`]).
fn footprints(src_len: usize, dst_len: usize) -> Vec<(usize, usize)> {
    let ratio = src_len as f32 / dst_len as f32;
    let stretch = ratio.max(1.0);
    (0..dst_len)
        .map(|i| {
            let centre = (i as f32 + 0.5) * ratio;
            let lo = ((centre - stretch).floor().max(0.0) as usize).min(src_len - 1);
            let hi = ((centre + stretch).ceil().max(0.0) as usize).min(src_len).max(lo + 1);
            (lo, hi)
        })
        .collect()
}

/// Scales `src` to `dw` x `dh`. The filter runs on colour multiplied by alpha, so a pixel's colour counts as much as its alpha says
/// and a transparent pixel counts for nothing (no dark fringe from transparent black); the result is straight alpha again, with the
/// colour of the transparent pixels left to [`bleed`]. Lanczos rings: past a hard edge its response dips below zero and comes back up a
/// few units of alpha two or three pixels out, a faint ghost outline. The alpha of a pixel is therefore held inside the range of the
/// source's alpha over the pixel's own footprint (so nothing is drawn where the source has nothing, and a flat opaque area stays
/// exactly opaque). A damaged `src` gives a transparent bitmap.
pub fn resize(src: &Bgra, dw: u32, dh: u32) -> Bgra {
    let (sw, sh) = (src.width as usize, src.height as usize);
    let mut out = Bgra { width: dw, height: dh, data: vec![0; dw as usize * dh as usize * 4] };
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.data.len() < sw * sh * 4 {
        return out;
    }
    let (dw, dh) = (dw as usize, dh as usize);
    // premultiplied floats: b, g, r in 0..=255 times alpha, a in 0..=1
    let mut pre = vec![0f32; sw * sh * 4];
    for (o, p) in pre.as_chunks_mut::<4>().0.iter_mut().zip(src.data.as_chunks::<4>().0) {
        let a = p[3] as f32 / 255.0;
        o[0] = p[0] as f32 * a;
        o[1] = p[1] as f32 * a;
        o[2] = p[2] as f32 * a;
        o[3] = a;
    }
    // along x: sh rows of dw pixels
    let tx = taps(sw, dw);
    let fx = footprints(sw, dw);
    let mut mid = vec![0f32; dw * sh * 4];
    // the least and the most alpha of the source under each output pixel's footprint along x
    let mut a_min = vec![0f32; dw * sh];
    let mut a_max = vec![0f32; dw * sh];
    for y in 0..sh {
        let row = &pre[y * sw * 4..(y + 1) * sw * 4];
        for (x, &(lo, hi)) in fx.iter().enumerate() {
            let alphas = (lo..hi).map(|j| row[j * 4 + 3]);
            a_min[y * dw + x] = alphas.clone().fold(1.0, f32::min);
            a_max[y * dw + x] = alphas.fold(0.0, f32::max);
        }
        for (x, t) in tx.iter().enumerate() {
            let mut acc = [0f32; 4];
            for (k, w) in t.weights.iter().enumerate() {
                let p = &row[(t.start + k) * 4..(t.start + k) * 4 + 4];
                for c in 0..4 {
                    acc[c] += p[c] * w;
                }
            }
            mid[(y * dw + x) * 4..(y * dw + x) * 4 + 4].copy_from_slice(&acc);
        }
    }
    // along y
    let ty = taps(sh, dh);
    let fy = footprints(sh, dh);
    for (y, t) in ty.iter().enumerate() {
        let (ylo, yhi) = fy[y];
        for x in 0..dw {
            let mut acc = [0f32; 4];
            for (k, w) in t.weights.iter().enumerate() {
                let p = &mid[((t.start + k) * dw + x) * 4..((t.start + k) * dw + x) * 4 + 4];
                for c in 0..4 {
                    acc[c] += p[c] * w;
                }
            }
            // the filter overshoots and rings at hard edges: alpha and the colour it carries stay inside what is possible
            let lo = (ylo..yhi).map(|j| a_min[j * dw + x]).fold(1.0, f32::min);
            let hi = (ylo..yhi).map(|j| a_max[j * dw + x]).fold(0.0, f32::max);
            let a = acc[3].clamp(lo, hi);
            let a8 = (a * 255.0).round();
            let o = (y * dw + x) * 4;
            if a8 >= 1.0 && acc[3] > 1e-4 {
                // (the colour is the filter's own premultiplied sum over its own alpha: holding the alpha inside its range must not
                // change the colour the sum stands for)
                for (dst, v) in out.data[o..o + 3].iter_mut().zip(&acc) {
                    *dst = (v / acc[3]).round().clamp(0.0, 255.0) as u8;
                }
                out.data[o + 3] = a8 as u8;
            }
        }
    }
    out
}

/// Gives the fully transparent pixels next to the picture the picture's colour (two pixels deep, enough for the sampler's blend of
/// neighbours at any reduction the dock draws). Their alpha stays 0: nothing is drawn there, but where the sampler blends them with a
/// drawn pixel it blends the same colour, not black.
pub fn bleed(img: &mut Bgra) {
    let (w, h) = (img.width as usize, img.height as usize);
    if w == 0 || h == 0 || img.data.len() < w * h * 4 {
        return;
    }
    let mut has: Vec<bool> = img.data.as_chunks::<4>().0.iter().take(w * h).map(|p| p[3] > 0).collect();
    for _ in 0..2 {
        let before = has.clone();
        for y in 0..h {
            for x in 0..w {
                if before[y * w + x] {
                    continue;
                }
                let (mut sum, mut weight) = ([0f32; 3], 0f32);
                for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        if !before[ny * w + nx] {
                            continue;
                        }
                        let p = &img.data[(ny * w + nx) * 4..(ny * w + nx) * 4 + 4];
                        // a neighbour that is drawn counts by its alpha, one that only got its colour from the pass before counts a little
                        let wgt = if p[3] > 0 { p[3] as f32 } else { 1.0 };
                        for c in 0..3 {
                            sum[c] += p[c] as f32 * wgt;
                        }
                        weight += wgt;
                    }
                }
                if weight > 0.0 {
                    let o = (y * w + x) * 4;
                    for (dst, v) in img.data[o..o + 3].iter_mut().zip(&sum) {
                        *dst = (v / weight).round().clamp(0.0, 255.0) as u8;
                    }
                    has[y * w + x] = true;
                }
            }
        }
    }
}

/// `src` fitted into a `px` x `px` square (scaled with [`resize`], kept in proportion, centred on transparent pixels), bled. A picture
/// that is already that size is only bled.
pub fn square(src: &Bgra, px: u32) -> Bgra {
    let usable = src.width > 0 && src.height > 0 && src.data.len() >= src.width as usize * src.height as usize * 4;
    let mut out = if !usable {
        Bgra { width: px, height: px, data: vec![0; px as usize * px as usize * 4] }
    } else if src.width == px && src.height == px {
        Bgra { width: px, height: px, data: src.data[..px as usize * px as usize * 4].to_vec() }
    } else {
        let k = px as f32 / src.width.max(src.height) as f32;
        let dw = ((src.width as f32 * k).round() as u32).clamp(1, px);
        let dh = ((src.height as f32 * k).round() as u32).clamp(1, px);
        let scaled = resize(src, dw, dh);
        let mut sq = Bgra { width: px, height: px, data: vec![0; px as usize * px as usize * 4] };
        let (ox, oy) = ((px - dw) / 2, (px - dh) / 2);
        for y in 0..dh {
            let from = (y * dw * 4) as usize;
            let to = (((y + oy) * px + ox) * 4) as usize;
            sq.data[to..to + (dw * 4) as usize].copy_from_slice(&scaled.data[from..from + (dw * 4) as usize]);
        }
        sq
    };
    bleed(&mut out);
    out
}

/// One picture per size in `sizes` (ascending, see `iconpick::level_sizes`), every one made from `src` itself - not from the next
/// larger one, which would blur the small ones twice.
pub fn levels(src: &Bgra, sizes: &[u32]) -> Vec<Bgra> {
    sizes.iter().map(|&s| square(src, s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> Bgra {
        Bgra { width: w, height: h, data: bgra.iter().copied().cycle().take((w * h * 4) as usize).collect() }
    }

    fn at(b: &Bgra, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * b.width + x) * 4) as usize;
        [b.data[i], b.data[i + 1], b.data[i + 2], b.data[i + 3]]
    }

    /// The left `cols` columns of a `w` x `h` bitmap are `ink`, the rest `ground`.
    fn split(w: u32, h: u32, cols: u32, ink: [u8; 4], ground: [u8; 4]) -> Bgra {
        let mut b = solid(w, h, ground);
        for y in 0..h {
            for x in 0..cols {
                b.data[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize].copy_from_slice(&ink);
            }
        }
        b
    }

    #[test]
    fn a_solid_colour_and_its_alpha_survive_every_size() {
        for (from, to) in [(256u32, 69u32), (256, 44), (96, 55), (64, 44), (48, 44), (32, 69), (32, 137), (44, 88), (137, 44)] {
            let f = square(&solid(from, from, [10, 200, 90, 255]), to);
            assert_eq!((f.width, f.height), (to, to));
            for (x, y) in [(0, 0), (to / 2, to / 2), (to - 1, to - 1), (0, to - 1)] {
                let p = at(&f, x, y);
                assert!(p[3] == 255 && (p[0] as i32 - 10).abs() <= 1 && (p[1] as i32 - 200).abs() <= 1 && (p[2] as i32 - 90).abs() <= 1, "{from} -> {to} at {x},{y}: {p:?}");
            }
            let t = square(&solid(from, from, [40, 60, 80, 128]), to);
            let p = at(&t, to / 2, to / 2);
            assert!((p[3] as i32 - 128).abs() <= 1 && (p[0] as i32 - 40).abs() <= 1 && (p[1] as i32 - 60).abs() <= 1 && (p[2] as i32 - 80).abs() <= 1, "{p:?}");
        }
    }

    #[test]
    fn the_exact_size_comes_back_as_it_was() {
        let b = split(44, 44, 20, [0, 0, 255, 255], [0, 0, 0, 0]);
        let f = square(&b, 44);
        // (only the colour of the transparent pixels next to the picture differs: that is the bleed)
        for y in 0..44 {
            for x in 0..44 {
                let (p, q) = (at(&b, x, y), at(&f, x, y));
                assert_eq!(p[3], q[3]);
                if p[3] > 0 {
                    assert_eq!(p, q);
                }
            }
        }
    }

    #[test]
    fn the_edge_of_a_picture_has_no_dark_fringe() {
        // the left 30 of 64 columns opaque red, the rest transparent black: after scaling, every pixel that is drawn at all is red, however faint
        let b = split(64, 64, 30, [0, 0, 255, 255], [0, 0, 0, 0]);
        for to in [44u32, 55, 69] {
            let f = square(&b, to);
            let mut partial = 0;
            for x in 0..to {
                let p = at(&f, x, to / 2);
                if p[3] > 0 {
                    assert!(p[2] >= 250 && p[0] <= 3 && p[1] <= 3, "{to}: x={x} {p:?}");
                }
                if p[3] > 0 && p[3] < 255 {
                    partial += 1;
                }
            }
            assert!(partial >= 1, "{to}: the boundary falls inside a pixel");
            // and no overshoot of alpha past the picture: well away from the edge it is nothing
            assert_eq!(at(&f, to - 2, to / 2)[3], 0);
        }
    }

    #[test]
    fn a_hard_edge_leaves_no_ghost_of_alpha_past_the_pixel_it_falls_in() {
        // The Lanczos filter rings: past a step from opaque to transparent its response dips below zero (clamped) and then rises to a
        // few units of alpha again, two pixels out. Nothing of the picture is there, so nothing may be drawn there either.
        for (from, cols, to) in [(64u32, 30u32, 44u32), (256, 120, 44), (256, 120, 69), (96, 49, 55), (256, 100, 137)] {
            let b = split(from, from, cols, [0, 0, 255, 255], [0, 0, 0, 0]);
            let f = resize(&b, to, to);
            let edge = cols as f32 * to as f32 / from as f32;
            let row: Vec<u8> = (0..to).map(|x| at(&f, x, to / 2)[3]).collect();
            for (x, &a) in row.iter().enumerate() {
                // (a pixel whose own footprint, one pixel each side of its centre, reaches the picture may have some alpha)
                if x as f32 - 0.5 > edge + 1.0 {
                    assert_eq!(a, 0, "{from} -> {to}: x={x} past the edge at {edge:.1}: {row:?}");
                }
            }
        }
    }

    #[test]
    fn the_transparent_pixels_next_to_the_picture_take_its_colour_and_the_far_ones_do_not() {
        let mut b = solid(16, 16, [0, 0, 0, 0]);
        for y in 6..10 {
            for x in 6..10 {
                b.data[((y * 16 + x) * 4) as usize..((y * 16 + x) * 4 + 4) as usize].copy_from_slice(&[200, 100, 50, 255]);
            }
        }
        bleed(&mut b);
        assert_eq!(at(&b, 5, 7), [200, 100, 50, 0], "one pixel out: the colour, still not drawn");
        assert_eq!(at(&b, 4, 7), [200, 100, 50, 0], "two pixels out");
        assert_eq!(at(&b, 3, 7), [0, 0, 0, 0], "three pixels out: untouched");
        assert_eq!(at(&b, 5, 5), [200, 100, 50, 0], "a corner");
        assert_eq!(at(&b, 7, 7), [200, 100, 50, 255], "the picture itself is not changed");
        // an empty picture has nothing to bleed and no panic
        let mut e = solid(4, 4, [0, 0, 0, 0]);
        bleed(&mut e);
        assert!(e.data.iter().all(|&v| v == 0));
        bleed(&mut Bgra { width: 4, height: 4, data: vec![] });
    }

    #[test]
    fn an_edge_stays_sharper_than_with_a_box_average() {
        // a black and white step in the middle of a pixel after reduction: how many pixels are neither black nor white
        let b = split(96, 96, 49, [255, 255, 255, 255], [0, 0, 0, 255]);
        let width = |f: &Bgra| (0..f.width).filter(|&x| (20..236).contains(&at(f, x, 4)[0])).count();
        let ours = resize(&b, 44, 44);
        // (the earlier box-average resampling of `icons`, on the same picture)
        let old = super::super::icons::fit_square(&b, 44);
        assert!(width(&ours) <= width(&old), "ours {} px wide, old {}", width(&ours), width(&old));
        assert!(width(&ours) <= 2, "{}", width(&ours));
    }

    #[test]
    fn a_picture_that_is_reduced_keeps_its_average_colour_at_every_kept_size() {
        // a smooth blob on transparent ground: the average premultiplied colour is the same at every size, so the picture does not
        // brighten or fade as it grows from one kept size to the next
        let mut b = solid(256, 256, [0, 0, 0, 0]);
        for y in 0..256i32 {
            for x in 0..256i32 {
                let d = (((x - 128) * (x - 128) + (y - 128) * (y - 128)) as f32).sqrt();
                if d < 100.0 {
                    let i = ((y * 256 + x) * 4) as usize;
                    b.data[i..i + 4].copy_from_slice(&[30, 120, 220, 255]);
                }
            }
        }
        let mean = |f: &Bgra| {
            let n = (f.width * f.height) as f32;
            f.data.as_chunks::<4>().0.iter().map(|p| p[2] as f32 * p[3] as f32 / 255.0).sum::<f32>() / n
        };
        let sizes = [44u32, 55, 69];
        let made = levels(&b, &sizes);
        assert_eq!(made.iter().map(|f| f.width).collect::<Vec<_>>(), sizes);
        let reference = mean(&b);
        for f in &made {
            assert!((mean(f) - reference).abs() / reference < 0.02, "{}: {} against {}", f.width, mean(f), reference);
        }
    }

    #[test]
    fn a_wide_picture_is_centred_on_a_transparent_square() {
        let f = square(&solid(64, 32, [1, 2, 3, 255]), 44);
        assert_eq!((f.width, f.height), (44, 44));
        assert_eq!(at(&f, 22, 22), [1, 2, 3, 255]);
        assert_eq!(at(&f, 22, 0)[3], 0, "above the picture");
        assert_eq!(at(&f, 22, 43)[3], 0, "below the picture");
        assert_eq!(at(&f, 0, 22)[3], 255, "the width is used in full");
    }

    #[test]
    fn a_small_picture_is_enlarged_to_the_size_without_a_panic() {
        for from in [1u32, 2, 3, 16, 32] {
            let f = square(&split(from, from, from / 2, [0, 255, 0, 255], [0, 0, 0, 0]), 69);
            assert_eq!((f.width, f.height, f.data.len()), (69, 69, 69 * 69 * 4));
        }
    }

    #[test]
    fn nonsense_input_gives_a_transparent_square_not_a_panic() {
        let f = square(&Bgra { width: 4, height: 4, data: vec![0; 3] }, 8);
        assert_eq!((f.width, f.height, f.data.len()), (8, 8, 256));
        assert!(f.data.iter().all(|&b| b == 0));
        let z = square(&Bgra { width: 0, height: 0, data: vec![] }, 8);
        assert_eq!(z.data.len(), 256);
        assert_eq!(resize(&solid(2, 2, [1, 1, 1, 255]), 0, 5).data.len(), 0);
    }

    #[test]
    fn the_taps_always_add_up_to_one_and_stay_inside_the_line() {
        for (s, d) in [(256usize, 69usize), (96, 44), (32, 137), (1, 9), (9, 1), (44, 44), (3, 2)] {
            for t in taps(s, d) {
                assert!(t.start + t.weights.len() <= s, "{s} -> {d}");
                assert!((t.weights.iter().sum::<f32>() - 1.0).abs() < 1e-4, "{s} -> {d}");
            }
        }
    }
}
