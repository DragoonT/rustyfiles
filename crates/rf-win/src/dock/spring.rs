//! The spring the dock's motions move with. Pure arithmetic: no clock of its own, no window.
//!
//! WHAT A SPRING IS HERE. A weight on a damped spring, pulled towards a target: `x'' = -2 z w x' - w^2 (x - target)`. It starts
//! gently (it has to pick up speed), runs fast in the middle and comes to rest softly, and, which no fixed curve can do, a motion
//! that is turned around in mid flight keeps its speed: it slows, turns and goes back, instead of starting again from a standstill.
//!
//! THE TWO NUMBERS are the ones Apple's animation interfaces describe a spring with (SwiftUI's `Spring(duration:bounce:)`, as
//! documented and shown at WWDC 2023; the arithmetic is the damped oscillator's, nobody's code): `duration`, the time the motion
//! is felt to take (one period of the spring without damping: `w = 2 pi / duration`), and `bounce`, how far it swings past its
//! target (`z = 1 - bounce`: 0 is the critically damped spring, which never swings past, their "smooth"; 0.15 their "snappy").
//! A spring with no bounce is within 1.4 % of its target after `duration` and within 0.1 % after about one and a half.
//!
//! TWO WAYS TO USE IT:
//! * [`Spring::at`]: where a spring that was a given distance from its target with a given speed is after a time. Exact (the closed
//!   form, not a step by step sum), so it is the same on a 60 Hz and a 240 Hz display and after a stall. `magnify::Presence` keeps a
//!   distance, a speed and a start time and asks this; turning around is a new start with the speed of that moment.
//! * [`curve`]: the spring's way from 0 to 1 as a curve over a FIXED time, for the motions that are a clock from 0 to 1 and a curve
//!   (the Start panel's growth, a panel's opening, an icon's slide). It is the spring released at rest, cut where it is within a
//!   third of a percent of its target and stretched to end exactly on 1.

use std::f32::consts::TAU;

/// The most bounce a spring may have here (a spring that hardly comes to rest is a toy, not an interface).
pub const MAX_BOUNCE: f32 = 0.6;

/// A spring: how fast (`omega`, rad per ms) and how damped (`zeta`: 1 critically, less swings past the target).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    omega: f32,
    zeta: f32,
}

impl Spring {
    /// `duration_ms`: the time the motion is felt to take (at least 1); `bounce`: 0 (none) to [`MAX_BOUNCE`].
    pub fn new(duration_ms: f32, bounce: f32) -> Spring {
        let d = if duration_ms.is_finite() { duration_ms.max(1.0) } else { 1.0 };
        let b = if bounce.is_finite() { bounce.clamp(0.0, MAX_BOUNCE) } else { 0.0 };
        Spring { omega: TAU / d, zeta: 1.0 - b }
    }

    /// No bounce: it never swings past its target.
    pub fn smooth(duration_ms: f32) -> Spring {
        Spring::new(duration_ms, 0.0)
    }

    /// Where the spring is `t_ms` after it was `e0` away from its target (`x - target`) and moving at `v0` (per ms): the distance
    /// left and the speed. Exact for any `t_ms`; a time that is not a number, or before the start, is the start.
    pub fn at(&self, e0: f32, v0: f32, t_ms: f32) -> (f32, f32) {
        let t = if t_ms.is_finite() { t_ms.max(0.0) } else { 0.0 };
        let (w, z) = (self.omega, self.zeta);
        if z >= 1.0 {
            // critically damped: (e0 + c t) e^(-w t)
            let c = v0 + w * e0;
            let decay = (-w * t).exp();
            return ((e0 + c * t) * decay, (v0 - w * c * t) * decay);
        }
        let wd = w * (1.0 - z * z).sqrt();
        let decay = (-z * w * t).exp();
        let (s, c) = (wd * t).sin_cos();
        let b = (v0 + z * w * e0) / wd;
        (decay * (e0 * c + b * s), decay * (v0 * c - (w * w * e0 + z * w * v0) / wd * s))
    }

