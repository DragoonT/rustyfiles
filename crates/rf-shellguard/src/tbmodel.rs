//! Hiding Explorer's taskbar (shell plan E1): the facts and the decisions, with no Windows in them.
//!
//! What is hidden: the taskbar windows of Explorer, `Shell_TrayWnd` (primary monitor) and every
//! `Shell_SecondaryTrayWnd` (one per other monitor), by making the windows invisible. Nothing else of Windows
//! is changed: not the taskbar's auto-hide state (`ABM_SETSTATE` writes the user's taskbar settings, and whether
//! that is undone on every path cannot be shown without running it), not a registry value, not a setting.
//!
//! The rules this file keeps, each with tests:
//! * A window is acted on only if it still is a window of the recorded class, owned by the recorded Explorer
//!   process (same process id AND same start time, because ids are reused) and that process runs `explorer.exe`
//!   from the Windows folder. A handle that fails any of these is "foreign" and is left alone, whatever the state
//!   file says.
//! * Only windows that were VISIBLE are hidden, and only those are ever shown again: a taskbar that something
//!   else hid before us stays as it was. (That is the rule while a record can be read; the guard's sweep, for a record that
//!   cannot be, shows every hidden taskbar window of Explorer's and so errs towards a visible taskbar.)
//! * The record of what is about to be hidden is written BEFORE the first window is hidden (see `tbctl`).
//! * Hiding is refused unless the guard is confirmed running (`may_hide`).
//! * The work area is only given back when the geometry is unambiguous (`giveback`), at a limited rate.

use crate::statefile::WorkAreaRec;

pub const PRIMARY: &str = "Shell_TrayWnd";
pub const SECONDARY: &str = "Shell_SecondaryTrayWnd";

pub fn is_taskbar_class(class: &str) -> bool {
    class == PRIMARY || class == SECONDARY
}

/// A taskbar-class window as seen right now (read-only facts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveWindow {
    pub handle: isize,
    pub class: String,
    /// Process id of the window's owner.
    pub pid: u32,
    /// Start time of that process (FILETIME ticks), 0 when it could not be read.
    pub start: u64,
    /// The owner runs `explorer.exe` from the Windows folder.
    pub explorer: bool,
    pub visible: bool,
    /// Left, top, right, bottom in screen pixels.
    pub rect: [i32; 4],
}

/// What a state file remembers about a window we hid: enough to tell it from any other window later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub handle: isize,
    pub class: String,
    pub pid: u32,
    pub start: u64,
}

impl Target {
    pub fn of(w: &LiveWindow) -> Target {
        Target { handle: w.handle, class: w.class.clone(), pid: w.pid, start: w.start }
    }
}

/// `hidden_taskbar` in `shell-state.json`: written before the first window is hidden, removed after the last was shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HideRecord {
    /// The dock host that hid them, and when that process started.
    pub host_pid: u32,
    pub host_start: u64,
    /// Wall clock, milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub windows: Vec<Target>,
}

/// Why a recorded window is not acted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    OtherClass,
    OtherOwner,
    /// Same process id, different start time: the id was reused.
    OtherStart,
    /// The owner is not Explorer.
    NotExplorer,
    /// The recorded process start time was unknown (0) or is now unknown: nothing proves it is the same process.
    Unproven,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Still the window we hid: safe to act on.
    Valid,
    /// No window with this handle exists any more.
    Gone,
    /// A window with this handle exists but is not the one we hid.
    Foreign(Why),
}

/// Is `t` still the window we recorded? `live` is the current list of taskbar-class windows; a handle that is not in it
/// is gone (or has another class, which is as good as gone for our purposes: the real `show` re-checks the class).
pub fn judge(t: &Target, live: &[LiveWindow]) -> Verdict {
    let Some(w) = live.iter().find(|w| w.handle == t.handle) else { return Verdict::Gone };
    if w.class != t.class {
        return Verdict::Foreign(Why::OtherClass);
    }
    if w.pid != t.pid {
        return Verdict::Foreign(Why::OtherOwner);
    }
    if t.start == 0 || w.start == 0 {
        return Verdict::Foreign(Why::Unproven);
    }
    if w.start != t.start {
        return Verdict::Foreign(Why::OtherStart);
    }
    if !w.explorer {
        return Verdict::Foreign(Why::NotExplorer);
    }
    Verdict::Valid
}

