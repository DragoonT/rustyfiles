//! The dock's size as a continuous value (a slider from %75 to %150) and how a running dock follows it while the slider is dragged.
//!
//! The value is kept in thousandths of the default ("permille"): 1000 is the dock as it always was, 850 / 1000 / 1200 / 1400 are what
//! the four names of the older setting (small, medium, large, xlarge) meant. A whole number keeps the value comparable (`Eq`, no float
//! noise) and the file readable (1250 is written as 1.25).
//!
//! WHAT IS HERE (all pure, tested without a window):
//! * the range, the step, the reading of old and new values and the percent text,
//! * the one-line message the settings window sends to the dock while the slider moves (`size <stamp> <permille> <0|1>`) and the outbox
//!   that keeps only the newest step,
//! * [`Redo`]: what a size change redoes while the slider is dragged and what only when it is let go,
//! * [`Live`]: the dock's side. It gets those messages, lays the islands out at most once a frame, does the deferred work once the
//!   slider rests or is released, and tells a settings file read that arrives meanwhile whether it may overrule the slider.
//!
//! WHY A MESSAGE AND NOT THE FILE. The file is written when the changes have settled (`settings::update_live`), so a dock that only
//! followed the file would jump after the drag. The message carries the number itself, costs no disk and is cheap to drop: the next
//! step replaces a lost one. The file is still written afterwards and stays the truth.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The smallest size the slider offers (%75).
pub const MIN: u16 = 750;
/// The biggest (%150).
pub const MAX: u16 = 1500;
/// The slider's step (%5).
pub const STEP: u16 = 50;
/// The dock as it always was.
pub const DEFAULT: u16 = 1000;
/// The last position of the slider (positions are 0..=POSITIONS).
pub const POSITIONS: u8 = ((MAX - MIN) / STEP) as u8;

/// The factor a value stands for.
pub fn factor(permille: u16) -> f32 {
    f32::from(permille) / 1000.0
}

/// A number read from a file (or typed), cut to the range. NaN has no place on the line, so it reads as the default; the infinities
/// and everything else outside the range go to the near end (zero and negative numbers to the smallest size).
pub fn from_number(v: f64) -> u16 {
    if v.is_nan() {
        return DEFAULT;
    }
    // (cut before the cast: `as` saturates, but a huge float should not depend on that)
    (v * 1000.0).round().clamp(f64::from(MIN), f64::from(MAX)) as u16
}

/// A stored value of the wire or a test, cut to the range.
pub fn clamp(permille: u16) -> u16 {
    permille.clamp(MIN, MAX)
}

/// What the four names of the older setting meant.
pub fn from_legacy_name(name: &str) -> Option<u16> {
    match name.trim().to_ascii_lowercase().as_str() {
        "small" => Some(850),
        "medium" => Some(DEFAULT),
        "large" => Some(1200),
        "xlarge" => Some(1400),
        _ => None,
    }
}

/// The old name of a value that is exactly one of the four sizes. Those are still written as their names, so a file this version
/// saves at the default (or a size the old dropdown had) is the file the old version wrote and an older build reads it.
pub fn legacy_name(permille: u16) -> Option<&'static str> {
    match permille {
        850 => Some("small"),
        DEFAULT => Some("medium"),
        1200 => Some("large"),
        1400 => Some("xlarge"),
        _ => None,
    }
}

/// The slider position (0..=POSITIONS) nearest to a value: what the thumb shows for a value between two steps.
pub fn position_of(permille: u16) -> u8 {
    ((clamp(permille) - MIN + STEP / 2) / STEP) as u8
}

/// The value at a slider position (a position past the end is the end).
pub fn at_position(position: u8) -> u16 {
    MIN + STEP * u16::from(position.min(POSITIONS))
}

/// The value shown next to the slider: "%100" in Turkish, "100%" in English. A value between two whole percents is rounded.
pub fn percent_text(permille: u16, turkish: bool) -> String {
    let percent = (u32::from(permille) + 5) / 10;
    if turkish { format!("%{percent}") } else { format!("{percent}%") }
}

/// The widest the left island gets, dock units (the logo, a long search field and the weather; the number `size`'s 1366 test uses).
const WIDEST_LEFT: f32 = 388.0;
/// The right island without its tray strip, dock units (the usual clock lines).
const RIGHT_BASE: f32 = 280.0;

/// The biggest size a screen `work_width` logical px wide can hold: the larger of `permille` and the range's smallest is cut so that the
/// two side islands, at their widest (the left one with its weather, the right one with a full tray strip), their margins to the screen
/// and the least gap between them still fit. Only the sides are counted, the middle island has always been the one that gives way on a
/// narrow screen (`layout::islands` squeezes it). WHY HERE AND NOT IN THE LAYOUT: the side islands are never squeezed, so a size the
/// screen cannot hold would make them overlap whatever the layout does; and the worst case is a fact of the screen alone, so every
/// place that works out the unit (the layout, the band, the views) gets the same answer from one function.
///
/// At 1366 px this is %142, so none of the four sizes the setting had before is touched there; at 1920 px and wider nothing is cut.
pub fn fit_to_width(permille: u16, work_width_logical: f32) -> u16 {
    use crate::dock::{layout, tray_strip};
    let permille = clamp(permille);
    if !work_width_logical.is_finite() || work_width_logical <= 0.0 {
        return permille;
    }
    let right = RIGHT_BASE + tray_strip::strip_limit(work_width_logical) + 2.0 * tray_strip::SEP_GAP + tray_strip::SEP_W;
    let needed = WIDEST_LEFT + right + 2.0 * layout::MARGIN_SIDE + layout::MIN_GAP;
    // (rounded down, then the layout's own pixel rounding has a pixel or two of play: a little under is what keeps the test exact)
    let room = (f64::from(work_width_logical) / f64::from(needed) * 1000.0 - 1.0).floor();
    permille.min(room.clamp(f64::from(MIN), f64::from(MAX)) as u16)
}

