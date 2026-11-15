//! Tries `dock::winmove` on ONE window, given by its handle: for a look at what "open on monitor N" and "fit to screen" of a dock
//! icon's menu do, on a window of our own (a test form), without the menu and without bringing the window forward.
//!
//!   winmove_probe                      lists the monitors with their numbers and work areas
//!   winmove_probe <hwnd> move <n>      moves the window to monitor n
//!   winmove_probe <hwnd> fit           brings the window inside its monitor's work area
//!   winmove_probe wait <program> <n>   waits 20 s for a NEW window of that program and moves it to monitor n (what the menu does
//!                                      for an app that is not running, without the start)
//!
//! Prints the window's rectangle before and after. Nothing is done to any other window.

use rf_win::dock::winmove;
use rf_win::dock::winops::WinRef;

fn rect(hwnd: isize) -> String {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let mut r = RECT::default();
    match unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r) } {
        Ok(()) => format!("{},{} {}x{}", r.left, r.top, r.right - r.left, r.bottom - r.top),
        Err(e) => format!("no rectangle: {e}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    for s in winmove::screens() {
        println!("monitor {}: work {},{} {}x{}", s.number, s.work.left, s.work.top, s.work.width(), s.work.height());
    }
    if args.first().map(String::as_str) == Some("wait") {
        let (Some(program), Some(n)) = (args.get(1), args.get(2).and_then(|n| n.parse::<u32>().ok())) else { return };
        let known = winmove::known_windows();
        println!("waiting for a new window of {program} ({} windows are there now)", known.len());
        match winmove::wait_for_new_window(std::path::Path::new(program), &known, std::time::Duration::from_secs(20)) {
            Some(w) => {
                println!("found {}: {}", w.hwnd, rect(w.hwnd));
                let done = winmove::move_to_screen(w, n);
                println!("after:  {}  (done: {done})", rect(w.hwnd));
            }
            None => println!("none came"),
        }
        return;
    }
    let Some(hwnd) = args.first().and_then(|a| a.parse::<isize>().ok()) else { return };
    let mut pid = 0u32;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(windows::Win32::Foundation::HWND(hwnd as *mut _), Some(&mut pid));
    }
    let w = WinRef { hwnd, pid };
    println!("before: {}", rect(hwnd));
    let done = match (args.get(1).map(String::as_str), args.get(2).and_then(|n| n.parse::<u32>().ok())) {
        (Some("move"), Some(n)) => winmove::move_to_screen(w, n),
        (Some("fit"), _) => winmove::fit_to_screen(w),
        _ => {
            println!("nothing asked");
            return;
        }
    };
    println!("after:  {}  (done: {done})", rect(hwnd));
}