/// The windows to hide: taskbar class, Explorer's, visible, with a provable owner.
pub fn windows_to_hide(live: &[LiveWindow]) -> Vec<Target> {
    live.iter().filter(|w| is_taskbar_class(&w.class) && w.explorer && w.visible && w.pid != 0 && w.start != 0).map(Target::of).collect()
}

/// What to do about a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestorePlan {
    /// Windows to show (still ours, currently hidden).
    pub show: Vec<Target>,
    /// Still ours and already visible: nothing to do, counted as restored.
    pub visible: Vec<Target>,
    pub gone: Vec<Target>,
    pub foreign: Vec<(Target, Why)>,
}

impl RestorePlan {
    /// Windows that are visible after the plan is carried out.
    pub fn restored_count(&self) -> usize {
        self.show.len() + self.visible.len()
    }
}

pub fn plan_restore(rec: &HideRecord, live: &[LiveWindow]) -> RestorePlan {
    let mut p = RestorePlan { show: vec![], visible: vec![], gone: vec![], foreign: vec![] };
    for t in &rec.windows {
        match judge(t, live) {
            Verdict::Valid => {
                if live.iter().any(|w| w.handle == t.handle && w.visible) {
                    p.visible.push(t.clone());
                } else {
                    p.show.push(t.clone());
                }
            }
            Verdict::Gone => p.gone.push(t.clone()),
            Verdict::Foreign(why) => p.foreign.push((t.clone(), why)),
        }
    }
    p
}

/// The plan for the "no usable record" case: every taskbar window of Explorer that is hidden. Used when the state file is
/// missing or garbled while a restore is due (we cannot know what was hidden, so everything of Explorer's that is hidden is shown).
pub fn plan_sweep(live: &[LiveWindow]) -> Vec<Target> {
    live.iter().filter(|w| is_taskbar_class(&w.class) && w.explorer && !w.visible && w.pid != 0 && w.start != 0).map(Target::of).collect()
}

/// The record after hiding `new` as well: windows already in the record stay, a handle that comes again with another
/// owner replaces the old entry (Explorer restarted and the number was reused).
pub fn merge_record(existing: Option<&HideRecord>, new: &[Target], host_pid: u32, host_start: u64, now_ms: u64) -> HideRecord {
    let mut windows: Vec<Target> = existing.map(|r| r.windows.clone()).unwrap_or_default();
    for t in new {
        windows.retain(|o| o.handle != t.handle);
        windows.push(t.clone());
    }
    HideRecord { host_pid, host_start, at_ms: now_ms, windows }
}

/// Is a record left by another host? `current_start` is the start time of the process now running as `current_pid`
/// (None when it could not be read: then only the id is compared).
pub fn is_stale(rec: &HideRecord, current_pid: u32, current_start: Option<u64>) -> bool {
    if rec.host_pid != current_pid {
        return true;
    }
    match current_start {
        Some(s) if s != 0 && rec.host_start != 0 => s != rec.host_start,
        _ => false,
    }
}

// ---------------------------------------------------------------- the gate

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardStatus {
    /// Never started (or told to finish).
    Absent,
    /// Started and has said it is armed: the host's process handle is open, the state file read.
    Confirmed,
    /// Was confirmed and ended, or never answered.
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    SettingOff,
    /// Explorer does not own the shell window: there is no Explorer taskbar to hide (or we are the shell).
    NoExplorerShell,
    /// `rustyfiles-shellguard.exe` was not found next to the program.
    GuardMissing,
    /// It was started and did not confirm in time.
    GuardNotConfirmed,
    /// It confirmed and then ended.
    GuardLost,
    /// The record could not be written, so nothing may be hidden.
    RecordNotSaved,
    /// Explorer has no visible taskbar window right now.
    NothingToHide,
}

