//! Tests of the look's settings (`LookParams`) and of the rim bound of small pieces. The numbers are checked without a GPU; what the
//! numbers do to a picture goes through the offscreen path like the other drawing tests (skipped where `can_draw` says so).

use super::gpu::{Look, LensLook};
use super::tests::{bare_optics, can_draw, drivers, expected_centre, picture, px, render_with, step_width};
use super::*;

fn params(frost: f32, tint: f32, refraction: f32, saturation: f32) -> LookParams {
    LookParams { frost, tint, refraction, saturation }
}

/// The numbers the app shipped with before the settings existed (and still ships with at the default settings). The lens of the light
/// theme is the one number that changed on purpose: its white veil, 0.35 then, is 0.82 now.
fn shipped(dark: bool) -> Look {
    Look {
        depth: 1.6,
        bezel: 0.421,
        max_bezel: 44.0,
        zoom: 1.0,
        dispersion: 0.0,
        blur: 10.351,
        saturate: 2.252,
        lum: if dark { 0.80 } else { 1.0 },
        contrast: if dark { 1.0 } else { 0.9 },
        tint: if dark { [20.0 / 255.0, 20.0 / 255.0, 24.0 / 255.0, 0.10] } else { [1.0, 1.0, 1.0, 0.70] },
        rim: 0.25,
        rim_width: 0.3,
        shade: 0.03,
        light: -135.0,
        rim_frost: 0.12,
        lens: LensLook {
            frost: 4.0,
            rim_max: 10.0,
            rim_frac: 0.45,
            depth: 0.9,
            zoom: 1.04,
            bright: 1.08,
            tint: [1.0, 1.0, 1.0, if dark { 0.10 } else { 0.70 + (1.0 - 0.70) * 0.4 }],
            highlight: 0.6,
            dark_line: 0.2,
            shadow: 0.22,
            shadow_dy: 1.5,
            shadow_blur: 8.0,
        },
    }
}

#[test]
fn the_default_settings_give_exactly_the_shipped_look() {
    for dark in [false, true] {
        let got = Look::regular(dark, &LookParams::DEFAULT);
        let want = shipped(dark);
        // field by field, so a failure says which one
        assert_eq!(got.depth, want.depth, "depth (dark {dark})");
        assert_eq!(got.bezel, want.bezel);
        assert_eq!(got.max_bezel, want.max_bezel);
        assert_eq!(got.zoom, want.zoom);
        assert_eq!(got.dispersion, want.dispersion);
        assert_eq!(got.blur, want.blur, "blur (dark {dark})");
        assert_eq!(got.saturate, want.saturate, "saturate (dark {dark})");
        assert_eq!(got.lum, want.lum, "lum (dark {dark})");
        assert_eq!(got.contrast, want.contrast);
        assert_eq!(got.tint, want.tint, "tint (dark {dark})");
        assert_eq!(got.rim, want.rim);
        assert_eq!(got.rim_width, want.rim_width);
        assert_eq!(got.shade, want.shade);
        assert_eq!(got.light, want.light);
        assert_eq!(got.rim_frost, want.rim_frost);
        assert_eq!(got.lens, want.lens, "lens (dark {dark})");
        assert!(got == want);
        assert_eq!(LookParams::default(), LookParams::DEFAULT);
        assert_eq!((LookParams::DEFAULT.frost, LookParams::DEFAULT.tint, LookParams::DEFAULT.refraction, LookParams::DEFAULT.saturation), (52.0, 50.0, 67.0, 62.0));
    }
    // the light lens: 0.82 at the default pane
    assert!((Look::regular(false, &LookParams::DEFAULT).lens.tint[3] - 0.82).abs() < 1e-6);
    assert_eq!(Look::regular(true, &LookParams::DEFAULT).lens.tint[3], 0.10, "the dark lens keeps its numbers");
}

