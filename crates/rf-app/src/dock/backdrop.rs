//! The glass behind one island: attaching it with the island's own radius, its own rectangle inside the (larger) window and its shadow,
//! watching what the engine reports, telling the island when to paint its own fill instead, and trying again after a failure.
//!
//! The decisions are in [`Machine`] (pure: the engine's report and the time are arguments); [`IslandGlass`] is the thin layer that
//! makes the calls into `rf_win::glass`. It also holds the island's [`Tone`]: whether the foreground is light or dark, which follows
//! the brightness of what is behind the glass (`rf_win::glass::backdrop_luma`) and gives the engine the matching glass tint.

use std::sync::LazyLock;
use std::time::Instant;

use rf_win::dock::layout::RADIUS;
use rf_win::glass::{GlassOptions, GlassState, Shadow};

use super::look::Palette;
use super::tone::Tone;

/// The clock of the islands' easing: milliseconds since the first look at it. One for the whole process, so the views and the host agree.
pub fn now_ms() -> u64 {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed().as_millis() as u64
}

pub use crate::glass_watch::{Act, Machine, Phase};

/// The corner radius of the island's glass in device px.
pub fn radius_px(scale: f32) -> f32 {
    RADIUS * scale.max(0.1)
}

/// The glass options of an island: its own radius, its own corners cut, its own rectangle inside the window (`shape`: x, y, w, h in
/// device px from the window's top left corner) and, only when the dock's shadow is switched on (`layout::shadow_on`: the window
/// region is expected to clip it away, see `rf_win::dock::hit`), the soft shadow around it. The window is bigger than the glass;
/// everything outside the shape is transparent.
pub fn options(dark: bool, scale: f32, shape: [f32; 4]) -> GlassOptions {
    GlassOptions { dark, radius_px: Some(radius_px(scale)), cut_corners: true, shape: Some(shape), shadow: shadow_on().then_some(Shadow::SOFT) }
}

fn shadow_on() -> bool {
    rf_win::dock::layout::shadow_on()
}

/// Where an island's glass is until it is told: nowhere (it draws nothing), never the whole window for a moment.
pub const NO_SHAPE: [f32; 4] = [0.0; 4];

/// One island's glass. Shared (`Rc<RefCell<..>>`) between the island's view, which reads `fallback()` while it paints, and the host,
/// which starts it, watches it and stops it.
pub struct IslandGlass {
    hwnd: isize,
    machine: Machine,
    /// Light or dark foreground and glass tint (starts from the system theme).
    tone: Tone,
    scale: f32,
    /// The glass's rectangle in the window, device px (see [`options`]).
    shape: [f32; 4],
    /// The tint the engine was last given (`true` is the dark glass).
    sent_dark: bool,
    /// Whether the engine has been told about the window (and so must be told to let go of it).
    attached: bool,
    /// The island's glass is put away while a panel's glass, grown out of it, stands in its place (`grow_host.rs`): the engine is given
    /// no shape (it draws nothing), the shape the layout wants is only remembered, and the tone stays as it was.
    hidden: bool,
}

impl IslandGlass {
    pub fn new() -> IslandGlass {
        IslandGlass { hwnd: 0, machine: Machine::default(), tone: Tone::new(true), scale: 1.0, shape: NO_SHAPE, sent_dark: true, attached: false, hidden: false }
    }

    /// Puts the glass away (`true`) or back: the engine is told at once, and only when something differs. The remembered shape is
    /// kept, so a layout that comes while it is away is not lost.
    pub fn set_hidden(&mut self, hidden: bool) {
        if hidden == self.hidden {
            return;
        }
        self.hidden = hidden;
        if self.attached {
            rf_win::glass::set_options(self.hwnd, self.engine_options());
        }
    }

    /// The window handle (0 before [`IslandGlass::start`]).
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// The options the engine holds for this island, or must hold: the tint it was last given, the scale, the shape as last told. The
    /// tint (`poll_tone`, `sync`) and the shape (`set_shape`) change the SAME options value: every change of one is sent with the
    /// current value of the other, so neither can undo the other.
    fn engine_options(&self) -> GlassOptions {
        options(self.sent_dark, self.scale, if self.hidden { NO_SHAPE } else { self.shape })
    }

    /// Where the glass is in the window (device px: x, y, w, h). Before the glass runs this is only remembered (it is what the glass
    /// starts with); while it runs the engine is told, and only when it is something new: this is called for every frame of the
    /// magnification, so it must cost nothing when nothing moved. The engine takes the change as one constant and one draw (only the
    /// shape differs from the options it holds).
    pub fn set_shape(&mut self, shape: [f32; 4]) {
        if shape == self.shape {
            return;
        }
        self.shape = shape;
        if self.attached && !self.hidden {
            rf_win::glass::set_options(self.hwnd, self.engine_options());
        }
    }

