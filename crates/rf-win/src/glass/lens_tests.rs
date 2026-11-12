//! Windowless tests of the lenses' drawing, through the same offscreen path as `tests.rs`: a hardware device or WARP, skipped where the
//! other drawing tests are skipped (`can_draw`), on the shared test renderer. The motion and the slot rules are tested without a GPU in
//! `spring.rs` and `lens.rs`.

use super::gpu::{self, Geometry, LensDraw, LensList, Look};
use super::tests::{can_draw, drivers, picture, px, symmetric_stripes, with_renderer};
use super::*;

/// The glass the lens tests draw: 320 x 280, scale 1, radius 8, its corners cut (a free-standing shape).
const GLASS: (i32, i32, u32, u32) = (160, 60, 320, 280);

/// A lens, clipped to the whole glass unless told otherwise; a capsule.
fn lens_at(rect: [f32; 4], opacity: f32) -> LensDraw {
    LensDraw { rect, clip: [0.0, 0.0, GLASS.2 as f32, GLASS.3 as f32], radius: rect[3] / 2.0, opacity }
}

/// Draws the glass with exactly these lenses (not filtered: an opacity of 0 reaches the shader as it is).
fn render_lenses(src: &[u8], sw: u32, sh: u32, look: &Look, lenses: &[LensDraw], warp: bool) -> Vec<u8> {
    let geo = Geometry { w: GLASS.2, h: GLASS.3, origin: (GLASS.0, GLASS.1), scale: 1.0, radius_px: 8.0, cut_corners: true };
    let mut list = LensList { n: lenses.len(), ..LensList::default() };
    list.l[..lenses.len()].copy_from_slice(lenses);
    with_renderer(warp, |r| unsafe { gpu::render_on_lenses(r, src, sw, sh, &geo, look, &list) }).expect("render")
}

/// The largest difference of any channel between two pictures over the pixels `inside` selects, and where.
fn worst_diff(a: &[u8], b: &[u8], w: u32, inside: impl Fn(u32, u32) -> bool) -> (i32, u32, u32) {
    let mut worst = (0, 0, 0);
    for y in 0..a.len() as u32 / 4 / w {
        for x in 0..w {
            if inside(x, y) {
                let (p, q) = (px(a, w, x, y), px(b, w, x, y));
                for c in 0..4 {
                    worst = worst.max(((p[c] as i32 - q[c] as i32).abs(), x, y));
                }
            }
        }
    }
    worst
}

fn gradient_source() -> (u32, u32, Vec<u8>) {
    (640, 400, picture(640, 400, |x, y| [(x / 3 % 256) as u8, (y / 2 % 256) as u8, 60]))
}

#[test]
fn a_lens_of_no_opacity_or_none_leaves_the_picture_as_it_is() {
    if !can_draw() {
        return;
    }
    let (sw, sh, src) = gradient_source();
    for dark in [false, true] {
        for warp in drivers() {
            let look = Look::regular(dark);
            let none = render_lenses(&src, sw, sh, &look, &[], warp);
            // the lens reaches the shader with opacity 0, and with a rectangle that would show if it counted
            let zero = render_lenses(&src, sw, sh, &look, &[lens_at([60.0, 100.0, 200.0, 36.0], 0.0)], warp);
            assert!(none == zero, "an invisible lens changed the picture (dark {dark}, warp {warp})");
            // drawn twice, the same: nothing in the picture depends on anything but its input
            assert!(none == render_lenses(&src, sw, sh, &look, &[], warp), "the picture is not repeatable (dark {dark}, warp {warp})");
        }
    }
}

