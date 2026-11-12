//! An island that grows into its panel: the part that carries the pure motion (`rf_win::dock::grow`) out on windows, glass and the
//! pointer. Today only the Start panel uses it (setting "Başlat adadan büyüyerek açılsın", `dock_start_grow`, on by default); the
//! right island's panels (notifications, calendar, control centre) are meant to open the same way next, see "Moving the right island's
//! panels onto it" at the end of this comment.
//!
//! WHAT WAS BUILT: THE FALLBACK OF THE BRIEF, AND WHY. The brief preferred hosting the panel's content in the island's own window, so
//! that one window's glass changes shape. That means the island's window (338 x 56 px, never activated, with the system's rules for an
//! island) becoming the Start panel's window (642 x 776, activated, with the keyboard, the text field, the menus, the glass of a
//! popup) and back, with `panel.rs`'s slot, closing rules and keyboard hand-back all rebuilt around a window that is also an island.
//! Nothing of that can be seen or tried from where this was written (no window may be shown), and a mistake in it would cost the user
//! the island, not only the panel. So the panel keeps its own window and everything that works: it is activated, takes the keyboard,
//! gives it back. What changed is its SIZE and its GLASS:
//!
//! * The panel's window is the GROWN rectangle: the panel's rectangle taken down to the island's bottom edge (the panel's top, the
//!   8 px gap, and the island's own row at the bottom). Its glass is a SHAPE inside that window (`GlassOptions::shape`, the way the
//!   middle island's glass is a shape inside its larger window), and every frame of the motion moves the shape and its corner radius
//!   (`glass::set_options`: one constant and one draw, no new picture of the screen) and the window's REGION with it (`hit::set_region`),
//!   so a click outside the visible glass goes to whatever lies below.
//! * The island's own glass is put away (`IslandGlass::set_hidden`: no shape, the tone frozen) for as long as the panel's glass stands
//!   in its place, and the island's window (that island only) is raised above the panel's and put back above it whenever the panel's
//!   activation has lifted it ([`keep_island_above`], from the panel's own 50 ms watch), so the logo stays where it is, is pressed
//!   where it is, and the eye sees ONE glass growing. The panel's wash (the dark or light tint over its glass, which the island's own
//!   glass does not have) comes in with the growth ([`ViewFrame::wash_alpha`]), so there is no step in the tone at either end. The island's window is never activated and its region stays its own rectangle: a click on the logo
//!   reaches the logo, whatever the panel's region is.
//! * The island's search field is not drawn while it is away; the panel's field is drawn in its place and travels (the panel's view
//!   does it: `start.rs` asks [`view_frame`] for the field's rectangle). The weather fades with [`island_fade`].
//!
//! WHERE IT DOES NOT HAPPEN. The grown glass is as wide as the panel down to the island's row. Where another island begins inside that
//! width ([`fits`]: 1920 px wide at 125 %, 1366 or 1600 px at 100 %) the glass would lie under that island, so the panel opens the
//! ordinary way there (one info line says why). The glass engine's shape is one rectangle: no L-shaped glass.
//!
//! THE CLOCK. [`rf_win::dock::grow::Run`] (phase, clock, animate; pure and tested) is started when the panel's glass is RUNNING (or
//! after `grow::ARM_MAX_MS` = 90 ms, so a glass that never comes does not hold the panel back): until then the island's own glass is
//! still the visible one and the panel is only its own rectangle (with no wash), so the hand-over from one glass to the other happens at
//! the first frame of the motion, where they are the same rectangle with the same radius, not in the middle of it. With animations off
//! the wait is the same (for the glass, not for time) and the panel is whole in one frame; its closing is one frame too and the island
//! gets its glass back in the deadline before the window is hidden, like in every closing. The button pressed again during a closing
//! turns the clock round and keeps what the panel shows (`panel.rs` calls `on_reopen`, not `on_open`).
//!
//! THE WINDOW CALLS. `set_frameless`, the region and the raising of an island are made from the message loop with nothing of the app
//! borrowed (a task, or the body of the task that shows the window), as the notes in `host.rs` ask. The glass engine's `set_options` (also
//! of the island's glass, `set_hidden`) is a message to the engine's thread, not a window call, and is made where the state is.
//!
//! WHO DRIVES THE FRAMES. [`view_frame`], called by the panel's view from its `render`, reads the clock, queues the glass shape and the
//! region for that frame (one task of the message loop, never a window call from inside a render; nothing is queued when the frame is
//! the one already sent) and says whether another frame is wanted (`moving`): the view asks for one with `request_animation_frame`, so
//! the frames follow the display. The island's view asks [`island_fade`] the same way. When the motion has arrived nothing asks for
//! another frame. A DEADLINE task per motion (a timer at the motion's end plus 25 ms, replaced at every turn) applies the last frame
//! whatever the views did, ends the motion, and logs it, so the glass, the region and the island are right even if no frame is ever
//! drawn again (a hidden window, a stalled renderer). The close path has its own net: `finish` is called when the window is hidden and
//! gives the island its glass and the panel's window its region and frame back whether the deadline ran or not.
//!
//! LOG (info). `dock: grow start: ...` when the clock starts or turns, `dock: grow end: ... after N frames in M ms` when the motion has
//! arrived. N counts the frames the panel's view drew for the motion (about 14 for 220 ms on a 60 Hz display, 27 on 120 Hz). After the
//! end line nothing of this runs: no task, no frame request (the views ask for a frame only while `moving`), and the next line of this
//! kind comes with the next press.
//!
//! Moving the right island's panels onto it: [`config`] gets a row for each panel (`Kind::Right`, `Stays::Row`, no field); the panel's
//! view asks [`view_frame`] and draws its content inside the part above the island's row (`extra_bottom` is the island's row and the
//! gap, so the view keeps it empty), and the right island's view asks [`island_fade`] (for `Stays::Row` it answers "nothing fades").
//! `panel.rs` already calls the rest for any slot that has a [`config`].

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use rf_win::dock::grow::{self, Again, Arrival, Corner, FRect, Frame, Phase, Run, Shrink, Spec, Stays};
use rf_win::dock::hit;
use rf_win::dock::island;
use rf_win::dock::layout::{self, Rect};
use rf_win::dock::panel::{PanelId, Side};
use rf_win::glass::GlassOptions;

