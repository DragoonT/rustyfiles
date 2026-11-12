//! The engine thread: one for the whole process, started by the first `attach`. It owns the D3D11 device, the DirectComposition device,
//! one swap chain per glass window and one screen duplication per monitor that has a visible glass window on it. Every D3D, DXGI and
//! DirectComposition object is created and used here and nowhere else.
//!
//! Where the glass goes: GPUI creates its DirectComposition target on the window with `topmost = true`. This engine adds a second
//! target on the same window with `topmost = false`, whose visual tree is composed BEHIND the scene GPUI draws. The app makes the
//! GPUI window transparent, so the glass shows through wherever the scene is.
//!
//! The loop never spins: it waits on the command channel when there is nothing to poll, waits inside AcquireNextFrame when it has a
//! duplication, and sleeps when neither happened.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use windows::Win32::Foundation::{HWND, LUID, POINT, RECT};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::DirectComposition::{DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_CLOAKED, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT, DWMWCP_DONOTROUND, DWMWCP_ROUNDSMALL, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGISwapChain1,
};
use windows::Win32::Graphics::Gdi::{ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsIconic, IsWindow, IsWindowVisible, IsZoomed};
use windows::core::{BOOL, Interface};

use super::capture::{MonError, Monitor, Pumped, is_device_lost, locate};
use super::gpu::{Geometry, Gpu, Look, Region, Renderer, glass_pass, prep, region_pad};
use super::{Cmd, GlassOptions, GlassState};

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
const STATS_EVERY: Duration = Duration::from_secs(5);
const DEVICE_CHECK_EVERY: Duration = Duration::from_secs(1);

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
}

struct Stats {
    since: Instant,
    frames: u32,
    with_image: u32,
    presented: u32,
    skipped: u32,
    prep: Duration,
    glass: Duration,
}

