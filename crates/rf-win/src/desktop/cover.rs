//! The thin Win32 layer of cover mode (shell plan F4): turns a UI-toolkit window into one of the desktop's cover windows and keeps it
//! where the pure rules in `rf_core::desktop::cover` say. NOTHING HERE TOUCHES EXPLORER OR ITS WINDOWS: it reads the z-order (which
//! window is `GetShellWindow()`, which one is above it) and moves ONLY OUR OWN windows.
//!
//! WHAT A COVER WINDOW IS: a borderless pop-up, one per monitor, with no taskbar button and not in Alt+Tab (`WS_EX_TOOLWINDOW`), not
//! topmost, left out of Aero Peek (so Peek keeps showing the desktop that is ours), activatable (the desktop takes the keyboard) but
//! activated by a press only where the pure rule says so (an icon: yes, empty space: no), and closed by nothing (Alt+F4 on the
//! desktop does nothing here; Explorer shows its shutdown dialog, and ours comes later).
//!
//! HOW IT STAYS WHERE IT BELONGS: the window procedure is put in front of the toolkit's and rewrites `hwndInsertAfter` in
//! `WM_WINDOWPOSCHANGING` (`zorder::on_pos_changing`). The one thing it cannot see is the DESKTOP moving (Show Desktop raises the
//! shell's desktop window above everything); [`ZWatch`] listens for the foreground and minimize events, and [`reinsert_if_needed`]
//! puts the windows back when they are no longer directly above the desktop. While our own Show Desktop is on ([`set_raised`]) the
//! same check ends it when a program comes to the front, as Explorer's does (`zorder::raise_ends`).
//!
//! WHAT STOPPING IS: closing the windows. Nothing of Explorer's was hidden, moved or changed, so there is nothing to put back: its
//! desktop was under ours all the time and is simply visible again. (The window procedure and the registry of cover windows go with
//! the windows: `WM_NCDESTROY`.)
//!
//! COMPILED, UNIT-TESTED FOR WHAT IS PURE, NEVER RUN against a real window in this increment: the live test is the lead's.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use rf_core::desktop::cover::activation;
use rf_core::desktop::cover::geometry::{Pos, client_is_whole, cover_rect, min_max, outer_correction, pinned, refused_command, was_overruled};
use rf_core::desktop::cover::zorder::{self, Desktop, Foreground, Neighbour, PosChange, Verdict};
use rf_core::desktop::geom::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_CLOAKED, DWMWA_COLOR_NONE, DWMWA_EXCLUDED_FROM_PEEK, DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, EnumWindows, GCL_STYLE, GWL_EXSTYLE, GWL_STYLE, GWLP_WNDPROC, GW_HWNDPREV, GetClassLongPtrW, GetClassNameW, GetCursorPos, GetForegroundWindow, GetPropW, GetShellWindow, GetWindow, GetWindowLongPtrW, GetWindowRect,
    MINMAXINFO, SetPropW, WM_GETMINMAXINFO, WM_NCCALCSIZE,
    HWND_NOTOPMOST, HWND_TOP, IsIconic, IsWindowVisible, SC_CLOSE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetClassLongPtrW, SetWindowLongPtrW, SetWindowPos,
    WINDOWPOS, WM_CLOSE, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_SYSCOMMAND, WM_WINDOWPOSCHANGING, WNDPROC, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

/// `CS_DROPSHADOW`: the window class asks for a shadow around its windows (the toolkit's class may).
const CS_DROPSHADOW: usize = 0x0002_0000;

fn hwnd_of(h: isize) -> HWND {
    HWND(h as *mut _)
}

/// One cover window the procedure knows.
struct Cover {
    hwnd: isize,
    /// The procedure the toolkit had.
    prev: isize,
    /// The click areas of the icons, in client pixels, and whether the view has said anything yet.
    hot: Vec<Rect>,
    known: bool,
    /// The rectangle this window is allowed to have (the cover rectangle of its monitor, device px) and that monitor's whole
    /// rectangle; `None` until [`place`] has been called. Every request for another rectangle is turned into this one.
    want: Option<Rect>,
    monitor: Rect,
}

/// The window property every cover window carries. Another process (a second shell host, an older copy) reads it to tell a desktop
/// window from a full-screen program: the list of covers above is per process.
const MARK: windows::core::PCWSTR = windows::core::w!("rustyfiles.DesktopCover");