use super::backdrop::{self, IslandGlass};
use super::host::{self, Kind};
use super::plan;
use crate::settings::Settings;

/// The corner radius the system gives a large panel (8 px at 100 %): the panel's glass ends with it.
const PANEL_RADIUS: f32 = 8.0;

/// How a panel is grown: which island it grows out of and what of that island stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cfg {
    pub kind: Kind,
    pub stays: Stays,
}

/// Whether (and how) this panel grows out of its island right now. The Start panel does when the setting is on; no other panel has a row
/// yet.
pub fn config(id: PanelId, cx: &App) -> Option<Cfg> {
    if id != PanelId::START {
        return None;
    }
    let s = cx.try_global::<Settings>()?;
    (s.dock_start_grow && s.dock_own_start).then_some(Cfg { kind: Kind::Left, stays: Stays::Logo })
}

/// Whether this panel is the one that is growing (open, or shrinking back and not hidden yet).
pub fn active_for(id: PanelId, cx: &App) -> bool {
    grow_ref(cx).is_some_and(|g| g.id == id)
}

struct Grow {
    id: PanelId,
    cfg: Cfg,
    spec: Spec,
    /// The life of the growth: its phase and its clock (pure, `rf_win::dock::grow::Run`; every decision of when is made there).
    run: Run,
    hwnd: isize,
    island: Rc<RefCell<IslandGlass>>,
    dark: bool,
    /// The monitor's scale: the panel window's logical px.
    scale: f32,
    /// How far the grown glass reaches below the panel's own rectangle (the gap and the island's row), device px.
    extra: i32,
    shown_at: Instant,
    began: Instant,
    frames: u32,
    /// The last shape and radius queued for the window, and its area (to send the smaller glass before the smaller region).
    sent: Option<(FRect, f32)>,
    arm: Option<Task<()>>,
    deadline: Option<Task<()>>,
}

#[derive(Default)]
struct GrowGlobal(Option<Grow>);

impl Global for GrowGlobal {}

fn grow_ref(cx: &App) -> Option<&Grow> {
    cx.try_global::<GrowGlobal>()?.0.as_ref()
}

fn grow_mut(cx: &mut App) -> Option<&mut Grow> {
    if !cx.has_global::<GrowGlobal>() {
        return None;
    }
    cx.global_mut::<GrowGlobal>().0.as_mut()
}

