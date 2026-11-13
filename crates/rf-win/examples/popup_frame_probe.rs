//! Where DWM puts the round corners, the shadow and a live picture of a pop-up styled like the dock's hover preview.
//!   popup_frame_probe framed|frameless <source window, hex> [x y] [ms]
//!
//! SHOWS A WINDOW for `ms` (2500): a 204 x 164 pop-up at (x, y), never activated, whose whole client area is a DWM picture of the
//! source window. `framed` is what the preview was until 2026-11-13: an island (`island::make_island`) that keeps the frame the
//! toolkit gives its windows (8 px left, right and below at 100 %). `frameless` is `island::make_popup`. A screenshot taken meanwhile
//! shows the difference: framed, the picture stands square inside a see-through band and the corners and shadow go around the band;
//! frameless, the picture has the round corners and the shadow starts at its edge.
//! The numbers it prints are the same facts without a screenshot.

use std::time::{Duration, Instant};

use rf_win::dock::layout::Rect;
use rf_win::dock::{island, panel};
use rf_win::switcher::thumbs::{Dwm, ThumbSet};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, NCCALCSIZE_PARAMS, PM_REMOVE, PeekMessageW, RegisterClassExW, WM_NCCALCSIZE, WNDCLASSEXW, WS_CAPTION, WS_CLIPCHILDREN,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOPMOST,
};
use windows::core::w;

/// The toolkit's answer for a window without a title bar: the resize frame comes off on three sides, the top stays.
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCALCSIZE && wp.0 != 0 {
        // SAFETY: with a true `wParam` the `lParam` points at the NCCALCSIZE_PARAMS the system is asking about
        let p = unsafe { &mut *(lp.0 as *mut NCCALCSIZE_PARAMS) };
        p.rgrc[0].left += 8;
        p.rgrc[0].right -= 8;
        p.rgrc[0].bottom -= 8;
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frameless = match args.get(1).map(String::as_str) {
        Some("framed") => false,
        Some("frameless") => true,
        _ => {
            eprintln!("popup_frame_probe framed|frameless <source window, hex> [x y] [ms]");
            return;
        }
    };
    let source = args.get(2).and_then(|s| isize::from_str_radix(s.trim_start_matches("0x"), 16).ok()).unwrap_or(0);
    let num = |i: usize, d: i32| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let client = Rect::from_xywh(num(3, 100), num(4, 100), 204, 164);
    let ms = num(5, 2500).max(0) as u64;
    // SAFETY: a window of this thread, made, shown and destroyed here; its messages are served below
    unsafe {
        let hinst = GetModuleHandleW(None).expect("the module");
        let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(proc), hInstance: hinst.into(), lpszClassName: w!("rustyfiles.PopupFrameProbe"), ..Default::default() };
        RegisterClassExW(&wc);
        let h = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST, wc.lpszClassName, w!(""), WS_CAPTION | WS_CLIPCHILDREN, client.left, client.top, 220, 172, None, None, Some(hinst.into()), None)
            .expect("the window");
        let key = h.0 as isize;
        let made = if frameless { island::make_popup(key) } else { island::make_island(key) };
        made.expect("styled");
        rf_win::window::set_round_corners(key);
        panel::show_at(key, client);
        let origin = panel::client_origin_in_window(key);
        let mut set = ThumbSet::new(Dwm, key).with_origin(origin);
        let pictures = set.show(&[(source, false, Rect::from_xywh(0, 0, client.width(), client.height()))]);
        let mut f = RECT::default();
        let _ = DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut f as *mut RECT as *mut _, size_of::<RECT>() as u32);
        println!("asked for the client area {client:?}");
        println!("window {:?}, frame insets {:?}, client area starts at {origin:?} in the window", island::rect(key), panel::frame_insets(key));
        println!("DWM's frame ({}, {})-({}, {}); the picture: {pictures:?}", f.left, f.top, f.right, f.bottom);
        let until = Instant::now() + Duration::from_millis(ms);
        let mut msg = MSG::default();
        while Instant::now() < until {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        drop(set);
        let _ = DestroyWindow(h);
    }
}
