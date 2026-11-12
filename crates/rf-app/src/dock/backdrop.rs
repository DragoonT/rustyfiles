//! The glass behind one island: attaching it with the island's own radius and corner cutting, watching what the engine reports,
//! telling the island when to paint its own fill instead, and trying again after a failure.
//!
//! The decisions are in [`Machine`] (pure: the engine's report and the time are arguments); [`IslandGlass`] is the thin layer that
//! makes the calls into `rf_win::glass`.

use rf_win::dock::layout::RADIUS;
use rf_win::glass::{GlassOptions, GlassState};

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
    dark: bool,
    scale: f32,
    /// Whether the engine has been told about the window (and so must be told to let go of it).
    attached: bool,
}

impl IslandGlass {
    pub fn new() -> IslandGlass {
        IslandGlass { hwnd: 0, machine: Machine::default(), dark: true, scale: 1.0, attached: false }
    }

    /// True while the island has to paint its own fill.
    pub fn fallback(&self) -> bool {
        self.machine.fallback()
    }

    pub fn settled(&self) -> bool {
        self.machine.settled()
    }

    /// Gives the window its glass. A window that cannot get one is not an error: the island paints its own fill.
    pub fn start(&mut self, hwnd: isize, dark: bool, scale: f32, now_ms: u64) {
        self.hwnd = hwnd;
        self.dark = dark;
        self.scale = scale;
        self.machine = Machine::default();
        self.attach(now_ms);
    }

    fn attach(&mut self, now_ms: u64) {
        if self.hwnd == 0 {
            self.machine.attach_failed(now_ms);
            return;
        }
        if rf_win::glass::attach_with(self.hwnd, options(self.dark, self.scale)) {
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

    /// The theme or the monitor's scale changed: the engine is told only what is new.
    pub fn sync(&mut self, dark: bool, scale: f32) {
        let (dark_changed, scale_changed) = (dark != self.dark, (scale - self.scale).abs() > 0.001);
        self.dark = dark;
        self.scale = scale;
        if !self.attached {
            return;
        }
        if scale_changed {
            // the radius is in device px: it follows the DPI (this also carries the new tint)
            rf_win::glass::set_options(self.hwnd, options(dark, scale));
        } else if dark_changed {
            rf_win::glass::set_dark(self.hwnd, dark);
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
    fn a_window_without_a_handle_is_a_fallback_not_a_crash() {
        let mut g = IslandGlass::new();
        g.start(0, true, 1.0, 0);
        assert!(g.fallback());
        assert!(!g.tick(10), "nothing changes");
        g.release();
        g.release();
    }
}
