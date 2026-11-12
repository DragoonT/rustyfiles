//! rustyfiles palette: Windows 11 Fluent surfaces + SwiftUI-like softness
//! (rounded corners, gentle fills, calm secondary text).

use gpui_kit::*;

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct Palette {
    pub dark: bool,
    /// Window background drawn over Mica (semi-transparent so the backdrop shows through).
    pub window_bg: Hsla,
    pub titlebar_bg: Hsla,
    pub sidebar_bg: Hsla,
    pub content_bg: Hsla,
    /// Opaque surface for menus, dialogs and inputs that float over other content.
    pub card_bg: Hsla,
    /// A bar of our own with controls and text in it that no other layer backs (the Open / Save window's name row). Clear on every
    /// backdrop but glass, where it is a wash of its own: the text must not sit straight on the live screen.
    pub bar_bg: Hsla,
    /// Cards that sit on the content layer (Home tiles, preview boxes): Fluent's translucent
    /// CardBackgroundFillColorDefault, so the backdrop tint shows through like in Explorer.
    pub tile_bg: Hsla,
    pub tile_hover: Hsla,
    pub border: Hsla,
    pub divider: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub accent: Hsla,
    pub accent_text: Hsla,
    pub hover: Hsla,
    pub pressed: Hsla,
    pub selection: Hsla,
    pub selection_border: Hsla,
    pub tab_active: Hsla,
    pub danger: Hsla,
}

fn hex(c: u32) -> Hsla {
    rgb(c).into()
}

fn hexa(c: u32, a: f32) -> Hsla {
    let mut h: Hsla = rgb(c).into();
    h.a = a;
    h
}

/// The user's Windows accent palette, read once.
fn system_accent() -> Option<rf_win::accent::AccentPalette> {
    static P: std::sync::OnceLock<Option<rf_win::accent::AccentPalette>> = std::sync::OnceLock::new();
    *P.get_or_init(rf_win::accent::palette)
}

impl Palette {
    // Fluent (Windows 11) values: chrome is Mica, content sits on one translucent layer,
    // selection is a quiet neutral fill plus an accent pill (see `selection_pill`), and the
    // accent is the user's own Windows accent color.
    pub fn light() -> Self {
        let accent = system_accent().map(|a| a.dark1()).unwrap_or(0x005FB8);
        Self {
            dark: false,
            window_bg: hexa(0xF3F3F3, 0.30),
            titlebar_bg: hexa(0xEBEBEB, 0.0),
            sidebar_bg: hexa(0xFFFFFF, 0.0),
            content_bg: hexa(0xFFFFFF, 0.50),
            card_bg: hex(0xF9F9F9),
            bar_bg: hexa(0xF9F9F9, 0.0),
            tile_bg: hexa(0xFFFFFF, 0.70),
            tile_hover: hexa(0xF9F9F9, 0.50),
            border: hexa(0x000000, 0.07),
            divider: hexa(0x000000, 0.07),
            text: hexa(0x000000, 0.90),
            text_secondary: hexa(0x000000, 0.62),
            text_tertiary: hexa(0x000000, 0.45),
            accent: hex(accent),
            accent_text: hex(0xFFFFFF),
            hover: hexa(0x000000, 0.04),
            pressed: hexa(0x000000, 0.07),
            selection: hexa(0x000000, 0.06),
            selection_border: hexa(accent, 0.55),
            tab_active: hexa(0xFFFFFF, 0.70),
            danger: hex(0xC42B1C),
        }
    }

