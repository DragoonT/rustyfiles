//! Show Desktop and Minimize All, as our own functions. Pure bookkeeping: which windows to minimize, what to remember, what to
//! restore and in which order. The Win32 side that does the minimizing is `rf_win::desktop::showdesktop`.
//!
//! WHAT THE TWO DO (research note 4, 2.5; Raymond Chen): Minimize All (Win+M) minimizes the windows that have a minimize command.
//! Show Desktop (Win+D) does that and then raises the desktop above the windows that could not be minimized. Doing either again
//! undoes it: the windows that were minimized come back, in REVERSE z-order (the bottom one first, so the one that was on top is
//! restored last and ends on top again). Windows that appeared in between stay as they are, and a window the user has already
//! brought back by hand is not touched.

use super::Hwnd;

/// One top-level window, as the Win32 layer reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Win {
    pub hwnd: Hwnd,
    pub visible: bool,
    /// Hidden by the shell (a window on another virtual desktop, a suspended app): `IsWindowVisible` still says yes.
    pub cloaked: bool,
    /// A tool window with no taskbar button: palettes, popups.
    pub tool_window: bool,
    /// Has a minimize box (`WS_MINIMIZEBOX`), so that "minimize" means something.
    pub minimizable: bool,
    pub minimized: bool,
    /// One of our own windows: the desktop windows themselves, the dock.
    pub ours: bool,
    /// The desktop, the taskbar or another piece of the shell.
    pub shell: bool,
}

/// Whether Show Desktop minimizes the window.
pub fn is_candidate(w: &Win) -> bool {
    w.visible && !w.cloaked && !w.tool_window && w.minimizable && !w.minimized && !w.ours && !w.shell
}

/// What is true of a remembered window now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    pub hwnd: Hwnd,
    pub exists: bool,
    pub minimized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Win+D: minimize and raise the desktop.
    ShowDesktop,
    /// Win+M: minimize only.
    MinimizeAll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Minimize these (topmost first) and, for Show Desktop, raise the desktop windows above the rest.
    Minimize { windows: Vec<Hwnd>, raise_desktop: bool },
    /// Restore these in this order (bottom first) and lower the desktop windows back into their place.
    Restore { windows: Vec<Hwnd>, lower_desktop: bool },
    Nothing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Remembered {
    kind: Kind,
    /// Top of the z-order first.
    windows: Vec<Hwnd>,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    current: Option<Remembered>,
}

impl State {
    /// Something is minimized by us and not yet brought back.
    pub fn showing(&self) -> bool {
        self.current.is_some()
    }

    /// The windows remembered, top of the z-order first (for the report and the tests).
    pub fn remembered(&self) -> &[Hwnd] {
        self.current.as_ref().map_or(&[], |r| &r.windows)
    }

    /// Minimizes. `z`: the top-level windows from the top of the z-order to the bottom. Called again while windows are remembered,
    /// the new ones are added (a window that was brought back and is in the way again, or one that appeared).
    pub fn minimize(&mut self, kind: Kind, z: &[Win]) -> Action {
        let fresh: Vec<Hwnd> = z.iter().filter(|w| is_candidate(w)).map(|w| w.hwnd).collect();
        let raise_desktop = kind == Kind::ShowDesktop;
        match &mut self.current {
            Some(r) => {
                // (windows already remembered stay in the list even when they are not minimized now: undo asks the system again)
                let mut all = fresh.clone();
                all.extend(r.windows.iter().copied().filter(|h| !fresh.contains(h)));
                r.windows = all;
                if raise_desktop {
                    r.kind = Kind::ShowDesktop;
                }
            }
            None => self.current = Some(Remembered { kind, windows: fresh.clone() }),
        }
        if fresh.is_empty() && !raise_desktop {
            // nothing to minimize and nothing to raise: no state to undo either
            if self.current.as_ref().is_some_and(|r| r.windows.is_empty()) {
                self.current = None;
            }
            return Action::Nothing;
        }
        Action::Minimize { windows: fresh, raise_desktop }
    }

    /// Brings back what was minimized. `now`: what the system says about the remembered windows (one entry per window; a window
    /// not listed is taken as gone).
    pub fn undo(&mut self, now: &[Now]) -> Action {
        let Some(r) = self.current.take() else { return Action::Nothing };
        let windows: Vec<Hwnd> = r
            .windows
            .iter()
            .rev()
            .copied()
            .filter(|h| now.iter().any(|n| n.hwnd == *h && n.exists && n.minimized))
            .collect();
        Action::Restore { windows, lower_desktop: r.kind == Kind::ShowDesktop }
    }

    /// Win+D: shows the desktop, or brings the windows back when it is shown. When the user has brought every window back by hand,
    /// the next press shows the desktop again rather than "undoing" nothing.
    pub fn toggle_show_desktop(&mut self, z: &[Win], now: &[Now]) -> Action {
        match &self.current {
            Some(r) if r.kind == Kind::ShowDesktop => {
                let any_minimized = r.windows.iter().any(|h| now.iter().any(|n| n.hwnd == *h && n.exists && n.minimized));
                if any_minimized || r.windows.is_empty() {
                    return self.undo(now);
                }
                self.current = None;
                self.minimize(Kind::ShowDesktop, z)
            }
            _ => self.minimize(Kind::ShowDesktop, z),
        }
    }

