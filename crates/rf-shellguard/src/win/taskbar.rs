//! Shows the Windows taskbar again (beside mode, plan 6.6; also part of the fall-back).
//! NOT RUN by the author of this crate (it touches Explorer's windows).
//!
//! It only brings back a taskbar that was hidden by making it invisible. A host that hides the taskbar some other way
//! (moving or resizing it) has to record how in the state file and this file has to learn it then (plan E1).
//!
//! //! It finds the windows by class name every time instead of trusting stored handles: a handle that was
//! saved before Explorer restarted may belong to some other window by now.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, IsWindowVisible, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos};
use windows::core::BOOL;

use crate::sys::TaskbarVisibility;

pub struct RealTaskbar;

struct Found {
    shown: usize,
}

unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut buf = [0u16; 64];
    // SAFETY: plain queries into a local buffer; lparam is the address of the `Found` on the caller's stack.
    unsafe {
        let n = GetClassNameW(hwnd, &mut buf);
        if n > 0 {
            let class = String::from_utf16_lossy(&buf[..n as usize]);
            if class == "Shell_TrayWnd" || class == "Shell_SecondaryTrayWnd" {
                if !IsWindowVisible(hwnd).as_bool() {
                    // Asynchronous: a plain SetWindowPos on another process's window waits for that process's thread to
                    // answer, and Explorer may be hung or restarting at the very moment the taskbar has to come back.
                    // Queued, the call returns at once and the request is carried out when Explorer's thread is free.
                    let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                }
                (*(lparam.0 as *mut Found)).shown += 1;
            }
        }
    }
    BOOL(1)
}

impl TaskbarVisibility for RealTaskbar {
    fn show_windows_taskbar(&mut self) -> Result<usize, String> {
        let mut found = Found { shown: 0 };
        // SAFETY: `found` outlives the call.
        unsafe { EnumWindows(Some(cb), LPARAM(&mut found as *mut Found as isize)) }.map_err(|e| e.to_string())?;
        Ok(found.shown)
    }
}
