//! Hiding Explorer's taskbar while the dock runs (shell plan E1): the dock host's side. The rules are in `rf_shellguard::tbmodel` and
//! `rf_shellguard::tbctl` (pure, tested with a pretend desktop); the code that touches Explorer's windows is `rf_shellguard::win::
//! tbwin` (the only file that does). This file is the glue between them and the host process:
//!
//! * [`TaskbarHider`] runs the controller on a THREAD OF ITS OWN with one hidden top-level window (never shown), because a call into
//!   another process's window can wait for that process, the guard has to be started and waited for, and none of that may happen on
//!   the interface thread. The window exists to hear `TaskbarCreated` (Explorer made its taskbar again), `WM_DISPLAYCHANGE`,
//!   `WM_SETTINGCHANGE` (work area) and `WM_ENDSESSION`, which are broadcast to top-level windows only.
//! * The bottom edge (docs/design/taskbar-hide.md): once the hide is confirmed the controller tells the dock to let go of its own
//!   desktop-toolbar band and to place itself from the monitor's bottom edge (`dock::system::take_edge`), and sets the work area of the
//!   monitors whose taskbar it hid. The wait for the dock is NOT a blocking wait: the thread answers Explorer's broadcasts while it
//!   lasts, so a short timer (100 ms) looks again until the dock has let go.
//! * One timer, once a second, only while the setting is on or a restore is still owed (a restore that failed is retried by it,
//!   whatever the setting says, and the thread does not end before it is done): look at the guard and at the windows (Explorer shows its taskbar again
//!   now and then, and nothing was found that says so; an event hook on `EVENT_OBJECT_SHOW` is the thing to try next, after a live run).
//! * [`install_panic_restore`]: a panic anywhere in the host shows the taskbar again before the program unwinds.
//!
//! NOT RUN by the author: the real world below creates windows, starts the guard and hides Explorer's taskbar. It is exercised only
//! through `rf_shellguard::tbfakes` (the same controller against a pretend desktop). `tests` here cover the pure parts only and
//! never start the thread.

use std::ffi::c_void;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rf_shellguard::statefile::{self, ShellStateFile};
use rf_shellguard::tbctl::{Controller, HideWorld, Status};
use rf_shellguard::tbmodel::{LiveWindow, MonitorGeom, Refusal, Target};
use crate::dock::layout::Rect;
use rf_shellguard::win::tbwin;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, RECT, SetLastError, WPARAM};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::core::{BOOL, PCWSTR};

/// The guard program's file name, next to the host's.
pub const GUARD_FILE: &str = "rustyfiles-shellguard.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Lets the guard leave the host's job object, if the job allows it: a job that is closed with kill-on-close (a launcher, a
/// terminal, a tool that ends "everything it started") would otherwise end the guard together with the host, and then nobody is
/// left to show the taskbar.
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// The creation flags to try for the guard, in order: out of the job when allowed, and in it when the job forbids leaving (the
/// first start then fails with "access denied"; hiding with a guard that is part of the host's job is still better than not
/// having one, and the next start of the host undoes a record the guard could not).
fn guard_creation_flag_sets() -> [u32; 2] {
    [CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW]
}

/// Counts the guards of THIS PROCESS (not of one `RealWorld`: the world is made again every time the feature is switched on, and
/// a counter that began again at 1 would name the new guard's events like the old one's, which a guard still ending holds open).
static GUARD_SEQ: AtomicU32 = AtomicU32::new(0);

fn next_guard_seq() -> u32 {
    GUARD_SEQ.fetch_add(1, Ordering::SeqCst) + 1
}

/// The names of the two events of one guard: in the session's own namespace, with the host's process id and the guard's number.
fn event_names(host_pid: u32, seq: u32) -> (String, String) {
    (format!("Local\\rustyfiles-beside-ready-{host_pid}-{seq}"), format!("Local\\rustyfiles-beside-done-{host_pid}-{seq}"))
}

