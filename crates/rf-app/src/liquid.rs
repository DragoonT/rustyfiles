//! The "liquid" look of our own controls: the switch, the slider, the dropdown button and the rows of menus and dropdown lists, in the
//! spirit of iOS 27's Liquid Glass (capsules, a thumb that turns into a glass lens while it is held, dense frosted panels). Nothing is
//! copied from any kit; the numbers are the ones decided for rustyfiles.
//!
//! WHEN: only in a window that shows glass and in the popups opened from one (`Palette::liquid`, decided by `glass::liquid`). In every
//! other window and over Mica, Acrylic and Solid the controls are what they always were. The accent stays the user's Windows accent
//! (the kit's green is not used).
//!
//! The geometry, the colours and the motion are pure functions here (tested); the few functions that build elements only put them on
//! the screen.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::theme::Palette;

/// A box in logical px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    #[cfg(test)]
    pub fn centre_x(&self) -> f32 {
        self.x + self.w / 2.0
    }

    /// The same box, `k` times as large in both directions about its centre.
    pub fn grown(&self, k: f32) -> Rect {
        let (w, h) = (self.w * k, self.h * k);
        Rect { x: self.x - (w - self.w) / 2.0, y: self.y - (h - self.h) / 2.0, w, h }
    }
}

fn white(a: f32) -> Hsla {
    hsla(0., 0., 1., a)
}

fn black(a: f32) -> Hsla {
    hsla(0., 0., 0., a)
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// A shadow with an offset and a blur (the element's `shadow_*` helpers are too heavy for a thumb).
fn shadow(y: f32, blur: f32, alpha: f32) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(y), black(alpha)).blur_radius(px(blur))]
}

// ---------------------------------------------------------------- the switch

pub const SW_W: f32 = 46.0;
pub const SW_H: f32 = 20.0;
/// The thumb's gap to the track's edge.
pub const SW_INSET: f32 = 1.0;
/// The thumb is a capsule: this much of the track's width and height.
pub const SW_THUMB_W: f32 = 0.58;
pub const SW_THUMB_H: f32 = 0.9;
/// While the pointer is held on it the thumb is this many times as large.
pub const SW_HELD: f32 = 1.3;
/// The thumb's slide from one end to the other.
pub const SW_SLIDE: Duration = Duration::from_millis(180);
/// A switch that was not built for this long was not on the screen (the page was closed): what it shows when it comes back is where it
/// starts, it does not slide to a value that changed while it was away. (The windows build their whole tree for every frame they draw,
/// so a switch that is on the screen is built at least when the pointer goes down on it.)
pub const SW_STALE: Duration = Duration::from_secs(2);

/// The thumb in the track's own coordinates. `progress` is 0 at the off end and 1 at the on end; `held`: the pointer is down on the
/// switch, so the thumb has grown (about its centre: it overhangs the track).
pub fn switch_thumb(progress: f32, held: bool) -> Rect {
    let (w, h) = (SW_W * SW_THUMB_W, SW_H * SW_THUMB_H);
    let travel = SW_W - 2.0 * SW_INSET - w;
    let rest = Rect { x: SW_INSET + progress.clamp(0.0, 1.0) * travel, y: (SW_H - h) / 2.0, w, h };
    if held { rest.grown(SW_HELD) } else { rest }
}

/// Where the thumb is `elapsed` after the switch flipped, having been at `from` (0..=1) then, on its way to the end `on` says. An
/// ease-out over [`SW_SLIDE`]; with animations off it is at the end at once.
pub fn slide_progress(from: f32, on: bool, elapsed: Duration, animate: bool) -> f32 {
    let to = if on { 1.0 } else { 0.0 };
    if !animate || elapsed >= SW_SLIDE {
        return to;
    }
    from + (to - from) * ease_out(elapsed.as_secs_f32() / SW_SLIDE.as_secs_f32())
}

/// What a switch remembers between two builds of the page: which way it is set and where its thumb was when it last flipped. (The
/// page is rebuilt from the setting on every draw, so the slide has to live outside it.)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitchMotion {
    on: bool,
    from: f32,
    /// When it last flipped; `None`: it has not moved since it was first seen.
    since: Option<Instant>,
    /// When it was last built (see [`SW_STALE`]).
    seen: Instant,
}

impl SwitchMotion {
    /// A switch seen for the first time: at its end, no slide (a page that opens does not animate its switches).
    pub fn new(on: bool, now: Instant) -> SwitchMotion {
        SwitchMotion { on, from: if on { 1.0 } else { 0.0 }, since: None, seen: now }
    }

