//! The arithmetic and the decisions of a dock panel (the Start panel, and any other window that opens above an island): where it
//! goes, whether a click on a button opens, closes or switches it, when it closes by itself, who gets the keyboard back, and how its
//! opening and closing is eased. All of it pure and tested; the few Win32 calls at the end only read the pointer and move a window.
//!
//! A panel is an ordinary (activatable) pop-up window of the dock process. The islands are never activated (see `island.rs`), a panel
//! is: it takes the keyboard while it is open and gives it back to the window that had it when it closes.

use super::layout::{Rect, px};

/// The gap between an island's visible glass and the panel above it, logical px.
pub const GAP: f32 = 8.0;

/// Opening and closing take this long (eased).
pub const ANIM_MS: u32 = 120;
/// The panel rises this far (logical px) while it opens and sinks the same while it closes.
pub const RISE_PX: f32 = 6.0;

/// A panel that has just been opened is not closed for losing the foreground before this many ms, unless it has been in front once
/// (Windows needs a moment to hand the foreground over, and the window in front meanwhile is still the old one).
pub const FOREGROUND_GRACE_MS: u64 = 1_200;

/// Which edge of its island a panel lines up with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The panel's left edge is the island's left edge (the left island).
    Left,
    /// The panel's right edge is the island's right edge (the right island).
    Right,
}

/// A panel's name. Only one panel is open at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PanelId(pub &'static str);

impl PanelId {
    /// The Start panel (and the search that shares it).
    pub const START: PanelId = PanelId("start");
    /// The control centre above the right island (volume, network, media, brightness).
    pub const CONTROL_CENTRE: PanelId = PanelId("control-centre");
}

/// The rectangle (device px, screen coordinates) of a panel of `size` logical px that sits above the island whose VISIBLE glass is
/// `glass`, `GAP` above its top edge, lined up with its `side` edge, and kept inside the monitor's `work` area. A panel wider than
/// the work area is narrowed to it; one taller than the room above the island is shortened to that room (a panel never covers the
/// island, whatever the screen).
pub fn place(glass: Rect, size: (f32, f32), scale: f32, side: Side, work: Rect) -> Rect {
    let w = px(size.0, scale).clamp(1, work.width().max(1));
    let room = (glass.top - px(GAP, scale) - work.top).max(1);
    let h = px(size.1, scale).clamp(1, room);
    let bottom = glass.top - px(GAP, scale);
    let top = (bottom - h).max(work.top);
    let wanted = match side {
        Side::Left => glass.left,
        Side::Right => glass.right - w,
    };
    let left = wanted.clamp(work.left, (work.right - w).max(work.left));
    Rect::from_xywh(left, top, w, h)
}

/// What a click on a panel's button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// Nothing is open: open it.
    Open,
    /// It is the open one: close it (the button is a toggle).
    Close,
    /// Another panel is open: close that one, open this one.
    Switch,
}

pub fn toggle(open: Option<PanelId>, asked: PanelId) -> Toggle {
    match open {
        None => Toggle::Open,
        Some(o) if o == asked => Toggle::Close,
        Some(_) => Toggle::Switch,
    }
}

/// Why a panel closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseCause {
    Escape,
    /// A mouse button went down outside the panel (and outside its own island's button).
    ClickOutside,
    /// Another window took the foreground.
    ForegroundLost,
    /// The dock hid for a full-screen application.
    FullScreen,
    /// Its button was clicked again, or another panel opened.
    Button,
    /// The user chose something in it (an app was started, a power action taken): what was started takes the foreground itself.
    Chosen,
    /// The monitor, its scale or its work area changed under the open panel: the islands moved and the panel would be left behind.
    LayoutChanged,
    /// The window was asked to close (Alt+F4): the panel is put away and its window kept for the next opening.
    WindowClose,
}

impl CloseCause {
    /// Whether the window that was in front when the panel opened gets the keyboard back. Only when the user simply put the panel
    /// away: after a click elsewhere, a foreground taken by another window or a thing started from the panel, the foreground is
    /// somebody else's already (or is about to be), and taking it back would be a theft.
    pub fn restores_foreground(self) -> bool {
        matches!(self, CloseCause::Escape | CloseCause::Button | CloseCause::WindowClose)
    }
}

