//! What a glass window that is seen but not in front is owed: the rules, with no window and no clock of their own (the engine passes
//! the facts and the time in, so the tests need nothing).
//!
//! WHY. A glass window that is hidden, minimised or covered is left out of everything already (`cover.rs`). The windows that remain are
//! all "seen", but only one of them, or none, is the one the user works in. A window behind the one in front, a dock island, a
//! window on the other monitor shows a frost of the screen behind it: nothing it shows needs a backdrop fresher than a few times a
//! second, while every backdrop costs the capture service, a frame handler, the engine thread, a copy of the monitor, a blur and a read
//! back. So the window is owed one of three things:
//!
//! * [`Care::Front`]: the window is the foreground window (or owned by it, or owns it), or the pointer is over it, or it was a moment
//!   ago. It follows the screen at the full rate, as before.
//! * [`Care::Behind`]: it follows the screen at [`BEHIND_FPS`] at most, and only when the capture says something under it changed (that
//!   part is the engine's old rule). A lens that slides or a shape that widens does not depend on the capture and is drawn at the
//!   display's rate as ever.
//! * [`Care::Frozen`]: the window asked for it (the setting "freeze the glass while not in front") and has a picture: nothing is
//!   prepared, drawn or read back for it, the last picture stays, and it is not counted when the engine decides what to capture. A
//!   window that never drew is never frozen: it would stay empty.

use std::time::{Duration, Instant};

/// The most backdrops a second a window that is seen but not in front is given. The capture is frosted by about 10 px and nothing in
/// front is looking at it. 20 is the upper end of what the design asked for (15 to 20): the cost is in proportion to the rate, and at
/// 20 a moving backdrop behind a 900 x 560 window cost about 2 % of a core against 17 % (numbers in `docs/design/dock.md`, "Glass that
/// is not in front"); how it looks at 20 against 15 was not judged on a screen.
pub(super) const BEHIND_FPS: f64 = 20.0;

/// A window stays [`Care::Front`] this long after it stopped being the foreground window or under the pointer: a click that moves the
/// focus from one window to the next, or a pointer that crosses the edge and returns, does not make the frost step down and up again.
pub(super) const HOLD: Duration = Duration::from_millis(400);

/// How long the engine waits for a frame when nothing has happened for a while and no window is in front: nothing it would notice
/// in time needs a shorter one (a change of the foreground window and the pointer coming over a window are events that end the wait
/// like a command, `watch.rs` and `glass::pointer_over`; a frame ends the wait of a capture session at once). This was NOT shown to
/// lower the wake-ups of the engine thread on the capture-session path (see `docs/design/dock.md`, "Glass that is not in front"); it
/// is kept because it cannot make anything later than the events above, and the per-cause turn counters
/// (`RUSTYFILES_GLASS_STATS=1`) are there to find what does wake the thread.
pub(super) const BEHIND_QUIET_WAIT_MS: u32 = 100;

/// The quiet wait for a frame: `base` (what the engine waits when nothing is in front or not), or `BEHIND_QUIET_WAIT_MS` when this is
/// a capture session, some glass is seen and none is in front, and no glass window of another process of ours was seen at the last
/// scan. Such a window is looked for once a turn (`crossproc.rs`, `SCAN_EVERY`) and the two processes must notice each other well
/// inside `crossproc::HOLD`, so while one exists the turn stays as short as the scan clock.
pub(super) fn quiet_wait_ms(base: u32, session: bool, none_in_front: bool, foreign_seen: bool) -> u32 {
    if session && none_in_front && !foreign_seen { base.max(BEHIND_QUIET_WAIT_MS) } else { base }
}

/// How long a window that is not worked for (hidden, minimised, covered, frozen) keeps its backdrop pyramid before it is given back.
/// A short cover (an Alt+Tab, a window dragged over it) never pays for a new one; the picture on the screen is the swap chain's and
/// stays. Coming back, the pyramid is made again by the draw that follows the first fresh capture, before anything is presented.
pub(super) const TRIM_AFTER: Duration = Duration::from_secs(15);