    pub fn at(&self, now: Instant, animate: bool) -> f32 {
        match self.since {
            None => slide_progress(self.from, self.on, SW_SLIDE, false),
            Some(s) => slide_progress(self.from, self.on, now.saturating_duration_since(s), animate),
        }
    }

    /// Whether the thumb is still on its way (a frame has to be asked for).
    pub fn running(&self, now: Instant, animate: bool) -> bool {
        animate && self.since.is_some_and(|s| now.saturating_duration_since(s) < SW_SLIDE)
    }

    /// Takes what the setting says now. When it flipped, a new slide starts from wherever the thumb is (a click in mid-slide turns it
    /// round without a jump). Returns the thumb's progress.
    pub fn see(&mut self, on: bool, now: Instant, animate: bool) -> f32 {
        if now.saturating_duration_since(self.seen) > SW_STALE {
            *self = SwitchMotion::new(on, now);
        }
        self.seen = now;
        if on != self.on {
            self.from = self.at(now, animate);
            self.on = on;
            self.since = Some(now);
        }
        self.at(now, animate)
    }
}

/// The track: ON is the app's accent, OFF a wash of white (dark theme) or black (light theme) at 16 %. A disabled switch has the track
/// at half strength.
pub fn switch_track_fill(on: bool, dark: bool, disabled: bool, accent: Hsla) -> Hsla {
    let mut c = if on {
        accent
    } else if dark {
        white(0.16)
    } else {
        black(0.16)
    };
    if disabled {
        c.a *= 0.5;
    }
    c
}

/// The thumb: solid white at rest; held, a glass lens (white at 55 %, see the rim); a disabled one light grey.
pub fn switch_thumb_fill(held: bool, disabled: bool) -> Hsla {
    if disabled {
        rgb(0xC8C8C8).into()
    } else if held {
        white(0.55)
    } else {
        white(1.0)
    }
}

/// The rim of a held thumb: 1 px, white at 90 %.
pub const LENS_RIM: f32 = 0.90;

fn switches() -> std::sync::MutexGuard<'static, BTreeMap<String, SwitchMotion>> {
    static SWITCHES: Mutex<BTreeMap<String, SwitchMotion>> = Mutex::new(BTreeMap::new());
    SWITCHES.lock().unwrap_or_else(|p| p.into_inner())
}

