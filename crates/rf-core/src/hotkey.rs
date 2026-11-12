//! The "Shortcut key" of a `.lnk` file: which combinations can be given, and the word that
//! `IShellLinkW::SetHotkey` stores for them (low byte = virtual key, high byte = HOTKEYF_*
//! modifier flags), in both directions between that word and its text ("Ctrl + Alt + K").
//!
//! Pure logic, no Windows calls, so the rules can be tested. The rules follow the Windows hot key
//! control that Explorer's Shortcut tab uses (`HKM_SETRULES`): a key typed alone, or with Shift
//! only, is completed to Ctrl + Alt; Backspace clears the field. Which combinations Windows then
//! really honours at run time was not run against the real shell (see docs/design/recycle-bin.md).

/// HOTKEYF_SHIFT, HOTKEYF_CONTROL, HOTKEYF_ALT, HOTKEYF_EXT (extended key: arrows, Insert, ...).
pub const SHIFT: u8 = 0x01;
pub const CTRL: u8 = 0x02;
pub const ALT: u8 = 0x04;
pub const EXT: u8 = 0x08;

/// The modifiers held when a key was pressed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Held {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Windows key: a link's hot key cannot carry it.
    pub win: bool,
}

impl Held {
    fn flags(self) -> u8 {
        (if self.ctrl { CTRL } else { 0 }) | (if self.alt { ALT } else { 0 }) | (if self.shift { SHIFT } else { 0 })
    }
}

/// Why a combination is not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A modifier on its own (Ctrl, Alt, Shift, Win).
    OnlyModifier,
    /// The Windows key is part of it.
    WinKey,
    /// A key a shortcut key cannot be (Esc, Tab, Enter, Print Screen, punctuation that depends on the keyboard layout...).
    KeyNotAllowed,
    /// Windows keeps this combination for itself (Alt + F4, Ctrl + Alt + Del, Alt + Space).
    Reserved,
}

/// What a key press in the field does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Backspace / Delete: the field is emptied.
    Clear,
    /// The word to store.
    Set(u16),
    Refused(Refusal),
}

pub fn word(vk: u8, mods: u8) -> u16 {
    ((mods as u16) << 8) | vk as u16
}

const VK_BACK: u8 = 0x08;
const VK_DELETE: u8 = 0x2e;
const VK_SPACE: u8 = 0x20;
const VK_F4: u8 = 0x73;

/// Keys that need HOTKEYF_EXT: the Windows hot key control sets it for the extended keys.
fn is_extended(vk: u8) -> bool {
    matches!(vk, 0x21..=0x28 | 0x2d | 0x2e | 0x6f)
}

/// Keys a shortcut key can be: letters, digits, F1-F24, the number pad, Space, and the navigation keys.
fn allowed_key(vk: u8) -> bool {
    matches!(vk, b'0'..=b'9' | b'A'..=b'Z' | 0x70..=0x87 | 0x60..=0x6b | 0x6d..=0x6f | VK_SPACE | 0x21..=0x28 | 0x2d | VK_DELETE)
}

/// What pressing `vk` with `held` does in the field.
pub fn decide(vk: u8, held: Held) -> Decision {
    if matches!(vk, 0x10 | 0x11 | 0x12 | 0x5b | 0x5c | 0xa0..=0xa5) {
        return Decision::Refused(Refusal::OnlyModifier);
    }
    if held.win {
        return Decision::Refused(Refusal::WinKey);
    }
    let mods = held.flags();
    if mods == 0 && (vk == VK_BACK || vk == VK_DELETE) {
        return Decision::Clear;
    }
    if !allowed_key(vk) {
        return Decision::Refused(Refusal::KeyNotAllowed);
    }
    // the control's rule: no modifier, or Shift alone, is not a valid combination; it is completed to Ctrl + Alt
    let mods = if mods & (CTRL | ALT) == 0 { CTRL | ALT } else { mods };
    let reserved = (mods & ALT != 0 && mods & CTRL == 0 && (vk == VK_F4 || vk == VK_SPACE)) || (mods & (CTRL | ALT) == (CTRL | ALT) && vk == VK_DELETE);
    if reserved {
        return Decision::Refused(Refusal::Reserved);
    }
    Decision::Set(word(vk, mods | if is_extended(vk) { EXT } else { 0 }))
}

/// The virtual key for the key name GPUI gives a key press ("a", "5", "f5", "pageup", "space"...), when it can be a shortcut key
/// or one of the keys the field reacts to ("backspace", "delete"). Number-pad keys are not told apart from the main row by GPUI.
pub fn vk_from_key_name(key: &str) -> Option<u8> {
    let b = key.as_bytes();
    if b.len() == 1 {
        return match b[0] {
            c @ b'a'..=b'z' => Some(c.to_ascii_uppercase()),
            c @ b'0'..=b'9' => Some(c),
            _ => None,
        };
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(0x6f + n);
    }
    Some(match key {
        "backspace" => VK_BACK,
        "delete" => VK_DELETE,
        "space" => VK_SPACE,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2d,
        // keys the field names in its refusal rather than ignoring
        "escape" => 0x1b,
        "tab" => 0x09,
        "enter" => 0x0d,
        "control" => 0x11,
        "alt" => 0x12,
        "shift" => 0x10,
        "platform" => 0x5b,
        _ => return None,
    })
}

