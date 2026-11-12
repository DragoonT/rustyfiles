//! The engine thread: one for the whole process, started by the first `attach`. It owns the D3D11 device, the DirectComposition device,
//! one swap chain per glass window and one screen capture per monitor that has a visible glass window on it (a Windows.Graphics.Capture
//! session, or Desktop Duplication: see `capture.rs`). Every D3D, DXGI, WinRT and DirectComposition object is created and used here and
//! nowhere else.
//!
//! Where the glass goes: GPUI creates its DirectComposition target on the window with `topmost = true`. This engine adds a second
//! target on the same window with `topmost = false`, whose visual tree is composed BEHIND the scene GPUI draws. The app makes the
//! GPUI window transparent, so the glass shows through wherever the scene is.
//!
//! The loop never spins: it waits on the command channel when there is nothing to poll, waits inside AcquireNextFrame (or on the session's
//! frame token) when it has a capture, and sleeps when neither happened. While a lens moves or fades it is paced by the display instead
//! (`pace`): one turn per refresh, and none of the other waits.

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
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGISwapChain1,
};
use windows::Win32::Graphics::Gdi::{ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsIconic, IsWindow, IsWindowVisible, IsZoomed};
use windows::core::{BOOL, Interface};

use super::capture::{Dirty, MonError, Monitor, Pumped, is_device_lost, locate};
use super::gpu::{Geometry, Gpu, LensList, Look, Region, Renderer, glass_pass, prep, region_pad, reuse};
use super::lens::LensSet;
use super::wgc::{self, Access, Exclusion};
use super::{CaptureMode, Cmd, GlassOptions, GlassState, set_mode};

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
/// A DwmFlush that returns faster than this did not wait for a composition at all.
const INSTANT_FLUSH: Duration = Duration::from_micros(500);
/// This many such returns in a row and `pace` waits on the channel instead, so a broken DwmFlush cannot make a busy loop.
const INSTANT_FLUSHES_BEFORE_WAIT: u32 = 3;
const STATS_EVERY: Duration = Duration::from_secs(5);
const DEVICE_CHECK_EVERY: Duration = Duration::from_secs(1);
/// How often a running session backend looks again at the permission it was started on (reading it is a cheap, silent query).
const ACCESS_CHECK_EVERY: Duration = Duration::from_secs(10);

/// The wait of a turn on a quiet desktop. Only Desktop Duplication needs the shorter one while a lens is held (see `LENS_WAIT_MS`).
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

unsafe fn measure(hwnd: isize) -> Seen {
    let h = hwnd_of(hwnd);
    if !IsWindow(Some(h)).as_bool() {
        return Seen::Gone;
    }
    if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
        return Seen::Hidden;
    }
    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, size_of::<u32>() as u32).is_ok() && cloaked != 0 {
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
    // What the system does with the corners: the attribute does not exist before Windows 11 (the call fails: square corners there).
    let mut pref = DWMWCP_DEFAULT;
    let round = if DwmGetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &mut pref as *mut _ as *mut c_void, size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32).is_err() || pref == DWMWCP_DONOTROUND {
        0.0
    } else if pref == DWMWCP_ROUNDSMALL {
        WINDOW_RADIUS_SMALL
    } else {
        WINDOW_RADIUS
    };
    Seen::Shown(WinGeo { x: origin.x, y: origin.y, w, h: ht, dpi, square: covers || IsZoomed(h).as_bool(), round, hmon: hmon.0 as isize })
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
    /// Draws that kept the backdrop of the last one (only a lens moved) and so made no copy, no pyramid and no read-back.
    reused: u32,
    prep: Duration,
    glass: Duration,
}

impl Stats {
    fn new(now: Instant) -> Stats {
        Stats { since: now, frames: 0, with_image: 0, presented: 0, skipped: 0, reused: 0, prep: Duration::ZERO, glass: Duration::ZERO }
    }
}

struct Win {
    hwnd: isize,
    dark: bool,
    /// The radius the app asked for, device px (`None`: what the system does, see `effective_radius`).
    radius_px: Option<f32>,
    /// The app asked for the glass to cut its own corners (a shape that is not a window).
    cut_corners: bool,
    visible: bool,
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
}

impl Win {
    fn new(hwnd: isize, opts: GlassOptions) -> Win {
        Win {
            hwnd,
            dark: opts.dark,
            radius_px: opts.radius_px,
            cut_corners: opts.cut_corners,
            visible: false,
            geo: None,
            target: None,
            region: None,
            dirty: true,
            presented: None,
            stats: Stats::new(Instant::now()),
            lenses: LensSet::new(),
            lens_tick: None,
            stale: true,
        }
    }