/// Whether the panel closes because the window in front is not the panel any more. `foreground` is the root window in front now
/// (0: none, for the moment between two windows), `been_front` whether the panel has been in front since it opened.
pub fn lost_foreground(panel: isize, foreground: isize, been_front: bool, since_open_ms: u64) -> bool {
    if foreground == 0 || foreground == panel {
        return false;
    }
    // the old window is still in front while Windows hands the foreground over
    been_front || since_open_ms >= FOREGROUND_GRACE_MS
}

/// A click outside: a mouse button went down just now (it was up at the last look), at a point that is neither on the panel nor on
/// `keep` (the panel's own island: its button toggles the panel itself, closing it here as well would reopen it at once).
pub fn outside_click(was_down: bool, down: bool, cursor: (i32, i32), panel: Rect, keep: Rect) -> bool {
    down && !was_down && !panel.contains(cursor.0, cursor.1) && !keep.contains(cursor.0, cursor.1)
}

/// The window that gets the foreground back when a panel closes: the one that had it when the panel opened, if it is still there
/// and nobody else has taken the foreground since (a click on another window is the user's choice and is left alone).
pub fn restore_to(previous: isize, foreground_now: isize, panel: isize, previous_exists: bool) -> Option<isize> {
    if previous == 0 || previous == panel || !previous_exists {
        return None;
    }
    (foreground_now == panel || foreground_now == 0).then_some(previous)
}

/// One frame of the opening or closing: `opacity` 0..=1 and the extra distance (logical px) the panel is lower than its place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub opacity: f32,
    pub drop_px: f32,
    pub done: bool,
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// The frame `elapsed_ms` after an opening (`opening`) or a closing started. With animations off it is over at once.
pub fn frame(elapsed_ms: u32, opening: bool, animate: bool) -> Frame {
    if !animate || elapsed_ms >= ANIM_MS {
        return Frame { opacity: if opening { 1.0 } else { 0.0 }, drop_px: if opening { 0.0 } else { RISE_PX }, done: true };
    }
    let e = ease_out(elapsed_ms as f32 / ANIM_MS as f32);
    if opening { Frame { opacity: e, drop_px: RISE_PX * (1.0 - e), done: false } } else { Frame { opacity: 1.0 - e, drop_px: RISE_PX * e, done: false } }
}

// ---------------------------------------------------------------- the few calls into Windows

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, HWND_TOPMOST, IsWindow, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SetWindowPos, ShowWindow};

/// Whether any mouse button is down right now.
pub fn any_button_down() -> bool {
    unsafe { [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].into_iter().any(|k| (GetAsyncKeyState(k.0 as i32) as u16 & 0x8000) != 0) }
}

/// The pointer, device px, screen coordinates.
pub fn cursor() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

/// Whether the window still exists.
pub fn exists(hwnd: isize) -> bool {
    hwnd != 0 && unsafe { IsWindow(Some(HWND(hwnd as *mut _))).as_bool() }
}

/// Moves and sizes the panel (device px) and shows it on top without activating it; the caller then brings it to the front
/// (`crate::window::bring_to_front`) so it gets the keyboard.
pub fn show_at(hwnd: isize, r: Rect) -> bool {
    unsafe {
        SetWindowPos(HWND(hwnd as *mut _), Some(HWND_TOPMOST), r.left, r.top, r.width().max(1), r.height().max(1), SWP_NOACTIVATE | SWP_SHOWWINDOW).is_ok()
    }
}

/// Gives the foreground to `hwnd` (the window that had it before the panel): this process had it a moment ago, so Windows allows it.
pub fn give_back(hwnd: isize) {
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    if exists(hwnd) {
        unsafe {
            let _ = SetForegroundWindow(HWND(hwnd as *mut _));
        }
    }
}

