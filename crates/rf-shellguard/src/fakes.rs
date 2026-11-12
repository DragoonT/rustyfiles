//! In-memory stand-ins for the system traits. The tests drive the whole runner with them, and
//! `--test-shell` uses `MemRegistry`, `NullWorkAreas` and `NullTaskbar` so that mode can never
//! reach the user's registry, work areas or taskbar.
//!
//! Every fake appends a line to one shared record, so a test can assert the order of calls across
//! different traits ("the Shell value is deleted before Explorer is started").

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use crate::machine::{Message, Millis};
use crate::statefile::{ShellStateFile, WorkAreaRec};
use crate::sys::{Clock, Hotkey, HostLaunch, HostLink, PidWait, Processes, Registry, Session, StateStore, Sys, TaskbarVisibility, Ui, Wait, WorkAreas};
use crate::tbfakes::{Desktop, set_visible};
use crate::tbmodel::{self, HideRecord};

pub type Rec = Rc<RefCell<Vec<String>>>;

pub fn new_rec() -> Rec {
    Rc::new(RefCell::new(Vec::new()))
}

#[derive(Clone)]
pub struct FakeClock(pub Rc<Cell<Millis>>);

impl Clock for FakeClock {
    fn now_ms(&self) -> Millis {
        self.0.get()
    }
}

/// A registry that lives in memory.
pub struct MemRegistry {
    pub shell: Option<String>,
    pub policy: Option<u32>,
    pub fail_delete: bool,
    rec: Rec,
}

impl MemRegistry {
    pub fn new(shell: Option<&str>, rec: Rec) -> MemRegistry {
        MemRegistry { shell: shell.map(str::to_owned), policy: None, fail_delete: false, rec }
    }
}

impl Registry for MemRegistry {
    fn read_shell_value(&mut self) -> Result<Option<String>, String> {
        Ok(self.shell.clone())
    }
    fn read_override_policy(&mut self) -> Result<Option<u32>, String> {
        Ok(self.policy)
    }
    fn delete_shell_value(&mut self) -> Result<bool, String> {
        self.rec.borrow_mut().push("registry.delete_shell_value".into());
        if self.fail_delete {
            return Err("access denied".into());
        }
        Ok(self.shell.take().is_some())
    }
}

/// A scripted process side. Each wait pops one `(clock advance, result)` entry; running out of script panics
/// (a test that loops forever would otherwise hang).
pub struct ScriptProcs {
    pub waits: VecDeque<(Millis, Wait)>,
    pub pid_waits: VecDeque<(Millis, PidWait)>,
    pub spawn: VecDeque<Result<u32, String>>,
    pub explorer_start: VecDeque<bool>,
    /// `explorer_shell_up` answers; an empty script answers true.
    pub explorer_up: VecDeque<bool>,
    pub hung: VecDeque<bool>,
    pub self_test: bool,
    /// Explorer already owns the desktop when the guard looks at boot.
    pub explorer_is_shell: bool,
    pub last_pid: u32,
    /// Start times by process id, for `start_time`.
    pub start_times: Vec<(u32, u64)>,
    rec: Rec,
    clock: Rc<Cell<Millis>>,
}

impl ScriptProcs {
    pub fn new(rec: Rec, clock: Rc<Cell<Millis>>) -> ScriptProcs {
        ScriptProcs {
            waits: VecDeque::new(),
            pid_waits: VecDeque::new(),
            spawn: VecDeque::new(),
            explorer_start: VecDeque::new(),
            explorer_up: VecDeque::new(),
            hung: VecDeque::new(),
            self_test: true,
            explorer_is_shell: false,
            last_pid: 100,
            start_times: Vec::new(),
            rec,
            clock,
        }
    }
}

