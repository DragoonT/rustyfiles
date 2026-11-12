//! What the dock looks like: the system's light / dark theme and accent colour (the dock follows the system like the Windows taskbar
//! does, not the app's own theme setting) and the colours that go with each.

use gpui_kit::{Global, Hsla, rgb};

/// A colour and how opaque it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tint {
    pub rgb: u32,
    pub alpha: f32,
}

impl Tint {
    pub const fn new(rgb: u32, alpha: f32) -> Tint {
        Tint { rgb, alpha }
    }

    pub fn hsla(self) -> Hsla {
        let mut h: Hsla = rgb(self.rgb).into();
        h.a = self.alpha;
        h
    }

    /// Between two tints: `t` 0 is `a`, 1 is `b` (colour channel by channel, and the opacity).
    pub fn mix(a: Tint, b: Tint, t: f32) -> Tint {
        let t = t.clamp(0.0, 1.0);
        let ch = |shift: u32| {
            let (x, y) = (((a.rgb >> shift) & 0xFF) as f32, ((b.rgb >> shift) & 0xFF) as f32);
            (x + (y - x) * t).round() as u32
        };
        Tint { rgb: ch(16) << 16 | ch(8) << 8 | ch(0), alpha: a.alpha + (b.alpha - a.alpha) * t }
    }
}

/// The colours of one island at one moment: the dark look (light text on dark glass) and the light look (dark text on light glass)
/// and everything between them while the island turns from one to the other (`tone.rs`). `0.0` is the dark look, `1.0` the light one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub mix: f32,
}

impl Palette {
    /// A palette at rest in the dark look (`true`) or the light one.
    #[cfg(test)]
    pub fn of(dark: bool) -> Palette {
        Palette { mix: if dark { 0.0 } else { 1.0 } }
    }

    fn pick(self, f: impl Fn(bool) -> Tint) -> Tint {
        Tint::mix(f(true), f(false), self.mix)
    }

    pub fn fallback_fill(self) -> Tint {
        self.pick(fallback_fill)
    }

    pub fn text(self) -> Tint {
        self.pick(text)
    }

    pub fn text_secondary(self) -> Tint {
        self.pick(text_secondary)
    }

    pub fn field_fill(self) -> Tint {
        self.pick(field_fill)
    }

    pub fn hover_fill(self) -> Tint {
        self.pick(hover_fill)
    }

    pub fn pressed_fill(self) -> Tint {
        self.pick(pressed_fill)
    }

    pub fn separator(self) -> Tint {
        self.pick(separator)
    }

    pub fn running_dot(self, in_front: bool) -> Tint {
        self.pick(|d| running_dot(d, in_front))
    }

    pub fn placeholder_fill(self) -> Tint {
        self.pick(placeholder_fill)
    }
}

/// What every island needs to know about the system: read again when `SystemEvent::ThemeChanged` or `LayoutChanged` arrives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Env {
    /// The system is in the dark theme.
    pub dark: bool,
    /// The system accent colour, 0xRRGGBB.
    pub accent: u32,
    /// Device px per logical px of the primary monitor.
    pub scale: f32,
}

impl Global for Env {}

impl Env {
    pub fn read(scale: f32) -> Env {
        Env { dark: rf_win::accent::system_is_dark(), accent: rf_win::accent::color(), scale: if scale > 0.0 { scale } else { 1.0 } }
    }
}

/// The island's own fill, painted only while the glass is not running (starting, failed): dark 0x1C1C1E at 72 %, light 0xF5F5F7 at
/// 78 %, so the dock is never invisible.
pub fn fallback_fill(dark: bool) -> Tint {
    if dark { Tint::new(0x1C1C1E, 0.72) } else { Tint::new(0xF5F5F7, 0.78) }
}

pub fn text(dark: bool) -> Tint {
    if dark { Tint::new(0xFFFFFF, 0.94) } else { Tint::new(0x1A1A1A, 0.92) }
}

pub fn text_secondary(dark: bool) -> Tint {
    if dark { Tint::new(0xFFFFFF, 0.70) } else { Tint::new(0x000000, 0.62) }
}

/// The search field: white at 10 % on the dark glass, a dark veil of the same weight on the light one (white on light glass would
/// not show).
pub fn field_fill(dark: bool) -> Tint {
    if dark { Tint::new(0xFFFFFF, 0.10) } else { Tint::new(0x000000, 0.07) }
}

/// A button under the pointer.
pub fn hover_fill(dark: bool) -> Tint {
    if dark { Tint::new(0xFFFFFF, 0.10) } else { Tint::new(0x000000, 0.08) }
}

/// A button being pressed.
pub fn pressed_fill(dark: bool) -> Tint {
    if dark { Tint::new(0xFFFFFF, 0.16) } else { Tint::new(0x000000, 0.13) }
}

