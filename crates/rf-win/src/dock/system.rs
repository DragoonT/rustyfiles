//! What the dock needs to know about the screen, and how it learns that it changed: the primary monitor (its rectangle, work area
//! and DPI), the band the application desktop toolbar reserves at the bottom of it, and one hidden window that hears about every
//! change.
//!
//! [`DockSystem`] owns that hidden window on a thread of its own (a zero-size, never shown pop-up: the toolbar needs an owner, and
//! the messages that tell of display, DPI, work-area, theme and clock changes are broadcast to top-level windows, which a
//! message-only window would not get). It sends [`SystemEvent`]s to a callback on that thread; the app turns them into a re-read of
//! [`primary_monitor`] and a re-placement of the islands. Dropping it takes the reserved band back.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::band::{AppBarCalls, Applied, Band, MAX_REGISTER_TRIES, TrayId, announcement_is_restart};
use super::layout::{Rect, placement_area};
use crate::shellhost::ShellMode;
use crate::shellhost::watch::{Cause, ModeWatch, Seen, SystemLook};

/// The primary monitor as the dock sees it, device px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    /// The monitor handle (an `HMONITOR`).
    pub handle: isize,
    /// The whole monitor.
    pub rect: Rect,
    /// What is left of it after the Windows taskbar and other desktop toolbars (ours included, once it has reserved its band).
    pub work: Rect,
    /// Effective DPI of the monitor (96 = 100 %).
    pub dpi: u32,
}

impl MonitorInfo {
    /// Device px per logical px.
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
}

/// Makes the calling thread per-monitor DPI aware (v2) for as long as it lives, so that rectangles come back in device px whatever
/// the process was started as. GPUI's own process already is; this makes the functions here correct from any caller.
struct Aware(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);

