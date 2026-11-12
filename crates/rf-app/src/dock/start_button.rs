//! The Start button of the left island: a gradient Windows-style logo that answers the pointer the way the Start button of Windows
//! 11's taskbar does, drawn by our own code (vector shapes, nothing taken from Windows' resources).
//!
//! * THE LOGO: four squares in a 2 x 2 grid with a small gap and slightly rounded corners, filled with ONE diagonal gradient across
//!   the whole logo, light sky blue at the top left to a deeper blue at the bottom right. The two colours are [`GRADIENT_FROM`] and
//!   [`GRADIENT_TO`], the only place to tune them. The toolkit fills one shape with one gradient, so each square gets the piece of
//!   the big gradient that its place has ([`square_stops`]); next to each other the four pieces read as one gradient. The squares are
//!   laid out on the DEVICE pixel grid ([`logo_geometry`]): whole device pixels for the squares and the gap, so every edge lands on a
//!   pixel and the gaps are equal at every scale factor and at every moment of the motion.
//! * THE PLATE: a rounded plate behind the logo fades in on hover, is stronger while pressed and stays (the "active" look) while the
//!   Start panel is open.
//! * THE MOTION: on press the logo shrinks to [`PRESS_SCALE`] quickly; on release it springs back past its size ([`OVERSHOOT`]) and
//!   settles at 1.0 in about [`RELEASE_MS`]. When the panel opens by some other way than a click on the logo (the keyboard, the search
//!   field) the same press and release plays once. With animations off every state changes at once.
//!
//! WHAT IS PURE (and tested): [`StartButton`], the states and what they look like at a moment in time. The view calls its events
//! (`enter`, `leave`, `press`, `release`, `panel_changed`) with the time of the event and asks [`StartButton::frame`] for the scale
//! and the plate strength at the time of a frame. [`StartButton::moving`] says whether anything is still in motion: the island asks for
//! frames only then, so a button at rest costs nothing.

use gpui_kit::*;
use rf_win::dock::magnify::ease_in_out;

use super::look::Tint;

// ---------------------------------------------------------------- the numbers (all in one place)

/// The gradient's colour at the top left of the logo, 0xRRGGBB.
pub const GRADIENT_FROM: u32 = 0x7FDBFF;
/// The gradient's colour at the bottom right of the logo, 0xRRGGBB.
pub const GRADIENT_TO: u32 = 0x0A84E0;

/// One square's side, logical px.
pub const SQUARE: f32 = 8.0;
/// The gap between the squares, logical px.
pub const GAP: f32 = 1.5;
/// The squares' corner radius, logical px ("very slightly rounded").
pub const CORNER: f32 = 1.2;
/// The whole logo's side at rest, logical px.
pub const LOGO_SIDE: f32 = 2.0 * SQUARE + GAP;

/// The size the logo shrinks to while pressed.
pub const PRESS_SCALE: f32 = 0.82;
/// How long the shrink takes.
pub const PRESS_MS: u32 = 80;
/// The size the logo springs past on release.
pub const OVERSHOOT: f32 = 1.06;
/// The way up from the pressed size to the overshoot.
pub const RELEASE_UP_MS: u32 = 110;
/// The way back from the overshoot to 1.0.
pub const RELEASE_SETTLE_MS: u32 = 170;
/// The whole release (the two stretches above; the tests check the total).
#[cfg(test)]
pub const RELEASE_MS: u32 = RELEASE_UP_MS + RELEASE_SETTLE_MS;
/// A release outside the button (or the pointer leaving while pressed) just eases back, without the spring.
pub const OUTSIDE_MS: u32 = 160;

/// The plate's strength under the pointer or while the panel is open: a multiple of the dock's hover fill.
pub const PLATE_ACTIVE: f32 = 1.0;
/// The plate's strength while pressed: the dock's pressed fill is about this much stronger than its hover fill.
pub const PLATE_PRESSED: f32 = 1.6;
/// The plate fading in or out.
pub const PLATE_FADE_MS: u32 = 140;
/// The plate growing to its pressed strength.
pub const PLATE_PRESS_MS: u32 = 70;

/// A panel that opens this soon after the logo was pressed was opened by that press: its own press and release is the animation.
pub const OWN_CLICK_MS: u64 = 1500;

// ---------------------------------------------------------------- the gradient

/// The colour of the big gradient at `t` (0 at the top left of the logo, 1 at the bottom right), channel by channel in sRGB.
pub fn gradient_at(t: f32) -> u32 {
    Tint::mix(Tint::new(GRADIENT_FROM, 1.0), Tint::new(GRADIENT_TO, 1.0), t).rgb
}

