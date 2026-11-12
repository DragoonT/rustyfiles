//! The dock host's side of hiding Explorer's taskbar (shell plan E1): WHEN to hide, WHEN to show, in what order.
//!
//! `Controller` holds the decisions; everything it touches (windows, processes, the state file, the guard) is behind
//! [`HideWorld`], so every way of losing the taskbar is a test with a fake world (`tbfakes`). The real world is in `rf-win`
//! (`taskbar_hide`): it runs this on a thread of its own, because a call into another process's window can wait for that process.
//!
//! The order that keeps the user safe, in one place:
//! 1. the guard is started and CONFIRMED (it has the host's process handle and has read the state file);
//! 2. the record of what is about to be hidden is written to `shell-state.json` (with `pending_restore`);
//! 3. only then is the first window hidden;
//! 4. leaving (setting off, quit) shows the windows, WAITS until they are visible (the show is only a queued request to another
//!    process), puts the work area back, removes the record, and only then tells the guard.
//! 5. a restore that did not complete is retried on the timer (with a growing pause), whatever the setting says, until it has
//!    or until the retries are used up; the guard is the net for what is left.
//!
//! The guard ending while windows are hidden shows them again at once (the next tick, a second at most) and the host stops hiding
//! until a guard is confirmed again.

use crate::statefile::ShellStateFile;
use crate::tbmodel::{self, Gate, GuardStatus, HideRecord, LiveWindow, MonitorGeom, Refusal, Target, Throttle};

/// How long the host waits for the guard to say it is armed.
pub const CONFIRM_WAIT_MS: u64 = 3_000;
/// Waits before the next try to start a guard that did not come up: after the 1st, 2nd and later failures.
pub const RESPAWN_BACKOFF_MS: [u64; 3] = [5_000, 15_000, 60_000];
/// A guard that has been up this long is trusted again: its earlier failures are forgotten.
pub const GUARD_STABLE_MS: u64 = 120_000;
/// A restore that did not complete is tried again after this long, then twice as long, up to the maximum (each try may set the
/// work area, which is broadcast to every window).
pub const RESTORE_RETRY_BASE_MS: u64 = 1_000;
pub const RESTORE_RETRY_MAX_MS: u64 = 30_000;
/// After this many retries the host stops trying; the guard, which was not told it can go, is what is left.
pub const RESTORE_RETRY_LIMIT: u32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Give the strip of a hidden taskbar back to programs (see `tbmodel::giveback`). Off unless asked for.
    pub give_back_work_area: bool,
}

