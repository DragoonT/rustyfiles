//! The engine thread: one for the whole process, started by the first `attach`. It owns the D3D11 device, the DirectComposition device,
//! one swap chain per glass window and one screen capture per monitor that has a visible glass window on it (a Windows.Graphics.Capture
//! session, or Desktop Duplication: see `capture.rs`). Every D3D, DXGI, WinRT and DirectComposition object is created and used here and
//! nowhere else.
//!
//! Where the glass goes: GPUI creates its DirectComposition target on the window with `topmost = true`. This engine adds a second
//! target on the same window with `topmost = false`, whose visual tree is composed BEHIND the scene GPUI draws. The app makes the
//! GPUI window transparent, so the glass shows through wherever the scene is.
//!
//! WORK ONLY FOR GLASS THAT IS SEEN. A glass window that is hidden, minimised or cloaked is left out of everything, and so is one that
//! the windows above it hide completely (`cover.rs`; looked at when the foreground window changes or moves, about ten times a second
//! while some glass is covered and twice a second otherwise, never per frame). A window that is left out is not prepared, not drawn and
//! not read back, and it reports the state it had: a window that was showing glass stays `Running` with its last picture on the screen
//! (the app must not fall back to its Mica fill and back for a cover that lasts a few seconds). When the window can be seen again it
//! is drawn from a fresh backdrop and presented whatever the loop guard remembers: its capture is resumed first (`Monitor::resume`: the
//! frames queued from before the pause are never drawn from), and until a frame made after the resume is in, the window keeps its last
//! picture. When no glass on a monitor is seen, that monitor's capture session is closed after `SESSION_GRACE` (its frame pool and
//! mirror are tens of megabytes of video memory, and the capture service stops working for us), and opened again on demand; a failure
//! to open it again is tried again a few times with a pause (`Reopen`) before the whole process gives up the session backend.
//!
//! WORK ONLY AS OFTEN AS IT IS LOOKED AT. Glass that is seen but not in front (not the foreground window, the pointer not over it,
//! `rest.rs`) follows the screen at 20 frames a second at most: its monitor's capture is asked for that rate when no glass on it is in
//! front, and a window that is behind while another one of its monitor is in front still draws from a new backdrop only at that rate.
//! The window the app marks and the setting asks for can be frozen: it is then left out like a covered one. A window that nobody
//! sees gives back its backdrop pyramid after a while. A change of the foreground window (`watch.rs`) and the pointer coming over a window
//! (`glass::pointer_over`) are events that wake the loop; the only clock added is the 1 s net while all glass is frozen (`FROZEN_NET`).
//!
//! The loop never spins: it waits on the command channel when there is nothing to poll, waits inside AcquireNextFrame (or on the session's
//! frame token) when it has a capture, and sleeps when neither happened. While a lens moves or fades it is paced by the display instead
//! (`pace`): one turn per refresh, and none of the other waits. A glass SHAPE that moves (a dock island widening) is not paced: its
//! commands come once per frame of the app's own animation, and the engine takes each the moment it arrives (a capture session's wait
//! ends on a command; Desktop Duplication's is cut short, see `SHAPE_WAIT_MS`) and presents it at once, so it lands in the same
//! composition as the scene the app drew for that frame. Pacing it by the display would show every frame one refresh late.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use windows::Win32::Foundation::{HWND, LUID, POINT, RECT};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::DirectComposition::{DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_CLOAKED, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT, DWMWCP_DONOTROUND, DWMWCP_ROUNDSMALL, DwmFlush, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIDevice3, IDXGIFactory2, IDXGISwapChain1,
};
use windows::Win32::Graphics::Gdi::{ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{GA_ROOTOWNER, GetAncestor, GetClientRect, GetCursorPos, IsIconic, IsWindow, IsWindowVisible, IsZoomed, WindowFromPoint};
use windows::core::{BOOL, Interface};

use super::capture::{Dirty, MonError, Monitor, Pumped, is_device_lost, locate};
use super::cover;
use super::crossproc::{self, Foreign};
use super::geom::Rc;
use super::gpu::{Geometry, Gpu, LensList, Look, Region, Renderer, glass_pass, prep, region_pad, reuse};
use super::lens::LensSet;
use super::rest::{self, Care};
use super::watch;
use super::retry::SessionRetry;
use super::wgc::{self, Access, Exclusion, Power};
use super::{CaptureMode, Cmd, GlassOptions, GlassState, Shadow, set_mode};

/// A window's corner radius, device px at 100 %: Windows 11 rounds its windows by 8 px, or by 4 px when asked for small corners.
const WINDOW_RADIUS: f32 = 8.0;
const WINDOW_RADIUS_SMALL: f32 = 4.0;
/// How long the loop sleeps on the channel when no glass window is visible.
const IDLE_WAIT: Duration = Duration::from_millis(250);
/// AcquireNextFrame's wait on the first monitor with a duplication, and the sleep of a turn that waited nowhere.
const FRAME_WAIT_MS: u32 = 8;
/// The same wait once nothing has happened for `QUIET_AFTER` (no frame, no window that moved, no command): a frame still ends the wait at
/// once, so only the notice of a window that starts to move is slower, by this much and only for the first turn. On an idle desktop this
/// is what keeps the thread from waking 125 times a second.
const QUIET_WAIT_MS: u32 = 50;
const QUIET_AFTER: Duration = Duration::from_secs(1);
/// While a window has a lens (even at rest) and the capture is Desktop Duplication, the quiet wait is cut to this: AcquireNextFrame
/// cannot be interrupted, so a lens command is only seen between two waits, and the selection pill must start sliding about when the
/// scene shows the new selection, not up to `QUIET_WAIT_MS` later. A capture session does not need it: its wait also ends the moment a
/// command is queued (`wgc::wait_end`), and every glass window of the app holds a lens at rest, so using it there kept the thread at
/// about 62 turns a second on an idle desktop instead of 20.
const LENS_WAIT_MS: u32 = 16;
/// A glass shape that changed this recently (a dock island widening under the pointer) shortens the wait for a frame to
/// `SHAPE_WAIT_MS`; this long after the last change it is at rest again and costs nothing.
const SHAPE_ACTIVE: Duration = Duration::from_millis(120);
/// The wait for a frame while a shape is moving: a command ends the wait of a capture session at once, and Desktop Duplication's
/// `AcquireNextFrame` does not know about commands, so this bounds how long a new shape waits to be taken.
const SHAPE_WAIT_MS: u32 = 2;
/// A DwmFlush that returns faster than this did not wait for a composition at all.
const INSTANT_FLUSH: Duration = Duration::from_micros(500);
/// This many such returns in a row and `pace` waits on the channel instead, so a broken DwmFlush cannot make a busy loop.
const INSTANT_FLUSHES_BEFORE_WAIT: u32 = 3;
/// How long a monitor's capture session is kept after the last glass that needed it stopped being seen (hidden, minimised, covered):
/// a window that comes back within this time (an Alt+Tab, a minimise and restore) finds the session as it was.
const SESSION_GRACE: Duration = Duration::from_secs(5);
/// How often the z order is walked to see whether a glass window is covered, while no glass is covered (a window is then only
/// noticed to go under others: that costs nothing but the work it could have saved) / while some glass is covered (it may come into
/// view any moment: the window that covered it was closed, minimised or moved, and nothing tells the engine). A change of the
/// foreground window, or a move of it, looks at once (`Engine::update_cover`).
const COVER_EVERY: Duration = Duration::from_millis(500);
const COVER_POLL: Duration = Duration::from_millis(100);
/// The longest the loop sleeps while all its glass is frozen and the foreground window is watched by an event: the net for a frozen
/// window that another program moves or resizes (no event reaches the engine for that).
const FROZEN_NET: Duration = Duration::from_millis(1000);
/// How often the power state (and with it the capture rate) is looked at.
/// The pauses before a capture session that could not be opened again (after it had worked in this run) is tried once more. A failure
/// after the last of them is a real one: the process goes over to Desktop Duplication.
const REOPEN_PAUSES: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(5)];
const POWER_CHECK_EVERY: Duration = Duration::from_secs(5);
const STATS_EVERY: Duration = Duration::from_secs(5);
const DEVICE_CHECK_EVERY: Duration = Duration::from_secs(1);
/// How often a running session backend looks again at the permission it was started on (reading it is a cheap, silent query).
const ACCESS_CHECK_EVERY: Duration = Duration::from_secs(10);

/// The wait of a turn on a quiet desktop. Only Desktop Duplication needs the shorter one while a lens is held (see `LENS_WAIT_MS`).
/// How long the loop sleeps when no glass is worked for. Covered glass may come into view at any moment and nothing tells (the z order
/// is looked at), so it is looked at often. Glass that is only frozen comes back by an event (the foreground window changed, the
/// pointer came over the window), so with the foreground watch set it sleeps `FROZEN_NET`, which is only the net for what has no event
/// (the window moved by another program); without the watch it is looked at as often as covered glass. Hidden glass is waited for as before.
fn idle_wait(covered: bool, frozen: bool, events: bool) -> Duration {
    if covered || (frozen && !events) {
        COVER_POLL
    } else if frozen {
        FROZEN_NET
    } else {
        IDLE_WAIT
    }
}

fn quiet_wait_ms(mode: CaptureMode, lens_held: bool) -> u32 {
    if lens_held && mode != CaptureMode::Session { LENS_WAIT_MS } else { QUIET_WAIT_MS }
}

fn hwnd_of(h: isize) -> HWND {
    HWND(h as *mut c_void)
}

/// Why a draw failed: for this window only, or for the device (and with it all windows).
enum Fail {
    Window(String),
    Device(String),
}

fn ctx(what: &str, e: windows::core::Error) -> Fail {
    if is_device_lost(&e) { Fail::Device(format!("{what}: {e}")) } else { Fail::Window(format!("{what}: {e}")) }
}

/// The device-wide objects, made when the first window needs them and dropped when the last one is gone.
struct Gfx {
    r: Renderer,
    factory: IDXGIFactory2,
    dcomp: IDCompositionDevice,
    luid: LUID,
}

impl Gfx {
    /// On the adapter the monitor `hmon` is attached to (a duplication only works on the device of its own adapter).
    unsafe fn new(hmon: isize) -> Result<Gfx, String> {
        let l = locate(hmon).map_err(|e| format!("the window's monitor is not among the DXGI outputs: {e}"))?;
        let luid = l.adapter.GetDesc1().map_err(|e| format!("adapter description: {e}"))?.AdapterLuid;
        let gpu = Gpu::on_adapter(&l.adapter).map_err(|e| format!("cannot create the D3D11 device: {e}"))?;
        let r = Renderer::new(gpu, false)?;
        let dxgi_dev: IDXGIDevice = r.g.dev.cast().map_err(|e| format!("IDXGIDevice: {e}"))?;
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_dev).map_err(|e| format!("cannot create the DirectComposition device: {e}"))?;
        let factory: IDXGIFactory2 = CreateDXGIFactory1().map_err(|e| format!("cannot create the DXGI factory: {e}"))?;
        Ok(Gfx { r, factory, dcomp, luid })
    }

    /// Lets the driver release the memory it caches for this device (what Windows asks of an app that goes to the background). Nothing
    /// is bound or in flight when it is called; the next draw allocates what it needs again.
    unsafe fn trim(&self) {
        self.r.unbind();
        self.r.g.ctx.ClearState();
        self.r.g.ctx.Flush();
        if let Ok(d) = self.r.g.dev.cast::<IDXGIDevice3>() {
            d.Trim();
        }
    }
}

/// The window that owns `hwnd` all the way up (itself when it is not owned): the window the user would call "the app's window".
fn root_owner(hwnd: isize) -> isize {
    unsafe { GetAncestor(hwnd_of(hwnd), GA_ROOTOWNER).0 as isize }
}

/// The root owner of the window under the pointer, 0 when there is none (or the pointer cannot be read).
fn root_at_pointer() -> isize {
    unsafe {
        let mut p = POINT::default();
        if GetCursorPos(&mut p).is_err() {
            return 0;
        }
        let h = WindowFromPoint(p);
        if h.0.is_null() { 0 } else { GetAncestor(h, GA_ROOTOWNER).0 as isize }
    }
}

/// The glass of one window on the screen: the composition target on the window, its visual, and the swap chain in it.
struct Target {
    sc: IDXGISwapChain1,
    /// Held so the visual outlives the target's root pointer.
    _visual: IDCompositionVisual,
    target: IDCompositionTarget,
    w: u32,
    h: u32,
}

impl Target {
    unsafe fn new(gfx: &Gfx, hwnd: isize, w: u32, h: u32) -> Result<Target, Fail> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w,
            Height: h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: BOOL(0),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let sc = gfx.factory.CreateSwapChainForComposition(&gfx.r.g.dev, &desc, None).map_err(|e| ctx("creating the swap chain", e))?;
        let visual = gfx.dcomp.CreateVisual().map_err(|e| ctx("creating the visual", e))?;
        visual.SetContent(&sc).map_err(|e| ctx("putting the swap chain into the visual", e))?;
        // topmost = false: composed behind the target GPUI made on the same window with topmost = true
        let target = gfx.dcomp.CreateTargetForHwnd(hwnd_of(hwnd), false).map_err(|e| ctx("creating the composition target", e))?;
        target.SetRoot(&visual).map_err(|e| ctx("setting the root visual", e))?;
        gfx.dcomp.Commit().map_err(|e| ctx("committing the composition", e))?;
        let t = Target { sc, _visual: visual, target, w, h };
        // both flip buffers must hold a cleared frame before the first draw, or DirectComposition may show uninitialised memory
        for _ in 0..2 {
            t.clear_and_present(gfx)?;
        }
        Ok(t)
    }

    unsafe fn clear_and_present(&self, gfx: &Gfx) -> Result<(), Fail> {
        let back: ID3D11Texture2D = self.sc.GetBuffer(0).map_err(|e| ctx("swap chain buffer", e))?;
        let mut rtv = None;
        gfx.r.g.dev.CreateRenderTargetView(&back, None, Some(&mut rtv)).map_err(|e| ctx("render target view", e))?;
        if let Some(rtv) = rtv {
            gfx.r.g.ctx.ClearRenderTargetView(&rtv, &[0.0; 4]);
        }
        self.sc.Present(0, DXGI_PRESENT(0)).ok().map_err(|e| ctx("Present", e))
    }

    unsafe fn resize(&mut self, gfx: &Gfx, w: u32, h: u32) -> Result<(), Fail> {
        // ResizeBuffers fails while anything still holds a buffer: nothing outside a draw does, and ClearState lets go of the context's
        gfx.r.g.ctx.ClearState();
        gfx.r.g.ctx.Flush();
        self.sc.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0)).map_err(|e| ctx("resizing the swap chain", e))?;
        self.w = w;
        self.h = h;
        Ok(())
    }

    /// Takes the glass off the window. Errors do not matter: the window may be gone already.
    unsafe fn remove(&self, gfx: &Gfx) {
        let _ = self.target.SetRoot(None::<&IDCompositionVisual>);
        let _ = gfx.dcomp.Commit();
    }
}

/// Where a window is and what it needs, as measured in one turn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WinGeo {
    /// Client area in screen px.
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    dpi: u32,
    /// Maximized, or covering its whole monitor: square corners.
    square: bool,
    /// The radius the system cuts this window's corners with, px at 100 % (0: it does not round, as on Windows 10).
    round: f32,
    hmon: isize,
}

impl WinGeo {
    fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
    fn radius_px(&self) -> f32 {
        if self.square { 0.0 } else { self.round * self.scale() }
    }
}

enum Seen {
    Gone,
    Hidden,
    Shown(WinGeo),
}

/// What the window manager says about a window: is it cloaked, and how does the system round its corners (px at 100 %).
#[derive(Clone, Copy, Debug)]
struct DwmFacts {
    at: Instant,
    cloaked: bool,
    round: f32,
}

/// The window manager is asked about a window at most this often. Each question is a call into another process (two changes of
/// thread), every window was asked two of them twice a turn, and with a video on the screen the engine turned 125 times a second:
/// with eight glass windows that alone was most of the 9 400 context switches a second measured on the engine's thread (2026-11-14).
/// Both answers change rarely (a window is cloaked when its virtual desktop goes away; the corner preference is set when the window
/// is made or its glass changes shape), and a command for the window asks again at once (`Engine::command`).
const DWM_ASK_EVERY: Duration = Duration::from_millis(250);

