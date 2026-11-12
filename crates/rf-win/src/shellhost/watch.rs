//! Noticing, while the host runs, that the shell window came or went (Explorer closed, crashed, was restarted, or a replacement shell
//! started). Until now the host looked once, at its start (`mode::detect`).
//!
//! THE COST. No timer of its own and no thread: the dock's hidden window already ticks every 500 ms and has a message for
//! "Explorer's taskbar was created". [`ModeWatch::check`] is called from the 2 s tick and from that message and does
//!
//! * on a tick: ONE call, `GetShellWindow()` (a read of a handle in the window station, no process is opened), compared with the handle
//!   seen last. Nothing else happens while it is the same: no allocation, no query of a process image, no event.
//! * when the handle differs, or when a program broadcast `TaskbarCreated` (Explorer after a restart, but any program can send it, and
//!   our own tray host does): the full look (`mode::detect`, which opens the owner's process to read its image name) - but never again
//!   within [`MIN_LOOK_MS`] of the previous full look, so neither a shell window that flickers nor a program that broadcasts in a loop
//!   can make the host look more than once in two seconds. A change that was held back is picked up by the next tick (the stored handle
//!   was not updated), and an announcement that was held back is remembered and answered by the next tick that is allowed to look, so
//!   nothing is lost by waiting. (The dock's BAND is not held back by any of this: it is registered again at every announcement, see
//!   `dock/band.rs`.)
//!
//! The result says WHAT to tell the parts that care ([`Seen`]); telling them is the caller's (`SystemEvent::ShellChanged`).

use super::mode::{self, ShellMode};

/// The least time between two full looks, ms.
pub const MIN_LOOK_MS: u64 = 2_000;

/// What the watch asks of the system. The real one is [`SystemLook`]; the tests use a fake.
pub trait ShellLook {
    /// The shell window now (`GetShellWindow()`), `None` when there is none. Cheap.
    fn shell_window(&self) -> Option<isize>;
    /// The full look: who owns it, and so the mode. Opens a process to read its name.
    fn mode(&self) -> ShellMode;
}

/// Why the watch is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The periodic tick (every 2 s).
    Tick,
    /// Explorer broadcast `TaskbarCreated`: it is new, whatever the handle looks like.
    Announced,
}

/// What a check found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// The same shell window as before (or a change that has to wait for the next tick).
    Nothing,
    /// The shell window is another one (or came, or went), and the mode is this. `mode_changed` is false when Explorer was replaced
    /// by another Explorer within one look.
    Changed { mode: ShellMode, mode_changed: bool },
}

#[derive(Debug)]
pub struct ModeWatch {
    mode: ShellMode,
    window: Option<isize>,
    last_look_ms: Option<u64>,
    /// An announcement came inside the throttle and has not been looked at yet.
    owed: bool,
}

impl ModeWatch {
    /// Starts from the state the host found at its start (one full look, made by the caller).
    pub fn new(look: &dyn ShellLook) -> ModeWatch {
        ModeWatch { mode: look.mode(), window: look.shell_window(), last_look_ms: None, owed: false }
    }

    pub fn mode(&self) -> ShellMode {
        self.mode
    }

    /// One check. `now_ms`: any steady clock, in ms.
    pub fn check(&mut self, look: &dyn ShellLook, now_ms: u64, cause: Cause) -> Seen {
        let window = look.shell_window();
        if cause == Cause::Tick && window == self.window && !self.owed {
            return Seen::Nothing;
        }
        let throttled = self.last_look_ms.is_some_and(|t| now_ms.saturating_sub(t) < MIN_LOOK_MS);
        if throttled {
            // (held back, not dropped: an announcement is answered by the next tick that may look)
            self.owed |= cause == Cause::Announced;
            return Seen::Nothing;
        }
        let announced = cause == Cause::Announced || self.owed;
        self.owed = false;
        let mode = look.mode();
        self.last_look_ms = Some(now_ms);
        let changed_window = window != self.window;
        self.window = window;
        let mode_changed = mode != self.mode;
        self.mode = mode;
        if changed_window || mode_changed || announced {
            Seen::Changed { mode, mode_changed }
        } else {
            Seen::Nothing
        }
    }
}

/// The real look: read-only.
pub struct SystemLook;

impl ShellLook for SystemLook {
    fn shell_window(&self) -> Option<isize> {
        use windows::Win32::UI::WindowsAndMessaging::GetShellWindow;
        let h = unsafe { GetShellWindow() };
        (!h.0.is_null()).then_some(h.0 as isize)
    }