#[test]
fn a_lens_changes_its_own_rectangle_and_by_its_shadow_a_few_px_around_it_and_nothing_further() {
    if !can_draw() {
        return;
    }
    let (sw, sh, src) = gradient_source();
    let lens = [60.0f32, 100.0, 200.0, 36.0];
    for dark in [false, true] {
        for warp in drivers() {
            let look = Look::regular(dark);
            let none = render_lenses(&src, sw, sh, &look, &[], warp);
            let with = render_lenses(&src, sw, sh, &look, &[lens_at(lens, 1.0)], warp);
            let w = GLASS.2;
            // how far a pixel is from the lens rectangle, px (0 inside)
            let away = |x: u32, y: u32| {
                let dx = (lens[0] - x as f32).max(x as f32 - (lens[0] + lens[2] - 1.0)).max(0.0);
                let dy = (lens[1] - y as f32).max(y as f32 - (lens[1] + lens[3] - 1.0)).max(0.0);
                (dx * dx + dy * dy).sqrt()
            };
            // inside: another picture (the lens reads the backdrop sharp, the glass around it frosted, and its grade is its own)
            let inside = worst_diff(&none, &with, w, |x, y| away(x, y) == 0.0 && (x as f32 - lens[0]).min(lens[0] + lens[2] - x as f32) > 20.0);
            assert!(inside.0 >= 6, "the lens hardly changed its own rectangle: {inside:?} (dark {dark}, warp {warp})");
            // the shadow: a few px below the lens, darker than without it
            let (a, b) = (px(&none, w, 160, 136 + 3), px(&with, w, 160, 136 + 3));
            let lum = |p: [u8; 4]| p[0] as i32 + p[1] as i32 + p[2] as i32;
            assert!(lum(b) < lum(a) - 3, "no shadow 3 px below the lens: {a:?} without, {b:?} with (dark {dark}, warp {warp})");
            // far from it nothing changed: exactly beyond the shadow (its tail is under a millionth), within one level just before
            let far = worst_diff(&none, &with, w, |x, y| away(x, y) > 40.0);
            assert_eq!(far.0, 0, "the picture changed 40 px or more away from the lens: first at x {} y {} (dark {dark}, warp {warp})", far.1, far.2);
            let near = worst_diff(&none, &with, w, |x, y| away(x, y) > 24.0);
            assert!(near.0 <= 1, "the shadow reaches further than 24 px: {near:?} (dark {dark}, warp {warp})");
        }
    }
}

#[test]
fn the_clip_rectangle_cuts_the_lens_and_its_shadow() {
    if !can_draw() {
        return;
    }
    let (sw, sh, src) = gradient_source();
    let lens = [60.0f32, 100.0, 200.0, 36.0];
    for warp in drivers() {
        let look = Look::regular(true);
        let none = render_lenses(&src, sw, sh, &look, &[], warp);
        let mut cut = lens_at(lens, 1.0);
        cut.clip = [0.0, 0.0, 160.0, 280.0]; // the right part of the lens (from x 160) is outside
        let with = render_lenses(&src, sw, sh, &look, &[cut], warp);
        let w = GLASS.2;
        let right = worst_diff(&none, &with, w, |x, _| x >= 162);
        assert_eq!(right.0, 0, "something of the lens shows right of its clip rectangle: {right:?} (warp {warp})");
        let left = worst_diff(&none, &with, w, |x, y| (80..150).contains(&x) && (105..131).contains(&y));
        assert!(left.0 >= 6, "inside the clip rectangle the lens is gone too: {left:?} (warp {warp})");
        // a clip that is empty shows nothing at all
        let mut empty = lens_at(lens, 1.0);
        empty.clip = [10.0, 10.0, 0.0, 0.0];
        assert!(none == render_lenses(&src, sw, sh, &look, &[empty], warp), "a lens with an empty clip rectangle shows (warp {warp})");
    }
}

#[test]
fn half_the_opacity_is_half_way_between_nothing_and_the_lens() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    let src = picture(sw, sh, |_, _| [200; 3]);
    let lens = [60.0f32, 100.0, 200.0, 36.0];
    for dark in [false, true] {
        for warp in drivers() {
            let look = Look::regular(dark);
            let at = |lenses: &[LensDraw]| {
                let out = render_lenses(&src, sw, sh, &look, lenses, warp);
                px(&out, GLASS.2, 160, 118)
            };
            let (none, half, full) = (at(&[]), at(&[lens_at(lens, 0.5)]), at(&[lens_at(lens, 1.0)]));
            // the three colour channels (BGRA order: 0 to 2)
            let differs = (0..3).any(|c| (none[c] as f32 - full[c] as f32).abs() >= 8.0);
            assert!(differs, "the lens must differ from the glass for this test to say anything: {none:?} {full:?} (dark {dark}, warp {warp})");
            for c in 0..3 {
                let (n, h, f) = (none[c] as f32, half[c] as f32, full[c] as f32);
                assert!((h - (n + f) / 2.0).abs() <= 2.0, "channel {c}: {n} without, {h} at half opacity, {f} at full (dark {dark}, warp {warp})");
            }
        }
    }
}

#[test]
fn a_lens_over_a_symmetric_source_is_symmetric() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    let src = symmetric_stripes(sw, sh);
    // centred on the glass, which is centred on the source's axis; the light has a direction, so the lens's highlight and dark line are
    // switched off (as the window's rim light is in the test of the window)
    let lens = [60.0f32, 100.0, 200.0, 36.0];
    let mut look = Look::regular(false);
    look.rim = 0.0;
    look.shade = 0.0;
    look.lens.highlight = 0.0;
    look.lens.dark_line = 0.0;
    for warp in drivers() {
        let out = render_lenses(&src, sw, sh, &look, &[lens_at(lens, 1.0)], warp);
        let w = GLASS.2;
        let mut worst = (0i32, 0u32, 0u32);
        for y in 0..GLASS.3 {
            for x in 0..w / 2 {
                let (a, b) = (px(&out, w, x, y), px(&out, w, w - 1 - x, y));
                for c in 0..4 {
                    worst = worst.max(((a[c] as i32 - b[c] as i32).abs(), x, y));
                }
            }
        }
        assert!(worst.0 <= 2, "left and right halves differ by up to {} of 255, first at x {} y {} (warp {warp})", worst.0, worst.1, worst.2);
        // and the lens is really in the picture: its middle differs from the window glass there
        let none = render_lenses(&src, sw, sh, &look, &[], warp);
        assert!(worst_diff(&none, &out, w, |x, y| (100..220).contains(&x) && (105..131).contains(&y)).0 >= 6, "the lens is not there (warp {warp})");
    }
}

