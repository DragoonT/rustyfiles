//! Glass: a window background that bends and frosts the live screen behind the window (the experimental "Glass" backdrop).
//!
//! HOW IT WORKS. The screen is captured, the window itself is kept out of that capture (otherwise the glass would bend its own picture),
//! and one pixel shader (`glass.hlsl`) draws the glass over a cropped, mip-blurred copy of the screen into a swap chain that
//! DirectComposition shows BEHIND the window's own scene. GPUI is not changed: it makes its composition target with `topmost = true`,
//! the engine makes a second one on the same window with `topmost = false`, and the app sets the GPUI window transparent so the glass
//! shows through.
//!
//! TWO WAYS TO CAPTURE ([`CaptureMode`], chosen by the engine each time it starts). Where the system allows capture without the
//! screenshot border (Windows 11 24H2, and the `graphicsCaptureWithoutBorder` permission reads Allowed), a Windows.Graphics.Capture
//! session leaves the windows out through its own exclusion list: `CaptureMode::Session`, no window flag is set, and the windows stay in
//! every screenshot, recording and screen share. Otherwise DXGI Desktop Duplication is used and the windows are marked
//! `WDA_EXCLUDEFROMCAPTURE`: `CaptureMode::Affinity`, which hides them from every capture of the screen. A session that fails later makes
//! the engine switch to the second way for all its windows.
//!
//! Optical model and presets after tomacco/amazing-glass (MIT); the shader was written from numbers and formulas, no code is copied.
//!
//! THE LOOK. Four numbers the user can set (`LookParams`: frost, tint, refraction, saturation; the fifth setting, the app's own surface
//! wash, is the app's business) change the glass of every window at once and for windows attached later ([`set_look`]); the numbers
//! behind each value are in `gpu.rs` (`Look::regular`). A small piece of glass (a dock island, a tile) is drawn with a calmer rim than a
//! window (`Look::sized`).
//!
//! LENSES. On top of a window's glass the engine can draw up to `MAX_LENSES` lenses (the selection pill of a list, a tab): clear, sharp pieces
//! of the same glass with their own rim, light and shadow, drawn by the same pixel shader in the same pass ([`set_lens`], `lens.rs` for
//! the slots and fades, `spring.rs` for the slide). The app's own icons and text are drawn by GPUI's scene on top of them. While a lens
//! moves or fades the engine draws once per display refresh; a lens at rest costs nothing more than the glass itself.
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
#[doc(hidden)]
pub mod check;
mod engine;
mod gpu;
mod lens;
mod spring;
#[cfg(test)]
mod lens_tests;
#[cfg(test)]
mod look_tests;
#[cfg(test)]
mod tests;
mod wgc;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex};

use crossbeam_channel::Sender;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowDisplayAffinity, GetWindowThreadProcessId, IsWindow, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
};

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

/// How the engine keeps its own windows out of the picture it bends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureMode {
    /// The engine is not running (no window has glass): nothing is captured and nothing excluded.
    Off,
    /// A capture session with its own exclusion list: the windows are visible to screenshots, recordings and screen sharing.
    Session,
    /// Desktop Duplication with `WDA_EXCLUDEFROMCAPTURE` on the windows: they are hidden from screenshots, recordings and screen sharing.
    Affinity,
}

static MODE: AtomicU8 = AtomicU8::new(0);

/// How the windows with glass are kept out of the capture right now. `Off` while no window has glass (the engine chooses each time it
/// starts, and again after it was idle), so the answer for a window that has just been attached is only known once the engine has chosen.
pub fn capture_mode() -> CaptureMode {
    match MODE.load(Ordering::Acquire) {
        1 => CaptureMode::Session,
        2 => CaptureMode::Affinity,
        _ => CaptureMode::Off,
    }
}

/// Engine side: records the choice.
pub(crate) fn set_mode(m: CaptureMode) {
    MODE.store(
        match m {
            CaptureMode::Off => 0,
            CaptureMode::Session => 1,
            CaptureMode::Affinity => 2,
        },
        Ordering::Release,
    );
}

/// How many lenses a window can have at once (slots 0 to 7). Keep equal to `MAX_LENSES` in glass.hlsl.
pub const MAX_LENSES: usize = 8;

/// What the user can set about the look of the glass, each from 0 to 100 (values outside are cut to that, a value that is not a number
/// is the default). Each one moves a few numbers of the optical model; the default is the look the app shipped with, so a setting that
/// is not touched changes nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookParams {
    /// The blur: 0 is clear glass, 100 a deviation of 20 CSS px (the default is 10.35 px).
    pub frost: f32,
    /// How much of the theme's tone lies over the picture and, in the dark theme, how much it is dimmed: 0 none, 100 about twice the
    /// default.
    pub tint: f32,
    /// The thickness of the rim: 0 no bending at the edges, 100 one and a half times the default.
    pub refraction: f32,
    /// 0 keeps the picture's own colours, 100 makes them three times as strong.
    pub saturation: f32,
}