/// Creates a manual-reset, unsignalled event with this name, and REFUSES one that exists already: `CreateEventW` hands out the
/// existing object for a name in use, and an event that somebody else holds (a guard still ending) may be signalled already.
fn create_new_event(name: &str) -> Result<isize, String> {
    let w = wide(name);
    // SAFETY: NUL-terminated name. The last error is cleared first because a success does not reset it.
    unsafe {
        SetLastError(ERROR_SUCCESS);
        let h = CreateEventW(None, true, false, PCWSTR(w.as_ptr())).map_err(|e| format!("event {name}: {e}"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            return Err(format!("event {name} exists already"));
        }
        Ok(h.0 as isize)
    }
}

/// Everything the host decides about hiding.
#[derive(Clone, Debug)]
pub struct Config {
    /// The folder with `shell-state.json` (the guard is given the same one).
    pub state_dir: PathBuf,
    /// `rustyfiles-shellguard.exe`.
    pub guard_exe: PathBuf,
    /// The thickness of the dock's band in device px (`None`: the dock reserves none, `dock_reserve` is off), as far as it is known at
    /// the start. It changes through [`TaskbarHider::set_band`]. Session only, never saved.
    pub band_px: Option<i32>,
    /// The dock has already said what its band is (`band_px` is its word, `None` included). When it has not, the bottom edge is not
    /// taken until it does (`TaskbarHider::set_band`): no band yet is not the same as no band wanted.
    pub band_known: bool,
}

/// Where the guard is expected: next to the running program.
pub fn guard_next_to_exe() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(GUARD_FILE))).unwrap_or_else(|| PathBuf::from(GUARD_FILE))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------- the real world

struct RealWorld {
    cfg: Config,
    host_pid: u32,
    child: Option<Child>,
    ready: Option<isize>,
    done: Option<isize>,
}

impl RealWorld {
    fn new(cfg: Config) -> RealWorld {
        RealWorld { cfg, host_pid: std::process::id(), child: None, ready: None, done: None }
    }

    fn state_path(&self) -> PathBuf {
        statefile::path_in(&self.cfg.state_dir)
    }

    fn close_events(&mut self) {
        for h in [self.ready.take(), self.done.take()].into_iter().flatten() {
            // SAFETY: created in `start_guard`, closed once.
            unsafe {
                let _ = CloseHandle(HANDLE(h as *mut c_void));
            }
        }
    }
}

unsafe extern "system" fn mon_cb(hmon: HMONITOR, _dc: HDC, _r: *mut RECT, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the address of the Vec on the caller's stack, alive during EnumDisplayMonitors.
    unsafe {
        let list = &mut *(lparam.0 as *mut Vec<MonitorGeom>);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut mi.monitorInfo as *mut _ as *mut _).as_bool() {
            let name = String::from_utf16_lossy(&mi.szDevice).trim_end_matches('\0').to_string();
            let (r, w) = (mi.monitorInfo.rcMonitor, mi.monitorInfo.rcWork);
            // MONITORINFOF_PRIMARY = 1
            list.push(MonitorGeom { id: name, rect: [r.left, r.top, r.right, r.bottom], work: [w.left, w.top, w.right, w.bottom], primary: mi.monitorInfo.dwFlags & 1 != 0 });
        }
    }
    BOOL(1)
}

impl HideWorld for RealWorld {
    fn now_ms(&mut self) -> u64 {
        now_ms()
    }

    fn log(&mut self, msg: &str) {
        log::info!("{msg}");
    }

    fn warn(&mut self, msg: &str) {
        log::warn!("{msg}");
    }

    fn explorer_owns_shell(&mut self) -> bool {
        crate::shellhost::mode::detect() == crate::shellhost::ShellMode::Beside
    }

    fn list_taskbars(&mut self) -> Result<Vec<LiveWindow>, String> {
        tbwin::list_taskbar_windows()
    }

    fn hide(&mut self, t: &Target) -> Result<(), String> {
        tbwin::set_visible(t, false)
    }

    fn show(&mut self, t: &Target) -> Result<(), String> {
        tbwin::set_visible(t, true)
    }

    fn wait_visible(&mut self, targets: &[Target]) -> usize {
        tbwin::wait_visible(targets)
    }

    fn wait_hidden(&mut self, targets: &[Target]) -> Vec<Target> {
        tbwin::wait_hidden(targets)
    }

    fn dock_take_edge(&mut self, edge: [i32; 4]) {
        crate::dock::system::take_edge(Rect::new(edge[0], edge[1], edge[2], edge[3]));
    }

    fn dock_edge_ready(&mut self) -> bool {
        !crate::dock::system::appbar_held()
    }

    fn dock_give_back_edge(&mut self) {
        crate::dock::system::give_back_edge();
    }

