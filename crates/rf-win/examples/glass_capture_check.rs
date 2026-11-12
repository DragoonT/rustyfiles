//! Windowless manual check of the glass's capture session backend (not a test). It opens the engine's own session code on the primary
//! monitor, puts ONE existing visible top-level window (the largest on that monitor, at least 1 % of it, whose display affinity is 0) into the session's
//! exclusion list, waits for the first picture made with that list, saves the mirror as a PNG, closes everything and prints the timings
//! and the backend's log lines. A second capture with an EMPTY list follows, to count how much of the window's rectangle differs.
//!
//!   cargo run -p rf-win --example glass_capture_check -- OUT.png
//!
//! Nothing is shown and no window is touched (windows are only read): no window is created, no picker, no consent prompt. The border-free
//! permission is only READ; when it is not Allowed the example says so and captures nothing.

use std::ffi::c_void;
use std::fs::File;
use std::io::BufWriter;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowDisplayAffinity, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    WS_EX_TOOLWINDOW,
};
use windows::core::BOOL;

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target().starts_with("rf_win")
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            println!("[log {}] {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

struct Cand {
    hwnd: isize,
    class: String,
    rect: RECT,
}

/// The visible, uncloaked, not minimised top-level windows with display affinity 0 that are not ours, in z order.
fn candidates() -> Vec<Cand> {
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        let v = unsafe { &mut *(lp.0 as *mut Vec<Cand>) };
        unsafe {
            if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
                return BOOL(1);
            }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4).is_ok() && cloaked != 0 {
                return BOOL(1);
            }
            let mut aff = 0u32;
            if GetWindowDisplayAffinity(h, &mut aff).is_err() || aff != 0 {
                return BOOL(1);
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid == GetCurrentProcessId() || GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                return BOOL(1);
            }
            let mut name = [0u16; 128];
            let n = GetClassNameW(h, &mut name).max(0) as usize;
            let class = String::from_utf16_lossy(&name[..n]);
            // the desktop and the shell's own surfaces are not "a window" for this check
            if ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Windows.UI.Core.CoreWindow"].contains(&class.as_str()) {
                return BOOL(1);
            }
            let mut r = RECT::default();
            if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut c_void, size_of::<RECT>() as u32).is_err() {
                return BOOL(1);
            }
            v.push(Cand { hwnd: h.0 as isize, class, rect: r });
        }
        BOOL(1)
    }
    let mut v: Vec<Cand> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut v as *mut Vec<Cand> as isize));
    }
    v
}

fn area(r: &RECT) -> i64 {
    ((r.right - r.left).max(0) as i64) * ((r.bottom - r.top).max(0) as i64)
}

fn inter(a: &RECT, b: &RECT) -> RECT {
    RECT { left: a.left.max(b.left), top: a.top.max(b.top), right: a.right.min(b.right), bottom: a.bottom.min(b.bottom) }
}

