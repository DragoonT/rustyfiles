//! Watching the glass engine for a surface of ours that sits on it (a dock island, a popup menu): the small state machine that decides
//! between "the glass is there" and "paint a plain fill of your own", and the popup's own use of it.
//!
//! The decisions are in [`Machine`] (pure: the engine's report and the time are arguments). The dock's islands wrap it in
//! `dock::backdrop::IslandGlass` (with retries); [`PopupGlass`] is the popup's: it attaches the popup window once, watches the engine
//! and, when the engine fails, keeps the plain fill until the popup closes (a popup is short-lived, a retry would only flicker).

use std::time::{Duration, Instant};

use gpui_kit::{Context, Task};
use rf_win::glass::GlassState;

/// How long to wait before trying again after the 1st, 2nd and 3rd failure. After the third retry fails the surface keeps its own fill
/// for good (the dock's island until the dock is restarted, a popup until it closes): a glass that cannot be kept should not be
/// retried forever.
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
    /// The engine gave up (or let go): detach the window from it, log the reason. The surface paints its own fill meanwhile.
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
    /// Whether the surface must paint its own fill: everything but a running glass (starting, failed, waiting to retry).
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

/// What a popup paints over the glass and what it paints until the glass is there. Over the glass it is only a wash (the picture
/// behind is blurred under it, and the sheet's menus are dense frosted glass, not clear): black at 28 % in the dark theme, white at
/// 45 % in the light one. While there is no glass (starting, failed) it is a plain panel, dark `0x1C1C1E` at 82 % and light
/// `0xF5F5F7` at 86 %: a popup is never see-through with nothing behind it.
pub fn panel_fill(dark: bool, fallback: bool) -> gpui_kit::Hsla {
    let (rgb, a) = match (dark, fallback) {
        (true, true) => (0x1C1C1E, 0.82),
        (false, true) => (0xF5F5F7, 0.86),
        (true, false) => (0x000000, 0.28),
        (false, false) => (0xFFFFFF, 0.45),
    };
    let mut c: gpui_kit::Hsla = gpui_kit::rgb(rgb).into();
    c.a = a;
    c
}

/// The glass behind one popup window (menu level, dropdown list, flyout). Lives in the popup's view: dropping it, or [`release`],
/// takes the glass off the window.
///
/// [`release`]: PopupGlass::release
pub struct PopupGlass {
    hwnd: isize,
    machine: Machine,
    started: Instant,
    /// The engine has been told about the window (and must be told to let go of it).
    attached: bool,
    /// Failed or never attached: the popup keeps its plain fill and the engine is not asked again.
    gave_up: bool,
}

impl PopupGlass {
    /// Marks the window as a liquid popup (its controls take the liquid look), asks for the system's small round corners and attaches
    /// the glass. A window that cannot get glass is not an error: [`fallback`](Self::fallback) stays true.
    pub fn start(hwnd: isize, dark: bool) -> PopupGlass {
        Self::start_with(hwnd, dark, false)
    }

    /// [`start`](Self::start) for a large panel (the dock's Start and control centre): the system's ordinary round corners (8 px at
    /// 100 %, what Windows' own Start has) instead of the small ones a menu gets.
    pub fn start_panel(hwnd: isize, dark: bool) -> PopupGlass {
        Self::start_with(hwnd, dark, true)
    }

    /// [`start_panel`](Self::start_panel) for a small panel that wants a THIN edge: the glass's rim band is at most `rim_cap` CSS px
    /// wide (`rf_win::glass::set_rim_cap`, set before the window is attached; it goes when the glass is released).
    pub fn start_panel_thin_rim(hwnd: isize, dark: bool, rim_cap: f32) -> PopupGlass {
        rf_win::glass::set_rim_cap(hwnd, Some(rim_cap));
        Self::start_with(hwnd, dark, true)
    }

    fn start_with(hwnd: isize, dark: bool, round: bool) -> PopupGlass {
        let mut g = PopupGlass { hwnd, machine: Machine::default(), started: Instant::now(), attached: false, gave_up: false };
        if hwnd == 0 {
            g.machine.attach_failed(0);
            g.gave_up = true;
            return g;
        }
        crate::glass::set_liquid_popup(hwnd, true);
        if round {
            rf_win::window::set_round_corners(hwnd);
        } else {
            rf_win::window::set_small_corners(hwnd);
        }
        if rf_win::glass::attach(hwnd, dark) {
            g.attached = true;
        } else {
            let why = match rf_win::glass::state(hwnd) {
                GlassState::Failed(why) => why,
                _ => "the glass engine did not accept the window".into(),
            };
            log::warn!("popup glass: {why}");
            rf_win::glass::detach(hwnd); // clears the failed entry
            g.machine.attach_failed(0);
            g.gave_up = true;
        }
        g
    }

