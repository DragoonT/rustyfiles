//! A pretend desktop for the taskbar-hiding tests: a list of windows that `hide` and `show` change, and a [`FakeWorld`] that
//! lets the host's controller run against it. The guard's `NullTaskbar` can share the same desktop, so a test can say "the host
//! hid the taskbar, the host died, the guard ran, the windows are visible".
//!
//! Nothing here touches Windows.

use std::cell::RefCell;
use std::rc::Rc;

use crate::statefile::ShellStateFile;
use crate::tbctl::HideWorld;
use crate::tbmodel::{LiveWindow, MonitorGeom, PRIMARY, SECONDARY, Target};

pub type Desktop = Rc<RefCell<Vec<LiveWindow>>>;

pub const EXPLORER_PID: u32 = 8800;
pub const EXPLORER_START: u64 = 133_699_000_000_000_001;

pub fn window(handle: isize, class: &str, rect: [i32; 4]) -> LiveWindow {
    LiveWindow { handle, class: class.into(), pid: EXPLORER_PID, start: EXPLORER_START, explorer: true, visible: true, rect }
}

/// This machine's shape: a 2560 x 1440 primary monitor with the taskbar on the bottom edge, and a portrait monitor to its left
/// with a negative origin.
pub fn two_monitor_desktop() -> Desktop {
    Rc::new(RefCell::new(vec![window(0x1001, PRIMARY, [0, 1392, 2560, 1440]), window(0x1002, SECONDARY, [-1440, 2311, 0, 2359])]))
}

pub fn visible_handles(d: &Desktop) -> Vec<isize> {
    d.borrow().iter().filter(|w| w.visible).map(|w| w.handle).collect()
}

/// What the real `show`/`hide` check again at the moment of the call: the window exists, has the class and the owner.
pub fn set_visible(d: &Desktop, t: &Target, visible: bool) -> Result<(), String> {
    let mut d = d.borrow_mut();
    match d.iter_mut().find(|w| w.handle == t.handle) {
        Some(w) if w.class == t.class && w.pid == t.pid => {
            w.visible = visible;
            Ok(())
        }
        Some(_) => Err("not the same window any more".into()),
        None => Err("no such window".into()),
    }
}

pub struct FakeWorld {
    pub desktop: Desktop,
    pub now: u64,
    pub explorer_shell: bool,
    pub list_fails: bool,
    pub hide_fails: Vec<isize>,
    pub show_fails: Vec<isize>,
    /// Windows whose `show` is accepted (`Ok`) and then never happens: Explorer is hung or dropped the queued request.
    pub show_lost: Vec<isize>,
    /// Windows whose `show` is accepted and takes effect only a moment later, while the caller waits for it.
    pub show_late: Vec<isize>,
    queued_shows: Vec<Target>,
    /// Windows whose `hide` is accepted (`Ok`) and then never happens: Explorer is busy and drops the queued request.
    pub hide_lost: Vec<isize>,
    /// Windows whose `hide` is accepted and takes effect only at the next `wait_hidden` (a moment later).
    pub hide_late: Vec<isize>,
    queued_hides: Vec<Target>,
    /// What the dock was last told its band is (`None`: it was not told, or it was given back).
    pub dock_edge: Option<[i32; 4]>,
    /// How many `dock_edge_ready` looks answer "not yet" after the dock was told, before it has let go.
    pub dock_release_after: u32,
    /// The dock never lets go (a dock that is stuck).
    pub dock_stuck: bool,
    /// The dock says it let go, but Explorer did not take the removal (its message did not arrive, or was refused on the way through
    /// the tray host): the band stays in the work area.
    pub explorer_keeps_band: bool,
    dock_looks: u32,
    /// The band (thickness) the dock has registered with Explorer now: it is part of the primary monitor's work area.
    pub dock_band: Option<i32>,
    /// The band the dock registers again when it is given the edge back.
    pub dock_wants_band: Option<i32>,
    pub monitors: Vec<MonitorGeom>,
    pub work_sets: Vec<[i32; 4]>,
    pub work_fails: bool,
    pub state: ShellStateFile,
    pub save_fails: bool,
    /// The state file takes this many saves and refuses the later ones (the disk fills up between the record and the originals).
    pub save_fails_after: Option<usize>,
    saves_done: usize,
    pub guard_exists: bool,
    pub guard_start_ok: bool,
    pub guard_ready_ok: bool,
    pub guard_running: bool,
    pub guard_starts: usize,
    pub guard_done: bool,
    pub guard_killed: bool,
    pub calls: Vec<String>,
    pub lines: Vec<String>,
}