/// The track and thumb of a switch, over glass. The caller adds the state word beside it. `toggle` runs on a click; a disabled switch
/// takes none. Keyboard behaviour is the same as the classic switch's (it has none of its own).
pub fn switch_track(id: ElementId, p: &Palette, on: bool, disabled: bool, toggle: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let now = Instant::now();
    let animate = crate::settings::animations();
    let (progress, running) = {
        let mut all = switches();
        let m = all.entry(id.to_string()).or_insert_with(|| SwitchMotion::new(on, now));
        (m.see(on, now, animate), m.running(now, animate))
    };
    let group: SharedString = format!("liquid-switch-{id}").into();
    let (rest, held) = (switch_thumb(progress, false), switch_thumb(progress, true));
    let mut thumb = div()
        .id(ElementId::from((id.clone(), "thumb")))
        .absolute()
        .left(px(rest.x))
        .top(px(rest.y))
        .w(px(rest.w))
        .h(px(rest.h))
        .rounded_full()
        .bg(switch_thumb_fill(false, disabled));
    thumb.style().box_shadow = Some(shadow(1., 2., 0.18));
    if !disabled {
        // while the pointer is down on the switch (the track is the group) the thumb grows into the glass lens; it overhangs the
        // track, so nothing above clips it
        thumb = thumb.group_active(group.clone(), move |mut s| {
            s = s.left(px(held.x)).top(px(held.y)).w(px(held.w)).h(px(held.h)).bg(switch_thumb_fill(true, false)).border_1().border_color(white(LENS_RIM));
            s.box_shadow = Some(shadow(3., 10., 0.28));
            s
        });
    }
    let mut track = div()
        .id(id)
        .group(group)
        // Tab reaches the switch, Enter / Space flips it (as the classic one)
        .tab_index(0)
        .relative()
        .flex_shrink_0()
        .w(px(SW_W))
        .h(px(SW_H))
        .rounded_full()
        .bg(switch_track_fill(on, p.dark, disabled, p.accent))
        // (only while the thumb slides a frame is asked for)
        .child(
            canvas(
                move |_, window, _| {
                    if running {
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(thumb);
    if !disabled {
        track = track.cursor_pointer().on_click(move |_, window, cx| toggle(window, cx));
    }
    track.into_any_element()
}

// ---------------------------------------------------------------- the slider

/// The slider's box (the settings page's: its height with the 1 px focus border).
pub const SL_BOX_H: f32 = 24.0;
pub const SL_BORDER: f32 = 1.0;
pub const SL_TRACK_H: f32 = 4.0;
/// The thumb at rest: a solid white capsule.
pub const SL_REST_W: f32 = 22.0;
pub const SL_REST_H: f32 = 14.0;
/// The thumb while it is dragged or the pointer is down on it: a glass lens.
pub const SL_HELD_W: f32 = 34.0;
pub const SL_HELD_H: f32 = 22.0;

/// The thumb's box relative to its resting box (`SL_REST_W` x `SL_REST_H`, the one the value places). The held thumb grows about the
/// centre, so the centre stays exactly on the value.
pub fn slider_thumb(held: bool) -> Rect {
    let (w, h) = if held { (SL_HELD_W, SL_HELD_H) } else { (SL_REST_W, SL_REST_H) };
    Rect { x: (SL_REST_W - w) / 2.0, y: (SL_REST_H - h) / 2.0, w, h }
}

/// Where the thumb's centre is in a slider `width` px wide for the share `frac` (0..=1) of the range: the centre travels from half a
/// resting thumb inside the left edge to half inside the right one. The pointer mapping (`slider_value_at` with `SL_REST_W`) is the
/// inverse of this, held or not.
#[cfg(test)]
pub fn slider_centre(width: f32, frac: f32) -> f32 {
    SL_REST_W / 2.0 + frac.clamp(0.0, 1.0) * (width - SL_REST_W)
}

/// Where the parts of a slider are, in the slider's inner box (`inner_w` wide: the box less its border). The elements are placed by
/// these numbers and the tests read the same ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderLayout {
    /// The track's left edge: half a resting thumb in.
    pub track_x: f32,
    pub track_w: f32,
    /// The accent part of the track.
    pub fill_w: f32,
    /// The left edge of the thumb's resting box.
    pub thumb_x: f32,
}

impl SliderLayout {
    /// The right end of the accent part, which is where the thumb's centre is.
    #[cfg(test)]
    pub fn fill_end(&self) -> f32 {
        self.track_x + self.fill_w
    }

    /// The thumb's centre, resting or held (it grows about it).
    #[cfg(test)]
    pub fn centre(&self, held: bool) -> f32 {
        self.thumb_x + slider_thumb(held).centre_x()
    }
}

pub fn slider_layout(inner_w: f32, frac: f32) -> SliderLayout {
    let track_w = (inner_w - SL_REST_W).max(0.0);
    let fill_w = frac.clamp(0.0, 1.0) * track_w;
    SliderLayout { track_x: SL_REST_W / 2.0, track_w, fill_w, thumb_x: fill_w }
}

/// The unfilled part of the track: white at 16 % (dark theme) or black at 14 % (light theme).
pub fn slider_track_fill(dark: bool) -> Hsla {
    if dark { white(0.16) } else { black(0.14) }
}

/// The track (4 px, round ends, filled with the accent up to the thumb's centre) and the thumb's box, as two absolutely placed
/// children for the slider's box (`inner_w` wide inside its border). `group` is the name the slider's root carries (`.group(group)`):
/// the thumb grows while the pointer is down on the slider; `dragging` keeps it grown for the length of a drag (the press state is
/// cleared when a drag starts).
pub fn slider_parts(id: usize, group: SharedString, p: &Palette, inner_w: f32, frac: f32, dragging: bool) -> (AnyElement, AnyElement) {
    let inner_h = SL_BOX_H - 2.0 * SL_BORDER;
    let at = slider_layout(inner_w, frac);
    let track = div()
        .absolute()
        .left(px(at.track_x))
        .top(px((inner_h - SL_TRACK_H) / 2.0))
        .w(px(at.track_w))
        .h(px(SL_TRACK_H))
        .rounded_full()
        .bg(slider_track_fill(p.dark))
        .child(div().h_full().w(px(at.fill_w)).rounded_full().bg(p.accent))
        .into_any_element();
    let (rest, held) = (slider_thumb(false), slider_thumb(true));
    let now = if dragging { held } else { rest };
    let mut thumb = div()
        .id(("hg-thumb", id))
        .absolute()
        .left(px(now.x))
        .top(px(now.y))
        .w(px(now.w))
        .h(px(now.h))
        .rounded_full();
    if dragging {
        thumb = thumb.bg(switch_thumb_fill(true, false)).border_1().border_color(white(LENS_RIM));
        thumb.style().box_shadow = Some(shadow(3., 10., 0.28));
    } else {
        thumb = thumb.bg(switch_thumb_fill(false, false)).group_active(group, move |mut s| {
            s = s.left(px(held.x)).top(px(held.y)).w(px(held.w)).h(px(held.h)).bg(switch_thumb_fill(true, false)).border_1().border_color(white(LENS_RIM));
            s.box_shadow = Some(shadow(3., 10., 0.28));
            s
        });
        thumb.style().box_shadow = Some(shadow(1., 2., 0.18));
    }
    // the resting box sits at the value; the thumb in it grows about its centre
    let lane = div().absolute().left(px(at.thumb_x)).top(px((inner_h - SL_REST_H) / 2.0)).w(px(SL_REST_W)).h(px(SL_REST_H)).child(thumb).into_any_element();
    (track, lane)
}

// ---------------------------------------------------------------- the dropdown button and the rows of menus and lists

/// The closed dropdown button: a capsule, white at 10 % (dark) or black at 6 % (light), no border; a little stronger under the pointer.
pub fn dropdown_fill(dark: bool, hovered: bool) -> Hsla {
    match (dark, hovered) {
        (true, false) => white(0.10),
        (true, true) => white(0.15),
        (false, false) => black(0.06),
        (false, true) => black(0.10),
    }
}

/// The capsule behind the row under the pointer or the keyboard focus: white at 12 % (dark) or black at 6 % (light), no border.
pub fn highlight_fill(dark: bool) -> Hsla {
    if dark { white(0.12) } else { black(0.06) }
}

/// Measures of a menu or a list of options: the row, its highlight capsule, the panel's padding and the separators.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub row_h: f32,
    /// The highlight capsule's height (0 where there is no capsule).
    pub hl_h: f32,
    /// The panel's padding at the sides: the capsule is inset by this much from the panel's edge.
    pub pad_x: f32,
    /// The panel's padding above and below the rows.
    pub pad_y: f32,
    pub sep_h: f32,
    /// A separator's inset from the row area (the panel's own padding comes on top: 8 + 10 = 18 from the panel's edge).
    pub sep_inset: f32,
}

pub fn metrics(liquid: bool) -> Metrics {
    if liquid {
        Metrics { row_h: 34.0, hl_h: 29.0, pad_x: 8.0, pad_y: 6.0, sep_h: 9.0, sep_inset: 10.0 }
    } else {
        Metrics { row_h: 32.0, hl_h: 0.0, pad_x: 4.0, pad_y: 4.0, sep_h: 9.0, sep_inset: 0.0 }
    }
}

/// The top of the highlight capsule for a row whose top is `row_top`: centred in the row.
pub fn highlight_top(row_top: f32, m: &Metrics) -> f32 {
    row_top + (m.row_h - m.hl_h) / 2.0
}

/// The column the tick lives in, at the leading edge, and the label's indent for the rows that have no tick.
pub const TICK_COL: f32 = 22.0;

/// Whether a level of a menu (or a list of options) reserves the leading tick column: in the liquid look when any row is ticked (a
/// list with no ticked row has no column to line up). The classic look ticks at the trailing edge or in the icon's place.
pub fn has_tick_column(liquid: bool, any_checked: bool) -> bool {
    liquid && any_checked
}

/// What a row has in the leading column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickSlot {
    /// The level has no column.
    None,
    /// The column, empty: the label lines up with the ticked rows.
    Blank,
    /// The tick.
    Tick,
}

pub fn tick_slot(column: bool, checked: bool) -> TickSlot {
    match (column, checked) {
        (false, _) => TickSlot::None,
        (true, false) => TickSlot::Blank,
        (true, true) => TickSlot::Tick,
    }
}

/// The leading tick column as an element (empty when the row has no tick).
pub fn tick_cell(slot: TickSlot, color: Hsla) -> Option<AnyElement> {
    use gpui_kit::assets::IconName as L;
    use gpui_kit::component::Icon;
    use gpui_kit::prelude::FluentBuilder as _;
    match slot {
        TickSlot::None => None,
        s => Some(div().w(px(TICK_COL)).flex_shrink_0().flex().items_center().when(s == TickSlot::Tick, |d| d.child(Icon::new(L::Check).size(px(14.)).text_color(color))).into_any_element()),
    }
}

/// Whether a row of a scrolling list lies wholly inside its viewport (all four in window px): when it does not, moving the highlight to
/// it scrolls the list, and the capsule must not glide (the list moves under it at once: it would slide in from a row away).
pub fn row_in_view(view_top: f32, view_bottom: f32, row_top: f32, row_bottom: f32) -> bool {
    row_top >= view_top - 0.5 && row_bottom <= view_bottom + 0.5
}

/// How long the highlight capsule takes to glide to another row.
pub const GLIDE: Duration = Duration::from_millis(120);

/// Where the capsule is `elapsed` after it was sent from `from` to `to` (px, in the list's own coordinates): an ease-out over
/// [`GLIDE`]; with animations off it is there at once.
pub fn glide_pos(from: f32, to: f32, elapsed: Duration, animate: bool) -> f32 {
    if !animate || elapsed >= GLIDE {
        return to;
    }
    from + (to - from) * ease_out(elapsed.as_secs_f32() / GLIDE.as_secs_f32())
}

/// One highlight capsule that moves (not a fade per row). Aimed at a place on every draw; a new place starts a glide from wherever the
/// capsule is at that moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    from: f32,
    to: f32,
    since: Option<Instant>,
}

