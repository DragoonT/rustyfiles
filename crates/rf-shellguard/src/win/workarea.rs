//! Puts saved work areas back (a crashed host may have changed them: the dock's band, or the strip of a hidden Windows taskbar that
//! was given back to programs) and sets one for the host (`set_work_area`, the only place that does).
//! NOT RUN by the author of this crate (`SPI_SETWORKAREA` changes the running session's work area).
//!
//! The work area is set WITHOUT `SPIF_UPDATEINIFILE`: it is the running session's rectangle, not a stored setting, and Explorer
//! computes it afresh whenever a toolbar changes, the monitors change or it restarts.

use std::ffi::c_void;

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{SPI_SETWORKAREA, SPIF_SENDCHANGE, SystemParametersInfoW};

use crate::statefile::WorkAreaRec;
use crate::sys::WorkAreas;

/// Sets the work area of the monitor that holds `rect` (left, top, right, bottom in virtual-screen pixels).
pub fn set_work_area(rect: [i32; 4]) -> Result<(), String> {
    let [left, top, right, bottom] = rect;
    if right <= left || bottom <= top {
        return Err("an empty rectangle".into());
    }
    let mut r = RECT { left, top, right, bottom };
    // SAFETY: `r` is a valid RECT for the call. Not persisted (no SPIF_UPDATEINIFILE). The rectangles in the state file were
    // measured by the host on a per-monitor-aware thread, so they are physical pixels; this program has no DPI manifest, and a
    // DPI-unaware caller would have them scaled as if they were virtualised. The thread is made aware for the one call, then
    // put back as it was.
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let result = SystemParametersInfoW(SPI_SETWORKAREA, 0, Some(&mut r as *mut RECT as *mut c_void), SPIF_SENDCHANGE);
        if !previous.is_invalid() {
            SetThreadDpiAwarenessContext(previous);
        }
        result
    }
    .map_err(|e| e.to_string())
}

pub struct RealWorkAreas;

impl WorkAreas for RealWorkAreas {
    fn restore(&mut self, saved: &[WorkAreaRec]) -> Result<(), String> {
        let mut failed = 0;
        for rec in saved {
            let [left, top, right, bottom] = rec.rect;
            if right <= left || bottom <= top {
                continue;
            }
            if set_work_area(rec.rect).is_err() {
                failed += 1;
            }
        }
        if failed == 0 { Ok(()) } else { Err(format!("{failed} work area(s) could not be restored")) }
    }
}