impl Aware {
    fn enter() -> Aware {
        use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
        Aware(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for Aware {
    fn drop(&mut self) {
        use windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext;
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}

/// The calling thread per-monitor DPI aware until the returned guard is dropped (for the other files of this module).
pub(super) fn aware_thread() -> impl Drop {
    Aware::enter()
}

fn rect_of(r: windows::Win32::Foundation::RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

/// The primary monitor now. `None` only when Windows has no monitor to give (a session without a display).
pub fn primary_monitor() -> Option<MonitorInfo> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    let _aware = Aware::enter();
    unsafe {
        // the primary monitor's top left corner is the origin of the virtual screen
        let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        if hmon.is_invalid() {
            return None;
        }
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
            return None;
        }
        let (mut dx, mut dy) = (0u32, 0u32);
        let dpi = if GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_ok() && dx > 0 { dx } else { 96 };
        Some(MonitorInfo { handle: hmon.0 as isize, rect: rect_of(mi.rcMonitor), work: rect_of(mi.rcWork), dpi })
    }
}

/// What changed on the system, for the dock to react to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemEvent {
    /// The display configuration, the primary monitor's DPI or its work area changed (also: the Windows taskbar moved, hid itself or
    /// came back, another desktop toolbar came or went, our own band was set). Read [`primary_monitor`] again and re-place the islands.
    /// Coalesced: a burst of messages gives one event.
    LayoutChanged,
    /// The light / dark theme or the accent colour changed. Read [`crate::accent`] again.
    ThemeChanged,
    /// A full-screen application came to the front on the primary monitor (`true`) or left it (`false`).
    Fullscreen(bool),
    /// The system time or time zone changed, or the machine woke up: show the clock again.
    TimeChanged,
    /// The shell window came, went or is another one (Explorer closed, restarted or replaced), and the mode is now this. Sent from the
    /// watch in [`crate::shellhost::watch`]; the receiver compares the mode with the one it knows to see whether the MODE changed.
    ShellChanged(ShellMode),
}

#[derive(Default)]
struct Shared {
    /// The thickness the app wants reserved (`None`: no band), device px.
    want: Option<i32>,
    /// The band the system granted.
    band: Option<Rect>,
}

/// The hidden window, the toolbar registration and the change watch. See the module docs.
pub struct DockSystem {
    hwnd: isize,
    shared: Arc<Mutex<Shared>>,
    fullscreen: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Signalled when the window thread has ended (so that dropping never waits for ever on a thread stuck in the shell).
    ended: std::sync::mpsc::Receiver<()>,
}

/// How long dropping waits for the window thread to take the band back and end.
const END_WAIT: Duration = Duration::from_secs(3);

/// The band the host took out of the work area ITSELF, with no shell to ask (see `shellhost/workarea.rs`). The islands float inside it
/// just like inside a granted band, so [`DockSystem::placement_area`] puts it back the same way. `None`: there is none.
static OWN_BAND: Mutex<Option<Rect>> = Mutex::new(None);

fn own_band() -> Option<Rect> {
    *OWN_BAND.lock().unwrap_or_else(|p| p.into_inner())
}

/// Tells the placement about a band the host took itself (or that it gave it back).
pub fn set_own_band(band: Option<Rect>) {
    *OWN_BAND.lock().unwrap_or_else(|p| p.into_inner()) = band;
}

// ---------------------------------------------------------------- the bottom edge while the taskbar hide is in force
//
// With `shell_hide_taskbar` on, the host hides Explorer's taskbar and TAKES THE BOTTOM EDGE: it sets the work area itself (the monitor
// less the dock's band) instead of leaving the hidden taskbar's strip in it (`rf_shellguard::tbwork`, `rf_win::taskbar_hide`). The dock
// has two parts in that: its placement works from the monitor's bottom edge, and its own desktop-toolbar registration with Explorer
// is GIVEN BACK and stays given back (a second band on top of the host's own). Both are one call here, [`take_edge`], and one back,
// [`give_back_edge`]; the registration is registered again when the edge is given back.

/// The dock's band while the host owns the bottom edge: the bottom strip of the primary monitor (empty when the dock reserves none).
static EDGE: Mutex<Option<Rect>> = Mutex::new(None);
/// The registration with Explorer is not to be held (the host owns the edge).
static APPBAR_SUPPRESSED: AtomicBool = AtomicBool::new(false);
/// Our registration with Explorer is held right now (kept by the system thread after every look).
static APPBAR_HELD: AtomicBool = AtomicBool::new(false);
/// The host has docks on the other monitors (or is about to): the 2 s tick then also looks at every monitor's rectangle, work area and
/// scale, so that a change of a monitor that is not the primary one (its scale, its resolution, its place) reaches the host too. Off, the
/// tick looks at the primary monitor only, as it always did.
static WATCH_ALL: AtomicBool = AtomicBool::new(false);

/// Switches the look at every monitor on or off (the host: a dock on every monitor is wanted).
pub fn watch_all_monitors(on: bool) {
    WATCH_ALL.store(on, Ordering::Relaxed);
}

/// The system window (0: there is none), so that the host's thread can wake it.
static SYSTEM_WINDOW: AtomicIsize = AtomicIsize::new(0);

fn edge_band() -> Option<Rect> {
    *EDGE.lock().unwrap_or_else(|p| p.into_inner())
}

fn wake_system() {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
    let h = SYSTEM_WINDOW.load(Ordering::SeqCst);
    if h != 0 {
        // SAFETY: posting to a window of this process; if it is gone the post fails and nothing happens.
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_RESERVE, WPARAM(0), LPARAM(0));
        }
    }
}

/// The host owns the bottom edge now: the dock's band is `band` (the bottom strip of the primary monitor), the islands are placed from
/// the monitor's bottom edge, and the registration with Explorer is given back and stays given back. Does NOT wait: [`appbar_held`]
/// turns false when the system thread has removed the registration. Telling it again only moves the strip.
pub fn take_edge(band: Rect) {
    *EDGE.lock().unwrap_or_else(|p| p.into_inner()) = Some(band);
    APPBAR_SUPPRESSED.store(true, Ordering::SeqCst);
    wake_system();
}

/// The host gives the edge back (the taskbar is shown again): the placement works from the work area again and the band is registered
/// with Explorer again. Does not wait. Safe when the edge was never taken.
pub fn give_back_edge() {
    let was = APPBAR_SUPPRESSED.swap(false, Ordering::SeqCst);
    let had = EDGE.lock().unwrap_or_else(|p| p.into_inner()).take().is_some();
    if was || had {
        wake_system();
    }
}

/// Whether the dock holds a desktop-toolbar registration with Explorer. False also when there is no dock system in this process.
pub fn appbar_held() -> bool {
    APPBAR_HELD.load(Ordering::SeqCst)
}

/// The window that holds our toolbar registration right now (0: none). Read by the panic hook, which can run on any thread.
static BAR_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

