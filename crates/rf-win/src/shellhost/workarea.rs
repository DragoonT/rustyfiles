//! The work area, set by the host itself, for a host that runs with no shell at all.
//!
//! BESIDE EXPLORER the band is Explorer's to give: `ABM_SETPOS` makes Explorer shrink the work area (`dock/band.rs`). With nobody
//! owning the shell window nobody answers those messages, so maximised windows would go under the dock. A shell has the other way: it
//! sets the work area itself with `SystemParametersInfo(SPI_SETWORKAREA)` - "the function sets the work area of the monitor that
//! contains the specified rectangle" (documentation) - to the monitor less its own band. That is what [`WorkAreaKeeper`] decides and
//! [`SystemSetter`] does.
//!
//! IT CHANGES A SYSTEM-WIDE SETTING, so there are three locks and each is tested:
//!
//! 1. [`WorkAreaKeeper::apply`] looks at the shell window AGAIN, through [`ShellState`], immediately before it would set anything, and
//!    does nothing unless nobody owns it ([`Shell::Nobody`]). Explorer owning it, or another shell, is a no-op for every input.
//! 2. [`SystemSetter::set`] looks again before the call to Windows and refuses unless nobody owns the shell window.
//! 3. The caller (`rf-app` `shell/live.rs`) only builds a keeper when the environment variable `RUSTYFILES_SET_WORKAREA=1` is set: there
//!    is no setting for it, and a host that nobody asked changes nothing.
//!
//! The value is NOT written to the profile (`SPIF_UPDATEINIFILE` is not used): it lasts until the session ends or something sets it
//! again, and Explorer resets it by itself when it comes back. When the band is given back while there is still no shell, the work
//! area that was there before is put back ([`WorkAreaKeeper::apply`] with no thickness).
//!
//! THE REAL PATH WAS NOT RUN in the increment that wrote this (the test machine has Explorer; the tests only show the rule [`permitted`] that
//! [`SystemSetter`] asks before it calls Windows, and the keeper on a fake).

use super::appcmd::{Shell, shell_now};
use crate::dock::layout::Rect;

/// Who owns the shell window, looked at fresh.
pub trait ShellState {
    fn shell(&self) -> Shell;
}

/// The real look.
pub struct SystemShell;

impl ShellState for SystemShell {
    fn shell(&self) -> Shell {
        shell_now()
    }
}

/// Sets the work area of the monitor that contains `rc`. True: Windows took it.
pub trait WorkAreaSetter {
    fn set(&mut self, rc: Rect) -> bool;
}

/// What one [`WorkAreaKeeper::apply`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Done {
    /// Nothing to do (or a lock held it back); see [`Held`].
    Nothing,
    /// The work area was set to this.
    Set(Rect),
    /// The work area from before was put back.
    Restored(Rect),
}

/// Why nothing was done, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// The shell window has an owner: its work area is its own business.
    ShellPresent(Shell),
    /// Windows refused the call.
    Refused,
    /// Already as wanted.
    AsWanted,
}

#[derive(Default)]
pub struct WorkAreaKeeper {
    /// The work area before the first set, to put back.
    before: Option<Rect>,
    /// What was set last.
    applied: Option<Rect>,
    last_held: Option<Held>,
}

/// The work area for a band of `thickness` px at the bottom of `monitor`.
pub fn target(monitor: Rect, thickness: i32) -> Rect {
    Rect { bottom: (monitor.bottom - thickness.max(1)).max(monitor.top + 1), ..monitor }
}

impl WorkAreaKeeper {
    pub fn new() -> WorkAreaKeeper {
        WorkAreaKeeper::default()
    }

    /// Why the last call did nothing.
    pub fn held(&self) -> Option<Held> {
        self.last_held
    }

    /// Whether this keeper has changed the work area and not yet put it back.
    pub fn active(&self) -> bool {
        self.applied.is_some()
    }

    /// Brings the work area in line: `thickness` Some = the monitor less the band; None = put back what was there. `work`: the work area
    /// of `monitor` now.
    pub fn apply(&mut self, state: &dyn ShellState, setter: &mut dyn WorkAreaSetter, thickness: Option<i32>, monitor: Rect, work: Rect) -> Done {
        // lock 1: the shell is looked at now, not remembered
        let shell = state.shell();
        if shell != Shell::Nobody {
            // (Explorer has come back or is there: it sets the work area itself. Whatever we changed is its to redo; we forget it.)
            self.before = None;
            self.applied = None;
            self.last_held = Some(Held::ShellPresent(shell));
            return Done::Nothing;
        }
        match thickness {
            Some(t) => {
                let want = target(monitor, t);
                if work == want {
                    self.last_held = Some(Held::AsWanted);
                    return Done::Nothing;
                }
                if self.before.is_none() {
                    self.before = Some(work);
                }
                if setter.set(want) {
                    self.applied = Some(want);
                    self.last_held = None;
                    Done::Set(want)
                } else {
                    self.last_held = Some(Held::Refused);
                    Done::Nothing
                }
            }
            None => {
                let Some(back) = self.before.take() else { return Done::Nothing };
                self.applied = None;
                if setter.set(back) {
                    self.last_held = None;
                    Done::Restored(back)
                } else {
                    self.last_held = Some(Held::Refused);
                    Done::Nothing
                }
            }
        }
    }
}