fn key_text(vk: u8) -> String {
    match vk {
        b'0'..=b'9' | b'A'..=b'Z' => (vk as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6f),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        0x6a => "Num *".into(),
        0x6b => "Num +".into(),
        0x6d => "Num -".into(),
        0x6e => "Num .".into(),
        0x6f => "Num /".into(),
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0d => "Enter".into(),
        0x1b => "Esc".into(),
        0x20 => "Space".into(),
        0x21 => "Page Up".into(),
        0x22 => "Page Down".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0x2d => "Insert".into(),
        0x2e => "Delete".into(),
        _ => format!("0x{vk:02X}"),
    }
}

/// "Ctrl + Alt + K" for a stored word (empty for 0 or a word without a key).
pub fn text(word: u16) -> String {
    let vk = (word & 0xff) as u8;
    if vk == 0 {
        return String::new();
    }
    let mods = (word >> 8) as u8;
    let mut parts: Vec<String> = Vec::new();
    if mods & CTRL != 0 {
        parts.push("Ctrl".into());
    }
    if mods & SHIFT != 0 {
        parts.push("Shift".into());
    }
    if mods & ALT != 0 {
        parts.push("Alt".into());
    }
    parts.push(key_text(vk));
    parts.join(" + ")
}

/// The word for text written the way [`text`] writes it (modifiers in any order, any case, "+" with or without spaces).
/// `None` for empty text or text that is not a combination. The text is not judged by [`decide`]: a word that a hot key
/// control would never produce still parses.
pub fn parse(s: &str) -> Option<u16> {
    let mut mods = 0u8;
    let mut vk = None;
    // "Num +" has a plus in its name: take it out before splitting
    let norm = s.trim().replace("Num +", "Num\u{1}plus").replace("Num -", "Num\u{1}minus").replace("Num *", "Num\u{1}star").replace("Num /", "Num\u{1}slash").replace("Num .", "Num\u{1}dot");
    for part in norm.split('+') {
        let part = part.trim().replace('\u{1}', " ");
        if part.is_empty() {
            return None;
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= CTRL,
            "shift" => mods |= SHIFT,
            "alt" => mods |= ALT,
            name => {
                if vk.is_some() {
                    return None;
                }
                vk = Some(vk_from_text(name)?);
            }
        }
    }
    let vk = vk?;
    Some(word(vk, mods | if is_extended(vk) { EXT } else { 0 }))
}

