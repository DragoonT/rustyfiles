//! Small top-level window helpers that GPUI does not expose.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    WS_EX_TOPMOST,
};

/// Keeps the window above all non-topmost windows ("compact overlay" / always on top).
pub fn set_topmost(hwnd: isize, on: bool) -> windows::core::Result<()> {
    let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
    unsafe { SetWindowPos(HWND(hwnd as *mut _), Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }
}

pub fn is_topmost(hwnd: isize) -> bool {
    let ex = unsafe { GetWindowLongPtrW(HWND(hwnd as *mut _), GWL_EXSTYLE) };
    ex as u32 & WS_EX_TOPMOST.0 != 0
}

/// Whether the foreground window belongs to this process.
pub fn app_in_front() -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        pid == GetCurrentProcessId()
    }
}

/// Window rectangle in physical pixels `[left, top, right, bottom]`, and whether it is maximized.
pub fn rect(hwnd: isize) -> Option<([i32; 4], bool)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsZoomed};
    let h = HWND(hwnd as *mut _);
    let mut r = RECT::default();
    unsafe { GetWindowRect(h, &mut r).ok()? };
    Some(([r.left, r.top, r.right, r.bottom], unsafe { IsZoomed(h).as_bool() }))
}

/// Restores (un-maximizes) and moves/resizes the window, physical pixels.
pub fn place(hwnd: isize, r: [i32; 4]) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_RESTORE, SWP_NOZORDER, ShowWindow};
    let h = HWND(hwnd as *mut _);
    unsafe {
        let _ = ShowWindow(h, SW_RESTORE);
        let _ = SetWindowPos(h, None, r[0], r[1], r[2] - r[0], r[3] - r[1], SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

pub fn maximize(hwnd: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_MAXIMIZE, ShowWindow};
    unsafe {
        let _ = ShowWindow(HWND(hwnd as *mut _), SW_MAXIMIZE);
    }
}

/// Overrides the minimum window size (logical pixels) at runtime, `None` = back to the
/// size given at creation. Read by our GPUI fork on WM_GETMINMAXINFO.
pub fn set_min_size(hwnd: isize, size: Option<(u16, u16)>) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::UI::WindowsAndMessaging::{RemovePropW, SetPropW};
    let h = HWND(hwnd as *mut _);
    let name = windows::core::w!("rustyfiles.MinSize");
    unsafe {
        match size {
            Some((w, hh)) => {
                let v = ((w as usize) << 16) | hh as usize;
                let _ = SetPropW(h, name, Some(HANDLE(v as *mut _)));
            }
            None => {
                let _ = RemovePropW(h, name);
            }
        }
    }
}

/// Dark or light window frame / system backdrop (Mica follows it), independent of the system
/// setting — so the app's own Light / Dark choice also tints Mica.
/// The process's AppUserModelID: windows group under, and pins match, the shortcuts carrying
/// the same id (not the exe path, which changes folder on every update).
pub fn set_app_id(id: &str) {
    let w = crate::wide::wide(id);
    if let Err(e) = unsafe { windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(windows::core::PCWSTR(w.as_ptr())) } {
        log::warn!("app id: {e}");
    }
}

pub fn set_dark_mode(hwnd: isize, dark: bool) {
    use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    let v: i32 = dark as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(HWND(hwnd as *mut _), DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const _ as _, std::mem::size_of::<i32>() as u32);
    }
}

/// Makes `owner` (a window of another process, possibly) the owner of `hwnd`: it stays above
/// it and minimizes with it, like the owner's own dialog.
pub fn set_owner(hwnd: isize, owner: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GWLP_HWNDPARENT, SetWindowLongPtrW};
    unsafe {
        SetWindowLongPtrW(HWND(hwnd as *mut _), GWLP_HWNDPARENT, owner);
    }
}

/// Gives the foreground back to `owner` (another app's window) while we still hold it: when our
/// window closes, Windows would otherwise activate whatever window comes next.
pub fn hand_foreground(owner: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow};
    unsafe {
        let h = HWND(owner as *mut _);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid != 0 {
            let _ = AllowSetForegroundWindow(pid);
        }
        let _ = SetForegroundWindow(h);
    }
}

/// Brings `hwnd` to the front and gives it the keyboard (the caller was allowed to by the app
/// in front, see AllowSetForegroundWindow).
///
/// When that permission didn't come (Chrome asks from a background helper process, which
/// can't grant it), the window would open behind the app: then our thread shares the input
/// state of the window in front for a moment, which lets the switch through.
pub fn bring_to_front(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SW_SHOW, SetForegroundWindow, ShowWindow};
    unsafe {
        let h = HWND(hwnd as *mut _);
        let _ = ShowWindow(h, SW_SHOW);
        if SetForegroundWindow(h).as_bool() && GetForegroundWindow() == h {
            return;
        }
        let front = GetForegroundWindow();
        let theirs = GetWindowThreadProcessId(front, None);
        let mine = GetCurrentThreadId();
        let attached = theirs != 0 && theirs != mine && AttachThreadInput(mine, theirs, true).as_bool();
        let _ = BringWindowToTop(h);
        let _ = SetForegroundWindow(h);
        if attached {
            let _ = AttachThreadInput(mine, theirs, false);
        }
    }
}