const WM_RESERVE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;
/// The message the system sends the toolbar's window about toolbars (`uCallbackMessage`).
const WM_BAR: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;
const TIMER_POLL: usize = 1;
const TIMER_DEBOUNCE: usize = 2;
/// One shot: the toolbar registration is tried again after Explorer refused it right after its announcement.
const TIMER_RETRY: usize = 3;
const RETRY_MS: u32 = 1000;
const POLL_MS: u32 = 500;
const DEBOUNCE_MS: u32 = 150;

struct Ctx {
    on_event: Box<dyn Fn(SystemEvent) + Send>,
    shared: Arc<Mutex<Shared>>,
    fullscreen: Arc<AtomicBool>,
    /// The toolbar registration and the band last granted (see `band.rs`).
    band: Band,
    /// Failed registrations since the last announcement or success.
    register_tries: u8,
    /// Whether the shell window is still the one the host saw (see `shellhost/watch.rs`).
    shell: ModeWatch,
    started: std::time::Instant,
    last_monitor: Option<MonitorInfo>,
    /// Every monitor as the last 2 s tick saw it (only kept while [`watch_all_monitors`] is on).
    last_all: Vec<MonitorInfo>,
    ticks: u32,
    layout_pending: bool,
    /// Explorer's taskbar window as it was when it took our registration (see [`announcement_is_restart`]).
    tray_at_registration: Option<TrayId>,
    /// The edge state the app was last told about, so that taking or giving back the edge is announced once.
    edge_seen: (bool, Option<Rect>),
}

impl Ctx {
    fn emit(&self, e: SystemEvent) {
        (self.on_event)(e);
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl DockSystem {
    /// Starts the hidden window and its thread. `on_event` runs on that thread (not the UI thread): hand the event over to the UI
    /// thread and return. It must not drop the `DockSystem`.
    pub fn start(on_event: impl Fn(SystemEvent) + Send + 'static) -> anyhow::Result<DockSystem> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let fullscreen = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel::<Result<isize, String>>();
        let (s2, f2) = (shared.clone(), fullscreen.clone());
        let (end_tx, ended) = std::sync::mpsc::channel::<()>();
        install_panic_cleanup();
        let thread = std::thread::Builder::new().name("rf-dock-system".into()).spawn(move || {
            let ctx = Box::new(Ctx {
                on_event: Box::new(on_event),
                shared: s2,
                fullscreen: f2,
                band: Band::new(),
                register_tries: 0,
                shell: ModeWatch::new(&SystemLook),
                started: std::time::Instant::now(),
                last_monitor: None,
                last_all: Vec::new(),
                ticks: 0,
                layout_pending: false,
                tray_at_registration: None,
                edge_seen: (false, None),
            });
            run(ctx, tx);
            let _ = end_tx.send(());
        })?;
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(hwnd)) => Ok(DockSystem { hwnd, shared, fullscreen, thread: Some(thread), ended }),
            Ok(Err(e)) => Err(anyhow::anyhow!(e)),
            Err(e) => Err(anyhow::anyhow!("the dock's system window did not start: {e}")),
        }
    }

    /// Asks the system to keep `thickness_px` (device px, see [`super::layout::reserved_thickness`]) free above the bottom of the
    /// primary monitor: maximised windows then stop above the band. Registers the toolbar the first time and moves it afterwards.
    /// Asynchronous: a [`SystemEvent::LayoutChanged`] follows when the work area has changed.
    pub fn reserve(&self, thickness_px: i32) {
        self.shared.lock().unwrap_or_else(|p| p.into_inner()).want = Some(thickness_px.max(1));
        self.post_reserve();
    }

    /// Gives the band back (the toolbar is unregistered). The dock then only floats.
    pub fn release(&self) {
        self.shared.lock().unwrap_or_else(|p| p.into_inner()).want = None;
        self.post_reserve();
    }

    fn post_reserve(&self) {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_RESERVE, WPARAM(0), LPARAM(0));
        }
    }

    /// The band the system granted, if any.
    pub fn band(&self) -> Option<Rect> {
        self.shared.lock().unwrap_or_else(|p| p.into_inner()).band
    }

    /// The area to place the islands in: the primary monitor's work area with this toolbar's band put back (see
    /// [`placement_area`]). `None` without a monitor.
    pub fn placement_area(&self) -> Option<(MonitorInfo, Rect)> {
        let m = primary_monitor()?;
        Some((m, placement_area(m.work, edge_band().or(self.band()).or(own_band()), m.rect)))
    }

    /// The last known state of the full-screen check (kept up to date by the watch).
    pub fn fullscreen(&self) -> bool {
        self.fullscreen.load(Ordering::Relaxed)
    }
}