fn save(path: &str, w: u32, h: u32, bgra: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for p in bgra.as_chunks::<4>().0 {
        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() {
    let Some(out) = std::env::args().nth(1) else {
        eprintln!("usage: glass_capture_check OUT.png");
        std::process::exit(2);
    };
    let _apartment = rf_win::glass::check::enter_apartment();
    let _ = log::set_logger(&Logger).map(|()| log::set_max_level(log::LevelFilter::Debug));
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // the monitor that contains a point, as a rectangle
    let monitor_at = |x: i32, y: i32| unsafe {
        let hm = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hm, &mut mi);
        mi.rcMonitor
    };
    let primary = monitor_at(0, 0);
    let all = candidates();
    println!("{} eligible top-level windows (visible, uncloaked, not minimised, affinity 0, not ours)", all.len());
    for c in &all {
        println!("  eligible: class '{}' rect ({},{})-({},{})", c.class, c.rect.left, c.rect.top, c.rect.right, c.rect.bottom);
    }
    // the largest one on the primary monitor (at least 1 % of it); when there is none there, the largest on any monitor
    let on_primary = |c: &Cand| area(&inter(&c.rect, &primary));
    let mut pick = all.iter().filter(|c| on_primary(c) * 100 >= area(&primary)).max_by_key(|c| on_primary(c));
    if pick.is_none() {
        pick = all.iter().max_by_key(|c| area(&c.rect));
        if pick.is_some() {
            println!("no eligible window on the primary monitor: the largest one on another monitor is used, and that monitor is captured");
        }
    }
    let at = pick.map_or((0, 0), |c| ((c.rect.left + c.rect.right) / 2, (c.rect.top + c.rect.bottom) / 2));
    let mon = monitor_at(at.0, at.1);
    let (list, window): (Vec<isize>, Option<&Cand>) = match pick {
        Some(c) => {
            println!(
                "excluded window: class '{}', rect ({},{})-({},{}) = {}x{} px, display affinity 0 (not changed, only read)",
                c.class,
                c.rect.left,
                c.rect.top,
                c.rect.right,
                c.rect.bottom,
                c.rect.right - c.rect.left,
                c.rect.bottom - c.rect.top
            );
            (vec![c.hwnd], Some(c))
        }
        None => {
            println!("no visible window with display affinity 0 exists: the list stays empty");
            (vec![], None)
        }
    };

    let with_list = match rf_win::glass::check::run(&list, Duration::from_secs(5), at) {
        Ok(r) => r,
        Err(e) => {
            println!("RESULT: capture session not run: {e}");
            return;
        }
    };
    println!(
        "captured monitor ({},{})-({},{}), picture {}x{}",
        with_list.monitor.0, with_list.monitor.1, with_list.monitor.2, with_list.monitor.3, with_list.width, with_list.height
    );
    println!(
        "timings: session made and started {:.1} ms; SetWindowExclusionList call {:.2} ms; first picture made with the list {:.1} ms after the start; {} frames delivered meanwhile; session reports {:?} windows in its list",
        ms(with_list.open),
        ms(with_list.list_call),
        ms(with_list.first_image),
        with_list.frames,
        with_list.reported
    );
    match save(&out, with_list.width, with_list.height, &with_list.bgra) {
        Ok(()) => println!("saved {out}"),
        Err(e) => {
            println!("RESULT: cannot save the picture: {e}");
            return;
        }
    }

    // the same capture with an empty list: the window is in this one, so the two differ inside its rectangle
    if let Some(c) = window {
        match rf_win::glass::check::run(&[], Duration::from_secs(5), at) {
            Ok(plain) if (plain.width, plain.height) == (with_list.width, with_list.height) => {
                let r = inter(&RECT { left: c.rect.left - mon.left, top: c.rect.top - mon.top, right: c.rect.right - mon.left, bottom: c.rect.bottom - mon.top }, &RECT { left: 0, top: 0, right: plain.width as i32, bottom: plain.height as i32 });
                let (mut inside, mut differ) = (0u64, 0u64);
                let w = plain.width as usize;
                for y in r.top.max(0)..r.bottom {
                    for x in r.left.max(0)..r.right {
                        let i = (y as usize * w + x as usize) * 4;
                        inside += 1;
                        if (0..3).any(|k| (plain.bgra[i + k] as i32 - with_list.bgra[i + k] as i32).abs() > 8) {
                            differ += 1;
                        }
                    }
                }
                println!(
                    "inside the window's rectangle {:.1} % of {} px differ between the capture without the window and the capture with it (about 100 % when it is left out, 0 when it is not)",
                    differ as f64 * 100.0 / inside.max(1) as f64,
                    inside
                );
            }
            Ok(_) => println!("second capture has another size, not compared"),
            Err(e) => println!("second capture failed: {e}"),
        }
    }
    println!("RESULT: done, sessions closed");
}