    /// Where the glass is in the window as last told (device px: x, y, w, h).
    pub fn shape(&self) -> [f32; 4] {
        self.shape
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

    /// The tone the island is going to (or is at): true is the dark glass with light text. A panel that opens above the island takes
    /// this, not the half-eased `palette` of a moment in which the island is still turning.
    pub fn dark(&self) -> bool {
        self.tone.glass_dark()
    }

    /// Whether the foreground has stopped changing (the island needs no more frames for it).
    pub fn tone_settled(&self, now_ms: u64) -> bool {
        self.tone.settled(now_ms)
    }

    /// Looks at how bright the backdrop is (a few times a second, from the host) and moves the foreground and the glass tint if the
    /// page behind went across a line. True when the island has to be drawn again; false and nothing else when nothing changed.
    pub fn poll_tone(&mut self, now_ms: u64) -> bool {
        // a glass that is put away measures nothing (it has no area): the tone stays as it was until the glass is back
        if self.hidden {
            return false;
        }
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
        if rf_win::glass::attach_with(self.hwnd, options(dark, self.scale, if self.hidden { NO_SHAPE } else { self.shape })) {
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
            // the radius is in device px: it follows the DPI (this also carries the tint and the shape)
            self.sent_dark = self.tone.glass_dark();
            rf_win::glass::set_options(self.hwnd, self.engine_options());
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
    fn an_island_uses_its_own_radius_and_cuts_its_own_corners() {
        let o = options(true, 1.5, [10.0, 10.0, 300.0, 72.0]);
        assert!(o.dark);
        assert_eq!(o.radius_px, Some(36.0));
        assert!(o.cut_corners);
        assert!(!options(false, 1.0, NO_SHAPE).dark);
        // its own rectangle inside the window and the dock's shadow: the window is bigger than the glass
        assert_eq!(o.shape, Some([10.0, 10.0, 300.0, 72.0]));
        assert_eq!(o.shadow, shadow_on().then_some(Shadow::SOFT), "no shadow unless the experiment is on (the region would clip it)");
        // a broken scale cannot make the radius zero or negative
        assert!(radius_px(0.0) > 0.0);
        assert!(radius_px(-3.0) > 0.0);
    }

    #[test]
    fn the_glass_starts_nowhere_and_remembers_where_it_is_told_to_be() {
        let mut g = IslandGlass::new();
        // never the whole window while nothing has said where the glass is
        assert_eq!(g.engine_options().shape, Some([0.0; 4]));
        g.set_shape([12.0, 28.0, 330.0, 48.0]);
        assert_eq!(g.engine_options().shape, Some([12.0, 28.0, 330.0, 48.0]));
        // the same again changes nothing (it is called for every frame of the animation), and an unattached glass sends nothing
        g.set_shape([12.0, 28.0, 330.0, 48.0]);
        assert_eq!(g.shape(), [12.0, 28.0, 330.0, 48.0]);
        assert_eq!(g.hwnd(), 0);
    }

    /// The tint and the shape are two changes of one options value. What a change of the shape sends carries the tint the engine
    /// has, and what a change of the scale sends carries the shape: neither can undo the other.
    #[test]
    fn the_tint_and_the_shape_do_not_undo_each_other() {
        let mut g = IslandGlass::new();
        g.set_shape([12.0, 28.0, 330.0, 48.0]);
        // the tone went across the line (the engine has the light glass), then the magnification moves the shape
        g.sent_dark = false;
        g.set_shape([8.0, 28.0, 340.0, 48.0]);
        let o = g.engine_options();
        assert!(!o.dark, "a new shape must not turn the tint back");
        assert_eq!(o.shape, Some([8.0, 28.0, 340.0, 48.0]));
        // and a new tint (or a new scale) keeps the shape
        g.sent_dark = true;
        g.scale = 2.0;
        let o = g.engine_options();
        assert!(o.dark);
        assert_eq!(o.shape, Some([8.0, 28.0, 340.0, 48.0]));
        assert_eq!(o.radius_px, Some(radius_px(2.0)));
        // the options of a tint change and of a shape change differ in that one field only (the engine then takes the cheap path)
        let before = g.engine_options();
        g.set_shape([0.0, 28.0, 360.0, 48.0]);
        let after = g.engine_options();
        assert!(before.dark == after.dark && before.radius_px == after.radius_px && before.cut_corners == after.cut_corners && before.shape != after.shape);
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

    /// While a panel's glass stands in the island's place the island's glass has no shape for the engine, whatever the layout says, and
    /// the shape the layout wanted meanwhile is there when the glass comes back.
    #[test]
    fn a_glass_put_away_draws_nothing_and_remembers_where_it_belongs() {
        let mut g = IslandGlass::new();
        g.set_shape([12.0, 28.0, 330.0, 48.0]);
        assert!(!g.hidden);
        g.set_hidden(true);
        assert_eq!(g.engine_options().shape, Some(NO_SHAPE), "nothing is drawn");
        g.set_shape([14.0, 28.0, 340.0, 48.0]);
        assert_eq!(g.shape(), [14.0, 28.0, 340.0, 48.0], "the layout is remembered");
        assert_eq!(g.engine_options().shape, Some(NO_SHAPE));
        assert!(!g.poll_tone(10_000), "no tone is measured meanwhile");
        g.set_hidden(true);
        g.set_hidden(false);
        assert_eq!(g.engine_options().shape, Some([14.0, 28.0, 340.0, 48.0]));
        // the tint of an hidden glass is still the one the engine has (a change of the tint carries the shape the engine has)
        g.set_hidden(true);
        g.sent_dark = false;
        let o = g.engine_options();
        assert!(!o.dark && o.shape == Some(NO_SHAPE));
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
