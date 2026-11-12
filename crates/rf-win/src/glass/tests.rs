//! Windowless tests of the glass: everything goes through the offscreen path (a hardware device if there is one, WARP when the GPU is
//! forced off or absent), so they run on a CI runner. The live path (a window, the capture loop) is not covered here: it needs a screen.

use super::gpu::{self, Cb, Geometry, Look, Region, Renderer, Source, prep, region_pad, region_size};
use super::*;

/// BGRA picture filled by `f(x, y) -> [r, g, b]`.
pub(super) fn picture(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = f(x, y);
            v.extend_from_slice(&[b, g, r, 255]);
        }
    }
    v
}

pub(super) fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// What a uniform grey source must become in the middle of the glass, computed on the CPU from the look's numbers: saturation does nothing
/// to a grey, then the contrast / lum transfer, then the tint. (Far from the edge the rim light and the dark line have faded to nothing.)
pub(super) fn expected_centre(grey: u8, look: &Look) -> [f32; 3] {
    let g = grey as f32 / 255.0;
    let c = (g * (look.lum * look.contrast) + 0.5 * (1.0 - look.contrast)).clamp(0.0, 1.0);
    let t = look.tint;
    [0, 1, 2].map(|i| (c + (t[i] - c) * t[3]) * 255.0)
}

/// `gpu::render` at scale 1 with a look and the choice between hardware and WARP.
pub(super) fn render_with(src: &[u8], sw: u32, sh: u32, rect: (i32, i32, u32, u32), radius: f32, look: &Look, force_warp: bool) -> Result<Vec<u8>, String> {
    let geo = Geometry { w: rect.2, h: rect.3, origin: (rect.0, rect.1), scale: 1.0, radius_px: radius, cut_corners: true };
    with_renderer(force_warp, |r| unsafe { gpu::render_on(r, src, sw, sh, &geo, look) })
}

/// One device per driver for all the tests here, made by the first test that draws and never destroyed. A device per picture meant
/// dozens of devices made and destroyed on parallel test threads; with the rest of the crate's tests running beside them the test
/// process then ended with a wrong exit code although every test had passed (seen on CI and here).
struct Shared(Renderer);
// SAFETY: a D3D11 device may be used from any thread; its immediate context only from one at a time, which the mutex sees to.
unsafe impl Send for Shared {}
static RENDERERS: [std::sync::Mutex<Option<Shared>>; 2] = [std::sync::Mutex::new(None), std::sync::Mutex::new(None)];

pub(super) fn with_renderer<T>(force_warp: bool, f: impl FnOnce(&Renderer) -> T) -> T {
    // the software renderer takes every core while it draws: not while a timing test measures (see `TEST_QUIET`)
    let _quiet = crate::TEST_QUIET.lock().unwrap_or_else(|p| p.into_inner());
    let mut slot = RENDERERS[force_warp as usize].lock().unwrap_or_else(|p| p.into_inner());
    let shared = slot.get_or_insert_with(|| Shared(unsafe { gpu::offscreen_renderer(force_warp) }.expect("a D3D11 device (WARP at least)")));
    f(&shared.0)
}

/// Whether the tests that draw run here: not on a CI runner (the `CI` variable GitHub sets), and not on a machine without a real
/// graphics adapter. On the runner they passed, all of them, but the test process then ended with STATUS_INVALID_PARAMETER on its way
/// out, with a device per picture and with the shared devices alike; the cause was not found (it does not happen here, where the same
/// tests also draw with the software renderer forced). The runner does list an adapter that is not marked as software, so the adapter
/// list alone does not tell it apart. So the drawing is tested on developers' machines only.
pub(super) fn can_draw() -> bool {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};
    static YES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *YES.get_or_init(|| unsafe {
        if std::env::var_os("CI").is_some() || std::env::var_os("GITHUB_ACTIONS").is_some() {
            eprintln!("glass tests: on a CI runner, the drawing tests are skipped");
            return false;
        }
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return false };
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            if a.GetDesc1().is_ok_and(|d| d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0) {
                return true;
            }
            i += 1;
        }
        eprintln!("glass tests: no graphics adapter here, the drawing tests are skipped");
        false
    })
}

pub(super) fn drivers() -> [bool; 2] {
    [false, true] // hardware (or WARP if there is none), then WARP forced
}

#[test]
fn uniform_grey_centre_is_grade_plus_tint() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (480u32, 360u32);
    let rect = (60, 40, 360u32, 280u32);
    for dark in [false, true] {
        for warp in drivers() {
            for grey in [60u8, 140, 220] {
                let src = picture(sw, sh, |_, _| [grey; 3]);
                let look = Look::regular(dark, &LookParams::DEFAULT);
                let out = render_with(&src, sw, sh, rect, 8.0, &look, warp).expect("render");
                assert_eq!(out.len(), (rect.2 * rect.3 * 4) as usize);
                let p = px(&out, rect.2, rect.2 / 2, rect.3 / 2);
                let want = expected_centre(grey, &look);
                assert_eq!(p[3], 255, "centre alpha (dark {dark}, warp {warp})");
                // BGRA order
                for (i, ch) in [2usize, 1, 0].into_iter().enumerate() {
                    assert!(
                        (p[ch] as f32 - want[i]).abs() <= 2.0,
                        "centre channel {i} is {} but the CPU says {:.1} (grey {grey}, dark {dark}, warp {warp})",
                        p[ch],
                        want[i]
                    );
                }
            }
        }
    }
}

