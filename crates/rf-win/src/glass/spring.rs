//! The motion of a lens: each of its four edges is a damped spring towards its target. Pure arithmetic, no GPU and no window, so it is
//! unit tested and the picture harness runs the very same code to show a slide.
//!
//! WHY EDGES AND NOT A POSITION. A lens that moves from one sidebar entry to another should stretch like a drop: the edge in front
//! of the travel runs ahead, the edge behind lags, and the shape settles to its size when they meet. That falls out of springing the
//! edges separately with two stiffnesses, the leading ones stiffer. A single spring on the centre (and one on the size) cannot do it.
//!
//! WHY THE CLOSED FORM. A spring is advanced with the exact solution of the damped oscillator over the step, not with an Euler step:
//! it is correct for any step size (60 or 240 Hz, or a stall of half a second), so the animation looks the same on every display and
//! can never blow up. Retargeting only changes the target; position and velocity carry over, so the motion stays continuous.

/// Angular frequency (rad/s) of an edge in front of the travel.
pub(super) const OMEGA_LEAD: f32 = 22.0;
/// Angular frequency (rad/s) of an edge behind the travel. (With 14 a trailing edge got within a quarter of a pixel of its target after
/// 0.59 to 0.60 s on a 144 px slide, right on the 0.6 s the design asks for; with 15 it takes 0.55 s, which is as good an "about 14".)
pub(super) const OMEGA_TRAIL: f32 = 15.0;
/// Damping ratio of every edge: under 1, so a very small overshoot (about 1 % of the distance) and a quick settle.
pub(super) const ZETA: f32 = 0.82;
/// A step longer than this is cut: after a stall the animation continues from where it was instead of jumping to the end.
pub(super) const MAX_STEP: f32 = 0.05;
/// An edge is done when it is this close to its target (px) ...
const REST_DISTANCE: f32 = 0.25;
/// ... and moves this slowly (px/s). Then it is put exactly on the target, so a lens at rest is pixel-identical from frame to frame.
const REST_SPEED: f32 = 1.0;
/// A centre that moves less than this (px) along an axis is not a travel along it.
const TRAVEL_EPS: f32 = 0.5;

/// One edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Spring {
    pub pos: f32,
    pub vel: f32,
    pub target: f32,
    pub omega: f32,
}

impl Spring {
    pub(super) fn at(x: f32) -> Spring {
        Spring { pos: x, vel: 0.0, target: x, omega: OMEGA_LEAD }
    }

    /// Advances by `dt` seconds with the exact solution of x'' = -2 zeta w x' - w^2 (x - target).
    pub(super) fn step(&mut self, dt: f32) {
        // clamp would pass a NaN on, and the closed form would then write it into the position for good
        let dt = if dt.is_finite() { dt.clamp(0.0, MAX_STEP) } else { 0.0 };
        if dt == 0.0 {
            return;
        }
        let (w, z) = (self.omega, ZETA);
        let wd = w * (1.0 - z * z).sqrt();
        let e0 = self.pos - self.target;
        let v0 = self.vel;
        let decay = (-z * w * dt).exp();
        let (s, c) = (wd * dt).sin_cos();
        let b = (v0 + z * w * e0) / wd;
        self.pos = self.target + decay * (e0 * c + b * s);
        self.vel = decay * (v0 * c - (w * w * e0 + z * w * v0) / wd * s);
    }

    pub(super) fn rested(&self) -> bool {
        (self.pos - self.target).abs() < REST_DISTANCE && self.vel.abs() < REST_SPEED
    }

    fn snap(&mut self, x: f32) {
        self.pos = x;
        self.vel = 0.0;
        self.target = x;
    }
}

/// The four edges of a rectangle: left, top, right, bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RectSpring {
    pub e: [Spring; 4],
}

fn edges(r: [f32; 4]) -> [f32; 4] {
    [r[0], r[1], r[0] + r[2], r[1] + r[3]]
}