/// How many times the procedure has overruled a request for another rectangle (the first few are logged, with who asked how).
static OVERRULED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

static COVERS: Mutex<Vec<Cover>> = Mutex::new(Vec::new());

/// Show Desktop is on: the desktop windows are allowed above the programs that could not be minimized.
static RAISED: AtomicBool = AtomicBool::new(false);

/// When Show Desktop was last switched on (the grace period before a foreground change may end it).
static RAISED_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

fn covers() -> std::sync::MutexGuard<'static, Vec<Cover>> {
    COVERS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Whether `hwnd` is a cover window of this process (the dock's full-screen test asks: ours are never a full-screen application).
pub fn is_cover(hwnd: isize) -> bool {
    covers().iter().any(|c| c.hwnd == hwnd)
}

/// Whether `hwnd` is a cover window of ANY process (the property [`prepare`] sets): the dock's full-screen test must not take a desktop
/// window for a game, whichever host made it. Read-only.
pub fn is_marked_cover(hwnd: isize) -> bool {
    !unsafe { GetPropW(hwnd_of(hwnd), MARK) }.is_invalid()
}

/// Every cover window that exists now.
pub fn all() -> Vec<isize> {
    covers().iter().map(|c| c.hwnd).collect()
}

/// Whether Show Desktop is on (the windows are allowed above the programs).
pub fn is_raised() -> bool {
    RAISED.load(Ordering::Relaxed)
}

/// Show Desktop is on (`true`) or off. Takes effect for the next placement; [`reinsert_all`] applies it now.
pub fn set_raised(on: bool) {
    RAISED.store(on, Ordering::Relaxed);
    *RAISED_AT.lock().unwrap_or_else(|p| p.into_inner()) = on.then(std::time::Instant::now);
}

/// The window in front, read-only.
fn foreground() -> Option<Foreground> {
    let h = unsafe { GetForegroundWindow() };
    if h.0.is_null() {
        return None;
    }
    let key = h.0 as isize;
    let mut class = [0u16; 128];
    let n = unsafe { GetClassNameW(h, &mut class) };
    let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
    Some(Foreground { ours: is_ours(key) || is_cover(key), shell: crate::dock::fullscreen::is_shell_class(&class), minimized: unsafe { IsIconic(h) }.as_bool() })
}

/// Tells the window where its icons are (client pixels), so that a press on one activates the window and a press on empty space does
/// not. Call it when the icons move; an unchanged list costs a comparison.
pub fn set_hot_regions(hwnd: isize, rects: &[Rect]) {
    let mut cs = covers();
    if let Some(c) = cs.iter_mut().find(|c| c.hwnd == hwnd)
        && (!c.known || c.hot != rects)
    {
        c.hot = rects.to_vec();
        c.known = true;
    }
}

/// Whether the window belongs to this process. Everything in this module that CHANGES a window first asks this, so that no call
/// here can move, restyle or reorder a window of Explorer's or of anybody else's, whatever handle a caller passes.
pub fn is_ours(hwnd: isize) -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd_of(hwnd), Some(&mut pid)) };
    pid != 0 && pid == unsafe { GetCurrentProcessId() }
}

fn is_topmost(h: HWND) -> bool {
    unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0 }
}

/// The shell's desktop window and the window directly above it. Read-only.
pub fn desktop_view() -> Desktop {
    let shell = unsafe { GetShellWindow() };
    if shell.0.is_null() {
        return Desktop { window: None, above: None };
    }
    let above = unsafe { GetWindow(shell, GW_HWNDPREV) }.ok().filter(|h| !h.0.is_null()).map(|h| Neighbour { hwnd: h.0 as isize, topmost: is_topmost(h) });
    Desktop { window: Some(shell.0 as isize), above }
}

