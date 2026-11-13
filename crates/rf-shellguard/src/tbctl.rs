//! The dock host's side of hiding Explorer's taskbar (shell plan E1): WHEN to hide, WHEN to show, in what order.
//!
//! `Controller` holds the decisions; everything it touches (windows, processes, the state file, the guard, the dock) is behind
//! [`HideWorld`], so every way of losing the taskbar is a test with a fake world (`tbfakes`). The real world is in `rf-win`
//! (`taskbar_hide`): it runs this on a thread of its own, because a call into another process's window can wait for that process.
//!
//! The order that keeps the user safe, in one place:
//! 1. the guard is started and CONFIRMED (it has the host's process handle and has read the state file);
//! 2. the record of what is about to be hidden is written to `shell-state.json` (with `pending_restore`);
//! 3. only then is the first window hidden, and the hide is CONFIRMED (the windows are hidden, not only asked to be: the change is
//!    queued in Explorer's own thread);
//! 4. only after that the dock takes the bottom edge (a hidden taskbar keeps its strip, so the dock would float above an empty
//!    strip): the user's own work areas are written down FIRST (`original_work_areas`), then the dock is told to let go of its own
//!    desktop-toolbar band and to place itself from the monitor's bottom edge, and only when it has let go (a second band on top of
//!    ours would be a second strip) AND the work area shows that Explorer really dropped it (the dock's own word is only what it sent,
//!    and the route of its messages may be the tray host's) are the work areas set (`tbwork`). A monitor that becomes ours later has
//!    its work area written down before the first change to it, too. Nothing of this happens while the hide is not confirmed, and a
//!    hide that is lost again (Explorer restarted and the new taskbar could not be hidden) gives the dock its band back;
//! 5. leaving (setting off, quit, the guard lost) shows the windows, WAITS until they are visible (the show is only a queued request
//!    to another process), puts the work areas back from the written-down rectangles (checked against the monitors as they are by
//!    then: a changed resolution or a monitor that went is not answered with a rectangle of the old screen), lets the dock register
//!    its band with Explorer
//!    again, removes the record, and only then tells the guard;
//! 6. a restore that did not complete is retried on the timer (with a growing pause), whatever the setting says, until it has
//!    or until the retries are used up; the guard is the net for what is left (it shows the windows and puts the written-down work
//!    areas back; the dock's registration dies with the host).
//!
//! The guard ending while windows are hidden shows them again at once (the next tick, a second at most) and the host stops hiding
//! until a guard is confirmed again.

use crate::statefile::ShellStateFile;
use crate::tbmodel::{self, Gate, GuardStatus, HideRecord, LiveWindow, MonitorGeom, Refusal, Target, Throttle};
use crate::tbwork::{self, Verdict};

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

/// How long the dock may take to let go of its desktop-toolbar band (Explorer answers the removal and recomputes the work area). After
/// that the dock gets its band back and nothing is set.
pub const EDGE_DOCK_WAIT_MS: u64 = 3_000;
/// Waits before the next try to take the edge after one that failed (the state file, or a dock that did not let go).
pub const EDGE_RETRY_MS: [u64; 3] = [5_000, 15_000, 60_000];

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
    /// Waits a short while (half a second at most) for the windows that were asked to hide to be hidden; returns the ones that are
    /// still visible. A window that no longer exists is not among them.
    fn wait_hidden(&mut self, targets: &[Target]) -> Vec<Target>;
    /// The monitors, with the work area each has NOW. `primary` marks the one the dock sits on.
    fn monitors(&mut self) -> Vec<MonitorGeom>;
    /// Sets the session's work area for the monitor that holds `rect` (never stored: no profile update).
    fn set_work_area(&mut self, rect: [i32; 4]) -> Result<(), String>;
    /// Holds the work area of a monitor whose edge is ours while Explorer's taskbar is hidden: from `from` (what it has now) to `to`.
    /// The real one sets it WITHOUT the change broadcast and fits the monitor's maximized windows itself, because Explorer answers
    /// the broadcast by taking the rectangle back (`win::workarea::hold_work_area` has the measurement). Putting the user's own
    /// rectangles back stays [`HideWorld::set_work_area`], with the broadcast: there Explorer is meant to work them out again.
    fn hold_work_area(&mut self, from: [i32; 4], to: [i32; 4]) -> Result<(), String> {
        let _ = from;
        self.set_work_area(to)
    }
    /// The dock takes the bottom edge: its placement is told that its band is `edge` (the bottom strip of the primary monitor; empty
    /// when the dock reserves nothing) and its own desktop-toolbar registration with Explorer is given back and stays given back.
    /// Does not wait: [`HideWorld::dock_edge_ready`] says when the registration is gone. Telling it again only updates the strip.
    fn dock_take_edge(&mut self, edge: [i32; 4]);
    /// Has the dock let go of its desktop-toolbar registration? True also when it never had one, or there is no dock in this process.
    fn dock_edge_ready(&mut self) -> bool;
    /// The hide is over: the dock registers its band with Explorer again and places itself from the work area. Does not wait. Safe
    /// to call when nothing was taken.
    fn dock_give_back_edge(&mut self);
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

