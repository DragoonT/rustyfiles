//! Screen capture for the glass: one capture per monitor that has a visible glass window on it, and a persistent mirror texture of that
//! monitor which the glass reads. The glass windows themselves are left out of the picture, so what is mirrored is what lies behind them.
//! Two backends do this behind the one [`Monitor`] type:
//!
//! * a Windows.Graphics.Capture session (`wgc.rs`), where the windows are put into the session's own exclusion list and stay visible to
//!   every other screenshot, recording and screen share;
//! * DXGI Desktop Duplication (here), the fallback, where the windows are left out by `WDA_EXCLUDEFROMCAPTURE`, which hides them from every
//!   capture of the screen.

use std::time::{Duration, Instant};

use std::ffi::c_void;

use crossbeam_channel::Receiver;
use windows::Win32::Foundation::{E_ACCESSDENIED, HANDLE, LUID, RECT};
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
    DXGI_ERROR_DRIVER_INTERNAL_ERROR, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1,
    IDXGIOutput, IDXGIOutput1, IDXGIOutput5, IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME,
};
use windows::core::Interface;

use super::Cmd;
use super::gpu::{Gpu, Source};
use super::wgc::{Exclusion, SessionMonitor};

type R<T> = windows::core::Result<T>;

/// How long a duplication may stay broken (mode change, a driver that refuses...) before the windows on that monitor give up. The time
/// the input desktop is not the user's own (lock screen, UAC prompt) does not count: see `input_desktop_is_ours`.
const BROKEN_LIMIT: Duration = Duration::from_secs(10);
const BACKOFF_FIRST: Duration = Duration::from_millis(100);
const BACKOFF_MAX: Duration = Duration::from_secs(1);

/// Whether an error means the whole D3D device is gone (all windows fail), not just this duplication.
pub(super) fn is_device_lost(e: &windows::core::Error) -> bool {
    let c = e.code();
    c == DXGI_ERROR_DEVICE_REMOVED || c == DXGI_ERROR_DEVICE_RESET || c == DXGI_ERROR_DEVICE_HUNG || c == DXGI_ERROR_DRIVER_INTERNAL_ERROR
}

pub(super) struct Located {
    pub adapter: IDXGIAdapter1,
    pub output: IDXGIOutput,
    pub rect: RECT,
    pub rotated: bool,
}

/// The adapter and output a monitor is attached to. A new factory every time: an old one does not see monitors that came later.
pub(super) unsafe fn locate(hmon: isize) -> R<Located> {
    let f: IDXGIFactory1 = CreateDXGIFactory1()?;
    let mut a = 0;
    while let Ok(adapter) = f.EnumAdapters1(a) {
        let mut o = 0;
        while let Ok(output) = adapter.EnumOutputs(o) {
            let d = output.GetDesc()?;
            if d.Monitor.0 as isize == hmon {
                let rotated = d.Rotation != DXGI_MODE_ROTATION_IDENTITY && d.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED;
                return Ok(Located { adapter, output, rect: d.DesktopCoordinates, rotated });
            }
            o += 1;
        }
        a += 1;
    }
    Err(DXGI_ERROR_NOT_FOUND.into())
}

/// Whether the desktop that takes the user's input is the normal one. On the lock screen, the Ctrl+Alt+Del screen and a UAC prompt it is
/// Winlogon's: a process of the user cannot open it, and Desktop Duplication answers ACCESS_LOST or E_ACCESSDENIED for as long as it
/// stays. That is a pause, not a failure: a PC that was locked for a minute must come back with its glass.
fn input_desktop_is_ours() -> bool {
    unsafe {
        let Ok(desk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) else { return false };
        let mut name = [0u16; 64];
        let got = GetUserObjectInformationW(HANDLE(desk.0), UOI_NAME, Some(name.as_mut_ptr() as *mut c_void), size_of_val(&name) as u32, None);
        let _ = CloseDesktop(desk);
        // opened but unnamed: nothing says it is not ours
        got.is_err() || String::from_utf16_lossy(&name).trim_end_matches('\0').eq_ignore_ascii_case("Default")
    }
}

/// DuplicateOutput1 asking for BGRA only (an HDR display then hands back an 8 bit tone-mapped picture), else the plain DuplicateOutput.
unsafe fn duplicate(g: &Gpu, out: &IDXGIOutput) -> R<IDXGIOutputDuplication> {
    if let Ok(o5) = out.cast::<IDXGIOutput5>()
        && let Ok(d) = o5.DuplicateOutput1(&g.dev, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM])
    {
        return Ok(d);
    }
    let o1: IDXGIOutput1 = out.cast()?;
    o1.DuplicateOutput(&g.dev)
}

