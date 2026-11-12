//! Minimized windows when no shell is there to hide them.
//!
//! WHAT WAS SEEN (the user's trial without Explorer): "küçülen uygulamalar sol altta küçük pencere olarak görünüyor". With no shell a
//! minimized window is drawn by Windows itself as a small title bar at the bottom left of the screen. A shell prevents that by telling
//! Windows to hide minimized windows. The documentation of `SystemParametersInfo` (`SPI_SETMINIMIZEDMETRICS`) and of `MINIMIZEDMETRICS`
//! (`iArrange`): "ARW_HIDE: hide minimized windows by moving them off the visible area of the screen". The window stays minimized
//! (`IsIconic`), stays a normal top-level window and is restored by the same calls as always (`ShowWindow(SW_RESTORE)`, the dock's
//! click); only where Windows keeps its icon changes. That is the state every window is in while Explorer runs.
//!
//! WHAT THIS FILE DOES. [`MinimizedKeeper::apply`] is the whole decision, over two small traits so that it is tested on a fake:
//!
//! * the host finds NO window owning the shell window ([`Shell::Nobody`], the same look the work area and the media keys use): the
//!   current metrics are read and remembered, and the same metrics with `ARW_HIDE` added are set;
//! * the host ends while it is still the only shell: the remembered metrics are put back, but only when the metrics are still the ones
//!   WE set (something else that changed them since is not ours to undo);
//! * a shell window appears: nothing is written. Explorer sets this value itself when it starts; writing the old value back after (or
//!   racing with) that would show the bars again beside a running Explorer. If the owner is another shell (not Explorer) and the
//!   metrics are still exactly ours, they are put back, because that shell may expect the system default.
//!
//! NOT WRITTEN TO THE PROFILE: the call passes no `SPIF_UPDATEINIFILE`, so the value lasts until the session ends or something sets it
//! again; no broadcast is asked for either. Both calls pass `WRITE_FLAGS`, and a test checks that it holds neither bit. Windows that were already minimized before the value
//! was set keep the place they have until they are restored and minimized again (re-arranging them would move other programs' windows,
//! which a first version leaves alone).
//!
//! THE REAL CALLS WERE NOT RUN (the machine this was written on has Explorer; the user's trial machine was not available): the keeper is
//! tested on a fake, and the real [`SystemMetrics`] only on its guard (`permitted`).

use super::appcmd::{Shell, shell_now};
use super::workarea::ShellState;

/// `ARW_HIDE` from winuser.h: hide minimized windows by moving them off the visible area of the screen.
pub const ARW_HIDE: u32 = 0x0008;

/// The parts of `MINIMIZEDMETRICS` that are kept (its `cbSize` is the size of the struct and is set when it is read or written).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    /// `iWidth`: the width of a minimized window.
    pub width: i32,
    pub horz_gap: i32,
    pub vert_gap: i32,
    /// `iArrange`: where the minimized windows start and in which direction they go; `ARW_HIDE` is one more bit.
    pub arrange: u32,
}

impl Metrics {
    pub fn hidden(&self) -> bool {
        self.arrange & ARW_HIDE != 0
    }

    /// The same metrics with the hide bit added.
    pub fn with_hide(&self) -> Metrics {
        Metrics { arrange: self.arrange | ARW_HIDE, ..*self }
    }
}

/// Why a value is being written. The real setter allows each only for some shell states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// Hiding the bars: only when nobody owns the shell window.
    Hide,
    /// Putting back what was there: when nobody owns it (the host ends) or another shell does (not Explorer, see the module).
    Restore,
}

/// Reads and writes the minimized-windows metrics.
pub trait MetricsApi {
    fn get(&mut self) -> Option<Metrics>;
    /// True: Windows took it.
    fn set(&mut self, m: Metrics, why: Write) -> bool;
}

/// What one [`MinimizedKeeper::apply`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Done {
    Nothing,
    /// The bars are hidden now (the metrics before are remembered).
    Hidden,
    /// The metrics from before were put back.
    Restored,
}

/// Why nothing was done, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// A shell window has an owner: the setting is that shell's business.
    ShellPresent(Shell),
    /// The metrics already hide minimized windows (set by something else): not ours, nothing to restore.
    AlreadyHidden,
    /// The metrics could not be read.
    Unreadable,
    /// Windows refused the write.
    Refused,
}

#[derive(Default)]
pub struct MinimizedKeeper {
    /// The metrics before the first write.
    before: Option<Metrics>,
    /// What was written, while it stands.
    applied: Option<Metrics>,
    last_held: Option<Held>,
}