impl Glide {
    /// The capsule at rest at `to` (its first sight: it appears there, it does not glide in).
    pub fn at(to: f32) -> Glide {
        Glide { from: to, to, since: None }
    }

    pub fn pos(&self, now: Instant, animate: bool) -> f32 {
        match self.since {
            None => self.to,
            Some(s) => glide_pos(self.from, self.to, now.saturating_duration_since(s), animate),
        }
    }

    /// Still on its way (a frame has to be asked for).
    pub fn running(&self, now: Instant, animate: bool) -> bool {
        animate && self.since.is_some_and(|s| now.saturating_duration_since(s) < GLIDE)
    }

    /// Aims the capsule at `to`; returns where to draw it now.
    pub fn aim(&mut self, to: f32, now: Instant, animate: bool) -> f32 {
        if (to - self.to).abs() > 0.01 {
            self.from = self.pos(now, animate);
            self.to = to;
            self.since = Some(now);
        }
        self.pos(now, animate)
    }

    /// Puts it at `to` without a glide (the list changed under it: a filter, a new menu).
    pub fn jump(&mut self, to: f32) {
        *self = Glide::at(to);
    }
}

/// The capsule itself, `top` px from the top of the list's content, as wide as the content.
pub fn highlight_capsule(top: f32, m: &Metrics, dark: bool) -> AnyElement {
    div().absolute().left_0().right_0().top(px(top)).h(px(m.hl_h)).rounded_full().bg(highlight_fill(dark)).into_any_element()
}