/// Whether a window that has not been worked for `idle` long has had it.
pub(super) fn trim_due(idle: Duration) -> bool {
    idle >= TRIM_AFTER
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Care {
    Front,
    Behind,
    Frozen,
}

/// What the engine knows about one window that is shown and not covered.
#[derive(Clone, Copy, Debug)]
pub(super) struct Facts {
    /// The window, or the window that owns it or that it owns, is the foreground window.
    pub front: bool,
    /// The pointer is over the window (the topmost window at the pointer is this one).
    pub pointer: bool,
    /// How long ago the window was last front or under the pointer; `None` if it never was (or is now).
    pub since: Option<Duration>,
    /// The app asked for this window's glass to freeze while it is not in front.
    pub freeze: bool,
    /// The window has put a picture on the screen at least once.
    pub drawn: bool,
}

pub(super) fn care(f: &Facts) -> Care {
    if f.front || f.pointer || f.since.is_some_and(|d| d < HOLD) {
        return Care::Front;
    }
    if f.freeze && f.drawn { Care::Frozen } else { Care::Behind }
}

/// [`care`] when the rules are on; `Front` for every window when they are off (`RUSTYFILES_GLASS_BG_FPS=0`, see [`rules_on`]).
pub(super) fn care_under(rules: bool, f: &Facts) -> Care {
    if rules { care(f) } else { Care::Front }
}

/// The facts of one window from what the engine reads once a turn: the root owner of the window (`own_root`), of the foreground window
/// (`fg_root`) and of the window under the pointer (`under_pointer`), each 0 for none, when the window was last in front (`last_front`)
/// and the clock. Returns the facts and the new `last_front`: now when the window is in front or the pointer is over it. A pointer on
/// a window of another monitor is a window of another root, so it gives nothing to this one.
pub(super) fn facts_of(own_root: isize, fg_root: isize, under_pointer: isize, last_front: Option<Instant>, now: Instant, freeze: bool, drawn: bool) -> (Facts, Option<Instant>) {
    let front = is_front(own_root, fg_root);
    let pointer = own_root != 0 && own_root == under_pointer;
    let last = if front || pointer { Some(now) } else { last_front };
    (Facts { front, pointer, since: last.map(|t| now.saturating_duration_since(t)), freeze, drawn }, last)
}

/// Whether the rate rules are on: `RUSTYFILES_GLASS_BG_FPS=0` turns them off, every seen window is then in front (nothing is slowed or
/// frozen and the engine's quiet wait is the old one). What a window nobody sees gives back after `TRIM_AFTER`, the scan for glass of
/// other processes and Desktop Duplication's reading of its dirty rectangles are not part of the rules and stay on.
pub(super) fn rules_on() -> bool {
    behind_fps() > 0.0
}

/// The capture rate a monitor's session is asked for, from the care of the windows on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pace {
    /// Some window on it is in front: the normal rate.
    Full,
    /// Windows are seen but none is in front.
    Slow,
}

/// The pace a monitor needs: `Full` as soon as one window of it is in front. (A monitor with no seen glass has no session to pace.)
pub(super) fn pace<I: IntoIterator<Item = Care>>(cares: I) -> Pace {
    if cares.into_iter().any(|c| c == Care::Front) { Pace::Full } else { Pace::Slow }
}

/// The shortest time between two frames for a pace, in 100 ns ticks: `normal` (what `wgc::interval_for` gives) for `Full`, and for
/// `Slow` the longer of that and `1 / BEHIND_FPS` (never shorter than the normal rate: a display or a battery that already asks for
/// less keeps it). `fps` overrides [`BEHIND_FPS`] (`RUSTYFILES_GLASS_BG_FPS`); `0` means no difference from `Full`.
pub(super) fn interval_ticks(normal: i64, pace: Pace, fps: f64) -> i64 {
    match pace {
        Pace::Full => normal,
        Pace::Slow if fps > 0.0 && fps.is_finite() => normal.max((1e7 / fps) as i64),
        Pace::Slow => normal,
    }
}