/// What the controller needs from the outside. Every method is cheap to fake; the real ones are in `rf-win`.
pub trait HideWorld {
    fn now_ms(&mut self) -> u64;
    fn log(&mut self, msg: &str);
    /// Something that should be seen in the log file even at the default level.
    fn warn(&mut self, msg: &str) {
        self.log(msg);
    }
    /// Read-only: does Explorer own the shell window (so that there is an Explorer taskbar to hide)?
    fn explorer_owns_shell(&mut self) -> bool;
    /// Read-only: the taskbar-class windows that exist now, with owner and visibility.
    fn list_taskbars(&mut self) -> Result<Vec<LiveWindow>, String>;
    fn hide(&mut self, t: &Target) -> Result<(), String>;
    /// Asks for the window to be shown. `Ok` means ASKED: the real call is queued in the other process (it must not wait for a
    /// Explorer that is hung), so whether it happened is only known from [`HideWorld::wait_visible`].
    fn show(&mut self, t: &Target) -> Result<(), String>;
    /// Waits a short while (half a second at most) for the windows to be visible; returns how many still are not. A window that
    /// no longer exists is not counted: there is nothing left to show.
    fn wait_visible(&mut self, targets: &[Target]) -> usize;
    fn monitors(&mut self) -> Vec<MonitorGeom>;
    /// Sets the session's work area for the monitor that holds `rect` (never stored: no profile update).
    fn set_work_area(&mut self, rect: [i32; 4]) -> Result<(), String>;
    fn load_state(&mut self) -> ShellStateFile;
    fn save_state(&mut self, f: &ShellStateFile) -> Result<(), String>;
    /// Is `rustyfiles-shellguard.exe` where it is expected?
    fn guard_exists(&mut self) -> bool;
    /// Starts the guard for this host (creates the events, passes the host's process id). Does not wait.
    fn start_guard(&mut self) -> Result<(), String>;
    /// Waits up to `wait_ms` for the guard to say it is armed; false when it ended or stayed silent.
    fn guard_ready(&mut self, wait_ms: u64) -> bool;
    /// Is the guard process still running?
    fn guard_alive(&mut self) -> bool;
    /// Tells the guard that the host restored cleanly (it exits by itself) and forgets it.
    fn tell_guard_done(&mut self);
    /// Ends a guard that never confirmed or that we no longer trust, and forgets it.
    fn kill_guard(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Off,
    /// This many Explorer taskbar windows are hidden.
    Hidden(usize),
    /// Wanted, not hidden, and why.
    Waiting(Refusal),
}

pub struct Controller {
    host_pid: u32,
    host_start: u64,
    opts: Options,
    want: bool,
    guard: GuardStatus,
    /// Windows hidden by us in this run (handles; forgotten when Explorer restarts).
    hidden: Vec<Target>,
    /// Their rectangles, for the work-area rule.
    bars: Vec<[i32; 4]>,
    refusal: Option<Refusal>,
    guard_failures: usize,
    next_guard_try_ms: u64,
    guard_confirmed_ms: u64,
    throttle: Throttle,
    /// The last restore did not complete: it is tried again on the timer until it does (or the retries run out).
    restore_pending: bool,
    restore_attempts: u32,
    next_restore_ms: u64,
}

impl Controller {
    pub fn new(host_pid: u32, host_start: u64, opts: Options) -> Controller {
        Controller {
            host_pid,
            host_start,
            opts,
            want: false,
            guard: GuardStatus::Absent,
            hidden: Vec::new(),
            bars: Vec::new(),
            refusal: None,
            guard_failures: 0,
            next_guard_try_ms: 0,
            guard_confirmed_ms: 0,
            // Explorer may recompute the work area whenever it likes: give it back at most 4 times a minute.
            throttle: Throttle::new(4, 60_000),
            restore_pending: false,
            restore_attempts: 0,
            next_restore_ms: 0,
        }
    }

    pub fn status(&self) -> Status {
        if !self.want {
            Status::Off
        } else if !self.hidden.is_empty() {
            Status::Hidden(self.hidden.len())
        } else {
            Status::Waiting(self.refusal.unwrap_or(Refusal::NothingToHide))
        }
    }

    pub fn guard_status(&self) -> GuardStatus {
        self.guard
    }

    /// A restore is still owed: the timer must keep running, and the host's thread must not end, until it is done.
    pub fn restore_pending(&self) -> bool {
        self.restore_pending
    }

    /// Windows may be hidden by us right now, or a restore is owed (the panic net looks at this).
    pub fn holds_windows(&self) -> bool {
        !self.hidden.is_empty() || self.restore_pending
    }

    /// The host has just started: a record left by an earlier host (it crashed, the power went) is undone first, whatever the
    /// setting is now.
    pub fn startup(&mut self, w: &mut dyn HideWorld) {
        let st = w.load_state();
        let stale = match st.hidden_taskbar() {
            Some(rec) => tbmodel::is_stale(&rec, self.host_pid, Some(self.host_start)),
            // a record that cannot be read: we cannot know what was hidden
            None => st.has_hidden_taskbar_key(),
        };
        if stale {
            w.warn("taskbar: a record of hidden windows from an earlier run was found; showing them first");
            if let Err(e) = self.restore(w) {
                w.warn(&format!("taskbar: the earlier run's windows could not all be shown: {e}"));
            }
        }
    }

    /// The setting changed (or the host started with it on).
    pub fn set_wanted(&mut self, w: &mut dyn HideWorld, on: bool) {
        if on == self.want {
            if on {
                self.reconcile(w);
            }
            return;
        }
        self.want = on;
        if on {
            self.guard_failures = 0;
            self.next_guard_try_ms = 0;
            self.refusal = None;
            self.reconcile(w);
        } else {
            self.leave(w);
        }
    }

    /// The host is quitting.
    pub fn quit(&mut self, w: &mut dyn HideWorld) {
        self.want = false;
        self.leave(w);
    }