/// Why a monitor stopped being usable.
pub(super) enum MonError {
    /// The device is gone: every window fails.
    Device(String),
    /// This monitor cannot be used (wrong adapter, rotated, odd format) or stayed broken too long: the windows on it fail.
    Monitor(String),
    /// The capture session backend cannot go on (a call failed, the item closed, access was withdrawn): the engine switches to Desktop
    /// Duplication for all windows instead of failing them.
    Backend(String),
}

/// Which parts of a monitor a refresh of the mirror changed, in the monitor's own pixels.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Dirty {
    /// Anywhere: Desktop Duplication does not say, and a frame forced by a change of the exclusion list covers the whole monitor.
    All,
    Rects(Vec<RECT>),
}

/// More rectangles than this are as good as the whole monitor.
const MAX_DIRTY_RECTS: usize = 64;

impl Dirty {
    pub(super) fn none() -> Dirty {
        Dirty::Rects(Vec::new())
    }

    /// Adds what `other` changed.
    pub(super) fn merge(&mut self, other: Dirty) {
        match (&mut *self, other) {
            (Dirty::All, _) => {}
            (me, Dirty::All) => *me = Dirty::All,
            (Dirty::Rects(a), Dirty::Rects(b)) => {
                a.extend(b);
                if a.len() > MAX_DIRTY_RECTS {
                    *self = Dirty::All;
                }
            }
        }
    }

    /// A refresh that reported nothing is not trusted to mean "nothing changed": every window is then treated as touched.
    pub(super) fn or_all_if_empty(self) -> Dirty {
        match self {
            Dirty::Rects(r) if r.is_empty() => Dirty::All,
            d => d,
        }
    }

    /// Whether a window's backdrop may have changed: its client rectangle (`x`, `y`, `w`, `h`, in monitor pixels) grown by `pad` on every
    /// side meets a dirty rectangle. The glass reads the backdrop that far around the window.
    pub(super) fn touches(&self, x: i32, y: i32, w: u32, h: u32, pad: u32) -> bool {
        match self {
            Dirty::All => true,
            Dirty::Rects(rects) => {
                let (l, t) = (x as i64 - pad as i64, y as i64 - pad as i64);
                let (r, b) = (x as i64 + w as i64 + pad as i64, y as i64 + h as i64 + pad as i64);
                rects.iter().any(|d| (d.left as i64) < r && (d.right as i64) > l && (d.top as i64) < b && (d.bottom as i64) > t)
            }
        }
    }
}

/// What one `pump` did.
pub(super) enum Pumped {
    /// Returned at once (waiting for a retry, or a failure that is being backed off): the caller must not count this as having waited.
    NoWait,
    /// The capture waited (or looked). `frame`: a frame came; `image`: the mirror was refreshed from it, `dirty` says where.
    Waited { frame: bool, image: bool, dirty: Dirty },
}

/// The screen capture of one monitor, by whichever backend the engine picked.
pub(super) enum Monitor {
    Dup(DupMonitor),
    Session(Box<SessionMonitor>),
}

impl Monitor {
    /// A Desktop Duplication monitor: checks that it can be mirrored on the glass device (`luid`): same adapter, not rotated.
    pub(super) unsafe fn open(hmon: isize, luid: LUID) -> Result<Monitor, String> {
        DupMonitor::open(hmon, luid).map(Monitor::Dup)
    }

    /// A capture session on the monitor. `Err` is the reason it cannot be had.
    pub(super) unsafe fn open_session(g: &Gpu, hmon: isize) -> Result<Monitor, String> {
        SessionMonitor::open(g, hmon).map(|s| Monitor::Session(Box::new(s)))
    }

    /// The monitor in desktop coordinates.
    pub(super) fn rect(&self) -> RECT {
        match self {
            Monitor::Dup(m) => m.rect,
            Monitor::Session(m) => m.rect,
        }
    }

    pub(super) fn mirror(&self) -> Option<&Source> {
        match self {
            Monitor::Dup(m) => m.mirror.as_ref(),
            Monitor::Session(m) => m.mirror.as_ref(),
        }
    }

    /// The mirror holds a picture of the desktop that may be drawn from (for a session: not one made before the last change of the
    /// exclusion list took effect).
    pub(super) fn has_image(&self) -> bool {
        match self {
            Monitor::Dup(m) => m.has_image,
            Monitor::Session(m) => m.ready(),
        }
    }