    /// True while the popup has to paint its plain fill (the glass is starting, failed or never was).
    pub fn fallback(&self) -> bool {
        self.machine.fallback()
    }

    /// Nothing more to watch (the glass failed for good).
    pub fn done(&self) -> bool {
        self.gave_up
    }

    /// The glass runs: it is looked at slowly (a lost device is noticed within half a second); before that, quickly.
    pub fn settled(&self) -> bool {
        self.machine.settled()
    }

    /// One look at the engine. True when the popup's picture has to change (its fill appeared or went).
    pub fn tick(&mut self) -> bool {
        if self.gave_up || !self.attached {
            return false;
        }
        let before = self.machine.fallback();
        let now_ms = self.started.elapsed().as_millis() as u64;
        match self.machine.observe(&rf_win::glass::state(self.hwnd), now_ms) {
            Act::GiveUp(why) => {
                log::warn!("popup glass: giving up, the popup keeps its own fill: {why}");
                self.release_glass();
                self.gave_up = true;
            }
            // (a popup never waits for a retry: it has given up above, for good)
            Act::Retry | Act::Nothing => {}
        }
        before != self.machine.fallback()
    }

    fn release_glass(&mut self) {
        if self.attached {
            rf_win::glass::detach(self.hwnd);
            self.attached = false;
        }
    }

    /// Takes the glass off the window and the liquid mark with it. Safe to call twice, and without glass.
    pub fn release(&mut self) {
        self.release_glass();
        if self.hwnd != 0 {
            crate::glass::set_liquid_popup(self.hwnd, false);
        }
    }
}

impl Drop for PopupGlass {
    fn drop(&mut self) {
        self.release();
    }
}

/// How often a popup looks at the engine: quickly until the glass runs, then slowly.
pub fn poll_every(settled: bool) -> Duration {
    Duration::from_millis(if settled { 500 } else { 50 })
}

/// A background task of a popup's view that watches its glass and redraws the view when its fill appears or goes. It ends when the
/// glass is given up or the view is gone (dropping the task stops it too).
pub fn watch<T: 'static>(cx: &mut Context<T>, glass: fn(&mut T) -> Option<&mut PopupGlass>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            let Ok((more, settled)) = this.update(cx, |t, cx| match glass(t) {
                Some(g) => {
                    if g.tick() {
                        cx.notify();
                    }
                    (!g.done(), g.settled())
                }
                None => (false, true),
            }) else {
                break;
            };
            if !more {
                break;
            }
            cx.background_executor().timer(poll_every(settled)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_with_the_fallback_fill_and_loses_it_when_the_glass_runs() {
        let mut m = Machine::default();
        assert!(m.fallback(), "while starting the surface paints its own fill");
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
    fn a_popup_without_glass_paints_a_plain_fill_and_over_glass_a_wash() {
        let near = |c: gpui_kit::Hsla, a: f32| (c.a - a).abs() < 1e-6;
        // no glass yet (or never): the plain panel, dark 0x1C1C1E at 82 %, light 0xF5F5F7 at 86 %
        let (d, l) = (panel_fill(true, true), panel_fill(false, true));
        assert!(near(d, 0.82) && near(l, 0.86));
        let (d, l) = (gpui_kit::Rgba::from(d), gpui_kit::Rgba::from(l));
        let ch = |v: u32| v as f32 / 255.0;
        for (got, want) in [(d.r, ch(0x1C)), (d.g, ch(0x1C)), (d.b, ch(0x1E)), (l.r, ch(0xF5)), (l.g, ch(0xF5)), (l.b, ch(0xF7))] {
            assert!((got - want).abs() < 0.004, "{got} vs {want}");
        }
        // over the glass: black at 28 % (dark), white at 45 % (light)
        let (d, l) = (panel_fill(true, false), panel_fill(false, false));
        assert!(near(d, 0.28) && near(l, 0.45));
        assert_eq!((d.l, l.l), (0.0, 1.0));
        // the fallback is always the denser one: a popup is never see-through with nothing behind it
        assert!(panel_fill(true, true).a > panel_fill(true, false).a && panel_fill(false, true).a > panel_fill(false, false).a);
    }

    #[test]
    fn a_popup_window_without_a_handle_keeps_its_fill_and_does_not_crash() {
        let mut g = PopupGlass::start(0, true);
        assert!(g.fallback() && g.done());
        assert!(!g.tick(), "nothing changes");
        g.release();
        g.release();
        assert!(!crate::glass::liquid(0));
    }

    #[test]
    fn a_popup_looks_quickly_until_the_glass_runs() {
        assert!(poll_every(false) < poll_every(true));
    }
}