/// `RUSTYFILES_GLASS_BG_FPS` read once: a number of frames a second for windows that are seen but not in front (`0`: the rate rules
/// are off, see [`rules_on`]; for comparing on a live desktop without a new build).
pub(super) fn behind_fps() -> f64 {
    static FPS: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *FPS.get_or_init(|| std::env::var("RUSTYFILES_GLASS_BG_FPS").ok().and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(BEHIND_FPS))
}

/// Whether a window that has a draw waiting may take its turn now. A window in front always may. A window that is seen but not in
/// front may draw because of a new backdrop (`stale`) only once the gap since its last such draw is over: the capture of its monitor
/// can be fast because another window of that monitor is in front, and this one still follows the screen at `BEHIND_FPS`. A draw
/// that only moves a lens or a shape (not `stale`) uses the backdrop the window has: cheap, and never held back.
pub(super) fn draw_due(care: Care, stale: bool, since_last: Option<Duration>, gap: Duration) -> bool {
    match care {
        Care::Behind if stale => since_last.is_none_or(|d| d >= gap),
        _ => true,
    }
}

/// How long until a window held back by [`draw_due`] may draw: `None` when it may now.
pub(super) fn draw_wait(care: Care, stale: bool, since_last: Option<Duration>, gap: Duration) -> Option<Duration> {
    if draw_due(care, stale, since_last, gap) { None } else { Some(gap - since_last.unwrap_or_default().min(gap)) }
}

/// The time Desktop Duplication leaves between two frames it takes at a rate (`fps`): none when the rate is zero or not a number.
/// Desktop Duplication has no minimum update interval of its own.
pub(super) fn gap_for(fps: f64) -> Duration {
    if fps > 0.0 && fps.is_finite() { Duration::from_secs_f64(1.0 / fps) } else { Duration::ZERO }
}