impl LookParams {
    /// The shipped look.
    pub const DEFAULT: LookParams = LookParams { frost: 52.0, tint: 50.0, refraction: 67.0, saturation: 62.0 };

    /// The same with every value in 0..=100 (a value that is not a number becomes the default one).
    pub fn clamped(self) -> LookParams {
        let c = |v: f32, d: f32| if v.is_nan() { d } else { v.clamp(0.0, 100.0) };
        LookParams {
            frost: c(self.frost, Self::DEFAULT.frost),
            tint: c(self.tint, Self::DEFAULT.tint),
            refraction: c(self.refraction, Self::DEFAULT.refraction),
            saturation: c(self.saturation, Self::DEFAULT.saturation),
        }
    }
}

impl Default for LookParams {
    fn default() -> LookParams {
        LookParams::DEFAULT
    }
}

/// A lens: a piece of clear glass on the window's glass (the selection pill of a list). All coordinates are DEVICE px relative to the
/// window's client area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensSpec {
    /// What the lens is on (the selected entry). The same key again means the place changed but not the selection; another key means the
    /// selection moved and the lens slides to the new rectangle.
    pub key: u64,
    /// x, y, w, h.
    pub rect: [f32; 4],
    /// x, y, w, h: nothing of the lens shows outside it (a list that scrolls cuts the pill at its own edge).
    pub clip: [f32; 4],
    /// Corner radius; a capsule is half the height.
    pub radius: f32,
}

pub(crate) enum Cmd {
    Attach { hwnd: isize, opts: GlassOptions },
    SetOptions { hwnd: isize, opts: GlassOptions },
    SetDark { hwnd: isize, dark: bool },
    Detach { hwnd: isize },
    /// The registry of extra windows changed (see [`exclude_from_capture`]).
    ExtrasChanged,
    SetLens { hwnd: isize, slot: u8, lens: Option<LensSpec>, animate: bool },
    /// The process-wide look changed (`set_look`): every window draws again.
    LookChanged,
}

/// What the callers see. The engine thread updates it through `set_state`, `fail` and `forget`.
static STATES: LazyLock<Mutex<HashMap<isize, GlassState>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// The engine thread's command channel, once it has been started.
static ENGINE: Mutex<Option<Sender<Cmd>>> = Mutex::new(None);
/// What the app last asked for in each lens slot, so the same call made again (the UI draws many times a second) sends nothing. An absent
/// entry is the same as `None`. Cleared for a window when its glass starts, ends or fails.
static LENSES_SENT: Mutex<BTreeMap<(isize, u8), LensSpec>> = Mutex::new(BTreeMap::new());
/// The look of the glass, one value for the whole process: the engine reads it at every draw, so a window attached later has it too.
static LOOK: Mutex<LookParams> = Mutex::new(LookParams::DEFAULT);
/// Other windows of ours (menus, flyouts, the Properties window, the preview pane's host) that asked to stay out of the screen capture.
/// The exclusion only exists while some window has glass. With `CaptureMode::Affinity` it is applied when the first one gets it and
/// lifted when the last one lets go, so a window that outlives the glass does not stay invisible to screenshots and screen sharing; with
/// `CaptureMode::Session` the engine puts these windows into the capture sessions' exclusion lists and no window is touched.
static EXTRAS: Mutex<BTreeSet<isize>> = Mutex::new(BTreeSet::new());

fn states() -> std::sync::MutexGuard<'static, HashMap<isize, GlassState>> {
    STATES.lock().unwrap_or_else(|p| p.into_inner())
}

fn engine() -> std::sync::MutexGuard<'static, Option<Sender<Cmd>>> {
    ENGINE.lock().unwrap_or_else(|p| p.into_inner())
}

fn lenses_sent() -> std::sync::MutexGuard<'static, BTreeMap<(isize, u8), LensSpec>> {
    LENSES_SENT.lock().unwrap_or_else(|p| p.into_inner())
}

fn forget_lenses(hwnd: isize) {
    lenses_sent().retain(|&(h, _), _| h != hwnd);
}

fn look_guard() -> std::sync::MutexGuard<'static, LookParams> {
    LOOK.lock().unwrap_or_else(|p| p.into_inner())
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

/// The registered windows that still exist.
pub(crate) fn extras_snapshot() -> BTreeSet<isize> {
    let mut e = extras();
    prune_extras(&mut e);
    e.clone()
}