impl Stats {
    fn new(now: Instant) -> Stats {
        Stats { since: now, frames: 0, with_image: 0, presented: 0, skipped: 0, prep: Duration::ZERO, glass: Duration::ZERO }
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
}

impl Win {
    fn new(hwnd: isize, opts: GlassOptions) -> Win {
        Win { hwnd, dark: opts.dark, radius_px: opts.radius_px, cut_corners: opts.cut_corners, visible: false, geo: None, target: None, region: None, dirty: true, presented: None, stats: Stats::new(Instant::now()) }
    }
}

impl Win {
    fn set_options(&mut self, opts: GlassOptions) {
        self.dark = opts.dark;
        self.radius_px = opts.radius_px;
        self.cut_corners = opts.cut_corners;
        self.dirty = true;
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
    let mirror = mon.mirror.as_ref().ok_or_else(|| Fail::Window("no mirror of the monitor".into()))?;

    // the swap chain: made on the first draw, resized when the client area changed
    match &mut w.target {
        None => {
            w.target = Some(Target::new(gfx, w.hwnd, geo.w, geo.h)?);
            super::set_state(w.hwnd, GlassState::Running);
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
    }
    let (Some(region), Some(target)) = (w.region.as_ref(), w.target.as_ref()) else { return Err(Fail::Window("lost its resources".into())) };

    let g = Geometry {
        w: geo.w,
        h: geo.h,
        origin: (geo.x - mon.rect.left, geo.y - mon.rect.top),
        scale: geo.scale(),
        radius_px: effective_radius(w.radius_px, geo.radius_px(), geo.w, geo.h),
        cut_corners: w.cut_corners,
    };
    let look = Look::regular(w.dark);
    let t0 = Instant::now();
    let prepared = prep(&gfx.r, region, mirror, &g, &look, false);
    // THE LOOP GUARD. Our own Present can make the compositor produce a new duplication frame whose picture (without our excluded
    // window) is unchanged; drawing for it would keep the engine busy at the refresh rate for ever. So a coarse level of the backdrop is
    // read back, and when it, the geometry and the look all equal the last presented state, nothing is drawn.
    let hash = region.fingerprint(&gfx.r.g, g.w, g.h).map_err(|e| ctx("reading the backdrop back", e))?;
    let t1 = Instant::now();
    w.stats.prep += t1 - t0;
    let now = Presented { hash, geo: g, look };
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
}

/// The engine thread's body. Returns when the command channel is closed.
pub(super) fn run(rx: Receiver<Cmd>) {
    unsafe {
        // window and monitor rectangles must come back in device pixels whatever the process was started as
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let stats_on = std::env::var("RUSTYFILES_GLASS_STATS").is_ok_and(|v| v == "1");
    let mut e = Engine { rx, gfx: None, wins: BTreeMap::new(), mons: BTreeMap::new(), stats_on, last_device_check: Instant::now(), last_activity: Instant::now() };
    while e.step() {}
    e.drop_all();
}

impl Engine {
    fn command(&mut self, c: Cmd) {
        self.last_activity = Instant::now();
        match c {
            Cmd::Attach { hwnd, opts } => {
                // The exclusion is set by the caller already; set again here because a Detach of an earlier attach of the same window
                // may have been handled since (it lifts the exclusion) and this command comes after it in the queue.
                let _ = super::set_affinity(hwnd, true);
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
                    w.dirty = true;
                }
            }
            Cmd::Detach { hwnd } => {
                if let Some(w) = self.wins.remove(&hwnd) {
                    self.teardown(w);
                    // only now may the window be captured again: its target is off it
                    let _ = super::set_affinity(hwnd, false);
                }
            }
        }
    }

    /// How long this turn may wait for a frame.
    fn frame_wait_ms(&self) -> u32 {
        if self.last_activity.elapsed() > QUIET_AFTER { QUIET_WAIT_MS } else { FRAME_WAIT_MS }
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
            // Nothing attached: no device, no duplication, and nothing to do until a command arrives.
            self.gfx = None;
            self.mons.clear();
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
        self.draw_dirty();
        self.housekeeping();
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
                    super::forget(k);
                }
                Seen::Hidden => {
                    if let Some(w) = self.wins.get_mut(&k) {
                        moved |= w.visible;
                        w.visible = false;
                    }
                }
                Seen::Shown(g) => {
                    if let Some(w) = self.wins.get_mut(&k) {
                        if !w.visible {
                            w.visible = true;
                            w.dirty = true;
                            w.presented = None; // it may have been off screen a long while
                            moved = true;
                        }
                        if w.geo != Some(g) {
                            w.geo = Some(g);
                            w.dirty = true;
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

    /// One duplication per monitor that has a visible window; none for the others.
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
            let Some(luid) = self.gfx.as_ref().map(|g| g.luid) else { continue };
            match unsafe { Monitor::open(hmon, luid) } {
                Ok(m) => {
                    self.mons.insert(hmon, m);
                }
                Err(why) => self.fail_on_monitor(hmon, &why),
            }
        }
    }

    /// Takes the frames the monitors have. Returns whether an AcquireNextFrame wait happened (the turn did not spin).
    fn pump(&mut self) -> bool {
        let Some(gfx) = self.gfx.as_ref() else { return false };
        let mut budget = self.frame_wait_ms(); // the first monitor that is acquired waits, the others only look
        let mut waited = false;
        let mut errors: Vec<(isize, MonError)> = vec![];
        let mut fresh: Vec<isize> = vec![];
        for (&hmon, m) in self.mons.iter_mut() {
            match unsafe { m.pump(&gfx.r.g, budget) } {
                Ok(Pumped::NoWait) => {}
                Ok(Pumped::Waited { frame, image }) => {
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
                        fresh.push(hmon);
                    }
                }
                Err(e) => errors.push((hmon, e)),
            }
        }
        for w in self.wins.values_mut().filter(|w| w.geo.is_some_and(|g| fresh.contains(&g.hmon))) {
            w.dirty = true;
        }
        for (hmon, e) in errors {
            match e {
                MonError::Device(why) => {
                    self.fail_all(&why);
                    return waited;
                }
                MonError::Monitor(why) => self.fail_on_monitor(hmon, &why),
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
            if !mon.has_image {
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
                        "glass {:#x}: {} frames acquired, {} with an image, {} draws presented, {} skipped by the loop guard, prep {:.2} ms, glass pass {:.2} ms (CPU time per draw)",
                        w.hwnd,
                        s.frames,
                        s.with_image,
                        s.presented,
                        s.skipped,
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
    }

    /// Takes the glass off a window that leaves the engine.
    fn teardown(&self, w: Win) {
        if let (Some(t), Some(gfx)) = (&w.target, &self.gfx) {
            unsafe { t.remove(gfx) };
        }
    }

    /// A window falls back: its glass is taken off, the capture exclusion lifted, the state says why. The app then shows Mica.
    fn fail_window(&mut self, hwnd: isize, why: &str) {
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
        self.mons.clear();
        self.gfx = None;
    }
}