// ---------------------------------------------------------------- the message

/// One size message of the settings window to the dock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Msg {
    /// Grows with every message of a sender (milliseconds since the epoch, made strictly increasing). The pipe gives every message its
    /// own thread, so two messages can arrive in the other order: the older one is dropped.
    pub stamp: u64,
    pub permille: u16,
    /// The slider was let go (or a key made a single step): the deferred work may be done now.
    pub released: bool,
}

pub fn message(m: Msg) -> String {
    format!("size {} {} {}", m.stamp, m.permille, u8::from(m.released))
}

/// The message a line is, or `None` for any other line or a malformed one. The value is cut to the range.
pub fn parse_message(line: &str) -> Option<Msg> {
    let mut it = line.split_whitespace();
    if it.next()? != "size" {
        return None;
    }
    let stamp = it.next()?.parse::<u64>().ok()?;
    let permille = clamp(it.next()?.parse::<u16>().ok()?);
    let released = match it.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    it.next().is_none().then_some(Msg { stamp, permille, released })
}

/// The messages waiting to be sent. A step that is followed by anything is stale (the next message carries the newer value), so only
/// the newest step and every release are kept: a burst of drag events is one write to the pipe.
#[derive(Default)]
pub struct Outbox {
    queue: VecDeque<Msg>,
}

impl Outbox {
    pub const fn new() -> Outbox {
        Outbox { queue: VecDeque::new() }
    }

    pub fn push(&mut self, m: Msg) {
        if self.queue.back().is_some_and(|last| !last.released) {
            self.queue.pop_back();
        }
        self.queue.push_back(m);
    }

    pub fn pop(&mut self) -> Option<Msg> {
        self.queue.pop_front()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

// ---------------------------------------------------------------- what is redone when

/// The pieces of work a size change can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Redo {
    /// The environment the views read (the unit) and the glass's corner radius.
    pub environment: bool,
    /// The three windows' places and regions, and the glass's shape inside each.
    pub windows: bool,
    /// A panel that is open is closed (it would stay where the islands were). Nothing is open after the first step, so later steps close
    /// nothing; nothing is ever opened by a size change.
    pub close_panels: bool,
    /// The tray strip's room.
    pub tray_strip: bool,
    /// The band the dock reserves at the screen's edge (every maximised window of every program is resized when it changes).
    pub reserved_band: bool,
    /// The icons are asked for again at the new pixel size (the old pictures stay on show until the new arrive).
    pub icons: bool,
}

/// One piece of work, in the order it is done in. What the dock performs is `Redo::steps`, so the table below is the code's own, not a
/// comment beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Environment,
    ClosePanels,
    TrayStrip,
    Windows,
    ReservedBand,
    Icons,
}

impl Redo {
    /// The pieces of work this asks for, in the order they must be done: the environment first (everything after reads the unit), the
    /// windows after the panels and the strip, the band and the icons last.
    pub fn steps(self) -> Vec<Step> {
        [
            (self.environment, Step::Environment),
            (self.close_panels, Step::ClosePanels),
            (self.tray_strip, Step::TrayStrip),
            (self.windows, Step::Windows),
            (self.reserved_band, Step::ReservedBand),
            (self.icons, Step::Icons),
        ]
        .into_iter()
        .filter_map(|(on, step)| on.then_some(step))
        .collect()
    }
}

/// Every step of a drag, at most once a frame. All of it is a few arithmetic results, three window moves and a redraw; nothing here waits
/// for the disk, the shell or another program.
pub const WHILE_DRAGGING: Redo = Redo { environment: true, windows: true, close_panels: true, tray_strip: true, reserved_band: false, icons: false };

/// Once, when the slider is let go or has rested for [`IDLE`]. The band is deferred because changing it resizes other programs'
/// windows (a drag would shake the whole desktop for every step); the icons because each step would ask the shell and the packages for
/// every icon again at a size that is gone by the time the picture arrives.
pub const AT_REST: Redo = Redo { environment: false, windows: false, close_panels: false, tray_strip: false, reserved_band: true, icons: true };

/// A size that arrives from the settings file (a change made by hand, a reload when nothing was dragged): everything at once.
pub const FROM_FILE: Redo = Redo {
    environment: WHILE_DRAGGING.environment || AT_REST.environment,
    windows: WHILE_DRAGGING.windows || AT_REST.windows,
    close_panels: WHILE_DRAGGING.close_panels || AT_REST.close_panels,
    tray_strip: WHILE_DRAGGING.tray_strip || AT_REST.tray_strip,
    reserved_band: WHILE_DRAGGING.reserved_band || AT_REST.reserved_band,
    icons: WHILE_DRAGGING.icons || AT_REST.icons,
};

// ---------------------------------------------------------------- the dock's side