#[test]
fn corner_is_cut_by_the_radius() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (300u32, 240u32);
    let rect = (20, 20, 260u32, 200u32);
    let src = picture(sw, sh, |x, y| [(x % 251) as u8, (y % 241) as u8, 90]);
    for warp in drivers() {
        let look = Look::regular(false, &LookParams::DEFAULT);
        let round = render_with(&src, sw, sh, rect, 8.0, &look, warp).expect("render");
        assert_eq!(px(&round, rect.2, 0, 0)[3], 0, "corner pixel with radius 8 (warp {warp})");
        assert_eq!(px(&round, rect.2, rect.2 / 2, rect.3 / 2)[3], 255, "centre pixel (warp {warp})");
        assert_eq!(px(&round, rect.2, rect.2 - 1, rect.3 - 1)[3], 0, "far corner with radius 8 (warp {warp})");
        let square = render_with(&src, sw, sh, rect, 0.0, &look, warp).expect("render");
        assert_eq!(px(&square, rect.2, 0, 0)[3], 255, "corner pixel with radius 0 (warp {warp})");
        assert_eq!(px(&square, rect.2, rect.2 - 1, rect.3 - 1)[3], 255, "far corner with radius 0 (warp {warp})");
        // a window: the system cuts the corners, the glass covers the whole rectangle whatever the radius of its rim line
        let geo = Geometry { w: rect.2, h: rect.3, origin: (rect.0, rect.1), scale: 1.0, radius_px: 8.0, cut_corners: false };
        let window = with_renderer(warp, |r| unsafe { gpu::render_on(r, &src, sw, sh, &geo, &look) }).expect("render");
        assert_eq!(px(&window, rect.2, 0, 0)[3], 255, "corner pixel of a window (warp {warp})");
        assert_eq!(px(&window, rect.2, rect.2 - 1, 0)[3], 255, "top right corner pixel of a window (warp {warp})");
        assert_eq!(px(&window, rect.2, rect.2 - 1, rect.3 - 1)[3], 255, "far corner pixel of a window (warp {warp})");
    }
}

/// Black / white / red bands, mirror symmetric about the vertical line x = w / 2, varying along y too.
pub(super) fn symmetric_stripes(w: u32, h: u32) -> Vec<u8> {
    picture(w, h, |x, y| {
        let d = x.min(w - 1 - x);
        match ((d / 12) + (y / 9)) % 3 {
            0 => [0, 0, 0],
            1 => [255, 255, 255],
            _ => [255, 0, 0],
        }
    })
}

#[test]
fn striped_source_has_no_non_finite_pixel() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    let src = symmetric_stripes(sw, sh);
    let rect = (160, 60, 320u32, 280u32);
    for dark in [false, true] {
        for warp in drivers() {
            for radius in [8.0f32, 0.0] {
                let out = render_with(&src, sw, sh, rect, radius, &Look::regular(dark, &LookParams::DEFAULT), warp).expect("render");
                // the shader's marker for a NaN or an infinity is opaque magenta
                let bad = out.as_chunks::<4>().0.iter().filter(|p| **p == [255, 0, 255, 255]).count();
                assert_eq!(bad, 0, "non-finite pixels (dark {dark}, warp {warp}, radius {radius})");
            }
        }
    }
}

#[test]
fn symmetric_source_gives_symmetric_picture() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    let src = symmetric_stripes(sw, sh);
    // The glass is centred on the source's axis and its half width is a multiple of 16 px, so the axis lies on a box edge of every pyramid
    // level the frost reads. The rim light has a direction (the top left is lit), so it is switched off here: what is checked is the
    // optics, the frost and the outline.
    let rect = (160, 60, 320u32, 280u32);
    let mut look = Look::regular(false, &LookParams::DEFAULT);
    look.rim = 0.0;
    look.shade = 0.0;
    for warp in drivers() {
        let out = render_with(&src, sw, sh, rect, 8.0, &look, warp).expect("render");
        let mut worst = (0i32, 0u32, 0u32);
        for y in 0..rect.3 {
            for x in 0..rect.2 / 2 {
                let (a, b) = (px(&out, rect.2, x, y), px(&out, rect.2, rect.2 - 1 - x, y));
                for c in 0..4 {
                    worst = worst.max(((a[c] as i32 - b[c] as i32).abs(), x, y));
                }
            }
        }
        assert!(worst.0 <= 2, "left and right halves differ by up to {} of 255, first at x {} y {} (warp {warp})", worst.0, worst.1, worst.2);
    }
}

#[test]
fn a_failed_attach_starts_nothing() {
    assert!(!attach(0, false));
    assert!(matches!(state(0), GlassState::Off | GlassState::Failed(_)));
    assert!(!any_active());
    assert!(!engine_started(), "attach of a non-window must not start the engine thread");
    // detach is idempotent, also for a handle that was never attached
    detach(0);
    detach(0);
    assert_eq!(state(0), GlassState::Off);
    assert!(!engine_started());
    assert_eq!(state(12345), GlassState::Off);
}

#[test]
fn constant_buffer_matches_the_shader() {
    assert_eq!(size_of::<Cb>() % 16, 0, "a constant buffer must be a whole number of float4");
    let src = gpu::SHADER_SRC;
    let start = src.find("cbuffer CB").expect("cbuffer CB in the shader");
    let body = &src[start..];
    let body = &body[body.find('{').unwrap() + 1..body.find("};").unwrap()];
    // `float4 uName;` is one float4, `float4 uName[12];` is twelve
    let fields: usize = body
        .lines()
        .map(|l| l.trim_start())
        .filter(|l| l.starts_with("float4 u"))
        .map(|l| {
            let decl = l.split(';').next().unwrap_or("");
            match (decl.find('['), decl.find(']')) {
                (Some(a), Some(b)) => decl[a + 1..b].trim().parse::<usize>().expect("an array length"),
                _ => 1,
            }
        })
        .sum();
    assert_eq!(fields, 12 + 4 + 3 * MAX_LENSES, "float4 fields of the shader's cbuffer");
    assert_eq!(size_of::<Cb>() / 16, fields, "the Rust struct and the shader's cbuffer must have the same number of float4");
    // the lens count of the shader is the engine's
    assert!(src.contains(&format!("static const uint MAX_LENSES = {MAX_LENSES};")), "MAX_LENSES differs between glass.hlsl and mod.rs");
    // the order of the fields, by name: the lens block comes last, after uOpt
    let names: Vec<&str> = body.lines().filter_map(|l| l.trim_start().strip_prefix("float4 ")).filter_map(|l| l.split([';', '[']).next()).collect();
    assert_eq!(names[11], "uOpt");
    assert_eq!(&names[12..], ["uLensA", "uLensB", "uLensC", "uLensT", "uLens"]);
}