impl RectSpring {
    /// At rest on `r` (x, y, w, h).
    pub(super) fn at(r: [f32; 4]) -> RectSpring {
        RectSpring { e: edges(r).map(Spring::at) }
    }

    /// Jumps onto `r`, no motion left.
    pub(super) fn snap(&mut self, r: [f32; 4]) {
        for (s, x) in self.e.iter_mut().zip(edges(r)) {
            s.snap(x);
        }
    }

    /// The target moves, the motion under way goes on towards it (a scroll or a resize while a slide runs).
    pub(super) fn follow(&mut self, r: [f32; 4]) {
        for (s, x) in self.e.iter_mut().zip(edges(r)) {
            s.target = x;
        }
    }

    /// The selection moved: new target, and the two edges that lead in the direction of travel get the stiffer spring. Position and
    /// velocity are untouched (a slide that is under way turns smoothly). Along an axis the centre does not travel on, both edges are
    /// stiff, so a pure change of size is quick and symmetric.
    pub(super) fn slide_to(&mut self, r: [f32; 4]) {
        let old = self.target_rect();
        let (dx, dy) = ((r[0] + r[2] * 0.5) - (old[0] + old[2] * 0.5), (r[1] + r[3] * 0.5) - (old[1] + old[3] * 0.5));
        let lead = |d: f32| -> (f32, f32) {
            // (omega of the near edge, omega of the far edge) along an axis: the far edge (right / bottom) leads when d > 0
            if d > TRAVEL_EPS {
                (OMEGA_TRAIL, OMEGA_LEAD)
            } else if d < -TRAVEL_EPS {
                (OMEGA_LEAD, OMEGA_TRAIL)
            } else {
                (OMEGA_LEAD, OMEGA_LEAD)
            }
        };
        let (lx, rx) = lead(dx);
        let (ty, by) = lead(dy);
        for ((s, x), w) in self.e.iter_mut().zip(edges(r)).zip([lx, ty, rx, by]) {
            s.target = x;
            s.omega = w;
        }
    }

    pub(super) fn step(&mut self, dt: f32) {
        for s in &mut self.e {
            s.step(dt);
        }
        if self.rested() {
            let t = self.target_rect();
            self.snap(t);
        }
    }

    pub(super) fn rested(&self) -> bool {
        self.e.iter().all(Spring::rested)
    }

    /// Exactly on the target and not moving: nothing to draw again.
    pub(super) fn settled(&self) -> bool {
        self.e.iter().all(|s| s.pos == s.target && s.vel == 0.0)
    }

    /// x, y, w, h now. The size never goes negative, whatever the edges do.
    pub(super) fn rect(&self) -> [f32; 4] {
        let [l, t, r, b] = self.e.map(|s| s.pos);
        [l, t, (r - l).max(0.0), (b - t).max(0.0)]
    }

