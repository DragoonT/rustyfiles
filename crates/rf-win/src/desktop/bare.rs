//! Our desktop windows when Explorer's desktop leaves and comes back: the Win32 half of `rf_core::desktop::cover::bare`.
//!
//! THE RULES are in the core file; the window procedure (`cover.rs`) already applies them to every move of a cover window, because
//! `zorder::target` gives the bottom of the z-order when there is no shell window. What a window procedure cannot do is hear that the
//! desktop APPEARED or VANISHED: nothing moves our window then. [`ZKeeper::sync`] is called when the host's shell watch says the shell
//! window changed, looks at the state through [`CoverZ`], and asks for the windows to be placed again when the core says so.
//!
//! TOUCHES ONLY OUR OWN WINDOWS (the same promise as `cover.rs`): the real [`SystemZ`] reads the shell window and the z-order and calls
//! `cover::reinsert_all`, which moves the cover windows and nothing else. Never run against a real Explorer coming or going in the
//! increment that wrote it; the keeper is tested on a fake z-order.

use std::sync::Mutex;

use rf_core::desktop::cover::bare::{self, Fix, State};
use rf_core::desktop::cover::zorder;

/// What the keeper needs to see and do.
pub trait CoverZ {
    /// `GetShellWindow()`: the shell's desktop window, if there is one.
    fn shell_window(&self) -> Option<isize>;
    /// The visible top-level windows, top of the z-order first.
    fn visible_order(&self) -> Vec<isize>;
    /// Our cover windows.
    fn covers(&self) -> Vec<isize>;
    /// Show Desktop is on: the covers are meant to be above the programs.
    fn raised(&self) -> bool;
    /// Asks every cover window to be placed (the window procedure puts each where the rules say).
    fn place_all_again(&mut self);
}

/// Remembers the state the windows were last placed for.
#[derive(Debug)]
pub struct ZKeeper {
    state: State,
}

impl ZKeeper {
    pub fn new(state: State) -> ZKeeper {
        ZKeeper { state }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Looks and, if the rules say so, places the windows again. Returns the state now and whether it placed them.
    pub fn sync(&mut self, z: &mut dyn CoverZ) -> (State, bool) {
        let shell = z.shell_window();
        let now = bare::state_of(shell);
        let covers = z.covers();
        // while Show Desktop is on the covers are above the programs on purpose; only a change of state overrides that
        let in_place = covers.is_empty() || z.raised() || zorder::is_in_place(&z.visible_order(), &covers, shell);
        let fix = bare::fix_after(self.state, now, in_place);
        let before = std::mem::replace(&mut self.state, now);
        if before != now {
            log::info!("cover: the shell's desktop window {} - the desktop windows go {}", if now == State::Bare { "is gone" } else { "is there" }, if now == State::Bare { "to the bottom" } else { "above it" });
        }
        let placed = fix == Fix::PlaceAgain && !covers.is_empty();
        if placed {
            z.place_all_again();
        }
        (now, placed)
    }
}

/// The real z-order, read-only, and the one call that moves (our) windows.
pub struct SystemZ;

impl CoverZ for SystemZ {
    fn shell_window(&self) -> Option<isize> {
        use windows::Win32::UI::WindowsAndMessaging::GetShellWindow;
        let h = unsafe { GetShellWindow() };
        (!h.0.is_null()).then_some(h.0 as isize)
    }

    fn visible_order(&self) -> Vec<isize> {
        super::cover::visible_order()
    }

    fn covers(&self) -> Vec<isize> {
        super::cover::all()
    }

    fn raised(&self) -> bool {
        super::cover::is_raised()
    }

    fn place_all_again(&mut self) {
        super::cover::reinsert_all();
    }
}

static KEEPER: Mutex<Option<ZKeeper>> = Mutex::new(None);

fn keeper() -> std::sync::MutexGuard<'static, Option<ZKeeper>> {
    KEEPER.lock().unwrap_or_else(|p| p.into_inner())
}

/// Cover mode starts: remembers the state the windows are about to be made for.
pub fn begin() {
    *keeper() = Some(ZKeeper::new(bare::state_of(SystemZ.shell_window())));
}

/// Cover mode stops.
pub fn end() {
    *keeper() = None;
}

/// The shell watch says the shell window changed (or Explorer announced itself): place the cover windows again if need be. Call it from
/// the message loop thread (it moves windows). False when cover mode is not on or nothing needed moving.
pub fn shell_changed() -> bool {
    let mut g = keeper();
    let Some(k) = g.as_mut() else { return false };
    k.sync(&mut SystemZ).1
}