/// Where a square's own gradient reaches its two end colours, as a fraction of the shader's 0..1 run. The shader runs a 135 degree
/// gradient as `0.5 + (dx + dy) / (sqrt 2 * side)` for a point `dx`, `dy` from the square's centre and clamps it to 0..1. A corner of
/// the square is `dx + dy = +-side` from the centre, which is `0.5 +- 1/sqrt 2` there: with the ends at these two fractions (outside
/// 0..1, which the shader takes as they are) no corner is cut off and no square has a flat patch.
pub const STOP_START: f32 = 0.5 - std::f32::consts::FRAC_1_SQRT_2;
/// See [`STOP_START`].
pub const STOP_END: f32 = 0.5 + std::f32::consts::FRAC_1_SQRT_2;

/// The big gradient's colour at the two corners of one square, the top left one and the bottom right one (these are the end colours
/// of that square's own gradient, set at [`STOP_START`] and [`STOP_END`]). A square at (`row`, `col`), both 0 or 1, of side `square`
/// with `gap` between the squares (any one unit, the same for both); the big gradient runs `(X + Y) / (2 * logo side)` across the
/// logo, so the corner colours follow from where the corners are, and the logo's own corners show exactly the two tuned colours.
pub fn square_stops(row: usize, col: usize, square: f32, gap: f32) -> (u32, u32) {
    let logo_side = 2.0 * square + gap;
    let (x0, y0) = (col as f32 * (square + gap), row as f32 * (square + gap));
    let t_start = (x0 + y0) / (2.0 * logo_side);
    let t_end = (x0 + y0 + 2.0 * square) / (2.0 * logo_side);
    (gradient_at(t_start), gradient_at(t_end))
}

/// The logo's squares in whole DEVICE pixels at one moment of the motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogoGeometry {
    /// One square's side, device px.
    pub square: u32,
    /// The gap between the squares, device px (the same across and down).
    pub gap: u32,
}

impl LogoGeometry {
    /// The whole logo's side, device px.
    pub fn side(self) -> u32 {
        2 * self.square + self.gap
    }
}

/// The squares and the gap of the logo at `scale` (1.0 at rest) on a screen of `scale_factor` device px per logical px, each a whole
/// number of device pixels. GPUI snaps every quad's four edges to the device pixels on its own, one quad at a time, so a size that is
/// not whole would give gaps of one pixel here and two there, and squares that change by a pixel from frame to frame in different
/// places. With whole numbers every edge is already on a pixel, and the same pixel offset moves all four squares together. The side
/// is rounded first and the gap after it, so the logo grows and shrinks by whole pixels as the scale moves (the spring is seen at
/// 100 % too: 18 px at rest, 20 px at the top of the spring, 15 px pressed).
pub fn logo_geometry(scale: f32, scale_factor: f32) -> LogoGeometry {
    let k = scale_factor.max(0.25) * scale.max(0.0);
    let side = (LOGO_SIDE * k).round().max(3.0);
    let gap = (GAP * k).round().max(1.0);
    let square = ((side - gap) / 2.0).round().max(1.0);
    LogoGeometry { square: square as u32, gap: gap as u32 }
}

// ---------------------------------------------------------------- tracks

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ease {
    /// Fast at the start, slow at the end: a press.
    Out,
    /// Slow at both ends.
    InOut,
}

impl Ease {
    fn at(self, t: f32) -> f32 {
        match self {
            Ease::Out => 1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3),
            Ease::InOut => ease_in_out(t),
        }
    }
}

/// One stretch of a value's way: from `from` to `to` over `dur` ms, beginning at `start` ms.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seg {
    start: u64,
    dur: u32,
    from: f32,
    to: f32,
    ease: Ease,
}

/// A value over time: it is `base` until the first stretch begins and follows the stretches one after the other. A new set of
/// stretches always begins at the value the track has at that moment, so nothing ever jumps.
#[derive(Clone, Debug, PartialEq)]
struct Track {
    base: f32,
    segs: Vec<Seg>,
}

impl Track {
    fn at_rest(v: f32) -> Track {
        Track { base: v, segs: Vec::new() }
    }

    fn value(&self, now: u64) -> f32 {
        let mut v = self.base;
        for s in &self.segs {
            if now < s.start {
                return v;
            }
            let elapsed = now - s.start;
            if elapsed >= u64::from(s.dur) {
                v = s.to;
                continue;
            }
            return s.from + (s.to - s.from) * s.ease.at(elapsed as f32 / s.dur as f32);
        }
        v
    }

    /// Still moving, or about to: a stretch has not ended yet.
    fn busy(&self, now: u64) -> bool {
        self.segs.last().is_some_and(|s| now < s.start + u64::from(s.dur))
    }