    pub(super) fn target_rect(&self) -> [f32; 4] {
        let [l, t, r, b] = self.e.map(|s| s.target);
        [l, t, (r - l).max(0.0), (b - t).max(0.0)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FROM: [f32; 4] = [16.0, 120.0, 220.0, 36.0];
    const DOWN: [f32; 4] = [16.0, 264.0, 220.0, 36.0];

    /// Steps a slide at `hz` and returns (time of the position being within 0.25 px, time of the full rest, worst overshoot of any
    /// edge in px beyond its target, in the direction of travel).
    fn run(from: [f32; 4], to: [f32; 4], hz: f32) -> (f32, f32, f32) {
        let mut s = RectSpring::at(from);
        s.slide_to(to);
        let dt = 1.0 / hz;
        let (mut near, mut over) = (f32::NAN, 0.0f32);
        for i in 1..(4.0 * hz) as usize {
            s.step(dt);
            let t = i as f32 * dt;
            for (k, e) in s.e.iter().enumerate() {
                let travel = edges(to)[k] - edges(from)[k];
                over = over.max((e.pos - e.target) * travel.signum() * if travel == 0.0 { 0.0 } else { 1.0 });
            }
            if near.is_nan() && s.e.iter().all(|e| (e.pos - e.target).abs() < REST_DISTANCE) {
                near = t;
            }
            if s.rested() {
                return (near, t, over);
            }
        }
        (near, f32::INFINITY, over)
    }

    #[test]
    fn reaches_the_target_exactly() {
        for hz in [60.0, 120.0, 240.0] {
            let mut s = RectSpring::at(FROM);
            s.slide_to(DOWN);
            for _ in 0..(3.0 * hz) as usize {
                s.step(1.0 / hz);
            }
            assert!(s.rested());
            assert_eq!(s.rect(), DOWN, "a lens at rest sits exactly on its target ({hz} Hz)");
            let before = s.rect();
            s.step(1.0 / hz);
            assert_eq!(s.rect(), before, "and does not move any more");
        }
    }

    #[test]
    fn leading_edge_is_ahead_of_the_trailing_edge() {
        // moving down: the bottom edge leads. Compare the share of the way each edge has gone.
        for (from, to, lead, trail) in [(FROM, DOWN, 3usize, 1usize), (DOWN, FROM, 1, 3)] {
            let mut s = RectSpring::at(from);
            s.slide_to(to);
            let dist = edges(to)[lead] - edges(from)[lead];
            let mut checked = 0;
            for _ in 0..24 {
                s.step(1.0 / 120.0);
                let done = |k: usize| (s.e[k].pos - edges(from)[k]) / dist;
                if done(lead) < 0.9 {
                    assert!(done(lead) > done(trail), "the leading edge must be ahead ({} vs {})", done(lead), done(trail));
                    checked += 1;
                }
            }
            assert!(checked > 5);
        }
        // and the lens is taller than at rest while it travels
        let mut s = RectSpring::at(FROM);
        s.slide_to(DOWN);
        let mut tallest = 0.0f32;
        for _ in 0..60 {
            s.step(1.0 / 120.0);
            tallest = tallest.max(s.rect()[3]);
        }
        assert!(tallest > FROM[3] + 20.0, "the drop stretches (tallest {tallest})");
    }

    #[test]
    fn small_overshoot_and_quick_settle_at_every_rate() {
        for hz in [60.0, 120.0, 240.0] {
            for to in [DOWN, [16.0, 156.0, 220.0, 36.0], [16.0, 120.0 - 144.0, 220.0, 36.0], [100.0, 120.0, 220.0, 36.0], [16.0, 120.0, 180.0, 60.0]] {
                let dist = to.iter().zip(FROM).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
                let (near, rest, over) = run(FROM, to, hz);
                assert!(over < 0.06 * dist, "overshoot {over} px of {dist} px at {hz} Hz");
                // the picture has settled (every edge within a quarter pixel of its target) well inside 0.6 s ...
                assert!(near < 0.6, "within 0.25 px only after {near} s at {hz} Hz for {to:?}");
                // ... and the last creep (under 1 px/s as well) ends soon after, then the lens is snapped and drawing stops
                assert!(rest < 0.8, "at rest only after {rest} s at {hz} Hz for {to:?}");
            }
        }
    }

    #[test]
    fn never_not_a_number_whatever_the_steps() {
        let mut s = RectSpring::at(FROM);
        let mut x = 12345u32;
        for i in 0..5000 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let r = |k: u32| ((x >> k) & 0xff) as f32;
            if i % 7 == 0 {
                s.slide_to([r(0) * 3.0, r(8) * 3.0, 10.0 + r(16), 10.0 + r(24)]);
            }
            // steps from nothing to a minute
            let dt = [0.0, 1e-6, 0.004, 0.0083, 0.016, 0.5, 60.0][(x >> 5) as usize % 7];
            s.step(dt);
            for e in s.e {
                assert!(e.pos.is_finite() && e.vel.is_finite());
            }
            let rect = s.rect();
            assert!(rect.iter().all(|v| v.is_finite()) && rect[2] >= 0.0 && rect[3] >= 0.0);
        }
        // a negative or non-finite step must not poison it either
        s.slide_to(DOWN);
        for bad in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let before = s.e;
            s.step(bad);
            assert!(s.rect().iter().all(|v| v.is_finite()), "step({bad})");
            assert_eq!(s.e, before, "a step that is not a time moves nothing (step({bad}))");
        }
    }

