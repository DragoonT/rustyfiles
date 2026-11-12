//! GPUI's key events as the chord wants them. Pure.

use rf_win::switcher::chord::{Key, KeyInput};

/// The chord's key for the name GPUI gives a keystroke. The "same app" key is the back quote; on layouts where that key types
/// something else GPUI names it differently and the key is just another key here (the real chord, fed by the hook, goes by the
/// key's scan code and is layout independent).
pub fn key_of(name: &str) -> Key {
    match name {
        "tab" => Key::Tab,
        "`" => Key::Grave,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "enter" => Key::Enter,
        "escape" => Key::Escape,
        "alt" => Key::Alt,
        _ => Key::Other,
    }
}

/// A key press in the switcher's window.
pub fn key_down(name: &str, shift: bool, alt: bool) -> KeyInput {
    KeyInput { key: key_of(name), down: true, shift, alt, injected: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_gpui_gives_map_to_the_chords_keys() {
        assert_eq!(key_of("tab"), Key::Tab);
        assert_eq!(key_of("escape"), Key::Escape);
        assert_eq!(key_of("enter"), Key::Enter);
        assert_eq!(key_of("`"), Key::Grave);
        for (n, k) in [("left", Key::Left), ("right", Key::Right), ("up", Key::Up), ("down", Key::Down)] {
            assert_eq!(key_of(n), k);
        }
        assert_eq!(key_of("a"), Key::Other);
        assert_eq!(key_of(""), Key::Other);
    }

    #[test]
    fn a_press_carries_the_modifiers() {
        let k = key_down("tab", true, true);
        assert!(k.down && k.shift && k.alt && !k.injected);
        assert_eq!(k.key, Key::Tab);
    }
}