/// Whether the real write may be made for the shell window as it is now (the second lock; the first is [`MinimizedKeeper::apply`]).
pub fn permitted(shell: Shell, why: Write) -> bool {
    match why {
        Write::Hide => shell == Shell::Nobody,
        Write::Restore => matches!(shell, Shell::Nobody | Shell::Foreign),
    }
}

impl MinimizedKeeper {
    pub fn new() -> MinimizedKeeper {
        MinimizedKeeper::default()
    }

    pub fn held(&self) -> Option<Held> {
        self.last_held
    }

    /// Whether this keeper has hidden the bars and not yet put them back.
    pub fn active(&self) -> bool {
        self.applied.is_some()
    }

    /// Brings the metrics in line: `hide == true` hides minimized windows (only with no shell), `false` gives back what this keeper
    /// changed. The shell window is looked at now, through `state`.
    pub fn apply(&mut self, state: &dyn ShellState, api: &mut dyn MetricsApi, hide: bool) -> Done {
        let shell = state.shell();
        if shell != Shell::Nobody {
            self.last_held = Some(Held::ShellPresent(shell));
            let ours = self.applied.take();
            let before = self.before.take();
            // another shell: its start is not Explorer's, put the system default back if the value is still exactly what we wrote
            if shell == Shell::Foreign
                && let (Some(ours), Some(before)) = (ours, before)
                && api.get() == Some(ours)
                && api.set(before, Write::Restore)
            {
                self.last_held = None;
                return Done::Restored;
            }
            return Done::Nothing;
        }
        if hide {
            let Some(cur) = api.get() else {
                self.last_held = Some(Held::Unreadable);
                return Done::Nothing;
            };
            if self.applied == Some(cur) {
                return Done::Nothing;
            }
            if cur.hidden() {
                // set by something else (an earlier host that was killed, a shell that ran before): not ours to undo
                self.last_held = Some(Held::AlreadyHidden);
                return Done::Nothing;
            }
            if self.before.is_none() {
                self.before = Some(cur);
            }
            let want = cur.with_hide();
            if api.set(want, Write::Hide) {
                self.applied = Some(want);
                self.last_held = None;
                Done::Hidden
            } else {
                self.last_held = Some(Held::Refused);
                Done::Nothing
            }
        } else {
            let (Some(ours), Some(before)) = (self.applied.take(), self.before.take()) else { return Done::Nothing };
            // somebody else changed it since: leave it alone
            if api.get() != Some(ours) {
                return Done::Nothing;
            }
            if api.set(before, Write::Restore) {
                self.last_held = None;
                Done::Restored
            } else {
                self.last_held = Some(Held::Refused);
                Done::Nothing
            }
        }
    }
}

/// The opt-out: `RUSTYFILES_KEEP_MINIMIZED=1` leaves the metrics alone even without a shell.
pub const ENV_KEEP: &str = "RUSTYFILES_KEEP_MINIMIZED";

/// Whether the variable's value asks for the metrics to be left alone: exactly `1`.
pub fn left_alone(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim() == "1")
}

/// The update flags both real calls pass: none. No `SPIF_UPDATEINIFILE` (the value must not reach the user's profile) and no
/// `SPIF_SENDCHANGE` (no broadcast).
pub const WRITE_FLAGS: u32 = 0;

/// The real thing: `SystemParametersInfoW` with `SPI_GETMINIMIZEDMETRICS` / `SPI_SETMINIMIZEDMETRICS`, no profile write, no broadcast.
pub struct SystemMetrics;