    #[test]
    fn a_stall_does_not_jump_to_the_end() {
        let mut s = RectSpring::at(FROM);
        s.slide_to(DOWN);
        s.step(2.0); // the UI froze for two seconds
        assert!(s.rect()[1] < DOWN[1] - 40.0, "one step is cut to {MAX_STEP} s, the slide goes on afterwards (top edge at {})", s.rect()[1]);
    }

    #[test]
    fn retarget_in_mid_flight_is_continuous() {
        let mut s = RectSpring::at(FROM);
        s.slide_to(DOWN);
        for _ in 0..14 {
            s.step(1.0 / 120.0);
        }
        let (rect, vel) = (s.rect(), s.e.map(|e| e.vel));
        assert!(vel.iter().any(|v| v.abs() > 100.0), "the slide is under way");
        // the selection moves again, back up and further
        s.slide_to([16.0, 60.0, 220.0, 36.0]);
        assert_eq!(s.rect(), rect, "position unchanged by a retarget");
        assert_eq!(s.e.map(|e| e.vel), vel, "velocity unchanged by a retarget");
        // and over the next step nothing jumps: the change in position is what the old velocity says, within the acceleration
        s.step(1.0 / 120.0);
        for (k, e) in s.e.iter().enumerate() {
            let before = [rect[0], rect[1], rect[0] + rect[2], rect[1] + rect[3]][k];
            assert!((e.pos - before).abs() < 12.0, "edge {k} jumped by {} px in one step", (e.pos - before).abs());
        }
        // the same for a target that only follows (a scroll)
        let mut f = RectSpring::at(FROM);
        f.slide_to(DOWN);
        for _ in 0..14 {
            f.step(1.0 / 120.0);
        }
        let (rect, vel) = (f.rect(), f.e.map(|e| e.vel));
        f.follow([16.0, 280.0, 220.0, 36.0]);
        assert_eq!((f.rect(), f.e.map(|e| e.vel)), (rect, vel));
    }

    #[test]
    fn size_change_without_travel_is_stiff_on_both_edges() {
        let mut s = RectSpring::at(FROM);
        s.slide_to([16.0, 108.0, 220.0, 60.0]); // taller by 24 around the same centre
        assert_eq!(s.e[1].omega, OMEGA_LEAD);
        assert_eq!(s.e[3].omega, OMEGA_LEAD);
        // a move down: the bottom (3) leads, the top (1) trails
        s.slide_to(DOWN);
        assert_eq!((s.e[3].omega, s.e[1].omega), (OMEGA_LEAD, OMEGA_TRAIL));
        // a move up: the other way round
        s.snap(DOWN);
        s.slide_to(FROM);
        assert_eq!((s.e[3].omega, s.e[1].omega), (OMEGA_TRAIL, OMEGA_LEAD));
    }

    #[test]
    fn closed_form_matches_a_fine_numerical_integration() {
        let mut a = Spring::at(0.0);
        a.target = 100.0;
        a.omega = OMEGA_TRAIL;
        let mut b = a;
        for _ in 0..30 {
            a.step(1.0 / 60.0);
        }
        // semi-implicit Euler with 1 ms steps
        for _ in 0..500 {
            b.vel += (-2.0 * ZETA * b.omega * b.vel - b.omega * b.omega * (b.pos - b.target)) * 0.001;
            b.pos += b.vel * 0.001;
        }
        assert!((a.pos - b.pos).abs() < 0.5, "{} vs {}", a.pos, b.pos);
        assert!((a.vel - b.vel).abs() < 2.0, "{} vs {}", a.vel, b.vel);
    }
}
