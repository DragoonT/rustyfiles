//! The user's Windows accent color palette (Settings → Personalization → Colors), so the app
//! wears the same color as the taskbar, Start and Files.

/// Shades of the accent, as 0xRRGGBB: `[light3, light2, light1, base, dark1, dark2, dark3]`.
/// Fluent uses `light2` for accents on dark surfaces and `dark1` on light ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentPalette {
    pub shades: [u32; 7],
}

impl AccentPalette {
    pub fn light2(&self) -> u32 {
        self.shades[1]
    }
    pub fn dark1(&self) -> u32 {
        self.shades[4]
    }
    pub fn base(&self) -> u32 {
        self.shades[3]
    }
}

/// `AccentPalette` is 8 RGBA quadruples; the first seven are the shades.
pub fn parse_palette(raw: &[u8]) -> Option<AccentPalette> {
    if raw.len() < 28 {
        return None;
    }
    let mut shades = [0u32; 7];
    for (i, s) in shades.iter_mut().enumerate() {
        let c = &raw[i * 4..i * 4 + 4];
        *s = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
    }
    Some(AccentPalette { shades })
}

/// Reads the palette from HKCU; `None` if unavailable (then use Fluent's default blue).
pub fn palette() -> Option<AccentPalette> {
    let key = windows_registry::CURRENT_USER.open(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent").ok()?;
    let raw = key.get_value("AccentPalette").ok()?;
    parse_palette(&raw)
}

/// Fluent's default blue, for a machine whose accent cannot be read.
pub const DEFAULT_ACCENT: u32 = 0x0078D4;

/// The accent colour as 0xRRGGBB (the palette's base shade), Fluent's blue when it cannot be read.
pub fn color() -> u32 {
    palette().map_or(DEFAULT_ACCENT, |p| p.base())
}

/// What Windows' "light theme" values mean: 0 is dark, anything else light, and a missing value is light (the system default).
pub fn dark_from_value(light_theme: Option<u32>) -> bool {
    light_theme == Some(0)
}

fn theme_value(name: &str) -> Option<u32> {
    let key = windows_registry::CURRENT_USER.open(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize").ok()?;
    key.get_u32(name).ok()
}

/// Whether the system (taskbar, Start, the shell's surfaces) is in the dark theme. This is what the dock follows, like the Windows
/// taskbar does. Read again when `dock::SystemEvent::ThemeChanged` arrives.
pub fn system_is_dark() -> bool {
    dark_from_value(theme_value("SystemUsesLightTheme"))
}

/// Whether apps are asked to use the dark theme (the other of the two settings; rustyfiles' own windows use it by default).
pub fn apps_use_dark() -> bool {
    dark_from_value(theme_value("AppsUseLightTheme"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_value_means_dark_only_when_it_is_zero() {
        assert!(dark_from_value(Some(0)));
        assert!(!dark_from_value(Some(1)));
        assert!(!dark_from_value(None), "no value: the light default");
    }

    #[test]
    fn the_live_values_are_readable() {
        // read-only; whatever the machine's theme is, the calls answer and the accent is a colour
        let _ = (system_is_dark(), apps_use_dark());
        assert!(color() <= 0xFFFFFF);
    }

    #[test]
    fn parses_shades() {
        let raw = [
            0xF0, 0xC0, 0xF4, 0, 0xDB, 0x9E, 0xE5, 0, 0xB7, 0x63, 0xCB, 0, 0xA9, 0x4D, 0xC1, 0, 0x8E, 0x3A, 0xA7, 0, 0x69, 0x27, 0x82, 0, 0x40, 0x0E,
            0x59, 0, 0x88, 0x17, 0x98, 0,
        ];
        let p = parse_palette(&raw).unwrap();
        assert_eq!(p.light2(), 0xDB9EE5);
        assert_eq!(p.dark1(), 0x8E3AA7);
        assert!(parse_palette(&raw[..20]).is_none());
    }
}
