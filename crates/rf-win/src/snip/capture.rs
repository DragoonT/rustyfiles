//! The one-time capture of the screen for the snip: every monitor into memory at its own physical size, and the list of windows with
//! the rectangles the user sees. Both are taken BEFORE any overlay window exists, so that neither shows the overlay.
//!
//! WHY GDI AND NOT DESKTOP DUPLICATION OR A CAPTURE SESSION. The glass engine (`glass/`) captures continuously, which needs a D3D device,
//! a duplication or a session per monitor kept alive, and a wait for the first frame; the duplication API also allows only a few
//! duplications per desktop, and the engine may be holding them when the user presses the key. A snip needs ONE picture of each monitor
//! at one moment: a `BitBlt` of the screen device context with `CAPTUREBLT` (layered windows included) into a DIB section is that, takes
//! tens of milliseconds, needs no device and no first-frame wait, and cannot fail because someone else has a duplication open. Its
//! limits are the ones Windows' own GDI capture has: a window excluded from capture (`WDA_EXCLUDEFROMCAPTURE`, which the glass engine
//! sets on its own windows when it works by Desktop Duplication) is left out, a protected video surface is black, and an HDR monitor is
//! captured as its SDR rendering.
//!
//! NEEDS A PER-MONITOR-DPI-AWARE PROCESS (the app is; a probe calls `desktop::monitors::make_process_per_monitor_aware` first): in an
//! unaware process the device context and the rectangles are scaled to 96 dpi and the picture would be a blurred, smaller copy.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use rf_core::desktop::geom::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, IsIconic, IsWindowVisible, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::BOOL;

use super::frame::{Frame, Shot};
use super::geom::{Monitor, Win};

/// What a capture brings.
pub struct Captured {
    pub shot: Shot,
    pub monitors: Vec<Monitor>,
    /// Topmost first.
    pub windows: Vec<Win>,
    /// How long the pixel copy took (the monitors together).
    pub took: Duration,
}

/// The monitors as the overlay needs them: physical rectangle and the scale the toolkit draws that monitor's windows at.
pub fn monitors() -> Vec<Monitor> {
    crate::desktop::monitors::list().iter().map(|m| Monitor { rect: m.rect, scale: m.dpi.max(96) as f32 / 96.0 }).collect()
}

/// Copies one rectangle of the virtual screen into a [`Frame`].
fn grab(screen: windows::Win32::Graphics::Gdi::HDC, r: Rect) -> Result<Frame, String> {
    let (w, h) = (r.width(), r.height());
    if w <= 0 || h <= 0 {
        return Err("empty monitor".to_string());
    }
    unsafe {
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            return Err("CreateCompatibleDC".to_string());
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // (negative: top-down, the order a Frame has)
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = match CreateDIBSection(Some(screen), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) if !bits.is_null() => b,
            _ => {
                let _ = DeleteDC(mem);
                return Err("CreateDIBSection".to_string());
            }
        };
        let old = SelectObject(mem, HGDIOBJ(dib.0));
        let blit = BitBlt(mem, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY | CAPTUREBLT);
        let out = if blit.is_ok() {
            let n = w as usize * h as usize * 4;
            let mut v = std::slice::from_raw_parts(bits as *const u8, n).to_vec();
            // (GDI leaves the fourth byte at 0 or at garbage: the picture is opaque)
            for px in v.as_chunks_mut::<4>().0.iter_mut() {
                px[3] = 255;
            }
            Ok(Frame { width: w as u32, height: h as u32, bgra: v })
        } else {
            Err(format!("BitBlt: {}", blit.err().map(|e| e.to_string()).unwrap_or_default()))
        };
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem);
        out
    }
}

