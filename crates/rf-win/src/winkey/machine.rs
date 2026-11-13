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
//! WIN+SHIFT+S (our screen snip, `rf-app/src/snip/`) is the one chord with a modifier: the Windows key held alone (a Shift may be held
//! before it or pressed after it, a Ctrl or an Alt may not), then S while Shift is down. Any other key in between, or a Shift that was
//! let go before S, makes it Windows' business again. Like the other two chords it is only SAID here; the layer decides.
//!
//! The machine is a few booleans; feeding it allocates nothing and takes no lock.

pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;

/// Win+E, Win+D and Win+Shift+S, the three chords of Explorer's that the layer takes over when no Explorer is the shell
/// (`shell_win_key`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combo {
    /// Win+E: a file window.
    Explorer,
    /// Win+D: show the desktop.
    Desktop,
    /// Win+Shift+S: the screen snip.
    Snip,
}

pub const VK_E: u32 = 0x45;
pub const VK_D: u32 = 0x44;
pub const VK_S: u32 = 0x53;
pub const VK_SHIFT: u32 = 0x10;
pub const VK_LSHIFT: u32 = 0xA0;
pub const VK_RSHIFT: u32 = 0xA1;

/// Whether `vk` is one of the Shift keys.
pub const fn is_shift(vk: u32) -> bool {
    vk == VK_SHIFT || vk == VK_LSHIFT || vk == VK_RSHIFT
}

/// The chord a letter key makes with a Windows key alone, if it is one of ours (Win+Shift+S needs the Shift and is decided in
/// [`WinMachine::feed_ex`]).
pub const fn combo_of(vk: u32) -> Option<Combo> {
    match vk {
        VK_E => Some(Combo::Explorer),
        VK_D => Some(Combo::Desktop),
        _ => None,
    }
}