/// Runs `job` from the message loop, outside any update of the app (every call that touches a window, a region or the glass engine is
/// made there: see the notes at the top of `host.rs`).
fn later(cx: &App, job: impl FnOnce() + 'static) {
    cx.foreground_executor()
        .spawn(async move {
            job();
        })
        .detach();
}

fn screen_rect(r: FRect) -> Rect {
    Rect::new(r[0].floor() as i32, r[1].floor() as i32, (r[0] + r[2]).ceil() as i32, (r[1] + r[3]).ceil() as i32)
}

// ---------------------------------------------------------------- what a frame sends

/// What one frame sends to the window: its region and its glass.
struct Push {
    hwnd: isize,
    dark: bool,
    parts: Vec<hit::Part>,
    shape: FRect,
    radius: f32,
    /// The glass got bigger than the last one sent: the region goes first, so the system never clips a glass that has just grown; for
    /// a smaller one the glass goes first and the region follows.
    growing: bool,
}

impl Push {
    fn run(self) {
        let glass = || {
            rf_win::glass::set_options(
                self.hwnd,
                GlassOptions { dark: self.dark, radius_px: Some(self.radius), cut_corners: true, shape: Some(self.shape), shadow: None },
            )
        };
        if self.growing {
            hit::set_region(self.hwnd, &self.parts);
            glass();
        } else {
            glass();
            hit::set_region(self.hwnd, &self.parts);
        }
    }
}

impl Grow {
    fn frame(&self, now: u64) -> Frame {
        self.spec.at(self.run.s_at(now))
    }

    /// The frame as a thing to send, or `None` when it is the one already sent.
    fn plan_send(&mut self, f: &Frame) -> Option<Push> {
        let shape = self.spec.in_window(f);
        if self.sent == Some((shape, f.radius)) {
            return None;
        }
        let area = |s: &FRect| s[2] * s[3];
        let growing = self.sent.is_none_or(|(old, _)| area(&shape) >= area(&old));
        self.sent = Some((shape, f.radius));
        let parts = hit::parts(shape, f.radius, &[], self.spec.grown.width(), self.spec.grown.height());
        Some(Push { hwnd: self.hwnd, dark: self.dark, parts, shape, radius: f.radius, growing })
    }
}

// ---------------------------------------------------------------- starting

/// What `panel.rs` knows when a panel that grows is opened.
pub struct Prep {
    pub id: PanelId,
    pub cfg: Cfg,
    pub side: Side,
    /// The island's visible glass and the panel's content rectangle (`rf_win::dock::panel::place`), device px, screen coordinates.
    pub island_glass: Rect,
    pub panel: Rect,
    pub hwnd: isize,
    pub island: Rc<RefCell<IslandGlass>>,
    pub dark: bool,
    /// The monitor's scale (the panel's lengths are in it; the island's are in dock units).
    pub scale: f32,
}

/// The search field's two places: in the left island (the layout of `left.rs`: padding, logo, gap, field, centred upright) and at the top
/// of the panel (`start_home.rs::search_pill`: 28 px from the sides, 24 under the top, 36 high). Screen coordinates, device px.
fn field_places(island: Rect, panel: Rect, unit: f32, scale: f32) -> (FRect, FRect) {
    use rf_win::dock::startpage as page;
    let h = plan::SEARCH_H * unit;
    let from = [island.left as f32 + (plan::SIDE_PAD + plan::LOGO + plan::LOGO_GAP) * unit, island.top as f32 + (island.height() as f32 - h) / 2.0, plan::SEARCH_W * unit, h];
    let to = [panel.left as f32 + page::SIDE_PAD * scale, panel.top as f32 + page::TOP_PAD * scale, panel.width() as f32 - 2.0 * page::SIDE_PAD * scale, page::SEARCH_H * scale];
    (from, to)
}

