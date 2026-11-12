//! The glass's screen capture through Windows.Graphics.Capture, the backend that does not hide our windows from the rest of the system.
//!
//! WHY. The glass has to capture the screen WITHOUT its own windows, or it would bend its own picture. `WDA_EXCLUDEFROMCAPTURE` does that
//! but for every capture of the screen: screenshots, recordings and screen sharing lose the windows too. A capture session made from a
//! display item can carry its own exclusion list (`IDisplayGraphicsCaptureSession::SetWindowExclusionList`): the windows in it are left
//! out of THIS capture only, and no window flag is set. The interface exists from Windows 11 24H2 (build 26100) and is in neither the
//! windows crate nor the SDK headers, so it is declared by hand below (IID and vtable order read from Windows.Graphics.winmd).
//!
//! WHEN IT IS USED. Only when the app may capture without the yellow border (`AppCapability("graphicsCaptureWithoutBorder")` reads
//! `Allowed`, a state that is only READ here: nothing asks the user, and nothing is shown) and a session on the first monitor can be made
//! and offers the exclusion list. Otherwise the engine uses Desktop Duplication with the display-affinity exclusion, as before.
//!
//! HOW. Per monitor: a free-threaded frame pool of two buffers on the engine's own device (frames are textures on our device, no CPU
//! copy), a session with the border and the cursor off. The `FrameArrived` handler only drops a token into a channel; the engine thread
//! waits on that channel (never spins), takes every frame the pool has, keeps the newest one that was rendered with the current exclusion
//! list and copies it into the monitor's mirror texture. A frame whose configuration iteration is below the number the last
//! `SetWindowExclusionList` returned was rendered with the OLD list (our window may be in it) and is dropped.

#![allow(non_snake_case)] // the hand-declared interfaces keep the names of the WinRT methods

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Select, bounded};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureDirtyRegionMode, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Security::Authorization::AppCapabilityAccess::{AppCapability, AppCapabilityAccessStatus};
use windows::UI::WindowId;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Multithread, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow, IsWindowVisible};
use windows::core::{HSTRING, IInspectable, Interface, PCWSTR};
use windows_collections::IIterable;

use super::Cmd;
use super::capture::{Dirty, MonError, Pumped, is_device_lost};
use super::gpu::{Gpu, Source};

type R<T> = windows::core::Result<T>;

// ---------------------------------------------------------------------------------------------------------------------------------
// Interfaces the windows crate does not have. Both extend IInspectable, which is declared by hand here as three methods after IUnknown's.
// ---------------------------------------------------------------------------------------------------------------------------------

/// Windows.Graphics.Capture.IDisplayGraphicsCaptureSession (Windows 11 24H2). Slot 6: `UINT64 SetWindowExclusionList(IIterable<WindowId>)`,
/// slot 7: `IVectorView<WindowId> GetWindowExclusionList()`.
#[windows::core::interface("bb91f61b-218a-587d-8580-2701a74c0525")]
unsafe trait IDisplayGraphicsCaptureSession: windows::core::IUnknown {
    fn GetIids(&self, count: *mut u32, iids: *mut *mut windows::core::GUID) -> windows::core::HRESULT;
    fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> windows::core::HRESULT;
    fn GetTrustLevel(&self, level: *mut i32) -> windows::core::HRESULT;
    fn SetWindowExclusionList(&self, windows: *mut c_void, result: *mut u64) -> windows::core::HRESULT;
    fn GetWindowExclusionList(&self, result: *mut *mut c_void) -> windows::core::HRESULT;
}

/// Windows.Graphics.Capture.IDirect3D11CaptureFrame3. Slot 6: `UINT64 get_ConfigurationIteration()`.
#[windows::core::interface("71616dc8-fea5-5741-a3d8-591acc39a9ee")]
unsafe trait IDirect3D11CaptureFrame3: windows::core::IUnknown {
    fn GetIids(&self, count: *mut u32, iids: *mut *mut windows::core::GUID) -> windows::core::HRESULT;
    fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> windows::core::HRESULT;
    fn GetTrustLevel(&self, level: *mut i32) -> windows::core::HRESULT;
    fn ConfigurationIteration(&self, result: *mut u64) -> windows::core::HRESULT;
}