    /// A time (ms) after which the spring stays within `eps` of its target, from the size its swing can still have: never too
    /// early, at most a little late. 0 when it is there already and at rest.
    pub fn settle_ms(&self, e0: f32, v0: f32, eps: f32) -> f32 {
        let eps = eps.abs().max(1e-6);
        let (w, z) = (self.omega, self.zeta);
        if z >= 1.0 {
            // |(e0 + c t) e^(-w t)| <= (|e0| + |c| t) e^(-w t): walked in steps of a fifth of 1/w until it is under eps and falling
            let c = (v0 + w * e0).abs();
            let a = e0.abs();
            if a < eps && c * (1.0 / w) * (-1.0f32).exp() < eps {
                return 0.0;
            }
            let step = 0.2 / w;
            let mut t = 0.0f32;
            for _ in 0..400 {
                t += step;
                // (past t = 1/w the bound only falls)
                if t * w >= 1.0 && (a + c * t) * (-w * t).exp() < eps {
                    break;
                }
            }
            return t;
        }
        let wd = w * (1.0 - z * z).sqrt();
        let amp = (e0 * e0 + ((v0 + z * w * e0) / wd).powi(2)).sqrt();
        if amp <= eps { 0.0 } else { (amp / eps).ln() / (z * w) }
    }
}

/// How much of the spring's way [`curve`] holds: the spring released at rest is cut at `w t = CURVE_REACH` (a spring without
/// bounce is 99.66 % there) and the rest is stretched away. It makes the curve as quick as the cubic ease-out it took the place of
/// (half the time is 90 % of the way) with a start that picks up speed instead of leaving at full speed.
pub const CURVE_REACH: f32 = 7.854;

/// The spring's way from 0 to 1 over a fixed time: `x` 0 to 1 is the time, the answer the place (0 at 0, exactly 1 at 1, and for
/// `bounce` 0 never above 1 and never falling). With bounce it swings past 1 on the way and comes back.
pub fn curve_with(x: f32, bounce: f32) -> f32 {
    if !x.is_finite() || x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    // (a spring of period 1 ms, asked at w t = CURVE_REACH x)
    let s = Spring::new(1.0, bounce);
    let reach = CURVE_REACH / s.omega;
    let at = |x: f32| 1.0 + s.at(-1.0, 0.0, reach * x).0;
    at(x) / at(1.0)
}

