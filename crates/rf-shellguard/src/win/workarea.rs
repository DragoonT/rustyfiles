//! Puts saved work areas back (a crashed host may have shrunk them for its dock). Until the AppBar
//! server and the work-area manager exist (plan H2) the state file holds no rectangles and this does nothing.
//! NOT RUN by the author of this crate (`SPI_SETWORKAREA` changes a Windows setting).

use std::ffi::c_void;

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{SPI_SETWORKAREA, SPIF_SENDCHANGE, SystemParametersInfoW};

use crate::statefile::WorkAreaRec;
use crate::sys::WorkAreas;

pub struct RealWorkAreas;

impl WorkAreas for RealWorkAreas {
    fn restore(&mut self, saved: &[WorkAreaRec]) -> Result<(), String> {
        let mut failed = 0;
        for rec in saved {
            let [left, top, right, bottom] = rec.rect;
            if right <= left || bottom <= top {
                continue;
            }
            let mut rect = RECT { left, top, right, bottom };
            // SAFETY: `rect` is a valid RECT for the call. Not persisted (no SPIF_UPDATEINIFILE): it is the running session's work area.
            let r = unsafe { SystemParametersInfoW(SPI_SETWORKAREA, 0, Some(&mut rect as *mut RECT as *mut c_void), SPIF_SENDCHANGE) };
            if r.is_err() {
                failed += 1;
            }
        }
        if failed == 0 { Ok(()) } else { Err(format!("{failed} work area(s) could not be restored")) }
    }
}
