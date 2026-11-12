//! The life of the tray window, in an order that cannot be got wrong: start makes the window and THEN asks programs to add their
//! icons; stop destroys the window FIRST and THEN broadcasts, so that the programs that answer the broadcast find Explorer's window
//! first and every one of them is back with Explorer. `Drop` is a stop. A stop that finds nothing to stop does nothing, not even the
//! broadcast.
//!
//! It also decides what the periodic tick (250 ms, on the host's own thread) does: keep ours the first `Shell_TrayWnd` a program
//! finds, copy Explorer's rectangles onto ours now and then, and say when it is time for the sweep of dead owners. The tick takes no
//! clock: the counts are the schedule.

use super::traits::{Broadcast, TrayWindow};

/// The tick period.
pub const TICK_MS: u32 = 250;
/// Mirror Explorer's rectangles every this many ticks (4 s).
pub const MIRROR_EVERY: u32 = 16;
/// Sweep for dead owners every this many ticks (5 s).
pub const SWEEP_EVERY: u32 = 20;

/// Whether a window property of Explorer's `Shell_TrayWnd` is copied onto ours. Programs (and Windows' own parts) that find the
/// taskbar through the first `Shell_TrayWnd` read FACTS about it from these: the window of the taskbar buttons (`TaskbandHWND`, which
/// ITaskbarList progress and overlay messages are sent to), its monitor, whether it is shown or auto-hidden, its DPI. Everything else
/// on Explorer's window is its own state and means nothing, or something wrong, on ours: the OLE drop-target interface pointer and
/// end-point id, the accessibility (`MSAA_*`) marker, `SysSetRedraw`, the `WindowOnEdge:*` docking state, the focus-consent flag.
///
/// The names are the 21 that Explorer's window has on this machine (read-only probe, build 26300). A name this does not know is not
/// copied: a missing fact is better than a wrong one.
pub fn should_mirror_prop(name: &str) -> bool {
    matches!(name, "TaskbandHWND" | "TaskbarMonitor" | "TaskbarVisible" | "IsAutoHideEnabled" | "LastAutoHideBarStuckMonitor") || name.starts_with("TaskbarDPI_")
}