    /// Continue from the value at `now` along `steps` (`(delay from now, ms, to, ease)`, one after the other). No stretches at all, or
    /// a total of 0 ms, and the track is at its last value at once.
    fn go(&mut self, now: u64, steps: &[(u32, u32, f32, Ease)]) {
        let mut from = self.value(now);
        let mut at = now;
        let mut segs = Vec::with_capacity(steps.len());
        for &(delay, dur, to, ease) in steps {
            at += u64::from(delay);
            segs.push(Seg { start: at, dur, from, to, ease });
            at += u64::from(dur);
            from = to;
        }
        if steps.iter().all(|s| s.0 == 0 && s.1 == 0) {
            self.base = from;
            self.segs.clear();
            return;
        }
        self.base = self.value(now);
        self.segs = segs;
    }
}

// ---------------------------------------------------------------- the button

/// What the button looks like at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The logo's size, 1.0 at rest.
    pub scale: f32,
    /// The plate's strength: 0 none, [`PLATE_ACTIVE`] hovered or open, [`PLATE_PRESSED`] pressed.
    pub plate: f32,
}

/// The states of the Start button and their motion. Every event takes the time it happened (ms on any clock that does not run back)
/// and whether animations are on; the answers depend on nothing else.
#[derive(Clone, Debug, PartialEq)]
pub struct StartButton {
    hovered: bool,
    pressed: bool,
    panel_open: bool,
    scale: Track,
    plate: Track,
    /// The plate's strength the track is heading for (not to start the same fade again).
    plate_goal: f32,
    last_press: Option<u64>,
}

impl Default for StartButton {
    fn default() -> Self {
        StartButton::new()
    }
}

impl StartButton {
    pub fn new() -> StartButton {
        StartButton { hovered: false, pressed: false, panel_open: false, scale: Track::at_rest(1.0), plate: Track::at_rest(0.0), plate_goal: 0.0, last_press: None }
    }

    /// The scale and the plate at `now`.
    pub fn frame(&self, now: u64) -> Frame {
        Frame { scale: self.scale.value(now), plate: self.plate.value(now) }
    }

    /// Whether anything is still moving at `now` (the island asks for another frame only then).
    pub fn moving(&self, now: u64) -> bool {
        self.scale.busy(now) || self.plate.busy(now)
    }

    pub fn is_pressed(&self) -> bool {
        self.pressed
    }

    fn goal(&self) -> f32 {
        if self.pressed {
            PLATE_PRESSED
        } else if self.hovered || self.panel_open {
            PLATE_ACTIVE
        } else {
            0.0
        }
    }

    /// The plate heads for the strength the states ask for.
    fn retarget_plate(&mut self, now: u64, animate: bool) {
        let goal = self.goal();
        if goal == self.plate_goal {
            return;
        }
        self.plate_goal = goal;
        let ms = if !animate {
            0
        } else if goal == PLATE_PRESSED {
            PLATE_PRESS_MS
        } else {
            PLATE_FADE_MS
        };
        self.plate.go(now, &[(0, ms, goal, Ease::InOut)]);
    }

    /// The pointer came onto the button.
    pub fn enter(&mut self, now: u64, animate: bool) {
        self.hovered = true;
        self.retarget_plate(now, animate);
    }

    /// The pointer left the button. A press that is still held goes on (the logo stays small and the plate stays pressed): the
    /// mouse button coming up decides, inside it springs back, outside it eases back, and the pointer coming back in meanwhile is
    /// just hovering again.
    pub fn leave(&mut self, now: u64, animate: bool) {
        self.hovered = false;
        self.retarget_plate(now, animate);
    }

    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// The mouse button went down on the button: the logo shrinks.
    pub fn press(&mut self, now: u64, animate: bool) {
        if self.pressed {
            return;
        }
        self.pressed = true;
        self.last_press = Some(now);
        if animate {
            self.scale.go(now, &[(0, PRESS_MS, PRESS_SCALE, Ease::Out)]);
        } else {
            self.scale.go(now, &[(0, 0, PRESS_SCALE, Ease::Out)]);
        }
        self.retarget_plate(now, animate);
    }

    /// The mouse button went up: inside the button the logo springs back past its size and settles; outside it eases back.
    pub fn release(&mut self, now: u64, inside: bool, animate: bool) {
        if !self.pressed {
            return;
        }
        self.pressed = false;
        match (animate, inside) {
            (false, _) => self.scale.go(now, &[(0, 0, 1.0, Ease::InOut)]),
            (true, true) => self.scale.go(now, &[(0, RELEASE_UP_MS, OVERSHOOT, Ease::Out), (0, RELEASE_SETTLE_MS, 1.0, Ease::InOut)]),
            (true, false) => self.scale.go(now, &[(0, OUTSIDE_MS, 1.0, Ease::InOut)]),
        }
        self.retarget_plate(now, animate);
    }

