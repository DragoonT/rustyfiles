//! Stand-ins for the traits of the tray host: they record what they were asked and answer from a script. Used by every test of the
//! host and by the windowless probe. They create no window, send no message and read no handle.
//!
//! All of them share one [`CallLog`] when the test cares about the ORDER of calls across traits (the pixels are read before the
//! message is passed on; the window is destroyed before the broadcast).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use parking_lot::Mutex;

use super::model::IconBitmap;
use super::traits::{Broadcast, Clock, IconPixels, OwnerSend, Raw, Relay, Relayed, TrayWindow};
use crate::tray_wire::forward::{Method, Outcome};

pub type CallLog = Arc<Mutex<Vec<String>>>;

pub fn new_log() -> CallLog {
    Arc::new(Mutex::new(Vec::new()))
}

// ---------------------------------------------------------------- the owner of an icon

#[derive(Default)]
struct OwnersInner {
    /// window -> its process
    windows: HashMap<isize, u32>,
    exes: HashMap<u32, String>,
    log: Vec<String>,
    sent: Vec<(isize, u32, usize, isize)>,
    fail_after: Option<usize>,
    sends: usize,
}

#[derive(Clone, Default)]
pub struct FakeOwners {
    inner: Arc<Mutex<OwnersInner>>,
}

impl FakeOwners {
    pub fn new() -> FakeOwners {
        FakeOwners::default()
    }

    pub fn add_window(&self, hwnd: isize, pid: u32) {
        self.inner.lock().windows.insert(hwnd, pid);
    }

    pub fn remove_window(&self, hwnd: isize) {
        self.inner.lock().windows.remove(&hwnd);
    }

    pub fn set_exe(&self, pid: u32, exe: &str) {
        self.inner.lock().exes.insert(pid, exe.to_string());
    }

    /// Sends after the first `n` fail (an elevated owner, a hung one).
    pub fn fail_sends_after(&self, n: usize) {
        self.inner.lock().fail_after = Some(n);
    }

    /// Every call that changes anything or asks for the foreground, in order (`allow_foreground 50`, `send 0x.. 0x.. 0x.. 0x..`).
    pub fn log(&self) -> Vec<String> {
        self.inner.lock().log.clone()
    }

    /// `(window, message, wParam, lParam)` of every send that went out.
    pub fn sent(&self) -> Vec<(isize, u32, usize, isize)> {
        self.inner.lock().sent.clone()
    }
}

impl OwnerSend for FakeOwners {
    fn is_window(&self, hwnd: isize) -> bool {
        self.inner.lock().windows.contains_key(&hwnd)
    }

    fn owner_pid(&self, hwnd: isize) -> Option<u32> {
        self.inner.lock().windows.get(&hwnd).copied()
    }

    fn owner_exe(&self, pid: u32) -> Option<String> {
        self.inner.lock().exes.get(&pid).cloned()
    }

    fn allow_foreground(&self, pid: u32) {
        self.inner.lock().log.push(format!("allow_foreground {pid}"));
    }

    fn send_notify(&self, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> bool {
        let mut g = self.inner.lock();
        if g.fail_after.is_some_and(|n| g.sends >= n) {
            return false;
        }
        g.sends += 1;
        g.log.push(format!("send {hwnd:#x} {msg:#x} {wparam:#x} {lparam:#x}"));
        g.sent.push((hwnd, msg, wparam, lparam));
        true
    }
}

// ---------------------------------------------------------------- pixels

#[derive(Clone)]
pub struct FakePixels {
    pictures: Arc<Mutex<HashMap<isize, IconBitmap>>>,
    reads: Arc<Mutex<Vec<isize>>>,
    log: Option<CallLog>,
}

impl FakePixels {
    pub fn new() -> FakePixels {
        FakePixels { pictures: Default::default(), reads: Default::default(), log: None }
    }

    pub fn with_log(log: CallLog) -> FakePixels {
        FakePixels { log: Some(log), ..FakePixels::new() }
    }

    pub fn set(&self, hicon: isize, w: u32, h: u32, fill: u8) {
        self.pictures.lock().insert(hicon, IconBitmap::new(w, h, vec![fill; (w * h * 4) as usize]).expect("a fake picture within bounds"));
    }