    fn monitors(&mut self) -> Vec<MonitorGeom> {
        let mut v: Vec<MonitorGeom> = Vec::new();
        // SAFETY: `v` outlives the call. The thread is per-monitor DPI aware (see `run_thread`), so rectangles are physical pixels.
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(mon_cb), LPARAM(&mut v as *mut Vec<MonitorGeom> as isize));
        }
        v
    }

    fn set_work_area(&mut self, rect: [i32; 4]) -> Result<(), String> {
        rf_shellguard::win::workarea::set_work_area(rect)
    }

    fn hold_work_area(&mut self, from: [i32; 4], to: [i32; 4]) -> Result<(), String> {
        rf_shellguard::win::workarea::hold_work_area(to)?;
        // (the thread is per-monitor DPI aware, see `run_thread`: the rectangles are physical pixels)
        crate::workfit::maximized_windows(from, to);
        Ok(())
    }

    fn load_state(&mut self) -> ShellStateFile {
        ShellStateFile::load(&self.state_path())
    }

    fn save_state(&mut self, f: &ShellStateFile) -> Result<(), String> {
        f.save(&self.state_path()).map_err(|e| e.to_string())
    }

    fn guard_exists(&mut self) -> bool {
        self.cfg.guard_exe.is_file()
    }

    fn start_guard(&mut self) -> Result<(), String> {
        self.kill_guard();
        let (ready_name, done_name) = event_names(self.host_pid, next_guard_seq());
        let mut made: Vec<isize> = Vec::new();
        for name in [&ready_name, &done_name] {
            match create_new_event(name) {
                Ok(h) => made.push(h),
                Err(e) => {
                    self.ready = made.first().copied();
                    self.done = None;
                    self.close_events();
                    return Err(e);
                }
            }
        }
        self.ready = Some(made[0]);
        self.done = Some(made[1]);
        let mut cmd = Command::new(&self.cfg.guard_exe);
        cmd.arg("--beside-guard")
            .arg(self.host_pid.to_string())
            .arg("--ready-event")
            .arg(&ready_name)
            .arg("--done-event")
            .arg(&done_name)
            .arg("--state-dir")
            .arg(&self.cfg.state_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut last_err = String::new();
        for flags in guard_creation_flag_sets() {
            std::os::windows::process::CommandExt::creation_flags(&mut cmd, flags);
            match cmd.spawn() {
                Ok(c) => {
                    self.child = Some(c);
                    return Ok(());
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        self.close_events();
        Err(last_err)
    }

    fn guard_ready(&mut self, wait_ms: u64) -> bool {
        let Some(ready) = self.ready else { return false };
        let started = std::time::Instant::now();
        loop {
            // SAFETY: the handle is ours and open.
            let r = unsafe { WaitForSingleObject(HANDLE(ready as *mut c_void), 50) };
            if r.0 == 0 {
                return true;
            }
            // A guard that ended on its way (the events could not be opened, the host was not found) will never confirm.
            if self.child.as_mut().is_none_or(|c| !matches!(c.try_wait(), Ok(None))) {
                return false;
            }
            if started.elapsed() >= Duration::from_millis(wait_ms) {
                return false;
            }
        }
    }

    fn guard_alive(&mut self) -> bool {
        self.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }

    fn tell_guard_done(&mut self) {
        if let Some(done) = self.done {
            // SAFETY: the handle is ours and open; the guard has its own handle to the same event.
            unsafe {
                let _ = SetEvent(HANDLE(done as *mut c_void));
            }
        }
        self.close_events();
        // Not killed: it exits by itself. The Child is dropped; the process is not ended by that.
        self.child = None;
    }

    fn kill_guard(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.close_events();
    }
}

// ---------------------------------------------------------------- the panic net

static HIDING: AtomicBool = AtomicBool::new(false);

/// A panic anywhere in the host (any thread) shows Explorer's taskbar again before the program unwinds, if hiding is on. The
/// show is queued and returns at once. Safe to call more than once.
pub fn install_panic_restore() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if HIDING.load(Ordering::SeqCst) {
                let _ = tbwin::sweep_show_all();
            }
            previous(info);
        }));
    });
}

// ---------------------------------------------------------------- the thread and its window

