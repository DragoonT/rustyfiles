//! The shadow that keeps an icon's name readable on any wallpaper. Pure: the numbers, and the arithmetic that checks them.
//!
//! WHAT EXPLORER DOES. The names on its desktop are white with a soft dark shadow, so that they read on a white photograph as well as
//! on a black one. GPUI draws text with no shadow of its own and a blur pass per label would cost a texture per name every frame, so
//! the shadow is the same text drawn a few times in translucent black under the white one, shifted by about a pixel (the view does
//! that; see `DesktopView::label_line`). Each copy is one more run of glyphs from the text system's cache: no blur, no offscreen
//! pass, nothing that is worked out per frame (the view is drawn when something changed, not on a timer).
//!
//! WHY THESE COPIES. A pixel one step beside a stroke of a letter is covered by every copy whose shift points that way. Four copies
//! at the diagonals cover each of the four sides of a stroke twice, and a lower copy (the drop) covers the pixels under it, so the
//! least-covered side of a stroke is darkened by `1 - (1 - a)^2` with `a` the diagonal copies' opacity. At 0.55 that is 80 % black
//! right beside the white strokes, which is what the contrast figures below are computed from. The first version had three copies
//! below the text only (one pixel to each side and under it), which left the top of every letter on the bare picture.
//!
//! THEMES. The ink is the same in the light and the dark theme: the name sits on the user's picture, not on a surface of the theme.
//! A selected name sits on the selection fill (the accent at a third of its strength over the picture), and the same shadow is under
//! it.

/// One translucent black copy of the text: its shift in canvas pixels (the view multiplies by the shown scale, never less than 0.6 of
/// a logical pixel) and its opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    pub dx: f32,
    pub dy: f32,
    pub alpha: f32,
}

/// The copies, drawn first (lowest first). The four diagonals do the work at the stroke's edges; the drop under the line gives the
/// softer fall-off below.
pub const LAYERS: [Layer; 5] = [
    Layer { dx: 0.0, dy: 2.0, alpha: 0.30 },
    Layer { dx: -1.0, dy: -1.0, alpha: 0.55 },
    Layer { dx: 1.0, dy: -1.0, alpha: 0.55 },
    Layer { dx: -1.0, dy: 1.0, alpha: 0.55 },
    Layer { dx: 1.0, dy: 1.0, alpha: 0.55 },
];

/// The opacity of the white text on top.
pub const TEXT_ALPHA: f32 = 1.0;

/// Never shift by less than this many logical pixels (a pixel at the shown scale of a small preview would vanish).
pub const MIN_UNIT: f32 = 0.6;

/// The shift of a layer in logical pixels, for `unit` logical pixels per canvas pixel.
pub fn shifted(layer: Layer, unit: f32) -> (f32, f32) {
    let u = unit.max(MIN_UNIT);
    (layer.dx * u, layer.dy * u)
}

/// How dark the pixel directly beside a stroke gets on its least-covered side (0 = untouched, 1 = black): the copies whose shift
/// points at that side add up as `1 - product(1 - alpha)`.
pub fn edge_darkness() -> f32 {
    // the four sides of a stroke: a pixel to its right is reached by every copy shifted right, and so on
    let sides: [fn(&Layer) -> bool; 4] = [|l| l.dx >= 1.0, |l| l.dx <= -1.0, |l| l.dy >= 1.0, |l| l.dy <= -1.0];
    sides.iter().map(|reaches| 1.0 - LAYERS.iter().filter(|l| reaches(l)).map(|l| 1.0 - l.alpha).product::<f32>()).fold(1.0, f32::min)
}

fn linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Relative luminance (WCAG) of an sRGB colour, each channel 0..1.
pub fn luminance(rgb: (f32, f32, f32)) -> f32 {
    0.2126 * linear(rgb.0) + 0.7152 * linear(rgb.1) + 0.0722 * linear(rgb.2)
}