/// Every monitor, at this moment, at its own physical size. Blocks for the time the copy takes: call it from a worker thread.
pub fn capture_screen() -> Result<(Shot, Vec<Monitor>, Duration), String> {
    let started = Instant::now();
    let mons = monitors();
    if mons.is_empty() {
        return Err("no monitor".to_string());
    }
    let screen = unsafe { GetDC(None) };
    if screen.is_invalid() {
        return Err("GetDC".to_string());
    }
    let mut shot = Shot::default();
    let mut err = None;
    for m in &mons {
        match grab(screen, m.rect) {
            Ok(f) => shot.parts.push((m.rect, f)),
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    unsafe {
        ReleaseDC(None, screen);
    }
    match err {
        Some(e) => Err(e),
        None => Ok((shot, mons, started.elapsed())),
    }
}

/// The capture and the window list together, the way the snip takes them (windows first would show a window that moved while the
/// pixels were copied; the pixels first and the list right after differ by milliseconds).
pub fn capture_all() -> Result<Captured, String> {
    let (shot, monitors, took) = capture_screen()?;
    let windows = list_windows();
    Ok(Captured { shot, monitors, windows, took })
}

// ---------------------------------------------------------------- the window list

/// What is known of a window when the list decides whether the window mode may pick it.
#[derive(Clone, Debug)]
pub struct Probe<'a> {
    pub visible: bool,
    pub minimized: bool,
    /// Hidden by DWM although "visible" (a window of another virtual desktop, a suspended app's frame).
    pub cloaked: bool,
    pub tool_window: bool,
    pub no_activate: bool,
    pub has_title: bool,
    pub class: &'a str,
    pub rect: Rect,
    /// One of our desktop cover windows.
    pub is_cover: bool,
}

/// Whether the window mode may pick the window. Pure.
pub fn accept(p: &Probe) -> bool {
    if !p.visible || p.minimized || p.cloaked || p.is_cover {
        return false;
    }
    // the desktop itself (its icons window and the wallpaper host) is not "a window" to snip: the region mode takes it
    if matches!(p.class, "Progman" | "WorkerW") {
        return false;
    }
    // tool windows without a title are the invisible helpers programs keep (and our own islands): nothing to point at
    if (p.tool_window || p.no_activate) && !p.has_title {
        return false;
    }
    // a sliver is a leftover of something else, not a window to pick
    p.rect.width() >= 8 && p.rect.height() >= 8
}

fn visible_bounds(hwnd: HWND) -> Option<Rect> {
    unsafe {
        let mut r = RECT::default();
        // (the frame the user sees: without the invisible resize border and the shadow that GetWindowRect includes)
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut c_void, size_of::<RECT>() as u32).is_err() && GetWindowRect(hwnd, &mut r).is_err() {
            return None;
        }
        Some(Rect::new(r.left, r.top, r.right, r.bottom))
    }
}

unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    let out = unsafe { &mut *(data.0 as *mut Vec<Win>) };
    unsafe {
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, size_of::<u32>() as u32);
        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let Some(rect) = visible_bounds(hwnd) else { return BOOL(1) };
        let probe = Probe {
            visible: IsWindowVisible(hwnd).as_bool(),
            minimized: IsIconic(hwnd).as_bool(),
            cloaked: cloaked != 0,
            tool_window: ex & WS_EX_TOOLWINDOW.0 != 0,
            no_activate: ex & WS_EX_NOACTIVATE.0 != 0,
            has_title: GetWindowTextLengthW(hwnd) > 0,
            class: &class,
            rect,
            is_cover: crate::desktop::cover::is_cover(hwnd.0 as isize),
        };
        if accept(&probe) {
            out.push(Win { hwnd: hwnd.0 as isize, rect });
        }
    }
    BOOL(1)
}

/// The windows the window mode can pick, topmost first. Cheap (a few milliseconds): reads, never changes a window.
pub fn list_windows() -> Vec<Win> {
    let mut v: Vec<Win> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut v as *mut Vec<Win> as isize));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(class: &str) -> Probe<'_> {
        Probe { visible: true, minimized: false, cloaked: false, tool_window: false, no_activate: false, has_title: true, class, rect: Rect::new(0, 0, 800, 600), is_cover: false }
    }

    #[test]
    fn an_ordinary_window_is_picked() {
        assert!(accept(&probe("Notepad")));
    }

    #[test]
    fn hidden_minimized_cloaked_and_cover_windows_are_not() {
        let mut p = probe("Notepad");
        p.visible = false;
        assert!(!accept(&p));
        let mut p = probe("Notepad");
        p.minimized = true;
        assert!(!accept(&p));
        let mut p = probe("ApplicationFrameWindow");
        p.cloaked = true;
        assert!(!accept(&p), "a window on another virtual desktop");
        let mut p = probe("GPUI");
        p.is_cover = true;
        assert!(!accept(&p));
    }

    #[test]
    fn the_desktop_windows_are_not_picked() {
        assert!(!accept(&probe("Progman")));
        assert!(!accept(&probe("WorkerW")));
    }

    #[test]
    fn untitled_helper_windows_and_slivers_are_not_picked() {
        let mut p = probe("X");
        p.tool_window = true;
        p.has_title = false;
        assert!(!accept(&p));
        p.has_title = true;
        assert!(accept(&p), "a titled tool window is a real one (a palette)");
        let mut q = probe("X");
        q.no_activate = true;
        q.has_title = false;
        assert!(!accept(&q));
        let mut s = probe("X");
        s.rect = Rect::new(0, 0, 5, 600);
        assert!(!accept(&s));
        s.rect = Rect::new(0, 0, 8, 8);
        assert!(accept(&s));
    }

    #[test]
    fn the_monitors_have_a_scale_of_at_least_one() {
        for m in monitors() {
            assert!(m.scale >= 1.0 && !m.rect.is_empty());
        }
    }

    #[test]
    fn the_window_list_is_read_only_and_every_entry_is_a_real_rectangle() {
        // (reads the windows of the machine the test runs on, and reports nothing about them)
        for w in list_windows() {
            assert!(w.rect.width() >= 8 && w.rect.height() >= 8);
            assert_ne!(w.hwnd, 0);
        }
    }
}
