//! The switcher's chord as a state machine over key events. Pure: it is fed [`KeyInput`]s and answers what to do with each
//! ([`Reaction`]); nothing here reads the keyboard. The hook that will feed it in the shell host is `hook.rs`, behind a trait, and
//! is not installed by anything.
//!
//! THE CHORD. Alt held:
//! * Tab opens the switcher (all windows) and steps forward; Shift+Tab opens it and steps back; Tab again steps on;
//! * the "same app" key (the key above Tab, [`Key::Grave`]) opens it with only the windows of the app in front, and steps the same way;
//! * the arrow keys move in the grid of cards, Enter chooses, Escape cancels;
//! * letting go of Alt chooses.
//!
//! TWO MODES. `Held` is the real chord: the user holds Alt, so letting go chooses. `Sticky` is the switcher opened by a command
//! (`--switcher-preview`) or by a function call, with no Alt held: Tab, the arrows, Enter and Escape work without it and releasing
//! Alt means nothing.
//!
//! WHAT THE HOOK MUST DO WITH AN ANSWER. `swallow` says the key must not reach the program in front (the hook returns nonzero). A
//! modifier's release is never swallowed: the system would think the key is still down. `mask_alt` says Alt is being released after
//! keys were swallowed: a lone Alt release lights up the menu bar of the program in front, so the hook must send one harmless key
//! (a key no program has, such as `VK_E8`) between the last swallowed key and the release.

/// The keys the chord cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Tab,
    /// The key above Tab (scan code 0x29, the back quote on a US keyboard; found by its place, not by what the layout types on it):
    /// "windows of this app".
    Grave,
    Left,
    Right,
    Up,
    Down,
    Enter,
    Escape,
    /// Either Alt key.
    Alt,
    /// Any other key (not swallowed, not used).
    Other,
}

/// One key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInput {
    pub key: Key,
    pub down: bool,
    /// Shift is down.
    pub shift: bool,
    /// Alt is down (the hook's `LLKHF_ALTDOWN`). Meaningless for the Alt key's own events.
    pub alt: bool,
    /// The event was sent by a program (`LLKHF_INJECTED`), not typed: the chord ignores those (it would answer its own taps).
    pub injected: bool,
}

impl KeyInput {
    pub const fn down(key: Key) -> KeyInput {
        KeyInput { key, down: true, shift: false, alt: false, injected: false }
    }
    pub const fn up(key: Key) -> KeyInput {
        KeyInput { key, down: false, shift: false, alt: false, injected: false }
    }
    pub const fn with_alt(mut self) -> KeyInput {
        self.alt = true;
        self
    }
    pub const fn with_shift(mut self) -> KeyInput {
        self.shift = true;
        self
    }
}

/// Which windows the switcher shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    AllWindows,
    /// Only the windows of the app that is in front.
    ThisApp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// What the chord asks the switcher to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Show the switcher (the first step is part of it: forward selects the window used before, backwards the oldest).
    Open { scope: Scope, backwards: bool },
    Step { backwards: bool },
    Move(Dir),
    Choose,
    Cancel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reaction {
    pub swallow: bool,
    pub effect: Option<Effect>,
    pub mask_alt: bool,
}

