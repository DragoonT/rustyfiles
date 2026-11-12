//! Wallpaper pixels: decode the picture once, cut / scale / tile it to ONE bitmap per monitor, in the monitor's physical size, with the
//! background colour in the bars (shell plan F1; note 4, 4.7). What goes where is decided by `rf_core::desktop::placement`; this file
//! moves pixels. Nothing is drawn per frame: the caller uploads each bitmap once.
//!
//! DECODING reuses the previewer's decoder (`look::decode::decode_full`: the `image` crate, then WIC for JPEG XR, HEIC and so on), which
//! sniffs the bytes, so the extensionless `TranscodedWallpaper` works. The full-size decode lives only as long as the monitors that use
//! it are being composed, then it is dropped (a 6016 x 4147 picture is 100 MB of pixels).
//!
//! CACHE. Before decoding, the bitmap for (path, modified time, size, target size, mode, colour, arrangement) is looked up in a
//! [`DiskCache`]; a hit costs one read of a file of the monitor's size and no decode. A miss decodes, composes and stores.
//!
//! RUN THIS OFF THE UI THREAD ([`spawn`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use image::imageops::{self, FilterType};
use image::{ImageBuffer, Rgba};
use rf_core::desktop::cover::bare::{Backdrop, backdrop_for};
use rf_core::desktop::geom::{Rect, Rgb, Size, virtual_screen};
use rf_core::desktop::placement::{Plan, TileAnchor, plan};
use rf_core::desktop::wallpaper::{CacheKey, Stamper, Wallpaper};

use super::wallcache::DiskCache;
use crate::thumbs::Bgra;

/// Turns a file into pixels. The real one is [`ImageDecoder`]; tests count calls with a fake.
pub trait Decoder {
    fn decode(&self, path: &Path) -> anyhow::Result<Bgra>;
}

/// The previewer's decoder, first frame, no downscaling below 16384 pixels on the long side.
pub struct ImageDecoder;

impl Decoder for ImageDecoder {
    fn decode(&self, path: &Path) -> anyhow::Result<Bgra> {
        let d = crate::look::decode::decode_full(path, 16384)?;
        let (first, _) = d.frames.into_iter().next().ok_or_else(|| anyhow::anyhow!("no frame"))?;
        Ok(first)
    }
}

fn fill(w: u32, h: u32, bg: Rgb) -> Bgra {
    let mut data = vec![0u8; w as usize * h as usize * 4];
    for px in data.as_chunks_mut::<4>().0.iter_mut() {
        *px = [bg.2, bg.1, bg.0, 255];
    }
    Bgra { width: w, height: h, data }
}

/// `src` over the opaque pixel `dst` (BGRA, straight alpha); the result is opaque.
#[inline]
fn blend(dst: &mut [u8], src: &[u8]) {
    let a = src[3] as u32;
    if a == 255 {
        dst.copy_from_slice(&[src[0], src[1], src[2], 255]);
    } else if a > 0 {
        for i in 0..3 {
            dst[i] = ((src[i] as u32 * a + dst[i] as u32 * (255 - a) + 127) / 255) as u8;
        }
        dst[3] = 255;
    }
}

/// Copies `pic` (all of it) onto the canvas with its top-left at (x, y), clipped to the canvas. A run of opaque pixels is one copy.
fn blit(canvas: &mut Bgra, pic: &Bgra, x: i32, y: i32) {
    let (cw, ch, pw, ph) = (canvas.width, canvas.height, pic.width, pic.height);
    let x0 = (-x).max(0);
    let x1 = (cw as i32 - x).min(pw as i32);
    if x1 <= x0 {
        return;
    }
    for py in 0..ph as i32 {
        let cy = y + py;
        if cy < 0 || cy >= ch as i32 {
            continue;
        }
        let row_s = py as usize * pw as usize * 4;
        let row_d = cy as usize * cw as usize * 4;
        let flush = |canvas: &mut Bgra, from: i32, to: i32| {
            let (s, d, n) = (row_s + from as usize * 4, row_d + (x + from) as usize * 4, (to - from) as usize * 4);
            canvas.data[d..d + n].copy_from_slice(&pic.data[s..s + n]);
        };
        let mut run_start: Option<i32> = None;
        for px in x0..x1 {
            let s = row_s + px as usize * 4;
            if pic.data[s + 3] == 255 {
                run_start.get_or_insert(px);
            } else {
                if let Some(from) = run_start.take() {
                    flush(canvas, from, px);
                }
                let d = row_d + (x + px) as usize * 4;
                blend(&mut canvas.data[d..d + 4], &pic.data[s..s + 4]);
            }
        }
        if let Some(from) = run_start {
            flush(canvas, from, x1);
        }
    }
}

