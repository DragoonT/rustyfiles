//! Whether a glass window can be seen at all: the other windows above it may hide it completely.
//!
//! A glass that nobody sees must cost nothing: no frame is copied for it, no backdrop is blurred, nothing is read back or presented
//! (`engine.rs` leaves such a window out of everything and, when no glass on a monitor is seen, lets its capture session go). The
//! question here is only a geometric one, answered from the z order: walk the top-level windows from the top down to the glass window
//! and collect the rectangles of those that certainly hide what lies under them; the glass is hidden when the union of those contains
//! its rectangle (`geom::covered_by`).
//!
//! "Certainly hide" is deliberately strict, because a wrong yes shows as a glass that is frozen while the user looks straight at it
//! through a window that only looked opaque, and a wrong no costs only the work that was done before. A window above counts only when
//! it is visible, not minimised, not cloaked (another virtual desktop, a suspended app), has no window region, is not click-through,
//! and is opaque: a layered window counts only with `LWA_ALPHA` 255, and one of rustyfiles' own glass windows (it carries the mark
//! of `crossproc.rs`) never counts: it is see-through by design.
//!
//! The doubtful one is a window with NO REDIRECTION BITMAP (a swap chain in DirectComposition, which can be see-through anywhere),
//! layered at alpha 255 or not layered at all. Today's browsers, terminals and store apps are such windows, and so can a transparent
//! Electron or CEF overlay be (an annotation tool, a game overlay, a widget host) that fills the screen over the glass window and
//! hides nothing. It counts only when it is maximised, or has a title bar of its own and is not a tool window or one that never
//! takes the focus (`is_app_window`): the things a browser or an editor window have and an overlay does not.
//!
//! The kind that is NOT layered counted for nothing until 2026-11-14 ("may be see-through anywhere"). That is the strict reading, and
//! it meant that a rustyfiles window behind a maximised Chrome was taken for a window in view: its glass went on being drawn from a
//! screen it could not be seen on (a user watching a video in the browser saw the video run in that window's glass, in the dock's
//! preview of it), and the engine never let its capture go. What is given up: an application window that really is see-through
//! without being layered (a terminal set to a translucent background) now hides the glass under it as far as the engine knows, and
//! the glass seen through it stands still until that window moves away.
//!
//! Its rectangle is what the window manager draws (the extended frame bounds, without the invisible resize border), and for a window
//! that is not maximised its four corners are left out, because Windows 11 rounds them.

use std::collections::BTreeMap;
use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::GetWindowRgnBox;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GWL_STYLE, GetForegroundWindow, GetLayeredWindowAttributes, GetWindowLongPtrW, GetWindowRect, IsIconic, IsWindowVisible,
    IsZoomed, LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, WS_CAPTION, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};
use windows::core::BOOL;

use super::geom::{Rc, covered_by};

/// What a window's extended style and layered attributes say about whether it hides what lies under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Look {
    pub style: u32,
    pub ex_style: u32,
    /// `GetLayeredWindowAttributes` of a layered window: (alpha, flags); `None` when the call failed (a window drawn with
    /// `UpdateLayeredWindow` has a per-pixel alpha and answers with an error).
    pub layered: Option<(u8, u32)>,
    /// The window has a window region of its own.
    pub has_region: bool,
    /// It is a glass window of rustyfiles (this process's or another's: the mark of `crossproc.rs`).
    pub glass: bool,
}

/// The signs of a real application window: maximised, or with a title bar of its own and neither a tool window nor one that never
/// takes the focus. What tells a browser, a terminal or an editor from an overlay made with the same toolkit.
fn is_app_window(l: &Look, zoomed: bool) -> bool {
    let captioned = l.style & WS_CAPTION.0 == WS_CAPTION.0;
    zoomed || (captioned && l.ex_style & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) == 0)
}

