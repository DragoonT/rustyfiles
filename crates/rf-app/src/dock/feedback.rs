//! What the dock's icons do after a click, so that a click can be felt (a user's report: "docktaki geri bildirim az, tıkladım mı
//! tıklamadım mı anlayamıyorum"). The press itself (the icon dips while the button is down) is `middle.rs`; this is what comes after:
//!
//! * STARTING: a click that starts an app bounces its icon twice (softly, upwards, in place) and shows a pulsing mark under it, until the
//!   app's first window is in the task list, the start fails, or ten seconds pass. A failed start shakes the icon once and leaves a red
//!   mark under it for a moment, which is what is left when animations are off (the sentence itself goes to the log: the dock has no
//!   surface for a line of text, the Start panel's notice needs its panel open).
//! * COMING TO THE FRONT: a click that brings a running app's window forward gives the icon one quick pulse, so the click is answered
//!   even when the window was already nearly in front.
//! * MINIMISING: a click on the app that is in front, which minimises it, dips the icon.
//!
//! Everything that is a rule is a pure function with a test (which answer a click gets, the curves, when the starting mark ends); the
//! state is a small map from an app's identity to what it is doing now. With animations off nothing moves: the starting mark is shown
//! steady, and the clicks are answered by their action alone.

use std::collections::HashMap;

use rf_win::dock::ClickAction;

/// The first bounce's height and the second's (dock units), and how long both take.
pub const BOUNCE_PEAK: f32 = 10.0;
pub const BOUNCE_PEAK_2: f32 = 4.0;
pub const BOUNCE_MS: u64 = 780;
/// Where the first bounce ends and the second starts (a fraction of the whole).
const BOUNCE_SPLIT: f32 = 0.56;
/// The pulse: how much bigger the icon gets (a fraction of its width) and how long it takes.
pub const PULSE_GROW: f32 = 0.10;
pub const PULSE_MS: u64 = 240;
/// The dip: how far down the icon goes (dock units) and how long it takes.
pub const DIP_DEPTH: f32 = 4.0;
pub const DIP_MS: u64 = 220;
/// The shake of a failed start: how far to each side (dock units), how many swings, how long.
pub const SHAKE_AMP: f32 = 4.0;
pub const SHAKE_SWINGS: f32 = 3.0;
pub const SHAKE_MS: u64 = 360;
/// How long the starting mark waits for the app's first window.
pub const START_TIMEOUT_MS: u64 = 10_000;
/// How long one pulse of the starting mark takes.
pub const MARK_PERIOD_MS: u64 = 900;
/// How long the red mark of a failed start stays under the icon.
pub const FAILED_HOLD_MS: u64 = 1_600;

/// What a click on an app gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Not running and not being started: start it.
    Start,
    /// A start is under way (the app has no window yet): the click is answered, the app is not started a second time.
    StillStarting,
    /// Running, not in front: bring its window forward.
    Raise,
    /// In front: minimise.
    Minimize,
}