#[test]
fn lenses_of_every_shape_make_no_non_finite_pixel() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    let src = symmetric_stripes(sw, sh);
    let sets: Vec<Vec<LensDraw>> = vec![
        vec![lens_at([60.0, 100.0, 200.0, 36.0], 1.0)],
        // a circle, a tiny one with a radius bigger than its size, a tall one
        vec![lens_at([100.0, 100.0, 36.0, 36.0], 1.0), LensDraw { radius: 500.0, ..lens_at([180.0, 20.0, 8.0, 8.0], 1.0) }, lens_at([200.0, 150.0, 20.0, 110.0], 1.0)],
        // ones that stick out of the glass
        vec![lens_at([-20.0, -10.0, 100.0, 40.0], 1.0), lens_at([300.0, 260.0, 60.0, 40.0], 1.0)],
        // one over the whole glass, square
        vec![LensDraw { radius: 0.0, ..lens_at([0.0, 0.0, 320.0, 280.0], 1.0) }],
        // four overlapping
        vec![lens_at([40.0, 40.0, 120.0, 40.0], 1.0), lens_at([60.0, 50.0, 120.0, 40.0], 0.6), lens_at([80.0, 60.0, 120.0, 40.0], 1.0), lens_at([100.0, 70.0, 120.0, 40.0], 0.3)],
    ];
    for dark in [false, true] {
        for warp in drivers() {
            for set in &sets {
                let out = render_lenses(&src, sw, sh, &Look::regular(dark), set, warp);
                let bad = out.as_chunks::<4>().0.iter().filter(|p| **p == [255, 0, 255, 255]).count();
                assert_eq!(bad, 0, "non-finite pixels with lenses {set:?} (dark {dark}, warp {warp})");
            }
        }
    }
}

/// The lens is the clear piece in a frosted pane: a one pixel line is still a line inside it and a smear outside.
#[test]
fn a_lens_is_sharp_where_the_window_glass_is_frosted() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    // a bright vertical line at the glass's middle column (source x 160 + 160) on a dark ground
    let src = picture(sw, sh, |x, _| if x == GLASS.0 as u32 + 160 { [255; 3] } else { [40; 3] });
    let lens = [60.0f32, 100.0, 200.0, 36.0];
    let lum = |p: [u8; 4]| p[0] as i32 + p[1] as i32 + p[2] as i32;
    for dark in [false, true] {
        for warp in drivers() {
            let look = Look::regular(dark);
            let w = GLASS.2;
            // the line's height above the ground beside it, in the lens's middle row
            let contrast = |lenses: &[LensDraw]| {
                let out = render_lenses(&src, sw, sh, &look, lenses, warp);
                let ground = (150..154).map(|x| lum(px(&out, w, x, 118))).sum::<i32>() / 4;
                (150..170).map(|x| lum(px(&out, w, x, 118))).max().unwrap() - ground
            };
            let (frosted, clear) = (contrast(&[]), contrast(&[lens_at(lens, 1.0)]));
            assert!(clear >= 3 * frosted.max(1) && clear >= 60, "line contrast {clear} in the lens, {frosted} in the window glass (dark {dark}, warp {warp})");
        }
    }
}