impl Refusal {
    pub fn describe(self) -> &'static str {
        match self {
            Refusal::SettingOff => "the setting is off",
            Refusal::NoExplorerShell => "Explorer does not own the shell window, so there is no Windows taskbar to hide",
            Refusal::GuardMissing => "rustyfiles-shellguard.exe was not found next to the program",
            Refusal::GuardNotConfirmed => "the guard did not confirm that it is running",
            Refusal::GuardLost => "the guard stopped, so the taskbar is shown again",
            Refusal::RecordNotSaved => "the state file could not be written, so nothing may be hidden",
            Refusal::NothingToHide => "Explorer has no visible taskbar window right now",
        }
    }
}

/// Everything that must be true before the first window is hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    pub setting_on: bool,
    pub explorer_shell: bool,
    pub guard: GuardStatus,
    pub record_saved: bool,
}

/// The order matters for the report (the first thing wrong is what the user is told), not for safety: all must hold.
pub fn may_hide(g: &Gate) -> Result<(), Refusal> {
    if !g.setting_on {
        return Err(Refusal::SettingOff);
    }
    if !g.explorer_shell {
        return Err(Refusal::NoExplorerShell);
    }
    match g.guard {
        GuardStatus::Confirmed => {}
        GuardStatus::Absent => return Err(Refusal::GuardNotConfirmed),
        GuardStatus::Lost => return Err(Refusal::GuardLost),
    }
    if !g.record_saved {
        return Err(Refusal::RecordNotSaved);
    }
    Ok(())
}

// ---------------------------------------------------------------- the work area

/// A monitor as the work-area rule needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorGeom {
    /// The device name (`\\.\DISPLAY1`).
    pub id: String,
    pub rect: [i32; 4],
    pub work: [i32; 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkChange {
    pub id: String,
    pub from: [i32; 4],
    pub to: [i32; 4],
}

fn area(a: [i32; 4], b: [i32; 4]) -> i64 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0) as i64;
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0) as i64;
    w * h
}

/// Gives the strip of a hidden taskbar back to programs, in the one case where the geometry is unambiguous: the monitor's work
/// area ends exactly where the taskbar begins (within one pixel). Then the work area is grown to the monitor's edge.
///
/// When another toolbar sits between the taskbar and the work area (our own dock's band does, when it reserves one), the strip
/// the taskbar left is not next to the work area, and which toolbar lies outside which is not something that can be told without
/// running it: NO change is made and the strip stays reserved. (The Windows setting that would be touched is the session's work
/// area, not a stored one: `SPI_SETWORKAREA` without the "update profile" flag.)
pub fn giveback(monitors: &[MonitorGeom], hidden: &[[i32; 4]]) -> Vec<WorkChange> {
    let mut out: Vec<WorkChange> = Vec::new();
    for bar in hidden {
        let Some(m) = monitors.iter().filter(|m| area(m.rect, *bar) > 0).max_by_key(|m| area(m.rect, *bar)) else { continue };
        if out.iter().any(|c| c.id == m.id) {
            continue;
        }
        let (mw, mh) = (m.rect[2] - m.rect[0], m.rect[3] - m.rect[1]);
        let (bw, bh) = (bar[2] - bar[0], bar[3] - bar[1]);
        if mw <= 0 || mh <= 0 || bw <= 0 || bh <= 0 {
            continue;
        }
        let near = |a: i32, b: i32| (a - b).abs() <= 1;
        let mut to = m.work;
        // A bar is on an edge when it spans (nearly) the whole monitor along that edge and touches it.
        let wide = i64::from(bw) * 10 >= i64::from(mw) * 9;
        let tall = i64::from(bh) * 10 >= i64::from(mh) * 9;
        if wide && near(bar[3], m.rect[3]) && bar[1] > m.rect[1] + mh / 2 && near(m.work[3], bar[1]) {
            to[3] = m.rect[3];
        } else if wide && near(bar[1], m.rect[1]) && bar[3] < m.rect[1] + mh / 2 && near(m.work[1], bar[3]) {
            to[1] = m.rect[1];
        } else if tall && near(bar[0], m.rect[0]) && bar[2] < m.rect[0] + mw / 2 && near(m.work[0], bar[2]) {
            to[0] = m.rect[0];
        } else if tall && near(bar[2], m.rect[2]) && bar[0] > m.rect[0] + mw / 2 && near(m.work[2], bar[0]) {
            to[2] = m.rect[2];
        }
        if to != m.work {
            out.push(WorkChange { id: m.id.clone(), from: m.work, to });
        }
    }
    out
}