    /// The Start panel opened or closed. Open keeps the plate as the "active" look. When it opened without a press on the logo (the
    /// keyboard, the search field) the press and release plays once; when the user pressed the logo a moment ago that press was it.
    pub fn panel_changed(&mut self, open: bool, now: u64, animate: bool) {
        if open == self.panel_open {
            return;
        }
        self.panel_open = open;
        let own = self.pressed || self.last_press.is_some_and(|t| now.saturating_sub(t) < OWN_CLICK_MS);
        if open && animate && !own {
            self.scale.go(now, &[(0, PRESS_MS, PRESS_SCALE, Ease::Out), (0, RELEASE_UP_MS, OVERSHOOT, Ease::Out), (0, RELEASE_SETTLE_MS, 1.0, Ease::InOut)]);
            self.plate_goal = PLATE_ACTIVE;
            self.plate.go(now, &[(0, PLATE_PRESS_MS, PLATE_PRESSED, Ease::InOut), (0, PLATE_FADE_MS, PLATE_ACTIVE, Ease::InOut)]);
            return;
        }
        self.retarget_plate(now, animate);
    }
}

// ---------------------------------------------------------------- drawing

/// The logo at `scale` on a screen of `scale_factor` device px per logical px: four squares of the gradient on the device pixel grid
/// ([`logo_geometry`]), laid out again at every size (vector shapes, so crisp at any scale and at any moment of the motion).
pub fn logo(scale: f32, scale_factor: f32) -> impl IntoElement {
    let g = logo_geometry(scale, scale_factor);
    let logical = move |device: u32| px(device as f32 / scale_factor);
    let radius = px(CORNER * scale);
    let step = g.square + g.gap;
    let cell = move |row: usize, col: usize| {
        let (from, to) = square_stops(row, col, g.square as f32, g.gap as f32);
        div()
            .absolute()
            .left(logical(col as u32 * step))
            .top(logical(row as u32 * step))
            .size(logical(g.square))
            .rounded(radius)
            .bg(linear_gradient(135., linear_color_stop(rgb(from), STOP_START), linear_color_stop(rgb(to), STOP_END)))
    };
    div().relative().w(logical(g.side())).h(logical(g.side())).child(cell(0, 0)).child(cell(0, 1)).child(cell(1, 0)).child(cell(1, 1))
}

/// What the plate is painted with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlatePaint {
    pub fill: Hsla,
    pub edge: Hsla,
}

/// The plate's colours at `plate` times the hover fill's strength; `light` is the palette's mix (0 dark glass, 1 light glass). Nothing at
/// all when the plate is not showing.
pub fn plate_paint(plate: f32, hover_fill: Tint, light: f32) -> Option<PlatePaint> {
    if plate <= 0.002 {
        return None;
    }
    let mut fill = hover_fill.hsla();
    fill.a = (fill.a * plate).min(0.9);
    // the edge is light on the dark glass and brighter still on the light one, where the plate itself is a dark veil
    let mut edge: Hsla = rgb(0xFFFFFF).into();
    edge.a = (0.14 + (0.55 - 0.14) * light.clamp(0.0, 1.0)) * plate.min(PLATE_PRESSED);
    Some(PlatePaint { fill, edge })
}

/// The plate behind the logo: the dock's hover fill at `plate` times its strength, with a hairline lighter top edge as the buttons of
/// Windows' taskbar have ([`plate_paint`]).
pub fn plate(plate: f32, hover_fill: Tint, light: f32, radius: f32) -> Option<impl IntoElement> {
    let paint = plate_paint(plate, hover_fill, light)?;
    Some(
        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .rounded(px(radius))
            .bg(paint.fill)
            .child(div().absolute().top(px(0.)).left(px(radius * 0.6)).right(px(radius * 0.6)).h(px(1.)).rounded_full().bg(paint.edge)),
    )
}

#[cfg(test)]
mod tests {
    // (not `super::*`: the toolkit's glob brings a `test` macro that would shadow `#[test]`)
    use super::{
        CORNER, Frame, GAP, GRADIENT_FROM, GRADIENT_TO, LOGO_SIDE, OUTSIDE_MS, OVERSHOOT, PLATE_ACTIVE, PLATE_FADE_MS, PLATE_PRESSED, PLATE_PRESS_MS, PRESS_MS,
        PRESS_SCALE, RELEASE_MS, RELEASE_UP_MS, SQUARE, STOP_END, STOP_START, StartButton, Tint, gradient_at, logo_geometry,
        plate_paint, square_stops,
    };

    const ON: bool = true;
    const OFF: bool = false;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn sample(b: &StartButton, from: u64, to: u64) -> Vec<f32> {
        (from..=to).map(|t| b.frame(t).scale).collect()
    }