    /// Explorer made its taskbar again (`TaskbarCreated`): the handles we knew are void.
    pub fn explorer_restarted(&mut self, w: &mut dyn HideWorld) {
        self.hidden.clear();
        self.bars.clear();
        self.reconcile(w);
    }

    /// Monitors came or went, the work area changed: look again (a new monitor has a new secondary taskbar).
    pub fn displays_changed(&mut self, w: &mut dyn HideWorld) {
        self.reconcile(w);
    }

    /// The work area was changed by someone (Explorer recomputes it when a toolbar changes): give the strip back again, if asked.
    pub fn work_area_changed(&mut self, w: &mut dyn HideWorld) {
        if self.want && !self.hidden.is_empty() {
            self.give_back(w);
        }
    }

    /// The timer: while hidden or while a restore is owed, once a second. Finishes an owed restore first, then checks the guard.
    pub fn tick(&mut self, w: &mut dyn HideWorld) {
        self.reconcile(w);
    }

    /// Tries an owed restore again, when its pause is over. Independent of the setting: a restore that failed when the setting
    /// went off is not forgotten because the setting is off.
    fn retry_restore(&mut self, w: &mut dyn HideWorld) {
        if !self.restore_pending {
            return;
        }
        let now = w.now_ms();
        if now < self.next_restore_ms {
            return;
        }
        match self.restore(w) {
            Ok(_) => {
                w.log("taskbar: the restore that was owed is done");
                self.after_restore(w);
            }
            Err(e) => {
                if self.restore_pending {
                    w.warn(&format!("taskbar: the restore is still not complete: {e}"));
                } else {
                    w.warn("taskbar: giving up on the restore; the guard stays to do it");
                }
            }
        }
    }

    /// A restore finished: when we are not hiding any more, the guard (if one is confirmed) can go.
    fn after_restore(&mut self, w: &mut dyn HideWorld) {
        if !self.want && self.guard == GuardStatus::Confirmed {
            w.tell_guard_done();
            self.guard = GuardStatus::Absent;
        }
    }

    /// Brings the world in line with the wish: guard alive, every visible Explorer taskbar window hidden and recorded.
    pub fn reconcile(&mut self, w: &mut dyn HideWorld) {
        self.retry_restore(w);
        if !self.want {
            return;
        }
        // 1. The guard first. A guard that ended means nothing is watching: show the taskbar before doing anything else.
        if self.guard == GuardStatus::Confirmed && !w.guard_alive() {
            w.warn("taskbar: the guard ended; showing the taskbar again");
            self.guard = GuardStatus::Lost;
            if let Err(e) = self.restore(w) {
                w.warn(&format!("taskbar: not everything could be shown: {e}"));
            }
            w.kill_guard();
            // A guard that keeps ending is not started again every second.
            let now = w.now_ms();
            self.note_guard_failure(now);
            self.refusal = Some(Refusal::GuardLost);
        }
        // A guard that has stayed up for a good while earns a clean record.
        if self.guard == GuardStatus::Confirmed && self.guard_failures > 0 {
            let now = w.now_ms();
            if now.saturating_sub(self.guard_confirmed_ms) >= GUARD_STABLE_MS {
                self.guard_failures = 0;
            }
        }
        // 2. Is there an Explorer taskbar at all? (Nothing is started for a session where there is none.)
        if !w.explorer_owns_shell() {
            self.refusal = Some(Refusal::NoExplorerShell);
            return;
        }
        // 3. The guard must be confirmed before anything is hidden.
        if self.guard != GuardStatus::Confirmed
            && let Err(r) = self.ensure_guard(w)
        {
            self.refusal = Some(r);
            return;
        }
        let live = match w.list_taskbars() {
            Ok(l) => l,
            Err(e) => {
                w.warn(&format!("taskbar: could not list the windows: {e}"));
                return;
            }
        };
        let to_hide = tbmodel::windows_to_hide(&live);
        if to_hide.is_empty() {
            if self.hidden.is_empty() {
                self.refusal = Some(Refusal::NothingToHide);
            }
            return;
        }
        self.hide_pass(w, &live, to_hide);
    }