impl Processes for ScriptProcs {
    fn spawn_host(&mut self, launch: &HostLaunch) -> Result<u32, String> {
        self.rec.borrow_mut().push(format!("procs.spawn_host {}", launch.exe.display()));
        match self.spawn.pop_front() {
            Some(r) => r,
            None => {
                self.last_pid += 1;
                Ok(self.last_pid)
            }
        }
    }
    fn wait(&mut self, _extra: isize, _timeout_ms: Option<Millis>) -> Wait {
        let (adv, w) = self.waits.pop_front().expect("test script ran out of waits");
        self.clock.set(self.clock.get() + adv);
        w
    }
    fn wait_pid(&mut self, pid: u32, _extra: isize, timeout_ms: Option<Millis>) -> PidWait {
        if pid == 0 {
            // A plain timed wait.
            self.clock.set(self.clock.get() + timeout_ms.unwrap_or(0));
            return PidWait::Timeout;
        }
        match self.pid_waits.pop_front() {
            Some((adv, r)) => {
                self.clock.set(self.clock.get() + adv);
                r
            }
            None => PidWait::Gone,
        }
    }
    fn start_time(&mut self, pid: u32) -> Option<u64> {
        self.start_times.iter().find(|(p, _)| *p == pid).map(|(_, t)| *t)
    }
    fn host_hung(&mut self) -> bool {
        self.hung.pop_front().unwrap_or(false)
    }
    fn kill_host(&mut self) {
        self.rec.borrow_mut().push("procs.kill_host".into());
    }
    fn adopt_host(&mut self, pid: u32, _image: &std::path::Path) {
        self.rec.borrow_mut().push(format!("procs.adopt_host {pid}"));
    }
    fn explorer_owns_shell_window(&mut self) -> bool {
        self.rec.borrow_mut().push("procs.explorer_owns_shell_window".into());
        self.explorer_is_shell
    }
    fn host_self_test(&mut self, _launch: &HostLaunch) -> bool {
        self.rec.borrow_mut().push("procs.host_self_test".into());
        self.self_test
    }
    fn start_explorer(&mut self) -> Result<(), String> {
        self.rec.borrow_mut().push("procs.start_explorer".into());
        if self.explorer_start.pop_front().unwrap_or(true) { Ok(()) } else { Err("cannot start".into()) }
    }
    fn explorer_shell_up(&mut self, wait_ms: Millis) -> bool {
        let up = self.explorer_up.pop_front().unwrap_or(true);
        self.rec.borrow_mut().push(format!("procs.explorer_shell_up({wait_ms}) -> {up}"));
        up
    }
}

pub struct FakeHotkey {
    pub arm_ok: bool,
    pub pressed: VecDeque<bool>,
    rec: Rec,
}

impl FakeHotkey {
    pub fn new(rec: Rec) -> FakeHotkey {
        FakeHotkey { arm_ok: true, pressed: VecDeque::new(), rec }
    }
}

impl Hotkey for FakeHotkey {
    fn arm(&mut self) -> Result<(), String> {
        self.rec.borrow_mut().push("hotkey.arm".into());
        if self.arm_ok { Ok(()) } else { Err("taken".into()) }
    }
    fn waitable(&self) -> isize {
        0
    }
    fn take_pressed(&mut self) -> bool {
        self.pressed.pop_front().unwrap_or(false)
    }
}

/// No escape chord at all (`--test-shell` without `--test-hotkey`).
pub struct NoHotkey;

