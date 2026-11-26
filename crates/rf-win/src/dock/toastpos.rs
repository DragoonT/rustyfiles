//! Windows' notification banners sit right above the dock, not a hand's width above it.
//!
//! WHY (the owner, with a screenshot of a WhatsApp banner: "bildirimler böyle yukarıda geliyor neden?"). Windows places its banner
//! (ShellExperienceHost's window of class `Windows.UI.Core.CoreWindow`, "New notification") above ITS taskbar, with a margin of its
//! own: measured on the owner's PC, the window's bottom edge stood 56 px above the work area's (the taskbar's place, which the dock
//! takes), so the banner floated 71 px over the dock. Windows has no setting for it.
//!
//! WHAT IS DONE. While the dock runs, a WinEvent hook hears a window of that class being shown or moved. A banner (no wider than
//! `MAX_W`, no taller than `MAX_H`: the notification centre and the calendar are bigger) whose bottom edge is a little above the
//! work area's bottom (between 1 and `MAX_LIFT` px) is moved down to it with `SetWindowPos`, without size, order or activation
//! changes. Its window carries a transparent shadow margin, so the banner then stands about 15 px above the dock, as on Windows' own
//! taskbar. Tried by hand on the owner's PC with a test banner: moved by 56 px, it stood just above the dock. Nothing is done while
//! the work area is the whole screen (the dock reserves no space: the banner would cover it).
//!
//! A BANNER THAT HANGS BELOW (the owner, 2026-11-26, with a picture of an Adobe Creative Cloud banner whose top stood 9 px above the
//! dock's band and whose rest was under the dock and off the screen: "bildirim alta gitmiş"). The banner's window opens as a sliver
//! and grows downward to its height. Moved while it was a sliver, its bottom was put on the work area's bottom, and what it grew
//! by afterwards hung below: the rule only ever moved a banner DOWN. Now a window lower than `MIN_H` is left alone (it is still
//! opening), and a banner whose bottom is below the work area's while its top is above it is moved UP to stand on it.

use std::sync::OnceLock;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW, GetClassNameW, GetMessageW, GetWindowRect, MSG, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    SetWindowPos, TranslateMessage, WINEVENT_OUTOFCONTEXT,
};

/// A banner is narrower and lower than this (physical px); the notification centre is not.
const MAX_W: i32 = 700;
const MAX_H: i32 = 900;
/// How far above the work area's bottom a banner may stand and still be moved (Windows' own margin was 56 px at 100 %).
const MAX_LIFT: i32 = 160;
/// A banner's window is at least this high (the lowest measured was 152 px); lower, it is still opening.
const MIN_H: i32 = 48;

/// How far a banner window at `win` is moved for a work area that ends at `work_bottom` on a screen that ends at `screen_bottom`:
/// down (more than 0) when it stands a little above the work area's bottom, up (less than 0) when it hangs below it; 0: it is left
/// alone (not a banner, still opening, already in place, or no reserved band to stand on).
pub fn lift_for(win: RECT, work_bottom: i32, screen_bottom: i32) -> i32 {
    let (w, h) = (win.right - win.left, win.bottom - win.top);
    if work_bottom >= screen_bottom || w <= 0 || h < MIN_H || w > MAX_W || h > MAX_H {
        return 0;
    }
    let gap = work_bottom - win.bottom;
    if (1..=MAX_LIFT).contains(&gap) || (gap < 0 && win.top < work_bottom) { gap } else { 0 }
}

fn is_core_window(hwnd: HWND) -> bool {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n]) == "Windows.UI.Core.CoreWindow"
}

/// Whether the window belongs to ShellExperienceHost (the banners' host; the class alone is shared by every Store app).
fn is_shell_experience(hwnd: HWND) -> bool {
    let mut pid = 0u32;
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    static KNOWN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if pid != 0 && KNOWN.load(std::sync::atomic::Ordering::Relaxed) == pid {
        return true;
    }
    let yes = crate::tasklist::process_exe_path(pid).and_then(|p| p.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case("ShellExperienceHost.exe"))).unwrap_or(false);
    if yes {
        KNOWN.store(pid, std::sync::atomic::Ordering::Relaxed);
    }
    yes
}

