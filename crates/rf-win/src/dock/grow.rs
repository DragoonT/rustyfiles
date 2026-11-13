//! An island that GROWS into its panel: the arithmetic of the motion, and nothing else. A pure function of time and of three
//! rectangles; the windows, the glass and the pointer region that carry it out are `rf-app`'s `dock/grow_host.rs`.
//!
//! WHAT THE EYE SEES. The island's glass (a capsule, 48 dock units high) becomes the panel's glass (a rectangle with the panel's
//! small round corners): its rectangle grows from the island's to the GROWN rectangle (the island's rectangle and the panel's, taken
//! together: the panel's content on top and the island's own row at the bottom) while one corner of it, the one the island is anchored
//! by, stays exactly where it is (the bottom left corner for the left island, the bottom right one for the right island). The
//! corner radius eases from the island's to the panel's with the rectangle. NOTHING of the island's row fades or moves: the whole row
//! (the left island's logo, search field and weather; the right island's network and volume symbols, clock, bell and tray strip) stays
//! exactly where it is and is the bottom row of the grown glass. The panel's content fades in over the second half of the motion.
//!
//! ONE NUMBER DRIVES IT. `s`, the position along the motion, 0 (the island) to 1 (the panel). Opening moves `s` up at 1 / 220 ms,
//! closing moves it down at 1 / 160 ms, and a reversal at any moment simply turns the direction, so the motion is continuous in `s`
//! and "the same motion backwards" is literally that: the glass follows `u = ease_out(s)`, so it leaves the island fast and settles
//! gently when opening, and the way back is the same curve run the other way (it leaves the panel gently and arrives fast). The
//! content's fade follows `s` itself, which is time: the panel's content is there from the middle on, and in a closing the content
//! goes first and the glass shrinks after it.
//!
//! WHAT IS HERE. [`Spec`] (the three rectangles, the corner that stays and the radii) turns an `s` into a [`Frame`]; [`Motion`] turns
//! the clock into an `s`; [`arm_done`] decides when the clock may start (the panel's glass must be running or have had its time);
//! [`Stays`] says what the panel draws in the island's row. [`set_frameless`] is the one call into Windows: the panel's window cuts
//! its own corners here (its glass and its window region do), so the system's rounding and border are put away for it.
//!
//! HOW THE PANELS USE IT. The right island's: `Spec::new(island, panel, Corner::BottomRight, ..)` with the right island's glass and
//! the panel `panel::place` gives with `Side::Right`; `Stays::Row` (the panel draws nothing in the row). The Start panel's: the left
//! island, `Corner::BottomLeft`, `Stays::RowAndField` (the panel draws its search field in the row, where the island's field stands).
//! Everything else, the clock, the glass, the region, the island's glass being put away while the panel stands, is the host's and
//! does not know which island it is. The rules about WHICH panel (a press on another button of the row, whether a panel may grow on
//! this screen) are `grow_panels`.

use super::layout::Rect;
use super::panel::Side;
use super::startpage;

/// Opening takes this long (ms) from the island to the panel.
pub const OPEN_MS: u32 = 220;
/// Closing takes this long (ms) from the panel back to the island.
pub const CLOSE_MS: u32 = 160;
/// The panel's content starts to appear at this `s` and is whole at 1.
pub const CONTENT_FADE_START: f32 = 0.5;
/// The longest the clock waits for a glass the engine has ACCEPTED and is still starting (ms), before it gives the engine up and starts
/// with the plain fill. It is a net for an engine that neither runs nor fails: the wait normally ends at the engine's own signal (its
/// first frame presented), a few ms to a few tens of ms after the panel is shown. During the wait the island is on the screen exactly as
/// it was, so a slow engine costs the press some delay and never a frame of the dark box.
pub const ARM_ENGINE_MAX_MS: u64 = 1000;

/// What the Start panel gives up when it grows out of the left island: its own bottom row (the line above it and the 64 px row with the
/// account, the gear and the power button). The glass's bottom row is the island's row then (the logo, the search field and the
/// weather), and the account, the gear and the power button stand in a header at the TOP of the panel instead, where the panel's search
/// field was (`rf-app`'s `dock/grow_head.rs`), so the glass is this much shorter than the panel was and the content between the header
/// and the island's row keeps the height it had.
pub const OWN_ROW_H: f32 = startpage::DIVIDER_H + startpage::FOOTER_H;

/// The height (logical px) a panel that gives up its own bottom row is placed with, from the height it would have had with it: the
/// content above keeps the height it had, the row is the island's. At least 1.
pub fn height_without_own_row(h: f32) -> f32 {
    (h - OWN_ROW_H).max(1.0)
}

/// The inverse for the page layout: the panel's height as the layout worked it out before (`startpage::home_layout` counts the line and
/// the row as part of its chrome), from the height of the content area above the island's row.
pub fn height_with_own_row(content_h: f32) -> f32 {
    content_h + OWN_ROW_H
}

/// A rectangle as x, y, w, h in device px (the shape the glass engine takes).
pub type FRect = [f32; 4];

/// The corner of the island that stays where it is while the glass grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    /// The left island: the glass grows up and to the right.
    BottomLeft,
    /// The right island: the glass grows up and to the left.
    BottomRight,
}

impl From<Side> for Corner {
    fn from(side: Side) -> Corner {
        match side {
            Side::Left => Corner::BottomLeft,
            Side::Right => Corner::BottomRight,
        }
    }
}

/// What the panel draws in the island's row while the island's glass has become the panel's. In both cases the WHOLE row stays as it is
/// (nothing fades, nothing moves) and is the bottom row of the grown glass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stays {
    /// The panel draws nothing in the row (the right island's network, volume, clock, bell and tray strip are the island's own).
    Row,
    /// The panel draws its search field in the row, exactly where the island's own field stands (the left island: logo, field, weather).
    /// The island does not draw its field meanwhile; the panel's is the one with the keyboard.
    RowAndField,
}