unsafe extern "system" fn cover_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let key = hwnd.0 as isize;
    // (the lock is held for a lookup only: a call out of here can come back in, for the same window)
    let prev = covers().iter().find(|c| c.hwnd == key).map(|c| c.prev);
    let Some(prev) = prev else { return unsafe { DefWindowProcW(hwnd, msg, wp, lp) } };
    // the desktop is not minimized: a cover that another program minimized comes back (`stayup.rs`; its own menu's minimize command
    // is refused below, with the other system commands)
    if crate::stayup::was_minimized(msg, wp.0) {
        crate::stayup::bring_back(key);
    }
    match msg {
        WM_WINDOWPOSCHANGING => {
            let pos = lp.0 as *mut WINDOWPOS;
            if !pos.is_null() {
                // SAFETY: for this message `lParam` points at the WINDOWPOS the system is about to apply
                let pos = unsafe { &mut *pos };
                // The cover refuses every rectangle but its own, whoever asks: the toolkit after a scale change, the system
                // maximizing it (a maximized pop-up is the work area plus a sizing frame: 2576 x 1447 at -8,-4 on a 2560 x 1440
                // monitor), a restored "maximized" placement.
                let want = covers().iter().find(|c| c.hwnd == key).and_then(|c| c.want);
                let asked = Pos { x: pos.x, y: pos.y, cx: pos.cx, cy: pos.cy, flags: pos.flags.0 };
                let kept = pinned(asked, want);
                if kept != asked {
                    if was_overruled(asked, kept) && OVERRULED.fetch_add(1, Ordering::Relaxed) < 12 {
                        log::warn!("cover: {key:#x} was asked to be {}x{} at {},{} (flags {:#x}); kept at {}x{} at {},{}", asked.cx, asked.cy, asked.x, asked.y, asked.flags, kept.cx, kept.cy, kept.x, kept.y);
                    }
                    (pos.x, pos.y, pos.cx, pos.cy) = (kept.x, kept.y, kept.cx, kept.cy);
                    pos.flags = windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS(kept.flags);
                }
                let c = PosChange { me: key, me_topmost: is_topmost(hwnd), flags: pos.flags.0, raised: RAISED.load(Ordering::Relaxed) };
                match zorder::on_pos_changing(&c, &desktop_view()) {
                    Verdict::Keep => {}
                    Verdict::KeepOrder => pos.flags = windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS(pos.flags.0 | SWP_NOZORDER.0),
                    Verdict::InsertAfter(after) => pos.hwndInsertAfter = HWND(after as *mut _),
                }
            }
        }
        WM_GETMINMAXINFO => {
            // "maximized" and every size the window can be dragged to are the cover rectangle (see `rf_core::...::geometry::min_max`)
            let r = unsafe { CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(prev), hwnd, msg, wp, lp) };
            let mm = covers().iter().find(|c| c.hwnd == key).and_then(|c| c.want.map(|w| min_max(w, c.monitor)));
            let info = lp.0 as *mut MINMAXINFO;
            if let (Some(mm), false) = (mm, info.is_null()) {
                // SAFETY: for this message `lParam` points at the MINMAXINFO the system fills in
                let info = unsafe { &mut *info };
                (info.ptMaxSize.x, info.ptMaxSize.y) = mm.max_size;
                (info.ptMaxPosition.x, info.ptMaxPosition.y) = mm.max_pos;
                (info.ptMinTrackSize.x, info.ptMinTrackSize.y) = mm.min_track;
                (info.ptMaxTrackSize.x, info.ptMaxTrackSize.y) = mm.max_track;
                return LRESULT(0);
            }
            return r;
        }
        // No non-client area, whatever style the window has: the client area is the whole window. `prepare` clears the sizing frame, and
        // the outer rectangle is pinned to the cover rectangle (see `pinned`), so a frame that came back (the toolkit re-applying the
        // style of its first placement) would otherwise eat 8 px of the client area on three sides: the content, laid out for the cover
        // rectangle, would be cut off and a grey band left at the right and the bottom.
        WM_NCCALCSIZE if wp.0 != 0 => return LRESULT(0),
        // a window that cannot be sized, moved, minimized or maximized from its system menu or a key (Alt+Space, Win+Up)
        WM_SYSCOMMAND if refused_command(wp.0) => {
            log::debug!("cover: {key:#x} refused system command {:#x}", wp.0 & 0xFFF0);
            return LRESULT(0);
        }
        WM_MOUSEACTIVATE => {
            let (hot, known) = covers().iter().find(|c| c.hwnd == key).map(|c| (c.hot.clone(), c.known)).unwrap_or_default();
            // (the pointer now, not `GetMessagePos`: that is where the last message taken from the queue was, and this one is sent
            // while the press is being processed, before its own message exists - for a touch or a pen it could be a stale point)
            let mut p = POINT::default();
            let _ = unsafe { GetCursorPos(&mut p) };
            let _ = unsafe { ScreenToClient(hwnd, &mut p) };
            // (the press is still delivered to the window when the answer is "do not activate")
            return LRESULT(activation::on_mouse_activate((p.x, p.y), &hot, known).reply());
        }
        // Alt+F4 on the desktop closes nothing (the toolkit would close the window and with it the desktop)
        WM_CLOSE => return LRESULT(0),
        WM_SYSCOMMAND if (wp.0 & 0xFFF0) == SC_CLOSE as usize => return LRESULT(0),
        WM_NCDESTROY => {
            covers().retain(|c| c.hwnd != key);
        }
        _ => {}
    }
    // SAFETY: `prev` is what `SetWindowLongPtrW(GWLP_WNDPROC)` returned for this very window
    unsafe { CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(prev), hwnd, msg, wp, lp) }
}

