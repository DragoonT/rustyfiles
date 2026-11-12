//! The lenses of one window: up to `MAX_LENSES` slots, each with a key (what the lens is on: the selected entry), its springs, its fade.
//! Pure state, no GPU and no window: the engine thread owns one of these per window, feeds it the app's calls and the time, and asks it
//! what to draw.
//!
//! The rules of `set_lens`, kept per window and slot:
//! - a lens into an empty slot appears: it fades in over `FADE_SECS` and grows from `GROW_FROM` of its size (`animate = false`: it is there);
//! - the same key again: the place changed but not the selection (a scroll, a resize). The lens follows at once: if it rests it is put
//!   on the new place, if a slide is under way only the target moves and the slide goes on towards it;
//! - another key: the selection moved, the lens slides to the new rectangle (`animate = false`: it jumps);
//! - none: the lens fades out over `FADE_SECS` (`animate = false`: gone at once). A lens that is asked for again while it fades out
//!   comes back from where it is (with another key it slides on from there).

use super::spring::{MAX_STEP, RectSpring};
use super::{LensSpec, MAX_LENSES};
use super::gpu::{LensDraw, LensList};

/// Fade in and fade out.
pub(super) const FADE_SECS: f32 = 0.12;
/// A new lens grows from this share of its size.
const GROW_FROM: f32 = 0.92;

struct Slot {
    key: u64,
    rect: RectSpring,
    clip: [f32; 4],
    radius: f32,
    /// 0..1 and the value it is going to; the opacity is the smoothstep of it.
    fade: f32,
    fade_to: f32,
    /// 0..1, only grows; the size is `GROW_FROM` + the rest times its smoothstep.
    grow: f32,
}