/// Whether `vk` is one of the two Windows keys.
pub const fn is_win(vk: u32) -> bool {
    vk == VK_LWIN || vk == VK_RWIN
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinOutcome {
    Nothing,
    /// The release of a clean tap: this is the moment to open Start.
    Tap,
    /// A chord of ours was pressed: the Windows key is down alone (no Shift, Ctrl or Alt, no other key before) and `E` or `D` went down.
    /// The machine only SAYS so; whether the key is taken is the layer's decision (the setting, the shell, standing down).
    Combo(Combo),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WinMachine {
    left: bool,
    right: bool,
    /// The press that began the current hold was seen by this machine.
    armed: bool,
    /// Nothing else happened since that press.
    clean: bool,
    /// Nothing but chords of ours (E, D) happened since that press, and no modifier was held at it: a further chord of ours is one.
    combo_ok: bool,
    /// The key of the chord reported last and still held: its auto-repeats are not new chords.
    last_combo: Option<u32>,
    /// Win+Shift+S is still possible: no Ctrl or Alt was held at the press, nothing but Shift keys has been pressed since.
    snip_ok: bool,
}

impl WinMachine {
    pub const fn new() -> WinMachine {
        WinMachine { left: false, right: false, armed: false, clean: false, combo_ok: false, last_combo: None, snip_ok: false }
    }

    /// The hook came up (or came up again) with these Windows keys already down: their releases are not taps.
    pub const fn seeded(left_down: bool, right_down: bool) -> WinMachine {
        WinMachine { left: left_down, right: right_down, armed: false, clean: false, combo_ok: false, last_combo: None, snip_ok: false }
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
        self.combo_ok = false;
        self.last_combo = None;
        self.snip_ok = false;
    }

    /// The keyboard says no Windows key is down although this machine believed one was (a release it never saw): forget the hold, so
    /// nothing that follows is taken for part of it and the next press of a Windows key starts a fresh one.
    pub fn forget_held(&mut self) {
        *self = WinMachine::new();
    }

    pub const fn any_down(&self) -> bool {
        self.left || self.right
    }

    /// One key event. `injected`: a program sent it. `modifier_held`: Shift, Ctrl or Alt was down at this event (only looked at on a
    /// Windows key's press).
    pub fn feed(&mut self, vk: u32, down: bool, injected: bool, modifier_held: bool) -> WinOutcome {
        self.feed_ex(vk, down, injected, modifier_held, false)
    }

    /// [`feed`](Self::feed) that also knows whether the held modifiers are Shift ALONE (`shift_only`: Shift is down, Ctrl and Alt are
    /// not), which is what Win+Shift+S needs at the press of the Windows key and again at the press of S.
    pub fn feed_ex(&mut self, vk: u32, down: bool, injected: bool, modifier_held: bool, shift_only: bool) -> WinOutcome {
        if injected {
            return WinOutcome::Nothing;
        }
        if !is_win(vk) {
            if !down {
                // (releases of other keys mean nothing, except that the key of a chord of ours may be pressed again)
                if self.last_combo == Some(vk) {
                    self.last_combo = None;
                }
                return WinOutcome::Nothing;
            }
            // another key pressed while a Windows key is down: the hold is a chord, not a tap
            if self.any_down() {
                self.clean = false;
                if self.last_combo == Some(vk) {
                    // the auto-repeat of the chord's key: not a second chord
                    return WinOutcome::Nothing;
                }
                // one Windows key (not both), nothing but our own chords so far, no modifier held now
                if self.combo_ok && !modifier_held && self.left != self.right && let Some(c) = combo_of(vk) {
                    self.last_combo = Some(vk);
                    return WinOutcome::Combo(c);
                }
                if self.snip_ok && self.left != self.right {
                    if is_shift(vk) {
                        // (Shift after the Windows key: not a chord of E or D any more, but still possibly Win+Shift+S)
                        self.combo_ok = false;
                        return WinOutcome::Nothing;
                    }
                    if vk == VK_S && shift_only {
                        self.last_combo = Some(vk);
                        return WinOutcome::Combo(Combo::Snip);
                    }
                }
                self.combo_ok = false;
                self.snip_ok = false;
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
                self.combo_ok = !modifier_held;
                self.snip_ok = !modifier_held || shift_only;
                self.last_combo = None;
            } else {
                // the second Windows key while the first is held
                self.clean = false;
                self.combo_ok = false;
                self.snip_ok = false;
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
        self.combo_ok = false;
        self.snip_ok = false;
        self.last_combo = None;
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

    const KEY_E: u32 = VK_E;
    const KEY_D: u32 = VK_D;

    #[test]
    fn win_and_e_is_a_chord_of_ours_and_the_release_is_no_tap() {
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_LWIN, true), (KEY_E, true), (KEY_E, false), (VK_LWIN, false)]);
        assert_eq!(r, vec![WinOutcome::Nothing, WinOutcome::Combo(Combo::Explorer), WinOutcome::Nothing, WinOutcome::Nothing], "no Start after a chord");
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_RWIN, true), (KEY_D, true), (KEY_D, false), (VK_RWIN, false)]);
        assert_eq!(r[1], WinOutcome::Combo(Combo::Desktop));
        assert_eq!(r[3], WinOutcome::Nothing);
    }

    #[test]
    fn the_auto_repeat_of_the_chords_key_is_not_a_second_chord_but_pressing_it_again_is() {
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_LWIN, true), (KEY_E, true), (KEY_E, true), (KEY_E, true), (KEY_E, false), (KEY_E, true), (KEY_E, false), (VK_LWIN, false)]);
        let chords = r.iter().filter(|o| matches!(o, WinOutcome::Combo(_))).count();
        assert_eq!(chords, 2, "{r:?}");
        assert_eq!(r[2], WinOutcome::Nothing);
        assert_eq!(r[3], WinOutcome::Nothing);
    }

    #[test]
    fn a_second_chord_of_ours_with_the_windows_key_still_held_is_taken_too() {
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_LWIN, true), (KEY_E, true), (KEY_E, false), (KEY_D, true), (KEY_D, false), (VK_LWIN, false)]);
        assert_eq!(r[1], WinOutcome::Combo(Combo::Explorer));
        assert_eq!(r[3], WinOutcome::Combo(Combo::Desktop));
        assert_eq!(r[5], WinOutcome::Nothing);
    }

    #[test]
    fn any_other_key_first_spoils_the_chord_and_it_stays_spoiled_until_the_windows_key_is_up() {
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_LWIN, true), (A, true), (A, false), (KEY_E, true), (KEY_E, false), (VK_LWIN, false)]);
        assert!(r.iter().all(|o| *o == WinOutcome::Nothing), "Win+A+E is Windows' business: {r:?}");
        // and the next hold is fresh
        assert_eq!(run(&mut m, &[(VK_LWIN, true), (KEY_E, true)])[1], WinOutcome::Combo(Combo::Explorer));
    }

    #[test]
    fn modifiers_and_both_windows_keys_and_programs_keys_are_never_our_chord() {
        // Shift held when the Windows key went down
        let mut m = WinMachine::new();
        m.feed(VK_LWIN, true, false, true);
        assert_eq!(m.feed(KEY_E, true, false, true), WinOutcome::Nothing);
        // Shift pressed after the Windows key: Shift itself is another key
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_LWIN, true), (0xA0, true), (KEY_E, true)]);
        assert_eq!(r[2], WinOutcome::Nothing, "Win+Shift+E");
        // both Windows keys
        let mut m = WinMachine::new();
        let r = run(&mut m, &[(VK_LWIN, true), (VK_RWIN, true), (KEY_E, true)]);
        assert_eq!(r[2], WinOutcome::Nothing);
        // an event a program sent
        let mut m = WinMachine::new();
        m.feed(VK_LWIN, true, false, false);
        assert_eq!(m.feed(KEY_E, true, true, false), WinOutcome::Nothing);
        // E alone is nothing
        assert_eq!(WinMachine::new().feed(KEY_E, true, false, false), WinOutcome::Nothing);
    }

    #[test]
    fn a_hold_seen_only_while_the_layer_stood_down_has_no_chord() {
        let mut m = WinMachine::new();
        m.feed(VK_LWIN, true, false, false);
        m.observe_unseen(VK_LWIN, true, false);
        assert_eq!(m.feed(KEY_E, true, false, false), WinOutcome::Nothing);
        let mut s = WinMachine::seeded(true, false);
        assert_eq!(s.feed(KEY_E, true, false, false), WinOutcome::Nothing, "a key already down when the hook came up");
    }

    // ---------------------------------------------------------------- Win+Shift+S

    const KEY_S: u32 = VK_S;

    /// Feeds events with the modifier state a real keyboard would show: a Shift is held from its own press to its release.
    fn feed_snip(m: &mut WinMachine, events: &[(u32, bool)]) -> Vec<WinOutcome> {
        let mut shift = false;
        events
            .iter()
            .map(|(vk, down)| {
                let before = shift;
                if is_shift(*vk) {
                    shift = *down;
                }
                // (the Shift's own press does not count itself as held)
                let held = if is_shift(*vk) { before } else { shift };
                m.feed_ex(*vk, *down, false, held, held)
            })
            .collect()
    }

    #[test]
    fn win_then_shift_then_s_is_the_snip_and_the_release_is_no_tap() {
        let mut m = WinMachine::new();
        let r = feed_snip(&mut m, &[(VK_LWIN, true), (VK_LSHIFT, true), (KEY_S, true), (KEY_S, false), (VK_LSHIFT, false), (VK_LWIN, false)]);
        assert_eq!(r[2], WinOutcome::Combo(Combo::Snip), "{r:?}");
        assert_eq!(r.iter().filter(|o| **o != WinOutcome::Nothing).count(), 1, "nothing else is said, and no Start after the chord");
        assert!(!m.any_down());
    }

    #[test]
    fn shift_first_then_win_then_s_is_the_snip_too() {
        let mut m = WinMachine::new();
        let r = feed_snip(&mut m, &[(VK_LSHIFT, true), (VK_RWIN, true), (KEY_S, true), (KEY_S, false)]);
        assert_eq!(r[2], WinOutcome::Combo(Combo::Snip), "{r:?}");
    }

    #[test]
    fn either_shift_and_either_windows_key_will_do() {
        for (win, shift) in [(VK_LWIN, VK_LSHIFT), (VK_LWIN, VK_RSHIFT), (VK_RWIN, VK_LSHIFT), (VK_RWIN, VK_RSHIFT), (VK_LWIN, VK_SHIFT)] {
            let mut m = WinMachine::new();
            let r = feed_snip(&mut m, &[(win, true), (shift, true), (KEY_S, true)]);
            assert_eq!(r[2], WinOutcome::Combo(Combo::Snip), "{win:#x} {shift:#x}");
        }
    }

    #[test]
    fn win_and_s_without_shift_is_windows_search_and_not_ours() {
        let mut m = WinMachine::new();
        let r = feed_snip(&mut m, &[(VK_LWIN, true), (KEY_S, true), (KEY_S, false), (VK_LWIN, false)]);
        assert!(r.iter().all(|o| *o == WinOutcome::Nothing), "{r:?}");
    }

    #[test]
    fn a_shift_let_go_before_s_makes_it_windows_business() {
        let mut m = WinMachine::new();
        let r = feed_snip(&mut m, &[(VK_LWIN, true), (VK_LSHIFT, true), (VK_LSHIFT, false), (KEY_S, true)]);
        assert_eq!(r[3], WinOutcome::Nothing);
    }

    #[test]
    fn another_key_or_a_ctrl_or_an_alt_anywhere_spoils_the_chord() {
        // Win+Shift+A+S
        let mut m = WinMachine::new();
        let r = feed_snip(&mut m, &[(VK_LWIN, true), (VK_LSHIFT, true), (A, true), (A, false), (KEY_S, true)]);
        assert_eq!(r[4], WinOutcome::Nothing);
        // Ctrl held when the Windows key went down: Win+Ctrl+Shift+S is not ours (the held modifiers are not Shift alone)
        let mut m = WinMachine::new();
        m.feed_ex(VK_LWIN, true, false, true, false);
        assert_eq!(m.feed_ex(VK_LSHIFT, true, false, true, false), WinOutcome::Nothing);
        assert_eq!(m.feed_ex(KEY_S, true, false, true, false), WinOutcome::Nothing);
        // Ctrl pressed after the Windows key
        let mut m = WinMachine::new();
        let r = feed_snip(&mut m, &[(VK_LWIN, true), (0xA2, true), (VK_LSHIFT, true), (KEY_S, true)]);
        assert_eq!(r[3], WinOutcome::Nothing);
        // both Windows keys
        let mut m = WinMachine::new();
        let r = feed_snip(&mut m, &[(VK_LWIN, true), (VK_RWIN, true), (VK_LSHIFT, true), (KEY_S, true)]);
        assert_eq!(r[3], WinOutcome::Nothing);
        // S and Shift alone, no Windows key
        let mut m = WinMachine::new();
        let r = feed_snip(&mut m, &[(VK_LSHIFT, true), (KEY_S, true)]);
        assert!(r.iter().all(|o| *o == WinOutcome::Nothing));
    }

    #[test]
    fn the_auto_repeat_of_s_is_not_a_second_snip_but_pressing_it_again_is() {
        let mut m = WinMachine::new();
        let r = feed_snip(&mut m, &[(VK_LWIN, true), (VK_LSHIFT, true), (KEY_S, true), (KEY_S, true), (KEY_S, true), (KEY_S, false), (KEY_S, true), (KEY_S, false)]);
        assert_eq!(r.iter().filter(|o| matches!(o, WinOutcome::Combo(Combo::Snip))).count(), 2, "{r:?}");
        assert_eq!(r[3], WinOutcome::Nothing);
    }

    #[test]
    fn win_shift_e_is_still_windows_business_and_win_e_is_still_ours() {
        let mut m = WinMachine::new();
        let r = feed_snip(&mut m, &[(VK_LWIN, true), (VK_LSHIFT, true), (KEY_E, true)]);
        assert_eq!(r[2], WinOutcome::Nothing, "Win+Shift+E");
        // the snip does not leave the machine in a state where the next plain Win+E is lost
        let mut m = WinMachine::new();
        feed_snip(&mut m, &[(VK_LWIN, true), (VK_LSHIFT, true), (KEY_S, true), (KEY_S, false), (VK_LSHIFT, false), (VK_LWIN, false)]);
        let r = feed_snip(&mut m, &[(VK_LWIN, true), (KEY_E, true)]);
        assert_eq!(r[1], WinOutcome::Combo(Combo::Explorer));
    }

    #[test]
    fn a_hold_seen_only_while_the_layer_stood_down_has_no_snip() {
        let mut m = WinMachine::new();
        m.feed_ex(VK_LWIN, true, false, false, false);
        m.observe_unseen(VK_LWIN, true, false);
        assert_eq!(m.feed_ex(KEY_S, true, false, true, true), WinOutcome::Nothing);
        let mut s = WinMachine::seeded(true, false);
        assert_eq!(s.feed_ex(KEY_S, true, false, true, true), WinOutcome::Nothing, "a key already down when the hook came up");
    }

    #[test]
    fn an_event_a_program_sent_is_never_the_snip() {
        let mut m = WinMachine::new();
        m.feed_ex(VK_LWIN, true, false, false, false);
        m.feed_ex(VK_LSHIFT, true, false, false, false);
        assert_eq!(m.feed_ex(KEY_S, true, true, true, true), WinOutcome::Nothing);
    }
}
