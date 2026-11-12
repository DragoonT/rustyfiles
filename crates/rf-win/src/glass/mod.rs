//! Glass: a window background that bends and frosts the live screen behind the window (the experimental "Glass" backdrop).
//!
//! HOW IT WORKS. The screen is captured with DXGI Desktop Duplication, the window itself is kept out of that capture
//! (`WDA_EXCLUDEFROMCAPTURE`, otherwise the glass would bend its own picture), and one pixel shader (`glass.hlsl`) draws the glass over
//! a cropped, mip-blurred copy of the screen into a swap chain that DirectComposition shows BEHIND the window's own scene. GPUI is not
//! changed: it makes its composition target with `topmost = true`, the engine makes a second one on the same window with
//! `topmost = false`, and the app sets the GPUI window transparent so the glass shows through.
//!
//! Optical model and presets after tomacco/amazing-glass (MIT); the shader was written from numbers and formulas, no code is copied.
//!
//! THREADING. One thread ("rf-glass") for the whole process, started by the first [`attach`] and idle (blocked on its channel, no
//! duplication held) when no window is attached. All D3D / DXGI / DirectComposition objects are made and used on that thread only. The
//! functions here are cheap and can be called from any thread, the UI thread included: they flip an entry in a table and send a command.
//!
//! FAILURE NEVER BREAKS THE WINDOW. On any error for a window the engine takes its glass off, lifts the capture exclusion, logs it and
//! sets [`GlassState::Failed`]; the app then falls back to another backdrop. [`detach`] clears a failed window's entry. (A PANIC on the
//! engine thread is caught and turned into the same failure only in debug builds and tests: the release profile is `panic = "abort"`, where
//! a panic ends the process like any other panic of the app. The engine code therefore has no `unwrap` or `expect` on a path that can fail.)
//!
//! `RUSTYFILES_GLASS_STATS=1` makes the engine log, every 5 s per window, how many frames it acquired, how many carried an image, how
//! many draws it presented and how many the loop guard skipped, and the average time of the prep and of the glass pass.

// All the D3D / DXGI / DirectComposition calls are `unsafe`; a block around every single one inside the `unsafe fn`s adds noise, not safety.
#![allow(unsafe_op_in_unsafe_fn)]

mod capture;
mod engine;
mod gpu;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};
use std::sync::{LazyLock, Mutex};

use crossbeam_channel::Sender;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE};

/// What a window's glass is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlassState {
    /// Not attached.
    Off,
    /// Attached; the engine has not drawn it yet (it may be waiting for the window to become visible).
    Starting,
    /// The glass is on the window.
    Running,
    /// The glass could not be kept; the reason is in the text. The capture exclusion is lifted and the window is not drawn any more.
    Failed(String),
}

/// What a window's glass looks like beyond the system's theme: how it is cut, and which tint.
///
/// `GlassOptions::window(dark)` is what [`attach`] always meant: a normal window, whose corners the system cuts and whose glass is
/// drawn with the radius the system reports. A free-standing shape (the dock's islands) cuts its own corners, with its own radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassOptions {
    /// The dark tint (`true`) or the light one.
    pub dark: bool,
    /// The corner radius of the glass in DEVICE px. `None` is what the system does (a window's 8 px corner at 100 %, none before
    /// Windows 11, none for a maximised window). Whoever gives a radius gives it again when the DPI changes ([`set_options`]).
    pub radius_px: Option<f32>,
    /// Whether the glass cuts its own corners out (transparent outside the radius). A window leaves that to the system; a window
    /// that is itself a rounded shape needs it.
    pub cut_corners: bool,
}

impl GlassOptions {
    /// An ordinary window: exactly what `attach(hwnd, dark)` does.
    pub const fn window(dark: bool) -> GlassOptions {
        GlassOptions { dark, radius_px: None, cut_corners: false }
    }

    /// The same options with a radius that cannot hurt: not a number is "what the system does", a negative one is a square corner.
    pub fn sanitized(self) -> GlassOptions {
        let radius_px = match self.radius_px {
            Some(r) if !r.is_finite() => None,
            Some(r) => Some(r.max(0.0)),
            None => None,
        };
        GlassOptions { radius_px, ..self }
    }
}

pub(crate) enum Cmd {
    Attach { hwnd: isize, opts: GlassOptions },
    SetOptions { hwnd: isize, opts: GlassOptions },
    SetDark { hwnd: isize, dark: bool },
    Detach { hwnd: isize },
}

/// What the callers see. The engine thread updates it through `set_state`, `fail` and `forget`.
static STATES: LazyLock<Mutex<HashMap<isize, GlassState>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// The engine thread's command channel, once it has been started.
static ENGINE: Mutex<Option<Sender<Cmd>>> = Mutex::new(None);
/// Other windows of ours (menus, flyouts, the Properties window, the preview pane's host) that asked to stay out of the screen capture.
/// The exclusion only exists while some window has glass: it is applied when the first one gets it and lifted when the last one lets go,
/// so a window that outlives the glass does not stay invisible to screenshots and screen sharing.
static EXTRAS: Mutex<BTreeSet<isize>> = Mutex::new(BTreeSet::new());