impl Stays {
    /// The island's own search field is not drawn: the panel's field stands in its place.
    pub fn field_is_the_panels(self) -> bool {
        self == Stays::RowAndField
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The glass's curve along the motion: the dock's spring over the motion's time (`spring::curve`): it picks up speed, is nine
/// tenths of the way after half the time and comes to rest softly. It was a cubic ease-out until 2026-11-13, as quick, but leaving
/// at full speed from the first frame.
pub fn ease_out(x: f32) -> f32 {
    super::spring::curve(x)
}

/// The three rectangles of a growth (device px, screen coordinates) and how the glass moves between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    /// The island's glass at rest.
    pub island: Rect,
    /// The glass when the panel stands: the island's rectangle and the panel's together. The panel's WINDOW is this rectangle.
    pub grown: Rect,
    pub corner: Corner,
    /// The corner radius of the island's glass and of the panel's, device px.
    pub radius_from: f32,
    pub radius_to: f32,
}

/// The rectangle that holds both: what the glass is once it has grown. The panel sits above its island, so this is the panel's
/// rectangle taken down to the island's bottom edge, and when the panel lines up with the island's anchored edge (it does, unless the
/// screen is too narrow for it) the anchored corner of the island is a corner of it.
pub fn grown_rect(island: Rect, panel: Rect) -> Rect {
    Rect::new(island.left.min(panel.left), island.top.min(panel.top), island.right.max(panel.right), island.bottom.max(panel.bottom))
}

fn frect(r: Rect) -> FRect {
    [r.left as f32, r.top as f32, r.width() as f32, r.height() as f32]
}

impl Spec {
    /// The growth of `island` into `panel` anchored by `corner`. Radii in device px.
    pub fn new(island: Rect, panel: Rect, corner: Corner, radius_from: f32, radius_to: f32) -> Spec {
        Spec { island, grown: grown_rect(island, panel), corner, radius_from: radius_from.max(0.0), radius_to: radius_to.max(0.0) }
    }

    /// Whether the island's anchored corner is also a corner of the grown glass (the "corner stays fixed" promise; false only when the
    /// panel had to be moved off the island's edge to fit the screen).
    pub fn corner_is_fixed(&self) -> bool {
        match self.corner {
            Corner::BottomLeft => self.grown.left == self.island.left && self.grown.bottom == self.island.bottom,
            Corner::BottomRight => self.grown.right == self.island.right && self.grown.bottom == self.island.bottom,
        }
    }

    /// The frame at position `s` (0 the island, 1 the panel; anything else is cut to that).
    pub fn at(&self, s: f32) -> Frame {
        let s = if s.is_finite() { s.clamp(0.0, 1.0) } else { 0.0 };
        let u = ease_out(s);
        let (a, b) = (frect(self.island), frect(self.grown));
        // each edge moves on its own, so an edge the two rectangles share does not move at all (`a + 0 * u` is exactly `a`)
        let left = lerp(a[0], b[0], u);
        let top = lerp(a[1], b[1], u);
        let right = lerp(a[0] + a[2], b[0] + b[2], u);
        let bottom = lerp(a[1] + a[3], b[1] + b[3], u);
        let rect = [left, top, right - left, bottom - top];
        let radius = lerp(self.radius_from, self.radius_to, u).min(rect[2].min(rect[3]) / 2.0).max(0.0);
        Frame {
            s,
            u,
            rect,
            radius,
            content: smoothstep(CONTENT_FADE_START, 1.0, s),
        }
    }

    /// A frame's glass in the panel window's own coordinates (the window is `grown`): x, y, w, h.
    pub fn in_window(&self, frame: &Frame) -> FRect {
        [frame.rect[0] - self.grown.left as f32, frame.rect[1] - self.grown.top as f32, frame.rect[2], frame.rect[3]]
    }
}

/// One moment of the motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The position along the motion, 0 to 1 (time).
    pub s: f32,
    /// The glass's progress, `ease_out(s)`.
    pub u: f32,
    /// The glass, screen coordinates: x, y, w, h.
    pub rect: FRect,
    /// Its corner radius, device px.
    pub radius: f32,
    /// The opacity of the panel's content (the header, the pages).
    pub content: f32,
}

/// The panel's wash over its glass: how opaque it is, and whether it is the PLAIN fill (the dense one a window with no glass paints) or
/// the thin wash that goes over a running glass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wash {
    pub alpha: f32,
    pub plain: bool,
}

/// The clock of one growth: where `s` is, and where it is going. Time is milliseconds on any steady clock, passed in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    s0: f32,
    t0: u64,
    open: bool,
}

impl Motion {
    /// At rest as the island.
    pub const fn closed() -> Motion {
        Motion { s0: 0.0, t0: 0, open: false }
    }

    /// The position at `now`.
    pub fn s_at(&self, now: u64) -> f32 {
        let dt = now.saturating_sub(self.t0) as f32;
        if self.open { (self.s0 + dt / OPEN_MS as f32).min(1.0) } else { (self.s0 - dt / CLOSE_MS as f32).max(0.0) }
    }

    /// Turns towards the panel from wherever it is (with `animate` false: it is there at once).
    pub fn open(&mut self, now: u64, animate: bool) {
        self.s0 = if animate { self.s_at(now) } else { 1.0 };
        self.t0 = now;
        self.open = true;
    }

    /// Turns back towards the island from wherever it is (with `animate` false: it is there at once).
    pub fn close(&mut self, now: u64, animate: bool) {
        self.s0 = if animate { self.s_at(now) } else { 0.0 };
        self.t0 = now;
        self.open = false;
    }

    /// Whether it is going towards the panel.
    pub fn is_opening(&self) -> bool {
        self.open
    }

    /// Whether it has arrived where it is going.
    pub fn settled(&self, now: u64) -> bool {
        self.s_at(now) == if self.open { 1.0 } else { 0.0 }
    }