impl MetricsApi for SystemMetrics {
    fn get(&mut self) -> Option<Metrics> {
        use windows::Win32::UI::WindowsAndMessaging::{MINIMIZEDMETRICS, SPI_GETMINIMIZEDMETRICS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
        let mut m = MINIMIZEDMETRICS { cbSize: size_of::<MINIMIZEDMETRICS>() as u32, ..Default::default() };
        let ok = unsafe { SystemParametersInfoW(SPI_GETMINIMIZEDMETRICS, m.cbSize, Some(&mut m as *mut MINIMIZEDMETRICS as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(WRITE_FLAGS)) }.is_ok();
        ok.then_some(Metrics { width: m.iWidth, horz_gap: m.iHorzGap, vert_gap: m.iVertGap, arrange: m.iArrange.0 as u32 })
    }

    fn set(&mut self, v: Metrics, why: Write) -> bool {
        // lock 2: looked at again right before the call
        if !permitted(shell_now(), why) {
            log::warn!("minimized windows: refused, the shell window has an owner");
            return false;
        }
        use windows::Win32::UI::WindowsAndMessaging::{MINIMIZEDMETRICS, MINIMIZEDMETRICS_ARRANGE, SPI_SETMINIMIZEDMETRICS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
        let mut m = MINIMIZEDMETRICS { cbSize: size_of::<MINIMIZEDMETRICS>() as u32, iWidth: v.width, iHorzGap: v.horz_gap, iVertGap: v.vert_gap, iArrange: MINIMIZEDMETRICS_ARRANGE(v.arrange as i32) };
        let ok = unsafe { SystemParametersInfoW(SPI_SETMINIMIZEDMETRICS, m.cbSize, Some(&mut m as *mut MINIMIZEDMETRICS as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(WRITE_FLAGS)) }.is_ok();
        if !ok {
            log::warn!("minimized windows: Windows did not take the metrics");
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct State(Cell<Shell>);
    impl ShellState for State {
        fn shell(&self) -> Shell {
            self.0.get()
        }
    }

    /// A system that holds one value and remembers every write.
    struct Fake {
        now: Option<Metrics>,
        writes: Vec<(Metrics, Write)>,
        refuse: bool,
    }
    impl Fake {
        fn with(m: Metrics) -> Fake {
            Fake { now: Some(m), writes: Vec::new(), refuse: false }
        }
    }
    impl MetricsApi for Fake {
        fn get(&mut self) -> Option<Metrics> {
            self.now
        }
        fn set(&mut self, m: Metrics, why: Write) -> bool {
            self.writes.push((m, why));
            if self.refuse {
                return false;
            }
            self.now = Some(m);
            true
        }
    }

    const DEFAULT: Metrics = Metrics { width: 160, horz_gap: 4, vert_gap: 4, arrange: 0 };

    #[test]
    fn with_no_shell_the_bars_are_hidden_and_what_was_there_is_remembered() {
        let st = State(Cell::new(Shell::Nobody));
        let mut api = Fake::with(DEFAULT);
        let mut k = MinimizedKeeper::new();
        assert_eq!(k.apply(&st, &mut api, true), Done::Hidden);
        assert!(api.now.unwrap().hidden());
        assert_eq!(api.now.unwrap(), DEFAULT.with_hide(), "only the hide bit is added, the sizes and gaps are the user's");
        assert!(k.active());
        // asking again changes nothing and writes nothing
        assert_eq!(k.apply(&st, &mut api, true), Done::Nothing);
        assert_eq!(api.writes.len(), 1);
    }

    #[test]
    fn the_host_ending_puts_the_old_metrics_back_exactly() {
        let st = State(Cell::new(Shell::Nobody));
        let mut api = Fake::with(Metrics { arrange: 3, ..DEFAULT });
        let mut k = MinimizedKeeper::new();
        k.apply(&st, &mut api, true);
        assert_eq!(k.apply(&st, &mut api, false), Done::Restored);
        assert_eq!(api.now, Some(Metrics { arrange: 3, ..DEFAULT }));
        assert!(!k.active());
        // a second end writes nothing
        assert_eq!(k.apply(&st, &mut api, false), Done::Nothing);
        assert_eq!(api.writes.len(), 2);
    }

    #[test]
    fn explorer_owning_the_shell_window_is_a_no_op_for_every_input() {
        for hide in [true, false] {
            let st = State(Cell::new(Shell::Explorer));
            let mut api = Fake::with(DEFAULT);
            let mut k = MinimizedKeeper::new();
            assert_eq!(k.apply(&st, &mut api, hide), Done::Nothing);
            assert!(api.writes.is_empty(), "nothing is written beside Explorer (hide = {hide})");
            assert_eq!(k.held(), Some(Held::ShellPresent(Shell::Explorer)));
        }
    }

    #[test]
    fn explorer_coming_back_is_not_overwritten_with_the_old_value() {
        // the trial: no shell, the bars are hidden by us, then Explorer starts and sets its own (hiding) value
        let st = State(Cell::new(Shell::Nobody));
        let mut api = Fake::with(DEFAULT);
        let mut k = MinimizedKeeper::new();
        k.apply(&st, &mut api, true);
        st.0.set(Shell::Explorer);
        // Explorer's value happens to be the same as ours: writing the old one back would show the bars beside it
        assert_eq!(k.apply(&st, &mut api, false), Done::Nothing);
        assert_eq!(api.now, Some(DEFAULT.with_hide()));
        assert_eq!(api.writes.len(), 1, "no write after the first");
        assert!(!k.active(), "the keeper has let go of it");
        // and when the shell goes again, the bars are hidden again
        st.0.set(Shell::Nobody);
        api.now = Some(DEFAULT);
        assert_eq!(k.apply(&st, &mut api, true), Done::Hidden);
    }

    #[test]
    fn another_shell_gets_the_system_default_back_only_if_the_value_is_still_ours() {
        let st = State(Cell::new(Shell::Nobody));
        let mut api = Fake::with(DEFAULT);
        let mut k = MinimizedKeeper::new();
        k.apply(&st, &mut api, true);
        st.0.set(Shell::Foreign);
        assert_eq!(k.apply(&st, &mut api, true), Done::Restored);
        assert_eq!(api.now, Some(DEFAULT));
        // changed by somebody meanwhile: left alone
        let mut api = Fake::with(DEFAULT);
        let mut k = MinimizedKeeper::new();
        st.0.set(Shell::Nobody);
        k.apply(&st, &mut api, true);
        api.now = Some(Metrics { width: 99, ..DEFAULT.with_hide() });
        st.0.set(Shell::Foreign);
        assert_eq!(k.apply(&st, &mut api, true), Done::Nothing);
        assert_eq!(api.now.unwrap().width, 99);
    }

    #[test]
    fn a_value_somebody_else_changed_is_not_undone_when_the_host_ends() {
        let st = State(Cell::new(Shell::Nobody));
        let mut api = Fake::with(DEFAULT);
        let mut k = MinimizedKeeper::new();
        k.apply(&st, &mut api, true);
        api.now = Some(Metrics { horz_gap: 40, ..DEFAULT.with_hide() });
        assert_eq!(k.apply(&st, &mut api, false), Done::Nothing);
        assert_eq!(api.now.unwrap().horz_gap, 40);
    }

    #[test]
    fn metrics_that_already_hide_are_not_claimed() {
        let st = State(Cell::new(Shell::Nobody));
        let mut api = Fake::with(DEFAULT.with_hide());
        let mut k = MinimizedKeeper::new();
        assert_eq!(k.apply(&st, &mut api, true), Done::Nothing);
        assert_eq!(k.held(), Some(Held::AlreadyHidden));
        assert!(api.writes.is_empty());
        assert!(!k.active());
        // so the host ending does not write them back either
        assert_eq!(k.apply(&st, &mut api, false), Done::Nothing);
        assert!(api.writes.is_empty());
    }

    #[test]
    fn unreadable_and_refused_are_said_and_change_nothing() {
        let st = State(Cell::new(Shell::Nobody));
        let mut api = Fake { now: None, writes: Vec::new(), refuse: false };
        let mut k = MinimizedKeeper::new();
        assert_eq!(k.apply(&st, &mut api, true), Done::Nothing);
        assert_eq!(k.held(), Some(Held::Unreadable));
        let mut api = Fake::with(DEFAULT);
        api.refuse = true;
        assert_eq!(k.apply(&st, &mut api, true), Done::Nothing);
        assert_eq!(k.held(), Some(Held::Refused));
        assert!(!k.active());
    }

    #[test]
    fn the_real_writer_is_allowed_only_where_the_shell_state_permits() {
        assert!(permitted(Shell::Nobody, Write::Hide));
        assert!(!permitted(Shell::Explorer, Write::Hide));
        assert!(!permitted(Shell::Foreign, Write::Hide));
        assert!(permitted(Shell::Nobody, Write::Restore));
        assert!(permitted(Shell::Foreign, Write::Restore));
        assert!(!permitted(Shell::Explorer, Write::Restore), "Explorer sets the value itself");
    }

    #[test]
    fn the_calls_are_not_written_to_the_profile_and_not_broadcast() {
        use windows::Win32::UI::WindowsAndMessaging::{SPIF_SENDCHANGE, SPIF_UPDATEINIFILE};
        // both real calls pass exactly this value (and nothing else), and it holds neither the profile bit nor the broadcast bit
        assert_eq!(WRITE_FLAGS, 0);
        assert_eq!(WRITE_FLAGS & (SPIF_UPDATEINIFILE.0 | SPIF_SENDCHANGE.0), 0);
    }

    #[test]
    fn the_opt_out_is_a_one_only() {
        assert!(!left_alone(None));
        assert!(!left_alone(Some("0")));
        assert!(!left_alone(Some("true")));
        assert!(left_alone(Some("1")));
        assert!(left_alone(Some(" 1 ")));
    }

    /// Read-only: the real metrics can be read (the one value any Windows has); nothing is written.
    #[test]
    fn the_real_metrics_can_be_read() {
        let m = SystemMetrics.get();
        eprintln!("minimized metrics readable: {}", m.is_some());
        assert!(m.is_some());
    }
}