unsafe fn ask_dwm(h: HWND, now: Instant) -> DwmFacts {
    let mut cloaked = 0u32;
    let cloaked = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, size_of::<u32>() as u32).is_ok() && cloaked != 0;
    // What the system does with the corners: the attribute does not exist before Windows 11 (the call fails: square corners there).
    let mut pref = DWMWCP_DEFAULT;
    let round = if DwmGetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &mut pref as *mut _ as *mut c_void, size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32).is_err() || pref == DWMWCP_DONOTROUND {
        0.0
    } else if pref == DWMWCP_ROUNDSMALL {
        WINDOW_RADIUS_SMALL
    } else {
        WINDOW_RADIUS
    };
    DwmFacts { at: now, cloaked, round }
}

unsafe fn measure(hwnd: isize, dwm: &mut BTreeMap<isize, DwmFacts>, now: Instant) -> Seen {
    let h = hwnd_of(hwnd);
    if !IsWindow(Some(h)).as_bool() {
        dwm.remove(&hwnd);
        return Seen::Gone;
    }
    if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
        return Seen::Hidden;
    }
    let facts = match dwm.get(&hwnd) {
        Some(f) if now.saturating_duration_since(f.at) < DWM_ASK_EVERY => *f,
        _ => {
            let f = ask_dwm(h, now);
            dwm.insert(hwnd, f);
            f
        }
    };
    if facts.cloaked {
        return Seen::Hidden;
    }
    let mut rc = RECT::default();
    let mut origin = POINT { x: 0, y: 0 };
    if GetClientRect(h, &mut rc).is_err() || !ClientToScreen(h, &mut origin).as_bool() {
        return Seen::Hidden;
    }
    let (w, ht) = ((rc.right - rc.left).max(0) as u32, (rc.bottom - rc.top).max(0) as u32);
    if w == 0 || ht == 0 {
        return Seen::Hidden;
    }
    let dpi = match GetDpiForWindow(h) {
        0 => 96,
        d => d,
    };
    let hmon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    let covers = GetMonitorInfoW(hmon, &mut mi).as_bool()
        && origin.x <= mi.rcMonitor.left
        && origin.y <= mi.rcMonitor.top
        && origin.x + w as i32 >= mi.rcMonitor.right
        && origin.y + ht as i32 >= mi.rcMonitor.bottom;
    Seen::Shown(WinGeo { x: origin.x, y: origin.y, w, h: ht, dpi, square: covers || IsZoomed(h).as_bool(), round: facts.round, hmon: hmon.0 as isize })
}

/// What was last put on the screen for a window. A draw that would produce the same is not made.
#[derive(Clone, Copy, PartialEq)]
struct Presented {
    hash: u64,
    geo: Geometry,
    look: Look,
    /// Where the lenses were drawn and how opaque: a moving lens makes a draw different, a resting one does not.
    lenses: LensList,
}

struct Stats {
    since: Instant,
    frames: u32,
    with_image: u32,
    presented: u32,
    skipped: u32,
    /// Draws that kept the backdrop of the last one (only a lens or the glass shape moved) and so made no copy, no pyramid and no read-back.
    reused: u32,
    prep: Duration,
    glass: Duration,
}

impl Stats {
    fn new(now: Instant) -> Stats {
        Stats { since: now, frames: 0, with_image: 0, presented: 0, skipped: 0, reused: 0, prep: Duration::ZERO, glass: Duration::ZERO }
    }
}

/// Why the engine thread woke, counted per cause and logged with the other statistics (`RUSTYFILES_GLASS_STATS=1`). On the capture-session
/// path the thread was seen waking about 500 times a second on a desktop where nothing changed behind the glass, and the reason was not
/// found by reading the code; these counters are there to find it on a live desktop instead of guessing.
#[derive(Default)]
struct Turns {
    since: Option<Instant>,
    /// Turns of the loop (`step`).
    turns: u32,
    /// Commands taken, by name.
    commands: BTreeMap<&'static str, u32>,
    /// Waits for a frame that ended with a frame taken / with none (a command, a frame of another monitor, or the time).
    frames: u32,
    no_frame: u32,
    /// Waits for a command that ran out of time.
    timeouts: u32,
    /// CPU cycles of the engine thread itself in each part of a turn ([`PHASES`]): which part the time goes to. Counted only with the
    /// statistics on.
    cycles: [u64; PHASES.len()],
}

/// The parts of a turn the cycles are counted for: looking at the windows (where they are, what covers them, which is in front),
/// keeping the captures in step with them, taking frames, drawing, and the housekeeping.
const PHASES: [&str; 5] = ["look", "sync", "frames", "draw", "housekeeping"];

#[link(name = "kernel32")]
unsafe extern "system" {
    /// (declared here: the bindings have it in a module this crate does not otherwise need)
    fn QueryThreadCycleTime(thread: *mut c_void, cycles: *mut u64) -> i32;
}

/// The CPU cycles this thread has used so far.
fn thread_cycles() -> u64 {
    let mut c = 0u64;
    // SAFETY: a query about the calling thread, into a value of this function's own
    unsafe {
        QueryThreadCycleTime(windows::Win32::System::Threading::GetCurrentThread().0, &mut c);
    }
    c
}

/// Counts the cycles between two marks of a turn into [`Turns::cycles`]. Off (the statistics are off): nothing is asked or counted.
struct Lap {
    on: bool,
    last: u64,
}

impl Lap {
    fn start(on: bool) -> Lap {
        Lap { on, last: if on { thread_cycles() } else { 0 } }
    }

    fn mark(&mut self, turns: &mut Turns, phase: usize) {
        if !self.on {
            return;
        }
        let now = thread_cycles();
        turns.cycles[phase] += now.saturating_sub(self.last);
        self.last = now;
    }
}

impl Turns {
    /// The line for the log, per second over `secs`; `None` when it is too early. Resets the counters when it returns a line.
    fn report(&mut self, now: Instant) -> Option<String> {
        let since = *self.since.get_or_insert(now);
        let secs = now.saturating_duration_since(since).as_secs_f64();
        if secs < STATS_EVERY.as_secs_f64() {
            return None;
        }
        let per = |n: u32| n as f64 / secs;
        let cmds: u32 = self.commands.values().sum();
        let names: Vec<String> = self.commands.iter().map(|(k, v)| format!("{k} {:.1}", per(*v))).collect();
        let parts: Vec<String> = PHASES.iter().zip(self.cycles).map(|(name, c)| format!("{name} {:.1}", c as f64 / secs / 1e6)).collect();
        let line = format!(
            "glass engine thread: {:.0} turns/s, {:.1} commands/s ({}), {:.1} frame waits/s ended by a frame, {:.1} ended without one, {:.1} command waits/s ran out; million CPU cycles/s of this thread: {}",
            per(self.turns),
            per(cmds),
            if names.is_empty() { "none".to_string() } else { names.join(", ") },
            per(self.frames),
            per(self.no_frame),
            per(self.timeouts),
            parts.join(", "),
        );
        *self = Turns { since: Some(now), ..Turns::default() };
        Some(line)
    }
}

struct Win {
    hwnd: isize,
    dark: bool,
    /// The radius the app asked for, device px (`None`: what the system does, see `effective_radius`).
    radius_px: Option<f32>,
    /// The app asked for the glass to cut its own corners (a shape that is not a window).
    cut_corners: bool,
    /// The glass is only this rectangle of the window (device px from its top left corner), see `GlassOptions::shape`.
    shape: Option<[f32; 4]>,
    shadow: Option<Shadow>,
    /// When the shape last changed: while that is recent the engine does not wait long for a frame (`Engine::frame_wait_ms`).
    shape_changed: Option<Instant>,
    visible: bool,
    /// Other windows hide this one completely (see `cover.rs`). Such a window is not worked for: see `Win::active`.
    covered: bool,
    geo: Option<WinGeo>,
    target: Option<Target>,
    region: Option<Region>,
    dirty: bool,
    presented: Option<Presented>,
    stats: Stats,
    /// The lenses (selection pills) the app asked for, with their springs.
    lenses: LensSet,
    /// When the lenses were last advanced; `None` while they rest, so the first step of a new motion is a zero step.
    lens_tick: Option<Instant>,
    /// The backdrop has to be copied and blurred again (a new picture of the screen, another place or size, another look). A draw that
    /// is dirty for a lens alone keeps the last backdrop.
    stale: bool,
    /// What the window is owed while it is seen (`rest.rs`): the full rate in front, a slower one behind, nothing when frozen.
    care: Care,
    /// The app marked this window as one whose glass may freeze while it is not in front (`set_freezable`).
    freezable: bool,
    /// Frozen: seen, not in front, asked to freeze and with a picture. Not worked for (see `Win::active`).
    frozen: bool,
    /// When the window was last in front or under the pointer.
    last_front: Option<Instant>,
    /// A frame of this window has been put on the screen (a window with none is never frozen: it would stay empty).
    had_picture: bool,
    /// Since when the window has not been worked for (hidden, covered or frozen); `None` while it is.
    idle_since: Option<Instant>,
    /// When the window last drew from a backdrop it looked at (a window that is not in front draws at most `rest::BEHIND_FPS` times a second).
    last_draw: Option<Instant>,
}

impl Win {
    fn new(hwnd: isize, opts: GlassOptions) -> Win {
        Win {
            hwnd,
            dark: opts.dark,
            radius_px: opts.radius_px,
            cut_corners: opts.cut_corners,
            shape: opts.shape,
            shadow: opts.shadow,
            shape_changed: None,
            visible: false,
            covered: false,
            geo: None,
            target: None,
            region: None,
            dirty: true,
            presented: None,
            stats: Stats::new(Instant::now()),
            lenses: LensSet::new(),
            lens_tick: None,
            stale: true,
            care: Care::Front,
            freezable: false,
            frozen: false,
            last_front: None,
            had_picture: false,
            idle_since: None,
            last_draw: None,
        }
    }

    /// Whether anybody can see this glass: it is shown, not hidden under other windows and not frozen. Only an active window is
    /// prepared, drawn, read back and counted when the engine decides what to capture and how long to wait.
    fn active(&self) -> bool {
        self.visible && !self.covered && !self.frozen
    }

    /// Freezes the glass (`true`) or lets it follow the screen again. Thawing means drawing at once from a fresh backdrop, whatever
    /// the loop guard remembers, like coming into view from under a cover. Returns whether it thawed.
    fn set_frozen(&mut self, frozen: bool) -> bool {
        if frozen == self.frozen {
            return false;
        }
        self.frozen = frozen;
        if frozen {
            return false;
        }
        self.restale();
        self.presented = None;
        true
    }

    /// Records whether other windows hide this one. Coming into view again means drawing at once from a fresh backdrop, and presenting
    /// whatever the loop guard remembers: the picture on the screen is from before the cover. Returns whether it came into view.
    fn set_covered(&mut self, covered: bool) -> bool {
        if covered == self.covered {
            return false;
        }
        self.covered = covered;
        if covered {
            return false;
        }
        self.restale();
        self.presented = None;
        true
    }

    /// The window's rectangle (or its monitor) changed: somebody is moving or resizing it, and its glass has to follow at once, at the
    /// rate of the window in front, whoever has the focus: a window that animates its size on its own (a panel growing, a window moved by
    /// another program) would otherwise get a frost drawn 20 times a second while its edge moves 60 times. It is treated as in front for
    /// `rest::HOLD` from now on, and a frozen window thaws.
    fn geometry_changed(&mut self, g: WinGeo, now: Instant) {
        self.geo = Some(g);
        self.restale();
        // A window that moves is in view (its owner is dragging it), and it reads a new part of the screen at every step.
        self.covered = false;
        self.last_front = Some(now);
        self.care = Care::Front;
        self.set_frozen(false);
    }

    /// Takes new options. Returns whether the place the glass shows on the screen changed in a way that makes a new look at the z order
    /// worth it right now: the shape of a covered window moved (it may be in view now), or the glass was nothing at all (an island
    /// attached with an empty shape, which is never "covered") and now has an area.
    fn set_options(&mut self, opts: GlassOptions) -> bool {
        let before = GlassOptions { dark: self.dark, radius_px: self.radius_px, cut_corners: self.cut_corners, shape: self.shape, shadow: self.shadow };
        if before == opts {
            return false;
        }
        let relook = before.shape != opts.shape && (self.covered || shape_is_empty(before.shape));
        let cheap = before.only_the_shape_differs(&opts) || before.only_the_shape_and_radius_differ(&opts);
        self.dark = opts.dark;
        self.radius_px = opts.radius_px;
        self.cut_corners = opts.cut_corners;
        if before.shape != opts.shape || before.shadow != opts.shadow {
            self.shape_changed = Some(Instant::now());
        }
        self.shape = opts.shape;
        self.shadow = opts.shadow;
        if cheap {
            // Only the shape or the shadow moved: the backdrop under the window is the one the region holds, so one draw with another
            // constant is all it takes (no copy of the screen, no pyramid, no read back). This is what an animation of the shape costs.
            self.dirty = true;
        } else {
            self.restale();
        }
        relook
    }

    /// The glass's shape changed a moment ago: it is being animated (a dock island that widens under the pointer, or that goes thin
    /// or grows back because a window was maximised or closed).
    fn shape_moving(&self, now: Instant) -> bool {
        self.shape_changed.is_some_and(|t| now.saturating_duration_since(t) < SHAPE_ACTIVE)
    }

    /// Whether a draw that is waiting may take its turn now. `rest::draw_due`, and a window whose shape is moving always may: every
    /// shape it is sent is a frame of an animation the UI draws at the display's rate, and the glass's edge has to be where the UI's
    /// content is.
    ///
    /// WHY THE SHAPE COUNTS (measured 2026-11-14 on the islands of a dock that grew back after a window closed, pictures of the screen
    /// every 13 ms). A window that is not in front and has a new backdrop waiting is held to `BEHIND_FPS`, and its new shape waited with
    /// it: the three islands, each with its own last draw, moved in steps of 30 to 70 ms and not together (the side islands began 60 ms
    /// after the middle one), while the icons on them moved at the display's rate. The backdrop under an island changes whenever a
    /// window near it is opened, closed or maximised, which is exactly when the islands change their shape. The cost is a full draw at
    /// the display's rate for the length of the animation (about a quarter of a second), as a window that moves has it
    /// (`geometry_changed`).
    fn draw_due(&self, now: Instant, gap: Duration) -> bool {
        self.shape_moving(now) || rest::draw_due(self.care, self.stale, self.last_draw.map(|t| now.saturating_duration_since(t)), gap)
    }

    /// Something behind the glass or about the glass itself changed: draw again, from a new backdrop.
    fn restale(&mut self) {
        self.dirty = true;
        self.stale = true;
    }
}

/// The corner radius a window's glass is drawn with, device px: the one the app asked for, else the one the system cuts the window
/// with (`system`, as measured). An asked-for radius is kept inside what the window can hold (no more than half of its shorter side:
/// a 48 px island with 24 px is a capsule, more would only make the shader's corner circle run past the edges) and ignores the
/// "maximised or covering its monitor" rule, which is the system's own rounding going away, not the app's shape.
pub(super) fn effective_radius(asked: Option<f32>, system: f32, w: u32, h: u32) -> f32 {
    match asked {
        Some(r) if r.is_finite() => r.clamp(0.0, w.min(h) as f32 / 2.0),
        _ => system,
    }
}

/// A shape with no area (the dock starts every island with an empty one until it has its first real shape).
fn shape_is_empty(shape: Option<[f32; 4]>) -> bool {
    shape.is_some_and(|[_, _, w, h]| w <= 0.0 || h <= 0.0)
}

/// What the glass of a window shows on the screen: its client area, or the shape inside it, rounded OUT to whole pixels (the glass is
/// not hidden unless the pixels around it are). `None` when there is nothing to show: a window or shape with no area has nothing that
/// could be hidden, so it is never "covered" (the empty target of `geom::covered_by` is trivially covered, which would hold back an
/// island that is only waiting for its first shape).
fn glass_on_screen(geo: &WinGeo, shape: Option<[f32; 4]>) -> Option<Rc> {
    if geo.w == 0 || geo.h == 0 || shape_is_empty(shape) {
        return None;
    }
    let [x, y, gw, gh] = Geometry { w: geo.w, h: geo.h, origin: (0, 0), scale: 1.0, radius_px: 0.0, cut_corners: false, shape, shadow: None }.glass_rect();
    let r = Rc::new(geo.x + x.floor() as i32, geo.y + y.floor() as i32, geo.x + (x + gw).ceil() as i32, geo.y + (y + gh).ceil() as i32);
    (!r.is_empty()).then_some(r)
}

/// How a draw ended.
enum Drawn {
    Presented,
    /// The loop guard: nothing behind the window changed.
    Skipped,
}