    #[test]
    fn at_rest_nothing_shows_and_nothing_moves() {
        let b = StartButton::new();
        assert_eq!(b.frame(0), Frame { scale: 1.0, plate: 0.0 });
        assert_eq!(b.frame(10_000), Frame { scale: 1.0, plate: 0.0 });
        assert!(!b.moving(0));
        assert!(!b.moving(10_000));
    }

    #[test]
    fn hovering_fades_the_plate_in_and_out_and_leaves_the_size_alone() {
        let mut b = StartButton::new();
        b.enter(1000, ON);
        assert!(b.moving(1010));
        let mid = b.frame(1000 + u64::from(PLATE_FADE_MS) / 2);
        assert!(mid.plate > 0.3 && mid.plate < 0.7, "half way in: {}", mid.plate);
        let done = b.frame(1000 + u64::from(PLATE_FADE_MS));
        assert!(near(done.plate, PLATE_ACTIVE));
        assert!(near(done.scale, 1.0));
        assert!(!b.moving(1000 + u64::from(PLATE_FADE_MS)));
        b.leave(2000, ON);
        assert!(near(b.frame(2000 + u64::from(PLATE_FADE_MS)).plate, 0.0));
        assert!(b.frame(2050).plate < PLATE_ACTIVE && b.frame(2050).plate > 0.0);
    }

    #[test]
    fn pressing_shrinks_to_the_pressed_size_in_80_ms_fast_at_first() {
        let mut b = StartButton::new();
        b.enter(0, ON);
        b.press(1000, ON);
        assert!(b.is_pressed());
        // eased out: past half of the way after half of the time
        let half = b.frame(1000 + u64::from(PRESS_MS) / 2).scale;
        assert!(half < (1.0 + PRESS_SCALE) / 2.0, "eased out, not linear: {half}");
        assert!(near(b.frame(1000 + u64::from(PRESS_MS)).scale, PRESS_SCALE));
        // it holds there for as long as the button is down
        assert!(near(b.frame(9000).scale, PRESS_SCALE));
        assert!(!b.moving(9000));
        // and the plate is stronger than under the pointer alone
        assert!(near(b.frame(9000).plate, PLATE_PRESSED));
        const { assert!(PLATE_PRESSED > PLATE_ACTIVE) };
        // never smaller than the pressed size on the way
        assert!(sample(&b, 1000, 1200).iter().all(|s| *s >= PRESS_SCALE - 1e-4));
    }

    #[test]
    fn releasing_inside_springs_past_the_size_and_settles_in_about_280_ms() {
        let mut b = StartButton::new();
        b.enter(0, ON);
        b.press(1000, ON);
        b.release(2000, true, ON);
        assert_eq!(RELEASE_MS, 280);
        let s = sample(&b, 2000, 2400);
        let peak = s.iter().cloned().fold(0.0, f32::max);
        assert!(near(peak, OVERSHOOT), "springs to {peak}");
        assert!(s.iter().all(|v| *v >= PRESS_SCALE - 1e-4 && *v <= OVERSHOOT + 1e-4));
        // the peak is at the end of the way up
        assert!(near(b.frame(2000 + u64::from(RELEASE_UP_MS)).scale, OVERSHOOT));
        // it is past 1.0 for a while and then comes back down to exactly 1.0
        assert!(b.frame(2000 + 150).scale > 1.0);
        assert!(near(b.frame(2000 + u64::from(RELEASE_MS)).scale, 1.0));
        assert!(near(b.frame(2400).scale, 1.0));
        assert!(b.moving(2000 + u64::from(RELEASE_MS) - 1));
        assert!(!b.moving(2000 + u64::from(RELEASE_MS)));
        // the pointer is still on the button: the plate goes back to the hover strength, not away
        assert!(near(b.frame(2400).plate, PLATE_ACTIVE));
    }

    #[test]
    fn releasing_outside_eases_back_without_the_spring() {
        let mut b = StartButton::new();
        b.enter(0, ON);
        b.press(1000, ON);
        b.release(1500, false, ON);
        let s = sample(&b, 1500, 1800);
        assert!(s.iter().all(|v| *v <= 1.0 + 1e-4), "no overshoot");
        assert!(s.windows(2).all(|w| w[1] >= w[0] - 1e-5), "only up");
        assert!(near(b.frame(1500 + u64::from(OUTSIDE_MS)).scale, 1.0));
    }