/// Of the properties we copied onto our window (`ours`, names with their NUL), those Explorer's window no longer has: they are removed
/// from ours, or a fact Explorer withdrew (`TaskbarVisible`) would stay on ours for good. When Explorer's list is EMPTY nothing is
/// removed: no taskbar window has no properties, so an empty list means the enumeration failed, not that every fact went away.
pub fn props_to_remove(ours: &[Vec<u16>], explorer: &[Vec<u16>]) -> Vec<Vec<u16>> {
    if explorer.is_empty() {
        return Vec::new();
    }
    ours.iter().filter(|o| !explorer.contains(o)).cloned().collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickWork {
    /// The window was not first and was raised.
    pub raised: bool,
    pub mirrored: bool,
    /// The caller should sweep for dead owners now.
    pub sweep: bool,
}

pub struct Lifecycle<W: TrayWindow, B: Broadcast> {
    window: W,
    broadcast: B,
    started: bool,
    ticks: u32,
    broadcast_on_start: bool,
}

impl<W: TrayWindow, B: Broadcast> Lifecycle<W, B> {
    pub fn new(window: W, broadcast: B, broadcast_on_start: bool) -> Lifecycle<W, B> {
        Lifecycle { window, broadcast, started: false, ticks: 0, broadcast_on_start }
    }

    /// Makes the window, then (unless configured off) asks every program to add its icons again. If the window cannot be made
    /// nothing is broadcast.
    pub fn start(&mut self, explorer_tray: Option<isize>) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        self.window.create()?;
        self.started = true;
        self.window.mirror(explorer_tray);
        if self.broadcast_on_start {
            self.broadcast.taskbar_created();
        }
        Ok(())
    }

    /// Destroys the window first, then broadcasts. Safe to call twice and when `start` failed: then nothing happens at all.
    pub fn stop(&mut self) {
        if !self.started {
            // (a window could still exist if `start` made it and failed afterwards; destroying it is always right)
            self.window.destroy();
            return;
        }
        self.started = false;
        self.window.destroy();
        self.broadcast.taskbar_created();
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn window(&self) -> &W {
        &self.window
    }

    pub fn window_mut(&mut self) -> &mut W {
        &mut self.window
    }

    /// One tick. `explorer_tray` is Explorer's tray window as the relay knows it.
    pub fn tick(&mut self, explorer_tray: Option<isize>) -> TickWork {
        if !self.started {
            return TickWork::default();
        }
        self.ticks = self.ticks.wrapping_add(1);
        let mut work = TickWork::default();
        if !self.window.is_first() {
            self.window.raise();
            work.raised = true;
        }
        if self.ticks.is_multiple_of(MIRROR_EVERY) {
            self.window.mirror(explorer_tray);
            work.mirrored = true;
        }
        work.sweep = self.ticks.is_multiple_of(SWEEP_EVERY);
        work
    }

    /// Explorer made its taskbar again (`TaskbarCreated` reached us): its window may be on top now, and its rectangles are new.
    pub fn on_taskbar_created(&mut self, explorer_tray: Option<isize>) {
        if !self.started {
            return;
        }
        if !self.window.is_first() {
            self.window.raise();
        }
        self.window.mirror(explorer_tray);
    }
}

impl<W: TrayWindow, B: Broadcast> Drop for Lifecycle<W, B> {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::super::fakes::{CallLog, FakeBroadcast, FakeWindow, new_log};
    use super::*;

    fn make(log: &CallLog, broadcast_on_start: bool) -> (Lifecycle<FakeWindow, FakeBroadcast>, FakeWindow) {
        let w = FakeWindow::new(log.clone());
        (Lifecycle::new(w.clone(), FakeBroadcast { log: log.clone() }, broadcast_on_start), w)
    }

    #[test]
    fn only_facts_about_the_taskbar_are_copied() {
        // the property names Explorer's window has on the research machine
        let all = [
            "LastAutoHideBarStuckMonitor",
            "AllowConsentToStealFocus",
            "TaskbarDPI_NotificationArea",
            "TaskbarDPI_Deskband",
            "TaskbarDPI_AllTaskbars",
            "TaskbarMonitor",
            "TaskbarVisible",
            "IsAutoHideEnabled",
            "MSAA_*FCFFFFFF00000000",
            "TaskbandHWND",
            "SysSetRedraw",
            "OleEndPointID",
            "OleDropTargetInterface",
            "WindowOnEdge:03300e5f:3",
            "WindowOnEdge:306b21a5:0",
        ];
        let copied: Vec<&str> = all.iter().copied().filter(|n| should_mirror_prop(n)).collect();
        assert_eq!(
            copied,
            vec!["LastAutoHideBarStuckMonitor", "TaskbarDPI_NotificationArea", "TaskbarDPI_Deskband", "TaskbarDPI_AllTaskbars", "TaskbarMonitor", "TaskbarVisible", "IsAutoHideEnabled", "TaskbandHWND"]
        );
        assert!(should_mirror_prop("TaskbandHWND"), "the one that matters most");
        assert!(!should_mirror_prop("taskbandhwnd"), "names compare exactly");
        assert!(!should_mirror_prop(""));
    }

    #[test]
    fn a_property_explorer_withdrew_is_removed_from_ours() {
        let n = |t: &str| -> Vec<u16> { t.encode_utf16().chain([0]).collect() };
        let ours = vec![n("TaskbandHWND"), n("TaskbarVisible")];
        assert_eq!(props_to_remove(&ours, &[n("TaskbandHWND"), n("SysSetRedraw")]), vec![n("TaskbarVisible")]);
        assert!(props_to_remove(&ours, &[n("TaskbarVisible"), n("TaskbandHWND")]).is_empty());
        assert!(props_to_remove(&ours, &[]).is_empty(), "an empty list is a failed enumeration, not a withdrawal of everything");
        assert!(props_to_remove(&[], &[n("TaskbandHWND")]).is_empty());
    }

    #[test]
    fn start_makes_the_window_then_asks_for_the_icons() {
        let log = new_log();
        let (mut l, _w) = make(&log, true);
        l.start(Some(5)).unwrap();
        assert_eq!(*log.lock(), vec!["window created", "window mirrored Some(5)", "broadcast TaskbarCreated"]);
        assert!(l.is_started());
    }

    #[test]
    fn stop_destroys_the_window_before_it_broadcasts() {
        let log = new_log();
        let (mut l, w) = make(&log, true);
        l.start(None).unwrap();
        log.lock().clear();
        l.stop();
        assert_eq!(*log.lock(), vec!["window destroyed", "broadcast TaskbarCreated"], "the order is the point");
        assert!(!w.exists());
        // a second stop does nothing at all
        log.lock().clear();
        l.stop();
        assert!(log.lock().is_empty());
    }

    #[test]
    fn drop_is_a_stop() {
        let log = new_log();
        let (mut l, w) = make(&log, false);
        l.start(None).unwrap();
        log.lock().clear();
        drop(l);
        assert_eq!(*log.lock(), vec!["window destroyed", "broadcast TaskbarCreated"]);
        assert!(!w.exists());
    }

    #[test]
    fn a_window_that_cannot_be_made_broadcasts_nothing_then_or_later() {
        let log = new_log();
        let (mut l, w) = make(&log, true);
        w.fail_create();
        assert!(l.start(None).is_err());
        assert!(!l.is_started());
        l.stop();
        drop(l);
        assert!(log.lock().is_empty(), "no window, no broadcast: {:?}", log.lock());
    }

    #[test]
    fn the_start_broadcast_can_be_left_out_but_the_stop_one_cannot() {
        let log = new_log();
        let (mut l, _w) = make(&log, false);
        l.start(None).unwrap();
        assert!(!log.lock().iter().any(|e| e.starts_with("broadcast")));
        l.stop();
        assert!(log.lock().iter().any(|e| e.starts_with("broadcast")));
    }

    #[test]
    fn start_twice_is_one_start() {
        let log = new_log();
        let (mut l, _w) = make(&log, true);
        l.start(None).unwrap();
        l.start(None).unwrap();
        assert_eq!(log.lock().iter().filter(|e| e.as_str() == "window created").count(), 1);
        assert_eq!(log.lock().iter().filter(|e| e.starts_with("broadcast")).count(), 1);
    }

    #[test]
    fn the_tick_raises_only_when_ours_is_not_first() {
        let log = new_log();
        let (mut l, w) = make(&log, false);
        l.start(None).unwrap();
        log.lock().clear();
        assert_eq!(l.tick(None), TickWork::default());
        assert!(log.lock().is_empty());
        w.set_first(false);
        let t = l.tick(None);
        assert!(t.raised);
        assert_eq!(*log.lock(), vec!["window raised"]);
        assert!(l.tick(None) == TickWork::default(), "raised once, first now");
    }

    #[test]
    fn mirror_and_sweep_follow_their_counts() {
        let log = new_log();
        let (mut l, _w) = make(&log, false);
        l.start(None).unwrap();
        let mut mirrors = vec![];
        let mut sweeps = vec![];
        for n in 1..=80u32 {
            let t = l.tick(Some(9));
            if t.mirrored {
                mirrors.push(n);
            }
            if t.sweep {
                sweeps.push(n);
            }
        }
        assert_eq!(mirrors, vec![16, 32, 48, 64, 80]);
        assert_eq!(sweeps, vec![20, 40, 60, 80]);
    }

    #[test]
    fn a_stopped_lifecycle_ticks_into_nothing() {
        let log = new_log();
        let (mut l, _w) = make(&log, false);
        assert_eq!(l.tick(None), TickWork::default());
        l.on_taskbar_created(None);
        assert!(log.lock().is_empty());
    }

    #[test]
    fn taskbar_created_raises_if_needed_and_mirrors() {
        let log = new_log();
        let (mut l, w) = make(&log, false);
        l.start(None).unwrap();
        log.lock().clear();
        w.set_first(false);
        l.on_taskbar_created(Some(3));
        assert_eq!(*log.lock(), vec!["window raised", "window mirrored Some(3)"]);
    }
}