    /// Milliseconds until it arrives (0 when it has).
    pub fn remaining_ms(&self, now: u64) -> u32 {
        let s = self.s_at(now);
        if self.open { ((1.0 - s) * OPEN_MS as f32).ceil() as u32 } else { (s * CLOSE_MS as f32).ceil() as u32 }
    }
}

/// What the glass engine says about the panel's window (`rf_win::glass::state`, read by the host).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Its first frame has been presented: the glass is on the window.
    Running,
    /// The engine accepted the window and has not drawn it yet.
    Starting,
    /// The window has no glass and the engine is not working on one (refused, failed or lost): the plain fill is all there is.
    Gone,
}

/// Whether the clock may start now. The island's own glass is put away the moment it starts, so it must not start before the panel's
/// glass has a frame on the screen: with the engine STARTING the clock waits for the engine's own signal (the limit is only a net,
/// [`ARM_ENGINE_MAX_MS`]; an earlier fixed limit of 90 ms put the island's glass away from under a slow engine and showed an opaque
/// dark box until the engine caught up). With the engine GONE no glass is coming, waiting would only delay the plain fill, so it starts
/// at once.
pub fn arm_done(engine: Engine, waited_ms: u64) -> bool {
    match engine {
        Engine::Running | Engine::Gone => true,
        Engine::Starting => waited_ms >= ARM_ENGINE_MAX_MS,
    }
}

/// The panel's window cuts its own corners (the glass, and the window region that follows the glass), so the system's rounding and its
/// border are put away while it is part of a growth (`on`), and given back for an ordinary panel (`off`). Ignored before Windows 11.
pub fn set_frameless(hwnd: isize, on: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_COLOR_DEFAULT, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND, DwmSetWindowAttribute};
    if hwnd == 0 {
        return;
    }
    let h = HWND(hwnd as *mut _);
    let corners = if on { DWMWCP_DONOTROUND } else { DWMWCP_ROUND };
    let border: u32 = if on { DWMWA_COLOR_NONE } else { DWMWA_COLOR_DEFAULT };
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &corners as *const _ as _, size_of_val(&corners) as u32);
        let _ = DwmSetWindowAttribute(h, DWMWA_BORDER_COLOR, &border as *const u32 as _, size_of::<u32>() as u32);
    }
}

/// Takes the window region away (the whole window shows and takes the pointer again): what a panel that has been part of a growth gets
/// before its next opening, which may be an ordinary one. Call it from outside any update of the app, like every window call.
pub fn clear_region(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::SetWindowRgn;
    if hwnd == 0 {
        return;
    }
    unsafe {
        SetWindowRgn(HWND(hwnd as *mut _), None, false);
    }
}

/// Whether the grown glass, with `margin` device px around it, would touch another island's glass (an empty rectangle is no island).
/// The grown glass is as wide as the panel all the way down to the island's row, so on a screen where the next island begins close to
/// this one it would lie under that island: such a panel does not grow, it opens the ordinary way (a stand-alone pop-up above the island).
pub fn reaches_other_island(grown: Rect, other: Rect, margin: i32) -> bool {
    other.width() > 0
        && other.height() > 0
        && other.left < grown.right + margin
        && other.right > grown.left - margin
        && other.top < grown.bottom + margin
        && other.bottom > grown.top - margin
}

/// Where a growth is in its life. Pure: the host (`rf-app`'s `grow_host.rs`) only carries out what the transitions below say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The window is shown and the panel's glass is starting; the island's glass is still the visible one, `s` is 0.
    Arming,
    /// The clock runs (or has arrived at the panel).
    Running,
    /// Arrived back at the island (or closed with nothing to shrink): the island has its glass; the window is about to go.
    Closed,
}

/// What opening the panel again does to a growth that is still there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Again {
    /// Nothing turns (still arming, or opening already).
    Same,
    /// It was shrinking and turns towards the panel from `from`; `remaining_ms` to go.
    TurnedBack { from: f32, remaining_ms: u32 },
    /// It had come home and the window was about to go: it starts again from the island.
    Restarted,
}

/// What putting the panel away does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shrink {
    /// Nothing to shrink (the glass never came, or it is home already): the window may go at once.
    Nothing,
    /// The clock turns towards the island from `from`; the window stays until `remaining_ms` and the deadline have passed. With
    /// animations off `remaining_ms` is 0: the glass is the island's in the next frame, but the island gets ITS glass back in the
    /// deadline like every other closing, before the window goes, so the island never stands without one.
    Shrinking { from: f32, remaining_ms: u32 },
}

/// What a deadline finds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arrival {
    /// Not running: nothing to end.
    Idle,
    /// The clock ran late: wait `remaining_ms` more.
    Late { remaining_ms: u32 },
    /// Arrived at the panel.
    Opened,
    /// Arrived back at the island (the phase is `Closed` now).
    Home,
}

/// The life of one growth: its phase, its clock and whether it animates. Every decision of the host about WHEN is one of these methods,
/// so that it can be tested without a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    pub phase: Phase,
    motion: Motion,
    pub animate: bool,
}

impl Run {
    pub const fn new(animate: bool) -> Run {
        Run { phase: Phase::Arming, motion: Motion::closed(), animate }
    }

    /// The position along the motion: the island until the clock runs and once it is home, the clock's own while it runs.
    pub fn s_at(&self, now: u64) -> f32 {
        match self.phase {
            Phase::Arming | Phase::Closed => 0.0,
            Phase::Running => self.motion.s_at(now),
        }
    }

    pub fn is_opening(&self) -> bool {
        self.motion.is_opening()
    }

    /// Shrinking back to the island right now (a press now turns it round).
    pub fn is_closing(&self) -> bool {
        self.phase == Phase::Running && !self.motion.is_opening()
    }

    /// Another frame is wanted.
    pub fn moving(&self, now: u64) -> bool {
        self.phase == Phase::Running && !self.motion.settled(now)
    }

    pub fn remaining_ms(&self, now: u64) -> u32 {
        self.motion.remaining_ms(now)
    }