impl FakeWorld {
    pub fn new(desktop: Desktop) -> FakeWorld {
        FakeWorld {
            desktop,
            now: 1_000_000,
            explorer_shell: true,
            list_fails: false,
            hide_fails: vec![],
            show_fails: vec![],
            show_lost: vec![],
            show_late: vec![],
            queued_shows: vec![],
            hide_lost: vec![],
            hide_late: vec![],
            queued_hides: vec![],
            dock_edge: None,
            dock_release_after: 0,
            dock_stuck: false,
            explorer_keeps_band: false,
            dock_looks: 0,
            dock_band: None,
            dock_wants_band: None,
            monitors: vec![],
            work_sets: vec![],
            work_fails: false,
            state: ShellStateFile::empty(),
            save_fails: false,
            save_fails_after: None,
            saves_done: 0,
            guard_exists: true,
            guard_start_ok: true,
            guard_ready_ok: true,
            guard_running: false,
            guard_starts: 0,
            guard_done: false,
            guard_killed: false,
            calls: vec![],
            lines: vec![],
        }
    }

    pub fn count(&self, prefix: &str) -> usize {
        self.calls.iter().filter(|c| c.starts_with(prefix)).count()
    }

    /// Explorer computes the primary monitor's work area from its toolbars: the taskbar's strip if it is shown OR hidden (a hidden
    /// taskbar keeps its strip), less the dock's band when one is registered.
    pub fn explorer_recomputes(&mut self, band: Option<i32>) {
        let bar_top = self.desktop.borrow().iter().find(|w| w.class == PRIMARY).map(|w| w.rect[1]);
        if let (Some(top), Some(m)) = (bar_top, self.monitors.iter_mut().find(|m| m.primary)) {
            m.work = [m.rect[0], m.rect[1], m.rect[2], top - band.unwrap_or(0)];
        }
    }

    pub fn pos(&self, prefix: &str) -> Option<usize> {
        self.calls.iter().position(|c| c.starts_with(prefix))
    }
}

