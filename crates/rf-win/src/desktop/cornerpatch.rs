//! The black pixels under the desktop's rounded bottom corners, in the pixel row a cover window leaves out.
//!
//! WHY A WINDOW OF ITS OWN: a cover window is its monitor less the bottom pixel row (`rf_core::desktop::cover::geometry`), so that
//! row is not ours to draw in, and Explorer's desktop shows there. With rounded corners (`rf_core::desktop::corners`) that is a short
//! line of wallpaper under each black bottom corner. The cover window cannot grow: reaching every edge of the monitor is what makes
//! a window a full-screen program to the shell and to other programs.
//!
//! WHAT A PATCH IS: a black pop-up one pixel row high and a few pixels wide, OWNED by its cover window. Windows keeps an owned window
//! above its owner wherever the owner goes in the z-order, hides it when the owner is minimized and destroys it with the owner, so a
//! patch needs no z-order rule and no clean-up of its own. It is transparent to the mouse (a press goes to what is under it), is
//! never activated, has no taskbar button and stays through Aero Peek, as the cover does.
//!
//! WHERE IT RUNS: on the thread that owns the cover window (the UI thread), whose message loop serves these windows too.
//! NOTHING OF EXPLORER'S IS TOUCHED: a patch lies over a few pixels of its desktop, nothing more.

use std::sync::{Mutex, MutexGuard, Once};

use rf_core::desktop::geom::Rect;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_EXCLUDED_FROM_PEEK, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GW_HWNDPREV, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongPtrW, HWND_TOP, IsWindowVisible, LWA_ALPHA, RegisterClassExW, SW_SHOWNOACTIVATE,
    SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowPos, ShowWindow, WINDOWPOS, WM_NCDESTROY, WM_SHOWWINDOW,
    WM_WINDOWPOSCHANGING, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("rustyfiles.CornerPatch");

#[derive(Clone, Copy)]
struct Patch {
    owner: isize,
    hwnd: isize,
    rect: Rect,
}

/// The patches that exist. A patch leaves the list when its window goes (`WM_NCDESTROY`), whoever destroyed it.
static PATCHES: Mutex<Vec<Patch>> = Mutex::new(Vec::new());

// (never held across a call into Windows: destroying a patch comes back into the procedure, which takes it too)
fn list() -> MutexGuard<'static, Vec<Patch>> {
    PATCHES.lock().unwrap_or_else(|p| p.into_inner())
}

fn hwnd_of(h: isize) -> HWND {
    HWND(h as *mut _)
}

/// Whether `hwnd` is a patch. The covers' z-order check leaves patches out: one stands between two cover windows, which would
/// otherwise read as "something came between the covers and the desktop".
pub fn is_patch(hwnd: isize) -> bool {
    list().iter().any(|p| p.hwnd == hwnd)
}

/// The rectangles of an owner's patches, as they were asked for (for the log and the live check).
pub fn of(owner: isize) -> Vec<Rect> {
    list().iter().filter(|p| p.owner == owner).map(|p| p.rect).collect()
}

unsafe extern "system" fn patch_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let key = hwnd.0 as isize;
    match msg {
        WM_NCDESTROY => list().retain(|p| p.hwnd != key),
        // Who shows and hides a patch is worth a line (it happens when it is made, and when its owner is minimized and comes back):
        // `cause` 0 is a ShowWindow call, 1 the owner being minimized, 3 the owner coming back.
        WM_SHOWWINDOW => log::info!("corner patch: {key:#x} {} (cause {})", if wp.0 != 0 { "is shown" } else { "is hidden" }, lp.0),
        WM_WINDOWPOSCHANGING => {
            let pos = lp.0 as *const WINDOWPOS;
            // SAFETY: for this message `lParam` points at the WINDOWPOS the system is about to apply
            if let Some(pos) = unsafe { pos.as_ref() }
                && pos.flags.0 & SWP_HIDEWINDOW.0 != 0
            {
                log::info!("corner patch: {key:#x} is being hidden by a position change (flags {:#x})", pos.flags.0);
            }
        }
        _ => {}
    }
    // SAFETY: the default procedure, with the message as it arrived
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Whether what is there is what is wanted. Pure.
pub fn unchanged(have: &[Rect], want: &[Rect]) -> bool {
    have == want
}