    /// The panel is opened again while this growth is still there.
    pub fn reopen(&mut self, now: u64, animate: bool) -> Again {
        self.animate = animate;
        match self.phase {
            Phase::Closed => {
                self.phase = Phase::Arming;
                self.motion = Motion::closed();
                Again::Restarted
            }
            Phase::Running if !self.motion.is_opening() => {
                let from = self.motion.s_at(now);
                self.motion.open(now, animate);
                Again::TurnedBack { from, remaining_ms: self.motion.remaining_ms(now) }
            }
            _ => Again::Same,
        }
    }

    /// The panel's glass runs (or has had its time): the clock starts. Returns the milliseconds to the panel (0 with animations off).
    pub fn start(&mut self, now: u64) -> u32 {
        self.motion.open(now, self.animate);
        self.phase = Phase::Running;
        self.motion.remaining_ms(now)
    }

    /// The panel is put away.
    pub fn begin_close(&mut self, now: u64) -> Shrink {
        if self.phase != Phase::Running {
            return Shrink::Nothing;
        }
        let from = self.motion.s_at(now);
        self.motion.close(now, self.animate);
        Shrink::Shrinking { from, remaining_ms: self.motion.remaining_ms(now) }
    }

    /// How opaque the plain wash over the panel's glass is (the dark or light tint the panel puts over its glass to read well on it).
    /// The island's own glass has none, so the wash comes in WITH the growth (`u`, the glass's progress) and is gone again at the end of
    /// a closing: at the first and the last frame the grown glass is the island's glass and nothing else, and there is no step in its
    /// tone where one glass takes over from the other. While the glass is only starting (`Arming`) or the island is home, nothing is
    /// drawn. A glass that never came (`glass_fallback`) shows the plain fill whole, since it is all there is.
    pub fn wash_alpha(&self, u: f32, glass_fallback: bool) -> f32 {
        match self.phase {
            Phase::Running if glass_fallback => 1.0,
            Phase::Running => u.clamp(0.0, 1.0),
            Phase::Arming | Phase::Closed => 0.0,
        }
    }

    /// What the panel paints over its glass at this moment of the motion, from what the ENGINE reports for the panel's window right now
    /// (`engine_running`: its first frame has been presented and it has not been lost since).
    ///
    /// WHY THE ENGINE AND NOT THE VIEW'S OWN BELIEF. The clock starts the moment the engine says it runs (`arm_done`, looked at every
    /// 8 ms), but the panel's view learns it from `PopupGlass::tick`, which a watch calls every 50 ms. For up to those 50 ms the view
    /// still believed "no glass yet" while the island's own glass had already been put away, and painted the plain fill WHOLE (dark
    /// 0x1C1C1E at 82 %, `Run::wash_alpha`'s "a glass that never came" case) over the growing shape: one opaque dark box before the
    /// first glass frame. The decision is made from one source now, the one the clock waited for.
    pub fn wash(&self, u: f32, engine_running: bool) -> Wash {
        Wash { alpha: self.wash_alpha(u, !engine_running), plain: !engine_running }
    }