    pub fn dark() -> Self {
        let accent = system_accent().map(|a| a.light2()).unwrap_or(0x60CDFF);
        Self {
            dark: true,
            // a neutral wash over Mica: keeps its depth but tames a strongly colored wallpaper
            // (a red desktop made the whole window look red)
            window_bg: hexa(0x202020, 0.40),
            titlebar_bg: hexa(0x1C1C1C, 0.0),
            sidebar_bg: hexa(0x000000, 0.0),
            // darker than Fluent's LayerFill (#3A3A3A 30%), close to Explorer's content pane
            // (~#1B1B1B over Mica): calmer, less desktop tint behind the files
            content_bg: hexa(0x141414, 0.45),
            card_bg: hex(0x2C2C2C),
            bar_bg: hexa(0x2C2C2C, 0.0),
            tile_bg: hexa(0xFFFFFF, 0.0512),
            tile_hover: hexa(0xFFFFFF, 0.0837),
            border: hexa(0xFFFFFF, 0.08),
            divider: hexa(0xFFFFFF, 0.07),
            text: hexa(0xFFFFFF, 0.94),
            text_secondary: hexa(0xFFFFFF, 0.70),
            text_tertiary: hexa(0xFFFFFF, 0.46),
            accent: hex(accent),
            // Fluent: text on a light accent fill is dark
            accent_text: hexa(0x000000, 0.90),
            hover: hexa(0xFFFFFF, 0.06),
            pressed: hexa(0xFFFFFF, 0.10),
            selection: hexa(0xFFFFFF, 0.085),
            selection_border: hexa(accent, 0.55),
            tab_active: hexa(0xFFFFFF, 0.06),
            danger: hex(0xFF99A4),
        }
    }

    pub fn for_window(window: &Window) -> Self {
        let mut p = match crate::settings::theme_override() {
            crate::settings::ThemePref::Light => Self::light(),
            crate::settings::ThemePref::Dark => Self::dark(),
            crate::settings::ThemePref::System => match window.appearance() {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
                _ => Self::light(),
            },
        };
        if crate::settings::solid_backdrop() {
            // "Solid" material: Windows 11's plain window color instead of Mica
            p.window_bg = if p.dark { hex(0x202020) } else { hex(0xF3F3F3) };
        }
        if crate::glass::shown(crate::look::window::hwnd_of(window)) {
            // Over the glass the two washes are the veil: Mica is already a quiet, nearly opaque tone, the glass is the live screen.
            // At the strength made for Mica they hid two thirds of it in the dark theme (the user: too black), so the default is half
            // of that; the HyperGlass "surface wash" setting moves it either way.
            (p.window_bg.a, p.content_bg.a) = glass_washes(p.dark, crate::settings::hg_surface());
            p.bar_bg = Hsla { a: glass_bar_wash(crate::settings::hg_surface()), ..p.card_bg };
        }
        p
    }
}

/// The alpha of `window_bg` and `content_bg` over a window that shows glass, for the surface value (0..=100) of the HyperGlass
/// settings. 50 is the veil the app shipped with (dark 0.18 / 0.22, light 0.20 / 0.35), 0 is no veil at all and 100 is the strength made
/// for Mica (dark 0.40 / 0.45, light 0.30 / 0.50: the values in `Palette::dark` and `light`); each side of 50 is a straight line. A
/// value above 100 counts as 100.
pub fn glass_washes(dark: bool, surface: u8) -> (f32, f32) {
    let ((w50, c50), (w100, c100)) = if dark { ((0.18, 0.22), (0.40, 0.45)) } else { ((0.20, 0.35), (0.30, 0.50)) };
    let v = f32::from(surface.min(100));
    let lerp = |a: f32, b: f32, t: f32| a * (1.0 - t) + b * t;
    if v <= 50.0 {
        let t = v / 50.0;
        (lerp(0.0, w50, t), lerp(0.0, c50, t))
    } else {
        let t = (v - 50.0) / 50.0;
        (lerp(w50, w100, t), lerp(c50, c100, t))
    }
}

/// The alpha of `Palette::bar_bg` over glass. It follows the surface value too, but never gets thin: 0.40 at 0, 0.55 at the default 50
/// and 0.70 at 100, so a bar's text is readable whatever the picture behind the glass is.
pub fn glass_bar_wash(surface: u8) -> f32 {
    0.40 + 0.30 * f32::from(surface.min(100)) / 100.0
}