/// Where the taking of the bottom edge stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Not ours: the dock keeps its own band registration, the work areas are Explorer's.
    Free,
    /// The user's work areas are written down and the dock was told to let go of its band; waiting for it (a tick looks again).
    Releasing { since_ms: u64 },
    /// The dock let go and the work areas of the monitors whose taskbar we hid are set by the rules of `tbwork`.
    Ours,
}

pub struct Controller {
    host_pid: u32,
    host_start: u64,
    want: bool,
    guard: GuardStatus,
    /// Windows hidden by us in this run (handles; forgotten when Explorer restarts).
    hidden: Vec<Target>,
    /// Their rectangles, for the work-area rule.
    bars: Vec<[i32; 4]>,
    refusal: Option<Refusal>,
    guard_failures: usize,
    next_guard_try_ms: u64,
    /// Why the guard is not up: the failure of the last pass that really looked for it (or the guard ending). It is what the passes inside
    /// the retry pause report, so that a guard that was never found keeps saying "not found" and does not turn into "lost" for 5, 15,
    /// 60 seconds. `None` while a guard is confirmed or none was wanted yet.
    guard_fault: Option<Refusal>,
    guard_confirmed_ms: u64,
    throttle: Throttle,
    /// The work area this controller last set for each monitor it changed (monitor id, rectangle): a work area that is one of them
    /// is ours (`tbwork::plan_edge_held`). Forgotten when the edge is given back and when the user's rectangles are put back.
    held: Vec<(String, [i32; 4])>,
    /// The "set too often" warning was written for the present stretch of refusals (it is said once, not every second).
    throttle_said: bool,
    /// The dock changed its band and the work area has not followed yet. A set that follows the dock's own word is not held back by
    /// the throttle (which is there against Explorer taking the work area back), and does not use up one of its slots.
    band_owed: bool,
    /// The dock's band thickness in device px (`None`: the dock reserves none, `dock_reserve` is off). Session only.
    band_px: Option<i32>,
    /// The dock has said what its band is (`band_px` is its word, `None` included). Until then the edge is not taken: a guess of "no
    /// band" would give the whole monitor to maximised windows over the dock and cost a second set when the real band arrives.
    band_known: bool,
    edge: Edge,
    /// What the dock was last told its band is.
    dock_edge: Option<[i32; 4]>,
    edge_failures: usize,
    next_edge_try_ms: u64,
    /// The "another toolbar reserves room" line was said already.
    foreign_said: bool,
    /// The last restore did not complete: it is tried again on the timer until it does (or the retries run out).
    restore_pending: bool,
    restore_attempts: u32,
    next_restore_ms: u64,
}