    /// Takes what the capture has (waiting up to `timeout_ms` for it) and refreshes the mirror. `ex` is the set of windows a session
    /// leaves out of its picture and `cmds` the engine's command queue (a session stops waiting when a command arrives); Desktop
    /// Duplication uses neither.
    pub(super) unsafe fn pump(&mut self, g: &Gpu, timeout_ms: u32, ex: &Exclusion, cmds: Option<&Receiver<Cmd>>) -> Result<Pumped, MonError> {
        match self {
            Monitor::Dup(m) => m.pump(g, timeout_ms),
            Monitor::Session(m) => m.pump(g, timeout_ms, ex, cmds),
        }
    }
}

pub(super) struct DupMonitor {
    pub hmon: isize,
    /// The monitor in desktop coordinates.
    pub rect: RECT,
    output: IDXGIOutput,
    dup: Option<IDXGIOutputDuplication>,
    pub mirror: Option<Source>,
    /// The mirror holds a picture of the desktop.
    pub has_image: bool,
    broken_since: Option<Instant>,
    /// The duplication was made anew and has not delivered a frame yet: its first frame is copied whatever its present time says, or the
    /// mirror would keep the picture from before the break.
    fresh: bool,
    retry_at: Instant,
    backoff: Duration,
    last_error: String,
}

impl DupMonitor {
    /// Checks that the monitor can be mirrored on the glass device (`luid`): same adapter, not rotated. The duplication itself is made
    /// on the first `pump`, and again whenever it was lost.
    unsafe fn open(hmon: isize, luid: LUID) -> Result<DupMonitor, String> {
        let l = locate(hmon).map_err(|e| format!("the monitor is not among the DXGI outputs: {e}"))?;
        let al = l.adapter.GetDesc1().map_err(|e| format!("adapter description: {e}"))?.AdapterLuid;
        if al.LowPart != luid.LowPart || al.HighPart != luid.HighPart {
            return Err("the monitor is attached to another graphics adapter than the one the glass is drawn on".into());
        }
        if l.rotated {
            return Err("the monitor is rotated, which the glass does not handle".into());
        }
        Ok(DupMonitor {
            hmon,
            rect: l.rect,
            output: l.output,
            dup: None,
            mirror: None,
            has_image: false,
            broken_since: None,
            fresh: false,
            retry_at: Instant::now(),
            backoff: BACKOFF_FIRST,
            last_error: String::new(),
        })
    }

    /// Records a failure of the duplication and arranges the retry. Gives up after BROKEN_LIMIT, except while the input desktop is not the
    /// user's (`access_refused`: the error is the kind the lock screen and a UAC prompt cause).
    fn broken(&mut self, why: String, now: Instant, access_refused: bool) -> Result<Pumped, MonError> {
        self.dup = None;
        if access_refused && !input_desktop_is_ours() {
            self.broken_since = None; // the clock starts when the desktop is back
            self.last_error = why;
            self.retry_at = now + BACKOFF_MAX;
            return Ok(Pumped::NoWait);
        }
        let since = *self.broken_since.get_or_insert(now);
        self.last_error = why;
        if now.duration_since(since) > BROKEN_LIMIT {
            return Err(MonError::Monitor(format!("screen capture stayed unavailable for {} s: {}", BROKEN_LIMIT.as_secs(), self.last_error)));
        }
        self.retry_at = now + self.backoff;
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
        Ok(Pumped::NoWait)
    }