impl Drop for DockSystem {
    fn drop(&mut self) {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take()
            && t.thread().id() != std::thread::current().id()
        {
            // The window thread makes synchronous calls to Explorer (the toolbar messages). With Explorer hung they never return,
            // and the dock must still be able to end: wait a while, then leave the thread to die with the process.
            if !matches!(self.ended.recv_timeout(END_WAIT), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                let _ = t.join();
            } else {
                log::warn!("dock: the system window did not end in {END_WAIT:?}; the toolbar band may stay until Explorer notices");
            }
        }
    }
}

/// If the dock panics (the release build then aborts, and no destructor runs) the toolbar band is given back first, from the panic
/// hook. Installed once; the hook that was there (the crash log) still runs after it.
fn install_panic_cleanup() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            remove_registered_bar();
            previous(info);
        }));
    });
}

/// `ABM_REMOVE` for the window that holds a registration, if any. Safe to call from any thread and more than once.
fn remove_registered_bar() {
    let h = BAR_WINDOW.swap(0, Ordering::SeqCst);
    if h != 0 {
        let mut d = bar_data(windows::Win32::Foundation::HWND(h as *mut _));
        unsafe { windows::Win32::UI::Shell::SHAppBarMessage(windows::Win32::UI::Shell::ABM_REMOVE, &mut d) };
    }
}

/// The id of the broadcast message Explorer sends every top-level window when its taskbar is (re)created: after it crashed or was
/// restarted it has forgotten every application toolbar.
fn taskbar_created() -> u32 {
    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| unsafe { windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(windows::core::w!("TaskbarCreated")) })
}

/// The toolbar message block of `SHAppBarMessage`, for this window.
fn bar_data(hwnd: windows::Win32::Foundation::HWND) -> windows::Win32::UI::Shell::APPBARDATA {
    windows::Win32::UI::Shell::APPBARDATA { cbSize: size_of::<windows::Win32::UI::Shell::APPBARDATA>() as u32, hWnd: hwnd, uCallbackMessage: WM_BAR, ..Default::default() }
}

/// The real toolbar calls: messages to Explorer's taskbar window (`SHAppBarMessage`), for this window.
struct RealBar {
    hwnd: windows::Win32::Foundation::HWND,
}

impl AppBarCalls for RealBar {
    fn register(&mut self) -> bool {
        let mut d = bar_data(self.hwnd);
        if unsafe { windows::Win32::UI::Shell::SHAppBarMessage(windows::Win32::UI::Shell::ABM_NEW, &mut d) } == 0 {
            return false;
        }
        BAR_WINDOW.store(self.hwnd.0 as isize, Ordering::SeqCst);
        true
    }

    fn query(&mut self, edge: Rect) -> Rect {
        let mut d = bar_data(self.hwnd);
        d.uEdge = windows::Win32::UI::Shell::ABE_BOTTOM;
        d.rc = windows::Win32::Foundation::RECT { left: edge.left, top: edge.top, right: edge.right, bottom: edge.bottom };
        unsafe { windows::Win32::UI::Shell::SHAppBarMessage(windows::Win32::UI::Shell::ABM_QUERYPOS, &mut d) };
        rect_of(d.rc)
    }

    fn set_pos(&mut self, rc: Rect) {
        let mut d = bar_data(self.hwnd);
        d.uEdge = windows::Win32::UI::Shell::ABE_BOTTOM;
        d.rc = windows::Win32::Foundation::RECT { left: rc.left, top: rc.top, right: rc.right, bottom: rc.bottom };
        unsafe { windows::Win32::UI::Shell::SHAppBarMessage(windows::Win32::UI::Shell::ABM_SETPOS, &mut d) };
    }

    fn remove(&mut self) {
        let mut d = bar_data(self.hwnd);
        unsafe { windows::Win32::UI::Shell::SHAppBarMessage(windows::Win32::UI::Shell::ABM_REMOVE, &mut d) };
        BAR_WINDOW.store(0, Ordering::SeqCst);
    }
}

