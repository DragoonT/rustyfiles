//! The decision of the keyboard layer: for each key event the hook gives, swallow it or not, and what to tell the interface. This is
//! the CALLBACK of the one hook; it runs on the hook thread and obeys the hook's rules:
//!
//! * it never blocks, allocates, logs, takes a lock or touches COM: it reads and writes the atomics of [`Shared`], its own `Copy`
//!   fields, and posts a `u32` to the queue; the one outward act is the mask key's `SendInput`, which does not wait for anybody;
//! * it decides from the event alone and from state it keeps itself, so it cannot be held up by the interface thread;
//! * a test scans this file for allocation, locks and logging (`mod.rs`).
//!
//! THE RULES, each pinned by a test below with a name that says it:
//! 1. FAIL OPEN: when the interface's heartbeat is stale or was never made, or a full-screen program is in front, every key passes
//!    and nothing is posted for a new press.
//! 2. A key-up follows its key-down: a release is swallowed if and only if the press was (the chord's set of swallowed keys decides,
//!    and it is consulted for every release, also while the layer stands down, and by the key's own identity: Ctrl pressed in between
//!    does not change which key a release is).
//! 3. Alt, the Windows keys and the modifiers themselves are never swallowed, neither pressed nor released. Win+anything passes.
//! 4. Events a program sent (injected) are ignored: the layer never answers its own mask key nor the dock's Start button.
//! 5. When the switcher could not be opened the chord is given up at once: Alt+Tab goes to Windows until Alt is released.
//! 6. LOG ONLY (`RUSTYFILES_KEYS_LOG_ONLY=1`): the layer decides and posts, so that the interface can log, but swallows nothing and
//!    sends no mask key.
//! 7. Stood down (or switched off) in the middle of a chord: the chord is closed and the interface is told (`Lost`) so that it closes
//!    the switcher; the releases of what was swallowed are still swallowed.
//! 8. Win+E and Win+D (`Shared::combos_on`, only while no Explorer is the shell): the press of E or D, with a Windows key alone down
//!    before it, is swallowed and posted; its auto-repeats are swallowed; its release is swallowed because its press was (also after the
//!    layer stood down); ONE mask key is sent for the hold, so that Windows' own Start does not open when the Windows key comes up
//!    (the system never saw a key between the press and the release). The Windows key itself is still never swallowed.

use std::sync::Arc;

use super::machine::{VK_D, VK_E, VK_LWIN, VK_RWIN, WinMachine, WinOutcome};
use super::mask::MaskSender;
use super::queue::Posted;
use super::service::KeyProbe;
use super::shared::{Shared, Tracked};
use crate::switcher::chord::{Chord, Effect, Key};
use crate::switcher::hook::{LLKHF_ALTDOWN, RawKey, VK_LMENU, VK_MENU, VK_RMENU, translate};

/// Which tracked keys reached the system, one flag each.
#[derive(Clone, Copy, Debug, Default)]
struct Held {
    lwin: bool,
    rwin: bool,
    lalt: bool,
    ralt: bool,
}

impl Held {
    fn tracked(&self) -> Tracked {
        Tracked { win: self.lwin || self.rwin, alt: self.lalt || self.ralt }
    }
}

pub struct KeyLayer {
    shared: Arc<Shared>,
    mask: Arc<dyn MaskSender>,
    win: WinMachine,
    chord: Chord,
    /// The number of the current opening of the switcher (0: none yet).
    seq: u16,
    /// The switcher could not be opened: Alt+Tab passes until Alt is released.
    given_up: bool,
    held: Held,
    /// The keys of Win+E and Win+D whose press was swallowed and whose release is still to come: `[E, D]`.
    combo_down: [bool; 2],
    /// The mask key was sent for the current Windows-key hold because a chord of ours took a key.
    combo_masked: bool,
    /// The keyboard's own state, asked once at the press of E or D (see `on_event`); `None` in the layer's unit tests that do not need it.
    probe: Option<Arc<dyn KeyProbe>>,
}

impl KeyLayer {
    /// A layer for a hook that has just come up; `win_left` and `win_right` say which Windows keys are down at this moment.
    pub fn new(shared: Arc<Shared>, mask: Arc<dyn MaskSender>, win_left: bool, win_right: bool) -> KeyLayer {
        KeyLayer {
            shared,
            mask,
            win: WinMachine::seeded(win_left, win_right),
            chord: Chord::new(),
            seq: 0,
            given_up: false,
            held: Held { lwin: win_left, rwin: win_right, ..Held::default() },
            combo_down: [false; 2],
            combo_masked: false,
            probe: None,
        }
    }

    /// The layer confirms a Windows key is really down (the keyboard's own state) before it takes E or D: a belief the hook's events
    /// left stale (a release it never saw: the secure desktop, a stalled hook) must not make it swallow a typed letter.
    pub fn with_probe(mut self, probe: Arc<dyn KeyProbe>) -> KeyLayer {
        self.probe = Some(probe);
        self
    }