    /// Makes the duplication if it is missing and its retry time has come; takes at most one new frame (waiting up to `timeout_ms`) and
    /// copies it into the mirror when it carries a new picture.
    unsafe fn pump(&mut self, g: &Gpu, timeout_ms: u32) -> Result<Pumped, MonError> {
        let now = Instant::now();
        if self.dup.is_none() {
            if now < self.retry_at {
                return Ok(Pumped::NoWait);
            }
            // The adapter list may have changed, and the output must be the current one after a mode change.
            match locate(self.hmon) {
                Ok(l) => {
                    self.output = l.output;
                    self.rect = l.rect;
                    if l.rotated {
                        return Err(MonError::Monitor("the monitor is rotated, which the glass does not handle".into()));
                    }
                }
                Err(e) => return self.broken(format!("the monitor is gone from the DXGI outputs: {e}"), now, false),
            }
            let d = match duplicate(g, &self.output) {
                Ok(d) => d,
                Err(e) if is_device_lost(&e) => return Err(MonError::Device(format!("device lost while starting the screen capture: {e}"))),
                Err(e) => {
                    let refused = e.code() == E_ACCESSDENIED || e.code() == DXGI_ERROR_ACCESS_LOST;
                    return self.broken(format!("Desktop Duplication is not available: {e}"), now, refused);
                }
            };
            let dd = d.GetDesc();
            // (The format is checked on the frames themselves: ModeDesc of an HDR display says FP16 although the picture asked for is BGRA.)
            if dd.DesktopImageInSystemMemory.as_bool() {
                return Err(MonError::Monitor("the desktop image is in system memory (a software or remote display), which the glass cannot read".into()));
            }
            if dd.Rotation != DXGI_MODE_ROTATION_IDENTITY && dd.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED {
                return Err(MonError::Monitor("the duplicated output is rotated, which the glass does not handle".into()));
            }
            let (mw, mh) = (dd.ModeDesc.Width, dd.ModeDesc.Height);
            if self.mirror.as_ref().is_none_or(|m| (m.w, m.h) != (mw, mh)) {
                match Source::new(g, mw, mh, None) {
                    Ok(s) => {
                        self.mirror = Some(s);
                        self.has_image = false;
                    }
                    Err(e) if is_device_lost(&e) => return Err(MonError::Device(format!("device lost: {e}"))),
                    Err(e) => return Err(MonError::Monitor(format!("the mirror texture of {mw}x{mh} px could not be made: {e}"))),
                }
            }
            // broken_since stays: a duplication that is made and lost again at once must still run into the limit
            self.dup = Some(d);
            self.fresh = true;
        }
        let Some(dup) = self.dup.clone() else { return Ok(Pumped::NoWait) };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        match dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
            Ok(()) => {
                // A frame counts when the desktop was really presented; one that only moved the cursor carries no new picture. The very
                // first one (of the monitor, and of every rebuilt duplication) is taken anyway, so a desktop that never changes still gets
                // mirrored.
                let want = info.LastPresentTime != 0 || !self.has_image || self.fresh;
                let mut image = false;
                let mut bad: Option<String> = None;
                if want {
                    match (res.take().and_then(|x| x.cast::<ID3D11Texture2D>().ok()), &self.mirror) {
                        (Some(tex), Some(m)) => {
                            let mut td = D3D11_TEXTURE2D_DESC::default();
                            tex.GetDesc(&mut td);
                            if td.Format != DXGI_FORMAT_B8G8R8A8_UNORM || (td.Width, td.Height) != (m.w, m.h) {
                                bad = Some(format!(
                                    "the duplicated frame is {}x{} {:?}, not {}x{} B8G8R8A8 (HDR or wide colour is not handled)",
                                    td.Width, td.Height, td.Format, m.w, m.h
                                ));
                            } else {
                                // The whole frame, not only the parts under the windows: a window dragged onto another part of an idle
                                // desktop gets no new frame, so the mirror must already hold everything.
                                g.ctx.CopyResource(&m.tex, &tex);
                                image = true;
                                self.has_image = true;
                            }
                        }
                        // frames that arrive but never become a picture would leave the window "starting" for ever
                        (None, _) => bad = Some("Desktop Duplication delivered a frame without a GPU texture".into()),
                        (_, None) => bad = Some("no mirror of the monitor".into()),
                    }
                }
                drop(res);
                let _ = dup.ReleaseFrame();
                if let Some(why) = bad {
                    return Err(MonError::Monitor(why));
                }
                self.fresh = false;
                self.broken_since = None;
                self.backoff = BACKOFF_FIRST;
                // Desktop Duplication's dirty rectangles are not read: after a refresh any window may be touched
                Ok(Pumped::Waited { frame: true, image, dirty: if image { Dirty::All } else { Dirty::none() } })
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                // nothing new on screen, but the duplication works
                self.broken_since = None;
                self.backoff = BACKOFF_FIRST;
                Ok(Pumped::Waited { frame: false, image: false, dirty: Dirty::none() })
            }
            Err(e) if is_device_lost(&e) => Err(MonError::Device(format!("device lost while capturing the screen: {e}"))),
            // mode change, lock screen, secure desktop...: build the duplication again after a pause
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => self.broken(format!("the screen capture was lost: {e}"), Instant::now(), true),
            Err(e) => self.broken(format!("AcquireNextFrame failed: {e}"), Instant::now(), e.code() == E_ACCESSDENIED),
        }
    }
}