fn place(hwnd: HWND) {
    if !is_core_window(hwnd) || !is_shell_experience(hwnd) {
        return;
    }
    let mut win = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut win) }.is_err() {
        return;
    }
    let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !unsafe { GetMonitorInfoW(mon, &mut info) }.as_bool() {
        return;
    }
    let dy = lift_for(win, info.rcWork.bottom, info.rcMonitor.bottom);
    if dy != 0 {
        // (its own move comes back as a location change, which then finds it in place)
        let _ = unsafe { SetWindowPos(hwnd, None, win.left, win.top + dy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
        log::debug!("dock: Windows' notification banner moved {} px {}, to stand above the dock", dy.abs(), if dy > 0 { "down" } else { "up" });
    }
}

unsafe extern "system" fn on_event(_h: HWINEVENTHOOK, _event: u32, hwnd: HWND, id_object: i32, id_child: i32, _thread: u32, _time: u32) {
    // whole windows only (the caret and the pointer move all the time)
    if id_object == 0 && id_child == 0 && !hwnd.is_invalid() {
        place(hwnd);
    }
}

/// Starts keeping Windows' banners above the dock, for the life of the process (once; later calls do nothing).
pub fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name("rf-dock-toasts".into()).spawn(|| unsafe {
        let a = SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT);
        let b = SetWinEventHook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT);
        if a.is_invalid() && b.is_invalid() {
            log::warn!("dock: cannot watch Windows' notification banners");
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
    if let Err(e) = spawned {
        log::warn!("dock: cannot watch Windows' notification banners: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(l: i32, t: i32, rr: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: rr, bottom: b }
    }

    #[test]
    fn a_banner_above_the_reserved_band_comes_down_to_it() {
        // measured on the owner's PC: the banner's window 2164,1184 - 2560,1336, the work area ending at 1392 on a 1440 screen
        assert_eq!(lift_for(r(2164, 1184, 2560, 1336), 1392, 1440), 56);
        assert_eq!(lift_for(r(2164, 1240, 2560, 1392), 1392, 1440), 0, "already in place: its own move is not moved again");
        assert_eq!(lift_for(r(2164, 1184, 2560, 1336), 1440, 1440), 0, "no reserved band: the banner would cover the dock");
        assert_eq!(lift_for(r(1900, 300, 2560, 1380), 1392, 1440), 0, "the notification centre is no banner");
        assert_eq!(lift_for(r(2164, 600, 2560, 900), 1392, 1440), 0, "far above: not a banner Windows put there");
    }

    /// The owner's picture, 2026-11-26: the banner's top 9 px above the dock's band, the rest under the dock and off the screen.
    #[test]
    fn a_banner_that_hangs_below_the_work_area_comes_up_to_it() {
        // its window while it opens: a sliver, which is not moved (moving it is what left the grown banner hanging)
        assert_eq!(lift_for(r(2164, 1327, 2560, 1336), 1392, 1440), 0, "9 px high: still opening");
        // grown downward from where the sliver had been put: 250 px from 1383
        assert_eq!(lift_for(r(2164, 1383, 2560, 1633), 1392, 1440), -241, "back up, to stand on the work area's bottom");
        assert_eq!(lift_for(r(2164, 1142, 2560, 1392), 1392, 1440), 0, "and then it is in place");
        assert_eq!(lift_for(r(2164, 1300, 2560, 1420), 1392, 1440), -28, "hanging into the dock's band only");
        assert_eq!(lift_for(r(2164, 1400, 2560, 1500), 1392, 1440), 0, "wholly in the band and below: nothing Windows shows as a banner");
        assert_eq!(lift_for(r(2164, 1383, 2560, 1633), 1440, 1440), 0, "no reserved band: left alone, as before");
    }
}
