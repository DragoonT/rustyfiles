//! Puts saved work areas back (a crashed host may have changed them: the dock's band, or the strip of a hidden Windows taskbar that
//! was given back to programs) and sets one for the host (`set_work_area`, the only place that does).
//! NOT RUN by the author of this crate (`SPI_SETWORKAREA` changes the running session's work area).
//!
//! The work area is set WITHOUT `SPIF_UPDATEINIFILE`: it is the running session's rectangle, not a stored setting, and Explorer
//! computes it afresh whenever a toolbar changes, the monitors change or it restarts.
//!
//! The saved rectangles are checked against the monitors and taskbars as they are when they are put back (`tbwork::work_restore`):
//! the screen may have changed since they were written down, and a rectangle of the old screen applied blindly lands on whatever
//! monitor is nearest. Reading them is read-only.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONULL, MONITORINFO, MONITORINFOEXW, MonitorFromRect, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, IsZoomed, SPI_SETWORKAREA, SPIF_SENDCHANGE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOOWNERZORDER, SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetWindowPos, SystemParametersInfoW,
};
use windows::core::BOOL;

use crate::statefile::WorkAreaRec;
use crate::sys::WorkAreas;
use crate::tbmodel::MonitorGeom;
use crate::tbwork;

/// Runs `f` as a per-monitor-aware caller, then puts the thread's awareness back as it was. The rectangles in the state file were
/// measured by the host on a per-monitor-aware thread, so they are physical pixels; this program has no DPI manifest, and a
/// DPI-unaware caller would have them scaled as if they were virtualised (and would read the monitors the same way).
fn aware<R>(f: impl FnOnce() -> R) -> R {
    // SAFETY: only the calling thread's awareness is changed, for the length of `f`, and put back.
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let result = f();
        if !previous.is_invalid() {
            SetThreadDpiAwarenessContext(previous);
        }
        result
    }
}

/// Sets the work area of the monitor that holds `rect` (left, top, right, bottom in virtual-screen pixels).
pub fn set_work_area(rect: [i32; 4]) -> Result<(), String> {
    let [left, top, right, bottom] = rect;
    if right <= left || bottom <= top {
        return Err("an empty rectangle".into());
    }
    let mut r = RECT { left, top, right, bottom };
    // SAFETY: `r` is a valid RECT for the call. Not persisted (no SPIF_UPDATEINIFILE).
    aware(|| unsafe { SystemParametersInfoW(SPI_SETWORKAREA, 0, Some(&mut r as *mut RECT as *mut c_void), SPIF_SENDCHANGE) }).map_err(|e| e.to_string())
}