/// Hides the panel.
pub fn hide(hwnd: isize) {
    unsafe {
        let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1920x1080 monitor at 100 % with the 48 px Windows taskbar: the left island's glass is 330x48 at (12, 974).
    fn left_glass() -> Rect {
        Rect::from_xywh(12, 1032 - 10 - 48, 330, 48)
    }

    #[test]
    fn a_panel_sits_eight_pixels_above_the_glass_lined_up_with_its_left_edge() {
        let work = Rect::new(0, 0, 1920, 1032);
        let r = place(left_glass(), (620.0, 680.0), 1.0, Side::Left, work);
        assert_eq!((r.width(), r.height()), (620, 680));
        assert_eq!(r.left, 12, "the island's left edge");
        assert_eq!(r.bottom, left_glass().top - 8, "8 px above the glass");
        assert!(work.covers(&r));
    }

    #[test]
    fn a_panel_of_the_right_island_lines_up_with_its_right_edge() {
        let work = Rect::new(0, 0, 1920, 1032);
        let glass = Rect::from_xywh(1920 - 12 - 280, 974, 280, 48);
        let r = place(glass, (400.0, 500.0), 1.0, Side::Right, work);
        assert_eq!(r.right, glass.right);
        assert_eq!(r.bottom, glass.top - 8);
        assert!(work.covers(&r));
    }

    #[test]
    fn at_150_percent_the_numbers_scale_and_round_once_each() {
        // 2880x1620 at 150 %, taskbar 72 px: the glass is 495x72 at (18, 1548 - 15 - 72)
        let work = Rect::new(0, 0, 2880, 1548);
        let glass = Rect::from_xywh(18, 1548 - 15 - 72, 495, 72);
        let r = place(glass, (620.0, 680.0), 1.5, Side::Left, work);
        assert_eq!((r.width(), r.height()), (930, 1020));
        assert_eq!(r.left, 18);
        assert_eq!(r.bottom, glass.top - 12, "8 px * 1.5");
        assert!(work.covers(&r));
    }

    #[test]
    fn on_a_monitor_with_a_negative_origin_the_panel_stays_on_that_monitor() {
        // a primary monitor that is to the left of another: its coordinates are negative
        let work = Rect::new(-2560, -200, 0, 1240);
        let glass = Rect::from_xywh(-2560 + 12, 1240 - 10 - 48, 330, 48);
        let r = place(glass, (620.0, 680.0), 1.0, Side::Left, work);
        assert_eq!(r.left, -2560 + 12);
        assert_eq!(r.bottom, glass.top - 8);
        assert!(work.covers(&r), "{r:?}");
        let right = Rect::from_xywh(-12 - 280, 1240 - 10 - 48, 280, 48);
        let r = place(right, (400.0, 500.0), 1.0, Side::Right, work);
        assert_eq!(r.right, right.right);
        assert!(work.covers(&r), "{r:?}");
    }

    #[test]
    fn a_panel_never_leaves_the_work_area_nor_covers_the_island() {
        let work = Rect::new(0, 0, 800, 600);
        // wider than the screen: narrowed
        let r = place(Rect::from_xywh(12, 520, 300, 48), (1000.0, 300.0), 1.0, Side::Left, work);
        assert_eq!((r.left, r.width()), (0, 800));
        // an island near the right edge with a panel lined up on its left edge would stick out: it is moved in
        let r = place(Rect::from_xywh(600, 520, 190, 48), (400.0, 300.0), 1.0, Side::Left, work);
        assert_eq!(r.right, 800);
        // taller than the room above the island: shortened to it, the gap kept
        let r = place(Rect::from_xywh(12, 520, 300, 48), (400.0, 900.0), 1.0, Side::Left, work);
        assert_eq!(r.top, 0);
        assert_eq!(r.bottom, 512);
        // an island at the very top of the work area leaves no room: a sliver, not a negative size
        let r = place(Rect::from_xywh(12, 2, 300, 48), (400.0, 300.0), 1.0, Side::Left, work);
        assert!(r.width() >= 1 && r.height() >= 1);
    }

    #[test]
    fn the_button_is_a_toggle_and_only_one_panel_is_open() {
        let other = PanelId::CONTROL_CENTRE;
        assert_eq!(toggle(None, PanelId::START), Toggle::Open);
        assert_eq!(toggle(Some(PanelId::START), PanelId::START), Toggle::Close);
        assert_eq!(toggle(Some(other), PanelId::START), Toggle::Switch);
        assert_eq!(toggle(Some(PanelId::START), other), Toggle::Switch);
    }

    #[test]
    fn a_panel_closes_when_another_window_takes_the_foreground_but_not_while_it_is_getting_it() {
        let (panel, other) = (0x100, 0x200);
        // still the old window in front just after opening: not a loss
        assert!(!lost_foreground(panel, other, false, 100));
        // the grace is over and the panel never got the foreground (it was refused): closing it would be a loop of opening and closing
        // nothing; it closes, the next click opens it again
        assert!(lost_foreground(panel, other, false, FOREGROUND_GRACE_MS));
        // it was in front and now another window is: closed at once
        assert!(lost_foreground(panel, other, true, 10));
        // the panel itself, or nobody (between two windows): stays
        assert!(!lost_foreground(panel, panel, true, 5_000));
        assert!(!lost_foreground(panel, 0, true, 5_000));
    }

    #[test]
    fn a_click_outside_is_a_button_going_down_away_from_the_panel_and_its_own_island() {
        let panel = Rect::new(0, 100, 600, 700);
        let island = Rect::new(0, 800, 330, 848);
        let outside = (900, 400);
        assert!(outside_click(false, true, outside, panel, island));
        // held since before (a drag that began elsewhere): not a new press
        assert!(!outside_click(true, true, outside, panel, island));
        assert!(!outside_click(false, false, outside, panel, island));
        // on the panel, or on the island's own button (that click toggles the panel)
        assert!(!outside_click(false, true, (300, 400), panel, island));
        assert!(!outside_click(false, true, (100, 820), panel, island));
        // on another island: outside
        assert!(outside_click(false, true, (1000, 820), panel, island));
    }

    #[test]
    fn the_keyboard_goes_back_only_after_the_user_simply_put_the_panel_away() {
        assert!(CloseCause::Escape.restores_foreground());
        assert!(CloseCause::Button.restores_foreground());
        assert!(CloseCause::WindowClose.restores_foreground());
        for c in [CloseCause::ClickOutside, CloseCause::ForegroundLost, CloseCause::FullScreen, CloseCause::Chosen, CloseCause::LayoutChanged] {
            assert!(!c.restores_foreground(), "{c:?}");
        }
    }

    #[test]
    fn the_keyboard_goes_back_only_when_nobody_else_took_it() {
        let (prev, panel) = (0x10, 0x20);
        assert_eq!(restore_to(prev, panel, panel, true), Some(prev), "the panel still has it: give it back");
        assert_eq!(restore_to(prev, 0, panel, true), Some(prev), "nobody has it");
        assert_eq!(restore_to(prev, 0x30, panel, true), None, "the user clicked another window");
        assert_eq!(restore_to(prev, panel, panel, false), None, "the old window is gone");
        assert_eq!(restore_to(0, panel, panel, true), None, "there was none");
        assert_eq!(restore_to(panel, panel, panel, true), None, "never to itself");
    }

    #[test]
    fn opening_and_closing_ease_over_120_ms_and_end_on_the_resting_state() {
        let start = frame(0, true, true);
        assert_eq!((start.opacity, start.drop_px, start.done), (0.0, RISE_PX, false));
        let mid = frame(60, true, true);
        assert!(mid.opacity > 0.5 && mid.opacity < 1.0, "ease-out is ahead of linear: {mid:?}");
        assert!(mid.drop_px < RISE_PX / 2.0);
        let end = frame(ANIM_MS, true, true);
        assert_eq!((end.opacity, end.drop_px, end.done), (1.0, 0.0, true));
        // closing is the way back
        let c0 = frame(0, false, true);
        assert_eq!((c0.opacity, c0.drop_px, c0.done), (1.0, 0.0, false));
        let c1 = frame(ANIM_MS + 50, false, true);
        assert_eq!((c1.opacity, c1.drop_px, c1.done), (0.0, RISE_PX, true));
        // monotone
        let mut last = 0.0;
        for t in 0..=ANIM_MS {
            let f = frame(t, true, true);
            assert!(f.opacity >= last);
            last = f.opacity;
        }
    }

    #[test]
    fn with_animations_off_it_is_over_at_once() {
        assert_eq!(frame(0, true, false), Frame { opacity: 1.0, drop_px: 0.0, done: true });
        assert_eq!(frame(0, false, false), Frame { opacity: 0.0, drop_px: RISE_PX, done: true });
    }

    #[test]
    fn a_window_that_is_not_there_is_left_alone() {
        assert!(!exists(0));
        assert!(!show_at(0, Rect::from_xywh(0, 0, 10, 10)));
        hide(0);
        give_back(0);
    }
}