/// Whether the grown glass of a panel that grows out of `cfg.kind`'s island would NOT lie under another island. The grown glass is as
/// wide as the panel all the way down to the island's row, so on a screen where the next island begins inside that width (a 1920 px
/// wide screen at 125 %, a 1366 or 1600 px wide one at 100 %) it would sit behind that island's first icons, tint what the island's own
/// glass shows and take its clicks. Such a panel does not grow: it opens the ordinary way, a stand-alone pop-up above the island with
/// its own 120 ms easing, exactly as with the setting off. Logged (once for each opening).
pub fn fits(cfg: Cfg, island_glass: Rect, panel: Rect, scale: f32, cx: &App) -> bool {
    let grown = grow::grown_rect(island_glass, panel);
    let margin = layout::px(rf_win::dock::panel::GAP, scale);
    for kind in [Kind::Left, Kind::Middle, Kind::Right] {
        if kind == cfg.kind {
            continue;
        }
        if let Some(other) = host::visible_glass_rect(kind, cx)
            && grow::reaches_other_island(grown, other, margin)
        {
            log::info!(
                "dock: grow skipped: the grown glass ({}x{} px) would lie under the {kind:?} island ({}x{} px at {},{}); the panel opens as an ordinary pop-up",
                grown.width(),
                grown.height(),
                other.width(),
                other.height(),
                other.left,
                other.top
            );
            return false;
        }
    }
    true
}

/// Sets up (or, for a panel that is already growing or shrinking, updates) the growth of a panel that is being opened, and returns the
/// panel window's rectangle: the grown rectangle. `None` when there is no layout yet; the panel then opens as an ordinary one.
/// Opening it again while it is closing keeps the clock where it is and turns it round.
pub fn prepare(p: Prep, cx: &mut App) -> Option<Rect> {
    let unit = host::dock_unit(cx)?;
    let mut spec = Spec::new(p.island_glass, p.panel, Corner::from(p.side), layout::RADIUS * unit, PANEL_RADIUS * p.scale);
    if p.cfg.stays == Stays::Logo {
        let (from, to) = field_places(p.island_glass, p.panel, unit, p.scale);
        spec = spec.with_field(from, to);
    }
    let grown = spec.grown;
    let extra = grown.bottom - p.panel.bottom;
    let animate = crate::settings::animations();
    let now = backdrop::now_ms();
    if let Some(g) = grow_mut(cx)
        && g.id == p.id
    {
        // already there (the second look after the view has opened, or the button pressed again during a closing): the rectangles may
        // have moved, the clock is kept
        g.spec = spec;
        g.extra = extra;
        g.dark = p.dark;
        g.sent = None;
        if let Again::TurnedBack { from, remaining_ms } = g.run.reopen(now, animate) {
            log::info!("dock: grow start: turned back to opening from s={from:.2}, {remaining_ms} ms to go");
            schedule_deadline(remaining_ms, cx);
        }
        return Some(grown);
    }
    // (a growth of another panel that was left behind gives the island its glass back first)
    release(cx);
    cx.set_global(GrowGlobal(Some(Grow {
        id: p.id,
        cfg: p.cfg,
        spec,
        run: Run::new(animate),
        hwnd: p.hwnd,
        island: p.island,
        dark: p.dark,
        scale: p.scale,
        extra,
        shown_at: Instant::now(),
        began: Instant::now(),
        frames: 0,
        sent: None,
        arm: None,
        deadline: None,
    })));
    Some(grown)
}

/// Whether this panel is on its way back into its island right now: the button pressed now is a REVERSAL of the motion (the panel keeps
/// what it shows: the typed text, the page), not a new opening.
pub fn is_closing(id: PanelId, cx: &App) -> bool {
    grow_ref(cx).is_some_and(|g| g.id == id && g.run.is_closing())
}

/// Called from the task that shows the panel's window, BEFORE it is shown: what to do (outside any update of the app, the window calls
/// of it are the message loop's) so that the window cuts its own corners from now on and its region is the island's rectangle (or
/// wherever the glass is, when this is a turn round): the window never shows as a whole for a frame. `None`: no growth.
pub fn before_show(cx: &mut App) -> Option<impl FnOnce() + use<>> {
    let now = backdrop::now_ms();
    let g = grow_mut(cx)?;
    let hwnd = g.hwnd;
    let f = g.frame(now);
    let send = g.plan_send(&f);
    Some(move || {
        grow::set_frameless(hwnd, true);
        if let Some(send) = send {
            send.run();
        }
    })
}

/// The island's window (the one the glass grows out of): to be raised above the panel's after the panel is in front, so that the logo
/// (or the island's row) is drawn over the grown glass and takes its clicks. Only that island: the others are not under the glass
/// (`fits`) and are none of the growth's business (the app menu, the hover preview and the tray tip stay where they are in the order).
/// The caller raises it (`rf_win::dock::island::raise`) outside any update of the app.
pub fn island_to_raise(cx: &App) -> Option<isize> {
    let g = grow_ref(cx)?;
    host::island_window(g.cfg.kind, cx).map(|(_, hwnd)| hwnd)
}