fn states() -> std::sync::MutexGuard<'static, HashMap<isize, GlassState>> {
    STATES.lock().unwrap_or_else(|p| p.into_inner())
}

fn engine() -> std::sync::MutexGuard<'static, Option<Sender<Cmd>>> {
    ENGINE.lock().unwrap_or_else(|p| p.into_inner())
}

fn extras() -> std::sync::MutexGuard<'static, BTreeSet<isize>> {
    EXTRAS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Forgets handles that are no longer a window of this process (closed, or the number was handed to a window of another process).
fn prune_extras(e: &mut BTreeSet<isize>) {
    let mine = unsafe { GetCurrentProcessId() };
    e.retain(|&h| unsafe {
        let w = hwnd_of(h);
        let mut pid = 0u32;
        IsWindow(Some(w)).as_bool() && GetWindowThreadProcessId(w, Some(&mut pid)) != 0 && pid == mine
    });
}

/// Applies or lifts the exclusion on every registered window.
fn apply_extras(exclude: bool) {
    let list: Vec<isize> = {
        let mut e = extras();
        prune_extras(&mut e);
        e.iter().copied().collect()
    };
    for h in list {
        let _ = set_affinity(h, exclude);
    }
}

/// Lifts the registered windows' exclusion once no window has glass any more.
fn lift_extras_if_idle() {
    if !any_active() {
        apply_extras(false);
    }
}

fn hwnd_of(h: isize) -> HWND {
    HWND(h as *mut _)
}

fn set_affinity(hwnd: isize, exclude: bool) -> windows::core::Result<()> {
    unsafe { SetWindowDisplayAffinity(hwnd_of(hwnd), if exclude { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE }) }
}

/// Sends a command to the engine thread, starting the thread if it is not running. False if it could not be started.
fn send(mut cmd: Cmd) -> bool {
    let mut g = engine();
    if let Some(tx) = g.as_ref() {
        match tx.send(cmd) {
            Ok(()) => return true,
            // the thread is gone (it ended after a panic): start a new one
            Err(e) => {
                cmd = e.into_inner();
                *g = None;
            }
        }
    }
    let (tx, rx) = crossbeam_channel::unbounded();
    let spawned = std::thread::Builder::new().name("rf-glass".into()).spawn(move || {
        // only effective in debug builds and tests (see the module docs): a release build aborts on a panic
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine::run(rx)));
        if r.is_err() {
            log::error!("glass: the engine thread panicked");
            *engine() = None;
            fail_everything("the glass engine crashed");
        }
    });
    if let Err(e) = spawned {
        log::warn!("glass: cannot start the engine thread: {e}");
        return false;
    }
    let ok = tx.send(cmd).is_ok();
    *g = Some(tx);
    ok
}

/// Sends a command only if the engine thread is running (a detach must not start it). Whether it was sent.
fn send_if_running(cmd: Cmd) -> bool {
    engine().as_ref().is_some_and(|tx| tx.send(cmd).is_ok())
}

/// Engine side: the window now has glass (only if the app has not detached it in the meantime).
pub(crate) fn set_state(hwnd: isize, st: GlassState) {
    if let Some(s) = states().get_mut(&hwnd) {
        *s = st;
    }
}

/// Engine side: the window is gone; its entry goes with it.
pub(crate) fn forget(hwnd: isize) {
    states().remove(&hwnd);
}

/// Engine side: the glass of this window failed. The capture exclusion is lifted, the state says why.
pub(crate) fn fail(hwnd: isize, why: &str) {
    let present = {
        let mut s = states();
        match s.get_mut(&hwnd) {
            Some(st) => {
                *st = GlassState::Failed(why.to_string());
                true
            }
            None => false,
        }
    };
    if present {
        let _ = set_affinity(hwnd, false);
        lift_extras_if_idle();
    }
}

fn fail_everything(why: &str) {
    let live: Vec<isize> = states().iter().filter(|(_, s)| matches!(s, GlassState::Starting | GlassState::Running)).map(|(k, _)| *k).collect();
    for k in live {
        fail(k, why);
    }
}

/// Gives the window glass. `dark` picks the dark tint. Returns false if it cannot even start (the handle is not a window, the capture
/// exclusion is refused, the engine thread does not start); the reason is in [`state`] and the log. Excludes the window from screen
/// capture first: without that the engine would bend the window's own picture.
///
/// This is [`attach_with`] with [`GlassOptions::window`]: the system cuts the window's corners, the glass takes the system's radius.
pub fn attach(hwnd: isize, dark: bool) -> bool {
    attach_with(hwnd, GlassOptions::window(dark))
}

