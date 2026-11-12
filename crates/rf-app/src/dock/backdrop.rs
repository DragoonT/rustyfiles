//! The glass behind one island: attaching it with the island's own radius and corner cutting, watching what the engine reports,
//! telling the island when to paint its own fill instead, and trying again after a failure.
//!
//! The decisions are in [`Machine`] (pure: the engine's report and the time are arguments); [`IslandGlass`] is the thin layer that
//! makes the calls into `rf_win::glass`. It also holds the island's [`Tone`]: whether the foreground is light or dark, which follows
//! the brightness of what is behind the glass (`rf_win::glass::backdrop_luma`) and gives the engine the matching glass tint.

use std::sync::LazyLock;
use std::time::Instant;

use rf_win::dock::layout::RADIUS;
use rf_win::glass::{GlassOptions, GlassState};

use super::look::Palette;
use super::tone::Tone;

/// The clock of the islands' easing: milliseconds since the first look at it. One for the whole process, so the views and the host agree.
pub fn now_ms() -> u64 {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed().as_millis() as u64
}

/// How long to wait before trying again after the 1st, 2nd and 3rd failure. After the third retry fails the island keeps its own fill
/// for good (until the dock is restarted): a glass that cannot be kept should not be retried forever.
pub const RETRY_AFTER_MS: [u64; 3] = [5_000, 30_000, 120_000];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Attached, the engine has not drawn yet.
    Starting,
    /// The glass is on the screen.
    Running,
    /// Gone wrong at this time (ms on the caller's clock); waiting to retry.
    Failed { at_ms: u64 },
}

/// What the caller has to do after looking at the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Nothing,
    /// The engine gave up (or let go): detach the window from it, log the reason. The island paints its own fill meanwhile.
    GiveUp(String),
    /// The wait is over: attach again.
    Retry,
}

#[derive(Clone, Debug)]
pub struct Machine {
    pub phase: Phase,
    /// How many retries have been made.
    pub retries: usize,
}

impl Default for Machine {
    fn default() -> Self {
        Machine { phase: Phase::Starting, retries: 0 }
    }
}

impl Machine {
    /// Whether the island must paint its own fill: everything but a running glass (starting, failed, waiting to retry).
    pub fn fallback(&self) -> bool {
        self.phase != Phase::Running
    }

    /// Whether another look at the engine can still change anything soon (a running glass is looked at slowly, a failed one only
    /// when a retry is due).
    pub fn settled(&self) -> bool {
        matches!(self.phase, Phase::Running)
    }

    /// The attach itself failed (the window has no handle, the engine thread did not start): the same as a failure reported later.
    pub fn attach_failed(&mut self, now_ms: u64) {
        self.phase = Phase::Failed { at_ms: now_ms };
    }

    /// One look at the engine's report (`state` is what `rf_win::glass::state` says) at `now_ms`.
    pub fn observe(&mut self, engine: &GlassState, now_ms: u64) -> Act {
        match self.phase {
            Phase::Failed { at_ms } => match RETRY_AFTER_MS.get(self.retries) {
                Some(&wait) if now_ms.saturating_sub(at_ms) >= wait => {
                    self.retries += 1;
                    self.phase = Phase::Starting;
                    Act::Retry
                }
                _ => Act::Nothing,
            },
            Phase::Starting | Phase::Running => match engine {
                GlassState::Running => {
                    self.phase = Phase::Running;
                    Act::Nothing
                }
                GlassState::Starting => Act::Nothing,
                GlassState::Failed(why) => {
                    self.phase = Phase::Failed { at_ms: now_ms };
                    Act::GiveUp(why.clone())
                }
                // taken off from elsewhere: no glass, whatever the reason
                GlassState::Off => {
                    self.phase = Phase::Failed { at_ms: now_ms };
                    Act::GiveUp("the engine no longer holds this window".into())
                }
            },
        }
    }
}

/// The corner radius of the island's glass in device px.
pub fn radius_px(scale: f32) -> f32 {
    RADIUS * scale.max(0.1)
}

/// The glass options of an island: its own radius and its own corners cut.
pub fn options(dark: bool, scale: f32) -> GlassOptions {
    GlassOptions { dark, radius_px: Some(radius_px(scale)), cut_corners: true }
}

/// One island's glass. Shared (`Rc<RefCell<..>>`) between the island's view, which reads `fallback()` while it paints, and the host,
/// which starts it, watches it and stops it.
pub struct IslandGlass {
    hwnd: isize,
    machine: Machine,
    /// Light or dark foreground and glass tint (starts from the system theme).
    tone: Tone,
    scale: f32,
    /// The tint the engine was last given (`true` is the dark glass).
    sent_dark: bool,
    /// Whether the engine has been told about the window (and so must be told to let go of it).
    attached: bool,
}