#[test]
fn the_ends_and_the_middle_of_each_setting() {
    let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
    for dark in [false, true] {
        let d = LookParams::DEFAULT;
        // frost: 0 is clear glass, 100 is 20 px, the default is 10.351
        assert_eq!(Look::regular(dark, &LookParams { frost: 0.0, ..d }).blur, 0.0);
        assert!(near(Look::regular(dark, &LookParams { frost: 100.0, ..d }).blur, 20.0));
        assert!(near(Look::regular(dark, &LookParams { frost: 26.0, ..d }).blur, 10.351 / 2.0), "straight line below the default");
        assert!(near(Look::regular(dark, &LookParams { frost: 76.0, ..d }).blur, (10.351 + 20.0) / 2.0), "straight line above the default");
        // refraction: 0 no bending, 100 a thickness of 2.4
        assert_eq!(Look::regular(dark, &LookParams { refraction: 0.0, ..d }).depth, 0.0);
        assert!(near(Look::regular(dark, &LookParams { refraction: 100.0, ..d }).depth, 2.4));
        // saturation: 1 is the picture's own colours, 3 at the top
        assert!(near(Look::regular(dark, &LookParams { saturation: 0.0, ..d }).saturate, 1.0));
        assert!(near(Look::regular(dark, &LookParams { saturation: 100.0, ..d }).saturate, 3.0));
        // frost, refraction and saturation do not touch each other or the tint
        let l = Look::regular(dark, &LookParams { frost: 0.0, refraction: 0.0, saturation: 0.0, ..d });
        assert_eq!((l.tint, l.lum), (shipped(dark).tint, shipped(dark).lum));
    }
    // tint, dark: alpha 0 and no dimming at 0; twice the alpha and lum 0.60 at 100; straight lines on both sides of 50
    let tint = |t: f32, dark: bool| Look::regular(dark, &LookParams { tint: t, ..LookParams::DEFAULT });
    let (z, full) = (tint(0.0, true), tint(100.0, true));
    assert!(z.tint[3] == 0.0 && near(z.lum, 1.0), "dark, tint 0: {} {}", z.tint[3], z.lum);
    assert!(near(full.tint[3], 0.20) && near(full.lum, 0.60), "dark, tint 100: {} {}", full.tint[3], full.lum);
    assert!(near(tint(25.0, true).tint[3], 0.05) && near(tint(25.0, true).lum, 0.90));
    assert!(near(tint(75.0, true).tint[3], 0.15) && near(tint(75.0, true).lum, 0.70));
    // light: the alpha is capped at 0.92 (twice 0.70 would be 1.4); the light glass is not dimmed
    let (z, full) = (tint(0.0, false), tint(100.0, false));
    assert!(z.tint[3] == 0.0 && z.lum == 1.0);
    assert!(near(full.tint[3], 0.92) && full.lum == 1.0, "light, tint 100: {}", full.tint[3]);
    assert!(near(tint(25.0, false).tint[3], 0.35) && near(tint(75.0, false).tint[3], 0.81));
}

#[test]
fn values_outside_the_scale_are_cut_and_nan_is_the_default() {
    let d = LookParams::DEFAULT;
    let top = LookParams { frost: 100.0, tint: 100.0, refraction: 100.0, saturation: 100.0 };
    let bottom = LookParams { frost: 0.0, tint: 0.0, refraction: 0.0, saturation: 0.0 };
    for dark in [false, true] {
        assert!(Look::regular(dark, &params(250.0, 1e9, f32::INFINITY, 101.0)) == Look::regular(dark, &top), "above the scale");
        assert!(Look::regular(dark, &params(-3.0, -1e9, f32::NEG_INFINITY, -0.5)) == Look::regular(dark, &bottom), "below the scale");
        assert!(Look::regular(dark, &params(f32::NAN, f32::NAN, f32::NAN, f32::NAN)) == Look::regular(dark, &d), "not a number");
    }
    assert_eq!(params(f32::NAN, 7.0, 300.0, -1.0).clamped(), params(52.0, 7.0, 100.0, 0.0));
}