/// The part `src` of the picture, resampled to `w` x `h`: the same size is a plain copy; otherwise Windows' imaging component (native,
/// SIMD, as fast in a debug build as in a release one), and the `image` crate's resampler if WIC cannot do it.
fn scale_crop(pic: &Bgra, src: Rect, w: u32, h: u32) -> Vec<u8> {
    let (sw, sh) = (src.width().max(1) as u32, src.height().max(1) as u32);
    if (sw, sh) == (w, h) {
        // one to one: copy the rows, no resampling
        let mut out = Vec::with_capacity(w as usize * h as usize * 4);
        for row in 0..h {
            let start = (((src.top as u32 + row) * pic.width + src.left as u32) * 4) as usize;
            out.extend_from_slice(&pic.data[start..start + w as usize * 4]);
        }
        return out;
    }
    match scale_crop_wic(pic, src, w, h) {
        Ok(v) => v,
        Err(e) => {
            log::debug!("WIC scaling failed ({e}); using the image crate");
            scale_crop_image(pic, src, w, h)
        }
    }
}

/// This thread's COM, for WIC. WIC is in-process and free-threaded, so the worker joins the multithreaded apartment. It must NOT
/// become a single-threaded one: that kind of apartment on a thread that never pumps messages is what hung the shell when workers
/// asked it for icons (see `sta::call`), and nothing here needs it. A thread that is already an apartment of the other kind keeps it
/// (`RPC_E_CHANGED_MODE`), which WIC does not mind.
fn ensure_mta() {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    DONE.with(|d| {
        if !d.get() {
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            d.set(true);
        }
    });
}

/// WIC: a bitmap made from the picture's memory, a clipper for the part, a scaler for the size. HighQualityCubic is WIC's good filter
/// for both directions (it widens its sampling area when shrinking).
fn scale_crop_wic(pic: &Bgra, src: Rect, w: u32, h: u32) -> windows::core::Result<Vec<u8>> {
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    ensure_mta();
    unsafe {
        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let bmp = f.CreateBitmapFromMemory(pic.width, pic.height, &GUID_WICPixelFormat32bppBGRA, pic.width * 4, &pic.data)?;
        let clip = f.CreateBitmapClipper()?;
        clip.Initialize(&bmp, &WICRect { X: src.left, Y: src.top, Width: src.width(), Height: src.height() })?;
        let sc = f.CreateBitmapScaler()?;
        sc.Initialize(&clip, w, h, WICBitmapInterpolationModeHighQualityCubic)?;
        let mut out = vec![0u8; w as usize * h as usize * 4];
        sc.CopyPixels(std::ptr::null(), w * 4, &mut out)?;
        Ok(out)
    }
}

/// The `image` crate: Lanczos when shrinking, Catmull-Rom when enlarging (Lanczos rings when enlarging). The fallback; slow in a debug build.
fn scale_crop_image(pic: &Bgra, src: Rect, w: u32, h: u32) -> Vec<u8> {
    let (sw, sh) = (src.width().max(1) as u32, src.height().max(1) as u32);
    // the channels are resampled one by one, so the byte order does not matter; the alpha is straight
    let buf = ImageBuffer::<Rgba<u8>, &[u8]>::from_raw(pic.width, pic.height, &pic.data[..]).expect("pixel buffer size");
    let view = imageops::crop_imm(&buf, src.left as u32, src.top as u32, sw, sh);
    let filter = if w < sw || h < sh { FilterType::Lanczos3 } else { FilterType::CatmullRom };
    imageops::resize(&*view, w, h, filter).into_raw()
}

