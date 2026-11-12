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
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGISwapChain1,
};
use windows::Win32::Graphics::Gdi::{ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsIconic, IsWindow, IsWindowVisible, IsZoomed};
use windows::core::{BOOL, Interface};

use super::capture::{Dirty, MonError, Monitor, Pumped, is_device_lost, locate};
use super::cover;
use super::geom::Rc;
use super::gpu::{Geometry, Gpu, LensList, Look, Region, Renderer, glass_pass, prep, region_pad, reuse};
use super::lens::LensSet;
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
        }
    }

    /// Whether anybody can see this glass: it is shown and not hidden under other windows. Only an active window is prepared, drawn,
    /// read back and counted when the engine decides what to capture and how long to wait.
    fn active(&self) -> bool {
        self.visible && !self.covered
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

    /// Takes new options. Returns whether the place the glass shows on the screen changed in a way that makes a new look at the z order
    /// worth it right now: the shape of a covered window moved (it may be in view now), or the glass was nothing at all (an island
    /// attached with an empty shape, which is never "covered") and now has an area.
    fn set_options(&mut self, opts: GlassOptions) -> bool {
        let before = GlassOptions { dark: self.dark, radius_px: self.radius_px, cut_corners: self.cut_corners, shape: self.shape, shadow: self.shadow };
        if before == opts {
            return false;
        }
        let relook = before.shape != opts.shape && (self.covered || shape_is_empty(before.shape));
        let cheap = before.only_the_shape_differs(&opts);
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
    let look = Look::regular(w.dark, &super::look()).sized(glass_w.min(glass_h) / geo.scale());
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
    /// The foreground window's rectangle at the last look at the z order, while some glass was covered: a covering window that is
    /// dragged or resized away changes no foreground window, but it does change this.
    last_fg_rect: Option<Rc>,
    /// Capture sessions that cannot be opened again are tried again after a pause (see `Reopen`).
    reopen: Reopen,
    /// A capture session was opened in this run, so the backend works here and a failure to open one again is probably a transient one.
    session_worked: bool,
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
            last_access_check: Instant::now(),
            access_job: None,
            instant_flushes: 0,
            power: wgc::read_power(),
            last_power_check: Instant::now(),
            parked: Parking::default(),
            last_foreground: 0,
            last_cover_check: Instant::now(),
            cover_due: true,
            last_fg_rect: None,
            reopen: Reopen::default(),
            session_worked: false,
        }
    }

    fn command(&mut self, c: Cmd) {
        self.last_activity = Instant::now();
        match c {
            Cmd::Attach { hwnd, opts } => {
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
        }
    }

    /// How long this turn may wait for a frame.
    fn frame_wait_ms(&self) -> u32 {
        if self.paced() {
            0 // the display paces the turn (`pace`); a frame that is not there yet is taken next turn
        } else if self.shape_moving() {
            SHAPE_WAIT_MS
        } else if self.last_activity.elapsed() > QUIET_AFTER {
            quiet_wait_ms(self.mode, self.wins.values().any(|w| w.active() && w.lenses.occupied()))
        } else {
            FRAME_WAIT_MS
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
        self.wins.values().any(|w| w.active() && w.shape_changed.is_some_and(|t| t.elapsed() < SHAPE_ACTIVE))
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
        } else if !self.wins.values().any(|w| w.active()) {
            // Nobody can see any glass. A window that is only covered is looked at often (it may come into view any moment); one that
            // is hidden is waited for as before.
            let wait = if self.wins.values().any(|w| w.visible && w.covered) { COVER_POLL } else { IDLE_WAIT };
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
        if self.wins.values().any(|w| w.geo.is_none() || !w.visible) {
            self.measure_all();
        }
        self.update_cover();
        self.sync_monitors();
        let waited = self.pump();
        // The window may have moved while we waited for the frame. A covered window that moved is in view (its owner moved it) and so
        // active now, while its monitor's capture may still be parked: it is resumed before anything is drawn from it, and the turn does
        // not wait (the new frame is asked for by the resume).
        let seen_before = self.active_set();
        self.measure_all();
        let appeared = !self.active_set().is_subset(&seen_before);
        if appeared {
            self.sync_monitors();
        }
        self.advance_lenses();
        self.draw_dirty();
        self.housekeeping();
        if self.paced() {
            return self.pace();
        }
        if !waited && !appeared && self.wins.values().any(|w| w.active()) {
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
                        w.covered = false;
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
                            w.geo = Some(g);
                            w.restale();
                            // A window that moves is in view (its owner is dragging it), and it reads a new part of the screen at
                            // every step.
                            w.covered = false;
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
        match wgc::pick_backend(wgc::access(), || unsafe { Monitor::open_session(&gfx.r.g, hmon, power) }) {
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
        let now = Instant::now();
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
                    self.use_affinity(&why, true);
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
        let todo: Vec<isize> = self.wins.values().filter(|w| w.active() && w.dirty).map(|w| w.hwnd).collect();
        for k in todo {
            let Some(gfx) = self.gfx.as_ref() else { return };
            let Some(w) = self.wins.get_mut(&k) else { continue };
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
}