unsafe fn draw(gfx: &Gfx, mon: &Monitor, w: &mut Win) -> Result<Drawn, Fail> {
    let geo = w.geo.ok_or_else(|| Fail::Window("drawn before it was measured".into()))?;
    let mirror = mon.mirror().ok_or_else(|| Fail::Window("no mirror of the monitor".into()))?;

    // the swap chain: made on the first draw, resized when the client area changed
    let first_draw = w.target.is_none();
    match &mut w.target {
        None => {
            w.target = Some(Target::new(gfx, w.hwnd, geo.w, geo.h)?);
        }
        Some(t) if (t.w, t.h) != (geo.w, geo.h) => {
            t.resize(gfx, geo.w, geo.h)?;
            w.presented = None; // the new buffers are empty
        }
        Some(_) => {}
    }
    // the backdrop pyramid: allocated generously, so a live resize does not make a new one at every step
    let pad = region_pad(geo.scale());
    if !w.region.as_ref().is_some_and(|r| r.fits(geo.w, geo.h, pad)) {
        w.region = Some(Region::new(&gfx.r.g, geo.w, geo.h, pad).map_err(|e| ctx("creating the backdrop texture", e))?);
        w.stale = true; // the new texture is empty
    }
    let (Some(region), Some(target)) = (w.region.as_ref(), w.target.as_ref()) else { return Err(Fail::Window("lost its resources".into())) };

    let mrect = mon.rect();
    let mut g = Geometry {
        w: geo.w,
        h: geo.h,
        origin: (geo.x - mrect.left, geo.y - mrect.top),
        scale: geo.scale(),
        radius_px: 0.0,
        cut_corners: w.cut_corners,
        shape: w.shape,
        shadow: w.shadow,
    };
    // the corner radius is the glass's own: a shape's is kept inside the shape, not inside the window that holds it
    let [_, _, glass_w, glass_h] = g.glass_rect();
    g.radius_px = effective_radius(w.radius_px, geo.radius_px(), glass_w as u32, glass_h as u32);
    // The settings' look; a shape below 160 CSS px (by the GLASS's short side, not the window's) gets its rim bounded by the shape
    // (a window never does, a dock island does). The look is part of what the loop guard compares, so a change of the settings (or of
    // the theme) is drawn.
    let look = Look::regular(w.dark, &super::look()).sized(glass_w.min(glass_h) / geo.scale()).rim_capped(super::rim_cap(w.hwnd));
    let lenses = w.lenses.draw_list();
    let t0 = Instant::now();
    let (mut prepared, hash) = match w.presented.map(|p| p.hash) {
        // only a lens moved: the region still holds the backdrop of the last draw, and its fingerprint is that draw's
        Some(hash) if !w.stale => {
            w.stats.reused += 1;
            (reuse(region, mirror, &g, &look), hash)
        }
        _ => {
            let prepared = prep(&gfx.r, region, mirror, &g, &look, false);
            // THE LOOP GUARD. Our own Present can make the compositor produce a new duplication frame whose picture (without our excluded
            // window) is unchanged; drawing for it would keep the engine busy at the refresh rate for ever. So a coarse level of the backdrop
            // is read back, and when it, the geometry, the look and the lenses all equal the last presented state, nothing is drawn.
            let read = region.read_back(&gfx.r.g, g.glass_rect()).map_err(|e| ctx("reading the backdrop back", e))?;
            // the same bytes tell how bright the backdrop under the glass (the SHAPE, when there is one) is (`backdrop_luma`); no extra
            // pass, copy or wait
            if let Some(luma) = read.luma {
                super::set_luma(w.hwnd, luma);
            }
            w.stale = false;
            (prepared, read.hash)
        }
    };
    prepared.set_lenses(&look, &lenses);
    let t1 = Instant::now();
    w.stats.prep += t1 - t0;
    let now = Presented { hash, geo: g, look, lenses };
    if w.presented == Some(now) {
        gfx.r.unbind();
        w.stats.skipped += 1;
        return Ok(Drawn::Skipped);
    }
    let back: ID3D11Texture2D = target.sc.GetBuffer(0).map_err(|e| ctx("swap chain buffer", e))?;
    let mut rtv = None;
    gfx.r.g.dev.CreateRenderTargetView(&back, None, Some(&mut rtv)).map_err(|e| ctx("render target view", e))?;
    let Some(rtv) = rtv else { return Err(Fail::Window("no render target view".into())) };
    glass_pass(&gfx.r, region, &prepared, &g, &rtv);
    target.sc.Present(0, DXGI_PRESENT(0)).ok().map_err(|e| ctx("Present", e))?;
    w.stats.glass += t1.elapsed();
    w.stats.presented += 1;
    w.presented = Some(now);
    w.had_picture = true;
    if first_draw {
        // Running means "the first frame is presented", not "the swap chain exists": a window that drops its own fill on Running (a popup
        // menu, the dock's island, the main window turning see-through) would otherwise be empty for the time the first frame takes.
        super::set_state(w.hwnd, GlassState::Running);
    }
    Ok(Drawn::Presented)
}

/// The capture sessions that are kept but not used (no glass on their monitor is seen), and since when.
#[derive(Default)]
struct Parking {
    since: BTreeMap<isize, Instant>,
}

/// What a look at the sessions changed.
#[derive(Debug, Default, PartialEq, Eq)]
struct ParkChange {
    /// Sessions that were parked and are wanted again: their mirror was not kept up to date meanwhile.
    resumed: Vec<isize>,
    /// Sessions that stayed unused for the whole grace time: to be closed.
    expired: Vec<isize>,
}

impl Parking {
    /// Whether the session of a monitor is kept but not used: its mirror and queue are from before the pause until the next
    /// `Engine::sync_monitors` has resumed it.
    fn is_parked(&self, hmon: isize) -> bool {
        self.since.contains_key(&hmon)
    }

    /// `open`: the monitors that have a session; `want`: those with a glass that is seen.
    fn update(&mut self, open: &[isize], want: &BTreeSet<isize>, now: Instant, grace: Duration) -> ParkChange {
        self.since.retain(|k, _| open.contains(k));
        let mut change = ParkChange::default();
        for k in open {
            if want.contains(k) {
                if self.since.remove(k).is_some() {
                    change.resumed.push(*k);
                }
            } else {
                self.since.entry(*k).or_insert(now);
            }
        }
        change.expired = self.since.iter().filter(|(_, t)| now.duration_since(**t) >= grace).map(|(k, _)| *k).collect();
        for k in &change.expired {
            self.since.remove(k);
        }
        change
    }
}

/// What to do with a capture session that could not be opened again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Retry {
    /// Try again after the pause (until then the windows keep their last picture).
    Later,
    /// Enough failures in a row: the failure is a real one.
    GiveUp,
}

/// Failed attempts to open the session of a monitor again, after sessions had worked in this run. A closed session is reopened every
/// time glass comes into view after more than `SESSION_GRACE`, so one transient failure (a display waking up, a mode change, a GPU
/// reset, the secure desktop) must not turn the whole process over to Desktop Duplication, hide its windows from screenshots and keep
/// it so for the rest of the run. It is tried again after 1, 2 and 5 seconds; the fourth failure in a row counts.
#[derive(Default)]
struct Reopen {
    /// Per monitor: failures in a row, and when the next attempt may be made.
    failed: BTreeMap<isize, (usize, Instant)>,
}

impl Reopen {
    /// Whether an attempt may be made now.
    fn due(&self, hmon: isize, now: Instant) -> bool {
        self.failed.get(&hmon).is_none_or(|(_, at)| now >= *at)
    }

    fn failed(&mut self, hmon: isize, now: Instant) -> Retry {
        let n = self.failed.get(&hmon).map_or(0, |(n, _)| *n);
        match REOPEN_PAUSES.get(n) {
            Some(pause) => {
                self.failed.insert(hmon, (n + 1, now + *pause));
                Retry::Later
            }
            None => {
                self.failed.remove(&hmon);
                Retry::GiveUp
            }
        }
    }

    fn opened(&mut self, hmon: isize) {
        self.failed.remove(&hmon);
    }
}

struct Engine {
    rx: Receiver<Cmd>,
    gfx: Option<Gfx>,
    wins: BTreeMap<isize, Win>,
    mons: BTreeMap<isize, Monitor>,
    stats_on: bool,
    last_device_check: Instant,
    /// The last time anything happened that the glass has to follow.
    last_activity: Instant,
    /// How the windows are kept out of the capture. Chosen when the device is made (the first visible window), `Off` until then and again
    /// when the engine goes idle.
    mode: CaptureMode,
    /// The windows a capture session leaves out.
    excl: Exclusion,
    /// The glass windows of other processes of ours that this process leaves out too (`crossproc.rs`), and when they were last looked for.
    foreign: Foreign,
    last_scan: Instant,
    /// A glass window of another process of ours was seen at the last scan (above or below ours): the turn then stays as short as the scan.
    foreign_seen: bool,
    /// Why the thread woke (statistics).
    turns: Turns,
    last_access_check: Instant,
    /// A permission check running on its own thread (see `access_tick`).
    access_job: Option<Receiver<Access>>,
    /// How many `DwmFlush` calls in a row returned without waiting (see `pace`).
    instant_flushes: u32,
    /// Where the power comes from, as last read, and when (the capture rate follows it).
    power: Power,
    last_power_check: Instant,
    /// Monitors whose capture session is kept but not used (no glass on them is seen). A session that stays here for `SESSION_GRACE` is
    /// closed.
    parked: Parking,
    /// The foreground window at the last look at the z order, and when that look was made. `cover_due`: look at the next turn.
    last_foreground: isize,
    last_cover_check: Instant,
    cover_due: bool,
    /// What the window manager last said about each window (`DwmFacts`, asked at most every `DWM_ASK_EVERY`).
    dwm: BTreeMap<isize, DwmFacts>,
    /// The foreground window's rectangle at the last look at the z order, while some glass was covered: a covering window that is
    /// dragged or resized away changes no foreground window, but it does change this.
    last_fg_rect: Option<Rc>,
    /// Capture sessions that cannot be opened again are tried again after a pause (see `Reopen`).
    reopen: Reopen,
    /// A capture session was opened in this run, so the backend works here and a failure to open one again is probably a transient one.
    session_worked: bool,
    /// How long Desktop Duplication still leaves its frames alone (`rest.rs`), as of the last `pump`: the turn waits that long.
    throttle: Option<Duration>,
    /// Set while Desktop Duplication is in use because a session could not be had (not because the system said no): the engine tries a
    /// session again on this schedule (`retry.rs`) and goes back to sessions, and so back to visible windows, when one opens.
    session_retry: Option<SessionRetry>,
}

/// The multithreaded WinRT apartment of the engine thread: entered before any WinRT object is touched, left when the thread ends (also
/// when it ends by a panic).
pub(super) struct Apartment(bool);

impl Apartment {
    pub(super) fn enter() -> Apartment {
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Apartment(true),
            Err(e) => {
                // another apartment kind is already set on this thread: it is not ours to leave, and WinRT may still work in it
                log::warn!("glass: the WinRT apartment could not be entered: {e}");
                Apartment(false)
            }
        }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { RoUninitialize() };
        }
    }
}