/// Puts the island back above the panel when something has put the panel above it: the activation of a topmost window raises it over
/// the other topmost windows (a native menu or dialog of the panel returning, the panel taken back by a click, a reversal). A look at
/// the order first, so that nothing is sent while the island is where it belongs. Called from the panel's own watch (every 50 ms while
/// it is open); the raising is a message-loop task like every window call.
pub fn keep_island_above(cx: &App) {
    let Some(g) = grow_ref(cx) else { return };
    let Some((_, isl)) = host::island_window(g.cfg.kind, cx) else { return };
    if island::is_above(g.hwnd, isl) {
        later(cx, move || island::raise(isl));
    }
}

/// The panel is on the screen and its glass has been asked for: the clock starts when the glass runs (or after the glass has had its time,
/// `grow::ARM_MAX_MS`; with animations off too: the wait is for the glass and not for any motion, so that the island's own glass is
/// not put away while the panel's is still starting; or at once when the glass is running already: a turn round).
pub fn shown(cx: &mut App) {
    let Some(g) = grow_mut(cx) else { return };
    g.shown_at = Instant::now();
    if g.run.phase != Phase::Arming {
        return;
    }
    let hwnd = g.hwnd;
    if glass_running(hwnd) {
        start_clock(cx);
        return;
    }
    let task = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(8)).await;
            if cx.update(arm_step) {
                break;
            }
        }
    });
    if let Some(g) = grow_mut(cx) {
        g.arm = Some(task);
    }
}

fn glass_running(hwnd: isize) -> bool {
    matches!(rf_win::glass::state(hwnd), rf_win::glass::GlassState::Running)
}

/// One look of the wait for the panel's glass. True when the wait is over.
fn arm_step(cx: &mut App) -> bool {
    let Some(g) = grow_ref(cx) else { return true };
    if g.run.phase != Phase::Arming {
        return true;
    }
    if grow::arm_done(glass_running(g.hwnd), g.shown_at.elapsed().as_millis() as u64) {
        start_clock(cx);
        return true;
    }
    false
}

/// The glass runs: the clock starts, the island's glass is put away (the panel's glass has the island's rectangle and radius at this
/// first frame, and no wash yet: `Run::wash_alpha`), and both views draw for it.
fn start_clock(cx: &mut App) {
    let now = backdrop::now_ms();
    let Some(g) = grow_mut(cx) else { return };
    let rem = g.run.start(now);
    g.began = Instant::now();
    g.frames = 0;
    g.arm = None;
    g.island.borrow_mut().set_hidden(true);
    let (a, b) = (g.spec.island, g.spec.grown);
    log::info!(
        "dock: grow start: opening, {rem} ms (animations {}), glass {}x{} -> {}x{} px, waited {:?} for the glass",
        if g.run.animate { "on" } else { "off" },
        a.width(),
        a.height(),
        b.width(),
        b.height(),
        g.shown_at.elapsed()
    );
    schedule_deadline(rem, cx);
    nudge_views(cx);
}

/// How long after a motion's end the deadline looks at it (ms): the last frame is applied and the motion ended then, whether or not a
/// view drew it.
const DEADLINE_SLACK_MS: u64 = 25;

/// How long after a closing motion's end the window is hidden (ms): after the deadline has given the island its glass back, which the
/// engine needs a frame or two to draw. The two glasses stand in the same rectangle meanwhile and the panel's wash is gone by then.
const WINDOW_STAYS_MS: u32 = 60;

/// One timer per motion: at its end (and a little over) the last frame is applied and the motion is ended, whether or not a view drew it.
fn schedule_deadline(remaining_ms: u32, cx: &mut App) {
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_millis(u64::from(remaining_ms) + DEADLINE_SLACK_MS)).await;
        cx.update(settle);
    });
    if let Some(g) = grow_mut(cx) {
        g.deadline = Some(task);
    }
}

