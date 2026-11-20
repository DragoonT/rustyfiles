//! Read-only Windows system parameters the UI follows.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SPI_GETSNAPTODEFBUTTON, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetCursorPos, SystemParametersInfoW};

/// Windows' "Animation effects" (Settings → Accessibility → Visual effects). True when it can't
/// be read (the Windows default).
pub fn client_area_animation() -> bool {
    // a Win32 BOOL
    let mut on: i32 = 1;
    unsafe {
        if SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut i32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).is_err() {
            return true;
        }
    }
    on != 0
}

/// Windows' "Snap To" (Mouse settings → Pointer Options: "Automatically move pointer to the default button in a dialog box"). False
/// when it can't be read (the Windows default).
pub fn snap_to_default_button() -> bool {
    let mut on: i32 = 0;
    unsafe {
        if SystemParametersInfoW(SPI_GETSNAPTODEFBUTTON, 0, Some(&mut on as *mut i32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).is_err() {
            return false;
        }
    }
    on != 0
}

/// Puts the pointer on a point of a window's client area (device px), as Windows' own dialogs do for "Snap To" (issue 148).
pub fn put_pointer_in(hwnd: isize, x: i32, y: i32) -> bool {
    let mut p = POINT { x, y };
    unsafe { ClientToScreen(HWND(hwnd as *mut _), &mut p).as_bool() && SetCursorPos(p.x, p.y).is_ok() }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_animation_effects() {
        // whatever the machine says, the call must not fail or panic
        let _ = super::client_area_animation();
        let _ = super::snap_to_default_button();
    }
}
