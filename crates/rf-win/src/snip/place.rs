//! The thin Win32 layer of the snip overlay: turns a toolkit window into a borderless topmost cover of one monitor and keeps it there.
//! Modelled on the desktop cover (`desktop/cover.rs`), whose lessons are kept: the toolkit's pop-up keeps an invisible resize frame that
//! makes its outer rectangle larger than its client area, and the toolkit sets a window's size again at moments of its own (after a
//! menu closed, on a change of scale), so the window's procedure holds the rectangle it was placed at.
//!
//! What it changes: only windows of this process (every function asks `is_ours` first). What it does not do: it never activates a
//! window, and it never touches a window of Explorer or of any other program.
//!
//! NOTHING OF THE SNIP TAKES THE FOREGROUND (`WS_EX_NOACTIVATE` on the overlays, the toolbar and the notice). Two reasons: a window that
//! is clicked is raised above the other topmost windows, which would put an overlay over its own toolbar; and the program the user
//! was in keeps the keyboard, so cancelling and finishing give it back with nothing to restore. Escape is therefore not read from a
//! focused window: `escape.rs` takes that one key with a hook while the overlay is up (so that it does not also reach the program
//! underneath), and [`escape_down`] is the fallback that only looks at the keyboard's state when the hook could not be installed.
//!
//! THE WINDOW'S OWN ANSWER TO A CLICK. The toolkit (the pinned gpui-pre-windows fork) answers `WM_MOUSEACTIVATE` with `MA_ACTIVATE` for
//! every window, on purpose. An explicit answer like that is not something `WS_EX_NOACTIVATE` is known to override, so the style alone
//! is not trusted: the procedure put in front of every window of the snip (the overlays, the toolbar and the notice) answers
//! `MA_NOACTIVATE` itself ([`no_activate_reply`]). The click is still delivered; it just does not change the foreground window or raise
//! the window inside the topmost band over its own toolbar. The dock's islands and the desktop cover do the same for the same reason.
//!
//! COMPILED, UNIT-TESTED FOR WHAT IS PURE, NEVER RUN against a real window in this increment: the live test is the lead's.

use std::sync::Mutex;

use rf_core::desktop::geom::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_EXCLUDED_FROM_PEEK, DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DwmSetWindowAttribute,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GCL_STYLE, GWL_EXSTYLE, GWL_STYLE, GWLP_WNDPROC, GetClassLongPtrW, GetWindowLongPtrW, HWND_TOPMOST, SET_WINDOW_POS_FLAGS, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetClassLongPtrW, SetWindowLongPtrW, SetWindowPos, WINDOWPOS, WM_MOUSEACTIVATE,
    WM_NCDESTROY, WM_WINDOWPOSCHANGING, WNDPROC, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

/// `CS_DROPSHADOW`: the window class asks for a shadow around its windows (the toolkit's class may).
const CS_DROPSHADOW: usize = 0x0002_0000;

fn hwnd_of(h: isize) -> HWND {
    HWND(h as *mut _)
}

/// One overlay window the procedure knows.
struct Held {
    hwnd: isize,
    /// The procedure the toolkit had.
    prev: isize,
    /// The outer rectangle the window is to have, once placed.
    want: Option<Rect>,
}

static HELD: Mutex<Vec<Held>> = Mutex::new(Vec::new());

fn held() -> std::sync::MutexGuard<'static, Vec<Held>> {
    HELD.lock().unwrap_or_else(|p| p.into_inner())
}

/// Whether the window belongs to this process.
pub fn is_ours(hwnd: isize) -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd_of(hwnd), Some(&mut pid)) };
    pid != 0 && pid == unsafe { GetCurrentProcessId() }
}

/// What a size or position about to be applied becomes: nothing changes when the window has no place yet or the change does not touch
/// the rectangle; otherwise the rectangle is the window's own place, whatever was asked.
pub fn guarded_rect(want: Option<Rect>, moves: bool, sizes: bool, asked: Rect) -> Option<Rect> {
    let want = want?;
    ((moves || sizes) && asked != want).then_some(want)
}