/// The monitor's bitmap from a plan. `picture` is `None` for a solid colour.
pub fn compose(picture: Option<&Bgra>, plan: &Plan, bg: Rgb) -> Bgra {
    let (w, h) = (plan.target.w, plan.target.h);
    let mut canvas = fill(w, h, bg);
    if let (Some(pic), true) = (picture, plan.has_picture()) {
        if let Some((ox, oy)) = plan.tile {
            let (pw, ph) = (pic.width.max(1) as i32, pic.height.max(1) as i32);
            let mut y = oy;
            while y < h as i32 {
                let mut x = ox;
                while x < w as i32 {
                    blit(&mut canvas, pic, x, y);
                    x += pw;
                }
                y += ph;
            }
        } else {
            let (dw, dh) = (plan.dst.width() as u32, plan.dst.height() as u32);
            let scaled = Bgra { width: dw, height: dh, data: scale_crop(pic, plan.src, dw, dh) };
            blit(&mut canvas, &scaled, plan.dst.left, plan.dst.top);
        }
    }
    canvas
}

/// One monitor's finished bitmap.
#[derive(Clone)]
pub struct MonitorBitmap {
    pub device: String,
    pub rect: Rect,
    pub bitmap: std::sync::Arc<Bgra>,
    /// Served from the disk cache (no decode).
    pub from_cache: bool,
    /// Why the picture is not on it, when it is not (the file is missing, it cannot be decoded).
    pub note: Option<String>,
}

pub struct RenderOptions<'a> {
    pub anchor: TileAnchor,
    pub cache: Option<&'a DiskCache>,
}

/// One bitmap per monitor of `wp`, in the same order. The pictures are decoded once each, however many monitors show them.
pub fn render_all(wp: &Wallpaper, opts: &RenderOptions, decoder: &dyn Decoder, stamper: &dyn Stamper) -> Vec<MonitorBitmap> {
    let rects: Vec<Rect> = wp.monitors.iter().map(|m| m.rect).collect();
    let vs = virtual_screen(&rects);
    let mut out: Vec<Option<MonitorBitmap>> = (0..wp.monitors.len()).map(|_| None).collect();
    let solid = |i: usize, note: Option<String>| {
        let m = &wp.monitors[i];
        let p = Plan::solid(m.rect.size());
        MonitorBitmap { device: m.device.clone(), rect: m.rect, bitmap: std::sync::Arc::new(compose(None, &p, wp.background)), from_cache: false, note }
    };

    // What each monitor shows is `bare::backdrop_for`'s rule (the picture with its fit mode, or the colour: never a black window), asked
    // here so that the rule that is tested is the one that runs. A file's stamp is read once however many monitors show it.
    let stamps: std::cell::RefCell<BTreeMap<PathBuf, Option<rf_core::desktop::wallpaper::FileStamp>>> = Default::default();
    let stamp_of = |p: &Path| *stamps.borrow_mut().entry(p.to_path_buf()).or_insert_with(|| stamper.stamp(p));
    // monitors by picture path, so each picture is decoded once
    let mut by_path: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (i, slot) in out.iter_mut().enumerate() {
        let unreadable = std::cell::Cell::new(false);
        match backdrop_for(wp, i, &|p| {
            let ok = stamp_of(p).is_some();
            unreadable.set(!ok);
            ok
        }) {
            Backdrop::Picture { path, .. } => by_path.entry(path).or_default().push(i),
            Backdrop::Colour(_) => *slot = Some(solid(i, unreadable.get().then(|| "the picture file is missing".to_string()))),
        }
    }
    for (path, monitors) in by_path {
        let Some(stamp) = stamp_of(&path) else { continue }; // (a picture only gets here when its stamp was read)
        let key_of = |i: usize| CacheKey::new(&path, stamp, wp.monitors[i].rect, vs, wp.position, wp.background, opts.anchor);
        // the cache first
        let mut misses = Vec::new();
        for &i in &monitors {
            match opts.cache.and_then(|c| c.get(&key_of(i))) {
                Some(b) if (b.width, b.height) == (wp.monitors[i].rect.width() as u32, wp.monitors[i].rect.height() as u32) => {
                    out[i] = Some(MonitorBitmap { device: wp.monitors[i].device.clone(), rect: wp.monitors[i].rect, bitmap: std::sync::Arc::new(b), from_cache: true, note: None });
                }
                _ => misses.push(i),
            }
        }
        if misses.is_empty() {
            continue;
        }
        match decoder.decode(&path) {
            Ok(pic) => {
                for i in misses {
                    let p = plan(wp.position, Size::new(pic.width, pic.height), wp.monitors[i].rect, vs, opts.anchor);
                    let bmp = compose(Some(&pic), &p, wp.background);
                    if let Some(c) = opts.cache
                        && let Err(e) = c.put(&key_of(i), &bmp)
                    {
                        log::debug!("wallpaper cache not written: {e}");
                    }
                    out[i] = Some(MonitorBitmap { device: wp.monitors[i].device.clone(), rect: wp.monitors[i].rect, bitmap: std::sync::Arc::new(bmp), from_cache: false, note: None });
                }
                // `pic` is dropped here: the full-size decode does not outlive its monitors
            }
            Err(e) => {
                for i in misses {
                    out[i] = Some(solid(i, Some(format!("the picture could not be decoded: {e:#}"))));
                }
            }
        }
    }
    out.into_iter().map(|o| o.expect("every monitor has a bitmap")).collect()
}