    /// The handles that were read, in order.
    pub fn reads(&self) -> Vec<isize> {
        self.reads.lock().clone()
    }
}

impl Default for FakePixels {
    fn default() -> Self {
        FakePixels::new()
    }
}

impl IconPixels for FakePixels {
    fn read(&self, hicon: isize) -> Option<IconBitmap> {
        self.reads.lock().push(hicon);
        if let Some(l) = &self.log {
            l.lock().push(format!("pixels {hicon:#x}"));
        }
        self.pictures.lock().get(&hicon).cloned()
    }
}

// ---------------------------------------------------------------- the relay

/// A relay that answers from a script. `hook` runs inside `relay`, with no lock held, which is how a test makes another message
/// arrive while this one waits for Explorer.
pub struct FakeRelay {
    pub log: Option<CallLog>,
    target: Mutex<Option<isize>>,
    outcome: Mutex<Outcome>,
    relayed: Mutex<Vec<(Raw, Method)>>,
    #[allow(clippy::type_complexity)]
    hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    invalidated: AtomicUsize,
}

impl FakeRelay {
    pub fn new(target: Option<isize>, outcome: Outcome) -> FakeRelay {
        FakeRelay { log: None, target: Mutex::new(target), outcome: Mutex::new(outcome), relayed: Mutex::new(Vec::new()), hook: Mutex::new(None), invalidated: AtomicUsize::new(0) }
    }

    pub fn with_log(mut self, log: CallLog) -> FakeRelay {
        self.log = Some(log);
        self
    }

    pub fn set_target(&self, t: Option<isize>) {
        *self.target.lock() = t;
    }

    pub fn set_outcome(&self, o: Outcome) {
        *self.outcome.lock() = o;
    }

    pub fn set_hook(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.hook.lock() = Some(f);
    }

    pub fn relayed(&self) -> Vec<(Raw, Method)> {
        self.relayed.lock().clone()
    }

    pub fn invalidations(&self) -> usize {
        self.invalidated.load(Ordering::SeqCst)
    }
}

impl Relay for FakeRelay {
    fn relay(&self, raw: Raw, method: Method) -> Relayed {
        if let Some(l) = &self.log {
            l.lock().push("relay".to_string());
        }
        self.relayed.lock().push((raw, method));
        let hook = self.hook.lock().clone();
        if let Some(h) = hook {
            h();
        }
        if self.target.lock().is_none() {
            return Relayed::NoTarget;
        }
        Relayed::Done(*self.outcome.lock())
    }

    fn target(&self) -> Option<isize> {
        *self.target.lock()
    }

    fn invalidate(&self) {
        self.invalidated.fetch_add(1, Ordering::SeqCst);
    }

    fn set_self(&self, _hwnd: isize) {}
}

// Arc'd relays are shared with the test that built them.
impl<T: Relay + ?Sized> Relay for Arc<T> {
    fn relay(&self, raw: Raw, method: Method) -> Relayed {
        (**self).relay(raw, method)
    }
    fn target(&self) -> Option<isize> {
        (**self).target()
    }
    fn invalidate(&self) {
        (**self).invalidate()
    }
    fn set_self(&self, hwnd: isize) {
        (**self).set_self(hwnd)
    }
}

// ---------------------------------------------------------------- window and broadcast

#[derive(Clone)]
pub struct FakeWindow {
    pub log: CallLog,
    state: Arc<Mutex<WindowState>>,
}

struct WindowState {
    exists: bool,
    first: bool,
    fail_create: bool,
}

impl FakeWindow {
    pub fn new(log: CallLog) -> FakeWindow {
        FakeWindow { log, state: Arc::new(Mutex::new(WindowState { exists: false, first: true, fail_create: false })) }
    }

    /// Whether a program looking for `Shell_TrayWnd` would find ours first.
    pub fn set_first(&self, first: bool) {
        self.state.lock().first = first;
    }

    pub fn fail_create(&self) {
        self.state.lock().fail_create = true;
    }

    pub fn exists(&self) -> bool {
        self.state.lock().exists
    }
}

impl TrayWindow for FakeWindow {
    fn create(&mut self) -> Result<(), String> {
        let mut s = self.state.lock();
        if s.fail_create {
            return Err("fake: cannot create".into());
        }
        s.exists = true;
        self.log.lock().push("window created".into());
        Ok(())
    }

    fn destroy(&mut self) {
        let mut s = self.state.lock();
        if s.exists {
            s.exists = false;
            self.log.lock().push("window destroyed".into());
        }
    }

    fn is_first(&mut self) -> bool {
        let s = self.state.lock();
        s.exists && s.first
    }

    fn raise(&mut self) {
        self.state.lock().first = true;
        self.log.lock().push("window raised".into());
    }

    fn mirror(&mut self, explorer_tray: Option<isize>) {
        self.log.lock().push(format!("window mirrored {explorer_tray:?}"));
    }

    fn hwnd(&self) -> Option<isize> {
        self.state.lock().exists.then_some(0x1000)
    }
}

pub struct FakeBroadcast {
    pub log: CallLog,
}

impl Broadcast for FakeBroadcast {
    fn taskbar_created(&mut self) {
        self.log.lock().push("broadcast TaskbarCreated".into());
    }
}

// ---------------------------------------------------------------- the clock

#[derive(Default)]
pub struct ManualClock(pub AtomicU64);

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