    fn mode(&self) -> ShellMode {
        mode::detect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fake {
        window: Cell<Option<isize>>,
        mode: Cell<ShellMode>,
        full_looks: Cell<u32>,
        cheap: Cell<u32>,
    }

    impl Fake {
        fn explorer(window: isize) -> Fake {
            Fake { window: Cell::new(Some(window)), mode: Cell::new(ShellMode::Beside), full_looks: Cell::new(0), cheap: Cell::new(0) }
        }
        fn explorer_gone(&self) {
            self.window.set(None);
            self.mode.set(ShellMode::Shell);
        }
        fn explorer_back(&self, window: isize) {
            self.window.set(Some(window));
            self.mode.set(ShellMode::Beside);
        }
    }

    impl ShellLook for Fake {
        fn shell_window(&self) -> Option<isize> {
            self.cheap.set(self.cheap.get() + 1);
            self.window.get()
        }
        fn mode(&self) -> ShellMode {
            self.full_looks.set(self.full_looks.get() + 1);
            self.mode.get()
        }
    }

    #[test]
    fn nothing_changing_costs_one_cheap_read_per_tick_and_no_full_look() {
        let f = Fake::explorer(10);
        let mut w = ModeWatch::new(&f);
        let (full0, cheap0) = (f.full_looks.get(), f.cheap.get());
        for i in 0..100 {
            assert_eq!(w.check(&f, i * 2_000, Cause::Tick), Seen::Nothing);
        }
        assert_eq!(f.full_looks.get(), full0, "no process is opened while nothing changes");
        assert_eq!(f.cheap.get() - cheap0, 100);
    }

    #[test]
    fn explorer_going_away_and_coming_back_is_noticed_both_ways() {
        let f = Fake::explorer(10);
        let mut w = ModeWatch::new(&f);
        f.explorer_gone();
        assert_eq!(w.check(&f, 2_000, Cause::Tick), Seen::Changed { mode: ShellMode::Shell, mode_changed: true });
        assert_eq!(w.mode(), ShellMode::Shell);
        // still gone: quiet
        assert_eq!(w.check(&f, 4_000, Cause::Tick), Seen::Nothing);
        // it is back: the announcement is looked at at once (the last look was 2 s ago)
        f.explorer_back(99);
        assert_eq!(w.check(&f, 4_000, Cause::Announced), Seen::Changed { mode: ShellMode::Beside, mode_changed: true });
        assert_eq!(w.mode(), ShellMode::Beside);
        assert_eq!(w.check(&f, 6_000, Cause::Tick), Seen::Nothing, "and the tick that follows finds the same window");
    }

    #[test]
    fn at_most_one_full_look_in_two_seconds_and_a_held_back_change_is_not_lost() {
        let f = Fake::explorer(10);
        let mut w = ModeWatch::new(&f);
        f.explorer_gone();
        assert!(matches!(w.check(&f, 10_000, Cause::Tick), Seen::Changed { .. }));
        let looks = f.full_looks.get();
        f.explorer_back(11);
        assert_eq!(w.check(&f, 10_500, Cause::Tick), Seen::Nothing, "held back: the last look was 500 ms ago");
        assert_eq!(f.full_looks.get(), looks, "no second look inside the two seconds");
        assert_eq!(w.check(&f, 12_000, Cause::Tick), Seen::Changed { mode: ShellMode::Beside, mode_changed: true }, "the next tick picks it up");
    }

    #[test]
    fn explorer_replaced_by_explorer_is_a_changed_window_but_not_a_changed_mode() {
        let f = Fake::explorer(10);
        let mut w = ModeWatch::new(&f);
        f.window.set(Some(20));
        assert_eq!(w.check(&f, 2_000, Cause::Tick), Seen::Changed { mode: ShellMode::Beside, mode_changed: false });
    }

    #[test]
    fn an_announcement_with_the_same_window_still_tells_the_parts() {
        // Explorer announced a new taskbar; its desktop window handle looked the same: the parts that depend on it still hear
        let f = Fake::explorer(10);
        let mut w = ModeWatch::new(&f);
        assert_eq!(w.check(&f, 5_000, Cause::Announced), Seen::Changed { mode: ShellMode::Beside, mode_changed: false });
    }

    #[test]
    fn a_burst_of_announcements_costs_one_full_look_in_two_seconds_and_the_tick_answers_the_rest() {
        // any program can broadcast TaskbarCreated (a restart loop, a tray tool): the process of the shell window's owner is opened
        // at most once in two seconds, whatever the number of announcements
        let f = Fake::explorer(10);
        let mut w = ModeWatch::new(&f);
        assert!(matches!(w.check(&f, 10_000, Cause::Announced), Seen::Changed { .. }), "the first one is looked at at once");
        let looks = f.full_looks.get();
        for i in 1..200 {
            assert_eq!(w.check(&f, 10_000 + i * 5, Cause::Announced), Seen::Nothing, "held back inside the two seconds");
        }
        assert_eq!(f.full_looks.get(), looks, "200 announcements, no further look");
        // the announcements that were held back are not lost: the first tick that may look answers them
        assert_eq!(w.check(&f, 11_000, Cause::Tick), Seen::Nothing, "still inside the two seconds");
        assert_eq!(w.check(&f, 12_000, Cause::Tick), Seen::Changed { mode: ShellMode::Beside, mode_changed: false });
        assert_eq!(f.full_looks.get(), looks + 1);
        assert_eq!(w.check(&f, 14_000, Cause::Tick), Seen::Nothing, "and once is enough");
    }

    #[test]
    fn the_real_look_is_read_only_and_answers() {
        let l = SystemLook;
        let _ = l.shell_window();
        let _ = l.mode();
    }
}