/// The deadline: if the motion has arrived, applies its last frame and ends it (the island gets its glass back when it came home);
/// if not (a clock that ran late), waits for the rest.
fn settle(cx: &mut App) {
    let now = backdrop::now_ms();
    let Some(g) = grow_mut(cx) else { return };
    let opening = match g.run.settle(now) {
        Arrival::Idle => return,
        Arrival::Late { remaining_ms } => {
            schedule_deadline(remaining_ms, cx);
            return;
        }
        Arrival::Opened => true,
        Arrival::Home => false,
    };
    let f = g.frame(now);
    let send = g.plan_send(&f);
    log::info!(
        "dock: grow end: {} finished after {} frames in {:?}",
        if opening { "opening" } else { "closing" },
        g.frames,
        g.began.elapsed()
    );
    g.deadline = None;
    if !opening {
        // home: the island's glass is back (same rectangle, same radius as the panel's now, and no wash on it), the field is the island's again
        g.island.borrow_mut().set_hidden(false);
    }
    if let Some(send) = send {
        later(cx, move || send.run());
    }
    if opening {
        keep_island_above(cx);
    }
    nudge_views(cx);
}

fn nudge_views(cx: &mut App) {
    if let Some(v) = super::panel::view(cx) {
        v.update(cx, |_, cx| cx.notify());
    }
    if let Some(l) = super::panel::left_view(cx) {
        l.update(cx, |_, cx| cx.notify());
    }
}

// ---------------------------------------------------------------- closing

// (the end of a motion that was cut short is logged by `release`)

/// The panel is being put away: the clock turns towards the island. Returns how long the window must stay, ms (0: it may go at once),
/// `None` when no growth is going on. A panel that closes before its glass ever came (still waiting) has nothing to shrink. With
/// animations off the clock arrives at once, and the island still gets its glass back in the deadline before the window goes, like in
/// every closing.
pub fn begin_close(cx: &mut App) -> Option<u32> {
    let now = backdrop::now_ms();
    let g = grow_mut(cx)?;
    match g.run.begin_close(now) {
        Shrink::Nothing => {
            g.arm = None;
            g.deadline = None;
            Some(0)
        }
        Shrink::Shrinking { from, remaining_ms } => {
            log::info!("dock: grow start: closing from s={from:.2}, {remaining_ms} ms{}", if g.run.animate { "" } else { " (animations off)" });
            g.began = Instant::now();
            g.frames = 0;
            schedule_deadline(remaining_ms, cx);
            nudge_views(cx);
            Some(remaining_ms + WINDOW_STAYS_MS)
        }
    }
}

/// The panel goes at once (another panel takes its place, the layout changed, a full-screen program came): no motion, the island has its
/// glass, the window has its region and its frame back. Called BEFORE the window is hidden where the order is ours (the engine is told
/// to draw the island's glass first).
pub fn abort(cx: &mut App) {
    release(cx);
}

/// The panel's window has been hidden: the island has its glass (it has, since the deadline: this is the net for a deadline that never
/// ran), the window its region and its system frame (the next opening, grown or not, starts from a plain window), the state is gone.
pub fn finish(cx: &mut App) {
    release(cx);
}

fn release(cx: &mut App) {
    if !cx.has_global::<GrowGlobal>() {
        return;
    }
    let Some(g) = cx.global_mut::<GrowGlobal>().0.take() else { return };
    // (only a motion that was still going is "cut short"; one that the deadline has ended, or an opened panel that is put away, is not)
    if g.run.moving(backdrop::now_ms()) {
        log::info!("dock: grow end: cut short after {} frames in {:?}", g.frames, g.began.elapsed());
    }
    g.island.borrow_mut().set_hidden(false);
    let hwnd = g.hwnd;
    // (the tasks go with the state)
    drop(g);
    later(cx, move || {
        grow::clear_region(hwnd);
        grow::set_frameless(hwnd, false);
    });
    nudge_views(cx);
}

// ---------------------------------------------------------------- what the views ask

/// What the panel's view draws for one frame, in the window's logical px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewFrame {
    /// The opacity of the pages and the account row.
    pub content: f32,
    /// The search field's rectangle while it travels; `None`: it stands where the layout puts it.
    pub field: Option<FRect>,
    /// The opacity of the search field (0 while the island's own field is still the one seen).
    pub field_alpha: f32,
    /// The glass's rectangle and radius: what the plain fill is drawn in (over the glass, or alone when the glass is not there).
    pub wash: FRect,
    pub wash_radius: f32,
    /// How opaque that fill is: it comes in with the growth (the island's own glass has none), see `Run::wash_alpha`.
    pub wash_alpha: f32,
    /// The height of the grown glass below the panel's own rectangle (the gap and the island's row): the view leaves it empty.
    pub extra_bottom: f32,
    /// Another frame is wanted.
    pub moving: bool,
}