/// Applies or lifts the display-affinity exclusion on every registered window.
fn apply_extras(exclude: bool) {
    for h in extras_snapshot() {
        let _ = set_affinity(h, exclude);
    }
}

/// Lifts the display-affinity exclusion of a window if it has one (an earlier capture mode may have set it; with the session backend no
/// window carries it).
pub(crate) fn clear_affinity(hwnd: isize) {
    let mut a = 0u32;
    if unsafe { GetWindowDisplayAffinity(hwnd_of(hwnd), &mut a) }.is_ok() && a != WDA_NONE.0 {
        let _ = set_affinity(hwnd, false);
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
            set_mode(CaptureMode::Off);
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
    forget_lenses(hwnd);
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
    forget_lenses(hwnd);
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
/// exclusion is refused, the engine thread does not start); the reason is in [`state`] and the log. The window is kept out of the capture
/// the engine bends, or the glass would bend the window's own picture: by the engine's session exclusion list, which needs nothing here,
/// or, when the engine is already capturing with Desktop Duplication, by the display-affinity flag set here first (when the engine
/// starts, it sets the flag itself before it captures anything).
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
    let affinity = capture_mode() == CaptureMode::Affinity;
    if affinity && let Err(e) = set_affinity(hwnd, true) {
        return fail_now(format!("the window cannot be excluded from screen capture: {e}"));
    }
    let first = !any_active();
    forget_lenses(hwnd);
    states().insert(hwnd, GlassState::Starting);
    if !send(Cmd::Attach { hwnd, opts }) {
        if affinity {
            let _ = set_affinity(hwnd, false);
        }
        return fail_now("the glass engine thread could not be started".into());
    }
    if first && affinity {
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
    forget_lenses(hwnd);
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

/// Puts a lens into one of the window's slots (0 to `MAX_LENSES` - 1), or takes it out with `None`. The engine keeps the lens's motion:
/// - into an empty slot the lens appears (it fades in over about 120 ms and grows from 92 % of its size; `animate = false`: it is there);
/// - with the key the slot's lens has, the place changed but not the selection (a scroll, a resize): the lens follows at once (a slide
///   that is under way keeps going towards the new place);
/// - with another key the selection moved: the lens slides to the new rectangle, stretching in the direction of travel like a drop and
///   settling in about 0.4 s (`animate = false`: it jumps);
/// - `None` fades it out (about 120 ms; `animate = false`: gone at once).
///
/// Ignored for a window that has no glass (Mica, Acrylic, a failed or detached window), for a slot that does not exist and for a lens
/// that cannot be drawn (not finite, no size). Calling it again with what the slot already has costs nothing, so the UI may call it on
/// every draw. Cheap from any thread.
pub fn set_lens(hwnd: isize, slot: u8, lens: Option<LensSpec>, animate: bool) {
    if slot as usize >= MAX_LENSES || !matches!(states().get(&hwnd), Some(GlassState::Starting | GlassState::Running)) {
        return;
    }
    {
        let mut sent = lenses_sent();
        if sent.get(&(hwnd, slot)).copied() == lens {
            return;
        }
        match lens {
            Some(l) => sent.insert((hwnd, slot), l),
            None => sent.remove(&(hwnd, slot)),
        };
    }
    send_if_running(Cmd::SetLens { hwnd, slot, lens, animate });
}

/// Sets the look of the glass for every window that has it and for windows that get it later. The values are cut to 0..=100. Every glass
/// window is drawn again once; calling it with what is set already does nothing. Cheap from any thread, so a slider may call it for
/// every step of a drag.
pub fn set_look(params: LookParams) {
    let params = params.clamped();
    {
        let mut cur = look_guard();
        if *cur == params {
            return;
        }
        *cur = params;
    }
    // without a running engine nothing is drawn; the thread that starts later reads the value itself
    send_if_running(Cmd::LookChanged);
}

/// The look set by [`set_look`] (the default until then).
pub fn look() -> LookParams {
    *look_guard()
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
    match capture_mode() {
        // the sessions' lists change; no window is touched
        CaptureMode::Session => {
            send_if_running(Cmd::ExtrasChanged);
        }
        // not chosen yet: the engine reads the registry when it chooses
        CaptureMode::Off => {}
        CaptureMode::Affinity => {
            if let Err(e) = set_affinity(hwnd, on) {
                log::debug!("glass: display affinity of {hwnd:#x} not changed: {e}");
            }
        }
    }
}

/// Draws the same glass the live path draws, through the same renderer code, over a picture the caller gives, into a private texture,
/// and returns its pixels as premultiplied BGRA, `rect` sized. Creates no window and no swap chain; tries a hardware device first and
/// falls back to WARP (a CI runner has no GPU). The look is the default one ([`LookParams::DEFAULT`]), not the process-wide one. A
/// rectangle whose short side is under 160 CSS px gets the calmer rim of a small piece, as a small piece does in a window; every larger one
/// (every window) is drawn with the look as it is. [`render_offscreen_look`] with `calm_small = false` draws a small piece with the rim of a window.
///
/// `src_bgra` is `src_w` x `src_h` tightly packed BGRA8. `rect` is `(x, y, w, h)` of the glass in picture pixels (the part of it outside the
/// picture reads the picture's edge pixels). `radius_px` is the corner radius in device pixels, `scale` the device pixels per CSS pixel
/// (the window's dpi / 96).
pub fn render_offscreen(src_bgra: &[u8], src_w: u32, src_h: u32, rect: (i32, i32, u32, u32), radius_px: f32, scale: f32, dark: bool) -> Result<Vec<u8>, String> {
    let look = shot_look(dark, &LookParams::DEFAULT, true, rect, scale);
    gpu::render(src_bgra, src_w, src_h, rect, radius_px, scale, &gpu::Opts { look, force_warp: false })
}

/// The look of an offscreen picture: the settings' numbers, and for a small piece the calmer rim (`calm_small`; a window is above the
/// size where that starts, so for it the flag changes nothing).
fn shot_look(dark: bool, params: &LookParams, calm_small: bool, rect: (i32, i32, u32, u32), scale: f32) -> gpu::Look {
    let look = gpu::Look::regular(dark, params);
    if calm_small { look.sized(rect.2.min(rect.3) as f32 / scale) } else { look }
}

/// One lens of an offscreen picture (see [`render_offscreen_frames`]). Device px relative to the glass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensShot {
    /// x, y, w, h.
    pub rect: [f32; 4],
    /// x, y, w, h; `None`: the whole glass.
    pub clip: Option<[f32; 4]>,
    pub radius: f32,
    /// 0 to 1.
    pub opacity: f32,
}

/// The same as [`render_offscreen`] with lenses, one picture per entry of `frames` (each entry the lenses of that picture, at most
/// `MAX_LENSES` of them): the backdrop is prepared once and the pictures differ in their lenses only. The default look, with the calmer rim
/// for a rectangle under 160 CSS px (see [`render_offscreen`]). For tests and the picture harness.
pub fn render_offscreen_frames(
    src: (&[u8], u32, u32),
    rect: (i32, i32, u32, u32),
    radius_px: f32,
    scale: f32,
    dark: bool,
    frames: &[Vec<LensShot>],
) -> Result<Vec<Vec<u8>>, String> {
    render_offscreen_look(src, rect, radius_px, scale, dark, &LookParams::DEFAULT, true, frames)
}

/// [`render_offscreen_frames`] with the settings' values; `calm_small = false` draws a small piece with the rim of a window (what it
/// looked like before the calmer rim), for comparing pictures.
#[allow(clippy::too_many_arguments)]
pub fn render_offscreen_look(
    src: (&[u8], u32, u32),
    rect: (i32, i32, u32, u32),
    radius_px: f32,
    scale: f32,
    dark: bool,
    params: &LookParams,
    calm_small: bool,
    frames: &[Vec<LensShot>],
) -> Result<Vec<Vec<u8>>, String> {
    let (src_bgra, src_w, src_h) = src;
    let whole = [0.0, 0.0, rect.2 as f32, rect.3 as f32];
    let lists: Vec<gpu::LensList> = frames
        .iter()
        .map(|f| gpu::LensList::from_draws(f.iter().map(|l| gpu::LensDraw { rect: l.rect, clip: l.clip.unwrap_or(whole), radius: l.radius, opacity: l.opacity })))
        .collect();
    let look = shot_look(dark, params, calm_small, rect, scale);
    gpu::render_frames(src_bgra, (src_w, src_h), &gpu::free_standing(rect, radius_px, scale), &gpu::Opts { look, force_warp: false }, &lists)
}

/// The rectangles (x, y, w, h) of a lens that slides from `from` to `to`, one per display refresh at `hz`, with the engine's own springs:
/// the first is `from`, the last is `to`. For the picture harness, which has no window to watch.
pub fn simulate_slide(from: [f32; 4], to: [f32; 4], hz: f32) -> Vec<[f32; 4]> {
    let mut s = spring::RectSpring::at(from);
    s.slide_to(to);
    let mut out = vec![s.rect()];
    let dt = 1.0 / hz.clamp(1.0, 1000.0);
    while !s.settled() && out.len() < 4000 {
        s.step(dt);
        out.push(s.rect());
    }
    out
}

/// Whether the engine thread has been started (the tests use it to see that a failed attach starts nothing).
#[cfg(test)]
fn engine_started() -> bool {
    engine().is_some()
}