// ---------------------------------------------------------------------------------------------------------------------------------
// The permission, and the choice it feeds
// ---------------------------------------------------------------------------------------------------------------------------------

/// What the system says about capturing without the border.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Access {
    Allowed,
    /// The system says no: the reason, for the log. This is the only answer that makes a running session give up.
    Not(String),
    /// The permission could not be read just now (the query failed): not `Allowed`, so no session is started on it, but not a "no"
    /// either, so a session that is running is left alone.
    Unreadable(String),
}

impl Access {
    /// Why the border-free capture cannot be used now, `None` when it is allowed.
    pub(super) fn why_not(&self) -> Option<&str> {
        match self {
            Access::Allowed => None,
            Access::Not(w) | Access::Unreadable(w) => Some(w),
        }
    }
}

/// Reads `graphicsCaptureWithoutBorder`. READ only (`CheckAccess`): `RequestAccessAsync` may put a consent prompt on the screen and is
/// never called.
pub(super) fn access() -> Access {
    let status = AppCapability::Create(&HSTRING::from("graphicsCaptureWithoutBorder")).and_then(|c| c.CheckAccess());
    match status {
        Ok(AppCapabilityAccessStatus::Allowed) => Access::Allowed,
        Ok(s) => {
            let name = match s {
                AppCapabilityAccessStatus::DeniedBySystem => "denied by the system",
                AppCapabilityAccessStatus::NotDeclaredByApp => "not declared by the app",
                AppCapabilityAccessStatus::DeniedByUser => "denied by the user",
                AppCapabilityAccessStatus::UserPromptRequired => "waiting for the user's consent",
                _ => "not allowed",
            };
            Access::Not(format!("capturing without the screenshot border is {name}"))
        }
        Err(e) => Access::Unreadable(format!("the screenshot border permission cannot be read: {e}")),
    }
}