    #[test]
    fn leaving_while_held_does_not_end_the_press_and_the_release_inside_still_springs() {
        // GPUI reports the pointer as gone on the first mouse move while the button is down, even one pixel inside the button:
        // that must not take the spring away.
        let mut b = StartButton::new();
        b.enter(0, ON);
        b.press(1000, ON);
        b.leave(1300, ON);
        assert!(b.is_pressed(), "still held");
        assert!(!b.is_hovered());
        assert!(near(b.frame(2000).scale, PRESS_SCALE), "still small");
        assert!(near(b.frame(2000).plate, PLATE_PRESSED), "the plate does not fade away under the held button");
        // the pointer is told to be back; the button comes up inside
        b.enter(2100, ON);
        b.release(2100, true, ON);
        let s = sample(&b, 2100, 2500);
        assert!(near(s.iter().cloned().fold(0.0, f32::max), OVERSHOOT), "the spring plays");
        assert!(near(b.frame(2100 + u64::from(RELEASE_MS)).scale, 1.0));
        assert!(near(b.frame(2600).plate, PLATE_ACTIVE));
    }

    #[test]
    fn leaving_while_held_and_releasing_outside_eases_back_and_lets_the_plate_go() {
        let mut b = StartButton::new();
        b.enter(0, ON);
        b.press(1000, ON);
        b.leave(1300, ON);
        b.release(1700, false, ON);
        assert!(!b.is_pressed());
        let s = sample(&b, 1700, 2000);
        assert!(s.iter().all(|v| *v <= 1.0 + 1e-4), "no spring outside");
        assert!(near(b.frame(1700 + u64::from(OUTSIDE_MS)).scale, 1.0));
        assert!(near(b.frame(1700 + u64::from(PLATE_FADE_MS)).plate, 0.0));
        // a second mouse-up changes nothing
        let before = b.clone();
        b.release(1800, false, ON);
        assert_eq!(b, before);
    }

    #[test]
    fn the_pointer_leaving_and_returning_while_held_is_still_one_press_and_one_spring() {
        let mut b = StartButton::new();
        b.enter(0, ON);
        b.press(100, ON);
        b.leave(150, ON);
        b.enter(170, ON);
        assert!(b.is_pressed() && b.is_hovered());
        assert!(near(b.frame(1000).scale, PRESS_SCALE));
        b.release(1000, true, ON);
        assert!(near(sample(&b, 1000, 1400).iter().cloned().fold(0.0, f32::max), OVERSHOOT));
    }

    #[test]
    fn a_press_during_a_release_starts_from_where_the_size_is_without_a_jump() {
        let mut b = StartButton::new();
        b.enter(0, ON);
        b.press(1000, ON);
        b.release(2000, true, ON);
        let at = 2000 + 60;
        let before = b.frame(at).scale;
        assert!(before < OVERSHOOT && before > PRESS_SCALE);
        b.press(at, ON);
        assert!(near(b.frame(at).scale, before), "no jump: {} then {}", before, b.frame(at).scale);
        assert!(b.frame(at + 10).scale <= before);
        assert!(near(b.frame(at + u64::from(PRESS_MS)).scale, PRESS_SCALE));
        // and once more on the way up from the overshoot
        b.release(at + 500, true, ON);
        let high = b.frame(at + 500 + u64::from(RELEASE_UP_MS) + 10).scale;
        assert!(high > 1.0);
        b.press(at + 500 + u64::from(RELEASE_UP_MS) + 10, ON);
        assert!(near(b.frame(at + 500 + u64::from(RELEASE_UP_MS) + 10).scale, high));
        assert!(near(b.frame(at + 500 + u64::from(RELEASE_UP_MS) + 10 + u64::from(PRESS_MS)).scale, PRESS_SCALE));
    }

    #[test]
    fn an_open_panel_keeps_the_plate_after_the_pointer_left_and_a_closed_one_lets_it_go() {
        let mut b = StartButton::new();
        b.enter(0, ON);
        b.press(1000, ON);
        b.release(1100, true, ON);
        b.panel_changed(true, 1200, ON);
        b.leave(1300, ON);
        assert!(near(b.frame(5000).plate, PLATE_ACTIVE), "active while open");
        assert!(near(b.frame(5000).scale, 1.0));
        b.panel_changed(false, 6000, ON);
        assert!(near(b.frame(6000 + u64::from(PLATE_FADE_MS)).plate, 0.0));
        assert!(!b.moving(6000 + u64::from(PLATE_FADE_MS)));
    }

    #[test]
    fn a_panel_opened_by_the_logos_own_click_does_not_play_the_press_again() {
        let mut b = StartButton::new();
        b.enter(0, ON);
        b.press(1000, ON);
        b.release(1090, true, ON);
        // the panel's window takes its time to appear, up to a second
        b.panel_changed(true, 1900, ON);
        // the release's spring is over by then: nothing but the plate is moving
        assert!(near(b.frame(1900).scale, 1.0));
        assert!(!b.scale.busy(1900));
        // while the button is still down it is the press itself
        let mut c = StartButton::new();
        c.press(100, ON);
        c.panel_changed(true, 150, ON);
        assert!(near(c.frame(5000).scale, PRESS_SCALE));
    }