/// The panel view's frame, and the sending of the glass's shape and the window's region for it. `None`: no growth is going on, the
/// view draws as an ordinary panel. `glass_fallback`: the panel's glass is not there (the view draws the plain fill whole). Called from
/// `render`: it queues the sending, it does not touch a window.
pub fn view_frame(glass_fallback: bool, cx: &mut App) -> Option<ViewFrame> {
    let now = backdrop::now_ms();
    let g = grow_mut(cx)?;
    let f = g.frame(now);
    let moving = g.run.moving(now);
    if moving {
        g.frames += 1;
    }
    let running = g.run.phase == Phase::Running;
    let scale = g.scale.max(0.1);
    let origin = (g.spec.grown.left as f32, g.spec.grown.top as f32);
    let logical = |r: FRect| [(r[0] - origin.0) / scale, (r[1] - origin.1) / scale, r[2] / scale, r[3] / scale];
    let vf = ViewFrame {
        content: if running { f.content } else { 0.0 },
        field: f.field.filter(|_| f.s < 1.0).map(logical),
        field_alpha: if running { 1.0 } else { 0.0 },
        wash: logical(f.rect),
        wash_radius: f.radius / scale,
        wash_alpha: g.run.wash_alpha(f.u, glass_fallback),
        extra_bottom: g.extra as f32 / scale,
        moving,
    };
    let send = g.plan_send(&f);
    if let Some(send) = send {
        later(cx, move || send.run());
    }
    Some(vf)
}

/// The panel's glass shape for the moment its glass is attached: x, y, w, h in the window's device px, and the radius. `None` when the
/// panel does not grow.
pub fn shape_for_attach(cx: &App) -> Option<(FRect, f32)> {
    let g = grow_ref(cx)?;
    let f = g.frame(backdrop::now_ms());
    Some((g.spec.in_window(&f), f.radius))
}

/// The glass's rectangle on the screen now (device px, rounded out): what a click is "on the panel" for. `None` when the panel does not grow.
pub fn visible_rect(cx: &App) -> Option<Rect> {
    let g = grow_ref(cx)?;
    Some(screen_rect(g.frame(backdrop::now_ms()).rect))
}

/// The height of the grown glass below the panel's own rectangle, logical px; 0 when the panel does not grow.
pub fn extra_bottom(cx: &App) -> f32 {
    grow_ref(cx).map_or(0.0, |g| g.extra as f32 / g.scale.max(0.1))
}

/// What an island draws while its glass is part of the grown one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IslandFade {
    /// The opacity of what is neither the logo nor the search field (the weather).
    pub others: f32,
    /// The island's search field is not drawn: the panel's stands in its place.
    pub field_hidden: bool,
    /// Another frame is wanted.
    pub moving: bool,
}

/// What the island `kind` draws now; `None` when its glass is its own (nothing grows out of it at the moment).
pub fn island_fade(kind: Kind, cx: &App) -> Option<IslandFade> {
    let g = grow_ref(cx)?;
    if g.cfg.kind != kind || g.run.phase != Phase::Running {
        return None;
    }
    let now = backdrop::now_ms();
    let f = g.frame(now);
    let fade = grow::island_fade(g.cfg.stays, &f, g.spec.field.is_some());
    Some(IslandFade { others: fade.others, field_hidden: fade.field_hidden, moving: g.run.moving(now) })
}

#[cfg(test)]
mod tests {
    // (not `super::*`: that would bring in gpui's own `test` attribute, which shadows the standard one)
    use super::{Cfg, Corner, Grow, IslandGlass, Instant, Kind, PanelId, Phase, Rc, RefCell, Rect, Run, Settings, Spec, Stays, field_places, hit, screen_rect};

    #[test]
    fn the_search_field_goes_from_its_place_in_the_island_to_the_top_of_the_panel() {
        // 100 %: the left glass (12, 1334, 342, 1382) and the panel (12, 606, 654, 1326)
        let island = Rect::new(12, 1334, 342, 1382);
        let panel = Rect::new(12, 606, 654, 1326);
        let (from, to) = field_places(island, panel, 1.0, 1.0);
        // padding 8, logo 32, gap 8 after the island's left edge; 32 high, centred in the 48 px island
        assert_eq!(from, [12.0 + 48.0, 1334.0 + 8.0, 220.0, 32.0]);
        // 28 from the panel's sides, 24 under its top, 36 high
        assert_eq!(to, [12.0 + 28.0, 606.0 + 24.0, 642.0 - 56.0, 36.0]);
        // at 150 % with a large dock (unit 1.8): the island's lengths follow the unit, the panel's the monitor
        let island = Rect::new(18, 1461, 513, 1533);
        let (from, to) = field_places(island, Rect::new(18, 369, 981, 1449), 1.8, 1.5);
        assert!((from[0] - (18.0 + 48.0 * 1.8)).abs() < 1e-3 && (from[3] - 32.0 * 1.8).abs() < 1e-3);
        assert!((to[2] - (963.0 - 56.0 * 1.5)).abs() < 1e-3 && (to[3] - 36.0 * 1.5).abs() < 1e-3);
    }

