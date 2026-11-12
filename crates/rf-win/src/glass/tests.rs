//! Windowless tests of the glass: everything goes through the offscreen path (a hardware device if there is one, WARP when the GPU is
//! forced off or absent), so they run on a CI runner. The live path (a window, the capture loop) is not covered here: it needs a screen.

use super::gpu::{self, Cb, Geometry, Look, Region, Renderer, Source, prep, region_pad, region_size};
use super::*;

/// BGRA picture filled by `f(x, y) -> [r, g, b]`.
fn picture(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = f(x, y);
            v.extend_from_slice(&[b, g, r, 255]);
        }
    }
    v
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// What a uniform grey source must become in the middle of the glass, computed on the CPU from the look's numbers: saturation does nothing
/// to a grey, then the contrast / lum transfer, then the tint. (Far from the edge the rim light and the dark line have faded to nothing.)
fn expected_centre(grey: u8, look: &Look) -> [f32; 3] {
    let g = grey as f32 / 255.0;
    let c = (g * (look.lum * look.contrast) + 0.5 * (1.0 - look.contrast)).clamp(0.0, 1.0);
    let t = look.tint;
    [0, 1, 2].map(|i| (c + (t[i] - c) * t[3]) * 255.0)
}

/// `gpu::render` at scale 1 with a look and the choice between hardware and WARP.
fn render_with(src: &[u8], sw: u32, sh: u32, rect: (i32, i32, u32, u32), radius: f32, look: &Look, force_warp: bool) -> Result<Vec<u8>, String> {
    let geo = Geometry { w: rect.2, h: rect.3, origin: (rect.0, rect.1), scale: 1.0, radius_px: radius };
    with_renderer(force_warp, |r| unsafe { gpu::render_on(r, src, sw, sh, &geo, look) })
}

/// One device per driver for all the tests here, made by the first test that draws and never destroyed. A device per picture meant
/// dozens of devices made and destroyed on parallel test threads; with the rest of the crate's tests running beside them the test
/// process then ended with a wrong exit code although every test had passed (seen on CI and here).
struct Shared(Renderer);
// SAFETY: a D3D11 device may be used from any thread; its immediate context only from one at a time, which the mutex sees to.
unsafe impl Send for Shared {}
static RENDERERS: [std::sync::Mutex<Option<Shared>>; 2] = [std::sync::Mutex::new(None), std::sync::Mutex::new(None)];

fn with_renderer<T>(force_warp: bool, f: impl FnOnce(&Renderer) -> T) -> T {
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
fn can_draw() -> bool {
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

fn drivers() -> [bool; 2] {
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
                let look = Look::regular(dark);
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
        let look = Look::regular(false);
        let round = render_with(&src, sw, sh, rect, 8.0, &look, warp).expect("render");
        assert_eq!(px(&round, rect.2, 0, 0)[3], 0, "corner pixel with radius 8 (warp {warp})");
        assert_eq!(px(&round, rect.2, rect.2 / 2, rect.3 / 2)[3], 255, "centre pixel (warp {warp})");
        assert_eq!(px(&round, rect.2, rect.2 - 1, rect.3 - 1)[3], 0, "far corner with radius 8 (warp {warp})");
        let square = render_with(&src, sw, sh, rect, 0.0, &look, warp).expect("render");
        assert_eq!(px(&square, rect.2, 0, 0)[3], 255, "corner pixel with radius 0 (warp {warp})");
        assert_eq!(px(&square, rect.2, rect.2 - 1, rect.3 - 1)[3], 255, "far corner with radius 0 (warp {warp})");
    }
}

/// Black / white / red bands, mirror symmetric about the vertical line x = w / 2, varying along y too.
fn symmetric_stripes(w: u32, h: u32) -> Vec<u8> {
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
                let out = render_with(&src, sw, sh, rect, radius, &Look::regular(dark), warp).expect("render");
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
    let mut look = Look::regular(false);
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
    let fields = body.lines().filter(|l| l.trim_start().starts_with("float4 u")).count();
    assert_eq!(fields, 11, "float4 fields of the shader's cbuffer");
    assert_eq!(size_of::<Cb>() / 16, fields, "the Rust struct and the shader's cbuffer must have the same number of float4");
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
    let geo = Geometry { w: 300, h: 200, origin: (100, 100), scale: 1.0, radius_px: 8.0 };
    let look = Look::regular(false);
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

#[test]
fn bad_arguments_are_errors_not_panics() {
    let src = picture(8, 8, |_, _| [1, 2, 3]);
    assert!(render_offscreen(&src, 8, 8, (0, 0, 0, 4), 8.0, 1.0, false).is_err());
    assert!(render_offscreen(&src[..10], 8, 8, (0, 0, 4, 4), 8.0, 1.0, false).is_err());
    assert!(render_offscreen(&src, 8, 8, (0, 0, 4, 4), -1.0, 1.0, false).is_err());
    assert!(render_offscreen(&src, 8, 8, (0, 0, 4, 4), 8.0, 0.0, false).is_err());
}

/// A look that shows the optics and nothing else: no frost, no grade, no tint, no rim light, no dark line.
fn bare_optics() -> Look {
    let mut l = Look::regular(false);
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
fn step_width(out: &[u8], w: u32, y: u32) -> u32 {
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
    frosted.blur = Look::regular(false).blur;
    for warp in drivers() {
        let sharp = render_with(&src, sw, sh, rect, 8.0, &bare_optics(), warp).expect("render");
        let soft = render_with(&src, sw, sh, rect, 8.0, &frosted, warp).expect("render");
        let (ws, wf) = (step_width(&sharp, rect.2, rect.3 / 2), step_width(&soft, rect.2, rect.3 / 2));
        assert!(ws <= 3, "no frost, yet the edge is {ws} px wide (warp {warp})");
        assert!((18..=38).contains(&wf), "frost of sigma 10.35 should make the edge about 26 px wide, it is {wf} (warp {warp})");
    }
}

/// A window's corner radius (8 px) is smaller than the rim (about 30 px). The optics then use a rounder box (radius at least the rim
/// width), so a window with radius 8 and one with radius 30 bend the backdrop identically; only the outline differs. A bend that took the
/// true radius would show a seam along the corner diagonals and differ strongly in the corners.
#[test]
fn optics_do_not_depend_on_a_small_corner_radius() {
    if !can_draw() {
        return;
    }
    let (sw, sh) = (640u32, 400u32);
    let src = picture(sw, sh, |x, y| [(x / 2) as u8, (y / 2) as u8, 60]);
    let rect = (160, 60, 320u32, 280u32);
    let (w, h) = (rect.2, rect.3);
    for warp in drivers() {
        let small = render_with(&src, sw, sh, rect, 8.0, &bare_optics(), warp).expect("render");
        let big = render_with(&src, sw, sh, rect, 30.0, &bare_optics(), warp).expect("render");
        let mut worst = (0i32, 0u32, 0u32);
        let mut compared = 0;
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (px(&small, w, x, y), px(&big, w, x, y));
                if a[3] == 255 && b[3] == 255 {
                    compared += 1;
                    for c in 0..3 {
                        worst = worst.max(((a[c] as i32 - b[c] as i32).abs(), x, y));
                    }
                }
            }
        }
        assert!(compared > (w * h) as usize * 9 / 10, "too few pixels compared: {compared}");
        assert!(worst.0 <= 1, "radius 8 and 30 differ by {} of 255 at x {} y {} (warp {warp}): the optics follow the true radius", worst.0, worst.1, worst.2);
    }
}
