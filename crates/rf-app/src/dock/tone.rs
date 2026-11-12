//! Light or dark foreground for one island, chosen from how bright the picture behind its glass is.
//!
//! The islands follow the system's theme: light text on a dark glass. Over a white page the glass is light grey and light text can
//! hardly be read. So each island looks at the brightness of what is BEHIND its glass (`rf_win::glass::backdrop_luma`, 0 to 1) and
//! turns its foreground and its glass tint the other way when the page behind is bright: dark glass with light text, or light glass
//! with dark text. Two docks that were read for ideas do the same (the value, not the thresholds, is theirs).
//!
//! Hysteresis keeps the island from flickering when the brightness sits near the line: it switches to dark text only above
//! [`TO_DARK_TEXT_ABOVE`] and back to light text only below [`TO_LIGHT_TEXT_BELOW`]; between the two it stays as it is. The change of
//! the foreground is eased over [`EASE_MS`]. Everything here is pure: the brightness and the time are arguments.
//!
//! WHEN THE GLASS IS NOT RUNNING there is no brightness to go by: the island paints its own fill and the system's theme decides, as
//! it did before. The same when the engine has not measured yet, and it is where the island starts from.

/// The backdrop has to be brighter than this for light glass with dark text to replace dark glass with light text.
pub const TO_DARK_TEXT_ABOVE: f32 = 0.58;
/// ... and darker than this for the dark glass to come back.
pub const TO_LIGHT_TEXT_BELOW: f32 = 0.46;
/// How long the foreground takes to change.
pub const EASE_MS: u64 = 300;

/// The hysteresis alone: whether the island wants the light glass (dark text) after seeing `luma`, given what it wants now.
pub fn wants_light_glass(now_light: bool, luma: f32) -> bool {
    if now_light {
        // stays light until the page is clearly dark again (`Tone::feed` never passes a value that is not a number)
        luma >= TO_LIGHT_TEXT_BELOW
    } else {
        luma > TO_DARK_TEXT_ABOVE
    }
}

/// The easing curve of a change: 0 to 1 over [`EASE_MS`], slow at both ends.
pub fn ease(elapsed_ms: u64) -> f32 {
    let t = (elapsed_ms as f32 / EASE_MS as f32).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    /// The system is in the dark theme.
    system_dark: bool,
    /// The island wants the light glass with dark text (the target of the easing).
    light: bool,
    /// Where the easing started, and where it goes, as a mix: 0 is the dark glass with light text, 1 the light glass with dark text.
    from: f32,
    to: f32,
    since_ms: u64,
}

impl Tone {
    /// An island that has not looked at anything yet: the system's theme, at rest.
    pub fn new(system_dark: bool) -> Tone {
        let to = if system_dark { 0.0 } else { 1.0 };
        Tone { system_dark, light: !system_dark, from: to, to, since_ms: 0 }
    }

    /// The glass tint the engine is asked for: dark glass goes with light text.
    pub fn glass_dark(&self) -> bool {
        !self.light
    }

    /// The target as a flag: true is light glass with dark text.
    #[cfg(test)]
    pub fn wants_light(&self) -> bool {
        self.light
    }

    /// 0 (dark glass, light text) to 1 (light glass, dark text) at `now_ms`, with the change eased.
    pub fn mix(&self, now_ms: u64) -> f32 {
        if self.from == self.to {
            return self.to;
        }
        self.from + (self.to - self.from) * ease(now_ms.saturating_sub(self.since_ms))
    }

    /// Whether nothing is moving at `now_ms` (the island needs no more frames for the foreground).
    pub fn settled(&self, now_ms: u64) -> bool {
        self.from == self.to || now_ms.saturating_sub(self.since_ms) >= EASE_MS
    }

    fn retarget(&mut self, light: bool, now_ms: u64) {
        if light == self.light {
            return;
        }
        // from wherever the mix is now, so a change that arrives while another is under way does not jump
        self.from = self.mix(now_ms);
        self.to = if light { 1.0 } else { 0.0 };
        self.since_ms = now_ms;
        self.light = light;
    }

    /// The system's theme changed. While the island goes by the system (no brightness known) this changes what it wants; otherwise the
    /// backdrop decides and only the starting point of a later fallback changes. True when the target changed.
    pub fn set_system(&mut self, dark: bool, backdrop_known: bool, now_ms: u64) -> bool {
        self.system_dark = dark;
        if backdrop_known {
            return false;
        }
        let before = self.light;
        self.retarget(!dark, now_ms);
        before != self.light
    }

