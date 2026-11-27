//! rustyfiles palette: Windows 11 Fluent surfaces + SwiftUI-like softness
//! (rounded corners, gentle fills, calm secondary text).

use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct Palette {
    pub dark: bool,
    /// Window background drawn over Mica (semi-transparent so the backdrop shows through).
    pub window_bg: Hsla,
    pub titlebar_bg: Hsla,
    pub sidebar_bg: Hsla,
    pub content_bg: Hsla,
    pub address_bg: Hsla,
    pub toolbar_bg: Hsla,
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
    /// The controls of this window are drawn the liquid way (the window shows glass, or it is a popup opened from one): see `liquid`.
    /// Set by `for_window`; `light()` and `dark()` are the classic look.
    pub liquid: bool,
}

fn hex(c: u32) -> Hsla {
    rgb(c).into()
}

fn hexa(c: u32, a: f32) -> Hsla {
    let mut h: Hsla = rgb(c).into();
    h.a = a;
    h
}

/// The accent palette the app wears: the colour the user chose for rustyfiles (Settings > Appearance), else the user's Windows
/// accent palette, which is read once.
fn system_accent() -> Option<rf_win::accent::AccentPalette> {
    static WINDOWS: std::sync::OnceLock<Option<rf_win::accent::AccentPalette>> = std::sync::OnceLock::new();
    match rf_win::accent::override_color() {
        Some(c) => Some(rf_win::accent::from_color(c)),
        None => *WINDOWS.get_or_init(rf_win::accent::system_palette),
    }
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
            address_bg: hexa(0x000000, 0.0),
            toolbar_bg: hexa(0x000000, 0.0),
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
            liquid: false,
        }
    }

    /// Pure black for OLED screens (issue 45): the dark palette with every surface black and opaque, the text white, and the lines
    /// and fills just bright enough to be seen on black.
    pub fn black() -> Self {
        Self {
            window_bg: hex(0x000000),
            titlebar_bg: hexa(0x000000, 0.0),
            sidebar_bg: hexa(0x000000, 0.0),
            content_bg: hex(0x000000),
            address_bg: hexa(0x000000, 0.0),
            toolbar_bg: hexa(0x000000, 0.0),
            card_bg: hex(0x0A0A0A),
            bar_bg: hexa(0x000000, 0.0),
            tile_bg: hexa(0xFFFFFF, 0.04),
            tile_hover: hexa(0xFFFFFF, 0.08),
            border: hexa(0xFFFFFF, 0.14),
            divider: hexa(0xFFFFFF, 0.12),
            text: hex(0xFFFFFF),
            text_secondary: hexa(0xFFFFFF, 0.72),
            text_tertiary: hexa(0xFFFFFF, 0.50),
            hover: hexa(0xFFFFFF, 0.08),
            pressed: hexa(0xFFFFFF, 0.12),
            selection: hexa(0xFFFFFF, 0.11),
            tab_active: hexa(0xFFFFFF, 0.08),
            ..Self::dark()
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
            address_bg: hexa(0x000000, 0.0),
            toolbar_bg: hexa(0x000000, 0.0),
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
            liquid: false,
        }
    }

    pub fn for_window(window: &Window) -> Self {
        let mut p = match crate::settings::theme_override() {
            crate::settings::ThemePref::Light => Self::light(),
            crate::settings::ThemePref::Dark => Self::dark(),
            crate::settings::ThemePref::Black => Self::black(),
            crate::settings::ThemePref::System => match window.appearance() {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
                _ => Self::light(),
            },
        };
        let explorer_look = crate::settings::windows_file_explorer_theme();
        if explorer_look && !p.dark {
            p.titlebar_bg = hex(0xE8E8E8);
            p.sidebar_bg = hex(0xFFFFFF);
            p.content_bg = hex(0xFFFFFF);
            p.address_bg = hex(0xF0F5F9);
            p.toolbar_bg = hex(0xFCFDFF);
            p.tab_active = hex(0xF8F8F8);
        }
        if explorer_look && !p.dark && window.is_window_active() {
            p.titlebar_bg = hex(0xCCDEEC);
            p.tab_active = hex(0xF0F5F9);
        }
        // The Explorer look in the light theme keeps the title and tab strip on the material, as Explorer itself does (Mica, or Mica
        // Alt tinted by the wallpaper): its fixed greys covered the material, and the window looked solid whatever was chosen (issue
        // 390; the dark theme's strip was see-through already). Where there is no material, or "Solid" is chosen, the greys stay.
        if explorer_look && !p.dark && crate::settings::materials_available() && !crate::settings::solid_backdrop() {
            p.titlebar_bg.a = 0.0;
        }
        if crate::settings::solid_backdrop() {
            // "Solid" material: Windows 11's plain window color instead of Mica
            p.window_bg = if p.dark { hex(0x202020) } else { hex(0xF3F3F3) };
        }
        // the user's theme folder: its colours over these (issues 93 and 94)
        crate::themes::apply(&mut p);
        if crate::settings::theme_override() == crate::settings::ThemePref::Black {
            // (opaque black whatever the material: no glass, no washes)
            return p;
        }
        let hwnd = crate::look::window::hwnd_of(window);
        // one place decides the look of the controls: every control asks the palette
        p.liquid = crate::glass::liquid(hwnd);
        if crate::glass::shown(hwnd) {
            // Over the glass the two washes are the veil: Mica is already a quiet, nearly opaque tone, the glass is the live screen.
            // At the strength made for Mica they hid two thirds of it in the dark theme (the user: too black), so the default is half
            // of that; the HyperGlass "surface wash" setting moves it either way.
            (p.window_bg.a, p.content_bg.a) = glass_washes(p.dark, crate::settings::hg_surface());
            if explorer_look {
                p.titlebar_bg.a = 0.0;
                p.sidebar_bg.a = 0.0;
                p.address_bg = Hsla { a: glass_bar_wash(crate::settings::hg_surface()), ..p.card_bg };
                p.toolbar_bg = Hsla { a: glass_bar_wash(crate::settings::hg_surface()), ..p.card_bg };
            }
            p.bar_bg = Hsla { a: glass_bar_wash(crate::settings::hg_surface()), ..p.card_bg };
        }
        p
    }
}