/// Whether a window with this look is opaque over its whole rectangle. `zoomed`: it is maximised.
pub(super) fn is_solid(l: &Look, zoomed: bool) -> bool {
    if l.has_region || l.glass || l.ex_style & WS_EX_TRANSPARENT.0 != 0 {
        return false;
    }
    if l.ex_style & WS_EX_LAYERED.0 != 0 {
        // opaque only when it is a plain alpha of 255 (no colour key, no per-pixel alpha): the window as a whole is composed at that
        // alpha, whatever its content (a DirectComposition tree included) says about its own
        if !matches!(l.layered, Some((255, flags)) if flags == LWA_ALPHA.0) {
            return false;
        }
        // Layered at 255 with no redirection bitmap is a browser, and also what a transparent overlay made with the same toolkit looks
        // like (the alpha is the window's, not its content's). Only the signs of a real application window count.
        if l.ex_style & WS_EX_NOREDIRECTIONBITMAP.0 != 0 {
            return is_app_window(l, zoomed);
        }
        return true;
    }
    // Not layered and drawn by DirectComposition without a bitmap: it may be see-through anywhere, and it is also what a browser, a
    // terminal and a store app are today. The same signs decide (see the module docs for what that gives up).
    if l.ex_style & WS_EX_NOREDIRECTIONBITMAP.0 != 0 {
        return is_app_window(l, zoomed);
    }
    true
}

/// The parts of a window's rectangle that certainly hide what is under it: the whole rectangle for a window that is maximised (or covers
/// its monitor), otherwise the rectangle without its four corners (`radius` px each way, the rounding Windows 11 gives).
pub(super) fn solid_parts(frame: Rc, square: bool, radius: i32) -> Vec<Rc> {
    if frame.is_empty() {
        return Vec::new();
    }
    if square || radius <= 0 || frame.r - frame.l <= 2 * radius || frame.b - frame.t <= 2 * radius {
        return vec![frame];
    }
    vec![Rc::new(frame.l + radius, frame.t, frame.r - radius, frame.b), Rc::new(frame.l, frame.t + radius, frame.r, frame.b - radius)]
}

/// The corner radius to leave out, device px, for a window on a display of this DPI (Windows 11 rounds by 8 px at 100 %; one more for
/// the anti aliased edge).
pub(super) fn corner_allowance(dpi: u32) -> i32 {
    (8.0 * dpi.max(96) as f32 / 96.0).ceil() as i32 + 1
}

fn rc_of(r: RECT) -> Rc {
    Rc::new(r.left, r.top, r.right, r.bottom)
}

unsafe fn is_cloaked(h: HWND) -> bool {
    let mut cloaked = 0u32;
    DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, size_of::<u32>() as u32).is_ok() && cloaked != 0
}

/// The rectangle the window manager shows of a window (without the invisible resize border), falling back to the window rectangle.
unsafe fn frame_of(h: HWND, window_rect: RECT) -> Rc {
    let mut r = RECT::default();
    if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut c_void, size_of::<RECT>() as u32).is_ok() {
        rc_of(r)
    } else {
        rc_of(window_rect)
    }
}

unsafe fn look_of(h: HWND) -> Look {
    let ex_style = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
    let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
    let layered = if ex_style & WS_EX_LAYERED.0 != 0 {
        let (mut alpha, mut flags, mut key) = (0u8, LAYERED_WINDOW_ATTRIBUTES_FLAGS(0), windows::Win32::Foundation::COLORREF(0));
        GetLayeredWindowAttributes(h, Some(&mut key), Some(&mut alpha), Some(&mut flags)).ok().map(|()| (alpha, flags.0))
    } else {
        None
    };
    let mut rb = RECT::default();
    // 0 (ERROR) is "no region"
    let has_region = GetWindowRgnBox(h, &mut rb) != windows::Win32::Graphics::Gdi::GDI_REGION_TYPE(0);
    Look { style, ex_style, layered, has_region, glass: super::crossproc::is_marked(h) }
}

struct Walk<'a> {
    /// The glass windows asked about, with the rectangle each shows on the screen.
    mine: &'a [(isize, Rc)],
    /// Rectangles of the opaque windows met so far (only those that touch some glass rectangle).
    above: Vec<Rc>,
    found: BTreeMap<isize, bool>,
}

unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
    let w = unsafe { &mut *(lp.0 as *mut Walk) };
    let key = h.0 as isize;
    if let Some((_, target)) = w.mine.iter().find(|(k, _)| *k == key) {
        w.found.insert(key, covered_by(*target, &w.above));
        // windows below the lowest one asked about cannot matter
        return BOOL((w.found.len() < w.mine.len()) as i32);
    }
    unsafe {
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
            return BOOL(1);
        }
        let mut wr = RECT::default();
        if GetWindowRect(h, &mut wr).is_err() {
            return BOOL(1);
        }
        let outer = rc_of(wr);
        // a window that touches no glass rectangle (the great majority) is not looked at any closer
        if !w.mine.iter().any(|(_, t)| t.meets(&outer)) {
            return BOOL(1);
        }
        let zoomed = IsZoomed(h).as_bool();
        if is_cloaked(h) || !is_solid(&look_of(h), zoomed) {
            return BOOL(1);
        }
        let frame = frame_of(h, wr);
        let dpi = GetDpiForWindow(h);
        for part in solid_parts(frame, zoomed, corner_allowance(dpi)) {
            w.above.push(part);
        }
    }
    BOOL(1)
}

/// For every glass window given (handle and the rectangle its glass shows, on the screen): whether the windows above it hide it
/// completely. A window the walk does not meet (not a top-level window) is not in the answer, which callers treat as "not hidden".
pub(super) fn covered_windows(mine: &[(isize, Rc)]) -> BTreeMap<isize, bool> {
    let mut walk = Walk { mine, above: Vec::new(), found: BTreeMap::new() };
    if mine.is_empty() {
        return walk.found;
    }
    unsafe {
        // (EnumWindows reports an error when the callback stopped it: that is how it ends here)
        let _ = EnumWindows(Some(each), LPARAM(&mut walk as *mut Walk as isize));
    }
    walk.found
}

/// One top-level window that meets a rectangle, as the survey of [`survey`] saw it.
pub(super) struct Surveyed {
    pub class: String,
    pub pid: u32,
    pub style: u32,
    pub ex_style: u32,
    pub layered: Option<(u8, u32)>,
    pub has_region: bool,
    pub cloaked: bool,
    pub zoomed: bool,
    pub frame: Rc,
    /// Whether the walk would count it as hiding what is under it.
    pub counts: bool,
}

/// Every visible, not minimised top-level window that meets `target`, from the top of the z order down, with what the walk thinks of it
/// (for the manual check `examples/glass_cover_probe.rs`; reads windows only).
pub(super) fn survey(target: Rc) -> Vec<Surveyed> {
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        let (target, out) = unsafe { &mut *(lp.0 as *mut (Rc, Vec<Surveyed>)) };
        unsafe {
            if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
                return BOOL(1);
            }
            let mut wr = RECT::default();
            if GetWindowRect(h, &mut wr).is_err() || !target.meets(&rc_of(wr)) {
                return BOOL(1);
            }
            let mut name = [0u16; 128];
            let n = windows::Win32::UI::WindowsAndMessaging::GetClassNameW(h, &mut name).max(0) as usize;
            let mut pid = 0u32;
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(h, Some(&mut pid));
            let look = look_of(h);
            let cloaked = is_cloaked(h);
            out.push(Surveyed {
                class: String::from_utf16_lossy(&name[..n]),
                pid,
                style: look.style,
                ex_style: look.ex_style,
                layered: look.layered,
                has_region: look.has_region,
                cloaked,
                zoomed: IsZoomed(h).as_bool(),
                frame: frame_of(h, wr),
                counts: !cloaked && is_solid(&look, IsZoomed(h).as_bool()),
            });
        }
        BOOL(1)
    }
    let mut state = (target, Vec::new());
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut state as *mut (Rc, Vec<Surveyed>) as isize));
    }
    state.1
}

/// A window's rectangle on the screen (`None` for no window): the engine watches the foreground window's, because a covering window
/// that is dragged away changes nothing else.
pub(super) fn window_rect(hwnd: isize) -> Option<Rc> {
    if hwnd == 0 {
        return None;
    }
    let mut r = RECT::default();
    unsafe { GetWindowRect(HWND(hwnd as *mut c_void), &mut r) }.ok().map(|()| rc_of(r))
}

