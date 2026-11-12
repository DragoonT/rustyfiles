//! What the dock needs to know about the screen, and how it learns that it changed: the primary monitor (its rectangle, work area
//! and DPI), the band the application desktop toolbar reserves at the bottom of it, and one hidden window that hears about every
//! change.
//!
//! [`DockSystem`] owns that hidden window on a thread of its own (a zero-size, never shown pop-up: the toolbar needs an owner, and
//! the messages that tell of display, DPI, work-area, theme and clock changes are broadcast to top-level windows, which a
//! message-only window would not get). It sends [`SystemEvent`]s to a callback on that thread; the app turns them into a re-read of
//! [`primary_monitor`] and a re-placement of the islands. Dropping it takes the reserved band back.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::layout::{Rect, placement_area};

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

/// The window that holds our toolbar registration right now (0: none). Read by the panic hook, which can run on any thread.
static BAR_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

const WM_RESERVE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;
/// The message the system sends the toolbar's window about toolbars (`uCallbackMessage`).
const WM_BAR: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;
const TIMER_POLL: usize = 1;
const TIMER_DEBOUNCE: usize = 2;
const POLL_MS: u32 = 500;
const DEBOUNCE_MS: u32 = 150;

struct Ctx {
    on_event: Box<dyn Fn(SystemEvent) + Send>,
    shared: Arc<Mutex<Shared>>,
    fullscreen: Arc<AtomicBool>,
    registered: bool,
    last_monitor: Option<MonitorInfo>,
    ticks: u32,
    layout_pending: bool,
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
            let ctx = Box::new(Ctx { on_event: Box::new(on_event), shared: s2, fullscreen: f2, registered: false, last_monitor: None, ticks: 0, layout_pending: false });
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
        Some((m, placement_area(m.work, self.band(), m.rect)))
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

/// Registers (once), asks the system where the band may go and takes it. Updates the shared band.
unsafe fn apply_reservation(hwnd: windows::Win32::Foundation::HWND, ctx: &mut Ctx) {
    use windows::Win32::UI::Shell::{ABE_BOTTOM, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, SHAppBarMessage};
    let want = ctx.shared().want;
    let mut d = bar_data(hwnd);
    match want {
        Some(thickness) => {
            let Some(mon) = primary_monitor() else { return };
            if !ctx.registered {
                if unsafe { SHAppBarMessage(ABM_NEW, &mut d) } == 0 {
                    log::warn!("dock: the desktop toolbar could not be registered");
                    return;
                }
                ctx.registered = true;
                BAR_WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);
            }
            // ask for the whole bottom edge of the monitor; the system takes away what the Windows taskbar (and any other toolbar that
            // came first) already holds, then the band is cut to its thickness at the bottom of what is left
            d.uEdge = ABE_BOTTOM;
            d.rc = windows::Win32::Foundation::RECT { left: mon.rect.left, top: mon.rect.top, right: mon.rect.right, bottom: mon.rect.bottom };
            unsafe { SHAppBarMessage(ABM_QUERYPOS, &mut d) };
            d.rc.top = d.rc.bottom - thickness;
            let granted = rect_of(d.rc);
            if ctx.shared().band == Some(granted) {
                return; // nothing changed: no new SETPOS, so no new notification either
            }
            unsafe { SHAppBarMessage(ABM_SETPOS, &mut d) };
            ctx.shared().band = Some(granted);
            log::debug!("dock: reserved {granted:?}");
            ctx.emit(SystemEvent::LayoutChanged);
        }
        None => {
            if ctx.registered {
                unsafe { SHAppBarMessage(ABM_REMOVE, &mut d) };
                ctx.registered = false;
                BAR_WINDOW.store(0, Ordering::SeqCst);
            }
            if ctx.shared().band.take().is_some() {
                ctx.emit(SystemEvent::LayoutChanged);
            }
        }
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
            if ctx.registered {
                let mut d = bar_data(hwnd);
                windows::Win32::UI::Shell::SHAppBarMessage(windows::Win32::UI::Shell::ABM_REMOVE, &mut d);
                ctx.registered = false;
                BAR_WINDOW.store(0, Ordering::SeqCst);
            }
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
            // before, so forget it, or the new registration would be skipped as "unchanged".
            ctx.registered = false;
            ctx.shared().band = None;
            BAR_WINDOW.store(0, Ordering::SeqCst);
            apply_reservation(hwnd, ctx);
            schedule_layout(ctx);
            return LRESULT(0);
        }
        match msg {
            WM_RESERVE => {
                apply_reservation(hwnd, ctx);
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
                    TIMER_POLL => {
                        check_fullscreen(ctx);
                        ctx.ticks = ctx.ticks.wrapping_add(1);
                        // every 2 s: a DPI change does not always come with a message to a window that is not shown
                        if ctx.ticks.is_multiple_of(4) {
                            let now = primary_monitor();
                            if now != ctx.last_monitor && !ctx.layout_pending {
                                ctx.last_monitor = now;
                                schedule_layout(ctx);
                            }
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_ENDSESSION => {
                if wp.0 != 0 && ctx.registered {
                    let mut d = bar_data(hwnd);
                    windows::Win32::UI::Shell::SHAppBarMessage(windows::Win32::UI::Shell::ABM_REMOVE, &mut d);
                    ctx.registered = false;
                    BAR_WINDOW.store(0, Ordering::SeqCst);
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

    #[test]
    fn events_are_comparable_and_small() {
        assert_eq!(SystemEvent::Fullscreen(true), SystemEvent::Fullscreen(true));
        assert_ne!(SystemEvent::Fullscreen(true), SystemEvent::Fullscreen(false));
        assert!(size_of::<SystemEvent>() <= 2);
    }
}
