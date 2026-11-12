//! The mask key: the ONE key this layer ever sends.
//!
//! WHY. Windows opens its Start menu (and lights up a program's menu bar) when the Windows key or Alt goes down and comes up with no
//! other key between. When the layer takes a tap of the Windows key for its own Start, or lets go of Alt after Alt+Tab, it puts one
//! harmless key between the press and the release - virtual key 0xE8, which no keyboard has and no program uses - so that the
//! system sees "something happened in between" and keeps Windows' own Start shut. (AutoHotkey does the same; its documentation
//! recommends the same key.) Down and up go in one `SendInput` call, so nothing else can slip between them.
//!
//! It is sent from inside the hook callback, before the release is let through: the system has not yet committed the release being
//! processed, so the mask lands before it. Whether that holds on this Windows build is the first thing the live test looks at.
//! The event comes back through the hook flagged as injected, which every part of the layer ignores. `dwExtraInfo` carries a tag
//! as well, for anybody who wants to tell it from other injected keys.
//!
//! This is the only place in the product's keyboard layer that sends input; a test scans the sources for it.

/// The key that is sent: nothing on a keyboard, nothing a program reacts to.
pub const MASK_VK: u16 = 0xE8;

/// `dwExtraInfo` of the mask key's events: "RFKY".
pub const MASK_TAG: usize = 0x5246_4B59;

/// Something that can send the mask key.
pub trait MaskSender: Send + Sync {
    /// Sends the mask key (down and up). False when the system did not take both events (a window of a higher integrity level can
    /// refuse input from a program of a lower one).
    fn send(&self) -> bool;
}

/// The real thing: `SendInput` of the mask key. Constructed by the shell host; never by a test.
pub struct SendInputMask;

impl MaskSender for SendInputMask {
    fn send(&self) -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY};
        let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(MASK_VK), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: MASK_TAG } },
        };
        let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
        // SAFETY: `inputs` is a valid slice of INPUT and the size is the size of one INPUT.
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) as usize == inputs.len() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_key_is_the_one_no_keyboard_has() {
        assert_eq!(MASK_VK, 0xE8);
        // not a letter, a digit, a modifier, a function key or anything the chord or the Windows-key machine looks at
        for used in [0x09u16, 0x0D, 0x10, 0x11, 0x12, 0x1B, 0x25, 0x26, 0x27, 0x28, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5] {
            assert_ne!(MASK_VK, used);
        }
    }

    #[test]
    fn the_real_sender_is_a_unit_value_that_does_nothing_until_it_is_asked() {
        // (constructed only: asking it would put a key into the user's input)
        let _ = SendInputMask;
        fn is_sync<T: Send + Sync>() {}
        is_sync::<SendInputMask>();
    }
}