#[test]
fn every_setting_makes_another_look_which_is_what_the_loop_guard_compares() {
    let d = LookParams::DEFAULT;
    for dark in [false, true] {
        let base = Look::regular(dark, &d);
        for p in [LookParams { frost: 10.0, ..d }, LookParams { tint: 90.0, ..d }, LookParams { refraction: 10.0, ..d }, LookParams { saturation: 10.0, ..d }] {
            assert!(Look::regular(dark, &p) != base, "{p:?} (dark {dark}) must be a different look");
        }
    }
}

/// The light lens is the lighter piece at every tint setting (its veil is measured from the pane's), the dark one keeps its own numbers.
#[test]
fn the_light_lens_veil_stays_above_the_panes_at_every_tint() {
    for t in 0..=100 {
        let l = Look::regular(false, &LookParams { tint: t as f32, ..LookParams::DEFAULT });
        assert!(l.lens.tint[3] > l.tint[3], "tint {t}: lens veil {} is not above the pane's {}", l.lens.tint[3], l.tint[3]);
        assert!(l.lens.tint[3] <= 1.0);
        let dk = Look::regular(true, &LookParams { tint: t as f32, ..LookParams::DEFAULT });
        assert_eq!(dk.lens.tint[3], 0.10, "the dark lens does not follow the tint (tint {t})");
    }
}

/// The rule for small pieces: the rim band is bounded by the shape (at most a third of its short side) and NOTHING else is weakened, so
/// an island has the same optics, frost, tint, saturation and rim light as a window. A shape of 160 px or more is the look itself.
#[test]
fn a_small_piece_gets_its_rim_bounded_by_its_size_and_a_window_does_not() {
    use super::gpu::{RIM_BOUND_BELOW, rim_width};
    for dark in [false, true] {
        let base = Look::regular(dark, &LookParams::DEFAULT);
        // from 160 CSS px up: the look itself, to the bit (every window)
        for s in [160.0f32, 161.0, 400.0, 640.0, 1440.0, f32::INFINITY, f32::NAN] {
            assert!(base.sized(s) == base, "a shape of {s} px must not change");
        }
        // below: only the share of the short side the band may take changes; the thickness, the frost, the rim light, the tint and the
        // saturation are the window's own
        for s in [159.0f32, 104.0, 72.0, 48.0, 30.0, 0.0, -5.0] {
            let c = base.sized(s);
            assert_eq!(c.bezel, 1.0 / 3.0, "bezel at {s}");
            assert!(c == Look { bezel: c.bezel, ..base }, "something besides the rim's share changed at {s}");
        }
        // the band: a third of the short side for the dock's islands, the window's 44 px for a window, never wider than before
        assert!((rim_width(&base.sized(48.0), 48.0) - 16.0).abs() < 1e-4);
        assert!((rim_width(&base.sized(72.0), 72.0) - 24.0).abs() < 1e-4);
        assert!((rim_width(&base.sized(640.0), 640.0) - 44.0).abs() < 1e-4);
        assert!((rim_width(&base.sized(160.0), 160.0) - 44.0).abs() < 1e-4);
        let mut last = 0.0f32;
        for s in 1..400 {
            let (small, old) = (rim_width(&base.sized(s as f32), s as f32), rim_width(&base, s as f32));
            assert!(small <= old + 1e-4, "the bound only ever narrows the band ({s})");
            assert!(s >= RIM_BOUND_BELOW as i32 || small <= (s as f32 / 3.0).max(4.0) + 1e-4, "a third of the short side at most ({s}): {small}");
            assert!(small + 1e-4 >= last, "the band never gets narrower as the shape grows ({s})");
            last = small;
        }
    }
    // the thickness setting still counts: 0 stays 0
    assert_eq!(Look::regular(true, &LookParams { refraction: 0.0, ..LookParams::DEFAULT }).sized(48.0).depth, 0.0);
}