/// Whether a window of `hwnd` (as its root owner `own_root`) is in front when the foreground window has the root owner `fg_root`. The
/// foreground window may be a popup the window owns, or the window may be a popup of the foreground window (a menu that is open): the
/// same root owner. Zero is no window.
pub(super) fn is_front(own_root: isize, fg_root: isize) -> bool {
    own_root != 0 && own_root == fg_root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts { front: false, pointer: false, since: None, freeze: false, drawn: true }
    }

    #[test]
    fn a_window_in_front_or_under_the_pointer_gets_the_full_rate() {
        assert_eq!(care(&Facts { front: true, ..facts() }), Care::Front);
        assert_eq!(care(&Facts { pointer: true, ..facts() }), Care::Front);
        // even when it asked to freeze
        assert_eq!(care(&Facts { front: true, freeze: true, ..facts() }), Care::Front);
        assert_eq!(care(&Facts { pointer: true, freeze: true, ..facts() }), Care::Front);
    }

    #[test]
    fn a_window_that_just_lost_the_front_keeps_the_full_rate_for_a_moment() {
        let just = Facts { since: Some(HOLD - Duration::from_millis(1)), ..facts() };
        assert_eq!(care(&just), Care::Front);
        assert_eq!(care(&Facts { freeze: true, ..just }), Care::Front);
        let later = Facts { since: Some(HOLD), ..facts() };
        assert_eq!(care(&later), Care::Behind);
        assert_eq!(care(&Facts { freeze: true, ..later }), Care::Frozen);
    }

    #[test]
    fn a_window_seen_and_not_in_front_is_slowed_or_frozen_as_it_asked() {
        assert_eq!(care(&facts()), Care::Behind);
        assert_eq!(care(&Facts { freeze: true, ..facts() }), Care::Frozen);
    }

    #[test]
    fn a_window_with_no_picture_is_never_frozen() {
        assert_eq!(care(&Facts { freeze: true, drawn: false, ..facts() }), Care::Behind, "it would stay empty");
    }

    #[test]
    fn one_window_in_front_keeps_its_monitor_at_the_full_rate() {
        assert_eq!(pace([Care::Behind, Care::Front, Care::Frozen]), Pace::Full);
        assert_eq!(pace([Care::Behind, Care::Behind]), Pace::Slow);
        assert_eq!(pace([Care::Frozen]), Pace::Slow);
        assert_eq!(pace([]), Pace::Slow, "no glass on it: nothing to pace");
    }

    #[test]
    fn the_slow_pace_is_twenty_frames_a_second_and_never_faster_than_the_normal_one() {
        let normal_60 = 166_666; // 16.7 ms
        assert_eq!(interval_ticks(normal_60, Pace::Full, BEHIND_FPS), normal_60);
        assert_eq!(interval_ticks(normal_60, Pace::Slow, BEHIND_FPS), 500_000, "50 ms");
        // on battery the normal rate is 30 a second (33 ms): the slow one is still 20
        assert_eq!(interval_ticks(333_333, Pace::Slow, BEHIND_FPS), 500_000);
        // a display or a setting that already gives less than 20 keeps it
        assert_eq!(interval_ticks(1_000_000, Pace::Slow, BEHIND_FPS), 1_000_000);
        // 0 or nonsense: the old behaviour
        assert_eq!(interval_ticks(normal_60, Pace::Slow, 0.0), normal_60);
        assert_eq!(interval_ticks(normal_60, Pace::Slow, f64::NAN), normal_60);
        assert_eq!(interval_ticks(normal_60, Pace::Slow, f64::INFINITY), normal_60);
    }

    #[test]
    fn desktop_duplication_leaves_a_gap_of_the_same_rate() {
        use super::super::capture::gap_left;
        use std::time::Instant;
        assert_eq!(gap_for(20.0), Duration::from_millis(50));
        assert_eq!(gap_for(0.0), Duration::ZERO);
        assert_eq!(gap_for(f64::NAN), Duration::ZERO);
        let t0 = Instant::now();
        let gap = gap_for(BEHIND_FPS);
        // nothing taken yet, or no gap: take at once
        assert_eq!(gap_left(None, gap, t0), None);
        assert_eq!(gap_left(Some(t0), Duration::ZERO, t0), None);
        // 10 ms after a take 40 ms are left; after the gap, none
        assert_eq!(gap_left(Some(t0), gap, t0 + Duration::from_millis(10)), Some(Duration::from_millis(40)));
        assert_eq!(gap_left(Some(t0), gap, t0 + gap), None);
        assert_eq!(gap_left(Some(t0), gap, t0 + gap * 3), None);
        // a clock that went back is not a reason to wait for ever
        assert_eq!(gap_left(Some(t0 + gap), gap, t0), Some(gap));
    }

    #[test]
    fn a_window_behind_another_of_the_same_monitor_still_draws_at_its_own_slow_rate() {
        let gap = gap_for(BEHIND_FPS);
        let ms = Duration::from_millis;
        // in front: always its turn, however fresh its last draw
        assert!(draw_due(Care::Front, true, Some(ms(1)), gap));
        // behind with a new backdrop: its turn only when the gap is over, or it never drew
        assert!(draw_due(Care::Behind, true, None, gap));
        assert!(!draw_due(Care::Behind, true, Some(ms(10)), gap));
        assert!(!draw_due(Care::Behind, true, Some(gap - ms(1)), gap));
        assert!(draw_due(Care::Behind, true, Some(gap), gap));
        // behind but only a lens or a shape moved (the backdrop is the one it has): never held back
        assert!(draw_due(Care::Behind, false, Some(ms(1)), gap));
        // a window with a zero gap (no rule: RUSTYFILES_GLASS_BG_FPS=0) is never held back
        assert!(draw_due(Care::Behind, true, Some(Duration::ZERO), Duration::ZERO));
        // how long it waits
        assert_eq!(draw_wait(Care::Behind, true, Some(ms(10)), gap), Some(ms(40)));
        assert_eq!(draw_wait(Care::Behind, true, Some(gap * 2), gap), None);
        assert_eq!(draw_wait(Care::Front, true, Some(ms(10)), gap), None);
    }

    #[test]
    fn a_window_nobody_sees_gives_back_its_pyramid_after_a_while_and_not_before() {
        assert!(!trim_due(Duration::ZERO));
        assert!(!trim_due(TRIM_AFTER - Duration::from_millis(1)), "an Alt+Tab does not pay for a new pyramid");
        assert!(trim_due(TRIM_AFTER));
        assert!(trim_due(TRIM_AFTER * 10));
        assert!(TRIM_AFTER > Duration::from_secs(5), "longer than a capture session is kept after its last glass (SESSION_GRACE)");
    }

    #[test]
    fn with_the_rules_off_every_window_is_in_front() {
        assert_eq!(care_under(false, &facts()), Care::Front);
        assert_eq!(care_under(false, &Facts { freeze: true, ..facts() }), Care::Front, "no freezing either");
        assert_eq!(care_under(true, &facts()), Care::Behind);
        assert_eq!(care_under(true, &Facts { freeze: true, ..facts() }), Care::Frozen);
    }

    #[test]
    fn the_quiet_wait_is_longer_only_for_a_session_with_glass_seen_and_none_in_front_and_no_other_process_near() {
        assert_eq!(quiet_wait_ms(50, true, true, false), BEHIND_QUIET_WAIT_MS);
        assert_eq!(quiet_wait_ms(50, true, false, false), 50, "something is in front");
        assert_eq!(quiet_wait_ms(50, false, true, false), 50, "Desktop Duplication's wait cannot be cut short by a command");
        assert_eq!(quiet_wait_ms(50, true, true, true), 50, "a glass window of another process is looked for once a turn");
        assert_eq!(quiet_wait_ms(250, true, true, false), 250, "never shorter than the wait it replaces");
    }

    #[test]
    fn the_facts_come_from_the_roots_of_the_windows() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(5);
        // the foreground window is this window, or a popup it owns (the same root owner): in front
        let (f, last) = facts_of(0x10, 0x10, 0x99, None, t0, false, true);
        assert!(f.front && !f.pointer);
        assert_eq!(last, Some(t0));
        assert_eq!(f.since, Some(Duration::ZERO));
        // the pointer is over it (or over a popup it owns), another program is in front
        let (f, last) = facts_of(0x10, 0x20, 0x10, None, t0, false, true);
        assert!(!f.front && f.pointer);
        assert_eq!(last, Some(t0));
        // another monitor: the pointer is over some other window, another is in front, this one is neither, and it was in front 5 s ago
        let (f, last) = facts_of(0x10, 0x20, 0x30, Some(t0), later, false, true);
        assert!(!f.front && !f.pointer);
        assert_eq!(last, Some(t0), "last_front is kept");
        assert_eq!(f.since, Some(Duration::from_secs(5)));
        assert_eq!(care(&f), Care::Behind);
        // no foreground window and no window under the pointer: nobody is in front, however the roots compare
        let (f, last) = facts_of(0, 0, 0, None, t0, false, true);
        assert!(!f.front && !f.pointer);
        assert_eq!(last, None);
        // a window that is not known (root 0) is never under the pointer
        assert!(!facts_of(0, 0x20, 0, None, t0, false, true).0.pointer);
        // the freeze and the picture are passed on
        let (f, _) = facts_of(0x10, 0x20, 0x30, None, t0, true, false);
        assert!(f.freeze && !f.drawn);
    }

    #[test]
    fn the_foreground_window_and_its_popups_are_the_same_front() {
        assert!(is_front(0x10, 0x10));
        assert!(!is_front(0x10, 0x20));
        assert!(!is_front(0, 0), "no foreground window is nobody's front");
    }
}