/// `MA_NOACTIVATE`: the reply to `WM_MOUSEACTIVATE` that keeps a click from activating the window but still delivers it.
const MA_NOACTIVATE: isize = 3;

/// The reply a window of the snip gives to a message itself, before the toolkit's procedure sees it. The toolkit answers
/// `WM_MOUSEACTIVATE` with `MA_ACTIVATE`; this is the one message whose answer is taken out of its hands.
pub fn no_activate_reply(msg: u32) -> Option<isize> {
    (msg == WM_MOUSEACTIVATE).then_some(MA_NOACTIVATE)
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let key = hwnd.0 as isize;
    // (the lock is held for a lookup only: a call out of here can come back in, for the same window)
    let prev = held().iter().find(|h| h.hwnd == key).map(|h| h.prev);
    let Some(prev) = prev else { return unsafe { DefWindowProcW(hwnd, msg, wp, lp) } };
    if let Some(reply) = no_activate_reply(msg) {
        return LRESULT(reply);
    }
    match msg {
        WM_WINDOWPOSCHANGING => {
            let pos = lp.0 as *mut WINDOWPOS;
            if !pos.is_null() {
                // SAFETY: for this message `lParam` points at the WINDOWPOS the system is about to apply
                let pos = unsafe { &mut *pos };
                let want = held().iter().find(|h| h.hwnd == key).and_then(|h| h.want);
                let (moves, sizes) = (pos.flags.0 & SWP_NOMOVE.0 == 0, pos.flags.0 & SWP_NOSIZE.0 == 0);
                let (x, y) = if moves { (pos.x, pos.y) } else { want.map_or((pos.x, pos.y), |w| (w.left, w.top)) };
                let (cx, cy) = if sizes { (pos.cx, pos.cy) } else { want.map_or((pos.cx, pos.cy), |w| (w.width(), w.height())) };
                if let Some(w) = guarded_rect(want, moves, sizes, Rect::new(x, y, x + cx, y + cy)) {
                    pos.x = w.left;
                    pos.y = w.top;
                    pos.cx = w.width();
                    pos.cy = w.height();
                    pos.flags = SET_WINDOW_POS_FLAGS(pos.flags.0 & !(SWP_NOMOVE.0 | SWP_NOSIZE.0));
                }
            }
        }
        WM_NCDESTROY => {
            held().retain(|h| h.hwnd != key);
        }
        _ => {}
    }
    // SAFETY: `prev` is what `SetWindowLongPtrW(GWLP_WNDPROC)` returned for this very window
    unsafe { CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(prev), hwnd, msg, wp, lp) }
}

/// The style of an overlay: a plain pop-up, no caption and no sizing frame.
pub const OVERLAY_STYLE: u32 = WS_POPUP.0 | WS_CLIPSIBLINGS.0 | WS_CLIPCHILDREN.0;

/// The extended style after the change: a tool window (no taskbar button, not in Alt+Tab), without the app-window bit, and "never
/// activate" (see the top of the file).
pub fn overlay_ex_style(ex: u32) -> u32 {
    (ex | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) & !WS_EX_APPWINDOW.0
}

/// Turns a window that has been created and not yet shown into an overlay window: its style, its look, its procedure. Must run on the
/// thread that owns the window. Does not move, show or activate it; [`place`] does. Can be called again.
pub fn prepare(hwnd: isize) -> anyhow::Result<()> {
    let h = hwnd_of(hwnd);
    if !crate::window::exists(hwnd) {
        anyhow::bail!("{hwnd:#x} is not a window");
    }
    if !is_ours(hwnd) {
        anyhow::bail!("{hwnd:#x} is not a window of this process");
    }
    unsafe {
        SetWindowLongPtrW(h, GWL_STYLE, OVERLAY_STYLE as isize);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(h, GWL_EXSTYLE, overlay_ex_style(ex) as isize);
        let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        let class_style = GetClassLongPtrW(h, GCL_STYLE);
        if class_style & CS_DROPSHADOW != 0 {
            SetClassLongPtrW(h, GCL_STYLE, (class_style & !CS_DROPSHADOW) as isize);
        }
        let set = |attr, value: u32| {
            if let Err(e) = DwmSetWindowAttribute(h, attr, &value as *const u32 as *const _, size_of::<u32>() as u32) {
                log::debug!("snip: DWM attribute {attr:?} not set on {hwnd:#x}: {e}");
            }
        };
        set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND.0 as u32);
        set(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
        set(DWMWA_NCRENDERING_POLICY, DWMNCRP_DISABLED.0 as u32);
        // Aero Peek hides the other windows to show the desktop: the overlay must not be one of the things it looks through
        set(DWMWA_EXCLUDED_FROM_PEEK, 1);
        install_proc(hwnd)?;
    }
    Ok(())
}