    fn a_growth() -> Grow {
        let island = Rect::new(12, 1334, 342, 1382);
        let panel = Rect::new(12, 606, 654, 1326);
        let spec = Spec::new(island, panel, Corner::BottomLeft, 24.0, 8.0);
        let mut run = Run::new(true);
        run.start(0);
        Grow {
            id: PanelId::START,
            cfg: Cfg { kind: Kind::Left, stays: Stays::Logo },
            spec,
            run,
            hwnd: 0,
            island: Rc::new(RefCell::new(IslandGlass::new())),
            dark: true,
            scale: 1.0,
            extra: 56,
            shown_at: Instant::now(),
            began: Instant::now(),
            frames: 0,
            sent: None,
            arm: None,
            deadline: None,
        }
    }

    /// Every frame sends the glass and the region once, never the same twice, the region first for a bigger glass and last for a smaller
    /// one, and the region is the glass (a click outside the glass is the window below's).
    #[test]
    fn a_frame_is_sent_once_and_the_region_follows_the_glass() {
        let mut g = a_growth();
        let first = g.plan_send(&g.frame(0)).expect("the first frame is sent");
        assert!(first.growing);
        assert_eq!(first.shape, [0.0, 728.0, 330.0, 48.0], "the island's rectangle in the grown window");
        assert!(hit::takes_pointer(&first.parts, 100, 750) && !hit::takes_pointer(&first.parts, 100, 700), "only the island's rectangle");
        assert!(g.plan_send(&g.frame(0)).is_none(), "the same frame again sends nothing");
        // running 110 ms of 220: bigger
        let mid = g.plan_send(&g.frame(110)).expect("a bigger glass");
        assert!(mid.growing && mid.shape[3] > 48.0 && mid.shape[3] < 776.0);
        assert!(hit::takes_pointer(&mid.parts, 100, 700));
        // closing again: smaller, so the glass goes first and the region after it
        g.run.begin_close(110);
        let back = g.plan_send(&g.frame(150)).expect("a smaller glass");
        assert!(!back.growing && back.shape[3] < mid.shape[3]);
        // arrived at the panel: the whole window, no frame is sent twice
        let mut h = a_growth();
        let last = h.plan_send(&h.frame(220)).unwrap();
        assert_eq!(last.shape, [0.0, 0.0, 642.0, 776.0]);
        assert!(h.plan_send(&h.frame(10_000)).is_none());
        // while the glass is still starting (nothing runs) the frame is the island's, whatever the clock says
        let mut w = a_growth();
        w.run = Run::new(true);
        assert_eq!(w.frame(10_000), w.spec.at(0.0));
        // and once the island is home again
        let mut c = a_growth();
        c.run.begin_close(300);
        c.run.settle(1_000);
        assert_eq!(c.run.phase, Phase::Closed);
        assert_eq!(c.frame(10_000), c.spec.at(0.0));
    }

    #[test]
    fn the_setting_is_on_by_default_and_can_be_turned_off() {
        assert!(Settings::default().dock_start_grow, "the user asked for it: on unless turned off");
        assert!(Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap().dock_start_grow);
        let on = Settings { dock_start_grow: false, ..Default::default() };
        let back = Settings::parse_lenient_pub(&toml::to_string_pretty(&on).unwrap()).unwrap();
        assert!(!back.dock_start_grow, "off survives a save and a load");
        assert!(Settings::parse_lenient_pub("dock_start_grow = 3\n").unwrap().dock_start_grow, "a wrong value falls back to the default");
    }

    #[test]
    fn a_rect_on_the_screen_is_rounded_out() {
        assert_eq!(screen_rect([10.4, 20.6, 100.0, 50.2]), Rect::new(10, 20, 111, 71));
    }
}