#[cfg(test)]
mod tests {
    // (named imports: `super::*` would bring gpui's own `test` attribute along and shadow the standard one)
    use super::{
        Glide, LENS_RIM, SL_BOX_H, SL_REST_H, SL_REST_W, SL_TRACK_H, SW_H, SW_STALE, SW_W, SwitchMotion, TickSlot, black, dropdown_fill, glide_pos, has_tick_column, highlight_fill,
        highlight_top, metrics, row_in_view, slide_progress, slider_centre, slider_layout, slider_thumb, slider_track_fill, switch_thumb, switch_thumb_fill, switch_track_fill, tick_cell, tick_slot, white,
    };
    use gpui_kit::{Rgba, hsla};
    use std::time::{Duration, Instant};

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn the_switch_thumb_is_a_capsule_inside_a_2_3_to_1_track() {
        assert!(near(SW_W / SW_H, 2.3));
        let (off, on) = (switch_thumb(0.0, false), switch_thumb(1.0, false));
        assert!(near(off.w, 46.0 * 0.58) && near(off.h, 20.0 * 0.9), "0.58 of the width, 0.9 of the height: {off:?}");
        assert!(off.w > off.h, "a capsule, not a circle");
        // inset 1 px at both ends and above and below
        assert!(near(off.x, 1.0) && near(off.y, 1.0));
        assert!(near(on.x + on.w, 46.0 - 1.0) && near(on.y + on.h, 20.0 - 1.0));
        // in between it is in between, and the progress is kept in range
        let mid = switch_thumb(0.5, false);
        assert!(near(mid.x, (off.x + on.x) / 2.0));
        assert_eq!(switch_thumb(-3.0, false), off);
        assert_eq!(switch_thumb(7.0, false), on);
    }