/// The state of the shell's desktop window now (for the wallpaper's route).
pub fn state_now() -> State {
    bare::state_of(SystemZ.shell_window())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_core::desktop::cover::zorder::{HWND_BOTTOM, HWND_TOP};

    const DESKTOP: isize = 7;

    /// A z-order of a few windows, top first, and the rule the window procedure applies when a cover window is asked to move.
    struct Fake {
        order: Vec<isize>,
        covers: Vec<isize>,
        shell: Option<isize>,
        raised: bool,
        placed: u32,
    }

    impl Fake {
        fn beside() -> Fake {
            // taskbar, two programs, our two windows, Explorer's desktop
            Fake { order: vec![1, 50, 51, 11, 12, DESKTOP], covers: vec![11, 12], shell: Some(DESKTOP), raised: false, placed: 0 }
        }
        fn explorer_leaves(&mut self) {
            self.order.retain(|w| *w != DESKTOP);
            self.shell = None;
        }
        fn explorer_returns(&mut self) {
            // its desktop window appears at the top of the non-topmost band: above ours, which the bottom had left under it
            self.order.insert(1, DESKTOP);
            self.shell = Some(DESKTOP);
        }
    }

    impl CoverZ for Fake {
        fn shell_window(&self) -> Option<isize> {
            self.shell
        }
        fn visible_order(&self) -> Vec<isize> {
            self.order.clone()
        }
        fn covers(&self) -> Vec<isize> {
            self.covers.clone()
        }
        fn raised(&self) -> bool {
            self.raised
        }
        fn place_all_again(&mut self) {
            self.placed += 1;
            // the window procedure's verdict for each cover window: bottom without a shell window, directly above it with one
            for c in self.covers.clone() {
                let desktop = zorder::Desktop {
                    window: self.shell,
                    above: self.shell.and_then(|d| self.order.iter().position(|w| *w == d)).and_then(|i| i.checked_sub(1)).map(|j| zorder::Neighbour { hwnd: self.order[j], topmost: self.order[j] == 1 }),
                };
                let Some(after) = zorder::target(c, false, &desktop, false) else { continue };
                self.order.retain(|w| *w != c);
                match Some(after) {
                    Some(HWND_BOTTOM) => self.order.push(c),
                    Some(HWND_TOP) => {
                        // the top of the non-topmost windows (the taskbar, 1, is topmost)
                        self.order.insert(1, c);
                    }
                    Some(x) => {
                        let at = self.order.iter().position(|w| *w == x).unwrap() + 1;
                        self.order.insert(at, c);
                    }
                    None => unreachable!(),
                }
            }
        }
    }

    #[test]
    fn nothing_changing_moves_nothing() {
        let mut f = Fake::beside();
        let mut k = ZKeeper::new(State::Beside);
        assert_eq!(k.sync(&mut f), (State::Beside, false));
        assert_eq!(f.placed, 0);
    }

    #[test]
    fn explorer_leaving_puts_the_windows_at_the_bottom_below_every_program() {
        let mut f = Fake::beside();
        let mut k = ZKeeper::new(State::Beside);
        f.explorer_leaves();
        // before the fix the covers are still directly above where the desktop was: right under the programs; after it, at the very bottom
        assert_eq!(k.sync(&mut f), (State::Bare, true));
        assert_eq!(f.order.len(), 5);
        let mut last = f.order[3..].to_vec();
        last.sort();
        assert_eq!(last, [11, 12], "ours are the last: {:?}", f.order);
        assert!(f.order[..3].iter().all(|w| !f.covers.contains(w)), "no program is below ours: {:?}", f.order);
        // and it is stable: a second look moves nothing
        assert_eq!(k.sync(&mut f), (State::Bare, false));
        assert_eq!(f.placed, 1);
    }

    #[test]
    fn explorers_desktop_coming_back_puts_the_windows_directly_above_it_again() {
        let mut f = Fake::beside();
        let mut k = ZKeeper::new(State::Beside);
        f.explorer_leaves();
        k.sync(&mut f);
        f.explorer_returns();
        // the desktop window is above ours now: ours are invisible until they are placed again
        assert!(!zorder::is_in_place(&f.order, &f.covers, f.shell));
        assert_eq!(k.sync(&mut f), (State::Beside, true));
        assert!(zorder::is_in_place(&f.order, &f.covers, f.shell), "{:?}", f.order);
        assert_eq!(k.state(), State::Beside);
        // the desktop window is directly under our two
        let d = f.order.iter().position(|w| *w == DESKTOP).unwrap();
        let band = &f.order[d - 2..d];
        assert!(band.contains(&11) && band.contains(&12), "{:?}", f.order);
    }

    #[test]
    fn show_desktop_with_the_shell_still_there_is_left_alone() {
        let mut f = Fake::beside();
        f.raised = true;
        f.order = vec![1, 11, 12, DESKTOP];
        let mut k = ZKeeper::new(State::Beside);
        assert_eq!(k.sync(&mut f), (State::Beside, false));
    }

    #[test]
    fn without_cover_windows_nothing_is_placed_but_the_state_is_followed() {
        let mut f = Fake::beside();
        f.covers.clear();
        let mut k = ZKeeper::new(State::Beside);
        f.explorer_leaves();
        assert_eq!(k.sync(&mut f), (State::Bare, false));
        assert_eq!(k.state(), State::Bare);
    }

    #[test]
    fn the_real_state_can_be_read_and_the_real_keeper_is_idle_without_cover_mode() {
        let _ = state_now();
        end();
        assert!(!shell_changed(), "no cover mode: nothing to place");
    }
}