    /// One key event. True: the key is swallowed.
    pub fn on_event(&mut self, k: &RawKey) -> bool {
        // the switcher closed by itself, or could not be opened: the chord of that opening is over
        if let Some(r) = self.shared.take_reset()
            && r.seq == self.seq
        {
            self.chord.closed_elsewhere();
            if r.give_up {
                self.given_up = true;
            }
        }
        let stand_down = self.shared.standing_down();
        let log_only = self.shared.log_only();

        // ---- the keys of Win+E / Win+D that were taken: their repeats and their release follow their press, whatever state the layer is in
        if let Some(slot) = combo_slot(k.vk)
            && self.combo_down[slot]
        {
            // (an event a program sent is not the user's key: it is never ours to take, and it is not tracked here. A synthetic key-up
            // of E, sent by a remote-input tool or an IME while the real key is still held, must not end the tracking: the real key's
            // repeats and release would then reach the focused program after the press was swallowed)
            if !k.injected() {
                if !k.down {
                    self.combo_down[slot] = false;
                    // the machine has to see the release too, or the next press of the same key (Windows key still held) is taken for
                    // an auto-repeat of this one and passes to the focused program
                    self.win.feed(k.vk, false, false, false);
                }
                return !log_only;
            }
        }

        // ---- the Windows key
        let mut taken = false;
        if self.shared.win_on() && !stand_down {
            // About to decide on E or D with a Windows key believed down: ask the keyboard once (a cheap read, only on these two keys).
            // If it says neither Windows key is down, the belief is stale: drop it, so the letter passes as the typing it is.
            if k.down && !k.injected() && combo_slot(k.vk).is_some() && self.win.any_down() && let Some(p) = &self.probe && !p.down(VK_LWIN) && !p.down(VK_RWIN) {
                self.win.forget_held();
                self.held.lwin = false;
                self.held.rwin = false;
                self.shared.set_believed(self.held.tracked());
            }
            let modifier_held = k.mods.shift() || k.mods.ctrl() || k.flags & LLKHF_ALTDOWN != 0;
            match self.win.feed(k.vk, k.down, k.injected(), modifier_held) {
                WinOutcome::Tap => {
                    // the mask goes in BEFORE the release is let through; in log-only mode nothing is sent
                    self.send_mask(log_only);
                    self.shared.post(Posted::StartTap);
                }
                WinOutcome::Combo(c) if self.shared.combos_on() => {
                    // one mask for the hold: the system will see the Windows key go down and up with nothing between
                    if !self.combo_masked {
                        self.combo_masked = true;
                        self.send_mask(log_only);
                    }
                    if let Some(slot) = combo_slot(k.vk) {
                        self.combo_down[slot] = true;
                    }
                    self.shared.post(Posted::WinCombo(c));
                    taken = true;
                }
                _ => {}
            }
            if !self.win.any_down() {
                self.combo_masked = false;
            }
        } else {
            // (not looking, but the held Windows keys are still tracked: see the method)
            self.win.observe_unseen(k.vk, k.down, k.injected());
            if !self.win.any_down() {
                self.combo_masked = false;
            }
        }
        if taken {
            // the press is swallowed (in log-only mode nothing is: the layer only says what it would do)
            return !log_only;
        }

        // ---- Alt+Tab
        let swallow = self.chord_step(k, stand_down, log_only);
        let swallow = swallow && !log_only;
        self.note(k, swallow);
        self.shared.set_chord_held(self.chord.is_open_held());
        swallow
    }

    fn chord_step(&mut self, k: &RawKey, stand_down: bool, log_only: bool) -> bool {
        let input = translate(k.vk, k.scan, k.flags, k.down, k.mods);
        if !k.down {
            // A release always goes through the chord, whatever the layer's state: it is swallowed exactly when its press was.
            let r = self.chord.feed(input);
            if input.key == Key::Alt && !input.injected {
                self.given_up = false;
            }
            if r.mask_alt {
                self.send_mask(log_only);
            }
            if let Some(e) = r.effect {
                self.post_effect(e);
            }
            return r.swallow;
        }
        if self.shared.alt_on() && !stand_down && !self.given_up {
            let r = self.chord.feed(input);
            if let Some(e) = r.effect {
                if matches!(e, Effect::Open { .. }) {
                    self.seq = self.shared.next_seq();
                }
                self.post_effect(e);
            }
            return r.swallow;
        }
        // A press while the chord is not ours to run: it passes. If a chord was open, it ends here.
        if self.chord.is_open() {
            self.chord.closed_elsewhere();
            self.shared.post(Posted::Lost);
        }
        false
    }

    fn post_effect(&self, effect: Effect) {
        self.shared.post(Posted::Chord { effect, seq: self.seq });
    }

    fn send_mask(&self, log_only: bool) {
        if !log_only && !self.mask.send() {
            self.shared.note_mask_failed();
        }
    }

    /// Keeps what the layer believes about the tracked keys (the Windows keys and Alt, which are never swallowed). The keyboard's own
    /// state, which the heartbeat compares this with, follows every one of their events, injected ones too.
    fn note(&mut self, k: &RawKey, swallowed: bool) {
        if swallowed {
            return;
        }
        let slot = match k.vk {
            VK_LWIN => &mut self.held.lwin,
            VK_RWIN => &mut self.held.rwin,
            VK_LMENU | VK_MENU => &mut self.held.lalt,
            VK_RMENU => &mut self.held.ralt,
            _ => return,
        };
        if *slot != k.down {
            *slot = k.down;
            self.shared.set_believed(self.held.tracked());
        }
    }
}

