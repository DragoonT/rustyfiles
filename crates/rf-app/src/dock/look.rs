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
    fn a_broken_scale_reads_as_one() {
        assert_eq!(Env::read(0.0).scale, 1.0);
        assert_eq!(Env::read(1.5).scale, 1.5);
    }
}
