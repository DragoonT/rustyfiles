//! Theme folders: colours and icons of the user's own (issues 93 and 94: "a theme folder that loads colors and icons" as the first
//! step towards themes and icon packs).
//!
//! A theme is a folder in `<settings folder>\themes\` (Settings > Appearance > Theme folder picks one):
//! * `theme.toml`: colours over the built-in palette, in a `[dark]` and a `[light]` table (one is enough), each key a colour of the
//!   palette (`accent`, `text`, `window_bg` ...: [`COLOR_KEYS`]) and each value `#RRGGBB` or `#RRGGBBAA`;
//! * `icons\`: pictures (`.png`, `.svg` or `.ico`) for folders (`folder`), for file types by extension (`pdf`, `zip` ...) and for
//!   every other file (`file`). A program's own icon (an .exe's, a shortcut's) stays unless the theme has a picture for exactly
//!   its extension.
//!
//! Read when the setting changes; a theme that cannot be read is no theme (and says why in the log). `docs/themes.md` has an
//! example.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use gpui_kit::Hsla;

use crate::theme::Palette;

/// The palette's colours a theme may set, by their names in `theme.toml`.
pub const COLOR_KEYS: &[&str] = &[
    "window_bg", "titlebar_bg", "sidebar_bg", "content_bg", "card_bg", "bar_bg", "tile_bg", "tile_hover", "border", "divider", "text",
    "text_secondary", "text_tertiary", "accent", "accent_text", "hover", "pressed", "selection", "selection_border", "tab_active",
    "danger",
];

const ICON_EXTS: &[&str] = &["png", "svg", "ico"];

#[derive(Default, Debug)]
pub struct Theme {
    pub name: String,
    dark: Vec<(String, Hsla)>,
    light: Vec<(String, Hsla)>,
    /// "folder", "file" or an extension (lower case) -> its picture.
    icons: HashMap<String, PathBuf>,
}

static CURRENT: RwLock<Option<Arc<Theme>>> = RwLock::new(None);

/// `<settings folder>\themes`.
pub fn dir() -> Option<PathBuf> {
    crate::settings::app_dir().map(|d| d.join("themes"))
}

/// The theme folders there, by name.
pub fn list() -> Vec<String> {
    let Some(d) = dir() else { return Vec::new() };
    let mut v: Vec<String> = std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

/// The theme in use (`""`: none). Called when the settings change.
pub fn set(name: &str) {
    let now = CURRENT.read().ok().and_then(|c| c.as_ref().map(|t| t.name.clone())).unwrap_or_default();
    if now == name && !name.is_empty() {
        return;
    }
    let theme = (!name.is_empty()).then(|| dir().map(|d| load(&d.join(name), name))).flatten().map(Arc::new);
    if let Ok(mut c) = CURRENT.write() {
        *c = theme;
    }
}

fn current() -> Option<Arc<Theme>> {
    CURRENT.read().ok()?.clone()
}

/// Reads a theme folder. Colours that cannot be read are left out and logged.
pub fn load(folder: &Path, name: &str) -> Theme {
    let mut t = Theme { name: name.to_string(), ..Default::default() };
    if let Ok(text) = std::fs::read_to_string(folder.join("theme.toml")) {
        match parse_colors(&text) {
            Ok((dark, light)) => (t.dark, t.light) = (dark, light),
            Err(e) => log::warn!("theme {name}: theme.toml: {e}"),
        }
    }
    for e in std::fs::read_dir(folder.join("icons")).into_iter().flatten().flatten() {
        let p = e.path();
        let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()) else { continue };
        if ICON_EXTS.contains(&ext.as_str()) {
            // (one picture per name: an .svg is preferred, then a .png)
            let better = |old: &PathBuf| rank(&ext) < rank(&old.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default());
            if t.icons.get(&stem).is_none_or(better) {
                t.icons.insert(stem, p);
            }
        }
    }
    t
}

fn rank(ext: &str) -> usize {
    ICON_EXTS.iter().position(|e| *e == ext).map_or(usize::MAX, |i| [1, 0, 2][i])
}

/// `[dark]` and `[light]` of a `theme.toml`. Pure.
pub fn parse_colors(text: &str) -> Result<(Vec<(String, Hsla)>, Vec<(String, Hsla)>), String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let part = |key: &str| -> Vec<(String, Hsla)> {
        let Some(toml::Value::Table(t)) = table.get(key) else { return Vec::new() };
        t.iter()
            .filter(|(k, _)| COLOR_KEYS.contains(&k.as_str()))
            .filter_map(|(k, v)| match v.as_str().and_then(parse_hex) {
                Some(c) => Some((k.clone(), c)),
                None => {
                    log::warn!("theme: {key}.{k} is no #RRGGBB colour");
                    None
                }
            })
            .collect()
    };
    Ok((part("dark"), part("light")))
}

