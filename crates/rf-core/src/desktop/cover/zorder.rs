//! Where a cover window may sit in the z-order. Pure: the window procedure feeds it what it sees and carries out the verdict.
//!
//! THE RULE. Our desktop windows sit DIRECTLY ABOVE the shell's desktop window (`Progman`, which is Explorer's while Explorer is the
//! shell) and BELOW every other window: Explorer's desktop is covered, every program window is above ours. Nothing may be allowed to
//! change that, and a lot tries: a click activates the window and Windows then raises it (`SetWindowPos(HWND_TOP)`), a program asks
//! for `HWND_TOPMOST`, our own code places the window. So the rule is applied where all of them pass: in `WM_WINDOWPOSCHANGING`, by
//! rewriting `WINDOWPOS::hwndInsertAfter` before the move happens. A window that "can never rise above an ordinary window" needs no
//! timer and no polling; what remains is the case where the DESKTOP moves (Show Desktop raises `Progman` above everything and our
//! windows are left under it): [`is_in_place`] tells the foreground-change handler whether to put them back.
//!
//! `SetWindowPos(hwnd, X, ...)` puts `hwnd` BEHIND `X`. "Directly above the desktop" therefore means "after the window that is now
//! directly above the desktop" - not "after the desktop", which would put us under it (research note 4, 2.3 says "insert after
//! Progman"; Cairo, which it cites, uses the window above the lowest desktop window, as here).

use super::Hwnd;

/// The special values `hwndInsertAfter` can have.
pub const HWND_TOP: Hwnd = 0;
pub const HWND_BOTTOM: Hwnd = 1;
pub const HWND_TOPMOST: Hwnd = -1;
pub const HWND_NOTOPMOST: Hwnd = -2;

/// `WINDOWPOS::flags` bits that matter here.
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_HIDEWINDOW: u32 = 0x0080;

/// The window directly above the desktop window in the z-order (`GetWindow(desktop, GW_HWNDPREV)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Neighbour {
    pub hwnd: Hwnd,
    pub topmost: bool,
}

/// What the shell's desktop looks like in the z-order right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Desktop {
    /// The shell's desktop window (`GetShellWindow()`); `None`: there is none (no shell is running).
    pub window: Option<Hwnd>,
    /// The window directly above it, which may be ours or a hidden one; `None`: nothing is above it.
    pub above: Option<Neighbour>,
}

/// A `WM_WINDOWPOSCHANGING` for one of our windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PosChange {
    pub me: Hwnd,
    /// Our window has the topmost style at this moment (a pop-up of the UI toolkit starts that way).
    pub me_topmost: bool,
    /// `WINDOWPOS::flags`.
    pub flags: u32,
    /// Show Desktop is on: the desktop is meant to be above the windows that could not be minimized.
    pub raised: bool,
}

/// What the window procedure does with the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Let it through as it is.
    Keep,
    /// Let it through but add `SWP_NOZORDER`: the window is already where it belongs, whatever order was asked for.
    KeepOrder,
    /// Let it through with `hwndInsertAfter` replaced by this.
    InsertAfter(Hwnd),
}

/// Where the window belongs: the `hwndInsertAfter` that puts it there, or `None` when it is already there.
pub fn target(me: Hwnd, me_topmost: bool, desktop: &Desktop, raised: bool) -> Option<Hwnd> {
    if raised {
        return Some(HWND_TOP);
    }
    if desktop.window.is_none() {
        // no shell window to stay above: the bottom of everything
        return Some(HWND_BOTTOM);
    }
    match desktop.above {
        // nothing above the desktop: it is at the top of the order, and so are we, directly over it
        None => Some(if me_topmost { HWND_NOTOPMOST } else { HWND_TOP }),
        Some(n) if n.hwnd == me && !me_topmost => None,
        // Behind a topmost window would make us topmost. The top of the non-topmost windows is directly above the desktop here, because
        // the desktop is the first non-topmost window. How to get there depends on what we are now: HWND_NOTOPMOST takes the topmost
        // style off a topmost window, but "has no effect if the window is already a non-topmost window" (SetWindowPos), so for an
        // ordinary window it would leave us under the desktop after Show Desktop; HWND_TOP is what moves such a window.
        Some(n) if n.topmost => Some(if me_topmost { HWND_NOTOPMOST } else { HWND_TOP }),
        Some(n) => Some(n.hwnd),
    }
}