enum Cmd {
    Wanted(bool),
    /// The dock's band changed (`None`: it reserves none).
    Band(Option<i32>),
    /// The docks on the other monitors changed their bands (monitor device name, thickness in device px).
    OtherBands(Vec<(String, i32)>),
    /// Leave (restore what is hidden, tell the guard) and end the thread.
    Stop,
    /// End the thread without leaving anything: the one-shot look at a record left by a crashed host. A restore that is still
    /// owed is finished first.
    Finish,
}

struct Ctx {
    ctl: Controller,
    world: RealWorld,
    rx: Receiver<Cmd>,
    status: Arc<Mutex<Status>>,
    wanted: bool,
    /// A panic inside the controller: nothing more is attempted but stopping.
    dead: bool,
    /// How many of the follow-up looks after a `TaskbarCreated` are still to come.
    follow_ups: u8,
    /// Asked to end, but a restore is still owed: the thread lives until it is done (the timer retries it).
    stopping: bool,
}

const WM_CMD: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;
const TIMER_TICK: usize = 1;
const TIMER_DISPLAYS: usize = 2;
const TIMER_WORK: usize = 3;
const TIMER_FOLLOW: usize = 4;
/// While the dock lets go of its band: a short look, so that the work area is set within a fraction of a second.
const TIMER_EDGE: usize = 5;
const EDGE_LOOK_MS: u32 = 100;
const TICK_MS: u32 = 1_000;
const DISPLAY_DEBOUNCE_MS: u32 = 400;
const WORK_DEBOUNCE_MS: u32 = 300;
/// Explorer creates the secondary taskbars a moment after `TaskbarCreated`: look again after these.
const FOLLOW_MS: [u32; 2] = [700, 2_500];

fn taskbar_created_message() -> u32 {
    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    // SAFETY: a plain registration of a message name.
    *ID.get_or_init(|| unsafe { windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(windows::core::w!("TaskbarCreated")) })
}

impl Ctx {
    fn publish(&self) {
        if let Ok(mut s) = self.status.lock() {
            *s = self.ctl.status();
        }
        // The panic net looks at this: on while we want to hide, while windows are hidden, and while a restore is owed.
        HIDING.store(self.wanted || (!self.dead && self.ctl.holds_windows()), Ordering::SeqCst);
    }

    /// The once-a-second timer is needed while hiding, and while a restore is owed (also after the setting went off).
    fn timer_needed(&self) -> bool {
        !self.dead && (self.wanted || self.ctl.restore_pending())
    }

    /// Runs one controller call; a panic inside it shows the taskbar and stops the feature instead of killing the host.
    fn guarded(&mut self, f: impl FnOnce(&mut Controller, &mut RealWorld)) {
        if self.dead {
            return;
        }
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut self.ctl, &mut self.world)));
        if r.is_err() {
            log::error!("taskbar hiding: an internal error; the Windows taskbar is shown again and hiding stops");
            let _ = tbwin::sweep_show_all();
            self.dead = true;
            self.wanted = false;
        }
        self.publish();
    }
}