    fn ensure_guard(&mut self, w: &mut dyn HideWorld) -> Result<(), Refusal> {
        let now = w.now_ms();
        if now < self.next_guard_try_ms {
            return Err(Refusal::GuardLost);
        }
        if !w.guard_exists() {
            // Looked for again only after a backoff: the file is not going to appear by itself.
            self.note_guard_failure(now);
            w.warn("taskbar: rustyfiles-shellguard.exe was not found; the taskbar stays");
            return Err(Refusal::GuardMissing);
        }
        if let Err(e) = w.start_guard() {
            self.note_guard_failure(now);
            w.warn(&format!("taskbar: the guard did not start: {e}"));
            return Err(Refusal::GuardNotConfirmed);
        }
        if !w.guard_ready(CONFIRM_WAIT_MS) {
            w.kill_guard();
            self.note_guard_failure(now);
            w.warn("taskbar: the guard did not confirm; the taskbar stays");
            return Err(Refusal::GuardNotConfirmed);
        }
        self.guard = GuardStatus::Confirmed;
        self.guard_confirmed_ms = w.now_ms();
        Ok(())
    }

    fn note_guard_failure(&mut self, now: u64) {
        let i = self.guard_failures.min(RESPAWN_BACKOFF_MS.len() - 1);
        self.next_guard_try_ms = now + RESPAWN_BACKOFF_MS[i];
        self.guard_failures += 1;
    }

    fn hide_pass(&mut self, w: &mut dyn HideWorld, live: &[LiveWindow], to_hide: Vec<Target>) {
        // The record first: it must be on disk before any window is hidden.
        let now = w.now_ms();
        let mut st = w.load_state();
        let rec: HideRecord = tbmodel::merge_record(st.hidden_taskbar().as_ref(), &to_hide, self.host_pid, self.host_start, now);
        st.set_hidden_taskbar(&rec);
        st.set_pending_restore(true);
        let saved = match w.save_state(&st) {
            Ok(()) => true,
            Err(e) => {
                w.warn(&format!("taskbar: the state file could not be written: {e}"));
                false
            }
        };
        let gate = Gate { setting_on: self.want, explorer_shell: true, guard: self.guard, record_saved: saved };
        if let Err(r) = tbmodel::may_hide(&gate) {
            self.refusal = Some(r);
            return;
        }
        let mut done = 0;
        for t in &to_hide {
            match w.hide(t) {
                Ok(()) => {
                    done += 1;
                    if !self.hidden.iter().any(|h| h.handle == t.handle) {
                        self.hidden.push(t.clone());
                    }
                    if let Some(l) = live.iter().find(|l| l.handle == t.handle)
                        && !self.bars.contains(&l.rect)
                    {
                        self.bars.push(l.rect);
                    }
                }
                Err(e) => w.warn(&format!("taskbar: a window could not be hidden: {e}")),
            }
        }
        if done > 0 {
            self.refusal = None;
            w.log(&format!("taskbar: {done} Explorer taskbar window(s) hidden"));
            if self.opts.give_back_work_area {
                self.give_back(w);
            }
        }
    }

    fn give_back(&mut self, w: &mut dyn HideWorld) {
        if !self.opts.give_back_work_area || self.bars.is_empty() {
            return;
        }
        let mons = w.monitors();
        let changes = tbmodel::giveback(&mons, &self.bars);
        if changes.is_empty() {
            return;
        }
        let now = w.now_ms();
        if !self.throttle.allow(now) {
            w.warn("taskbar: the work area was given back too often lately; left alone");
            return;
        }
        // The user's own rectangles are written down before the first one is changed.
        let mut st = w.load_state();
        let originals = tbmodel::originals_to_save(&st.original_work_areas(), &changes);
        st.set_original_work_areas(&originals);
        st.set_pending_restore(true);
        if let Err(e) = w.save_state(&st) {
            w.warn(&format!("taskbar: the state file could not be written; the work area is left alone: {e}"));
            return;
        }
        for c in &changes {
            if let Err(e) = w.set_work_area(c.to) {
                w.warn(&format!("taskbar: a work area could not be set: {e}"));
            }
        }
    }