/// One frame: the islands are laid out at most this often while a slider is dragged.
pub const FRAME: Duration = Duration::from_millis(16);
/// A slider that has not moved for this long counts as rested: the deferred work is done even if no release message came (the window
/// was closed with the button down, the message was lost).
pub const IDLE: Duration = Duration::from_millis(600);
/// For this long after a message the settings file does not overrule the slider: a file written at an earlier step of the same drag
/// can be read after a later message has arrived.
pub const HOLD: Duration = Duration::from_millis(1500);
/// A message whose stamp is more than this much older than the newest seen is not a message that overtook another on the pipe (that
/// is a matter of milliseconds) but one from a clock that went back, or from a settings window started after one with a far-future clock:
/// it is taken, and its stamp becomes the newest.
pub const STAMP_SKEW: u64 = 5_000;

/// A job for the dock to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Lay the islands out at this size ([`WHILE_DRAGGING`]).
    Layout(u16),
    /// Do the deferred work for this size ([`AT_REST`]).
    Settle(u16),
}

impl Act {
    pub fn redo(self) -> Redo {
        match self {
            Act::Layout(_) => WHILE_DRAGGING,
            Act::Settle(_) => AT_REST,
        }
    }
}

/// What the dock does after an event, and when to ask again.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Out {
    pub acts: Vec<Act>,
    /// Call [`Live::tick`] after this long (a layout held back to the next frame, or the rest check).
    pub wake: Option<Duration>,
}

/// What a settings file read does to the size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum File {
    /// A slider is (or just was) at work: keep the size it gave. The file may still hold the final value that never arrived as a
    /// message, so a look at it is asked for when the hold is over (`recheck`, set once until [`Live::recheck_fired`]).
    Keep { recheck: Option<Duration> },
    /// The file is the truth. When `settle` is set the sizes agree but the deferred work was never done: do it.
    Take { settle: bool },
}

/// The dock's state for sizes that arrive as messages.
pub struct Live {
    stamp: u64,
    wanted: u16,
    laid_at: u16,
    /// The islands were laid out since the deferred work was last done. (Not "the size differs": a drag that returns to where it began
    /// has still made the icons ask for pictures at the sizes in between.)
    dirty: bool,
    last_layout: Option<Instant>,
    idle_at: Option<Instant>,
    last_msg: Option<Instant>,
    /// The time of the timer already asked for, so a burst of steps asks once.
    armed: Option<Instant>,
    /// A look at the file is already asked for (at the end of the hold).
    recheck_asked: bool,
}

impl Live {
    /// A dock that has laid itself out at `at` and done all its work for it.
    pub fn new(at: u16) -> Live {
        let at = clamp(at);
        Live { stamp: 0, wanted: at, laid_at: at, dirty: false, last_layout: None, idle_at: None, last_msg: None, armed: None, recheck_asked: false }
    }

    /// The size the islands are laid out at now.
    pub fn laid_at(&self) -> u16 {
        self.laid_at
    }

    /// A message arrived. A step is laid out at once if the last layout was a frame ago, else when the frame is over; a release lays out
    /// whatever is missing and does the deferred work. An older message than one already seen is dropped.
    pub fn message(&mut self, now: Instant, m: Msg) -> Out {
        if m.stamp <= self.stamp && self.stamp - m.stamp <= STAMP_SKEW {
            return Out::default();
        }
        self.stamp = m.stamp;
        self.wanted = clamp(m.permille);
        self.last_msg = Some(now);
        let mut acts = Vec::new();
        if m.released {
            self.idle_at = None;
            self.lay_out(now, &mut acts);
            self.settle(&mut acts);
            return Out { acts, wake: None };
        }
        self.idle_at = Some(now + IDLE);
        if self.layout_due(now) {
            self.lay_out(now, &mut acts);
        }
        Out { acts, wake: self.next_wake(now) }
    }

    /// A timer asked for by an earlier [`Out::wake`] fired (it may have fired a little early or late; this is safe to call at any time).
    pub fn tick(&mut self, now: Instant) -> Out {
        // (the timer that brought us here is spent, early or not; if another is still on its way the worst case is one more tick,
        // which finds nothing to do)
        self.armed = None;
        let mut acts = Vec::new();
        if self.layout_due(now) {
            self.lay_out(now, &mut acts);
        }
        if self.idle_at.is_some_and(|i| now >= i) && self.laid_at == self.wanted {
            self.idle_at = None;
            self.settle(&mut acts);
        }
        Out { acts, wake: self.next_wake(now) }
    }

    /// The settings file was read and says `from_file`. While a slider is at work the file may hold an earlier step.
    pub fn file(&mut self, now: Instant, from_file: u16) -> File {
        if let Some(since) = self.last_msg.map(|t| now.saturating_duration_since(t)).filter(|since| *since < HOLD) {
            // (the file may be the one that is right and the message that carried its value lost: look again when the hold is over)
            let recheck = (!self.recheck_asked).then(|| {
                self.recheck_asked = true;
                HOLD - since + Duration::from_millis(50)
            });
            return File::Keep { recheck };
        }
        let from_file = clamp(from_file);
        let settle = self.laid_at == from_file && self.dirty;
        // (a size that differs from the layout is laid out and settled by the caller, as a change from the file always was)
        self.wanted = from_file;
        self.laid_at = from_file;
        self.dirty = false;
        self.idle_at = None;
        File::Take { settle }
    }

    /// The look at the file asked for by [`File::Keep`] is being made.
    pub fn recheck_fired(&mut self) {
        self.recheck_asked = false;
    }

    /// The islands were laid out for a size the deferred work has not been done for: a slider is at work. What the icons ask for waits
    /// while this is true (a task-list refresh in the middle of a drag must not ask for pictures at a size that is gone).
    pub fn dragging(&self) -> bool {
        self.dirty
    }