/// The window in front, as far as the end of Show Desktop is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Foreground {
    /// One of our own windows (the dock, a panel, the desktop itself).
    pub ours: bool,
    /// The desktop, the taskbar or another piece of the shell.
    pub shell: bool,
    pub minimized: bool,
}

/// How long after Show Desktop a change of the foreground is not read as "the user went back to work": minimizing the programs moves
/// the foreground through windows that are about to go, and a program that is slow to answer may stay in front for a moment.
pub const RAISE_GRACE_MS: u64 = 1_000;

/// Whether Show Desktop (the covers above the programs) is over. Explorer ends its own show-desktop state when a window is
/// activated; the same rule here: a program (not ours, not the shell, not minimized) came to the front. Without it the covers would
/// stay on top of a window the user opened from the dock after Show Desktop. `fg`: `None` when nothing is in front or it could not be
/// read. `ms_since_raise`: how long ago the covers were raised.
pub fn raise_ends(fg: Option<Foreground>, ms_since_raise: u64) -> bool {
    ms_since_raise >= RAISE_GRACE_MS && fg.is_some_and(|f| !f.ours && !f.shell && !f.minimized)
}

/// The verdict for one `WM_WINDOWPOSCHANGING`.
pub fn on_pos_changing(c: &PosChange, desktop: &Desktop) -> Verdict {
    // a move or resize that asks nothing of the order, and a window being hidden, are none of our business
    if c.flags & (SWP_NOZORDER | SWP_HIDEWINDOW) != 0 {
        return Verdict::Keep;
    }
    match target(c.me, c.me_topmost, desktop, c.raised) {
        None => Verdict::KeepOrder,
        Some(after) => Verdict::InsertAfter(after),
    }
}