impl HideWorld for FakeWorld {
    fn now_ms(&mut self) -> u64 {
        self.now
    }
    fn log(&mut self, msg: &str) {
        self.lines.push(msg.to_string());
    }
    fn explorer_owns_shell(&mut self) -> bool {
        self.explorer_shell
    }
    fn list_taskbars(&mut self) -> Result<Vec<LiveWindow>, String> {
        self.calls.push("list".into());
        if self.list_fails { Err("cannot list".into()) } else { Ok(self.desktop.borrow().clone()) }
    }
    fn hide(&mut self, t: &Target) -> Result<(), String> {
        self.calls.push(format!("hide:{:x}", t.handle));
        if self.hide_fails.contains(&t.handle) {
            return Err("refused".into());
        }
        // Like the real, queued call: `Ok` says "asked", the window changes later or never.
        if self.hide_lost.contains(&t.handle) {
            return Ok(());
        }
        if self.hide_late.contains(&t.handle) {
            self.queued_hides.push(t.clone());
            return Ok(());
        }
        set_visible(&self.desktop, t, false)
    }
    fn show(&mut self, t: &Target) -> Result<(), String> {
        self.calls.push(format!("show:{:x}", t.handle));
        if self.show_fails.contains(&t.handle) {
            return Err("refused".into());
        }
        // Like the real, queued call: `Ok` says "asked", the window changes later or never.
        if self.show_lost.contains(&t.handle) {
            return Ok(());
        }
        if self.show_late.contains(&t.handle) {
            self.queued_shows.push(t.clone());
            return Ok(());
        }
        set_visible(&self.desktop, t, true)
    }
    fn wait_visible(&mut self, targets: &[Target]) -> usize {
        self.calls.push("wait_visible".into());
        for t in std::mem::take(&mut self.queued_shows) {
            let _ = set_visible(&self.desktop, &t, true);
        }
        let d = self.desktop.borrow();
        targets.iter().filter(|t| d.iter().any(|w| w.handle == t.handle && !w.visible)).count()
    }
    fn wait_hidden(&mut self, targets: &[Target]) -> Vec<Target> {
        self.calls.push("wait_hidden".into());
        for t in std::mem::take(&mut self.queued_hides) {
            let _ = set_visible(&self.desktop, &t, false);
        }
        let d = self.desktop.borrow();
        targets.iter().filter(|t| d.iter().any(|w| w.handle == t.handle && w.visible)).cloned().collect()
    }
    fn monitors(&mut self) -> Vec<MonitorGeom> {
        self.monitors.clone()
    }
    fn set_work_area(&mut self, rect: [i32; 4]) -> Result<(), String> {
        self.calls.push(format!("work:{},{},{},{}", rect[0], rect[1], rect[2], rect[3]));
        self.work_sets.push(rect);
        if self.work_fails {
            return Err("busy".into());
        }
        // The monitor that holds the rectangle's centre takes it as its work area, as the system would.
        let (cx, cy) = ((rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2);
        if let Some(m) = self.monitors.iter_mut().find(|m| cx >= m.rect[0] && cx < m.rect[2] && cy >= m.rect[1] && cy < m.rect[3]) {
            m.work = rect;
        }
        Ok(())
    }
    fn dock_take_edge(&mut self, edge: [i32; 4]) {
        self.calls.push(format!("dock.take:{},{},{},{}", edge[0], edge[1], edge[2], edge[3]));
        if self.dock_edge.is_none() {
            self.dock_looks = 0;
        }
        self.dock_edge = Some(edge);
    }
    fn dock_edge_ready(&mut self) -> bool {
        self.calls.push("dock.ready?".into());
        self.dock_looks += 1;
        let ready = !self.dock_stuck && self.dock_looks > self.dock_release_after;
        if ready && !self.explorer_keeps_band && self.dock_band.take().is_some() {
            // Explorer took the registration away and computed the work area from its taskbar alone
            self.explorer_recomputes(None);
        }
        ready
    }
    fn dock_give_back_edge(&mut self) {
        self.calls.push("dock.give".into());
        self.dock_edge = None;
        self.dock_band = self.dock_wants_band;
        self.explorer_recomputes(self.dock_band);
    }
    fn load_state(&mut self) -> ShellStateFile {
        self.state.clone()
    }
    fn save_state(&mut self, f: &ShellStateFile) -> Result<(), String> {
        self.calls.push("save".into());
        if self.save_fails || self.save_fails_after.is_some_and(|n| self.saves_done >= n) {
            return Err("disk full".into());
        }
        self.saves_done += 1;
        self.state = f.clone();
        Ok(())
    }
    fn guard_exists(&mut self) -> bool {
        self.guard_exists
    }
    fn start_guard(&mut self) -> Result<(), String> {
        self.calls.push("guard.start".into());
        self.guard_starts += 1;
        if self.guard_start_ok {
            self.guard_running = true;
            Ok(())
        } else {
            Err("cannot start".into())
        }
    }
    fn guard_ready(&mut self, _wait_ms: u64) -> bool {
        self.calls.push("guard.ready".into());
        self.guard_ready_ok && self.guard_running
    }
    fn guard_alive(&mut self) -> bool {
        self.guard_running
    }
    fn tell_guard_done(&mut self) {
        self.calls.push("guard.done".into());
        self.guard_done = true;
        self.guard_running = false;
    }
    fn kill_guard(&mut self) {
        self.calls.push("guard.kill".into());
        self.guard_killed = true;
        self.guard_running = false;
    }
}