    #[test]
    fn a_held_switch_thumb_is_1_3_times_as_large_about_the_same_centre() {
        for p in [0.0, 0.4, 1.0] {
            let (r, h) = (switch_thumb(p, false), switch_thumb(p, true));
            assert!(near(h.w, r.w * 1.3) && near(h.h, r.h * 1.3));
            assert!(near(h.centre_x(), r.centre_x()), "it grows where it is");
            assert!(near(h.y + h.h / 2.0, r.y + r.h / 2.0));
        }
        // at the ends and in height it overhangs the track: nothing may clip it, so the track carries no clip
        let h = switch_thumb(1.0, true);
        assert!(h.x + h.w > SW_W && h.y < 0.0 && h.y + h.h > SW_H);
    }

    #[test]
    fn the_thumb_slides_over_180_ms_with_an_ease_out() {
        let ms = Duration::from_millis;
        // to the on end from the off end
        assert_eq!(slide_progress(0.0, true, ms(0), true), 0.0);
        assert_eq!(slide_progress(0.0, true, ms(180), true), 1.0);
        assert_eq!(slide_progress(0.0, true, ms(900), true), 1.0);
        let half = slide_progress(0.0, true, ms(90), true);
        assert!(half > 0.5 && half < 1.0, "an ease-out is past the half way at half the time: {half}");
        let mut last = 0.0;
        for t in (0..=180).step_by(10) {
            let v = slide_progress(0.0, true, ms(t), true);
            assert!(v >= last, "it only goes forward");
            last = v;
        }
        // the way back
        assert_eq!(slide_progress(1.0, false, ms(0), true), 1.0);
        assert_eq!(slide_progress(1.0, false, ms(180), true), 0.0);
        // animations off: at the end at once
        assert_eq!(slide_progress(0.0, true, ms(0), false), 1.0);
        assert_eq!(slide_progress(1.0, false, ms(5), false), 0.0);
    }