/// Brings the band in line with what the app wants (see `band.rs` for the rules) and tells the app when it changed.
unsafe fn apply_reservation(hwnd: windows::Win32::Foundation::HWND, ctx: &mut Ctx) {
    use windows::Win32::UI::WindowsAndMessaging::SetTimer;
    // While the host owns the bottom edge no band is registered (the host sets the work area itself): the wish is "none".
    let want = if APPBAR_SUPPRESSED.load(Ordering::SeqCst) { None } else { ctx.shared().want };
    let view = primary_monitor().map(|m| (m.rect, m.work));
    let applied = ctx.band.apply(&mut RealBar { hwnd }, want, view);
    APPBAR_HELD.store(ctx.band.registered(), Ordering::SeqCst);
    if !ctx.band.registered() {
        ctx.tray_at_registration = None;
    }
    match applied {
        Applied::Set(granted) => {
            ctx.register_tries = 0;
            if ctx.tray_at_registration.is_none() {
                ctx.tray_at_registration = explorer_tray_id();
            }
            ctx.shared().band = Some(granted);
            log::debug!("dock: reserved {granted:?}");
            ctx.emit(SystemEvent::LayoutChanged);
        }
        Applied::Released => {
            ctx.shared().band = None;
            ctx.emit(SystemEvent::LayoutChanged);
        }
        Applied::RegisterFailed => {
            ctx.register_tries += 1;
            if ctx.register_tries < MAX_REGISTER_TRIES {
                log::debug!("dock: the desktop toolbar was refused (try {}), again in {RETRY_MS} ms", ctx.register_tries);
                unsafe { SetTimer(Some(hwnd), TIMER_RETRY, RETRY_MS, None) };
            } else {
                log::warn!("dock: the desktop toolbar could not be registered ({} tries); it is tried again at the next change of the screen or of the taskbar", ctx.register_tries);
                ctx.register_tries = 0;
            }
        }
        Applied::Unchanged => {}
    }
}

/// Explorer's own taskbar window right now (the one of class `Shell_TrayWnd` that Explorer owns): handle, owner, the owner's start
/// time. `None` when there is none or it cannot be told. A look through the list of windows, made only when an announcement or a
/// registration needs it.
fn explorer_tray_id() -> Option<TrayId> {
    let live = rf_shellguard::win::tbwin::list_taskbar_windows().ok()?;
    live.iter().find(|w| w.class == rf_shellguard::tbmodel::PRIMARY && w.explorer && w.start != 0).map(|w| TrayId { hwnd: w.handle, pid: w.pid, start: w.start })
}

/// The 2 s tick and the taskbar announcement ask the shell watch whether the shell window is another one; the app is told when it is.
fn watch_shell(ctx: &mut Ctx, cause: Cause) {
    let now = ctx.started.elapsed().as_millis() as u64;
    if let Seen::Changed { mode, mode_changed } = ctx.shell.check(&SystemLook, now, cause) {
        log::info!("shell host: the shell window changed, mode {}{}", mode.name(), if mode_changed { " (the mode changed)" } else { "" });
        ctx.emit(SystemEvent::ShellChanged(mode));
    }
}