impl IslandGlass {
    pub fn new() -> IslandGlass {
        IslandGlass { hwnd: 0, machine: Machine::default(), tone: Tone::new(true), scale: 1.0, sent_dark: true, attached: false }
    }

    /// True while the island has to paint its own fill.
    pub fn fallback(&self) -> bool {
        self.machine.fallback()
    }

    pub fn settled(&self) -> bool {
        self.machine.settled()
    }

    /// The colours the island paints with at `now_ms`: the system's while the glass is not running, otherwise those that suit what
    /// is behind it, eased from one look to the other.
    pub fn palette(&self, now_ms: u64) -> Palette {
        Palette { mix: self.tone.mix(now_ms) }
    }

    /// Whether the foreground has stopped changing (the island needs no more frames for it).
    pub fn tone_settled(&self, now_ms: u64) -> bool {
        self.tone.settled(now_ms)
    }

    /// Looks at how bright the backdrop is (a few times a second, from the host) and moves the foreground and the glass tint if the
    /// page behind went across a line. True when the island has to be drawn again; false and nothing else when nothing changed.
    pub fn poll_tone(&mut self, now_ms: u64) -> bool {
        // a glass that is not running measures nothing: the system's theme decides, as for the fallback fill
        let luma = if self.attached && !self.machine.fallback() { rf_win::glass::backdrop_luma(self.hwnd) } else { None };
        let changed = self.tone.feed(luma, now_ms);
        if changed {
            self.send_tint();
        }
        changed
    }

    /// The engine is told the tint when it differs from the one it has.
    fn send_tint(&mut self) {
        let dark = self.tone.glass_dark();
        if self.attached && dark != self.sent_dark {
            rf_win::glass::set_dark(self.hwnd, dark);
            self.sent_dark = dark;
        }
    }

    /// Gives the window its glass. A window that cannot get one is not an error: the island paints its own fill.
    pub fn start(&mut self, hwnd: isize, dark: bool, scale: f32, now_ms: u64) {
        self.hwnd = hwnd;
        self.tone = Tone::new(dark);
        self.scale = scale;
        self.machine = Machine::default();
        self.attach(now_ms);
    }

    fn attach(&mut self, now_ms: u64) {
        if self.hwnd == 0 {
            self.machine.attach_failed(now_ms);
            return;
        }
        let dark = self.tone.glass_dark();
        if rf_win::glass::attach_with(self.hwnd, options(dark, self.scale)) {
            self.sent_dark = dark;
            self.attached = true;
        } else {
            let why = match rf_win::glass::state(self.hwnd) {
                GlassState::Failed(why) => why,
                _ => "the glass engine did not accept the window".into(),
            };
            log::warn!("dock glass: {why}");
            self.release();
            self.machine.attach_failed(now_ms);
        }
    }

    /// The theme or the monitor's scale changed: the engine is told only what is new. The theme decides the tint only while the
    /// brightness behind the glass is not known (see `tone.rs`).
    pub fn sync(&mut self, dark: bool, scale: f32, now_ms: u64) {
        let scale_changed = (scale - self.scale).abs() > 0.001;
        self.scale = scale;
        let known = self.attached && !self.machine.fallback() && rf_win::glass::backdrop_luma(self.hwnd).is_some();
        self.tone.set_system(dark, known, now_ms);
        if !self.attached {
            return;
        }
        if scale_changed {
            // the radius is in device px: it follows the DPI (this also carries the tint)
            let tint = self.tone.glass_dark();
            rf_win::glass::set_options(self.hwnd, options(tint, scale));
            self.sent_dark = tint;
        } else {
            self.send_tint();
        }
    }

    /// One look at the engine. True when the island's picture has to change (its fill appeared or went).
    pub fn tick(&mut self, now_ms: u64) -> bool {
        let before = self.machine.fallback();
        let engine = if self.attached { rf_win::glass::state(self.hwnd) } else { GlassState::Off };
        // a window we never managed to attach is "off" for the machine, but that is a failure it already knows
        let act = if self.attached || matches!(self.machine.phase, Phase::Failed { .. }) { self.machine.observe(&engine, now_ms) } else { Act::Nothing };
        match act {
            Act::Nothing => {}
            Act::GiveUp(why) => {
                log::warn!("dock glass: giving up on this island for now, it paints its own fill: {why}");
                self.release();
            }
            Act::Retry => {
                log::info!("dock glass: trying again");
                self.attach(now_ms);
            }
        }
        before != self.machine.fallback()
    }

    /// Takes the glass off the window (the capture exclusion goes with it). Safe to call twice, and without glass.
    pub fn release(&mut self) {
        if self.attached {
            rf_win::glass::detach(self.hwnd);
            self.attached = false;
        }
    }
}