/// The style a cover window has: a plain pop-up. No caption and no sizing frame: the toolkit's pop-up keeps an invisible resize
/// frame that would make its outer rectangle 8 px larger than the client area on three sides, and the one-pixel rule (the window
/// must not cover its monitor) is about the OUTER rectangle.
pub const COVER_STYLE: u32 = WS_POPUP.0 | WS_CLIPSIBLINGS.0 | WS_CLIPCHILDREN.0;

/// The extended style after the change: a tool window (no taskbar button, not in Alt+Tab), without the app-window bit and without
/// "never activate". `WS_EX_TOPMOST` is left alone here: it goes with the first placement (see [`place`]).
pub fn cover_ex_style(ex: u32) -> u32 {
    (ex | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0 & !WS_EX_NOACTIVATE.0
}

/// Turns a window that has been created and not yet shown into a cover window: its style, its look, its procedure. Must run on the
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
        SetWindowLongPtrW(h, GWL_STYLE, COVER_STYLE as isize);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(h, GWL_EXSTYLE, cover_ex_style(ex) as isize);
        let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        let class_style = GetClassLongPtrW(h, GCL_STYLE);
        if class_style & CS_DROPSHADOW != 0 {
            SetClassLongPtrW(h, GCL_STYLE, (class_style & !CS_DROPSHADOW) as isize);
        }
        let set = |attr, value: u32| {
            if let Err(e) = DwmSetWindowAttribute(h, attr, &value as *const u32 as *const _, size_of::<u32>() as u32) {
                log::debug!("cover: DWM attribute {attr:?} not set on {hwnd:#x}: {e}");
            }
        };
        set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND.0 as u32);
        set(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
        set(DWMWA_NCRENDERING_POLICY, DWMNCRP_DISABLED.0 as u32);
        // Aero Peek hides the other windows to show the desktop: this IS the desktop and must stay
        set(DWMWA_EXCLUDED_FROM_PEEK, 1);
        // (any process can see this: the dock's full-screen test is not fooled by a desktop window of another host)
        if let Err(e) = SetPropW(h, MARK, Some(windows::Win32::Foundation::HANDLE(std::ptr::dangling_mut()))) {
            log::debug!("cover: the window mark was not set on {hwnd:#x}: {e}");
        }
        if covers().iter().all(|c| c.hwnd != hwnd) {
            let old = SetWindowLongPtrW(h, GWLP_WNDPROC, cover_proc as *const () as usize as isize);
            if old == 0 {
                anyhow::bail!("the window procedure of {hwnd:#x} could not be replaced");
            }
            covers().push(Cover { hwnd, prev: old, hot: Vec::new(), known: false, want: None, monitor: Rect::default() });
        }
    }
    Ok(())
}

/// Places the window on `monitor` (the monitor's whole rectangle, device px): the cover rectangle, directly above the desktop. Shows
/// it without activating it. The window's CLIENT area gets the rectangle (the view draws 1:1 in it); after [`prepare`] there is no
/// frame, so the outer rectangle is the same and still short of the monitor. Returns false when Windows refused.
pub fn place(hwnd: isize, monitor: Rect) -> bool {
    if !is_ours(hwnd) {
        return false;
    }
    let want = cover_rect(monitor);
    // The procedure keeps this rectangle from now on (see `pinned`), so the outer rectangle IS the cover rectangle: a measured frame
    // (`frame_insets`) is not added any more. Adding it was how a window could come out larger than its monitor when the measure was
    // taken while the toolkit's frame was on it.
    if let Some(c) = covers().iter_mut().find(|c| c.hwnd == hwnd) {
        c.want = Some(want);
        c.monitor = monitor;
    }
    // HWND_TOP is only what is asked: the procedure turns it into "directly above the desktop"
    let ok = unsafe { SetWindowPos(hwnd_of(hwnd), Some(HWND_TOP), want.left, want.top, want.width().max(1), want.height().max(1), SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW | SWP_FRAMECHANGED).is_ok() };
    if ok {
        keep_inside(hwnd, want);
        check_client(hwnd, want);
    }
    ok
}

