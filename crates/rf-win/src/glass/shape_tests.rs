//! Tests of a glass SHAPE inside a larger transparent window and of its shadow (what the dock's islands use). The numbers are checked
//! without a GPU; what the shader does with them goes through the offscreen path like the other drawing tests (skipped where `can_draw`
//! says so, with the shared renderer and `TEST_QUIET`).

use super::gpu::{self, Geometry, Look, clamp_rect, fingerprint_texels};
use super::tests::{bare_optics, can_draw, drivers, picture, px, with_renderer};
use super::*;

/// A geometry for a window of `w` x `h` at (`x`, `y`) in the picture, scale 1, with a shape and a shadow.
fn geo(window: (i32, i32, u32, u32), shape: Option<[f32; 4]>, shadow: Option<Shadow>, radius: f32) -> Geometry {
    Geometry { w: window.2, h: window.3, origin: (window.0, window.1), scale: 1.0, radius_px: radius, cut_corners: true, shape, shadow }
}

fn draw(src: &[u8], sw: u32, sh: u32, g: &Geometry, look: &Look, warp: bool) -> Vec<u8> {
    with_renderer(warp, |r| unsafe { gpu::render_on(r, src, sw, sh, g, look) }).expect("render")
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------------------------------------------------------------

#[test]
fn the_glass_rectangle_is_the_shape_cut_to_the_window() {
    let g = |shape| geo((0, 0, 400, 200), shape, None, 24.0);
    assert_eq!(g(None).glass_rect(), [0.0, 0.0, 400.0, 200.0], "no shape: the whole window, as always");
    assert_eq!(g(Some([28.0, 28.0, 300.0, 48.0])).glass_rect(), [28.0, 28.0, 300.0, 48.0]);
    // sticking out on the right and the bottom: cut
    assert_eq!(g(Some([350.0, 150.0, 100.0, 100.0])).glass_rect(), [350.0, 150.0, 50.0, 50.0]);
    // sticking out on the left and the top: the part inside stays where it is
    assert_eq!(g(Some([-20.0, -10.0, 60.0, 40.0])).glass_rect(), [0.0, 0.0, 40.0, 30.0]);
    // wholly outside, negative size, not a number: nothing (never the whole window)
    assert_eq!(g(Some([500.0, 0.0, 10.0, 10.0])).glass_rect()[2], 0.0);
    assert_eq!(g(Some([10.0, 10.0, -50.0, 20.0])).glass_rect()[2], 0.0);
    assert_eq!(g(Some([f32::NAN, 0.0, 10.0, 10.0])).glass_rect(), [0.0; 4]);
    assert_eq!(g(Some([0.0, 0.0, f32::INFINITY, 10.0])).glass_rect(), [0.0; 4]);
    assert_eq!(clamp_rect([1.0, 2.0, 3.0, 4.0], 10.0, 10.0), [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn options_with_a_shape_are_cleaned_up() {
    let o = GlassOptions { shape: Some([f32::NAN, 1.0, 2.0, 3.0]), ..GlassOptions::window(true) }.sanitized();
    assert_eq!(o.shape, Some([0.0; 4]), "a shape that is not a number draws nothing, not the whole window");
    let o = GlassOptions { shape: Some([5.0, 6.0, -7.0, 8.0]), ..GlassOptions::window(true) }.sanitized();
    assert_eq!(o.shape, Some([5.0, 6.0, 0.0, 8.0]));
    assert_eq!(GlassOptions::window(true).sanitized().shape, None);
    // shadows: a useful one stays, one that draws nothing or cannot be drawn is none
    let with = |s| GlassOptions { shadow: Some(s), ..GlassOptions::window(true) }.sanitized().shadow;
    assert_eq!(with(Shadow::SOFT), Some(Shadow::SOFT));
    assert_eq!(with(Shadow { opacity: 0.0, ..Shadow::SOFT }), None);
    assert_eq!(with(Shadow { blur: 0.0, ..Shadow::SOFT }), None);
    assert_eq!(with(Shadow { opacity: f32::NAN, ..Shadow::SOFT }), None);
    assert_eq!(with(Shadow { dy: f32::INFINITY, ..Shadow::SOFT }), None);
    assert_eq!(with(Shadow { opacity: 3.0, ..Shadow::SOFT }).map(|s| s.opacity), Some(1.0));
    // what changes only the shape
    let a = GlassOptions { shape: Some([1.0, 2.0, 3.0, 4.0]), shadow: Some(Shadow::SOFT), ..GlassOptions::window(true) };
    assert!(a.only_the_shape_differs(&GlassOptions { shape: Some([9.0, 2.0, 30.0, 4.0]), ..a }));
    assert!(a.only_the_shape_differs(&GlassOptions { shadow: None, ..a }));
    assert!(!a.only_the_shape_differs(&GlassOptions { dark: false, ..a }));
    assert!(!a.only_the_shape_differs(&GlassOptions { radius_px: Some(3.0), ..a }));
    assert!(!a.only_the_shape_differs(&GlassOptions { cut_corners: true, ..a }));
    // what changes the shape and the radius together (a growing island) is as cheap; the radius alone is not
    let r = GlassOptions { radius_px: Some(24.0), cut_corners: true, ..a };
    assert!(r.only_the_shape_and_radius_differ(&GlassOptions { radius_px: Some(9.0), shape: Some([9.0, 2.0, 30.0, 40.0]), ..r }));
    assert!(!r.only_the_shape_and_radius_differ(&GlassOptions { radius_px: Some(9.0), ..r }), "a radius without a new shape is a DPI change");
    assert!(!r.only_the_shape_and_radius_differ(&GlassOptions { radius_px: Some(9.0), shape: Some([9.0, 2.0, 30.0, 40.0]), dark: false, ..r }));
    assert!(!a.only_the_shape_and_radius_differ(&GlassOptions { radius_px: Some(9.0), shape: Some([9.0, 2.0, 30.0, 40.0]), ..a }), "no radius asked for: the system's");
}

/// The shadow: 26 % black at the outline of the moved shape, gone `blur` px outside it, smooth, never negative or above its opacity.
#[test]
fn the_shadow_falls_smoothly_from_the_outline_to_nothing() {
    let s = Shadow::SOFT;
    assert_eq!((s.opacity, s.dy, s.blur), (0.26, 4.0, 22.0));
    assert!((s.alpha(0.0) - 0.26).abs() < 1e-6, "at the outline: 26 %");
    assert_eq!(s.alpha(-10.0), 0.26, "under the moved shape (just below the glass) it is as dark as at the outline");
    assert_eq!(s.alpha(22.0), 0.0);
    assert_eq!(s.alpha(500.0), 0.0);
    let mut last = s.alpha(0.0);
    let mut max_step = 0.0f32;
    for i in 1..=220 {
        let a = s.alpha(i as f32 / 10.0);
        assert!(a <= last + 1e-7 && (0.0..=0.26 + 1e-6).contains(&a), "monotonic, in range at {i}");
        max_step = max_step.max(last - a);
        last = a;
    }
    assert!(max_step < 0.006, "no jump anywhere: {max_step}");
    // no kink at either end (zero slope), so the shadow has no visible edge
    assert!((s.alpha(0.0) - s.alpha(0.1)).abs() < 1e-4 && (s.alpha(21.9) - s.alpha(22.0)).abs() < 1e-4);
    // halfway it is half
    assert!((s.alpha(11.0) - 0.13).abs() < 1e-6);
    // below the glass the moved outline is `dy` further down: the shadow is darker below than above
    let below_edge = s.alpha(10.0 - s.dy); // 10 px under the glass's bottom edge
    let above_edge = s.alpha(10.0 + s.dy); // 10 px over its top edge
    assert!(below_edge > above_edge);
    assert_eq!(s.reach(), 26.0);
}

/// The constants the shader gets for a shape: its half size, and where its centre is from the window's centre (the sub pixel placement).
#[test]
fn the_shape_is_placed_by_its_centre() {
    for (win, shape, want_sub) in [
        ((600u32, 200u32), [28.0f32, 28.0, 300.0, 48.0], (28.0 + 150.0 - 300.0, 28.0 + 24.0 - 100.0)),
        ((600, 200), [0.0, 0.0, 600.0, 200.0], (0.0, 0.0)),
        ((601, 200), [10.5, 20.0, 100.0, 50.0], (10.5 + 50.0 - 300.5, 20.0 + 25.0 - 100.0)),
    ] {
        let g = geo((0, 0, win.0, win.1), Some(shape), None, 24.0);
        assert_eq!(g.centre_shift(), [want_sub.0, want_sub.1]);
    }
    // a window that is a whole glass has no placement at all (its constants are the ones it always had)
    let w = geo((0, 0, 1100, 640), None, None, 8.0);
    assert_eq!(w.centre_shift(), [0.0, 0.0]);
}

/// The loop guard looks at what the SHAPE can see: for a glass that is the whole window the rectangle it always read, for a shape the
/// shape and its padding only.
#[test]
fn the_loop_guard_reads_the_shapes_backdrop_not_the_windows() {
    let pad = 96u32;
    let level = 3u32;
    let size = (512u32 >> level, 512u32 >> level);
    // no shape: from 0, as wide as the glass and its padding: (300 + 192) / 8 = 61.5 -> 62 texels, (100 + 192) / 8 = 36.5 -> 37
    assert_eq!(fingerprint_texels([0.0, 0.0, 300.0, 100.0], pad, level, size), (0, 62, 0, 37));
    // a shape well inside a window: it starts at its own x (region px = window px + pad, the padded area starts at x - pad + pad)
    assert_eq!(fingerprint_texels([128.0, 40.0, 100.0, 48.0], pad, level, size), (16, 53, 5, 35));
    // cut to the region
    assert_eq!(fingerprint_texels([400.0, 200.0, 500.0, 500.0], pad, level, size), (50, 64, 25, 64));
    // nothing: an empty range, never a negative one
    let (x0, x1, y0, y1) = fingerprint_texels([0.0; 4], 0, level, size);
    assert!(x1 >= x0 && y1 >= y0);
    let (x0, x1, _, _) = fingerprint_texels([1000.0, 0.0, 10.0, 10.0], pad, level, size);
    assert!(x1 >= x0);
    // moving the shape moves what is hashed, so an island that widens is not mistaken for an unchanged backdrop
    assert_ne!(fingerprint_texels([100.0, 40.0, 300.0, 48.0], pad, level, size), fingerprint_texels([60.0, 40.0, 380.0, 48.0], pad, level, size));
}

/// The brightness and the loop guard's fingerprint are about the same texels: the brightness's are the glass (the shape for a shape),
/// the fingerprint's the glass and its padding, so the first always lie inside the second. Whatever the shape, wherever it is.
#[test]
fn the_brightness_looks_at_the_texels_of_the_shape_the_fingerprint_hashes() {
    let pad = 96u32;
    for level in 0..6u32 {
        let size = (((1024u32) >> level).max(1), ((768u32) >> level).max(1));
        for shape in [[0.0, 0.0, 300.0, 100.0], [128.0, 40.0, 100.0, 48.0], [28.0, 28.0, 300.0, 48.0], [400.0, 200.0, 500.0, 500.0], [3.0, 5.0, 7.0, 9.0]] {
            let (fx0, fx1, fy0, fy1) = fingerprint_texels(shape, pad, level, size);
            let (lx0, ly0, lx1, ly1) = luma::glass_texels(level, pad, shape, size.0, size.1);
            assert!(lx0 < lx1 && ly0 < ly1, "level {level}, {shape:?}: the shape has a texel");
            assert!(fx0 <= lx0 && lx1 <= fx1 && fy0 <= ly0 && ly1 <= fy1, "level {level}, {shape:?}: brightness {lx0}..{lx1} x {ly0}..{ly1} inside hash {fx0}..{fx1} x {fy0}..{fy1}");
        }
    }
    // a shape moves the texels the brightness counts; a glass that is the whole window starts at the padding as it always did
    assert_ne!(luma::glass_texels(2, pad, [28.0, 28.0, 300.0, 48.0], 128, 128), luma::glass_texels(2, pad, [0.0, 0.0, 356.0, 104.0], 128, 128));
    assert_eq!(luma::glass_texels(0, pad, [0.0, 0.0, 300.0, 200.0], 512, 512), (96, 96, 396, 296));
    assert_eq!(luma::glass_texels(0, pad, [28.0, 20.0, 300.0, 48.0], 512, 512), (124, 116, 424, 164));
    // no area, no texel (never the one under the corner of the window)
    assert_eq!(luma::glass_texels(2, pad, [10.0, 10.0, 0.0, 30.0], 128, 128), (0, 0, 0, 0));
    assert_eq!(luma::glass_texels(2, pad, [f32::NAN, 0.0, 30.0, 30.0], 128, 128), (0, 0, 0, 0));
    assert_eq!(luma::glass_texels(2, pad, [0.0; 4], 128, 128), (0, 0, 0, 0));
}

/// The brightness a glass reports is that of the backdrop under its SHAPE: white behind the shape in a dark scene is near 1, white in
/// the rest of the window (the transparent margin) does not count, while the same scene seen by a glass that is the whole window does
/// count it. This is what an island of the dock (a shape in a larger window) turns its text by.
#[test]
fn the_brightness_under_a_shape_is_that_of_the_shape_and_not_of_its_window() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (700u32, 600u32);
    // the window lies at 100,100 on the screen and is 300 x 200; the shape is 120 x 48 at 60,70 in it: the screen's 160..280 x 170..218
    let window = (100, 100, 300, 200);
    let shape = [60.0, 70.0, 120.0, 48.0];
    let with_shape = geo(window, Some(shape), None, 24.0);
    let plain = geo(window, None, None, 8.0);
    let look = Look::regular(false, &LookParams::DEFAULT);
    let in_window = |x: u32, y: u32| (100..400).contains(&x) && (100..300).contains(&y);
    let near_shape = |x: u32, y: u32| (148..292).contains(&x) && (158..230).contains(&y); // the shape and 12 px around it
    let behind_shape = |x: u32, y: u32| (160..280).contains(&x) && (170..218).contains(&y);
    with_renderer(true, |r| unsafe {
        let measure = |g: &Geometry, data: &[u8]| {
            let region = gpu::Region::new(&r.g, g.w, g.h, gpu::region_pad(g.scale)).expect("region");
            let src = gpu::Source::new(&r.g, sw, sh, Some(data)).expect("source");
            gpu::prep(r, &region, &src, g, &look, false);
            region.read_back(&r.g, g.glass_rect()).expect("read back").luma.expect("a glass has texels")
        };
        let white_behind_the_shape = picture(sw, sh, |x, y| if behind_shape(x, y) { [255, 255, 255] } else { [0, 0, 0] });
        let white_in_the_margin = picture(sw, sh, |x, y| if in_window(x, y) && !near_shape(x, y) { [255, 255, 255] } else { [0, 0, 0] });
        let a = measure(&with_shape, &white_behind_the_shape);
        let b = measure(&with_shape, &white_in_the_margin);
        let c = measure(&plain, &white_in_the_margin);
        let d = measure(&plain, &white_behind_the_shape);
        println!("shape: white behind it {a:.3}, white in the margin {b:.3}; whole-window glass: margin {c:.3}, behind the shape's place {d:.3}");
        assert!(a > 0.8, "white behind the shape: {a}");
        assert!(b < 0.12, "white in the window's margin, not behind the shape: {b}");
        assert!(c > 0.5, "the same margin is most of a glass that is the whole window: {c}");
        assert!(d < 0.2, "a small bright place is little of the whole window: {d}");
        // a mid grey behind the shape reads as a mid grey, whatever is around it
        let grey = picture(sw, sh, |x, y| if behind_shape(x, y) { [128, 128, 128] } else { [255, 255, 255] });
        let g = measure(&with_shape, &grey);
        assert!((0.3..0.7).contains(&g), "mid grey behind the shape, white around it: {g}");
    });
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------------------------------------------

/// A shape inside a bigger window: its middle is opaque glass, outside it (with no shadow) every pixel is fully transparent, also in
/// the window's corners and right beside the outline.
#[test]
fn a_shape_is_glass_inside_and_nothing_outside() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (700u32, 400u32);
    let src = picture(sw, sh, |x, y| [(x % 251) as u8, (y % 241) as u8, 120]);
    let window = (100, 80, 500u32, 200u32);
    let shape = [60.0, 70.0, 300.0, 48.0];
    let look = Look::regular(true, &LookParams::DEFAULT).sized(48.0);
    for warp in drivers() {
        let out = draw(&src, sw, sh, &geo(window, Some(shape), None, 24.0), &look, warp);
        assert_eq!(out.len(), (window.2 * window.3 * 4) as usize);
        assert_eq!(px(&out, window.2, 210, 94)[3], 255, "the centre of the shape (warp {warp})");
        assert_eq!(px(&out, window.2, 70, 94)[3], 255, "near the left end, in the middle of the capsule's end (warp {warp})");
        // outside: every pixel at least 2 px from the shape's rectangle is exactly nothing
        let mut visible = 0;
        for y in 0..window.3 {
            for x in 0..window.2 {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let outside = fx < shape[0] - 2.0 || fx > shape[0] + shape[2] + 2.0 || fy < shape[1] - 2.0 || fy > shape[1] + shape[3] + 2.0;
                if outside && px(&out, window.2, x, y) != [0, 0, 0, 0] {
                    visible += 1;
                }
            }
        }
        assert_eq!(visible, 0, "{visible} pixels outside the shape are not transparent (warp {warp})");
        // the capsule's corners are cut by the shape's own radius
        assert_eq!(px(&out, window.2, 61, 71)[3], 0, "the shape's top left corner (warp {warp})");
    }
}

/// The backdrop behind a shape is where the screen has it: the same glass drawn as a free-standing piece at the same place on the
/// screen gives the same picture (the shape's constants and the window's region do not shift it), with the optics bare so that no
/// blur level's alignment comes into it, and with the settings' frost on a smooth picture.
#[test]
fn the_backdrop_behind_a_shape_is_where_the_screen_has_it() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (700u32, 400u32);
    let window = (100, 80, 500u32, 200u32);
    let shape = [60.0f32, 70.0, 300.0, 48.0];
    let screen = (100 + 60, 80 + 70, 300u32, 48u32);
    // sharp, strongly structured picture, no frost: any shift of the backdrop shows at once
    let sharp = picture(sw, sh, |x, y| [(x % 13 * 19) as u8, (y % 7 * 36) as u8, ((x / 5 + y / 3) % 2 * 200) as u8]);
    let smooth = picture(sw, sh, |x, y| [(x / 3) as u8, (y / 2) as u8, 90]);
    for warp in drivers() {
        for (name, src, look, tolerance) in [
            ("bare optics, sharp picture", &sharp, Look { depth: 1.6, ..bare_optics() }, 3i32),
            ("the settings' look, smooth picture", &smooth, Look::regular(false, &LookParams::DEFAULT).sized(48.0), 4),
        ] {
            let inside = draw(src, sw, sh, &geo(window, Some(shape), None, 24.0), &look, warp);
            let alone = draw(src, sw, sh, &geo(screen, None, None, 24.0), &look, warp);
            let mut worst = 0i32;
            for y in 0..48u32 {
                for x in 0..300u32 {
                    let a = px(&inside, window.2, 60 + x, 70 + y);
                    let b = px(&alone, 300, x, y);
                    for c in 0..4 {
                        worst = worst.max((a[c] as i32 - b[c] as i32).abs());
                    }
                }
            }
            assert!(worst <= tolerance, "{name}: a shape inside a window differs from the same glass alone by up to {worst} (warp {warp})");
        }
    }
}

/// Moving or widening the shape inside the window never shifts the picture behind it: the pixels that are glass in both pictures and
/// far from either outline (the clear centre) are equal.
#[test]
fn moving_the_shape_does_not_move_the_backdrop() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (700u32, 400u32);
    let src = picture(sw, sh, |x, y| [(x * 2 % 256) as u8, (y * 3 % 256) as u8, ((x + y) % 256) as u8]);
    let window = (100, 80, 500u32, 260u32);
    let look = Look { blur: 0.0, ..bare_optics() };
    for warp in drivers() {
        // the second shape is moved by a fraction of a pixel and made wider; the rim is 44 px wide on both
        let a = draw(&src, sw, sh, &geo(window, Some([40.0, 50.0, 300.0, 120.0]), None, 24.0), &look, warp);
        let b = draw(&src, sw, sh, &geo(window, Some([71.5, 62.5, 340.0, 120.0]), None, 24.0), &look, warp);
        // window x 118..294, y 108..125: inside both shapes' clear centres (x 84..296 and 115.5..367.5, y 94..126 and 106.5..138.5)
        for y in 108..125u32 {
            for x in 118..294u32 {
                let (p, q) = (px(&a, window.2, x, y), px(&b, window.2, x, y));
                for c in 0..3 {
                    assert!((p[c] as i32 - q[c] as i32).abs() <= 1, "the picture behind the shape moved at ({x}, {y}) channel {c} (warp {warp})");
                }
            }
        }
    }
}

/// The shadow: with it off nothing outside the shape is drawn; on, the pixels beside and below the glass are black with the opacity the
/// falloff says; the glass itself is the same picture with or without it.
#[test]
fn the_shadow_is_outside_the_glass_and_follows_its_falloff() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (700u32, 400u32);
    let src = picture(sw, sh, |x, y| [(x % 251) as u8, (y % 241) as u8, 120]);
    let window = (100, 80, 500u32, 200u32);
    let shape = [100.0f32, 60.0, 300.0, 72.0];
    let look = Look::regular(false, &LookParams::DEFAULT).sized(72.0);
    let s = Shadow::SOFT;
    for warp in drivers() {
        let plain = draw(&src, sw, sh, &geo(window, Some(shape), None, 24.0), &look, warp);
        let shaded = draw(&src, sw, sh, &geo(window, Some(shape), Some(s), 24.0), &look, warp);
        // the glass is untouched: every pixel that is glass (alpha 255 in the plain picture) is the same pixel
        let mut changed = 0;
        for i in 0..plain.len() / 4 {
            if plain[i * 4 + 3] == 255 && plain[i * 4..i * 4 + 4] != shaded[i * 4..i * 4 + 4] {
                changed += 1;
            }
        }
        assert_eq!(changed, 0, "{changed} pixels of the glass changed with the shadow on (warp {warp})");
        // below the glass: row k px under the bottom edge (y = 132 + k), at the middle: the moved outline is dy further down
        let mid = 250u32;
        for k in [0u32, 2, 6, 10, 14, 18, 24, 28] {
            let p = px(&shaded, window.2, mid, 132 + k);
            let want = (s.alpha(k as f32 + 0.5 - s.dy) * 255.0).round() as i32;
            assert!((p[3] as i32 - want).abs() <= 3, "{k} px below the glass the shadow is {} of 255, the falloff says {want} (warp {warp})", p[3]);
            assert_eq!(&p[..3], &[0, 0, 0], "the shadow is black (premultiplied), {k} px below (warp {warp})");
        }
        // beside the glass: k px left of the left end at the middle height (the capsule's end is round: use the vertical middle)
        for k in [3u32, 8, 14, 20] {
            let p = px(&shaded, window.2, 100 - k, 96);
            let want = (s.alpha(k as f32 - 0.5) * 255.0).round() as i32;
            assert!((p[3] as i32 - want).abs() <= 4, "{k} px left of the glass the shadow is {} of 255, the falloff says {want} (warp {warp})", p[3]);
        }
        // above it is lighter than below
        assert!(px(&shaded, window.2, mid, 60 - 6)[3] < px(&shaded, window.2, mid, 132 + 6)[3], "darker below than above (warp {warp})");
        // far away: nothing
        assert_eq!(px(&shaded, window.2, 5, 5), [0, 0, 0, 0], "the corner of the window (warp {warp})");
        assert_eq!(px(&shaded, window.2, mid, 132 + 40)[3], 0);
    }
}

