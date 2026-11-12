//! The mean brightness of the backdrop under one glass, worked out from the bytes the loop guard reads back anyway (one coarse level of
//! the backdrop pyramid, `Region::read_back` in gpu.rs). Nothing here touches the GPU: it only reads a block of texels that is already
//! in memory, so the value costs no extra pass, no extra copy and no extra wait.
//!
//! The pyramid is `R10G10B10A2_UNORM` holding gamma encoded values (see glass.hlsl, "COLOUR SPACE"): a texel is one little endian `u32`,
//! red in bits 0..10, green in 10..20, blue in 20..30. The brightness is the Rec. 709 weighting of those encoded values, 0 (black) to 1
//! (white). It is what a person sees as "light" or "dark", which is what a program that picks light or dark text over the glass needs
//! (`rf_win::glass::backdrop_luma`).
//!
//! Only the texels that belong to the glass itself count, not the padding the pyramid keeps around it for the blur: a bright window
//! beside a dock island must not turn the island's text dark while the island itself sits over a dark wall.

/// The texels of a pyramid level that lie under the glass, as `(x0, y0, x1, y1)` with the end exclusive. `level` is the pyramid level
/// read back, `pad` the padding (device px) the region keeps around the glass, `glass_w` x `glass_h` the glass (device px) and
/// `level_w` x `level_h` the size of the level. A texel counts when its centre is inside the glass; a glass smaller than one texel is
/// given the one texel under its middle, so there is always something to average.
pub(super) fn glass_texels(level: u32, pad: u32, glass_w: u32, glass_h: u32, level_w: u32, level_h: u32) -> (u32, u32, u32, u32) {
    let s = 1u32 << level.min(31);
    let span = |start: u32, len: u32, limit: u32| {
        let half = s / 2;
        // first texel whose centre (i * s + s / 2) is at or after `start`, and the first whose centre is at or after `start + len`
        let first = (start.saturating_sub(half)).div_ceil(s);
        let end = (start + len).saturating_sub(half).div_ceil(s);
        let (first, end) = (first.min(limit.saturating_sub(1)), end.min(limit));
        if end > first {
            (first, end)
        } else {
            let mid = ((start + len / 2) / s).min(limit.saturating_sub(1));
            (mid, mid + 1)
        }
    };
    let (x0, x1) = span(pad, glass_w, level_w);
    let (y0, y1) = span(pad, glass_h, level_h);
    (x0, y0, x1, y1)
}

/// The brightness (0 to 1) of one `R10G10B10A2` texel (what `mean_luma` averages, one at a time).
#[cfg(test)]
pub(super) fn texel_luma(v: u32) -> f32 {
    let (r, g, b) = (v & 0x3ff, (v >> 10) & 0x3ff, (v >> 20) & 0x3ff);
    // Rec. 709 weights in ten thousandths, so the sum stays an integer
    (2126 * r + 7152 * g + 722 * b) as f32 / (10_000.0 * 1023.0)
}

