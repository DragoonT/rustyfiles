//! The Windows-key machine: tells a clean TAP of the Windows key from everything else. Pure. It is fed the key events the keyboard
//! hook sees and answers [`WinOutcome::Tap`] on the release that ends a tap.
//!
//! WHAT A TAP IS. The Windows key goes down and comes up with no other key pressed in between, and no modifier was held when it went
//! down. Either Windows key counts; both together are not a tap. Auto-repeat of the held key (more "down" events of the same key) is
//! not another key. Held for a long time, it is still one tap: Windows opens Start on that release too.
//!
//! WHAT IT NEVER DOES. It never decides to swallow anything: the Windows key's own events always pass (the caller's rule, see
//! `layer.rs`), and a chord with the key (Win+E, Win+D, ...) is "another key pressed in between", so it is never a tap and the
//! chord reaches Windows untouched.
//!
//! WHAT IT IGNORES. Events a program sent (`LLKHF_INJECTED`: the dock's own Start button sends the Windows key this way when its
//! own-Start setting is off, and the layer's mask key comes back through the hook this way). A key that was already down when the
//! hook came up: its release is no tap, because the press was not seen and so "nothing in between" is not known.
//!
//! The machine is a few booleans; feeding it allocates nothing and takes no lock.

pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;

/// Whether `vk` is one of the two Windows keys.
pub const fn is_win(vk: u32) -> bool {
    vk == VK_LWIN || vk == VK_RWIN
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinOutcome {
    Nothing,
    /// The release of a clean tap: this is the moment to open Start.
    Tap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WinMachine {
    left: bool,
    right: bool,
    /// The press that began the current hold was seen by this machine.
    armed: bool,
    /// Nothing else happened since that press.
    clean: bool,
}

impl WinMachine {
    pub const fn new() -> WinMachine {
        WinMachine { left: false, right: false, armed: false, clean: false }
    }

    /// The hook came up (or came up again) with these Windows keys already down: their releases are not taps.
    pub const fn seeded(left_down: bool, right_down: bool) -> WinMachine {
        WinMachine { left: left_down, right: right_down, armed: false, clean: false }
    }

    /// A key event while the layer is NOT looking (it stands down, or the setting is off). The machine only learns which Windows keys
    /// are held, and that it did not see what they were held with: such a hold is never a tap, however it ends, and an auto-repeat of
    /// the held key after the layer is back is not a fresh press. (Forgetting the held key instead would make that first auto-repeat
    /// look like a clean press, and Win+arrow, held through a stand-down, would end as a tap.)
    pub fn observe_unseen(&mut self, vk: u32, down: bool, injected: bool) {
        if injected || !is_win(vk) {
            return;
        }
        if vk == VK_LWIN {
            self.left = down;
        } else {
            self.right = down;
        }
        self.armed = false;
        self.clean = false;
    }

    pub const fn any_down(&self) -> bool {
        self.left || self.right
    }

    /// One key event. `injected`: a program sent it. `modifier_held`: Shift, Ctrl or Alt was down at this event (only looked at on a
    /// Windows key's press).
    pub fn feed(&mut self, vk: u32, down: bool, injected: bool, modifier_held: bool) -> WinOutcome {
        if injected {
            return WinOutcome::Nothing;
        }
        if !is_win(vk) {
            // another key pressed while a Windows key is down: the hold is a chord, not a tap (releases of other keys mean nothing)
            if down && self.any_down() {
                self.clean = false;
            }
            return WinOutcome::Nothing;
        }
        let slot = if vk == VK_LWIN { &mut self.left } else { &mut self.right };
        if down {
            if *slot {
                // auto-repeat of the key that is held: not a new press
                return WinOutcome::Nothing;
            }
            let first = !(self.left || self.right);
            if vk == VK_LWIN {
                self.left = true;
            } else {
                self.right = true;
            }
            if first {
                self.armed = true;
                self.clean = !modifier_held;
            } else {
                // the second Windows key while the first is held
                self.clean = false;
            }
            return WinOutcome::Nothing;
        }
        // a release
        if !*slot {
            // (its press was never seen)
            return WinOutcome::Nothing;
        }
        *slot = false;
        if self.left || self.right {
            return WinOutcome::Nothing;
        }
        let tap = self.armed && self.clean;
        self.armed = false;
        self.clean = false;
        if tap { WinOutcome::Tap } else { WinOutcome::Nothing }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u32 = 0x41;
    const E8: u32 = 0xE8;

    fn run(m: &mut WinMachine, events: &[(u32, bool)]) -> Vec<WinOutcome> {
        events.iter().map(|(vk, down)| m.feed(*vk, *down, false, false)).collect()
    }

    #[test]
    fn a_press_and_release_of_the_left_windows_key_alone_is_a_tap() {
        let mut m = WinMachine::new();
        assert_eq!(run(&mut m, &[(VK_LWIN, true), (VK_LWIN, false)]), vec![WinOutcome::Nothing, WinOutcome::Tap]);
        assert!(!m.any_down());
    }

    #[test]
    fn the_right_windows_key_taps_too() {
        let mut m = WinMachine::new();
        assert_eq!(run(&mut m, &[(VK_RWIN, true), (VK_RWIN, false)]), vec![WinOutcome::Nothing, WinOutcome::Tap]);
    }

    #[test]
    fn a_letter_between_press_and_release_makes_it_a_chord_and_not_a_tap() {
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_LWIN, true), (A, true), (A, false), (VK_LWIN, false)]);
        assert!(r.iter().all(|o| *o == WinOutcome::Nothing));
        // and the next tap is a tap again
        assert_eq!(run(&mut m, &[(VK_LWIN, true), (VK_LWIN, false)])[1], WinOutcome::Tap);
    }

    #[test]
    fn a_modifier_pressed_between_also_makes_it_a_chord() {
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_LWIN, true), (0xA0, true), (0xA0, false), (VK_LWIN, false)]);
        assert!(r.iter().all(|o| *o == WinOutcome::Nothing));
    }

    #[test]
    fn a_modifier_held_when_the_key_goes_down_is_not_a_clean_tap() {
        let mut m = WinMachine::new();
        assert_eq!(m.feed(VK_LWIN, true, false, true), WinOutcome::Nothing);
        assert_eq!(m.feed(VK_LWIN, false, false, false), WinOutcome::Nothing);
    }

    #[test]
    fn auto_repeat_of_the_held_key_is_not_another_key_and_the_hold_is_one_tap() {
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_LWIN, true), (VK_LWIN, true), (VK_LWIN, true), (VK_LWIN, false)]);
        assert_eq!(r.iter().filter(|o| **o == WinOutcome::Tap).count(), 1);
        assert_eq!(r[3], WinOutcome::Tap);
    }

    #[test]
    fn both_windows_keys_together_are_not_a_tap() {
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_LWIN, true), (VK_RWIN, true), (VK_LWIN, false), (VK_RWIN, false)]);
        assert!(r.iter().all(|o| *o == WinOutcome::Nothing));
        assert!(!m.any_down());
    }

    #[test]
    fn a_key_that_was_already_down_when_the_hook_came_up_is_not_a_tap() {
        let mut m = WinMachine::seeded(true, false);
        assert_eq!(m.feed(VK_LWIN, false, false, false), WinOutcome::Nothing, "its press was not seen");
        // the machine is usable at once afterwards
        assert_eq!(run(&mut m, &[(VK_LWIN, true), (VK_LWIN, false)])[1], WinOutcome::Tap);
        // and a seeded key that is pressed again while still down (auto-repeat) does not arm it
        let mut s = WinMachine::seeded(true, false);
        assert_eq!(s.feed(VK_LWIN, true, false, false), WinOutcome::Nothing);
        assert_eq!(s.feed(VK_LWIN, false, false, false), WinOutcome::Nothing);
    }

    #[test]
    fn a_release_with_no_press_is_nothing() {
        let mut m = WinMachine::new();
        assert_eq!(m.feed(VK_RWIN, false, false, false), WinOutcome::Nothing);
    }

    #[test]
    fn events_a_program_sent_are_ignored_whatever_they_are() {
        // the dock's own Start button sends the Windows key; the layer's mask key comes back; neither is the user's tap
        let mut m = WinMachine::new();
        assert_eq!(m.feed(VK_LWIN, true, true, false), WinOutcome::Nothing);
        assert_eq!(m.feed(VK_LWIN, false, true, false), WinOutcome::Nothing);
        assert!(!m.any_down());
        // and the mask key in the middle of a real tap does not spoil it
        let mut t = WinMachine::new();
        t.feed(VK_LWIN, true, false, false);
        assert_eq!(t.feed(E8, true, true, false), WinOutcome::Nothing);
        assert_eq!(t.feed(E8, false, true, false), WinOutcome::Nothing);
        assert_eq!(t.feed(VK_LWIN, false, false, false), WinOutcome::Tap);
    }

    #[test]
    fn keys_pressed_when_no_windows_key_is_down_change_nothing() {
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(A, true), (A, false), (VK_LWIN, true), (VK_LWIN, false)]);
        assert_eq!(r[3], WinOutcome::Tap);
    }

    #[test]
    fn a_hold_seen_only_while_the_layer_stood_down_is_never_a_tap_and_its_auto_repeat_does_not_arm_the_machine() {
        let mut m = WinMachine::new();
        m.feed(VK_LWIN, true, false, false);
        // the layer stands down: the key stays held in the machine's mind, unseen
        m.observe_unseen(VK_LWIN, true, false);
        m.observe_unseen(A, true, false);
        assert!(m.any_down());
        // back again: an auto-repeat is not a new press, the release is not a tap
        assert_eq!(m.feed(VK_LWIN, true, false, false), WinOutcome::Nothing);
        assert_eq!(m.feed(VK_LWIN, false, false, false), WinOutcome::Nothing);
        assert!(!m.any_down());
        // and the next real tap is one
        assert_eq!(run(&mut m, &[(VK_LWIN, true), (VK_LWIN, false)])[1], WinOutcome::Tap);
        // an unseen release clears the key; an injected event is ignored
        let mut u = WinMachine::new();
        u.observe_unseen(VK_RWIN, true, false);
        u.observe_unseen(VK_RWIN, false, false);
        assert!(!u.any_down());
        u.observe_unseen(VK_RWIN, true, true);
        assert!(!u.any_down());
    }

    #[test]
    fn the_machine_is_plain_copy_data() {
        fn is_copy<T: Copy>() {}
        is_copy::<WinMachine>();
    }
}