/// An empty shape draws nothing at all: not the whole window, not a dot.
#[test]
fn an_empty_shape_draws_nothing() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (300u32, 200u32);
    let src = picture(sw, sh, |x, y| [(x % 251) as u8, (y % 241) as u8, 120]);
    let look = Look::regular(true, &LookParams::DEFAULT);
    for shape in [[50.0f32, 50.0, 0.0, 0.0], [f32::NAN, 0.0, 10.0, 10.0], [500.0, 500.0, 10.0, 10.0]] {
        let out = draw(&src, sw, sh, &geo((0, 0, 200, 120), Some(shape), None, 24.0), &look, true);
        assert!(out.as_chunks::<4>().0.iter().all(|p| p[3] == 0), "{shape:?} drew something");
    }
}

/// A shape that is the whole window is the glass of the window itself (same pixels as no shape with its corners cut).
#[test]
fn a_shape_that_is_the_whole_window_is_the_plain_glass() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (500u32, 360u32);
    let src = picture(sw, sh, |x, y| [(x % 251) as u8, (y % 241) as u8, 120]);
    let window = (60, 40, 360u32, 280u32);
    let look = Look::regular(false, &LookParams::DEFAULT);
    for warp in drivers() {
        let whole = draw(&src, sw, sh, &geo(window, None, None, 8.0), &look, warp);
        let shaped = draw(&src, sw, sh, &geo(window, Some([0.0, 0.0, 360.0, 280.0]), None, 8.0), &look, warp);
        assert!(whole == shaped, "the whole window as a shape differs from no shape (warp {warp})");
    }
}