    /// The deadline of a motion.
    pub fn settle(&mut self, now: u64) -> Arrival {
        if self.phase != Phase::Running {
            return Arrival::Idle;
        }
        if !self.motion.settled(now) {
            return Arrival::Late { remaining_ms: self.motion.remaining_ms(now) };
        }
        if self.motion.is_opening() {
            Arrival::Opened
        } else {
            self.phase = Phase::Closed;
            Arrival::Home
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::hit;
    use crate::dock::layout::{self, Widths};
    use crate::dock::panel::{self, GAP};
    use crate::dock::size::{self, DockSize};
    use crate::dock::startpage;

    const DPIS: [f32; 3] = [1.0, 1.25, 1.5];

    /// The island's glass, the panel's rectangle and the spec for a 2560 x 1440 monitor (taskbar 48 px at 100 %) at this scale and dock
    /// size, built the way the dock builds them: `layout::islands` with the dock unit, `panel::place` with the monitor's scale.
    fn setup(dpi: f32, size_factor: f32, side: Side) -> (Rect, Rect, Spec) {
        let work = Rect::new(0, 0, 2560, 1440 - (48.0 * dpi).round() as i32);
        let unit = size::unit(dpi, size_factor);
        let isl = layout::islands(work, unit, Widths { left: 330.0, middle: 600.0, right: 280.0 });
        let island = if side == Side::Left { isl.left } else { isl.right };
        let panel_rect = panel::place(island, (startpage::PANEL_W, startpage::PANEL_MAX_H), dpi, side, work);
        let spec = Spec::new(island, panel_rect, side.into(), layout::RADIUS * unit, 8.0 * dpi);
        (island, panel_rect, spec)
    }

    #[test]
    fn the_grown_glass_holds_the_island_and_the_panel_and_keeps_the_anchored_corner() {
        for dpi in DPIS {
            for f in DockSize::ALL.map(DockSize::factor) {
                for side in [Side::Left, Side::Right] {
                    let (island, panel_rect, spec) = setup(dpi, f, side);
                    assert!(spec.grown.covers(&island) && spec.grown.covers(&panel_rect), "{dpi} {f} {side:?}");
                    assert_eq!(spec.grown.top, panel_rect.top, "the panel's top is the glass's top");
                    assert_eq!(spec.grown.bottom, island.bottom, "the island's bottom edge is the glass's bottom edge");
                    assert!(spec.corner_is_fixed(), "{dpi} {f} {side:?}: {spec:?}");
                    assert_eq!(spec.grown.width(), panel_rect.width().max(island.width()));
                    // the gap between the panel and the island is inside the glass
                    assert_eq!(spec.grown.height(), panel_rect.height() + layout::px(GAP, dpi) + island.height());
                }
            }
        }
    }

    #[test]
    fn at_the_dock_sizes_and_scales_the_glass_starts_as_the_island_and_ends_as_the_grown_rectangle() {
        for dpi in DPIS {
            for s in DockSize::ALL {
                for side in [Side::Left, Side::Right] {
                    let (island, _, spec) = setup(dpi, s.factor(), side);
                    let first = spec.at(0.0);
                    assert_eq!(first.rect, frect(island), "{dpi} {s:?} {side:?}");
                    assert_eq!(first.radius, spec.radius_from.min(island.height() as f32 / 2.0));
                    assert_eq!(first.content, 0.0);
                    let last = spec.at(1.0);
                    assert_eq!(last.rect, frect(spec.grown));
                    assert_eq!(last.radius, spec.radius_to);
                    assert_eq!(last.content, 1.0);
                }
            }
        }
    }

    #[test]
    fn the_anchored_corner_never_moves_and_the_glass_only_grows() {
        for dpi in DPIS {
            for side in [Side::Left, Side::Right] {
                let (island, _, spec) = setup(dpi, 1.0, side);
                let mut prev = spec.at(0.0);
                for i in 0..=1000 {
                    let f = spec.at(i as f32 / 1000.0);
                    let [x, y, w, h] = f.rect;
                    match spec.corner {
                        Corner::BottomLeft => assert_eq!((x, y + h), (island.left as f32, island.bottom as f32), "s={}", f.s),
                        Corner::BottomRight => assert_eq!((x + w, y + h), (island.right as f32, island.bottom as f32), "s={}", f.s),
                    }
                    assert!(w >= prev.rect[2] - 1e-3 && h >= prev.rect[3] - 1e-3, "it only grows: s={}", f.s);
                    // always inside the grown rectangle (the window)
                    assert!(x >= spec.grown.left as f32 - 1e-3 && y >= spec.grown.top as f32 - 1e-3, "s={}", f.s);
                    assert!(x + w <= spec.grown.right as f32 + 1e-3 && y + h <= spec.grown.bottom as f32 + 1e-3, "s={}", f.s);
                    // the radius goes from the island's to the panel's and never leaves that range or half the short side
                    let (lo, hi) = (spec.radius_from.min(spec.radius_to), spec.radius_from.max(spec.radius_to));
                    assert!(f.radius >= lo.min(h / 2.0) - 1e-3 && f.radius <= hi + 1e-3 && f.radius <= w.min(h) / 2.0 + 1e-3, "s={} r={}", f.s, f.radius);
                    prev = f;
                }
            }
        }
    }

    #[test]
    fn the_glass_leaves_fast_and_arrives_gently() {
        let (_, _, spec) = setup(1.0, 1.0, Side::Left);
        let early = spec.at(0.25);
        assert!(early.u > 0.55, "ease out: a quarter of the time is more than half of the way ({})", early.u);
        let mid = spec.at(0.5);
        assert!(mid.u > 0.85);
        let (a, b) = (spec.at(0.9).u, spec.at(1.0).u);
        assert!(b - a < 0.004, "it settles");
        // a spring: it picks up speed (the first twentieth of the time is under a twentieth of the way) and never passes its place
        assert!(spec.at(0.05).u < 0.07);
        assert!((0..=100).all(|i| spec.at(i as f32 / 100.0).u <= 1.0));
        assert_eq!(spec.at(-3.0), spec.at(0.0));
        assert_eq!(spec.at(7.0), spec.at(1.0));
        assert_eq!(spec.at(f32::NAN), spec.at(0.0), "not a number is the island");
    }

    #[test]
    fn the_content_fades_in_over_the_second_half_and_nothing_of_the_islands_row_does() {
        let (_, _, spec) = setup(1.0, 1.0, Side::Left);
        let mut last_content = 0.0;
        for i in 0..=100 {
            let f = spec.at(i as f32 / 100.0);
            assert!(f.content >= last_content - 1e-6);
            last_content = f.content;
            if f.s <= CONTENT_FADE_START {
                assert_eq!(f.content, 0.0, "nothing of the panel before the middle: s={}", f.s);
            }
        }
        assert!(spec.at(0.75).content > 0.0 && spec.at(0.75).content < 1.0);
        assert_eq!(spec.at(1.0).content, 1.0);
        // the island's row stays whole in both kinds of growth; only the left island's field is the panel's
        assert!(Stays::RowAndField.field_is_the_panels());
        assert!(!Stays::Row.field_is_the_panels());
    }

    #[test]
    fn opening_takes_220_ms_and_closing_160() {
        let mut m = Motion::closed();
        assert!(m.settled(0) && m.s_at(10_000) == 0.0);
        m.open(1_000, true);
        assert!(m.is_opening() && !m.settled(1_000));
        assert_eq!(m.s_at(1_000), 0.0);
        assert!((m.s_at(1_110) - 0.5).abs() < 1e-5);
        assert_eq!(m.remaining_ms(1_000), 220);
        assert_eq!(m.remaining_ms(1_110), 110);
        assert!(!m.settled(1_219));
        assert!(m.settled(1_220) && m.s_at(1_220) == 1.0 && m.s_at(5_000) == 1.0);
        assert_eq!(m.remaining_ms(1_220), 0);
        m.close(2_000, true);
        assert!(!m.is_opening() && m.s_at(2_000) == 1.0);
        assert!((m.s_at(2_080) - 0.5).abs() < 1e-5);
        assert_eq!(m.remaining_ms(2_000), 160);
        assert!(!m.settled(2_159) && m.settled(2_160) && m.s_at(2_160) == 0.0);
    }

    #[test]
    fn a_reversal_turns_where_it_is_and_goes_back_the_way_it_came() {
        let mut m = Motion::closed();
        m.open(0, true);
        let s = m.s_at(110);
        m.close(110, true);
        assert_eq!(m.s_at(110), s, "no jump at the turn");
        // it takes the closing speed back: half way up takes 80 ms down
        assert_eq!(m.remaining_ms(110), 80);
        assert!(m.settled(190) && !m.settled(189));
        // turned again part of the way down: no jump, and the rest of the way up is the rest of 220 ms
        m.close(1_000, true);
        m.open(1_000, true);
        m.close(2_000, true);
        let down = m.s_at(2_040);
        assert!((down - 0.75).abs() < 1e-5);
        m.open(2_040, true);
        assert_eq!(m.s_at(2_040), down);
        assert_eq!(m.remaining_ms(2_040), 55);
        // and the glass is continuous across a turn (a pure function of s)
        let (_, _, spec) = setup(1.5, 1.2, Side::Left);
        assert_eq!(spec.at(m.s_at(2_040)), spec.at(down));
    }

    #[test]
    fn with_animations_off_it_is_there_in_one_frame_both_ways() {
        let mut m = Motion::closed();
        m.open(50, false);
        assert!(m.settled(50) && m.s_at(50) == 1.0 && m.remaining_ms(50) == 0);
        m.close(60, false);
        assert!(m.settled(60) && m.s_at(60) == 0.0 && m.remaining_ms(60) == 0);
        // also from the middle of a motion
        m.open(100, true);
        m.close(150, false);
        assert!(m.settled(150));
        // a clock that goes backwards does not move it
        m.open(500, true);
        assert_eq!(m.s_at(100), 0.0);
    }

    #[test]
    fn the_clock_waits_for_the_glass_but_not_for_ever() {
        assert!(!arm_done(Engine::Starting, 0));
        assert!(!arm_done(Engine::Starting, ARM_ENGINE_MAX_MS - 1));
        assert!(arm_done(Engine::Starting, ARM_ENGINE_MAX_MS));
        assert!(arm_done(Engine::Running, 0));
        // no glass is coming: the plain fill is all there is, and nothing is gained by waiting
        assert!(arm_done(Engine::Gone, 0));
    }

    /// The window region follows the glass: at the first frame only the island takes the pointer, at the last the whole panel does, and
    /// at every frame between a point outside the glass is the pointer of whatever lies below.
    #[test]
    fn the_pointer_region_is_the_glass_at_every_frame() {
        for dpi in DPIS {
            for side in [Side::Left, Side::Right] {
                let (island, panel_rect, spec) = setup(dpi, 1.0, side);
                let (w, h) = (spec.grown.width(), spec.grown.height());
                let parts = |s: f32| {
                    let f = spec.at(s);
                    (hit::parts(spec.in_window(&f), f.radius, &[], w, h), f)
                };
                let inside = |p: &[hit::Part], x: i32, y: i32| hit::takes_pointer(p, x - spec.grown.left, y - spec.grown.top);
                // the first frame: the island's centre is the island's, a point in the panel's rectangle above it is not
                let (p0, _) = parts(0.0);
                let (cx, cy) = ((island.left + island.right) / 2, (island.top + island.bottom) / 2);
                assert!(inside(&p0, cx, cy));
                assert!(!inside(&p0, cx, panel_rect.top + 40), "{dpi} {side:?}: the panel is not there yet");
                assert!(!inside(&p0, cx, island.top - 6), "the gap is not the island's");
                // the last frame: all of the panel and the island's row; the window's corner is cut
                let (p1, _) = parts(1.0);
                assert!(inside(&p1, cx, panel_rect.top + 40) && inside(&p1, cx, cy) && inside(&p1, cx, island.top - 6));
                assert!(!inside(&p1, spec.grown.left, spec.grown.top), "the rounded corner lets the pointer through");
                // in between: the centre of the glass is the glass's, the middle of the rectangle's top edge a pixel above it is not
                for i in 1..100 {
                    let (p, f) = parts(i as f32 / 100.0);
                    let (mx, my) = ((f.rect[0] + f.rect[2] / 2.0) as i32, (f.rect[1] + f.rect[3] / 2.0) as i32);
                    assert!(inside(&p, mx, my), "{dpi} {side:?} s={}", f.s);
                    assert!(!inside(&p, mx, f.rect[1] as i32 - 4), "{dpi} {side:?} s={}: above the glass is not ours", f.s);
                    let b = hit::bounds(&p).unwrap();
                    assert!(Rect::new(0, 0, w, h).covers(&b), "the region stays inside the window");
                }
            }
        }
    }

    /// The grown glass is as wide as the panel all the way down to the island's row. Where the next island begins inside that width
    /// (a 1920 px wide screen at 125 %, the common laptop) the glass would lie under it and take its clicks: such a panel does not grow.
    #[test]
    fn a_growth_that_would_lie_under_the_next_island_does_not_happen() {
        let reaches = |width: i32, dpi: f32, size_factor: f32| {
            let work = Rect::new(0, 0, width, 1080 - (48.0 * dpi).round() as i32);
            let unit = size::unit(dpi, size_factor);
            let isl = layout::islands(work, unit, Widths { left: 330.0, middle: 600.0, right: 280.0 });
            let panel_rect = panel::place(isl.left, (startpage::PANEL_W, startpage::PANEL_MAX_H), dpi, Side::Left, work);
            let grown = grown_rect(isl.left, panel_rect);
            let margin = layout::px(GAP, dpi);
            (reaches_other_island(grown, isl.middle, margin), reaches_other_island(grown, isl.right, margin), grown, isl.middle)
        };
        // 1920 x 1080 at 125 %: the glass is 803 px wide from x = 15, the middle island begins at x = 585 (the reviewer's numbers)
        let (middle, _, grown, mid) = reaches(1920, 1.25, 1.0);
        assert!(middle && grown.right > mid.left, "{grown:?} {mid:?}");
        // 1366 and 1600 wide at 100 %: the same
        assert!(reaches(1366, 1.0, 1.0).0 && reaches(1600, 1.0, 1.0).0);
        // the user's 2560 x 1440 at 100 %: nothing is reached, so it grows
        let (middle, right, ..) = reaches(2560, 1.0, 1.0);
        assert!(!middle && !right);
        // an island with no area is no island (a dock without a right island)
        let any = Rect::new(0, 0, 100, 100);
        assert!(!reaches_other_island(any, Rect::default(), 8));
        // a neighbour a little further than the margin is not reached, one inside the margin is
        let grown = Rect::new(10, 600, 650, 1380);
        assert!(!reaches_other_island(grown, Rect::new(700, 1330, 1300, 1380), 8));
        assert!(reaches_other_island(grown, Rect::new(655, 1330, 1300, 1380), 8));
        assert!(!reaches_other_island(grown, Rect::new(655, 1330, 1300, 1380), 0));
    }

    #[test]
    fn a_growth_runs_its_life_arming_running_home() {
        let mut r = Run::new(true);
        assert_eq!((r.phase, r.s_at(5_000), r.moving(5_000), r.is_opening()), (Phase::Arming, 0.0, false, false), "the island until the glass runs");
        assert_eq!(r.start(1_000), 220);
        assert_eq!(r.phase, Phase::Running);
        assert!(r.moving(1_100) && r.is_opening() && !r.is_closing());
        assert_eq!(r.settle(1_100), Arrival::Late { remaining_ms: 120 });
        assert_eq!(r.settle(1_220), Arrival::Opened);
        assert!(!r.moving(1_220) && r.phase == Phase::Running, "opened: nothing asks for another frame, the phase stays");
        assert_eq!(r.s_at(9_999), 1.0);
        // put away: the clock turns, the window stays until it has arrived and the deadline has seen it
        assert_eq!(r.begin_close(2_000), Shrink::Shrinking { from: 1.0, remaining_ms: 160 });
        assert!(r.is_closing() && r.moving(2_100));
        assert_eq!(r.settle(2_100), Arrival::Late { remaining_ms: 60 });
        assert_eq!(r.phase, Phase::Running);
        assert_eq!(r.settle(2_160), Arrival::Home);
        assert_eq!((r.phase, r.s_at(2_160), r.moving(2_160)), (Phase::Closed, 0.0, false));
        assert_eq!(r.settle(2_500), Arrival::Idle, "nothing ends twice");
        assert_eq!(r.begin_close(2_500), Shrink::Nothing, "home already: the window may go at once");
    }

    /// Pressed again while it shrinks: it turns round from where it is, and the glass is where it was a moment ago.
    #[test]
    fn pressed_again_during_a_closing_it_turns_round_from_where_it_is() {
        let (_, _, spec) = setup(1.25, 1.0, Side::Left);
        let mut r = Run::new(true);
        r.start(0);
        r.settle(220);
        assert!(matches!(r.begin_close(1_000), Shrink::Shrinking { .. }));
        let at_turn = r.s_at(1_080);
        assert!((at_turn - 0.5).abs() < 1e-5);
        let again = r.reopen(1_080, true);
        assert!(r.is_opening() && !r.is_closing());
        assert_eq!(r.s_at(1_080), at_turn, "no jump at the turn");
        assert_eq!(spec.at(r.s_at(1_080)), spec.at(at_turn));
        match again {
            Again::TurnedBack { from, remaining_ms } => {
                assert_eq!(from, at_turn);
                assert_eq!(remaining_ms, 110, "the rest of the way up is the rest of 220 ms");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(r.reopen(1_081, true), Again::Same, "pressed while it opens: nothing turns");
        assert_eq!(r.settle(1_190), Arrival::Opened);
        // it had come home and the window was about to go when the button was pressed: it starts again from the island
        let mut h = Run::new(true);
        h.start(0);
        h.begin_close(500);
        assert_eq!(h.settle(700), Arrival::Home);
        assert_eq!(h.reopen(710, true), Again::Restarted);
        assert_eq!((h.phase, h.s_at(710)), (Phase::Arming, 0.0));
        // and while it is still arming nothing turns
        assert_eq!(Run::new(true).reopen(0, true), Again::Same);
    }

    #[test]
    fn a_panel_put_away_before_its_glass_came_has_nothing_to_shrink() {
        let mut r = Run::new(true);
        assert_eq!(r.begin_close(10), Shrink::Nothing);
        assert_eq!(r.phase, Phase::Arming);
        assert_eq!(r.settle(10), Arrival::Idle);
    }

    /// With animations off the glass is the panel in one frame and the island in one frame, and the island still gets its glass back
    /// in the deadline BEFORE the window goes (not after it: a window hidden first leaves the island with no glass for a frame).
    #[test]
    fn with_animations_off_it_is_there_in_one_frame_and_the_island_gets_its_glass_back_before_the_window_goes() {
        let mut r = Run::new(false);
        assert_eq!(r.start(100), 0);
        assert_eq!((r.s_at(100), r.moving(100)), (1.0, false));
        assert_eq!(r.settle(100), Arrival::Opened);
        assert_eq!(r.begin_close(300), Shrink::Shrinking { from: 1.0, remaining_ms: 0 });
        assert_eq!((r.s_at(300), r.moving(300)), (0.0, false));
        assert_eq!(r.phase, Phase::Running, "home is for the deadline to say");
        assert_eq!(r.settle(325), Arrival::Home);
        assert_eq!(r.phase, Phase::Closed);
    }

    /// The wash over the panel's glass comes in with the growth: the island's own glass has none, so the first and the last frame are
    /// the island's glass and nothing else (no step in the tone where one glass takes over from the other).
    #[test]
    fn the_wash_comes_in_with_the_growth_and_is_gone_when_the_island_is_home() {
        let (_, _, spec) = setup(1.0, 1.0, Side::Left);
        let mut r = Run::new(true);
        let u = |s: f32| spec.at(s).u;
        assert_eq!(r.wash_alpha(u(0.0), false), 0.0, "arming: the panel's fill is not drawn behind the island");
        assert_eq!(r.wash_alpha(u(0.0), true), 0.0, "not even the plain fill of a glass that has not come yet");
        r.start(0);
        assert_eq!(r.wash_alpha(u(0.0), false), 0.0, "the first frame of the motion is the island's glass");
        let mut last = 0.0;
        for i in 0..=100 {
            let a = r.wash_alpha(u(i as f32 / 100.0), false);
            assert!(a >= last && (0.0..=1.0).contains(&a), "i={i} a={a}");
            last = a;
        }
        assert_eq!(last, 1.0, "the panel as it always was");
        assert_eq!(r.wash_alpha(u(0.3), true), 1.0, "a glass that never came: the plain fill is all there is");
        r.begin_close(0);
        r.settle(160);
        assert_eq!(r.phase, Phase::Closed);
        assert_eq!(r.wash_alpha(u(0.0), false), 0.0, "home: the island's glass is back, the wash is gone");
    }

    /// What the wash is at each moment of the motion for a running glass (thin, coming in with the growth) and for none (plain, whole).
    /// The rule that it is picked from the ENGINE'S report at the frame and not from the view's belief (a watch that looks every 50 ms)
    /// is in the host (`view_frame`) and is not provable here; this pins the values and the home case.
    #[test]
    fn the_wash_is_thin_over_a_running_glass_and_whole_and_plain_only_where_there_is_none() {
        let (_, _, spec) = setup(1.0, 1.0, Side::Left);
        let mut r = Run::new(true);
        // the glass has not come: nothing is drawn behind the island, plain or not
        assert_eq!(r.wash(spec.at(0.0).u, false), Wash { alpha: 0.0, plain: true });
        r.start(1_000);
        // the clock has just started because the engine runs: every frame of the motion is a thin wash that comes in with the growth
        for i in 0..=100 {
            let w = r.wash(spec.at(i as f32 / 100.0).u, true);
            assert!(!w.plain, "i={i}: the engine runs, so the dense fill must never be painted");
            assert!(w.alpha <= 1.0 && w.alpha >= 0.0);
        }
        assert_eq!(r.wash(spec.at(0.0).u, true), Wash { alpha: 0.0, plain: false }, "the first frame is the island's glass, nothing over it");
        let early = r.wash(spec.at(0.03).u, true);
        assert!(early.alpha < 0.3 && !early.plain, "a few ms in, the wash is still faint: {early:?}");
        // a glass that never came (the wait ran out), or one lost during the motion: the plain fill is all there is, whole
        assert_eq!(r.wash(spec.at(0.3).u, false), Wash { alpha: 1.0, plain: true });
        // home again: nothing
        r.begin_close(1_000);
        r.settle(1_160);
        assert_eq!(r.wash(spec.at(0.0).u, true), Wash { alpha: 0.0, plain: false });
    }

    /// THE DARK BOX OF A SLOW ENGINE. The island's glass is put away when the clock starts; if the clock starts while the engine is still
    /// starting (the old rule did, 90 ms after the panel was shown) the panel's wash is the plain, dense fill over a shape with no glass
    /// under it. So whenever the clock may start without the engine running, the engine must be gone for good (the plain fill is the
    /// right picture then) or the net must have run out, and a slow engine alone (anything below the net) never starts it. With the old
    /// rule `arm_done(false, 90)` was true: a 90 ms start of the engine showed the box.
    #[test]
    fn a_slow_engine_never_gets_the_island_put_away_from_under_it() {
        for waited in [0, 8, 90, 91, 200, 500, ARM_ENGINE_MAX_MS - 1] {
            assert!(!arm_done(Engine::Starting, waited), "{waited} ms and the engine is still starting");
        }
        // only the net (an engine that neither runs nor fails for a whole second) starts it with the plain fill, and the longest wait is
        // the one the docs and the log promise
        assert!(arm_done(Engine::Starting, ARM_ENGINE_MAX_MS));
        assert_eq!(ARM_ENGINE_MAX_MS, 1000);
    }

    /// The glass of a panel that grows out of the left island is one row shorter than the panel was: the line and the 64 px row are
    /// the island's row now, and the content above keeps the height it had (the layout is worked out from the height it had).
    #[test]
    fn a_growing_start_panel_gives_up_its_own_bottom_row_and_the_content_keeps_its_height() {
        assert_eq!(OWN_ROW_H, 65.0);
        assert_eq!(height_without_own_row(720.0), 655.0);
        assert_eq!(height_with_own_row(655.0), 720.0, "the layout sees the height the panel had");
        for h in [720.0, 540.0, 300.0, 400.5] {
            assert_eq!(height_with_own_row(height_without_own_row(h)), h);
        }
        // a screen with next to no room: never zero or negative
        assert_eq!(height_without_own_row(1.0), 1.0);
        assert_eq!(height_without_own_row(40.0), 1.0);
        // on the user's screen: the glass is 65 px shorter than the 776 it was (panel 720 + 8 gap + 48 island) and still holds the island
        let (island, panel_rect, spec) = setup(1.0, 1.0, Side::Left);
        let work = Rect::new(0, 0, 2560, 1392);
        let shorter = panel::place(island, (startpage::PANEL_W, height_without_own_row(startpage::PANEL_MAX_H)), 1.0, Side::Left, work);
        assert_eq!(shorter.bottom, panel_rect.bottom, "still 8 px above the island");
        let grown = grown_rect(island, shorter);
        assert_eq!(grown.height(), spec.grown.height() - 65);
        assert_eq!(grown.bottom, island.bottom);
    }

    #[test]
    fn the_start_panel_numbers_at_100_percent_on_the_users_screen() {
        // 2560 x 1440 at 100 %, taskbar 48: the left glass (12, 1340, 342, 1388), the panel (12, 612, 654, 1332); the grown glass is
        // the panel's top to the island's bottom
        let (island, panel_rect, spec) = setup(1.0, 1.0, Side::Left);
        assert_eq!(island, Rect::new(12, 1340, 342, 1388));
        assert_eq!(panel_rect, Rect::new(12, 612, 654, 1332));
        assert_eq!(spec.grown, Rect::new(12, 612, 654, 1388));
        assert_eq!(spec.at(1.0).rect, [12.0, 612.0, 642.0, 776.0]);
        assert_eq!(spec.in_window(&spec.at(0.0)), [0.0, 728.0, 330.0, 48.0]);
    }
}
