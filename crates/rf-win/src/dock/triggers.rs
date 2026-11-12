//! Keystrokes the dock sends to Windows on the user's behalf: the Windows key (the Start menu), Win+A (quick settings) and Win+N
//! (the notification centre and calendar).
//!
//! ONLY as the direct answer to the user's click on the element that stands for it. Never at start-up, never from a timer, never to
//! "warm something up": a key that was not asked for is a surprise in whatever window is in front. Each press goes out in ONE
//! `SendInput` call, key down and key up together, with the modifier released at the end, so no other input can slip in between and
//! the Windows key is never left held.

/// One step of a key sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStep {
    /// Virtual-key code.
    pub vk: u16,
    pub up: bool,
}

pub const VK_LWIN: u16 = 0x5B;
pub const VK_A: u16 = 0x41;
pub const VK_N: u16 = 0x4E;

/// The steps of "hold the Windows key, tap `key`, release the Windows key" (no `key`: the Windows key alone).
pub fn win_chord(key: Option<u16>) -> Vec<KeyStep> {
    let mut v = vec![KeyStep { vk: VK_LWIN, up: false }];
    if let Some(k) = key {
        v.push(KeyStep { vk: k, up: false });
        v.push(KeyStep { vk: k, up: true });
    }
    v.push(KeyStep { vk: VK_LWIN, up: true });
    v
}

/// Whether every key that goes down comes up again, last pressed first released (the sequence leaves nothing held).
pub fn is_balanced(steps: &[KeyStep]) -> bool {
    let mut held: Vec<u16> = Vec::new();
    for s in steps {
        if s.up {
            // a key may be released while a later one is still held (a tap inside a chord), but it has to be held
            match held.iter().rposition(|&k| k == s.vk) {
                Some(i) => {
                    held.remove(i);
                }
                None => return false,
            }
        } else {
            held.push(s.vk);
        }
    }
    held.is_empty()
}

/// The Start menu: a tap of the Windows key.
pub fn start_menu_steps() -> Vec<KeyStep> {
    win_chord(None)
}

/// Quick settings (network, sound, brightness): Win+A.
pub fn quick_settings_steps() -> Vec<KeyStep> {
    win_chord(Some(VK_A))
}

/// The notification centre and the calendar: Win+N.
pub fn notification_center_steps() -> Vec<KeyStep> {
    win_chord(Some(VK_N))
}

/// Sends the steps in one `SendInput` call. Whether all of them were accepted (a window of a higher integrity level can block the
/// input; then nothing was sent).
pub fn send(steps: &[KeyStep]) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY};
    if steps.is_empty() || !is_balanced(steps) {
        return false;
    }
    let inputs: Vec<INPUT> = steps
        .iter()
        .map(|s| {
            // the Windows keys are "extended" keys
            let mut flags = if s.vk == VK_LWIN { KEYEVENTF_EXTENDEDKEY.0 } else { 0 };
            if s.up {
                flags |= KEYEVENTF_KEYUP.0;
            }
            INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(s.vk), wScan: 0, dwFlags: KEYBD_EVENT_FLAGS(flags), time: 0, dwExtraInfo: 0 } } }
        })
        .collect();
    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) as usize == inputs.len() }
}

/// The user clicked the Windows logo: open the Start menu.
pub fn start_menu() -> bool {
    send(&start_menu_steps())
}

/// The user clicked the network or the volume glyph: open quick settings.
pub fn quick_settings() -> bool {
    send(&quick_settings_steps())
}

/// The user clicked the clock or the bell: open the notification centre.
pub fn notification_center() -> bool {
    send(&notification_center_steps())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(vk: u16, up: bool) -> KeyStep {
        KeyStep { vk, up }
    }

    #[test]
    fn the_start_menu_is_a_tap_of_the_windows_key() {
        assert_eq!(start_menu_steps(), vec![k(VK_LWIN, false), k(VK_LWIN, true)]);
    }

    #[test]
    fn quick_settings_and_notifications_are_win_chords_released_in_reverse() {
        assert_eq!(quick_settings_steps(), vec![k(VK_LWIN, false), k(VK_A, false), k(VK_A, true), k(VK_LWIN, true)]);
        assert_eq!(notification_center_steps(), vec![k(VK_LWIN, false), k(VK_N, false), k(VK_N, true), k(VK_LWIN, true)]);
    }

    #[test]
    fn every_sequence_leaves_nothing_held() {
        for s in [start_menu_steps(), quick_settings_steps(), notification_center_steps(), win_chord(Some(0x41))] {
            assert!(is_balanced(&s), "{s:?}");
        }
    }

    #[test]
    fn unbalanced_sequences_are_recognised_and_never_sent() {
        assert!(!is_balanced(&[k(VK_LWIN, false)]));
        assert!(!is_balanced(&[k(VK_LWIN, true)]));
        assert!(!is_balanced(&[k(VK_LWIN, false), k(VK_A, false), k(VK_LWIN, true)]));
        // `send` refuses before it touches the input stream, so this is safe to run
        assert!(!send(&[]));
        assert!(!send(&[k(VK_LWIN, false)]));
    }
}