/// The mean brightness of the texels `rect` (`x0, y0, x1, y1`, end exclusive) of a block of `R10G10B10A2` texels whose rows are `pitch`
/// bytes apart. `None` when the rectangle is empty or reaches beyond `bytes`.
pub(super) fn mean_luma(bytes: &[u8], pitch: usize, rect: (u32, u32, u32, u32)) -> Option<f32> {
    let (x0, y0, x1, y1) = (rect.0 as usize, rect.1 as usize, rect.2 as usize, rect.3 as usize);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut sum = 0u64;
    for y in y0..y1 {
        let from = y * pitch + x0 * 4;
        let row = bytes.get(from..from + (x1 - x0) * 4)?;
        for px in row.as_chunks::<4>().0 {
            let v = u32::from_le_bytes(*px);
            let (r, g, b) = ((v & 0x3ff) as u64, ((v >> 10) & 0x3ff) as u64, ((v >> 20) & 0x3ff) as u64);
            sum += 2126 * r + 7152 * g + 722 * b;
        }
    }
    let n = ((x1 - x0) * (y1 - y0)) as f64;
    Some((sum as f64 / (n * 10_000.0 * 1023.0)) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texel(r: u32, g: u32, b: u32) -> [u8; 4] {
        (r | g << 10 | b << 20 | 3 << 30).to_le_bytes()
    }

    /// A block of `w` x `h` texels with `pitch` bytes per row (padding bytes after each row are 0xAB, which would show if they were read).
    fn block(w: usize, h: usize, pitch: usize, f: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        let mut v = vec![0xABu8; pitch * h];
        for y in 0..h {
            for x in 0..w {
                v[y * pitch + x * 4..y * pitch + x * 4 + 4].copy_from_slice(&f(x, y));
            }
        }
        v
    }

    #[test]
    fn white_is_one_black_is_zero_and_grey_is_between() {
        assert!((texel_luma(u32::from_le_bytes(texel(1023, 1023, 1023))) - 1.0).abs() < 1e-6);
        assert_eq!(texel_luma(u32::from_le_bytes(texel(0, 0, 0))), 0.0);
        let mid = texel_luma(u32::from_le_bytes(texel(512, 512, 512)));
        assert!((mid - 512.0 / 1023.0).abs() < 1e-4, "{mid}");
    }

    #[test]
    fn the_channels_are_weighted_like_the_eye_does() {
        // green counts most, blue least (Rec. 709)
        let (r, g, b) = (texel(1023, 0, 0), texel(0, 1023, 0), texel(0, 0, 1023));
        let l = |t: [u8; 4]| texel_luma(u32::from_le_bytes(t));
        assert!((l(r) - 0.2126).abs() < 1e-4);
        assert!((l(g) - 0.7152).abs() < 1e-4);
        assert!((l(b) - 0.0722).abs() < 1e-4);
    }

    #[test]
    fn a_known_block_has_the_expected_mean() {
        // 4 x 2 texels, the left half white and the right half black: the mean of the whole block is one half
        let pitch = 32; // wider than the 16 bytes of a row: the bytes after a row must not be read
        let b = block(4, 2, pitch, |x, _| if x < 2 { texel(1023, 1023, 1023) } else { texel(0, 0, 0) });
        let m = mean_luma(&b, pitch, (0, 0, 4, 2)).unwrap();
        assert!((m - 0.5).abs() < 1e-6, "{m}");
        // a rectangle inside it only sees its own texels
        assert!((mean_luma(&b, pitch, (0, 0, 2, 2)).unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(mean_luma(&b, pitch, (2, 0, 4, 2)).unwrap(), 0.0);
        assert!((mean_luma(&b, pitch, (1, 1, 3, 2)).unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn an_empty_or_overreaching_rectangle_is_unknown() {
        let b = block(2, 2, 8, |_, _| texel(100, 100, 100));
        assert_eq!(mean_luma(&b, 8, (1, 1, 1, 2)), None);
        assert_eq!(mean_luma(&b, 8, (0, 0, 3, 2)), None, "wider than the bytes");
        assert_eq!(mean_luma(&b, 8, (0, 0, 2, 3)), None, "taller than the bytes");
        assert_eq!(mean_luma(&[], 0, (0, 0, 1, 1)), None);
    }

    #[test]
    fn the_glass_texels_leave_the_padding_out() {
        // level 2 (4 px per texel), pad 96 = 24 texels, a glass of 300 x 200 px = 75 x 50 texels, a level of 128 x 128
        let (x0, y0, x1, y1) = glass_texels(2, 96, 300, 200, 128, 128);
        assert_eq!((x0, x1), (24, 99));
        assert_eq!((y0, y1), (24, 74));
        // level 0: exactly the glass rectangle
        assert_eq!(glass_texels(0, 96, 300, 200, 512, 512), (96, 96, 396, 296));
    }

    #[test]
    fn a_texel_counts_when_its_centre_is_inside_the_glass() {
        // 16 px texels, pad 100, glass 40 px wide: the glass is [100, 140); centres are at 8, 24, ..., 88, 104, 120, 136, 152
        let (x0, _, x1, _) = glass_texels(4, 100, 40, 40, 64, 64);
        assert_eq!((x0, x1), (6, 9), "the centres 104, 120 and 136 are inside; 88 and 152 are not");
    }

    #[test]
    fn a_glass_smaller_than_a_texel_still_has_one() {
        let (x0, y0, x1, y1) = glass_texels(5, 96, 10, 10, 32, 32);
        assert_eq!((x1 - x0, y1 - y0), (1, 1));
        assert!(x0 < 32 && y0 < 32);
    }

    #[test]
    fn the_texels_never_leave_the_level() {
        for level in 0..8 {
            for (gw, gh) in [(1u32, 1u32), (48, 48), (300, 200), (4000, 3000)] {
                let (lw, lh) = (((1024u32) >> level).max(1), ((768u32) >> level).max(1));
                let (x0, y0, x1, y1) = glass_texels(level, 96, gw, gh, lw, lh);
                assert!(x0 < x1 && y0 < y1 && x1 <= lw && y1 <= lh, "level {level} glass {gw}x{gh}: {x0},{y0},{x1},{y1} in {lw}x{lh}");
            }
        }
    }
}