    fn layout_due(&self, now: Instant) -> bool {
        self.laid_at != self.wanted && self.last_layout.is_none_or(|t| now.saturating_duration_since(t) >= FRAME)
    }

    fn lay_out(&mut self, now: Instant, acts: &mut Vec<Act>) {
        if self.laid_at != self.wanted {
            acts.push(Act::Layout(self.wanted));
            self.laid_at = self.wanted;
            self.dirty = true;
            self.last_layout = Some(now);
        }
    }

    fn settle(&mut self, acts: &mut Vec<Act>) {
        if self.dirty {
            acts.push(Act::Settle(self.wanted));
            self.dirty = false;
        }
    }

    /// When the next timer is needed, unless one that fires early enough is already asked for.
    fn next_wake(&mut self, now: Instant) -> Option<Duration> {
        let mut at: Option<Instant> = None;
        let mut want = |t: Instant| at = Some(at.map_or(t, |a| a.min(t)));
        if self.laid_at != self.wanted {
            want(self.last_layout.map_or(now, |t| t + FRAME));
        }
        if let Some(i) = self.idle_at {
            want(i);
        }
        let at = at?;
        if self.armed.is_some_and(|a| a > now && a <= at) {
            return None;
        }
        self.armed = Some(at);
        Some(at.saturating_duration_since(now).max(Duration::from_millis(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(stamp: u64, permille: u16, released: bool) -> Msg {
        Msg { stamp, permille, released }
    }

    // ---- the range and the readings

    #[test]
    fn the_range_the_step_and_the_default() {
        assert_eq!((MIN, MAX, STEP, DEFAULT, POSITIONS), (750, 1500, 50, 1000, 15));
        assert_eq!(at_position(0), 750);
        assert_eq!(at_position(POSITIONS), 1500);
        assert_eq!(at_position(5), DEFAULT, "the default is a step of the slider");
        assert_eq!(at_position(200), 1500);
        for p in 0..=POSITIONS {
            assert_eq!(position_of(at_position(p)), p);
        }
        assert_eq!(factor(1000), 1.0);
        assert_eq!(factor(850), 0.85);
        assert_eq!(factor(1250), 1.25);
    }

    #[test]
    fn a_value_between_two_steps_shows_at_the_nearer_one_and_the_ends_cut() {
        assert_eq!(position_of(1000), 5);
        assert_eq!(position_of(1024), 5);
        assert_eq!(position_of(1025), 6);
        assert_eq!(position_of(1230), 10);
        assert_eq!(position_of(0), 0);
        assert_eq!(position_of(u16::MAX), POSITIONS);
    }

    #[test]
    fn numbers_load_as_numbers_and_what_is_outside_the_range_is_cut() {
        assert_eq!(from_number(1.0), 1000);
        assert_eq!(from_number(1.25), 1250);
        assert_eq!(from_number(1.23), 1230, "a number between two steps is kept as it is");
        assert_eq!(from_number(0.75), 750);
        assert_eq!(from_number(1.5), 1500);
        assert_eq!(from_number(0.7), 750);
        assert_eq!(from_number(0.0), 750, "zero is cut to the smallest size");
        assert_eq!(from_number(-0.0), 750);
        assert_eq!(from_number(-3.0), 750);
        assert_eq!(from_number(f64::NEG_INFINITY), 750);
        assert_eq!(from_number(1.51), 1500);
        assert_eq!(from_number(2.0), 1500);
        assert_eq!(from_number(1e300), 1500);
        assert_eq!(from_number(f64::INFINITY), 1500);
        assert_eq!(from_number(f64::NAN), DEFAULT, "NaN is nothing to cut: the default");
    }

    #[test]
    fn the_four_old_names_mean_what_they_always_meant() {
        assert_eq!(from_legacy_name("small"), Some(850));
        assert_eq!(from_legacy_name("medium"), Some(1000));
        assert_eq!(from_legacy_name("large"), Some(1200));
        assert_eq!(from_legacy_name("xlarge"), Some(1400));
        assert_eq!(from_legacy_name(" Large "), Some(1200));
        assert_eq!(from_legacy_name("gigantic"), None);
        assert_eq!(from_legacy_name(""), None);
        // the numbers are the old DockSize factors
        for (name, f) in [("small", 0.85f32), ("medium", 1.0), ("large", 1.2), ("xlarge", 1.4)] {
            assert_eq!(factor(from_legacy_name(name).unwrap()), f, "{name}");
            assert_eq!(legacy_name(from_legacy_name(name).unwrap()), Some(name));
        }
        assert_eq!(legacy_name(1250), None);
        assert_eq!(legacy_name(1001), None);
    }

    #[test]
    fn the_percent_text_is_the_value_in_whole_percent_in_each_language() {
        assert_eq!(percent_text(1000, true), "%100");
        assert_eq!(percent_text(1000, false), "100%");
        assert_eq!(percent_text(750, true), "%75");
        assert_eq!(percent_text(1500, true), "%150");
        assert_eq!(percent_text(850, true), "%85");
        assert_eq!(percent_text(1230, true), "%123");
        assert_eq!(percent_text(1235, true), "%124", "half a percent rounds up");
        assert_eq!(percent_text(1234, false), "123%");
    }

    // ---- the message and the outbox

    #[test]
    fn a_message_round_trips_and_a_wrong_line_is_nothing() {
        for m in [msg(1, 750, false), msg(1731234567890, 1000, true), msg(u64::MAX, 1500, false)] {
            assert_eq!(parse_message(&message(m)), Some(m));
        }
        assert_eq!(message(msg(7, 1250, true)), "size 7 1250 1");
        // a value outside the range is cut, not refused
        assert_eq!(parse_message("size 5 3000 0").map(|m| m.permille), Some(1500));
        assert_eq!(parse_message("size 5 10 0").map(|m| m.permille), Some(750));
        for bad in ["", "size", "size 5", "size 5 1000", "size 5 1000 2", "size x 1000 0", "size 5 -3 0", "size 5 1000 0 extra", "reload", "stop", "ping", "sizes 5 1000 0", "size 5 1e3 0"] {
            assert_eq!(parse_message(bad), None, "{bad:?}");
        }
        assert_eq!(parse_message("  size  5  1000  0  ").map(|m| m.stamp), Some(5));
    }

    #[test]
    fn the_outbox_keeps_the_newest_step_and_every_release() {
        let mut o = Outbox::default();
        for i in 1..=50u64 {
            o.push(msg(i, 750 + (i as u16 % 15) * 50, false));
        }
        assert_eq!(o.len(), 1, "fifty steps are one write");
        assert_eq!(o.pop().unwrap().stamp, 50);
        assert!(o.is_empty());
        // a release is never replaced by a later step
        o.push(msg(1, 900, false));
        o.push(msg(2, 900, true));
        o.push(msg(3, 950, false));
        o.push(msg(4, 1000, false));
        assert_eq!(o.len(), 2);
        assert_eq!(o.pop(), Some(msg(2, 900, true)));
        assert_eq!(o.pop(), Some(msg(4, 1000, false)));
        assert_eq!(o.pop(), None);
        // two releases are two messages
        o.push(msg(5, 1000, true));
        o.push(msg(6, 1000, true));
        assert_eq!(o.len(), 2);
    }

    // ---- what is redone when

    #[test]
    fn while_dragging_the_band_and_the_icons_wait_and_at_rest_only_they_are_done() {
        const { assert!(WHILE_DRAGGING.environment && WHILE_DRAGGING.windows && WHILE_DRAGGING.tray_strip && WHILE_DRAGGING.close_panels); }
        const { assert!(!WHILE_DRAGGING.reserved_band, "the band resizes other programs' windows"); }
        const { assert!(!WHILE_DRAGGING.icons, "icons asked for at a size that is gone when they arrive"); }
        const { assert!(AT_REST.reserved_band && AT_REST.icons); }
        const { assert!(!AT_REST.windows && !AT_REST.environment && !AT_REST.close_panels && !AT_REST.tray_strip, "nothing is laid out twice"); }
        // together they are the whole of what a size from the file always redid, and every piece is done by exactly one of the two
        let all = Redo { environment: true, windows: true, close_panels: true, tray_strip: true, reserved_band: true, icons: true };
        assert_eq!(FROM_FILE, all);
        for (d, r) in [
            (WHILE_DRAGGING.environment, AT_REST.environment),
            (WHILE_DRAGGING.windows, AT_REST.windows),
            (WHILE_DRAGGING.close_panels, AT_REST.close_panels),
            (WHILE_DRAGGING.tray_strip, AT_REST.tray_strip),
            (WHILE_DRAGGING.reserved_band, AT_REST.reserved_band),
            (WHILE_DRAGGING.icons, AT_REST.icons),
        ] {
            assert!(d != r, "a piece of work is done while dragging or at rest, not both and not neither");
        }
        assert_eq!(Act::Layout(1000).redo(), WHILE_DRAGGING);
        assert_eq!(Act::Settle(1000).redo(), AT_REST);
    }

    // ---- the dock's side

    fn live() -> (Live, Instant) {
        (Live::new(DEFAULT), Instant::now())
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_first_step_is_laid_out_at_once_and_nothing_else_is_done_until_rest() {
        let (mut l, t0) = live();
        let out = l.message(t0, msg(1, 1100, false));
        assert_eq!(out.acts, vec![Act::Layout(1100)]);
        assert_eq!(out.wake, Some(IDLE), "the rest check is the only thing to wait for");
        assert_eq!(l.laid_at(), 1100);
    }

    #[test]
    fn steps_inside_one_frame_are_one_layout_with_the_newest_value() {
        let (mut l, t0) = live();
        assert_eq!(l.message(t0, msg(1, 1100, false)).acts, vec![Act::Layout(1100)]);
        // five more steps in the next 10 ms: nothing laid out, the frame's end is asked for once
        let mut asked = 0;
        for (i, w) in [2u64, 4, 6, 8, 10].into_iter().enumerate() {
            let out = l.message(t0 + ms(w), msg(2 + i as u64, 1150 + 50 * i as u16, false));
            assert!(out.acts.is_empty(), "step at {w} ms");
            asked += usize::from(out.wake.is_some());
        }
        assert_eq!(asked, 1, "one timer for the whole burst");
        // the frame's end: the newest value, once
        let out = l.tick(t0 + FRAME);
        assert_eq!(out.acts, vec![Act::Layout(1350)]);
        // and not again
        assert!(l.tick(t0 + FRAME + ms(1)).acts.is_empty());
    }

    #[test]
    fn a_timer_that_fires_early_waits_the_rest_of_the_frame() {
        let (mut l, t0) = live();
        l.message(t0, msg(1, 1100, false));
        let out = l.message(t0 + ms(3), msg(2, 1200, false));
        assert_eq!(out.wake, Some(ms(13)));
        let early = l.tick(t0 + ms(10));
        assert!(early.acts.is_empty());
        assert_eq!(early.wake, Some(ms(6)), "the same frame's end, asked for again");
        assert_eq!(l.tick(t0 + ms(16)).acts, vec![Act::Layout(1200)]);
    }

    #[test]
    fn at_most_one_layout_a_frame_however_many_steps() {
        let (mut l, t0) = live();
        let mut layouts = 0;
        let mut stamp = 0;
        // 1000 steps, one every millisecond, for one second; the timers are served on time
        let mut due: Option<Instant> = None;
        for i in 0..1000u64 {
            let now = t0 + ms(i);
            stamp += 1;
            if due.is_some_and(|d| d <= now) {
                due = None;
                layouts += l.tick(now).acts.len();
            }
            let out = l.message(now, msg(stamp, 750 + (i as u16 % 16) * 50, false));
            layouts += out.acts.iter().filter(|a| matches!(a, Act::Layout(_))).count();
            if let Some(w) = out.wake {
                due = Some(due.map_or(now + w, |d: Instant| d.min(now + w)));
            }
        }
        assert!(layouts <= 1000 / 16 + 2, "{layouts} layouts in 1000 ms");
        assert!(layouts >= 1000 / 16 - 2, "{layouts}: the dock follows the slider, it does not freeze");
    }

    #[test]
    fn the_band_and_the_icons_wait_for_the_release() {
        let (mut l, t0) = live();
        l.message(t0, msg(1, 1100, false));
        l.message(t0 + ms(40), msg(2, 1300, false));
        let out = l.message(t0 + ms(80), msg(3, 1300, true));
        // the layout that was missing (the step at 40 ms was laid out at once, so none is) and then the deferred work, once
        assert_eq!(out.acts, vec![Act::Settle(1300)]);
        assert_eq!(out.wake, None);
        // nothing is left to do: the rest check finds nothing
        assert!(l.tick(t0 + ms(80) + IDLE).acts.is_empty());
    }

    #[test]
    fn a_release_inside_a_frame_lays_out_the_final_value_first() {
        let (mut l, t0) = live();
        l.message(t0, msg(1, 1100, false));
        l.message(t0 + ms(5), msg(2, 1400, false));
        let out = l.message(t0 + ms(6), msg(3, 1400, true));
        assert_eq!(out.acts, vec![Act::Layout(1400), Act::Settle(1400)]);
        // the frame's timer, still on its way, finds nothing
        assert!(l.tick(t0 + FRAME).acts.is_empty());
    }

    #[test]
    fn a_drag_that_comes_back_to_where_it_began_still_settles() {
        // the icons asked for pictures at the sizes in between: the end of the drag asks again at the size it ended at
        let (mut l, t0) = live();
        assert_eq!(l.message(t0, msg(1, 1300, false)).acts, vec![Act::Layout(1300)]);
        assert_eq!(l.message(t0 + ms(40), msg(2, DEFAULT, false)).acts, vec![Act::Layout(DEFAULT)]);
        assert_eq!(l.message(t0 + ms(80), msg(3, DEFAULT, true)).acts, vec![Act::Settle(DEFAULT)]);
        // and only once
        assert!(l.message(t0 + ms(90), msg(4, DEFAULT, true)).acts.is_empty());
    }

    #[test]
    fn a_release_that_changes_nothing_does_nothing() {
        let (mut l, t0) = live();
        // a key press on the size the dock already has
        let out = l.message(t0, msg(1, DEFAULT, true));
        assert!(out.acts.is_empty());
        assert_eq!(out.wake, None);
    }

    #[test]
    fn a_slider_that_rests_without_a_release_is_settled_by_the_rest_check() {
        let (mut l, t0) = live();
        let out = l.message(t0, msg(1, 1200, false));
        assert_eq!(out.acts, vec![Act::Layout(1200)]);
        let mut now = t0;
        let mut acts = Vec::new();
        let mut wake = out.wake;
        while let Some(w) = wake {
            now += w;
            let o = l.tick(now);
            acts.extend(o.acts);
            wake = o.wake;
            assert!(now - t0 < ms(2000), "the timers never end");
        }
        assert_eq!(acts, vec![Act::Settle(1200)]);
        assert!(now - t0 >= IDLE);
        // a late release for the same value has nothing left to do
        assert!(l.message(now + ms(1), msg(2, 1200, true)).acts.is_empty());
    }

    #[test]
    fn a_step_postpones_the_rest_check() {
        let (mut l, t0) = live();
        l.message(t0, msg(1, 1100, false));
        l.message(t0 + ms(500), msg(2, 1200, false));
        // the first rest check fires at 600 ms: the slider moved at 500, so it is not at rest
        let o = l.tick(t0 + IDLE);
        assert!(o.acts.is_empty(), "{o:?}");
        assert_eq!(o.wake, Some(ms(500)));
        assert_eq!(l.tick(t0 + ms(500) + IDLE).acts, vec![Act::Settle(1200)]);
    }

    #[test]
    fn the_rest_check_waits_for_a_layout_that_is_still_held_back() {
        let (mut l, t0) = live();
        l.message(t0, msg(1, 1100, false));
        l.message(t0 + ms(2), msg(2, 1200, false));
        // a very late tick: the layout first, the settling at the same time or after, never the other way round
        let out = l.tick(t0 + ms(5000));
        assert_eq!(out.acts, vec![Act::Layout(1200), Act::Settle(1200)]);
    }

    #[test]
    fn an_older_message_that_arrives_late_is_dropped() {
        let (mut l, t0) = live();
        l.message(t0, msg(10, 1200, false));
        assert!(l.message(t0 + ms(1), msg(9, 800, false)).acts.is_empty(), "older");
        assert!(l.message(t0 + ms(1), msg(10, 800, false)).acts.is_empty(), "the same stamp twice");
        // the release came first, then a step that was sent before it: the step must not undo the release
        let mut l = Live::new(DEFAULT);
        assert_eq!(l.message(t0, msg(20, 1300, true)).acts, vec![Act::Layout(1300), Act::Settle(1300)]);
        let late = l.message(t0 + ms(1), msg(19, 1250, false));
        assert!(late.acts.is_empty() && late.wake.is_none());
        assert_eq!(l.laid_at(), 1300);
    }

    #[test]
    fn a_file_read_during_a_drag_does_not_overrule_the_slider() {
        let (mut l, t0) = live();
        l.message(t0, msg(1, 1200, false));
        // the file holds an earlier step of the drag
        assert!(matches!(l.file(t0 + ms(300), 1100), File::Keep { .. }));
        // and just after the release too (the file is saved a moment after it)
        l.message(t0 + ms(400), msg(2, 1200, true));
        assert!(matches!(l.file(t0 + ms(400) + ms(1400), 1100), File::Keep { .. }));
        // later the file is the truth again
        assert_eq!(l.file(t0 + ms(400) + HOLD, 1100), File::Take { settle: false });
        assert_eq!(l.laid_at(), 1100);
    }

    #[test]
    fn a_file_without_any_drag_is_taken_as_it_always_was() {
        let (mut l, t0) = live();
        assert_eq!(l.file(t0, 1400), File::Take { settle: false });
        assert_eq!(l.laid_at(), 1400);
        // the same value again: nothing to do, and no deferred work pending
        assert_eq!(l.file(t0 + ms(10), 1400), File::Take { settle: false });
        // a file with a value outside the range is cut
        assert_eq!(l.file(t0 + ms(20), 9000), File::Take { settle: false });
        assert_eq!(l.laid_at(), MAX);
    }

    #[test]
    fn deferred_work_that_was_never_done_is_done_when_the_file_arrives() {
        let (mut l, t0) = live();
        l.message(t0, msg(1, 1200, false));
        // no release, no rest check (the timers were lost) - a file read long after finds the sizes equal and the work pending
        assert_eq!(l.file(t0 + ms(5000), 1200), File::Take { settle: true });
        assert_eq!(l.file(t0 + ms(5001), 1200), File::Take { settle: false }, "once");
    }

    #[test]
    fn a_sizeless_start_is_clamped() {
        assert_eq!(Live::new(0).laid_at(), MIN);
        assert_eq!(Live::new(u16::MAX).laid_at(), MAX);
    }

    #[test]
    fn the_islands_never_overlap_at_any_slider_position_on_the_narrow_screens() {
        // The widest the side islands get (the left one with its weather, the right one with a full tray strip) at every position of the
        // slider, cut to what the screen can hold. 1366 px is the narrowest screen the dock is planned for; 1280 and 1024 are checked
        // for the sides too (the middle island has always been the one to give way there).
        use crate::dock::layout::{self, MIN_GAP, Rect, Widths, px};
        use crate::dock::size::unit;
        use crate::dock::tray_strip;
        for width in [1024i32, 1280, 1366, 1440, 1920, 2560] {
            let work = Rect::new(0, 0, width, 720);
            let logical = width as f32;
            let widest_right = RIGHT_BASE + tray_strip::strip_limit(logical) + 2.0 * tray_strip::SEP_GAP + tray_strip::SEP_W;
            for p in 0..=POSITIONS {
                let fitted = fit_to_width(at_position(p), logical);
                assert!(fitted <= at_position(p) && fitted >= MIN, "{width} px, position {p}");
                let u = unit(1.0, factor(fitted));
                for icons in [4usize, 14] {
                    let middle = layout::middle_width(&vec![44.0; icons], 1);
                    let i = layout::islands(work, u, Widths { left: WIDEST_LEFT, middle, right: widest_right });
                    assert!(i.left.right + px(MIN_GAP, u) <= i.right.left, "{width} px, position {p} ({fitted}), {icons} icons: the side islands meet: {i:?}");
                    if i.middle.width() > 0 {
                        assert!(i.left.right + px(MIN_GAP, u) <= i.middle.left, "{width} px, position {p}: {i:?}");
                        assert!(i.middle.right + px(MIN_GAP, u) <= i.right.left, "{width} px, position {p}: {i:?}");
                    }
                    assert!(work.covers(&i.left) && work.covers(&i.right) && work.covers(&i.middle), "{width} px, position {p}: inside the screen");
                }
            }
        }
    }

    #[test]
    fn what_the_screen_cuts_and_what_it_leaves() {
        // 1366 px: more than the old %140 fits, so the four old sizes are untouched there
        for p in [850u16, 1000, 1200, 1400] {
            assert_eq!(fit_to_width(p, 1366.0), p, "{p} at 1366");
        }
        let cut = fit_to_width(MAX, 1366.0);
        assert!((1400..1450).contains(&cut), "{cut}");
        assert_eq!(fit_to_width(1450, 1366.0), cut.min(1450));
        // a wide screen cuts nothing, at any size of the range
        for p in 0..=POSITIONS {
            assert_eq!(fit_to_width(at_position(p), 1920.0), at_position(p));
        }
        // a very narrow screen goes down to the smallest size and no further
        assert_eq!(fit_to_width(MAX, 300.0), MIN);
        // nonsense about the screen cuts nothing
        assert_eq!(fit_to_width(1300, f32::NAN), 1300);
        assert_eq!(fit_to_width(1300, 0.0), 1300);
        assert_eq!(fit_to_width(1300, -5.0), 1300);
        // and the size itself is cut to the range first
        assert_eq!(fit_to_width(60_000, 5000.0), MAX);
        assert_eq!(fit_to_width(0, 5000.0), MIN);
    }

    #[test]
    fn what_a_redo_performs_is_listed_in_order_and_the_dock_runs_exactly_that() {
        assert_eq!(WHILE_DRAGGING.steps(), vec![Step::Environment, Step::ClosePanels, Step::TrayStrip, Step::Windows]);
        assert_eq!(AT_REST.steps(), vec![Step::ReservedBand, Step::Icons]);
        assert_eq!(FROM_FILE.steps().len(), 6);
        // the acts map to those lists (the dock performs `act.redo().steps()`)
        assert_eq!(Act::Layout(1100).redo().steps(), WHILE_DRAGGING.steps());
        assert_eq!(Act::Settle(1100).redo().steps(), AT_REST.steps());
        // a step of a drag never reaches the other programs' windows or the shell for icons
        assert!(!Act::Layout(1100).redo().steps().iter().any(|s| matches!(s, Step::ReservedBand | Step::Icons)));
    }

    #[test]
    fn a_run_of_key_steps_is_one_settling_after_the_rest() {
        // holding an arrow key: a step every 30 ms for a second, none of them a release
        let (mut l, t0) = live();
        let (mut settles, mut layouts) = (0, 0);
        let mut due: Option<Instant> = None;
        let count = |acts: &[Act], settles: &mut usize, layouts: &mut usize| {
            *settles += acts.iter().filter(|a| matches!(a, Act::Settle(_))).count();
            *layouts += acts.iter().filter(|a| matches!(a, Act::Layout(_))).count();
        };
        for i in 0..34u64 {
            let now = t0 + ms(i * 30);
            if due.is_some_and(|d| d <= now) {
                let o = l.tick(now);
                count(&o.acts, &mut settles, &mut layouts);
                due = o.wake.map(|w| now + w);
            }
            let o = l.message(now, msg(1 + i, 1000 + 10 * (i as u16 % 40) + 10, false));
            count(&o.acts, &mut settles, &mut layouts);
            if let Some(w) = o.wake {
                due = Some(due.map_or(now + w, |d: Instant| d.min(now + w)));
            }
        }
        assert_eq!(settles, 0, "nothing is settled while the keys keep coming");
        assert!(layouts >= 30, "{layouts}");
        // the keys stop: the band and the icons are done once, by the rest check
        let mut now = t0 + ms(34 * 30);
        while let Some(d) = due.take() {
            now = now.max(d);
            let o = l.tick(now);
            count(&o.acts, &mut settles, &mut layouts);
            if let Some(w) = o.wake {
                due = Some(now + w);
            }
        }
        assert_eq!(settles, 1);
    }

    #[test]
    fn a_clock_that_went_back_does_not_silence_the_slider() {
        let (mut l, t0) = live();
        // a settings window whose clock was far ahead
        assert_eq!(l.message(t0, msg(5_000_000_000_000, 1200, true)).acts, vec![Act::Layout(1200), Act::Settle(1200)]);
        // the clock is put right and the next message is from a smaller stamp: it is taken, not dropped for ever
        let out = l.message(t0 + ms(30), msg(1_000_000_000_000, 1300, true));
        assert_eq!(out.acts, vec![Act::Layout(1300), Act::Settle(1300)]);
        // the messages that follow count from the new stamp, and an overtaken one is still dropped
        assert!(l.message(t0 + ms(40), msg(1_000_000_000_000 - 3, 800, false)).acts.is_empty());
        assert_eq!(l.message(t0 + ms(50), msg(1_000_000_000_001, 1400, false)).acts, vec![Act::Layout(1400)]);
    }

    #[test]
    fn a_file_that_was_kept_is_looked_at_again_when_the_hold_is_over() {
        let (mut l, t0) = live();
        l.message(t0, msg(1, 1200, false));
        // the file holds an earlier step (the release and the last step never arrived): kept, and a look is asked for at the hold's end
        let File::Keep { recheck: Some(after) } = l.file(t0 + ms(400), 1100) else { panic!("a look at the file should be asked for") };
        assert!(after >= HOLD - ms(400));
        // asked for once, however many reads land in the meantime
        assert_eq!(l.file(t0 + ms(500), 1100), File::Keep { recheck: None });
        // the look is made at the hold's end and the file is the truth again
        l.recheck_fired();
        assert_eq!(l.file(t0 + ms(400) + after, 1250), File::Take { settle: false });
        assert_eq!(l.laid_at(), 1250);
    }

    #[test]
    fn a_dock_is_dragging_between_the_first_layout_and_the_settling() {
        let (mut l, t0) = live();
        assert!(!l.dragging());
        l.message(t0, msg(1, 1100, false));
        assert!(l.dragging(), "icons wait");
        l.message(t0 + ms(40), msg(2, 1100, true));
        assert!(!l.dragging(), "the deferred work is handed out");
    }
}