/// Drawn: a 300 x 48 island keeps a clear centre band (a third of its height or more is not bent at all), its rim still bends the picture
/// as the window's does (same thickness ratio), and a window-sized glass is the same picture with or without the rule.
#[test]
fn a_small_piece_keeps_a_clear_centre_and_a_bending_rim() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    // red rises by one level per 2 px DOWN, so the bend shows in the rows
    let src = picture(sw, sh, |_, y| [(y.saturating_sub(60) / 2).min(255) as u8, 0, 60]);
    let rect = (150, 120, 300u32, 48u32);
    for warp in drivers() {
        let window_rim = bare_optics();
        let bounded = window_rim.sized(48.0);
        let flat = Look { depth: 0.0, ..window_rim };
        let draw = |l: &Look| render_with(&src, sw, sh, rect, 24.0, l, warp).expect("render");
        let (a, b, f) = (draw(&window_rim), draw(&bounded), draw(&flat));
        let x = rect.2 / 2;
        // rows (of 48) at the middle of the island whose red is the straight picture's (within one level): the clear centre
        let clear = |img: &[u8]| (0..rect.3).filter(|&y| (px(img, rect.2, x, y)[2] as i32 - px(&f, rect.2, x, y)[2] as i32).abs() <= 1).count();
        let (with_window_rim, with_bounded) = (clear(&a), clear(&b));
        assert!(with_bounded >= 14, "the bounded rim leaves only {with_bounded} of 48 rows clear (warp {warp})");
        assert!(with_bounded > with_window_rim + 4, "the window's rim leaves {with_window_rim} rows clear, the bounded one {with_bounded}: no more room in the middle (warp {warp})");
        // the rim bends: the first rows under the top edge read the picture from further in
        let shift = |img: &[u8]| (1..8).map(|y| (px(img, rect.2, x, y)[2] as i32 - px(&f, rect.2, x, y)[2] as i32).abs()).max().unwrap_or(0);
        assert!(shift(&b) >= 4, "the bounded rim does not bend (shift {}, warp {warp})", shift(&b));
        // and a window-sized glass is the same picture with or without the rule
        let big = (100, 40, 320u32, 280u32);
        let w1 = render_with(&src, sw, sh, big, 8.0, &window_rim, warp).expect("render");
        let w2 = render_with(&src, sw, sh, big, 8.0, &window_rim.sized(280.0), warp).expect("render");
        assert!(w1 == w2, "a 280 px glass changed with the rule (warp {warp})");
    }
}

/// `set_look` keeps the cut values for windows that come later and does nothing for the same value. (The only test that touches the
/// process-wide value; it puts the default back.)
#[test]
fn set_look_stores_the_cut_values() {
    assert_eq!(look(), LookParams::DEFAULT);
    set_look(params(250.0, f32::NAN, -4.0, 70.0));
    assert_eq!(look(), params(100.0, 50.0, 0.0, 70.0));
    set_look(params(100.0, 50.0, 0.0, 70.0)); // the same again
    assert_eq!(look(), params(100.0, 50.0, 0.0, 70.0));
    set_look(LookParams::default());
    assert_eq!(look(), LookParams::DEFAULT);
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------------------------------------------

/// What the settings do to a picture: the same numbers as on the CPU, through the shader.
#[test]
fn a_uniform_grey_follows_the_tint_and_frost_settings() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (480u32, 360u32);
    let rect = (60, 40, 360u32, 280u32);
    let src = picture(sw, sh, |_, _| [140; 3]);
    for dark in [false, true] {
        for p in [params(52.0, 0.0, 67.0, 62.0), params(52.0, 100.0, 67.0, 62.0), params(52.0, 25.0, 67.0, 62.0), params(0.0, 50.0, 0.0, 0.0), params(100.0, 75.0, 100.0, 100.0)] {
            let look = Look::regular(dark, &p);
            for warp in drivers() {
                let out = render_with(&src, sw, sh, rect, 8.0, &look, warp).expect("render");
                let c = px(&out, rect.2, rect.2 / 2, rect.3 / 2);
                let want = expected_centre(140, &look);
                for (i, ch) in [2usize, 1, 0].into_iter().enumerate() {
                    assert!((c[ch] as f32 - want[i]).abs() <= 2.0, "centre channel {i} is {} but the CPU says {:.1} ({p:?}, dark {dark}, warp {warp})", c[ch], want[i]);
                }
            }
        }
    }
}