    /// One look at the backdrop: `Some(brightness)` while the glass runs and has been measured, `None` otherwise (the system decides).
    /// True when the target changed, so the island has to be drawn again (and the engine told the tint).
    pub fn feed(&mut self, luma: Option<f32>, now_ms: u64) -> bool {
        let before = self.light;
        let want = match luma {
            Some(l) if l.is_finite() => wants_light_glass(self.light, l),
            _ => !self.system_dark,
        };
        self.retarget(want, now_ms);
        before != self.light
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_from_the_system_theme() {
        assert!(!Tone::new(true).wants_light(), "a dark system: dark glass, light text");
        assert!(Tone::new(true).glass_dark());
        assert!(Tone::new(false).wants_light());
        assert!(!Tone::new(false).glass_dark());
        assert_eq!(Tone::new(true).mix(0), 0.0);
        assert_eq!(Tone::new(false).mix(12_345), 1.0);
        assert!(Tone::new(true).settled(0));
    }

    #[test]
    fn the_thresholds_are_the_agreed_ones() {
        assert_eq!(TO_DARK_TEXT_ABOVE, 0.58);
        assert_eq!(TO_LIGHT_TEXT_BELOW, 0.46);
        assert_eq!(EASE_MS, 300);
    }

    #[test]
    fn a_bright_page_turns_the_text_dark_only_above_the_upper_line() {
        assert!(!wants_light_glass(false, 0.58), "exactly on the line is not above it");
        assert!(wants_light_glass(false, 0.5801));
        assert!(!wants_light_glass(false, 0.0));
        assert!(wants_light_glass(false, 1.0));
    }

    #[test]
    fn it_turns_back_only_below_the_lower_line() {
        assert!(wants_light_glass(true, 0.46), "exactly on the line is not below it");
        assert!(!wants_light_glass(true, 0.4599));
        assert!(wants_light_glass(true, 1.0));
    }

    #[test]
    fn between_the_lines_it_stays_as_it_is() {
        for l in [0.47, 0.5, 0.52, 0.57] {
            assert!(!wants_light_glass(false, l), "dark text is not wanted at {l} when it was light text");
            assert!(wants_light_glass(true, l), "light text is not wanted at {l} when it was dark text");
        }
    }

    #[test]
    fn a_value_wandering_across_the_band_changes_it_twice_not_many_times() {
        let mut t = Tone::new(true);
        let mut changes = 0;
        // up from dark through the band to bright, wobbling around the middle of the band, then down again
        for (i, l) in [0.30, 0.50, 0.56, 0.52, 0.59, 0.55, 0.50, 0.47, 0.57, 0.50, 0.45, 0.50, 0.55].into_iter().enumerate() {
            changes += usize::from(t.feed(Some(l), i as u64 * 250));
        }
        assert_eq!(changes, 2, "to dark text at 0.59, back at 0.45");
        assert!(!t.wants_light());
    }

    #[test]
    fn a_nan_changes_nothing_but_falls_back_to_the_system() {
        let mut t = Tone::new(true);
        t.feed(Some(0.9), 0);
        assert!(t.wants_light());
        // not a number: no brightness known, so the system decides (dark theme: light text)
        assert!(t.feed(Some(f32::NAN), 1000));
        assert!(!t.wants_light());
    }

    #[test]
    fn the_system_decides_while_nothing_is_known_and_again_when_the_glass_goes() {
        let mut t = Tone::new(false);
        assert!(t.wants_light(), "light system");
        assert!(!t.feed(None, 100), "nothing known: still the system's");
        // the glass runs over a dark wall
        assert!(t.feed(Some(0.1), 200));
        assert!(!t.wants_light());
        // the glass fails: back to what the system says
        assert!(t.feed(None, 5_000));
        assert!(t.wants_light());
    }

    #[test]
    fn a_new_system_theme_matters_only_while_the_backdrop_is_not_known() {
        let mut t = Tone::new(true);
        assert!(t.set_system(false, false, 10), "no glass: the island follows the theme");
        assert!(t.wants_light());
        // with a brightness known the backdrop decides; the theme is only remembered for later
        let mut t = Tone::new(true);
        t.feed(Some(0.9), 0);
        assert!(!t.set_system(false, true, 10));
        assert!(t.wants_light(), "the bright page still decides");
        assert!(t.feed(Some(0.1), 20));
        assert!(!t.wants_light());
        assert!(t.feed(None, 30), "and without a brightness the new theme (light) decides");
        assert!(t.wants_light());
    }

    #[test]
    fn the_change_is_eased_over_300_ms() {
        let mut t = Tone::new(true);
        t.feed(Some(0.9), 1_000);
        assert_eq!(t.mix(1_000), 0.0, "it starts where it was");
        let mid = t.mix(1_150);
        assert!((mid - 0.5).abs() < 1e-6, "half way at half the time: {mid}");
        assert!(t.mix(1_075) < 0.25 && t.mix(1_225) > 0.75, "slow at both ends");
        assert_eq!(t.mix(1_300), 1.0);
        assert_eq!(t.mix(9_999), 1.0);
        assert!(!t.settled(1_000) && !t.settled(1_299));
        assert!(t.settled(1_300));
        // never runs backwards or past the ends
        let mut last = 0.0;
        for ms in (1_000..1_400).step_by(10) {
            let m = t.mix(ms);
            assert!((0.0..=1.0).contains(&m) && m >= last);
            last = m;
        }
    }

    #[test]
    fn a_change_that_arrives_mid_ease_starts_from_where_the_mix_is() {
        let mut t = Tone::new(true);
        t.feed(Some(0.9), 0);
        let at = t.mix(150);
        assert!((at - 0.5).abs() < 1e-6);
        // the page goes dark again half way through
        assert!(t.feed(Some(0.1), 150));
        assert!((t.mix(150) - at).abs() < 1e-6, "no jump");
        assert!(t.mix(300) < at, "on its way back");
        assert_eq!(t.mix(450), 0.0);
    }

    #[test]
    fn feeding_the_same_value_again_changes_nothing_and_costs_no_redraw() {
        let mut t = Tone::new(true);
        t.feed(Some(0.9), 0);
        let copy = t;
        for i in 1..10 {
            assert!(!t.feed(Some(0.9), i * 250));
        }
        assert_eq!(t, copy, "not even the easing is restarted");
    }
}