/// Reads the client rectangle back and says so in the log when it is not the cover rectangle (it cannot be, with the frame removed by
/// `WM_NCCALCSIZE`; the line is how a live run shows that nothing else is at work).
fn check_client(hwnd: isize, want: Rect) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    let mut r = RECT::default();
    if unsafe { GetClientRect(hwnd_of(hwnd), &mut r) }.is_ok() && !client_is_whole(want, r.right - r.left, r.bottom - r.top) {
        log::warn!("cover: {hwnd:#x} has a client area of {}x{}, not {}x{}", r.right - r.left, r.bottom - r.top, want.width(), want.height());
    }
}

/// The one-pixel rule is about the OUTER rectangle, and nothing but a measurement shows it holds: read the rectangle back and, if the
/// window came out larger than the cover rectangle (a frame that was not expected), set the outer rectangle to exactly it.
fn keep_inside(hwnd: isize, limit: Rect) {
    use windows::Win32::Foundation::RECT;
    let mut r = RECT::default();
    if unsafe { GetWindowRect(hwnd_of(hwnd), &mut r) }.is_err() {
        return;
    }
    if let Some(fix) = outer_correction(Rect::new(r.left, r.top, r.right, r.bottom), limit) {
        log::warn!("cover: the window came out as {}x{} at {},{}; set to {}x{} so that it is not a full-screen window", r.right - r.left, r.bottom - r.top, r.left, r.top, fix.width(), fix.height());
        unsafe {
            let _ = SetWindowPos(hwnd_of(hwnd), None, fix.left, fix.top, fix.width().max(1), fix.height().max(1), SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        }
    }
}

/// The outer rectangle that gives `client` as the client area, for a window whose outer rectangle reaches `insets` (left, top, right,
/// bottom) beyond its client area. All zero after [`prepare`]: a cover window has no frame.
pub fn outer_for_client(client: Rect, insets: [i32; 4]) -> Rect {
    Rect::new(client.left - insets[0], client.top - insets[1], client.right + insets[2], client.bottom + insets[3])
}

/// The visible top-level windows from the top of the z-order to the bottom (cloaked ones, which are on another virtual desktop, left
/// out: where they stand in the order is nothing anyone sees).
pub fn visible_order() -> Vec<isize> {
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> windows::core::BOOL {
        // SAFETY: `lp` is the address of the Vec the caller passed
        let out = unsafe { &mut *(lp.0 as *mut Vec<isize>) };
        if unsafe { IsWindowVisible(h) }.as_bool() {
            let mut cloaked = 0u32;
            let hr = unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, size_of::<u32>() as u32) };
            if hr.is_err() || cloaked == 0 {
                out.push(h.0 as isize);
            }
        }
        windows::core::BOOL(1)
    }
    let mut v: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut v as *mut Vec<isize> as isize));
    }
    v
}

/// Puts every cover window back directly above the desktop if any is not. True when it had to. The cost when nothing is wrong is one
/// enumeration of the top-level windows (a few dozen): cheap enough for every foreground change.
pub fn reinsert_if_needed() -> bool {
    let mine = all();
    if mine.is_empty() {
        return false;
    }
    if RAISED.load(Ordering::Relaxed) {
        // Show Desktop is on, and ends as it does for Explorer: when a program comes to the front. The covers go back under the programs.
        let since = RAISED_AT.lock().unwrap_or_else(|p| p.into_inner()).map_or(u64::MAX, |t| t.elapsed().as_millis() as u64);
        if zorder::raise_ends(foreground(), since) {
            set_raised(false);
            reinsert_all();
            return true;
        }
        return false;
    }
    let desktop = unsafe { GetShellWindow() };
    let d = (!desktop.0.is_null()).then_some(desktop.0 as isize);
    // (a corner patch stands directly above its own cover window, so between two covers: it is not "something that came between")
    let order: Vec<isize> = visible_order().into_iter().filter(|h| !super::cornerpatch::is_patch(*h)).collect();
    if zorder::is_in_place(&order, &mine, d) {
        return false;
    }
    reinsert_all();
    true
}