/// Whether the real call may be made for the shell window as it is now.
pub fn permitted(shell: Shell) -> bool {
    shell == Shell::Nobody
}

/// The real setter: `SystemParametersInfoW(SPI_SETWORKAREA)` with a broadcast of the change, not written to the profile.
pub struct SystemSetter;

impl WorkAreaSetter for SystemSetter {
    fn set(&mut self, rc: Rect) -> bool {
        // lock 2: looked at again right before the call
        if !permitted(shell_now()) {
            log::warn!("work area: refused, the shell window has an owner");
            return false;
        }
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{SPI_SETWORKAREA, SPIF_SENDCHANGE, SystemParametersInfoW};
        let mut r = RECT { left: rc.left, top: rc.top, right: rc.right, bottom: rc.bottom };
        let ok = unsafe { SystemParametersInfoW(SPI_SETWORKAREA, 0, Some(&mut r as *mut RECT as *mut _), SPIF_SENDCHANGE) }.is_ok();
        if !ok {
            log::warn!("work area: Windows did not take {rc:?}");
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const MON: Rect = Rect::new(0, 0, 2560, 1440);
    const FULL: Rect = MON;

    struct State(Cell<Shell>);
    impl ShellState for State {
        fn shell(&self) -> Shell {
            self.0.get()
        }
    }

    #[derive(Default)]
    struct Setter {
        calls: Vec<Rect>,
        refuse: bool,
    }
    impl WorkAreaSetter for Setter {
        fn set(&mut self, rc: Rect) -> bool {
            self.calls.push(rc);
            !self.refuse
        }
    }

    #[test]
    fn the_target_is_the_monitor_less_the_band_at_the_bottom() {
        assert_eq!(target(MON, 78), Rect::new(0, 0, 2560, 1362));
        assert_eq!(target(MON, 0), Rect::new(0, 0, 2560, 1439), "a band is at least one pixel");
        assert_eq!(target(Rect::new(0, 0, 100, 50), 500), Rect::new(0, 0, 100, 1), "never an empty or inverted work area");
    }

    #[test]
    fn with_nobody_owning_the_shell_window_the_band_is_taken_and_given_back() {
        let (st, mut s, mut k) = (State(Cell::new(Shell::Nobody)), Setter::default(), WorkAreaKeeper::new());
        assert_eq!(k.apply(&st, &mut s, Some(78), MON, FULL), Done::Set(Rect::new(0, 0, 2560, 1362)));
        assert!(k.active());
        // already as wanted: nothing is sent again
        assert_eq!(k.apply(&st, &mut s, Some(78), MON, Rect::new(0, 0, 2560, 1362)), Done::Nothing);
        assert_eq!(k.held(), Some(Held::AsWanted));
        assert_eq!(s.calls.len(), 1);
        // the host stops (or the band is switched off) with still no shell: what was there comes back
        assert_eq!(k.apply(&st, &mut s, None, MON, Rect::new(0, 0, 2560, 1362)), Done::Restored(FULL));
        assert!(!k.active());
        assert_eq!(k.apply(&st, &mut s, None, MON, FULL), Done::Nothing, "only once");
    }

    #[test]
    fn it_cannot_be_reached_while_explorer_owns_the_shell_window() {
        for shell in [Shell::Explorer, Shell::Foreign] {
            let (st, mut s, mut k) = (State(Cell::new(shell)), Setter::default(), WorkAreaKeeper::new());
            for t in [Some(1), Some(78), Some(5000), None] {
                for work in [FULL, Rect::new(0, 0, 2560, 1392), target(MON, 78)] {
                    assert_eq!(k.apply(&st, &mut s, t, MON, work), Done::Nothing);
                }
            }
            assert!(s.calls.is_empty(), "{shell:?}: the setter was called");
            assert_eq!(k.held(), Some(Held::ShellPresent(shell)));
        }
    }

    #[test]
    fn explorer_coming_back_stops_it_at_once_and_it_does_not_put_anything_back() {
        let (st, mut s, mut k) = (State(Cell::new(Shell::Nobody)), Setter::default(), WorkAreaKeeper::new());
        k.apply(&st, &mut s, Some(78), MON, FULL);
        st.0.set(Shell::Explorer);
        let calls = s.calls.len();
        assert_eq!(k.apply(&st, &mut s, Some(78), MON, target(MON, 78)), Done::Nothing);
        assert_eq!(k.apply(&st, &mut s, None, MON, target(MON, 78)), Done::Nothing);
        assert_eq!(s.calls.len(), calls, "Explorer sets the work area itself; we do not fight it");
        assert!(!k.active());
    }

    #[test]
    fn a_refusal_is_reported_and_leaves_nothing_to_put_back() {
        let (st, mut s, mut k) = (State(Cell::new(Shell::Nobody)), Setter { refuse: true, ..Default::default() }, WorkAreaKeeper::new());
        assert_eq!(k.apply(&st, &mut s, Some(78), MON, FULL), Done::Nothing);
        assert_eq!(k.held(), Some(Held::Refused));
        assert!(!k.active());
    }

    #[test]
    fn the_real_setter_only_goes_ahead_for_a_shell_window_with_no_owner() {
        // (the real call is never made here: this is the rule it asks before it calls Windows)
        assert!(permitted(Shell::Nobody));
        assert!(!permitted(Shell::Explorer));
        assert!(!permitted(Shell::Foreign));
    }
}