/// WCAG contrast ratio of two luminances.
pub fn contrast(a: f32, b: f32) -> f32 {
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// The contrast of the white text against the picture right beside a stroke, where the picture is the colour `bg`: the shadow
/// darkens it by [`edge_darkness`].
///
/// THIS IS A MODEL, NOT A PICTURE. It scales LINEAR light by `1 - darkness`; the renderer blends translucent black over the
/// sRGB-ENCODED values, which darkens a mid tone far more in linear terms (a factor of `(1 - d)^2.4`, see [`contrast_encoded_blend`]),
/// so this figure is the cautious one. What it does not model: the antialiased edge of a glyph (partial coverage), the 0.6 floor of the
/// shift at small scales. The look on a real wallpaper is a live check, not something these numbers decide.
pub fn contrast_beside_strokes(bg: (f32, f32, f32)) -> f32 {
    let under = luminance(bg) * (1.0 - edge_darkness());
    contrast(luminance((TEXT_ALPHA, TEXT_ALPHA, TEXT_ALPHA)), under)
}

/// The same contrast with the shadow blended the way the renderer does it: translucent black over the sRGB-encoded channels.
pub fn contrast_encoded_blend(bg: (f32, f32, f32)) -> f32 {
    let keep = 1.0 - edge_darkness();
    contrast(luminance((TEXT_ALPHA, TEXT_ALPHA, TEXT_ALPHA)), luminance((bg.0 * keep, bg.1 * keep, bg.2 * keep)))
}

/// The colour of the selection fill over the picture `bg`: `accent` at `alpha` (the view's `sel_bg`).
pub fn selected_over(bg: (f32, f32, f32), accent: (f32, f32, f32), alpha: f32) -> (f32, f32, f32) {
    let mix = |b: f32, a: f32| b * (1.0 - alpha) + a * alpha;
    (mix(bg.0, accent.0), mix(bg.1, accent.1), mix(bg.2, accent.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: (f32, f32, f32) = (1.0, 1.0, 1.0);
    const MID: (f32, f32, f32) = (0.5, 0.5, 0.5);
    const BLACK: (f32, f32, f32) = (0.0, 0.0, 0.0);

    #[test]
    fn every_side_of_a_stroke_is_covered_at_least_80_percent() {
        let d = edge_darkness();
        assert!(d >= 0.79, "{d}");
        // the drop alone would leave the top of a letter bare: the diagonals reach all four sides
        let drop_only = 1.0 - (1.0 - LAYERS[0].alpha);
        assert!(drop_only < 0.5, "the drop is the soft part, not the edge");
    }

    #[test]
    fn the_name_reads_on_white_on_a_mid_tone_and_on_black() {
        // 3:1 is the WCAG floor for large text; the names are small, and Explorer's own shadow gives about that on white
        assert!(contrast_beside_strokes(WHITE) >= 3.5, "{}", contrast_beside_strokes(WHITE));
        assert!(contrast_beside_strokes(MID) >= 7.0, "{}", contrast_beside_strokes(MID));
        assert!(contrast_beside_strokes(BLACK) >= 15.0, "{}", contrast_beside_strokes(BLACK));
        // and the shadow is no worse for any grey in between
        for g in 0..=20 {
            let v = g as f32 / 20.0;
            assert!(contrast_beside_strokes((v, v, v)) >= 3.5, "grey {v}");
        }
        // a saturated light picture (yellow, cyan) is the hard case after white
        for bg in [(1.0, 1.0, 0.0), (0.0, 1.0, 1.0), (1.0, 0.9, 0.2)] {
            assert!(contrast_beside_strokes(bg) >= 3.5, "{bg:?}");
        }
    }

    #[test]
    fn the_linear_model_is_the_cautious_one() {
        // blending over encoded values never gives a lower contrast than the figure the other tests hold to
        for g in 0..=20 {
            let v = g as f32 / 20.0;
            assert!(contrast_encoded_blend((v, v, v)) >= contrast_beside_strokes((v, v, v)) - 1e-4, "grey {v}");
        }
        assert!(contrast_encoded_blend(WHITE) >= 3.5);
    }

    #[test]
    fn the_old_shadow_was_not_enough_on_white() {
        // one pixel to each side and under it, never above: what the first version drew
        let old = [(0.0f32, 1.0f32, 0.55f32), (1.0, 1.0, 0.40), (-1.0, 1.0, 0.40), (0.0, 2.0, 0.25)];
        let top_side: f32 = 1.0 - old.iter().filter(|l| l.1 <= -1.0).map(|l| 1.0 - l.2).product::<f32>();
        assert_eq!(top_side, 0.0, "nothing above the strokes");
    }

    #[test]
    fn the_ink_does_not_depend_on_the_theme_and_the_selection_fill_keeps_it_readable() {
        // light-theme and dark-theme accents of Windows (a deep blue, a light blue) over a white and a mid-grey picture
        for accent in [(0.0, 0.47, 0.83), (0.38, 0.80, 1.0), (0.85, 0.2, 0.2)] {
            for bg in [WHITE, MID, BLACK] {
                let fill = selected_over(bg, accent, 0.34);
                assert!(contrast_beside_strokes(fill) >= 3.5, "accent {accent:?} over {bg:?}");
            }
        }
    }

    #[test]
    fn the_shift_is_a_pixel_at_the_shown_scale_and_never_vanishes() {
        assert_eq!(shifted(LAYERS[1], 1.0), (-1.0, -1.0));
        assert_eq!(shifted(LAYERS[4], 1.0 / 1.5), (0.6666667, 0.6666667), "a 150 % display: a physical pixel is 2/3 of a logical one");
        assert_eq!(shifted(LAYERS[0], 0.1), (0.0, 1.2), "a tiny preview still shifts by the floor");
        assert!(LAYERS.iter().all(|l| l.alpha > 0.0 && l.alpha < 1.0));
        assert_eq!(TEXT_ALPHA, 1.0);
    }
}
