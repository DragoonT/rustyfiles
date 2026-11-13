//! Windowless check of the screen snip's Windows half (not a test). No window is created, nothing is shown, the clipboard is never
//! opened and no file is written.
//!
//!   cargo run -p rf-win --example snip_probe
//!
//! WHAT IT DOES
//! 1. makes the process per-monitor DPI aware (the app is) and lists the monitors as the snip sees them;
//! 2. takes ONE real capture of every monitor into memory, checks that each picture is as large as its monitor's rectangle in device
//!    pixels, and DROPS the pixels. It reports sizes, the time the copy took and, per monitor, only whether the picture is a single
//!    colour (a black or blank result would mean the capture failed); no pixel value is printed or kept;
//! 3. lists how many windows the window mode could pick (a count, no titles);
//! 4. cuts a region across the virtual screen's middle out of the real capture, and out of a made-up picture of the same layout, and
//!    makes the three formats from the made-up one, timing each (the real pixels never go to an encoder);
//! 5. runs the clipboard writer's call order against a fake that only counts bytes: the real clipboard is not touched.

use std::time::Instant;

use rf_core::desktop::geom::Rect;
use rf_win::snip::capture;
use rf_win::snip::clip::{self, ClipApi, Format};
use rf_win::snip::encode::Formats;
use rf_win::snip::frame::{Frame, Shot};
use rf_win::snip::geom;

struct Counting {
    calls: Vec<String>,
}

impl ClipApi for Counting {
    fn open(&mut self) -> Result<(), String> {
        self.calls.push("open".into());
        Ok(())
    }
    fn empty(&mut self) -> Result<(), String> {
        self.calls.push("empty".into());
        Ok(())
    }
    fn set(&mut self, f: Format, b: &[u8]) -> Result<(), String> {
        self.calls.push(format!("set {f:?} {} bytes", b.len()));
        Ok(())
    }
    fn close(&mut self) {
        self.calls.push("close".into());
    }
}

fn main() {
    let aware = rf_win::desktop::monitors::make_process_per_monitor_aware();
    println!("per-monitor aware: {aware}");
    let monitors = capture::monitors();
    for (i, m) in monitors.iter().enumerate() {
        println!("monitor {i}: {}x{} at ({}, {}), scale {:.2}", m.rect.width(), m.rect.height(), m.rect.left, m.rect.top, m.scale);
    }
    let limit = geom::virtual_screen(&monitors);
    println!("virtual screen: {}x{} at ({}, {})", limit.width(), limit.height(), limit.left, limit.top);

    let t = Instant::now();
    let captured = match capture::capture_all() {
        Ok(c) => c,
        Err(e) => {
            println!("capture FAILED: {e}");
            std::process::exit(1);
        }
    };
    println!("capture_all: {} ms in all (pixel copy {} ms)", t.elapsed().as_millis(), captured.took.as_millis());
    let mut all_ok = captured.shot.parts.len() == monitors.len();
    let mut bytes = 0usize;
    for (i, (rect, f)) in captured.shot.parts.iter().enumerate() {
        let size_ok = f.width as i32 == rect.width() && f.height as i32 == rect.height() && f.is_consistent();
        let uniform = f.bgra.as_chunks::<4>().0.iter().all(|p| p == &f.bgra[..4]);
        let opaque = f.bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
        bytes += f.bgra.len();
        println!("picture {i}: {}x{} (monitor {}x{}), size matches: {size_ok}, opaque: {opaque}, single colour: {uniform}", f.width, f.height, rect.width(), rect.height());
        all_ok &= size_ok && opaque;
    }
    println!("pictures held: {} bytes ({:.1} MB)", bytes, bytes as f64 / 1_048_576.0);
    println!("windows the window mode can pick: {}", captured.windows.len());
    let in_limit = captured.windows.iter().filter(|w| !w.rect.intersect(&limit).is_empty()).count();
    println!("  of them on the screen: {in_limit}");

    // the cut: a region across the middle of the virtual screen, from the real capture (size only) ...
    let mid = ((limit.left + limit.right) / 2, (limit.top + limit.bottom) / 2);
    let region = Rect::new(mid.0 - 300, mid.1 - 200, mid.0 + 300, mid.1 + 200);
    let t = Instant::now();
    let cut = captured.shot.crop(region);
    println!("crop of a {}x{} region from the real capture: {:?} in {} ms", region.width(), region.height(), cut.as_ref().map(|f| (f.width, f.height)), t.elapsed().as_millis());
    all_ok &= cut.as_ref().is_some_and(|f| (f.width as i32, f.height as i32) == (region.width(), region.height()));
    drop(cut);
    // ... the real pixels are gone now
    drop(captured);

    // the formats, from a made-up picture of the same layout
    let made_up = Shot {
        parts: monitors
            .iter()
            .map(|m| {
                let (w, h) = (m.rect.width() as u32, m.rect.height() as u32);
                let mut f = Frame::filled(w, h, [0, 0, 0, 255]);
                for (i, px) in f.bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    px[0] = (i % w as usize) as u8;
                    px[1] = (i / w as usize) as u8;
                    px[2] = ((i / 7) % 251) as u8;
                }
                (m.rect, f)
            })
            .collect(),
    };
    let full = made_up.crop(region).expect("the made-up cut");
    let t = Instant::now();
    let formats = Formats::of(&full);
    println!("formats of a {}x{} picture: dib {} bytes, dibv5 {} bytes, png {} bytes, in {} ms", full.width, full.height, formats.dib.len(), formats.dibv5.len(), formats.png.len(), t.elapsed().as_millis());
    let whole = made_up.crop(monitors.first().map_or(region, |m| m.rect)).expect("a whole monitor");
    let t = Instant::now();
    let big = Formats::of(&whole);
    println!("formats of a whole first monitor ({}x{}): png {} bytes, all three in {} ms", whole.width, whole.height, big.png.len(), t.elapsed().as_millis());
    all_ok &= formats.is_complete() && big.is_complete();

    let mut fake = Counting { calls: Vec::new() };
    let r = clip::write(&mut fake, &formats);
    println!("clipboard writer against a fake: {r:?}; calls: {}", fake.calls.join(", "));
    all_ok &= r.is_ok() && fake.calls.first().map(String::as_str) == Some("open") && fake.calls.last().map(String::as_str) == Some("close");

    println!("{}", if all_ok { "ALL OK" } else { "SOMETHING IS WRONG" });
    std::process::exit(if all_ok { 0 } else { 2 });
}