impl Palette {
    /// The palette of a window that is MATTE whatever material the user chose: its own plain colour as the window's surface, the
    /// one the "Solid" material has. For the Open / Save window (the user, with a picture of it over a busy desktop: "aç
    /// penceresinde ... arkaplanı da mat"): it opens over another program's window, and a see-through material showed that program
    /// and the desktop through the file list.
    pub fn matte(mut self) -> Self {
        self.window_bg = if self.dark { hex(0x202020) } else { hex(0xF3F3F3) };
        self
    }
}

/// The material a window asks Windows for: the wanted one where Windows has materials, none where it has not (`rf_win::osver`,
/// issue 24). There the palette paints the window itself (`settings::solid_backdrop`), and asking for "none" also switches off a
/// blur-behind that an earlier choice may have left on the window. A see-through window (glass, an overlay) stays what it is.
pub fn window_material(wanted: WindowBackgroundAppearance) -> WindowBackgroundAppearance {
    match wanted {
        WindowBackgroundAppearance::Transparent | WindowBackgroundAppearance::Opaque => wanted,
        _ if crate::settings::materials_available() => wanted,
        _ => WindowBackgroundAppearance::Opaque,
    }
}

/// [`window_material`] for a MAIN window: on a Windows without materials it may still have Acrylic, the old blur behind it, when that
/// is the chosen material (`settings::main_window_only`, issue 78). Popups never ask this.
pub fn main_window_material(wanted: WindowBackgroundAppearance) -> WindowBackgroundAppearance {
    match wanted {
        WindowBackgroundAppearance::Blurred if !crate::settings::materials_available() => WindowBackgroundAppearance::Blurred,
        _ => window_material(wanted),
    }
}

/// The material of a popup (a menu, a flyout, a drop-down list): real glass when it was opened from a window that shows glass,
/// else Windows' Acrylic, else none (and then [`popup_fill`] is its surface).
pub fn popup_material(liquid: bool) -> WindowBackgroundAppearance {
    if liquid { WindowBackgroundAppearance::Transparent } else { window_material(WindowBackgroundAppearance::Blurred) }
}