    #[test]
    fn a_panel_opened_from_the_keyboard_or_the_search_field_plays_one_press_and_release() {
        let mut b = StartButton::new();
        b.panel_changed(true, 5000, ON);
        assert!(b.moving(5010));
        assert!(near(b.frame(5000 + u64::from(PRESS_MS)).scale, PRESS_SCALE));
        let s = sample(&b, 5000, 5400);
        let peak = s.iter().cloned().fold(0.0, f32::max);
        assert!(near(peak, OVERSHOOT));
        assert!(s.iter().all(|v| *v >= PRESS_SCALE - 1e-4));
        let end = 5000 + u64::from(PRESS_MS + RELEASE_MS);
        assert!(near(b.frame(end).scale, 1.0));
        assert!(!b.moving(end));
        // the plate went up to the pressed strength and settled on the active one
        assert!(near(b.frame(5000 + u64::from(PLATE_PRESS_MS)).plate, PLATE_PRESSED));
        assert!(near(b.frame(end).plate, PLATE_ACTIVE));
        // only once: closing and opening again after the time of a click plays again, the same call twice does not restart it
        b.panel_changed(true, end + 10, ON);
        assert!(!b.moving(end + 10));
        b.panel_changed(false, end + 20, ON);
        b.panel_changed(true, end + 5000, ON);
        assert!(b.moving(end + 5010));
    }

    #[test]
    fn with_animations_off_every_state_changes_at_once() {
        let mut b = StartButton::new();
        b.enter(100, OFF);
        assert_eq!(b.frame(100), Frame { scale: 1.0, plate: PLATE_ACTIVE });
        assert!(!b.moving(100));
        b.press(200, OFF);
        assert_eq!(b.frame(200), Frame { scale: PRESS_SCALE, plate: PLATE_PRESSED });
        assert!(!b.moving(200));
        b.release(300, true, OFF);
        assert_eq!(b.frame(300), Frame { scale: 1.0, plate: PLATE_ACTIVE });
        assert!(sample(&b, 300, 700).iter().all(|v| *v == 1.0), "no spring");
        b.panel_changed(true, 400, OFF);
        b.leave(500, OFF);
        assert_eq!(b.frame(500), Frame { scale: 1.0, plate: PLATE_ACTIVE });
        b.panel_changed(false, 600, OFF);
        assert_eq!(b.frame(600), Frame { scale: 1.0, plate: 0.0 });
        assert!(!b.moving(600));
    }

    #[test]
    fn a_panel_opened_with_animations_off_plays_nothing() {
        let mut b = StartButton::new();
        b.panel_changed(true, 100, OFF);
        assert!(!b.moving(100));
        assert_eq!(b.frame(100), Frame { scale: 1.0, plate: PLATE_ACTIVE });
    }

    #[test]
    fn the_same_event_twice_changes_nothing() {
        let mut b = StartButton::new();
        b.enter(0, ON);
        let once = b.clone();
        b.enter(50, ON);
        assert_eq!(b, once);
        b.press(100, ON);
        let pressed = b.clone();
        b.press(120, ON);
        assert_eq!(b, pressed);
        b.release(300, true, ON);
        let released = b.clone();
        b.release(310, true, ON);
        assert_eq!(b, released);
    }

    fn channels(c: u32) -> [i32; 3] {
        [((c >> 16) & 0xFF) as i32, ((c >> 8) & 0xFF) as i32, (c & 0xFF) as i32]
    }

    fn close_to(a: u32, b: u32, tol: i32) -> bool {
        channels(a).iter().zip(channels(b)).all(|(x, y)| (x - y).abs() <= tol)
    }