/// The thin separator of the middle and the right island.
pub fn separator(dark: bool) -> Tint {
    if dark { Tint::new(0xFFFFFF, 0.22) } else { Tint::new(0x000000, 0.20) }
}

/// The mark under a running app: quiet for a running app, full for the one in front.
pub fn running_dot(dark: bool, in_front: bool) -> Tint {
    match (dark, in_front) {
        (true, false) => Tint::new(0xFFFFFF, 0.60),
        (true, true) => Tint::new(0xFFFFFF, 0.95),
        (false, false) => Tint::new(0x000000, 0.50),
        (false, true) => Tint::new(0x000000, 0.85),
    }
}

/// The running mark's size (logical px): a dot, and a wider one for the app in front.
pub fn running_dot_size(in_front: bool) -> (f32, f32) {
    if in_front { (14.0, 4.0) } else { (4.0, 4.0) }
}

/// The placeholder drawn while an icon loads (or when the app has none).
pub fn placeholder_fill(dark: bool) -> Tint {
    if dark { Tint::new(0xFFFFFF, 0.14) } else { Tint::new(0x000000, 0.10) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fallback_fill_is_the_one_the_design_names() {
        assert_eq!(fallback_fill(true), Tint::new(0x1C1C1E, 0.72));
        assert_eq!(fallback_fill(false), Tint::new(0xF5F5F7, 0.78));
    }

    #[test]
    fn dark_and_light_differ_everywhere() {
        let pairs = [
            (text(true), text(false)),
            (text_secondary(true), text_secondary(false)),
            (field_fill(true), field_fill(false)),
            (hover_fill(true), hover_fill(false)),
            (pressed_fill(true), pressed_fill(false)),
            (separator(true), separator(false)),
            (running_dot(true, true), running_dot(false, true)),
            (placeholder_fill(true), placeholder_fill(false)),
        ];
        for (d, l) in pairs {
            assert_ne!(d, l);
        }
    }

    #[test]
    fn text_is_light_on_dark_and_dark_on_light() {
        assert_eq!(text(true).rgb, 0xFFFFFF);
        assert!(text(false).rgb < 0x404040);
    }

    #[test]
    fn the_search_field_is_white_at_ten_percent_on_the_dark_glass() {
        assert_eq!(field_fill(true), Tint::new(0xFFFFFF, 0.10));
    }

    #[test]
    fn pressing_is_stronger_than_hovering() {
        for dark in [true, false] {
            assert!(pressed_fill(dark).alpha > hover_fill(dark).alpha);
        }
    }

    #[test]
    fn the_app_in_front_has_a_wider_mark() {
        let (w0, h0) = running_dot_size(false);
        let (w1, h1) = running_dot_size(true);
        assert!(w1 > w0);
        assert_eq!(h0, h1);
        assert!(running_dot(true, true).alpha > running_dot(true, false).alpha);
    }

    #[test]
    fn a_tint_becomes_a_colour_with_its_opacity() {
        let h = Tint::new(0x1C1C1E, 0.72).hsla();
        assert!((h.a - 0.72).abs() < 1e-6);
    }

    #[test]
    fn a_palette_at_rest_is_the_plain_look_and_between_it_is_a_mix() {
        assert_eq!(Palette::of(true).text(), text(true));
        assert_eq!(Palette::of(false).text(), text(false));
        assert_eq!(Palette::of(true).fallback_fill(), fallback_fill(true));
        assert_eq!(Palette::of(false).running_dot(true), running_dot(false, true));
        let mid = Palette { mix: 0.5 }.text();
        let (d, l) = (text(true), text(false));
        assert!(mid.rgb > l.rgb && mid.rgb < d.rgb, "between the dark text colour and the light one: {:06x}", mid.rgb);
        assert!(mid.alpha > l.alpha.min(d.alpha) - 1e-6 && mid.alpha < d.alpha.max(l.alpha) + 1e-6);
    }

    #[test]
    fn mixing_tints_stays_inside_their_range() {
        let (a, b) = (Tint::new(0x000000, 0.0), Tint::new(0xFFFFFF, 1.0));
        assert_eq!(Tint::mix(a, b, 0.0), a);
        assert_eq!(Tint::mix(a, b, 1.0), b);
        assert_eq!(Tint::mix(a, b, 7.0), b, "beyond the ends is the end");
        assert_eq!(Tint::mix(a, b, -1.0), a);
        let m = Tint::mix(a, b, 0.5);
        assert_eq!(m.rgb, 0x808080);
        assert!((m.alpha - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_broken_scale_reads_as_one() {
        assert_eq!(Env::read(0.0).scale, 1.0);
        assert_eq!(Env::read(1.5).scale, 1.5);
    }
}