impl Default for IslandGlass {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for IslandGlass {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_with_the_fallback_fill_and_loses_it_when_the_glass_runs() {
        let mut m = Machine::default();
        assert!(m.fallback(), "while starting the island paints its own fill");
        assert_eq!(m.observe(&GlassState::Starting, 10), Act::Nothing);
        assert!(m.fallback());
        assert_eq!(m.observe(&GlassState::Running, 60), Act::Nothing);
        assert!(!m.fallback());
        assert!(m.settled());
    }

    #[test]
    fn a_failure_brings_the_fill_back_and_says_why_once() {
        let mut m = Machine::default();
        m.observe(&GlassState::Running, 0);
        let act = m.observe(&GlassState::Failed("lost the device".into()), 1_000);
        assert_eq!(act, Act::GiveUp("lost the device".into()));
        assert!(m.fallback());
        // the next looks (the window is detached, so the engine says Off) do not report it again
        assert_eq!(m.observe(&GlassState::Off, 1_500), Act::Nothing);
        assert_eq!(m.observe(&GlassState::Off, 2_000), Act::Nothing);
    }

    #[test]
    fn the_engine_letting_go_is_a_failure_too() {
        let mut m = Machine::default();
        m.observe(&GlassState::Running, 0);
        assert!(matches!(m.observe(&GlassState::Off, 10), Act::GiveUp(_)));
        assert!(m.fallback());
    }

    #[test]
    fn it_retries_after_the_waits_and_then_stops_trying() {
        let mut m = Machine::default();
        m.attach_failed(0);
        assert_eq!(m.observe(&GlassState::Off, 4_999), Act::Nothing);
        assert_eq!(m.observe(&GlassState::Off, 5_000), Act::Retry);
        assert!(m.fallback(), "still no glass until the engine says so");
        // fails again
        assert!(matches!(m.observe(&GlassState::Failed("x".into()), 5_100), Act::GiveUp(_)));
        assert_eq!(m.observe(&GlassState::Off, 5_100 + 29_999), Act::Nothing);
        assert_eq!(m.observe(&GlassState::Off, 5_100 + 30_000), Act::Retry);
        assert!(matches!(m.observe(&GlassState::Failed("y".into()), 40_000), Act::GiveUp(_)));
        assert_eq!(m.observe(&GlassState::Off, 40_000 + 120_000), Act::Retry);
        assert!(matches!(m.observe(&GlassState::Failed("z".into()), 170_000), Act::GiveUp(_)));
        // three retries are all there are
        assert_eq!(m.observe(&GlassState::Off, 10_000_000), Act::Nothing);
        assert!(m.fallback());
        assert_eq!(m.retries, 3);
    }

    #[test]
    fn a_retry_that_works_ends_the_fallback() {
        let mut m = Machine::default();
        m.attach_failed(0);
        assert_eq!(m.observe(&GlassState::Off, 6_000), Act::Retry);
        assert_eq!(m.observe(&GlassState::Starting, 6_050), Act::Nothing);
        assert_eq!(m.observe(&GlassState::Running, 6_100), Act::Nothing);
        assert!(!m.fallback());
    }

    #[test]
    fn an_island_uses_its_own_radius_and_cuts_its_own_corners() {
        let o = options(true, 1.5);
        assert!(o.dark);
        assert_eq!(o.radius_px, Some(36.0));
        assert!(o.cut_corners);
        assert!(!options(false, 1.0).dark);
        // a broken scale cannot make the radius zero or negative
        assert!(radius_px(0.0) > 0.0);
        assert!(radius_px(-3.0) > 0.0);
    }

    #[test]
    fn an_island_with_no_glass_has_the_systems_look_and_follows_its_theme() {
        let mut g = IslandGlass::new();
        g.start(0, true, 1.0, 0);
        assert_eq!(g.palette(0).mix, 0.0, "dark system: the dark look");
        assert!(g.tone_settled(0));
        assert!(!g.poll_tone(10), "nothing is measured without glass: no change, no redraw");
        assert!(!g.poll_tone(260));
        // the user switches the system to the light theme: the look turns over 300 ms
        g.sync(false, 1.0, 1_000);
        assert!(!g.tone_settled(1_100));
        assert!(g.palette(1_150).mix > 0.4 && g.palette(1_150).mix < 0.6);
        assert_eq!(g.palette(1_300).mix, 1.0);
        assert!(g.tone_settled(1_300));
        // a restart of the island starts from the system again
        g.start(0, false, 1.0, 0);
        assert_eq!(g.palette(0).mix, 1.0);
    }

    #[test]
    fn the_clock_of_the_easing_only_goes_forward() {
        let a = now_ms();
        let b = now_ms();
        assert!(b >= a);
    }

    #[test]
    fn a_window_without_a_handle_is_a_fallback_not_a_crash() {
        let mut g = IslandGlass::new();
        g.start(0, true, 1.0, 0);
        assert!(g.fallback());
        assert!(!g.tick(10), "nothing changes");
        g.release();
        g.release();
    }
}