/// Each slot keeps its own rectangle, clip, radius and opacity: two lenses far apart draw as each draws alone, in either order.
#[test]
fn lenses_in_different_slots_do_not_mix_their_numbers() {
    if !can_draw() {
        return;
    }
    let (sw, sh, src) = gradient_source();
    let look = Look::regular(true);
    // left: square, clipped at x 130; right: a capsule, clipped to its upper half. 60 px apart, well beyond the shadow's reach.
    let a = LensDraw { rect: [10.0, 40.0, 120.0, 50.0], clip: [0.0, 0.0, 130.0, 280.0], radius: 0.0, opacity: 1.0 };
    let b = LensDraw { rect: [190.0, 180.0, 120.0, 50.0], clip: [150.0, 0.0, 170.0, 205.0], radius: 25.0, opacity: 0.5 };
    for warp in drivers() {
        let w = GLASS.2;
        let alone_a = render_lenses(&src, sw, sh, &look, &[a], warp);
        let alone_b = render_lenses(&src, sw, sh, &look, &[b], warp);
        let none = render_lenses(&src, sw, sh, &look, &[], warp);
        assert!(alone_a != none && alone_b != none, "each lens must show for this test to say anything (warp {warp})");
        let in_a = |x: u32, y: u32| x < 100 && y < 140;
        let in_b = |x: u32, y: u32| x >= 170 && y >= 140;
        for order in [[a, b], [b, a]] {
            let both = render_lenses(&src, sw, sh, &look, &order, warp);
            assert_eq!(worst_diff(&both, &alone_a, w, in_a).0, 0, "lens a changed by the other being in slot {} (warp {warp})", (order[0] == a) as u8 ^ 1);
            assert_eq!(worst_diff(&both, &alone_b, w, in_b).0, 0, "lens b changed by the other being there (warp {warp})");
        }
        // the radius is the lens's own: the same rectangle square and as a capsule differs at the corner
        let square = render_lenses(&src, sw, sh, &look, &[LensDraw { radius: 0.0, ..lens_at([60.0, 100.0, 200.0, 36.0], 1.0) }], warp);
        let capsule = render_lenses(&src, sw, sh, &look, &[lens_at([60.0, 100.0, 200.0, 36.0], 1.0)], warp);
        assert!(worst_diff(&square, &capsule, w, |x, y| (62..68).contains(&x) && (102..108).contains(&y)).0 >= 6, "the corner radius has no effect (warp {warp})");
    }
}

/// The public offscreen API: one picture per frame, and with no lens the plain picture.
#[test]
fn offscreen_frames_draw_one_picture_per_frame() {
    if !can_draw() {
        return;
    }
    let (sw, sh, src) = gradient_source();
    let shot = |y: f32| LensShot { rect: [60.0, y, 200.0, 36.0], clip: None, radius: 18.0, opacity: 1.0 };
    let frames = render_offscreen_frames((&src, sw, sh), GLASS, 8.0, 1.0, false, &[vec![], vec![shot(100.0)], vec![shot(160.0)], vec![]]).expect("frames");
    assert_eq!(frames.len(), 4);
    assert!(frames[0] == frames[3], "no lens: the same picture");
    assert!(frames[0] != frames[1] && frames[1] != frames[2]);
    let plain = render_offscreen(&src, sw, sh, GLASS, 8.0, 1.0, false).expect("plain");
    assert!(plain == frames[0], "the lens API with no lens is the plain picture");
    let odd = render_offscreen_frames((&src, sw, sh), GLASS, 8.0, 1.0, false, &[vec![shot(f32::NAN)]]).expect("a lens that cannot be drawn is left out");
    assert!(odd[0] == plain, "and leaves the picture as it is");
}

/// The loop guard compares the drawn lenses: a lens that moved or faded is presented again, one at rest is not.
#[test]
fn the_draw_list_is_what_the_loop_guard_compares() {
    let base = lens_at([60.0, 100.0, 200.0, 36.0], 1.0);
    let list = |d: LensDraw| LensList::from_draws([d]);
    assert!(list(base) == list(base));
    assert!(list(base) != list(LensDraw { rect: [60.0, 100.5, 200.0, 36.0], ..base }), "half a pixel is a different picture");
    assert!(list(base) != list(LensDraw { opacity: 0.99, ..base }));
    assert!(list(base) != list(LensDraw { clip: [0.0, 0.0, 200.0, 280.0], ..base }));
    assert!(LensList::from_draws([]) == LensList::default());
    // an opacity above 1 would give the shader a negative blend weight: it is cut to 1; an invisible or broken lens is left out
    assert!(list(LensDraw { opacity: 2.0, ..base }) == list(base));
    assert_eq!(LensList::from_draws([LensDraw { opacity: 0.0, ..base }, LensDraw { opacity: f32::NAN, ..base }, base]).n, 1);
    // and no more than MAX_LENSES
    assert_eq!(LensList::from_draws([base; MAX_LENSES + 2]).n, MAX_LENSES);
}

/// The spec's slide, as the harness plays it.
#[test]
fn a_simulated_slide_runs_from_the_start_to_the_target() {
    let (from, to) = ([16.0, 120.0, 220.0, 36.0], [16.0, 264.0, 220.0, 36.0]);
    let sim = simulate_slide(from, to, 120.0);
    assert_eq!(sim[0], from);
    assert_eq!(*sim.last().unwrap(), to);
    assert!(sim.windows(2).all(|p| p[0] != p[1]), "every refresh moves it, until the last");
    assert!((60..120).contains(&sim.len()), "{} refreshes", sim.len());
}