fn vk_from_text(name: &str) -> Option<u8> {
    let b = name.as_bytes();
    if b.len() == 1 {
        return match b[0] {
            c @ b'a'..=b'z' => Some(c.to_ascii_uppercase()),
            c @ b'0'..=b'9' => Some(c),
            _ => None,
        };
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(0x6f + n);
    }
    if let Some(n) = name.strip_prefix("num ") {
        return match n {
            "+" | "plus" => Some(0x6b),
            "-" | "minus" => Some(0x6d),
            "*" | "star" => Some(0x6a),
            "/" | "slash" => Some(0x6f),
            "." | "dot" => Some(0x6e),
            d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => Some(0x60 + (d.as_bytes()[0] - b'0')),
            _ => None,
        };
    }
    Some(match name {
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" => 0x0d,
        "esc" => 0x1b,
        "space" => 0x20,
        "page up" => 0x21,
        "page down" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2d,
        "delete" => 0x2e,
        _ => {
            let hex = name.strip_prefix("0x")?;
            u8::from_str_radix(hex, 16).ok().filter(|&v| v != 0)?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn held(ctrl: bool, alt: bool, shift: bool) -> Held {
        Held { ctrl, alt, shift, win: false }
    }

    #[test]
    fn a_letter_alone_or_with_shift_is_completed_to_ctrl_alt() {
        assert_eq!(decide(b'K', held(false, false, false)), Decision::Set(word(b'K', CTRL | ALT)));
        assert_eq!(decide(b'K', held(false, false, true)), Decision::Set(word(b'K', CTRL | ALT)));
        assert_eq!(text(word(b'K', CTRL | ALT)), "Ctrl + Alt + K");
    }

    #[test]
    fn modifiers_that_are_given_are_kept() {
        assert_eq!(decide(b'K', held(true, true, false)), Decision::Set(word(b'K', CTRL | ALT)));
        assert_eq!(decide(b'K', held(true, false, true)), Decision::Set(word(b'K', CTRL | SHIFT)));
        assert_eq!(decide(b'7', held(false, true, false)), Decision::Set(word(b'7', ALT)));
        // function keys: alone they too become Ctrl + Alt, with modifiers they stay
        assert_eq!(decide(0x74, held(false, false, false)), Decision::Set(word(0x74, CTRL | ALT)));
        assert_eq!(decide(0x74, held(true, false, true)), Decision::Set(word(0x74, CTRL | SHIFT)));
        assert_eq!(text(word(0x74, CTRL | SHIFT)), "Ctrl + Shift + F5");
        assert_eq!(text(word(0x87, ALT)), "Alt + F24");
    }

    #[test]
    fn extended_keys_carry_the_ext_flag() {
        let Decision::Set(w) = decide(0x25, held(true, true, false)) else { panic!() };
        assert_eq!(w, word(0x25, CTRL | ALT | EXT));
        assert_eq!(text(w), "Ctrl + Alt + Left");
    }

    #[test]
    fn backspace_and_delete_clear_only_when_no_modifier_is_held() {
        assert_eq!(decide(VK_BACK, held(false, false, false)), Decision::Clear);
        assert_eq!(decide(VK_DELETE, held(false, false, false)), Decision::Clear);
        // Ctrl + Delete is a combination, Ctrl + Alt + Delete is Windows'
        assert_eq!(decide(VK_DELETE, held(true, false, false)), Decision::Set(word(VK_DELETE, CTRL | EXT)));
        assert_eq!(decide(VK_DELETE, held(true, true, false)), Decision::Refused(Refusal::Reserved));
        // Backspace with a modifier is not a key a shortcut can be
        assert_eq!(decide(VK_BACK, held(true, false, false)), Decision::Refused(Refusal::KeyNotAllowed));
    }

    #[test]
    fn refused_combinations() {
        for vk in [0x1b, 0x09, 0x0d, 0x2c, 0x13, 0x14, 0xba] {
            assert_eq!(decide(vk, held(true, true, false)), Decision::Refused(Refusal::KeyNotAllowed), "vk {vk:#x}");
        }
        assert_eq!(decide(0x11, held(true, false, false)), Decision::Refused(Refusal::OnlyModifier));
        assert_eq!(decide(b'A', Held { win: true, ..held(true, false, false) }), Decision::Refused(Refusal::WinKey));
        assert_eq!(decide(VK_F4, held(false, true, false)), Decision::Refused(Refusal::Reserved));
        assert_eq!(decide(VK_SPACE, held(false, true, false)), Decision::Refused(Refusal::Reserved));
        // Ctrl + Alt + F4 and Alt + Shift + F4 are not Windows' own
        assert_eq!(decide(VK_F4, held(true, true, false)), Decision::Set(word(VK_F4, CTRL | ALT)));
        assert_eq!(decide(VK_F4, held(true, false, false)), Decision::Set(word(VK_F4, CTRL)));
    }

    #[test]
    fn gpui_key_names_map_to_virtual_keys() {
        assert_eq!(vk_from_key_name("k"), Some(b'K'));
        assert_eq!(vk_from_key_name("5"), Some(b'5'));
        assert_eq!(vk_from_key_name("f1"), Some(0x70));
        assert_eq!(vk_from_key_name("f24"), Some(0x87));
        assert_eq!(vk_from_key_name("f25"), None);
        assert_eq!(vk_from_key_name("pageup"), Some(0x21));
        assert_eq!(vk_from_key_name("backspace"), Some(0x08));
        assert_eq!(vk_from_key_name("escape"), Some(0x1b));
        assert_eq!(vk_from_key_name("ç"), None);
        assert_eq!(vk_from_key_name("-"), None);
        assert_eq!(vk_from_key_name(""), None);
    }

    #[test]
    fn text_and_word_go_both_ways() {
        for vk in (b'A'..=b'Z').chain(b'0'..=b'9').chain(0x70..=0x87).chain(0x60..=0x69).chain([0x6a, 0x6b, 0x6d, 0x6e, 0x6f]).chain(0x20..=0x28).chain([0x2d, 0x2e]) {
            for mods in [0, CTRL, ALT, CTRL | ALT, CTRL | SHIFT, CTRL | ALT | SHIFT, SHIFT] {
                let mods = mods | if is_extended(vk) { EXT } else { 0 };
                let w = word(vk, mods);
                assert_eq!(parse(&text(w)), Some(w), "{}", text(w));
            }
        }
        assert_eq!(parse("ctrl+alt+k"), Some(word(b'K', CTRL | ALT)));
        assert_eq!(parse("  Alt + Ctrl + Num +  "), Some(word(0x6b, CTRL | ALT)));
        assert_eq!(parse("Ctrl + Alt + Page Up"), Some(word(0x21, CTRL | ALT | EXT)));
        assert_eq!(parse(""), None);
        assert_eq!(parse("Ctrl + Alt"), None);
        assert_eq!(parse("Ctrl + A + B"), None);
        assert_eq!(parse("Ctrl + + A"), None);
        assert_eq!(parse("Ctrl + F25"), None);
        assert_eq!(text(0), "");
        assert_eq!(text(word(0xba, CTRL)), "Ctrl + 0xBA");
        assert_eq!(parse("Ctrl + 0xBA"), Some(word(0xba, CTRL)));
    }
}