/// The owner's patches become exactly `rects` (virtual-screen device px). An owner that is not a visible window of this process gets
/// none: a patch shown before its owner would stand above every program. Must run on the thread that owns `owner`. Cheap when
/// nothing changed.
pub fn set(owner: isize, rects: &[Rect]) {
    let old: Vec<Patch> = list().iter().filter(|p| p.owner == owner).copied().collect();
    let shown = crate::window::exists(owner) && super::cover::is_ours(owner) && unsafe { IsWindowVisible(hwnd_of(owner)) }.as_bool();
    let want: &[Rect] = if shown { rects } else { &[] };
    if unchanged(&old.iter().map(|p| p.rect).collect::<Vec<_>>(), want) {
        return;
    }
    for p in &old {
        // (the procedure takes it off the list)
        let _ = unsafe { DestroyWindow(hwnd_of(p.hwnd)) };
    }
    for r in want {
        if let Some(hwnd) = create(owner, *r) {
            list().push(Patch { owner, hwnd, rect: *r });
        }
    }
    log::debug!("corner patch: {owner:#x} has {} patch(es)", of(owner).len());
}

fn create(owner: isize, r: Rect) -> Option<isize> {
    static CLASS_ONCE: Once = Once::new();
    // SAFETY: standard window-class registration and window creation, on the thread that owns `owner` and runs a message loop
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        CLASS_ONCE.call_once(|| {
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(patch_proc),
                hInstance: hinst.into(),
                lpszClassName: CLASS,
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                ..Default::default()
            };
            RegisterClassExW(&wc);
        });
        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let h = match CreateWindowExW(ex, CLASS, w!(""), WS_POPUP, r.left, r.top, r.width().max(1), r.height().max(1), Some(hwnd_of(owner)), None, Some(hinst.into()), None) {
            Ok(h) => h,
            Err(e) => {
                log::warn!("corner patch: a patch for {owner:#x} could not be made: {e}");
                return None;
            }
        };
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), 255, LWA_ALPHA);
        let on: u32 = 1;
        let _ = DwmSetWindowAttribute(h, DWMWA_EXCLUDED_FROM_PEEK, &on as *const u32 as *const _, size_of::<u32>() as u32);
        // Shown directly above its owner: under the window that stands above the owner now. (Behind a topmost window would make the
        // patch topmost; a topmost window above the owner means the owner is the first ordinary window, and the top of the ordinary
        // windows is then the place.)
        let above = GetWindow(hwnd_of(owner), GW_HWNDPREV).ok().filter(|p| !p.0.is_null());
        let after = match above {
            Some(p) if GetWindowLongPtrW(p, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 == 0 => p,
            _ => HWND_TOP,
        };
        let key = h.0 as isize;
        // TWO STEPS: the place first, the show afterwards. Asked for in one call (the place together with SWP_SHOWWINDOW) a patch
        // came back placed and NOT shown, with no error (seen live, 2026-11-13: both patches of the main monitor on one start, one
        // of them on the next); shown on its own it is on the screen every time.
        if let Err(e) = SetWindowPos(h, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER) {
            let mut name = [0u16; 64];
            let n = GetClassNameW(after, &mut name).max(0) as usize;
            log::warn!("corner patch: {key:#x} could not be put under {:#x} ({}): {e}", after.0 as isize, String::from_utf16_lossy(&name[..n]));
        }
        let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW);
        if !IsWindowVisible(h).as_bool() {
            // (a patch that is not on the screen is the line of wallpaper it is for)
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            log::warn!("corner patch: {key:#x} was not shown by its placement; asked again: {}", IsWindowVisible(h).as_bool());
        }
        Some(key)
    }
}

#[cfg(test)]
mod tests {
    use super::{Rect, is_patch, of, unchanged};

    #[test]
    fn the_same_rectangles_in_the_same_order_are_left_alone() {
        let (a, b) = (Rect::new(0, 1439, 9, 1440), Rect::new(2551, 1439, 2560, 1440));
        assert!(unchanged(&[], &[]));
        assert!(unchanged(&[a, b], &[a, b]));
        assert!(!unchanged(&[a, b], &[a]), "a corner went (the radius is 0 now)");
        assert!(!unchanged(&[], &[a, b]), "the first time");
        assert!(!unchanged(&[a, b], &[a, Rect::new(2546, 1439, 2560, 1440)]), "another radius");
    }

    #[test]
    fn a_window_that_was_never_patched_is_no_patch_and_has_none() {
        assert!(!is_patch(0));
        assert!(!is_patch(0x1234));
        assert!(of(0x1234).is_empty());
    }
}
