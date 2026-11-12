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
    pub monitors: Vec<MonitorGeom>,
    pub work_sets: Vec<[i32; 4]>,
    pub work_fails: bool,
    pub state: ShellStateFile,
    pub save_fails: bool,
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
            monitors: vec![],
            work_sets: vec![],
            work_fails: false,
            state: ShellStateFile::empty(),
            save_fails: false,
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
    fn load_state(&mut self) -> ShellStateFile {
        self.state.clone()
    }
    fn save_state(&mut self, f: &ShellStateFile) -> Result<(), String> {
        self.calls.push("save".into());
        if self.save_fails {
            return Err("disk full".into());
        }
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