/// The choice of backend. The session backend is wanted when the permission is `Allowed` AND `probe` (a session on the first monitor that
/// offers the exclusion list) succeeds; the probe is not even tried otherwise. `Err` is why not: the engine then uses Desktop Duplication
/// with the display-affinity exclusion.
pub(super) fn pick_backend<T>(access: Access, probe: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match access {
        Access::Allowed => probe(),
        Access::Not(why) | Access::Unreadable(why) => Err(why),
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// The set of windows a session leaves out
// ---------------------------------------------------------------------------------------------------------------------------------

/// What a look at a listed window found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WinProbe {
    /// Not a window of this process any more.
    Gone,
    Hidden,
    Visible,
}

/// Looks at one of our windows.
pub(super) fn probe_window(h: isize) -> WinProbe {
    unsafe {
        let w = HWND(h as *mut c_void);
        let mut pid = 0u32;
        if !IsWindow(Some(w)).as_bool() || GetWindowThreadProcessId(w, Some(&mut pid)) == 0 || pid != GetCurrentProcessId() {
            WinProbe::Gone
        } else if IsWindowVisible(w).as_bool() {
            WinProbe::Visible
        } else {
            WinProbe::Hidden
        }
    }
}

/// The windows every open session leaves out of its picture: all attached glass windows (visible or not) and all windows of the engine's
/// extras registry (menus, flyouts...). `version` changes whenever what must be SENT changes, so a session sends its list when its
/// own record differs and never otherwise (one call is 1 to 2 ms and forces a frame).
///
/// A listed window that was hidden and becomes visible also bumps the version: it is not known whether a handle that was listed while
/// its window was hidden is honoured once the window shows, and sending the list again costs next to nothing.
#[derive(Default)]
pub(super) struct Exclusion {
    attached: BTreeSet<isize>,
    extras: BTreeSet<isize>,
    /// Listed windows last seen visible.
    shown: BTreeSet<isize>,
    version: u64,
}

impl Exclusion {
    fn listed(&self, h: isize) -> bool {
        self.attached.contains(&h) || self.extras.contains(&h)
    }

    pub(super) fn attach(&mut self, h: isize) {
        if self.attached.insert(h) {
            self.version += 1;
        }
    }

    pub(super) fn detach(&mut self, h: isize) {
        if self.attached.remove(&h) {
            if !self.listed(h) {
                self.shown.remove(&h);
            }
            self.version += 1;
        }
    }

    /// Replaces the registry of extra windows.
    pub(super) fn set_extras(&mut self, extras: BTreeSet<isize>) {
        if self.extras != extras {
            self.extras = extras;
            let keep: BTreeSet<isize> = self.shown.iter().copied().filter(|h| self.listed(*h)).collect();
            self.shown = keep;
            self.version += 1;
        }
    }

    /// The caller saw a listed window visible or hidden. A window that goes from hidden (or never seen) to visible makes the list worth
    /// sending again.
    pub(super) fn set_visible(&mut self, h: isize, visible: bool) {
        if !self.listed(h) {
            return;
        }
        if visible {
            if self.shown.insert(h) {
                self.version += 1;
            }
        } else {
            self.shown.remove(&h);
        }
    }

    /// Looks at the extras (they come and go without the engine being told when they show) and keeps the record of who is visible. A
    /// handle that is no longer a window of ours is dropped from the list (the number may be handed to another process's window, which
    /// would then be cut out of the backdrop).
    pub(super) fn poll_extras(&mut self, mut probe: impl FnMut(isize) -> WinProbe) {
        let extras: Vec<isize> = self.extras.iter().copied().collect();
        for h in extras {
            match probe(h) {
                WinProbe::Gone => {
                    self.extras.remove(&h);
                    self.shown.remove(&h);
                    self.version += 1;
                }
                WinProbe::Hidden => self.set_visible(h, false),
                WinProbe::Visible => self.set_visible(h, true),
            }
        }
    }

    pub(super) fn extras(&self) -> &BTreeSet<isize> {
        &self.extras
    }

    pub(super) fn version(&self) -> u64 {
        self.version
    }

    /// The list a session is given: attached windows and extras, each once, in order.
    pub(super) fn list(&self) -> Vec<isize> {
        self.attached.union(&self.extras).copied().collect()
    }
}

/// Whether a frame was rendered with an exclusion list older than the last one sent: its configuration iteration is below the number
/// `SetWindowExclusionList` returned.
pub(super) fn is_stale(frame_iteration: u64, last_set: u64) -> bool {
    frame_iteration < last_set
}

/// What a frame's size says about the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SizeCheck {
    /// The frame has the mirror's size.
    Fine,
    /// The monitor changed its size (resolution, rotation): the pool keeps delivering buffers of its old size, so the pool and the mirror
    /// are made anew and the frame is dropped.
    Recreate,
    /// The frame is of the old size but the pool was made anew already: a frame that was in flight. Dropped.
    Stale,
}

/// Compares a frame (its content size and the size of its texture) with the mirror. `item` is asked only when they differ.
pub(super) fn size_check(content: (i32, i32), texture: (u32, u32), mirror: (u32, u32), item: impl FnOnce() -> (i32, i32)) -> SizeCheck {
    let same = |a: (i32, i32), b: (u32, u32)| a.0 >= 0 && a.1 >= 0 && (a.0 as u32, a.1 as u32) == b;
    if same(content, mirror) && texture == mirror {
        SizeCheck::Fine
    } else if !same(item(), mirror) {
        SizeCheck::Recreate
    } else {
        SizeCheck::Stale
    }
}

/// The shortest time between two frames the session is asked for: one refresh period of the monitor, never below 4 ms (a monitor that
/// does not say its rate gets the session's own default, 16 ms).
pub(super) fn min_interval_ticks(hz: u32) -> i64 {
    let ms = if hz > 1 { 1000.0 / hz as f64 } else { 16.0 };
    (ms.max(4.0) * 10_000.0) as i64
}

