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

/// Windows' own palette, read from HKCU; `None` if unavailable (then use Fluent's default blue).
pub fn system_palette() -> Option<AccentPalette> {
    let key = windows_registry::CURRENT_USER.open(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent").ok()?;
    let raw = key.get_value("AccentPalette").ok()?;
    parse_palette(&raw)
}

/// The colour the user chose for rustyfiles in place of Windows' accent (`NO_OVERRIDE`: none). One value for the process, set from the
/// settings whenever they are applied.
static OVERRIDE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(NO_OVERRIDE);
const NO_OVERRIDE: u32 = u32::MAX;

/// Sets or clears the user's own accent colour (0xRRGGBB).
///
/// WHY (a user on Threads, 2026-11-15: "could u also add an option to change the orange accent"). The accent was Windows' and nothing
/// else: whoever did not want that colour in a file manager had to change it for the whole system.
pub fn set_override(color: Option<u32>) {
    OVERRIDE.store(color.map_or(NO_OVERRIDE, |c| c & 0xFF_FFFF), std::sync::atomic::Ordering::Relaxed);
}

/// The user's own accent colour, when they chose one.
pub fn override_color() -> Option<u32> {
    let c = OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    (c != NO_OVERRIDE).then_some(c)
}

/// The palette the app wears: the user's own colour with shades made from it, else Windows' palette.
pub fn palette() -> Option<AccentPalette> {
    match override_color() {
        Some(c) => Some(from_color(c)),
        None => system_palette(),
    }
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xFF) as f32, ((b >> shift) & 0xFF) as f32);
        ((x + (y - x) * t).round() as u32).min(255) << shift
    };
    ch(16) | ch(8) | ch(0)
}

/// A palette made from one colour: three lighter shades (towards white) and three darker ones (towards black), the colour itself
/// in the middle, as Windows' own palette is laid out.
pub fn from_color(base: u32) -> AccentPalette {
    let base = base & 0xFF_FFFF;
    AccentPalette {
        shades: [mix(base, 0xFF_FFFF, 0.54), mix(base, 0xFF_FFFF, 0.36), mix(base, 0xFF_FFFF, 0.18), base, mix(base, 0, 0.15), mix(base, 0, 0.30), mix(base, 0, 0.45)],
    }
}

/// A colour as the user writes it: "#1A2B3C", "1a2b3c", or the short "#abc". `None` for anything else.
pub fn parse_hex(text: &str) -> Option<u32> {
    let t = text.trim().trim_start_matches('#');
    if !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match t.len() {
        6 => u32::from_str_radix(t, 16).ok(),
        3 => {
            let d: Vec<u32> = t.chars().filter_map(|c| c.to_digit(16)).collect();
            Some((d[0] * 17) << 16 | (d[1] * 17) << 8 | d[2] * 17)
        }
        _ => None,
    }
}

/// A colour as it is kept in the settings: "#RRGGBB".
pub fn to_hex(color: u32) -> String {
    format!("#{:06X}", color & 0xFF_FFFF)
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
    fn a_colour_is_read_as_the_user_writes_it() {
        assert_eq!(parse_hex("#1A2B3C"), Some(0x1A2B3C));
        assert_eq!(parse_hex("  1a2b3c "), Some(0x1A2B3C));
        assert_eq!(parse_hex("#abc"), Some(0xAABBCC));
        assert_eq!(parse_hex("#000000"), Some(0));
        for bad in ["", "#", "#12345", "#1234567", "turuncu", "#GGGGGG", "12 34 56"] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
        assert_eq!(to_hex(0x1A2B3C), "#1A2B3C");
        assert_eq!(to_hex(0xFF_0000_0A), "#00000A", "only the colour's three bytes");
        assert_eq!(parse_hex(&to_hex(0xF7630C)), Some(0xF7630C));
    }

    #[test]
    fn a_palette_made_from_one_colour_has_it_in_the_middle_lighter_before_and_darker_after() {
        let p = from_color(0xF7630C);
        assert_eq!(p.base(), 0xF7630C);
        let light = |c: u32| ((c >> 16) & 0xFF) + ((c >> 8) & 0xFF) + (c & 0xFF);
        for w in p.shades.windows(2) {
            assert!(light(w[0]) > light(w[1]), "{:06X} is not lighter than {:06X}", w[0], w[1]);
        }
        // the ends are still colours, not white and black
        assert!(p.shades[0] != 0xFF_FFFF && p.shades[6] != 0);
        // white and black have nowhere to go on one side, and stay themselves in the middle
        assert_eq!(from_color(0xFF_FFFF).light2(), 0xFF_FFFF);
        assert_eq!(from_color(0).dark1(), 0);
    }

    /// The user's own colour takes the place of Windows' everywhere the palette is asked for, and goes when it is cleared. (One
    /// test for the one value of the process.)
    #[test]
    fn the_users_own_colour_takes_the_place_of_windows_accent_until_it_is_cleared() {
        set_override(Some(0x107C10));
        assert_eq!(override_color(), Some(0x107C10));
        assert_eq!(color(), 0x107C10);
        assert_eq!(palette(), Some(from_color(0x107C10)));
        set_override(None);
        assert_eq!(override_color(), None);
        assert_eq!(palette(), system_palette());
    }

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