/// Holds the work area of a monitor for a host that keeps Explorer's taskbar hidden (`from`: what the monitor has now), and fits
/// that monitor's maximized windows to it. Returns after the windows were asked (queued: a program that hangs does not hold us).
///
/// WITHOUT the change broadcast, unlike [`set_work_area`]. Measured on the user's PC (Windows 11 build 26300, two monitors, both
/// taskbars hidden): set with `SPIF_SENDCHANGE`, both monitors were back at Explorer's own rectangles 290 ms later, every time (it
/// answers the broadcast by working its taskbars' strips out again, hidden or not); set without it, both rectangles were still
/// ours a minute later. The price, also measured: without the broadcast the maximized windows stay at their old size, so they are
/// moved here (`tbmodel::refit_maximized`). Programs that remember the work area learn of it the next time they ask.
pub fn hold_work_area(from: [i32; 4], to: [i32; 4]) -> Result<(), String> {
    let [left, top, right, bottom] = to;
    if right <= left || bottom <= top {
        return Err("an empty rectangle".into());
    }
    let mut r = RECT { left, top, right, bottom };
    aware(|| {
        // SAFETY: `r` is a valid RECT for the call. Not persisted and not broadcast (no flags).
        unsafe { SystemParametersInfoW(SPI_SETWORKAREA, 0, Some(&mut r as *mut RECT as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) }.map_err(|e| e.to_string())?;
        let mut fit = Refit { from, to, monitor: HMONITOR::default(), monitor_rect: [0; 4], own: std::process::id() };
        // SAFETY: plain queries; `fit` outlives EnumWindows, which calls back on this thread.
        unsafe {
            fit.monitor = MonitorFromRect(&RECT { left, top, right, bottom }, MONITOR_DEFAULTTONULL);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if fit.monitor.is_invalid() || !GetMonitorInfoW(fit.monitor, &mut mi).as_bool() {
                return Ok(());
            }
            fit.monitor_rect = [mi.rcMonitor.left, mi.rcMonitor.top, mi.rcMonitor.right, mi.rcMonitor.bottom];
            let _ = EnumWindows(Some(refit_cb), LPARAM(&mut fit as *mut Refit as isize));
        }
        Ok(())
    })
}

struct Refit {
    from: [i32; 4],
    to: [i32; 4],
    monitor: HMONITOR,
    monitor_rect: [i32; 4],
    own: u32,
}

unsafe extern "system" fn refit_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the address of the `Refit` on the caller's stack, alive during EnumWindows; the calls read, and the one
    // SetWindowPos is queued to the window's own thread. A window of a higher integrity level refuses it; that is left so.
    unsafe {
        let fit = &*(lparam.0 as *const Refit);
        if !IsWindowVisible(hwnd).as_bool() || !IsZoomed(hwnd).as_bool() || MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL) != fit.monitor {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let mut r = RECT::default();
        if pid == fit.own || GetWindowRect(hwnd, &mut r).is_err() {
            return true.into();
        }
        if let Some([l, t, rr, b]) = crate::tbmodel::refit_maximized([r.left, r.top, r.right, r.bottom], fit.monitor_rect, fit.from, fit.to) {
            let _ = SetWindowPos(hwnd, None, l, t, rr - l, b - t, SWP_ASYNCWINDOWPOS | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE);
        }
        true.into()
    }
}

unsafe extern "system" fn monitor_cb(hmon: HMONITOR, _dc: HDC, _r: *mut RECT, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the address of the Vec on the caller's stack, alive during EnumDisplayMonitors.
    unsafe {
        let list = &mut *(lparam.0 as *mut Vec<MonitorGeom>);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut mi.monitorInfo as *mut _ as *mut _).as_bool() {
            let name = String::from_utf16_lossy(&mi.szDevice).trim_end_matches('\0').to_string();
            let (r, w) = (mi.monitorInfo.rcMonitor, mi.monitorInfo.rcWork);
            // MONITORINFOF_PRIMARY = 1
            list.push(MonitorGeom { id: name, rect: [r.left, r.top, r.right, r.bottom], work: [w.left, w.top, w.right, w.bottom], primary: mi.monitorInfo.dwFlags & 1 != 0 });
        }
    }
    BOOL(1)
}

/// The monitors as they are now, in physical pixels. Read-only; empty when they cannot be listed.
fn monitors_now() -> Vec<MonitorGeom> {
    let mut v: Vec<MonitorGeom> = Vec::new();
    // SAFETY: `v` outlives the call.
    aware(|| unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(monitor_cb), LPARAM(&mut v as *mut Vec<MonitorGeom> as isize));
    });
    v
}

/// The rectangles of Explorer's taskbars that are shown now, in physical pixels. Read-only.
fn shown_taskbars() -> Vec<[i32; 4]> {
    aware(crate::win::tbwin::list_taskbar_windows).map(|l| l.iter().filter(|w| w.explorer && w.visible).map(|w| w.rect).collect()).unwrap_or_default()
}

pub struct RealWorkAreas;

impl WorkAreas for RealWorkAreas {
    fn restore(&mut self, saved: &[WorkAreaRec]) -> Result<(), String> {
        // (the host is gone or leaving; the taskbar has just been shown again, so its rectangle is the one Explorer reserves)
        let back = tbwork::work_restore(saved, &monitors_now(), &shown_taskbars());
        let failed = back.put.iter().filter(|rect| set_work_area(**rect).is_err()).count();
        if failed == 0 { Ok(()) } else { Err(format!("{failed} work area(s) could not be restored")) }
    }
}