/// Asks every cover window to go to the top; the procedure puts it where it belongs (or, while Show Desktop is on, at the top).
pub fn reinsert_all() {
    for h in all() {
        unsafe {
            let _ = SetWindowPos(hwnd_of(h), Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        }
    }
}

/// Gives the foreground to one of our own cover windows: the rubber band ended with icons selected, so the keyboard belongs to them.
/// Our process has just received the mouse-up, which is what lets it take the foreground. (The toolkit's own `activate_window` is not
/// used for this: it puts the window back to its first rectangle and sends fake key presses to win the foreground.) The z-order is not
/// affected: the procedure keeps the window directly above the desktop whoever is active.
pub fn activate(hwnd: isize) {
    if !is_ours(hwnd) {
        return;
    }
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd_of(hwnd)) };
}

/// Takes the topmost style off a window that has it (a pop-up of the toolkit starts topmost). [`place`] ends up doing the same through
/// the procedure; this is for the moment before the procedure is in.
pub fn make_not_topmost(hwnd: isize) {
    if !is_ours(hwnd) {
        return;
    }
    unsafe {
        let _ = SetWindowPos(hwnd_of(hwnd), Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
    }
}

/// Listens for what moves the shell's desktop above our windows (the foreground changes, windows are minimized and restored) and
/// calls `on_event` from a thread of its own, coalesced. The callback must only hand the news to the thread that owns the windows.
pub struct ZWatch {
    thread: u32,
}

thread_local! {
    static EVENTS: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
}

unsafe extern "system" fn on_event(_: windows::Win32::UI::Accessibility::HWINEVENTHOOK, _event: u32, hwnd: HWND, id_object: i32, id_child: i32, _thread: u32, _time: u32) {
    // only whole windows, not their parts
    if id_object != 0 || id_child != 0 || hwnd.is_invalid() {
        return;
    }
    EVENTS.with(|e| {
        if let Some(tx) = e.borrow().as_ref() {
            let _ = tx.send(());
        }
    });
}

impl ZWatch {
    pub fn start(on_event_cb: impl Fn() + Send + 'static) -> anyhow::Result<ZWatch> {
        use std::time::Duration;
        use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, GetMessageW, MSG, TranslateMessage, WINEVENT_OUTOFCONTEXT,
        };
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<u32>();
        std::thread::Builder::new().name("rf-cover-zwatch".into()).spawn(move || {
            EVENTS.with(|e| *e.borrow_mut() = Some(tx));
            let ranges = [(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND), (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND)];
            // (our own process's events are of no interest: our windows never move the shell's desktop)
            let hooks: Vec<_> = ranges
                .iter()
                .map(|&(a, b)| unsafe { SetWinEventHook(a, b, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT | windows::Win32::UI::WindowsAndMessaging::WINEVENT_SKIPOWNPROCESS) })
                .collect();
            let _ = ready_tx.send(unsafe { windows::Win32::System::Threading::GetCurrentThreadId() });
            let mut msg = MSG::default();
            unsafe {
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                for h in hooks {
                    let _ = UnhookWinEvent(h);
                }
            }
            EVENTS.with(|e| *e.borrow_mut() = None);
        })?;
        let thread = ready_rx.recv_timeout(Duration::from_secs(5))?;
        std::thread::Builder::new().name("rf-cover-zwatch-coalesce".into()).spawn(move || {
            // a foreground change comes with a burst (the old window deactivates, the new one activates, windows restore one by one)
            while rx.recv().is_ok() {
                let until = std::time::Instant::now() + Duration::from_millis(40);
                loop {
                    let left = until.saturating_duration_since(std::time::Instant::now());
                    match rx.recv_timeout(left) {
                        Ok(()) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        Err(_) => return,
                    }
                }
                on_event_cb();
            }
        })?;
        Ok(ZWatch { thread })
    }
}