#[test]
fn the_frost_setting_widens_an_edge() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    let src = picture(sw, sh, |x, _| if x < 320 { [20; 3] } else { [230; 3] });
    let rect = (120, 60, 400u32, 280u32);
    for warp in drivers() {
        let width = |frost: f32| {
            let look = Look { blur: Look::regular(false, &params(frost, 50.0, 67.0, 62.0)).blur, ..bare_optics() };
            step_width(&render_with(&src, sw, sh, rect, 8.0, &look, warp).expect("render"), rect.2, rect.3 / 2)
        };
        let (clear, normal, full) = (width(0.0), width(52.0), width(100.0));
        assert!(clear <= 3, "frost 0 is clear glass, yet the edge is {clear} px wide (warp {warp})");
        assert!((18..=38).contains(&normal), "frost 52 should make the edge about 26 px wide, it is {normal} (warp {warp})");
        // a deviation of 20 px: 2.56 sigma = 51 px between 10 % and 90 %
        assert!((42..=64).contains(&full), "frost 100 should make the edge about 51 px wide, it is {full} (warp {warp})");
    }
}

#[test]
fn the_refraction_setting_moves_the_rim() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    let src = picture(sw, sh, |x, y| [(x / 3) as u8, (y / 2) as u8, 60]);
    let rect = (160, 60, 320u32, 280u32);
    let bare = |p: LookParams| Look { blur: 0.0, saturate: 1.0, lum: 1.0, contrast: 1.0, tint: [1.0, 1.0, 1.0, 0.0], rim: 0.0, shade: 0.0, ..Look::regular(false, &p) };
    for warp in drivers() {
        let draw = |l: &Look| render_with(&src, sw, sh, rect, 8.0, l, warp).expect("render");
        let (none, normal, thick) = (draw(&bare(params(52.0, 50.0, 0.0, 62.0))), draw(&bare(LookParams::DEFAULT)), draw(&bare(params(52.0, 50.0, 100.0, 62.0))));
        // refraction 0 is a glass of no thickness: the picture as it is, also at the rim
        let flat = draw(&Look { depth: 0.0, ..bare(LookParams::DEFAULT) });
        assert!(none == flat, "refraction 0 is not the same as a glass with no thickness (warp {warp})");
        // 3 px inside the left edge: the further the rays turn, the further from the straight picture
        let red = |img: &[u8]| px(img, rect.2, 3, rect.3 / 2)[2] as i32;
        let (a, b, c) = (red(&none), red(&normal), red(&thick));
        assert!(a < b && b < c, "red at the rim: {a} (refraction 0), {b} (default), {c} (100) must rise (warp {warp})");
    }
}

#[test]
fn the_saturation_setting_strengthens_the_colours() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (480u32, 360u32);
    let rect = (60, 40, 360u32, 280u32);
    let src = picture(sw, sh, |_, _| [200, 120, 90]);
    for warp in drivers() {
        let chroma = |s: f32| {
            let look = Look { tint: [1.0, 1.0, 1.0, 0.0], contrast: 1.0, ..Look::regular(false, &params(52.0, 50.0, 67.0, s)) };
            let p = px(&render_with(&src, sw, sh, rect, 8.0, &look, warp).expect("render"), rect.2, rect.2 / 2, rect.3 / 2);
            p[2].max(p[1]).max(p[0]) as i32 - p[2].min(p[1]).min(p[0]) as i32
        };
        let (zero, normal, full) = (chroma(0.0), chroma(62.0), chroma(100.0));
        // factor 1.0 leaves the picture's own colours: 200 - 90 = 110
        assert!((zero - 110).abs() <= 2, "saturation 0 changed the colours: chroma {zero} (warp {warp})");
        assert!(zero < normal && normal < full, "chroma {zero} / {normal} / {full} must rise (warp {warp})");
    }
}