/// Which of the two taken keys a virtual key is: `[E, D]`.
fn combo_slot(vk: u32) -> Option<usize> {
    match vk {
        VK_E => Some(0),
        VK_D => Some(1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::switcher::chord::Scope;
    use crate::switcher::hook::{FakeFeed, KeyFeed, LLKHF_INJECTED, VK_LCONTROL, VK_LSHIFT, VK_TAB};
    use crate::winkey::fakes::FakeMask;
    use crate::winkey::shared::{Clock, STALE_AFTER_MS};

    const A: u32 = 0x41;
    const LEFT: u32 = 0x25;
    const ESC: u32 = 0x1B;
    const GRAVE: u32 = 0xC0;
    const SCAN_TAB: u32 = 0x0F;
    const SCAN_ALT: u32 = 0x38;
    const SCAN_GRAVE: u32 = 0x29;
    const ALT_DOWN: u32 = LLKHF_ALTDOWN;

    /// A layer behind a fake feed, with a clock the test moves and a mask that counts.
    struct Rig {
        feed: FakeFeed,
        shared: Arc<Shared>,
        clock: Arc<Clock>,
        mask: Arc<FakeMask>,
    }

    fn rig_with(win: bool, alt: bool, log_only: bool) -> Rig {
        rig_probing(win, alt, log_only, None)
    }

    /// [`rig_with`] whose layer also asks a fake keyboard whether a Windows key is really down (what the service gives the real layer).
    fn rig_probing(win: bool, alt: bool, log_only: bool, probe: Option<Arc<crate::winkey::fakes::FakeProbe>>) -> Rig {
        let clock = Arc::new(Clock::manual());
        clock.set(10_000);
        let shared = Arc::new(Shared::new(clock.clone(), log_only));
        shared.set_wanted(win, alt);
        shared.beat(false);
        let mask = Arc::new(FakeMask::default());
        let mut layer = KeyLayer::new(shared.clone(), mask.clone(), false, false);
        if let Some(p) = probe {
            layer = layer.with_probe(p);
        }
        let mut feed = FakeFeed::default();
        feed.start_raw(Box::new(move |k| layer.on_event(k))).unwrap();
        Rig { feed, shared, clock, mask }
    }

    fn rig() -> Rig {
        rig_with(true, true, false)
    }

    impl Rig {
        fn key(&mut self, vk: u32, scan: u32, down: bool, alt: bool) -> bool {
            self.feed.push_raw(vk, scan, if alt { ALT_DOWN } else { 0 }, down)
        }
        fn win_down(&mut self, vk: u32) -> bool {
            self.feed.push_raw(vk, 0x5B, 0, true)
        }
        fn win_up(&mut self, vk: u32) -> bool {
            self.feed.push_raw(vk, 0x5B, 0, false)
        }
        fn alt_down(&mut self) -> bool {
            self.feed.push_raw(VK_LMENU, SCAN_ALT, ALT_DOWN, true)
        }
        fn alt_up(&mut self) -> bool {
            self.feed.push_raw(VK_LMENU, SCAN_ALT, 0, false)
        }
        fn tab_down(&mut self) -> bool {
            self.key(VK_TAB, SCAN_TAB, true, true)
        }
        fn tab_up(&mut self) -> bool {
            self.key(VK_TAB, SCAN_TAB, false, true)
        }
        fn posted(&self) -> Vec<Posted> {
            std::iter::from_fn(|| self.shared.pop()).collect()
        }
    }

    // ---------------------------------------------------------------- the Windows key

    #[test]
    fn a_clean_tap_of_the_windows_key_sends_the_mask_once_posts_one_start_toggle_and_lets_both_events_pass() {
        let mut r = rig();
        assert!(!r.win_down(VK_LWIN), "the press is never swallowed");
        assert_eq!(r.mask.sent(), 0, "nothing is sent on the press");
        assert!(!r.win_up(VK_LWIN), "the release passes");
        assert_eq!(r.mask.sent(), 1, "the mask went in during the release's callback, before the release was let through");
        assert_eq!(r.posted(), vec![Posted::StartTap]);
    }

    #[test]
    fn the_right_windows_key_taps_too() {
        let mut r = rig();
        r.win_down(VK_RWIN);
        r.win_up(VK_RWIN);
        assert_eq!(r.posted(), vec![Posted::StartTap]);
        assert_eq!(r.mask.sent(), 1);
    }

    #[test]
    fn win_plus_anything_is_never_touched_nothing_swallowed_nothing_sent_nothing_posted() {
        let mut r = rig();
        for letter in [A, 0x45 /* E */, 0x44 /* D */, 0x4C /* L */, 0x09 /* Tab */, 0x31 /* 1 */] {
            assert!(!r.win_down(VK_LWIN));
            assert!(!r.feed.push_raw(letter, 0x1E, 0, true), "the chord's key passes");
            assert!(!r.feed.push_raw(letter, 0x1E, 0, false));
            assert!(!r.win_up(VK_LWIN));
        }
        assert_eq!(r.mask.sent(), 0);
        assert_eq!(r.posted(), vec![]);
    }

    #[test]
    fn win_with_shift_or_ctrl_held_is_not_a_tap() {
        let mut r = rig();
        r.feed.push_raw(VK_LSHIFT, 0x2A, 0, true);
        r.win_down(VK_LWIN);
        r.win_up(VK_LWIN);
        r.feed.push_raw(VK_LSHIFT, 0x2A, 0, false);
        r.feed.push_raw(VK_LCONTROL, 0x1D, 0, true);
        r.win_down(VK_LWIN);
        r.win_up(VK_LWIN);
        assert_eq!(r.mask.sent(), 0);
        assert_eq!(r.posted(), vec![]);
    }

    #[test]
    fn a_held_key_with_auto_repeat_released_is_one_tap_and_a_double_tap_is_two() {
        let mut r = rig();
        r.win_down(VK_LWIN);
        r.win_down(VK_LWIN);
        r.win_down(VK_LWIN);
        r.win_up(VK_LWIN);
        r.win_down(VK_LWIN);
        r.win_up(VK_LWIN);
        assert_eq!(r.posted(), vec![Posted::StartTap, Posted::StartTap]);
        assert_eq!(r.mask.sent(), 2);
    }

    #[test]
    fn a_windows_key_that_was_already_down_when_the_hook_came_up_does_not_tap_on_its_release() {
        let clock = Arc::new(Clock::manual());
        clock.set(1);
        let shared = Arc::new(Shared::new(clock, false));
        shared.set_wanted(true, true);
        shared.beat(false);
        let mask = Arc::new(FakeMask::default());
        let mut layer = KeyLayer::new(shared.clone(), mask.clone(), true, false);
        let mut feed = FakeFeed::default();
        feed.start_raw(Box::new(move |k| layer.on_event(k))).unwrap();
        assert!(!feed.push_raw(VK_LWIN, 0x5B, 0, false));
        assert_eq!(mask.sent(), 0);
        assert!(shared.pop().is_none());
    }

    #[test]
    fn injected_windows_key_events_are_ignored_the_docks_own_start_button_goes_to_windows() {
        let mut r = rig();
        assert!(!r.feed.push_raw(VK_LWIN, 0x5B, LLKHF_INJECTED, true));
        assert!(!r.feed.push_raw(VK_LWIN, 0x5B, LLKHF_INJECTED, false));
        assert_eq!(r.mask.sent(), 0);
        assert_eq!(r.posted(), vec![]);
    }

    #[test]
    fn with_the_windows_key_setting_off_a_tap_passes_untouched() {
        let mut r = rig_with(false, true, false);
        r.win_down(VK_LWIN);
        assert!(!r.win_up(VK_LWIN));
        assert_eq!(r.mask.sent(), 0);
        assert_eq!(r.posted(), vec![]);
    }

    #[test]
    fn the_windows_key_setting_switched_on_in_the_middle_of_a_hold_does_not_tap_on_that_release() {
        let mut r = rig_with(false, false, false);
        r.win_down(VK_LWIN);
        r.shared.set_wanted(true, false);
        r.win_up(VK_LWIN);
        assert_eq!(r.posted(), vec![], "its press was not seen with the setting on");
    }

    // ---------------------------------------------------------------- Alt+Tab

    #[test]
    fn alt_tab_swallows_the_tab_and_its_release_posts_the_opening_and_masks_the_alt_release() {
        let mut r = rig();
        assert!(!r.alt_down(), "Alt itself is never swallowed");
        assert!(r.tab_down(), "the Tab is swallowed");
        assert!(r.tab_up(), "and its release is");
        assert!(!r.alt_up(), "the Alt release passes");
        assert_eq!(r.mask.sent(), 1, "a lone Alt release would light up the menu bar of the program in front");
        assert_eq!(
            r.posted(),
            vec![
                Posted::Chord { effect: Effect::Open { scope: Scope::AllWindows, backwards: false }, seq: 1 },
                Posted::Chord { effect: Effect::Choose, seq: 1 },
            ]
        );
    }

    #[test]
    fn tab_shift_tab_the_key_above_tab_the_arrows_enter_and_escape_drive_the_switcher() {
        let mut r = rig();
        r.alt_down();
        assert!(r.tab_down());
        assert!(r.tab_up());
        assert!(r.tab_down(), "Tab again steps");
        r.tab_up();
        r.feed.push_raw(VK_LSHIFT, 0x2A, ALT_DOWN, true);
        assert!(r.key(VK_TAB, SCAN_TAB, true, true), "Shift+Tab steps back");
        r.key(VK_TAB, SCAN_TAB, false, true);
        r.feed.push_raw(VK_LSHIFT, 0x2A, ALT_DOWN, false);
        assert!(r.key(GRAVE, SCAN_GRAVE, true, true), "the key above Tab steps within the app");
        r.key(GRAVE, SCAN_GRAVE, false, true);
        assert!(r.key(LEFT, 0x4B, true, true), "an arrow moves");
        r.key(LEFT, 0x4B, false, true);
        assert!(r.key(ESC, 0x01, true, true), "Escape cancels");
        r.key(ESC, 0x01, false, true);
        let effects: Vec<Effect> = r
            .posted()
            .into_iter()
            .filter_map(|p| match p {
                Posted::Chord { effect, .. } => Some(effect),
                _ => None,
            })
            .collect();
        assert_eq!(
            effects,
            vec![
                Effect::Open { scope: Scope::AllWindows, backwards: false },
                Effect::Step { backwards: false },
                Effect::Step { backwards: true },
                Effect::Step { backwards: false },
                Effect::Move(crate::switcher::chord::Dir::Left),
                Effect::Cancel,
            ]
        );
    }

    #[test]
    fn alt_and_the_windows_keys_are_never_swallowed_in_any_state_of_the_chord() {
        let mut r = rig();
        for _ in 0..2 {
            assert!(!r.alt_down());
            assert!(r.tab_down());
            // while the chord is open
            assert!(!r.alt_down(), "Alt auto-repeat");
            assert!(!r.win_down(VK_LWIN));
            assert!(!r.win_up(VK_LWIN));
            assert!(!r.feed.push_raw(VK_RMENU, SCAN_ALT, ALT_DOWN, true));
            assert!(!r.feed.push_raw(VK_RMENU, SCAN_ALT, 0, false));
            r.tab_up();
            assert!(!r.alt_up());
        }
    }

    #[test]
    fn a_key_that_passed_down_has_its_release_passed_not_swallowed() {
        let mut r = rig();
        // Tab typed without Alt passes, and so does its release, wherever the chord is by then
        assert!(!r.key(VK_TAB, SCAN_TAB, true, false));
        r.alt_down();
        assert!(!r.key(VK_TAB, SCAN_TAB, false, true), "the Tab went down before Alt: its release is not ours");
        // an arrow pressed with no switcher open passes, and so does its release
        assert!(!r.key(LEFT, 0x4B, true, false));
        assert!(!r.key(LEFT, 0x4B, false, false));
        r.alt_up();
    }

    #[test]
    fn a_key_that_was_swallowed_down_has_its_release_swallowed_even_after_the_layer_stood_down() {
        let mut r = rig();
        r.alt_down();
        assert!(r.tab_down());
        // the interface stops answering
        r.clock.set(10_000 + STALE_AFTER_MS + 500);
        assert!(r.tab_up(), "the press never reached the program, so its release must not either");
        assert!(!r.alt_up());
    }

    #[test]
    fn a_press_while_the_layer_stands_down_passes_and_its_release_passes_after_the_layer_comes_back() {
        let mut r = rig();
        r.alt_down();
        r.tab_down();
        r.tab_up();
        r.clock.set(10_000 + STALE_AFTER_MS + 500); // stale
        assert!(!r.key(LEFT, 0x4B, true, true), "the chord was open: the arrow passes now");
        r.shared.beat(false); // the interface is back
        assert!(!r.key(LEFT, 0x4B, false, true), "the press passed, so the release passes");
        r.alt_up();
    }

    #[test]
    fn stale_heartbeat_fails_open_every_key_passes_and_nothing_is_swallowed_sent_or_posted() {
        let mut r = rig();
        r.clock.set(10_000 + STALE_AFTER_MS + 1);
        r.alt_down();
        assert!(!r.tab_down(), "Alt+Tab goes to Windows");
        assert!(!r.key(GRAVE, SCAN_GRAVE, true, true));
        r.tab_up();
        r.alt_up();
        r.win_down(VK_LWIN);
        assert!(!r.win_up(VK_LWIN), "Windows' own Start opens");
        assert_eq!(r.mask.sent(), 0);
        assert_eq!(r.posted(), vec![]);
    }

    #[test]
    fn a_heartbeat_that_was_never_stamped_passes_every_key() {
        let clock = Arc::new(Clock::manual());
        let shared = Arc::new(Shared::new(clock, false));
        shared.set_wanted(true, true);
        let mask = Arc::new(FakeMask::default());
        let mut layer = KeyLayer::new(shared.clone(), mask.clone(), false, false);
        let mut feed = FakeFeed::default();
        feed.start_raw(Box::new(move |k| layer.on_event(k))).unwrap();
        feed.push_raw(VK_LMENU, SCAN_ALT, ALT_DOWN, true);
        assert!(!feed.push_raw(VK_TAB, SCAN_TAB, ALT_DOWN, true));
        feed.push_raw(VK_LWIN, 0x5B, 0, true);
        feed.push_raw(VK_LWIN, 0x5B, 0, false);
        assert_eq!(mask.sent(), 0);
        assert!(shared.pop().is_none());
    }

    #[test]
    fn the_layer_comes_back_when_the_heartbeat_does() {
        let mut r = rig();
        r.clock.set(10_000 + STALE_AFTER_MS + 1);
        assert!(!r.tab_down());
        r.tab_up();
        r.shared.beat(false);
        assert!(r.tab_down(), "Alt+Tab is ours again");
    }

    #[test]
    fn a_full_screen_program_in_front_stands_both_layers_down() {
        let mut r = rig();
        r.shared.beat(true);
        r.alt_down();
        assert!(!r.tab_down());
        r.tab_up();
        r.alt_up();
        r.win_down(VK_LWIN);
        r.win_up(VK_LWIN);
        assert_eq!(r.mask.sent(), 0);
        assert_eq!(r.posted(), vec![]);
        r.shared.beat(false);
        r.win_down(VK_LWIN);
        r.win_up(VK_LWIN);
        assert_eq!(r.posted(), vec![Posted::StartTap], "and it is back when the program leaves");
    }

    #[test]
    fn log_only_decides_and_posts_but_swallows_nothing_and_sends_nothing() {
        let mut r = rig_with(true, true, true);
        r.win_down(VK_LWIN);
        assert!(!r.win_up(VK_LWIN));
        r.alt_down();
        assert!(!r.tab_down(), "log-only: the Tab reaches Windows");
        assert!(!r.tab_up());
        assert!(!r.alt_up());
        assert_eq!(r.mask.sent(), 0, "no mask key in log-only mode");
        let p = r.posted();
        assert_eq!(p[0], Posted::StartTap, "the decision is posted, so that it can be logged");
        assert!(matches!(p[1], Posted::Chord { effect: Effect::Open { .. }, .. }));
    }

    #[test]
    fn when_the_switcher_could_not_be_opened_the_chord_is_given_up_at_once() {
        let mut r = rig();
        r.alt_down();
        assert!(r.tab_down());
        let seq = match r.posted()[0] {
            Posted::Chord { seq, .. } => seq,
            other => panic!("{other:?}"),
        };
        // the interface says: could not open
        r.shared.give_up(seq);
        assert!(r.tab_up(), "the press was swallowed: its release is too");
        assert!(!r.tab_down(), "and from now on the Tab goes to Windows");
        assert!(!r.key(GRAVE, SCAN_GRAVE, true, true));
        r.tab_up();
        assert!(!r.alt_up());
        assert_eq!(r.mask.sent(), 1, "the Tab that was swallowed still needs the Alt release masked");
        assert_eq!(r.posted(), vec![], "nothing more is posted");
        // after Alt is let go the next Alt+Tab is ours again
        r.alt_down();
        assert!(r.tab_down());
    }

    #[test]
    fn a_switcher_that_closes_by_itself_ends_the_chord_and_the_next_tab_opens_a_new_one() {
        let mut r = rig();
        r.alt_down();
        r.tab_down();
        r.tab_up();
        let seq = match r.posted()[0] {
            Posted::Chord { seq, .. } => seq,
            other => panic!("{other:?}"),
        };
        r.shared.chord_closed(seq); // a click chose a card
        assert!(r.tab_down(), "still Alt+Tab, and it is a new opening");
        let p = r.posted();
        assert!(matches!(p[0], Posted::Chord { effect: Effect::Open { .. }, seq: 2 }), "{p:?}");
    }

    #[test]
    fn a_late_close_of_an_older_opening_does_not_end_the_newer_one() {
        let mut r = rig();
        r.alt_down();
        r.tab_down();
        r.tab_up();
        r.key(ESC, 0x01, true, true); // cancelled: the chord is closed
        r.key(ESC, 0x01, false, true);
        r.tab_down(); // a second opening, number 2
        r.tab_up();
        r.shared.chord_closed(1); // the first switcher's "closed" arrives late
        assert!(r.key(LEFT, 0x4B, true, true), "the second opening is still ours: the arrow moves in it");
    }

    #[test]
    fn switching_the_setting_off_in_the_middle_of_a_chord_closes_it_tells_the_interface_and_passes_new_keys() {
        let mut r = rig();
        r.alt_down();
        r.tab_down();
        r.tab_up();
        let _ = r.posted();
        r.shared.set_wanted(true, false);
        assert!(!r.key(LEFT, 0x4B, true, true));
        assert_eq!(r.posted(), vec![Posted::Lost]);
        assert!(!r.alt_up());
        assert_eq!(r.mask.sent(), 1, "the Tab that was swallowed still needs the mask");
    }

    #[test]
    fn altgr_and_ctrl_alt_tab_are_not_the_chord() {
        let mut r = rig();
        // AltGr: a right Alt with a Ctrl Windows makes up
        r.feed.push_raw(VK_LCONTROL, 0x1D, 0, true);
        r.feed.push_raw(VK_RMENU, SCAN_ALT, ALT_DOWN, true);
        assert!(!r.key(GRAVE, SCAN_GRAVE, true, true), "AltGr + the key above Tab types its character");
        assert!(!r.key(VK_TAB, SCAN_TAB, true, true), "Ctrl+Alt+Tab is Windows' own");
        assert_eq!(r.posted(), vec![]);
    }

    #[test]
    fn injected_alt_tab_is_ignored() {
        let mut r = rig();
        assert!(!r.feed.push_raw(VK_TAB, SCAN_TAB, ALT_DOWN | LLKHF_INJECTED, true));
        assert_eq!(r.posted(), vec![]);
        assert_eq!(r.mask.sent(), 0);
    }

    #[test]
    fn a_plain_alt_press_and_release_is_left_alone() {
        let mut r = rig();
        r.alt_down();
        assert!(!r.alt_up());
        assert_eq!(r.mask.sent(), 0, "no Tab was swallowed: the program's own menu-bar behaviour stays");
    }

    #[test]
    fn a_swallowed_keys_release_is_swallowed_also_when_ctrl_went_down_in_between_and_the_next_plain_press_and_release_pass() {
        // Ctrl pressed in the middle of the chord turns a PRESS into another key for the chord (Ctrl+Alt+Tab is Windows' own), but a
        // release must be found by the key it is: otherwise the release of a swallowed press passes (the system gets a release with
        // no press) and the stale mark swallows the release of a later, plain press (the system believes the key is held).
        for (vk, scan) in [(VK_TAB, SCAN_TAB), (LEFT, 0x4B), (ESC, 0x01)] {
            let mut r = rig();
            r.alt_down();
            assert!(r.tab_down(), "Alt+Tab opens the chord");
            if vk != VK_TAB {
                r.tab_up();
                assert!(r.key(vk, scan, true, true), "the open chord swallows its key");
            }
            r.feed.push_raw(VK_LCONTROL, 0x1D, ALT_DOWN, true);
            assert!(r.key(vk, scan, false, true), "its press never reached the program: neither may its release, Ctrl or not");
            r.feed.push_raw(VK_LCONTROL, 0x1D, ALT_DOWN, false);
            r.alt_up();
            assert!(!r.key(vk, scan, true, false), "a later plain press passes");
            assert!(!r.key(vk, scan, false, false), "and its release passes: the system must not believe the key is still held");
        }
    }

    #[test]
    fn a_windows_key_held_across_a_stand_down_is_not_a_tap_when_it_is_released_after_the_layer_is_back() {
        // Win+arrow (snapping) with the key's auto-repeat: the layer stood down in the middle (stale beat), so the arrow was never
        // seen; the first auto-repeat after it came back must not look like a fresh clean press.
        let mut r = rig();
        r.win_down(VK_LWIN);
        r.clock.set(10_000 + STALE_AFTER_MS + 500);
        r.win_down(VK_LWIN); // auto-repeat while standing down
        r.key(LEFT, 0x4B, true, false);
        r.key(LEFT, 0x4B, false, false);
        r.shared.beat(false); // the interface is back
        r.win_down(VK_LWIN); // auto-repeat
        assert!(!r.win_up(VK_LWIN));
        assert_eq!(r.mask.sent(), 0, "no mask: Windows' own Start is not ours to touch for a chord");
        assert_eq!(r.posted(), vec![]);
        // and the layer is usable at once
        r.win_down(VK_LWIN);
        r.win_up(VK_LWIN);
        assert_eq!(r.posted(), vec![Posted::StartTap]);
    }

    #[test]
    fn a_windows_key_pressed_while_the_layer_stands_down_and_released_after_it_is_back_is_not_a_tap() {
        let mut r = rig();
        r.shared.beat(true); // a full-screen program: standing down
        r.win_down(VK_LWIN);
        r.shared.beat(false);
        assert!(!r.win_up(VK_LWIN));
        assert_eq!(r.posted(), vec![], "its press was not ours to see");
        assert_eq!(r.mask.sent(), 0);
    }

    // ---------------------------------------------------------------- what the layer believes (for the watchdog)

    #[test]
    fn the_belief_follows_the_keys_that_reached_the_system_and_not_the_swallowed_ones() {
        let mut r = rig();
        r.alt_down();
        assert_eq!(r.shared.believed(), Tracked { win: false, alt: true });
        r.tab_down();
        r.tab_up();
        r.alt_up();
        assert_eq!(r.shared.believed(), Tracked::default());
        r.win_down(VK_RWIN);
        assert!(r.shared.believed().win);
        r.win_up(VK_RWIN);
        assert!(!r.shared.believed().win);
    }

    #[test]
    fn injected_keys_count_for_the_belief_because_the_keyboards_state_follows_them() {
        let mut r = rig();
        r.feed.push_raw(VK_LMENU, SCAN_ALT, LLKHF_INJECTED | ALT_DOWN, true);
        assert!(r.shared.believed().alt);
        r.feed.push_raw(VK_LMENU, SCAN_ALT, LLKHF_INJECTED, false);
        assert!(!r.shared.believed().alt);
    }

    #[test]
    fn a_mask_the_system_refuses_is_counted_and_the_layer_goes_on() {
        let mut r = rig();
        r.mask.refuse.store(true, std::sync::atomic::Ordering::Relaxed);
        r.win_down(VK_LWIN);
        assert!(!r.win_up(VK_LWIN), "the release still passes");
        assert_eq!(r.shared.mask_failures(), 1);
        assert_eq!(r.posted(), vec![Posted::StartTap]);
    }

    #[test]
    fn a_layer_is_plain_data_and_ids_and_codes_are_the_only_state_it_keeps() {
        fn is_send<T: Send>() {}
        is_send::<KeyLayer>();
    }

    // ---------------------------------------------------------------- Win+E and Win+D (rule 8)

    const KEY_E: u32 = 0x45;
    const KEY_D: u32 = 0x44;
    const SCAN_E: u32 = 0x12;
    const SCAN_D: u32 = 0x20;

    fn combo_rig(log_only: bool) -> Rig {
        let r = rig_with(true, false, log_only);
        r.shared.set_combos(true);
        r
    }

    #[test]
    fn win_and_e_takes_the_e_posts_the_chord_masks_once_and_never_opens_start() {
        let mut r = combo_rig(false);
        assert!(!r.win_down(VK_LWIN));
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true), "the press of E is swallowed");
        assert_eq!(r.mask.sent(), 1, "the mask goes in while the Windows key is down, so Windows' Start does not open on its release");
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true), "the auto-repeat is swallowed too");
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, false), "the release follows the press");
        assert!(!r.win_up(VK_LWIN), "the Windows key itself always passes");
        assert_eq!(r.posted(), vec![Posted::WinCombo(crate::winkey::machine::Combo::Explorer)], "one chord, and no StartTap");
        assert_eq!(r.mask.sent(), 1);
        // the next ordinary tap is a tap again
        r.win_down(VK_LWIN);
        r.win_up(VK_LWIN);
        assert_eq!(r.posted(), vec![Posted::StartTap]);
        assert_eq!(r.mask.sent(), 2);
    }

    #[test]
    fn a_second_chord_in_the_same_hold_is_taken_with_no_second_mask() {
        let mut r = combo_rig(false);
        r.win_down(VK_RWIN);
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        r.feed.push_raw(KEY_E, SCAN_E, 0, false);
        assert!(r.feed.push_raw(KEY_D, SCAN_D, 0, true));
        r.feed.push_raw(KEY_D, SCAN_D, 0, false);
        r.win_up(VK_RWIN);
        use crate::winkey::machine::Combo;
        assert_eq!(r.posted(), vec![Posted::WinCombo(Combo::Explorer), Posted::WinCombo(Combo::Desktop)]);
        assert_eq!(r.mask.sent(), 1, "one mask for the hold");
    }

    #[test]
    fn the_release_of_a_taken_key_is_swallowed_even_after_the_layer_stood_down() {
        let mut r = combo_rig(false);
        r.win_down(VK_LWIN);
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        r.clock.set(10_000 + STALE_AFTER_MS + 10);
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, false), "a key that was swallowed down never comes up in the program");
        // the next E is not tracked any more: while standing down it passes
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, false));
        r.win_up(VK_LWIN);
    }

    #[test]
    fn with_the_chords_not_ours_every_key_passes_and_nothing_is_masked() {
        // the setting for the chords is off (an Explorer shell answers them itself)
        let mut r = rig_with(true, false, false);
        r.win_down(VK_LWIN);
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, false));
        r.win_up(VK_LWIN);
        assert_eq!(r.mask.sent(), 0);
        assert_eq!(r.posted(), vec![]);
        // and with the Windows-key setting off altogether, the chords are not looked at either
        let mut r = rig_with(false, true, false);
        r.shared.set_combos(true);
        r.win_down(VK_LWIN);
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        r.win_up(VK_LWIN);
        assert_eq!(r.posted(), vec![]);
    }

    #[test]
    fn other_chords_a_shifted_chord_and_a_programs_own_key_are_not_ours() {
        let mut r = combo_rig(false);
        // Win+Shift+E
        r.win_down(VK_LWIN);
        r.feed.push_raw(VK_LSHIFT, 0x2A, 0, true);
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        r.feed.push_raw(KEY_E, SCAN_E, 0, false);
        r.feed.push_raw(VK_LSHIFT, 0x2A, 0, false);
        r.win_up(VK_LWIN);
        // Win+L, Win+R, Win+Tab
        for key in [0x4C, 0x52, 0x09] {
            r.win_down(VK_LWIN);
            assert!(!r.feed.push_raw(key, 0x1E, 0, true));
            r.feed.push_raw(key, 0x1E, 0, false);
            r.win_up(VK_LWIN);
        }
        // an E a program sent while the Windows key is held
        r.win_down(VK_LWIN);
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, LLKHF_INJECTED, true));
        r.feed.push_raw(KEY_E, SCAN_E, LLKHF_INJECTED, false);
        r.win_up(VK_LWIN);
        // (a key a program sent is not the user's key: it does not spoil the tap, so that hold ends as a tap, as the machine says)
        assert_eq!(r.posted(), vec![Posted::StartTap]);
        assert_eq!(r.mask.sent(), 1, "only that tap's mask");
    }

    #[test]
    fn in_log_only_mode_the_chord_is_posted_and_nothing_is_swallowed_or_sent() {
        let mut r = combo_rig(true);
        r.win_down(VK_LWIN);
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, false));
        r.win_up(VK_LWIN);
        assert_eq!(r.mask.sent(), 0);
        assert_eq!(r.posted(), vec![Posted::WinCombo(crate::winkey::machine::Combo::Explorer)]);
    }

    #[test]
    fn a_mask_the_system_refused_is_counted_and_the_chord_still_works() {
        let mut r = combo_rig(false);
        r.mask.refuse.store(true, std::sync::atomic::Ordering::Relaxed);
        r.win_down(VK_LWIN);
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        r.feed.push_raw(KEY_E, SCAN_E, 0, false);
        r.win_up(VK_LWIN);
        assert_eq!(r.shared.mask_failures(), 1);
        assert_eq!(r.posted().len(), 1);
    }

    #[test]
    fn the_same_chord_key_pressed_again_while_the_windows_key_is_still_held_is_taken_again() {
        // Hold Win, press E (a window opens), let go of E, press E again: the second press is a second chord, not an auto-repeat
        let mut r = combo_rig(false);
        r.win_down(VK_LWIN);
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true), "its auto-repeat is swallowed");
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, false));
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true), "the second press is taken, not passed to the focused program");
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, false));
        r.win_up(VK_LWIN);
        use crate::winkey::machine::Combo;
        assert_eq!(r.posted(), vec![Posted::WinCombo(Combo::Explorer), Posted::WinCombo(Combo::Explorer)]);
        assert_eq!(r.mask.sent(), 1, "still one mask for the hold");
    }

    #[test]
    fn a_key_up_a_program_sent_does_not_end_the_tracking_of_a_taken_key() {
        // A remote-input tool or an IME injects an E key-up while the user still holds E: the real repeats and the real release must
        // keep being swallowed (the press was)
        let mut r = combo_rig(false);
        r.win_down(VK_LWIN);
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        r.feed.push_raw(KEY_E, SCAN_E, LLKHF_INJECTED, false);
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true), "the real auto-repeat after the synthetic key-up is still swallowed");
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, false), "and the real release");
        // after the real release the key is free again
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, false), "a stray release now is nobody's to swallow");
        r.win_up(VK_LWIN);
    }

    #[test]
    fn a_windows_key_the_keyboard_says_is_up_does_not_make_a_typed_e_a_chord() {
        // The layer believes Win is down (its release was never seen: secure desktop, a stalled hook), the keyboard says it is up.
        let probe = Arc::new(crate::winkey::fakes::FakeProbe::default());
        let mut r = rig_probing(true, false, false, Some(probe.clone()));
        r.shared.set_combos(true);
        probe.set(VK_LWIN, true);
        r.win_down(VK_LWIN);
        probe.set(VK_LWIN, false); // released where the hook could not see it
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, true), "a typed e passes");
        assert!(!r.feed.push_raw(KEY_E, SCAN_E, 0, false));
        assert_eq!(r.mask.sent(), 0, "no mask is sent for a chord that never was");
        assert_eq!(r.posted(), vec![], "nothing is opened");
        // the belief was repaired: the hold is over for the layer, and the next tap is a tap
        r.win_down(VK_LWIN);
        probe.set(VK_LWIN, true);
        r.win_up(VK_LWIN);
        assert_eq!(r.posted(), vec![Posted::StartTap]);
    }

    #[test]
    fn a_windows_key_the_keyboard_says_is_down_still_makes_the_chord() {
        let probe = Arc::new(crate::winkey::fakes::FakeProbe::default());
        let mut r = rig_probing(true, false, false, Some(probe.clone()));
        r.shared.set_combos(true);
        probe.set(VK_LWIN, true);
        r.win_down(VK_LWIN);
        assert!(r.feed.push_raw(KEY_E, SCAN_E, 0, true));
        r.feed.push_raw(KEY_E, SCAN_E, 0, false);
        r.win_up(VK_LWIN);
        assert_eq!(r.posted(), vec![Posted::WinCombo(crate::winkey::machine::Combo::Explorer)]);
    }
}