    fn set_options(&mut self, opts: GlassOptions) {
        self.dark = opts.dark;
        self.radius_px = opts.radius_px;
        self.cut_corners = opts.cut_corners;
        self.restale();
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
    let g = Geometry {
        w: geo.w,
        h: geo.h,
        origin: (geo.x - mrect.left, geo.y - mrect.top),
        scale: geo.scale(),
        radius_px: effective_radius(w.radius_px, geo.radius_px(), geo.w, geo.h),
        cut_corners: w.cut_corners,
    };
    // The settings' look; a shape below 160 CSS px gets the calmer rim (a window never does, a dock island does). The look is part of
    // what the loop guard compares, so a change of the settings (or of the theme) is drawn.
    let look = Look::regular(w.dark, &super::look()).sized(geo.w.min(geo.h) as f32 / geo.scale());
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
            let read = region.read_back(&gfx.r.g, g.w, g.h).map_err(|e| ctx("reading the backdrop back", e))?;
            // the same bytes tell how bright the backdrop under the glass is (`backdrop_luma`); no extra pass, copy or wait
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
    if first_draw {
        // Running means "the first frame is presented", not "the swap chain exists": a window that drops its own fill on Running (a popup
        // menu, the dock's island, the main window turning see-through) would otherwise be empty for the time the first frame takes.
        super::set_state(w.hwnd, GlassState::Running);
    }
    Ok(Drawn::Presented)
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
    last_access_check: Instant,
    /// A permission check running on its own thread (see `access_tick`).
    access_job: Option<Receiver<Access>>,
    /// How many `DwmFlush` calls in a row returned without waiting (see `pace`).
    instant_flushes: u32,
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
    let mut e = Engine {
        rx,
        gfx: None,
        wins: BTreeMap::new(),
        mons: BTreeMap::new(),
        stats_on,
        last_device_check: Instant::now(),
        last_activity: Instant::now(),
        mode: CaptureMode::Off,
        excl: Exclusion::default(),
        last_access_check: Instant::now(),
        access_job: None,
        instant_flushes: 0,
    };
    while e.step() {}
    e.drop_all();
}

impl Engine {
    fn command(&mut self, c: Cmd) {
        self.last_activity = Instant::now();
        match c {
            Cmd::Attach { hwnd, opts } => {
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
                if let Some(w) = self.wins.get_mut(&hwnd) {
                    w.set_options(opts);
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
        }
    }

    /// How long this turn may wait for a frame.
    fn frame_wait_ms(&self) -> u32 {
        if self.lens_active() {
            0 // the display paces the turn (`pace`); a frame that is not there yet is taken next turn
        } else if self.last_activity.elapsed() > QUIET_AFTER {
            quiet_wait_ms(self.mode, self.wins.values().any(|w| w.visible && w.lenses.occupied()))
        } else {
            FRAME_WAIT_MS
        }
    }

    /// Whether a lens of a visible window is moving or fading: the engine then draws once per display refresh.
    fn lens_active(&self) -> bool {
        self.wins.values().any(|w| w.visible && w.lenses.animating())
    }

    /// Steps the lenses of the visible windows to now and marks them dirty. The step is the time since the last one, so the motion does not
    /// depend on how often the loop turns; `LensSet::advance` cuts a step after a stall.
    fn advance_lenses(&mut self) {
        let now = Instant::now();
        for w in self.wins.values_mut() {
            if !w.visible || !w.lenses.animating() {
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
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
        true
    }

    /// One turn of the loop. False ends the thread.
    fn step(&mut self) -> bool {
        if self.wins.is_empty() {
            // Nothing attached: no device, no capture, and nothing to do until a command arrives.
            self.release_gfx();
            match self.rx.recv() {
                Ok(c) => self.command(c),
                Err(_) => return false,
            }
        } else if !self.wins.values().any(|w| w.visible) && !self.wait_command(IDLE_WAIT) {
            return false;
        }
        while let Ok(c) = self.rx.try_recv() {
            self.command(c);
        }
        if self.wins.is_empty() {
            return true;
        }
        // The look after the wait of the last turn is still good when every window is visible and measured; only windows that are not
        // yet known or are hidden (they may have come back) need a look now.
        if self.wins.values().any(|w| w.geo.is_none() || !w.visible) {
            self.measure_all();
        }
        self.sync_monitors();
        let waited = self.pump();
        self.measure_all(); // the window may have moved while we waited for the frame
        self.advance_lenses();
        self.draw_dirty();
        self.housekeeping();
        if self.lens_active() {
            return self.pace();
        }
        if !waited && self.wins.values().any(|w| w.visible) {
            // no AcquireNextFrame wait happened this turn (a duplication is being rebuilt, or failed and backs off): never spin
            return self.wait_command(Duration::from_millis(self.frame_wait_ms() as u64));
        }
        true
    }

    /// Looks at every window: gone ones are dropped, the others get their geometry and are marked dirty when it changed.
    fn measure_all(&mut self) {
        let mut moved = false;
        let keys: Vec<isize> = self.wins.keys().copied().collect();
        for k in keys {
            match unsafe { measure(k) } {
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
                    }
                }
                Seen::Shown(g) => {
                    // a window listed while it was hidden may not be honoured once it shows: the list is sent again
                    self.excl.set_visible(k, true);
                    if let Some(w) = self.wins.get_mut(&k) {
                        if !w.visible {
                            w.visible = true;
                            w.restale();
                            w.presented = None; // it may have been off screen a long while
                            moved = true;
                        }
                        if w.geo != Some(g) {
                            w.geo = Some(g);
                            w.restale();
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

    /// One capture per monitor that has a visible window; none for the others. The first one made also decides the backend.
    fn sync_monitors(&mut self) {
        let want: BTreeSet<isize> = self.wins.values().filter(|w| w.visible).filter_map(|w| w.geo.map(|g| g.hmon)).collect();
        self.mons.retain(|k, _| want.contains(k));
        for hmon in want {
            if self.mons.contains_key(&hmon) {
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
        match wgc::pick_backend(wgc::access(), || unsafe { Monitor::open_session(&gfx.r.g, hmon) }) {
            Ok(m) => {
                self.mons.insert(hmon, m);
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
            Err(why) => self.use_affinity(&why, false),
        }
    }

    /// Desktop Duplication with the display-affinity exclusion from now on: the flag goes on every window of ours that must stay out of the
    /// picture BEFORE any capture is made, and the sessions (if there are any) are closed. `fell_back`: a session was in use and failed.
    fn use_affinity(&mut self, why: &str, fell_back: bool) {
        if fell_back {
            log::info!("glass: the capture session failed ({why}); falling back to Desktop Duplication, rustyfiles windows are hidden from screenshots and recordings");
        } else {
            log::info!("glass: screen capture through Desktop Duplication, rustyfiles windows are hidden from screenshots and recordings ({why})");
        }
        self.mode = CaptureMode::Affinity;
        set_mode(CaptureMode::Affinity);
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
            let opened = self.gfx.as_ref().map(|gfx| unsafe { Monitor::open_session(&gfx.r.g, hmon) });
            match opened {
                Some(Ok(m)) => {
                    self.mons.insert(hmon, m);
                    return;
                }
                Some(Err(why)) => self.use_affinity(&why, true),
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

    /// Takes the frames the monitors have. Returns whether a wait for a frame happened (the turn did not spin).
    fn pump(&mut self) -> bool {
        let Some(gfx) = self.gfx.as_ref() else { return false };
        if self.mode == CaptureMode::Session {
            // menus and flyouts show without telling the engine
            self.excl.poll_extras(wgc::probe_window);
        }
        let mut budget = self.frame_wait_ms(); // the first monitor that is waited on waits, the others only look
        // A frame on another monitor ends that wait too, or a long quiet wait (`quiet_wait_ms`) would delay the notice of it by as much.
        // With one monitor this is an empty list and costs nothing.
        let others: Vec<Receiver<()>> = if self.mons.len() > 1 { self.mons.values().filter_map(|m| m.wake().cloned()).collect() } else { Vec::new() };
        let mut waited = false;
        let mut errors: Vec<(isize, MonError)> = vec![];
        let mut fresh: Vec<(isize, Dirty)> = vec![];
        for (&hmon, m) in self.mons.iter_mut() {
            match unsafe { m.pump(&gfx.r.g, budget, &self.excl, Some(&self.rx), &others) } {
                Ok(Pumped::NoWait) => {}
                Ok(Pumped::Waited { frame, image, dirty }) => {
                    if budget > 0 {
                        waited = true;
                        budget = 0;
                    }
                    for w in self.wins.values_mut().filter(|w| w.geo.is_some_and(|g| g.hmon == hmon)) {
                        w.stats.frames += frame as u32;
                        w.stats.with_image += image as u32;
                    }
                    if frame {
                        self.last_activity = Instant::now();
                    }
                    if image {
                        fresh.push((hmon, dirty));
                    }
                }
                Err(e) => errors.push((hmon, e)),
            }
        }
        // only the windows whose backdrop (their rectangle and the padding the glass reads around it) was touched are drawn again
        for (hmon, dirty) in fresh {
            let Some(r) = self.mons.get(&hmon).map(|m| m.rect()) else { continue };
            for w in self.wins.values_mut() {
                let Some(g) = w.geo.filter(|g| g.hmon == hmon) else { continue };
                if dirty.touches(g.x - r.left, g.y - r.top, g.w, g.h, region_pad(g.scale())) {
                    w.restale();
                }
            }
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
                    self.use_affinity(&why, true);
                    return waited;
                }
            }
        }
        waited
    }

    fn draw_dirty(&mut self) {
        let todo: Vec<isize> = self.wins.values().filter(|w| w.visible && w.dirty).map(|w| w.hwnd).collect();
        for k in todo {
            let Some(gfx) = self.gfx.as_ref() else { return };
            let Some(w) = self.wins.get_mut(&k) else { continue };
            let Some(mon) = w.geo.and_then(|g| self.mons.get(&g.hmon)) else { continue };
            if !mon.has_image() {
                continue; // dirty stays; drawn as soon as the first picture of the screen is in
            }
            match unsafe { draw(gfx, mon, w) } {
                Ok(_) => w.dirty = false,
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
        if self.stats_on {
            for w in self.wins.values_mut() {
                let s = &mut w.stats;
                if now.duration_since(s.since) >= STATS_EVERY {
                    let draws = (s.presented + s.skipped).max(1) as f64;
                    log::info!(
                        "glass {:#x}: {} frames acquired, {} with an image, {} draws presented, {} skipped by the loop guard ({} draws only moved a lens), prep {:.2} ms, glass pass {:.2} ms (CPU time per draw)",
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
            self.use_affinity(&format!("the permission was withdrawn: {why}"), true);
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
        let victims: Vec<isize> = self.wins.values().filter(|w| w.visible && w.geo.is_some_and(|g| g.hmon == hmon)).map(|w| w.hwnd).collect();
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
        self.gfx = None;
        self.access_job = None;
        self.mode = CaptureMode::Off;
        set_mode(CaptureMode::Off);
        self.excl = Exclusion::default();
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
        let mut e = Engine {
            rx,
            gfx: None,
            wins: BTreeMap::new(),
            mons: BTreeMap::new(),
            stats_on: false,
            last_device_check: Instant::now(),
            last_activity: Instant::now(),
            mode: CaptureMode::Off,
            excl: Exclusion::default(),
            last_access_check: Instant::now(),
            access_job: None,
            instant_flushes: 0,
        };
        for hwnd in [1isize, 2] {
            let mut w = Win::new(hwnd, GlassOptions::window(hwnd == 1));
            w.dirty = false;
            w.stale = false;
            e.wins.insert(hwnd, w);
        }
        e.command(Cmd::LookChanged);
        assert!(e.wins.values().all(|w| w.dirty && w.stale), "a window was not marked by the look change");
    }

    /// The look is part of what the loop guard compares: the same picture with another look is not skipped.
    #[test]
    fn the_loop_guard_sees_a_new_look() {
        let geo = Geometry { w: 400, h: 300, origin: (0, 0), scale: 1.0, radius_px: 8.0, cut_corners: false };
        let lenses = LensList::default();
        let a = Presented { hash: 7, geo, look: Look::regular(false, &LookParams::DEFAULT), lenses };
        let b = Presented { look: Look::regular(false, &LookParams { frost: 80.0, ..LookParams::DEFAULT }), ..a };
        let same = a;
        assert!(same == a && a != b);
    }

    fn idle_engine(mode: CaptureMode) -> Engine {
        let (_tx, rx) = crossbeam_channel::unbounded();
        Engine {
            rx,
            gfx: None,
            wins: BTreeMap::new(),
            mons: BTreeMap::new(),
            stats_on: false,
            last_device_check: Instant::now(),
            // quiet for longer than QUIET_AFTER
            last_activity: Instant::now().checked_sub(QUIET_AFTER * 2).unwrap_or_else(Instant::now),
            mode,
            excl: Exclusion::default(),
            last_access_check: Instant::now(),
            access_job: None,
            instant_flushes: 0,
        }
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
}