    /// Shows what was hidden (by this run or an earlier one), puts the work areas back and removes the record. `Ok` carries the
    /// number of windows that are visible afterwards; `Err` leaves the record in place so that the guard or the next start tries again.
    /// An `Err` also makes the restore OWED: the timer tries it again (see [`Controller::restore_pending`]), with growing pauses.
    fn restore(&mut self, w: &mut dyn HideWorld) -> Result<usize, String> {
        match self.restore_once(w) {
            Ok(n) => {
                self.restore_pending = false;
                self.restore_attempts = 0;
                Ok(n)
            }
            Err(e) => {
                if self.restore_attempts >= RESTORE_RETRY_LIMIT {
                    self.restore_pending = false;
                } else {
                    let pause = (RESTORE_RETRY_BASE_MS << self.restore_attempts.min(8)).min(RESTORE_RETRY_MAX_MS);
                    self.next_restore_ms = w.now_ms() + pause;
                    self.restore_attempts += 1;
                    self.restore_pending = true;
                }
                Err(e)
            }
        }
    }

    fn restore_once(&mut self, w: &mut dyn HideWorld) -> Result<usize, String> {
        let st = w.load_state();
        let rec = st.hidden_taskbar();
        let due = rec.is_some() || st.has_hidden_taskbar_key() || !self.hidden.is_empty();
        if !due {
            return Ok(0);
        }
        let live = w.list_taskbars()?;
        let mut shown = 0usize;
        let mut failed = 0usize;
        // The windows that were asked to come back: the request is queued in Explorer, so Ok from `show` proves nothing.
        let mut asked: Vec<Target> = Vec::new();
        match &rec {
            Some(rec) => {
                let plan = tbmodel::plan_restore(rec, &live);
                for t in &plan.show {
                    match w.show(t) {
                        Ok(()) => {
                            shown += 1;
                            asked.push(t.clone());
                        }
                        Err(e) => {
                            failed += 1;
                            w.warn(&format!("taskbar: a window could not be shown: {e}"));
                        }
                    }
                }
                shown += plan.visible.len();
                if !plan.foreign.is_empty() {
                    w.warn(&format!("taskbar: {} recorded window(s) are not the ones that were hidden any more; left alone", plan.foreign.len()));
                }
            }
            None => {
                // No usable record although one is due: show every hidden taskbar window of Explorer's.
                for t in tbmodel::plan_sweep(&live) {
                    match w.show(&t) {
                        Ok(()) => {
                            shown += 1;
                            asked.push(t.clone());
                        }
                        Err(e) => {
                            failed += 1;
                            w.warn(&format!("taskbar: a window could not be shown: {e}"));
                        }
                    }
                }
            }
        }
        // Only now is it known whether they came back. If they did not (Explorer is busy or hung and dropped the request), the
        // record, the pending flag and the guard all stay: this is exactly the case they are for.
        if !asked.is_empty() {
            let still_hidden = w.wait_visible(&asked);
            if still_hidden > 0 {
                shown = shown.saturating_sub(still_hidden);
                failed += still_hidden;
            }
        }
        if failed > 0 {
            return Err(format!("{failed} window(s) could not be shown"));
        }
        // Work areas next, from the rectangles written down before they were changed.
        let originals = st.original_work_areas();
        if st.pending_restore() {
            let mut bad = 0;
            for o in &originals {
                if w.set_work_area(o.rect).is_err() {
                    bad += 1;
                }
            }
            if bad > 0 {
                return Err(format!("{bad} work area(s) could not be put back"));
            }
        }
        let mut st = w.load_state();
        st.clear_hidden_taskbar();
        st.set_pending_restore(false);
        st.set_original_work_areas(&[]);
        if let Err(e) = w.save_state(&st) {
            w.warn(&format!("taskbar: the record could not be removed (the windows are shown): {e}"));
        }
        self.hidden.clear();
        self.bars.clear();
        Ok(shown)
    }

    /// Setting off or quit: restore, then tell the guard. A restore that failed leaves the guard running as the safety net.
    fn leave(&mut self, w: &mut dyn HideWorld) {
        self.refusal = None;
        let had_guard = self.guard == GuardStatus::Confirmed;
        match self.restore(w) {
            Ok(_) => {
                if had_guard {
                    w.tell_guard_done();
                }
                self.guard = GuardStatus::Absent;
            }
            Err(e) => {
                // The guard stays as the net; the timer tries again meanwhile (`retry_restore`), and tells the guard when it works.
                w.warn(&format!("taskbar: not restored ({e}); it is tried again, and the guard stays to do it"));
            }
        }
    }
}