/// Icon tint per file category (SwiftUI-style colored symbols).
pub fn kind_color(cat: crate::icons::Category, dark: bool) -> Hsla {
    use crate::icons::Category::*;
    let c = match cat {
        Folder => 0xF2B33D,
        Image => 0xAF52DE,
        Video => 0xFF375F,
        Audio => 0xFF9F0A,
        Archive => 0xA2845E,
        Code => 0x30B0C7,
        Document => 0x0A84FF,
        Spreadsheet => 0x30D158,
        Pdf => 0xFF453A,
        Executable => 0x8E8E93,
        Text => 0x8E8E93,
        Other => 0x8E8E93,
    };
    let mut h = hex(c);
    if !dark && matches!(cat, Folder) {
        h.l *= 0.92;
    }
    h
}

/// The selection marker shared by every list (sidebar, details, columns, menus): a short
/// rounded accent bar on the left edge of a selected row. The row must be `relative()`.
pub fn selection_pill(p: &Palette) -> Div {
    div().absolute().left(px(0.)).top(px(8.)).bottom(px(8.)).w(px(3.)).rounded_full().bg(p.accent)
}

pub const RADIUS: Pixels = px(8.);
pub const RADIUS_SM: Pixels = px(6.);
pub const ROW_H: Pixels = px(30.);
pub const TITLEBAR_H: Pixels = px(40.);

#[cfg(test)]
mod tests {
    use super::{glass_bar_wash, glass_washes};

    #[test]
    fn a_bar_over_glass_always_keeps_a_wash_of_its_own() {
        assert!((glass_bar_wash(0) - 0.40).abs() < 1e-6);
        assert!((glass_bar_wash(50) - 0.55).abs() < 1e-6);
        assert!((glass_bar_wash(100) - 0.70).abs() < 1e-6);
        assert_eq!(glass_bar_wash(200), glass_bar_wash(100));
        assert!((0..=100u8).all(|v| glass_bar_wash(v) >= 0.40), "readable at every setting");
    }

    #[test]
    fn the_default_surface_is_the_shipped_veil_exactly() {
        assert_eq!(glass_washes(true, 50), (0.18, 0.22));
        assert_eq!(glass_washes(false, 50), (0.20, 0.35));
        assert_eq!(glass_washes(true, crate::settings::HG_DEFAULT[4]), (0.18, 0.22), "and 50 is what the settings default to");
    }

    #[test]
    fn the_ends_are_no_veil_and_the_mica_strength() {
        assert_eq!(glass_washes(true, 0), (0.0, 0.0));
        assert_eq!(glass_washes(false, 0), (0.0, 0.0));
        assert_eq!(glass_washes(true, 100), (0.40, 0.45));
        assert_eq!(glass_washes(false, 100), (0.30, 0.50));
        assert_eq!(glass_washes(true, 255), glass_washes(true, 100), "a value out of range counts as the end");
    }

    #[test]
    fn the_veil_only_grows_with_the_value_and_stays_a_valid_alpha() {
        for dark in [true, false] {
            let mut last = (-1.0f32, -1.0f32);
            for v in 0..=100u8 {
                let (w, c) = glass_washes(dark, v);
                assert!(w >= last.0 && c >= last.1, "dark={dark} v={v}: {w} {c} after {last:?}");
                assert!((0.0..=1.0).contains(&w) && (0.0..=1.0).contains(&c));
                last = (w, c);
            }
        }
        // a quarter of the way is half of the shipped veil, three quarters is halfway to the Mica strength
        let (w, c) = glass_washes(true, 25);
        assert!((w - 0.09).abs() < 1e-6 && (c - 0.11).abs() < 1e-6, "{w} {c}");
        let (w, c) = glass_washes(false, 75);
        assert!((w - 0.25).abs() < 1e-6 && (c - 0.425).abs() < 1e-6, "{w} {c}");
    }
}