/// Whether every window of `mine` stands directly above the desktop in `order`. `order`: the visible top-level windows from the top
/// of the z-order to the bottom (what `EnumWindows` gives, hidden ones left out). A window of ours that is missing from `order`
/// (hidden, or being created) is not judged. With no desktop window the windows belong at the bottom.
pub fn is_in_place(order: &[Hwnd], mine: &[Hwnd], desktop: Option<Hwnd>) -> bool {
    let present: Vec<Hwnd> = mine.iter().copied().filter(|m| order.contains(m)).collect();
    if present.is_empty() {
        return true;
    }
    let end = match desktop {
        Some(d) => match order.iter().position(|w| *w == d) {
            Some(i) => i,
            // the desktop window is not among the visible ones: nothing to judge by
            None => return true,
        },
        None => order.len(),
    };
    if present.len() > end {
        return false;
    }
    // the block right before the desktop (or at the very end) is made of our windows and nothing else
    order[end - present.len()..end].iter().all(|w| present.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Hwnd = 100;
    const OTHER: Hwnd = 101;
    const DESKTOP: Hwnd = 7;

    fn desk(above: Option<(Hwnd, bool)>) -> Desktop {
        Desktop { window: Some(DESKTOP), above: above.map(|(hwnd, topmost)| Neighbour { hwnd, topmost }) }
    }
    fn change(flags: u32, topmost: bool, raised: bool) -> PosChange {
        PosChange { me: ME, me_topmost: topmost, flags, raised }
    }

    #[test]
    fn a_window_is_put_behind_the_window_that_is_above_the_desktop_not_behind_the_desktop() {
        // an ordinary window (50) is the lowest one above the desktop: we go right behind it, and so directly above the desktop
        assert_eq!(on_pos_changing(&change(0, false, false), &desk(Some((50, false)))), Verdict::InsertAfter(50));
        // another cover window is directly above the desktop: behind that one (they share the band)
        assert_eq!(on_pos_changing(&change(0, false, false), &desk(Some((OTHER, false)))), Verdict::InsertAfter(OTHER));
    }

    #[test]
    fn a_request_to_rise_is_turned_into_staying_where_we_are() {
        // the click that activated us made Windows ask for HWND_TOP; we are already directly above the desktop
        assert_eq!(on_pos_changing(&change(0, false, false), &desk(Some((ME, false)))), Verdict::KeepOrder);
    }

    #[test]
    fn a_request_that_does_not_touch_the_order_or_hides_the_window_passes() {
        assert_eq!(on_pos_changing(&change(SWP_NOZORDER, false, false), &desk(Some((50, false)))), Verdict::Keep);
        assert_eq!(on_pos_changing(&change(SWP_HIDEWINDOW, false, false), &desk(Some((50, false)))), Verdict::Keep);
        assert_eq!(on_pos_changing(&change(SWP_NOZORDER | 0x0010, true, false), &desk(None)), Verdict::Keep);
    }

    #[test]
    fn behind_a_topmost_neighbour_the_window_must_not_become_topmost() {
        // only the taskbar (topmost) is above the desktop: "behind the taskbar" would make us topmost, and HWND_NOTOPMOST would do
        // nothing for a window that is not topmost; HWND_TOP is the top of the non-topmost band, which is right above the desktop
        assert_eq!(on_pos_changing(&change(0, false, false), &desk(Some((50, true)))), Verdict::InsertAfter(HWND_TOP));
    }

    #[test]
    fn a_window_that_starts_topmost_is_brought_down_even_when_it_is_the_neighbour() {
        assert_eq!(on_pos_changing(&change(0, true, false), &desk(Some((ME, true)))), Verdict::InsertAfter(HWND_NOTOPMOST));
        assert_eq!(on_pos_changing(&change(0, true, false), &desk(Some((50, false)))), Verdict::InsertAfter(50), "behind a non-topmost window the topmost style goes");
    }

    #[test]
    fn nothing_above_the_desktop_means_the_top() {
        assert_eq!(on_pos_changing(&change(0, false, false), &desk(None)), Verdict::InsertAfter(HWND_TOP));
    }

    #[test]
    fn without_a_shell_window_the_bottom() {
        assert_eq!(on_pos_changing(&change(0, false, false), &Desktop { window: None, above: None }), Verdict::InsertAfter(HWND_BOTTOM));
    }

    #[test]
    fn while_the_desktop_is_shown_the_window_goes_to_the_top() {
        assert_eq!(on_pos_changing(&change(0, false, true), &desk(Some((50, false)))), Verdict::InsertAfter(HWND_TOP));
    }

    #[test]
    fn show_desktop_ends_when_a_program_comes_to_the_front_and_not_before_or_for_anything_else() {
        let program = Foreground { ours: false, shell: false, minimized: false };
        assert!(raise_ends(Some(program), RAISE_GRACE_MS), "a program is in front: the user is back at work");
        assert!(!raise_ends(Some(program), RAISE_GRACE_MS - 1), "inside the grace period the foreground is still settling");
        assert!(!raise_ends(Some(Foreground { shell: true, ..program }), 60_000), "the desktop or the taskbar in front is Show Desktop itself");
        assert!(!raise_ends(Some(Foreground { ours: true, ..program }), 60_000), "the dock or a panel of ours in front does not end it");
        assert!(!raise_ends(Some(Foreground { minimized: true, ..program }), 60_000), "a window that is minimized is not in front");
        assert!(!raise_ends(None, 60_000), "nothing in front, or unreadable: stay as we are");
    }

    #[test]
    fn the_band_check() {
        let mine = [11, 12];
        // [taskbar, app, app, ours, ours, desktop]
        assert!(is_in_place(&[1, 50, 51, 11, 12, DESKTOP], &mine, Some(DESKTOP)));
        assert!(is_in_place(&[1, 50, 51, 12, 11, DESKTOP], &mine, Some(DESKTOP)), "their order among themselves does not matter");
        // one of ours has risen above an app
        assert!(!is_in_place(&[1, 11, 50, 51, 12, DESKTOP], &mine, Some(DESKTOP)));
        // the desktop was raised (Show Desktop)
        assert!(!is_in_place(&[1, DESKTOP, 11, 12], &mine, Some(DESKTOP)));
        // an app slipped between ours and the desktop
        assert!(!is_in_place(&[1, 11, 12, 50, DESKTOP], &mine, Some(DESKTOP)));
        // not at the top of the stack at all: nothing above the desktop to be wrong about
        assert!(is_in_place(&[11, 12, DESKTOP, 50], &mine, Some(DESKTOP)));
    }

    #[test]
    fn the_band_check_leaves_out_what_is_not_there_and_has_no_opinion_without_a_desktop_window_in_the_list() {
        assert!(is_in_place(&[1, 50], &[11], Some(DESKTOP)), "none of ours is visible");
        assert!(is_in_place(&[1, 50, 11, 12], &[11, 12], Some(DESKTOP)), "the desktop window is not among the visible ones");
        assert!(is_in_place(&[1, 11, DESKTOP], &[11, 12], Some(DESKTOP)), "a window of ours that is hidden is not judged");
        // with no desktop window the band is the bottom of the order
        assert!(is_in_place(&[1, 50, 11, 12], &[11, 12], None));
        assert!(!is_in_place(&[1, 11, 12, 50], &[11, 12], None));
    }

    // ---- a model of the z-order, to see that the rule holds up when the desktop and the windows move ----

    #[derive(Clone, Debug)]
    struct Stack {
        /// top to bottom, with the topmost flag
        w: Vec<(Hwnd, bool)>,
    }

    impl Stack {
        fn order(&self) -> Vec<Hwnd> {
            self.w.iter().map(|(h, _)| *h).collect()
        }
        fn first_non_topmost(&self) -> usize {
            self.w.iter().position(|(_, t)| !*t).unwrap_or(self.w.len())
        }
        fn desktop_view(&self) -> Desktop {
            let i = self.w.iter().position(|(h, _)| *h == DESKTOP);
            Desktop { window: i.map(|_| DESKTOP), above: i.and_then(|i| i.checked_sub(1)).map(|j| Neighbour { hwnd: self.w[j].0, topmost: self.w[j].1 }) }
        }
        /// What `SetWindowPos(hwnd, after)` does to the order.
        fn set_pos(&mut self, h: Hwnd, after: Hwnd) {
            let i = self.w.iter().position(|(x, _)| *x == h).unwrap();
            let was_topmost = self.w[i].1;
            // SetWindowPos: HWND_NOTOPMOST "has no effect if the window is already a non-topmost window" (it does not reorder it)
            if after == HWND_NOTOPMOST && !was_topmost {
                return;
            }
            self.w.remove(i);
            match after {
                HWND_TOP => {
                    let at = if was_topmost { 0 } else { self.first_non_topmost() };
                    self.w.insert(at, (h, was_topmost));
                }
                HWND_BOTTOM => self.w.push((h, false)),
                HWND_TOPMOST => self.w.insert(0, (h, true)),
                HWND_NOTOPMOST => {
                    let at = self.first_non_topmost();
                    self.w.insert(at, (h, false));
                }
                x => {
                    let j = self.w.iter().position(|(y, _)| *y == x).unwrap();
                    let topmost = self.w[j].1;
                    self.w.insert(j + 1, (h, topmost));
                }
            }
        }
        /// A `SetWindowPos(me, requested)` on one of OUR windows, as the window procedure sees it.
        fn ask(&mut self, me: Hwnd, requested: Hwnd, raised: bool) {
            let me_topmost = self.w.iter().find(|(h, _)| *h == me).unwrap().1;
            let c = PosChange { me, me_topmost, flags: 0, raised };
            match on_pos_changing(&c, &self.desktop_view()) {
                Verdict::Keep => self.set_pos(me, requested),
                Verdict::KeepOrder => {}
                Verdict::InsertAfter(a) => self.set_pos(me, a),
            }
        }
    }

    fn start() -> Stack {
        // the taskbar (topmost), two program windows, the desktop; our two windows were just made, TOPMOST (a pop-up's default)
        Stack { w: vec![(1, true), (11, true), (12, true), (50, false), (51, false), (DESKTOP, false)] }
    }

    #[test]
    fn two_new_topmost_pop_ups_end_up_directly_above_the_desktop_and_below_the_programs() {
        let mut s = start();
        s.ask(11, HWND_TOP, false);
        s.ask(12, HWND_TOP, false);
        assert_eq!(s.order(), [1, 50, 51, 11, 12, DESKTOP]);
        let order = s.order();
        assert!(is_in_place(&order, &[11, 12], Some(DESKTOP)), "{order:?}");
        assert_eq!(order[0], 1, "the taskbar is still on top");
        assert!(s.w.iter().all(|(h, t)| !(*h == 11 || *h == 12) || !*t), "neither is topmost any more");
    }

    #[test]
    fn nothing_asked_of_the_window_can_raise_it_above_a_program() {
        let mut s = start();
        s.ask(11, HWND_TOP, false);
        s.ask(12, HWND_TOP, false);
        for requested in [HWND_TOP, HWND_TOPMOST, HWND_NOTOPMOST, 50, 51, 1] {
            s.ask(11, requested, false);
            s.ask(12, requested, false);
            assert!(is_in_place(&s.order(), &[11, 12], Some(DESKTOP)), "after asking for {requested}: {:?}", s.order());
        }
    }

    #[test]
    fn show_desktop_leaves_the_windows_under_the_desktop_and_one_look_puts_them_back() {
        let mut s = start();
        s.ask(11, HWND_TOP, false);
        s.ask(12, HWND_TOP, false);
        // Explorer minimized the programs and raised the desktop to the top of the non-topmost windows
        s.w.retain(|(h, _)| *h != 50 && *h != 51);
        s.set_pos(DESKTOP, HWND_TOP);
        assert_eq!(s.order(), [1, DESKTOP, 11, 12]);
        assert!(!is_in_place(&s.order(), &[11, 12], Some(DESKTOP)));
        // the foreground handler re-inserts both. The only window above the desktop is the (topmost) taskbar, which is the verdict
        // that asks for HWND_NOTOPMOST when it should ask for HWND_TOP: Windows ignores NOTOPMOST on a window that is not topmost.
        s.ask(11, HWND_TOP, false);
        s.ask(12, HWND_TOP, false);
        assert_eq!(s.order(), [1, 11, 12, DESKTOP]);
        assert!(is_in_place(&s.order(), &[11, 12], Some(DESKTOP)));
    }

    #[test]
    fn a_window_that_is_not_topmost_is_never_sent_to_notopmost_because_windows_ignores_that() {
        // a topmost neighbour only (the taskbar), or nothing above the desktop, and we are an ordinary window: the top of the
        // non-topmost band is directly above the desktop here
        assert_eq!(target(ME, false, &desk(Some((50, true))), false), Some(HWND_TOP));
        assert_eq!(target(ME, false, &desk(None), false), Some(HWND_TOP));
        // a topmost window of ours does need NOTOPMOST, which takes its topmost style off
        assert_eq!(target(ME, true, &desk(Some((50, true))), false), Some(HWND_NOTOPMOST));
        assert_eq!(target(ME, true, &desk(None), false), Some(HWND_NOTOPMOST));
    }

    #[test]
    fn the_model_does_what_windows_does_with_notopmost() {
        let mut s = Stack { w: vec![(1, true), (50, false), (11, false), (DESKTOP, false)] };
        s.set_pos(11, HWND_NOTOPMOST);
        assert_eq!(s.order(), [1, 50, 11, DESKTOP], "no effect on a window that is not topmost");
        let mut t = Stack { w: vec![(1, true), (11, true), (50, false), (DESKTOP, false)] };
        t.set_pos(11, HWND_NOTOPMOST);
        assert_eq!(t.order(), [1, 11, 50, DESKTOP], "a topmost window becomes non-topmost at the top of the band");
        assert!(!t.w[1].1);
    }

    #[test]
    fn when_the_desktop_goes_back_down_the_programs_that_come_back_are_above_ours() {
        let mut s = Stack { w: vec![(1, true), (11, false), (12, false), (DESKTOP, false)] };
        // the programs are restored: each comes to the top of the non-topmost windows
        s.w.insert(1, (50, false));
        s.w.insert(1, (51, false));
        assert!(is_in_place(&s.order(), &[11, 12], Some(DESKTOP)));
        // and one of ours is wrongly above a program (say the desktop went down after a raise and ours stayed up)
        let mut t = Stack { w: vec![(1, true), (11, false), (50, false), (12, false), (DESKTOP, false)] };
        assert!(!is_in_place(&t.order(), &[11, 12], Some(DESKTOP)));
        t.ask(11, HWND_TOP, false);
        assert_eq!(t.order(), [1, 50, 12, 11, DESKTOP], "behind the window that is above the desktop: 12, which is ours");
        t.ask(12, HWND_TOP, false);
        assert!(is_in_place(&t.order(), &[11, 12], Some(DESKTOP)), "{:?}", t.order());
    }

    #[test]
    fn while_the_desktop_is_shown_a_request_to_rise_is_honoured_and_afterwards_the_band_comes_back() {
        let mut s = start();
        s.ask(11, HWND_TOP, false);
        s.ask(12, HWND_TOP, false);
        s.ask(11, HWND_TOP, true);
        s.ask(12, HWND_TOP, true);
        assert_eq!(s.order()[1..3], [12, 11], "above the programs that could not be minimized");
        s.ask(11, HWND_TOP, false);
        s.ask(12, HWND_TOP, false);
        assert!(is_in_place(&s.order(), &[11, 12], Some(DESKTOP)), "{:?}", s.order());
    }
}