impl Reaction {
    const NOTHING: Reaction = Reaction { swallow: false, effect: None, mask_alt: false };
    fn swallowed(effect: Effect) -> Reaction {
        Reaction { swallow: true, effect: Some(effect), mask_alt: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Closed,
    Open { held: bool },
}

#[derive(Clone, Debug)]
pub struct Chord {
    state: State,
    /// Keys whose press was swallowed: their release is swallowed too (a release with no press confuses programs).
    swallowed: Vec<Key>,
    /// A key was swallowed while Alt was down: the next Alt release needs the mask key.
    tainted: bool,
}

impl Default for Chord {
    fn default() -> Self {
        Chord::new()
    }
}

impl Chord {
    pub fn new() -> Chord {
        Chord { state: State::Closed, swallowed: Vec::new(), tainted: false }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, State::Open { .. })
    }

    /// The switcher was opened by a command: open, with no Alt held (the `Sticky` mode).
    pub fn open_by_command(&mut self) {
        self.state = State::Open { held: false };
    }

    /// The switcher was closed by something else (a click on a card, losing the foreground): the chord forgets it was open.
    pub fn closed_elsewhere(&mut self) {
        self.state = State::Closed;
    }

    fn swallow_down(&mut self, key: Key, alt: bool) {
        if !self.swallowed.contains(&key) {
            self.swallowed.push(key);
        }
        if alt {
            self.tainted = true;
        }
    }

    /// One key event.
    pub fn feed(&mut self, k: KeyInput) -> Reaction {
        if k.injected {
            return Reaction::NOTHING;
        }
        // a release: Alt's own decides, other keys follow their press
        if !k.down {
            if k.key == Key::Alt {
                let mask = std::mem::take(&mut self.tainted);
                let choose = matches!(self.state, State::Open { held: true });
                if choose {
                    self.state = State::Closed;
                }
                return Reaction { swallow: false, effect: choose.then_some(Effect::Choose), mask_alt: mask };
            }
            if let Some(p) = self.swallowed.iter().position(|s| *s == k.key) {
                self.swallowed.remove(p);
                return Reaction { swallow: true, ..Reaction::NOTHING };
            }
            return Reaction::NOTHING;
        }
        match self.state {
            State::Closed => {
                let scope = match k.key {
                    Key::Tab if k.alt => Scope::AllWindows,
                    Key::Grave if k.alt => Scope::ThisApp,
                    _ => return Reaction::NOTHING,
                };
                self.state = State::Open { held: true };
                self.swallow_down(k.key, true);
                Reaction::swallowed(Effect::Open { scope, backwards: k.shift })
            }
            State::Open { .. } => {
                let effect = match k.key {
                    Key::Tab | Key::Grave => Effect::Step { backwards: k.shift },
                    Key::Left => Effect::Move(Dir::Left),
                    Key::Right => Effect::Move(Dir::Right),
                    Key::Up => Effect::Move(Dir::Up),
                    Key::Down => Effect::Move(Dir::Down),
                    Key::Enter => Effect::Choose,
                    Key::Escape => Effect::Cancel,
                    // Alt pressed again (auto repeat) or any other key: not ours
                    Key::Alt | Key::Other => return Reaction::NOTHING,
                };
                self.swallow_down(k.key, k.alt);
                if matches!(effect, Effect::Choose | Effect::Cancel) {
                    self.state = State::Closed;
                }
                Reaction::swallowed(effect)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fed(c: &mut Chord, inputs: &[KeyInput]) -> Vec<Reaction> {
        inputs.iter().map(|i| c.feed(*i)).collect()
    }

    fn alt_tab() -> KeyInput {
        KeyInput::down(Key::Tab).with_alt()
    }

    #[test]
    fn alt_tab_opens_all_windows_and_swallows_the_tab() {
        let mut c = Chord::new();
        let r = c.feed(alt_tab());
        assert_eq!(r.effect, Some(Effect::Open { scope: Scope::AllWindows, backwards: false }));
        assert!(r.swallow);
        assert!(c.is_open());
    }

    #[test]
    fn alt_shift_tab_opens_backwards() {
        let mut c = Chord::new();
        let r = c.feed(alt_tab().with_shift());
        assert_eq!(r.effect, Some(Effect::Open { scope: Scope::AllWindows, backwards: true }));
    }

    #[test]
    fn alt_and_the_key_above_tab_opens_the_windows_of_this_app() {
        let mut c = Chord::new();
        let r = c.feed(KeyInput::down(Key::Grave).with_alt());
        assert_eq!(r.effect, Some(Effect::Open { scope: Scope::ThisApp, backwards: false }));
    }

    #[test]
    fn tab_without_alt_does_nothing_when_closed() {
        let mut c = Chord::new();
        let r = c.feed(KeyInput::down(Key::Tab));
        assert_eq!(r, Reaction::NOTHING);
        assert!(!c.is_open());
        // and keys that are nothing to it are passed on
        assert_eq!(c.feed(KeyInput::down(Key::Other).with_alt()), Reaction::NOTHING);
        assert_eq!(c.feed(KeyInput::down(Key::Enter)), Reaction::NOTHING);
    }

    #[test]
    fn tab_steps_on_while_alt_is_held_and_shift_tab_steps_back() {
        let mut c = Chord::new();
        c.feed(alt_tab());
        c.feed(KeyInput::up(Key::Tab));
        let r = fed(&mut c, &[alt_tab(), alt_tab().with_shift()]);
        assert_eq!(r[0].effect, Some(Effect::Step { backwards: false }));
        assert_eq!(r[1].effect, Some(Effect::Step { backwards: true }));
        assert!(r.iter().all(|r| r.swallow));
    }

    #[test]
    fn the_arrow_keys_move_in_the_grid() {
        let mut c = Chord::new();
        c.feed(alt_tab());
        for (key, dir) in [(Key::Left, Dir::Left), (Key::Right, Dir::Right), (Key::Up, Dir::Up), (Key::Down, Dir::Down)] {
            let r = c.feed(KeyInput::down(key).with_alt());
            assert_eq!(r.effect, Some(Effect::Move(dir)));
            assert!(r.swallow);
        }
    }

    #[test]
    fn releasing_alt_chooses_and_closes() {
        let mut c = Chord::new();
        c.feed(alt_tab());
        c.feed(KeyInput::up(Key::Tab));
        let r = c.feed(KeyInput::up(Key::Alt));
        assert_eq!(r.effect, Some(Effect::Choose));
        assert!(!r.swallow, "a modifier's release is never swallowed: the system would think Alt is still down");
        assert!(r.mask_alt, "a Tab was swallowed under Alt: the release needs the mask key");
        assert!(!c.is_open());
    }

    #[test]
    fn enter_chooses_and_escape_cancels_and_both_close() {
        let mut c = Chord::new();
        c.feed(alt_tab());
        let r = c.feed(KeyInput::down(Key::Enter).with_alt());
        assert_eq!(r.effect, Some(Effect::Choose));
        assert!(!c.is_open());
        c.feed(alt_tab());
        let r = c.feed(KeyInput::down(Key::Escape).with_alt());
        assert_eq!(r.effect, Some(Effect::Cancel));
        assert!(!c.is_open());
    }

    #[test]
    fn after_escape_the_alt_release_chooses_nothing_but_is_still_masked() {
        let mut c = Chord::new();
        c.feed(alt_tab());
        c.feed(KeyInput::down(Key::Escape).with_alt());
        let r = c.feed(KeyInput::up(Key::Alt));
        assert_eq!(r.effect, None);
        assert!(r.mask_alt, "the program in front saw Alt go down and will see it come up with nothing between");
        // and only once
        assert!(!c.feed(KeyInput::up(Key::Alt)).mask_alt);
    }

    #[test]
    fn the_release_of_a_swallowed_key_is_swallowed_and_others_are_not() {
        let mut c = Chord::new();
        c.feed(alt_tab());
        assert!(c.feed(KeyInput::up(Key::Tab)).swallow);
        assert!(!c.feed(KeyInput::up(Key::Tab)).swallow, "only once per press");
        assert!(!c.feed(KeyInput::up(Key::Other)).swallow);
    }

    #[test]
    fn a_plain_alt_press_and_release_is_left_alone() {
        let mut c = Chord::new();
        let r = c.feed(KeyInput::down(Key::Alt));
        assert_eq!(r, Reaction::NOTHING);
        let r = c.feed(KeyInput::up(Key::Alt));
        assert_eq!(r, Reaction::NOTHING, "no Tab was swallowed: the program's menu bar behaviour is its own");
    }

    #[test]
    fn injected_events_are_ignored() {
        let mut c = Chord::new();
        let mut i = alt_tab();
        i.injected = true;
        assert_eq!(c.feed(i), Reaction::NOTHING);
        assert!(!c.is_open());
        // also while open: the hook's own mask key must not be taken for a typed key
        c.feed(alt_tab());
        let mut mask = KeyInput::down(Key::Other);
        mask.injected = true;
        assert_eq!(c.feed(mask), Reaction::NOTHING);
    }

    #[test]
    fn other_keys_pass_through_while_open() {
        let mut c = Chord::new();
        c.feed(alt_tab());
        assert_eq!(c.feed(KeyInput::down(Key::Other).with_alt()), Reaction::NOTHING);
        assert!(c.is_open());
    }

    #[test]
    fn a_switcher_opened_by_a_command_works_without_alt_and_alt_means_nothing() {
        let mut c = Chord::new();
        c.open_by_command();
        assert!(c.is_open());
        let r = c.feed(KeyInput::down(Key::Tab));
        assert_eq!(r.effect, Some(Effect::Step { backwards: false }));
        let r = c.feed(KeyInput::down(Key::Tab).with_shift());
        assert_eq!(r.effect, Some(Effect::Step { backwards: true }));
        // Alt pressed and released: nothing is chosen
        c.feed(KeyInput::down(Key::Alt));
        let r = c.feed(KeyInput::up(Key::Alt));
        assert_eq!(r.effect, None);
        assert!(c.is_open());
        // Enter chooses
        let r = c.feed(KeyInput::down(Key::Enter));
        assert_eq!(r.effect, Some(Effect::Choose));
        assert!(!c.is_open());
    }

    #[test]
    fn closing_by_a_click_resets_the_chord() {
        let mut c = Chord::new();
        c.open_by_command();
        c.closed_elsewhere();
        assert!(!c.is_open());
        assert_eq!(c.feed(KeyInput::down(Key::Tab)), Reaction::NOTHING);
    }

    #[test]
    fn a_second_alt_tab_after_a_full_cycle_opens_again() {
        let mut c = Chord::new();
        for _ in 0..3 {
            let r = c.feed(alt_tab());
            assert!(matches!(r.effect, Some(Effect::Open { .. })));
            c.feed(KeyInput::up(Key::Tab));
            let r = c.feed(KeyInput::up(Key::Alt));
            assert_eq!(r.effect, Some(Effect::Choose));
        }
    }

    #[test]
    fn same_app_key_steps_like_tab_while_open() {
        let mut c = Chord::new();
        c.feed(KeyInput::down(Key::Grave).with_alt());
        let r = c.feed(KeyInput::down(Key::Grave).with_alt().with_shift());
        assert_eq!(r.effect, Some(Effect::Step { backwards: true }));
    }
}