/// The answer to a click for what the model decides (`model::click_action`) and whether a start of this app is pending.
pub fn answer(action: ClickAction, start_pending: bool) -> Answer {
    match action {
        ClickAction::Start if start_pending => Answer::StillStarting,
        ClickAction::Start => Answer::Start,
        ClickAction::Raise(_) => Answer::Raise,
        ClickAction::Minimize(_) => Answer::Minimize,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Bounce,
    Pulse,
    Dip,
    Shake,
}

impl Motion {
    pub fn duration_ms(self) -> u64 {
        match self {
            Motion::Bounce => BOUNCE_MS,
            Motion::Pulse => PULSE_MS,
            Motion::Dip => DIP_MS,
            Motion::Shake => SHAKE_MS,
        }
    }

    /// The motion that answers a click.
    pub fn of(answer: Answer) -> Motion {
        match answer {
            Answer::Start => Motion::Bounce,
            Answer::StillStarting | Answer::Raise => Motion::Pulse,
            Answer::Minimize => Motion::Dip,
        }
    }
}

/// One arc of a bounce: 0 at both ends, `peak` in the middle, a parabola (a ball's path).
fn arc(u: f32, peak: f32) -> f32 {
    4.0 * peak * u * (1.0 - u)
}

/// How high the icon is raised `ms` after the click that started it (dock units): two soft bounces, the second smaller. Zero at the
/// start and from the end on, never above [`BOUNCE_PEAK`].
pub fn bounce_height(ms: u64) -> f32 {
    if ms >= BOUNCE_MS {
        return 0.0;
    }
    let u = ms as f32 / BOUNCE_MS as f32;
    if u < BOUNCE_SPLIT { arc(u / BOUNCE_SPLIT, BOUNCE_PEAK) } else { arc((u - BOUNCE_SPLIT) / (1.0 - BOUNCE_SPLIT), BOUNCE_PEAK_2) }
}

/// How much bigger the icon is `ms` into a pulse (a fraction of its width): up and back, once.
pub fn pulse_grow(ms: u64) -> f32 {
    if ms >= PULSE_MS {
        return 0.0;
    }
    PULSE_GROW * (std::f32::consts::PI * ms as f32 / PULSE_MS as f32).sin()
}

/// How far the icon is lowered `ms` into a dip (dock units, positive is down): down and back, once.
pub fn dip_depth(ms: u64) -> f32 {
    if ms >= DIP_MS {
        return 0.0;
    }
    DIP_DEPTH * (std::f32::consts::PI * ms as f32 / DIP_MS as f32).sin()
}

/// How far sideways the icon is `ms` into the shake of a failed start (dock units): swings that die away.
pub fn shake_dx(ms: u64) -> f32 {
    if ms >= SHAKE_MS {
        return 0.0;
    }
    let u = ms as f32 / SHAKE_MS as f32;
    SHAKE_AMP * (2.0 * std::f32::consts::PI * SHAKE_SWINGS * u).sin() * (1.0 - u)
}

/// How an icon is drawn away from its place for a moment: raised, shifted sideways, bigger (a fraction of its width; negative is
/// smaller). All zero is the icon as it rests.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    /// Dock units up (negative: down).
    pub up: f32,
    pub dx: f32,
    pub grow: f32,
}

impl Pose {
    #[cfg(test)]
    pub fn is_still(&self) -> bool {
        *self == Pose::default()
    }
}

/// The pose `ms` into a motion.
pub fn pose_of(motion: Motion, ms: u64) -> Pose {
    match motion {
        Motion::Bounce => Pose { up: bounce_height(ms), ..Pose::default() },
        Motion::Pulse => Pose { grow: pulse_grow(ms), ..Pose::default() },
        Motion::Dip => Pose { up: -dip_depth(ms), ..Pose::default() },
        Motion::Shake => Pose { dx: shake_dx(ms), ..Pose::default() },
    }
}

/// Why a starting mark ends, or that it has not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartEnd {
    Pending,
    WindowSeen,
    Failed,
    TimedOut,
}

/// Where a starting mark stands: it began at `since` (ms), now is `now`. The app's window is what ends it first (the start worked, whatever
/// the launch said), then a failed launch, then the time.
pub fn start_state(since: u64, now: u64, window_seen: bool, failed: bool) -> StartEnd {
    if window_seen {
        StartEnd::WindowSeen
    } else if failed {
        StartEnd::Failed
    } else if now.saturating_sub(since) >= START_TIMEOUT_MS {
        StartEnd::TimedOut
    } else {
        StartEnd::Pending
    }
}

/// How strong the starting mark is `ms` after the click, 0 to 1 (a smooth pulse that never goes dark: the app is on its way).
pub fn mark_phase(ms: u64) -> f32 {
    let u = (ms % MARK_PERIOD_MS) as f32 / MARK_PERIOD_MS as f32;
    0.5 - 0.5 * (2.0 * std::f32::consts::PI * u).cos()
}

/// The opacity factor and the size factor of the starting mark for a phase.
pub fn mark_look(phase: f32) -> (f32, f32) {
    (0.35 + 0.65 * phase.clamp(0.0, 1.0), 0.85 + 0.3 * phase.clamp(0.0, 1.0))
}