/// The window that has the keyboard focus, as a number (0: none). A change of it is the usual moment a window comes to the top or goes
/// under another.
pub(super) fn foreground() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: Look = Look { style: 0, ex_style: 0, layered: None, has_region: false, glass: false };

    #[test]
    fn only_certainly_opaque_windows_count() {
        let solid = |l: &Look| is_solid(l, false);
        assert!(solid(&PLAIN), "an ordinary window");
        assert!(!solid(&Look { has_region: true, ..PLAIN }), "a window with a region has holes");
        assert!(!solid(&Look { ex_style: WS_EX_TRANSPARENT.0, ..PLAIN }), "click-through");
        assert!(!solid(&Look { ex_style: WS_EX_NOREDIRECTIONBITMAP.0, ..PLAIN }), "drawn by DirectComposition: may be see-through anywhere");
        assert!(!solid(&Look { ex_style: WS_EX_NOREDIRECTIONBITMAP.0 | WS_EX_LAYERED.0, layered: Some((0, LWA_ALPHA.0)), ..PLAIN }), "an invisible overlay");
        let layered = |layered| Look { ex_style: WS_EX_LAYERED.0, layered, ..PLAIN };
        assert!(solid(&layered(Some((255, LWA_ALPHA.0)))), "a layered window with alpha 255");
        assert!(!solid(&layered(Some((200, LWA_ALPHA.0)))), "a translucent one");
        assert!(!solid(&layered(Some((255, LWA_ALPHA.0 | 1)))), "one with a colour key");
        assert!(!solid(&layered(None)), "per-pixel alpha (UpdateLayeredWindow)");
    }

    /// A browser is layered at alpha 255 with no redirection bitmap, and so can a transparent overlay made with the same toolkit: the
    /// overlay must not count as hiding the window under it (the glass would freeze while the user looks straight at it).
    #[test]
    fn a_layered_window_without_a_redirection_bitmap_counts_only_with_the_signs_of_an_application_window() {
        let both = WS_EX_NOREDIRECTIONBITMAP.0 | WS_EX_LAYERED.0;
        let ambiguous = |style: u32, extra_ex: u32| Look { style, ex_style: both | extra_ex, layered: Some((255, LWA_ALPHA.0)), has_region: false, glass: false };
        let caption = WS_CAPTION.0;
        // a browser window: a title bar (the browser draws over it), takes the focus, not a tool window
        assert!(is_solid(&ambiguous(caption, 0), false), "a browser, not maximised");
        assert!(is_solid(&ambiguous(0, 0), true), "maximised counts whatever its styles say");
        // a transparent overlay: a pop-up without a title bar, usually a tool window or one that never takes the focus
        assert!(!is_solid(&ambiguous(0, 0), false), "no title bar, not maximised: an overlay");
        assert!(!is_solid(&ambiguous(caption, WS_EX_TOOLWINDOW.0), false), "a tool window");
        assert!(!is_solid(&ambiguous(caption, WS_EX_NOACTIVATE.0), false), "a window that never takes the focus");
        assert!(!is_solid(&ambiguous(0x80000000, WS_EX_TOOLWINDOW.0), false), "a pop-up tool window");
        // the plain layered window is not in doubt
        assert!(is_solid(&Look { style: 0, ex_style: WS_EX_LAYERED.0, layered: Some((255, LWA_ALPHA.0)), has_region: false, glass: false }, false));
    }

    /// Today's Chrome, Windows Terminal and the store apps: no redirection bitmap and NOT layered. Measured on a user's desktop
    /// (2026-11-14): a maximised Chrome with style 17CF0000 and extended style 00200100 over a rustyfiles window whose glass went on
    /// showing the browser's video.
    #[test]
    fn an_application_window_without_a_redirection_bitmap_hides_what_is_under_it_and_an_overlay_does_not() {
        let dcomp = |style: u32, extra_ex: u32| Look { style, ex_style: WS_EX_NOREDIRECTIONBITMAP.0 | 0x100 | extra_ex, layered: None, has_region: false, glass: false };
        assert!(is_solid(&dcomp(0x17CF_0000, 0), true), "the maximised browser of the report");
        assert!(is_solid(&dcomp(0x14CF_0000, 0), false), "a terminal or a browser window that is not maximised: it has a title bar");
        assert!(is_solid(&dcomp(0x9400_0000, 0), true), "maximised counts whatever its styles say");
        // an overlay: a pop-up with no title bar, a tool window, a window that never takes the focus
        assert!(!is_solid(&dcomp(0x9400_0000, 0), false), "a full-screen pop-up with no title bar (an input host, an overlay)");
        assert!(!is_solid(&dcomp(0x14CF_0000, WS_EX_TOOLWINDOW.0), false));
        assert!(!is_solid(&dcomp(0x14CF_0000, WS_EX_NOACTIVATE.0), false));
        // with a region it has holes, whatever else it is
        assert!(!is_solid(&Look { has_region: true, ..dcomp(0x17CF_0000, 0) }, true));
    }

    #[test]
    fn a_glass_window_of_rustyfiles_never_hides_what_is_under_it() {
        // (it is see-through by design: another rustyfiles window with the glass backdrop, maximised over this one)
        let glass = Look { style: 0x14CF_0000, ex_style: WS_EX_NOREDIRECTIONBITMAP.0 | 0x100, layered: None, has_region: false, glass: true };
        assert!(!is_solid(&glass, false));
        assert!(!is_solid(&glass, true), "maximised too");
        assert!(!is_solid(&Look { glass: true, ..PLAIN }, false));
        assert!(is_solid(&Look { glass: false, ..glass }, false), "the same window without the mark is an application window");
    }

    #[test]
    fn rounded_corners_are_not_counted_as_covered() {
        let frame = Rc::new(100, 100, 500, 400);
        // a window that sits over a glass's corner leaves the corner pixel open: the glass is not hidden by it
        let parts = solid_parts(frame, false, 9);
        assert!(!covered_by(Rc::new(100, 100, 500, 400), &parts), "the corners are open");
        assert!(covered_by(Rc::new(110, 110, 490, 390), &parts), "a glass inside, clear of the corners, is hidden");
        assert!(covered_by(Rc::new(100, 150, 120, 200), &parts), "the left edge, away from the corners, is hidden");
        assert!(covered_by(Rc::new(100, 100, 500, 400), &solid_parts(frame, true, 9)), "a maximised window is square");
        assert!(solid_parts(Rc::new(0, 0, 0, 10), false, 9).is_empty());
        // a window too small to cut corners out of is taken whole
        assert_eq!(solid_parts(Rc::new(0, 0, 10, 10), false, 9), vec![Rc::new(0, 0, 10, 10)]);
        assert_eq!(corner_allowance(96), 9);
        assert_eq!(corner_allowance(192), 17);
        assert_eq!(corner_allowance(0), 9, "a DPI that is not known is 96");
    }

    #[test]
    fn the_usual_stacks() {
        // a maximised browser over the glass window
        let glass = Rc::new(200, 100, 1000, 700);
        let browser = solid_parts(Rc::new(0, 0, 2560, 1400), true, 9);
        assert!(covered_by(glass, &browser));
        // a smaller window that leaves a strip of the glass visible
        let small = solid_parts(Rc::new(0, 0, 900, 800), false, 9);
        assert!(!covered_by(glass, &small));
        // two windows side by side that together hide it, with their rounded corners not on the glass
        let left = solid_parts(Rc::new(0, 0, 640, 800), false, 9);
        let right = solid_parts(Rc::new(640, 0, 1280, 800), false, 9);
        let both: Vec<Rc> = left.into_iter().chain(right).collect();
        assert!(covered_by(Rc::new(200, 100, 1000, 700), &both), "the seam between them is clear of both windows' corners");
        assert!(!covered_by(Rc::new(200, 0, 1000, 100), &both), "at the top the rounded corners of the two windows leave a notch open");
    }

    /// The walk over the real windows of this desktop: it must come back with an answer for a handle that is not a window and must not
    /// panic. (Read only: nothing is created.)
    #[test]
    fn a_handle_that_is_no_window_is_not_in_the_answer() {
        let a = covered_windows(&[(0x7fff_fff0, Rc::new(0, 0, 10, 10))]);
        assert!(a.is_empty());
        assert!(covered_windows(&[]).is_empty());
        let _ = foreground();
    }
}