impl Controller {
    pub fn new(host_pid: u32, host_start: u64) -> Controller {
        Controller {
            host_pid,
            host_start,
            want: false,
            guard: GuardStatus::Absent,
            hidden: Vec::new(),
            bars: Vec::new(),
            refusal: None,
            guard_failures: 0,
            next_guard_try_ms: 0,
            guard_fault: None,
            guard_confirmed_ms: 0,
            // Explorer may recompute the work area whenever it likes: give it back at most 4 times a minute.
            throttle: Throttle::new(4, 60_000),
            held: Vec::new(),
            throttle_said: false,
            band_owed: false,
            band_px: None,
            band_known: false,
            edge: Edge::Free,
            dock_edge: None,
            edge_failures: 0,
            next_edge_try_ms: 0,
            foreign_said: false,
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

    /// Where the taking of the bottom edge stands.
    pub fn edge(&self) -> Edge {
        self.edge
    }

    /// The dock was told to let go of its band and has not yet: the host asks for a quicker look than the once-a-second tick.
    pub fn edge_waiting(&self) -> bool {
        self.want && !self.hidden.is_empty() && matches!(self.edge, Edge::Releasing { .. })
    }

    /// The quick look while the dock is letting go: only that, no listing of windows.
    pub fn edge_tick(&mut self, w: &mut dyn HideWorld) {
        if self.edge_waiting() {
            self.drive_edge(w);
        }
    }

    /// The band the dock wants kept free at the bottom of the primary monitor, when it is known at the start (before any window is
    /// hidden); later changes come through [`Controller::set_band`].
    pub fn with_band(mut self, band_px: Option<i32>) -> Controller {
        self.band_px = band_px;
        self.band_known = true;
        self
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
            self.guard_fault = None;
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

    /// `TaskbarCreated` was heard: Explorer may have made its taskbar again, and the handles we knew may be void.
    ///
    /// Not every `TaskbarCreated` is an Explorer restart: our own tray host broadcasts it when it starts and stops. A window that is
    /// still the one we hid (same handle, class, owner and start time) stays in `hidden`; forgetting it would leave the taskbar hidden
    /// and unaccounted for, since only a VISIBLE window is ever hidden again. After a real restart the old windows are gone or
    /// foreign (new process, new start time) and are dropped; the new taskbar is hidden by the pass that follows.
    pub fn explorer_restarted(&mut self, w: &mut dyn HideWorld) {
        if self.want
            && !self.hidden.is_empty()
            && let Ok(live) = w.list_taskbars()
        {
            self.hidden.retain(|t| tbmodel::judge(t, &live) == tbmodel::Verdict::Valid);
            self.refresh_bars(&live);
        }
        self.reconcile(w);
    }

    /// The dock's band changed (its size, or `dock_reserve`): while the edge is ours the work area follows.
    pub fn set_band(&mut self, w: &mut dyn HideWorld, band_px: Option<i32>) {
        if band_px == self.band_px && self.band_known {
            return;
        }
        self.band_px = band_px;
        self.band_known = true;
        self.band_owed = true;
        if self.want && !self.hidden.is_empty() {
            self.drive_edge(w);
        }
    }

    /// Monitors came or went, the work area changed: look again (a new monitor has a new secondary taskbar).
    pub fn displays_changed(&mut self, w: &mut dyn HideWorld) {
        self.reconcile(w);
    }

    /// The work area was changed by someone (Explorer recomputes it when a toolbar changes): while the edge is ours it is set again,
    /// a few times a minute at most.
    pub fn work_area_changed(&mut self, w: &mut dyn HideWorld) {
        if self.want && !self.hidden.is_empty() {
            self.drive_edge(w);
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
            self.guard_fault = Some(Refusal::GuardLost);
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
        self.refresh_bars(&live);
        let to_hide = tbmodel::windows_to_hide(&live);
        if to_hide.is_empty() {
            if self.hidden.is_empty() {
                self.refusal = Some(Refusal::NothingToHide);
                self.release_unheld_edge(w);
            } else {
                self.drive_edge(w);
            }
            return;
        }
        self.hide_pass(w, &live, to_hide);
        self.release_unheld_edge(w);
    }

    /// The edge is ours but no taskbar of ours is hidden any more (Explorer restarted and its new taskbar could not be hidden, or was
    /// not there): the strip is Explorer's again, so the dock gets its band back at once instead of floating over a visible taskbar
    /// with no band and no hidden strip. Nothing else is put back: Explorer computed the work areas anew with the restart, and the
    /// written-down record stays for the way out. Called after a pass that left `hidden` empty; the next pass starts from `Free`.
    fn release_unheld_edge(&mut self, w: &mut dyn HideWorld) {
        if !self.hidden.is_empty() || self.edge == Edge::Free {
            return;
        }
        w.warn("taskbar: no taskbar of ours is hidden any more; the dock gets its band back");
        w.dock_give_back_edge();
        self.dock_edge = None;
        self.edge = Edge::Free;
        self.held.clear();
        self.edge_failures = 0;
        self.next_edge_try_ms = 0;
    }

    /// The rectangles of the taskbar windows we hid, as they are now (a monitor may have changed).
    fn refresh_bars(&mut self, live: &[LiveWindow]) {
        self.bars = self.hidden.iter().filter_map(|t| live.iter().find(|l| l.handle == t.handle).map(|l| l.rect)).collect();
    }

    fn ensure_guard(&mut self, w: &mut dyn HideWorld) -> Result<(), Refusal> {
        let now = w.now_ms();
        if now < self.next_guard_try_ms {
            // inside the pause: the reason is still the one the last look found
            return Err(self.guard_fault.unwrap_or(Refusal::GuardLost));
        }
        if !w.guard_exists() {
            // Looked for again only after a backoff: the file is not going to appear by itself.
            self.note_guard_failure(now);
            self.guard_fault = Some(Refusal::GuardMissing);
            w.warn("taskbar: rustyfiles-shellguard.exe was not found; the taskbar stays");
            return Err(Refusal::GuardMissing);
        }
        if let Err(e) = w.start_guard() {
            self.note_guard_failure(now);
            self.guard_fault = Some(Refusal::GuardNotConfirmed);
            w.warn(&format!("taskbar: the guard did not start: {e}"));
            return Err(Refusal::GuardNotConfirmed);
        }
        if !w.guard_ready(CONFIRM_WAIT_MS) {
            w.kill_guard();
            self.note_guard_failure(now);
            self.guard_fault = Some(Refusal::GuardNotConfirmed);
            w.warn("taskbar: the guard did not confirm; the taskbar stays");
            return Err(Refusal::GuardNotConfirmed);
        }
        self.guard_fault = None;
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
        // Asked to hide: the change is queued in Explorer's own thread, so `Ok` only means "asked".
        let mut asked: Vec<Target> = Vec::new();
        for t in &to_hide {
            match w.hide(t) {
                Ok(()) => asked.push(t.clone()),
                Err(e) => w.warn(&format!("taskbar: a window could not be hidden: {e}")),
            }
        }
        // The hide counts once it is CONFIRMED: the windows are hidden. Nothing else (the dock's edge, the work areas) is touched
        // for a window that was only asked to hide.
        let still_visible = if asked.is_empty() { Vec::new() } else { w.wait_hidden(&asked) };
        let mut done = 0;
        for t in &asked {
            if still_visible.iter().any(|v| v.handle == t.handle) {
                continue;
            }
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
        if !still_visible.is_empty() {
            w.warn(&format!("taskbar: {} window(s) were asked to hide but are still visible; nothing else is changed for them", still_visible.len()));
        }
        if done > 0 {
            self.refusal = None;
            w.log(&format!("taskbar: {done} Explorer taskbar window(s) hidden"));
        }
        if !self.hidden.is_empty() {
            self.drive_edge(w);
        }
    }

    /// The edge: while our hide is in force the dock takes the bottom edge (see `tbwork`). One call does whatever is due next; it is
    /// called after every hide pass, on the timer, when the work area changes and when the dock's band changes. Nothing here runs
    /// unless the hide is confirmed (`hidden` holds only windows that were seen hidden).
    fn drive_edge(&mut self, w: &mut dyn HideWorld) {
        if !self.want || self.hidden.is_empty() {
            return;
        }
        let now = w.now_ms();
        match self.edge {
            Edge::Free => {
                // the dock has not said what its band is: wait for its word (`set_band` drives the edge when it comes)
                if !self.band_known || now < self.next_edge_try_ms {
                    return;
                }
                let mons = w.monitors();
                let plan = tbwork::plan_edge_held(&mons, &self.bars, self.band_px, &self.held_rects());
                if !self.foreign_said && plan.monitors.iter().any(|m| m.verdict == Verdict::Foreign) {
                    self.foreign_said = true;
                    w.warn("taskbar: another toolbar reserves room on a monitor (or the taskbar is not at the bottom); its work area is left alone and the taskbar's strip stays there");
                }
                if !plan.any_ours() {
                    return;
                }
                // 1. The user's own work areas are written down before anything is changed - the dock letting go of its band is a
                //    change, too (Explorer recomputes the work area).
                if !self.write_down_originals(w, &plan) {
                    self.fail_edge(now);
                    return;
                }
                // 2. The dock lets go of its own band and places itself from the monitor's bottom edge.
                if plan.primary_ours()
                    && let Some(edge) = plan.dock_edge(self.band_px)
                {
                    w.dock_take_edge(edge);
                    self.dock_edge = Some(edge);
                    self.edge = Edge::Releasing { since_ms: now };
                    w.log("taskbar: the dock takes the bottom edge; waiting for it to let go of its own band");
                    self.continue_release(w, now);
                } else {
                    // the dock is not on a monitor of ours: only other monitors are set
                    self.edge = Edge::Ours;
                    self.set_areas(w, now);
                }
            }
            Edge::Releasing { .. } => self.continue_release(w, now),
            Edge::Ours => self.hold_edge(w, now),
        }
    }

    /// The dock was told to let go: when it has - and the work area shows that Explorer took the removal - the work areas are set; when
    /// it takes too long it gets its band back and the try is made again after a pause.
    ///
    /// The dock's own word is not enough: it says what it SENT, and with the tray host on its messages go to Explorer through the
    /// host's window. A removal that Explorer did not take would leave its band in the work area, and a work area set on top of it
    /// would be a second strip. So the work area is read, too: while it still is the monitor less the strip less a band, nothing is set.
    fn continue_release(&mut self, w: &mut dyn HideWorld, now: u64) {
        let Edge::Releasing { since_ms } = self.edge else { return };
        if w.dock_edge_ready() && self.explorer_dropped_band(w) {
            self.edge = Edge::Ours;
            self.edge_failures = 0;
            self.set_areas(w, now);
        } else if now.saturating_sub(since_ms) >= EDGE_DOCK_WAIT_MS {
            w.warn("taskbar: the dock did not let go of its band in time (or Explorer still holds it); it gets it back and the edge is tried again later");
            w.dock_give_back_edge();
            self.dock_edge = None;
            self.edge = Edge::Free;
        self.held.clear();
            self.fail_edge(now);
        }
    }

    /// Does the primary monitor's work area show that Explorer holds no band of the dock's any more?
    fn explorer_dropped_band(&mut self, w: &mut dyn HideWorld) -> bool {
        let plan = tbwork::plan_edge(&w.monitors(), &self.bars, self.band_px);
        !plan.primary().is_some_and(|m| m.band_held)
    }

    fn fail_edge(&mut self, now: u64) {
        let i = self.edge_failures.min(EDGE_RETRY_MS.len() - 1);
        self.next_edge_try_ms = now + EDGE_RETRY_MS[i];
        self.edge_failures += 1;
    }

    /// The edge is ours: the dock's strip follows the band, and a work area that Explorer took back is set again.
    fn hold_edge(&mut self, w: &mut dyn HideWorld, now: u64) {
        let mons = w.monitors();
        let plan = tbwork::plan_edge_held(&mons, &self.bars, self.band_px, &self.held_rects());
        if plan.primary_ours()
            && let Some(edge) = plan.dock_edge(self.band_px)
            && self.dock_edge != Some(edge)
        {
            w.dock_take_edge(edge);
            self.dock_edge = Some(edge);
        }
        self.set_changes(w, now, &plan);
    }

    /// Reads the monitors afresh (Explorer recomputed them when the dock let go) and sets what the rules want.
    fn set_areas(&mut self, w: &mut dyn HideWorld, now: u64) {
        let mons = w.monitors();
        let plan = tbwork::plan_edge_held(&mons, &self.bars, self.band_px, &self.held_rects());
        self.set_changes(w, now, &plan);
    }

    /// Writes the user's own work area of every monitor that is or is about to be ours into the state file (with `pending_restore`),
    /// keeping what an earlier record holds. `false`: the file could not be written, and nothing may be changed. Nothing is written
    /// when the file already says it (a settled edge is quiet).
    fn write_down_originals(&mut self, w: &mut dyn HideWorld, plan: &tbwork::EdgePlan) -> bool {
        let mut st = w.load_state();
        let have = st.original_work_areas();
        let originals = tbmodel::originals_to_save(&have, &plan.originals());
        if originals == have && st.pending_restore() {
            return true;
        }
        st.set_original_work_areas(&originals);
        st.set_pending_restore(true);
        match w.save_state(&st) {
            Ok(()) => true,
            Err(e) => {
                w.warn(&format!("taskbar: the state file could not be written; the work area is left alone: {e}"));
                false
            }
        }
    }

    fn held_rects(&self) -> Vec<[i32; 4]> {
        self.held.iter().map(|(_, r)| *r).collect()
    }

    fn set_changes(&mut self, w: &mut dyn HideWorld, now: u64, plan: &tbwork::EdgePlan) {
        let changes = plan.changes();
        if changes.is_empty() {
            self.band_owed = false;
            return;
        }
        // A monitor can become ours after the edge was taken (plugged in, its taskbar hidden by a later pass): its own work area is
        // written down before the first change to it, like the first monitors' were.
        if !self.write_down_originals(w, plan) {
            return;
        }
        // Explorer may recompute the work area whenever it likes; answering every time could fight it for ever. (The dock's own change
        // of its band is not Explorer's doing: the thin dock changes it as windows are maximised and restored, never faster than
        // every two seconds, and the work area must be the one the dock stands under at every one of them.)
        if !std::mem::take(&mut self.band_owed) && !self.throttle.allow(now) {
            if !self.throttle_said {
                self.throttle_said = true;
                w.warn("taskbar: the work area was set too often lately; left alone");
            }
            return;
        }
        self.throttle_said = false;
        for c in &changes {
            match w.hold_work_area(c.from, c.to) {
                Ok(()) => {
                    self.held.retain(|(id, _)| *id != c.id);
                    self.held.push((c.id.clone(), c.to));
                }
                Err(e) => w.warn(&format!("taskbar: a work area could not be set: {e}")),
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
        // Work areas next, from the rectangles written down before they were changed - checked against the monitors as they are now:
        // the screen may have changed in between (a resolution, a scale, a monitor that went), and a rectangle of the old screen
        // applied blindly lands on whatever monitor is nearest. The taskbars are read again, now that they are shown.
        let originals = st.original_work_areas();
        if st.pending_restore() {
            let bars: Vec<[i32; 4]> = w.list_taskbars().unwrap_or_else(|_| live.clone()).iter().filter(|b| b.explorer && b.visible).map(|b| b.rect).collect();
            let back = tbwork::work_restore(&originals, &w.monitors(), &bars);
            if back.refreshed > 0 || back.dropped > 0 {
                w.warn(&format!("taskbar: the screen changed while the taskbar was hidden; {} work area(s) worked out again, {} left to Explorer", back.refreshed, back.dropped));
            }
            let mut bad = 0;
            for rect in &back.put {
                if w.set_work_area(*rect).is_err() {
                    bad += 1;
                }
            }
            if bad > 0 {
                return Err(format!("{bad} work area(s) could not be put back"));
            }
        }
        // The dock registers its band with Explorer again only now: the taskbar is shown and the user's work areas are back, so
        // Explorer computes the band's place from them (registered earlier, the written-down rectangles would overrule it).
        if self.edge != Edge::Free {
            if self.dock_edge.take().is_some() {
                w.dock_give_back_edge();
            }
            self.edge = Edge::Free;
        self.held.clear();
            self.edge_failures = 0;
            self.next_edge_try_ms = 0;
        }
        let mut st = w.load_state();
        st.clear_hidden_taskbar();
        st.set_pending_restore(false);
        st.set_original_work_areas(&[]);
        self.held.clear();
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