impl Hotkey for NoHotkey {
    fn arm(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn waitable(&self) -> isize {
        0
    }
    fn take_pressed(&mut self) -> bool {
        false
    }
}

pub struct FakeSession {
    pub safe_mode: bool,
    pub shift: bool,
    pub build: Option<String>,
    pub delegate_ok: bool,
    pub ending: VecDeque<bool>,
    pub ready_event_ok: bool,
    rec: Rec,
}

impl FakeSession {
    pub fn new(rec: Rec) -> FakeSession {
        FakeSession { safe_mode: false, shift: false, build: Some("26300.9974".into()), delegate_ok: false, ending: VecDeque::new(), ready_event_ok: true, rec }
    }
}

impl Session for FakeSession {
    fn safe_mode(&mut self) -> bool {
        self.safe_mode
    }
    fn shift_held(&mut self) -> bool {
        self.shift
    }
    fn build(&mut self) -> Option<String> {
        self.build.clone()
    }
    fn create_shell_ready_event(&mut self) -> Result<(), String> {
        self.rec.borrow_mut().push("session.create_shell_ready_event".into());
        if self.ready_event_ok { Ok(()) } else { Err("denied".into()) }
    }
    fn signal_shell_ready(&mut self) -> Result<(), String> {
        self.rec.borrow_mut().push("session.signal_shell_ready".into());
        Ok(())
    }
    fn release_identity(&mut self) {
        self.rec.borrow_mut().push("session.release_identity".into());
    }
    fn signal_running_guard_panic(&mut self) -> bool {
        self.rec.borrow_mut().push("session.signal_running_guard_panic".into());
        self.delegate_ok
    }
    fn session_ending(&mut self) -> bool {
        self.ending.pop_front().unwrap_or(false)
    }
}

/// Work areas that are not touched; records the call (and what it was asked to put back).
pub struct NullWorkAreas {
    pub fail: bool,
    pub restored: Vec<Vec<WorkAreaRec>>,
    rec: Rec,
}

impl NullWorkAreas {
    pub fn new(rec: Rec) -> NullWorkAreas {
        NullWorkAreas { fail: false, restored: Vec::new(), rec }
    }
}

impl WorkAreas for NullWorkAreas {
    fn restore(&mut self, saved: &[WorkAreaRec]) -> Result<(), String> {
        self.rec.borrow_mut().push("workarea.restore".into());
        self.restored.push(saved.to_vec());
        if self.fail { Err("busy".into()) } else { Ok(()) }
    }
}

/// A taskbar that is not touched; answers from a script (empty script: one window shown). With a [`Desktop`] it also behaves like
/// one: the sweep and the record-driven show change the pretend windows, with the same ownership rules as the real thing.
pub struct NullTaskbar {
    pub results: VecDeque<Result<usize, String>>,
    pub desktop: Option<Desktop>,
    rec: Rec,
}

impl NullTaskbar {
    pub fn new(rec: Rec) -> NullTaskbar {
        NullTaskbar { results: VecDeque::new(), desktop: None, rec }
    }
}

impl TaskbarVisibility for NullTaskbar {
    fn show_windows_taskbar(&mut self) -> Result<usize, String> {
        self.rec.borrow_mut().push("taskbar.show".into());
        if let (Some(d), None) = (&self.desktop, self.results.front()) {
            let live = d.borrow().clone();
            let targets = tbmodel::plan_sweep(&live);
            for t in &targets {
                set_visible(d, t, true)?;
            }
            return Ok(live.iter().filter(|w| tbmodel::is_taskbar_class(&w.class)).count());
        }
        self.results.pop_front().unwrap_or(Ok(1))
    }
    fn show_recorded(&mut self, rec: &HideRecord) -> Result<usize, String> {
        self.rec.borrow_mut().push("taskbar.show_recorded".into());
        if let Some(r) = self.results.pop_front() {
            return r;
        }
        let Some(d) = &self.desktop else { return Ok(rec.windows.len()) };
        let live = d.borrow().clone();
        let plan = tbmodel::plan_restore(rec, &live);
        for t in &plan.show {
            set_visible(d, t, true)?;
        }
        Ok(plan.restored_count())
    }
}

/// The host's two events, as a fake: records what the guard did with them.
pub struct FakeLink {
    pub arm_ok: bool,
    pub confirm_ok: bool,
    pub done_handle: isize,
    rec: Rec,
}

impl FakeLink {
    pub fn new(rec: Rec) -> FakeLink {
        FakeLink { arm_ok: true, confirm_ok: true, done_handle: 77, rec }
    }
}

impl HostLink for FakeLink {
    fn arm(&mut self) -> Result<(), String> {
        self.rec.borrow_mut().push("link.arm".into());
        if self.arm_ok { Ok(()) } else { Err("no such event".into()) }
    }
    fn done_waitable(&self) -> isize {
        self.done_handle
    }
    fn confirm_ready(&mut self) -> Result<(), String> {
        self.rec.borrow_mut().push("link.confirm_ready".into());
        if self.confirm_ok { Ok(()) } else { Err("denied".into()) }
    }
}

pub struct MemStore {
    pub file: ShellStateFile,
    pub saves: usize,
}

impl MemStore {
    pub fn new(file: ShellStateFile) -> MemStore {
        MemStore { file, saves: 0 }
    }
}

impl StateStore for MemStore {
    fn load(&mut self) -> ShellStateFile {
        self.file.clone()
    }
    fn save(&mut self, file: &ShellStateFile) -> Result<(), String> {
        if !file.writable {
            return Err("newer version".into());
        }
        self.saves += 1;
        self.file = file.clone();
        Ok(())
    }
}

#[derive(Default)]
pub struct FakeUi {
    pub messages: Vec<Message>,
}

impl Ui for FakeUi {
    fn message(&mut self, m: Message) {
        self.messages.push(m);
    }
}

/// All the fakes together; `sys()` lends them to the runner.
pub struct World {
    pub rec: Rec,
    pub time: Rc<Cell<Millis>>,
    pub clock: FakeClock,
    pub registry: MemRegistry,
    pub procs: ScriptProcs,
    pub hotkey: FakeHotkey,
    pub session: FakeSession,
    pub workarea: NullWorkAreas,
    pub taskbar: NullTaskbar,
    pub store: MemStore,
    pub ui: FakeUi,
}

impl World {
    /// A world where the Shell value is set to the guard and the clock stands at `start`.
    pub fn new(start: Millis) -> World {
        let rec = new_rec();
        let time = Rc::new(Cell::new(start));
        World {
            clock: FakeClock(time.clone()),
            registry: MemRegistry::new(Some("C:\\guard\\guard.exe"), rec.clone()),
            procs: ScriptProcs::new(rec.clone(), time.clone()),
            hotkey: FakeHotkey::new(rec.clone()),
            session: FakeSession::new(rec.clone()),
            workarea: NullWorkAreas::new(rec.clone()),
            taskbar: NullTaskbar::new(rec.clone()),
            store: MemStore::new(ShellStateFile::empty()),
            ui: FakeUi::default(),
            rec,
            time,
        }
    }

    pub fn sys(&mut self) -> Sys<'_> {
        Sys {
            clock: &self.clock,
            registry: &mut self.registry,
            procs: &mut self.procs,
            hotkey: &mut self.hotkey,
            session: &mut self.session,
            workarea: &mut self.workarea,
            taskbar: &mut self.taskbar,
            store: &mut self.store,
            ui: &mut self.ui,
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.rec.borrow().clone()
    }

    /// Position of the first recorded call that starts with `prefix`.
    pub fn pos(&self, prefix: &str) -> Option<usize> {
        self.rec.borrow().iter().position(|c| c.starts_with(prefix))
    }
}