    #[test]
    fn a_switch_that_flips_slides_and_one_that_is_first_seen_does_not() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut m = SwitchMotion::new(false, t0);
        assert_eq!(m.see(false, t0, true), 0.0);
        assert!(!m.running(t0, true), "a page that opens does not animate");
        // it is switched on
        assert_eq!(m.see(true, t0 + ms(1000), true), 0.0, "the slide starts where the thumb was");
        assert!(m.running(t0 + ms(1010), true));
        let mid = m.see(true, t0 + ms(1090), true);
        assert!(mid > 0.0 && mid < 1.0);
        assert_eq!(m.see(true, t0 + ms(1180), true), 1.0);
        assert!(!m.running(t0 + ms(1180), true), "no more frames once it is there");
        // switched off again in the middle of the slide: it turns round from where it is, without a jump
        let mut m = SwitchMotion::new(false, t0);
        m.see(true, t0, true);
        let at = m.see(true, t0 + ms(60), true);
        let back = m.see(false, t0 + ms(60), true);
        assert!(near(at, back), "no jump when it turns round: {at} {back}");
        assert_eq!(m.see(false, t0 + ms(60) + ms(180), true), 0.0);
        // animations off: it jumps, and nothing is running
        let mut m = SwitchMotion::new(false, t0);
        assert_eq!(m.see(true, t0, false), 1.0);
        assert!(!m.running(t0, false));
    }

    #[test]
    fn a_switch_that_was_away_comes_back_where_the_setting_is_without_sliding() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut m = SwitchMotion::new(false, t0);
        m.see(false, t0, true);
        // the page was closed; the setting changed meanwhile; it opens again
        let back = t0 + SW_STALE + ms(1);
        assert_eq!(m.see(true, back, true), 1.0, "it starts at its end");
        assert!(!m.running(back, true), "and nothing slides");
        // a switch that is built all the time slides when it flips, however long it rested
        let mut m = SwitchMotion::new(false, t0);
        let mut t = t0;
        for _ in 0..10 {
            t += SW_STALE - ms(1);
            m.see(false, t, true);
        }
        assert_eq!(m.see(true, t + ms(5), true), 0.0, "a click after a long rest still starts a slide");
        assert!(m.running(t + ms(10), true));
    }

    #[test]
    fn the_switch_colours_follow_the_sheet() {
        let accent = hsla(0.6, 0.8, 0.6, 1.0);
        assert_eq!(switch_track_fill(true, true, false, accent), accent, "ON is the app's accent, in both themes");
        assert_eq!(switch_track_fill(true, false, false, accent), accent);
        assert_eq!(switch_track_fill(false, true, false, accent), white(0.16));
        assert_eq!(switch_track_fill(false, false, false, accent), black(0.16));
        // disabled: the track at half strength
        assert!(near(switch_track_fill(true, true, true, accent).a, 0.5));
        assert!(near(switch_track_fill(false, true, true, accent).a, 0.08));
        // the thumb: solid white, then the glass lens, and light grey when disabled
        assert_eq!(switch_thumb_fill(false, false), white(1.0));
        assert_eq!(switch_thumb_fill(true, false), white(0.55));
        let grey = Rgba::from(switch_thumb_fill(false, true));
        assert!(near(grey.r, 200.0 / 255.0) && grey.a == 1.0);
        assert_eq!(switch_thumb_fill(true, true), switch_thumb_fill(false, true), "a disabled thumb does not turn to glass");
        assert!(near(LENS_RIM, 0.9));
    }

    #[test]
    fn the_slider_thumb_is_a_22_by_14_capsule_that_grows_to_34_by_22_about_the_same_centre() {
        let (r, h) = (slider_thumb(false), slider_thumb(true));
        assert_eq!((r.w, r.h), (22.0, 14.0));
        assert_eq!((h.w, h.h), (34.0, 22.0));
        assert_eq!((r.x, r.y), (0.0, 0.0), "the resting thumb is its resting box");
        assert!(near(r.centre_x(), SL_REST_W / 2.0) && near(h.centre_x(), SL_REST_W / 2.0), "the centre does not move when it grows");
        assert!(near(h.y + h.h / 2.0, SL_REST_H / 2.0));
        // the held thumb does not fit the 24 px box less its border by more than a couple of px: it is centred, so it overhangs evenly
        assert!(h.h <= SL_BOX_H);
    }

    #[test]
    fn the_slider_thumbs_centre_is_exactly_on_the_value_held_or_not() {
        // the elements are placed by `slider_layout`; the thumb's centre must be where the accent part of the track ends
        let inner = 218.0;
        for v in [0u8, 1, 37, 50, 99, 100] {
            let frac = f32::from(v) / 100.0;
            let at = slider_layout(inner, frac);
            for held in [false, true] {
                assert!(near(at.centre(held), at.fill_end()), "v={v} held={held}");
            }
        }
        let (first, last) = (slider_layout(inner, 0.0), slider_layout(inner, 1.0));
        assert!(near(first.centre(false), 11.0) && near(last.centre(false), inner - 11.0), "half a resting thumb in from both ends");
        assert!(near(first.thumb_x, 0.0) && near(last.thumb_x + SL_REST_W, inner), "the resting box never leaves the inner box");
        assert!(near(last.track_x + last.track_w + SL_REST_W / 2.0, inner), "the track ends where the thumb's centre can reach");
        assert_eq!(slider_layout(inner, 5.0), last, "a share out of range is cut");
        assert_eq!(slider_layout(inner, -1.0), first);
        assert!(near(slider_centre(220.0, 0.0), 11.0) && near(slider_centre(220.0, 1.0), 209.0));
        assert_eq!(slider_centre(220.0, 5.0), slider_centre(220.0, 1.0), "a share out of range is cut");
        // the held thumb overhangs the box at the ends (it is not clipped: the slider has no clip), by half its extra width
        assert!(near(slider_thumb(true).x, -6.0));
        // a box narrower than the thumb does not make a negative track
        assert_eq!(slider_layout(10.0, 0.5).track_w, 0.0);
    }

    #[test]
    fn the_slider_track_is_a_wash_that_follows_the_theme() {
        assert_eq!(slider_track_fill(true), white(0.16));
        assert_eq!(slider_track_fill(false), black(0.14));
        assert_eq!(SL_TRACK_H, 4.0);
    }

    #[test]
    fn the_dropdown_and_the_highlight_are_washes_of_white_or_black() {
        assert_eq!(dropdown_fill(true, false), white(0.10));
        assert_eq!(dropdown_fill(false, false), black(0.06));
        assert!(dropdown_fill(true, true).a > dropdown_fill(true, false).a && dropdown_fill(false, true).a > dropdown_fill(false, false).a);
        assert_eq!(highlight_fill(true), white(0.12));
        assert_eq!(highlight_fill(false), black(0.06));
    }

    #[test]
    fn rows_are_34_high_with_a_29_capsule_inset_8_in_the_liquid_look_only() {
        let (l, c) = (metrics(true), metrics(false));
        assert_eq!((l.row_h, l.hl_h, l.pad_x, l.pad_y), (34.0, 29.0, 8.0, 6.0));
        assert_eq!(l.sep_inset + l.pad_x, 18.0, "hairlines are 18 px in from the panel's edge");
        // the classic menu is what it always was
        assert_eq!((c.row_h, c.pad_x, c.pad_y, c.sep_h, c.sep_inset), (32.0, 4.0, 4.0, 9.0, 0.0));
        // the capsule is centred in its row
        assert_eq!(highlight_top(68.0, &l), 68.0 + 2.5);
        assert!(near(highlight_top(0.0, &l) + l.hl_h / 2.0, l.row_h / 2.0));
    }

    #[test]
    fn the_tick_moves_to_the_leading_edge_and_the_other_rows_make_room() {
        // classic: never a column
        assert!(!has_tick_column(false, true) && !has_tick_column(false, false));
        // liquid: a column when some row is ticked, none when none is
        assert!(has_tick_column(true, true));
        assert!(!has_tick_column(true, false));
        assert_eq!(tick_slot(true, true), TickSlot::Tick);
        assert_eq!(tick_slot(true, false), TickSlot::Blank, "the same column, so the labels line up");
        assert_eq!(tick_slot(false, false), TickSlot::None);
        assert_eq!(tick_slot(false, true), TickSlot::None, "no column, no tick here");
        assert!(tick_cell(TickSlot::None, white(1.0)).is_none());
    }

    #[test]
    fn a_row_partly_out_of_the_list_means_the_list_will_scroll() {
        // viewport 100..400 (a 300 px list)
        assert!(row_in_view(100.0, 400.0, 100.0, 134.0));
        assert!(row_in_view(100.0, 400.0, 366.0, 400.0), "touching the edge is inside");
        assert!(!row_in_view(100.0, 400.0, 380.0, 414.0), "cut at the bottom: the list scrolls");
        assert!(!row_in_view(100.0, 400.0, 80.0, 114.0), "cut at the top");
        assert!(!row_in_view(100.0, 400.0, 500.0, 534.0), "far below");
    }

    #[test]
    fn the_highlight_glides_over_120_ms_to_the_row_it_is_sent_to() {
        let ms = Duration::from_millis;
        assert_eq!(glide_pos(0.0, 100.0, ms(0), true), 0.0);
        assert_eq!(glide_pos(0.0, 100.0, ms(120), true), 100.0);
        assert_eq!(glide_pos(0.0, 100.0, ms(500), true), 100.0);
        let half = glide_pos(0.0, 100.0, ms(60), true);
        assert!(half > 50.0 && half < 100.0, "{half}");
        // upwards too
        assert!(glide_pos(100.0, 0.0, ms(60), true) < 50.0);
        // animations off: it is there at once
        assert_eq!(glide_pos(0.0, 100.0, ms(0), false), 100.0);
        assert_eq!(glide_pos(100.0, 3.0, ms(10), false), 3.0);
    }

    #[test]
    fn one_capsule_moves_and_a_new_aim_starts_from_where_it_is() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut g = Glide::at(2.5);
        assert_eq!(g.pos(t0, true), 2.5);
        assert!(!g.running(t0, true), "nothing to draw while it rests");
        // the same aim again: nothing happens
        assert_eq!(g.aim(2.5, t0, true), 2.5);
        assert!(!g.running(t0, true));
        // another row: the glide starts at the old place
        assert_eq!(g.aim(36.5, t0, true), 2.5);
        assert!(g.running(t0 + ms(10), true));
        let mid = g.aim(36.5, t0 + ms(60), true);
        assert!(mid > 2.5 && mid < 36.5);
        assert_eq!(g.aim(36.5, t0 + ms(120), true), 36.5);
        assert!(!g.running(t0 + ms(120), true), "no more frames once it has arrived");
        // sent on in mid-glide: from the place it has reached, not from the old target
        let mut g = Glide::at(0.0);
        g.aim(100.0, t0, true);
        let here = g.pos(t0 + ms(60), true);
        assert!(near(g.aim(200.0, t0 + ms(60), true), here));
        // animations off: it jumps, nothing runs
        let mut g = Glide::at(0.0);
        assert_eq!(g.aim(100.0, t0, false), 100.0);
        assert!(!g.running(t0, false));
        // a list that changed under it: no glide
        g.aim(50.0, t0, true);
        g.jump(7.0);
        assert_eq!((g.pos(t0, true), g.running(t0, true)), (7.0, false));
    }
}
