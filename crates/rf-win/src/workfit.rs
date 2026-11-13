//! Fitting the maximized windows of a monitor to a work area that was set without the change broadcast.
//!
//! While the host keeps Explorer's taskbar hidden it sets the monitor's work area WITHOUT `SPIF_SENDCHANGE` (with the broadcast
//! Explorer takes the rectangle back within 290 ms: `rf_shellguard::win::workarea::hold_work_area` has the measurement). The
//! price, also measured: without the broadcast the system leaves maximized windows at their old size. So the host moves them: each
//! edge keeps its overhang (`rf_shellguard::tbmodel::refit_maximized`, pure and tested).
//!
//! This is the host's work and lives in the host's crate. The guard crate (`rf-shellguard`) changes one kind of window in one place
//! only, Explorer's taskbar, and its sources are scanned for that (`tests/tb_static.rs`): the fitting stood there for a day and
//! that scan failed in CI, rightly.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromRect, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, IsZoomed, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowPos,
};
use windows::core::BOOL;

struct Fit {
    from: [i32; 4],
    to: [i32; 4],
    monitor: HMONITOR,
    monitor_rect: [i32; 4],
    own: u32,
}

/// Asks every visible maximized window of the monitor that holds `to` (not this process's own) to take the place it has in the work
/// area `to`, given that the monitor's work area was `from`. Queued (a program that hangs does not hold the caller); a window of a
/// higher integrity level refuses and stays as it is. Call it on a per-monitor DPI aware thread: the rectangles are physical pixels.
pub fn maximized_windows(from: [i32; 4], to: [i32; 4]) {
    let [left, top, right, bottom] = to;
    let mut fit = Fit { from, to, monitor: HMONITOR::default(), monitor_rect: [0; 4], own: std::process::id() };
    // SAFETY: plain queries; `fit` outlives EnumWindows, which calls back on this thread.
    unsafe {
        fit.monitor = MonitorFromRect(&RECT { left, top, right, bottom }, MONITOR_DEFAULTTONULL);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if fit.monitor.is_invalid() || !GetMonitorInfoW(fit.monitor, &mut mi).as_bool() {
            return;
        }
        fit.monitor_rect = [mi.rcMonitor.left, mi.rcMonitor.top, mi.rcMonitor.right, mi.rcMonitor.bottom];
        let _ = EnumWindows(Some(each), LPARAM(&mut fit as *mut Fit as isize));
    }
}

unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the address of the `Fit` on the caller's stack, alive during EnumWindows; the calls read, and the one
    // SetWindowPos is queued to the window's own thread.
    unsafe {
        let fit = &*(lparam.0 as *const Fit);
        if !IsWindowVisible(hwnd).as_bool() || !IsZoomed(hwnd).as_bool() || MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL) != fit.monitor {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let mut r = RECT::default();
        if pid == fit.own || GetWindowRect(hwnd, &mut r).is_err() {
            return true.into();
        }
        if let Some([l, t, rr, b]) = rf_shellguard::tbmodel::refit_maximized([r.left, r.top, r.right, r.bottom], fit.monitor_rect, fit.from, fit.to) {
            let _ = SetWindowPos(HWND(hwnd.0 as *mut c_void), None, l, t, rr - l, b - t, SWP_ASYNCWINDOWPOS | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE);
        }
        true.into()
    }
}