/// The rectangles to write to the state file before a work area is changed. A rectangle already saved by an earlier, unfinished
/// run is kept: it is the user's real one, the monitor's current rectangle may already be ours.
pub fn originals_to_save(already: &[WorkAreaRec], changes: &[WorkChange]) -> Vec<WorkAreaRec> {
    let mut out = already.to_vec();
    for c in changes {
        if !out.iter().any(|o| o.monitor == c.id) {
            out.push(WorkAreaRec { monitor: c.id.clone(), rect: c.from });
        }
    }
    out
}

/// At most `max` events in any `window_ms`: Explorer may recompute the work area whenever it likes, and giving it back
/// again every time could fight it for ever.
#[derive(Clone, Debug)]
pub struct Throttle {
    stamps: Vec<u64>,
    max: usize,
    window_ms: u64,
}

impl Throttle {
    pub fn new(max: usize, window_ms: u64) -> Throttle {
        Throttle { stamps: Vec::new(), max, window_ms }
    }

    pub fn allow(&mut self, now_ms: u64) -> bool {
        self.stamps.retain(|t| now_ms.saturating_sub(*t) < self.window_ms);
        if self.stamps.len() >= self.max {
            return false;
        }
        self.stamps.push(now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(handle: isize, class: &str, pid: u32, start: u64, visible: bool) -> LiveWindow {
        LiveWindow { handle, class: class.into(), pid, start, explorer: true, visible, rect: [0, 1392, 2560, 1440] }
    }

    fn t(handle: isize, class: &str, pid: u32, start: u64) -> Target {
        Target { handle, class: class.into(), pid, start }
    }

    #[test]
    fn only_visible_explorer_taskbars_with_a_provable_owner_are_hidden() {
        let mut other = live(5, PRIMARY, 100, 7, true);
        other.explorer = false;
        let list = vec![
            live(1, PRIMARY, 100, 7, true),
            live(2, SECONDARY, 100, 7, true),
            live(3, SECONDARY, 100, 7, false), // already hidden by something else: not ours to show later
            live(4, "Other_Class", 100, 7, true),
            other,
            live(6, PRIMARY, 100, 0, true), // start time unreadable: unprovable
        ];
        let got: Vec<isize> = windows_to_hide(&list).iter().map(|t| t.handle).collect();
        assert_eq!(got, vec![1, 2]);
    }

    #[test]
    fn a_recorded_window_is_acted_on_only_if_it_is_still_the_same_window() {
        let rec = t(1, SECONDARY, 100, 7);
        assert_eq!(judge(&rec, &[live(1, SECONDARY, 100, 7, false)]), Verdict::Valid);
        assert_eq!(judge(&rec, &[]), Verdict::Gone);
        assert_eq!(judge(&rec, &[live(2, SECONDARY, 100, 7, false)]), Verdict::Gone);
        assert_eq!(judge(&rec, &[live(1, PRIMARY, 100, 7, false)]), Verdict::Foreign(Why::OtherClass));
        assert_eq!(judge(&rec, &[live(1, SECONDARY, 101, 7, false)]), Verdict::Foreign(Why::OtherOwner));
        assert_eq!(judge(&rec, &[live(1, SECONDARY, 100, 8, false)]), Verdict::Foreign(Why::OtherStart));
        let mut not = live(1, SECONDARY, 100, 7, false);
        not.explorer = false;
        assert_eq!(judge(&rec, &[not]), Verdict::Foreign(Why::NotExplorer));
        // a record from a file that did not carry the start time proves nothing
        assert_eq!(judge(&t(1, SECONDARY, 100, 0), &[live(1, SECONDARY, 100, 7, false)]), Verdict::Foreign(Why::Unproven));
    }

    #[test]
    fn a_restore_plan_shows_the_hidden_counts_the_visible_and_leaves_foreign_windows_alone() {
        let rec = HideRecord { host_pid: 9, host_start: 1, at_ms: 5, windows: vec![t(1, PRIMARY, 100, 7), t(2, SECONDARY, 100, 7), t(3, SECONDARY, 100, 7), t(4, SECONDARY, 55, 7)] };
        let list = vec![live(1, PRIMARY, 100, 7, false), live(2, SECONDARY, 100, 7, true), live(4, SECONDARY, 66, 7, false)];
        let p = plan_restore(&rec, &list);
        assert_eq!(p.show, vec![t(1, PRIMARY, 100, 7)]);
        assert_eq!(p.visible, vec![t(2, SECONDARY, 100, 7)]);
        assert_eq!(p.gone, vec![t(3, SECONDARY, 100, 7)]);
        assert_eq!(p.foreign, vec![(t(4, SECONDARY, 55, 7), Why::OtherOwner)]);
        assert_eq!(p.restored_count(), 2);
    }

    #[test]
    fn the_sweep_shows_only_hidden_taskbars_of_explorer() {
        let mut other = live(3, SECONDARY, 100, 7, false);
        other.explorer = false;
        let got = plan_sweep(&[live(1, PRIMARY, 100, 7, false), live(2, SECONDARY, 100, 7, true), other, live(4, "X", 1, 1, false)]);
        assert_eq!(got, vec![t(1, PRIMARY, 100, 7)]);
    }

    #[test]
    fn merging_keeps_old_entries_and_replaces_a_reused_handle() {
        let old = HideRecord { host_pid: 9, host_start: 1, at_ms: 5, windows: vec![t(1, PRIMARY, 100, 7), t(2, SECONDARY, 100, 7)] };
        let r = merge_record(Some(&old), &[t(2, SECONDARY, 200, 9), t(3, SECONDARY, 200, 9)], 9, 1, 50);
        assert_eq!(r.windows, vec![t(1, PRIMARY, 100, 7), t(2, SECONDARY, 200, 9), t(3, SECONDARY, 200, 9)]);
        assert_eq!(r.at_ms, 50);
        assert_eq!(merge_record(None, &[], 9, 1, 3).windows, vec![]);
    }

    #[test]
    fn a_record_of_another_host_or_of_a_reused_process_id_is_stale() {
        let rec = HideRecord { host_pid: 9, host_start: 100, at_ms: 5, windows: vec![] };
        assert!(!is_stale(&rec, 9, Some(100)));
        assert!(is_stale(&rec, 10, Some(100)));
        assert!(is_stale(&rec, 9, Some(101)), "same id, different start: the id was reused");
        assert!(!is_stale(&rec, 9, None), "the id matches and the start could not be read");
    }

    #[test]
    fn the_gate_refuses_in_order_and_opens_only_when_everything_holds() {
        let ok = Gate { setting_on: true, explorer_shell: true, guard: GuardStatus::Confirmed, record_saved: true };
        assert_eq!(may_hide(&ok), Ok(()));
        assert_eq!(may_hide(&Gate { setting_on: false, ..ok }), Err(Refusal::SettingOff));
        assert_eq!(may_hide(&Gate { explorer_shell: false, ..ok }), Err(Refusal::NoExplorerShell));
        assert_eq!(may_hide(&Gate { guard: GuardStatus::Absent, ..ok }), Err(Refusal::GuardNotConfirmed));
        assert_eq!(may_hide(&Gate { guard: GuardStatus::Lost, ..ok }), Err(Refusal::GuardLost));
        assert_eq!(may_hide(&Gate { record_saved: false, ..ok }), Err(Refusal::RecordNotSaved));
    }

    fn mon(id: &str, rect: [i32; 4], work: [i32; 4]) -> MonitorGeom {
        MonitorGeom { id: id.into(), rect, work }
    }

    #[test]
    fn the_strip_is_given_back_when_the_work_area_ends_where_the_taskbar_begins() {
        let m = mon("A", [0, 0, 2560, 1440], [0, 0, 2560, 1392]);
        let c = giveback(&[m], &[[0, 1392, 2560, 1440]]);
        assert_eq!(c, vec![WorkChange { id: "A".into(), from: [0, 0, 2560, 1392], to: [0, 0, 2560, 1440] }]);
    }

    #[test]
    fn the_other_edges_and_a_negative_origin_work_the_same_way() {
        // portrait monitor to the left of the primary, taskbar on top
        let m = mon("B", [-1440, -200, 0, 2359], [-1440, -152, 0, 2359]);
        let c = giveback(std::slice::from_ref(&m), &[[-1440, -200, 0, -152]]);
        assert_eq!(c[0].to, [-1440, -200, 0, 2359]);
        // taskbar on the left
        let m = mon("C", [0, 0, 1920, 1080], [48, 0, 1920, 1080]);
        assert_eq!(giveback(&[m], &[[0, 0, 48, 1080]])[0].to, [0, 0, 1920, 1080]);
        // taskbar on the right
        let m = mon("D", [0, 0, 1920, 1080], [0, 0, 1872, 1080]);
        assert_eq!(giveback(&[m], &[[1872, 0, 1920, 1080]])[0].to, [0, 0, 1920, 1080]);
    }

    #[test]
    fn no_change_when_another_toolbar_lies_between_the_taskbar_and_the_work_area() {
        // our dock's band (1320..1392) reserved above the taskbar: the work area ends at 1320, not at 1392
        let m = mon("A", [0, 0, 2560, 1440], [0, 0, 2560, 1320]);
        assert!(giveback(&[m], &[[0, 1392, 2560, 1440]]).is_empty());
    }

    #[test]
    fn no_change_for_a_taskbar_that_is_not_on_an_edge_or_a_monitor_that_is_not_found() {
        let m = mon("A", [0, 0, 2560, 1440], [0, 0, 2560, 1392]);
        assert!(giveback(std::slice::from_ref(&m), &[[100, 1392, 900, 1440]]).is_empty(), "a bar that does not span the monitor");
        assert!(giveback(std::slice::from_ref(&m), &[[0, 600, 2560, 648]]).is_empty(), "not touching an edge");
        assert!(giveback(&[m], &[[5000, 1392, 7560, 1440]]).is_empty(), "no monitor under it");
        assert!(giveback(&[], &[[0, 1392, 2560, 1440]]).is_empty());
    }

    #[test]
    fn two_monitors_are_decided_one_by_one() {
        let a = mon("A", [0, 0, 2560, 1440], [0, 0, 2560, 1392]);
        let b = mon("B", [-1440, -200, 0, 2359], [-1440, -200, 0, 2359]); // its taskbar is not reserving anything
        let c = giveback(&[a, b], &[[0, 1392, 2560, 1440], [-1440, 2311, 0, 2359]]);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].id, "A");
    }

    #[test]
    fn saved_originals_keep_the_first_rectangle_of_an_unfinished_run() {
        let already = vec![WorkAreaRec { monitor: "A".into(), rect: [0, 0, 2560, 1392] }];
        let ch = vec![
            WorkChange { id: "A".into(), from: [0, 0, 2560, 1440], to: [0, 0, 2560, 1440] },
            WorkChange { id: "B".into(), from: [1, 2, 3, 4], to: [1, 2, 3, 5] },
        ];
        let got = originals_to_save(&already, &ch);
        assert_eq!(got, vec![WorkAreaRec { monitor: "A".into(), rect: [0, 0, 2560, 1392] }, WorkAreaRec { monitor: "B".into(), rect: [1, 2, 3, 4] }]);
    }

    #[test]
    fn the_throttle_allows_a_few_per_window_and_forgets_old_ones() {
        let mut th = Throttle::new(3, 60_000);
        assert!(th.allow(0) && th.allow(1_000) && th.allow(2_000));
        assert!(!th.allow(3_000));
        assert!(th.allow(60_500), "the first one has aged out");
        assert!(!th.allow(60_600));
    }
}