/// The engine thread's body. Returns when the command channel is closed.
pub(super) fn run(rx: Receiver<Cmd>) {
    let _apartment = Apartment::enter();
    unsafe {
        // window and monitor rectangles must come back in device pixels whatever the process was started as
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let stats_on = std::env::var("RUSTYFILES_GLASS_STATS").is_ok_and(|v| v == "1");
    let mut e = Engine::new(rx);
    e.stats_on = stats_on;
    while e.step() {}
    e.drop_all();
}

impl Engine {
    fn new(rx: Receiver<Cmd>) -> Engine {
        Engine {
            rx,
            gfx: None,
            wins: BTreeMap::new(),
            mons: BTreeMap::new(),
            stats_on: false,
            last_device_check: Instant::now(),
            last_activity: Instant::now(),
            mode: CaptureMode::Off,
            excl: Exclusion::default(),
            foreign: Foreign::default(),
            last_scan: Instant::now(),
            foreign_seen: false,
            turns: Turns::default(),
            last_access_check: Instant::now(),
            access_job: None,
            instant_flushes: 0,
            power: wgc::read_power(),
            last_power_check: Instant::now(),
            parked: Parking::default(),
            last_foreground: 0,
            last_cover_check: Instant::now(),
            cover_due: true,
            dwm: BTreeMap::new(),
            last_fg_rect: None,
            reopen: Reopen::default(),
            session_worked: false,
            throttle: None,
            session_retry: None,
        }
    }

    fn command(&mut self, c: Cmd) {
        self.last_activity = Instant::now();
        *self.turns.commands.entry(c.name()).or_default() += 1;
        // a window that something was just said about is asked about afresh (its corners or its place may be what changed)
        if let Cmd::Attach { hwnd, .. } | Cmd::SetOptions { hwnd, .. } | Cmd::Detach { hwnd } = &c {
            self.dwm.remove(hwnd);
        }
        match c {
            Cmd::Attach { hwnd, opts } => {
                // the foreground window is watched by an event from the first glass on (see `watch.rs`)
                watch::start();
                self.cover_due = true;
                self.excl.attach(hwnd);
                if self.mode == CaptureMode::Affinity {
                    // The caller set the flag already; set again here because a Detach of an earlier attach of the same window may have
                    // been handled since (it lifts the flag) and this command comes after it in the queue.
                    let _ = super::set_affinity(hwnd, true);
                }
                let w = self.wins.entry(hwnd).or_insert_with(|| Win::new(hwnd, opts));
                w.set_options(opts);
            }
            Cmd::SetOptions { hwnd, opts } => {
                if let Some(w) = self.wins.get_mut(&hwnd)
                    && w.set_options(opts)
                {
                    self.cover_due = true;
                }
            }
            Cmd::SetDark { hwnd, dark } => {
                if let Some(w) = self.wins.get_mut(&hwnd) {
                    w.dark = dark;
                    w.restale();
                }
            }
            Cmd::LookChanged => {
                // Every window draws again, once, with the look that `draw` reads from the process-wide value.
                for w in self.wins.values_mut() {
                    w.restale();
                }
            }
            Cmd::SetLens { hwnd, slot, lens, animate } => {
                // a window the engine does not know (detached since, or failed) has nothing to put a lens on
                if let Some(w) = self.wins.get_mut(&hwnd) {
                    w.lenses.set(slot as usize, lens, animate);
                    w.dirty = true;
                }
            }
            Cmd::Detach { hwnd } => {
                self.excl.detach(hwnd);
                if let Some(w) = self.wins.remove(&hwnd) {
                    self.teardown(w);
                    // only now may the window be captured again: its target is off it
                    super::clear_affinity(hwnd);
                }
            }
            Cmd::ExtrasChanged => self.excl.set_extras(super::extras_snapshot()),
            Cmd::Freezable { hwnd, on } => {
                if let Some(w) = self.wins.get_mut(&hwnd) {
                    w.freezable = on;
                }
            }
            // (the next turn looks at what every window is owed; the command itself ended the wait)
            Cmd::CareChanged => {}
        }
    }

    /// How long until a window that is held back (it is not in front and drew a moment ago) may draw: the turn must not wait longer.
    fn draw_wait(&self) -> Option<Duration> {
        let now = Instant::now();
        let gap = rest::gap_for(rest::behind_fps());
        self.wins
            .values()
            // (a window whose shape is moving is not held back: `Win::draw_due`)
            .filter(|w| w.active() && w.dirty && !w.shape_moving(now))
            .filter_map(|w| rest::draw_wait(w.care, w.stale, w.last_draw.map(|t| now.saturating_duration_since(t)), gap))
            .min()
    }

    /// How long this turn may wait for a frame.
    fn frame_wait_ms(&self) -> u32 {
        let ms = self.frame_wait_unheld_ms();
        match self.draw_wait() {
            Some(d) => ms.min(d.as_millis().max(1) as u32),
            None => ms,
        }
    }

    fn frame_wait_unheld_ms(&self) -> u32 {
        if self.paced() {
            0 // the display paces the turn (`pace`); a frame that is not there yet is taken next turn
        } else if self.shape_moving() {
            SHAPE_WAIT_MS
        } else if self.last_activity.elapsed() > QUIET_AFTER {
            let ms = quiet_wait_ms(self.mode, self.wins.values().any(|w| w.active() && w.lenses.occupied()));
            // Seen glass but none in front: nothing it would notice in time needs a shorter wait (`rest.rs`). Only for a capture session,
            // whose wait also ends for a command; Desktop Duplication's cannot be interrupted, so a longer wait would delay a lens.
            rest::quiet_wait_ms(ms, self.mode == CaptureMode::Session, self.none_in_front(), self.foreign_seen)
        } else if self.mode == CaptureMode::Session && self.none_in_front() {
            // NOT QUIET, BUT NOTHING IN FRONT. The short wait exists to notice a glass window that MOVES (nothing says so: its place is
            // read every turn), and a window that is neither in front nor under the pointer is not being dragged. Everything else it
            // could be owed ends a session's wait by itself: a frame does, a command does (a shape that changes, a lens, a window that
            // comes to the front or under the pointer), and a draw that is held back has its own deadline (`draw_wait`). Without this
            // a video beside the dock kept the thread at 125 turns a second for as long as it played: the picture behind the dock's
            // glass changed, so the second of quiet never came (measured 2026-11-14: 125 turns and 9 400 context switches a second,
            // 6.5 % of a core, with 12 frames a second actually taken).
            QUIET_WAIT_MS
        } else {
            FRAME_WAIT_MS
        }
    }

    /// Some glass is worked for and none of it is in front or under the pointer.
    fn none_in_front(&self) -> bool {
        let mut any = false;
        for w in self.wins.values().filter(|w| w.active()) {
            if w.care == Care::Front {
                return false;
            }
            any = true;
        }
        any
    }

    /// Looks at what every glass window that is seen is owed (`rest.rs`): is it in front (its root owner is the foreground window's),
    /// is the pointer over it, has it been either a moment ago. A window that is to freeze stops being worked for; one that thaws is
    /// drawn at once from a fresh backdrop. One `GetForegroundWindow`, one `GetCursorPos` + `WindowFromPoint` and one `GetAncestor` per
    /// window, a turn. A turn comes when the foreground window changes (`watch.rs`) or the pointer comes over a window
    /// (`glass::pointer_over`) because both send a command, so a long wait does not delay either; the app's windows that are not in front
    /// are published (`glass::set_not_front`) so that the pointer event costs nothing for a window that is in front.
    fn update_care(&mut self) {
        if !self.wins.values().any(|w| w.visible && !w.covered) {
            return;
        }
        let now = Instant::now();
        let fg = cover::foreground();
        let fg_root = if fg == 0 { 0 } else { root_owner(fg) };
        let under_pointer = root_at_pointer();
        let freeze_all = super::freeze_when_behind();
        let rules = rest::rules_on();
        let mut thawed = false;
        for w in self.wins.values_mut().filter(|w| w.visible && !w.covered) {
            let (facts, last) = rest::facts_of(root_owner(w.hwnd), fg_root, under_pointer, w.last_front, now, freeze_all && w.freezable, w.had_picture);
            w.last_front = last;
            w.care = rest::care_under(rules, &facts);
            thawed |= w.set_frozen(w.care == Care::Frozen);
        }
        if thawed {
            self.last_activity = now;
        }
        super::set_not_front(self.wins.values().filter(|w| w.visible && w.care != Care::Front).map(|w| w.hwnd).collect());
    }

    /// Tells every capture how fast its monitor's glass wants frames: the normal rate while some glass on it is in front, a slower one
    /// while none is (`rest::pace`). A running session takes the new interval at once.
    fn sync_pace(&mut self) {
        let power = self.power;
        for (hmon, m) in self.mons.iter_mut() {
            let pace = rest::pace(self.wins.values().filter(|w| w.active() && w.geo.is_some_and(|g| g.hmon == *hmon)).map(|w| w.care));
            if unsafe { m.set_pace(pace, power) } {
                log::debug!("glass: monitor {hmon:#x} is paced {pace:?}");
            }
        }
    }

    /// Gives back what a window that nobody sees does not need: after `rest::TRIM_AFTER` of not being worked for its backdrop pyramid
    /// goes (the next draw makes it again, from a fresh capture, before anything is shown), and when no glass is worked for at all the
    /// driver is told to let go of what it caches for the device (`IDXGIDevice3::Trim`, what Windows asks of an app that goes to the
    /// background). The swap chain stays: it holds the picture the window shows.
    fn trim_idle(&mut self, now: Instant) {
        let mut dropped = false;
        for w in self.wins.values_mut() {
            if w.active() {
                w.idle_since = None;
                continue;
            }
            let since = *w.idle_since.get_or_insert(now);
            if w.region.is_some() && rest::trim_due(now.saturating_duration_since(since)) {
                w.region = None;
                w.stale = true;
                dropped = true;
            }
        }
        if dropped
            && !self.wins.values().any(|w| w.active())
            && let Some(gfx) = &self.gfx
        {
            unsafe { gfx.trim() };
        }
    }

    /// The windows that are shown and not hidden under others.
    fn active_set(&self) -> BTreeSet<isize> {
        self.wins.values().filter(|w| w.active()).map(|w| w.hwnd).collect()
    }

    /// Whether a lens of a visible window is moving or fading: the engine then draws once per display refresh.
    fn lens_active(&self) -> bool {
        self.wins.values().any(|w| w.active() && w.lenses.animating())
    }

    /// Whether a visible window's glass shape changed a moment ago (an island widening under the pointer sends a new shape for every
    /// frame of the animation, and each must be drawn about when the UI draws its scene: soon after it arrives, not at the next refresh).
    fn shape_moving(&self) -> bool {
        let now = Instant::now();
        self.wins.values().any(|w| w.active() && w.shape_moving(now))
    }

    /// Whether the display paces the loop: a lens moves or fades. (Not a shape: see the module docs.)
    fn paced(&self) -> bool {
        self.lens_active()
    }

    /// Steps the lenses of the visible windows to now and marks them dirty. The step is the time since the last one, so the motion does not
    /// depend on how often the loop turns; `LensSet::advance` cuts a step after a stall.
    fn advance_lenses(&mut self) {
        let now = Instant::now();
        for w in self.wins.values_mut() {
            if !w.active() || !w.lenses.animating() {
                w.lens_tick = None;
                continue;
            }
            let dt = w.lens_tick.map_or(0.0, |t| now.duration_since(t).as_secs_f32());
            w.lens_tick = Some(now);
            w.lenses.advance(dt);
            if !w.lenses.animating() {
                // At rest: the next motion starts with a zero step, not with the time that passed since this one's last step.
                w.lens_tick = None;
            }
            w.dirty = true; // also the step that puts the lens exactly on its target: that one is drawn, the next turn is not
        }
    }

    /// Ends a turn that drew a moving lens: waits for the display's next composition (DwmFlush returns once per refresh, so a 60 Hz
    /// screen is drawn 60 times a second and a 240 Hz one 240 times, with no spinning). Should the call fail or return at once, a short
    /// wait on the channel stands in, so this can never become a busy loop.
    fn pace(&mut self) -> bool {
        let t = Instant::now();
        let ok = unsafe { DwmFlush() }.is_ok();
        // The stopwatch starts after the draw, so a healthy DwmFlush only waits for the next composition: on a 144 or 240 Hz screen that
        // is often well under 2 ms. One quick return therefore proves nothing; only a run of returns that took no time at all (the
        // compositor is off, or the call does not wait any more) or an error switches to the channel wait.
        if ok && t.elapsed() >= INSTANT_FLUSH {
            self.instant_flushes = 0;
        } else if ok {
            self.instant_flushes += 1;
        }
        if !ok || self.instant_flushes >= INSTANT_FLUSHES_BEFORE_WAIT {
            return self.wait_command(Duration::from_millis(4));
        }
        true
    }

    /// Waits for a command for at most `t`. False when the channel is closed.
    fn wait_command(&mut self, t: Duration) -> bool {
        match self.rx.recv_timeout(t) {
            Ok(c) => self.command(c),
            Err(RecvTimeoutError::Timeout) => self.turns.timeouts += 1,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
        true
    }

    /// One turn of the loop. False ends the thread.
    fn step(&mut self) -> bool {
        self.turns.turns += 1;
        if self.wins.is_empty() {
            // Nothing attached: no device, no capture, and nothing to do until a command arrives.
            self.release_gfx();
            match self.rx.recv() {
                Ok(c) => self.command(c),
                Err(_) => return false,
            }
        } else if !self.wins.values().any(|w| w.active()) {
            // Nobody can see any glass. A window that is only covered is looked at often (it may come into view any moment); one that
            // is hidden is waited for as before.
            let wait = idle_wait(self.wins.values().any(|w| w.visible && w.covered), self.wins.values().any(|w| w.visible && w.frozen), watch::running());
            if !self.wait_command(wait) {
                return false;
            }
        }
        while let Ok(c) = self.rx.try_recv() {
            self.command(c);
        }
        if self.wins.is_empty() {
            return true;
        }
        // The look after the wait of the last turn is still good when every window is visible and measured; only windows that are not
        // yet known or are hidden (they may have come back) need a look now.
        let mut lap = Lap::start(self.stats_on);
        if self.wins.values().any(|w| w.geo.is_none() || !w.visible) {
            self.measure_all();
        }
        self.update_cover();
        self.update_care();
        lap.mark(&mut self.turns, 0);
        self.sync_monitors();
        self.sync_pace();
        lap.mark(&mut self.turns, 1);
        let waited = self.pump();
        lap.mark(&mut self.turns, 2);
        // The window may have moved while we waited for the frame. A covered window that moved is in view (its owner moved it) and so
        // active now, while its monitor's capture may still be parked: it is resumed before anything is drawn from it, and the turn does
        // not wait (the new frame is asked for by the resume).
        let seen_before = self.active_set();
        self.measure_all();
        let appeared = !self.active_set().is_subset(&seen_before);
        lap.mark(&mut self.turns, 0);
        if appeared {
            self.sync_monitors();
            self.sync_pace();
        }
        lap.mark(&mut self.turns, 1);
        self.advance_lenses();
        self.draw_dirty();
        lap.mark(&mut self.turns, 3);
        self.housekeeping();
        lap.mark(&mut self.turns, 4);
        if self.paced() {
            return self.pace();
        }
        if !waited && !appeared && self.wins.values().any(|w| w.active()) {
            // no AcquireNextFrame wait happened this turn (a duplication is being rebuilt, or failed and backs off, or leaves its frames
            // alone for a while: then exactly that long): never spin
            let wait = self.throttle.take().map_or_else(|| Duration::from_millis(self.frame_wait_ms() as u64), |left| left.max(Duration::from_millis(1)));
            return self.wait_command(wait);
        }
        true
    }

    /// Looks at every window: gone ones are dropped, the others get their geometry and are marked dirty when it changed.
    fn measure_all(&mut self) {
        let mut moved = false;
        let keys: Vec<isize> = self.wins.keys().copied().collect();
        for k in keys {
            match unsafe { measure(k, &mut self.dwm, Instant::now()) } {
                Seen::Gone => {
                    // The app destroyed it without a detach. Nothing to restore on the window; the state goes with it.
                    self.wins.remove(&k);
                    self.excl.detach(k);
                    super::forget(k);
                }
                Seen::Hidden => {
                    self.excl.set_visible(k, false);
                    if let Some(w) = self.wins.get_mut(&k) {
                        moved |= w.visible;
                        w.visible = false;
                        w.covered = false;
                        w.frozen = false;
                    }
                }
                Seen::Shown(g) => {
                    // a window listed while it was hidden may not be honoured once it shows: the list is sent again
                    self.excl.set_visible(k, true);
                    if let Some(w) = self.wins.get_mut(&k) {
                        if !w.visible {
                            w.visible = true;
                            w.covered = false;
                            w.restale();
                            w.presented = None; // it may have been off screen a long while
                            self.cover_due = true;
                            moved = true;
                        }
                        if w.geo != Some(g) {
                            w.geometry_changed(g, Instant::now());
                            moved = true;
                        }
                    }
                }
            }
        }
        if moved {
            self.last_activity = Instant::now();
        }
    }

    /// How often the z order is walked at least: fast while some glass is covered (it may come into view at any moment, and nothing but
    /// a look tells), slowly otherwise (a window that goes under others is only a saving missed for a moment). The dock's islands are
    /// always in view, so "some glass is uncovered" must not set the pace: the main window under a browser would be noticed 500 ms late.
    fn cover_interval(&self) -> Duration {
        if self.wins.values().any(|w| w.visible && w.covered) { COVER_POLL } else { COVER_EVERY }
    }

    /// Looks at the z order and marks the glass windows that other windows hide completely (and unmarks those that are in view again).
    /// Done when a window appeared or its shape moved, when the foreground window changed or moved (the window on top is dragged, resized,
    /// snapped, closed: one `GetWindowRect` a turn, and only while some glass is covered), and otherwise at `cover_interval`, never per
    /// frame: one pass over the top-level windows is about a tenth of a millisecond.
    fn update_cover(&mut self) {
        let any_visible = self.wins.values().any(|w| w.visible);
        if !any_visible {
            self.last_foreground = 0;
            self.last_fg_rect = None;
            return;
        }
        let fg = cover::foreground();
        let fg_rect = if self.wins.values().any(|w| w.visible && w.covered) { cover::window_rect(fg) } else { None };
        let fg_moved = fg_rect != self.last_fg_rect;
        if !(self.cover_due || fg != self.last_foreground || fg_moved || self.last_cover_check.elapsed() >= self.cover_interval()) {
            return;
        }
        self.cover_due = false;
        self.last_foreground = fg;
        self.last_fg_rect = fg_rect;
        self.last_cover_check = Instant::now();
        // what each visible glass shows, on the screen
        let mine: Vec<(isize, Rc)> = self.wins.values().filter(|w| w.visible).filter_map(|w| Some((w.hwnd, glass_on_screen(&w.geo?, w.shape)?))).collect();
        let answer = cover::covered_windows(&mine);
        self.apply_cover(&answer, Instant::now());
    }

    /// Takes the verdict of a look at the z order: every shown window is covered when the answer says so (a window the walk did not meet
    /// is not, and neither is one with nothing to show). Changes nothing a window reports: see the module docs.
    fn apply_cover(&mut self, answer: &BTreeMap<isize, bool>, now: Instant) {
        let mut uncovered = false;
        for w in self.wins.values_mut().filter(|w| w.visible) {
            let Some(geo) = w.geo else { continue };
            let covered = glass_on_screen(&geo, w.shape).is_some() && answer.get(&w.hwnd).copied().unwrap_or(false);
            let was = w.covered;
            if w.set_covered(covered) {
                log::debug!("glass: window {:#x} is in view again", w.hwnd);
                uncovered = true;
            } else if covered && !was {
                log::debug!("glass: window {:#x} is covered by other windows: no work is done for it", w.hwnd);
            }
        }
        if uncovered {
            self.last_activity = now;
        }
    }

    /// Desktop Duplication is in use only because a session could not be had: on the schedule of `retry.rs`, looks at the permission and
    /// tries a session on a monitor that has glass in view. When one opens the engine goes back to sessions for all its windows.
    fn try_session_again(&mut self, now: Instant) {
        if !self.session_retry.as_ref().is_some_and(|r| r.due(now)) {
            return;
        }
        // nothing in view, nothing to gain (and nothing to count): the look waits for glass to be seen
        let Some(hmon) = self.wins.values().filter(|w| w.active()).find_map(|w| w.geo.map(|g| g.hmon)) else { return };
        let power = self.power;
        let tried = match wgc::access() {
            Access::Allowed => self.gfx.as_ref().map(|gfx| unsafe { Monitor::open_session(&gfx.r.g, hmon, power) }),
            other => Some(Err(other.why_not().unwrap_or("not allowed").to_string())),
        };
        match tried {
            Some(Ok(m)) => {
                log::info!("glass: a capture session could be made again; going back to it, rustyfiles windows are visible to screenshots and recordings again");
                self.switch_to_session(hmon, m);
            }
            Some(Err(why)) => {
                if let Some(r) = self.session_retry.as_mut() {
                    r.failed(now);
                    log::info!("glass: the capture session still cannot be made ({why}); try {} of the schedule failed", r.failures());
                }
            }
            None => {}
        }
    }

    /// From Desktop Duplication to sessions: `first` is the session just opened for `hmon`. The windows' flag goes, the other monitors
    /// open their sessions on the next turn, every window draws from a new backdrop.
    fn switch_to_session(&mut self, hmon: isize, first: Monitor) {
        self.mons.clear();
        self.mons.insert(hmon, first);
        self.session_worked = true;
        self.reopen = Reopen::default();
        self.session_retry = None;
        self.mode = CaptureMode::Session;
        set_mode(CaptureMode::Session);
        self.excl.set_extras(super::extras_snapshot());
        let mut all: Vec<isize> = self.wins.keys().copied().collect();
        all.extend(self.excl.extras().iter().copied());
        for h in all {
            super::clear_affinity(h);
        }
        for w in self.wins.values_mut() {
            w.restale();
        }
    }

    /// One capture per monitor that has an ACTIVE window (shown and not covered); none for the others. A capture whose glass is no
    /// longer seen is kept for `SESSION_GRACE` (and not used), then closed, and opened again when a glass needs it. The first one made
    /// also decides the backend.
    fn sync_monitors(&mut self) {
        let want: BTreeSet<isize> = self.wins.values().filter(|w| w.active()).filter_map(|w| w.geo.map(|g| g.hmon)).collect();
        let open: Vec<isize> = self.mons.keys().copied().collect();
        let change = self.parked.update(&open, &want, Instant::now(), SESSION_GRACE);
        for k in change.resumed {
            // back in use after a pause: it was not pumped meanwhile, so its mirror and the frames queued in it are old; a new picture
            // is asked for and nothing is drawn until it is in
            if let Some(m) = self.mons.get_mut(&k) {
                m.resume();
            }
        }
        for k in change.expired {
            log::debug!("glass: no glass on monitor {k:#x} is seen: its capture is closed");
            self.mons.remove(&k);
        }
        let now = Instant::now();
        for hmon in want {
            if self.mons.contains_key(&hmon) || !self.reopen.due(hmon, now) {
                continue;
            }
            if self.gfx.is_none() {
                match unsafe { Gfx::new(hmon) } {
                    Ok(g) => self.gfx = Some(g),
                    Err(why) => {
                        self.fail_on_monitor(hmon, &why);
                        continue;
                    }
                }
            }
            if self.mode == CaptureMode::Off {
                self.choose_backend(hmon);
                if self.mons.contains_key(&hmon) {
                    continue;
                }
            }
            self.open_monitor(hmon);
        }
    }

    /// Picks the backend, once per start of the engine: a capture session when the border-free capture is allowed and a session on this
    /// first monitor can be made and offers the exclusion list (that session is then the monitor's capture), Desktop Duplication with
    /// the display-affinity exclusion otherwise.
    fn choose_backend(&mut self, hmon: isize) {
        let Some(gfx) = self.gfx.as_ref() else { return };
        let power = self.power;
        let access = wgc::access();
        // a system that says no is a decision, a session that fails with the permission there is a failure that may pass
        let failure = matches!(access, Access::Allowed);
        match wgc::pick_backend(access, || unsafe { Monitor::open_session(&gfx.r.g, hmon, power) }) {
            Ok(m) => {
                self.mons.insert(hmon, m);
                self.session_worked = true;
                self.mode = CaptureMode::Session;
                set_mode(CaptureMode::Session);
                // The registry is read AFTER the mode is published: a popup that registered before this read is in the snapshot, one that
                // registers later sees `Session` and sends ExtrasChanged. Read before, one in between would be in neither.
                self.excl.set_extras(super::extras_snapshot());
                log::info!("glass: screen capture through a Windows.Graphics.Capture session; rustyfiles windows stay visible to screenshots and recordings");
                // no window carries the flag any more (an earlier mode of this process may have set it)
                let mut all: Vec<isize> = self.wins.keys().copied().collect();
                all.extend(self.excl.extras().iter().copied());
                for h in all {
                    super::clear_affinity(h);
                }
            }
            Err(why) => self.use_affinity(&why, false, failure),
        }
    }

    /// Desktop Duplication with the display-affinity exclusion from now on: the flag goes on every window of ours that must stay out of the
    /// picture BEFORE any capture is made, and the sessions (if there are any) are closed. `fell_back`: a session was in use and failed.
    fn use_affinity(&mut self, why: &str, fell_back: bool, failure: bool) {
        if fell_back {
            log::info!("glass: the capture session failed ({why}); falling back to Desktop Duplication, rustyfiles windows are hidden from screenshots and recordings");
        } else {
            log::info!("glass: screen capture through Desktop Duplication, rustyfiles windows are hidden from screenshots and recordings ({why})");
        }
        self.mode = CaptureMode::Affinity;
        set_mode(CaptureMode::Affinity);
        self.session_retry = failure.then(|| SessionRetry::new(Instant::now()));
        if failure {
            log::info!("glass: a capture session will be tried again in {} s, and every {} s after the first minutes", super::retry::PAUSES[0].as_secs(), super::retry::LAST_PAUSE.as_secs());
        }
        self.mons.clear();
        let keys: Vec<isize> = self.wins.keys().copied().collect();
        for k in keys {
            if let Err(e) = super::set_affinity(k, true) {
                self.fail_window(k, &format!("the window cannot be excluded from screen capture: {e}"));
            }
        }
        for h in super::extras_snapshot() {
            let _ = super::set_affinity(h, true);
        }
    }

    /// Opens the capture of a monitor with the backend that was chosen. A session that cannot be made switches everything to Desktop
    /// Duplication; a monitor Desktop Duplication cannot use makes its windows fail.
    fn open_monitor(&mut self, hmon: isize) {
        if self.mode == CaptureMode::Session {
            let power = self.power;
            let opened = self.gfx.as_ref().map(|gfx| unsafe { Monitor::open_session(&gfx.r.g, hmon, power) });
            match opened {
                Some(Ok(m)) => {
                    self.mons.insert(hmon, m);
                    self.session_worked = true;
                    self.reopen.opened(hmon);
                    return;
                }
                // Sessions worked in this run: this failure is probably a passing one (a display waking up, a mode change). The
                // windows keep their last picture and the session is tried again after a pause; only repeated failures change the backend.
                Some(Err(why)) if self.session_worked && self.reopen.failed(hmon, Instant::now()) == Retry::Later => {
                    log::info!("glass: the capture session of monitor {hmon:#x} could not be opened again ({why}); trying again shortly");
                    return;
                }
                Some(Err(why)) => self.use_affinity(&why, true, true),
                None => return,
            }
        }
        let Some(luid) = self.gfx.as_ref().map(|g| g.luid) else { return };
        match unsafe { Monitor::open(hmon, luid) } {
            Ok(m) => {
                self.mons.insert(hmon, m);
            }
            Err(why) => self.fail_on_monitor(hmon, &why),
        }
    }

    /// Looks at the glass windows of other processes of ours (a few times a second, only while some glass is seen) and leaves out of
    /// the capture the ones that lie above one of ours: two glass windows of two processes that read each other build up a flat red and
    /// blue (see `crossproc.rs`). One walk of the top-level windows, a property test per window.
    fn scan_foreign(&mut self, now: Instant) {
        if now.saturating_duration_since(self.last_scan) < crossproc::SCAN_EVERY {
            return;
        }
        let seen: Vec<&Win> = self.wins.values().filter(|w| w.active()).collect();
        if seen.is_empty() {
            return;
        }
        self.last_scan = now;
        // the glass reads this far around itself; a window whose edge is nearer than that can show in the frost
        let margin = seen.iter().filter_map(|w| w.geo).map(|g| region_pad(g.scale())).max().unwrap_or(0) as i32;
        let mine: BTreeSet<isize> = self.wins.keys().copied().collect();
        let order = crossproc::scan(&mine);
        self.foreign_seen = order.iter().any(|e| !e.mine);
        let wanted = crossproc::foreign_above(&order, margin);
        if self.foreign.update(&wanted, now, wgc::is_window) && self.excl.set_foreign(self.foreign.set()) {
            log::debug!("glass: {} glass windows of other processes are left out of the capture", self.foreign.set().len());
        }
    }

    /// Takes the frames the monitors have. Returns whether a wait for a frame happened (the turn did not spin).
    fn pump(&mut self) -> bool {
        self.throttle = None;
        if self.gfx.is_none() {
            return false;
        }
        let now = Instant::now();
        if self.mode == CaptureMode::Affinity && self.session_retry.is_some() {
            self.try_session_again(now);
        }
        if self.mode == CaptureMode::Session {
            self.scan_foreign(now);
        }
        let Some(gfx) = self.gfx.as_ref() else { return false };
        if self.mode == CaptureMode::Session {
            // menus and flyouts show without telling the engine
            self.excl.poll_extras(wgc::probe_window);
            self.excl.settle(now, wgc::is_window);
        }
        // only monitors with a glass that is seen are looked at; the others hold their session for a while (`parked`)
        let in_use: BTreeSet<isize> = self.wins.values().filter(|w| w.active()).filter_map(|w| w.geo.map(|g| g.hmon)).collect();
        let mut budget = self.frame_wait_ms(); // the first monitor that is waited on waits, the others only look
        // A frame on another monitor ends that wait too, or a long quiet wait (`quiet_wait_ms`) would delay the notice of it by as much.
        // With one monitor this is an empty list and costs nothing.
        let others: Vec<Receiver<()>> = if in_use.len() > 1 { self.mons.iter().filter(|(h, _)| in_use.contains(h)).filter_map(|(_, m)| m.wake().cloned()).collect() } else { Vec::new() };
        let mut waited = false;
        let mut errors: Vec<(isize, MonError)> = vec![];
        let mut fresh: Vec<(isize, Dirty)> = vec![];
        for (&hmon, m) in self.mons.iter_mut().filter(|(h, _)| in_use.contains(h)) {
            match unsafe { m.pump(&gfx.r.g, budget, &self.excl, Some(&self.rx), &others) } {
                Ok(Pumped::NoWait) => {}
                Ok(Pumped::Waited { frame, image, dirty }) => {
                    if budget > 0 {
                        waited = true;
                        budget = 0;
                        if frame {
                            self.turns.frames += 1;
                        } else {
                            self.turns.no_frame += 1;
                        }
                    }
                    for w in self.wins.values_mut().filter(|w| w.geo.is_some_and(|g| g.hmon == hmon)) {
                        w.stats.frames += frame as u32;
                        w.stats.with_image += image as u32;
                    }
                    if image {
                        fresh.push((hmon, dirty));
                    }
                }
                Err(e) => errors.push((hmon, e)),
            }
        }
        // a Desktop Duplication monitor that is leaving its frames alone says for how long: the turn then waits exactly that
        if !waited {
            self.throttle = self.mons.iter().filter(|(h, _)| in_use.contains(h)).filter_map(|(_, m)| m.throttled_for()).min();
        }
        // only the windows whose backdrop (their rectangle and the padding the glass reads around it) was touched are drawn again
        for (hmon, dirty) in fresh {
            let Some(r) = self.mons.get(&hmon).map(|m| m.rect()) else { continue };
            self.note_fresh(hmon, r, &dirty);
        }
        for (hmon, e) in errors {
            match e {
                MonError::Device(why) => {
                    self.fail_all(&why);
                    return waited;
                }
                MonError::Monitor(why) => self.fail_on_monitor(hmon, &why),
                MonError::Backend(why) => {
                    // the windows are not failed: Desktop Duplication takes over for all of them (opened on the next turn)
                    self.use_affinity(&why, true, true);
                    return waited;
                }
            }
        }
        waited
    }

    /// A new picture of a monitor came in, `dirty` says where it changed: the active windows whose backdrop (their rectangle and the
    /// padding the glass reads around it) was touched are drawn again. Only a picture that touched some glass counts as activity: a
    /// frame that changed something elsewhere (a blinking caret, a terminal) must not keep the quiet clock from running down, or an
    /// ordinary desktop would keep the loop at its short wait for ever instead of 20 turns a second.
    fn note_fresh(&mut self, hmon: isize, r: RECT, dirty: &Dirty) -> bool {
        let mut touched = false;
        for w in self.wins.values_mut().filter(|w| w.active()) {
            let Some(g) = w.geo.filter(|g| g.hmon == hmon) else { continue };
            if dirty.touches(g.x - r.left, g.y - r.top, g.w, g.h, region_pad(g.scale())) {
                w.restale();
                touched = true;
            }
        }
        if touched {
            self.last_activity = Instant::now();
        }
        touched
    }

    fn draw_dirty(&mut self) {
        let now = Instant::now();
        let gap = rest::gap_for(rest::behind_fps());
        let todo: Vec<isize> =
            self.wins.values().filter(|w| w.active() && w.dirty && w.draw_due(now, gap)).map(|w| w.hwnd).collect();
        for k in todo {
            let Some(gfx) = self.gfx.as_ref() else { return };
            let Some(w) = self.wins.get_mut(&k) else { continue };
            let looked = w.stale;
            let Some(hmon) = w.geo.map(|g| g.hmon) else { continue };
            // A parked session has not been resumed yet (see `sync_monitors`): its mirror is from before the pause.
            if self.parked.is_parked(hmon) {
                continue;
            }
            let Some(mon) = self.mons.get(&hmon) else { continue };
            if !mon.has_image() {
                continue; // dirty stays; drawn as soon as the first picture of the screen is in
            }
            match unsafe { draw(gfx, mon, w) } {
                Ok(_) => {
                    w.dirty = false;
                    if looked {
                        w.last_draw = Some(Instant::now());
                    }
                }
                Err(Fail::Window(why)) => self.fail_window(k, &why),
                Err(Fail::Device(why)) => {
                    self.fail_all(&why);
                    return;
                }
            }
        }
    }

    /// Statistics every few seconds (RUSTYFILES_GLASS_STATS=1), and a look at the device.
    fn housekeeping(&mut self) {
        let now = Instant::now();
        self.trim_idle(now);
        if self.stats_on {
            if let Some(line) = self.turns.report(now) {
                log::info!("{line}");
            }
            for w in self.wins.values_mut() {
                let s = &mut w.stats;
                if now.duration_since(s.since) >= STATS_EVERY {
                    let draws = (s.presented + s.skipped).max(1) as f64;
                    log::info!(
                        "glass {:#x}: {} frames acquired, {} with an image, {} draws presented, {} skipped by the loop guard ({} draws only moved a lens or the shape), prep {:.2} ms, glass pass {:.2} ms (CPU time per draw)",
                        w.hwnd,
                        s.frames,
                        s.with_image,
                        s.presented,
                        s.skipped,
                        s.reused,
                        s.prep.as_secs_f64() * 1000.0 / draws,
                        s.glass.as_secs_f64() * 1000.0 / s.presented.max(1) as f64,
                    );
                    *s = Stats::new(now);
                }
            }
        }
        if now.duration_since(self.last_power_check) >= POWER_CHECK_EVERY {
            self.last_power_check = now;
            let power = wgc::read_power();
            self.power = power;
            // (a call that changes nothing when the rate stays: only a change of power or of the display's rate is sent on)
            for m in self.mons.values_mut() {
                unsafe { m.retune(power) };
            }
        }
        if now.duration_since(self.last_device_check) >= DEVICE_CHECK_EVERY {
            self.last_device_check = now;
            if let Some(gfx) = &self.gfx
                && let Err(e) = unsafe { gfx.r.g.dev.GetDeviceRemovedReason() }
            {
                self.fail_all(&format!("the graphics device was removed: {e}"));
            }
        }
        if self.mode == CaptureMode::Session
            && let Some(why) = self.access_tick(now)
        {
            self.use_affinity(&format!("the permission was withdrawn: {why}"), true, false);
        }
    }

    /// The look at the capture permission, off the engine thread: reading it takes about 3 ms (a WinRT call into the capability broker),
    /// which would be a stall of the draw loop every `ACCESS_CHECK_EVERY` (15 refreshes at 200 Hz, if a lens happens to be sliding then).
    /// Starts a check when one is due and none is running; returns the reason when a finished check says the permission is withdrawn.
    /// Only a definite "no" counts: a read that failed just now says nothing about the permission.
    fn access_tick(&mut self, now: Instant) -> Option<String> {
        if let Some(job) = &self.access_job {
            match job.try_recv() {
                Ok(Access::Not(why)) => {
                    self.access_job = None;
                    return Some(why);
                }
                Ok(_) | Err(crossbeam_channel::TryRecvError::Disconnected) => self.access_job = None,
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
        } else if now.duration_since(self.last_access_check) >= ACCESS_CHECK_EVERY {
            self.last_access_check = now;
            let (tx, rx) = crossbeam_channel::bounded(1);
            // WinRT needs the thread to be in an apartment; the check ends the thread, nothing is kept
            let spawned = std::thread::Builder::new().name("rf-glass-access".into()).spawn(move || {
                let _apartment = Apartment::enter();
                let _ = tx.send(wgc::access());
            });
            if spawned.is_ok() {
                self.access_job = Some(rx);
            }
        }
        None
    }

    /// Takes the glass off a window that leaves the engine.
    fn teardown(&self, w: Win) {
        if let (Some(t), Some(gfx)) = (&w.target, &self.gfx) {
            unsafe { t.remove(gfx) };
        }
    }

    /// A window falls back: its glass is taken off, the capture exclusion lifted, the state says why. The app then shows Mica.
    fn fail_window(&mut self, hwnd: isize, why: &str) {
        self.excl.detach(hwnd);
        if let Some(w) = self.wins.remove(&hwnd) {
            self.teardown(w);
        }
        log::warn!("glass: window {hwnd:#x} falls back: {why}");
        super::fail(hwnd, why);
    }

    fn fail_on_monitor(&mut self, hmon: isize, why: &str) {
        let victims: Vec<isize> = self.wins.values().filter(|w| w.active() && w.geo.is_some_and(|g| g.hmon == hmon)).map(|w| w.hwnd).collect();
        for k in victims {
            self.fail_window(k, why);
        }
        self.mons.remove(&hmon);
    }

    fn fail_all(&mut self, why: &str) {
        let all: Vec<isize> = self.wins.keys().copied().collect();
        for k in all {
            self.fail_window(k, why);
        }
        self.drop_all();
    }

    fn drop_all(&mut self) {
        let all: Vec<Win> = std::mem::take(&mut self.wins).into_values().collect();
        for w in all {
            self.teardown(w);
        }
        self.release_gfx();
    }

    /// Lets go of the captures and the device. The next device made chooses the backend anew.
    fn release_gfx(&mut self) {
        self.mons.clear();
        self.parked = Parking::default();
        self.reopen = Reopen::default();
        self.session_worked = false;
        self.session_retry = None;
        self.gfx = None;
        self.access_job = None;
        self.mode = CaptureMode::Off;
        set_mode(CaptureMode::Off);
        self.excl = Exclusion::default();
        self.foreign = Foreign::default();
        self.foreign_seen = false;
        super::set_not_front(BTreeSet::new());
    }
}

#[cfg(test)]
mod tests {
    use super::super::LookParams;
    use super::*;

    /// A change of the look reaches every window the engine holds, whether or not it was drawn before: each is marked for a new backdrop
    /// and a new draw. (The draw reads the value itself, and the loop guard's `Presented` holds the whole `Look`, so that draw is made.)
    /// No window or device is involved: the engine is only given its commands.
    #[test]
    fn a_look_change_marks_every_window_for_a_new_draw() {
        let (_tx, rx) = crossbeam_channel::unbounded();
        let mut e = Engine::new(rx);
        for hwnd in [1isize, 2] {
            let mut w = Win::new(hwnd, GlassOptions::window(hwnd == 1));
            w.dirty = false;
            w.stale = false;
            e.wins.insert(hwnd, w);
        }
        e.command(Cmd::LookChanged);
        assert!(e.wins.values().all(|w| w.dirty && w.stale), "a window was not marked by the look change");
    }

    /// A change of the shape alone (the dock's island widening under the pointer) draws again from the backdrop the window already has:
    /// no new copy of the screen, no new blur pyramid, no read back. Anything else about the options is a new backdrop pass as before.
    #[test]
    fn a_shape_change_is_one_draw_and_not_a_new_backdrop() {
        let island = GlassOptions { radius_px: Some(24.0), cut_corners: true, shape: Some([28.0, 28.0, 300.0, 48.0]), shadow: Some(Shadow::SOFT), ..GlassOptions::window(true) };
        let mut w = Win::new(1, island);
        assert!(w.dirty && w.stale, "a new window needs everything");
        let settled = |w: &mut Win| {
            w.dirty = false;
            w.stale = false;
            w.shape_changed = None;
        };
        settled(&mut w);
        // the same options again: nothing at all
        w.set_options(island);
        assert!(!w.dirty && !w.stale && w.shape_changed.is_none());
        // a wider shape: one draw, and the engine does not wait long for a frame for a moment
        w.set_options(GlassOptions { shape: Some([8.0, 28.0, 340.0, 48.0]), ..island });
        assert!(w.dirty && !w.stale, "the backdrop is the one the region holds");
        assert_eq!(w.shape, Some([8.0, 28.0, 340.0, 48.0]));
        assert!(w.shape_changed.is_some());
        // the shadow going on or off is as cheap
        settled(&mut w);
        w.set_options(GlassOptions { shape: Some([8.0, 28.0, 340.0, 48.0]), shadow: None, ..island });
        assert!(w.dirty && !w.stale);
        // a theme, a radius or a corner change is a new backdrop pass, as before
        for changed in [GlassOptions { dark: false, ..island }, GlassOptions { radius_px: Some(12.0), ..island }, GlassOptions { cut_corners: false, ..island }] {
            settled(&mut w);
            w.set_options(island);
            settled(&mut w);
            w.set_options(changed);
            assert!(w.dirty && w.stale, "{changed:?}");
        }
        // an island growing into its panel eases its radius together with its rectangle: as cheap as the rectangle alone (the radius is
        // a constant of the draw); the theme or the corner cutting with it, or a radius with no new rectangle, is not
        settled(&mut w);
        w.set_options(island);
        settled(&mut w);
        w.set_options(GlassOptions { radius_px: Some(12.0), shape: Some([8.0, 28.0, 340.0, 300.0]), ..island });
        assert!(w.dirty && !w.stale, "shape and radius together");
        assert_eq!(w.radius_px, Some(12.0));
        // What the draw takes from it: the radius the draw uses is worked out from the window's options at EVERY draw (`effective_radius`
        // in `draw`, and nowhere else is the radius used: the backdrop pass takes none), so a glass that reuses its backdrop is drawn with
        // the radius of the moment. Eased over a motion, frame by frame, each one cheap and each one drawn with its own radius.
        settled(&mut w);
        w.set_options(island);
        settled(&mut w);
        for i in 1..=20 {
            let t = i as f32 / 20.0;
            let (radius, height) = (24.0 + (8.0 - 24.0) * t, 48.0 + (700.0 - 48.0) * t);
            w.set_options(GlassOptions { radius_px: Some(radius), shape: Some([8.0, 28.0, 340.0 + 300.0 * t, height]), ..island });
            assert!(w.dirty && !w.stale, "frame {i}: no new backdrop");
            let drawn = effective_radius(w.radius_px, 8.0, (340.0 + 300.0 * t) as u32, height as u32);
            assert_eq!(drawn, radius.min((340.0 + 300.0 * t).min(height).floor() / 2.0), "frame {i}: the radius of the moment is the one drawn");
            settled(&mut w);
        }
        w.set_options(GlassOptions { radius_px: Some(12.0), shape: Some([8.0, 28.0, 340.0, 300.0]), dark: false, ..island });
        assert!(w.dirty && w.stale, "a theme change is a new backdrop pass even with a new shape");
    }

    /// While a shape changes the engine takes each new one at once (a short wait for a frame, and not paced by the display, which would
    /// show every frame one refresh late); a moment after the last change it is at rest again.
    #[test]
    fn a_changing_shape_shortens_the_wait_and_a_resting_one_does_not() {
        let mut e = idle_engine(CaptureMode::Off);
        // just after activity, so the plain wait is the frame wait (the quiet one is tested apart)
        e.last_activity = Instant::now();
        let mut w = Win::new(1, GlassOptions { shape: Some([0.0, 0.0, 10.0, 10.0]), ..GlassOptions::window(true) });
        w.visible = true;
        e.wins.insert(1, w);
        assert!(!e.shape_moving(), "a window with a shape that never changed is at rest");
        assert_eq!(e.frame_wait_ms(), FRAME_WAIT_MS);
        e.command(Cmd::SetOptions { hwnd: 1, opts: GlassOptions { shape: Some([0.0, 0.0, 20.0, 10.0]), ..GlassOptions::window(true) } });
        assert!(e.shape_moving(), "the shape changed a moment ago");
        assert!(!e.paced(), "the display does not pace a shape: every frame would be a refresh late");
        assert_eq!(e.frame_wait_ms(), SHAPE_WAIT_MS, "a short wait, so the next shape is taken soon after it is sent");
        e.wins.get_mut(&1).unwrap().shape_changed = Some(Instant::now() - SHAPE_ACTIVE - Duration::from_millis(1));
        assert!(!e.shape_moving(), "and at rest again after a moment");
        e.wins.get_mut(&1).unwrap().visible = false;
        e.wins.get_mut(&1).unwrap().shape_changed = Some(Instant::now());
        assert!(!e.shape_moving(), "a hidden window does not keep the loop running");
    }

    /// On a quiet desktop the quiet wait is long (main's rule); a shape that moves still cuts it to the short one in both capture modes,
    /// and once it rests the quiet wait is back.
    #[test]
    fn a_moving_shape_wins_over_the_quiet_wait_and_the_quiet_wait_comes_back() {
        for mode in [CaptureMode::Session, CaptureMode::Affinity] {
            let mut e = idle_engine(mode);
            let mut w = Win::new(1, GlassOptions { shape: Some([0.0, 0.0, 10.0, 10.0]), ..GlassOptions::window(true) });
            w.visible = true;
            e.wins.insert(1, w);
            assert_eq!(e.frame_wait_ms(), QUIET_WAIT_MS, "{mode:?}: a quiet desktop");
            e.command(Cmd::SetOptions { hwnd: 1, opts: GlassOptions { shape: Some([0.0, 0.0, 20.0, 10.0]), ..GlassOptions::window(true) } });
            assert_eq!(e.frame_wait_ms(), SHAPE_WAIT_MS, "{mode:?}: a moving shape");
            // (a command counts as activity; let the desktop be quiet again as well as the shape at rest)
            e.last_activity = Instant::now().checked_sub(QUIET_AFTER * 2).unwrap_or_else(Instant::now);
            e.wins.get_mut(&1).unwrap().shape_changed = Some(Instant::now() - SHAPE_ACTIVE - Duration::from_millis(1));
            assert_eq!(e.frame_wait_ms(), QUIET_WAIT_MS, "{mode:?}: at rest again");
        }
    }

    /// The look is part of what the loop guard compares: the same picture with another look is not skipped.
    #[test]
    fn the_loop_guard_sees_a_new_look() {
        let geo = Geometry { w: 400, h: 300, origin: (0, 0), scale: 1.0, radius_px: 8.0, cut_corners: false, shape: None, shadow: None };
        let lenses = LensList::default();
        let a = Presented { hash: 7, geo, look: Look::regular(false, &LookParams::DEFAULT), lenses };
        let b = Presented { look: Look::regular(false, &LookParams { frost: 80.0, ..LookParams::DEFAULT }), ..a };
        let same = a;
        assert!(same == a && a != b);
    }

    fn idle_engine(mode: CaptureMode) -> Engine {
        let (_tx, rx) = crossbeam_channel::unbounded();
        let mut e = Engine::new(rx);
        // quiet for longer than QUIET_AFTER
        e.last_activity = Instant::now().checked_sub(QUIET_AFTER * 2).unwrap_or_else(Instant::now);
        e.mode = mode;
        e
    }

    /// A glass window at rest with a selection lens on it, as every glass window of the app is.
    fn window_with_resting_lens(hwnd: isize) -> Win {
        let mut w = Win::new(hwnd, GlassOptions::window(true));
        w.visible = true;
        w.dirty = false;
        w.lenses.set(0, Some(super::super::LensSpec { key: 1, rect: [10.0, 10.0, 80.0, 30.0], clip: [0.0, 0.0, 200.0, 100.0], radius: 15.0 }), false);
        assert!(w.lenses.occupied() && !w.lenses.animating(), "the lens is placed and at rest");
        w
    }

    /// On an idle desktop the loop turns once per wait. With a capture session a held lens no longer shortens the wait (the session's
    /// wait ends for a command anyway); Desktop Duplication, whose wait cannot be interrupted, keeps it.
    #[test]
    fn a_resting_lens_shortens_the_quiet_wait_only_where_a_command_cannot_end_it() {
        let turns_a_second = |ms: u32| 1000.0 / ms as f64;
        for (mode, want) in [(CaptureMode::Session, QUIET_WAIT_MS), (CaptureMode::Affinity, LENS_WAIT_MS)] {
            let mut e = idle_engine(mode);
            e.wins.insert(1, window_with_resting_lens(1));
            assert_eq!(e.frame_wait_ms(), want, "{mode:?} with a lens at rest");
            println!("{mode:?}, lens at rest, idle desktop: wait {} ms = {:.0} turns a second", e.frame_wait_ms(), turns_a_second(e.frame_wait_ms()));
        }
        // without a lens both wait the same
        for mode in [CaptureMode::Session, CaptureMode::Affinity] {
            let mut e = idle_engine(mode);
            let mut w = Win::new(1, GlassOptions::window(true));
            w.visible = true;
            e.wins.insert(1, w);
            assert_eq!(e.frame_wait_ms(), QUIET_WAIT_MS);
        }
        // and a hidden window's lens does not count in either
        let mut e = idle_engine(CaptureMode::Affinity);
        let mut w = window_with_resting_lens(1);
        w.visible = false;
        e.wins.insert(1, w);
        assert_eq!(e.frame_wait_ms(), QUIET_WAIT_MS);
    }

    /// A moving lens is still paced by the display, and a turn soon after activity is still the short one, whatever the backend.
    #[test]
    fn the_short_waits_after_activity_and_while_a_lens_moves_are_unchanged() {
        let mut e = idle_engine(CaptureMode::Session);
        e.last_activity = Instant::now();
        e.wins.insert(1, window_with_resting_lens(1));
        assert_eq!(e.frame_wait_ms(), FRAME_WAIT_MS, "just after activity");
        e.last_activity = Instant::now().checked_sub(QUIET_AFTER * 2).unwrap_or_else(Instant::now);
        e.wins.get_mut(&1).unwrap().lenses.set(0, Some(super::super::LensSpec { key: 2, rect: [100.0, 10.0, 80.0, 30.0], clip: [0.0, 0.0, 200.0, 100.0], radius: 15.0 }), true);
        assert!(e.lens_active());
        assert_eq!(e.frame_wait_ms(), 0, "a sliding lens: the display paces the turn");
    }

    /// The permission look runs on a thread of its own: `housekeeping` only starts it (and later collects the answer), so the draw loop
    /// is not held up for the call's few milliseconds every ten seconds.
    #[test]
    fn the_permission_check_is_not_made_on_the_engine_thread() {
        let mut e = idle_engine(CaptureMode::Session);
        // not due yet: nothing starts
        assert!(e.access_tick(Instant::now()).is_none());
        assert!(e.access_job.is_none());
        // due: a check starts and the call returns without its answer
        e.last_access_check = Instant::now().checked_sub(ACCESS_CHECK_EVERY * 2).unwrap_or_else(Instant::now);
        let t = Instant::now();
        let first = e.access_tick(Instant::now());
        let started_in = t.elapsed();
        assert!(first.is_none(), "the answer cannot be there yet");
        assert!(e.access_job.is_some(), "a check is running on its own thread");
        // the same call made inline, for comparison
        let t = Instant::now();
        let inline = wgc::access();
        let inline_took = t.elapsed();
        let t = Instant::now();
        let _ = wgc::access();
        println!("starting the check took {started_in:?}; the same check inline took {inline_took:?} (first call of a process), then {:?}", t.elapsed());
        // no second check is started while one runs; the answer arrives
        let deadline = Instant::now() + Duration::from_secs(10);
        while e.access_job.is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            // a refusal is reported once, as the reason; anything else is dropped
            if let Some(why) = e.access_tick(Instant::now()) {
                assert!(matches!(inline, Access::Not(_)), "a refusal was reported but the inline check said {inline:?}: {why}");
            }
        }
        assert!(e.access_job.is_none(), "the check finished");
        // and the next one is only due after ACCESS_CHECK_EVERY
        assert!(e.access_tick(Instant::now()).is_none());
        assert!(e.access_job.is_none());
    }

    // --- glass that is seen -----------------------------------------------------------------------------------------------------------

    fn geo_at(x: i32, y: i32, w: u32, h: u32, hmon: isize) -> WinGeo {
        WinGeo { x, y, w, h, dpi: 96, square: false, round: 8.0, hmon }
    }

    /// A window that is shown, measured, drawn and at rest.
    fn settled_window(hwnd: isize, geo: WinGeo) -> Win {
        let mut w = Win::new(hwnd, GlassOptions::window(true));
        w.visible = true;
        w.geo = Some(geo);
        w.dirty = false;
        w.stale = false;
        let g = Geometry { w: geo.w, h: geo.h, origin: (geo.x, geo.y), scale: 1.0, radius_px: 8.0, cut_corners: false, shape: None, shadow: None };
        w.presented = Some(Presented { hash: 7, geo: g, look: Look::regular(true, &LookParams::DEFAULT), lenses: LensList::default() });
        w
    }

    /// A dock island as the dock attaches it: a window much larger than its glass, with the shape still empty.
    fn island(hwnd: isize, shape: [f32; 4]) -> Win {
        let opts = GlassOptions { radius_px: Some(24.0), cut_corners: true, shape: Some(shape), shadow: Some(Shadow::SOFT), ..GlassOptions::window(true) };
        let mut w = Win::new(hwnd, opts);
        w.visible = true;
        w.geo = Some(geo_at(100, 1300, 720, 96, 1));
        w
    }

    /// A window that comes into view again is drawn at once from a fresh backdrop and presented whatever the loop guard remembers: the
    /// picture on its screen is from before it was covered. Going under other windows marks nothing.
    #[test]
    fn a_window_that_comes_into_view_is_drawn_at_once_from_a_fresh_backdrop_and_never_skipped() {
        let mut w = settled_window(1, geo_at(0, 0, 400, 300, 1));
        assert!(w.active());
        assert!(!w.set_covered(true), "going under other windows is not coming into view");
        assert!(w.covered && !w.active());
        assert!(!w.dirty && !w.stale && w.presented.is_some(), "being covered marks nothing and forgets nothing");
        assert!(!w.set_covered(true), "still covered: nothing");
        assert!(w.set_covered(false), "in view again");
        assert!(w.dirty && w.stale, "drawn again, from a new backdrop and not from the one it had");
        assert!(w.presented.is_none(), "and presented even if the loop guard remembers the same picture");
        assert!(!w.set_covered(false), "in view and still in view: nothing");
    }

    /// Glass nobody sees does not pace the loop, does not shorten the wait and wants no capture.
    #[test]
    fn glass_that_is_covered_does_not_keep_the_engine_busy() {
        let mut e = idle_engine(CaptureMode::Session);
        let mut w = settled_window(1, geo_at(0, 0, 400, 300, 1));
        let spec = |key, x| Some(super::super::LensSpec { key, rect: [x, 10.0, 80.0, 30.0], clip: [0.0, 0.0, 400.0, 300.0], radius: 15.0 });
        w.lenses.set(0, spec(1, 10.0), false);
        w.lenses.set(0, spec(2, 100.0), true);
        w.shape_changed = Some(Instant::now());
        e.wins.insert(1, w);
        assert!(e.lens_active() && e.shape_moving() && e.paced(), "an uncovered one does");
        e.wins.get_mut(&1).unwrap().covered = true;
        assert!(!e.lens_active() && !e.shape_moving() && !e.paced(), "a covered one does not");
        assert!(!e.wins.values().any(|w| w.active()));
        e.advance_lenses();
        assert!(e.wins.get(&1).unwrap().lens_tick.is_none(), "its lens is not stepped");
        // the wait of a turn with nothing to do is the long one: no lens counts
        e.last_activity = Instant::now().checked_sub(QUIET_AFTER * 2).unwrap_or_else(Instant::now);
        assert_eq!(e.frame_wait_ms(), QUIET_WAIT_MS);
        // it is not among the windows a frame is drawn for
        e.wins.get_mut(&1).unwrap().dirty = true;
        let todo: Vec<isize> = e.wins.values().filter(|w| w.active() && w.dirty).map(|w| w.hwnd).collect();
        assert!(todo.is_empty());
    }

    /// The dock attaches every island with an empty shape and gives it the real one a moment later. An empty glass has nothing that
    /// could be hidden: the empty target of the covering test is trivially covered, and the island must not wait (with the plain fill
    /// of its fallback on the screen) for the next look at the z order.
    #[test]
    fn an_island_with_an_empty_shape_is_not_covered_and_its_real_shape_is_drawn_at_once() {
        let geo = geo_at(100, 1300, 720, 96, 1);
        assert_eq!(glass_on_screen(&geo, Some([0.0; 4])), None, "an empty shape shows nothing");
        assert_eq!(glass_on_screen(&geo, Some([20.0, 10.0, 0.0, 48.0])), None);
        assert_eq!(glass_on_screen(&geo, Some([20.0, 10.5, 300.0, 48.0])), Some(Rc::new(120, 1310, 420, 1359)), "a shape is rounded out to whole pixels");
        assert_eq!(glass_on_screen(&geo, None), Some(Rc::new(100, 1300, 820, 1396)), "a window shows its client area");
        assert_eq!(glass_on_screen(&geo_at(0, 0, 0, 30, 1), None), None);

        let mut e = idle_engine(CaptureMode::Session);
        e.wins.insert(1, island(1, [0.0; 4]));
        e.wins.insert(2, settled_window(2, geo_at(0, 0, 400, 300, 1)));
        // the look at the z order says that everything on the screen is covered
        let all_covered = BTreeMap::from([(1isize, true), (2isize, true)]);
        e.apply_cover(&all_covered, Instant::now());
        assert!(!e.wins.get(&1).unwrap().covered, "an island with an empty shape is waiting for its shape, not hidden");
        assert!(e.wins.get(&2).unwrap().covered, "an ordinary window under others is");

        // its first real shape arrives: nothing is waited for
        e.cover_due = false;
        e.command(Cmd::SetOptions { hwnd: 1, opts: GlassOptions { radius_px: Some(24.0), cut_corners: true, shape: Some([20.0, 10.0, 300.0, 48.0]), shadow: Some(Shadow::SOFT), ..GlassOptions::window(true) } });
        assert!(e.cover_due, "a glass that was nothing and is something now is looked at at once");
        e.cover_due = false;
        e.apply_cover(&all_covered, Instant::now());
        assert!(e.wins.get(&1).unwrap().covered, "and a glass with an area that is under others is covered, as any");
    }

    /// A covered window whose shape moves may come into view by it; a shape that moves in a window in view changes nothing about that.
    #[test]
    fn a_shape_that_moves_in_a_covered_window_asks_for_a_new_look_at_the_z_order() {
        let mut e = idle_engine(CaptureMode::Session);
        e.wins.insert(1, island(1, [20.0, 10.0, 300.0, 48.0]));
        let moved = |x: f32| GlassOptions { radius_px: Some(24.0), cut_corners: true, shape: Some([x, 10.0, 300.0, 48.0]), shadow: Some(Shadow::SOFT), ..GlassOptions::window(true) };
        e.cover_due = false;
        e.command(Cmd::SetOptions { hwnd: 1, opts: moved(30.0) });
        assert!(!e.cover_due, "a shape that moves in a window in view: the walk is not made per animation frame");
        e.wins.get_mut(&1).unwrap().covered = true;
        e.command(Cmd::SetOptions { hwnd: 1, opts: moved(40.0) });
        assert!(e.cover_due, "a covered window whose glass moved may be in view now");
        e.cover_due = false;
        e.command(Cmd::SetOptions { hwnd: 1, opts: moved(40.0) });
        assert!(!e.cover_due, "the same options again: nothing");
    }

    /// The dock's islands are always in view, so the pace of the walks must follow the covered windows, not the others: the main window
    /// under a browser that is dragged away is noticed within the short interval, not the long one.
    #[test]
    fn the_z_order_is_looked_at_often_while_any_glass_is_covered_whatever_else_is_in_view() {
        let mut e = idle_engine(CaptureMode::Session);
        e.wins.insert(1, island(1, [20.0, 10.0, 300.0, 48.0]));
        e.wins.insert(2, settled_window(2, geo_at(0, 0, 400, 300, 1)));
        assert_eq!(e.cover_interval(), COVER_EVERY, "everything in view: no hurry");
        e.wins.get_mut(&2).unwrap().covered = true;
        assert_eq!(e.cover_interval(), COVER_POLL, "one covered window while the island is in view: the short interval");
        assert!(COVER_POLL <= Duration::from_millis(100));
        e.wins.get_mut(&2).unwrap().visible = false;
        assert_eq!(e.cover_interval(), COVER_EVERY, "a hidden window is not covered, it is waited for as before");
    }

    /// What a window reports while its glass is covered or its capture sleeps is the state it had: `Running` stays `Running` (the app
    /// would put its Mica fill on and take it off again for a cover that lasts seconds), a window that never drew stays `Starting`.
    /// Run through the engine's own steps (the verdict, the sessions' bookkeeping, the sleep and the wake), with the states table.
    #[test]
    fn covering_a_window_and_waking_its_capture_never_change_the_state_it_reports() {
        let (running, starting) = (0x7e57_0001isize, 0x7e57_0002isize);
        {
            let mut st = super::super::states();
            st.insert(running, GlassState::Running);
            st.insert(starting, GlassState::Starting);
        }
        let mut e = idle_engine(CaptureMode::Session);
        e.wins.insert(running, settled_window(running, geo_at(0, 0, 400, 300, 1)));
        let mut never_drawn = settled_window(starting, geo_at(500, 0, 400, 300, 1));
        never_drawn.presented = None;
        e.wins.insert(starting, never_drawn);
        let answer = BTreeMap::from([(running, true), (starting, true)]);
        let states = || (super::super::state(running), super::super::state(starting));
        for round in 0..3 {
            e.apply_cover(&answer, Instant::now());
            e.sync_monitors(); // nothing is shown: no monitor wanted, none opened, no device made
            assert!(e.wins.values().all(|w| w.covered), "round {round}");
            assert_eq!(states(), (GlassState::Running, GlassState::Starting), "covered, round {round}");
            e.apply_cover(&BTreeMap::new(), Instant::now());
            assert!(e.wins.values().all(|w| !w.covered));
            assert_eq!(states(), (GlassState::Running, GlassState::Starting), "in view again, round {round}");
        }
        super::super::forget(running);
        super::super::forget(starting);
    }

    /// A failure to open a capture session again, after sessions worked in this run, is tried again after a pause; only the fourth in a
    /// row changes the backend. A success starts the count again, and monitors count apart.
    #[test]
    fn a_session_that_cannot_be_opened_again_is_tried_again_before_the_process_gives_the_backend_up() {
        let t0 = Instant::now();
        let mut r = Reopen::default();
        assert!(r.due(1, t0), "nothing failed yet");
        assert_eq!(r.failed(1, t0), Retry::Later);
        assert!(!r.due(1, t0 + Duration::from_millis(900)), "paused for a second");
        assert!(r.due(1, t0 + Duration::from_secs(1)));
        assert!(r.due(2, t0), "another monitor is not held back");
        let t1 = t0 + Duration::from_secs(1);
        assert_eq!(r.failed(1, t1), Retry::Later);
        assert!(!r.due(1, t1 + Duration::from_millis(1900)) && r.due(1, t1 + Duration::from_secs(2)), "then two seconds");
        let t2 = t1 + Duration::from_secs(2);
        assert_eq!(r.failed(1, t2), Retry::Later);
        assert!(!r.due(1, t2 + Duration::from_millis(4900)) && r.due(1, t2 + Duration::from_secs(5)), "then five");
        assert_eq!(r.failed(1, t2 + Duration::from_secs(5)), Retry::GiveUp, "the fourth in a row is a real failure");
        assert!(r.due(1, t2 + Duration::from_secs(5)), "and the count starts again");
        // a success clears the count
        assert_eq!(r.failed(3, t0), Retry::Later);
        assert_eq!(r.failed(3, t0 + Duration::from_secs(1)), Retry::Later);
        r.opened(3);
        assert!(r.due(3, t0));
        assert_eq!(r.failed(3, t0), Retry::Later);
        assert_eq!(r.failed(3, t0 + Duration::from_secs(1)), Retry::Later);
        assert_eq!(r.failed(3, t0 + Duration::from_secs(3)), Retry::Later, "three failures are still not enough after a success");
    }

    /// Only a picture that touched some glass is activity. A frame that changed something else on the monitor (a caret that blinks, a
    /// terminal) must not keep the quiet clock from running down: on an ordinary desktop the loop would stay at its short wait, about a
    /// hundred turns a second, instead of 20.
    #[test]
    fn a_picture_that_touches_no_glass_is_not_activity() {
        let mon = RECT { left: 0, top: 0, right: 2560, bottom: 1440 };
        let mut e = idle_engine(CaptureMode::Session);
        e.wins.insert(1, settled_window(1, geo_at(1000, 500, 400, 300, 7)));
        let far = Dirty::Rects(vec![RECT { left: 10, top: 10, right: 40, bottom: 30 }]);
        let near = Dirty::Rects(vec![RECT { left: 1100, top: 600, right: 1120, bottom: 620 }]);
        let quiet = |e: &mut Engine| e.last_activity = Instant::now().checked_sub(QUIET_AFTER * 2).unwrap_or_else(Instant::now);
        assert!(e.last_activity.elapsed() > QUIET_AFTER);
        assert!(!e.note_fresh(7, mon, &far));
        assert!(e.last_activity.elapsed() > QUIET_AFTER, "a change elsewhere on the monitor leaves the quiet clock running");
        assert_eq!(e.frame_wait_ms(), QUIET_WAIT_MS);
        assert!(!e.wins.get(&1).unwrap().dirty && !e.wins.get(&1).unwrap().stale, "and the window is not drawn again");
        assert!(!e.note_fresh(7, mon, &Dirty::none()));
        // another monitor's picture is not this window's
        assert!(!e.note_fresh(9, mon, &near));
        quiet(&mut e);
        assert!(e.note_fresh(7, mon, &near), "a change under the window is");
        assert!(e.last_activity.elapsed() < QUIET_AFTER);
        let w = e.wins.get(&1).unwrap();
        assert!(w.dirty && w.stale, "and the window is drawn again from a new backdrop");
        // a picture of the whole monitor (the first one, or after a new exclusion list) touches every window
        quiet(&mut e);
        assert!(e.note_fresh(7, mon, &Dirty::All));
        // a covered window is no one's concern
        quiet(&mut e);
        e.wins.get_mut(&1).unwrap().covered = true;
        assert!(!e.note_fresh(7, mon, &Dirty::All));
        assert!(e.last_activity.elapsed() > QUIET_AFTER);
    }

    /// A monitor whose capture is parked is not drawn from until it is resumed: a window that came into view between two looks (it
    /// moved while it was covered) is active before `sync_monitors` has run for it.
    #[test]
    fn a_parked_capture_is_known_as_parked_until_it_is_wanted_again() {
        let t0 = Instant::now();
        let grace = Duration::from_secs(5);
        let mut p = Parking::default();
        let want = |ks: &[isize]| ks.iter().copied().collect::<BTreeSet<_>>();
        assert!(!p.is_parked(1));
        p.update(&[1], &want(&[]), t0, grace);
        assert!(p.is_parked(1), "no glass is seen on it: parked");
        p.update(&[1], &want(&[]), t0 + Duration::from_secs(1), grace);
        assert!(p.is_parked(1));
        let c = p.update(&[1], &want(&[1]), t0 + Duration::from_secs(2), grace);
        assert_eq!(c.resumed, vec![1]);
        assert!(!p.is_parked(1), "wanted again: resumed, no longer parked");
        p.update(&[1], &want(&[]), t0 + Duration::from_secs(3), grace);
        p.update(&[1], &want(&[]), t0 + Duration::from_secs(9), grace);
        assert!(!p.is_parked(1), "closed after the grace time: nothing is kept");
    }

    /// A capture is kept for the grace time after its last seen glass went, closed then, and one that is used again has missed frames.
    #[test]
    fn a_session_is_kept_for_the_grace_time_then_closed_and_one_that_comes_back_is_not_trusted() {
        let t0 = Instant::now();
        let grace = Duration::from_secs(5);
        let mut p = Parking::default();
        let want = |ks: &[isize]| ks.iter().copied().collect::<BTreeSet<_>>();
        let nothing = ParkChange::default();
        // two monitors with a session, glass seen on both
        assert_eq!(p.update(&[1, 2], &want(&[1, 2]), t0, grace), nothing);
        // the glass on 2 is covered: its session is kept
        assert_eq!(p.update(&[1, 2], &want(&[1]), t0 + Duration::from_secs(1), grace), nothing);
        assert_eq!(p.update(&[1, 2], &want(&[1]), t0 + Duration::from_millis(5900), grace), nothing, "4.9 s: still kept");
        // it comes back in time: kept, and told that its mirror was not kept up to date
        let c = p.update(&[1, 2], &want(&[1, 2]), t0 + Duration::from_millis(5950), grace);
        assert_eq!(c, ParkChange { resumed: vec![2], expired: vec![] });
        // covered again, for good: closed after the grace time, counted from when it was parked
        assert_eq!(p.update(&[1, 2], &want(&[1]), t0 + Duration::from_secs(10), grace), nothing);
        assert_eq!(p.update(&[1, 2], &want(&[1]), t0 + Duration::from_secs(14), grace), nothing);
        let c = p.update(&[1, 2], &want(&[1]), t0 + Duration::from_secs(15), grace);
        assert_eq!(c, ParkChange { resumed: vec![], expired: vec![2] });
        // closed: the next look sees one session, and the one for 2 is forgotten
        assert_eq!(p.update(&[1], &want(&[1]), t0 + Duration::from_secs(16), grace), nothing);
        // nothing seen anywhere: closed once, after the grace time
        assert_eq!(p.update(&[1], &want(&[]), t0 + Duration::from_secs(20), grace), nothing);
        let c = p.update(&[1], &want(&[]), t0 + Duration::from_secs(25), grace);
        assert_eq!(c.expired, vec![1]);
        // a session that was closed by someone else (a failed monitor) is forgotten without being closed again
        assert_eq!(p.update(&[], &want(&[]), t0 + Duration::from_secs(40), grace), nothing);
    }

    /// The real thing, with no window: a capture session that was not used for a while holds frames from before the pause (the pool
    /// fills up and newer ones are dropped), and on a still desktop nothing newer comes by itself. After `resume` those frames are not
    /// accepted and a new picture arrives within a moment. Needs the capture permission and a monitor; reads the screen, shows nothing.
    #[test]
    fn a_session_that_is_used_again_after_a_pause_never_draws_from_the_frames_queued_before_it() {
        let _quiet = crate::TEST_QUIET.lock().unwrap_or_else(|p| p.into_inner());
        let _apartment = super::super::check::enter_apartment();
        match super::super::check::probe_resume(Duration::from_millis(1500), Duration::from_millis(1500), (0, 0)) {
            Err(e) => println!("not run: {e}"),
            Ok(r) => {
                println!("{r:?}");
                if !r.frame_was_queued {
                    println!("the capture service made no frame for the forced list: nothing to test");
                    return;
                }
                assert!(!r.ready_right_after, "the mirror must not be ready before a picture made after the resume is in");
                assert!(!r.old_frame_accepted, "a frame queued before the pause was taken as the picture");
                assert!(r.fresh_ms.is_some_and(|ms| ms < 1000.0), "a new picture must follow the resume at once: {:?}", r.fresh_ms);
            }
        }
    }

    // --- glass that is seen but not in front ------------------------------------------------------------------------------------------

    /// A window that is frozen is not worked for: not active, no capture kept for it, and the loop waits as it does for a covered one.
    /// Thawing is drawing at once from a fresh backdrop, like coming into view.
    #[test]
    fn a_frozen_window_is_not_worked_for_and_a_thawed_one_is_drawn_at_once_from_a_fresh_backdrop() {
        let mut e = idle_engine(CaptureMode::Session);
        let mut w = settled_window(1, geo_at(0, 0, 400, 300, 1));
        w.freezable = true;
        w.had_picture = true;
        e.wins.insert(1, w);
        assert!(e.active_set().contains(&1));
        assert!(!e.wins.get_mut(&1).unwrap().set_frozen(true), "freezing is not thawing");
        let w = e.wins.get(&1).unwrap();
        assert!(w.frozen && !w.active(), "frozen glass is not worked for");
        assert!(!w.dirty && !w.stale && w.presented.is_some(), "freezing forgets nothing: the picture on the screen stays");
        assert!(e.active_set().is_empty() && !e.paced() && !e.shape_moving());
        assert!(!e.wins.get_mut(&1).unwrap().set_frozen(true), "still frozen: nothing");
        assert!(e.wins.get_mut(&1).unwrap().set_frozen(false), "thawed");
        let w = e.wins.get(&1).unwrap();
        assert!(w.dirty && w.stale && w.presented.is_none() && w.active(), "drawn from a fresh backdrop, and presented whatever the loop guard remembers");
        assert!(!e.wins.get_mut(&1).unwrap().set_frozen(false), "thawed and still thawed: nothing");
    }

    /// Windows that are hidden lose the frozen mark with the rest of what they knew about being seen; a window freezes only when the
    /// app marked it, and only once it has a picture.
    #[test]
    fn only_a_marked_window_with_a_picture_is_to_freeze() {
        let mut e = idle_engine(CaptureMode::Session);
        e.command(Cmd::Attach { hwnd: 1, opts: GlassOptions::window(true) });
        assert!(!e.wins[&1].freezable, "no window freezes unless the app says so (a dock island, a menu)");
        e.command(Cmd::Freezable { hwnd: 1, on: true });
        assert!(e.wins[&1].freezable);
        e.command(Cmd::Freezable { hwnd: 1, on: false });
        assert!(!e.wins[&1].freezable);
        // a window the engine does not know is ignored
        e.command(Cmd::Freezable { hwnd: 99, on: true });
        assert!(!e.wins.contains_key(&99));
        // the setting changing only wakes the loop
        e.command(Cmd::CareChanged);
    }

    /// When no glass that is worked for is in front, the quiet wait is the longer one; one window in front brings back the short one.
    #[test]
    fn the_quiet_wait_is_longer_while_no_glass_is_in_front() {
        let mut e = idle_engine(CaptureMode::Session);
        let mut a = settled_window(1, geo_at(0, 0, 400, 300, 1));
        a.care = Care::Behind;
        e.wins.insert(1, a);
        assert!(e.none_in_front());
        assert_eq!(e.frame_wait_ms(), 100, "twice the 50 ms of a quiet desktop with something in front");
        // a glass window of another process was seen at the last scan: the turn stays as short as the scan (crossproc.rs)
        e.foreign_seen = true;
        assert_eq!(e.frame_wait_ms(), QUIET_WAIT_MS);
        e.foreign_seen = false;
        let mut b = settled_window(2, geo_at(500, 0, 400, 300, 1));
        b.care = Care::Front;
        e.wins.insert(2, b);
        assert!(!e.none_in_front());
        assert_eq!(e.frame_wait_ms(), QUIET_WAIT_MS);
        // a frozen or covered window in front does not count: it is not worked for
        e.wins.get_mut(&2).unwrap().covered = true;
        assert!(e.none_in_front());
        // no glass worked for at all is not "none in front": there is nothing to wait for
        e.wins.get_mut(&1).unwrap().visible = false;
        assert!(!e.none_in_front());
        // Recently active with NOTHING in front (a video plays beside the dock: the picture behind its glass changes all the time):
        // the quiet wait at once. The short wait is for a window that may be dragged, and none of these can be; a frame or a command
        // ends the wait anyway. It was the short wait until 2026-11-14, and the thread turned 125 times a second for as long as
        // the video played.
        let mut e = idle_engine(CaptureMode::Session);
        let mut a = settled_window(1, geo_at(0, 0, 400, 300, 1));
        a.care = Care::Behind;
        e.wins.insert(1, a);
        e.last_activity = Instant::now();
        assert_eq!(e.frame_wait_ms(), QUIET_WAIT_MS);
        // recently active and something IS in front (the user may be dragging it): the short wait
        let mut b = settled_window(2, geo_at(500, 0, 400, 300, 1));
        b.care = Care::Front;
        e.wins.insert(2, b);
        e.last_activity = Instant::now();
        assert_eq!(e.frame_wait_ms(), FRAME_WAIT_MS);
        // Desktop Duplication's wait cannot be ended by a command: it keeps the short wait, in front or not
        let mut d = idle_engine(CaptureMode::Affinity);
        let mut a = settled_window(1, geo_at(0, 0, 400, 300, 1));
        a.care = Care::Behind;
        d.wins.insert(1, a);
        d.last_activity = Instant::now();
        assert_eq!(d.frame_wait_ms(), FRAME_WAIT_MS);
    }

    /// A pyramid is given back only by a window nobody sees, only after the time, and a window that is seen again starts the clock over.
    #[test]
    fn the_idle_clock_runs_only_for_a_window_that_is_not_worked_for() {
        let mut e = idle_engine(CaptureMode::Session);
        e.wins.insert(1, settled_window(1, geo_at(0, 0, 400, 300, 1)));
        let t0 = Instant::now();
        e.trim_idle(t0);
        assert_eq!(e.wins[&1].idle_since, None, "a window that is worked for is never idle");
        e.wins.get_mut(&1).unwrap().covered = true;
        e.trim_idle(t0);
        assert_eq!(e.wins[&1].idle_since, Some(t0));
        e.trim_idle(t0 + Duration::from_secs(3));
        assert_eq!(e.wins[&1].idle_since, Some(t0), "the clock keeps its start");
        e.wins.get_mut(&1).unwrap().covered = false;
        e.trim_idle(t0 + Duration::from_secs(4));
        assert_eq!(e.wins[&1].idle_since, None, "seen again: the clock starts over");
    }

    /// A window that is not in front draws at most 20 times a second even when the capture is fast (another window of its monitor is in
    /// front): it is held back until the gap is over and the loop never sleeps past that moment. A window in front is never held.
    #[test]
    fn a_window_behind_another_is_held_to_its_own_rate_and_the_loop_wakes_for_it() {
        let mut e = idle_engine(CaptureMode::Session);
        e.last_activity = Instant::now();
        let mut behind = settled_window(1, geo_at(0, 0, 400, 300, 1));
        behind.care = Care::Behind;
        behind.dirty = true;
        behind.stale = true;
        behind.last_draw = Some(Instant::now());
        e.wins.insert(1, behind);
        let held = e.draw_wait().expect("a window that drew a moment ago waits");
        assert!(held <= Duration::from_millis(50) && held > Duration::ZERO, "{held:?}");
        assert!(e.frame_wait_ms() <= 50, "the turn does not wait past the moment the window may draw: {}", e.frame_wait_ms());
        // the gap is over: its turn
        e.wins.get_mut(&1).unwrap().last_draw = Some(Instant::now() - Duration::from_millis(60));
        assert_eq!(e.draw_wait(), None);
        // in front: never held
        let w = e.wins.get_mut(&1).unwrap();
        w.last_draw = Some(Instant::now());
        w.care = Care::Front;
        assert_eq!(e.draw_wait(), None);
        // a draw that only moves a lens uses the backdrop it has: never held
        let w = e.wins.get_mut(&1).unwrap();
        w.care = Care::Behind;
        w.stale = false;
        assert_eq!(e.draw_wait(), None);
    }


    /// A window that is not in front and has a new backdrop waiting is held to its own rate; while its glass's SHAPE is being animated
    /// it is not (the islands of a dock that goes thin or grows back): every shape is a frame the UI draws at the display's rate.
    #[test]
    fn a_window_whose_shape_is_moving_is_not_held_back_and_is_again_once_the_shape_rests() {
        let mut e = idle_engine(CaptureMode::Session);
        e.last_activity = Instant::now();
        let mut behind = settled_window(1, geo_at(0, 0, 400, 300, 1));
        behind.care = Care::Behind;
        behind.dirty = true;
        behind.stale = true;
        behind.last_draw = Some(Instant::now());
        e.wins.insert(1, behind);
        let gap = rest::gap_for(rest::BEHIND_FPS);
        assert!(e.draw_wait().is_some() && !e.wins[&1].draw_due(Instant::now(), gap), "held: it drew a moment ago");
        // a new shape arrives (the backdrop is still waiting, the last draw is still a moment ago)
        e.command(Cmd::SetOptions { hwnd: 1, opts: GlassOptions { shape: Some([0.0, 10.0, 400.0, 40.0]), ..GlassOptions::window(true) } });
        let w = &e.wins[&1];
        assert!(w.dirty && w.stale && w.care == Care::Behind);
        assert!(w.draw_due(Instant::now(), gap), "its shape is moving: drawn now");
        assert_eq!(e.draw_wait(), None, "and the loop does not wait for its gap");
        // the shape rests: the window is held to its own rate again
        let w = e.wins.get_mut(&1).unwrap();
        w.shape_changed = Some(Instant::now() - SHAPE_ACTIVE - Duration::from_millis(1));
        w.dirty = true;
        w.last_draw = Some(Instant::now());
        assert!(!e.wins[&1].draw_due(Instant::now(), gap));
        assert!(e.draw_wait().is_some());
    }

    /// A window that moves or is resized is followed at the full rate whoever has the focus, and a frozen one thaws. This is what keeps
    /// a panel that grows, or a window another program drags, from getting a frost at 20 draws a second while its edge moves at 60.
    #[test]
    fn a_window_that_moves_is_in_front_for_a_moment_and_a_frozen_one_thaws() {
        let now = Instant::now();
        let mut w = settled_window(1, geo_at(0, 0, 400, 300, 1));
        w.care = Care::Behind;
        w.dirty = false;
        w.stale = false;
        w.last_draw = Some(now);
        let before = w.care;
        assert_eq!(before, Care::Behind);
        // not moved: held to its own rate when the backdrop is new
        w.restale();
        assert!(!rest::draw_due(w.care, w.stale, Some(Duration::from_millis(5)), rest::gap_for(rest::BEHIND_FPS)));
        // moved
        w.geometry_changed(geo_at(10, 0, 500, 300, 1), now);
        assert_eq!(w.care, Care::Front);
        assert!(w.dirty && w.stale && !w.covered);
        assert_eq!(w.geo, Some(geo_at(10, 0, 500, 300, 1)));
        assert!(rest::draw_due(w.care, w.stale, Some(Duration::from_millis(5)), rest::gap_for(rest::BEHIND_FPS)), "drawn at once, not at the slow rate");
        // the clock the next update_care reads keeps it in front for HOLD
        let (facts, _) = rest::facts_of(0, 0, 0, w.last_front, now + Duration::from_millis(100), false, true);
        assert_eq!(rest::care(&facts), Care::Front);
        let (facts, _) = rest::facts_of(0, 0, 0, w.last_front, now + rest::HOLD, false, true);
        assert_eq!(rest::care(&facts), Care::Behind, "and not for ever");
        // a frozen window that moved thaws, to be drawn at once from a fresh backdrop
        let mut f = settled_window(2, geo_at(0, 0, 400, 300, 1));
        f.freezable = true;
        f.had_picture = true;
        f.set_frozen(true);
        f.care = Care::Frozen;
        assert!(!f.active());
        f.geometry_changed(geo_at(0, 0, 600, 300, 1), now);
        assert!(f.active() && !f.frozen && f.care == Care::Front);
        assert!(f.presented.is_none(), "presented whatever the loop guard remembers");
    }

    /// How long the loop sleeps when no glass is worked for: covered glass is looked at often, glass that is only frozen sleeps long
    /// when the foreground window is watched by an event and as often as covered glass when it is not, hidden glass as before.
    #[test]
    fn the_idle_sleep_depends_on_what_can_wake_the_loop() {
        assert_eq!(idle_wait(true, false, true), COVER_POLL, "covered: nothing tells when it comes into view");
        assert_eq!(idle_wait(true, true, true), COVER_POLL);
        assert_eq!(idle_wait(false, true, true), FROZEN_NET, "frozen: an event thaws it");
        assert_eq!(idle_wait(false, true, false), COVER_POLL, "no event: it is looked at on a clock, as before");
        assert_eq!(idle_wait(false, false, true), IDLE_WAIT);
        assert_eq!(idle_wait(false, false, false), IDLE_WAIT);
        assert!(FROZEN_NET > COVER_POLL && FROZEN_NET <= Duration::from_secs(2));
    }

    /// Where the thread's wakes are counted: by cause, per second, after the statistics period; nothing before it.
    #[test]
    fn the_wake_counters_report_per_second_after_the_period_and_start_over() {
        let t0 = Instant::now();
        let mut t = Turns::default();
        assert!(t.report(t0).is_none(), "the first look only starts the clock");
        t.turns = 500;
        t.frames = 10;
        t.no_frame = 20;
        t.timeouts = 5;
        *t.commands.entry("care").or_default() += 5;
        assert!(t.report(t0 + Duration::from_secs(1)).is_none(), "too early");
        let line = t.report(t0 + STATS_EVERY).expect("due");
        assert!(line.contains("100 turns/s"), "{line}");
        assert!(line.contains("1.0 commands/s (care 1.0)"), "{line}");
        assert!(line.contains("2.0 frame waits/s ended by a frame, 4.0 ended without one, 1.0 command waits/s"), "{line}");
        assert_eq!(t.turns, 0);
        assert!(t.commands.is_empty());
        assert!(t.report(t0 + STATS_EVERY + Duration::from_secs(1)).is_none());
    }

    /// A command is counted by its name.
    #[test]
    fn a_command_is_counted_by_name_and_a_care_change_changes_nothing_else() {
        let mut e = idle_engine(CaptureMode::Session);
        e.command(Cmd::CareChanged);
        e.command(Cmd::CareChanged);
        e.command(Cmd::LookChanged);
        assert_eq!(e.turns.commands.get("care"), Some(&2));
        assert_eq!(e.turns.commands.get("look"), Some(&1));
    }
}