impl Drop for ZWatch {
    fn drop(&mut self) {
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extended_style_is_a_tool_window_that_may_be_activated_and_has_no_taskbar_button() {
        let ex = WS_EX_APPWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0 | 0x0020_0000 /* WS_EX_NOREDIRECTIONBITMAP */;
        let new = cover_ex_style(ex);
        assert_ne!(new & WS_EX_TOOLWINDOW.0, 0, "not in Alt+Tab, no taskbar button");
        assert_eq!(new & WS_EX_APPWINDOW.0, 0);
        assert_eq!(new & WS_EX_NOACTIVATE.0, 0, "the desktop takes the keyboard");
        assert_ne!(new & 0x0020_0000, 0, "the toolkit's composition style stays");
        assert_eq!(cover_ex_style(new), new, "idempotent");
    }

    #[test]
    fn the_style_is_a_plain_pop_up_with_no_frame_and_no_minimize_box() {
        assert_eq!(COVER_STYLE & 0x00C0_0000, 0, "no caption");
        assert_eq!(COVER_STYLE & 0x0004_0000, 0, "no sizing frame");
        assert_eq!(COVER_STYLE & 0x0002_0000, 0, "no minimize box: Show Desktop leaves it alone");
        assert_ne!(COVER_STYLE & WS_POPUP.0, 0);
    }

    /// A message-only window is never shown and has no rectangle on any screen: it can carry the mark without a window appearing.
    #[test]
    fn a_window_with_the_mark_is_a_marked_cover_of_any_process_and_one_without_is_not() {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE};
        let h = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), windows::core::w!("STATIC"), windows::core::w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None) }.expect("a message-only window");
        let key = h.0 as isize;
        assert!(!is_marked_cover(key), "no mark yet");
        unsafe { SetPropW(h, MARK, Some(windows::Win32::Foundation::HANDLE(std::ptr::dangling_mut()))) }.expect("set the mark");
        assert!(is_marked_cover(key), "the mark is what another process reads");
        assert!(!is_cover(key), "and it is not one of this process's own covers");
        unsafe { DestroyWindow(h) }.expect("destroy the test window");
    }

    #[test]
    fn a_non_window_is_an_error_and_a_non_cover_is_not_known() {
        assert!(prepare(0).is_err());
        assert!(!is_ours(0), "no window is ours");
        assert!(!is_cover(0x1234));
        assert!(!place(0, Rect::new(0, 0, 100, 100)));
        // no cover windows: nothing to put back
        assert!(!reinsert_if_needed());
        set_hot_regions(0x1234, &[Rect::new(0, 0, 1, 1)]); // unknown window: ignored
    }

    /// How it is known that stopping leaves nothing of Explorer's to put back: this layer has no call that could change or message a
    /// window it does not own. The only calls that change a window are `SetWindowLongPtrW`, `SetWindowPos`, `SetClassLongPtrW` and
    /// `DwmSetWindowAttribute`, and each is behind [`is_ours`] (or runs for a window that [`prepare`] accepted, which is ours). The
    /// calls below are the ones that could reach another process's window; none of them is made here.
    #[test]
    fn this_layer_calls_nothing_that_could_change_or_message_a_foreign_window() {
        let src = include_str!("cover.rs");
        let code = &src[..src.find("#[cfg(test)]").expect("the tests follow the code")];
        for forbidden in [
            "SendMessage", "PostMessage", "SendNotifyMessage", "ShowWindow", "DestroyWindow", "CloseWindow", "SetParent", "BringWindowToTop", "SetShellWindow", "SetTaskmanWindow",
            "RegisterShellHook", "RegisterHotKey", "SetWindowsHookEx", "TerminateProcess", "RegSetValue", "RegCreateKey", "RegDeleteValue", "SPI_SET", "SystemParametersInfo", "ABM_",
            // (the glass engine must keep seeing the cover windows as the backdrop of the glass above them: no capture exclusion, ever)
            "exclude_from_capture", "SetWindowDisplayAffinity", "WDA_EXCLUDEFROMCAPTURE",
        ] {
            assert!(!code.contains(forbidden), "cover.rs must not call {forbidden}");
        }
        // the things that change a window are each guarded: count them against the guards
        // the functions that act on a window by its handle, each starting with the ownership check
        let acting = ["fn prepare(", "fn place(", "fn make_not_topmost(", "fn activate("];
        for f in acting {
            let at = code.find(f).unwrap_or_else(|| panic!("{f} exists"));
            let body = &code[at..];
            let first_lines = &body[..body.len().min(600)];
            assert!(first_lines.contains("is_ours(hwnd)"), "{f} checks that the window is ours before it acts");
        }
        // SetForegroundWindow appears once, in `activate`, behind that check
        assert_eq!(code.matches("SetForegroundWindow").count(), 1);
    }

    /// Read-only: the real z-order can be read; some visible window is above the desktop's or the desktop is at the top.
    #[test]
    fn the_real_z_order_can_be_read() {
        let order = visible_order();
        assert!(!order.is_empty());
        let _ = desktop_view();
    }
}