/// Renders on a thread of its own and hands the result to `done`. The thread runs at below-normal priority: a wallpaper is never urgent.
pub fn spawn(wp: Wallpaper, anchor: TileAnchor, cache: Option<DiskCache>, done: impl FnOnce(Vec<MonitorBitmap>) + Send + 'static) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("rf-wallpaper".into()).spawn(move || {
        unsafe {
            use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
        let r = render_all(&wp, &RenderOptions { anchor, cache: cache.as_ref() }, &ImageDecoder, &super::wallpaper::DiskStamper);
        done(r);
    })
}

#[cfg(test)]
#[allow(clippy::chunks_exact_to_as_chunks)] // tests compare whole pixels; the lint is new and wants const-generic chunks everywhere
mod tests {
    use rf_core::desktop::wallpaper::BackgroundKind;
    use super::*;
    use rf_core::desktop::placement::Position;
    use rf_core::desktop::wallpaper::{FileStamp, MonitorWallpaper, Source};
    use std::cell::{Cell, RefCell};

    const RED: [u8; 4] = [0, 0, 255, 255]; // BGRA
    const BLUE: [u8; 4] = [255, 0, 0, 255];
    const BG: Rgb = Rgb(10, 20, 30);
    const BG_PX: [u8; 4] = [30, 20, 10, 255];

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Bgra {
        Bgra { width: w, height: h, data: px.repeat(w as usize * h as usize) }
    }
    /// Left half red, right half blue.
    fn halves(w: u32, h: u32) -> Bgra {
        let mut d = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                d.extend(if x < w / 2 { RED } else { BLUE });
            }
        }
        Bgra { width: w, height: h, data: d }
    }
    fn px(b: &Bgra, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * b.width + x) * 4) as usize;
        b.data[i..i + 4].try_into().unwrap()
    }
    fn p(mode: Position, pic: &Bgra, mon: Rect) -> Plan {
        plan(mode, Size::new(pic.width, pic.height), mon, mon, TileAnchor::VirtualScreen)
    }
    const M100: Rect = Rect::new(0, 0, 100, 100);

    #[test]
    fn fit_draws_bars_in_the_background_colour() {
        let pic = solid(40, 20, RED);
        let b = compose(Some(&pic), &p(Position::Fit, &pic, M100), BG);
        assert_eq!((b.width, b.height), (100, 100));
        assert_eq!(px(&b, 50, 10), BG_PX);
        assert_eq!(px(&b, 50, 24), BG_PX);
        assert_eq!(px(&b, 50, 25), RED);
        assert_eq!(px(&b, 0, 50), RED);
        assert_eq!(px(&b, 99, 74), RED);
        assert_eq!(px(&b, 50, 75), BG_PX);
        assert_eq!(px(&b, 50, 90), BG_PX);
    }

    #[test]
    fn center_leaves_the_surroundings_alone() {
        let pic = solid(4, 4, BLUE);
        let b = compose(Some(&pic), &p(Position::Center, &pic, Rect::new(0, 0, 10, 10)), BG);
        assert_eq!(px(&b, 3, 3), BLUE);
        assert_eq!(px(&b, 6, 6), BLUE);
        assert_eq!(px(&b, 2, 3), BG_PX);
        assert_eq!(px(&b, 7, 6), BG_PX);
        assert_eq!(px(&b, 0, 0), BG_PX);
        // a picture bigger than the monitor: only its middle shows
        let big = halves(20, 10);
        let b = compose(Some(&big), &p(Position::Center, &big, Rect::new(0, 0, 10, 10)), BG);
        assert_eq!((px(&b, 0, 0), px(&b, 4, 0), px(&b, 5, 0), px(&b, 9, 9)), (RED, RED, BLUE, BLUE));
    }

    #[test]
    fn fill_crops_exactly_without_resampling_at_one_to_one() {
        // 20 x 10 on 10 x 10: the middle 10 columns (5 to 15), copied as they are
        let pic = halves(20, 10);
        let b = compose(Some(&pic), &p(Position::Fill, &pic, M100.intersect(&Rect::new(0, 0, 10, 10))), BG);
        assert_eq!((px(&b, 0, 0), px(&b, 4, 0), px(&b, 5, 0), px(&b, 9, 0)), (RED, RED, BLUE, BLUE));
    }

    #[test]
    fn stretch_and_a_solid_picture_survive_any_scaling() {
        let pic = solid(7, 3, RED);
        let b = compose(Some(&pic), &p(Position::Stretch, &pic, M100), BG);
        assert!(b.data.chunks_exact(4).all(|c| c == RED), "a solid picture stays solid through Lanczos");
        let small = solid(1000, 500, BLUE);
        let b = compose(Some(&small), &p(Position::Fill, &small, M100), BG);
        assert!(b.data.chunks_exact(4).all(|c| c == BLUE), "also when shrinking");
    }

    #[test]
    fn tile_repeats_from_the_given_origin() {
        // 2 x 2 picture: (0,0) red, (1,0) blue, (0,1) blue, (1,1) red
        let pic = Bgra { width: 2, height: 2, data: [RED, BLUE, BLUE, RED].concat() };
        let mon = Rect::new(0, 0, 5, 5);
        let plan = Plan { target: Size::new(5, 5), src: Rect::new(0, 0, 2, 2), dst: Rect::new(0, 0, 5, 5), bars: vec![], tile: Some((-1, -1)) };
        let b = compose(Some(&pic), &plan, BG);
        for y in 0..5u32 {
            for x in 0..5u32 {
                let (tx, ty) = ((x + 1) % 2, (y + 1) % 2);
                let want = if tx == ty { RED } else { BLUE };
                assert_eq!(px(&b, x, y), want, "({x},{y})");
            }
        }
        // and through the planner: the monitor at the virtual screen's corner starts a tile at its corner
        let pl = plan_for_tile(&pic, mon);
        assert_eq!(pl.tile, Some((0, 0)));
    }
    fn plan_for_tile(pic: &Bgra, mon: Rect) -> Plan {
        plan(Position::Tile, Size::new(pic.width, pic.height), mon, mon, TileAnchor::VirtualScreen)
    }

    #[test]
    fn a_transparent_picture_shows_the_background_through() {
        let pic = Bgra { width: 2, height: 1, data: [[1, 2, 3, 0], [200, 100, 50, 255]].concat() };
        let plan = Plan { target: Size::new(2, 1), src: Rect::new(0, 0, 2, 1), dst: Rect::new(0, 0, 2, 1), bars: vec![], tile: None };
        let b = compose(Some(&pic), &plan, BG);
        assert_eq!(px(&b, 0, 0), BG_PX);
        assert_eq!(px(&b, 1, 0), [200, 100, 50, 255]);
        // half transparent: halfway between
        let half = Bgra { width: 1, height: 1, data: vec![0, 0, 0, 128] };
        let plan = Plan { target: Size::new(1, 1), src: Rect::new(0, 0, 1, 1), dst: Rect::new(0, 0, 1, 1), bars: vec![], tile: None };
        let b = compose(Some(&half), &plan, Rgb(200, 200, 200));
        assert_eq!(px(&b, 0, 0)[0], 100, "(0*128 + 200*127 + 127) / 255");
        assert_eq!(px(&b, 0, 0)[3], 255);
    }

    #[test]
    fn a_span_across_two_monitors_joins_up() {
        let left = Rect::new(0, 0, 10, 10);
        let right = Rect::new(10, 0, 20, 10);
        let pic = halves(20, 10);
        let pl = plan_all_for(&pic, &[left, right]);
        let a = compose(Some(&pic), &pl[0], BG);
        let c = compose(Some(&pic), &pl[1], BG);
        assert!(a.data.chunks_exact(4).all(|x| x == RED));
        assert!(c.data.chunks_exact(4).all(|x| x == BLUE));
    }
    fn plan_all_for(pic: &Bgra, mons: &[Rect]) -> Vec<Plan> {
        rf_core::desktop::placement::plan_all(Position::Span, Size::new(pic.width, pic.height), mons, TileAnchor::VirtualScreen)
    }

    #[test]
    fn opaque_runs_and_translucent_pixels_in_one_row_blit_correctly() {
        // opaque, translucent, opaque, transparent, opaque
        let row = Bgra { width: 5, height: 1, data: [[1, 1, 1, 255], [100, 100, 100, 128], [2, 2, 2, 255], [9, 9, 9, 0], [3, 3, 3, 255]].concat() };
        let pixels = |b: &Bgra| -> Vec<[u8; 4]> { b.data.chunks_exact(4).map(|c| c.try_into().unwrap()).collect() };
        let mut canvas = fill(5, 1, Rgb(200, 200, 200));
        blit(&mut canvas, &row, 0, 0);
        assert_eq!(pixels(&canvas), [[1, 1, 1, 255], [150, 150, 150, 255], [2, 2, 2, 255], [200, 200, 200, 255], [3, 3, 3, 255]]);
        // clipped on the left and the right
        let mut canvas = fill(3, 1, Rgb(0, 0, 0));
        blit(&mut canvas, &row, -1, 0);
        assert_eq!(pixels(&canvas), [[50, 50, 50, 255], [2, 2, 2, 255], [0, 0, 0, 255]]);
        // entirely off the canvas
        let mut canvas = fill(3, 1, Rgb(7, 7, 7));
        blit(&mut canvas, &row, 10, 0);
        blit(&mut canvas, &row, -10, 0);
        blit(&mut canvas, &row, 0, 4);
        assert!(pixels(&canvas).iter().all(|c| *c == [7, 7, 7, 255]));
    }

    #[test]
    fn the_wic_scaler_and_the_image_crate_agree() {
        // a horizontal gradient 64 x 32, cut and shrunk / enlarged both ways
        let mut data = Vec::new();
        for _ in 0..32 {
            for x in 0..64u32 {
                let v = (x * 4) as u8;
                data.extend([v, 255 - v, 128, 255]);
            }
        }
        let pic = Bgra { width: 64, height: 32, data };
        for (src, w, h) in [(Rect::new(0, 0, 64, 32), 16, 8), (Rect::new(8, 4, 40, 20), 64, 32), (Rect::new(0, 0, 64, 32), 100, 50)] {
            let a = scale_crop_wic(&pic, src, w, h).expect("WIC is available");
            let b = scale_crop_image(&pic, src, w, h);
            assert_eq!((a.len(), b.len()), (w as usize * h as usize * 4, w as usize * h as usize * 4));
            // the middle row, interior columns (the filters treat the very edges differently): within a few levels
            let row = (h / 2) as usize * w as usize * 4;
            for x in (w as usize / 8)..(w as usize * 7 / 8) {
                for c in 0..3 {
                    let (p, q) = (a[row + x * 4 + c] as i32, b[row + x * 4 + c] as i32);
                    assert!((p - q).abs() <= 12, "{src:?} -> {w}x{h}, column {x}, channel {c}: wic {p} vs image {q}");
                }
                assert_eq!(a[row + x * 4 + 3], 255);
            }
        }
    }

    #[test]
    fn the_scaling_worker_is_not_a_single_threaded_apartment() {
        // a fresh thread, as the wallpaper worker is: after scaling with WIC it must be in the multithreaded apartment, never in a
        // single-threaded one that does not pump messages
        let t = std::thread::spawn(|| {
            use windows::Win32::System::Com::{APTTYPE, APTTYPE_MAINSTA, APTTYPE_MTA, APTTYPE_NA, APTTYPE_STA, CoGetApartmentType};
            let pic = Bgra { width: 8, height: 8, data: vec![200; 8 * 8 * 4] };
            let out = scale_crop_wic(&pic, Rect::new(0, 0, 8, 8), 4, 4).expect("WIC is available");
            assert_eq!(out.len(), 4 * 4 * 4);
            let (mut kind, mut qual) = (APTTYPE(0), Default::default());
            unsafe { CoGetApartmentType(&mut kind, &mut qual) }.expect("this thread has an apartment");
            assert!(kind != APTTYPE_STA && kind != APTTYPE_MAINSTA, "single-threaded apartment: {kind:?}");
            assert!(kind == APTTYPE_MTA || kind == APTTYPE_NA, "{kind:?}");
        });
        t.join().unwrap();
    }

    #[test]
    fn no_picture_is_the_colour() {
        let b = compose(None, &Plan::solid(Size::new(3, 2)), BG);
        assert!(b.data.chunks_exact(4).all(|c| c == BG_PX) && b.data.len() == 24);
    }

    // ---- the pipeline with a fake decoder, a fake clock for the file and a real cache in a temp folder

    struct CountingDecoder {
        calls: Cell<u32>,
        pic: Bgra,
        fail: bool,
    }
    impl Decoder for CountingDecoder {
        fn decode(&self, _: &Path) -> anyhow::Result<Bgra> {
            self.calls.set(self.calls.get() + 1);
            if self.fail { anyhow::bail!("not a picture") } else { Ok(self.pic.clone()) }
        }
    }
    struct Stamps(RefCell<BTreeMap<PathBuf, FileStamp>>);
    impl Stamper for Stamps {
        fn stamp(&self, p: &Path) -> Option<FileStamp> {
            self.0.borrow().get(p).copied()
        }
    }

    fn wp(pics: &[Option<&str>], mode: Position) -> Wallpaper {
        Wallpaper {
            source: Source::DesktopWallpaperCom,
            monitors: pics
                .iter()
                .enumerate()
                .map(|(i, p)| MonitorWallpaper { device: format!("M{i}"), rect: Rect::from_size(i as i32 * 100, 0, 100, 50), picture: p.map(PathBuf::from) })
                .collect(),
            position: mode,
            position_assumed: false,
            background: BG,
            kind: BackgroundKind::Picture,
            slideshow: None,
            per_desktop: vec![],
        }
    }
    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-render-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn one_decode_for_many_monitors_and_none_for_a_second_run() {
        let dir = temp("pipe");
        let cache = DiskCache::new(dir.clone(), 8);
        let dec = CountingDecoder { calls: Cell::new(0), pic: solid(400, 300, RED), fail: false };
        let stamps = Stamps(RefCell::new([(PathBuf::from(r"D:\a.png"), FileStamp { modified_ms: 10, size: 5 })].into()));
        let w = wp(&[Some(r"D:\a.png"), Some(r"D:\a.png"), None], Position::Fill);
        let opts = RenderOptions { anchor: TileAnchor::VirtualScreen, cache: Some(&cache) };
        let r = render_all(&w, &opts, &dec, &stamps);
        assert_eq!(r.len(), 3);
        assert_eq!(dec.calls.get(), 1, "two monitors, one picture, one decode");
        assert!(r[0].bitmap.data.chunks_exact(4).all(|c| c == RED) && !r[0].from_cache);
        assert!(r[2].bitmap.data.chunks_exact(4).all(|c| c == BG_PX), "no picture: the colour");
        assert_eq!((r[1].bitmap.width, r[1].bitmap.height), (100, 50));
        // again: all from the cache, no decode
        let r2 = render_all(&w, &opts, &dec, &stamps);
        assert_eq!(dec.calls.get(), 1);
        assert!(r2[0].from_cache && r2[1].from_cache && !r2[2].from_cache);
        assert_eq!(r2[0].bitmap.data, r[0].bitmap.data);
        // the file was rewritten in place (new modified time): decode again
        stamps.0.borrow_mut().get_mut(Path::new(r"D:\a.png")).unwrap().modified_ms = 11;
        render_all(&w, &opts, &dec, &stamps);
        assert_eq!(dec.calls.get(), 2);
        // another mode: a different bitmap, decode again; the old entries are still valid
        let w2 = wp(&[Some(r"D:\a.png")], Position::Fit);
        render_all(&w2, &opts, &dec, &stamps);
        assert_eq!(dec.calls.get(), 3);
        render_all(&w2, &opts, &dec, &stamps);
        assert_eq!(dec.calls.get(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_broken_picture_is_a_solid_colour_with_a_note() {
        let dec = CountingDecoder { calls: Cell::new(0), pic: solid(1, 1, RED), fail: true };
        let stamps = Stamps(RefCell::new([(PathBuf::from(r"D:\bad.png"), FileStamp { modified_ms: 1, size: 1 })].into()));
        let r = render_all(&wp(&[Some(r"D:\bad.png"), Some(r"D:\gone.png")], Position::Fill), &RenderOptions { anchor: TileAnchor::Monitor, cache: None }, &dec, &stamps);
        assert!(r[0].note.as_deref().is_some_and(|n| n.contains("could not be decoded")));
        assert!(r[1].note.as_deref().is_some_and(|n| n.contains("missing")));
        assert_eq!(dec.calls.get(), 1, "the missing file is not even opened");
        assert!(r.iter().all(|m| m.bitmap.data.chunks_exact(4).all(|c| c == BG_PX)));
    }

    #[test]
    fn a_solid_colour_background_draws_no_picture() {
        let dec = CountingDecoder { calls: Cell::new(0), pic: solid(1, 1, RED), fail: false };
        let mut w = wp(&[Some(r"D:\a.png")], Position::Fill);
        w.kind = BackgroundKind::SolidColor;
        let r = render_all(&w, &RenderOptions { anchor: TileAnchor::Monitor, cache: None }, &dec, &Stamps(RefCell::new(BTreeMap::new())));
        assert_eq!(dec.calls.get(), 0);
        assert!(r[0].bitmap.data.chunks_exact(4).all(|c| c == BG_PX));
    }

    #[test]
    fn the_real_decoder_reads_an_extensionless_png_and_a_jpeg_by_their_bytes() {
        let dir = temp("real");
        let mut img = image::RgbaImage::new(8, 4);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 4 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) };
        }
        let png = dir.join("TranscodedWallpaper");
        img.save_with_format(&png, image::ImageFormat::Png).unwrap();
        let b = ImageDecoder.decode(&png).unwrap();
        assert_eq!((b.width, b.height), (8, 4));
        assert_eq!(px(&b, 0, 0), RED);
        assert_eq!(px(&b, 7, 3), BLUE);
        // the whole way: decode, plan, compose, with the disk stamper, on a real file
        let mut w = wp(&[None], Position::Stretch);
        w.monitors[0].picture = Some(png.clone());
        w.monitors[0].rect = Rect::new(0, 0, 16, 8);
        let r = render_all(&w, &RenderOptions { anchor: TileAnchor::Monitor, cache: None }, &ImageDecoder, &super::super::wallpaper::DiskStamper);
        assert_eq!(r[0].note, None);
        assert_eq!((px(&r[0].bitmap, 0, 0), px(&r[0].bitmap, 15, 7)), (RED, BLUE));
        // a jpeg with the wrong extension
        let jpg = dir.join("looks-like.txt");
        image::DynamicImage::ImageRgba8(img).to_rgb8().save_with_format(&jpg, image::ImageFormat::Jpeg).unwrap();
        let b = ImageDecoder.decode(&jpg).unwrap();
        assert_eq!((b.width, b.height), (8, 4));
        assert!(ImageDecoder.decode(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spawn_runs_off_the_calling_thread() {
        let (tx, rx) = std::sync::mpsc::channel();
        let me = std::thread::current().id();
        let mut w = wp(&[None], Position::Fill);
        w.monitors[0].picture = None;
        spawn(w, TileAnchor::Monitor, None, move |r| {
            let _ = tx.send((std::thread::current().id(), r.len()));
        })
        .unwrap();
        let (tid, n) = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_ne!(tid, me);
        assert_eq!(n, 1);
    }
}