/// Puts the snip's procedure in front of the toolkit's on this window, once (it answers a click's activation itself, and the overlays'
/// guard holds their rectangle). A second call finds it there and does nothing.
fn install_proc(hwnd: isize) -> anyhow::Result<()> {
    if held().iter().any(|c| c.hwnd == hwnd) {
        return Ok(());
    }
    let old = unsafe { SetWindowLongPtrW(hwnd_of(hwnd), GWLP_WNDPROC, overlay_proc as *const () as usize as isize) };
    if old == 0 {
        anyhow::bail!("the window procedure of {hwnd:#x} could not be replaced");
    }
    held().push(Held { hwnd, prev: old, want: None });
    Ok(())
}

/// Places the window exactly over `monitor` (device px) above every other window, topmost, and shows it without activating it.
/// Returns false when Windows refused.
pub fn place(hwnd: isize, monitor: Rect) -> bool {
    if !is_ours(hwnd) {
        return false;
    }
    // (the procedure's guard is told the new place first, or it would hold the window at the old one)
    if let Some(c) = held().iter_mut().find(|c| c.hwnd == hwnd) {
        c.want = Some(monitor);
    }
    let o = crate::desktop::cover::outer_for_client(monitor, crate::dock::panel::frame_insets(hwnd));
    unsafe { SetWindowPos(hwnd_of(hwnd), Some(HWND_TOPMOST), o.left, o.top, o.width().max(1), o.height().max(1), SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW).is_ok() }
}

/// Makes a small window of the toolkit (the toolbar, the notice) one that is never activated, by its style and by its own answer to a
/// click, and has no taskbar button. Its look, its corners and its size stay the toolkit's. Must run on the window's thread, before it
/// is shown. Can be called again. False when the window is not ours or its procedure could not be put in front.
pub fn prepare_small(hwnd: isize) -> bool {
    if !is_ours(hwnd) {
        return false;
    }
    let h = hwnd_of(hwnd);
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(h, GWL_EXSTYLE, overlay_ex_style(ex) as isize);
        let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
    // (the guard of the procedure has no place for this window, so it leaves its size and position to the toolkit)
    match install_proc(hwnd) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("snip: {e:#}");
            false
        }
    }
}

/// The foreground window now (0 when there is none, which also happens for a moment while the foreground changes hands).
pub fn foreground() -> isize {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize
}

/// The virtual screen's rectangle and the number of monitors, as one value: it differs after a monitor was plugged, unplugged or moved
/// and is cheap enough to ask every few milliseconds (two metrics, no enumeration).
pub fn screen_signature() -> [i32; 5] {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN};
    unsafe {
        [GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN), GetSystemMetrics(SM_CMONITORS)]
    }
}

/// The pointer's place in the virtual screen (device pixels).
pub fn cursor() -> Option<(i32, i32)> {
    let mut p = windows::Win32::Foundation::POINT::default();
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) }.ok().map(|()| (p.x, p.y))
}

/// Whether Escape is down at this moment, whichever window has the keyboard. It looks at the one key, and only while an overlay is up;
/// it records nothing.
pub fn escape_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
    let state = unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) };
    state as u16 & 0x8000 != 0
}