unsafe fn sync_timer(hwnd: HWND, ctx: &Ctx) {
    use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
    // SAFETY: plain timer calls on our own window.
    unsafe {
        if ctx.timer_needed() {
            SetTimer(Some(hwnd), TIMER_TICK, TICK_MS, None);
        } else {
            let _ = KillTimer(Some(hwnd), TIMER_TICK);
        }
        if !ctx.dead && ctx.ctl.edge_waiting() {
            SetTimer(Some(hwnd), TIMER_EDGE, EDGE_LOOK_MS, None);
        } else {
            let _ = KillTimer(Some(hwnd), TIMER_EDGE);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CREATESTRUCTW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, KillTimer, PostQuitMessage, SetTimer, SetWindowLongPtrW, WM_DESTROY, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_NCCREATE,
        WM_SETTINGCHANGE, WM_TIMER,
    };
    const SPI_SETWORKAREA: usize = 0x002F;
    // SAFETY: the Ctx pointer is set in WM_NCCREATE from the creation parameter and freed in WM_DESTROY, on this thread only.
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Ctx;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_DESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
            PostQuitMessage(0);
            return LRESULT(0);
        }
        // The work-area broadcast is also delivered while THIS thread is inside its own call that sets the work area, that is while the
        // controller holds the context: it is answered from a plain read of one field, never with a second `&mut Ctx`.
        if msg == WM_SETTINGCHANGE {
            if (*ptr).wanted && wp.0 == SPI_SETWORKAREA {
                SetTimer(Some(hwnd), TIMER_WORK, WORK_DEBOUNCE_MS, None);
            }
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let ctx = &mut *ptr;
        if msg == taskbar_created_message() {
            if ctx.wanted {
                ctx.guarded(|c, w| c.explorer_restarted(w));
                sync_timer(hwnd, ctx);
                ctx.follow_ups = FOLLOW_MS.len() as u8;
                SetTimer(Some(hwnd), TIMER_FOLLOW, FOLLOW_MS[0], None);
            }
            return LRESULT(0);
        }
        match msg {
            WM_CMD => {
                while let Ok(cmd) = ctx.rx.try_recv() {
                    match cmd {
                        Cmd::Wanted(on) => {
                            if on {
                                HIDING.store(true, Ordering::SeqCst);
                            }
                            ctx.wanted = on && !ctx.dead;
                            ctx.guarded(|c, w| c.set_wanted(w, on));
                            sync_timer(hwnd, ctx);
                        }
                        Cmd::Band(band) => {
                            ctx.guarded(|c, w| c.set_band(w, band));
                            sync_timer(hwnd, ctx);
                        }
                        Cmd::OtherBands(bands) => {
                            ctx.guarded(|c, w| c.set_other_bands(w, bands));
                            sync_timer(hwnd, ctx);
                        }
                        Cmd::Stop | Cmd::Finish => {
                            ctx.wanted = false;
                            if matches!(cmd, Cmd::Stop) {
                                ctx.guarded(|c, w| c.quit(w));
                            }
                            // A restore that is still owed keeps the thread (and its timer) alive until it is done.
                            if ctx.ctl.restore_pending() && !ctx.dead {
                                ctx.stopping = true;
                                sync_timer(hwnd, ctx);
                            } else {
                                let _ = DestroyWindow(hwnd);
                                return LRESULT(0);
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_TICK => {
                        ctx.guarded(|c, w| c.tick(w));
                        if ctx.stopping && !(ctx.ctl.restore_pending() && !ctx.dead) {
                            // the owed restore is done (or given up): the thread can end now
                            let _ = DestroyWindow(hwnd);
                            return LRESULT(0);
                        }
                        // kills the timer when nothing is owed and nothing is wanted any more
                        sync_timer(hwnd, ctx);
                    }
                    TIMER_EDGE => {
                        ctx.guarded(|c, w| c.edge_tick(w));
                        sync_timer(hwnd, ctx);
                    }
                    TIMER_DISPLAYS => {
                        let _ = KillTimer(Some(hwnd), TIMER_DISPLAYS);
                        ctx.guarded(|c, w| c.displays_changed(w));
                        sync_timer(hwnd, ctx);
                    }
                    TIMER_WORK => {
                        let _ = KillTimer(Some(hwnd), TIMER_WORK);
                        ctx.guarded(|c, w| c.work_area_changed(w));
                        sync_timer(hwnd, ctx);
                    }
                    TIMER_FOLLOW => {
                        let _ = KillTimer(Some(hwnd), TIMER_FOLLOW);
                        ctx.guarded(|c, w| c.displays_changed(w));
                        sync_timer(hwnd, ctx);
                        ctx.follow_ups = ctx.follow_ups.saturating_sub(1);
                        if ctx.follow_ups > 0 {
                            SetTimer(Some(hwnd), TIMER_FOLLOW, FOLLOW_MS[1], None);
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                if ctx.wanted {
                    SetTimer(Some(hwnd), TIMER_DISPLAYS, DISPLAY_DEBOUNCE_MS, None);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_ENDSESSION => {
                if wp.0 != 0 {
                    // The session is ending: leave Explorer's taskbar as it was found.
                    ctx.wanted = false;
                    ctx.guarded(|c, w| c.quit(w));
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn run_thread(ctx: Box<Ctx>, tx: Sender<Result<isize, String>>) {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DispatchMessageW, GetMessageW, MSG, RegisterClassExW, TranslateMessage, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP};
    use windows::core::w;
    // SAFETY: standard window-class registration, creation and message loop on this thread.
    unsafe {
        // Rectangles in physical pixels, whatever the process was started as.
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(Err(format!("module handle: {e}")));
                return;
            }
        };
        let class = w!("RustyfilesTaskbarHideWatch");
        let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassExW(&wc);
        let raw = Box::into_raw(ctx);
        // a zero-size pop-up that is never shown: top-level, so that it gets the broadcasts
        let hwnd = match CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!(""), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), Some(raw as *const _)) {
            Ok(h) => h,
            Err(e) => {
                drop(Box::from_raw(raw));
                let _ = tx.send(Err(format!("the taskbar watch window: {e}")));
                return;
            }
        };
        let _ = tx.send(Ok(hwnd.0 as isize));
        // A record left by a host that crashed or lost power is undone first, whatever the setting says now. On this thread, like
        // everything else that touches Explorer's windows.
        (*raw).guarded(|c, w| c.startup(w));
        // (a restore that could not be finished at once is retried by the timer)
        sync_timer(hwnd, &*raw);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// A fault that keeps a WANTED hide from being in force: something the user can be told about and may fix. The states that are only
/// the situation ("Explorer does not own the shell window", "Explorer has no visible taskbar right now") are not faults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// `rustyfiles-shellguard.exe` was not found next to the program.
    GuardMissing,
    /// The guard was started and did not confirm in time.
    GuardNotConfirmed,
    /// The guard confirmed and then ended.
    GuardLost,
    /// The state file (the record that lets the guard undo the hide) could not be written, so nothing may be hidden.
    RecordNotSaved,
}

/// The fault in a status, or `None` (hidden, off, or waiting for a reason that is not a fault).
pub fn fault_of(status: Status) -> Option<Fault> {
    match status {
        Status::Waiting(Refusal::GuardMissing) => Some(Fault::GuardMissing),
        Status::Waiting(Refusal::GuardNotConfirmed) => Some(Fault::GuardNotConfirmed),
        Status::Waiting(Refusal::GuardLost) => Some(Fault::GuardLost),
        Status::Waiting(Refusal::RecordNotSaved) => Some(Fault::RecordNotSaved),
        Status::Waiting(Refusal::SettingOff | Refusal::NoExplorerShell | Refusal::NothingToHide) | Status::Off | Status::Hidden(_) => None,
    }
}

/// The running feature. Dropping it stops it (the taskbar is shown again).
pub struct TaskbarHider {
    tx: Sender<Cmd>,
    hwnd: isize,
    status: Arc<Mutex<Status>>,
    thread: Option<std::thread::JoinHandle<()>>,
    ended: Receiver<()>,
}

impl TaskbarHider {
    /// Starts the thread and its window. Nothing is hidden until [`TaskbarHider::set_wanted`]`(true)`. The thread first undoes the
    /// record of a crashed earlier host, whatever the setting says.
    pub fn start(cfg: Config) -> Result<TaskbarHider, String> {
        install_panic_restore();
        let host_pid = std::process::id();
        let host_start = tbwin::process_start_time(host_pid).unwrap_or(0);
        let ctl = Controller::new(host_pid, host_start);
        let ctl = if cfg.band_known { ctl.with_band(cfg.band_px) } else { ctl };
        let (tx, rx) = std::sync::mpsc::channel::<Cmd>();
        let status = Arc::new(Mutex::new(Status::Off));
        let ctx = Box::new(Ctx { ctl, world: RealWorld::new(cfg), rx, status: status.clone(), wanted: false, dead: false, follow_ups: 0, stopping: false });
        let (wtx, wrx) = std::sync::mpsc::channel();
        let (etx, ended) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("taskbar-hide".into())
            .spawn(move || {
                run_thread(ctx, wtx);
                let _ = etx.send(());
            })
            .map_err(|e| e.to_string())?;
        let hwnd = wrx.recv_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())??;
        Ok(TaskbarHider { tx, hwnd, status, thread: Some(thread), ended })
    }

    fn post(&self, cmd: Cmd) {
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        if self.tx.send(cmd).is_ok() {
            // SAFETY: posting to our own window; if it is gone the post fails and nothing happens.
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as *mut c_void)), WM_CMD, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// The setting. `true` starts the guard, waits for its confirmation and hides; `false` shows the taskbar again.
    pub fn set_wanted(&self, on: bool) {
        self.post(Cmd::Wanted(on));
    }

    /// The dock's band changed (its size, or `dock_reserve`): `Some(thickness in device px)`, `None` for none. While the hide is in
    /// force the work area follows; before, it is the wish for when it comes - and the first word the dock says is what lets the
    /// bottom edge be taken at all. Session only.
    pub fn set_band(&self, band_px: Option<i32>) {
        self.post(Cmd::Band(band_px));
    }

    /// The docks on the monitors that are not the primary one changed their bands: `(the monitor's device name, thickness in device px)`
    /// for each dock that reserves one. While the hide is in force the work area of such a monitor, if its taskbar is one of the hidden
    /// ones, is the monitor less the band; the others are given whole, as before. The user's own work areas are written down first and
    /// put back on every way out, like the primary monitor's. Session only.
    pub fn set_other_bands(&self, bands: Vec<(String, i32)>) {
        self.post(Cmd::OtherBands(bands));
    }

    /// Whether the hide is confirmed and in force (the taskbar windows are hidden, and the work areas are the edge's).
    pub fn in_force(&self) -> bool {
        matches!(self.status(), Status::Hidden(_))
    }

    /// What the last look found.
    pub fn status(&self) -> Status {
        self.status.lock().map_or(Status::Off, |s| *s)
    }

    /// The fault that keeps the wanted hide from being in force, if that is what the last look found (see [`fault_of`]).
    pub fn fault(&self) -> Option<Fault> {
        fault_of(self.status())
    }

    /// Shows the taskbar again, ends the thread and waits for it for at most `wait`. After a timeout the thread is left to finish by
    /// itself (a restore that is still owed goes on being retried by its timer for as long as the process lives); the guard
    /// restores anything it did not.
    pub fn stop(mut self, wait: Duration) {
        self.post(Cmd::Stop);
        self.wait_for_end(wait);
    }

    /// Ends the thread without touching the taskbar (the thread has undone a stale record at its start, whatever the setting
    /// said). Waits for it for at most `wait`; a restore that startup could not finish goes on in the background.
    pub fn finish(mut self, wait: Duration) {
        self.post(Cmd::Finish);
        self.wait_for_end(wait);
    }

    fn wait_for_end(&mut self, wait: Duration) {
        if self.ended.recv_timeout(wait).is_ok() {
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        } else {
            log::warn!("taskbar hiding: the thread did not finish in time; it goes on with an owed restore, and the guard finishes what it cannot");
        }
    }
}

impl Drop for TaskbarHider {
    fn drop(&mut self) {
        // A hider that is dropped without `stop` still asks its thread to restore; it does not wait. (After a timed-out `stop` the
        // thread is still running and gets the same request again, which ends nothing early: it waits for an owed restore.)
        if self.thread.is_some() {
            self.post(Cmd::Stop);
        }
    }
}

/// Is a record of hidden windows in the state file of this folder (readable or not)? Read-only. The host looks at this once at its
/// start when the setting is off, to decide whether the one-shot undo of a crashed earlier host is needed.
pub fn stale_record_present(state_dir: &std::path::Path) -> bool {
    ShellStateFile::peek(&statefile::path_in(state_dir)).has_hidden_taskbar_key()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_is_expected_next_to_the_program() {
        let p = guard_next_to_exe();
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some(GUARD_FILE));
    }

    /// The sources of this file are scanned: the only calls that change a window or the work area go through `rf_shellguard`
    /// (`tbwin::set_visible`, `workarea::set_work_area`) and run only on the worker thread's controller. No test here starts it.
    #[test]
    fn this_file_calls_no_window_changing_api_itself() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/taskbar_hide.rs")).expect("own source");
        let code: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        // (assembled from parts so that this test does not name them itself)
        let banned = [["Set", "WindowPos"].concat(), ["Show", "Window("].concat(), ["SHApp", "BarMessage"].concat(), ["ABM", "_SETSTATE"].concat(), ["System", "ParametersInfo"].concat(), ["Reg", "SetValue"].concat()];
        for b in &banned {
            assert!(!code.contains(b.as_str()), "taskbar_hide.rs names {b}");
        }
        // the thread is started only by `TaskbarHider::start`, never by a test in this file
        let tests_at = code.find("#[cfg(test)]").expect("tests");
        let tests = &code[tests_at..];
        assert!(!tests.contains(&["TaskbarHider", "::start"].concat()));
    }

    #[test]
    fn the_guard_counter_belongs_to_the_process_so_a_new_world_never_reuses_a_name() {
        // `RealWorld::new` runs at every switch-on; the counter must not start again from 1 with it
        let a = next_guard_seq();
        let _world = RealWorld::new(Config { state_dir: PathBuf::new(), guard_exe: PathBuf::new(), band_px: None, band_known: false });
        let b = next_guard_seq();
        let _another = RealWorld::new(Config { state_dir: PathBuf::new(), guard_exe: PathBuf::new(), band_px: None, band_known: false });
        let c = next_guard_seq();
        assert!(a < b && b < c, "{a} {b} {c}");
        let (r1, d1) = event_names(4242, a);
        let (r2, d2) = event_names(4242, b);
        assert!(r1 != r2 && d1 != d2 && r1 != d1);
        assert!(r1.starts_with("Local\\") && d1.starts_with("Local\\"), "the session's own namespace");
    }

    #[test]
    fn an_event_name_that_is_in_use_is_refused_not_shared() {
        // a real, named, unsignalled event: no window, no process (the guard still ending in the real use holds such an object)
        let name = format!("Local\rustyfiles-test-event-{}-{}", std::process::id(), next_guard_seq());
        let first = create_new_event(&name).expect("a fresh name works");
        let second = create_new_event(&name);
        assert!(second.is_err(), "the existing object must not be handed out again");
        // SAFETY: the handle was just created here and is closed once.
        unsafe {
            let _ = CloseHandle(HANDLE(first as *mut c_void));
        }
        // once it is gone the name is free again
        let third = create_new_event(&name).expect("free again");
        unsafe {
            let _ = CloseHandle(HANDLE(third as *mut c_void));
        }
    }

    #[test]
    fn the_guard_is_started_out_of_the_hosts_job_first_and_inside_it_as_the_fallback() {
        let [first, second] = guard_creation_flag_sets();
        assert!(first & CREATE_BREAKAWAY_FROM_JOB != 0 && first & CREATE_NO_WINDOW != 0);
        assert!(second & CREATE_BREAKAWAY_FROM_JOB == 0 && second & CREATE_NO_WINDOW != 0);
    }

    #[test]
    fn a_record_in_the_state_file_is_seen_whether_or_not_it_can_be_read() {
        let dir = std::env::temp_dir().join(format!("rf-win-tbhide-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp folder");
        let p = statefile::path_in(&dir);
        assert!(!stale_record_present(&dir), "no file: nothing to undo");
        std::fs::write(&p, r#"{"version":1,"pending_restore":false}"#).expect("write");
        assert!(!stale_record_present(&dir), "a file without the key");
        std::fs::write(&p, r#"{"version":1,"hidden_taskbar":{"host_pid":5,"windows":[]}}"#).expect("write");
        assert!(stale_record_present(&dir), "a record that names nothing usable is still a reason to look");
        std::fs::write(&p, r#"{"version":1,"hidden_taskbar":"???"}"#).expect("write");
        assert!(stale_record_present(&dir), "garbled but there");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_fault_is_a_fault() {
        assert_eq!(fault_of(Status::Waiting(Refusal::GuardMissing)), Some(Fault::GuardMissing));
        assert_eq!(fault_of(Status::Waiting(Refusal::GuardNotConfirmed)), Some(Fault::GuardNotConfirmed));
        assert_eq!(fault_of(Status::Waiting(Refusal::GuardLost)), Some(Fault::GuardLost));
        assert_eq!(fault_of(Status::Waiting(Refusal::RecordNotSaved)), Some(Fault::RecordNotSaved));
        // the situation is not a fault: no Explorer taskbar to hide, none visible right now, the setting off, hidden, off
        for s in [Status::Off, Status::Hidden(2), Status::Waiting(Refusal::NoExplorerShell), Status::Waiting(Refusal::NothingToHide), Status::Waiting(Refusal::SettingOff)] {
            assert_eq!(fault_of(s), None, "{s:?}");
        }
    }

    #[test]
    fn the_wish_travels_as_a_command_and_the_status_starts_off() {
        // no thread, no window: only the plain parts
        let (tx, rx) = std::sync::mpsc::channel::<Cmd>();
        tx.send(Cmd::Wanted(true)).expect("send");
        assert!(matches!(rx.try_recv(), Ok(Cmd::Wanted(true))));
        assert_eq!(*Arc::new(Mutex::new(Status::Off)).lock().expect("lock"), Status::Off);
        assert_eq!(FOLLOW_MS.len(), 2);
    }
}