fn run(ctx: Box<Ctx>, tx: std::sync::mpsc::Sender<Result<isize, String>>) {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, MSG, RegisterClassExW, SetTimer, TranslateMessage, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::w;
    unsafe {
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(Err(format!("module handle: {e}")));
                return;
            }
        };
        let class = w!("RustyfilesDockSystem");
        let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
        // a second DockSystem in the same process registers the same class again: that failing is fine
        RegisterClassExW(&wc);
        let raw = Box::into_raw(ctx);
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!(""), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), Some(raw as *const _));
        let hwnd = match hwnd {
            Ok(h) => h,
            Err(e) => {
                drop(Box::from_raw(raw));
                let _ = tx.send(Err(format!("the dock's system window: {e}")));
                return;
            }
        };
        SetTimer(Some(hwnd), TIMER_POLL, POLL_MS, None);
        SYSTEM_WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);
        let _ = tx.send(Ok(hwnd.0 as isize));
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: windows::Win32::Foundation::HWND, msg: u32, wp: windows::Win32::Foundation::WPARAM, lp: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        CREATESTRUCTW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, KillTimer, PostQuitMessage, SetTimer, SetWindowLongPtrW, WM_CLOSE, WM_DESTROY,
        WM_DISPLAYCHANGE, WM_ENDSESSION, WM_NCCREATE, WM_POWERBROADCAST, WM_SETTINGCHANGE, WM_THEMECHANGED, WM_TIMECHANGE, WM_TIMER,
    };
    // WM_DWMCOLORIZATIONCOLORCHANGED
    const WM_DWM_COLOR: u32 = 0x0320;
    // PBT_APMRESUMEAUTOMATIC
    const PBT_RESUME: usize = 0x0012;
    const SPI_SETWORKAREA: usize = 0x002F;

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
            // the last message: free the state, end the loop
            let mut ctx = Box::from_raw(ptr);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            // (the band is given back: want None)
            SYSTEM_WINDOW.store(0, Ordering::SeqCst);
            ctx.band.apply(&mut RealBar { hwnd }, None, None);
            APPBAR_HELD.store(false, Ordering::SeqCst);
            ctx.shared().band = None;
            drop(ctx);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        let ctx = &mut *ptr;
        let schedule_layout = |ctx: &mut Ctx| {
            if !ctx.layout_pending {
                ctx.layout_pending = true;
                SetTimer(Some(hwnd), TIMER_DEBOUNCE, DEBOUNCE_MS, None);
            }
        };
        if msg == taskbar_created() {
            // Explorer started again and knows nothing of our toolbar: register anew. The granted band is the same rectangle as
            // before, so forget it, or the new registration would be skipped as "unchanged". (Not every announcement is a restart - our
            // own tray host broadcasts it too - so `Band` gives a registration that is still held back first. BAR_WINDOW is left as it
            // is: it names the window that may hold a registration, which the panic hook gives back.)
            //
            // A broadcast that is not a restart (our own tray host sends one when it starts and stops) leaves the registration alone:
            // Explorer's taskbar window is provably the same one (see `announcement_is_restart`).
            let registered = ctx.band.registered();
            let restart = announcement_is_restart(registered, ctx.tray_at_registration, if registered { explorer_tray_id() } else { None });
            if restart {
                ctx.band.announced();
                ctx.tray_at_registration = None;
                ctx.register_tries = 0;
                ctx.shared().band = None;
                apply_reservation(hwnd, ctx);
                schedule_layout(ctx);
            } else {
                log::debug!("dock: TaskbarCreated from another program; Explorer's taskbar is the same one, the band stays");
            }
            // (and the shell window is looked at: it is new, and the parts that depend on it - the desktop, the band - are told)
            watch_shell(ctx, Cause::Announced);
            return LRESULT(0);
        }
        match msg {
            WM_RESERVE => {
                apply_reservation(hwnd, ctx);
                // the host took or gave back the bottom edge: the islands are placed again (from the edge, or from the work area)
                let edge = (APPBAR_SUPPRESSED.load(Ordering::SeqCst), edge_band());
                if edge != ctx.edge_seen {
                    ctx.edge_seen = edge;
                    ctx.emit(SystemEvent::LayoutChanged);
                }
                LRESULT(0)
            }
            WM_BAR => {
                // ABN_STATECHANGE 0, ABN_POSCHANGED 1: the Windows taskbar or another toolbar changed, ask again where the band may go
                // ABN_FULLSCREENAPP 2: look at the window in front now
                match wp.0 {
                    0 | 1 => {
                        apply_reservation(hwnd, ctx);
                        schedule_layout(ctx);
                    }
                    2 => check_fullscreen(ctx),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                schedule_layout(ctx);
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                if wp.0 == SPI_SETWORKAREA {
                    schedule_layout(ctx);
                } else if wp.0 == 0 && lp.0 != 0 {
                    // a named setting: "ImmersiveColorSet" is the theme or the accent, "TraySettings" the Windows taskbar's own
                    let name = windows::core::PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default();
                    match name.as_str() {
                        "ImmersiveColorSet" => ctx.emit(SystemEvent::ThemeChanged),
                        "TraySettings" | "WindowMetrics" => schedule_layout(ctx),
                        _ => {}
                    }
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_THEMECHANGED | WM_DWM_COLOR => {
                ctx.emit(SystemEvent::ThemeChanged);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_TIMECHANGE => {
                ctx.emit(SystemEvent::TimeChanged);
                LRESULT(0)
            }
            WM_POWERBROADCAST => {
                if wp.0 == PBT_RESUME {
                    ctx.emit(SystemEvent::TimeChanged);
                    schedule_layout(ctx);
                }
                LRESULT(1) // TRUE: the request is granted
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_DEBOUNCE => {
                        let _ = KillTimer(Some(hwnd), TIMER_DEBOUNCE);
                        ctx.layout_pending = false;
                        // the band depends on the monitor (its size, the taskbar's place): ask again, then tell the app
                        apply_reservation(hwnd, ctx);
                        ctx.last_monitor = primary_monitor();
                        ctx.emit(SystemEvent::LayoutChanged);
                    }
                    TIMER_RETRY => {
                        let _ = KillTimer(Some(hwnd), TIMER_RETRY);
                        apply_reservation(hwnd, ctx);
                    }
                    TIMER_POLL => {
                        check_fullscreen(ctx);
                        ctx.ticks = ctx.ticks.wrapping_add(1);
                        // every 2 s: a DPI change does not always come with a message to a window that is not shown
                        if ctx.ticks.is_multiple_of(4) {
                            // (one GetShellWindow call; see shellhost/watch.rs)
                            watch_shell(ctx, Cause::Tick);
                            let now = primary_monitor();
                            if now != ctx.last_monitor && !ctx.layout_pending {
                                ctx.last_monitor = now;
                                schedule_layout(ctx);
                            }
                            // the other monitors (a DPI change of one of them is not told to this window): only with docks on them
                            if WATCH_ALL.load(Ordering::Relaxed) {
                                let all: Vec<MonitorInfo> = super::monitors::displays().into_iter().map(|d| d.info).collect();
                                if all != ctx.last_all {
                                    let first = ctx.last_all.is_empty();
                                    ctx.last_all = all;
                                    if !first && !ctx.layout_pending {
                                        schedule_layout(ctx);
                                    }
                                }
                            } else if !ctx.last_all.is_empty() {
                                ctx.last_all.clear();
                            }
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_ENDSESSION => {
                if wp.0 != 0 {
                    // (written to the log so that a dock that ends with the session says why)
                    log::warn!("shell host exits: the session is ending");
                    ctx.band.apply(&mut RealBar { hwnd }, None, None);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn check_fullscreen(ctx: &mut Ctx) {
    let now = super::fullscreen::on_primary();
    if ctx.fullscreen.swap(now, Ordering::Relaxed) != now {
        ctx.emit(SystemEvent::Fullscreen(now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_primary_monitor_is_readable() {
        // read-only: this machine has a display (the suite needs a desktop session anyway)
        let m = primary_monitor().expect("a primary monitor");
        assert!(m.rect.width() > 0 && m.rect.height() > 0);
        assert!(m.rect.covers(&m.work), "{m:?}");
        assert!(m.dpi >= 96 || m.dpi > 0);
        assert!(m.scale() > 0.5);
    }

    /// The statics are the process's: this is the only test that touches them. There is no system window here (the id is 0), so
    /// nothing is posted and nothing reaches Explorer.
    #[test]
    fn taking_the_edge_suppresses_the_registration_and_giving_it_back_registers_again_and_is_safe_twice() {
        assert!(!APPBAR_SUPPRESSED.load(Ordering::SeqCst) && edge_band().is_none() && !appbar_held(), "nothing is taken at the start");
        let strip = Rect::new(0, 1362, 2560, 1440);
        take_edge(strip);
        assert!(APPBAR_SUPPRESSED.load(Ordering::SeqCst), "the registration is not to be held");
        assert_eq!(edge_band(), Some(strip), "the placement works from the monitor's bottom edge");
        // telling again only moves the strip
        let thinner = Rect::new(0, 1348, 2560, 1440);
        take_edge(thinner);
        assert_eq!(edge_band(), Some(thinner));
        give_back_edge();
        assert!(!APPBAR_SUPPRESSED.load(Ordering::SeqCst) && edge_band().is_none());
        give_back_edge();
        assert!(!APPBAR_SUPPRESSED.load(Ordering::SeqCst) && edge_band().is_none(), "a second give-back changes nothing");
        // (with no system window the held flag is simply false: the host's wait for the dock to let go ends at once)
        assert!(!appbar_held());
    }

    #[test]
    fn events_are_comparable_and_small() {
        assert_eq!(SystemEvent::Fullscreen(true), SystemEvent::Fullscreen(true));
        assert_ne!(SystemEvent::Fullscreen(true), SystemEvent::Fullscreen(false));
        assert!(size_of::<SystemEvent>() <= 2);
    }
}