/// `#RRGGBB` or `#RRGGBBAA`. Pure.
pub fn parse_hex(s: &str) -> Option<Hsla> {
    let h = s.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        6 => Some(gpui_kit::rgb(v).into()),
        8 => Some(gpui_kit::rgba(v).into()),
        _ => None,
    }
}

/// Puts the theme's colours over a palette (its dark or light set, as the palette is).
pub fn apply(p: &mut Palette) {
    let Some(t) = current() else { return };
    let set = if p.dark { &t.dark } else { &t.light };
    for (k, c) in set {
        if let Some(slot) = field(p, k) {
            *slot = *c;
        }
    }
}

fn field<'a>(p: &'a mut Palette, key: &str) -> Option<&'a mut Hsla> {
    Some(match key {
        "window_bg" => &mut p.window_bg,
        "titlebar_bg" => &mut p.titlebar_bg,
        "sidebar_bg" => &mut p.sidebar_bg,
        "content_bg" => &mut p.content_bg,
        "card_bg" => &mut p.card_bg,
        "bar_bg" => &mut p.bar_bg,
        "tile_bg" => &mut p.tile_bg,
        "tile_hover" => &mut p.tile_hover,
        "border" => &mut p.border,
        "divider" => &mut p.divider,
        "text" => &mut p.text,
        "text_secondary" => &mut p.text_secondary,
        "text_tertiary" => &mut p.text_tertiary,
        "accent" => &mut p.accent,
        "accent_text" => &mut p.accent_text,
        "hover" => &mut p.hover,
        "pressed" => &mut p.pressed,
        "selection" => &mut p.selection,
        "selection_border" => &mut p.selection_border,
        "tab_active" => &mut p.tab_active,
        "danger" => &mut p.danger,
        _ => return None,
    })
}

/// The theme's picture for an item: a folder's, its extension's, or (for a file without a picture of its own, such as a
/// program's) the theme's picture for every file.
pub fn icon_file(is_dir: bool, ext: &str, own_picture: bool) -> Option<PathBuf> {
    let t = current()?;
    pick_icon(&t.icons, is_dir, ext, own_picture)
}

/// [`icon_file`] for a given set of pictures. Pure.
fn pick_icon(icons: &HashMap<String, PathBuf>, is_dir: bool, ext: &str, own_picture: bool) -> Option<PathBuf> {
    if is_dir {
        return icons.get("folder").cloned();
    }
    let ext = ext.to_lowercase();
    if let Some(p) = icons.get(&ext).filter(|_| !ext.is_empty()) {
        return Some(p.clone());
    }
    (!own_picture).then(|| icons.get("file").cloned()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_read_by_the_palettes_names() {
        let (dark, light) = parse_colors("[dark]\naccent = \"#88C0D0\"\nwindow_bg = \"#2E3440CC\"\nnot_a_colour = \"#FFFFFF\"\ntext = \"blue\"\n[light]\naccent = \"#5E81AC\"\n").unwrap();
        let keys: Vec<&str> = dark.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["accent", "window_bg"], "unknown keys and colours that are no #RRGGBB are left out");
        assert_eq!(light.len(), 1);
        assert!((dark[1].1.a - 0.8).abs() < 0.01, "#RRGGBBAA keeps its alpha");
        assert!(parse_colors("[dark\n").is_err());
    }

    #[test]
    fn a_program_keeps_its_own_icon_unless_its_extension_has_a_picture() {
        let icons: HashMap<String, PathBuf> =
            [("folder", "f.svg"), ("file", "x.svg"), ("pdf", "pdf.png"), ("lnk", "lnk.svg")].into_iter().map(|(k, v)| (k.to_string(), PathBuf::from(v))).collect();
        assert_eq!(pick_icon(&icons, true, "", true), Some(PathBuf::from("f.svg")));
        assert_eq!(pick_icon(&icons, false, "PDF", false), Some(PathBuf::from("pdf.png")));
        assert_eq!(pick_icon(&icons, false, "txt", false), Some(PathBuf::from("x.svg")), "any other file");
        assert_eq!(pick_icon(&icons, false, "exe", true), None, "a program's own icon stays");
        assert_eq!(pick_icon(&icons, false, "lnk", true), Some(PathBuf::from("lnk.svg")), "unless its extension has a picture");
    }
}