/// The rectangle (`[x, y, w, h]`, dock units, the window's coordinates) an icon covers while it is drawn away from its place: its cell
/// (`x` from the left, `w` wide, standing on `bottom`) joined with its picture raised by `up`, moved sideways by `dx` and grown by `grow`
/// (a fraction of the width; a picture that is smaller stays inside the cell). The window is cut to a region of the glass and the boxes of
/// the icons that stand out of it, a picture outside the region is not shown, and an icon's box at rest is the cell's: so the island adds
/// this box to the region while an icon is posed, or a bounce, a pulse or the lift of an icon in the hand is cut off flat at the cell's
/// edge. Exactly the geometry the picture is drawn with in `middle.rs`.
pub fn posed_box(x: f32, w: f32, bottom: f32, up: f32, dx: f32, grow: f32) -> [f32; 4] {
    let g = grow.max(0.0) * w;
    let (cell_top, cell_right) = (bottom - w, x + w);
    let (left, top) = (x.min(x + dx - g), cell_top.min(cell_top - up - g));
    let (right, low) = (cell_right.max(x + dx - g + w + 2.0 * g), bottom.max(cell_top - up - g + w + 2.0 * g));
    [left, top, right - left, low - top]
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    motion: Option<(Motion, u64)>,
    /// When the start was clicked.
    starting: Option<u64>,
    /// When the start failed.
    failed: Option<u64>,
}

/// What the icons are doing after clicks, by the identity of the app (see `key_of`).
#[derive(Default)]
pub struct Feedback {
    entries: HashMap<String, Entry>,
}

/// The identity feedback is kept under. A pinned app keeps its pin's identity before, while and after it runs (the running window's own
/// identity can differ from the pin's, and a start's mark has to find the app again once it has a window); a running app that is not
/// pinned has its group's. (The pin's name is its identity, not its launch: the launch changes under a pin when the dock's list is read
/// again from its own copy of the shortcut.)
pub fn key_of(app: &rf_win::dock::DockApp) -> String {
    if app.pinned { format!("pin:{}", app.name.to_lowercase()) } else { format!("run:{}", app.key) }
}

impl Feedback {
    /// A click got `answer` at `now`. `animate`: the app's animations are on.
    pub fn click(&mut self, key: &str, answer: Answer, now: u64, animate: bool) {
        let e = self.entries.entry(key.to_string()).or_insert(Entry { motion: None, starting: None, failed: None });
        // (a new click answers the old failure)
        e.failed = None;
        if answer == Answer::Start {
            e.starting = Some(now);
        }
        e.motion = animate.then(|| (Motion::of(answer), now));
    }

    /// A start of this app is under way (and has not timed out).
    pub fn start_pending(&self, key: &str, now: u64) -> bool {
        self.entries.get(key).and_then(|e| e.starting).is_some_and(|since| start_state(since, now, false, false) == StartEnd::Pending)
    }

    /// Whether any start is pending (any time-out is the render's to find).
    pub fn has_pending_start(&self) -> bool {
        self.entries.values().any(|e| e.starting.is_some())
    }

    /// The app has a window: the starting mark ends.
    pub fn window_seen(&mut self, key: &str) {
        if let Some(e) = self.entries.get_mut(key) {
            e.starting = None;
        }
    }

    /// The start failed: the mark ends, the icon shakes (if animations are on) and a red mark stays under it for [`FAILED_HOLD_MS`].
    pub fn failed(&mut self, key: &str, now: u64, animate: bool) {
        if let Some(e) = self.entries.get_mut(key) {
            e.starting = None;
            e.motion = animate.then_some((Motion::Shake, now));
            e.failed = Some(now);
        }
    }

    /// Whether the red mark of a failed start is under this app's icon at `now`.
    pub fn failed_mark(&self, key: &str, now: u64) -> bool {
        self.entries.get(key).and_then(|e| e.failed).is_some_and(|t| now.saturating_sub(t) < FAILED_HOLD_MS)
    }

    /// How the icon of this app is drawn away from its place at `now`.
    pub fn pose(&self, key: &str, now: u64) -> Pose {
        match self.entries.get(key).and_then(|e| e.motion) {
            Some((m, t0)) => pose_of(m, now.saturating_sub(t0)),
            None => Pose::default(),
        }
    }