pub(super) struct LensSet {
    slots: [Option<Slot>; MAX_LENSES],
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Whether a spec can be drawn at all (a NaN or an empty rectangle is ignored, not drawn).
fn usable(s: &LensSpec) -> bool {
    s.rect.iter().chain(&s.clip).chain([&s.radius]).all(|v| v.is_finite()) && s.rect[2] > 0.0 && s.rect[3] > 0.0 && s.radius >= 0.0
}

impl LensSet {
    pub(super) fn new() -> LensSet {
        LensSet { slots: std::array::from_fn(|_| None) }
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    pub(super) fn set(&mut self, slot: usize, lens: Option<LensSpec>, animate: bool) {
        let Some(place) = self.slots.get_mut(slot) else { return };
        // A spec that cannot be drawn (NaN, no size) is no lens: for an occupied slot that means the lens is gone, not that the old one
        // stays behind at its last place with its last clip.
        match (place.as_mut(), lens.filter(usable)) {
            (None, Some(s)) => {
                let v = if animate { 0.0 } else { 1.0 };
                *place = Some(Slot { key: s.key, rect: RectSpring::at(s.rect), clip: s.clip, radius: s.radius, fade: v, fade_to: 1.0, grow: v });
            }
            (Some(sl), Some(s)) => {
                sl.clip = s.clip;
                sl.radius = s.radius;
                sl.fade_to = 1.0;
                if !animate {
                    sl.fade = 1.0;
                    sl.grow = 1.0;
                }
                if s.key == sl.key {
                    if sl.rect.settled() {
                        sl.rect.snap(s.rect);
                    } else {
                        sl.rect.follow(s.rect);
                    }
                } else {
                    sl.key = s.key;
                    if animate {
                        sl.rect.slide_to(s.rect);
                    } else {
                        sl.rect.snap(s.rect);
                    }
                }
            }
            (Some(sl), None) => {
                if animate {
                    sl.fade_to = 0.0;
                } else {
                    *place = None;
                }
            }
            (None, None) => {}
        }
    }

    /// Whether any slot holds a lens (at rest or not).
    pub(super) fn occupied(&self) -> bool {
        self.slots.iter().any(Option::is_some)
    }

    /// Whether anything is still moving or fading:the engine redraws at the display's rate while it is, and not at all when it is not.
    pub(super) fn animating(&self) -> bool {
        self.slots.iter().flatten().any(|s| !s.rect.settled() || s.fade != s.fade_to || s.grow < 1.0)
    }

    /// Advances by `dt` seconds (cut to `MAX_STEP`).
    pub(super) fn advance(&mut self, dt: f32) {
        let dt = if dt.is_finite() { dt.clamp(0.0, MAX_STEP) } else { 0.0 };
        let d = dt / FADE_SECS;
        for place in &mut self.slots {
            let Some(s) = place else { continue };
            s.rect.step(dt);
            s.fade = if s.fade < s.fade_to { (s.fade + d).min(s.fade_to) } else { (s.fade - d).max(s.fade_to) };
            s.grow = (s.grow + d).min(1.0);
            if s.fade_to == 0.0 && s.fade <= 0.0 {
                *place = None;
            }
        }
    }

    /// What to draw now, in slot order.
    pub(super) fn draw_list(&self) -> LensList {
        LensList::from_draws(self.slots.iter().flatten().map(|s| {
            let [x, y, w, h] = s.rect.rect();
            let k = GROW_FROM + (1.0 - GROW_FROM) * smooth(s.grow);
            let (nw, nh) = (w * k, h * k);
            LensDraw { rect: [x + (w - nw) * 0.5, y + (h - nh) * 0.5, nw, nh], clip: s.clip, radius: s.radius * k, opacity: smooth(s.fade) }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(key: u64, y: f32) -> LensSpec {
        LensSpec { key, rect: [16.0, y, 220.0, 36.0], clip: [0.0, 0.0, 260.0, 600.0], radius: 18.0 }
    }

    fn settle(l: &mut LensSet) -> usize {
        let mut n = 0;
        while l.animating() && n < 1000 {
            l.advance(1.0 / 120.0);
            n += 1;
        }
        n
    }

    #[test]
    fn a_new_lens_fades_in_and_grows_and_then_rests() {
        let mut l = LensSet::new();
        l.set(1, Some(spec(1, 100.0)), true);
        assert!(l.animating());
        let first = l.draw_list();
        assert_eq!(first.n, 0, "at opacity 0 nothing is drawn yet");
        l.advance(1.0 / 120.0);
        l.advance(1.0 / 120.0);
        let early = l.draw_list();
        assert_eq!(early.n, 1);
        assert!(early.l[0].opacity > 0.0 && early.l[0].opacity < 0.2);
        assert!(early.l[0].rect[2] < 220.0 && early.l[0].rect[2] > 220.0 * GROW_FROM - 0.01, "it grows from 92 % of its size");
        let steps = settle(&mut l);
        assert!((12..=16).contains(&steps), "about 120 ms at 120 Hz, took {steps} steps");
        let done = l.draw_list();
        assert_eq!(done.l[0], LensDraw { rect: [16.0, 100.0, 220.0, 36.0], clip: [0.0, 0.0, 260.0, 600.0], radius: 18.0, opacity: 1.0 });
        assert!(!l.animating(), "a lens at rest asks for no redraw");
    }

    #[test]
    fn without_animation_it_is_simply_there_and_gone() {
        let mut l = LensSet::new();
        l.set(0, Some(spec(7, 50.0)), false);
        assert!(!l.animating());
        assert_eq!(l.draw_list().l[0].opacity, 1.0);
        l.set(0, Some(spec(8, 300.0)), false);
        assert!(!l.animating(), "another key without animation jumps");
        assert_eq!(l.draw_list().l[0].rect[1], 300.0);
        l.set(0, None, false);
        assert!(l.is_empty());
        assert_eq!(l.draw_list().n, 0);
    }

    #[test]
    fn another_key_slides_and_the_same_key_follows() {
        let mut l = LensSet::new();
        l.set(0, Some(spec(1, 120.0)), false);
        l.set(0, Some(spec(2, 264.0)), true);
        assert!(l.animating());
        l.advance(1.0 / 60.0);
        l.advance(1.0 / 60.0);
        let mid = l.draw_list().l[0].rect;
        assert!(mid[1] > 120.0 && mid[1] < 264.0, "on its way");
        // the same key with another place while the slide runs: the target moves, the slide goes on (no jump)
        l.set(0, Some(spec(2, 300.0)), true);
        assert_eq!(l.draw_list().l[0].rect, mid);
        settle(&mut l);
        assert_eq!(l.draw_list().l[0].rect, [16.0, 300.0, 220.0, 36.0]);
        // the same key at rest: the lens is on the new place at once (a scroll)
        l.set(0, Some(spec(2, 280.0)), true);
        assert!(!l.animating());
        assert_eq!(l.draw_list().l[0].rect, [16.0, 280.0, 220.0, 36.0]);
    }

    #[test]
    fn none_fades_out_and_a_fading_lens_can_come_back() {
        let mut l = LensSet::new();
        l.set(2, Some(spec(1, 100.0)), false);
        l.set(2, None, true);
        l.advance(0.06);
        let half = l.draw_list().l[0].opacity;
        assert!(half > 0.3 && half < 0.7, "half way out: {half}");
        l.set(2, Some(spec(1, 100.0)), true);
        settle(&mut l);
        assert_eq!(l.draw_list().l[0].opacity, 1.0);
        l.set(2, None, true);
        settle(&mut l);
        assert!(l.is_empty() && !l.animating());
    }

    #[test]
    fn slots_are_independent_and_bad_input_is_ignored() {
        let mut l = LensSet::new();
        l.set(3, Some(spec(1, 10.0)), false);
        l.set(0, Some(spec(2, 60.0)), false);
        l.set(4, Some(spec(3, 60.0)), false); // no such slot
        l.set(1, Some(LensSpec { rect: [f32::NAN, 0.0, 10.0, 10.0], ..spec(4, 0.0) }), false);
        l.set(1, Some(LensSpec { rect: [0.0, 0.0, 0.0, 10.0], ..spec(5, 0.0) }), false);
        let d = l.draw_list();
        assert_eq!(d.n, 2);
        assert_eq!((d.l[0].rect[1], d.l[1].rect[1]), (60.0, 10.0), "in slot order");
        l.set(0, None, false);
        assert_eq!(l.draw_list().n, 1);
        // a huge or non-finite step does not hurt
        l.set(3, Some(spec(9, 500.0)), true);
        l.advance(f32::NAN);
        l.advance(1e9);
        assert!(l.draw_list().l[0].rect.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn an_unusable_spec_for_an_occupied_slot_takes_the_lens_off() {
        let mut l = LensSet::new();
        l.set(0, Some(spec(1, 40.0)), false);
        assert!(l.occupied());
        // a collapsed rectangle is "no lens": it fades out like None, it does not leave a ghost at the last good place
        l.set(0, Some(LensSpec { rect: [16.0, 40.0, 0.0, 36.0], ..spec(1, 40.0) }), true);
        settle(&mut l);
        assert!(l.is_empty() && !l.occupied());
        l.set(0, Some(spec(1, 40.0)), false);
        l.set(0, Some(LensSpec { radius: f32::NAN, ..spec(1, 40.0) }), false);
        assert!(l.is_empty(), "without animation it is gone at once");
    }
}