/// [`attach`] with the options of [`GlassOptions`]: a window of its own shape (the dock's islands) gives its corner radius and has
/// the glass cut its corners. Attaching a window that already has glass changes its options like [`set_options`] does.
pub fn attach_with(hwnd: isize, opts: GlassOptions) -> bool {
    let opts = opts.sanitized();
    let fail_now = |why: String| {
        log::warn!("glass: window {hwnd:#x} cannot be attached: {why}");
        states().insert(hwnd, GlassState::Failed(why));
        false
    };
    if hwnd == 0 || !unsafe { IsWindow(Some(hwnd_of(hwnd))) }.as_bool() {
        return fail_now("the handle is not a window".into());
    }
    {
        let s = states();
        if matches!(s.get(&hwnd), Some(GlassState::Starting | GlassState::Running)) {
            drop(s);
            set_options(hwnd, opts);
            return true;
        }
    }
    if let Err(e) = set_affinity(hwnd, true) {
        return fail_now(format!("the window cannot be excluded from screen capture: {e}"));
    }
    let first = !any_active();
    states().insert(hwnd, GlassState::Starting);
    if !send(Cmd::Attach { hwnd, opts }) {
        let _ = set_affinity(hwnd, false);
        return fail_now("the glass engine thread could not be started".into());
    }
    if first {
        apply_extras(true);
    }
    true
}

/// Changes the tint of a window that has glass (its radius and corner cutting stay as they are). Ignored for other windows.
pub fn set_dark(hwnd: isize, dark: bool) {
    if matches!(states().get(&hwnd), Some(GlassState::Starting | GlassState::Running)) {
        send_if_running(Cmd::SetDark { hwnd, dark });
    }
}

/// Changes all the options of a window that has glass: a new radius after the monitor's DPI changed, a new tint. Ignored for other
/// windows.
pub fn set_options(hwnd: isize, opts: GlassOptions) {
    if matches!(states().get(&hwnd), Some(GlassState::Starting | GlassState::Running)) {
        send_if_running(Cmd::SetOptions { hwnd, opts: opts.sanitized() });
    }
}

/// Takes the glass off a window and clears its entry (also a failed one). Does nothing for a window that has none, so it can be called
/// any number of times.
pub fn detach(hwnd: isize) {
    let had = states().remove(&hwnd).is_some();
    // The engine lifts the window's capture exclusion itself, AFTER it has taken its target off the window: lifted earlier, one more
    // frame could be captured with the window in it and bent into the glass that is just being removed. Without a running engine
    // (or when the command cannot be sent) nothing draws, so it is lifted here.
    if !send_if_running(Cmd::Detach { hwnd }) && had {
        let _ = set_affinity(hwnd, false);
    }
    if had {
        lift_extras_if_idle();
    }
}

/// What the window's glass is doing now.
pub fn state(hwnd: isize) -> GlassState {
    states().get(&hwnd).cloned().unwrap_or(GlassState::Off)
}

/// Whether some window currently has glass (or is about to). The app can use it to keep a watch on the power cost.
pub fn any_active() -> bool {
    states().values().any(|s| matches!(s, GlassState::Starting | GlassState::Running))
}

/// Keeps another window of ours (a menu, a flyout, the Properties window, the preview pane's host) out of the screen capture, so the
/// glass behind a window does not bend the window's own popups. It can be called for every such window whether or not glass is on: the
/// window is only remembered until some window has glass, the exclusion is applied then and lifted again when the last glass goes.
/// `on = false` forgets the window and lifts its exclusion.
pub fn exclude_from_capture(hwnd: isize, on: bool) {
    if hwnd == 0 {
        return;
    }
    if on {
        {
            // menus come and go all day without glass ever being on: do not let their handles pile up
            let mut e = extras();
            if e.len() >= 64 {
                prune_extras(&mut e);
            }
            e.insert(hwnd);
        }
        if !any_active() {
            return;
        }
    } else {
        extras().remove(&hwnd);
    }
    if let Err(e) = set_affinity(hwnd, on) {
        log::debug!("glass: display affinity of {hwnd:#x} not changed: {e}");
    }
}

/// Draws the same glass the live path draws, through the same renderer code, over a picture the caller gives, into a private texture,
/// and returns its pixels as premultiplied BGRA, `rect` sized. Creates no window and no swap chain; tries a hardware device first and
/// falls back to WARP (a CI runner has no GPU).
///
/// `src_bgra` is `src_w` x `src_h` tightly packed BGRA8. `rect` is `(x, y, w, h)` of the glass in picture pixels (the part of it outside the
/// picture reads the picture's edge pixels). `radius_px` is the corner radius in device pixels, `scale` the device pixels per CSS pixel
/// (the window's dpi / 96).
pub fn render_offscreen(src_bgra: &[u8], src_w: u32, src_h: u32, rect: (i32, i32, u32, u32), radius_px: f32, scale: f32, dark: bool) -> Result<Vec<u8>, String> {
    gpu::render(src_bgra, src_w, src_h, rect, radius_px, scale, &gpu::Opts { look: gpu::Look::regular(dark), force_warp: false })
}

/// Whether the engine thread has been started (the tests use it to see that a failed attach starts nothing).
#[cfg(test)]
fn engine_started() -> bool {
    engine().is_some()
}