/// [`curve_with`] no bounce: what the dock's fixed-time motions use.
pub fn curve(x: f32) -> f32 {
    curve_with(x, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spring_without_bounce_arrives_without_swinging_past_and_is_nearly_there_after_its_duration() {
        let s = Spring::smooth(240.0);
        // released one unit away, at rest: the distance only shrinks, the sign never changes
        let mut last = 1.0f32;
        for t in 0..=1000 {
            let (e, _) = s.at(1.0, 0.0, t as f32);
            assert!(e <= last + 1e-6 && e >= 0.0, "at {t}: {e}");
            last = e;
        }
        assert_eq!(s.at(1.0, 0.0, 0.0), (1.0, 0.0));
        let (half, _) = s.at(1.0, 0.0, 64.0);
        assert!((half - 0.5).abs() < 0.02, "half way after about a quarter of the duration: {half}");
        assert!(s.at(1.0, 0.0, 240.0).0 < 0.015, "within 1.4 % after the duration");
        assert!(s.at(1.0, 0.0, 360.0).0 < 0.001, "within 0.1 % after one and a half");
        // it starts gently: after a tenth of the duration it has gone less than a fifth of the way
        assert!(s.at(1.0, 0.0, 24.0).0 > 0.8);
    }

    #[test]
    fn the_closed_form_is_the_same_whatever_the_steps_are() {
        // one step of 300 ms, or 300 steps of 1 ms each started from where the last one ended: the same place and the same speed
        for bounce in [0.0f32, 0.15, 0.3] {
            let s = Spring::new(240.0, bounce);
            let whole = s.at(1.0, 0.0, 300.0);
            let (mut e, mut v) = (1.0f32, 0.0f32);
            for _ in 0..300 {
                (e, v) = s.at(e, v, 1.0);
            }
            assert!((e - whole.0).abs() < 2e-4 && (v - whole.1).abs() < 2e-5, "bounce {bounce}: {e} {v} vs {whole:?}");
        }
    }

    #[test]
    fn a_spring_turned_around_in_mid_flight_keeps_its_speed_and_does_not_jump() {
        let s = Spring::smooth(240.0);
        // on its way from 0 to 1 (distance -1 from the target 1), turned back towards 0 at 60 ms
        let (e, v) = s.at(-1.0, 0.0, 60.0);
        let x = 1.0 + e;
        assert!(v > 0.0 && x > 0.3 && x < 0.7);
        // the new motion starts where it was, with the speed it had: it still rises for a moment, then comes back
        let (x1, v1) = s.at(x, v, 1.0);
        assert!((x1 - x).abs() < 0.02, "no jump");
        assert!(v1 > 0.0 && v1 < v, "still going up, already slowing");
        let peak = (0..200).map(|t| s.at(x, v, t as f32).0).fold(x, f32::max);
        assert!(peak > x && peak < 1.0, "it turns before it would have arrived: {peak}");
        assert!(s.at(x, v, 600.0).0.abs() < 1e-3);
        // and it never goes below its target on the way back
        assert!((0..800).all(|t| s.at(x, v, t as f32).0 > -1e-6));
    }

    #[test]
    fn bounce_swings_past_the_target_by_what_the_damping_says_and_settles() {
        // the first swing past is e^(-pi z / sqrt(1 - z^2)) of the way
        for (bounce, past) in [(0.15f32, 0.0063f32), (0.3, 0.046)] {
            let s = Spring::new(300.0, bounce);
            let lowest = (0..2000).map(|t| s.at(1.0, 0.0, t as f32).0).fold(1.0, f32::min);
            assert!((-lowest - past).abs() < 0.003, "bounce {bounce}: {lowest}");
            assert!(s.at(1.0, 0.0, 2000.0).0.abs() < 1e-4);
        }
        // bounce is kept in its range, and nonsense is a plain spring
        assert_eq!(Spring::new(300.0, 5.0), Spring::new(300.0, MAX_BOUNCE));
        assert_eq!(Spring::new(300.0, -1.0), Spring::smooth(300.0));
        assert_eq!(Spring::new(f32::NAN, f32::NAN), Spring::smooth(1.0));
    }

    #[test]
    fn the_settle_time_is_never_too_early() {
        for bounce in [0.0f32, 0.15, 0.4] {
            let s = Spring::new(240.0, bounce);
            for (e0, v0) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.5, 0.004), (0.2, -0.01), (0.0, 0.01), (-0.7, 0.008)] {
                let t = s.settle_ms(e0, v0, 0.001);
                assert!(t.is_finite() && t >= 0.0 && t < 2_000.0, "{t}");
                for later in [0.0f32, 1.0, 10.0, 50.0, 200.0, 1_000.0] {
                    assert!(s.at(e0, v0, t + later).0.abs() <= 0.001 + 1e-6, "bounce {bounce}, from {e0} at {v0}: {} at +{later}", s.at(e0, v0, t + later).0);
                }
            }
            assert_eq!(s.settle_ms(0.0, 0.0, 0.001), 0.0, "at rest on the target: nothing to wait for");
        }
        // a smooth spring of 240 ms released one unit away is done (to a thousandth) in under 400 ms
        assert!(Spring::smooth(240.0).settle_ms(1.0, 0.0, 0.001) < 400.0);
    }

    #[test]
    fn the_curve_goes_from_0_to_exactly_1_and_without_bounce_never_falls_or_passes_1() {
        assert_eq!((curve(0.0), curve(1.0)), (0.0, 1.0));
        assert_eq!((curve(-3.0), curve(7.0), curve(f32::NAN)), (0.0, 1.0, 0.0));
        let mut last = 0.0;
        for i in 0..=1000 {
            let u = curve(i as f32 / 1000.0);
            assert!(u >= last - 1e-6 && (0.0..=1.0).contains(&u), "{i}: {u}");
            last = u;
        }
        // as quick as the ease-out it took the place of: half the time is nine tenths of the way, a quarter more than half
        assert!(curve(0.5) > 0.88 && curve(0.25) > 0.55);
        // and unlike it, a start that picks up speed: the first twentieth of the time is under a twentieth of the way
        assert!(curve(0.05) < 0.07);
        assert!(1.0 - (1.0f32 - 0.05).powi(3) > 0.14, "(the cubic ease-out leaves at full speed)");
        // nothing is left to jump at the end
        assert!(1.0 - curve(0.999) < 1e-3);
    }

    #[test]
    fn a_curve_with_bounce_swings_past_1_and_ends_on_it() {
        let top = (0..=1000).map(|i| curve_with(i as f32 / 1000.0, 0.3)).fold(0.0, f32::max);
        assert!(top > 1.02 && top < 1.08, "{top}");
        assert_eq!(curve_with(1.0, 0.3), 1.0);
        assert!((curve_with(0.999, 0.3) - 1.0).abs() < 0.01);
    }
}