/// What a popup paints behind its content on a Windows that gives it no Acrylic: the plain colour Windows 11 itself uses for a
/// flyout without transparency. `None` where the Acrylic is the surface and the popup paints nothing.
pub fn popup_fill(p: &Palette) -> Option<Hsla> {
    (!crate::settings::materials_available()).then(|| if p.dark { hex(0x2C2C2C) } else { hex(0xF9F9F9) })
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
    selection_pill_joined(p, false, false)
}

/// The pill of a selected row whose neighbour above or below is selected too: it reaches nearly to that edge with a square end, so
/// that a run of selected rows reads as ONE line cut between its rows, each row still counted by its own piece. The user,
/// 2026-11-20: "ortadakiler birleşik gibi mi olsa ... hem bir görünür hem de sayılabilir çizgiler ile"; 2026-11-27: the gaps should
/// look cut, not like separate lines (they were rounded pills 4 px apart).
pub fn selection_pill_joined(p: &Palette, above: bool, below: bool) -> Div {
    let (top, bottom) = pill_insets(above, below);
    let r = px(1.5);
    div()
        .absolute()
        .left(px(0.))
        .top(px(top))
        .bottom(px(bottom))
        .w(px(3.))
        .when(!above, |d| d.rounded_tl(r).rounded_tr(r))
        .when(!below, |d| d.rounded_bl(r).rounded_br(r))
        .bg(p.accent)
}

/// How far the pill stays from the row's top and bottom: 8 px on a side with no selected neighbour, 1 px towards one (a 2 px cut).
pub fn pill_insets(above: bool, below: bool) -> (f32, f32) {
    (if above { 1. } else { 8. }, if below { 1. } else { 8. })
}

pub const RADIUS: Pixels = px(8.);
pub const RADIUS_SM: Pixels = px(6.);
pub const ROW_H: Pixels = px(30.);
pub const TITLEBAR_H: Pixels = px(40.);

/// The file manager window's tab row: [`TITLEBAR_H`], or 32 with "Compact window" (the tabs are 28 high and stand at its bottom).
pub fn titlebar_h() -> Pixels {
    if crate::settings::compact_window() { px(32.) } else { TITLEBAR_H }
}

/// The address bar's row (`explorer`: the Windows File Explorer look, which keeps its own).
pub fn address_row_h(explorer: bool) -> f32 {
    if explorer {
        60.
    } else if crate::settings::compact_window() {
        40.
    } else {
        48.
    }
}

/// The command bar's row (`explorer`: as in [`address_row_h`]).
pub fn command_bar_h(explorer: bool) -> f32 {
    if explorer {
        58.
    } else if crate::settings::compact_window() {
        38.
    } else {
        44.
    }
}

#[cfg(test)]
mod tests {
    use super::{glass_bar_wash, glass_washes};

    #[test]
    fn a_run_of_selected_rows_has_long_pills_with_small_gaps_and_a_lone_row_its_short_one() {
        assert_eq!(super::pill_insets(false, false), (8., 8.));
        assert_eq!(super::pill_insets(true, true), (1., 1.));
        // the gap between two pills in a run: one row's bottom and the next one's top
        let gap = super::pill_insets(false, true).1 + super::pill_insets(true, false).0;
        assert_eq!(gap, 2.);
    }

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

/// The display a popup of `owner` (a menu level, a flyout, a dropdown) is opened on: the owner's own.
///
/// WHY IT IS NAMED (issue 47, a 1920x1080 monitor at 100% beside a 3440x1440 one at 125%): positions are logical pixels, and on
/// Windows a monitor's logical pixels are its device pixels divided by its own scale, so the two monitors' logical ranges overlap. A
/// popup that names no display is put on the first display whose scale makes its position land on it, and a submenu meant for the
/// 100% monitor landed on the 125% one, a screen away and a quarter larger ("the menu breaks and shifts down"). The owner's
/// coordinates are its display's, so that display is the one that reads them right.
pub fn popup_display(owner: &Window, cx: &App) -> Option<DisplayId> {
    owner.display(cx).map(|d| d.id())
}
