//! The keys that choose an icon size on the desktop (Explorer's Ctrl+Shift+2 / 3 / 4), and what each is called by the keyboard
//! layout in use. Pure: the layout is a parameter (the caller passes `rf_win::keys::shifted_char`, the tests a fake layout).
//!
//! WHY MORE THAN ONE FORM. GPUI reports a Shift+digit chord as the layout's shifted character with no Shift (Turkish Q: Ctrl+Shift+2
//! arrives as `ctrl-"`, Ctrl+Shift+4 as `ctrl-+`), so `ctrl-shift-4` alone never fires there. The table in `menu_plan::SIZE_KEYS` is the
//! layout-independent form; [`layout_keys`] adds the form the layout itself produces.
//!
//! THE CHORDS THAT COLLIDE. A shifted character that is a letter or a digit (AZERTY: Shift+& is `1`) is never bound: `ctrl-1` is a plain
//! Ctrl+digit shortcut. A shifted `+` `-` `=` IS bound here, unlike in the file list, where Ctrl++ is "zoom in" and wins: on a layout
//! where Shift+4 types `+` (Turkish Q) Ctrl+Shift+4 and Ctrl++ are the same chord, and on the desktop the menu says Ctrl+Shift+4 is
//! "small icons", so that is what it must do. The desktop's key context is the deeper one and is bound later, so it wins over the file
//! list's zoom keys there; Ctrl+= and Ctrl+- (a different key on that layout) and Ctrl+wheel still zoom.

use rf_core::desktop::grid::IconSize;

use super::menu_plan::SIZE_KEYS;

/// The virtual key of the digit that chooses each size (2, 3, 4).
const DIGIT_KEYS: [(IconSize, u16); 3] = [(IconSize::Large, 0x32), (IconSize::Medium, 0x33), (IconSize::Small, 0x34)];

/// The layout-independent chords: `ctrl-shift-2` ... (what the menu's hints say).
pub fn plain_keys() -> [(&'static str, IconSize); 3] {
    SIZE_KEYS.map(|(size, keys)| (keys, size))
}

/// The chords the keyboard layout produces for the same three keys: `ctrl-` and the character that Shift+digit types.
/// `shifted` gives that character (lower case) for a virtual key, or `None` when the layout has none.
pub fn layout_keys(shifted: impl Fn(u16) -> Option<String>) -> Vec<(String, IconSize)> {
    DIGIT_KEYS
        .iter()
        .filter_map(|&(size, vk)| {
            let ch = shifted(vk)?;
            // a letter or digit would be a plain Ctrl+key shortcut (AZERTY)
            if ch.chars().any(|c| c.is_alphanumeric()) {
                return None;
            }
            Some((format!("ctrl-{ch}"), size))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout<'a>(pairs: &'a [(u16, &'a str)]) -> impl Fn(u16) -> Option<String> + 'a {
        move |vk| pairs.iter().find(|(k, _)| *k == vk).map(|(_, c)| c.to_string())
    }

    #[test]
    fn the_plain_chords_are_explorers_three() {
        assert_eq!(plain_keys(), [("ctrl-shift-2", IconSize::Large), ("ctrl-shift-3", IconSize::Medium), ("ctrl-shift-4", IconSize::Small)]);
    }

    #[test]
    fn on_turkish_q_all_three_sizes_have_a_key_and_the_third_is_ctrl_plus() {
        // Shift+2 is ", Shift+3 is ^ (a dead key GPUI still reports), Shift+4 is +
        let tr = layout(&[(0x32, "\""), (0x33, "^"), (0x34, "+")]);
        assert_eq!(layout_keys(tr), vec![("ctrl-\"".to_string(), IconSize::Large), ("ctrl-^".to_string(), IconSize::Medium), ("ctrl-+".to_string(), IconSize::Small)]);
    }

    #[test]
    fn on_us_layout_the_symbols_are_bound() {
        let us = layout(&[(0x32, "@"), (0x33, "#"), (0x34, "$")]);
        let k = layout_keys(us);
        assert_eq!(k.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), ["ctrl-@", "ctrl-#", "ctrl-$"]);
    }

    #[test]
    fn on_azerty_a_digit_is_never_bound_as_a_chord() {
        // Shift+é types 2: binding ctrl-2 would take a plain Ctrl+2
        let az = layout(&[(0x32, "2"), (0x33, "3"), (0x34, "4")]);
        assert!(layout_keys(az).is_empty());
    }

    #[test]
    fn every_form_is_a_keystroke_the_toolkit_can_parse() {
        let tr = layout(&[(0x32, "\""), (0x33, "^"), (0x34, "+")]);
        let all = plain_keys().map(|(k, _)| k.to_string()).into_iter().chain(layout_keys(tr).into_iter().map(|(k, _)| k));
        for k in all {
            let ok = gpui_kit::Keystroke::parse_with_os(k.as_str(), gpui_kit::OperatingSystem::Windows).is_ok();
            assert!(ok, "{k}");
        }
    }

    #[test]
    fn a_key_the_layout_does_not_type_is_left_out() {
        let partial = layout(&[(0x34, "+")]);
        assert_eq!(layout_keys(partial), vec![("ctrl-+".to_string(), IconSize::Small)]);
    }
}