    /// The starting mark of this app at `now`: its phase (0 to 1, steady 1 with animations off), or `None` when it has none.
    pub fn mark(&self, key: &str, now: u64, animate: bool) -> Option<f32> {
        let since = self.entries.get(key)?.starting?;
        (start_state(since, now, false, false) == StartEnd::Pending).then(|| if animate { mark_phase(now.saturating_sub(since)) } else { 1.0 })
    }

    /// Whether anything needs another frame at `now`: a motion under way, or a pulsing mark.
    pub fn active(&self, now: u64, animate: bool) -> bool {
        self.entries.values().any(|e| {
            e.motion.is_some_and(|(m, t0)| now.saturating_sub(t0) < m.duration_ms())
                || (animate && e.starting.is_some_and(|s| start_state(s, now, false, false) == StartEnd::Pending))
                // (the red mark is steady, but the frame after it ends is the one that takes it away)
                || (animate && e.failed.is_some_and(|t| now.saturating_sub(t) < FAILED_HOLD_MS))
        })
    }

    /// Forgets what is over: motions that ended, starts that timed out.
    pub fn prune(&mut self, now: u64) {
        for e in self.entries.values_mut() {
            if e.motion.is_some_and(|(m, t0)| now.saturating_sub(t0) >= m.duration_ms()) {
                e.motion = None;
            }
            if e.starting.is_some_and(|s| start_state(s, now, false, false) != StartEnd::Pending) {
                e.starting = None;
            }
            if e.failed.is_some_and(|t| now.saturating_sub(t) >= FAILED_HOLD_MS) {
                e.failed = None;
            }
        }
        self.entries.retain(|_, e| e.motion.is_some() || e.starting.is_some() || e.failed.is_some());
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_gets_the_answer_for_how_the_app_stands() {
        // not running, nothing pending: start it
        assert_eq!(answer(ClickAction::Start, false), Answer::Start);
        // not running and a start is pending: answered, not started again
        assert_eq!(answer(ClickAction::Start, true), Answer::StillStarting);
        // running and not in front
        assert_eq!(answer(ClickAction::Raise(5), false), Answer::Raise);
        // in front
        assert_eq!(answer(ClickAction::Minimize(5), false), Answer::Minimize);
        // (a pending start never applies to a running app: it ended with its window)
        assert_eq!(answer(ClickAction::Raise(5), true), Answer::Raise);
        assert_eq!(answer(ClickAction::Minimize(5), true), Answer::Minimize);
    }

    #[test]
    fn each_answer_has_its_own_motion() {
        assert_eq!(Motion::of(Answer::Start), Motion::Bounce);
        assert_eq!(Motion::of(Answer::Raise), Motion::Pulse);
        assert_eq!(Motion::of(Answer::StillStarting), Motion::Pulse);
        assert_eq!(Motion::of(Answer::Minimize), Motion::Dip);
    }

    #[test]
    fn the_bounce_starts_and_ends_at_zero_stays_in_bounds_and_has_two_unequal_bounces() {
        assert_eq!(bounce_height(0), 0.0);
        assert_eq!(bounce_height(BOUNCE_MS), 0.0);
        assert_eq!(bounce_height(BOUNCE_MS + 5_000), 0.0, "and it stays ended");
        let mut peak = 0.0f32;
        let mut peaks = Vec::new();
        let mut prev = 0.0;
        let mut rising = true;
        for ms in 0..BOUNCE_MS {
            let h = bounce_height(ms);
            assert!((0.0..=BOUNCE_PEAK + 1e-4).contains(&h), "{ms}: {h}");
            peak = peak.max(h);
            if rising && h < prev {
                peaks.push(prev);
                rising = false;
            } else if !rising && h > prev {
                rising = true;
            }
            prev = h;
        }
        assert!((peak - BOUNCE_PEAK).abs() < 0.3, "the first bounce reaches about its peak: {peak}");
        assert_eq!(peaks.len(), 2, "two bounces: {peaks:?}");
        // the second bounce is lower than the first
        assert!(peaks[1] > 2.0 && peaks[1] < peaks[0], "{peaks:?}");
        // between the two it touches the ground
        let split = (BOUNCE_MS as f32 * BOUNCE_SPLIT) as u64;
        assert!(bounce_height(split) < 0.3);
    }

    #[test]
    fn the_pulse_the_dip_and_the_shake_start_and_end_at_rest() {
        for (at0, at_end, after) in [(pulse_grow(0), pulse_grow(PULSE_MS), pulse_grow(PULSE_MS + 1)), (dip_depth(0), dip_depth(DIP_MS), dip_depth(DIP_MS + 1)), (shake_dx(0), shake_dx(SHAKE_MS), shake_dx(SHAKE_MS + 1))] {
            assert_eq!((at0, at_end, after), (0.0, 0.0, 0.0));
        }
        let top = (0..PULSE_MS).map(pulse_grow).fold(0.0f32, f32::max);
        assert!(top > 0.09 && top <= PULSE_GROW + 1e-4, "{top}");
        let low = (0..DIP_MS).map(dip_depth).fold(0.0f32, f32::max);
        assert!(low > 3.8 && low <= DIP_DEPTH + 1e-4, "{low}");
        let (mut left, mut right) = (0.0f32, 0.0f32);
        for ms in 0..SHAKE_MS {
            let d = shake_dx(ms);
            assert!(d.abs() <= SHAKE_AMP + 1e-4);
            left = left.min(d);
            right = right.max(d);
        }
        assert!(left < -1.0 && right > 1.0, "it swings to both sides: {left} {right}");
        // the swings die away
        assert!((SHAKE_MS * 3 / 4..SHAKE_MS).map(|m| shake_dx(m).abs()).fold(0.0f32, f32::max) < (0..SHAKE_MS / 4).map(|m| shake_dx(m).abs()).fold(0.0f32, f32::max));
    }

    #[test]
    fn a_dip_goes_down_and_a_bounce_goes_up() {
        assert!(pose_of(Motion::Dip, DIP_MS / 2).up < 0.0);
        assert!(pose_of(Motion::Bounce, 100).up > 0.0);
        assert!(pose_of(Motion::Pulse, PULSE_MS / 2).grow > 0.0);
        assert!(pose_of(Motion::Shake, 30).dx != 0.0);
        assert!(pose_of(Motion::Bounce, BOUNCE_MS).is_still());
    }

    #[test]
    fn the_starting_mark_ends_when_the_window_is_seen_the_start_fails_or_ten_seconds_pass() {
        assert_eq!(start_state(1_000, 1_000, false, false), StartEnd::Pending);
        assert_eq!(start_state(1_000, 4_000, false, false), StartEnd::Pending);
        assert_eq!(start_state(1_000, 1_000 + START_TIMEOUT_MS - 1, false, false), StartEnd::Pending);
        assert_eq!(start_state(1_000, 1_000 + START_TIMEOUT_MS, false, false), StartEnd::TimedOut);
        assert_eq!(start_state(1_000, 99_000, false, false), StartEnd::TimedOut);
        assert_eq!(start_state(1_000, 2_000, true, false), StartEnd::WindowSeen);
        assert_eq!(start_state(1_000, 2_000, false, true), StartEnd::Failed);
        // the window is the proof that it worked, whatever the launch call said or however late it is
        assert_eq!(start_state(1_000, 50_000, true, true), StartEnd::WindowSeen);
        // a clock that is behind the start is not a time-out
        assert_eq!(start_state(1_000, 10, false, false), StartEnd::Pending);
    }

    #[test]
    fn the_starting_mark_pulses_and_never_goes_dark() {
        assert!(mark_phase(0) < 0.01, "it starts at its weakest");
        assert!((mark_phase(MARK_PERIOD_MS / 2) - 1.0).abs() < 0.01);
        assert!((mark_phase(MARK_PERIOD_MS) - mark_phase(0)).abs() < 0.01, "and comes round");
        for ms in (0..3 * MARK_PERIOD_MS).step_by(7) {
            let (alpha, size) = mark_look(mark_phase(ms));
            assert!((0.35..=1.0).contains(&alpha) && (0.85..=1.15).contains(&size), "{ms}: {alpha} {size}");
        }
        let (alpha, size) = mark_look(1.0);
        assert!((alpha - 1.0).abs() < 1e-5 && (size - 1.15).abs() < 1e-5);
    }

    #[test]
    fn a_start_bounces_marks_and_is_forgotten_when_its_window_comes() {
        let mut f = Feedback::default();
        assert!(f.is_empty() && !f.has_pending_start());
        f.click("pin:a", Answer::Start, 100, true);
        assert!(f.start_pending("pin:a", 5_000) && f.has_pending_start());
        assert!(f.pose("pin:a", 300).up > 0.0, "the icon is up in its bounce");
        assert!(f.mark("pin:a", 300, true).is_some());
        assert!(f.active(300, true));
        // another app is not affected
        assert_eq!(f.pose("pin:b", 300), Pose::default());
        assert!(!f.start_pending("pin:b", 300));
        // its window is in the list: the mark goes, the bounce (if still going) finishes by itself
        f.window_seen("pin:a");
        assert!(!f.start_pending("pin:a", 400) && f.mark("pin:a", 400, true).is_none());
        f.prune(100 + BOUNCE_MS);
        assert!(f.is_empty());
    }

    #[test]
    fn a_failed_start_ends_the_mark_and_shakes_the_icon() {
        let mut f = Feedback::default();
        f.click("pin:a", Answer::Start, 0, true);
        f.failed("pin:a", 500, true);
        assert!(!f.start_pending("pin:a", 501));
        assert!(f.pose("pin:a", 530).dx != 0.0);
        f.prune(500 + SHAKE_MS);
        assert!(f.failed_mark("pin:a", 500 + SHAKE_MS), "the red mark outlasts the shake");
        f.prune(500 + FAILED_HOLD_MS);
        assert!(f.is_empty());
        // a failure for an app nothing was started for is no news
        f.failed("pin:zzz", 600, true);
        assert!(f.is_empty());
    }

    #[test]
    fn a_start_that_never_gets_a_window_times_out_after_ten_seconds() {
        let mut f = Feedback::default();
        f.click("pin:a", Answer::Start, 0, true);
        assert!(f.mark("pin:a", START_TIMEOUT_MS - 1, true).is_some());
        assert!(f.mark("pin:a", START_TIMEOUT_MS, true).is_none());
        assert!(!f.start_pending("pin:a", START_TIMEOUT_MS));
        f.prune(START_TIMEOUT_MS);
        assert!(f.is_empty());
        assert!(!f.active(START_TIMEOUT_MS, true));
    }

    #[test]
    fn with_animations_off_nothing_moves_and_the_mark_is_steady() {
        let mut f = Feedback::default();
        f.click("pin:a", Answer::Start, 0, false);
        assert_eq!(f.pose("pin:a", 100), Pose::default());
        assert_eq!(f.mark("pin:a", 100, false), Some(1.0));
        assert!(!f.active(100, false), "a steady mark needs no frames");
        // the other answers leave nothing to draw at all
        f.click("pin:b", Answer::Raise, 0, false);
        f.click("pin:c", Answer::Minimize, 0, false);
        assert_eq!(f.pose("pin:b", 10), Pose::default());
        assert_eq!(f.pose("pin:c", 10), Pose::default());
        f.failed("pin:a", 200, false);
        assert_eq!(f.pose("pin:a", 210), Pose::default());
    }

    #[test]
    fn a_pulse_and_a_dip_pass_and_leave_no_trace() {
        let mut f = Feedback::default();
        f.click("run:x", Answer::Raise, 1_000, true);
        assert!(f.pose("run:x", 1_100).grow > 0.0 && f.active(1_100, true));
        f.click("run:y", Answer::Minimize, 1_000, true);
        assert!(f.pose("run:y", 1_100).up < 0.0);
        assert!(!f.active(1_000 + DIP_MS.max(PULSE_MS), true));
        f.prune(1_000 + DIP_MS.max(PULSE_MS));
        assert!(f.is_empty());
    }

    #[test]
    fn a_failed_start_leaves_a_red_mark_for_a_moment_with_animations_on_or_off() {
        for animate in [true, false] {
            let mut f = Feedback::default();
            f.click("pin:a", Answer::Start, 0, animate);
            assert!(!f.failed_mark("pin:a", 100), "not before it failed");
            f.failed("pin:a", 500, animate);
            assert!(f.failed_mark("pin:a", 500) && f.failed_mark("pin:a", 500 + FAILED_HOLD_MS - 1));
            assert!(!f.failed_mark("pin:a", 500 + FAILED_HOLD_MS));
            assert!(!f.failed_mark("pin:b", 600), "another app has none");
            // a steady mark asks for frames only while animations are on (with them off the island repaints once when it is over)
            assert_eq!(f.active(600 + SHAKE_MS, animate), animate);
            // it is forgotten when over, and a new click answers an old failure
            f.prune(500 + FAILED_HOLD_MS);
            assert!(f.is_empty());
            f.click("pin:a", Answer::Start, 0, animate);
            f.failed("pin:a", 10, animate);
            f.click("pin:a", Answer::Start, 20, animate);
            assert!(!f.failed_mark("pin:a", 30));
        }
    }

    #[test]
    fn a_raised_or_grown_picture_is_inside_the_windows_region() {
        use rf_win::dock::hit;
        // the glass lies at y 24..96 (window units), the icons stand on y 84; the icon under the pointer is 68 wide at full magnification:
        // its cell is y 16..84, eight units over the glass's top edge
        let glass = [0.0, 24.0, 400.0, 72.0];
        let (x, w, bottom) = (100.0, 68.0, 84.0);
        let rest = [x, bottom - w, w, w];
        let region = |extra: Option<[f32; 4]>| {
            let mut boxes = vec![rest];
            boxes.extend(extra);
            hit::parts(glass, 20.0, &boxes, 500, 120)
        };
        // a bounce of 10 puts the picture's top at y 6: nothing but the cell's own box is there, so it was cut
        let up = 10.0;
        assert!(!hit::takes_pointer(&region(None), 130, 10), "without the posed box the top of the bounce is outside the region");
        let posed = posed_box(x, w, bottom, up, 0.0, 0.0);
        assert!(hit::takes_pointer(&region(Some(posed)), 130, 10), "with it the whole picture is inside");
        assert!(hit::takes_pointer(&region(Some(posed)), 130, 83), "and the cell still is");
        // a pulse grows the picture on all sides
        let pulse = posed_box(x, w, bottom, 0.0, 0.0, PULSE_GROW);
        assert!(pulse[0] < x && pulse[1] < bottom - w && pulse[2] > w && pulse[3] > w);
        assert!(hit::takes_pointer(&region(Some(pulse)), (x - 4.0) as i32, 30));
        // a shake moves it sideways
        let shake = posed_box(x, w, bottom, 0.0, SHAKE_AMP, 0.0);
        assert_eq!((shake[0], shake[2]), (x, w + SHAKE_AMP));
        // a dip goes down inside the cell's column and a smaller picture (a press) stay inside the cell: the cell's own box
        assert_eq!(posed_box(x, w, bottom, -DIP_DEPTH, 0.0, 0.0), [x, bottom - w, w, w + DIP_DEPTH]);
        assert_eq!(posed_box(x, w, bottom, 0.0, 0.0, -0.07), rest);
        assert_eq!(posed_box(x, w, bottom, 0.0, 0.0, 0.0), rest, "an icon at rest covers its cell");
    }

    #[test]
    fn a_pinned_app_keeps_one_identity_before_and_while_it_runs() {
        use rf_win::dock::{DockApp, IconSource, Launch};
        let launch = Launch::Exe { path: "C:/a/a.exe".into(), args: String::new() };
        let mut app = DockApp { key: "c:/a/a.exe".into(), name: "A".into(), pinned: true, launch: Some(launch), icon: IconSource::File("C:/a/a.exe".into()), windows: vec![], active: false };
        let idle = key_of(&app);
        // once it runs its group's key can be another one (a window that names itself): the identity is the pin's
        app.windows = vec![7];
        app.key = "some.aumid".into();
        assert_eq!(key_of(&app), idle);
        // an app that is only running has its group's
        app.pinned = false;
        assert_eq!(key_of(&app), "run:some.aumid");
    }
}