    #[test]
    fn the_four_squares_read_as_one_gradient_from_light_to_deep_blue() {
        for (square, gap) in [(8.0, 1.5), (8.0, 2.0), (13.0, 2.0), (17.0, 3.0)] {
            let side = 2.0 * square + gap;
            let all = [square_stops(0, 0, square, gap), square_stops(0, 1, square, gap), square_stops(1, 0, square, gap), square_stops(1, 1, square, gap)];
            // the corners of the whole logo show exactly the two tuned colours: nothing is clamped away
            assert_eq!(all[0].0, GRADIENT_FROM);
            assert_eq!(all[3].1, GRADIENT_TO);
            // the other two squares are the same colours (the gradient is along the diagonal) and each piece goes light to deep
            assert_eq!(all[1], all[2]);
            for (a, b) in all {
                assert!(channels(a)[0] > channels(b)[0], "{a:06x} {b:06x}");
            }
            // each square's pieces start where the big gradient is at its top left corner and end at its bottom right one
            let want = |x: f32, y: f32| gradient_at((x + y) / (2.0 * side));
            assert_eq!(all[1].0, want(square + gap, 0.0));
            assert_eq!(all[1].1, want(square + gap + square, square));
            // the middle of a square's piece is the big gradient at the middle of that square
            let (a, b) = all[3];
            let mid = square + gap + square / 2.0;
            assert!(close_to(Tint::mix(Tint::new(a, 1.0), Tint::new(b, 1.0), 0.5).rgb, want(mid, mid), 1));
        }
        assert_eq!(gradient_at(0.0), GRADIENT_FROM);
        assert_eq!(gradient_at(1.0), GRADIENT_TO);
        // the ends of the shader's run sit a corner away from the centre, so that no corner is clamped
        assert!((STOP_START - (0.5 - 1.0 / 2f32.sqrt())).abs() < 1e-6 && (STOP_END - (0.5 + 1.0 / 2f32.sqrt())).abs() < 1e-6);
        const { assert!(STOP_START < 0.0 && STOP_END > 1.0) };
    }

    #[test]
    fn the_logo_sits_on_whole_device_pixels_with_equal_gaps_at_every_scale_factor() {
        for sf in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0] {
            let rest = logo_geometry(1.0, sf);
            let pressed = logo_geometry(PRESS_SCALE, sf);
            let spring = logo_geometry(OVERSHOOT, sf);
            // the three sizes the motion goes through are different, in the right order, in whole pixels
            assert!(pressed.side() < rest.side() && rest.side() < spring.side(), "sf {sf}: {} {} {}", pressed.side(), rest.side(), spring.side());
            // the rest is the size that was designed: within a pixel of the logical one
            assert!((rest.side() as f32 - super::LOGO_SIDE * sf).abs() <= 1.0, "sf {sf}: {}", rest.side());
            assert!(rest.gap >= 1 && pressed.gap >= 1 && spring.gap >= 1, "a gap that is a real line");
            // along the whole press-to-spring range every square and every gap is at least 1 px and the logo never gets smaller
            // while the scale grows
            let mut last = 0;
            let mut s = PRESS_SCALE;
            while s <= OVERSHOOT {
                let g = logo_geometry(s, sf);
                assert!(g.square >= 1 && g.gap >= 1);
                assert!(g.side() + 1 >= last, "sf {sf} scale {s}: {} after {last}", g.side());
                last = g.side();
                s += 0.001;
            }
        }
        // 100 %: the numbers the doc comment gives
        assert_eq!((logo_geometry(1.0, 1.0).side(), logo_geometry(OVERSHOOT, 1.0).side(), logo_geometry(PRESS_SCALE, 1.0).side()), (18, 20, 15));
    }

    #[test]
    fn the_logo_is_a_little_larger_than_two_squares_and_fits_its_button() {
        assert_eq!(LOGO_SIDE, 2.0 * SQUARE + GAP);
        const { assert!(LOGO_SIDE * OVERSHOOT < super::super::plan::LOGO, "even the spring stays inside the button") };
        const { assert!(CORNER > 0.0 && CORNER < SQUARE / 4.0, "slightly rounded") };
    }

    #[test]
    fn the_plate_shows_nothing_at_rest_and_is_stronger_when_pressed() {
        let fill = Tint::new(0xFFFFFF, 0.10);
        assert!(plate_paint(0.0, fill, 0.0).is_none());
        let active = plate_paint(PLATE_ACTIVE, fill, 0.0).unwrap();
        let pressed = plate_paint(PLATE_PRESSED, fill, 0.0).unwrap();
        // under the pointer: the dock's hover fill as it is; pressed: 1.6 times as strong
        assert!((active.fill.a - 0.10).abs() < 1e-6, "{}", active.fill.a);
        assert!((pressed.fill.a - 0.16).abs() < 1e-6, "{}", pressed.fill.a);
        // half way through the fade it is half as strong
        assert!((plate_paint(0.5, fill, 0.0).unwrap().fill.a - 0.05).abs() < 1e-6);
        // the hairline edge: faint white on the dark glass, brighter on the light one, stronger when pressed
        assert!((active.edge.a - 0.14).abs() < 1e-6 && (plate_paint(PLATE_ACTIVE, fill, 1.0).unwrap().edge.a - 0.55).abs() < 1e-6);
        assert!(pressed.edge.a > active.edge.a);
        assert_eq!(active.edge.l, 1.0, "white");
        // a strong fill is capped
        assert!(plate_paint(PLATE_PRESSED, Tint::new(0x000000, 0.8), 1.0).unwrap().fill.a <= 0.9);
    }
}