// ---------------------------------------------------------------------------------------------------------------------------------
// One session
// ---------------------------------------------------------------------------------------------------------------------------------

const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
const POOL_BUFFERS: i32 = 2;
/// How long the mirror stays unused after a list was sent when no frame answers it (a list change forces a frame within 3 to 12 ms).
const PENDING_MAX: Duration = Duration::from_millis(500);
/// Frames of the wrong size in a row, after the pool was made anew, before the session is given up.
const MAX_MISMATCHED: u32 = 120;

/// The WinRT objects of a session. Dropping it closes them, in the order the API wants, also on a half made session.
struct Live {
    item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    disp: IDisplayGraphicsCaptureSession,
    arrived: Option<i64>,
    closed_ev: Option<i64>,
}

impl Drop for Live {
    fn drop(&mut self) {
        if let Some(t) = self.arrived.take() {
            let _ = self.pool.RemoveFrameArrived(t);
        }
        if let Some(t) = self.closed_ev.take() {
            let _ = self.item.RemoveClosed(t);
        }
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

pub(super) struct SessionMonitor {
    /// The monitor in desktop coordinates.
    pub rect: RECT,
    pub mirror: Option<Source>,
    /// The mirror holds a picture of the desktop.
    pub has_image: bool,
    hmon: isize,
    live: Live,
    winrt: IDirect3DDevice,
    /// A token for every `FrameArrived` (at most one waits): what the engine thread blocks on.
    wake: Receiver<()>,
    closed: Arc<AtomicBool>,
    /// DirtyRegions can be read (mode ReportOnly was accepted).
    dirty_ok: bool,
    /// The exclusion version last sent to this session.
    sent: Option<u64>,
    /// The number the last `SetWindowExclusionList` returned: frames below it are stale.
    last_set: u64,
    /// The iteration of the last frame that was used: a different one means the list changed, and the frame is a whole refresh.
    last_iter: Option<u64>,
    /// Frames in a row that had another size than the mirror although the pool was made anew for it (in-flight ones are dropped; this
    /// many in a row is an error).
    mismatched: u32,
    /// When the exclusion list was last sent while no frame made with it has been copied yet: the mirror may still hold our own windows,
    /// so nothing is drawn from it (`ready`).
    pending: Option<Instant>,
    /// How long the last `SetWindowExclusionList` took, for the manual check.
    pub last_set_time: Duration,
}

struct Taken {
    frame: bool,
    image: bool,
    dirty: Dirty,
}

fn hmonitor(h: isize) -> HMONITOR {
    HMONITOR(h as *mut c_void)
}

fn winrt_device(g: &Gpu) -> R<IDirect3DDevice> {
    let dxgi: IDXGIDevice = g.dev.cast()?;
    unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast() }
}

/// The monitor's rectangle.
unsafe fn monitor_rect(hmon: isize) -> Option<RECT> {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    GetMonitorInfoW(hmonitor(hmon), &mut mi).as_bool().then_some(mi.rcMonitor)
}

/// The monitor's rectangle and the current refresh rate of its display (the rate is only needed when a session is opened).
unsafe fn monitor_info(hmon: isize) -> Option<(RECT, u32)> {
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if !GetMonitorInfoW(hmonitor(hmon), &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
        return None;
    }
    let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    let hz = if EnumDisplaySettingsW(PCWSTR(mi.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() { dm.dmDisplayFrequency } else { 0 };
    Some((mi.monitorInfo.rcMonitor, hz))
}

fn iteration_of(frame: &Direct3D11CaptureFrame) -> Option<u64> {
    let f3 = frame.cast::<IDirect3D11CaptureFrame3>().ok()?;
    let mut n = 0u64;
    unsafe { f3.ConfigurationIteration(&mut n) }.ok().ok()?;
    Some(n)
}

fn dirty_of(frame: &Direct3D11CaptureFrame) -> Dirty {
    let Ok(v) = frame.DirtyRegions() else { return Dirty::All };
    let Ok(n) = v.Size() else { return Dirty::All };
    if n > 64 {
        return Dirty::All;
    }
    let mut rects = Vec::with_capacity(n as usize);
    for i in 0..n {
        match v.GetAt(i) {
            Ok(r) => rects.push(RECT { left: r.X, top: r.Y, right: r.X.saturating_add(r.Width), bottom: r.Y.saturating_add(r.Height) }),
            Err(_) => return Dirty::All,
        }
    }
    Dirty::Rects(rects)
}

/// Why a session's wait for a frame ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WaitEnd {
    /// A command is queued: the engine must read it before it takes any frame.
    Command,
    /// A frame arrived (on this monitor or one of `also`), or the wait ran out: the caller takes what there is.
    Other,
}

/// Waits up to `wait` for the first of: a frame of this session (`wake`), a frame of another session (`also`), a command in `cmds`. This
/// is what lets the engine's quiet wait be long: nothing the engine has to react to can arrive without ending it. A channel of `also`
/// that is the session's own is skipped.
pub(super) fn wait_end(wake: &Receiver<()>, also: &[Receiver<()>], cmds: Option<&Receiver<Cmd>>, wait: Duration) -> WaitEnd {
    let mut sel = Select::new();
    sel.recv(wake);
    for r in also.iter().filter(|r| !r.same_channel(wake)) {
        sel.recv(r);
    }
    let cmd_ix = cmds.map(|c| sel.recv(c));
    match (sel.ready_timeout(wait), cmds) {
        (Ok(i), Some(c)) if Some(i) == cmd_ix => {
            if !c.is_empty() {
                return WaitEnd::Command;
            }
            // the queue reads ready but is empty: it is closed. Wait on the frames only, or this would spin.
            let _ = wake.recv_timeout(wait);
            WaitEnd::Other
        }
        _ => WaitEnd::Other,
    }
}

impl SessionMonitor {
    /// The channel a frame of this session wakes (see `pump`).
    pub(super) fn wake(&self) -> &Receiver<()> {
        &self.wake
    }

    /// Makes the session for a monitor and starts it. Never asks the user for anything: the caller has checked the permission
    /// (`access`), the border and the cursor are switched off before the start. `Err` is the reason it cannot be had.
    pub(super) unsafe fn open(g: &Gpu, hmon: isize) -> Result<SessionMonitor, String> {
        let (rect, hz) = monitor_info(hmon).ok_or("the monitor is gone")?;
        let step = |what: &str| {
            let what = what.to_string();
            move |e: windows::core::Error| format!("{what}: {e}")
        };
        // WinRT work from our thread, and the capture service uses the device from its own: the device must be thread safe
        if let Ok(mt) = g.ctx.cast::<ID3D11Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
        let winrt = winrt_device(g).map_err(step("the WinRT view of the device"))?;
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(step("the capture item factory"))?;
        let item: GraphicsCaptureItem = interop.CreateForMonitor(hmonitor(hmon)).map_err(step("capture item for the monitor"))?;
        let size = item.Size().map_err(step("the capture item's size"))?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err("the capture item has no size".into());
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt, FORMAT, POOL_BUFFERS, size).map_err(step("the frame pool"))?;
        let session = match pool.CreateCaptureSession(&item) {
            Ok(s) => s,
            Err(e) => {
                let _ = pool.Close();
                return Err(format!("the capture session: {e}"));
            }
        };
        let disp = match session.cast::<IDisplayGraphicsCaptureSession>() {
            Ok(d) => d,
            Err(e) => {
                let _ = session.Close();
                let _ = pool.Close();
                return Err(format!("the capture session has no window exclusion list (it needs Windows 11 24H2): {e}"));
            }
        };
        // from here on a `?` drops `live`, which closes everything
        let mut live = Live { item, pool, session, disp, arrived: None, closed_ev: None };

        // No border, no cursor, BEFORE the capture starts. The read-back only proves that the values were stored: nothing in the API says
        // whether a border is really drawn, so this relies on the permission having read Allowed (the caller checked it).
        live.session.SetIsBorderRequired(false).map_err(step("switching the capture border off"))?;
        live.session.SetIsCursorCaptureEnabled(false).map_err(step("switching the cursor capture off"))?;
        if live.session.IsBorderRequired().unwrap_or(true) || live.session.IsCursorCaptureEnabled().unwrap_or(true) {
            return Err("the capture border or the cursor capture could not be switched off".into());
        }
        // Only the places that changed are reported; the frames themselves stay whole.
        let dirty_ok = live.session.SetDirtyRegionMode(GraphicsCaptureDirtyRegionMode::ReportOnly).is_ok();
        // A frame is wanted at most once per refresh of the monitor (the default is 16 ms whatever the monitor does).
        let _ = live.session.SetMinUpdateInterval(TimeSpan { Duration: min_interval_ticks(hz) });

        let (tx, wake) = bounded::<()>(1);
        let tx2 = tx.clone();
        live.arrived = Some(
            live.pool
                .FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |_, _| {
                    // runs on a thread of the system: only wake the engine
                    let _ = tx.try_send(());
                    Ok(())
                }))
                .map_err(step("the frame handler"))?,
        );
        let closed = Arc::new(AtomicBool::new(false));
        let closed2 = closed.clone();
        live.closed_ev = Some(
            live.item
                .Closed(&TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                    closed2.store(true, Ordering::Release);
                    let _ = tx2.try_send(());
                    Ok(())
                }))
                .map_err(step("the close handler"))?,
        );
        let mirror = Source::new(g, size.Width as u32, size.Height as u32, None).map_err(step("the mirror texture"))?;
        live.session.StartCapture().map_err(step("starting the capture"))?;
        Ok(SessionMonitor {
            rect,
            mirror: Some(mirror),
            has_image: false,
            hmon,
            live,
            winrt,
            wake,
            closed,
            dirty_ok,
            sent: None,
            last_set: 0,
            last_iter: None,
            mismatched: 0,
            pending: None,
            last_set_time: Duration::ZERO,
        })
    }

    fn backend(why: impl Into<String>) -> MonError {
        MonError::Backend(why.into())
    }

    /// Sends the exclusion list when it is not the one this session has. Frames made before the call are dropped by their iteration.
    unsafe fn sync_exclusion(&mut self, ex: &Exclusion) -> Result<(), MonError> {
        if self.sent == Some(ex.version()) {
            return Ok(());
        }
        let ids: Vec<WindowId> = ex.list().into_iter().map(|h| WindowId { Value: h as usize as u64 }).collect();
        let count = ids.len();
        let it: IIterable<WindowId> = ids.into();
        let mut n = 0u64;
        let t = std::time::Instant::now();
        self.live.disp.SetWindowExclusionList(it.as_raw(), &mut n).ok().map_err(|e| Self::backend(format!("SetWindowExclusionList: {e}")))?;
        self.last_set_time = t.elapsed();
        log::debug!("glass: exclusion list of {count} windows sent, iteration {n}, {:?}", self.last_set_time);
        self.last_set = n;
        self.sent = Some(ex.version());
        self.pending = Some(Instant::now());
        Ok(())
    }

    /// The monitor may be moved in the desktop arrangement without a new item.
    unsafe fn refresh_rect(&mut self) {
        if let Some(r) = monitor_rect(self.hmon) {
            self.rect = r;
        }
    }

    /// The mirror may be drawn from. False while the exclusion list was just sent and no picture made with it has come yet (the mirror
    /// then may hold the pixels of a window that was added, which the glass would bend), but never for longer than `PENDING_MAX`: a
    /// session that does not answer a list with a frame must not keep the windows from drawing for ever.
    pub(super) fn ready(&self) -> bool {
        self.has_image && self.pending.is_none_or(|t| t.elapsed() > PENDING_MAX)
    }

    /// Waits (up to `timeout_ms`, only when no frame is waiting already) for a frame, takes what the pool has and refreshes the mirror.
    ///
    /// `cmds` is the engine's command queue: a command waiting in it (a popup registered, a window attached) ends the wait and keeps this
    /// turn from taking any frame, so the engine reads the command and sends the new list BEFORE a frame that may show the new window is
    /// copied. The frames stay in the pool; the ones made with the old list are dropped by their iteration on the next turn.
    ///
    /// `also` are the wake channels of the engine's OTHER monitors: only the first monitor of a turn waits, and without them a frame on
    /// a second monitor would be noticed only when that wait ran out.
    pub(super) unsafe fn pump(
        &mut self,
        g: &Gpu,
        timeout_ms: u32,
        ex: &Exclusion,
        cmds: Option<&Receiver<Cmd>>,
        also: &[Receiver<()>],
    ) -> Result<Pumped, MonError> {
        let none = || Pumped::Waited { frame: false, image: false, dirty: Dirty::none() };
        let command_waiting = || cmds.is_some_and(|c| !c.is_empty());
        self.check_open()?;
        if command_waiting() {
            return Ok(none());
        }
        self.refresh_rect();
        self.sync_exclusion(ex)?;
        while self.wake.try_recv().is_ok() {}
        let mut got = self.take(g)?;
        if !got.frame && timeout_ms > 0 {
            if wait_end(&self.wake, also, cmds, Duration::from_millis(timeout_ms as u64)) == WaitEnd::Command {
                return Ok(none());
            }
            self.check_open()?;
            if command_waiting() {
                return Ok(none());
            }
            got = self.take(g)?;
        }
        Ok(Pumped::Waited { frame: got.frame, image: got.image, dirty: got.dirty })
    }

    fn check_open(&self) -> Result<(), MonError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Self::backend("the capture item of the monitor was closed"));
        }
        Ok(())
    }

    /// Takes every frame the pool has, keeps the newest one made with the current list, and copies it into the mirror. `frame` of the
    /// result is true only for a frame made with the current list: frames of an older one are dropped and do not end the caller's wait.
    unsafe fn take(&mut self, g: &Gpu) -> Result<Taken, MonError> {
        let mut newest: Option<Direct3D11CaptureFrame> = None;
        let mut dirty = Dirty::none();
        let mut queued = 0usize;
        let drop_frame = |f: Option<Direct3D11CaptureFrame>| {
            if let Some(f) = f {
                let _ = f.Close();
            }
        };
        loop {
            let f = match self.live.pool.TryGetNextFrame() {
                Ok(f) => f,
                // the call succeeded and returned no frame: the pool is empty
                Err(e) if e.code().is_ok() => break,
                Err(e) => {
                    drop_frame(newest.take());
                    return Err(if is_device_lost(&e) {
                        MonError::Device(format!("device lost while taking a frame: {e}"))
                    } else {
                        Self::backend(format!("taking a frame: {e}"))
                    });
                }
            };
            queued += 1;
            let Some(it) = iteration_of(&f) else {
                let _ = f.Close();
                drop_frame(newest.take());
                return Err(Self::backend("a frame has no configuration iteration (the exclusion list cannot be told apart)"));
            };
            // where it changed counts even for a frame that is not used: the mirror jumps over it
            dirty.merge(if self.dirty_ok { dirty_of(&f) } else { Dirty::All });
            if is_stale(it, self.last_set) {
                let _ = f.Close();
                continue;
            }
            if self.last_iter != Some(it) {
                // the list changed: this frame is a whole new picture
                self.last_iter = Some(it);
                dirty.merge(Dirty::All);
            }
            drop_frame(newest.replace(f));
        }
        if queued >= POOL_BUFFERS as usize {
            // Every buffer was full, so the system may have skipped frames; whether the dirty rectangles of a frame are relative to the
            // last frame delivered or to the last one made has not been measured. Any window may be touched.
            dirty.merge(Dirty::All);
        }
        let Some(frame) = newest else { return Ok(Taken { frame: false, image: false, dirty: Dirty::none() }) };
        let was = self.has_image;
        let image = self.copy_frame(g, &frame);
        let _ = frame.Close();
        let image = image?;
        if image {
            if !was {
                dirty = Dirty::All;
            }
            dirty = dirty.or_all_if_empty();
        }
        Ok(Taken { frame: true, image, dirty: if image { dirty } else { Dirty::none() } })
    }

    /// Copies a frame into the mirror. False when the frame had another size than the mirror (it is dropped; the pool and the mirror are
    /// made anew when the monitor really changed its size).
    unsafe fn copy_frame(&mut self, g: &Gpu, frame: &Direct3D11CaptureFrame) -> Result<bool, MonError> {
        let lost = |what: &str, e: windows::core::Error| {
            if is_device_lost(&e) { MonError::Device(format!("{what}: {e}")) } else { Self::backend(format!("{what}: {e}")) }
        };
        let surface = frame.Surface().map_err(|e| lost("a frame's surface", e))?;
        let tex = surface
            .cast::<IDirect3DDxgiInterfaceAccess>()
            .and_then(|a| a.GetInterface::<ID3D11Texture2D>())
            .map_err(|e| lost("a frame's GPU texture", e))?;
        let mut td = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut td);
        let Some(m) = self.mirror.as_ref() else { return Err(MonError::Monitor("no mirror of the monitor".into())) };
        if td.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(Self::backend(format!("the captured frame is {:?}, not B8G8R8A8", td.Format)));
        }
        // The pool keeps delivering buffers of the size it was made with after the monitor changed its resolution or rotation, with
        // the new picture in `ContentSize`: compared on every frame.
        let content = frame.ContentSize().map(|s| (s.Width, s.Height)).unwrap_or((m.w as i32, m.h as i32));
        let item = &self.live.item;
        let item_size = || item.Size().map(|s| (s.Width, s.Height)).unwrap_or((0, 0));
        match size_check(content, (td.Width, td.Height), (m.w, m.h), item_size) {
            SizeCheck::Fine => {}
            SizeCheck::Recreate => {
                self.resize(g)?;
                return Ok(false);
            }
            SizeCheck::Stale => {
                self.mismatched += 1;
                if self.mismatched > MAX_MISMATCHED {
                    return Err(Self::backend("frames keep arriving in another size than the capture item"));
                }
                return Ok(false);
            }
        }
        g.ctx.CopyResource(&m.tex, &tex);
        self.has_image = true;
        self.mismatched = 0;
        self.pending = None;
        Ok(true)
    }

    /// The monitor changed its size: a pool and a mirror of the new size. Frames of the old size are dropped meanwhile.
    unsafe fn resize(&mut self, g: &Gpu) -> Result<(), MonError> {
        let size = self.live.item.Size().map_err(|e| Self::backend(format!("the capture item's size: {e}")))?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err(Self::backend("the capture item has no size"));
        }
        log::info!("glass: the monitor is now {}x{} px, the capture is made anew", size.Width, size.Height);
        self.live.pool.Recreate(&self.winrt, FORMAT, POOL_BUFFERS, size).map_err(|e| Self::backend(format!("recreating the frame pool: {e}")))?;
        match Source::new(g, size.Width as u32, size.Height as u32, None) {
            Ok(s) => self.mirror = Some(s),
            Err(e) if is_device_lost(&e) => return Err(MonError::Device(format!("device lost: {e}"))),
            Err(e) => return Err(MonError::Monitor(format!("the mirror texture of {}x{} px could not be made: {e}", size.Width, size.Height))),
        }
        self.has_image = false;
        self.mismatched = 0;
        Ok(())
    }

    /// The list the session reports, for the manual check: `None` when it has none yet (the call succeeds with a null pointer then).
    pub(super) unsafe fn reported_list(&self) -> Option<Vec<u64>> {
        let mut raw: *mut c_void = std::ptr::null_mut();
        self.live.disp.GetWindowExclusionList(&mut raw).ok().ok()?;
        if raw.is_null() {
            return None;
        }
        let insp = IInspectable::from_raw(raw);
        let v = insp.cast::<windows_collections::IVectorView<WindowId>>().ok()?;
        Some(v.into_iter().map(|w| w.Value).collect())
    }
}