#[test]
fn region_is_allocated_in_steps() {
    let pad = region_pad(1.0);
    assert_eq!(pad, 96);
    assert_eq!(region_pad(1.5), 144);
    let (w, h) = region_size(1100, 640, pad);
    assert_eq!((w, h), (1536, 1024));
    assert_eq!(region_size(1101, 641, pad), (w, h), "a live resize by a few px must not change the allocation");
    assert_eq!(region_size(10, 10, pad), (256, 256));
}

/// The loop guard's building block: the same backdrop gives the same fingerprint, a changed one (a video behind the window) another.
#[test]
fn fingerprint_follows_the_backdrop() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (700u32, 600u32);
    let geo = Geometry { w: 300, h: 200, origin: (100, 100), scale: 1.0, radius_px: 8.0, cut_corners: true };
    let look = Look::regular(false, &LookParams::DEFAULT);
    with_renderer(true, |r| unsafe {
        let region = Region::new(&r.g, geo.w, geo.h, region_pad(geo.scale)).expect("region");
        let base = picture(sw, sh, |x, y| [(x / 5 % 256) as u8, (y / 5 % 256) as u8, 40]);
        let fp = |data: &[u8]| {
            let src = Source::new(&r.g, sw, sh, Some(data)).expect("source");
            prep(r, &region, &src, &geo, &look, false);
            region.fingerprint(&r.g, geo.w, geo.h).expect("fingerprint")
        };
        let a = fp(&base);
        assert_eq!(a, fp(&base), "an unchanged screen must give the same fingerprint");
        // a block of 60 x 40 px lights up inside the glass: what a playing video does
        let mut moved = base.clone();
        for y in 150..190u32 {
            for x in 200..260u32 {
                let i = ((y * sw + x) * 4) as usize;
                moved[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        assert_ne!(a, fp(&moved), "a change behind the glass must change the fingerprint");
        // Inside the allocated region but beyond the glass and its padding: the glass is at 100..400 x 100..300, the padding 96, the region
        // starts at 4 and is allocated up to 516, so rows 420..500 are slack that the glass never reads (the fingerprint must leave it out,
        // or a video playing beside the window would keep the glass drawing)
        let mut outside = base.clone();
        for y in 420..500u32 {
            for x in 100..400u32 {
                let i = ((y * sw + x) * 4) as usize;
                outside[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        assert_eq!(a, fp(&outside), "a change in the slack of the region, beyond the glass's padding, must not change the fingerprint");
    });
}

/// The brightness read back from the pyramid is that of the backdrop under the glass: white is near 1, black near 0, a mid grey in
/// between, and a bright block in the padding (outside the glass) does not count.
#[test]
fn backdrop_luma_is_the_brightness_under_the_glass() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (700u32, 600u32);
    let geo = Geometry { w: 300, h: 200, origin: (100, 100), scale: 1.0, radius_px: 8.0, cut_corners: true };
    let look = Look::regular(false, &LookParams::DEFAULT);
    with_renderer(true, |r| unsafe {
        let region = Region::new(&r.g, geo.w, geo.h, region_pad(geo.scale)).expect("region");
        let luma_of = |data: &[u8]| {
            let src = Source::new(&r.g, sw, sh, Some(data)).expect("source");
            prep(r, &region, &src, &geo, &look, false);
            region.read_back(&r.g, geo.w, geo.h).expect("read back").luma.expect("a glass has texels")
        };
        let white = luma_of(&picture(sw, sh, |_, _| [255, 255, 255]));
        let black = luma_of(&picture(sw, sh, |_, _| [0, 0, 0]));
        let grey = luma_of(&picture(sw, sh, |_, _| [128, 128, 128]));
        assert!(white > 0.97, "white backdrop: {white}");
        assert!(black < 0.03, "black backdrop: {black}");
        assert!((grey - 128.0 / 255.0).abs() < 0.03, "mid grey backdrop: {grey}");
        // the glass is at 100..400 x 100..300: a white block on the left of it, in the padding, must not make a dark glass bright
        let beside = luma_of(&picture(sw, sh, |x, y| if x < 90 && y < 400 { [255, 255, 255] } else { [10, 10, 10] }));
        assert!(beside < 0.06, "a bright block beside the glass, not behind it: {beside}");
        // a bright half behind the glass counts for its half
        let half = luma_of(&picture(sw, sh, |x, _| if x < 250 { [255, 255, 255] } else { [0, 0, 0] }));
        assert!((0.4..0.6).contains(&half), "half of the glass is white: {half}");
    });
}

/// The value the app reads: unknown until the engine has measured, only for a window the app still holds, gone with the glass.
#[test]
fn the_brightness_is_unknown_until_measured_and_goes_with_the_glass() {
    let h: isize = 0x7ffe_0b01; // not a window: only the table is looked at
    assert_eq!(backdrop_luma(h), None, "nothing measured");
    set_luma(h, 0.7);
    assert_eq!(backdrop_luma(h), None, "a window the app does not hold (detached meanwhile) keeps no value");
    states().insert(h, GlassState::Running);
    set_luma(h, 0.7);
    assert_eq!(backdrop_luma(h), Some(0.7));
    set_luma(h, 0.2);
    assert_eq!(backdrop_luma(h), Some(0.2), "the latest measurement");
    fail(h, "lost the device");
    assert_eq!(backdrop_luma(h), None, "a failed glass has no backdrop");
    states().insert(h, GlassState::Running);
    set_luma(h, 0.9);
    detach(h);
    assert_eq!(backdrop_luma(h), None, "detached");
    states().insert(h, GlassState::Running);
    set_luma(h, 0.9);
    forget(h);
    assert_eq!(backdrop_luma(h), None, "forgotten by the engine");
}

#[test]
fn bad_arguments_are_errors_not_panics() {
    let src = picture(8, 8, |_, _| [1, 2, 3]);
    assert!(render_offscreen(&src, 8, 8, (0, 0, 0, 4), 8.0, 1.0, false).is_err());
    assert!(render_offscreen(&src[..10], 8, 8, (0, 0, 4, 4), 8.0, 1.0, false).is_err());
    assert!(render_offscreen(&src, 8, 8, (0, 0, 4, 4), -1.0, 1.0, false).is_err());
    assert!(render_offscreen(&src, 8, 8, (0, 0, 4, 4), 8.0, 0.0, false).is_err());
}

/// A look that shows the optics and nothing else: no frost, no grade, no tint, no rim light, no dark line.
pub(super) fn bare_optics() -> Look {
    let mut l = Look::regular(false, &LookParams::DEFAULT);
    l.blur = 0.0;
    l.saturate = 1.0;
    l.lum = 1.0;
    l.contrast = 1.0;
    l.tint[3] = 0.0;
    l.rim = 0.0;
    l.shade = 0.0;
    l
}

/// The rim must bend: with a source that changes along x, the pixels of the outer band show another part of it than they would without
/// the glass thickness, and the middle is untouched.
#[test]
fn rim_bends_the_backdrop_and_the_middle_does_not() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    let src = picture(sw, sh, |x, y| [(x / 3) as u8, (y / 2) as u8, 60]);
    let rect = (160, 60, 320u32, 280u32);
    let flat = Look { depth: 0.0, ..bare_optics() };
    for warp in drivers() {
        let bent = render_with(&src, sw, sh, rect, 8.0, &bare_optics(), warp).expect("render");
        let straight = render_with(&src, sw, sh, rect, 8.0, &flat, warp).expect("render");
        // 3 px inside the left edge, half way down: the outer part of the band (about 30 px wide), where the rays turn most (the bend
        // reads the picture about 19 px further in, which is 6 levels of red at one level per 3 px)
        let (a, b) = (px(&bent, rect.2, 3, rect.3 / 2), px(&straight, rect.2, 3, rect.3 / 2));
        let shift = (a[2] as i32 - b[2] as i32).abs();
        assert!(shift >= 4, "the rim did not move the picture (red differs by {shift}, warp {warp})");
        let (a, b) = (px(&bent, rect.2, rect.2 / 2, rect.3 / 2), px(&straight, rect.2, rect.2 / 2, rect.3 / 2));
        for c in 0..3 {
            assert!((a[c] as i32 - b[c] as i32).abs() <= 1, "the middle moved (channel {c}, warp {warp})");
        }
    }
}

/// The width of the transition of a vertical step edge in the middle of the glass, in px between 10 % and 90 % of the step.
pub(super) fn step_width(out: &[u8], w: u32, y: u32) -> u32 {
    let row: Vec<f32> = (0..w).map(|x| px(out, w, x, y)[1] as f32).collect();
    let (lo, hi) = (row[2], row[w as usize - 3]);
    let at = |frac: f32| row.iter().position(|&v| v >= lo + (hi - lo) * frac).unwrap_or(0) as u32;
    at(0.9) - at(0.1)
}

/// The frost: a step edge becomes a ramp of about 2.56 sigma between 10 % and 90 % (sigma 10.35 css px at scale 1), and without frost it
/// stays a step.
#[test]
fn frost_blurs_an_edge_by_the_look_s_sigma() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    let src = picture(sw, sh, |x, _| if x < 320 { [20; 3] } else { [230; 3] });
    let rect = (120, 60, 400u32, 280u32);
    let mut frosted = bare_optics();
    frosted.blur = Look::regular(false, &LookParams::DEFAULT).blur;
    for warp in drivers() {
        let sharp = render_with(&src, sw, sh, rect, 8.0, &bare_optics(), warp).expect("render");
        let soft = render_with(&src, sw, sh, rect, 8.0, &frosted, warp).expect("render");
        let (ws, wf) = (step_width(&sharp, rect.2, rect.3 / 2), step_width(&soft, rect.2, rect.3 / 2));
        assert!(ws <= 3, "no frost, yet the edge is {ws} px wide (warp {warp})");
        assert!((18..=38).contains(&wf), "frost of sigma 10.35 should make the edge about 26 px wide, it is {wf} (warp {warp})");
    }
}

/// A window's corner radius (8 px) is smaller than the rim (about 30 px). How much a ray is bent depends on the distance to the TRUE
/// outline, so the bent band hugs the real corner: on the corner's diagonal a point close to the corner is bent some pixels, a point
/// further in less. (When the depth came from a rounder box, the point close to the corner lay outside that box and read the backdrop
/// from about 28 px away, like everything else in the wedge between the two outlines; a second, rounder outline showed inside the
/// window.) The DIRECTION of the bend still turns smoothly through the corner: beside the diagonal it is not straight away from the
/// nearest edge.
#[test]
fn the_bent_band_follows_the_true_corner() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    // red rises by one level per pixel to the right
    let src = picture(sw, sh, |x, _| [x.saturating_sub(80).min(255) as u8, 0, 60]);
    let rect = (100, 60, 320u32, 280u32);
    // the rim the numbers below were worked out for (the shipped look's rim may be tuned without touching this test)
    let rim = Look { depth: 1.188, max_bezel: 30.629, ..bare_optics() };
    let flat = Look { depth: 0.0, ..rim };
    for warp in drivers() {
        let bent = render_with(&src, sw, sh, rect, 8.0, &rim, warp).expect("render");
        let straight = render_with(&src, sw, sh, rect, 8.0, &flat, warp).expect("render");
        // how many px further right (further in, seen from the top left corner) the bend reads at a point of the corner's diagonal
        let shift = |d: u32| px(&bent, rect.2, d, d)[2] as i32 - px(&straight, rect.2, d, d)[2] as i32;
        let (near, far) = (shift(6), shift(14));
        // the model: depth 5.9 px -> 10.6 px along the diagonal = 7.5 px to the right; depth 14.5 px -> 2.2 px = 1.6 px to the right
        assert!((4..=11).contains(&near), "6 px into the corner the bend is {near} px (warp {warp})");
        assert!((0..=4).contains(&far) && far < near, "14 px into the corner the bend is {far} px, 6 px in it is {near} (warp {warp})");
        // 20 px right, 10 px down of the corner: the nearest edge is the top one, straight away from it would not move the picture
        // sideways at all. The smooth direction does: depth 10.5 px -> 4.7 px along (0.45, 0.89) = 2.1 px to the right.
        let side = px(&bent, rect.2, 20, 10)[2] as i32 - px(&straight, rect.2, 20, 10)[2] as i32;
        assert!((1..=4).contains(&side), "beside the diagonal the bend moves the picture {side} px sideways (warp {warp})");
    }
}

/// The choice of the radius a window's glass is drawn with: the app's own wins, kept inside the window; without one the system's stays.
#[test]
fn the_radius_is_the_apps_or_the_systems() {
    use super::engine::effective_radius;
    assert_eq!(effective_radius(None, 8.0, 300, 200), 8.0, "no radius asked: what the system does, as before");
    assert_eq!(effective_radius(None, 0.0, 300, 200), 0.0);
    assert_eq!(effective_radius(Some(24.0), 8.0, 330, 48), 24.0, "an island's own radius beats the system's");
    assert_eq!(effective_radius(Some(24.0), 0.0, 330, 48), 24.0, "also where the system does not round at all");
    assert_eq!(effective_radius(Some(0.0), 8.0, 330, 48), 0.0, "an asked-for square corner stays square");
    assert_eq!(effective_radius(Some(36.0), 8.0, 330, 48), 24.0, "no more than half of the shorter side: a capsule");
    assert_eq!(effective_radius(Some(36.0), 8.0, 48, 330), 24.0, "whichever side is shorter");
    assert_eq!(effective_radius(Some(-3.0), 8.0, 330, 48), 0.0);
    assert_eq!(effective_radius(Some(f32::NAN), 8.0, 330, 48), 8.0, "not a number: the system's");
    assert_eq!(effective_radius(Some(f32::INFINITY), 8.0, 330, 48), 8.0);
}

#[test]
fn options_are_what_attach_always_meant_unless_asked_otherwise() {
    assert_eq!(GlassOptions::window(true), GlassOptions { dark: true, radius_px: None, cut_corners: false });
    let o = GlassOptions { dark: false, radius_px: Some(f32::NAN), cut_corners: true }.sanitized();
    assert_eq!(o.radius_px, None);
    assert!(o.cut_corners);
    assert_eq!(GlassOptions { dark: false, radius_px: Some(-4.0), cut_corners: true }.sanitized().radius_px, Some(0.0));
    assert_eq!(GlassOptions { dark: false, radius_px: Some(36.0), cut_corners: true }.sanitized().radius_px, Some(36.0));
}

#[test]
fn attach_with_a_non_window_starts_nothing_either() {
    assert!(!attach_with(0, GlassOptions { dark: true, radius_px: Some(24.0), cut_corners: true }));
    assert!(!any_active());
    assert!(!engine_started());
    // the other calls that take options do nothing for a window that has no glass
    set_options(0, GlassOptions::window(false));
    set_dark(0, true);
    assert!(!engine_started());
}

/// A free-standing island: 48 px high, 330 px wide, radius 24 and cutting its own corners (a capsule). Its corner pixels are
/// transparent, the middle of its ends is not, and the middle is opaque. Through the same offscreen path the live draw uses.
#[test]
fn an_island_with_its_own_radius_is_a_capsule() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (500u32, 200u32);
    let rect = (80, 60, 330u32, 48u32);
    let src = picture(sw, sh, |x, y| [(x % 251) as u8, (y % 241) as u8, 120]);
    for warp in drivers() {
        let look = Look::regular(true, &LookParams::DEFAULT);
        let out = render_with(&src, sw, sh, rect, 24.0, &look, warp).expect("render");
        assert_eq!(px(&out, rect.2, 0, 0)[3], 0, "top left corner (warp {warp})");
        assert_eq!(px(&out, rect.2, rect.2 - 1, 0)[3], 0, "top right corner (warp {warp})");
        assert_eq!(px(&out, rect.2, 0, rect.3 - 1)[3], 0, "bottom left corner (warp {warp})");
        assert_eq!(px(&out, rect.2, rect.2 - 1, rect.3 - 1)[3], 0, "bottom right corner (warp {warp})");
        assert_eq!(px(&out, rect.2, 3, rect.3 / 2)[3], 255, "middle of the left end (warp {warp})");
        assert_eq!(px(&out, rect.2, rect.2 / 2, rect.3 / 2)[3], 255, "centre (warp {warp})");
        assert_eq!(px(&out, rect.2, rect.2 / 2, 0)[3], 255, "middle of the top edge (warp {warp})");
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// The shaders made at build time
// ---------------------------------------------------------------------------------------------------------------------------------

/// What the build embeds is what a run-time compile makes, byte for byte, so a window's glass cannot look different for it, and a change
/// of `glass.hlsl` or of the flags in only one of the two places is caught here. (An empty blob would mean the build could not make it
/// and the program compiles at start: allowed for the program, not for this machine's build.)
#[test]
fn the_shaders_made_at_build_time_are_what_a_compile_at_start_makes() {
    use gpu::{compile, embedded};
    use windows::Win32::Graphics::Direct3D::Fxc::D3DCOMPILE_IEEE_STRICTNESS;
    let cases: [(&str, &[u8], &str, &str, u32); 5] = [
        ("vs_main", embedded::VS_MAIN, "VSMain", "vs_5_0", 0),
        ("ps_crop", embedded::PS_CROP, "PSCrop", "ps_5_0", 0),
        ("ps_down", embedded::PS_DOWN, "PSDown", "ps_5_0", 0),
        ("ps_glass", embedded::PS_GLASS, "PSGlass", "ps_5_0", 0),
        ("ps_glass_strict", embedded::PS_GLASS_STRICT, "PSGlass", "ps_5_0", D3DCOMPILE_IEEE_STRICTNESS),
    ];
    for (name, built, entry, target, flags) in cases {
        let now = unsafe { compile(entry, target, flags) }.expect("the shader compiles");
        assert!(!built.is_empty(), "{name}: the build made no bytecode (see the build warnings)");
        assert!(built == now.as_slice(), "{name}: {} bytes from the build differ from the {} a compile now makes", built.len(), now.len());
    }
}

/// The same picture from a renderer with the build's shaders and one that compiled its own, on one device; and what the start costs.
#[test]
fn a_renderer_from_build_time_shaders_draws_the_same_and_starts_much_sooner() {
    if !can_draw() {
        return;
    }
    use gpu::{Gpu, Shaders};
    use std::time::Instant;
    let src = picture(96, 96, |x, y| [(x * 2) as u8, (y * 2) as u8, ((x ^ y) * 3) as u8]);
    let geo = Geometry { w: 48, h: 40, origin: (24, 28), scale: 1.0, radius_px: 12.0, cut_corners: true };
    let look = Look::regular(false, &LookParams::DEFAULT);
    with_renderer(false, |shared| unsafe {
        let same_device = || Gpu { dev: shared.g.dev.clone(), ctx: shared.g.ctx.clone() };
        let t = Instant::now();
        let built = Renderer::with_shaders(same_device(), true, Shaders::Embedded).expect("renderer from the build's shaders");
        let from_build = t.elapsed();
        let t = Instant::now();
        let compiled = Renderer::with_shaders(same_device(), true, Shaders::Compile).expect("renderer with compiled shaders");
        let from_compile = t.elapsed();
        println!("Renderer::new: build-time shaders {from_build:?}, compiled at start {from_compile:?}");
        assert!(from_build < from_compile, "creating the shaders from bytecode must be faster than compiling them");
        let a = gpu::render_on(&built, &src, 96, 96, &geo, &look).expect("picture from build-time shaders");
        let b = gpu::render_on(&compiled, &src, 96, 96, &geo, &look).expect("picture from compiled shaders");
        assert!(a == b, "the two pictures differ");
        assert!(a.iter().any(|&v| v != 0), "and are not empty");
    });
}

// ---------------------------------------------------------------------------------------------------------------------------------
// The capture backends' pure parts: no session is started and nothing is drawn here.
// ---------------------------------------------------------------------------------------------------------------------------------

mod capture_backends {
    use super::super::capture::Dirty;
    use super::super::wgc::{Access, Exclusion, SizeCheck, WinProbe, is_stale, min_interval_ticks, pick_backend, size_check};
    use super::super::{CaptureMode, capture_mode, set_mode};
    use std::collections::BTreeSet;
    use windows::Win32::Foundation::RECT;

    fn rc(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: r, bottom: b }
    }

    /// A window of 200 x 100 at (100, 100) and the 96 px padding the glass reads around it: the backdrop it depends on is 4..396 x 4..296.
    #[test]
    fn a_window_is_dirty_only_when_a_dirty_rectangle_meets_it_or_its_padding() {
        let touches = |d: &Dirty| d.touches(100, 100, 200, 100, 96);
        // inside the window
        assert!(touches(&Dirty::Rects(vec![rc(150, 120, 160, 130)])));
        // in the padding, on the right: 396 is the first column past it
        assert!(touches(&Dirty::Rects(vec![rc(390, 150, 420, 160)])));
        assert!(!touches(&Dirty::Rects(vec![rc(396, 150, 420, 160)])), "starts at the first column past the padding");
        // above and below, left
        assert!(touches(&Dirty::Rects(vec![rc(150, 0, 160, 5)])));
        assert!(!touches(&Dirty::Rects(vec![rc(150, 0, 160, 4)])));
        assert!(!touches(&Dirty::Rects(vec![rc(150, 296, 160, 400)])));
        assert!(!touches(&Dirty::Rects(vec![rc(0, 150, 4, 160)])));
        // far away, and several rectangles of which one is near
        assert!(!touches(&Dirty::Rects(vec![rc(1000, 800, 1100, 900), rc(0, 900, 50, 1000)])));
        assert!(touches(&Dirty::Rects(vec![rc(1000, 800, 1100, 900), rc(120, 120, 121, 121)])));
        // a refresh of the whole monitor touches every window; no rectangle touches none
        assert!(touches(&Dirty::All));
        assert!(!touches(&Dirty::none()));
        // extreme numbers do not overflow
        assert!(Dirty::All.touches(i32::MIN + 1, i32::MAX - 1, u32::MAX, u32::MAX, u32::MAX));
        assert!(!Dirty::Rects(vec![rc(0, 0, 1, 1)]).touches(i32::MAX - 10, i32::MAX - 10, 5, 5, 1));
    }

    #[test]
    fn dirty_regions_merge_and_an_empty_report_is_not_trusted() {
        let mut d = Dirty::none();
        d.merge(Dirty::Rects(vec![rc(0, 0, 10, 10)]));
        d.merge(Dirty::Rects(vec![rc(20, 20, 30, 30)]));
        assert_eq!(d, Dirty::Rects(vec![rc(0, 0, 10, 10), rc(20, 20, 30, 30)]));
        d.merge(Dirty::All);
        assert_eq!(d, Dirty::All);
        d.merge(Dirty::Rects(vec![rc(0, 0, 1, 1)]));
        assert_eq!(d, Dirty::All, "once the whole monitor changed it stays so");
        // very many small rectangles are as good as the whole monitor
        let mut many = Dirty::none();
        for i in 0..100 {
            many.merge(Dirty::Rects(vec![rc(i, i, i + 1, i + 1)]));
        }
        assert_eq!(many, Dirty::All);
        // a picture that came with no rectangle at all touches every window (the report may be missing, not the change)
        assert_eq!(Dirty::none().or_all_if_empty(), Dirty::All);
        assert_eq!(Dirty::Rects(vec![rc(0, 0, 1, 1)]).or_all_if_empty(), Dirty::Rects(vec![rc(0, 0, 1, 1)]));
    }

    /// A frame is stale when it was rendered before the last exclusion list took effect: its iteration is below the number the last
    /// SetWindowExclusionList returned.
    #[test]
    fn frames_of_an_older_exclusion_list_are_dropped() {
        assert!(is_stale(6, 7));
        assert!(!is_stale(7, 7), "the iteration of the call itself is the new list");
        assert!(!is_stale(8, 7));
        assert!(!is_stale(0, 0), "before any list was sent nothing is stale");
        assert!(is_stale(0, 1));
        assert!(!is_stale(u64::MAX, u64::MAX));
    }

    #[test]
    fn the_exclusion_set_follows_attached_windows_and_extras() {
        let mut ex = Exclusion::default();
        assert!(ex.list().is_empty());
        let v0 = ex.version();
        // attached windows are listed whether or not they are visible; a repeat changes nothing
        ex.attach(30);
        ex.attach(10);
        let v1 = ex.version();
        assert_ne!(v0, v1);
        ex.attach(10);
        assert_eq!(ex.version(), v1, "attaching a window that is attached already sends nothing");
        assert_eq!(ex.list(), vec![10, 30]);
        // extras join; a handle that is attached and an extra too is listed once
        ex.set_extras(BTreeSet::from([20, 30, 40]));
        assert_eq!(ex.list(), vec![10, 20, 30, 40]);
        let v2 = ex.version();
        assert_ne!(v1, v2);
        ex.set_extras(BTreeSet::from([20, 30, 40]));
        assert_eq!(ex.version(), v2, "the same registry again sends nothing");
        // detach: an attached window that is also an extra stays listed through its extra entry
        ex.detach(30);
        assert_eq!(ex.list(), vec![10, 20, 30, 40]);
        ex.detach(10);
        assert_eq!(ex.list(), vec![20, 30, 40]);
        let v3 = ex.version();
        ex.detach(10);
        assert_eq!(ex.version(), v3, "detaching a window that is not attached sends nothing");
        // an extra that is closed leaves the list
        ex.set_extras(BTreeSet::from([20]));
        assert_eq!(ex.list(), vec![20]);
        assert_eq!(ex.extras(), &BTreeSet::from([20]));
    }

    /// A handle listed while its window is hidden is sent again when the window shows (it is not known whether the system honours it
    /// then); nothing else about visibility changes what is sent.
    #[test]
    fn a_listed_window_that_was_hidden_and_shows_makes_the_list_go_out_again() {
        let mut ex = Exclusion::default();
        ex.attach(7);
        let listed = ex.version();
        ex.set_visible(7, false);
        assert_eq!(ex.version(), listed, "hidden stays hidden: nothing to send");
        ex.set_visible(7, true);
        let shown = ex.version();
        assert_ne!(shown, listed, "it shows: the list is sent again");
        ex.set_visible(7, true);
        assert_eq!(ex.version(), shown, "still visible: nothing more");
        ex.set_visible(7, false);
        ex.set_visible(7, true);
        assert_ne!(ex.version(), shown, "hidden again, shown again");
        // a window that is not listed is of no concern
        let v = ex.version();
        ex.set_visible(99, true);
        ex.set_visible(99, false);
        assert_eq!(ex.version(), v);
        // an extra that shows counts the same
        ex.set_extras(BTreeSet::from([50]));
        let v = ex.version();
        ex.set_visible(50, true);
        assert_ne!(ex.version(), v);
        // a window that is detached forgets that it was shown: attached again, its first show sends the list
        ex.detach(7);
        ex.attach(7);
        let v = ex.version();
        ex.set_visible(7, true);
        assert_ne!(ex.version(), v);
    }

    #[test]
    fn the_backend_choice_needs_the_permission_and_a_session() {
        // permission and session: the session backend, and the session the probe made is what comes back
        assert_eq!(pick_backend(Access::Allowed, || Ok::<_, String>(5)), Ok(5));
        // the permission is there but a session cannot be made (an older Windows has no exclusion list): Desktop Duplication
        assert_eq!(pick_backend(Access::Allowed, || Err::<u8, _>("no exclusion list".to_string())), Err("no exclusion list".to_string()));
        // no permission: the probe is not even tried (it would make a capture session)
        let mut tried = false;
        let r = pick_backend(Access::Not("denied by the user".into()), || {
            tried = true;
            Ok::<_, String>(1)
        });
        assert_eq!(r, Err("denied by the user".to_string()));
        assert!(!tried, "no session may be made while the permission is not Allowed");
    }

    /// A handle that is no longer a window of ours is dropped from the list; the others are watched for showing.
    #[test]
    fn dead_extras_leave_the_list_and_a_shown_one_sends_it_again() {
        let mut ex = Exclusion::default();
        ex.set_extras(BTreeSet::from([1, 2, 3]));
        let v = ex.version();
        // 1 is hidden, 2 visible, 3 gone (destroyed, or its number now belongs to another process's window)
        ex.poll_extras(|h| match h {
            1 => WinProbe::Hidden,
            2 => WinProbe::Visible,
            _ => WinProbe::Gone,
        });
        assert_eq!(ex.list(), vec![1, 2], "the dead handle is not sent any more");
        assert_ne!(ex.version(), v);
        // nothing changed: nothing to send
        let v = ex.version();
        ex.poll_extras(|h| if h == 2 { WinProbe::Visible } else { WinProbe::Hidden });
        assert_eq!(ex.version(), v);
        // 1 shows
        ex.poll_extras(|_| WinProbe::Visible);
        assert_ne!(ex.version(), v);
        // a dead window that was attached too stays listed through its attach
        ex.attach(2);
        ex.poll_extras(|_| WinProbe::Gone);
        assert_eq!(ex.list(), vec![2]);
        assert!(ex.extras().is_empty());
    }

    /// A monitor that changed its resolution makes the pool deliver buffers of its old size with the new picture in the content size.
    #[test]
    fn a_frame_of_another_size_than_the_mirror_is_told_apart() {
        let item_never = || -> (i32, i32) { panic!("the item is not asked while the frame has the mirror's size") };
        assert_eq!(size_check((2560, 1440), (2560, 1440), (2560, 1440), item_never), SizeCheck::Fine);
        // the monitor went to 1920x1080: the buffer is still 2560x1440, the content is not
        assert_eq!(size_check((1920, 1080), (2560, 1440), (2560, 1440), || (1920, 1080)), SizeCheck::Recreate);
        // rotated: the sides swap
        assert_eq!(size_check((1440, 2560), (2560, 1440), (2560, 1440), || (1440, 2560)), SizeCheck::Recreate);
        // same content size but another buffer (the pool was made anew for a size the mirror does not have yet)
        assert_eq!(size_check((1920, 1080), (1920, 1080), (2560, 1440), || (1920, 1080)), SizeCheck::Recreate);
        // the pool and the mirror are of the item's size already, and a frame of the old size was still in flight: dropped, not an error
        assert_eq!(size_check((2560, 1440), (2560, 1440), (1920, 1080), || (1920, 1080)), SizeCheck::Stale);
        // an item that cannot say its size (0, 0) never matches a mirror: the pool is made anew, which then fails loudly if it must
        assert_eq!(size_check((1, 1), (2, 2), (3, 3), || (0, 0)), SizeCheck::Recreate);
        // negative sizes are not a size
        assert_eq!(size_check((-1, 1080), (1920, 1080), (1920, 1080), || (1920, 1080)), SizeCheck::Stale);
    }

    #[test]
    fn a_permission_that_cannot_be_read_does_not_start_a_session_and_is_not_a_refusal() {
        let mut tried = false;
        let r = pick_backend(Access::Unreadable("query failed".into()), || {
            tried = true;
            Ok::<_, String>(1)
        });
        assert_eq!(r, Err("query failed".to_string()));
        assert!(!tried);
        assert_eq!(Access::Allowed.why_not(), None);
        assert_eq!(Access::Not("no".into()).why_not(), Some("no"));
        assert_eq!(Access::Unreadable("?".into()).why_not(), Some("?"));
        // the engine gives a running session up only for `Not`
        assert_ne!(Access::Unreadable("?".into()), Access::Not("?".into()));
    }

    #[test]
    fn the_capture_mode_is_what_the_engine_last_set() {
        // without an engine the mode is Off
        assert_eq!(capture_mode(), CaptureMode::Off, "tests never start the engine");
        set_mode(CaptureMode::Session);
        assert_eq!(capture_mode(), CaptureMode::Session);
        set_mode(CaptureMode::Affinity);
        assert_eq!(capture_mode(), CaptureMode::Affinity);
        set_mode(CaptureMode::Off);
        assert_eq!(capture_mode(), CaptureMode::Off);
    }

    #[test]
    fn the_frame_interval_is_one_refresh_and_never_below_four_ms() {
        assert_eq!(min_interval_ticks(60), 166_666);
        assert_eq!(min_interval_ticks(144), 69_444);
        assert_eq!(min_interval_ticks(240), 41_666);
        assert_eq!(min_interval_ticks(1000), 40_000, "not below 4 ms");
        assert_eq!(min_interval_ticks(0), 160_000, "an unknown rate gets the session's own default");
        assert_eq!(min_interval_ticks(1), 160_000);
    }

    /// The long quiet wait is only safe because nothing the engine has to react to can arrive without ending it: a command, a frame of
    /// this monitor and a frame of another monitor all do, at once and not when the wait runs out.
    #[test]
    fn a_session_wait_ends_at_once_for_a_command_or_a_frame_on_any_monitor() {
        use super::super::Cmd;
        use super::super::wgc::{WaitEnd, wait_end};
        use crossbeam_channel::bounded;
        use std::time::{Duration, Instant};
        const LONG: Duration = Duration::from_secs(5);
        const PROMPT: Duration = Duration::from_millis(1000); // far from LONG, generous for a loaded machine
        let (wake_tx, wake) = bounded::<()>(1);
        let (other_tx, other) = bounded::<()>(1);
        let (cmd_tx, cmds) = crossbeam_channel::unbounded::<Cmd>();
        let others = [wake.clone(), other.clone()]; // the session's own channel is among them, as in the engine

        // a frame of another monitor, sent from another thread while the wait is under way
        let t = Instant::now();
        let sender = other_tx.clone(); // the monitor keeps its sender for as long as it lives; a dropped one would read ready for ever
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let _ = sender.try_send(());
        });
        assert_eq!(wait_end(&wake, &others, Some(&cmds), LONG), WaitEnd::Other);
        assert!(t.elapsed() < PROMPT, "a frame on the second monitor must end the wait, took {:?}", t.elapsed());
        let _ = h.join();
        assert!(other.try_recv().is_ok(), "the wait only looks: the other monitor takes its own token");

        // a frame of this monitor
        wake_tx.try_send(()).unwrap();
        let t = Instant::now();
        assert_eq!(wait_end(&wake, &others, Some(&cmds), LONG), WaitEnd::Other);
        assert!(t.elapsed() < PROMPT);
        let _ = wake.try_recv();

        // a command
        cmd_tx.send(Cmd::LookChanged).unwrap();
        let t = Instant::now();
        assert_eq!(wait_end(&wake, &others, Some(&cmds), LONG), WaitEnd::Command);
        assert!(t.elapsed() < PROMPT, "a command must end the wait, took {:?}", t.elapsed());
        let _ = cmds.try_recv();

        // nothing: the wait runs its time out and says nothing happened
        let t = Instant::now();
        assert_eq!(wait_end(&wake, &others, Some(&cmds), Duration::from_millis(40)), WaitEnd::Other);
        assert!(t.elapsed() >= Duration::from_millis(30), "it must really wait, took {:?}", t.elapsed());

        // a closed command queue reads ready for ever: that must not make the wait return at once, again and again
        drop(cmd_tx);
        let t = Instant::now();
        assert_eq!(wait_end(&wake, &others, Some(&cmds), Duration::from_millis(40)), WaitEnd::Other);
        assert!(t.elapsed() >= Duration::from_millis(30), "a closed queue must not turn the wait into a spin, took {:?}", t.elapsed());
    }
}