    /// Win+M is Minimize All; Shift+Win+M is its undo (Windows' own pair).
    pub fn minimize_all(&mut self, z: &[Win]) -> Action {
        self.minimize(Kind::MinimizeAll, z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(h: Hwnd) -> Win {
        Win { hwnd: h, visible: true, cloaked: false, tool_window: false, minimizable: true, minimized: false, ours: false, shell: false }
    }
    fn now(list: &[(Hwnd, bool, bool)]) -> Vec<Now> {
        list.iter().map(|(hwnd, exists, minimized)| Now { hwnd: *hwnd, exists: *exists, minimized: *minimized }).collect()
    }

    #[test]
    fn only_windows_that_can_and_should_be_minimized_are() {
        let mut z = vec![win(1), win(2), win(3), win(4), win(5), win(6), win(7), win(8), win(9)];
        z[1].visible = false;
        z[2].cloaked = true;
        z[3].tool_window = true;
        z[4].minimizable = false;
        z[5].minimized = true;
        z[6].ours = true;
        z[7].shell = true;
        let mut s = State::default();
        assert_eq!(s.minimize(Kind::MinimizeAll, &z), Action::Minimize { windows: vec![1, 9], raise_desktop: false });
        assert_eq!(s.remembered(), [1, 9]);
    }

    #[test]
    fn show_desktop_minimizes_and_raises_and_the_second_press_restores_bottom_first() {
        let z = [win(10), win(20), win(30)];
        let mut s = State::default();
        assert_eq!(s.toggle_show_desktop(&z, &[]), Action::Minimize { windows: vec![10, 20, 30], raise_desktop: true });
        assert!(s.showing());
        // the second press: all three are minimized now
        let n = now(&[(10, true, true), (20, true, true), (30, true, true)]);
        assert_eq!(s.toggle_show_desktop(&[], &n), Action::Restore { windows: vec![30, 20, 10], lower_desktop: true });
        assert!(!s.showing());
        assert_eq!(s.undo(&n), Action::Nothing, "nothing left to undo");
    }

    #[test]
    fn minimize_all_does_not_raise_the_desktop_and_undo_does_not_lower_it() {
        let z = [win(10), win(20)];
        let mut s = State::default();
        assert_eq!(s.minimize_all(&z), Action::Minimize { windows: vec![10, 20], raise_desktop: false });
        let n = now(&[(10, true, true), (20, true, true)]);
        assert_eq!(s.undo(&n), Action::Restore { windows: vec![20, 10], lower_desktop: false });
    }

    #[test]
    fn a_window_the_user_brought_back_or_closed_meanwhile_is_left_alone() {
        let z = [win(10), win(20), win(30), win(40)];
        let mut s = State::default();
        s.minimize(Kind::MinimizeAll, &z);
        // 20 was restored by hand, 30 was closed, 40 is still minimized
        let n = now(&[(10, true, true), (20, true, false), (30, false, false), (40, true, true)]);
        assert_eq!(s.undo(&n), Action::Restore { windows: vec![40, 10], lower_desktop: false });
    }

    #[test]
    fn windows_that_appeared_in_between_stay_where_they_are() {
        let mut s = State::default();
        s.minimize(Kind::MinimizeAll, &[win(10), win(20)]);
        // a new window (99) opened meanwhile and is not remembered
        let n = now(&[(10, true, true), (20, true, true), (99, true, false)]);
        match s.undo(&n) {
            Action::Restore { windows, .. } => assert!(!windows.contains(&99)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn show_desktop_with_every_window_already_brought_back_shows_the_desktop_again() {
        let mut s = State::default();
        s.toggle_show_desktop(&[win(10), win(20)], &[]);
        // the user restored both by hand: the press must not "undo" nothing
        let n = now(&[(10, true, false), (20, true, false)]);
        assert_eq!(s.toggle_show_desktop(&[win(10), win(20)], &n), Action::Minimize { windows: vec![10, 20], raise_desktop: true });
        assert!(s.showing());
    }

    #[test]
    fn show_desktop_with_nothing_to_minimize_still_raises_the_desktop_and_the_next_press_lowers_it() {
        let mut s = State::default();
        assert_eq!(s.toggle_show_desktop(&[], &[]), Action::Minimize { windows: vec![], raise_desktop: true });
        assert!(s.showing());
        assert_eq!(s.toggle_show_desktop(&[], &[]), Action::Restore { windows: vec![], lower_desktop: true });
        assert!(!s.showing());
    }

    #[test]
    fn minimize_all_with_nothing_to_minimize_does_nothing_and_remembers_nothing() {
        let mut s = State::default();
        assert_eq!(s.minimize_all(&[]), Action::Nothing);
        assert!(!s.showing());
    }

    #[test]
    fn show_desktop_after_minimize_all_raises_and_restores_everything_in_one_undo() {
        let mut s = State::default();
        s.minimize_all(&[win(10), win(20)]);
        // a window (30) was opened and is in the way; Win+D now
        assert_eq!(s.toggle_show_desktop(&[win(30)], &now(&[(10, true, true), (20, true, true)])), Action::Minimize { windows: vec![30], raise_desktop: true });
        assert_eq!(s.remembered(), [30, 10, 20]);
        let n = now(&[(10, true, true), (20, true, true), (30, true, true)]);
        assert_eq!(s.undo(&n), Action::Restore { windows: vec![20, 10, 30], lower_desktop: true });
    }
}