/// Where the window is now (outer rectangle), for the check that it came out where it was placed.
pub fn rect_of(hwnd: isize) -> Option<Rect> {
    let mut r = windows::Win32::Foundation::RECT::default();
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd_of(hwnd), &mut r) }.ok()?;
    Some(Rect::new(r.left, r.top, r.right, r.bottom))
}

/// Places a small window (the toolbar, the notice) at `r` above everything without activating it. It has its own style from the
/// toolkit; only its place and its z-order are set.
pub fn place_small(hwnd: isize, r: Rect) -> bool {
    if !is_ours(hwnd) {
        return false;
    }
    unsafe { SetWindowPos(hwnd_of(hwnd), Some(HWND_TOPMOST), r.left, r.top, r.width().max(1), r.height().max(1), SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW).is_ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_is_a_plain_pop_up_and_a_tool_window_that_is_never_activated() {
        let ex = overlay_ex_style(WS_EX_APPWINDOW.0);
        assert_eq!(ex & WS_EX_TOOLWINDOW.0, WS_EX_TOOLWINDOW.0, "no taskbar button, not in Alt+Tab");
        assert_eq!(ex & WS_EX_APPWINDOW.0, 0);
        assert_eq!(ex & WS_EX_NOACTIVATE.0, WS_EX_NOACTIVATE.0, "a click must not raise it over its own toolbar");
        // and the style of a window that has other bits keeps them
        assert_eq!(overlay_ex_style(0x20) & 0x20, 0x20);
        assert_eq!(OVERLAY_STYLE & WS_POPUP.0, WS_POPUP.0);
    }

    #[test]
    fn a_click_never_activates_a_window_of_the_snip_whatever_the_toolkit_answers() {
        // WM_MOUSEACTIVATE (0x0021) is answered MA_NOACTIVATE (3): the click is delivered, the window is not activated
        assert_eq!(no_activate_reply(WM_MOUSEACTIVATE), Some(3));
        assert_eq!(no_activate_reply(0x0021), Some(3));
        // every other message is the toolkit's
        for msg in [0x0000, 0x0006 /* WM_ACTIVATE */, 0x0007, 0x0201 /* WM_LBUTTONDOWN */, 0x0046 /* WM_WINDOWPOSCHANGING */] {
            assert_eq!(no_activate_reply(msg), None, "{msg:#x}");
        }
    }

    #[test]
    fn the_screen_signature_and_the_foreground_can_be_read() {
        let sig = screen_signature();
        assert!(sig[2] > 0 && sig[3] > 0 && sig[4] >= 1, "{sig:?}");
        assert_eq!(sig, screen_signature(), "steady while nothing is plugged");
        let _ = foreground();
    }

    #[test]
    fn the_guard_holds_the_rectangle_it_was_placed_at() {
        let want = Some(Rect::new(-1440, -560, 0, 2000));
        // the toolkit resizes it to a different rectangle: held
        assert_eq!(guarded_rect(want, true, true, Rect::new(-1448, -564, 8, 2007)), want);
        // a change that keeps the rectangle (a z-order change alone, or the same rectangle): left alone
        assert_eq!(guarded_rect(want, false, false, Rect::new(0, 0, 10, 10)), None);
        assert_eq!(guarded_rect(want, true, true, Rect::new(-1440, -560, 0, 2000)), None);
        // before the first placement the guard does nothing
        assert_eq!(guarded_rect(None, true, true, Rect::new(0, 0, 10, 10)), None);
    }

    #[test]
    fn nothing_is_changed_on_a_window_that_is_not_ours() {
        // the desktop window of the machine: not a window of this process
        let shell = unsafe { windows::Win32::UI::WindowsAndMessaging::GetShellWindow() };
        if !shell.0.is_null() {
            let h = shell.0 as isize;
            assert!(!is_ours(h));
            assert!(prepare(h).is_err());
            assert!(!place(h, Rect::new(0, 0, 10, 10)));
            assert!(!place_small(h, Rect::new(0, 0, 10, 10)));
            assert!(!prepare_small(h));
        }
        assert!(!is_ours(0));
    }
}
